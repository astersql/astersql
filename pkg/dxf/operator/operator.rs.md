# `pkg/dxf/operator/operator.rs`

## 文件定位

本文件是 `astersql-dxf-operator` crate 的基础算子层，源码入口为 [`operator.rs`](operator.rs)，crate 边界由 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs) 确认。它位于通道组合层 [`compose.rs`](compose.rs) 与管道编排层 [`pipeline.rs`](pipeline.rs) 之间：前者把相邻算子的输出、输入接到同一条通道，后者只依赖本文件的 `Operator`/`TunableOperator` 接口管理生命周期。

文件同时提供一个通用的 `AsyncOperator<T, R>` 实现，把输入任务 `T` 交给 `astersql-resourcemanager-pool-workerpool` 的真实 `WorkerPool<T, R>` 并产出 `R`。当前仓库中它最直接的生产使用者是 [`wrapper.rs`](wrapper.rs) 的 `SimpleOperator<T, R>`；更专用的 DXF 算子也实现同一个 `Operator` trait，例如 [`../importinto/encode_and_sort_operator.rs`](../importinto/encode_and_sort_operator.rs) 中的 `AsyncEncodeSortOperator`，但不复用本文件的 `AsyncOperator`。

## 核心职责

- `Operator` 统一算子的 `Open`、`Close` 和 `String` 生命周期/诊断接口，使 [`pipeline.rs`](pipeline.rs) 能以 `Box<dyn Operator>` 保存异构算子。
- `TunableOperator` 抽象运行期 worker 数调整与容量查询；`Operator::as_tunable_operator(_mut)` 为 Rust trait object 提供显式的可调能力探测。
- `AsyncOperator<T, R>` 保存共享取消/错误上下文和 worker 池，并把 `Open`/`Close`、通道绑定、调节操作转发给池。
- `NewAsyncOperatorWithTransform` 将一个线程安全的 `Fn(T) -> R` 转换闭包包装成 worker 工厂，避免每个 worker 各自持有不可共享的闭包副本。
- 私有 `asyncWorker<T, R>` 将单项处理限定为“调用 transform，再调用结果发送回调”；线程循环、panic 恢复、取消和通道关闭均交由 workerpool 实现。

## 主要符号

- `pub trait Operator`：公开基础接口。`Open`/`Close` 返回 workerpool 的 `Error`；两个 `as_tunable_operator` 默认返回 `None`，实现者必须显式覆盖才会被管道识别为可调算子。
- `pub trait TunableOperator`：公开调节接口。`TuneWorkerPoolSize(workerNum, wait)` 改变目标并发度；`GetWorkerPoolSize` 返回配置容量，而非当前正在执行任务的数量。
- `pub struct AsyncOperator<T, R>`：公开通用异步算子，字段保持私有。`T` 必须实现 `TaskMayPanic + Send + 'static`，以供 workerpool 在任务 panic 时构造错误；`R` 必须为 `Send + 'static`。
- `pub fn NewAsyncOperatorWithTransform`：便捷构造器。调用 `WorkerPool::NewWorkerPool(name, (), workerNum, newAsyncWorkerCtor(transform))` 后委托给 `NewAsyncOperator`。
- `pub fn NewAsyncOperator`：接收已配置的池和 `workerpool::Context`，适用于调用者需要先自定义池时。
- `impl Operator for AsyncOperator`：`Open` 调用 `pool.Start(ctx.clone())`，`Close` 调用 `pool.Release()`，`String` 用 `type_name::<T/R>()` 生成 `AsyncOp[T, R]`。
- `impl WithSource<T>` / `impl WithSink<R>`：分别把 `SimpleDataChannel` 的底层 channel 交给 `SetTaskReceiver` / `SetResultSender`；这些绑定应在 `Open` 之前完成。
- `impl TunableOperator for AsyncOperator`：直接转发到 `WorkerPool::Tune` 和 `WorkerPool::Cap`。
- `struct asyncWorker<T, R>` 与 `fn newAsyncWorkerCtor`：私有 worker/工厂。闭包存于 `Arc<dyn Fn(T) -> R + Send + Sync>`，`PhantomData<fn(T)>` 只表达泛型类型关系，不保存任务值。
- `impl Worker<T, R> for asyncWorker`：`HandleTask` 同步执行 transform 并发送一个结果；`Close` 没有本地资源，恒返回成功。

