# `pkg/executor/aggregate/agg_hash_base_worker.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-executor-aggregate`；该包以同目录的 `lib.rs` 为 crate 根，并通过 `pub mod agg_hash_base_worker` 公开本模块。它定义 Hash 聚合 worker 可共享的最小状态对象 `BaseHashAggWorker`，意图对应 Go 版本中由 `HashAggPartialWorker` 与 `HashAggFinalWorker` 匿名嵌入的 `baseHashAggWorker`。

对应生产源文件：[`agg_hash_base_worker.rs`](./agg_hash_base_worker.rs)。

当前 Rust 接线状态需要与这一设计意图区分：全仓 Rust 引用仅见于 `agg_hash_base_worker_test.rs`，生产态的 `agg_hash_partial_worker.rs`、`agg_hash_final_worker.rs` 和 `agg_hash_executor.rs` 各自持有聚合状态，并未构造或嵌入 `BaseHashAggWorker`。因此本文件目前是已公开、已单测的公共状态组件，不是 Rust Hash 聚合执行主链上的实际 worker 基类。

## 核心职责

- 通过 `BaseHashAggWorker::new` 原样保存结束标志、聚合描述列表与输出 chunk 上限。
- 通过 `aligned_partial_result_len` 把聚合槽位数调整为 Go 方法 `getPartialResultSliceLenConsiderByteAlign` 使用的布局：单个聚合保持 1，其余奇数补成偶数，0 和偶数保持不变。
- 通过 `is_finished` 非阻塞读取共享结束标志，让未来的 partial/final worker 能轮询退出请求。

本文件不执行分组、聚合、shuffle、spill 或结果输出，也不创建线程；这些行为分别位于同 crate 的 `agg_hash_partial_worker.rs`、`agg_hash_final_worker.rs`、`agg_hash_executor.rs` 和 `agg_spill.rs`。

## 主要符号

- `pub struct BaseHashAggWorker`：公开结构体。`aggregations` 与 `max_chunk_size` 公开，`finish` 私有，调用方只能通过 `is_finished` 观察结束状态。
- `pub fn new(finish: Arc<AtomicBool>, aggregations: Arc<Vec<Aggregation>>, max_chunk_size: usize) -> Self`：无校验、无转换的构造函数；即使 chunk 上限为 0 也会保留，测试以此确认与 Go 构造函数的直存语义一致。
- `pub fn aligned_partial_result_len(&self) -> usize`：读取 `aggregations.len()`；长度为 1 时返回 1，否则返回 `count + (count & 1)`。该表达式让大于 1 的奇数加 1，并保留偶数与 0。
- `pub fn is_finished(&self) -> bool`：以 `Ordering::Acquire` 加载私有 `AtomicBool`。

文件没有模块级常量、trait、错误类型或条件编译项。

## 执行流程

1. 上游先创建一个共享的 `Arc<AtomicBool>` 和一个共享的 `Arc<Vec<Aggregation>>`，再调用 `BaseHashAggWorker::new`。构造过程只移动三个参数到字段中。
2. 需要为每组中间聚合态安排槽位时，调用 `aligned_partial_result_len`：先取聚合数；恰有一个聚合时直接返回 1；其余情况用最低位判断奇偶并补齐到偶数。
3. worker 的循环或收尾点可调用 `is_finished`。返回 `true` 表示共享标志已被其他所有者置位，调用方应决定是否退出；本方法自身没有循环、等待或清理动作。

当前 Rust 生产执行器并未调用以上流程。可运行的 `HashAggExec` 直接构造 `HashAggPartialWorker`/`HashAggFinalWorker`，它们共享的是 `Arc<Vec<Aggregation>>`，而不是本类型。

## 数据与状态

