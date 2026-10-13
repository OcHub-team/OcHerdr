# Remote 直连与界面简化设计

**状态：v1，决策已确认，实现中**
**适用范围：OcHerdr macOS 客户端；要求 Herdr ≥ 0.9（private protocol ≥ 22，endpoint generation 1）**

## 1. 摘要

本设计做两件事，共同服务于"界面简单明了、易于操控"：

1. **Remote 直连**：数据传输层对齐原版 herdr 的 remote 模型——一条连接承载一切。本地走 Unix socket，远程走 `ssh -T <dest> 'exec herdr remote-client-bridge'` 的 stdio 字节直通，协议为 `endpoint.hello.v1`（按名协商、跨版本稳定）。删除 0.8.x legacy 兼容（private v20 编解码、双 StreamLocal 隧道、NDJSON api.sock 数据面）。
2. **界面收敛**：导航收敛为"一个侧栏"（聚合视图从模式降级为侧栏机器分组，删除 Single/Aggregate 切换与 Host Switcher 弹层），控制面收敛为"一个 socket"（本机代理 socket 让本地 skill 以原生协议直达任意已连接 host），外加工具栏/设置/菜单/确认框瘦身。

## 2. 决策记录（用户确认）

| # | 决策点 | 结论 |
| --- | --- | --- |
| 1 | 老服务器（herdr 0.8.x，protocol 20/21）兼容 | **删除**。endpoint 协商失败即报错提示升级，不回退 |
| 2 | 远端 herdr 二进制 | **直接依赖**：远端必须已安装 herdr ≥ 0.9（沿用现有 PATH/常见路径发现），不做自动上传/安装 |
| 3 | 聚合视图命运 | 从"侧栏模式"降级为"侧栏顶部的机器分组"，跨机一览能力保留，不再存在第二套导航逻辑 |

## 3. 背景与现状诊断

### 3.1 原版 herdr 的 remote 模型（已对源码核对）

核对基线：`../herdr` 工作树（`src/remote/attach.rs`、`src/remote/host_unix.rs`、`src/main.rs`、`src/protocol/wire.rs`）。

- 原版 `--remote` 的**传输层就是 SSH**，不存在绕过 SSH 的直连通道。它与"SSH 串起来"的区别是**协议直通**：本地 thin client ⇄ 本地桥 socket ⇄ `ssh -T <dest> 'exec herdr remote-client-bridge'` stdio ⇄ 远端 client socket，中间逐字节透传、不解帧（`remote-client-bridge` 为隐藏子命令，`src/main.rs:575`；远端桥实现 `src/remote/host_unix.rs`）。
- 远端桥按需拉起远端 `herdr server`，连接远端 client socket，把 socket 与自身 stdin/stdout 互拷。
- 原版**没有跨机聚合视图**，也没有 API socket 桥接；agent 跑在远端 pane 内，其 `herdr` CLI 天然操作远端本机 server。

### 3.2 OcHerdr 现状的复杂度根源

1. **传输层拼凑**：远程 = ssh exec 发现（`herdr session list --json` 解析）+ 双 StreamLocal 隧道（api.sock + client.sock 转发到 /tmp）+ NDJSON 控制面 + bincode 终端面。故障面 = 发现脚本 + 隧道进程 + 两个本地 socket 文件 + 版本严格相等。
2. **三套主机切换 UI 并存**：状态栏 Host Switcher 弹层、Aggregate 侧栏模式、整窗 Host Center；Aggregate 开关藏在状态栏 20px 图标内，发现性极差。
3. **双份体系**：⌃B 前缀与 ⌘ 系快捷键同时印在菜单；Appearance 与内嵌 Herdr Settings 两个"设置"入口并排；同一动作（关 pane/重命名/新 tab）各有 4 处入口。

### 3.3 关键事实：endpoint 协议已具备直通的一切零件

`crates/ocherdr-herdr/src/endpoint.rs`（herdr 0.9+，protocol ≥ 22）：

- **一条连接承载全部**：session snapshot 推送 + 合成渲染面 + 语义输入 + endpoint 作用域 JSON API 通道（`EndpointCommand::Request`）。
- `endpoint.hello.v1` 按名协商编解码，跨 herdr 版本稳定（不要求构建版本相等）。
- 该协议就跑在 client socket 上——正是原版 `remote-client-bridge` 桥接的那个 socket。

因此 `ssh -T <dest> 'exec herdr remote-client-bridge --session <name>'` + `endpoint.hello.v1` 即可替代今天"发现 ssh + 双隧道 + NDJSON api.sock"整条数据链路。

## 4. 目标与非目标

### 4.1 目标

- 每台 host 的数据面只有**一条连接**：本地 Unix socket / 远程 ssh stdio 桥，均跑 endpoint 协议。
- 本机任意 agent/script 可用库存 herdr CLI 直达任意已连接 host：`HERDR_SOCKET_PATH=~/.config/ocherdr/hosts/<name>.sock herdr pane list`。
- 侧栏是唯一导航面：顶部恒常机器分组（跨机一览），其下为当前 host 的 workspaces/agents。
- `just ci`（fmt + clippy -D warnings + test）全绿。

### 4.2 非目标

- 不做远端 herdr 二进制的自动上传/安装。
- 不保留 herdr 0.8.x（protocol ≤ 21）任何兼容路径。
- 不引入 TCP/TLS 直连通道（传输层委托 OpenSSH，与原版一致）。
- 不动文件面板的 SFTP 通道、不动 pane 拖拽交互。

## 5. 设计

### 5.1 P1：一条连接（传输层直连 + legacy 删除）

