# `pkg/ddl/backfilling_merge_temp.rs`

源码：[backfilling_merge_temp.rs](backfilling_merge_temp.rs)；独立测试：[backfilling_merge_temp_test.rs](backfilling_merge_temp_test.rs)。

## 文件定位

本文件属于 `astersql-ddl` crate（见 [Cargo.toml](Cargo.toml)），描述分布式 ADD INDEX 回填中“合并临时索引”子任务的 Rust 执行器。在线回填期间，并发 DML 的索引变化先进入临时索引；回填后需要把这些变化合并回正式索引。调度侧已经用 `BackfillStep::MergeTemporaryIndex` 表示这个阶段，并在 `LitBackfillScheduler::get_next_step` 中支持 `Init -> MergeTemporaryIndex -> Done`（[backfilling_dist_executor.rs](backfilling_dist_executor.rs)、[backfilling_dist_scheduler.rs](backfilling_dist_scheduler.rs)）。

当前接线范围需要特别区分：`lib.rs` 公开声明了 `backfilling_merge_temp` 模块，RustCodeGraph 显示本文件被 `index.rs` 和独立测试关联，但生产 Rust 源码中没有构造或调用 `MergeTemporaryIndexExecutor`；对该类型的直接调用证据仅见 [backfilling_merge_temp_test.rs](backfilling_merge_temp_test.rs)。因此它是可测试的执行逻辑移植件，尚不能据此声称 Rust DDL 主链已实际驱动该执行器。Go 生产入口仍是 `newMergeTempIndexExecutor` / `RunSubtask`（[backfilling_merge_temp.go](backfilling_merge_temp.go)）。

## 核心职责

- `PhysicalTableCatalog` 与 `TemporaryIndexInfo` 提供一个轻量目录，用索引 ID、父表物理 ID、分区 ID 和全局索引标志代替 Go 的完整 `table.PhysicalTable` / `model.IndexInfo` 对象。
- `MergeTemporaryIndexExecutor::initialize_by_meta` 从子任务起始 key 解码真实物理表 ID 和临时索引 ID，选择父表或局部分区；它刻意不以 `element_ids` 或 `physical_table_id` 作为索引定位依据。
- `MergeTemporaryIndexExecutor::run_subtask` 在 `[start_key, end_key)` 内循环调用 `MergeTemporaryIndexWorker::handle_one_range`，收集每批结果并累计行数与合并指标。
- `decode_temporary_index_key` 解析 tablecodec 风格的 `t{tableID}_i{indexID}` 头，并去掉临时索引 ID 的高位前缀。
- 生命周期钩子 `init`、`cleanup`、`task_meta_modified`、`resource_modified` 当前均为空；这里只提供接口形状，不执行环境准备、资源调整或清理。

## 主要符号

- `PhysicalTableCatalog { parent_physical_id, partition_ids, indexes }`：一次执行可见的表/分区/索引目录。空 `partition_ids` 表示按非分区表处理。
- `TemporaryIndexInfo { info, global }`：`info` 复用 `backfilling_import_cloud::IndexInfo` 的 ID、名称、唯一性字段；`global` 决定数据应落在父表 key 空间还是分区 key 空间。
- `MergeTemporaryIndexError`：区分索引不存在、分区不存在、元数据/推进范围非法和下游 `OperatorError`。
- `MergeTemporaryIndexExecutor<S: TemporaryIndexStore>`：保存任务/作业 ID、批大小、解析出的表和索引、统计量以及实际 worker。`task_id`、`job_id` 和执行器自身的 `batch_count` 当前只保存输入值，运行逻辑实际读取 `worker.batch_count`。
- `new(task_id, job_id, batch_count, parent_table, store)`：构造 worker，并把最大重试次数固定为 16。传入的 `batch_count` 原样保存；真正取批时 worker 使用 `max(1)`，并非在构造时改写为至少 1。
- `initialize_by_meta(&BackfillSubTaskMeta)`：验证 key 范围，解码起始 key，查找索引并确定物理表。
- `run_subtask(subtask_id, meta, records)`：同步处理调用方提供的临时索引记录切片，返回每批 `TemporaryIndexResult`。
- `reset_summary()`：仅清零 `SubtaskSummary`；不会清零 `total_rows`、两个指标 map、worker 扫描计数或已解析的表/索引。
- `decode_temporary_index_key(key)`：私有 table/index key 头解析器；至少需要 `t`、8 字节表 ID、`_i` 和 8 字节索引 ID。

