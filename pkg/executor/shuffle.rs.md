# [`pkg/executor/shuffle.rs`](shuffle.rs)

## 文件定位

本文件属于 `astersql-executor` crate：`pkg/executor/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/executor/lib.rs` 通过 `pub mod shuffle;` 公开模块，并在测试配置下把独立的 `shuffle_test.rs` 注册为单测模块。它实现 root executor 层的 Shuffle 数据重分布核心：从多个上游 `dataSources` 并行取 `Chunk`，按哈希键或已排序连续组路由给固定数量的 worker，再汇总各 worker 内 child executor 的输出。

当前 Rust 接线需要分层理解。`ShuffleExec`、receiver/worker、两种 splitter、线程与通道生命周期均有实际实现；`pkg/executor/builder.rs::buildShuffle` 也会收集数据源和 worker 计划。但是 builder 最终只调用抽象的 `ExecutorBuilderDependencies::build_shuffle_executor`，仓库搜索未找到该方法的具体实现，也未找到直接构造本文件 `ShuffleExec` 的生产调用点。因此本文件是完整的可运行并发核心与适配边界，不能仅凭模块导出断言 Rust SQL 主链已经绑定它。

## 核心职责

- `ShuffleExecutor` 抽象 source、worker child 与 base executor 的 `Open/Next/Close`、Chunk 创建及运行时统计能力；本文件不提供“成功但不做事”的默认实现。
- `ShuffleRuntimeContext` 抽象分组键计算、连续组识别、failpoint、测试模式和并发统计，使并发拓扑不直接依赖具体 session/expression 实现。
- `ShuffleExec` 在 `Open` 中搭建有界通道和 Chunk 回收池，在第一次 `Next` 时启动 source、worker 与 coordinator 线程，在 `Close` 中广播结束、等待线程、排空通道并关闭所有执行器。
- `fetchDataAndSplit` 把每个 source 的行分配给对应 worker receiver；`shuffleWorker::run` 驱动 worker child，并把非空结果送回顶层输出通道。
- `partitionHashSplitter` 保证相同序列化 group key 经 MurmurHash3 后进入同一 worker；`partitionRangeSplitter` 要求输入已按分组键聚集，把完整连续组轮询分配给 worker，且轮询游标跨 Chunk 保留。

## 主要符号

- `pub type ShuffleExpression = Arc<dyn Any + Send + Sync>`：分区表达式的类型擦除句柄。表达式求值不在本文件完成，而由 `ShuffleRuntimeContext::GetGroupKey/NewGroupChecker` 解释。
- `pub trait ShuffleExecutor`：并发执行边界。`Open/Next` 接收共享 runtime context；`NewFirstChunk/TryNewCacheChunk` 决定 source、receiver、worker 间复用 Chunk 的形状与容量；`HasRuntimeStats` 控制关闭时是否登记并发度。
- `pub trait ShuffleGroupChecker` 与 `pub trait ShuffleRuntimeContext`：分别提供连续组迭代和 session/evaluation/failpoint 适配。实现者必须让 `GetNextGroup` 产生覆盖当前输入的合法半开区间。
- `finishSignal`、`sendUntilFinished`、`receiveUntilFinished`：原子结束标志加 `Condvar` 的取消协议。同步通道满时以约 2ms 间隔重试，接收以 10ms 超时轮询，从而能观察 `Close` 发出的结束信号。
- `shuffleOutput`、`shuffleMessage`、`sourceRoute`：内部消息模型。正常 worker 输出携带 `Chunk` 与回收 sender；错误输出只携带 `SharedError`；`Finished` 由 coordinator 在所有线程退出后发送。
- `pub struct ShuffleExec`：顶层状态机。公开字段保存 base、并发度、worker、splitter 和 source；私有字段保存 prepared/executed 状态、通道、source→worker 路由、coordinator handle、测试退出标志与 runtime context。
- `pub struct shuffleReceiver`：worker 内每个 source 的叶子执行器，从输入通道取得已分区 Chunk，通过 `SwapColumns` 交给 child，并归还空壳供 source 复用。
- `pub struct shuffleWorker`：持有一个 child executor 和多个 receiver；循环调用 child `Next`，只发送非空输出。
- `pub trait partitionSplitter` 及 `partitionHashSplitter`/`partitionRangeSplitter`：按行生成 worker 下标。构造函数 `buildPartitionHashSplitter`、`buildPartitionRangeSplitter` 都要求 `concurrency > 0`。
- `murmur3Sum32`：seed 0 的 MurmurHash3 x86 32-bit 本地实现，用于与 Go `twmb/murmur3.Sum32` 的分区结果对齐。

