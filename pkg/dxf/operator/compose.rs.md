# `pkg/dxf/operator/compose.rs` 逻辑说明

## 文件定位

`compose.rs` 是 Cargo 包 `astersql-dxf-operator` 的通道组合层。crate 入口 `pkg/dxf/operator/lib.rs` 公开导出 `compose`，同一 crate 的 `operator.rs` 和 `wrapper.rs` 通过这里定义的 trait 接收输入/输出通道；`pipeline.rs` 负责算子的启停顺序，不负责接线。`pkg/dxf/operator/Cargo.toml` 将该 crate 的库入口指定为 `lib.rs`，并以路径依赖 `workerpool = astersql-resourcemanager-pool-workerpool` 提供真正的通道实现；本文件本身没有 feature 条件或条件编译项。

在完整应用中，本文件位于 DXF 异步数据流的“接线”位置：调用方先构造 Source、Transform、Sink，再用 `Compose` 把相邻算子的同类型输出和输入连接起来，最后由 `AsyncPipeline` 启动它们。`pkg/dxf/operator/pipeline_test.rs::test_pipeline_async_multi_operators_without_error` 展示了 `Source -> lower -> trimmer -> counter -> collector` 的完整链。生产路径目前还在 `pkg/dxf/importinto/encode_and_sort_operator.rs::RunEncodeSortChunks` 中直接使用本模块的 `NewSimpleDataChannel`、`WithSource` 和 `DataChannel`，但该路径手工接线，并不调用 `Compose`。

## 核心职责

本文件有三项职责：

1. 用 `WithSource<T>` 与 `WithSink<T>` 描述“可以被注入输入/输出通道”的最小能力。
2. 用 `Compose<T, S, D>` 创建容量为 0 的共享通道，将同一通道的克隆交给上游 Sink 和下游 Source，从类型层面保证相邻两端传递同一种 `T`。
3. 用 `DataChannel<T>` 和 `SimpleDataChannel<T>` 封装底层 `workerpool::Channel<T>`，统一暴露取得通道句柄与显式结束数据流的操作。

它不负责变换数据、调度 worker、收集错误或决定管道的启动/关闭顺序；这些职责分别位于 `operator.rs`、底层 workerpool、各算子实现和 `pipeline.rs`。

## 主要符号

- `pub trait WithSource<T>`：公开输入端能力，唯一方法 `SetSource(&mut self, SimpleDataChannel<T>)`。实现者包括 `AsyncOperator<T, R>`、`SimpleOperator<T, R>`、`SimpleSink<R>`，以及 import-into 的 `AsyncEncodeSortOperator`。
- `pub trait WithSink<T>`：公开输出端能力，唯一方法 `SetSink(&mut self, SimpleDataChannel<T>)`。实现者包括 `AsyncOperator<T, R>`、`SimpleOperator<T, R>` 和 `SimpleDataSource<T>`。接口注释规定实现方在最后一次发送后调用 `Finish`。
- `pub fn Compose<T, S, D>(&mut S, &mut D)`：公开的泛型接线函数，约束 `S: WithSink<T>`、`D: WithSource<T>`；它创建一个无缓冲通道并注入两端，不启动算子。
- `pub trait DataChannel<T>`：公开通道操作接口；`Channel` 返回底层 `Channel<T>` 的克隆句柄，`Finish` 显式关闭共享通道。
- `pub struct SimpleDataChannel<T>`：公开包装类型，唯一字段 `channel` 私有。手写 `Clone`，克隆的是底层共享句柄而不是数据队列。
- `pub fn NewSimpleDataChannel<T>(Channel<T>)`：公开构造函数，可包装任意容量的 workerpool 通道；`Compose` 固定传入 `Channel::bounded(0)`，而其他调用者可以选择不同容量。
- `impl DataChannel<T> for SimpleDataChannel<T>`：`Channel` 克隆底层句柄，`Finish` 调用其 `close()`。

文件级 `#![allow(dead_code, non_snake_case)]` 允许 Go 移植风格的公开名称。文件中没有模块级常量、枚举、type alias 或条件编译项。

## 执行流程

典型流程如下：

