# 窗格拖拽、交换与重排设计

**状态：v2，已按 Herdr 源码核对；阶段 1–3 与拖动中实时草稿布局已实现（75ba207 / b562db0 / 142536e / 59ab9f9）**
**适用范围：OcHerdr macOS 客户端；Herdr 保持为未修改的公开发行版（≥ 0.7.0，protocol 14）**

## 1. 摘要

OcHerdr 提供两类直接操作：

1. **窗格交换**：从窗格标题栏的拖拽把手抓起一个 pane，拖到同一 tab 内另一个 pane 的中心，交换两个 pane 所处的布局叶节点。
2. **窗格重排与缩放**：拖动分隔边连续改变相邻窗格比例；将 pane 拖到另一 pane 的左、右、上、下落点，把它重插入目标旁边形成新的分屏。

Herdr 是会话、PTY、进程和布局的唯一权威。OcHerdr 不维护私有布局协议，不要求用户安装定制 Herdr。中心交换直接使用 `pane.swap`；同 tab 四边重排使用公开 API 的两步编排（先移到临时 tab，再移回原 tab 的目标边），左/上再补一次 `pane.swap` 纠正顺序。

v2 与 v1 的差别：v1 假定 OcHerdr 客户端已经具备"增量消费事件、读取响应、正确预测布局"的能力，经源码核对这三点都不成立，因此 v2 把它们列为 **P0 前置工作**（§6），并把 Herdr 侧确认过的副作用写成**已知代价**（§9）。

## 2. 目标与非目标

### 2.1 目标

- 在不重启 pane 内进程的前提下交换、重排正在运行的终端。
- 通过拖动分隔边连续调整 pane 的相对大小。
- 拖动时提供清晰的空间反馈：被抓起的窗格、原位置空槽、有效落点、最终布局预览、落位动画。
- 把后端的两步移动隐藏为一次连贯操作；本地或 SSH 连接上都能安全恢复。
- 不干扰终端正文的鼠标选择、鼠标报告或滚动。
- 尊重"减少动态效果"，提供键盘与屏幕阅读器等价操作。

### 2.2 非目标

- 不把平铺 pane 变成可重叠、可悬浮的窗口。
- 不在拖动的每个鼠标事件发送 resize，不连续重排终端字符网格。
- 不修改 Herdr 私有协议、内存状态或磁盘会话文件；不要求用户安装非官方 Herdr。
- 首版不支持把 pane 拖到其他客户端窗口，不提供跨会话迁移。
- 不承诺对**其他客户端**隐藏临时 tab（见 §9）。

## 3. Herdr 侧约束（已核对源码）

核对基线：herdr `b816f3ea`（2026-08-25）。以下每条都对应实现位置，升级 Herdr 时按此清单复核。

| 约束 | Herdr 实现位置 |
| --- | --- |
| `pane.swap(source_pane_id, target_pane_id)` 交换同 tab 任意两叶子的 id，不动 split 形状与比例；跨 tab 返回 `cross_tab`；成功后切到该 tab 并把焦点给 source | `src/layout.rs::swap_panes`，`src/app/api/panes.rs` `handle_pane_swap` |
| `pane.move` 同 tab 单次移动返回 `changed:false, reason:"same_tab"` | `src/app/api/panes.rs` `PaneMoveReason::SameTab` |
| `pane.move` 的 `split` 只有 `right`/`down`；移入 pane 成为目标 pane 的**第二子节点**（右/下） | `src/layout.rs::split_at` |
| `ratio` 缺省 0.5，钳制到 `0.1..=0.9`，非有限值回落 0.5 | `src/layout.rs::valid_split_ratio` |
| `layout.set_split_ratio` 同样钳制 `0.1..=0.9` | `src/layout.rs::set_ratio_at` |
| 移出 tab 的最后一个 pane 会删除该 tab 并广播 `tab.closed` | `src/workspace.rs::take_pane_for_move` |
| `focus:false` 时不切换 active tab（`if focus \|\| active.is_none()`） | `src/app/api/panes.rs` `handle_pane_move` |
| 同 workspace 内移动，pane 公共 id 不变（别名只在跨 workspace 时建立） | 同上，`cross_workspace` 分支 |
| `pane.move` 响应携带 `created_tab`、`pane.tab_id`、`source_layout`、`target_layout`、`closed_tab_id` | `src/api/schema/panes.rs::PaneMoveResult` |
| 一次 `pane.move` 依序广播 `tab.closed?`、`tab.created?`、`pane.moved`、`layout.updated`(source)、`layout.updated`(target) | `handle_pane_move` 末尾 |
| 目标 pane 必须在目标 tab 内，否则 `target_pane_not_found` | 同上 |
| zoomed 源或目标 tab 返回 `reason:"zoomed_tab"` | 同上 |
| headless 会随布局 resize **后台 tab** 的终端，但处于 `direct_attach_resize_locks` 的终端被跳过 | `src/ui/panes.rs`，`src/server/headless.rs` 直连 attach 路径 |
| 从 split 中取走一个 pane 时，父 split **塌缩**，兄弟子树占据父区域；同时源 tab `zoomed=false`，`root_pane` 可能被重新提升 | `src/workspace/tab.rs::take_pane_for_move`，`src/layout.rs::remove_pane` |
| 每个新建 tab 永久消耗一个公共 tab 编号 | `src/workspace.rs::create_tab_from_existing_pane` |
| 第二步目标 tab 消失时，Herdr 自行把 pane 放回一个新 tab（不会丢 pane） | `src/app/api/panes.rs::recover_failed_pane_move` |

