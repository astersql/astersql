# `br/pkg/streamhelper/daemon/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-streamhelper-daemon` 的 crate 根。`Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定它，并在 `[package.metadata.porting]` 中把该 crate 对应到 Go 包 `br/pkg/streamhelper/daemon`。根 workspace 的 `Cargo.toml` 已把本目录列为成员；但仓库内未发现其他 Rust `Cargo.toml` 对这个包的依赖，且相邻的 `br/pkg/streamhelper/lib.rs` 没有挂载本目录。因此，就当前代码而言，它是可独立构建和测试的迁移单元，而不是 Rust 日志备份主链上已经接通的生产入口。

本文件自身不实现选举或 tick 算法。它负责组装 [`interface.rs`](interface.rs) 与 [`owner_daemon.rs`](owner_daemon.rs)，再把两个模块的公开项扁平导出，使调用者可以从 crate 根使用 Go 同包风格的 API。

## 核心职责

- 用 `#[path = "interface.rs"] pub mod interface` 暴露生命周期、取消上下文、错误和 owner 管理器抽象。
- 用 `#[path = "owner_daemon.rs"] pub mod owner_daemon` 暴露 owner 守护进程及其异步循环。
- 通过 `pub use interface::*` 和 `pub use owner_daemon::*` 建立 crate 根兼容门面；公开 API 的真实实现仍在上述两个子模块。
- 仅在 `cfg(test)` 下挂载 `owner_daemon_test.rs`、`interface_test.rs` 和 `parity_test.rs`，保证独立测试与生产导出分离。
- 以 crate 级 `#![allow(...)]` 容纳从 Go 迁移而来的命名方式和暂未接线的符号；这些 lint 放宽不改变运行时语义。

## 主要符号

`lib.rs` 没有自定义常量、结构体、trait 或函数，只有模块声明和再导出。经门面公开的关键符号如下：

- `interface::DaemonError` 与 `interface::Result<T>`：只保存字符串消息的统一错误及结果别名。
- `interface::Context`、`CancelFunc`：基于 `tokio_util::sync::CancellationToken` 的 Go `context.Context` / `context.CancelFunc` 对应物；支持父子取消传播、显式取消和异步等待。
- `interface::Interface: Send`：业务守护逻辑的 `OnStart`、`OnBecomeOwner`、`OnTick`、`Name` 生命周期契约。
- `interface::Manager: Send + Sync` 与 `SharedManager = Arc<dyn Manager>`：`OwnerDaemon` 实际需要的 owner 管理器最小接口，包括身份、所有权查询、竞选、强制夺主和卸任。
- `owner_daemon::OwnerDaemon` 与构造函数 `New`：组合 `Box<dyn Interface>`、共享 `Manager` 和 tick 周期。`Running`、`ForceToBeOwner`、`RetireIfOwner`、`manager` 是其公开控制面。
- `owner_daemon::DaemonLoop`：`OwnerDaemon::Begin` 返回的异步循环对象；调用者必须显式执行 `DaemonLoop::run` 才会开始周期处理。
- `owner_daemon::share_manager`：把具体的 `Manager` 实现转换成 `SharedManager`。

## 执行流程

1. 使用方从 crate 根取得 `New`、`Interface`、`Manager` 等扁平导出，提供业务回调、owner 管理器和非零 `Duration`。
2. `New` 只建立状态，尚不竞选 owner；`OwnerDaemonState.cancel` 初始为 `None`。
3. `OwnerDaemon::Begin` 先读取守护名称并调用 `Manager::CampaignOwner`。竞选失败立即返回错误，不调用 `Interface::OnStart`。
4. 竞选成功后同步调用 `OnStart`，校验 tick 周期非零，并在 `Begin` 时记录首个 deadline，随后返回 `DaemonLoop`。这与 Go 在 `Begin` 内创建 `time.Ticker` 的时点一致。
5. 调用者异步执行 `DaemonLoop::run`。循环在父 `Context` 取消时退出；每次 tick 先调用 `Manager::IsOwner`。
6. 若当前是 owner，首次进入时由 `ownerTick` 创建子 `Context`、保存 `CancelFunc`、调用 `OnBecomeOwner`，随后调用 `OnTick`；后续 owner tick 不重复调用 `OnBecomeOwner`。
7. 若当前不是 owner，`cancelRun` 取出并调用保存的取消句柄，使业务层收到失主通知，同时令 `Running` 变为 `false`。之后若重新夺主，会建立新的 owner 会话并再次执行 `OnBecomeOwner`。

Go 生产链的直接证据是 `br/pkg/task/stream.go` 中的 `RunStreamAdvancer`：它构造日志备份 advancer 和 owner manager，调用 `daemon.New`、`Begin`，可选地周期调用 `ForceToBeOwner` / `RetireIfOwner`，最后运行返回的闭包。Rust 目前没有等价生产调用者；不能仅凭本门面存在就声称 Rust 主链已接通。