本文件没有模块级常量、枚举、条件编译项或可变静态状态。

## 执行流程

1. [`wrapper.rs`](wrapper.rs) 的 `newSimpleOperator` 调用 `NewAsyncOperatorWithTransform(ctx, "simple", concurrency, transform)`；也可由外部调用者先构造 `WorkerPool` 再调用 `NewAsyncOperator`。
2. `newAsyncWorkerCtor` 把 transform 放入 `Arc`，返回可重复调用的 worker 工厂；每次调用工厂都创建一个持有同一闭包 `Arc` 的 `asyncWorker`。
3. 管道装配阶段，`Compose` 创建容量为 0 的共享通道，先调用上游 `WithSink::SetSink`，再调用下游 `WithSource::SetSource`。对 `AsyncOperator` 而言，这会在池启动前设置结果发送端或任务接收端。
4. `AsyncPipeline::Execute` 按算子顺序调用 `Open`。`AsyncOperator::Open` 启动 workerpool；底层池为每个 worker 建线程，并在任务、缩容、通道关闭和 context 取消之间等待。
5. workerpool 收到一个 `T` 后调用 `asyncWorker::HandleTask`；后者同步执行 transform，并立刻通过 `send` 回调提交一个 `R`。无缓冲管道会使发送与下游接收形成背压。
6. 动态调节经 `TuneWorkerPoolSize` 进入 `WorkerPool::Tune`：扩容会新建 worker；缩容会发退出请求，`wait=true` 时等待被移除 worker 的 `Close` 完成。非正目标值由底层池钳制为 1。
7. 管道关闭时调用 `AsyncOperator::Close`。`WorkerPool::Release` join 全部 worker，然后取消子 context 并关闭结果通道；因此输入通道必须已关闭或 context 已取消，否则仍在等待输入的 worker 无法退出，`Close` 也会继续等待。

## 数据与状态

`AsyncOperator` 只拥有两个状态字段：`ctx` 是可克隆的共享错误/取消上下文，`pool` 拥有 worker 数、线程句柄、输入/输出通道及调节状态。算子本身不缓存任务或结果，也不维护处理顺序。

transform 以 `Arc` 共享给所有 worker，因此它必须实现 `Send + Sync + 'static`。闭包若要维护可变共享状态，需要调用者自行使用 `Mutex`、原子类型或其他同步机制；[`pipeline_test.rs`](pipeline_test.rs) 的计数 transform 就使用 `Arc<Mutex<HashMap<...>>>`。多个 worker 可并发完成，输出顺序没有稳定保证；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的真实流水线测试在断言前显式排序结果。

`GetWorkerPoolSize` 返回 `WorkerPool::Cap()` 的配置值，不表示 `runningTask`，也不保证调节瞬间已有同等数量的任务在运行。`String` 使用 Rust 完整类型名，属于诊断文本，不应作为稳定序列化格式。

## 依赖与调用关系

上游调用链的直接证据是 RustCodeGraph 的 `NewAsyncOperatorWithTransform` 节点：Rust 调用者为 [`wrapper.rs`](wrapper.rs) 的 `newSimpleOperator`；该包装器继续把 `Open`、`Close`、通道绑定和调节全部委托给内部 `AsyncOperator`。[`pipeline.rs`](pipeline.rs) 通过 `Operator` trait 调用生命周期，并通过 `as_tunable_operator_mut` 取得固定四算子拓扑中的 reader/writer 调节接口。

下游依赖分为两组：

- `crate::compose::{DataChannel, SimpleDataChannel, WithSource, WithSink}` 提供类型化通道及接线接口。
- `crate::workerpool::{Context, Error, TaskMayPanic, Worker, WorkerPool}`（由 [`lib.rs`](lib.rs) 再导出 Cargo 依赖 `astersql-resourcemanager-pool-workerpool`）承担线程、任务接收、结果发送、panic/错误汇报、取消、调节和释放。

