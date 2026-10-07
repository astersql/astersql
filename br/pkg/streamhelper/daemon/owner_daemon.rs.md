# `br/pkg/streamhelper/daemon/owner_daemon.rs`

## 文件定位

本文件是 `astersql-br-pkg-streamhelper-daemon` 独立 library crate 的 owner 生命周期编排实现，由同目录 [`lib.rs`](lib.rs) 以 `owner_daemon` 模块加载并扁平再导出。crate 在根 `Cargo.toml` 的 workspace members 中，但仓库内除自身 `Cargo.toml` 外没有其他 Cargo manifest 依赖该 crate；因此当前 Rust 实现已经具备可测试的公开 API，却尚未接入 Rust 生产调用主链。真实生产入口仍是 Go：`br/pkg/task/stream.go` 的 `RunStreamAdvancer` 和 `pkg/domain/domain.go` 的 `initLogBackup` 创建 `daemon.OwnerDaemon`，分别服务命令行日志备份 checkpoint advancer 与 TiDB domain 内的日志备份 advancer。

文件只负责把业务守护对象 [`Interface`](interface.rs) 与选举抽象 [`Manager`](interface.rs) 组合起来，不实现 etcd 选举，也不保存业务进度。源码明确要求业务 daemon 无状态，避免不同节点依赖各自内存或本地存储形成不一致。

## 核心职责

- `New` 保存业务 daemon、共享 manager 和 tick 间隔，但不启动竞选或定时任务。
- `OwnerDaemon::Begin` 依次执行 `Manager::CampaignOwner`、`Interface::OnStart`，然后构造可移入 Tokio task 的 `DaemonLoop`。竞选失败时立即返回错误，`OnStart` 不执行。
- `DaemonLoop::run` 在每个周期读取 `Manager::IsOwner`：是 owner 时执行 `ownerTick`，不是 owner 时执行 `cancelRun`。
- `ownerTick` 保证每段连续 owner 任期只调用一次 `OnBecomeOwner`，但每个 owner tick 都调用 `OnTick`。
- `cancelRun` 在检测到失主时取消本轮 owner 子上下文并清除运行标志，使业务层能释放任期资源；之后若重新成为 owner，会创建新子上下文并再次调用 `OnBecomeOwner`。
- `ForceToBeOwner`、`RetireIfOwner` 与 `manager` 只提供 manager 透传/访问，不自行改变 `OwnerDaemonState::cancel`；状态转换仍由后续 tick 完成。

## 主要符号

- `OwnerDaemonState { daemon: Box<dyn Interface>, cancel: Option<CancelFunc> }`：受互斥锁保护的内部可变状态。`cancel.is_some()` 是“业务已进入 owner 任期”的本地事实，而不是 manager 当前选举状态的直接镜像。
- `OwnerDaemon { state: Arc<Mutex<OwnerDaemonState>>, manager: SharedManager, tickInterval: Duration }`：公开控制句柄。`state` 与返回的循环共享，所以调用方可在循环运行期间查询 `Running` 或请求 manager 操作。
- `New(Box<dyn Interface>, SharedManager, Duration) -> OwnerDaemon`：公开构造函数，保持 Go 风格命名。
- `OwnerDaemon::Running(&self) -> bool`：锁定状态并检查 `cancel`；在 `Begin` 成功到第一次 owner tick 之间仍为 `false`。
- `OwnerDaemon::cancelRun(&mut OwnerDaemonState)`：私有失主任期清理函数；仅在存在取消句柄时记录日志、调用幂等的 `CancelFunc::call` 并通过 `take` 清空句柄。
- `OwnerDaemon::ownerTick(&mut OwnerDaemonState, &dyn Manager, &Context)`：私有 owner tick。任期首次进入时通过 `Context::with_cancel` 创建子上下文，先保存 cancel，再调用 `OnBecomeOwner`，随后调用 `OnTick`。
- `OwnerDaemon::Begin(&self, Context) -> Result<DaemonLoop>`：同步启动阶段；记录第一个 tick 的绝对 deadline，以复现 Go 在 `Begin` 内创建 ticker 的时序。
- `OwnerDaemon::ForceToBeOwner` / `RetireIfOwner`：分别转发 `Manager::ForceToBeOwner` 和 `Manager::RetireOwner`。
- `OwnerDaemon::manager() -> SharedManager`：克隆并返回 `Arc<dyn Manager>`，当前注释定位为测试或外部卸任用途。
- `DaemonLoop { state, manager, tickInterval, firstTick, ctx }` 与 `DaemonLoop::run(self)`：拥有主循环资源；`run` 消耗自身，防止同一个 loop 被重复启动。
- `share_manager<M: Manager + 'static>(M) -> SharedManager`：将具体 manager 封装为 `Arc<dyn Manager>` 的便利函数。

