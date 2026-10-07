# `br/pkg/backup/prepare_snap/stream.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-backup-prepare-snap`。包入口 [`lib.rs`](lib.rs) 以 `pub mod stream` 挂载它；[`Cargo.toml`](Cargo.toml) 将 `lib.rs` 指定为库入口，并以 `package.metadata.porting.go-package = "br/pkg/backup/prepare_snap"` 标明 Go 对照包。该 crate 的依赖表为空，文件所用的 `brpb`、`metapb`、上下文和客户端接口均由同 crate 的 [`env.rs`](env.rs) 提供的本地适配类型承载。

它位于快照备份准备链路的“每个 TiKV store 一条 PrepareSnapshot 双向流”这一层：上游 [`prepare.rs`](prepare.rs) 的 `Preparer` 负责枚举 store、按 leader 聚合 region 和维护整体状态机；本文件负责单条 store 流的初始化、请求转发、响应事件化、lease 续约与关闭。它不是进程入口，也不负责 region 覆盖/空洞判断。

直接 Go 原型是 [`stream.go`](stream.go)；Rust 文件已有 `// Copyright 2026 AsterSQL.`，表明该移植文件已进入 AsterSQL 处理范围。

## 核心职责

1. 用 `prepareStream` 封装一个 store 的 `PrepareClient`，在 `InitConn` 时建立初始 lease，并启动后台收包/续约循环。
2. 用 `AsyncStreamBy` 把可能阻塞的 `PrepareClient::Recv` 放入独立线程，将结果送入有界 channel，使控制线程可以同时处理停止信号、续约节拍和取消。
3. 把 `PrepareSnapshotBackupResponse` 归一化成内部 `event`：`WaitApplyDone` 交给 `Preparer` 更新 region 状态，失效 lease 和未知响应变成不可恢复的 misc 错误事件，有效续约确认不向上游制造事件。
4. 在 `Finalize` 中停止后台循环、发送 `Finish`、排空剩余响应，并把后台错误或关闭阶段错误返回给上层。

边界由 `PrepareClient` trait 刻意隔开：本文件只依赖并发安全的 `Send`/`Recv`，不直接创建真实 gRPC 连接；连接建立和测试替身均由 `Env`/`PrepareClient` 的实现负责。

## 主要符号

- `eventType = i32`、`eventMiscErr = 0`、`eventWaitApplyDone = 1`：与 Go `iota` 顺序对齐的内部事件类别。
- `event { ty, storeID, err, region }`：从流层送往 `Preparer` 的载体。`WaitApplyDone` 通常携带 `region` 和可选 region 错误；misc 路径携带流级错误。其 `Display` 输出类型、store、错误和 region，便于诊断。
- `StreamResult<T> { Err, Item }`：对照 Go `utils.Result[T]` 的接收结果信封。生成器失败时 `Err = Some(...)` 且 `Item = T::default()`。
- `AsyncStreamBy<T, F>(generator) -> Receiver<StreamResult<T>>`：启动一个未保存句柄的后台线程，反复调用生成器；成功项持续发送，首个错误发送后终止，接收端消失时也终止。内部 `sync_channel(64)` 提供背压。
- `SharedStreamRx`：`Arc<Mutex<Receiver<StreamResult<PrepareSnapshotBackupResponse>>>>`，使 `client_loop` 与关闭排空路径串行共享唯一接收端。
- `prepareStream`：每 store 状态对象。公开字段保存 `storeID`、可选客户端、lease 时长和事件发送端；私有字段保存共享响应端、后台线程句柄和原子停止位。
- `prepareStream::new`：构造尚未激活的流；此时 `cli`、响应端和后台句柄均为空。
- `InitConn` / `GoLeaseLoop`：安装客户端，清除停止位；同步发送首个 `UpdateLease` 并要求首包必须是 `UpdateLeaseResult`，随后才启动异步接收和 `client_loop`。
- `Send`：转发上游聚合好的请求；未初始化时返回 `prepare stream client not initialized`。
- `Finalize` / `stopClientLoop`：完成资源收尾和错误汇聚。
- `client_loop`：后台响应分发与周期续约循环。
- `onResponse`、`on_response_static`：分别供关闭排空路径和后台循环使用的响应处理器，二者都委托 `convert_to_event`。
- `convert_to_event`：协议响应到内部事件的唯一映射点。

## 执行流程

初始化与正常处理流程如下：