## 执行流程

1. 构造时 `ShuffleExec::new` 断言并发度为正、worker 数等于并发度、splitter 数等于 source 数，并把执行器/splitter 放进 `Arc<Mutex<_>>` 以供专用线程持有。
2. `Open` 依次打开所有 source 和 base，重置状态并创建容量为 `concurrency + dataSources.len()` 的顶层输出通道。随后为每个 worker 的每个 receiver 创建容量 1 的输入通道和容量 1 的 holder 通道，在每条 source→worker 路由中预放一个 source 形状的空 Chunk；每个 worker 也获得一个容量 1 的输出 holder，并打开 child。
3. 首次 `Next` 调用 `prepare4ParallelExec`：每个 source 启动一个线程，每个 worker 启动一个线程，另有 coordinator 线程 join 它们。source/worker 主体都在 `catch_unwind` 内运行，panic 会被转换为输出错误。
4. source 线程 `fetchDataAndSplit` 循环调用 source `Next`。非空输入交给对应 splitter，返回值必须与输入行数相同且每个 worker 下标都在路由范围内；随后逐行追加到该 worker 的暂存 Chunk，满 Chunk 立即发给 receiver，源耗尽时刷新各 worker 的尾部 Chunk。
5. worker 线程先从输出 holder 取得空 Chunk，再调用 child `Next`。child 通常通过其树中的 `shuffleReceiver::Next` 消费一个或多个 source 的分区输入；得到非空结果后，worker 将结果及 holder sender 发送到顶层。
6. 顶层 `Next` 接收一条消息：错误则返回错误；正常输出通过 `SwapColumns` 移入调用方 `req`，并把交换后的空 Chunk 归还 worker holder；`Finished` 或通道断开则把 `executed` 置为真，之后持续返回空结果。
7. `Close` 先 `Finish` 唤醒/终止阻塞通道操作，再 join coordinator、排空输出、清理 receiver 输入并关闭 child、source 与 base。它保留第一个关闭错误；若 base 声明有运行时统计，则登记 `ShuffleConcurrency`。

## 数据与状态

`prepared` 表示并行线程拓扑是否已经由首次 `Next` 启动，`executed` 表示输出流是否结束；两者在 `Open` 重置。`finishCh` 是一次 Open/Close 周期内所有 source、receiver、worker 和 coordinator 共享的取消令牌。`sourceRoutes[source][worker]` 保存一条有界输入管道和对应空 Chunk holder，确保每条路由至多有一个待填充/待消费 Chunk，形成背压并限制缓冲内存。

`partitionHashSplitter.hashKeys` 和调用方传回的 `workerIndices` 都被复用，减少每批分配；它要求 runtime 返回的 key 数恰好等于行数。`partitionRangeSplitter.idx` 在多个 Chunk 之间持续递增取模，所以相邻 Chunk 的第一个新组不会总落到 worker 0；同一个连续组内的所有行保持同一 worker。Range 模式只保证连续相等键不被拆分，输入“同键连续”的前置条件由计划/上游排序保证，本文件不重新排序。

Chunk 所有权通过 `SwapColumns` 与 holder 循环转移：source 侧按路由复用空 Chunk，worker 输出侧由消费端归还空壳。这一协议避免逐批新建列缓冲；绕过 holder 或不归还输出会使有界管道停止前进。

## 依赖与调用关系

直接外部依赖只有 `astersql-errors`、`astersql-util-chunk` 和启用 `failpoints` feature 的 `fail`，均在 `pkg/executor/Cargo.toml` 声明；线程、原子、互斥锁、条件变量和同步通道来自标准库。`pkg/executor/lib.rs` 是模块入口，并将测试放在独立的 `shuffle_test.rs`，符合生产与测试分文件约束。

已核验的上游与下游关系如下：

- Rust 构建入口 `pkg/executor/builder.rs::executorBuilder::buildShuffle` 构建 `ShufflePlanData` 的 data sources 与 workers，再委托 `ExecutorBuilderDependencies::build_shuffle_executor`；仓库内只有 trait 声明和该调用，没有具体实现或对 `ShuffleExec::new` 的生产引用。
- RustCodeGraph 的 `explore` 确认内部调用链：`ShuffleExec::Next -> prepare4ParallelExec -> fetchDataAndSplit / shuffleWorker::run -> ShuffleExecutor::Next`，`fetchDataAndSplit -> partitionSplitter::split`，range splitter 继续调用 `ShuffleGroupChecker::{SplitIntoGroups, IsExhausted, GetNextGroup}`，hash splitter调用 `GetGroupKey` 与 `murmur3Sum32`。
- Go 完整主链是 `pkg/executor/builder.go` 的物理计划分派进入 `buildShuffle`，构造 splitter、source、每个 worker 的 receiver/child，最终由统一 executor 协议驱动 `ShuffleExec.Open/Next/Close`。
- `pkg/executor/statement_ru_plan_walk.rs`/`.go` 识别物理 Shuffle 并计算 RU，但这是计划统计关系，不会调用本文件运行时类型。