[`Cargo.toml`](Cargo.toml) 将本 crate 的 Go 对照包声明为 `pkg/dxf/operator`，没有 feature 声明；本文件没有直接依赖 crate 的 `crossbeam-channel`、`fail` 或 `log`，这些细节位于 compose/workerpool 等下层实现。

## 错误处理与边界

- `AsyncOperator::Open` 和 `Close` 当前在完成转发后总返回 `Ok(())`；`WorkerPool::Start` 的重复启动是断言失败，线程创建失败也会 panic，并不会转换成本文件的 `Error`。
- transform 的签名是 `Fn(T) -> R`，无法直接返回业务错误。需要报告运行期错误时，现有调用方式是捕获共享 `Context` 并调用 `OnError`；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 [`pipeline_test.rs`](pipeline_test.rs) 都覆盖了这种路径。
- transform panic 由 workerpool 的 `catch_unwind` 边界捕获，并利用 `T::RecoverArgs` 产生错误写回调用方 context；发生在线程框架之外的 panic 则在 `Release` join 时转成通用 context 错误。
- 结果发送受 worker 子 context 取消控制；若取消或通道关闭先发生，结果可能不再发送。`asyncWorker::HandleTask` 本身仍返回 `Ok(())`，发送结果由池的回调负责。
- `SetSource`/`SetSink` 没有在类型层阻止启动后重绑，但底层文档要求在 `Start` 前设置；安全扩展不应依赖运行期换通道。
- `Close` 依赖输入结束或取消使 worker 循环退出；调用顺序不当可能阻塞。通道终结责任由流水线端点与 workerpool 生命周期共同承担，而不是本文件自动关闭输入。
- `Operator` 的默认 downcast 钩子为 `None`。只实现 `TunableOperator` 而忘记覆盖钩子时，`GetReaderAndWriter` 仍无法发现该能力。

## 并发与资源生命周期

每个 `AsyncOperator` 拥有一个 `WorkerPool`。`Open` 只能成功执行一次；底层池按容量创建 OS 线程，每个线程拥有独立 `asyncWorker`，但共享同一个 transform `Arc`、输入通道、可选结果通道和 context。任务值在线程间移动，要求 `T: Send`；结果通过通道移动，要求 `R: Send`。

无缓冲 `Compose` 通道提供自然背压：worker 产生结果后可能等待下游接收。并发 worker 会竞争同一输入队列，调度与输出顺序不确定。缩容的 `wait` 仅决定是否等待被要求退出的 worker 完成 `Worker::Close`；它不是等待整个池所有任务完成的开关。

正常生命周期是“先接线，后 `Open`，上游最终关闭输入，再 `Close`”。`Release` join 所有线程、取消子 context、关闭结果通道；`WorkerPool` 的 `Drop` 还提供兜底取消/join/关闭，但不应替代显式管道关闭。`asyncWorker::Close` 当前无操作，因为 transform 没有专门的关闭协议；若未来 transform 持有需显式释放的资源，应改用自定义 `Worker` 和 `NewAsyncOperator`，或扩展 worker 抽象并补独立测试。

## 与 Go 版本的对应关系

直接对照文件是 [`operator.go`](operator.go)，Rust 保留了 `Operator`、`TunableOperator`、`AsyncOperator`、两个构造器、通道绑定、动态调节及 `asyncWorker` 的一一对应职责。Go 的 `Open`/`Close` 同样只启动/释放池并返回 `nil`，`HandleTask` 同样执行一次 transform、发送一次结果并返回 `nil`。

需要注意的语义/表示差异：