1. 调用者分别构造上游和下游算子，但两者的通道字段最初为空。例如 `SimpleDataSource::target` 与 `SimpleSink::source` 都是 `Option<SimpleDataChannel<_>>`。
2. `Compose` 调用 `Channel::bounded(0)`。底层 `pkg/resourcemanager/pool/workerpool/workerpool.rs::Channel::bounded` 使用 `crossbeam_channel::bounded(0)` 建立 rendezvous 通道，并另外建立关闭信号。
3. `NewSimpleDataChannel` 包装底层通道；`Compose` 克隆包装后先调用 `op1.SetSink(...)`，再调用 `op2.SetSource(...)`。两个包装内部最终指向同一个 `Arc<ChannelInner<T>>`。
4. 管道启动后，上游通过 `DataChannel::Channel().send(value)` 发送。容量为 0 时，发送要等下游接收，形成逐项背压。
5. 下游通过 `recv()` 获取数据。上游完成或错误/取消路径调用 `Finish` 后，等待中的接收方被关闭信号唤醒；无剩余数据时 `recv()` 返回 `None`。

`pkg/dxf/operator/wrapper.rs::SimpleDataSource::Open` 在正常或异常退出时都调用输出通道的 `Finish`；`SimpleSink::Open` 持续接收至 `None`。`AsyncOperator::SetSource` 与 `SetSink` 则把克隆出的底层句柄交给 worker pool。因此 `Compose` 只建立连接，实际的数据循环和结束责任由端点实现承担。

## 数据与状态

`SimpleDataChannel<T>` 只保存一个 `Channel<T>`，没有独立缓冲、游标或错误字段。底层 `Channel<T>` 的状态位于共享的 `Arc<ChannelInner<T>>`：包含 crossbeam sender/receiver、`AtomicBool closed`、受 `Mutex` 保护的一次性关闭发送端和关闭接收端。因而：

- 克隆 `SimpleDataChannel` 或调用其 `Channel()` 都不会复制队列，而是增加共享句柄。
- `Compose` 创建的容量为 0 的通道不会积压元素；每次成功发送都对应一个并发接收。
- 通道状态只表达“开放/关闭”和待传输数据，不携带业务错误。业务错误由共享的 workerpool `Context` 记录和传播，例如 `context_error_cancels_the_pipeline_and_close_reports_it` 最终由 `AsyncPipeline::Close` 报告错误。
- `NewSimpleDataChannel` 不强制容量；例如迁移测试显式包装 `Channel::bounded(1)`，import-into 生产路径则显式选择容量 0。

## 依赖与调用关系

直接下游依赖只有 `crate::workerpool::Channel`。关键调用边为：

- `Compose -> Channel::bounded(0) -> NewSimpleDataChannel -> SimpleDataChannel::clone -> WithSink::SetSink / WithSource::SetSource`。
- `SimpleDataChannel::Channel -> Channel::clone`。
- `SimpleDataChannel::Finish -> Channel::close`。

主要上游关系为：

- `pkg/dxf/operator/wrapper.rs`：`SimpleDataSource` 实现 `WithSink`，`SimpleSink` 实现 `WithSource`，`SimpleOperator` 同时实现两者。
- `pkg/dxf/operator/operator.rs`：`AsyncOperator` 的 `SetSource`/`SetSink` 分别转交给 `WorkerPool::SetTaskReceiver`/`SetResultSender`。
- `pkg/dxf/operator/pipeline_test.rs`：连续四次调用 `Compose` 建立五段数据流。
- `pkg/dxf/operator/migration_aster_unit_test.rs`：直接验证 `Compose`、`Finish` 和完整三段管道。
- `pkg/dxf/importinto/encode_and_sort_operator.rs`：`AsyncEncodeSortOperator` 实现 `WithSource`；`RunEncodeSortChunks` 通过 `NewSimpleDataChannel(Channel::bounded(0))` 手工供给任务，完成后调用 `Finish`。

`pkg/dxf/operator/Cargo.toml` 的直接依赖还有 `crossbeam-channel`、`fail` 和 `log`，但 `compose.rs` 不直接导入它们；crossbeam 的具体行为由 workerpool crate 封装。

