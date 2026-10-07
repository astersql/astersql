# `br/pkg/streamhelper/daemon/interface.rs`

## 文件定位

本文件是 `astersql-br-pkg-streamhelper-daemon` crate 的契约层，定义守护任务实现、owner 选举管理器与取消上下文之间的边界；它不执行选举，也不创建 tick 循环。crate 入口 `br/pkg/streamhelper/daemon/lib.rs` 通过 `pub mod interface` 挂载本文件并用 `pub use interface::*` 扁平导出其公开符号，实际生命周期编排位于 `br/pkg/streamhelper/daemon/owner_daemon.rs`。

`br/pkg/streamhelper/daemon/Cargo.toml` 将该目录声明为独立 library crate（`lib.rs` 为入口），运行时直接依赖 `tokio`、`tokio-util` 和 `tracing`。本文件只直接使用 `tokio_util::sync::CancellationToken`；日志与异步定时编排由相邻的 `owner_daemon.rs` 使用。文件内没有条件编译项、模块级常量或自由函数。

## 核心职责

本文件提供四组基础契约：

1. `DaemonError` 与 `Result<T>` 为守护钩子和选举操作提供统一、轻量的错误通道。
2. `Context` 与 `CancelFunc` 用 `CancellationToken` 表达 Go `context.Context` / `context.CancelFunc` 在本 crate 实际需要的取消子集。
3. `Interface` 规定业务守护组件在服务启动、获得 owner、周期 tick 与日志命名四个阶段的回调。
4. `Manager` 与 `SharedManager` 抽取 `OwnerDaemon` 所需的最小 owner 管理表面，使该精简 crate 不必直接拉入 `astersql-owner`、etcd 或 kvproto。

因此，这个文件存在的目的不是承载业务算法，而是让 `OwnerDaemon` 能在不知道具体业务守护实现和选举后端的情况下，按 Go 版本的顺序驱动生命周期，并能在失去 owner 时通过取消传播触发业务清理。

## 主要符号

- `pub struct DaemonError { message: String }`：仅保存消息字符串的错误类型。`DaemonError::new(impl Into<String>)` 构造错误，`fmt::Display` 原样输出消息，并实现标准库 `Error`。它没有错误码、结构化字段或下层 `source`。
- `pub type Result<T> = std::result::Result<T, DaemonError>`：统一 `Interface::OnTick` 以及 `Manager::{CampaignOwner,ForceToBeOwner}` 的错误类型。
- `pub struct Context { token: CancellationToken }`：可克隆的取消上下文。`background()` 创建独立根 token；`with_cancel(&Context)` 用 `child_token()` 返回子上下文和控制该子上下文的 `CancelFunc`；`is_done()` 非阻塞检查；`cancelled()` 返回借用当前上下文的等待 future；`token()` 提供底层 token 的只读引用；`cancel()` 显式取消当前 token。
- `pub struct CancelFunc { token: CancellationToken }`：可克隆取消句柄；`call()` 调用 token 的 `cancel()`。重复调用和多个克隆并发调用均依赖 `CancellationToken` 的幂等、线程安全语义。
- `pub trait Interface: Send`：业务守护契约。`OnStart(&mut self, &Context)` 在是否为 owner 无关的服务启动阶段调用；`OnBecomeOwner(&mut self, Context)` 接收一个在失主时会取消的独立 owner 会话上下文；`OnTick(&mut self, &Context) -> Result<()>` 执行周期工作；`Name(&self) -> String` 提供日志/追踪标识。trait 只要求 `Send`，串行可变访问由 `OwnerDaemonState` 的互斥锁保证。
- `pub trait Manager: Send + Sync`：选举后端的最小接口。`ID()` 返回竞选身份，`IsOwner()` 查询当前身份，`CampaignOwner()` 启动或继续竞选，`ForceToBeOwner(&Context)` 强制重启竞选，`RetireOwner()` 主动卸任。
- `pub type SharedManager = Arc<dyn Manager>`：允许 `OwnerDaemon` 与其返回的 `DaemonLoop` 共享同一个线程安全管理器对象。

所有上述符号都是公开 API；字段 `DaemonError::message`、`Context::token` 和 `CancelFunc::token` 保持私有，以防调用者绕开既定构造与取消边界。

