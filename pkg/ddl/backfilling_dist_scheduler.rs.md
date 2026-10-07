# `pkg/ddl/backfilling_dist_scheduler.rs`

## 文件定位

本文件属于 `astersql-ddl` crate，由 [`pkg/ddl/lib.rs`](lib.rs) 以 `pub mod backfilling_dist_scheduler` 公开。它移植了分布式 DDL 回填调度器中可独立表达的状态机、Region 分批、全局排序计划、临时索引合并计划和重试判定逻辑；任务与子任务的共享元数据来自 [`backfilling_dist_executor.rs`](backfilling_dist_executor.rs)，排序结果摘要来自 [`backfilling_read_index.rs`](backfilling_read_index.rs)。

它位于 DDL job 的 reorg/backfill 路径：概念上接收任务元数据及上一步子任务输出，生成下一阶段的 `BackfillSubTaskMeta`。但当前 Rust 代码库中没有生产调用者：RustCodeGraph 将本文件标为仅被 [`backfilling_dist_scheduler_test.rs`](backfilling_dist_scheduler_test.rs) 使用，仓库搜索也只发现测试引用；完整的 DXF `scheduler.Extension` 接线、存储访问、对象存储 I/O 和子任务元数据序列化仍由同路径 [`backfilling_dist_scheduler.go`](backfilling_dist_scheduler.go) 承担。因此本文件目前是“已实现并有单测的纯计划层”，不是可独立运行的完整调度器。

## 核心职责

- `LitBackfillScheduler` 根据 `BackfillTaskMeta` 选择三条阶段路径：本地 ingest 为 `Init -> ReadIndex -> Done`；云端全局排序为 `Init -> ReadIndex -> MergeSort -> WriteAndIngest -> Done`；临时索引合并为 `Init -> MergeTemporaryIndex -> Done`。
- `generate_plan_for_physical_table` / `try_generate_plan_for_physical_table` 把单个物理表的连续 Region 范围分批，给每批生成独立快照时间戳和读索引子任务。
- `generate_merge_sort_plan` 根据排序文件重叠度决定是否跳过 merge-sort；不能跳过时调用 `astersql_ingestor_globalsort::DivideMergeSortDataFiles` 分配文件。
- `merge_meta_groups`、`plan_global_sort_ingest` 和 `split_subtask_meta_for_one_kv_group` 汇总前序排序结果，切分 write-and-ingest 范围，并只在计划成功后写入任务摘要。
- `generate_temporary_index_plan` 按 Region 切分临时索引 key 范围，生成最终合并子任务。
- `retry_region_plan` 对 Region 扫描期间的拓扑不连续和 TSO 分配失败执行最多八次的整轮重试，防止发布半成品计划。
- `modify_meta` 与错误可重试判定为 DXF 动态调参和调度失败处理提供纯逻辑实现。

## 主要符号

