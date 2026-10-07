# `pkg/ddl/testutil/operator.rs`

## 文件定位

本文件属于 `astersql-ddl-testutil` crate，由 `pkg/ddl/testutil/lib.rs` 公开为 `operator` 模块。它不是 DDL owner、作业调度或 schema 状态机的一部分，而是一组供数据流算子测试使用的内存 Source/Sink 辅助设施：测试可以预置输入、连接通道、异步搬运数据，再收集输出。crate 边界由 `pkg/ddl/testutil/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/ddl/testutil"` 确认。

RustCodeGraph 将该文件识别为包含 31 个符号的 Rust 源文件；仓库 Rust 调用搜索目前只找到同 crate 的 `pkg/ddl/testutil/operator_test.rs` 直接构造这些辅助类型。因此，它当前是已实现并有单元测试的测试工具，但尚未成为 Rust DDL 执行主链的组成部分。Go 对照则已在 `pkg/ddl/export_test.go::FetchChunk4Test` 中用于连接表扫描算子。

## 核心职责

1. `DataChannel<T>` 用 `Arc<Mutex<ChannelState<T>>>`、`Condvar` 和 `VecDeque<T>` 模拟 Go 无缓冲 channel：每次 `send` 必须等到队列中的当前值被接收后才能返回。
2. `OperatorTestSource<T>` 在 `Open` 创建的后台线程中按顺序发送预置值，最后关闭通道；`Close` 负责等待线程结束。
3. `OperatorTestSink<T>` 在 `Open` 创建的后台线程中持续接收，直到通道关闭，并把结果顺序追加到共享集合；`Close` 等待接收线程结束，`Collect` 返回结果快照。
4. `OperatorError` 把重复启动与后台线程 panic 暴露为确定的调用方错误，而不是让测试静默失效。

这里的“无缓冲”是行为语义而不是零容量容器：实现会短暂把一个元素放入 `VecDeque`，但发送线程在该元素被取走前不会继续。`pkg/ddl/testutil/operator_test.rs::source_uses_go_equivalent_unbuffered_channel` 用超时断言验证了该背压性质。

## 主要符号

- `OperatorError::{WorkerPanicked, AlreadyOpen}`：公开错误枚举。前者对应 `JoinHandle::join` 发现工作线程 panic，后者对应同一对象在仍持有 worker 时再次 `Open`。
- `ChannelState<T>`：私有可变状态，包含 FIFO `queue` 和单向的 `closed` 标志。
- `ChannelInner<T>`：私有同步容器，以 `Mutex` 保护状态，以 `Condvar` 协调发送者和接收者。
- `DataChannel<T>`：公开、可克隆的通道句柄；`new`/`Default` 创建未关闭的空通道，私有 `send`、`receive`、`close` 实现传输协议。克隆只增加同一个 `ChannelInner` 的 `Arc` 引用，不复制队列。
- `OperatorTestSource<T>` 与 `NewOperatorTestSource(Vec<T>)`：保存一个可选 worker、输出通道和公开的待发送列表 `toBeSent`。`SetSink` 替换输出通道，`DataChannel` 暴露其克隆句柄，`String` 固定返回 `"testSource"`。
- `OperatorTestSink<T>` 与 `NewOperatorTestSink()`：保存一个可选 worker、输入通道和 `Arc<Mutex<Vec<T>>>` 结果集。`SetSource` 替换输入通道，`DataChannel` 返回句柄，`Collect` 克隆结果，`String` 固定返回 `"testSink"`。
- 两类算子的 `Open`/`Close` 仅在 `T: Send + 'static` 时可用，因为值会跨线程移动；`Collect` 额外要求 `T: Clone`。

命名保留 Go 风格，文件通过 `#![allow(dead_code, non_snake_case, non_upper_case_globals)]` 允许这些迁移接口；这不是惯用 Rust 命名规范的新 API 示例。

## 执行流程

典型直连流程如下，证据来自 `pkg/ddl/testutil/operator_test.rs::source_uses_go_equivalent_unbuffered_channel`：

