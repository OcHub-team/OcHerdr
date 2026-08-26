# Pane 跨 Tab 拖拽：新建与合并交互方案

## 1. 目标

用户拖动 pane 标题栏的抓手到 tab 栏后，可以：

- 拖到“新 tab 投放区”，将 pane 拆成当前 workspace 内的新 tab；
- 拖到已有 tab pill，把 pane 合并到该 tab。

实现分两阶段推进，但最终版本必须同时支持两种目标。tab 栏投放允许从单 pane tab 发起；如果被移动的是源 tab 最后一个 pane，Herdr 在 move 事务内关闭空 tab，OcHerdr 消费权威 `tab.closed`，不得再额外发送 `tab.close`。

## 2. 可行性结论

可行，且不需要修改 Herdr 协议。OcHerdr 已识别 `pane.move` capability，Herdr 已支持新 tab 目标：

```json
{
  "pane_id": "<source-pane>",
  "destination": {
    "type": "new_tab",
    "workspace_id": "<current-workspace>"
  },
  "focus": true
}
```

以及已有 tab 目标：

```json
{
  "pane_id": "<source-pane>",
  "destination": {
    "type": "tab",
    "tab_id": "<target-tab>",
    "target_pane_id": "<target-tab-focused-pane>",
    "split": "right",
    "ratio": 0.5
  },
  "focus": true
}
```

OcHerdr 的增量事件模型已经能处理 `tab.created`、`tab.closed`、`pane.moved` 和源/目标 `layout.updated`。主要工作是增加跨 terminal/tab-bar 的拖拽意图、主题化反馈和一阶段提交状态，而不是扩展后端。

## 3. 用户交互

### 3.1 进入拖拽

- 连接支持 `pane.move`、布局未 zoom 且 tab 没有 relocation/template commit 时开放 tab 栏拖拽；源 tab 可以只有一个 pane。
- pane-local swap/edge/template 仍沿用各自原有的 pane 数量约束。单 pane tab 开始拖拽后，只能命中 tab 栏目标。
- 沿用现有 pane 标题栏抓手和拖拽阈值；没有越过阈值时仍是普通选中。
- 拖拽浮层、源 pane 降低存在感和 Esc 取消沿用现有行为。

### 3.2 投放区

- `+` 按钮是主要目标。
- `+` 右侧、pane action toolbar 左侧的 tab 栏空白区也是同一个目标，以提供足够大的命中面积。
- 每个非源 tab pill 都是“合并到该 tab”的目标；源 tab pill 不可投放，避免无意义请求。
- 合并的确定性默认位置是目标 tab 当前 focused pane 的右侧、比例 1:1。若 snapshot 没有 focused pane，则使用目标 layout 的第一个 pane。目标 tab zoom、无 pane、结构锁定或缺少 layout 时不可投放。
- 本版不在 hover 时偷偷切换 tab，也不要求用户在隐藏 target layout 上选择精确位置。以后可增加“悬停打开目标 tab 后继续选择 pane-local 落位”的增强模式。
- 命中使用实际绘制元素自己的语义事件区域，禁止依赖终端 surface 反算，避免浮层命中坐标漂移问题重现。

### 3.3 命中反馈

- 未命中：tab 栏保持原样，浮动 pane 继续跟随指针。
- 命中新 tab：`+` 与空白投放区形成一个主题色高亮目标，显示 `+` 图标和“松开以新建标签页”。
- 命中已有 tab：对应 pill 使用主题色描边/填充，并显示“松开以移入 {tab_name}”及合并图标；其他 tab 不高亮。
- 使用现有 theme token：accent 边框、柔和 accent 背景和可读的 accent text；不可硬编码颜色。
- 鼠标离开投放区时立即取消高亮；再次进入立即恢复。
- 命中 tab 栏期间，terminal 内的布局模板与 pane-local drop target 必须清空，tab target intent 优先级最高。

### 3.4 松手、取消与失败

- 在投放区松手：发送一次 `pane.move`，`destination.type = new_tab`、workspace 为源 workspace、`focus = true`。
- 在已有 tab pill 松手：发送一次 `pane.move`，目标是该 tab 的 focused/first pane，`split = right`、`ratio = 0.5`、`focus = true`。
- 成功：Herdr 的权威事件创建并选中新 tab；新 tab 中只有被拖 pane；源 tab 的其余 pane 自动填满折叠后的布局。
- 合并成功：Herdr 聚焦目标 tab，被拖 pane 出现在默认锚点右侧；源 tab 和目标 tab 都以最新权威布局 settle。
- 如果源 tab 因最后一个 pane 被移走而变空，只接受 Herdr 自动产生的 `tab.closed`；不得补发 `tab.close`，不得短暂复活空 tab。
- Esc、窗口失焦或投放区外松手：不发送请求，执行现有返回动画。
- 请求失败或返回 `changed = false`：解除锁定，恢复源 tab 权威布局并显示现有错误通知；不得留下假 tab 或隐藏 pane。
- 重复 mouse-up、迟到响应和陈旧 operation id 均不得二次提交。