`layout.apply` 会创建新 tab/pane 并关闭旧 tab，不能保留进程，不可用作重排替代。

## 4. 后端编排

### 4.1 中心交换

```text
pane.swap { source_pane_id, target_pane_id }
```

一次请求，响应里的 `layout` 与随后的 `layout.updated` 一致。

### 4.2 四边重排

```text
源 pane（原 tab T，目标 pane P，落点 edge，比例 r）
  1. pane.move { pane_id: S, destination: { type: new_tab, workspace_id: W }, focus: false }
       → 读响应 created_tab.tab_id = T_tmp
  2. pane.move { pane_id: S, destination: { type: tab, tab_id: T, target_pane_id: P,
                 split: right|down, ratio: r' }, focus: true }
       → T_tmp 因失去最后一个 pane 自动删除
  3. edge ∈ {left, up} 时：pane.swap { source_pane_id: S, target_pane_id: P }
```

- 右/下：`split` 取 `right`/`down`，`r' = r`。
- 左/上：先以 `right`/`down` 插入得到 `split(P, S, r')`，再 swap 得到 `split(S, P, r')`。第一子节点占 `r'`，所以 **`r' = 1 - r`**（首版 `r = 0.5`，二者相同，但代码必须按公式写）。
- 三条请求严格串行：第 2 条必须在第 1 条**响应**到达后发出（否则 Herdr 视为同 tab 移动），第 3 条必须在第 2 条响应后发出。中间不插入 UI 刷新、快照 resync 或人为延时。
- 第 2 条使用第 1 条响应里的 `pane.pane_id`（同 workspace 内不变，但以响应为准）。

### 4.3 分隔线缩放

保持现状：拖动期间只做本地预览，松手时发一次 `layout.set_split_ratio { tab_id, path, ratio }`。

### 4.4 布局模板重建

Pane 拖拽超过阈值后，终端画布顶部中央显示与当前 pane 数量匹配的 2/3/4-pane 常用布局。缩略图的每个叶子都是落点；悬停时直接用目标二叉树预测整个 tab，松手前不发送请求。

Herdr 没有保留进程的原子 `layout.apply`，因此提交按目标树的逆向叶子裁剪计划串行执行：保留一个锚点 pane，将其余 pane 分别以 `focus:false` 暂存到临时 tab，再按树的构造顺序以 `pane.move` 的 `right` / `down` split 插回原 tab。构造算法只裁剪父节点的第二叶子，因此无需额外 `pane.swap`，并能精确还原目标方向和比例。整个批次保持预测布局、隐藏临时 tab、冻结终端网格；最后一个响应和匹配的 `layout.updated` 到达后才释放。

## 5. 交互规范

### 5.1 抓取区域

每个 pane 标题栏左侧新增 20×24 px 的 `DragHandle`，在 hover、键盘聚焦或 pane 被选中时显现；默认 `grab` 光标，按下后 `grabbing`。