## 错误处理与边界

`Open` 用 `?` 返回 source/base/child 打开错误和 holder 初始化错误，但若中途失败，本函数不会自动回滚此前已打开的对象；调用者仍需遵循执行器生命周期进行清理。构造期的零并发、worker 数不匹配、splitter/source 数不匹配以断言失败表示编程/计划装配错误。

运行期的 source `Next`、splitter、child `Next` 错误被包装为 `shuffleMessage::Output` 传给顶层 `Next`。本文件还显式拒绝 splitter 返回错误行数或越界 worker 下标。`ShuffleNextError` 可在顶层收消息前注入错误；worker failpoint 和 runtime 的 source/worker failpoint 位于线程的 panic 恢复边界内。`panicError/recoveryShuffleExec` 保留字符串 payload，否则使用 `shuffle panicked`，并打印强制 backtrace。

Mutex/Condvar poison 和缺少必需通道均使用 `expect`，属于内部不变量破坏而非可恢复用户错误。`Close` 会尽量关闭全部 child/source/base，并返回遇到的第一个错误；coordinator panic也转成关闭错误。通道断开通常被解释为结束或转成明确的 recycle/holder disconnected 错误。顶层收到某个运行错误后并不会自动设置 finish，调用者应继续按统一执行器协议调用 `Close`，否则其他线程可能仍受背压阻塞。

## 并发与资源生命周期

每次首次 `Next` 启动 `M + N + 1` 个线程：每个 source 一个、每个 worker 一个、coordinator 一个。source 与其 splitter是一一对应的，因此 splitter 的 mutex 不用于多 source 共享语义；worker child 及 receiver 列表只由对应 worker 驱动。`Arc<Mutex<_>>` 主要解决对象跨线程所有权和外部 trait object 的可变访问。

所有数据通道均有界：source→receiver 与各 holder 容量为 1，顶层输出容量为 `N + M`。`sendUntilFinished` 不永久阻塞在标准库 `send` 上，而是 `try_send` 加短等待；`receiveUntilFinished` 使用超时接收，使 `finishSignal` 能终止双方。`finishSignal` 的 Release/Acquire 配对保证关闭标志在线程间可见，`Condvar::notify_all` 缩短发送方观察关闭的时间。

coordinator join 全部 source/worker 后将 `allSourceAndWorkerExitForTest` 置真并发送 `Finished`。测试模式下，若已启动却在 `Close` 完成协调后仍观察到线程未退出，代码会 panic，作为泄漏检测不变量。`Close` 必须是最终资源回收点：它广播 finish、join coordinator、断开通道引用、清空 receiver 缓冲并关闭执行器；重复使用必须重新 `Open`，但本文件没有声明同一实例可并发调用 `Open/Next/Close`。

## 与 Go 版本的对应关系

`pkg/executor/shuffle.go` 是直接语义来源。两版都有 `ShuffleExec`、`shuffleOutput`、`shuffleReceiver`、`shuffleWorker`、`partitionSplitter` 及 hash/range 两种实现；`Open` 建通道并预填空 Chunk，首次 `Next` 懒启动并发拓扑，source 分发、worker 汇总、consumer 归还 Chunk，`Close` 广播退出并记录并发统计，整体结构相同。

Rust 用 `std::thread`、`SyncSender`、`finishSignal` 和显式 `catch_unwind` 替代 Go goroutine、channel、`context.Context` 与 `recover`。Go `closeCh`/关闭 channel 的广播语义在 Rust 中变成原子标志加轮询；Go `WaitGroup` 变成保存所有 `JoinHandle` 的 coordinator。Rust 额外校验 hash key 数、splitter 输出行数和 worker 下标，并在构造函数中断言拓扑数量一致。

哈希版 Rust 内嵌 MurmurHash3 x86-32 seed 0，Go 调用 `murmur3.Sum32`；两边都对 worker 数取模。Range 版都借助 group checker 获取连续半开区间，并在组与组之间 round-robin；Go 注释明确要求 source 已分组，Rust 保留同一约束且让 `idx` 跨 Chunk 延续。