## 错误处理与边界

本文件所有 API 都不返回 `Result`。构造和接线阶段没有业务错误通道；缺少接线的错误由具体算子在 `Open` 时检查，例如 `SimpleDataSource::Open` 返回 `simple data source has no sink`，`SimpleSink::Open` 返回 `simple sink has no source`，`AsyncEncodeSortOperator::Open` 返回 `encode sort source is not set`。

重要边界如下：

- `Compose` 要求两个 `&mut` 端点，防止接线调用期间同时修改端点；但它不会检查旧通道，具体 `SetSource`/`SetSink` 实现可能覆盖已有连接。
- `Channel::send` 在通道已关闭或等待期间收到关闭信号时返回 `false`；本模块不把它转换成错误，调用者必须解释失败原因。
- `Channel::recv` 会先取出已经缓冲的数据，耗尽且关闭后才返回 `None`。对 `Compose` 的容量 0 通道通常没有待排空缓冲，但该语义影响用 `NewSimpleDataChannel` 包装有缓冲通道的调用者。
- Rust 底层 `Channel::close` 通过原子交换实现幂等，所以多端并发调用 `Finish` 不会重复关闭崩溃；这不同于 Go 原生 channel 的重复 `close` 会 panic。
- `T` 在本文件的 trait、结构和函数上没有 `Send`/`Sync` 限制；跨线程要求由实际算子实现施加，例如 wrapper 类型要求 `T: Send + 'static`。

## 并发与资源生命周期

容量 0 是本文件最重要的并发选择：发送方和接收方必须同时推进，否则单线程先发送再接收会阻塞。`migration_aster_unit_test.rs::compose_shares_an_unbuffered_channel_and_finish_closes_it` 因此把发送放到独立线程，并在接收后 join。

生命周期由共享所有权与显式关闭共同管理。`SimpleDataChannel::Clone` 允许上游、下游、worker 和取消监控线程持有同一通道；仅丢弃某个包装不会表达流结束，因为其他克隆仍可能存在。协议要求生产者完成最后一次发送后调用 `Finish`。wrapper 中的 Source 用 `Finish` 结束正常流，Source/Sink 的取消监控线程也会调用它以打断阻塞；import-into worker 失败时同样关闭输入通道以唤醒其他线程。