- 单击标题栏其余区域：选中 pane。
- 把手按下并移动 ≤ 6 px：按普通点击处理。
- 超过 6 px：进入 pane 拖拽，终端正文不会收到选择手势。
- `Esc`、切换 tab/workspace、断开连接、目标布局变化：取消当前拖拽或事务。

### 5.2 拖拽中的视觉状态

- **拖拽预览**：pane 标题、状态和最近一帧 `RenderedFrame`；相对鼠标保持抓取偏移，透明度 0.92、缩放 1.015、低对比度阴影。只复用当前帧，不截图、不复制 IOSurface。
- **实时草稿布局**：进入有效落点后立即运行与提交时相同的 `predict_swap` / `predict_relocation`；其他 pane 壳层挤压到预测矩形，源 pane 的壳层成为目标位置的半透明虚线占位槽。浮动预览继续跟随指针，不连续 resize 终端网格。
- **无落点状态**：尚未进入有效落点时，源 pane 仍在原位显示透明度 0.22 的弱虚线空槽；离开有效落点后，所有壳层一起回到权威布局。
- **目标反馈**：有效目标显示强调色描边与半透明覆盖；中心落点标签"交换"，四边落点标签"移至左侧/右侧/上方/下方"。
- **无效区域**：预览透明度 0.55；松开回原位，不发请求。

命中几何与显示几何分离：五区判定始终使用拖动开始时的权威布局，草稿布局只用于渲染；因此 pane 被挤压后不会反过来改变鼠标下的落点。切换落点时壳层以 140 ms `ease_out_quint` 过渡；同一落点内的连续 mouse move 不重启动画。拖动期间隐藏权威 split handle，避免其与尚未提交的草稿树拓扑错位。开启"减少动态效果"时直接显示草稿终态，仍保留占位槽与落点文字。

### 5.3 落点模型

目标 pane 可见矩形划分五区：中心 44% × 44% 为交换区，其余为四个方向区。

- 中心：`pane.swap`。
- 四边：§4.2 编排。
- 初始比例 0.5；后续可提供 1/3、1/2、2/3 预设（左/上按 §4.2 取 `1 - r`）。首版不按鼠标在边缘区的精细位置改变比例。
- 源 pane 与目标 pane 必须属于同一可见 tab。zoomed tab、缺少布局快照、事务进行中的 tab、以及**能力探测未通过的连接**（§8）不显示四边落点。
- 源 pane 是 tab 内唯一 pane 时没有任何落点。

### 5.4 分隔线缩放

split handle 热区保持 10 px。hover 时分隔线由 4 px 中性线过渡到强调色；按下后：预览线跟随指针；该 split 子树内 pane 的外框与遮罩按预览比例挤压/扩展；终端表面不连续 resize；松开只发一次 `layout.set_split_ratio`。

"挤压"是空间预览：边框、背景和可用区域连续变化，终端最后一帧在裁剪区域内保持稳定；收到权威布局后 Ghostty surface 一次性以最终尺寸重绘。

冻结结束后的第一帧必须主动用缓存的最终 body bounds 重新排队一次 terminal resize，并立即 `refresh` / `try_frame`。预测终态与权威终态通常拥有完全相同的矩形，GPUI 此时不会再次触发 canvas measure；若只解除冻结而不主动刷新，surface 会保持空白直到下一次点击或输入。canvas 测量只记录最新尺寸，连续的启动帧、动画帧和过期回调会在 120 ms 稳定窗口内合并；稳定后才真正 resize Ghostty 或替换 bootstrap observer，避免同一个 pane 因瞬态尺寸反复白屏、reflow 和重连。

实现注记：Herdr 的 `split_rect` 给第一子节点 `round(size × ratio)` 个整格，所以预览按整格步进而不是连续像素跟随（否则松手时会出现最多半格的跳动，真机验收测得 12 px）。预览与最终布局使用同一套 `split_rect` + chrome 管线。

## 6. P0 前置工作（客户端）

这三项不做，§4.2 的"一次连贯操作"无法兑现。它们独立于拖拽 UI，先行落地并单独测试。

### 6.1 `pane.moved` 改为增量应用

