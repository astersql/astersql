# `pkg/executor/parallel_apply.rs`

## 文件定位

本说明对应源码 [`parallel_apply.rs`](parallel_apply.rs)。该文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod parallel_apply` 导出该模块，直接依赖由 `pkg/executor/Cargo.toml` 声明的 `astersql-errors` 与 `astersql-util-chunk`。它实现 Parallel Nested Loop Apply 的 Rust 版本：对外层行绑定相关列后反复执行内层计划，并用多个内层 worker 并行完成相关子查询。

当前接线状态需要谨慎理解：仓库级 Rust 搜索只发现 `pkg/executor/parallel_apply_test.rs` 使用这里的公开类型，没有找到 Rust 生产构造者；实际 SQL 执行器构造证据仍在 Go 的 `pkg/executor/builder.go`，其中 `executorBuilder` 在 `PhysicalApply.Concurrency > 1` 时构造 Go `ParallelNestedLoopApplyExec`。因此，本文件是可独立驱动并有 Rust 单测覆盖的移植实现，但“已由 Rust SQL 主链调用”未得到代码证据。

## 核心职责

- `ParallelNestedLoopApplyExec` 管理 `Open`、惰性 `Next`、`Close` 生命周期，并协调一个外层拉取线程、`concurrency` 个内层线程以及可选的保序重排线程。
- 外层线程 `outerWorker` 批量拉取、向量化过滤外层数据，再把每行连同 `selected` 和单调递增的 `seq` 投递给内层 worker。
- 内层路径为每个选中的外层行绑定相关列，经 `fetchAllInners` 执行、过滤或缓存内层结果，再委托 `ParallelApplyJoiner` 产生连接输出；未匹配时由 `OnMissMatch` 实现 outer/semi 类语义。
- `keepOrder=false` 时，worker 直接竞争外层行并以完成顺序输出；`keepOrder=true` 时，worker 产生带序号的 `orderedResult`，`reorderWorker` 按外层行顺序合并输出。
- 模块还封装取消、panic 转错、错误传播、Chunk 复用、内存计量、缓存命中统计和关闭期资源回收。

## 主要符号

- `ApplyExpression`、`ApplyCorrelatedColumn`：`Arc<dyn Any + Send + Sync>` 类型擦除句柄；具体表达式解释、相关列绑定及键编码由运行时上下文承担。
- `ParallelApplyCancellation`：只暴露 `Cancel`，要求实现能打断正在进行的 `Next`。
- `ParallelApplyExecutor`：外层、内层和基础 Chunk 分配器的统一边界，包含 `Open`、`Next`、`Close`、取消令牌及 Chunk 工厂。
- `ParallelApplyJoiner`：`TryToMatchInners` 消费内层行并推进 `cursor`，`OnMissMatch` 写入未匹配结果。调用方显式检查“仍有行但游标未推进”，以避免死循环。
- `ParallelApplyRuntimeContext`：集中承接向量过滤、相关列绑定和编码、Apply 缓存、内存 tracker、运行统计、关闭错误日志及 failpoint。它是本文件与会话/表达式/缓存基础设施之间的适配边界。
- `ParallelApplyMemoryTracker`：用 `AtomicI64` 记录 Apply 所持 Chunk 的字节数；`BytesConsumed` 是公开读接口，内部 `Consume` 由 `rowList` 增减。
- `finishSignal`、`sendUntilFinished`、`receiveUntilFinished`：以原子结束标志、条件变量及短超时轮询，使有界同步通道的发送/接收可被关闭打断。
- `result`、`outerRow`、`orderedResult`、`orderedMessage`：分别描述主输出、外层工作项、单行保序结果和保序通道消息。
- `rowList`：拥有 Box 化 Chunk，提供行视图、缓存快照、替换和清空，并同步更新内存计量。
- `applyWorkerState`、`applyWorker`、`parallelApplyShared`：保存每 worker 的相关列、过滤器、内层列表、游标和 joiner，以及跨线程共享的执行器、通道、配置、计数器与结束信号。
- `ParallelNestedLoopApplyExec`：公开主类型；`new` 校验所有 per-worker 向量与并发度一致，`Open` 初始化，`Next` 消费结果，`Close` 取消并回收。
- `panicError`：把 `&str`/`String` panic 载荷还原为共享错误，其他载荷使用通用错误消息。

## 执行流程

1. `new` 要求 `concurrency > 0`，且 `innerExecs`、`joiners`、`corCols`、`innerFilter` 的长度都等于并发度；随后为每个 worker 固化独立的内层执行器、取消令牌、joiner 和状态。
2. `Open` 先打开外层执行器并挂载 `ParallelApplyMemoryTracker`，重置每个 worker 的内层列表、可复用 Chunk、选择位图、当前外层行和匹配状态。它建立 free/result/outer 通道；保序模式再建立 ordered/pace 通道，预填 `concurrency` 个输出 Chunk，并按需初始化缓存。
3. 首次 `Next` 通过 `started.compare_exchange` 惰性调用 `startWorkers`。之后每次阻塞接收一个 `result`：错误直接返回，`chk=None` 表示 EOF 并置 `drained`，正常 Chunk 通过 `SwapColumns` 交给调用方，旧 Chunk 送回 free 池复用。
4. `outerWorker` 循环调用外层 `Next`，EOF 时设置 `outerFinished`；非空 Chunk 经 `VectorizedFilterOuter` 后存入 `outerList`，逐行生成序号。保序模式先取得 pace 令牌，再经零容量 outer 通道把行交给 worker。
5. 乱序的 `innerWorker` 从 free 池取 Chunk，调用 `fillInnerChunk` 尽量填满后直接送 result 通道。`fillInnerChunk` 在当前内层行耗尽时补未匹配结果、领取下一外层行、重置状态、调用 `fetchAllInners`，然后反复让 joiner 消费内层行。
6. 保序的 `innerWorkerOrdered` 每次完整处理一个 `outerRow`。`processOneOuterRow` 对未通过过滤的外层行按 outer 语义决定补行或跳过；其余行拉齐全部内层结果并生成一个或多个 Chunk，再携带 `seq` 发送到 ordered 通道。
7. `reorderWorker` 用 `BTreeMap<u64, orderedResult>` 暂存乱序完成项，从 `nextSeq` 起连续消费；每推进一个序号便消费一个 pace 令牌，把小结果合并进复用输出 Chunk。无序协调路径由 `notifyWorker` 等待全部 worker 后发 EOF。
8. `Close` 先触发 finish 并取消外层和所有内层执行器，再等待协调线程、排空结果、关闭外层执行器；最后清空 Apply 持有的行列表，登记并发度、缓存命中率和剩余内存，分离 tracker 并清除本轮共享状态。

## 数据与状态

- `started` 保证一轮 `Open` 后只启动一次线程组；`drained` 让 EOF 后的后续 `Next` 立即返回空 Chunk。这两个标志在 `Open` 和 `Close` 重置。
- 每个 `applyWorkerState` 独占 `innerList`、`innerChunk`、`innerSelected`、`outerRow`、`innerCursor`、`hasMatch`、`hasNull` 和 joiner，避免不同 worker 共享相关列或游标状态。
- `outerList` 长期拥有外层 Chunk，确保跨线程传递的 `chunk::Row` 视图底层存储仍存活；各 `innerList` 同理保存本次相关键的内层行。
- 缓存键由 `EncodeCorrelatedKey` 生成；每次查询先增加 `cacheAccessCounter`，命中时以缓存 Chunk 替换 `innerList` 并增加 `cacheHitCounter`，未命中成功执行后写入快照。
- free 池容量为并发度，result 池容量为并发度加一；outer 通道容量为零，形成直接交接。ordered 和 pace 容量分别为 `2 * concurrency`、`4 * concurrency`，后者限制早完成结果在重排表中的积压。
- `rowList::Add/Replace/Reset` 是内存数字的唯一 Chunk 归属计量点。`Close` 在发布统计前清空 outer/inner 列表，所以正常关闭后的 `memory_bytes` 应为零；Rust 测试对此有断言。

## 依赖与调用关系

- 模块入口：`pkg/executor/lib.rs` 导出 `parallel_apply`，并仅在测试配置下引入独立的 `parallel_apply_test.rs`。
- crate 边界：`pkg/executor/Cargo.toml` 的包名是 `astersql-executor`；本文件直接使用 `astersql-errors` 的 `SharedError/New` 和 `astersql-util-chunk` 的 `Chunk/Row`。`nextgen` feature 与本文件没有条件编译关系，本文件自身也没有 `cfg` 项。
- 已验证的 Rust 上游只有 `pkg/executor/parallel_apply_test.rs`，它直接构造 `ParallelNestedLoopApplyExec`。未检出其他 Rust 文件引用这些公开 trait 或主类型，因此生产 Rust 上游调用者未验证。
- Go 生产上游是 `pkg/executor/builder.go`：并发度大于一时克隆内层计划，为每个 worker 创建执行器、相关列、过滤器和 joiner，并传入 `keepOrder`、`CanUseCache`。
- 核心内部边为：`Next -> startWorkers`；`outerWorker -> ParallelApplyExecutor::Next + VectorizedFilterOuter`；`innerWorker -> fillInnerChunk -> fetchAllInners`；`innerWorkerOrdered -> processOneOuterRow -> fetchAllInners`；`fetchAllInners -> BindCorrelatedColumns/ApplyCache*/inner executor Open-Next-Close`；两条连接路径最终调用 `TryToMatchInners/OnMissMatch`。
- RustCodeGraph 索引能定位 `ParallelNestedLoopApplyExec`、`fetchAllInners` 和 `innerWorkerOrdered`，但文件过滤未返回目标路径，且 callers/callees 查询未在限定时间内给出可靠边；因此上游范围又用仓库级精确符号搜索核验，并在此明确记录限制。

## 错误处理与边界

- `new` 对零并发或 per-worker 向量长度不一致直接 panic，这是构造期不变量而非可恢复运行时错误；独立 Rust 测试覆盖零并发。
- 外层/内层过滤返回的布尔数组长度必须等于输入行数，否则返回明确错误，防止静默错配行。
- joiner 在尚有内层行时必须推进 `innerCursor`；两个连接路径均检测停滞并返回 `parallel apply joiner did not advance inner cursor`。
- 执行器、worker 状态和接收端 Mutex 中毒通过 `expect` 触发 panic；worker 线程外围 `catch_unwind` 将 panic 转换成结果错误并输出强制回溯。主调用线程上的构造/Open/Next/Close panic 不由此捕获。
- `fetchAllInners` 保留内层主执行错误；内层 `Close` 失败只交给 `LogInnerCloseError`，不会覆盖主结果，与 Go 的延迟关闭并记录错误意图一致。
- 通道断开、输出回收被 finish 打断、协调线程 panic、保序序号出现缺口都有独立错误。正常关闭期由 finish 和取消令牌抑制部分预期中的 worker 错误。
- 调用契约要求先 `Open` 再 `Next`；否则 `resultReceiver`/`shared` 的 `expect` 会 panic。`Close` 可以在尚未启动 worker 时回收 Open 创建的状态。
- 当前文件源码中的线程协调和重排分支应在后续代码交付中继续以编译/测试验证；本任务按约束只做文档分析且不运行 Cargo，不能把结构阅读替代为完整可构建性证明。

## 并发与资源生命周期

- 线程模型为一个 outer worker、N 个 inner worker、一个 coordinator；保序模式由 coordinator 再启动 reorder worker。执行器和 worker 状态通过 `Arc<Mutex<_>>` 共享，状态标志与计数器使用原子操作。
- `finishSignal` 先以 Release 发布结束，再唤醒 Condvar；读取使用 Acquire。发送方使用 `try_send + 2ms` 等待，接收方使用 `recv_timeout(10ms)`，从而能周期性观察 finish，避免永久阻塞在满/空通道。
- `Close` 的顺序是“发布 finish -> 取消可能阻塞的外/内 Next -> join coordinator -> 关闭外层 -> 释放列表内存 -> 注册统计 -> detach tracker”。这是 LIMIT 提前结束和错误退出时不泄漏后台线程的关键约束。
- 外层结束以 `outerFinished` 原子标志配合 outer 通道超时接收表达，而不是关闭共享 Receiver；inner worker 在超时后观察到结束即可退出。
- 保序模式必须维持 `seq` 连续性与 pace 一进一出：outer 投递前占一个令牌，reorder 消费对应序号时释放一个。改变通道容量或释放时机可能造成无界 pending、死锁或顺序破坏。
- Chunk 通过 free 池循环复用；重排换取旧 Chunk 后必须 `Reset`，否则 `SwapColumns` 后残留的列数据可能污染下一批输出。
- `rowList` 保存 Row 的底层 Chunk 所有权；不能只保留行视图后提前释放 Chunk。缓存替换、切换外层行及关闭时都必须同步调整 tracker。

## 与 Go 版本的对应关系

Rust 主流程与 `pkg/executor/parallel_apply.go` 的同名类型和方法一一对应：`Open/Next/Close`、`outerWorker`、乱序/保序 inner worker、`processOneOuterRow`、`reorderWorker`、`fetchAllInners`、`fetchNextOuterRow`、`fillInnerChunk` 和 panic 处理均有直接语义来源。

Rust 用 trait 把 Go 的 `exec.Executor`、`join.Joiner`、表达式、applycache、memory tracker、failpoint 和 runtime stats 适配为可注入边界；用 `std::sync::mpsc::sync_channel` 和 `finishSignal` 对应 Go channel 加 `exit`/context cancellation；用 `BTreeMap` 对应 Go pending map，但显式以 `nextSeq` 取有序项。Go 的 `chunk.Iterator` 在 Rust 中变为 `Vec<chunk::Row> + innerCursor`，因此 Rust 额外检查 joiner 必须推进游标。

两版都以 `concurrency*4` pace 容量限制保序 pending 的增长，以缓存访问/命中计数计算命中率，并在关闭时取消正在执行的扫描。Go 的生产构造已在 `builder.go` 验证；Rust 目前只有模块导出和单测构造证据，不能据此宣称已替代 Go 生产路径。

Go 测试 `pkg/executor/parallel_apply_test.go` 覆盖计划选择、正确性、执行中取消、保序、边界条件、大内层结果、left outer semi join、goroutine panic、kill signal 和嵌套 Apply。Rust 独立测试目前只覆盖构造不变量、初始内存，以及一个两 worker 保序场景的输出顺序、打开次数、tracker attach/detach、运行统计与关闭后零内存；Go 的广泛回归面尚未在 Rust 测试中等量复刻。

## 扩展指南

- 新增执行器能力或运行时集成点时，优先扩展 `ParallelApplyExecutor` 或 `ParallelApplyRuntimeContext`，并同步所有实现者；避免把会话、表达式或缓存具体类型直接塞进并发核心。
- 修改连接语义时同时检查 `processOneOuterRow` 与 `fillInnerChunk` 两条路径，保证 `hasMatch/hasNull`、未选中外层行和 `OnMissMatch` 行为一致；joiner 必须满足游标推进不变量。
- 修改保序算法时重点守住 `outerRow.seq`、pace 令牌、pending 连续排空、EOF 与错误优先级；性能风险主要是早序号慢任务造成积压、过小通道造成吞吐下降，以及 Chunk 合并过早导致碎片化输出。
- 修改缓存时保持键编码与 Go `codec.EncodeKey` 语义兼容，确保缓存命中和未命中都正确替换/重置 `innerList`，并维护 access/hit 计数与内存归属。
- 修改关闭或错误路径时验证阻塞中的外/内 `Next` 能被 `Cancellation::Cancel` 打断，所有线程可 join，tracker 恰好 attach/detach 一次，并且 close 日志错误不会遮蔽主错误。
- Rust 回归应放在独立的 `pkg/executor/parallel_apply_test.rs`，不要嵌入生产文件。至少同步 Go 测试中的取消、panic、kill、缓存命中/失败、保序大结果与 outer/semi 边界；若将来接入 Rust 生产 builder，还应新增构造主链测试。
- 兼容性风险集中在相关键编码、NULL/未匹配语义和输出顺序；性能风险集中在锁粒度、全量内层物化、缓存快照克隆和通道背压。任何优化都应与 Go 行为逐项对照，而不是删减路径。

## 验证依据

- 源码全量阅读：`pkg/executor/parallel_apply.rs`，核对公开 trait/类型、内部状态、`new/Open/Next/Close`、两种 worker 流程、缓存、重排、取消、内存及 panic 路径。
- crate 与模块：`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`；确认 crate 名、直接依赖、公开模块及独立测试模块。
- Rust 测试：`pkg/executor/parallel_apply_test.rs`；确认零 worker panic、初始零内存、保序输出 `(3,10),(1,10),(2,10)`、三次内层打开、tracker 生命周期和关闭统计。
- Go 对照：`pkg/executor/parallel_apply.go`、`pkg/executor/builder.go`、`pkg/executor/parallel_apply_test.go`；确认生产构造条件、逐方法移植关系、取消/保序/缓存设计和现有回归范围。
- RustCodeGraph：`status` 显示索引含 Rust/Go 文件；`query ParallelNestedLoopApplyExec --kind struct` 同时定位 Go 与 Rust 类型，`query fetchAllInners --kind function` 和 `query innerWorkerOrdered --kind function` 定位 Rust 实现，`node --file pkg/executor/parallel_apply.rs --offset 378 --limit 120` 核对主类型与构造器。`files --filter pkg/executor/parallel_apply` 未命中，callers/callees 未形成可用结果，故调用者结论又以 `rg` 精确符号搜索复核。
- 结构验收使用任务指定命令，要求目标文件存在且上述固定二级标题恰好 11 个；本任务为纯文档分析，按计划不运行 Cargo。
