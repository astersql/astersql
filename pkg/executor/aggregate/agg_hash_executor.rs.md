# `pkg/executor/aggregate/agg_hash_executor.rs`

## 文件定位

本文件属于 `astersql-executor-aggregate` crate，是 Rust 聚合子模块中负责组织 Hash Aggregation 的执行器实现。模块入口 `pkg/executor/aggregate/lib.rs` 将它公开为 `agg_hash_executor`；输入、分组键、聚合状态等轻量数据模型来自同 crate 的 `agg_util.rs`，Partial/Final 两阶段计算分别委托给 `agg_hash_partial_worker.rs` 和 `agg_hash_final_worker.rs`，可选落盘由 `agg_spill.rs` 提供。

当前 Rust 实现是一个可直接驱动的进程内执行器：调用方预先把所有 `Chunk` 放入 `HashAggInput`，再依次调用 `open`、反复调用 `next`、最后调用 `close`。RustCodeGraph 显示该文件直接被 `agg_hash_executor_test.rs`、`pkg/executor/benchmark_test.rs` 和 `pkg/executor/internal/exec/adaptive_limit_controller_test.rs` 使用；路径搜索还确认 `pkg/executor/test/aggregate/aggregate_test.rs` 与 `agg_spill_test.rs` 构造它。没有发现生产计划构建器把它接入 Rust `Executor` trait 或按需拉取子执行器，因此它不能被描述为 Go SQL 执行主链的完整替代品。

文件前半部第 21—232 行保留了 Go `HashAggInput`、`HashAggExec.Close` 及并发拓扑的注释化迁移材料；真正参与 Rust 编译的实现从第 233 行的 `use` 开始。

## 核心职责

- `HashAggExec` 管理执行生命周期与一次性计算：`open` 复位状态，首次 `next` 触发完整聚合，后续 `next` 从内存结果队列逐批返回，`close` 清理队列并提交可选 hash-state 统计。
- `execute` 按 `partial_concurrency` 将输入 chunk 轮询分桶，在 scoped threads 中并行构建 Partial 聚合状态，再按 group-key 哈希将中间结果路由到 `final_concurrency` 个 Final worker。
- 未触发 spill 时，Final worker 直接合并 Partial map；任一 Partial worker 触发 spill 后，所有仍在内存中的 Partial map 也统一写入共享 spill helper，避免同一 group 一部分在内存、一部分在磁盘而产生重复结果。
- spill 数据按分区恢复并合并，最终结果按 `max_chunk_size` 切分为 `FinalResult`，存入 `results: VecDeque<FinalResult>`。
- 在无 `GROUP BY` 且所有输入 chunk 都为空时创建一个空 group，使 `COUNT`、`SUM` 等聚合仍产生一行默认聚合结果；有分组列的空输入不创建该行。
- 可选把各 Final worker 完成构建的 group 数写入 `HashStateRuntimeStats`，并在 `close` 时注册到外部 `RuntimeStatsColl`。

## 主要符号

- `pub struct HashAggInput`：完整输入快照，包含 `chunks: Vec<Chunk>`、`group_columns: Vec<usize>` 和 `aggregations: Vec<Aggregation>`。它与 Go 同名结构的“单个 chunk + 返还 channel”含义不同。
- `pub struct HashAggExec`：执行器主体。配置字段包括 Partial/Final 并发度、最大输出 chunk 大小和可选 spill 限额；生命周期字段为 `opened`、`executed`；结果与统计字段为 `results`、`runtime_stats`、`hash_state_stats`、`runtime_stats_coll` 和 `plan_id`。
- `pub fn new(...) -> Self`：构造执行器，把两个并发度和 `max_chunk_size` 都归一化为至少 1；`spill_limit: None` 禁用 spill。
- `pub fn with_runtime_stats(...) -> Self`：builder 风格地绑定 plan ID 与共享 `RuntimeStatsColl`，使下一次 `open` 创建 typed hash-state 统计。
- `pub fn open(&mut self)`：清空旧结果、复位 `executed` 和运行时统计、设置 `opened = true`；只有已绑定统计集合时才创建 `HashStateRuntimeStats`。
- `pub fn next(&mut self) -> Result<Option<Chunk>, String>`：未打开时报错；第一次调用执行 `execute`，成功后设置 `executed`，再从队首返回 chunk、传播 `FinalResult.error`，队列耗尽返回 `Ok(None)`。
- `fn execute(&mut self) -> Result<(), String>`：文件的核心私有入口，负责 worker 编排、spill 收敛、空输入语义、统计和结果生成。
- `pub fn close(&mut self)`：注册并取走 hash-state 统计，清空队列，复位打开/执行状态。互斥锁中毒会在此处通过 `expect` panic。
- `pub fn is_spill_triggered(&self) -> bool`：测试/诊断接口，以 `runtime_stats.spill_count > 0` 判断本轮是否发生过 spill。