1. `NewOperatorTestSource(vec![...])` 建立带预置数据和自有空通道的 Source；`NewOperatorTestSink()` 建立带另一条自有空通道的 Sink。
2. 调用 `sink.SetSource(source.DataChannel())`，让两端共享 Source 的 `ChannelInner`。反向也可用 `source.SetSink(sink.DataChannel())` 完成同类接线，但两种方式不应同时接入不同通道。
3. `source.Open()` 通过 `mem::take` 一次性取走 `toBeSent`，启动发送线程。对每个元素，`send` 入队、通知接收者，然后在队列非空期间等待，因此在 Sink 尚未打开时 Source 线程会阻塞。
4. `sink.Open()` 启动接收线程。`receive` 取出队首后通知发送者；发送者才能继续处理下一个元素。由此保持输入顺序并提供逐元素背压。
5. Source 发送完所有值后调用 `close`，将 `closed` 置为 `true` 并唤醒等待者。Sink 会先消费已有队列项；只有“队列为空且已关闭”时 `receive` 才返回 `None` 并退出循环。
6. 调用方通过两端的 `Close` join 工作线程，之后用 `sink.Collect()` 获取克隆快照。测试还确认 Source 在 Sink 开始接收前不能完成 `Close`。

若把本辅助接入多级管线，开闭顺序必须保证每个通道最终有消费者且上游最终关闭，否则等待无缓冲握手的 Source 或等待结束信号的 Sink 会一直阻塞。

## 数据与状态

- 通道状态只有 `queue` 与 `closed`。FIFO 队列维护元素顺序；`closed` 只从 `false` 变为 `true`，代码没有重新打开通道的路径。
- `send` 没有检查 `closed`。当前 Source 的唯一发送线程会在全部发送结束后自行关闭，因此正常流程不会发生关闭后发送；如果未来扩大 `DataChannel` 的公开发送能力，必须先定义并测试关闭后发送的行为。
- Source 的 `toBeSent` 在首次成功进入 `Open` 路径时被 `mem::take` 清空，数据所有权转移到后台线程。worker join 后可以再次 `Open`，但除非调用方重新填充公开字段，否则第二次不会产生数据，只会关闭通道。
- Sink 的 `collected` 不会在再次 `Open` 前自动清空；`Collect` 返回当前完整快照。若复用 Sink，历史结果会保留。
- Source/Sink 各自初始拥有独立通道，只有显式调用 `SetSink` 或 `SetSource` 才会共享状态。忘记接线会使 Source 在自己的通道上永久等待，而 Sink 在另一通道上等待数据或关闭。

## 依赖与调用关系

下游仅使用 Rust 标准库：`VecDeque` 提供 FIFO，`Arc` 提供共享所有权，`Mutex`/`Condvar` 提供同步，`JoinHandle` 管理线程生命周期。`operator.rs` 本身没有调用 `astersql-ddl-testutil/Cargo.toml` 中列出的业务 crate。

模块入口为 `pkg/ddl/testutil/lib.rs::operator`，独立测试模块由同文件的 `#[cfg(test)] mod operator_test` 装配。RustCodeGraph 的符号查询确认 `OperatorTestSource`、`OperatorTestSink` 和 `DataChannel` 均定义于本文件；仓库范围的 Rust 文本调用搜索只发现 `pkg/ddl/testutil/operator_test.rs` 使用构造器和 `DataChannel()`。

需要特别区分两个同名抽象：本文件定义自己的 `operator::DataChannel<T>` 结构；`pkg/dxf/operator/compose.rs` 则定义 `DataChannel<T>` trait 和 `SimpleDataChannel<T>`，并由 `Compose` 通过 `WithSink`/`WithSource` 接线。当前测试 Source/Sink 没有实现 `pkg/dxf/operator/operator.rs::Operator` 或上述接线 traits，也没有使用 `astersql-dxf-operator`，所以不能直接传给 Rust `Compose`/异步管线。Go 对照类型依赖 Go `pkg/dxf/operator` 接口，并已在 `pkg/ddl/export_test.go::FetchChunk4Test` 的 `source → TableScanOperator → sink` 管线中使用；这是 Rust 后续接线应追踪的语义目标，不是当前 Rust 已具备的能力。