- `aggregations: Arc<Vec<Aggregation>>`：共享、只读的聚合描述集合。`Aggregation` 定义于 `agg_util.rs`，记录 `AggKind`、可选输入列和 `distinct` 标志。`Arc` 克隆只增加引用计数，不复制整个列表。
- `max_chunk_size: usize`：调用方提供的 chunk 行数上限；本文件只保存，不解释或强制其范围。
- `finish: Arc<AtomicBool>`：共享结束状态。字段私有可限制本模块外直接读写，但调用方通常仍持有传入 `Arc` 的克隆并负责置位。

`BaseHashAggWorker` 没有自定义 `Drop`，其生命周期结束时由 Rust 自动释放三个字段；最后一个 `Arc` 所有者离开时才释放共享对象。结构体也没有内部可变的聚合数据、统计数据、内存计数器或 partial result 缓冲区。

## 依赖与调用关系

直接依赖只有两类：`crate::agg_util::Aggregation` 提供聚合描述；标准库的 `Arc`、`AtomicBool` 和 `Ordering` 提供共享所有权与原子结束标志。`Cargo.toml` 将本文件编入 `astersql-executor-aggregate`，本文件本身没有直接使用该 manifest 中列出的外部 crate。

RustCodeGraph 将本文件识别为 8 个符号，并在文件级关系中列出 `agg_hash_base_worker_test.rs` 与 `agg_hash_executor_test.rs` 两个测试使用方；精确文本引用进一步确认，`BaseHashAggWorker`、`aligned_partial_result_len` 和 `is_finished` 的实际直接引用均只在 `agg_hash_base_worker_test.rs`。`agg_hash_executor_test.rs` 测试同 crate 的执行器，但不直接引用本类型，属于文件级测试编译关系而非方法调用边。

Go 主链则已完整接线：`agg_hash_executor.go` 的 `initPartialWorkers` 和 `initFinalWorkers` 调用 `newBaseHashAggWorker`，partial worker 用对齐方法计算 `partialResultNumInRow`，partial/final worker 在收尾、恢复 spill 数据等位置调用结束检查。

## 错误处理与边界

本文件所有方法都是无失败返回值的纯构造或查询操作，不产生 `Result`，也不捕获 panic。

- 聚合数为 0 时对齐长度为 0；为 1 时特判为 1；大于 1 的奇数向上补成偶数；偶数不变。独立测试覆盖 0 到 5。
- `max_chunk_size` 不做非零检查；0 会被原样保存。是否允许用 0 执行应由真正消费该字段的执行器决定。
- `usize` 极端值存在算术上溢的理论边界：当聚合数是 `usize::MAX` 时，`count + 1` 在 debug 构建会 panic、release 构建会回绕。但 `Vec` 不可能在正常可寻址内存中达到该长度，这不是现实输入路径。
- `is_finished` 只是一次快照读取；返回 `false` 后标志可立即被其他线程改为 `true`，调用方不能把它当作长期保证。

## 并发与资源生命周期

`Arc<Vec<Aggregation>>` 允许多个 worker 共享不可变描述，避免重复复制；本文件没有修改该向量的接口。`Arc<AtomicBool>` 允许结束信号跨线程共享。测试方以 `finish.store(true, Ordering::Release)` 发布信号，`is_finished` 以 `Ordering::Acquire` 读取，看到 `true` 时也获得发布线程在 Release 之前写入数据的可见性保证。

与 Go 的关闭 channel 不同，布尔标志不会阻塞等待，也不能携带关闭事件或唤醒睡眠线程；worker 必须主动轮询。多个线程重复写入 `true` 在原子层面安全，但本文件不负责将标志复位或协调线程 join。资源回收完全依赖 `Arc` 引用计数，没有 channel、锁、任务句柄、事务或 I/O 资源需要在此清理。

## 与 Go 版本的对应关系

Go 原型是同目录 `agg_hash_base_worker.go`：