## 执行流程

1. 调用方用 `HashAggInput` 提供所有输入批次、分组列下标和聚合描述，并用 `HashAggExec::new` 配置并发度、输出批大小、spill 限额；需要 hash-state 统计时再调用 `with_runtime_stats`。
2. `open` 清除上轮结果和统计。`next` 若发现 `opened == false`，立即返回 `"hash aggregate is not open"`；否则首次调用进入 `execute`。
3. `execute` 克隆聚合描述并包装为 `Arc<Vec<Aggregation>>`；若配置了限额，则以 Final 并发度作为分区数构造一个共享 `ParallelHashAggSpillHelper`。
4. Partial worker 数取 `min(partial_concurrency, max(input.chunks.len(), 1))`。输入 chunk 按下标模 worker 数轮询分桶，所以并行单位是 chunk，而不是行；即使没有输入 chunk，也会建立一个空 bucket。
5. `std::thread::scope` 为每个 bucket 启动线程。线程内构造 `HashAggPartialWorker`，逐 chunk 调用 `update_partial_result`，再调用 `shuffle_intermediate_data(final_concurrency)`，得到每个 Final worker 对应的一份 `AggMap`。scope 返回前会 join 全部线程；线程 panic 被转换为 `"partial aggregate worker panicked"`。
6. 主线程构造 `final_concurrency` 个 `HashAggFinalWorker`。如果共享 helper 状态已不再是 `NoSpill`，每份非空 Partial 输出都继续调用 `spill`；否则按 worker 位置 `zip` 后调用 `merge_input`。这个“全量走同一路径”的分支保证同一 group 不会横跨内存 Final map 和磁盘恢复流。
7. 只有第一个 Final worker 调用 `restore_from_disk`。这是有意设计：helper 的 `next_partition` 是单消费游标；聚合最终值与落在哪个 Final worker 无关，恢复到一个 worker 可避免多个消费者争抢分区。
8. helper 曾触发 spill 或仍有磁盘数据时，`runtime_stats.spill_count` 加一。无分组且全部输入为空时，向第一个 Final worker注入一个 key 为空、每个聚合都为初始 `AggState` 的 map。
9. 对每个 Final worker，先把 `hash_state_rows()` 累加到 typed 统计，再调用 `generate_result(max_chunk_size)`；生成的 `FinalResult` 依次进入 `results`。
10. `next` 从 `results` 队首取结果；完成一次 `execute` 后不会重复计算。`close` 后再次 `next` 会因 `opened == false` 失败；重新 `open` 会允许同一份不可变输入再次执行。

## 数据与状态

`Chunk` 在 `agg_util.rs` 中是 `Vec<Row>`，`Row` 是 `Vec<Value>`；`Value` 支持 Null、整数、浮点、文本、字节和布尔。`Aggregation` 描述聚合种类、输入列以及 DISTINCT 标志。Partial worker 用 `AggMap = BTreeMap<Vec<u8>, (Row, Vec<AggState>)>` 保存“编码 group key → 原分组列值 + 每个聚合的中间态”，Final worker按 key 合并这些状态。

配置在构造后保持不变：`partial_concurrency`、`final_concurrency`、`max_chunk_size` 均至少为 1；`spill_limit` 为 `Some` 时才建立 helper。输入被执行器拥有，但 `execute` 会克隆输入 chunk、分组列和聚合描述以安全送入线程；这让并发所有权简单，也意味着内存峰值包含输入副本。