## 错误处理与边界

- `Open` 在 `worker.is_some()` 时立即返回 `OperatorError::AlreadyOpen`；不会启动第二条线程，也不会取走待发送值。
- `Close` 在存在 worker 时取出句柄并 join；线程 panic 映射为 `WorkerPanicked`。没有 worker 时为幂等成功 `Ok(())`。
- 所有 Mutex 获取都用 `unwrap_or_else(|poisoned| poisoned.into_inner())` 恢复被 poison 的数据，不会仅因 poison 再次 panic。代价是继续使用可能处于业务不变量未知状态的数据；当前临界区操作仅为简单队列/向量变更，风险有限。
- 错误类型没有携带 panic payload，也不区分 Source 与 Sink；需要诊断后台 panic 时应扩充错误上下文和测试，不能假定 `WorkerPanicked` 足够定位根因。
- 该实现没有取消、超时或 Drop 自动 join。未接消费者、未关闭生产端、错误接线或错误的 Close 顺序都可能造成永久等待；调用者需在测试编排层确保通道闭环。
- 当前独立测试覆盖背压、顺序收集和名称，但没有直接覆盖重复 `Open`、空输入、关闭后排空、worker panic、复用对象或错误接线。这些行为应按现有实现理解，不能视为已有回归保证。

## 并发与资源生命周期

`DataChannel` 的所有共享状态均由同一 Mutex 保护；`Condvar` 等待均放在循环中重新检查谓词，能够处理伪唤醒。`notify_all` 同时用于“有新数据”“元素已被消费”和“通道已关闭”三类状态变化，唤醒者必须重新检查 `queue`/`closed` 才决定下一步。

每个 Source 和 Sink 最多同时持有一个 `JoinHandle`。`Open` 启动 OS 线程，`Close` 是唯一显式回收点；`Close` 先 `take` 句柄再 join，因此 join 完成后对象回到可再次打开的状态。类型未实现自定义 `Drop`，若调用方直接丢弃尚在等待的对象，丢弃 `JoinHandle` 会使线程分离运行；只要其他线程仍持有通道 `Arc`，资源和等待就可能继续存在。

发送端每次最多留下一个等待消费的元素：虽然内部是 `VecDeque`，单个 Source 线程只有在队列清空后才进行下一次 `send`。若未来允许多个发送线程，它们可能在第一个消费通知后竞争并形成超过一个元素的队列，当前实现不保证多生产者下严格的 Go 无缓冲语义。因此扩展并发发送前必须重新设计会合协议。

