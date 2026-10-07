# `pkg/executor/analyze.rs` 逻辑说明

## 文件定位

`pkg/executor/analyze.rs` 属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs` 以 `pub mod analyze` 暴露它，`pkg/executor/Cargo.toml` 声明该 crate，并直接依赖 `astersql-statistics-handle` 和带 `failpoints` feature 的 `fail`。本文件是 ANALYZE 统计收集的编排与边界层：它不实现列采样、索引扫描或统计存储细节，而是定义任务、结果、取消和运行时 trait，并规定预刷 delta、构建/保存、发布、作业收尾及全局统计合并的顺序。

当前应用接线要区分两条路径。会话层 `pkg/session/runtime/statistics.rs` 的 `ConcreteSession::execute_analyze_with_context` 构造 `SessionAnalyzeRuntime`，调用 `AnalyzeExec::RunCanonicalWithContext`；这是 SQL 会话实际使用的规范批次路径。`AnalyzeExec::Next` 则保留与 `pkg/executor/analyze.go` 的 `AnalyzeExec.Next` 对齐的显式任务/worker 管线，RustCodeGraph 当前找到的直接构造者是 `pkg/executor/analyze_test.rs::executor`。因此不能仅凭 `Next` 的完整实现断言生产会话正从该入口运行。

## 核心职责

- `flushStatsDeltaForAnalyze` 在列分析读取 `mysql.stats_meta` 基数前，按库表去重并尝试广播 `FLUSH STATS_DELTA ... CLUSTER`；旧节点不支持广播执行类型时只记录警告，其他错误向上返回。
- `AnalyzeExec::RunCanonicalWithContext` 固定 canonical 批次的阶段顺序：检查取消/kill、预刷 delta、准备批次、在线程中保存、再发布；任何捕获到的失败交给 `CanonicalAnalyzeRuntime::record_failure`。
- `AnalyzeExec::Next` / `next_inner` 管理 Go 对齐的多任务管线：过滤锁表、计算并发、登记作业、并行收集、并行保存、合并动态分区全局统计、保存分析选项并刷新统计缓存。
- `handleResultsErrorWithConcurrency` 汇集 worker 输出，分发保存任务，去重保存错误和历史统计表 ID，并保证每个作业走成功或失败收尾。
- `analyzeContext`、`buildAnalyzeKillCtx`、`analyzeWorkerExitErr` 和 `trySendAnalyzeResult` 把父取消、SQL kill 与跨线程提前退出统一为可传播的 `AnalyzeError`。

## 主要符号

- 全局配置：`RandSeed` 和 `MaxRegionSampleSize` 是与 Go 侧测试可复现性及 Region 采样上限对齐的原子值；本文件只定义，不在本地流程中修改。
- 错误与取消：`AnalyzeError`、`AnalyzeResultValue`、`analyzeContext`、RAII 守卫 `analyzeStop`、`killSignal`。`analyzeContext::error` 先看自身 cause，再沿父链查询；`analyzeStop::drop` 在尚无错误时写入 `context canceled`。
- 计划/任务模型：`statsObject`、`analyzePlan`、`analyzeColumnsPlanTask`、`tableInfo`、`partitionDefinition`、`analyzeTableID`、`analyzeTask`、`analyzeColumnsExec`、`analyzeIndexExec` 与 `taskType`。`analyzeTableID::statisticsID` 对分区返回物理分区 ID，否则返回表 ID。
- 结果/全局统计：`analyzeResults`、`analyzeResultPart`、`histogram`、`globalStatsKey`、`globalStatsInfo`、`globalStatsMap`。列统计以 `indexID = -1` 聚合，索引统计按直方图 ID 分键。
- 运行时边界：`analyzeRuntime` 承担广播、统计读写、作业状态、kill、指标和 SQL 等旧式管线副作用；`CanonicalAnalyzeRuntime` 是当前会话 canonical 路径的较窄边界；`CanonicalAnalyzeBatch` 携带版本、`TableStats` profiles 和 `RuntimeAnalyzeJob`。
- 主执行器：`AnalyzeExec` 持有任务、选项、`errExitCh` 和 `analyzeRuntime`；`baseAnalyzeExec` 保留单任务基础配置。公开编排入口包括 `RunCanonical`、`RunCanonicalWithContext`、`Next`、`waitFinish`、`saveAnalyzeOptions`、`handleResultsError*`、`buildAnalyzeKillCtx`、`trySendAnalyzeResult` 和 `analyzeWorker`。
- 辅助函数：`filterAndCollectTasks`、`getLockedTableAndPartitionIDs`、`warnLockedTableMsg`、`recordHistoricalStats`、`AddNewAnalyzeJob`、`finishJobWithLog`、`handleGlobalStats` 等封装可单独验证的策略。

## 执行流程

规范会话路径由 `RunCanonicalWithContext` 执行：先检查 `analyzeContext`，再依次调用 `check_killed`、`preflush_stats_delta`、再次检查上下文和 kill；随后在 `catch_unwind` 内调用 `prepare_batch`，用 scoped thread 执行 `save_batch`，成功后 `publish_batch`。保存线程或外层流程 panic 都经 `getAnalyzePanicErr` 转换；最终错误先 `record_failure` 再返回。`RunCanonical` 只是使用默认上下文的便利入口。

Go 对齐路径从 `Next` 进入。`Next` 记录是否为 restricted SQL，调用 `next_inner`，无论成功失败都遍历列任务并 `memoryTracker::detach`，手工 ANALYZE 还上报 `succ`/`failed` 指标。`next_inner` 的主要阶段是：

1. `buildAnalyzeKillCtx` 创建子上下文和 kill watcher；`filterAndCollectTasks` 一次查询涉及的表/分区锁，跳过锁定对象并追加警告。
2. 构造待更新表 ID，取 `min(build_stats_concurrency, tasks.len())`；零并发是错误。采样并发在主线程解析一次并写入所有列任务的原子字段。
3. 准备并插入作业，清空 `errExitCh`，创建有界 task/result channel；启动 build workers 和一个结果处理线程。
4. `analyzeWorker` 取任务、检查退出条件、启动作业并按 `taskType` 调用 `analyze_columns` 或 `analyze_index`；结果经 `trySendAnalyzeResult` 非阻塞重试发送。worker panic 被转换成带当前作业的错误结果。
5. `handleResultsError` 决定保存并发并捕获 panic；`handleResultsErrorWithConcurrency` 启动 save workers，消费分析结果、构建全局统计键、保存结果并收集错误，随后尝试登记历史统计。
6. `waitFinish` 优先返回 handler 错误并置 `errExitCh`，否则检查每个 worker。失败时补齐队列内和尚未发送任务的作业收尾；成功时按需 `merge_global_stats`，将 `saveAnalyzeOptions` 失败降为 warning，最后 `update_stats`。

## 数据与状态

共享状态主要通过 `Arc` 和原子量传递。`errExitCh: AtomicBool` 是整个旧式管线的快速失败标志；`samplingStatsConcurrency` 在 worker 启动前以 Release 写入；`analyzeContext.cancelled` 用 Acquire/Release 读写，具体 cause 受 `Mutex` 保护。任务与结果跨线程使用 `Arc`，全局统计映射、接收端及历史表 ID 集合使用 `Mutex`。

`OptionsMap` 以物理表 ID 保存 `v2AnalyzeOptions`。`saveAnalyzeOptions` 对缺失的 raw option 写 SQL `DEFAULT`，`SampleRate` 从位模式恢复 `f64`；动态分区裁剪时只 REPLACE 表级行，并对分区旧行中明确 reset 的字段执行 `UPDATE ... SET ...=DEFAULT`。`columnChoice` 会转义单引号，列 ID 以逗号串持久化。

`handleGlobalStats` 只处理动态裁剪下的分区结果。列 part 忽略空直方图并把所有 histogram ID 放入 `(tableID, -1)`；索引 part 为每个非空直方图建立 `(tableID, histogram.id)`。重复键采用 `BTreeMap::insert` 的后写覆盖语义。

## 依赖与调用关系

RustCodeGraph 核实 `AnalyzeExec::next_inner` 仅由同文件 `AnalyzeExec::Next` 调用，并向下调用锁表过滤、并发参数、作业登记、`analyzeWorker`、`handleResultsError`、`waitFinish`、全局合并、选项保存和统计更新。`handleResultsErrorWithConcurrency` 由 `handleResultsError` 调用，并向下调用 `save_analyze_result`、`finishJobWithLog`、`handleGlobalStats` 与 `recordHistoricalStats`。

规范路径的上游是 `pkg/session/runtime/statistics.rs::ConcreteSession::execute_analyze_with_context`；同文件的 `SessionAnalyzeRuntime` 实现 `CanonicalAnalyzeRuntime`，把本文件的顺序约束接到真实 catalog、统计 handle、kill 和发布逻辑。`CanonicalAnalyzeBatch` 的统计类型来自 Cargo 直接依赖 `astersql-statistics-handle`。

旧式路径把外部系统依赖收敛到 `analyzeRuntime`，因此本文件并不直接操作 TiKV、系统表或指标实现。列/索引任务的真实算法边界分别由 `analyze_columns` 和 `analyze_index` 注入；相邻实现分布在 `pkg/executor/analyze_col.rs`、`analyze_idx.rs`、`analyze_worker.rs`、`analyze_global_stats.rs` 和 `analyze_utils.rs`。预刷测试回退还直接使用标准库 `TcpStream::connect_timeout` 探测 TiDB status RPC 地址。

## 错误处理与边界

canonical 路径在每个有副作用的主要阶段前检查 kill，并在预刷后复查 context；保存成功而发布失败仍会记录失败。panic payload 通过 `crate::analyze_utils::getAnalyzePanicErr` 规范化，两个 failpoint 常量覆盖 analyze worker 与单线程结果处理位置。

旧式路径中，广播遇到包含 `exec type` 与 `doesn't support yet` 的滚动升级兼容错误会降级继续，其他广播错误传播。锁表不是错误：任务被跳过并产生 warning。插入作业失败和历史统计登记失败只写日志；保存 analyze options 失败只追加 warning；统计结果保存错误、kill、worker/handler panic、零构建并发、全局合并失败和最终 update 失败则终止执行。