生命周期由两个布尔量区分：`opened` 决定 `next` 是否合法，`executed` 决定是否需要进行一次完整计算。结果是预先物化的 `VecDeque`，因此 `next` 并非边计算边流式输出。`runtime_stats.spill_count` 是本文件实际更新的简化统计；`hash_state_stats` 则记录所有 Final map 的 group 条目数，直到 `close` 才注册。

spill helper 内部以 `Mutex<PartialResultSpill>` 保护磁盘存储，以原子状态记录 `NoSpill/NeedSpill/Spilling/Triggered`，并以原子递减游标从最高分区恢复到 0。Partial worker按当前 `AggState` 的堆内存估算触发阈值，落盘成功后交出 map 并把自身内存计数清零。

## 依赖与调用关系

上游调用模式由独立测试与基准给出：`agg_hash_executor_test.rs::spill_merges_tail_rows_with_their_existing_group` 绑定统计后按 `open → next* → close` 驱动；`pkg/executor/test/aggregate/aggregate_test.rs::hash_rows` 是不绑定统计的通用驱动；`pkg/executor/benchmark_test.rs` 在多处构造该执行器。RustCodeGraph 的文件关系还列出 `adaptive_limit_controller_test.rs`，但没有发现生产 Rust 计划构建器直接构造本类型。

`execute` 的主要下游调用边为：

- `HashAggPartialWorker::new → update_partial_result → shuffle_intermediate_data`：编码分组键、更新 `AggState`，必要时调用 `ParallelHashAggSpillHelper::set_need_spill/spill`，最后按 Murmur3 key 哈希分发。
- `HashAggFinalWorker::new → merge_input`：校验 partial 状态宽度，并逐聚合调用 `AggState::merge`。
- `HashAggFinalWorker::restore_from_disk`：循环调用 helper 的 `next_partition/restore_partition`，恢复并合并落盘 map。
- `HashAggFinalWorker::generate_result`：对各状态调用最终值生成逻辑，并按最大行数切 chunk。
- `HashStateRuntimeStats::AddRows` 与 `RuntimeStatsColl::RegisterStats`：分别发生在结果构建和关闭阶段。

`Cargo.toml` 将 crate 根设为同目录 `lib.rs`。本文件实际使用的 crate 级外部依赖是 `astersql-util-execdetails`；其余核心类型来自同 crate 模块，而这些模块再使用 `astersql-executor-aggfuncs`、`astersql-util-chunk` 和 `astersql-util-serialization`。Cargo 还列出一组仅 Windows 目标启用的迁移依赖，但本文件当前可编译实现没有相应条件编译分支。

## 错误处理与边界

- `next` 在未 `open` 或已 `close` 状态下返回明确字符串错误；`open` 本身不返回 `Result`，也不验证列下标或聚合描述。
- 非法分组列下标由 Partial worker 的 `get_group_key`/分组行提取路径返回，例如 `"group column 1 out of range"`；`aggregate_test.rs::test_random_panic_consume` 覆盖了错误上抛和关闭后拒绝读取。
- Partial 线程返回的业务错误通过 join 结果汇总；线程 panic 被统一转换为字符串错误。Final 阶段当前不启动线程，因此不存在 Final thread panic 的转换路径。
- Final merge 会拒绝中间态数量与目标/聚合描述数量不一致，返回 `"partial result width mismatch"`。spill 的锁中毒、序列化、磁盘读写和恢复错误以 `String` 传播；不过若恢复值类型不符，`expect("restored aggregate state type")` 会 panic。
- `close` 注册统计时使用 `Mutex::lock().expect(...)`，锁中毒会 panic，而不是返回错误；没有绑定统计集合时该分支被跳过。`close` 可重复调用，第二次没有待注册统计，测试 `aggregate_test.rs::test_issue50849` 验证了幂等关闭。
- `spill_limit` 的触发比较来自 helper：聚合估算内存达到限额五分之一即可置 `NeedSpill`。限额极小会很早触发；`None` 完全禁用执行器级 spill。
- 输出 group 顺序不能作为接口保证。虽然单个 `AggMap` 是 `BTreeMap`，输入被分桶且结果按 Final worker顺序拼接；测试在比较多组输出时会显式排序。
- 空输入边界只有“无分组列”才产生默认一行；有分组列返回空结果。空输入检测是“所有 chunk 都为空”，空 `chunks` 也满足该条件。