## 执行流程

1. 调用方用表目录和一个 `TemporaryIndexStore` 构造执行器；worker 获得相同批大小和 16 次最大尝试次数。
2. `run_subtask` 首先调用 `initialize_by_meta`。后者要求起始 key 非空且字节序严格小于结束 key，再从起始 key 解码表 ID 与索引 ID。
3. 执行器按解码后的索引 ID 查询 `parent_table.indexes`。局部分区索引要求解码表 ID 出现在 `partition_ids`；全局索引或无分区目录的表统一选择 `parent_physical_id`。
4. `subtask_id` 尝试转成 `usize`；转换失败（例如负数）时退回 0。首个 `TemporaryIndexScanTask` 使用元数据的半开区间 `[start, end)`。
5. 每轮把当前 `next` 作为新起点调用 `worker.handle_one_range`。worker 从 `records` 中筛选区间内记录、最多取一批，把非 `skip` 记录转换成正式索引 Set/Delete 加临时 key Delete，并通过 `TemporaryIndexStore::apply_batch` 原子提交。
6. worker 遇到可重试存储错误会把批大小减半（最小 1），最多尝试 16 次；成功或失败退出前恢复原批大小。结果携带扫描数、实际合并数、下一 key 和完成标志。
7. 执行器要求 `result.next_key` 严格大于本轮起点，以阻止不推进造成的死循环；随后以 `(meta.physical_table_id, index_id)` 累加 `merge_metrics`，并用 `add_count` 同步增加 `summary.row_count` 与 `total_rows`。
8. `done` 为真或 `next >= end` 时结束，返回已按轮次收集的结果。执行器不自行持久化检查点，调用方只能从返回结果中的 `next_key` 获取批次推进信息。

## 数据与状态

子任务的权威范围是 `meta.legacy_sorted_kv_meta.start_key/end_key`。索引与实际物理表由起始 key 解码，而 `meta.physical_table_id` 只用于指标标签；这与 Go 的 `DecodeTableID` / `findIndexInfoByDecodingKey` 选择逻辑及指标标签语义一致。临时索引 ID 解码后与 `0x0000_ffff_ffff_ffff` 做掩码，以移除临时索引高位标志。

执行器保存跨调用的可变状态：`physical_table_id`、`index_info` 会被最近一次初始化覆盖；`summary.row_count`、`total_rows`、`merge_metrics` 以及 worker 的 `total_scan_count` 会累积。`conflict_metrics` 虽然公开存在，但本文件没有写入点。`reset_summary` 只重置摘要，因此复用执行器时若需要“每子任务”统计，调用方必须理解其他累计字段不会一起归零。

输入 `records` 由调用方持有，本文件不从真实 KV 存储扫描。worker 按切片现有顺序筛选并以最后选中记录计算 `next_key`；安全扩展时应维持临时 key 的有序输入约束，否则后续范围推进可能漏掉排在切片后方但 key 更小的记录。该约束由实现方式推导，当前类型签名并未强制。

## 依赖与调用关系

上游阶段关系是 `BackfillTaskMeta.merge_temporary_index` → `LitBackfillScheduler` → `BackfillStep::MergeTemporaryIndex`；`BackfillDistExecutor::get_step_executor` 也接受该步骤。但仓库搜索未发现调度器到本文件 `MergeTemporaryIndexExecutor` 的生产构造边，因此当前直接调用面是独立测试，而不是已闭合的运行时主链。

直接下游依赖均为同 crate 模块：