## 执行流程

正常启动和 owner 生命周期由 `br/pkg/streamhelper/daemon/owner_daemon.rs` 串接：

1. `OwnerDaemon::Begin` 先调用 `Interface::Name` 记录标识，再调用 `Manager::CampaignOwner`；竞选失败通过本文件的 `Result` 立即返回，`OnStart` 不会执行。
2. 竞选请求成功后，`Begin` 在锁内调用 `Interface::OnStart(&ctx)`，随后构造并返回 `DaemonLoop`。
3. `DaemonLoop::run` 每个 tick 先调用 `Manager::IsOwner`。首次观察到 owner 时，`OwnerDaemon::ownerTick` 调用 `Context::with_cancel(ctx)`，保存 `CancelFunc`，把子 `Context` 按值交给 `Interface::OnBecomeOwner`，然后调用 `Interface::OnTick`。
4. 后续仍为 owner 的 tick 不会重复调用 `OnBecomeOwner`，只调用 `OnTick`。
5. 观察到非 owner 时，`OwnerDaemon::cancelRun` 取走并调用保存的 `CancelFunc`。业务实现可等待 `OnBecomeOwner` 收到的 `Context::cancelled()`，停止仅允许 owner 运行的后台资源。
6. 再次获得 owner 时会创建新的子上下文和取消句柄，形成新的 owner 会话。外层根 `Context` 取消时，`DaemonLoop::run` 退出；`CancellationToken` 的父子传播也会使仍存活的 owner 子上下文完成取消。

`Context::cancel()` 主要供外层关停和测试使用；生产编排中的失主路径使用 `CancelFunc::call()`，两者最终都落到相应 `CancellationToken::cancel()`。

## 数据与状态

本文件自身没有全局状态。每个 `Context`/`CancelFunc` 保存一个 `CancellationToken` 句柄：克隆共享同一 token 状态，`child_token()` 则建立单向的父到子取消关系。父 token 取消会传播到已有子节点；已取消父节点上新建的子节点也立即处于取消状态；取消子节点不会反向取消父节点。这些不变量由 `interface_test.rs` 的两个测试直接断言。

`DaemonError` 拥有一个 `String`，错误信息的生命周期独立于输入。`SharedManager` 用引用计数共享一个动态分派对象；其 `Send + Sync` 约束允许主对象与异步循环跨任务持有。`Interface` 对象则由 `Box<dyn Interface>` 独占，生命周期状态保存在相邻文件的 `OwnerDaemonState` 中，并在 `Mutex` 保护下以 `&mut self` 串行调用。

## 依赖与调用关系

直接下游依赖只有标准库的 `Error`、`fmt`、`Future`、`Arc`，以及 `tokio_util::sync::CancellationToken`。`Context::cancelled` 把 tokio-util 的等待 future 以 `impl Future<Output = ()> + '_` 隐藏起来，调用者无需依赖具体 future 类型。

直接上游是 `br/pkg/streamhelper/daemon/owner_daemon.rs`：

- `OwnerDaemonState` 持有 `Box<dyn Interface>` 与 `Option<CancelFunc>`。
- `OwnerDaemon` 和 `DaemonLoop` 持有 `SharedManager`。
- `Begin` 调用 `Name`、`CampaignOwner`、`OnStart`。
- `ownerTick` 调用 `Context::with_cancel`、`Manager::ID`、`Interface::{Name,OnBecomeOwner,OnTick}`。
- `cancelRun` 调用 `Name` 与 `CancelFunc::call`。
- `DaemonLoop::run` 等待 `Context::cancelled`，并调用 `Manager::{ID,IsOwner}`。
- `ForceToBeOwner`、`RetireIfOwner` 分别透传到同名 manager 行为；`share_manager` 把具体 `Manager` 实现提升为 `SharedManager`。