- Go 通过运行期类型断言 `op.(TunableOperator)` 探测调节能力；Rust 无法直接在 `dyn Operator` 上做该断言，因此增加 `as_tunable_operator` 和 `as_tunable_operator_mut`，默认 `None`，可调实现显式返回 `Some(self)`。
- Go 构造池时传 `util.DistTask` component；Rust 传 `()`。当前 Rust `WorkerPool::NewWorkerPool` 将 component 参数命名为 `_component` 并忽略，因此现阶段不改变执行行为，但若未来 workerpool 使用 component 做资源归类，这里必须同步调整。
- Go 构造器返回指针，Rust 按值返回 `AsyncOperator`，所有需要可变操作的方法通过 `&mut self` 表达独占访问。
- Go `String` 通过零值和 `%T` 输出类型，Rust 使用 `std::any::type_name`；两者目的相同，但字符串格式只保证当前测试所覆盖的 Rust 表示。
- Rust 的泛型约束显式加入 `Send + 'static`，transform 还要求 `Send + Sync + 'static`，这是跨线程安全的编译期约束；Go 版本没有对应的类型系统表达。

相关 Go 流水线行为由 [`pipeline_test.go`](pipeline_test.go) 覆盖；Rust 对照验证位于独立的 [`pipeline_test.rs`](pipeline_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，测试逻辑未嵌入生产文件。

## 扩展指南

- 新增通用 transform 算子时，优先通过 [`wrapper.rs`](wrapper.rs) 的 `newSimpleOperator` 使用本实现，并在 `Compose` 阶段完成上下游接线；不要复制 worker 线程循环。
- 需要自定义每-worker 状态、可失败处理或显式清理时，实现 workerpool 的 `Worker<T, R>`、配置 `WorkerPool`，再调用 `NewAsyncOperator`。若错误是业务结果的一部分，可令 `R` 承载 `Result`；若要取消整条管道，则沿用共享 `Context::OnError` 契约。
- 新增可调 `Operator` 实现时，必须同时实现 `TunableOperator` 和两个 `as_tunable_operator` 钩子，并同步验证 [`pipeline.rs`](pipeline.rs) 的 `GetReaderAndWriter` 访问路径。
- 改动 `Open`/`Close` 时要保持 workerpool 的单次启动、输入终结、join、context 取消和结果通道关闭顺序；尤其避免在仍可能阻塞接收时直接 join。
- 改动 transform 并发约束或输出顺序时，要评估闭包共享状态的同步成本、无缓冲通道的背压以及乱序兼容性。
- 测试应继续放在独立文件：通用算子/生命周期回归放入 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，多段流水线与诊断字符串放入 [`pipeline_test.rs`](pipeline_test.rs)，并与 [`operator.go`](operator.go) 及 [`pipeline_test.go`](pipeline_test.go) 的行为保持一致。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/dxf/operator` 确认目标、Go 对照、包装器及两份独立 Rust 测试均已索引。
- RustCodeGraph `node --file pkg/dxf/operator/operator.rs`：完整读取 208 行，核对全部 trait、struct、函数、impl、泛型约束和无条件编译事实。
- RustCodeGraph `node NewAsyncOperatorWithTransform`：确认它调用 `newAsyncWorkerCtor` 与 `NewAsyncOperator`，并由 Rust `wrapper.rs::newSimpleOperator` 调用。
- RustCodeGraph 文件节点：读取 [`compose.rs`](compose.rs)、[`pipeline.rs`](pipeline.rs)、[`wrapper.rs`](wrapper.rs) 及下层 [`../../resourcemanager/pool/workerpool/workerpool.rs`](../../resourcemanager/pool/workerpool/workerpool.rs)，核对接线、生命周期、线程循环、错误恢复、调节与释放行为。
- crate/移植证据：读取 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs) 和 Go 对照 [`operator.go`](operator.go)，核对 crate 名、模块导出、workerpool 依赖和 Go/Rust 差异。
- 测试证据：读取 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的可调接口、真实 Source→Transform→Sink、取消测试，以及 [`pipeline_test.rs`](pipeline_test.rs) 的多算子、并发共享状态、错误注入和字符串测试；同目录不存在 `operator_test.rs`/`operator_test.go`，Go 侧最近的行为测试为 [`pipeline_test.go`](pipeline_test.go)。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令验证文档恰含 11 个固定二级章节，并人工复核本文每项行为结论均可回溯到上述源码、调用边或测试。