**传输抽象**：`crates/ocherdr-herdr/src/endpoint.rs` 的连接从具体 `UnixStream` 抽象为 `Read + Write`（reader/writer 两半）。载体枚举：

- `Local(UnixStream)`：连接 `<session>.sock` 对应的 client socket（现状不变）。
- `SshStdio(Child)`：spawn `ssh -T <dest> 'exec <herdr> remote-client-bridge --session <name>'`，endpoint 帧直接写在子进程 stdin/stdout。远端 herdr 路径沿用现有 `remote_herdr_command` 发现逻辑。

**协商与错误**：`endpoint.hello.v1` 被拒或超时 → 明确报错"远端 herdr 需 ≥ 0.9"，**无回退路径**（决策 1）。

**删除清单**：

- `private_v20.rs`（冻结 v20 wire schema）及 legacy `TerminalSession` 逐 pane 流路径。
- `SshTunnel`（双 StreamLocal 转发）及其生命周期管理。
- NDJSON api.sock 数据面（`request_socket`、`EventStream`/`subscribe_events`）：API 调用统一走 endpoint API lane；事件统一走 endpoint 推送（snapshot 增量 + semantic notification）。
- 应用侧 legacy/endpoint 双路径事件处理，只留 endpoint 路径。

**保留**（带外管理操作，与原版一致）：`herdr session list --json` 发现（本地直接 exec / 远程 ssh exec）、`session stop/delete`、`--version` 健康探测、SSH config 导入。

**收益**：每台 host 只剩一个 ssh 进程（无 /tmp socket 对、无隧道保活竞态）；重连 = 重启该进程；版本协商内建于握手。

### 5.2 P2：一个控制面（本机 skill 直达远程）

- OcHerdr 为每个**已连接**的 host 在本机监听一个代理 socket：`~/.config/ocherdr/hosts/<profile-label-slug>.sock`，权限 0600，说原生 NDJSON public API（与 herdr.sock 同 schema）。
- 收到请求后经该 host 的 endpoint API lane 转发（注意 lane 单飞：代理层需排队串行化），响应原样 relay。
- 本地 host 的代理 socket 直接转发到本机 herdr.sock（或同样走 endpoint lane，取实现简单者）。
- 用途：本机 agent/skill/脚本以**零改动**的库存 CLI 控制任意已连接 host。这超出原版能力（原版不桥接 API），但手感与原版"agent 在远端 pane 内操作本机 server"完全一致。

### 5.3 P3：一个侧栏（导航收敛）

- **侧栏顶部恒常"机器"分组**：本机 + 各远程 host（状态点、名称、折叠 chevron），组内列 workspace 摘要行 + agent 状态行——即现有 Aggregate 渲染逻辑上移为唯一侧栏形态。点击他机的行 = 切 host 并聚焦该 pane（复用 `select_aggregate_target`）。
- **删除**：`SidebarMode::Single/Aggregate` 及状态栏 20px 切换图标、状态栏 Host Switcher 弹层。
- **Host Center 只做管理**（增删改/SSH 导入/健康检查），入口为机器分组头部一个齿轮按钮。
- **parked host 轻量化**：非活跃但已连接的 host 保留 endpoint 连接，关闭渲染流（`client_shell.surface.set active=false`），只收 snapshot 增量；从未连接的 host 显示灰色缓存态（上次 snapshot 或仅名称）。
- 状态栏左侧 = 当前 host 名 + 连接状态（掉线可点重连），不再承担切换功能。

### 5.4 P4：chrome 瘦身

- **工具栏**右侧 8 → 5：`split-right`、`split-down`｜`files`、`settings`、`hosts`。zoom/close-pane 收进 pane 右键菜单与快捷键。
- **设置入口合一**：工具栏只留一个设置按钮（⌘,）；Herdr Settings（内嵌 TUI）成为 Appearance 页的一个分区，删除标签栏独立齿轮与 ⌃B s 之外的双入口提示。
- **菜单只印 ⌘ 快捷键**；⌃B 前缀保留为肌肉记忆层，不在菜单重复广告。
- **关闭确认加"不再询问"**（存 ui-state.json，可在设置中复位）。
- **Agent 面板补正常入口**：pane 标题栏 agent 状态点可点击打开。

## 6. 验收标准

1. `just ci` 全绿。
2. 远程 host 连接全程只有一个 ssh 进程（`remote-client-bridge`），/tmp 无 `ocherdr-*` 转发 socket。
3. `HERDR_SOCKET_PATH=~/.config/ocherdr/hosts/<name>.sock herdr workspace list` 对本机已连接的远程 host 返回该 host 的真实数据。
4. 侧栏无模式切换；任一已配置 host 两行内（展开分组→点击）可切换；Host Center 仅承担管理。
5. 连接 herdr 0.8.x 服务器得到明确的"需升级"错误，无回退、无崩溃。

## 7. 已知代价与风险

- **endpoint API lane 单飞**：高并发 API 调用（代理 socket + GUI 自身）会排队；代理层与 GUI 共用 lane 时延迟互相可见。缓解：代理请求与 GUI 请求公平排队，snapshot 推送不受影响。
- **legacy 删除是一次性断路**：升级 OcHerdr 即要求所有 host 的 herdr ≥ 0.9；报错文案必须可直接指导升级。
- **测试基建重写**：`ocherdr-herdr` 与应用侧的 fake socket 测试需从 NDJSON/v20 迁到 endpoint 帧。
- **parked host 仍占一条 ssh 进程/台**：比现状（隧道+双 socket+事件流+per-pane 订阅）轻得多，但机器极多时仍需后续做按需连接。