1. [`prepare.rs`](prepare.rs) 的 `Preparer::PrepareConnections` 或 `streamOf` 调用 `createAndCacheStream`，创建 `prepareStream::new(storeID, event_tx, LeaseDuration)`。
2. `InitConn` 保存 `Arc<dyn PrepareClient>`，然后进入 `GoLeaseLoop`。后者先同步发送 `UpdateLease { LeaseInSeconds: dur.as_secs() }`，再同步 `Recv` 一次；只有收到 `UpdateLeaseResult` 才算握手成功。此时并不检查首包的 `LastLeaseIsValid`，只检查响应类型。
3. 握手成功后，`AsyncStreamBy` 在独立线程持续执行 `PrepareClient::Recv`，结果经容量 64 的 channel 进入 `SharedStreamRx`；另一个线程运行 `client_loop`。
4. [`prepare.rs`](prepare.rs) 的 `sendWaitApply` 从缓存中取流并调用 `Send`，把按 leader store 聚合的 `WaitApply` 请求发给 TiKV。
5. `client_loop` 以 `min(50ms, max(dur/4, 1ms))` 的间隔轮询响应。收到响应时，`on_response_static` 调用 `convert_to_event`；需投递的事件进入 `Preparer.event_tx`，随后由 `Preparer::WaitAndHandleNextEvent`/`onEvent` 消费。
6. 无响应且已经过 `dur/4` 时，循环发送新的 `UpdateLease`。发送成功更新 `last_success`；失败且距上次成功超过整个 lease 时长，则投递 `eventMiscErr` 并让后台线程以错误退出。

关闭流程由 `Preparer::Finalize` 并行调用每个 store 的 `prepareStream::Finalize`：

1. `stopClientLoop` 先以 `SeqCst` 设置 `stop_bg`，再 join `client_loop`，保存其错误或 panic 信息。
2. 向客户端发送 `Finish`，要求对端结束 lease 并关闭流。
3. 从共享响应 channel 以 50ms 超时循环排空；响应仍按正常规则转为事件。遇到 EOF、channel 断开，或后台已失败后的静默超时则结束排空；上下文取消会立即返回取消错误。
4. 若排空本身没有先失败，则最后返回之前保存的后台错误。上层 `Preparer::Finalize` 同时持续消费事件，避免同步 channel 因无人读取而阻塞。

## 数据与状态

`prepareStream` 存在三个阶段：

- 未初始化：只有 store、lease 配置和 `output`；`Send`/`Finalize` 会明确报客户端或服务端流未初始化。
- 活跃：`cli`、`shared_server_stream`、`client_loop_handle` 均已设置，`stop_bg = false`。请求可由上游发送，响应接收器由后台循环持锁短暂消费。
- 正在/已经关闭：`stop_bg = true`，线程句柄经 `take()` 只能 join 一次；随后发 `Finish` 并排空。实现没有单独的 finalized 枚举或幂等保护，调用者应按 `Preparer` 生命周期只关闭一次。

协议数据的不变量由 `convert_to_event` 表达：

- `WaitApplyDone` 原样克隆 `Region`，并用 [`errors.rs`](errors.rs) 的 `convertErr` 转换协议错误。
- `UpdateLeaseResult && !LastLeaseIsValid` 产生 `leaseExpired()`；有效 lease 响应被消费但不投递。
- 其他响应类型产生以 `unsupported()` 为 cause 的 misc 错误。

`last_success` 初始被设为约一年前，因此第一次周期续约若发送失败，会立刻满足“超过 lease 时长”的失败条件；成功发送一次后才以当前时刻作为后续容错窗口。lease 秒数通过 `Duration::as_secs()` 下取整，亚秒配置会以 0 秒写入协议，不应作为生产配置使用。

## 依赖与调用关系

上游调用链（RustCodeGraph 与源码共同核对）为：

`Preparer::PrepareConnections/streamOf` → `createAndCacheStream` → `prepareStream::new` → `InitConn` → `GoLeaseLoop` → `AsyncStreamBy + client_loop`。

运行时请求链为：

`Preparer::workOnPendingRanges` → `sendWaitApply` → `prepareStream::Send` → `PrepareClient::Send`。

响应链为：

`PrepareClient::Recv` → `AsyncStreamBy` → `client_loop` → `on_response_static` → `convert_to_event` → `SyncSender<event>` → `Preparer::WaitAndHandleNextEvent/onEvent`。

关闭链为：

`Preparer::Finalize` → 每 store 的 `prepareStream::Finalize` → `stopClientLoop`，而 `Preparer::Finalize` 在各关闭线程完成前并行排空总事件 channel。

直接下游依赖包括：[`env.rs`](env.rs) 的 `Context`、`PrepareClient`、`brpb`、`metapb`；[`errors.rs`](errors.rs) 的 `Error`、`Result`、`convertErr`、`leaseExpired`、`unsupported`；标准库线程、原子变量、`mpsc`、`Arc<Mutex<_>>` 和时间类型。`lib.rs` 公开模块本身，但包根只 re-export `env`、错误和 `prepare` API，没有把这些小写 stream 类型直接提升到包根。

## 错误处理与边界