- [backfilling_dist_executor.rs](backfilling_dist_executor.rs)：`BackfillSubTaskMeta` 与旧版单组范围元数据。
- [backfilling_import_cloud.rs](backfilling_import_cloud.rs)：轻量 `IndexInfo`。
- [backfilling_operators.rs](backfilling_operators.rs)：扫描任务、记录/结果、存储 trait、worker 和 `OperatorError`；真正的批处理、原子写入与重试在这里。
- [backfilling_read_index.rs](backfilling_read_index.rs)：可重置的 `SubtaskSummary`。

`Cargo.toml` 将 crate 命名为 `astersql-ddl`，库入口是 `lib.rs`；本文件自身只使用标准库 `BTreeMap` 和 crate 内类型，没有新增外部 crate 依赖或 feature 条件。Go 侧更深的正式索引写入逻辑还会进入 `backfilling_operators.go` 与 `index_merge_tmp.go`，Rust 本文件则通过 `TemporaryIndexStore` 隔离存储实现。

## 错误处理与边界

- 空起点、`start >= end`、无法识别 `t..._i...` 头，以及 worker 返回不推进的 `next_key` 都映射为 `InvalidMetaRange`。
- key 解码只检查固定头和所需字节，不验证尾部内容，也不验证结束 key 属于同一表/索引；范围一致性依赖上游规划。
- 解码索引不在目录时返回 `IndexNotFound(id)`；局部分区索引的表 ID 不在分区目录时返回 `PartitionNotFound(id)`。全局索引忽略解码表 ID 并使用父表。
- worker 的致命写错误、重试耗尽等以 `MergeTemporaryIndexError::Operator` 原样分类。已完成批次的统计已写入执行器状态；后续批次失败时函数返回错误而不返回先前的 `results`，本层没有回滚此前已经提交的批次。
- `index_info.as_ref().expect("initialized index")` 依赖 `initialize_by_meta` 成功这一内部不变量；当前顺序成立，但重构时不应绕过初始化。
- `subtask_id` 无法转为 `usize` 时静默变为 0，可能造成诊断 ID 丢失；若将该 ID 用于去重或持久化，应改为显式校验并补测试。
- 零批大小不会在构造时改写；worker 每次选择记录时按 1 处理。源码构造函数注释中“钳制为至少 1”与实际字段值不一致，测试明确要求字段保留 0，扩展时应以实现与测试为准。

## 并发与资源生命周期

Rust 执行器没有线程、异步任务、channel、锁或资源配额对象；`run_subtask` 与 worker 都在调用线程同步执行。存储批次的原子性由 `TemporaryIndexStore::apply_batch` 契约提供，本文件不打开或提交真实事务。多批之间不构成单一原子事务，失败恢复依赖下游写入幂等性及上游重试/检查点策略。

worker 的可重试错误处理在单次调用内完成：暂时缩小批大小，退出前恢复原值。`init`、`cleanup` 不分配或释放资源，`task_meta_modified`、`resource_modified` 也不调整并发；因此动态 CPU/批大小调整尚未接线。执行器包含可变 store 和累计字段，只通过 `&mut self` 使用，本类型没有声明跨线程共享保证。

Go 实现会创建 worker pool、source/merge/sink 异步 pipeline，并从步骤资源读取 CPU 容量；这些并发与关闭语义尚未出现在本 Rust 文件中。

## 与 Go 版本的对应关系

主要一一对应关系是：`mergeTempIndexExecutor` ↔ `MergeTemporaryIndexExecutor`，`initializeByMeta` ↔ `initialize_by_meta`，`RunSubtask` ↔ `run_subtask`，`mergeTempIndexCollector` 的计数 ↔ Rust 摘要与累计字段，空生命周期方法也保留了相同接口意图。

已对齐的关键语义包括：从 `StartKey` 解码表/索引而不是依赖 `EleIDs`；局部分区索引选择解码分区、全局索引选择父表；合并指标仍用 `meta.PhysicalTableID` 标签；collector 的 `Processed(_, rows)` 令 `addCount` 与 `scanCount` 同增，所以 Rust 的 `summary.row_count` 和 `total_rows` 都使用 `add_count`。独立 Rust 测试覆盖了解码及临时索引前缀掩码、全局索引父表选择、指标标签、skip 记录计数、零批大小字段保留和非法分区。