本文件没有常量、trait 或条件编译项；trait、上下文、错误类型及共享 manager 别名均定义在相邻 `interface.rs`。

## 执行流程

1. 调用方用 `New` 注入实现 `Interface` 的业务对象、实现 `Manager` 的选举对象和正 tick 间隔。此时 `cancel` 为 `None`，`Running()` 为 `false`。
2. `Begin` 先在短临界区读取 daemon 名称用于日志，再调用 `CampaignOwner()`。这里“竞选成功返回”由具体 manager 定义；本文件不假定已立即获得租约。
3. 竞选调用成功后，`Begin` 锁住 state 并同步调用一次 `OnStart(&ctx)`。该回调与是否已经成为 owner 无关。
4. `Begin` 检查 `tickInterval` 非零，并把 `Instant::now() + tickInterval` 记录为 `firstTick`；随后返回持有共享状态、manager、根上下文与时钟参数的 `DaemonLoop`。
5. 调用方显式 spawn/await `DaemonLoop::run`。循环用 `tokio::time::interval_at(firstTick, tickInterval)`，并把 missed-tick 策略设为 `Skip`，避免任务延迟后回放全部积压 tick。
6. 每轮 `tokio::select!` 同时等待根上下文取消和 ticker。根上下文取消时记录退出日志并直接返回；ticker 到期时先在锁外读取 `IsOwner()`，再锁定 state。
7. 若是 owner，`ownerTick` 在新任期创建可取消子上下文、保存 cancel、调用 `OnBecomeOwner`，然后调用 `OnTick`；连续任期后续 tick 跳过 `OnBecomeOwner`，只调用 `OnTick`。
8. 若不是 owner，`cancelRun` 取消本轮子上下文并清空 cancel。业务对象负责监听该子上下文并完成自己的任期资源清理。
9. 外部调用 `RetireIfOwner` 只令 manager 卸任；最迟到下一次 tick 才由本文件观察失主并取消子上下文。重新夺主后流程从步骤 7 的新任期分支继续。

## 数据与状态

核心状态机可概括为：`未启动(cancel=None)` → `Begin 成功但尚未 owner tick(cancel=None)` → `任期运行(cancel=Some)` → `失主后的未运行(cancel=None)`。`Manager::IsOwner` 是选举层快照，`cancel` 是业务层任期是否已经启动的标志，两者允许存在一个 tick 周期内的短暂差异。

`daemon` 与 `cancel` 放在同一个 `Mutex` 中，使 `Running`、`OnStart`、`OnBecomeOwner`、`OnTick` 和失主清理串行观察状态。`manager` 使用 `Arc<dyn Manager>` 独立共享；其实现必须满足 `Send + Sync`。`Context` 包装 `CancellationToken`，`Context::with_cancel` 创建会随父上下文取消的 child token；`CancelFunc` 保留同一 child token 的取消能力。

`firstTick` 在 `Begin` 内计算，而不是在 `run` 开始时计算。这意味着调用方延迟启动 loop 且已错过首个 deadline 时，首次 tick 会立即就绪；这是与 Go `time.NewTicker` 构造位置一致的可观察状态。`DaemonLoop::run(self)` 消耗 loop 所有权，天然限定每个返回值只运行一次。

## 依赖与调用关系

直接标准库依赖是 `Arc`、`Mutex` 和 `Duration`。外部依赖由同目录 `Cargo.toml` 声明：`tokio` 提供 `select!`、时间 interval 与运行时能力，`tracing` 提供 debug/info/warn 日志；`tokio-util` 由 `interface.rs` 的 `CancellationToken` 使用。该 crate 没有 feature 开关。

模块内下游关系为：`New` 构造 `OwnerDaemon`；`Begin` 调用 `Manager::CampaignOwner` 与 `Interface::OnStart` 并构造 `DaemonLoop`；`DaemonLoop::run` 调用 `Manager::IsOwner`，再分派到 `ownerTick` 或 `cancelRun`；`ownerTick` 调用 `Context::with_cancel`、`Manager::ID`、`Interface::Name`、`OnBecomeOwner` 与 `OnTick`；`cancelRun` 调用 `Interface::Name` 和 `CancelFunc::call`。