- `Modification::{BatchSize, MaxWriteSpeed, Unknown}`：运行中允许修改的任务参数。`modify_meta` 直接更新 `task_meta.batch_size` 或 `max_write_speed`，未知项无动作；`BatchSize(0)` 被原样保存，以保留 Go 侧“回退到配置值”的兼容语义。
- `LitBackfillScheduler`：持有 `global_sort`、`merge_temporary_index`、节点 CPU/内存/磁盘估计和 `task_meta`。`new` 通过非空 `cloud_storage_uri` 推导全局排序，并给资源字段设置 `4 CPU / 16 GiB / 100 GiB` 默认值；当前文件中的计划函数尚未读取这些资源字段。
- `LitBackfillScheduler::get_next_step`：纯状态转移函数；`Done` 幂等地返回 `Done`，未知状态在 Rust 枚举中不可构造。
- `LitBackfillScheduler::plan_global_sort_ingest`：先调用 `merge_meta_groups`，以 wrapping addition 计算所有组的 KV 总大小，再调用注入的 `build_plan`；只有后者成功才写入 `task_meta.summary`，因此失败不会留下错误的成功摘要。
- `LitBackfillScheduler::{is_retryable_error,is_retryable_scheduler_message}`：只有 `[GlobalSort:TooManyDataFiles]` 永久错误不可重试；前者沿 error source 链识别规范错误，后者用于 DXF 仅持久化错误文本的场景。
- `PlanError`：覆盖零节点、merge-sort 底层错误、TSO/Region 扫描失败、空或不连续 Region、非法范围、元数据组错位和索引缺失。`MergeSort` 保留底层 error source，其余变体用调试形式显示。
- `RegionMeta`：左闭右开 Region key 范围 `[start_key, end_key)`；`calculate_region_batch` 计算读索引阶段批大小。
- `generate_plan_for_physical_table`：无失败 TSO 适配入口；把 `FnMut() -> u64` 包装后委托给可失败版本。
- `try_generate_plan_for_physical_table`：完整读索引计划器；排序和校验 Region、分批、逐批分配 TSO，并把首尾批次收敛到真实表范围。
- `MultipleFilesStat` / `skip_merge_sort`：分别描述排序文件列表与最大重叠层数，并以 `max(concurrency, 1) * 2` 为跳过阈值。
- `generate_merge_sort_plan`：按索引组收集文件并生成 merge-sort 子任务；每个子任务携带对应的单个 `element_id`（若该位置存在）。
- `RangeSplit` / `split_subtask_meta_for_one_kv_group`：描述 write-and-ingest 的一个范围切片，并补齐 range job keys、Region split keys、文件、TSO 和 KV 摘要。
- `merge_meta_groups`：按位置合并多个子任务的 `SortedKvMeta`，元素 ID 取首个子任务；分组数不一致时拒绝合并。
- `find_index_infos_by_ids`：保持请求顺序校验索引 ID，缺失即返回 `IndexNotFound`。
- `calculate_temporary_index_region_batch` / `generate_temporary_index_plan`：按节点平均切分临时索引 Region；参数 `_index_id` 当前故意未写入 `element_ids`，目标由编码后的临时索引 key 范围表达。
- `retry_region_plan`：重载 Region 后调用计划器，仅重试 `RegionsNotContinuous` 和 `TimestampAllocation`。

文件没有 trait 实现、宏或条件编译项；公开符号都是 crate 外可见 API，辅助行为主要封装在 `LitBackfillScheduler` 的方法中。

## 执行流程

1. 创建 `LitBackfillScheduler` 时，从 `BackfillTaskMeta` 判定本地、全局排序或临时索引合并模式。
2. 框架概念上调用 `get_next_step` 决定下一阶段；Rust 当前没有实现 Go 侧 `scheduler.Extension::OnNextSubtasksBatch` 的运行时接线。
3. `ReadIndex` 阶段对每个物理表调用 `generate_plan_for_physical_table`：空表返回空计划；非空表要求合法表范围和非空、连续 Region；然后按节点数与存储模式计算批大小，为每批分配 TSO，生成 `row_start/row_end` 子任务。
4. Region 扫描与计划构建应由 `retry_region_plan` 包裹。每次可重试失败都会重新执行 `load_regions`，等待时间依次为 200、400、800、1600、2000、2000、2000、2000 毫秒；最多八次，返回最后一次错误。已经生成的局部向量不会跨尝试发布。
5. 本地模式在 `ReadIndex` 后完成。全局排序模式先汇总 read-index 输出的文件重叠统计；若每组都满足阈值则 `generate_merge_sort_plan` 返回空计划，框架仍可进入 write-and-ingest 并读取 read-index 输出。
6. 需要 merge-sort 时，每个索引组的文件交给 `DivideMergeSortDataFiles`，输出若干携带 `data_files` 和 `element_ids` 的子任务。
7. write-and-ingest 规划先用 `merge_meta_groups` 汇总 merge-sort 输出（该阶段被跳过时应汇总 read-index 输出），再由外部 range splitter 形成 `RangeSplit`；`split_subtask_meta_for_one_kv_group` 逐段补齐闭合边界数组并生成 ingest 子任务。
8. `plan_global_sort_ingest` 在外部计划闭包成功后更新 `BackfillTaskSummary.index_kv_size`；失败则任务元数据保持原样。
9. 临时索引模式跳过普通读索引与全局排序，`generate_temporary_index_plan` 将临时索引整体范围按连续 Region 和节点数拆开，然后直接完成。

## 数据与状态