现状：`ocherdr-core/src/event.rs` 把 `HerdrEvent::PaneMoved` 映射为 `SnapshotUpdate::Resync`，每次 `pane.move` 触发一次 `session.snapshot` 全量拉取、快照整体替换、`reconcile_split_drag`/`reconcile_reorder_drag`，并可能按 `session_terminals_need_rebuild` 重建终端流。两步编排 = 两次全量 resync，临时 tab 会进入快照并被渲染。

要求：

- `PaneMoved` 事件携带 `previous_pane_id / previous_workspace_id / previous_tab_id / pane / created_workspace / created_tab / closed_workspace_id / closed_tab_id`，信息足够本地增量应用：更新 pane 记录的 workspace/tab、按 `created_*` 增加记录、按 `closed_*` 删除记录、必要时删除失效 layout。
- 布局本身由紧随其后的 `layout.updated` 更新，`PaneMoved` 不自行推导布局。
- 只有事件字段与本地快照矛盾（例如 pane 不存在）时才回落 `Resync`。
- 为事件序列 `tab.created → pane.moved → layout.updated ×2` 和 `pane.moved(closed_tab) → layout.updated` 各写一条快照测试，断言不触发 resync 且终态与 `session.snapshot` 一致。

### 6.2 可读取响应的串行调用

现状：`controller.rs::spawn_invoke` 对 `request_socket` 的返回值执行 `.map(|_| ())`，响应被丢弃；每个请求是独立 socket 连接。

要求：

- 新增 `invoke_with_response`（命名可调）：后台执行请求，成功时把 `Value` 交回主线程回调；失败沿用 `notify_command_failure`。
- 事务用它把三条请求串起来：上一条回调内立即发下一条；不等待 UI 刷新、事件或快照。
- `command_needs_snapshot_resync` 对 `pane.move`/`pane.swap` 保持 false（它们都发事件）。

### 6.3 预测布局几何

现状：v1 假设"用目标矩形和落点方向切分"即可，但 Herdr 取走源 pane 时会先塌缩父 split。

要求（放在 `ocherdr-core`，无副作用，纯几何 + 单元测试）：

- 从 `PaneLayoutSnapshot`（`panes` + `splits` 的矩形和 ratio）重建二叉树；无法重建时四边落点不可用。
- `predict_relocation(layout, source, target, edge, ratio)`：**移除源并塌缩 → 在目标处 `split_at` → 左/上时交换叶子** → 输出各 pane 的预测矩形。
- `predict_swap(layout, a, b)`：只交换两叶子的矩形。
- `drop_zone(target_rect, point)`：五区判定，中心 44% × 44%。
- `layout_fingerprint(layout)`：pane id 集合 + split 路径/方向 + 目标 pane 矩形的哈希，用于事务失效检测。
- 测试覆盖：水平/垂直/嵌套 split、源与目标为相邻兄弟、源在深层子树、ratio 边界、极小 pane。

## 7. 本地乐观事务

### 7.1 `RelocationPlan`

用户松手后立即生成不可变计划：

```text
RelocationPlan
  operation_id
  source_pane_id, source_tab_id
  target_pane_id, target_tab_id
  intent: Swap | Insert { edge, ratio }
  layout_fingerprint            // 松手时刻的目标 tab 指纹
  predicted_final_rects         // §6.3 输出
  visual_snapshot               // 源 pane 最近一帧
```

`predicted_final_rects` 只用于渲染与动效，绝不替代 Herdr 的布局。

### 7.2 状态机

```text
Idle
 └─ 按下把手 / 越过阈值 → Dragging
Dragging
 ├─ 松开中心落点 → Swapping
 ├─ 松开四边落点 → Parking
 └─ Esc、无效放置、布局失效 → Cancelled → Idle
Swapping
 └─ pane.swap 响应 + 匹配的 layout.updated → Settling → Idle
Parking
 ├─ 第 1 条 pane.move 响应成功（记录 T_tmp）→ Inserting
 └─ 失败 → Failed(NotStarted) → Idle
Inserting
 ├─ 右/下：第 2 条响应成功 + layout.updated → Settling → Idle
 ├─ 左/上：第 2 条响应成功 → CorrectingOrder
 └─ 失败 → Parked(T_tmp)
CorrectingOrder
 ├─ pane.swap 响应成功 + layout.updated → Settling → Idle
 └─ 失败 → Misordered（pane 已回原 tab，只是在右/下而非左/上）→ Idle 并提示
Parked(T_tmp)
 ├─ 用户"重试放置" → Inserting
 └─ 用户"返回原 tab"/超时 → 清除预测，切到 T_tmp，展示权威快照
```