RustCodeGraph 对目标文件给出的精确内部调用边是 `run → ownerTick` 和 `run → cancelRun`。当前 Rust 上游由 `owner_daemon_test.rs` 的 `test_daemon` 以及 `parity_test.rs` 的时序/契约测试组成；全仓 `rg` 只在这些测试中找到 Rust `New`/`Begin`/`run` 使用，且只有该 crate 自身 manifest 声明包名，所以不能宣称 Rust 生产路径已经接线。

Go 生产对照有两条：`br/pkg/task/stream.go:1025-1038` 构造命令行 advancer daemon，按配置可周期 `ForceToBeOwner`/`RetireIfOwner`，最终同步运行返回的 loop；`pkg/domain/domain.go:981-988` 构造 TiDB checkpoint advancer daemon，并通过 domain wait group 运行 loop。这两条是业务位置证据，不是 Rust 调用边。

## 错误处理与边界

- `Begin` 只显式传播 `CampaignOwner()` 的 `DaemonError`；传播前不会调用 `OnStart`，测试 `go_rust_public_contract_matches` 覆盖此短路。
- `Interface::OnStart` 和 `OnBecomeOwner` 没有返回值；若实现 panic，锁会中毒，随后本文件所有 `lock().unwrap()` 访问也会 panic。本文件未做 panic 隔离或 poisoned-lock 恢复。
- `OnTick` 错误仅以 `warn!` 记录，循环继续。契约测试用连续错误仍累计至少两次 tick 验证这一点。
- `Duration::ZERO` 在 `OnStart` 完成后由 `assert!` 触发 panic，消息为 `non-positive interval for NewTicker`。Rust `Duration` 无法表示 Go 的负时长，因此只覆盖可表示的零值边界。
- `RetireIfOwner` 不保证立即令 `Running()` 变为 `false`；必须等待 manager 的状态变化被下一轮 tick 观察。相反，manager 报告 owner 后，`Running()` 也要到 `ownerTick` 建立 cancel 才变为 `true`。
- 根 `ctx` 取消会令 `run` 退出，但退出分支没有调用 `cancelRun`。父子 `CancellationToken` 关系仍会取消已创建的 owner 子上下文；若 loop 从未进入 owner，当然不存在任期子上下文。
- `tickInterval` 极短会增加锁竞争和回调频率；missed tick 使用 `Skip`，因此不提供“每个理论 tick 必达”的保证。

## 并发与资源生命周期

`OwnerDaemon` 控制句柄与 `DaemonLoop` 通过 `Arc<Mutex<OwnerDaemonState>>` 共享业务对象和任期 cancel。外部可并发调用 `Running`、`ForceToBeOwner`、`RetireIfOwner` 或克隆出的 manager；业务回调本身则在 state 锁内串行执行。尤其 `OnStart`、`OnBecomeOwner` 和 `OnTick` 不应在同步路径中回调需要获取同一 state 锁的操作，否则存在自死锁风险，也不应长时间阻塞，否则会延迟 `Running` 查询和失主处理。

`run` 在调用 `Manager::IsOwner` 后才获取 state 锁，缩短 manager 检查期间的持锁时间；但 owner 状态可能在检查后变化，这是轮询设计固有窗口。任期取消句柄先写入 state、后调用 `OnBecomeOwner`，确保回调开始后 `Running()` 已有一致标志。失主时 `take()` 先取得句柄再调用取消，最终状态为 `None`，重复失主 tick 不会重复执行业务取消。

业务层从 `OnBecomeOwner` 接收拥有所有权的 child `Context`，可将其移动到后台 task；失主或父 ctx 结束会唤醒这些 task。`DaemonLoop` 没有显式 ticker close：Rust future/interval 在 `run` 返回并 drop 时释放。Manager 自身的关闭不由本文件管理；Go 生产调用方分别通过 `defer ownerMgr.Close()` 或 domain 生命周期管理它。

## 与 Go 版本的对应关系

Rust 的 `OwnerDaemonState::cancel` 对应 Go `OwnerDaemon.cancel`，`New`、`Running`、`cancelRun`、`ownerTick`、`Begin`、`ForceToBeOwner` 和 `RetireIfOwner` 均保留 Go 的职责与调用顺序。Rust 将 Go 结构体中可变字段拆入 `Arc<Mutex<_>>`，因为返回的 `DaemonLoop` 需要拥有 `'static` 可 spawn 状态，而原 Go 注释只要求调用方同步访问字段。

Go `Begin` 返回 `func()`，Rust 返回 `DaemonLoop`，调用方以 `run().await` 执行；Go `time.NewTicker` 在 `Begin` 中启动计时，Rust用 `firstTick` 加 `interval_at` 保留相同首 tick deadline，并用 `MissedTickBehavior::Skip` 近似 Go ticker 丢弃积压发送的行为。Go 会对非正 interval panic；Rust只能构造非负 `Duration`，因而用零值 assert 保留可表达边界。