- `BackfillTaskMeta` 是任务级状态：本文件读取 `cloud_storage_uri`、`merge_temporary_index`，修改 `batch_size`、`max_write_speed` 与可选 `summary`。它的版本与序列化定义在 `backfilling_dist_executor.rs`，不是本文件所有。
- `BackfillSubTaskMeta` 是阶段间载荷。读索引路径写入 `physical_table_id`、`row_start`、`row_end`、`ts`；merge-sort 写入 `data_files`、`element_ids`；write-and-ingest 写入范围 keys、数据/统计文件、`meta_groups`、`element_ids` 与 `ts`；临时索引路径写入 `physical_table_id` 和 `legacy_sorted_kv_meta`。
- 所有 key 都是 `Vec<u8>`（`backfilling::Key`），范围统一按左闭右开解释。计划器依赖字节序比较，调用方必须提供同一编码域中的 key。
- Region 先按 `start_key` 排序，再验证相邻 `end_key == next.start_key`。这只保证给定 Region 序列内部无空洞；首尾是否覆盖目标表范围由首尾子任务强制改写，而不是额外检查 Region 外边界。
- 本地磁盘读索引批次最多按三个节点均摊，至少倾向于包含 100 个 Region，但不会超过总数；云存储按全部节点均摊且每批最多 4000 个 Region。
- `SortedKvMeta::merge` 决定跨子任务的 key 范围、文件数与 KV 大小如何合并；本文件只保证不同子任务的分组位置一一对应。
- `LitBackfillScheduler` 本身不含锁、原子变量或内部共享所有权；所有状态修改要求 `&mut self`。

## 依赖与调用关系

上游边界：

- Rust 模块入口是 `pkg/ddl/lib.rs:36`；测试模块在 `pkg/ddl/lib.rs:135` 注册。
- RustCodeGraph 的文件关系显示唯一直接使用者为 `pkg/ddl/backfilling_dist_scheduler_test.rs`；对主要符号执行精确 `query` 成功，但 `callers`/`callees` 命令在当前索引上未返回结果，因此又用仓库级符号搜索确认没有生产 Rust 调用边。
- 实际应用的 Go 上游为 `pkg/ddl/ddl.go` 中的 `newLitBackfillScheduler` 注册，以及 `backfilling_dist_scheduler.go::OnNextSubtasksBatch` 对各计划函数语义对应物的分派。这些是 Go 运行链证据，不代表 Rust 已经接线。

下游依赖：

- `crate::backfilling_dist_executor::{BackfillStep, BackfillSubTaskMeta, BackfillTaskMeta, BackfillTaskSummary}` 提供阶段与元数据协议。
- `crate::backfilling_read_index::SortedKvMeta` 提供排序 KV 摘要及 `merge` 行为；`crate::backfilling::Key` 提供 key 类型别名。
- `astersql-ingestor-globalsort::DivideMergeSortDataFiles` 实施文件分组，并可能返回永久的文件数上限错误；`astersql-ingestor-errdef::IsTooManyDataFilesError` 沿错误链识别该错误。二者均由 `pkg/ddl/Cargo.toml` 的普通依赖声明。
- `std::time::Duration` 只用于把退避策略交给注入的 `wait` 闭包；本文件不直接 sleep。

Go 版本还直接依赖 DXF scheduler/storage、PD/TiKV RegionCache 与 TSO、对象存储、table/meta、failpoint、metrics 和日志。Rust 文件没有这些依赖或 I/O，因此不能据此推断 Rust 已具备生产调度能力。

## 错误处理与边界

- `node_count == 0` 在所有需要分批的路径返回 `NoNodes`，避免除零；读索引空表是特殊成功路径，会在检查节点数前返回空计划。
- 非空读索引要求 `table_start < table_end`；临时索引计划当前没有独立检查 `range_start < range_end`，而是用 Region 批次生成元数据。扩展时不能假设两条路径验证完全相同。
- 空 Region 对非空读索引和临时索引都是 `EmptyRegions`；Region 内部断裂为 `RegionsNotContinuous { expected, actual }`。
- TSO 分配可能在已生成若干批次后失败，但局部 `plan` 只存在于本次函数调用中；配合 `retry_region_plan`，下一次会重新扫描 Region 并从空计划开始。
- `retry_region_plan` 不重试 `RegionScan`、`NoNodes`、`InvalidRange`、merge-sort、元数据组或索引错误。`wait` 自身失败时立即返回其错误。
- merge-sort 在全部组都可跳过时先返回空计划，因此即使 `node_count == 0` 也不会报 `NoNodes`；这是当前代码顺序的真实行为。
- `generate_merge_sort_plan` 对缺失的 `stats_groups[index]` 返回 `EmptyMetaGroup(index)`；若 `element_ids` 较短，该组仍会生成子任务，但 `element_ids` 为空，以兼容旧元数据。
- `split_subtask_meta_for_one_kv_group` 对空首尾 key 返回空计划；零实例返回 `NoNodes`；每段必须严格递增。它信任 `RangeSplit` 的内部 keys 已排序且处于边界内，当前不会逐个验证。
- KV 大小使用 `kv_meta.total_kv_size / instance_count` 写入每个范围子任务，这与范围数量未必相等；它是 Go 语义的估算值，不是精确分摊。总摘要则用 `wrapping_add`，极端溢出会回绕而非报错。
- `find_index_infos_by_ids` 允许重复请求，并保持重复和顺序；只验证存在性。
- `is_retryable_error` 将文件数永久上限以外的所有错误视为可重试；调用方仍需结合任务阶段与重试预算。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。计划输出是拥有所有数据的 `Vec<BackfillSubTaskMeta>`，函数返回后不借用输入；注入闭包都在当前调用栈同步执行。