关闭不等于线程回收：`Compose` 不创建线程，也不 join 线程。`SimpleDataSource::Close`、`SimpleSink::Close`、`AsyncOperator::Close` 和 `AsyncPipeline::Close` 分别负责等待其持有的执行资源。新增端点时若可能在 `send`/`recv` 上阻塞，必须同时设计正常完成、取消和错误三条关闭路径，不能依赖 Rust drop 自动通知所有克隆。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/operator/compose.go`，总体语义一致：Go `make(chan T)` 与 Rust `Channel::bounded(0)` 都建立无缓冲连接；`Compose` 都将同一逻辑通道交给上游 Sink 和下游 Source；`Channel`/`Finish` 分别提供收发入口和结束信号。`pkg/dxf/operator/pipeline_test.go::TestPipelineAsyncMultiOperatorsWithoutError` 与 Rust 的同名语义测试都用四次 Compose 建立五段词频管道，并覆盖正常和注错路径。

需要注意的移植差异：

- Go 的 `WithSource`/`WithSink` 接受 `DataChannel[T]` 接口，可注入其他实现；Rust trait 当前固定接受具体 `SimpleDataChannel<T>`，扩展替代通道实现的能力更窄。
- Go `NewSimpleDataChannel` 返回指针，Rust 返回值；Rust 的值克隆仍通过底层 `Arc` 共享状态。
- Go `Channel()` 返回原生 `chan T`，Rust 返回可克隆的 workerpool `Channel<T>` 句柄。
- Go `Finish` 直接 `close`，重复调用会 panic；Rust `Finish` 转到幂等的 `Channel::close`，以支持取消监控、worker 失败和正常收尾可能竞争关闭。
- Go 的关闭由原生 channel 表达；Rust 为了在仍存在多个克隆句柄时模拟共享显式关闭，底层额外维护原子关闭标志和关闭信号。

这些差异是当前实现事实，不能据此假定任意 Go `DataChannel` 实现可直接替换 Rust 的 `SimpleDataChannel`。

## 扩展指南

新增相邻算子时，优先让上游实现 `WithSink<输出类型>`、下游实现 `WithSource<同一类型>`，并把通道保存或传给 worker pool；随后用 `Compose` 接线。生产者必须保证所有正常、错误和取消出口最终调用 `Finish`，消费者必须把 `recv() == None` 解释为数据流结束，并从共享 Context 单独判断是否为错误取消。

若要支持缓冲连接，应优先新增明确表达容量的构造/组合入口，而不是悄悄改变 `Compose` 的容量 0 语义；这会改变背压、内存占用和任务交错顺序，需要同步 `migration_aster_unit_test.rs` 的通道测试、`pipeline_test.rs` 的端到端测试以及 Go 对照策略。若要支持多种通道实现，则需要调整 `WithSource`/`WithSink` 当前固定的 `SimpleDataChannel<T>` 参数，并评估对象安全、泛型传播与 workerpool API 的兼容性。

最可能修改的符号是 `Compose`（接线策略）、`WithSource`/`WithSink`（端点抽象）、`DataChannel`（通道能力）和 `SimpleDataChannel`（状态包装）。风险包括：丢失无缓冲背压导致内存/调度变化；遗漏关闭造成死锁；提前关闭造成数据丢失；把业务错误错误地等同于正常 EOF；改变 Clone 共享语义导致上下游不再观察同一关闭状态。应至少同步 `pkg/dxf/operator/migration_aster_unit_test.rs`、`pkg/dxf/operator/pipeline_test.rs`，若影响直接构造通道的用法，还要同步 `pkg/dxf/importinto/encode_and_sort_operator_test.rs`。

## 验证依据

- 目标实现：`pkg/dxf/operator/compose.rs`，完整核对 `WithSource`、`WithSink`、`Compose`、`DataChannel`、`SimpleDataChannel`、`NewSimpleDataChannel` 及两个 impl。
- crate 边界：`pkg/dxf/operator/Cargo.toml`、`pkg/dxf/operator/lib.rs`；确认包名、库入口、workerpool 路径依赖、模块公开导出和测试模块装配。
- 底层语义：`pkg/resourcemanager/pool/workerpool/workerpool.rs::Channel`、`Channel::bounded`、`send`、`recv`、`close`、`is_closed`。
- Rust 直接实现/调用：`pkg/dxf/operator/operator.rs::AsyncOperator::{SetSource,SetSink}`，`pkg/dxf/operator/wrapper.rs::{SimpleDataSource,SimpleSink,SimpleOperator}`，`pkg/dxf/importinto/encode_and_sort_operator.rs::{AsyncEncodeSortOperator,RunEncodeSortChunks}`。
- Rust 测试：`pkg/dxf/operator/migration_aster_unit_test.rs::{compose_shares_an_unbuffered_channel_and_finish_closes_it,real_source_transform_and_sink_pipeline_processes_every_item,context_error_cancels_the_pipeline_and_close_reports_it}`，以及 `pkg/dxf/operator/pipeline_test.rs::test_pipeline_async_multi_operators_without_error`；import-into 的失败/取消边界由 `pkg/dxf/importinto/encode_and_sort_operator_test.rs::async_encode_operator_cancels_on_worker_error` 佐证。
- Go 对照：`pkg/dxf/operator/compose.go`、`operator.go`、`wrapper.go` 和 `pipeline_test.go::TestPipelineAsyncMultiOperatorsWithoutError`。
- RustCodeGraph：索引状态为 11,467 个文件；目标文件识别出 14 个符号。`node Compose` 给出的直接被调边为 `SetSource`、`SetSink`、`clone`、`NewSimpleDataChannel`，直接调用者为上述三项迁移测试；`node NewSimpleDataChannel` 还确认了 import-into 生产入口和相关独立测试调用者。全仓 `rg` 用于补核 Cargo、模块装配及 Rust/Go 引用集合。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按任务约束仅执行固定章节结构验证，并人工核对上述源码与调用边。