`Collect` 在锁内克隆整个向量，快照与随后到达的数据相互独立；大量结果会产生 O(n) 时间和额外内存。通常应在 `sink.Close()` 之后调用，以得到完整、稳定的结果。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/testutil/operator.go`：

- Go `chan T` 对应 Rust `DataChannel<T>`；Go 的无缓冲发送天然阻塞，Rust 用单元素入队后等待队列清空模拟。
- Go `errgroup.Group` 对应 Rust 的单个 `Option<JoinHandle<()>>`。当前两边每个辅助对象都只启动一个 goroutine/thread；Rust 额外显式暴露 `AlreadyOpen` 和 `WorkerPanicked`。
- Go Source 接受可变参数 `toBeSent ...T`，Rust 构造器接受 `Vec<T>`；两边均按输入顺序发送并在结束后关闭通道。
- Go Source/Sink 实现 DXF operator 所需的 `SetSink`/`SetSource`、`Open`、`Close`、`String` 方法，并可由 `operator.Compose`/`NewAsyncPipeline` 使用。Rust 保留同名方法，但目前是固有方法，没有实现 Rust DXF traits，且通道类型与 `SimpleDataChannel` 不兼容。
- Go Sink 的 `collected []T` 直接返回；Rust 为跨线程共享改为 `Arc<Mutex<Vec<T>>>`，`Collect` 因而要求 `T: Clone` 并返回快照。
- Go 版本允许多次 `Open` 向同一个 `errgroup` 增加 goroutine；Rust 在 worker 尚未 join 时拒绝重复打开。这是有意的安全边界，但不是逐字等价行为。

Go 的真实使用证据是 `pkg/ddl/export_test.go::FetchChunk4Test`：它构造测试 Source/Sink，经 `operator.Compose` 连接 `ddl.NewTableScanOperator`，再执行/关闭异步管线并读取扫描结果。Rust 独立测试目前只验证 Source 与 Sink 直接相连，尚未覆盖这条 DDL 表扫描集成路径。

## 扩展指南

- 若目标是接入 Rust DXF 管线，优先修改 `OperatorTestSource`/`OperatorTestSink` 以实现 `pkg/dxf/operator/operator.rs::Operator` 及 `pkg/dxf/operator/compose.rs::{WithSink, WithSource}`，并统一使用 `SimpleDataChannel`；不要平行维护两套不兼容的公开通道协议。错误类型还需与 DXF `workerpool::Error` 的契约对齐。
- 若继续保留本地 `DataChannel`，新增功能应围绕 `send`/`receive`/`close` 的状态机修改，并在 `pkg/ddl/testutil/operator_test.rs` 增加独立测试；Rust 测试逻辑不要内嵌进生产源文件。
- 扩展生命周期时必须明确：是否允许关闭后发送、是否允许 reopen、Drop 是否取消或 join、多个生产者/消费者是否支持、错误是否传播 panic payload。每个新语义都需有不会无限阻塞的超时测试。
- 增加结果读取 API 时注意锁持有时长与克隆成本；若数据量大，可考虑在关闭后转移所有权，但需保持 Go `Collect` 的调用预期或清晰记录差异。
- 如需复刻 `FetchChunk4Test` 集成行为，应另在对应 Rust DDL 独立测试中覆盖 `source → scan operator → sink`，并验证空输入、单元素、多元素、算子错误和关闭顺序；不能用当前直连测试代替。
- 对公开 API 的命名或语义调整需同步核对 `pkg/ddl/testutil/operator.go`、`pkg/dxf/operator/{operator.rs,compose.rs}` 和 `pkg/ddl/testutil/operator_test.rs`，避免迁移接口继续漂移。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/ddl/testutil` 确认本 crate 的 Rust/Go 源与独立测试；`node --file pkg/ddl/testutil/operator.rs --offset 1 --limit 400` 读取本文件全部 268 行；`query OperatorTestSource`、`query OperatorTestSink`、`query DataChannel` 核对主要符号；`node` 读取 `pkg/dxf/operator/operator.rs` 与 `pkg/dxf/operator/compose.rs` 核对 DXF traits 和 Compose 通道契约。
- 源码与模块边界：`pkg/ddl/testutil/operator.rs`、`pkg/ddl/testutil/lib.rs`、`pkg/ddl/testutil/Cargo.toml`。
- Rust 回归证据：`pkg/ddl/testutil/operator_test.rs::source_uses_go_equivalent_unbuffered_channel`，验证未启动 Sink 时 Source 保持阻塞，启动后按序收集 `[1, 2, 3]`，并验证 `"testSink"` 名称。
- Go 对照与集成证据：`pkg/ddl/testutil/operator.go`；`pkg/ddl/export_test.go::FetchChunk4Test`。
- 仓库调用搜索：`rg` 查找 `NewOperatorTestSource`、`NewOperatorTestSink`、`OperatorTestSource`、`OperatorTestSink` 与 `DataChannel()`；Rust 侧仅发现独立测试的直接使用，Go 侧发现 `FetchChunk4Test` 的集成使用。
- 本任务为纯文档分析，按总计划不运行 Cargo；结构验收以任务指定的 11 个固定二级标题检查为准。