尚未等价或尚未接线的部分包括：Go 从 JSON 子任务元数据开始并自行从 KV 扫描，Rust 接收已经解析的 meta 和内存记录切片；Go 使用并发 pipeline、真实事务、日志、Prometheus counter、资源 CPU 容量和完整表对象，Rust 使用同步 worker 与抽象 store；Go 初始化 `mergeCounter` / `conflictCounter`，Rust 的 `conflict_metrics` 没有更新；Go 的修改通知标注为未来支持，Rust 同样为空；Rust worker 固定 16 次最大尝试，而 Go 片段按可重试事务错误持续退避，没有相同的本地次数上限。

## 扩展指南

- 接入生产主链时，最可能修改 `BackfillDistExecutor::get_step_executor` 周边的执行器工厂/分派逻辑，并构造 `PhysicalTableCatalog`、真实 `TemporaryIndexStore` 和有序扫描输入；必须新增独立测试证明 `MergeTemporaryIndex` 阶段确实调用本执行器，而非只返回枚举值。
- 改变表或索引解析时优先修改 `initialize_by_meta` / `decode_temporary_index_key`，同步覆盖错误 key、结束 key 跨索引、全局索引和分区缺失场景。必须保留“起始 key 是索引定位权威”的 Go 兼容语义，除非同时更新任务元数据协议。
- 改变批处理、重试或事务语义应落在 [backfilling_operators.rs](backfilling_operators.rs) 的 `MergeTemporaryIndexWorker` 与独立 operator 测试；不要把测试嵌入生产 `.rs` 文件。重点风险是批间部分成功、输入顺序、重试幂等和批大小恢复。
- 增加冲突统计时，应明确由 store/worker 如何报告冲突，再更新 `conflict_metrics`；不能仅增加计数而没有与 Go 唯一键冲突语义对应的事件来源。
- 实现 `resource_modified` 或并发执行前，需要定义 store 是否可跨 worker、安全的汇总合并方式、取消/关闭顺序与检查点持久化；同步参考 Go 的 source/merge/sink pipeline，但不要假定当前同步接口已提供这些保证。
- 修改摘要重置规则时同时检查 `reset_summary`、`total_rows`、metrics maps 和 worker 扫描计数，明确哪些是子任务级、任务级或进程级累计值，并在 [backfilling_merge_temp_test.rs](backfilling_merge_temp_test.rs) 添加回归覆盖。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ddl/backfilling_merge_temp.rs` 确认目标文件有 23 个符号；`node --file ... --offset 1 --limit 500` 读取了全部 284 行，并报告关联文件为 `pkg/ddl/backfilling_merge_temp_test.rs` 与 `pkg/ddl/index.rs`。组合 `callers/callees` 查询超时且未返回边，因此调用关系又用精确仓库搜索核验。
- 已读源码/配置：目标 [backfilling_merge_temp.rs](backfilling_merge_temp.rs)、[Cargo.toml](Cargo.toml)、`lib.rs`、`backfilling_dist_executor.rs`、`backfilling_dist_scheduler.rs`、`backfilling_operators.rs`、`backfilling_read_index.rs`、`backfilling_import_cloud.rs` 与 `index.rs`。
- Go 对照：[backfilling_merge_temp.go](backfilling_merge_temp.go)、`backfilling_operators.go`、`index.go`；DDL 上下文还核对了 `docs/agents/ddl/README.md` 与 `docs/agents/ddl/06-add-index.md`，但行为结论以上述代码和测试为准。
- 测试证据：[backfilling_merge_temp_test.rs](backfilling_merge_temp_test.rs) 的三个测试覆盖 key 解码、全局/分区选择、指标/统计、skip 记录和零批大小；`backfilling_dist_scheduler_test.rs` 覆盖阶段流转及合并计划不设置 `element_ids`。本任务按计划为纯文档分析，未运行 Cargo 或测试二进制。
- 人工仓库搜索未发现生产 Rust 代码构造 `MergeTemporaryIndexExecutor` 或调用其 `run_subtask`；该限制已在“文件定位”和“依赖与调用关系”中明确记录。