## 4. 状态与架构

### 4.1 拖拽状态

给 `PaneDrag` 增加明确的 new-tab hover/intent 状态，不能用一个易混淆的 bool 表示。建议：

```rust
enum PaneTabDropTarget {
    NewTab,
    Existing {
        tab_id: String,
        target_pane_id: String,
    },
}
```

或等价的小型结构。它必须进入 `PartialEq`，便于纯状态测试。

tab-bar 的实际元素通过专用 controller 方法更新这个语义目标。普通 terminal `pane_mouse_move` 在指针回到 terminal 后会清除此目标并恢复现有 template/pane hit testing。

### 4.2 本地预览

- new-tab hover 时，源 tab 应预览“移除被拖 pane 后父 split 折叠”的最终布局；不要回放中间事件。
- 复用 `ocherdr-core::relocation` 的纯布局树算法，提取/新增一个有单元测试的“预测移除 pane”入口，不在 UI 层复制 split-tree 算法。
- 被拖 pane 继续作为 floating preview；其余 pane 实时扩张到预测位置。
- 离开目标时从预测布局平滑回到之前的拖拽布局或权威布局，尊重 reduce-motion。

### 4.3 提交状态

两种目标都是单请求事务，不应伪装成现有三步 insert orchestration，也不能把真正的新 tab 当成需要隐藏的临时 parking tab。

建议增加独立的 pending detach/new-tab commit，至少保存：

- operation id；
- source workspace/tab/pane id；
- destination（new tab 或已有 tab/anchor pane）；
- release-time layout fingerprint；
- 源 tab 移除 pane 后的 predicted rects/topology；
- 已有目标 tab 的 release-time fingerprint 和插入后的 predicted rects/topology；
- 已知 tab ids，用于识别本次创建的目标 tab；
- response/event 收敛状态。

源 tab 和已有目标 tab 在提交期间锁定结构操作。渲染只展示最新预测布局；权威事件达到预期状态后直接 settle，不回放 `tab.created/tab.closed → pane.moved → layout.updated` 的中间帧。

### 4.4 生命周期约束

- terminal runtime 不能因 pane 临时变更 tab id 而白屏；沿用 optimistic visibility 和及时 resize 策略。
- `tab.created` 可能先于请求 response，response 也可能先到；两种顺序都必须收敛。
- `tab.closed`、`pane.moved`、两个 layout event 与 response 的顺序不可假定；最后一个 pane 离开源 tab 的场景必须独立覆盖。
- 断线、外部结构变更或 fingerprint 不一致时取消本地预测，回到 snapshot。
- 不增加超过 1500 行的上帝文件；UI、纯模型和 controller transaction 分开。

## 5. 无障碍和一致性

- 新 tab 投放区提供稳定的 role/aria label：“将窗格移到新标签页”；已有 tab 目标提供“将窗格移到 {tab_name}”。
- 命中状态不能只靠颜色：同时出现 `+` 图标、文案和边框变化。
- 不抢走用户键盘焦点；拖动完成后由 Herdr `focus: true` 决定新 tab/pane 焦点。
- 不改变 tab reorder 手势：只有 `SurfaceDrag::Pane` 时投放区接管鼠标移动和松手。

## 6. 测试与验收标准

必须新增以下测试：

1. 纯模型：2、3、4 pane 的移除预测会正确折叠父 split，pane 集合和面积不丢失。
2. GPUI 事件：真实拖拽进入 `+`/空白投放区后，new-tab hover 出现，主题目标可通过 debug selector 定位。
3. GPUI 请求：松手只发送一次 `pane.move`；分别验证 `new_tab + workspace_id + focus:true` 和 `tab + target pane + right + 0.5 + focus:true`。
4. 离开目标、Esc、投放区外松手均不发送请求。
5. 单 pane tab 可以命中 tab 栏目标，但不能进入 pane-local drop；zoom、无 `pane.move` capability 和已锁定 tab 不开放目标。
6. 事件/响应多种先后顺序都能收敛；失败、断线与陈旧 response 不残留锁或 optimistic layout。
7. 最后一个 pane 移出时只发送 `pane.move`，随后 `tab.closed` 会移除源 tab；测试明确断言没有 `tab.close`。
8. 回归：pane-local swap、edge relocation、layout template、tab reorder、tab create 点击均保持可用。

完成标准：

- `cargo fmt --all -- --check`
- `cargo test --workspace --locked -- --test-threads=1`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `git diff --check`
- `just qa-app`
- 所有被修改的 Rust 文件不超过 1500 行；若接近阈值，应先拆分模块。

## 7. 非目标

- 跨 workspace 拖拽；
- 从 OcHerdr 窗口拖成新的 macOS 原生窗口；
- 修改 Herdr wire protocol；
- 主动发送 `tab.close` 关闭已被 Herdr 自动清理的源 tab；
- 自动重命名或重新排序新 tab；
- hover 已有 tab 后自动打开并选择精确 pane-local 落位（后续增强）。