## 数据与状态

本文件本身无运行时字段。再导出实现中的关键状态集中在 `owner_daemon.rs`：

- `OwnerDaemonState` 保存唯一的 `Box<dyn Interface>` 和 `Option<CancelFunc>`；后者既代表当前 owner 会话，也作为 `Running` 的判据。
- `OwnerDaemon.state` 是 `Arc<Mutex<OwnerDaemonState>>`，让控制对象与可独立 spawn 的 `DaemonLoop` 共享业务回调和 owner 会话状态。
- `OwnerDaemon.manager` / `DaemonLoop.manager` 是 `Arc<dyn Manager>`；选举状态由管理器拥有，本 crate 只读取和转发操作。
- `tickInterval` 和 `firstTick` 定义调度。`firstTick` 在 `Begin` 中计算，因此延迟启动 `run` 不会重新起算第一个周期。
- `Context` 内部是可克隆的 `CancellationToken`；`with_cancel` 创建子 token，取消向后代传播但不反向取消父级。

重要不变量是：`cancel.is_some()` 表示已经调用过本轮 `OnBecomeOwner`；失主必须 `take` 并调用它，下一轮成为 owner 才能重新进入。业务实现应按 Go 注释保持无状态或把持久状态放在共享存储中，不能依赖单节点本地内存来保证集群正确性。

## 依赖与调用关系

- crate 依赖仅有 `tokio`、`tokio-util` 和 `tracing`。`tokio` 提供 interval、select 和异步测试运行时，`tokio-util` 提供取消 token，`tracing` 记录生命周期与错误。
- `lib.rs → interface.rs`：声明并再导出上下文、错误、`Interface`、`Manager` 和共享句柄。
- `lib.rs → owner_daemon.rs`：声明并再导出 `New`、`OwnerDaemon`、`DaemonLoop` 与 `share_manager`；后者反向依赖 `crate::interface` 的契约。
- `OwnerDaemon::Begin → Manager::CampaignOwner → Interface::OnStart`；`DaemonLoop::run → Manager::IsOwner → ownerTick/cancelRun`；`ownerTick → Interface::OnBecomeOwner/OnTick`。
- RustCodeGraph 没有识别出本 crate 的生产 Rust 调用链；结合 workspace、各 Cargo manifest 和相邻 crate 根的文本搜索，可确认当前 crate 的隔离接线状态。
- Go 侧直接上游是 `br/pkg/task/stream.go::RunStreamAdvancer`，下游是 `br/pkg/streamhelper/daemon/interface.go`、`owner_daemon.go` 以及 `pkg/owner.Manager`。Rust 为控制依赖规模，用本地 `Manager` trait 代替完整的 owner/etcd 依赖。

## 错误处理与边界

- `CampaignOwner` 错误通过 `?` 原样作为 `DaemonError` 返回，且 `OnStart` 不发生；`parity_test.rs::go_rust_public_contract_matches` 覆盖该短路顺序。
- `OnTick` 错误只通过 `tracing::warn!` 记录，循环继续；同一测试要求连续至少两次 tick，证明错误不会终止守护任务。
- `ForceToBeOwner` 直接返回管理器错误；`RetireIfOwner` 的 trait 签名无返回值，且只发出卸任请求，`cancel` 要到后续非 owner tick 才清理。
- tick 周期为零时，`Begin` 在 `OnStart` 之后 panic，模拟 Go `time.NewTicker` 对非正周期的边界；Rust `Duration` 无法表达负值。
- `Mutex::lock().unwrap()` 会在锁中毒时 panic；当前实现没有恢复策略。业务回调在持锁期间执行，因此回调不应重入同一 `OwnerDaemon` 状态，也不应长时间阻塞。
- 父 `Context` 取消会退出循环，但 `DaemonLoop::run` 的取消分支不会额外调用 `cancelRun`。因此“整个循环退出时是否必须显式取消当前 owner 子上下文”不是现有实现保证；不要在文档外推该清理行为。
- `DaemonError` 只有文本，不携带错误分类或 source 链。需要可重试分类时应先扩展错误契约和测试，而不是解析消息字符串。

## 并发与资源生命周期

`OwnerDaemon` 与 `DaemonLoop` 通过 `Arc<Mutex<_>>` 共享状态，`Manager` 通过 `Arc<dyn Manager + Send + Sync>` 跨任务共享。循环每个 tick 先读取 `IsOwner`，再锁住状态并执行回调，以避免在调用管理器时长期占有状态锁；但 `Name`、`OnBecomeOwner`、`OnTick` 仍在锁内运行。

成为 owner 时创建的子 `Context` 是 owner 资格的资源作用域。失主 tick 调用 `CancelFunc::call`，业务层应监听 `Context::cancelled` 并停止本轮 owner 专属任务。`CancelFunc` 可克隆、可重复和并发调用；`interface_test.rs` 验证了幂等性、父子传播以及子取消不影响父上下文。