通道断开或退出时，`trySendAnalyzeResult` 优先使用 context cause，其次使用 kill signal 当前错误，最后回退为 `query interrupted` 并结束作业。代码中若任务类型和可选 executor 不一致会 `expect` panic，这是构造 `analyzeTask` 必须保持的内部不变量。`Mutex` poisoned 也通过 `expect` 视为不可恢复的内部错误，而非业务错误。

测试环境的 stats-delta 回退只在 `cfg!(test)` 下启用：注册节点为空、地址为空/非法、上下文已取消或任一 50ms TCP 探测失败都会认为不能广播，继而按计划中的表和全部分区物理 ID 去重后调用 `dump_stats_delta`。

## 并发与资源生命周期

`RunCanonicalWithContext` 的保存阶段使用 scoped thread，因此返回前必定 join；panic 会在 join 点转换为 `AnalyzeError`。旧式管线的 build worker 数受 `build_stats_concurrency` 与任务数共同限制，task channel 容量等于 build 并发，result channel 容量为 1，save channel 容量等于 save 并发，形成背压而非无界堆积。

`buildAnalyzeKillCtx` 另起 watcher 线程等待 `killSignal`；`analyzeStop` 随 `next_inner` 退出而取消上下文，使 watcher 可结束。worker 和 handler 使用 `thread::scope`，局部借用不会越过主调用；结果处理 scope 结束前会 drop `save_sender`，促使 save workers 排空后退出。`Next` 在所有路径上 detach 每个列执行器的内存 tracker，这是列采样内存归还边界。