`Parking`/`Inserting`/`CorrectingOrder` 期间：

- 界面继续渲染 `RelocationPlan` 的预测布局；
- 事件流按 §6.1 增量更新真实快照，但渲染层对该 tab 使用预测矩形，且**隐藏 `created_tab == T_tmp` 的 tab 条目**；
- 事务期间冻结该 tab 内所有 pane 的 `resize_pixels`（终端网格尺寸只在 Settling 时改变一次）；
- 该 tab 的 split handle、关闭操作、再次拖拽全部禁用；其他 tab 不受影响。

### 7.3 失效与恢复

- 一个 tab 同时只允许一个重排/交换/resize 事务。
- 松手时与每条响应到达时都比对指纹（§6.3）；`layout.updated` 的 pane 集合、split 路径或目标 pane 与计划不符则终止计划、展示权威快照。
- 第 1 条失败：撤销预测，不改变选择。
- 第 2 条失败：pane 停在 `T_tmp`。清除预测，在原 tab 位置显示一个内联提示（"窗格已暂存到临时标签页"），提供"重试放置"与"前往该标签页"。不得假装完成或静默丢失 pane。
- 第 3 条失败：布局已经合法（只是方向相反），不回滚，提示一次即可。
- 断线：中止本地事务，重连后以 `session.snapshot` 为准；若 `T_tmp` 仍存在，按 Parked 处理。
- `Esc` 只在 `Dragging` 有效；请求已发出后不可取消。

## 8. 能力探测与降级

- OcHerdr 目前没有任何 Herdr 版本探测。新增：连接时从 `session.snapshot` 的版本/protocol 元数据判断 `pane.move` 可用（Herdr ≥ 0.7.0 / protocol ≥ 14），结果缓存在连接状态里。
- 探测未通过：只提供中心交换和分隔线缩放，四边落点不出现。
- 探测通过但 `pane.move` 返回未知方法：同样降级，并记一次日志。
- 后续若 Herdr 上游提供同 tab 原子 `pane.move`（含 `left`/`up`），以同一探测机制切换为单调用；两步编排作为旧版本降级路径保留。

## 9. 已知代价（Herdr 不可改，必须接受）

- **其他客户端会看到临时 tab**：`tab.created`/`tab.closed` 是真实广播，同时 attach 的 Herdr TUI、其他 OcHerdr 窗口、监听事件的 agent 都会看到一个 tab 出现又消失。OcHerdr 只能对自己隐藏。
- **tab 编号跳号**：每次四边重排永久消耗一个公共 tab 编号。
- **源 tab 的 zoom 被清除**：取走 pane 时 `zoomed=false`。四边落点已在 zoomed tab 上禁用，此处只影响并发的外部 zoom。
- **源 tab 的 `root_pane` 可能改变**：拖走当时的 root pane 时 Herdr 会提升另一个 pane。对 OcHerdr 无可见影响，记录备查。
- **PTY 尺寸与 control**：Herdr 只为 attach（control）流调整 PTY，observe 流的 resize 只记录该 observer 的视口。Pane 初始 observe；点击、滚轮或输入只把交互目标提升为 control，且不释放本机已控制的其他可见 pane。切到隐藏 tab 或被另一客户端 takeover 时，仅对应 pane 降回 observe；再次交互才重新取得 control。键盘输入与焦点仍只绑定选中 pane，滚轮绑定鼠标所在 pane。
- **非原子**：三条请求之间其他客户端/agent 可以修改布局，靠指纹检测而不是靠锁。

## 10. 动效规范

| 时机 | 效果 | 时长 |
| --- | --- | --- |
| 抓起 | 透明度提升、缩放 1.015、阴影出现 | 120 ms |
| 落点切换 | 目标描边和标签淡入淡出 | 100 ms |
| 松手到后端提交 | 预览停在本地最终位置 | 0 ms |
| 最终落位 | 壳层和边框从预测矩形校正到权威矩形 | 120–180 ms |
| 无效放置/取消 | 预览回到源矩形 | 120 ms |