- 初始 `UpdateLease` 的发送错误被注解为 `failed to initialize the lease`，首包接收错误被注解为 `failed to recv the initialize lease result`；首包类型不符直接拒绝激活。
- `AsyncStreamBy` 只传播首个生成器错误；发送错误项后线程退出。若接收端已消失，错误项发送失败也直接退出。
- 后台收到流错误时添加 `failed to recv from the stream`，向上游投递同一错误的 clone，并同时把错误留在线程返回值中。因此驱动阶段和 Finalize 阶段都可能观察到该故障。
- `output.send(...)` 的断开错误被刻意忽略；但它是同步发送，接收方仍存在而缓冲区满时会阻塞。正常关闭依赖 `Preparer::Finalize` 同步排空事件。
- `stopClientLoop` 将 EOF（`is_eof()` 或消息恰为 `EOF`）和响应 channel 断开视为正常结束；其他排空错误立即返回。若后台已经失败，后续一次 50ms 静默超时也可视作关闭，再返回保存的后台错误。
- 上下文只在关闭排空循环中显式检查；`InitConn` 的 `_ctx` 未使用，后台循环使用独立原子停止位。阻塞中的底层 `PrepareClient::Recv` 是否能被取消取决于具体实现和对端在 `Finish` 后的行为。
- `Mutex::lock(...).expect(...)` 和线程 join panic 会走不同策略：共享接收器锁中毒会 panic，`client_loop` 自身 panic 则在关闭时转成 `client loop panicked`。
- Rust 的响应类型不是指针，因此没有 Go `convertToEvent(nil)` 的“忽略 nil 响应”分支；无法构造的 nil 状态由类型系统排除。

## 并发与资源生命周期

每条活跃流至少涉及三个执行主体：调用/状态机线程、`AsyncStreamBy` 的阻塞接收线程、`client_loop` 的响应分发/续约线程。`PrepareClient: Send + Sync` 且方法接收 `&self`，允许发送和接收并行，具体实现负责必要的内部同步。

`Receiver` 本身不能复制，因此 `SharedStreamRx` 用 `Arc<Mutex<_>>` 在后台循环与 Finalize 排空之间转移消费权。每次只在 `recv_timeout` 调用期间持锁，超时后释放。`stop_bg` 使用 `SeqCst`，确保 Finalize 发出停止后，循环能在下一轮检查中退出；最坏等待受当前 `recv_timeout(poll)` 限制，而不是整个 lease 时长。

关闭顺序是关键不变量：先置停止位并 join `client_loop`，避免它继续与关闭路径竞争接收器或发送续约；再发 `Finish`，由仍存活的 `AsyncStreamBy` 接收线程读取对端尾部响应并关闭 channel。`AsyncStreamBy` 的线程句柄没有保存，不能显式 join；其退出依赖生成器报错、接收端断开或 `Finish` 使底层 `Recv` 结束。

相关独立 Rust 测试位于 [`prepare_test.rs`](prepare_test.rs) 和 [`parity_test.rs`](parity_test.rs)：`test_lease_timeout` 验证 Finalize 能表面过期 lease，`test_lease_timeout_while_taking_snapshot` 验证运行中失效 lease 进入主事件循环，`test_many_messages_when_finalizing` 验证连接错误和大量延迟响应下关闭仍上浮错误，`contract_resource_finalize_clears_lease` 验证正常 Finalize 清除 lease。测试逻辑与生产源码分文件，扩展时应继续保持该结构。

## 与 Go 版本的对应关系

Rust 实现逐项对应 [`stream.go`](stream.go) 的 `eventType`、`event`、`prepareStream`、`InitConn`、`Finalize`、`GoLeaseLoop`、`onResponse`、`stopClientLoop`、`clientLoop`、`sendErr` 和 `convertToEvent`。事件编号、初始 lease 握手、每 `dur/4` 续约、失败超过 lease 后报错、`WaitApplyDone`/`UpdateLeaseResult` 的映射以及 `Finish` 后排空的意图均保持一致。

主要机制差异如下：

- Go 使用 channel、`context.CancelFunc`、`errgroup.Group` 和 ticker；Rust 使用 `sync_channel`、`AtomicBool`、`JoinHandle` 与 `Instant`/`recv_timeout`。
- Go 的 `utils.AsyncStreamBy` 被本文件的泛型 `AsyncStreamBy` 就地复刻，容量固定为 64；上游事件 channel 在 `prepare.rs::New` 中容量为 128。
- Go 在 `stopClientLoop` 中先取消，再发 `Finish`、排空，最后 `errgroup.Wait()`；Rust 先停止并 join `client_loop`，再发 `Finish`、排空，并在末尾返回保存的 loop 错误。二者目标都是关闭返回前不再向 `output` 追加消息，但 Rust 顺序明确消除了关闭阶段的接收竞争。
- Go 可接收 nil protobuf 指针并忽略；Rust 响应按值传递，不存在 nil 消息。
- Go 带有结构化日志；Rust 当前主要通过带上下文的错误和 `Display` 提供诊断，没有等价日志调用。