结果 handler 检测到 kill、保存通道退出或 worker 错误时保留最后的分析错误；保存错误按文本放入 `BTreeSet` 去重并优先返回。`panic_count` 允许在所有 build workers 都以 panic 错误退出后停止等待，避免结果通道生命周期造成死锁。动态全局统计映射只由 handler 更新，但通过 `Arc<Mutex<_>>` 在 handler 与主线程合并阶段安全移交。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/analyze.go`。Rust 的 `flushStatsDeltaForAnalyze`、广播兼容降级、测试环境 RPC 探测和目标 ID 收集与 Go 同名函数保持阶段和边界一致；差异是 Go 用 AST restore SQL、`context.Context` 和真实 infosync/domain，Rust 通过轻量数据结构与 `analyzeRuntime` 注入这些能力。

Rust `Next`、`filterAndCollectTasks`、`saveAnalyzeOptions`、`handleResultsErrorWithConcurrency`、`buildAnalyzeKillCtx`、`analyzeWorker`、`finishJobWithLog`、`handleGlobalStats` 对应 Go 同名逻辑。线程、`mpsc`、`AtomicBool`/`Mutex` 分别替代 goroutine、channel close、context/errgroup 和 map；关键语义仍是有界并发、失败时停止派发、作业完整收尾、保存错误去重、历史统计 best-effort、动态分区全局统计合并和选项持久化。

仍需注意当前接线差异：Go `AnalyzeExec.Next` 是执行器入口；Rust SQL 会话目前走 `RunCanonicalWithContext`，由 `SessionAnalyzeRuntime` 自行准备、保存和发布 `CanonicalAnalyzeBatch`。因此扩展生产 ANALYZE 行为时必须先判断变化属于 canonical runtime、批次顺序，还是仅属于尚未成为会话入口的 Go 对齐 worker 管线，不能只改同名 Rust `Next`。

`pkg/executor/analyze_test.rs` 与 Go `pkg/executor/analyze_test.go` 共享多组测试意图：索引全局统计拆键、动态分区构建并发上限、保存失败不挂起、预刷使用语句 context、kill 中断不挂起、列 tracker 及时释放。Rust 另外直接验证了动态/静态分区选项过滤、缺省值写 `DEFAULT` 和 reset 分区覆盖；panic/OOM 的集成覆盖位于 `pkg/executor/test/analyzetest/panictest/panic_test.rs`。

## 扩展指南

- 新增 canonical 阶段或改变提交原子性时，修改 `RunCanonicalWithContext`、`CanonicalAnalyzeRuntime` 以及 `pkg/session/runtime/statistics.rs::SessionAnalyzeRuntime`；补充会话层成功、kill、保存失败、发布失败与 panic 测试，并明确失败后是否允许部分可见。
- 新增列/索引任务类型或结果字段时，更新 `taskType`、`analyzeTask`、`analyzeResults`、`getTableIDFromTask`、`analyzeWorker` 和 `handleGlobalStats`，同步 `pkg/executor/analyze_test.rs` 与 Go 同名逻辑；必须维持任务类型与对应 executor 存在性的内在约束。
- 修改并发策略时同时审查 task/result/save 三层容量、`errExitCh`、`panic_count` 和作业补收尾逻辑。性能风险主要是过小通道造成吞吐下降、锁住 receiver 时串行化，以及保存并发放大系统表写压力；正确性风险是提前退出导致任务或作业永不结束。
- 修改选项持久化时同步 reader 的 unset sentinel、`mysql.analyze_options` 默认值以及 Go `writeSavedAnalyzeOption`/`resetAnalyzeOptionsForPartitions`。兼容风险包括把未设置值固定化、动态/静态裁剪切换后旧分区覆盖复活，以及 SQL 转义错误。
- 修改取消或 panic 处理时保留原始 cause，确保 dropped result、未发送任务和当前作业各自只完成一次；同步 `pkg/executor/test/analyzetest/panictest/panic_test.rs` 与不挂起测试。
- 修改预刷逻辑时同步 `collectStatsDeltaFlushObjectsForAnalyze`、测试回退目标 ID 和滚动升级兼容判断；50ms 探测只用于测试路径，不应被误用为生产健康检查。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；查询 `AnalyzeExec` 定位 Rust/Go 定义，`node RunCanonicalWithContext` 核实 canonical 阶段和下游 trait 调用，`node next_inner` 核实 `Next` 上游与 worker/handler/合并调用边，`node flushStatsDeltaForAnalyze` 核实预刷调用链，`node handleResultsErrorWithConcurrency` 核实结果保存、作业收尾、全局统计和历史统计调用边。
- 已读生产路径：`pkg/executor/analyze.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`、`pkg/session/runtime/statistics.rs`；`pkg/executor` 下不存在 `doc.go`，因此包契约取自 crate 根文档与模块声明。
- 已读 Go 对照：`pkg/executor/analyze.go` 中的预刷、`AnalyzeExec.Next`、锁表过滤、选项保存、结果处理、取消、worker、作业和全局统计实现。
- 已读独立测试：`pkg/executor/analyze_test.rs`、Go `pkg/executor/analyze_test.go`，并定位 `pkg/executor/test/analyzetest/panictest/panic_test.rs` 的 panic/OOM 集成覆盖。
- 人工复核结论：本文件存在的原因是集中约束 ANALYZE 的阶段顺序、失败/取消语义和并发资源生命周期；当前生产入口与 Go 对齐旧式入口已明确区分，扩展点及需要同步的测试、兼容性和性能风险均有具体符号与路径依据。