`br/pkg/streamhelper/daemon/lib.rs` 是公开入口；仓库当前的具体 `Interface`、`Manager` 实现主要位于独立测试 `owner_daemon_test.rs` 与 `parity_test.rs`。RustCodeGraph 对目录的文件清单显示该 crate 共 9 个已索引 Go/Rust 文件，对 `interface.rs` 识别出 24 个符号；图结果同时定位到 `owner_daemon.rs` 中的 `manager`/`share_manager` 以及本文件全部类型和方法。对于常见名称（如 `Context`、`Manager`），图的全局名字查询会混入其他模块，本文的调用边因此以精确路径查询结果与相邻源码交叉核对，而不把同名结果当作本 crate 的调用者。

## 错误处理与边界

- `DaemonError` 只保留字符串，适合当前窄接口，但会丢失结构化错误类别和 source chain；新增需要分类重试或诊断的后端时，应先评估是否扩展该类型，不能仅依赖字符串匹配。
- `Manager::CampaignOwner` 与 `ForceToBeOwner` 可失败并返回 `Result<()>`。`Begin` 使用 `?` 保证竞选失败短路；`parity_test.rs::go_rust_public_contract_matches` 验证失败时 `OnStart` 没有副作用。
- `Interface::OnTick` 的错误由 `OwnerDaemon::ownerTick` 记录为 warning 后吞掉，主循环继续；测试用连续 tick 计数证明错误不会终止 daemon。实现者不应把返回错误理解为停止请求。
- `OnStart` 与 `OnBecomeOwner` 没有返回值，不能通过接口上报同步失败；需要自行记录或在内部安排可观察状态。
- `Context` 只模拟取消，不携带 Go context 的 deadline、键值或错误原因；调用者不能假设 `Deadline`、`Value`、`Err`/cause 等完整 Go API 已被移植。
- `Context::token()` 暴露共享引用，允许组合 select 分支，但不转移所有权；`cancelled()` 返回的 future 也借用 `self`，等待期间上下文必须存活。
- 本文件不做超时、panic 捕获或锁中毒恢复。相邻实现使用 `Mutex::lock().unwrap()`，trait 实现若 panic 会沿任务传播。

## 并发与资源生命周期

`CancellationToken` 使 `Context` 和 `CancelFunc` 可安全克隆并跨线程取消。`interface_test.rs::cancel_func_is_repeatable_concurrent_and_child_scoped` 用两个 OS 线程并发调用克隆的 `CancelFunc`，再重复调用原句柄，验证取消幂等、子树传播且不影响父上下文；`parent_cancellation_propagates_to_existing_and_new_children` 验证父取消对既有和新建子节点都生效。

owner 资源生命周期以每次 `OnBecomeOwner` 收到的子 `Context` 为界。业务实现可以派生异步任务等待 `ctx.cancelled()`；失主时 `CancelFunc::call` 唤醒这些任务。`owner_daemon_test.rs` 的 `AnApp` 在 owner 上下文取消后清除 `begun` 并关闭 stop 信号，证明失主清理实际发生；重新获得 owner 会使用新上下文，而不是复用已取消 token。

`Interface: Send` 配合 `Box<dyn Interface>` 允许整体移入可 spawn 的状态，但没有要求业务对象自行 `Sync`。相邻 `OwnerDaemon` 用 `Arc<Mutex<OwnerDaemonState>>` 串行化 `OnStart`、`OnBecomeOwner`、`OnTick` 和 `Name` 的访问。`Manager: Send + Sync` 则是因为同一实例通过 `Arc` 同时由 `OwnerDaemon`、`DaemonLoop` 及可能的测试/外部控制路径访问；具体实现必须保证自身内部同步。

## 与 Go 版本的对应关系

`Interface` 的四个方法逐一对应 `br/pkg/streamhelper/daemon/interface.go` 的同名方法，Rust 保留了 Go 风格方法名以便移植对照：`OnStart` 在所有节点启动，`OnBecomeOwner` 只在获得 owner 时启动一轮资源，`OnTick` 周期执行并可返回错误，`Name` 用于追踪。

主要类型映射如下：

- Go `context.Context` → Rust `Context`（仅取消语义）。
- Go `context.CancelFunc` → Rust `CancelFunc::call`。
- Go `error` → Rust `DaemonError` / `Result<T>`。
- Go `owner.Manager` → Rust 本地 `Manager` trait。
- Go 接口值 `Interface` → Rust `Box<dyn Interface>`（使用点在 `owner_daemon.rs`）。
- Go `owner.Manager` 共享接口值 → Rust `Arc<dyn Manager>`，即 `SharedManager`。