并发度只作为规划参数：节点数控制 Region 批大小和 merge-sort 文件组数量，`concurrency` 控制跳过 merge-sort 的重叠阈值以及底层文件分组限制。`node_cpu`、`node_memory`、`node_disk` 是保存的资源估计，当前 Rust 文件未据此计算并发。

生命周期上的关键不变量是“完整尝试后发布”：Region/TSO 重试不会复用部分计划，global-sort 摘要也只在计划闭包成功后写入。另一方面，对象存储句柄、range splitter 的关闭、任务元数据持久化、子任务分发与取消/暂停/恢复都在 Go 运行时层，Rust 当前没有对应资源清理逻辑。

## 与 Go 版本的对应关系

同路径 `backfilling_dist_scheduler.go` 是直接对照实现。主要对应关系如下：

- Rust `new` + `get_next_step` 对应 Go `Init` 中的模式推导和 `GetNextStep`，三条状态路径一致。
- Rust `calculate_region_batch` 与 Go `CalculateRegionBatch` 保留本地盘最多三个节点、最小 100 Region，以及云存储最大 4000 Region 的算法；Rust 额外把零节点变为显式错误。
- Rust `try_generate_plan_for_physical_table` 与 Go `generatePlanForPhysicalTable` 的纯计划部分一致：排序、连续性校验、整轮重扫、逐批新 TSO、首尾收敛。Go 负责从 PD RegionCache 加载 Region、从表和 reorg context 取得 key 范围、序列化元数据；Rust 通过闭包和参数注入这些外部操作。
- Rust `retry_region_plan` 对齐 Go `handle.RunWithRetry(..., 8, exponential backoff)`，包括 200ms 起步、2s 封顶和拓扑变化时重新加载 Region。Rust 将 TSO 错误表示为 `TimestampAllocation`，将 scan 错误区分为不重试的 `RegionScan`。
- Rust `skip_merge_sort` / `generate_merge_sort_plan` 对齐 Go `skipMergeSort` / `generateMergeSortPlan` 的阈值与 `DivideMergeSortDataFiles`；Go 还读取外部元数据、写计划到对象存储并记 metrics。
- Rust `merge_meta_groups`、`plan_global_sort_ingest`、`split_subtask_meta_for_one_kv_group` 对齐 Go `generateGlobalSortIngestPlan` 与 `splitSubtaskMetaForOneKVMetaGroup` 的汇总、range splitter 输出消费和成功后摘要更新；对象存储与 splitter 的创建/关闭仍只在 Go。
- Rust `generate_temporary_index_plan` 对齐 Go `generateMergeTempIndexPlan` 的范围切分，并刻意不设置 `EleIDs`；`_index_id` 仅保留接口语义，目标索引由临时 key 范围编码。
- Rust `modify_meta` 对齐 Go `ModifyMeta` 的 batch size / max write speed 修改；Rust 的 `Unknown` 静默忽略，而 Go 会记录 warning 并重新序列化任务元数据。
- Rust `is_retryable_error` 对齐 Go `IsRetryableErr`，并增加文本消息检查以适配 DXF 错误持久化后的场景。

尚未移植到本文件的 Go 职责包括 `scheduler.Extension`/`BaseScheduler` 集成、`OnNextSubtasksBatch`、节点实例发现、NextGen 节点计数、表元数据读取、PD/TSO、RegionCache、对象存储、外部 meta 编解码、日志、指标、failpoint 以及 `OnPrepare`/`OnDone` 等生命周期钩子。扩展 Rust 时应维持这一差异清单，不能用本文件的纯函数测试替代端到端接线证据。