统一 `ease_out_quint`；禁止 bounce、elastic、持续脉冲。`减少动态效果`开启时所有位移/缩放/回退直接落到终态，保留落点高亮与文字状态。

## 11. 可访问性与键盘等价操作

- 拖拽把手名称："拖动窗格：{pane name}"。
- 拖拽时用可访问文本描述当前意图（交换 / 移至某侧）。
- `Esc` 取消；键盘拖拽模式下方向键选落点、`Enter` 确认。
- pane 上下文菜单提供"与左/右/上/下窗格交换"，保留 Herdr 的方向交换快捷键。
- 动效关闭时不依赖动画传达结果。

## 12. 实现边界与文件规划

| 文件 | 修改 |
| --- | --- |
| `crates/ocherdr-core/src/event.rs` | `PaneMoved` 增量应用（§6.1）及测试 |
| `crates/ocherdr-core/src/lib.rs`（或新 `relocation.rs`） | 树重建、`predict_relocation`、`predict_swap`、`drop_zone`、`layout_fingerprint`（§6.3）及测试 |
| `crates/ocherdr-app/src/controller.rs` | `invoke_with_response`（§6.2）；能力探测（§8）；抓取、命中测试、状态机、串行编排、响应校准、取消与恢复 |
| `crates/ocherdr-app/src/main.rs` | `SurfaceDrag::Pane`、`PaneDrag`、`RelocationPlan`、`PendingPaneRelocation`、动效常量 |
| `crates/ocherdr-app/src/ui/hierarchy.rs` | 标题栏把手、拖拽预览、空槽、五区落点、预测布局渲染、临时 tab 隐藏、分隔线挤压预览、落位动画 |
| `crates/ocherdr-app/src/a11y.rs`、`i18n.rs` | 可访问名称、状态文本、中英文文案 |
| `crates/ocherdr-herdr` | 不改协议定义；仅在需要时增加响应类型的反序列化结构 |

## 13. 交付阶段

1. **P0**：§6.1、§6.2、§6.3、§8。纯逻辑，单元测试覆盖，不改 UI。
2. **中心交换 + 分隔线挤压预览**：`SurfaceDrag::Pane`、把手、五区判定（仅中心可落）、`pane.swap`、Settling 动画、a11y/i18n。
3. **四边重排（实验开关）**：§4.2 编排、§7 状态机、临时 tab 隐藏、Parked 恢复 UI；收集本地/SSH 延迟与失败率。已实现：配置键 `pane-edge-relocation = true|false`（默认 false），需同时通过 §8 能力探测；键盘模式（前缀键后 `m`，方向键选目标，Tab 循环落点，Enter 确认）随同落地；比例预设（1/3、2/3）尚未提供，固定 0.5。
4. **默认开启**：四边重排稳定后默认开启；不支持 `pane.move` 的连接继续走阶段 2 的能力。

## 14. 测试与验收

### 14.1 单元测试

- §6.1 两组事件序列不触发 resync，终态与快照一致。
- §6.3 全部几何用例；`1 - r` 公式。
- 6 px 阈值、抓取偏移、五区判定。
- 状态机所有正常、取消、外部变更、失败分支（含 Parked、Misordered）。

### 14.2 集成测试（可用 fake socket）

- 中心落点只调用一次 `pane.swap`。
- 右/下落点严格按"new_tab → tab"顺序两次 `pane.move`，第 2 条使用第 1 条响应的 tab/pane id。
- 左/上落点再补一次 `pane.swap`，比例为 `1 - r`。
- 后续请求在前一条响应后立即发出，不等待快照。
- 任一步失败回到正确的可见状态；pane 停在临时 tab 时出现恢复入口。
- 事件乱序、远程延迟、目标 pane 被关闭、连接断开。
- 减少动态效果下无连续动画帧，但布局与焦点结果一致。

### 14.3 手动验收

- 两窗格左右/上下交换、三窗格嵌套布局交换。
- 拖到四边形成正确分屏，pane 内长时间运行的命令不中断。
- 本地与 SSH 会话各一次，确认临时 tab 不在**当前客户端**闪现。
- 拖动分隔线时终端不在每个鼠标事件 reflow，松手后尺寸正确。
- 拖拽过程中全屏 TUI、鼠标报告应用、文本选择的输入语义未被破坏。