调度使用 `tokio::time::interval_at` 和 `MissedTickBehavior::Skip`，对应 Go ticker 在消费者落后时不会补放全部历史 tick 的行为。`owner_daemon_test.rs::test_daemon` 验证启动、首次成为 owner、tick、卸任取消、再次夺主和退出；它使用 mock manager，不覆盖真实 etcd 会话或网络故障。

## 与 Go 版本的对应关系

- `interface.rs::Interface` 逐项对应 `interface.go::Interface`；Rust 用 `Context` 包装 `CancellationToken`，而 Go 直接使用 `context.Context`。
- `owner_daemon.rs::OwnerDaemon`、`New`、`Running`、`Begin`、`ForceToBeOwner`、`RetireIfOwner` 对应 `owner_daemon.go` 同名类型和方法。Rust 把 Go 返回的 `func()` 主循环表示为 `DaemonLoop` 及其 `async fn run`。
- Go 结构声明为同步使用；Rust 为允许循环拥有 `'static` 数据并被 spawn，增加 `Arc<Mutex<OwnerDaemonState>>`。这是实现机制差异，不改变 owner 切换次序。
- Go 直接依赖 `pkg/owner.Manager` 和 etcd；Rust 只定义其实际使用的方法子集 `Manager`，所以真实 owner 后端仍需适配器才能进入生产链。
- Go `RunStreamAdvancer` 已实际构造并运行该 daemon；Rust 相邻 `advancer_daemon.rs` 只为 `CheckpointAdvancer` 提供另一组同步生命周期方法，并未实现这里的 `Interface`，也未引用此 crate。这是当前迁移状态的明确缺口，不应描述为已对齐接线。
- `owner_daemon_test.rs` 映射 Go `owner_daemon_test.go::TestDaemon`；`parity_test.rs` 额外固定竞选失败、tick 错误、调度起点、零周期 panic 和 Force/Retire 转发等契约。

## 扩展指南

- 新增业务守护进程时，实现 crate 根导出的 `Interface`，并提供满足 `Manager` 的适配器；生产接线还需在上层 Cargo manifest 中声明本 crate 依赖，不能只修改 `lib.rs`。
- 新增生命周期钩子或 manager 能力时，应同步修改 `interface.rs`、`owner_daemon.rs`、Go 对照接口/行为评估以及独立的 `interface_test.rs`、`owner_daemon_test.rs`、`parity_test.rs`。不要把测试逻辑嵌入 `lib.rs`。
- 若只增加内部实现，优先保留当前 `pub mod` 与扁平 `pub use` 的兼容面；收窄公开项可能破坏未来按 Go 包级符号迁移的调用者。
- 修改调度时必须保留或明确变更三个契约：首个 deadline 从 `Begin` 起算、missed tick 使用 Skip、`OnTick` 错误不中止循环。相应回归入口是 `tick_schedule_starts_during_begin`、`zero_tick_interval_panics_during_begin` 和 `go_rust_public_contract_matches`。
- 修改取消或锁策略时，重点检查失主只取消当前 owner 子树、重新夺主只创建一个会话、回调重入/阻塞风险和循环退出时的资源清理。若补上退出时取消当前会话，应同时对照 Go 行为并添加明确回归测试。
- 接入真实 owner/etcd 后端会扩大兼容与故障面，应测试竞选失败、租约丢失、重新竞选、关闭期间的竞态，并评估 tick 持锁调用对延迟的影响。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`node --file br/pkg/streamhelper/daemon/lib.rs` 确认本文件 41 行的模块/再导出结构；对 `interface.rs`、`owner_daemon.rs` 和三份测试的 `node` 查询核对了符号、状态和流程；`explore` 给出 `run → ownerTick/cancelRun` 等直接调用边，并未找到本 crate 的生产 Rust 上游。
- crate 与接线：`br/pkg/streamhelper/daemon/Cargo.toml`、根 `Cargo.toml`、`br/pkg/streamhelper/lib.rs`、`br/pkg/streamhelper/advancer_daemon.rs`。
- Rust 实现：`br/pkg/streamhelper/daemon/interface.rs`、`br/pkg/streamhelper/daemon/owner_daemon.rs`。
- Rust 独立测试：`br/pkg/streamhelper/daemon/interface_test.rs`、`owner_daemon_test.rs`、`parity_test.rs`。
- Go 对照与生产入口：`br/pkg/streamhelper/daemon/interface.go`、`owner_daemon.go`、`owner_daemon_test.go`、`br/pkg/task/stream.go::RunStreamAdvancer`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的正则结构检查，确认本文恰好包含十一个固定二级章节；同时人工复核门面职责、当前未接线事实、生命周期顺序、边界和扩展入口均有上述代码或测试依据。