测试对照方面，`pkg/executor/shuffle_test.go::TestPartitionRangeSplitter` 的 13 行/6 组预期被 `pkg/executor/shuffle_test.rs::partition_range_splitter_matches_go_group_round_robin` 复刻，Rust 还验证了跨 Chunk 轮询状态和 hash splitter 的正并发约束。Go `pkg/executor/executor_failpoint_test.go::TestShuffleExit` 同时覆盖 Next/source/worker failpoint 与 SQL 级退出；Rust `pkg/executor/executor_failpoint_test.rs` 目前只直接验证 `shuffleWorkerRun` 钩子在可恢复边界内 panic，尚无本文件完整并发拓扑或 SQL 主链回归证据。

## 扩展指南

- 新增 splitter 时，应实现 `partitionSplitter::split`，保证结果长度等于输入行数、下标小于 worker 数，并在 builder 的 plan splitter 映射处接线；测试放在独立 `pkg/executor/shuffle_test.rs`，覆盖跨 Chunk 状态、空输入、错误传播和确定性。
- 接通真实 Rust SQL 主链时，应实现 `ExecutorBuilderDependencies::build_shuffle_executor`，把计划中的表达式、source、worker tail/receiver 关系适配为本文件的三个 trait；必须保留 Go builder 为每个 worker 重建 child 树、每个 source 对应一个 receiver 的拓扑，不能把所有 worker 共享成同一可变 child。
- 修改 hash 编码或算法前必须与 `aggregate.GetGroupKey` 和 Go `murmur3.Sum32` 做跨语言向量测试；分区改变会影响负载均衡、结果局部性和分布式兼容性，即使 SQL 结果表面不变也不可视为无风险重构。
- 修改通道容量、holder 回收或取消协议时，应增加完整 `Open -> 多次 Next -> Close` 测试，覆盖消费端提前停止、source/child 出错、panic、通道背压和线程全部退出；不要把测试嵌入生产 `.rs`。
- Range splitter 的调用者必须保证同键行连续；若要支持无序输入，应在计划中增加排序或选择 hash splitter，而不是在 `split` 内偷偷排序并改变内存/时延特征。
- 性能敏感点包括逐行 `AppendRow`、每批 group key 编码、互斥锁获取、2ms/10ms 取消轮询和 `M×N` 路由 Chunk。优化时必须维持有界背压、Chunk 所有权归还、同键同 worker 以及关闭可终止性。

## 验证依据

- Rust 源码：`pkg/executor/shuffle.rs` 的三个适配 trait、`ShuffleExec::{new, Open, Next, Close, prepare4ParallelExec, fetchDataAndSplit}`、`shuffleReceiver`、`shuffleWorker::run`、两种 splitter 与 `murmur3Sum32`。
- crate 与模块：`pkg/executor/Cargo.toml` 的 `[lib]`、`astersql-errors`、`astersql-util-chunk`、`fail` 依赖；`pkg/executor/lib.rs` 的 `pub mod shuffle` 和独立 `shuffle_test.rs` 注册。
- Rust 构建链：`pkg/executor/builder.rs::executorBuilder::buildShuffle`、`ShufflePlanData` 与 `ExecutorBuilderDependencies::build_shuffle_executor`；全仓搜索只发现 trait 声明和调用，未发现该依赖方法实现或 `ShuffleExec::new` 的生产构造点。
- Go 对照：`pkg/executor/shuffle.go`、`pkg/executor/builder.go::buildShuffle/buildShuffleReceiverStub`。
- 测试证据：`pkg/executor/shuffle_test.rs` 的正并发断言、13 行 range 分组映射和跨 Chunk 游标测试；`pkg/executor/executor_failpoint_test.rs::shuffle_worker_failpoint_panics_inside_the_recovered_worker_boundary`；Go 的 `pkg/executor/shuffle_test.go::TestPartitionRangeSplitter` 与 `pkg/executor/executor_failpoint_test.go::TestShuffleExit`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；精确 `query` 找到 `pkg/executor/shuffle.rs::ShuffleExec`、`buildPartitionRangeSplitter`、`fetchDataAndSplit`，`explore` 给出 `Next -> prepare4ParallelExec -> fetchDataAndSplit/run`、splitter、group checker、failpoint 和测试调用关系。对精确 ID 执行 `callers/callees/impact` 未产生额外输出，因此跨模块接线结论以 builder 源码和 `rg` 交叉核验。
- 未运行 Cargo，符合本纯文档任务约束；结构验证单独执行并记录。