- Rust `new` 对应 `newBaseHashAggWorker`，都原样保存聚合列表和 chunk 上限。
- Rust `aligned_partial_result_len` 精确复现 `getPartialResultSliceLenConsiderByteAlign` 的分支和奇偶计算；类型从 Go `int` 变为 Rust `usize`。
- Rust `is_finished` 对应 `checkFinishChClosed` 的“立即检查、绝不等待”语义，但机制从对只读 channel 的 `select/default` 改为原子布尔读取。
- Go `aggFuncs []aggfuncs.AggFunc` 是可执行聚合函数对象；Rust `Arc<Vec<Aggregation>>` 是本 crate 自定义的描述数据，实际更新/合并由 `AggState` 和 worker 完成，两者不是逐类型等价。
- Go 基座还有 `stats *AggWorkerStat` 和 `memTracker *memory.Tracker`；Rust 基座尚未包含统计与内存跟踪字段。虽然 `agg_util.rs` 已定义 Rust `AggWorkerStat`，本文件没有使用它。
- Go partial/final worker 已匿名嵌入基座并由 executor 构造；Rust 对应 worker 当前没有包含本类型。这是迁移/接线缺口，不能把 Go 调用关系当成 Rust 已支持行为。

## 扩展指南

若要让 Rust 生产执行链真正复用此基座，应从 `agg_hash_executor.rs` 的 worker 构造处接入，并在 `agg_hash_partial_worker.rs`、`agg_hash_final_worker.rs` 中明确组合 `BaseHashAggWorker`；不要只添加字段而保留重复的 `aggregations` 状态。结束检查应放在可能长时间循环、spill 恢复和输出等待的安全退出点，同时设计唤醒策略，因为单独轮询 `AtomicBool` 不能唤醒阻塞操作。

若补齐 Go 的统计或内存跟踪语义，需要先确定 Rust 所有权和并发更新方式，并复用 `agg_util.rs` 的 `AggWorkerStat` 或现有 spill 内存统计，避免产生两套互不一致的计数。若改变槽位对齐公式，必须同时核对 partial result 的实际内存布局与 Go 兼容意图，不能仅凭函数名假定缓存行宽度。

测试必须继续放在独立文件 `agg_hash_base_worker_test.rs`，不得内嵌进生产源文件。新增生产接线时还应同步扩展 `agg_hash_executor_test.rs`、`agg_hash_partial_worker_test.rs`，验证取消传播、并发退出、0/1/奇偶聚合数量、chunk 上限以及统计/内存生命周期。涉及 Go 对齐的修改应同步核对 `agg_hash_base_worker.go`、`agg_hash_executor.go`、`agg_hash_partial_worker.go` 和 `agg_hash_final_worker.go`。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/executor/aggregate` 确认目标 Rust/Go/测试及相邻 worker 均已索引；`node --file pkg/executor/aggregate/agg_hash_base_worker.rs --offset 1 --limit 260` 读取完整 103 行并列出两个文件级测试使用方；`query BaseHashAggWorker` 确认 Rust 类型及 Go 对应符号；`callees aligned_partial_result_len` 与 `callees is_finished` 核对其直接操作，未发现生产 callers。
- Rust 源与模块边界：`pkg/executor/aggregate/agg_hash_base_worker.rs`、`agg_util.rs`、`agg_hash_partial_worker.rs`、`agg_hash_final_worker.rs`、`agg_hash_executor.rs`、`lib.rs`、`Cargo.toml`。
- Rust 独立测试：`pkg/executor/aggregate/agg_hash_base_worker_test.rs` 覆盖构造直存、0 到 5 个聚合的对齐值，以及 Release 写/Acquire 读结束标志；`agg_hash_executor_test.rs` 仅提供同 crate 执行器行为背景，不是本类型的直接测试。
- Go 对照：`pkg/executor/aggregate/agg_hash_base_worker.go` 定义原型；`agg_hash_executor.go` 负责构造；`agg_hash_partial_worker.go` 与 `agg_hash_final_worker.go` 展示嵌入字段及结束检查的真实调用点。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求文档存在且恰有 11 个固定二级标题。