## 并发与资源生命周期

并发只发生在 Partial 阶段。`std::thread::scope` 允许线程借用 scope 生命周期内的值，并保证 `execute` 离开 scope 前所有线程完成；因此不会留下后台 worker，也不需要 Go 版本的 finish channel、WaitGroup 或 Close 时异步排空。Final worker创建和合并均在调用 `next` 的线程串行完成。

聚合描述、分组列和可选 spill helper分别通过克隆或 `Arc` 共享。spill helper 的磁盘存储用 `Mutex` 串行化访问，状态与分区游标用原子变量协调；但本文件仅由第一个 Final worker恢复，主动维持单消费者约束。`PartialResultSpill` 的 `Drop` 会关闭所有磁盘文件，恢复单个分区也会在读取后关闭并清空对应文件状态，因此 helper 的最后一个 `Arc` 释放时完成兜底清理。

`results` 拥有所有未消费的输出 chunk；`close` 直接丢弃它们。执行器没有取消正在执行的能力，因为首次 `next` 会同步等到所有 Partial thread join、Final 合并和结果物化完成后才返回。大量输入、输入克隆、完整结果物化以及 Final 串行合并都是扩展时需要评估的内存/延迟边界。

统计集合由外部以 `Arc<Mutex<RuntimeStatsColl>>` 持有。`open` 创建本轮统计，`execute` 累加行数，`close` 通过 `take` 确保每轮最多注册一次；如果调用方执行完成后不调用 `close`，typed hash-state 统计不会进入集合。

## 与 Go 版本的对应关系

语义主线与 `pkg/executor/aggregate/agg_hash_executor.go` 一致：都是 Partial 更新、按 key 路由、Final 合并、内存压力下 spill/restore，并保留“无 GROUP BY 的空输入仍输出默认聚合行”和 hash-state 行数统计。Rust 中“发生任意 spill 后把所有 Partial 剩余 map 也落盘”的注释与 Go `fetchChildData` 收尾调用 `spill()` 的意图对应；分区游标的逆序单次恢复也对应 Go spill helper 的消费模型。

但两者并非逐字段或逐调度机制等价：

- Go `HashAggInput` 是 data fetcher 与 Partial worker 间的一次 chunk/channel 消息；Rust `HashAggInput` 是整个执行所需的所有 chunk、分组列和聚合描述。
- Go `HashAggExec` 嵌入 `BaseExecutor`，从 child executor 拉取数据，并根据 `IsUnparallelExec` 选择完整的单线程或 goroutine/channel 并行路径；Rust 不实现该接口、不拉取 child，也没有独立串行模式。
- Go 并行路径有 input reader、M 个 Partial worker、N 个并发 Final worker、多个 channel、finish 信号、WaitGroup、内存/磁盘 tracker、panic recovery 和 failpoint；Rust 只有 scoped Partial threads，Final 阶段和结果收集同步串行，也没有取消/failpoint 接线。
- Go 从 session variables取得并发度和 spill 开关，并由内存 tracker 的超限 action 触发落盘；Rust 由构造参数直接决定，使用简化的 `usize` 内存估算和固定的五分之一阈值。
- Go 同时支持非并行模式的多轮“保留已有 group、把新 group 原始行落盘再读回”算法；Rust 只实现并行 Partial 状态整体序列化/恢复模型。
- Go 运行时统计包含 Partial/Final worker 时间与任务数并在 Close 注册；Rust 本文件只更新 spill 次数以及 typed hash-state rows。前半部注释保存的 Go 字段和 `Close` 伪代码不是 Rust 已实现能力。

因此后续迁移应以 Go 文件为行为规格，但必须区分“当前 Rust 可运行核心”与“注释化待接线控制流”，不能仅凭同名类型声称已完成生产主链替换。

## 扩展指南