Rust 本地 `Manager` 并非 `interface.go` 中的声明，而是从 `owner_daemon.go` 实际调用的 `owner.Manager` 方法裁剪而来。它刻意不等价于完整上游 owner 包，只包含 `ID`、`IsOwner`、`CampaignOwner`、`ForceToBeOwner`、`RetireOwner`。Rust 的 `Context` 也不是完整 Go context：没有 deadline、值传递和取消原因。其父子取消、重复取消与失主清理语义通过 `interface_test.rs`、`owner_daemon_test.rs` 和 `parity_test.rs` 保持与当前 Go 使用方式一致。

## 扩展指南

- 新增业务 daemon：实现 `Interface`，将只需一次、且与 owner 无关的初始化放在 `OnStart`；把仅 owner 可持有的后台资源绑定到 `OnBecomeOwner` 的 `Context`；让 `OnTick` 可重复执行并处理重试；提供稳定且低开销的 `Name`。同步在独立测试文件中覆盖启动顺序、失主取消、重新获得 owner 和 tick 错误路径，不要把测试嵌入生产文件。
- 新增选举后端：实现 `Manager: Send + Sync`，明确 `CampaignOwner` 是否启动后台竞选、`RetireOwner` 后是否自动再竞选，以及 `IsOwner` 的一致性窗口；用 `owner_daemon_test.rs`/`parity_test.rs` 同类 mock 验证透传与状态转换。
- 扩展取消能力：优先在 `Context`/`CancelFunc` 中集中封装，保持父取消向子传播、子取消不影响父和重复取消幂等三个不变量；在 `interface_test.rs` 添加针对性的并发测试。若需要 deadline/value/cause，必须同时评估 Go 对照语义与所有调用点，不能把普通 token 字段误称为完整 context。
- 扩展错误模型：修改 `DaemonError` 与 `Result<T>` 前检查 `OwnerDaemon::Begin` 的短路路径和 `ownerTick` 的“记录后继续”策略，并更新 `parity_test.rs` 的竞选失败、tick 失败断言。结构化错误可能影响日志格式与调用者匹配，属于兼容风险。
- 修改 trait 方法签名是公开 API 破坏性变更，会同时影响 `OwnerDaemon`、所有具体实现及测试 mock。性能上应避免在高频 `OnTick`/`IsOwner` 路径引入不必要分配；当前 `Name`/`ID` 每次返回 `String` 已会分配，若优化必须与 Go 可观测日志语义一起评估。

## 验证依据

本说明基于以下直接证据人工交叉核对：

- 目标源码：`br/pkg/streamhelper/daemon/interface.rs`（全部类型、trait、impl 与公开方法）。
- crate 与导出边界：`br/pkg/streamhelper/daemon/Cargo.toml`、`br/pkg/streamhelper/daemon/lib.rs`。
- Rust 直接调用者：`br/pkg/streamhelper/daemon/owner_daemon.rs`。
- Go 对照：`br/pkg/streamhelper/daemon/interface.go`、`br/pkg/streamhelper/daemon/owner_daemon.go`。
- Rust 独立测试：`br/pkg/streamhelper/daemon/interface_test.rs`、`owner_daemon_test.rs`、`parity_test.rs`。
- Go 独立测试：`br/pkg/streamhelper/daemon/owner_daemon_test.go`。
- RustCodeGraph：`status` 报告索引包含 7,032 个 Rust 文件、索引时间戳 `1791342965170`；`files --filter br/pkg/streamhelper/daemon` 列出 9 个 Go/Rust 文件；针对 `DaemonError`、`CancelFunc`、`Interface`、`Manager` 的 `query` 及调用查询确认目标符号与相邻 `owner_daemon.rs` 的使用位置。常见名字的查询存在跨模块同名噪声，最终调用关系以精确文件路径和源码行核实。

本任务是纯文档分析，按计划未运行 Cargo。结构完整性由任务指定的 11 章节命令验证；行为结论来自现有源码和测试内容，并未在本任务中重新执行运行时测试。