Go 回归测试 [`prepare_test.go`](prepare_test.go) 的 `TestLeaseTimeout`、`TestLeaseTimeoutWhileTakingSnapshot`、`TestHooks`、`TestManyMessagesWhenFinalizing` 分别有对应 Rust 场景。Rust 测试对取消竞态、错误文案及清 lease 又给出了更显式的断言。

## 扩展指南

- 新增协议响应类型时，应首先扩展 `convert_to_event`，同时决定是内部静默控制响应还是必须交给 `Preparer` 的事件；若新增事件类型，还要同步修改 [`prepare.rs`](prepare.rs) 的 `Preparer::onEvent`，并同时更新 Go 对照或明确记录差异。
- 修改 lease 策略时，重点审查 `GoLeaseLoop` 的首包契约、`client_loop` 的 `tick/poll/last_success` 和 `stopClientLoop` 的停止顺序。需要补充 [`prepare_test.rs`](prepare_test.rs) 中首次续约失败、连续失败后恢复、极短/零时长配置和取消时阻塞 `Recv` 等独立测试。
- 修改 channel 容量或投递策略时，要评估背压：`AsyncStreamBy` 容量 64、上游事件 channel 容量 128，且 `SyncSender::send` 会阻塞。必须保留 Finalize 期间上层并行排空的契约，并用大量消息回归场景验证无死锁和无事件丢失。
- 需要支持重复初始化或重复 Finalize 时，不能只在现有方法外包一层；应为 `prepareStream` 引入明确生命周期状态，定义旧接收线程如何结束、句柄如何回收以及第二次 `Finish` 的协议语义。
- 接入真实 gRPC 行为应放在 `env.rs` 的 `PrepareClient` 实现或环境适配层，不应把具体传输依赖塞进本文件；Cargo 当前无外部依赖，这一边界也是 arm64 本地 stub 策略的一部分。
- 所有测试继续放在同目录独立测试文件，优先扩展 `prepare_test.rs` 的行为场景和 `parity_test.rs` 的 Go/Rust 契约，不把测试内嵌进 `stream.rs`。

兼容风险主要是事件编号、错误文案、首包类型和 `LastLeaseIsValid` 语义；正确性风险主要是关闭次序、错误重复/丢失和同步 channel 死锁；性能风险主要是每 store 两个线程、50ms 轮询以及固定 channel 容量。改动这些位置时应同时观察多 store 和大量 region 场景。

## 验证依据

- Rust 生产源码：[`stream.rs`](stream.rs) 的 `AsyncStreamBy`、`prepareStream`、`InitConn`、`GoLeaseLoop`、`stopClientLoop`、`client_loop`、`convert_to_event`；[`prepare.rs`](prepare.rs) 的 `createAndCacheStream`、`sendWaitApply`、`onEvent`、`Finalize`；[`env.rs`](env.rs) 的 `PrepareClient` trait；[`errors.rs`](errors.rs) 的错误转换/构造器；[`lib.rs`](lib.rs) 的模块装配。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的包名、`lib.path`、Go porting 元数据和空依赖表。
- Go 对照：[`stream.go`](stream.go) 全部 221 行，以及 [`prepare_test.go`](prepare_test.go) 的 `TestLeaseTimeout`、`TestLeaseTimeoutWhileTakingSnapshot`、`TestHooks`、`TestManyMessagesWhenFinalizing`。
- Rust 独立测试：[`prepare_test.rs`](prepare_test.rs) 的 `MockStore::Send/Recv`、`test_lease_timeout`、`test_lease_timeout_while_taking_snapshot`、`test_many_messages_when_finalizing`；[`parity_test.rs`](parity_test.rs) 的 `contract_normal_basic_prepare`、`contract_resource_finalize_clears_lease`。
- RustCodeGraph：索引状态为 11,467 文件（其中 Rust 7,032）；目标目录共识别 14 个 Go/Rust 文件。查询确认 `stream.rs` 的 446 行源码和 `prepare.rs` 主链，并给出 `createAndCacheStream`、`streamOf`、`sendWaitApply`、`Finalize` 及 lease 相关测试的调用/引用关系。精确 method callers 查询未返回 Rust impl method 边，因此这些方法级调用边又由 RustCodeGraph 的文件节点源码交叉核对。
- 本任务只新增说明文档，不修改运行时代码；按计划不运行 Cargo。交付前以任务指定命令确认本文恰有 11 个固定二级章节，并人工复核链接、符号名、Go/Rust 差异和扩展风险。