Go 直接依赖 `pkg/owner.Manager` 和 `context.Context`；Rust 相邻 `interface.rs` 定义精简 `Manager` trait，并以 `tokio_util::sync::CancellationToken` 包装 `Context`，避免当前独立 crate 拉入完整 etcd/kvproto 依赖。Go 每个 tick 在日志和分支中可能调用两次 `IsOwner()`；Rust先读取一次保存为 `is_owner`，让本轮日志与分支使用同一快照。

`owner_daemon_test.go::TestDaemon` 的启动、首次任期、tick、卸任、清理、再夺主与再次 tick 场景由 Rust `owner_daemon_test.rs::test_daemon` 对照覆盖；Rust `parity_test.rs` 额外验证 Campaign 错误短路、tick 错误不中断、Force/Retire 透传、ticker 从 `Begin` 起算及零 interval panic。测试使用 mock manager，不验证真实 etcd 租约或多节点故障时序。

## 扩展指南

- 新增 owner 任期级资源时，应在业务 `Interface::OnBecomeOwner` 中创建并监听传入 child context，而不是把节点本地状态塞入 `OwnerDaemon`；同步在独立测试文件中覆盖失主、再夺主和父 ctx 取消清理。
- 改变启动顺序时重点修改 `OwnerDaemon::Begin`，并同步 `parity_test.rs` 的 Campaign 失败与零 interval 场景；必须保持 Go 的 `CampaignOwner → OnStart → ticker` 可观察顺序，除非 Go 版本也发生对应变化。
- 改变 tick 行为时修改 `DaemonLoop::run`/`ownerTick`，同步验证首次 deadline、missed tick、`OnBecomeOwner` 每任期一次、`OnTick` 错误继续运行。不要把 Rust 单元测试内嵌到生产源文件；现有测试位置是 `owner_daemon_test.rs` 和 `parity_test.rs`。
- 若要接入 Rust 生产路径，需由实际上层 crate 在 Cargo manifest 增加带路径的 workspace 依赖并构造真实 `Manager`/业务 `Interface` 实现；当前仅根 workspace membership 不代表生产可达。接线还需对照 Go 的 `RunStreamAdvancer`、`initLogBackup` 以及 manager 关闭责任，避免泄漏选举后台任务。
- 若要降低锁风险，可评估把回调串行执行与状态标记拆分，但必须保证同一任期不重复 `OnBecomeOwner`、失主 cancel 不丢失、`Running` 语义不变；任何解锁后回调方案都需处理任期在窗口内变化的问题。
- 兼容性风险主要是 Go/Rust 生命周期顺序和 `Running` 语义漂移；性能风险主要来自锁内执行用户回调及过短 tick；正确性风险主要来自失主清理延迟、上下文取消遗漏和重复任期启动。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/streamhelper/daemon` 确认目标、接口、模块入口及独立测试均已索引；`node --file br/pkg/streamhelper/daemon/owner_daemon.rs` 阅读完整 232 行源码；查询/探索确认 `run → ownerTick`、`run → cancelRun` 以及测试侧 `New`/`Begin`/`run` 使用。
- Rust 源与模块：`br/pkg/streamhelper/daemon/owner_daemon.rs`、`interface.rs`、`lib.rs`。
- crate 边界：`br/pkg/streamhelper/daemon/Cargo.toml` 声明 library path、porting 元数据及 `tokio`/`tokio-util`/`tracing` 依赖；根 `Cargo.toml` 将其列为 workspace member；全仓搜索未发现其他 manifest 引用包名。
- Go 对照与生产入口：`br/pkg/streamhelper/daemon/owner_daemon.go`、`owner_daemon_test.go`、`br/pkg/task/stream.go:1017-1067`、`pkg/domain/domain.go:975-989`。
- Rust 独立测试：`br/pkg/streamhelper/daemon/owner_daemon_test.rs`、`parity_test.rs`。它们验证生命周期时序、失主取消、再夺主、错误策略、强制/卸任透传和 ticker 边界；未运行 Cargo，符合本任务纯文档约束。
- 人工复核结论：本文件存在的原因是把 owner 选举状态转换成无状态业务 daemon 的启动/任期/tick/失主取消协议；安全扩展点集中在 `Begin`、`DaemonLoop::run`、`ownerTick` 以及独立测试文件，且当前 Rust 生产接线状态已明确标为未完成而非推测已支持。