- 新增生命周期行为时优先修改 `HashAggExec::{open,next,execute,close}`，并保持 `opened/executed` 的状态约束；同步在独立的 `agg_hash_executor_test.rs` 或 `pkg/executor/test/aggregate/aggregate_test.rs` 增加测试，禁止把测试内嵌进本源文件。
- 新增聚合种类或改变中间态时，通常需要同步 `agg_util.rs::{Aggregation,AggState}`、Partial 更新、Final merge，以及 `agg_spill.rs::SpillEntry` 的序列化兼容性。尤其要验证 DISTINCT 状态在跨 Partial worker和 spill 后只计一次。
- 改变 worker 路由时必须保持同一 group 的所有内存中间态落到同一 Final worker；改变 spill 路径时必须保持“触发后统一落盘”不变量，否则同一 group 可能输出两行或漏合并。相关回归入口是 `agg_hash_executor_test.rs::spill_merges_tail_rows_with_their_existing_group` 与 `agg_spill_test.rs::distinct_growth_in_one_group_spills_and_merges_overlapping_workers`。
- 若要并行化 Final 阶段，需重新设计 helper 的单消费 `next_partition`、结果队列同步、错误/panic 汇聚和统计一致性；不能简单地让所有 Final worker同时调用当前恢复接口。
- 若要接入生产 Rust Executor 主链，需要补齐按需 child 拉取、取消/关闭、执行上下文、内存与磁盘 tracker、session 配置、错误类型和计划构建器接线，并对照 Go `Open/OpenSelf/Next/parallelExec/unparallelExec/Close`。这属于比本文件当前轻量接口更大的迁移范围。
- 调整结果顺序前先明确是否要建立新的稳定顺序契约；现有测试通常排序后比较，依赖 `BTreeMap` 的局部顺序并不足以保证跨 Final worker的全局顺序。
- 性能修改要同时检查输入全量克隆、结果全量物化、Final 串行合并和 spill `Mutex` 争用；正确性修改还需覆盖空输入、有/无分组、非法列、重复 `close`、多 chunk 同 group、DISTINCT 与极低 spill 限额。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件且目标文件已索引；`files --filter pkg/executor/aggregate` 确认聚合模块的 Rust/Go 对照文件；`node --file ... --symbols-only` 列出 `HashAggInput`、`HashAggExec`、`new`、`with_runtime_stats`、`open`、`close`、`next`、`execute`、`is_spill_triggered`；整文件 node 输出确认目标文件由 3 个文件直接使用。精确 `callers next --file ...` 在大型索引中持续超过 90 秒未返回，已终止，并以索引给出的 used-by 关系和精确路径搜索补齐调用证据。
- 主源码：`pkg/executor/aggregate/agg_hash_executor.rs` 第 241—455 行；直接依赖实现：`agg_hash_partial_worker.rs` 的 `HashAggPartialWorker`，`agg_hash_final_worker.rs` 的 `HashAggFinalWorker/FinalResult`，`agg_spill.rs` 的 `ParallelHashAggSpillHelper/SpillStatus`，`agg_util.rs` 的 `Chunk/AggMap/AggState/Aggregation/HashAggRuntimeStats`。
- crate 边界：`pkg/executor/aggregate/Cargo.toml` 与 `pkg/executor/aggregate/lib.rs`。目标包及其上级 `pkg/executor` 未发现 `doc.go`，因此无额外包契约可读。
- Rust 测试：`pkg/executor/aggregate/agg_hash_executor_test.rs` 验证 spill 后尾部重复 group 合并及 close 时统计注册；`agg_spill_test.rs` 验证普通/空输入、磁盘写入、DISTINCT 增长触发 spill 等边界；`pkg/executor/test/aggregate/aggregate_test.rs` 验证 DISTINCT、空输入、spill 阈值、错误传播、并行分组和重复关闭；`pkg/executor/benchmark_test.rs` 提供基准调用入口。
- Go 对照：`pkg/executor/aggregate/agg_hash_executor.go` 的 `HashAggInput`、`HashAggExec`、`Open/OpenSelf`、`initForParallelExec`、`Next`、`fetchChildData`、`spillIfNeed/spill`、`prepare4ParallelExec`、`parallelExec/unparallelExec`、`execute`、`getNextChunk`、`initRuntimeStats`；Go spill 回归位于 `pkg/executor/aggregate/agg_spill_test.go`，执行器生命周期/故障注入回归位于 `pkg/executor/test/aggregate/aggregate_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构命令，并人工复查文档只陈述上述源码、调用边、Cargo 和测试能够支持的事实。