## 扩展指南

- 新增回填阶段时，先扩展 `backfilling_dist_executor.rs::BackfillStep`，再修改 `get_next_step`，并在独立的 `backfilling_dist_scheduler_test.rs` 中覆盖每种模式的完整序列；若要生产可用，还必须补上 Rust DXF 运行时的阶段分派，而不只是状态枚举。
- 修改 Region 分批或范围规则时，集中改 `calculate_region_batch`、`try_generate_plan_for_physical_table` 或 `generate_temporary_index_plan`，保持左闭右开、首尾收敛、连续性校验和每子任务独立 TSO。同步覆盖空表、零节点、非法范围、Region 分裂/合并和重试耗尽。
- 修改重试策略时，以 `retry_region_plan` 为唯一策略入口，明确哪些 `PlanError` 可安全整轮重试；不要在已经发布部分子任务后复用该函数。退避应继续通过 `wait` 注入，便于测试且避免隐藏阻塞。
- 修改全局排序时，同时审查 `skip_merge_sort`、`generate_merge_sort_plan`、`merge_meta_groups`、`split_subtask_meta_for_one_kv_group` 和 `plan_global_sort_ingest`。需要保持旧元数据缺少 element ID 的兼容行为、永久文件上限错误识别，以及摘要“成功后提交”。
- 若加强输入校验，应特别考虑当前未校验的临时索引整体范围、`RangeSplit` 内部 key 排序和 `element_ids`/分组长度关系；收紧行为前必须与 Go 版本和存量元数据兼容性对齐。
- 测试必须继续放在同目录独立文件 `pkg/ddl/backfilling_dist_scheduler_test.rs`，不要内嵌到生产源文件。Go 语义变化还应核对 `pkg/ddl/backfilling_dist_scheduler_test.go`。
- 真正接入 Rust 生产链时，需在独立工作中补齐 scheduler trait、存储/PD/对象存储抽象、序列化与资源关闭，并用集成测试证明任务持久化、owner 切换、暂停/取消与重试；这超出当前文件已实现范围。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/ddl/backfilling_dist_scheduler.rs` 识别 41 个符号及唯一使用文件；`node --file ... --offset 1 --limit 500` 与 `--offset 500 --limit 140` 覆盖全部 587 行；对 `LitBackfillScheduler`、`generate_plan_for_physical_table`、`generate_merge_sort_plan`、`generate_temporary_index_plan` 的 `query` 确认精确符号位置。图的 `explore` 和 `callers/callees` 在本次查询中未产出调用边，故用模块入口与仓库符号搜索补充验证，并明确记录为当前工具限制。
- Rust 源码：`pkg/ddl/backfilling_dist_scheduler.rs`；协议与直接类型：`pkg/ddl/backfilling_dist_executor.rs`、`pkg/ddl/backfilling_read_index.rs`、`pkg/ddl/backfilling.rs`；模块入口：`pkg/ddl/lib.rs`。
- crate 边界：`pkg/ddl/Cargo.toml` 确认包名 `astersql-ddl`、`lib.rs` 入口以及 `astersql-ingestor-errdef`、`astersql-ingestor-globalsort` 普通依赖。
- Rust 独立测试：`pkg/ddl/backfilling_dist_scheduler_test.rs` 覆盖三种状态机、Region 批量、空表、摘要原子更新、临时索引不设置 element ID、零 batch size、TSO/Region 重试与退避、永久/临时错误、merge 文件数边界。
- Go 对照：`pkg/ddl/backfilling_dist_scheduler.go`、`pkg/ddl/backfilling_dist_scheduler_test.go`；生产注册入口 `pkg/ddl/ddl.go:810`。这些文件验证了运行时位置，也证明 Rust 当前只覆盖纯逻辑子集。
- DDL 背景契约：`pkg/ddl/doc.go` 说明 DDL schema version 不变量；`docs/agents/ddl/README.md` 与 `docs/agents/ddl/03-reorg-backfill.md` 仅作为导航，并已用上述代码和测试复核本文件相关结论。
- 本任务是纯文档分析，按总计划不运行 Cargo；交付前使用任务指定命令检查目标文档存在且恰有十一个固定二级标题，并人工复核所有“当前已支持”陈述均有上述直接证据。
