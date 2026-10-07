# `pkg/ddl/backfilling_read_index.rs`

## 文件定位

该文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod backfilling_read_index` 导出。它描述分布式 ADD INDEX 回填中 read-index 阶段的 Rust 数据模型、生命周期状态和统计聚合规则。

需要区分“模型”与“完整执行链”：当前 Rust 生产模块主要复用这里的 `SortedKvMeta` 和 `SubtaskSummary`，例如 `backfilling_dist_executor.rs`、`backfilling_dist_scheduler.rs`、`backfilling_merge_sort.rs`、`backfilling_import_cloud.rs` 和 `backfilling_clean_s3.rs`。代码搜索没有发现生产 Rust 模块构造或调用 `ReadIndexStepExecutor`；它目前由独立测试 `backfilling_read_index_test.rs` 直接覆盖。Go 版本的完整 read-index 执行器则由 `backfilling_dist_executor.go::newBackfillStepExecutor` 在 `proto.BackfillStepReadIndex` 分支创建。

从 DDL 生命周期看，这一阶段属于 job 驱动的 ADD INDEX reorg/backfill 工作负载，而不是 metadata-only 快路径：它扫描某个物理表或分区的行键范围并生成索引 KV。该 Rust 文件本身不推进 schema state、不更新 schema version，也不持久化 DDL job/checkpoint；这些职责不应从文件名推断为已经在此实现。

## 核心职责

1. `SortedKvMeta` 表示一批有序索引 KV 文件的键边界、文件数和字节数，并提供可结合的 `merge`。
2. `SubtaskSummary` 统一保存读取字节、处理字节、处理行数以及对象存储 GET/PUT 次数，并提供清零和合并操作。
3. `ReadIndexStepExecutor` 用显式字段模拟初始化、后端打开、流水线启动/关闭、运行时调参、子任务产物回写和清理的生命周期。
4. `table_start_end_key` 在已有分区范围与整表范围之间选择扫描边界。
5. `DistTaskRowCountCollector` 同步累计子任务摘要、指标行数和集群读取字节，保留 Go 的整数转换/溢出语义。

该文件不读取表数据、不编码索引键、不创建对象存储客户端，也不执行本地 ingest。`run_subtask` 的 `generated_meta` 与 `collected_summary` 都由调用方传入，因此它负责状态编排和汇总，而不是生产这些结果。

## 主要符号

- `SortedKvMeta { start_key, end_key, file_count, total_kv_size }`：跨 read-index、merge-sort、调度规划和云导入共享的产物元信息。`merge(&mut self, other)` 取非空最小起点、最大终点，并以 `wrapping_add` 累加数量和大小。
- `SubtaskSummary`：五项子任务计数器；`reset` 恢复默认值，`merge` 对每项使用环绕加法。
- `BackfillSubtaskMeta` 与 `Subtask`：本文件内的轻量子任务表示。前者保存物理表 ID、扫描范围、按索引排列的 `meta_groups` 和 `element_ids`；后者增加 task/subtask ID。它们不同于 `backfilling_dist_executor.rs::BackfillSubTaskMeta`，后者才含外部路径、文件列表、时间戳和兼容字段。
- `StepResource { cpu }` 与 `PipelineState`：分别描述本步骤可用 CPU 和计算出的 reader/writer 数、运行/关闭标志；不是实际 worker pool 或异步 pipeline。
- `ReadIndexSummary { index_id, meta }`：一个索引的累计产物；由 `summary_map: BTreeMap<i64, ReadIndexSummary>` 按 index ID 保存，因而具备稳定的键顺序。
- `ReadIndexError`：区分未初始化、流水线不可调、非法范围、后端关闭及本地排序磁盘准入失败。
- `ReadIndexStepExecutor`：核心状态容器。公开配置包含 job/index/table/partition ID、行宽估计、云 URI、批大小、限速、执行节点和 slots；私有 `initialized`、`backend_open` 维护前置条件。
- `table_start_end_key(...)`：存在 `physical_table_id` 映射时克隆分区范围，否则返回调用方提供的整表范围。
- `DistTaskRowCountCollector::{accepted, processed}`：分别累计读取量，或累计处理量与行数。

文件没有 trait、模块常量、泛型执行抽象或条件编译项；所有错误均为本地枚举，不实现 `std::error::Error`。

## 执行流程

典型的 Rust 状态流程如下：

1. `ReadIndexStepExecutor::new(job_id, index_ids, physical_table_id)` 建立默认状态：batch size 为 256，未初始化、后端关闭、无 pipeline。
2. 可先调用 `set_runtime_context(exec_id, runtime_slots)` 注入 DXF 节点和 slots。`init(uri)` 以 URI 是否为空决定 global/cloud sort；本地模式且 `exec_id` 非空时，通过 `astersql_ddl_ingest::env::ingest_temp_data_dir` 取得临时目录并调用 `check_local_sort_disk_space_at_path` 做一次磁盘准入检查。成功后才同时设置 `backend_open` 和 `initialized`。
3. `run_subtask` 依次拒绝未初始化、后端关闭和 `row_start > row_end`；随后 `reset_subtask` 清掉上一个子任务的摘要、索引汇总与 pipeline。
4. 它调用 `backfilling_txn_executor.rs::expected_ingest_worker_count`。CPU 至少按 1 计算；全局排序让读写 worker 都等于并发度，本地模式还根据 `average_row_size` 调整读写比例。
5. 新建一个 `running = true, closed = false` 的 `PipelineState`。这一步只记录预期 worker 数，不创建真实线程、通道或 worker pool。
6. 云存储模式把 `generated_meta` 克隆进 `subtask.meta.meta_groups`，把全部 `index_ids` 写入 `element_ids`，再用 `zip(index_ids, generated_meta)` 按位置合并进 `summary_map`。本地模式仅清空 `meta_groups`。
7. 合并 `collected_summary`，随后 `on_finished` 把 pipeline 标记为停止且关闭。成功返回后 pipeline 仍保留为 `Some(closed)`，所以不能再通过 `resource_modified` 调整。
8. `cleanup` 再次关闭 pipeline、关闭逻辑后端、撤销初始化并清空索引汇总；它不清空 `summary`、配置字段或子任务元数据。

运行期间，`task_meta_modified` 始终把 batch size 夹到至少 1，但仅在本地模式更新 `max_write_speed`；`resource_modified` 只允许对存在且未关闭的 pipeline 重算 worker 数。

## 数据与状态

- 生命周期不变量：成功 `run_subtask` 必须经过成功 `init`，且 `backend_open` 为真；`cleanup` 后再次运行会返回 `NotInitialized`。
- 范围不变量：仅 `row_start > row_end` 非法，空范围或 `row_start == row_end` 在本模型中允许。`table_start_end_key` 本身不验证边界。
- 元数据位置不变量：云模式的 `meta_groups` 与 `element_ids` 预期按索引位置对应；代码使用 `zip`，但没有校验两者长度相等。多出的 ID 或元数据会被静默忽略于 `summary_map`，而原始 `generated_meta` 仍完整写进子任务。
- 合并规则：空 `start_key` 被当作“尚无起点”；非空输入才能替换已有起点。`end_key` 单纯取字典序较大值。数量、字节和所有统计计数均显式采用环绕算术，release/debug 构建语义一致。
- 重置范围：`reset_subtask` 清 `summary`、`summary_map`、`pipeline`；`SubtaskSummary::reset` 清全部五项计数；`cleanup` 不清 `summary`。
- `partition_ids` 当前只被存储，没有被本文件的方法读取；实际查范围由独立函数的 `partition_ranges` 参数决定。
- `job_id`、`physical_table_id`、`batch_size`、`max_write_speed` 在本模型中主要是配置/可观察状态，并未驱动真实扫描或写入。

## 依赖与调用关系

crate 内直接依赖只有：

- `crate::backfilling::Key`：键类型；从比较、克隆和 `is_empty` 的用法看，它承担字节序键边界。
- `crate::backfilling_txn_executor::expected_ingest_worker_count`：根据 CPU、平均行宽和 global-sort 标志计算 reader/writer 数。
- `astersql_ddl_ingest::{env, disk_root}`：仅在本地排序初始化且设置了 `exec_id` 时取得 ingest 临时目录并检查磁盘余量。`pkg/ddl/Cargo.toml` 以路径依赖 `astersql-ddl-ingest = { path = "ingest" }` 声明它。
- 标准库 `BTreeMap`：用于分区范围查找和按索引 ID 汇总。

RustCodeGraph 的文件节点把该文件标记为被 `backfilling_dist_executor.rs`、`backfilling_dist_scheduler.rs`、`backfilling_merge_sort.rs` 及两个相应测试文件使用；全文搜索还显示 `backfilling_import_cloud.rs`、`backfilling_clean_s3.rs`、`backfilling_merge_temp.rs` 复用共享元数据/摘要类型。主要下游关系是：

- `BackfillSubTaskMeta` 的序列化和兼容修正包含 `Vec<SortedKvMeta>`；
- `backfilling_dist_scheduler.rs::merge_meta_groups` 按组调用 `SortedKvMeta::merge`，全局排序写入计划以这些边界和大小为输入；
- `backfilling_merge_sort.rs` 合并多个输出 summary 后把单组 `SortedKvMeta` 持久化为外部子任务元数据；
- 云导入和清理步骤消费 `SortedKvMeta`/`SubtaskSummary` 计算导入范围和计量数据。

当前没有查到 Rust 生产调用者驱动 `ReadIndexStepExecutor::{new, init, run_subtask, cleanup}`，因此不能把它描述为已接入 Rust DXF 的真实步骤执行器。

## 错误处理与边界

- `init` 的唯一可失败分支是本地排序磁盘检查：未初始化 ingest 环境或磁盘准入失败会成为 `ReadIndexError::LocalSortDisk(String)`。字段只在检查成功后切换为已初始化/后端打开，因此失败不会留下“半初始化成功”状态；但 URI 和 `use_cloud_storage` 已经更新。
- `run_subtask` 的前置错误不修改摘要和 pipeline，因为验证发生在 `reset_subtask` 之前。通过验证后，现有实现没有其他 `Result` 失败点。
- `resource_modified` 将“没有 pipeline”和“pipeline 已关闭”统一报告为 `PipelineNotRunning`。
- `BackendClosed` 在公开 API 的正常序列中通常被 `NotInitialized` 遮蔽：`cleanup` 同时清两个标志，而字段私有；该变体仍表达两个概念上的检查层次。
- 代码不检查 `generated_meta.len() == index_ids.len()`，也不检查键范围是否为空或属于 `physical_table_id`；扩展真实执行链时必须决定这些是否应成为持久化前的硬错误。
- 本地模式不消费 `generated_meta`，并明确清空子任务的 `meta_groups`；这是模式分支，不是数据丢失错误。

## 并发与资源生命周期

本文件所有可变操作都要求 `&mut self`，没有 `Arc`、锁、原子、异步任务、线程或通道；`PipelineState` 只是同步状态快照。因此单个 executor 不能由该 API 并发修改，文件也不实现 Go 版本的并发 pipeline。

资源生命周期是逻辑化的：`init` 只将 `backend_open` 置真（并可能做一次磁盘检查），`run_subtask` 创建/关闭状态对象，`cleanup` 置假并清汇总。没有 RAII guard 或 `Drop` 实现；调用方若忘记 `cleanup`，此文件不会自动释放真实资源，不过当前模型也没有持有真实后端句柄。

Go 对照使用 `atomic.Pointer[operator.AsyncPipeline]` 暴露当前真实 pipeline，`ResourceModified` 动态调整 reader/writer pool；global-sort writer callback 以 mutex 保护每个索引的 `SortedKVMeta`；本地 ingest backend context 通过 `defer Close` 和成功/失败分支注销引擎；对象存储也通过 defer 关闭并合并访问计数。这些并发和资源保证尚未由本 Rust 文件复现。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/backfilling_read_index.go`：

- Rust `ReadIndexStepExecutor` 对应 Go `readIndexStepExecutor` 的部分配置、摘要、pipeline 状态与生命周期方法；Rust `new/init/run_subtask/task_meta_modified/resource_modified/on_finished/cleanup` 的命名和阶段与 Go 基本对应。
- Rust 以“非空字符串”判定云模式，包含仅空格的 URI；独立测试 `nonempty_cloud_uri_uses_global_sort_like_go` 固化了 Go `len(uri) > 0` 的语义。
- Rust `init` 已对齐 Go 本地排序的初始化时磁盘准入意图，并接收 exec ID/runtime slots；但 Go 还设置 `UseCloudStorage`、注册指标、按存储类型创建真实 local backend。
- Go `RunSubtask` 解码持久化元数据、创建可取消 worker context、打开带访问记录的对象存储，并分别运行 `runGlobalPipeline` 或 `runLocalPipeline`。Rust `run_subtask` 接收已经生成的 meta/summary，不执行这些 I/O。
- Go 本地 pipeline 注册 ingest engines、处理分布式锁/检查点、在失败时清数据、成功时检查重复键；Rust 无对应句柄或错误分支。
- Go global pipeline 在 writer 并发关闭时按 group offset 聚合元数据，随后写外部元数据并 marshal 回 `subtask.Meta`；Rust 只更新内存结构。
- Rust `table_start_end_key` 是纯映射回退；Go `getTableStartEndKey` 还解析 `PartitionedTable`，并在升级兼容场景（分区且 `RowStart` 为空）读取有效版本、重新计算表范围，错误会记录日志并传播。
- Rust `DistTaskRowCountCollector` 对应 Go `distTaskRowCntCollector`。`accepted(-1)` 转 `u64` 以及有符号计数溢出由 Rust 独立测试固定；Go 还直接更新 Prometheus counter 和 metering recorder。

因此当前差异不是可忽略的实现细节：Rust 文件保留关键数据语义与可测试状态转换，但尚不能替代 Go 的真实分布式 read-index 执行路径。

## 扩展指南

- 若接入真实 Rust DXF 执行链，应从步骤工厂/调度分支构造 `ReadIndexStepExecutor`，并把解码后的 `backfilling_dist_executor.rs::BackfillSubTaskMeta` 与本文件的轻量 `BackfillSubtaskMeta` 统一或建立明确转换，避免两套同名元数据漂移。
- 若实现真实扫描/写入，最可能改动 `init`、`run_subtask`、`on_finished` 和 `cleanup`：需要补对象存储、backend、pipeline、取消、引擎清理、重复键检查与元数据持久化，并保持 Go 成功/失败清理顺序。
- 调整并发计算时，应同步 `backfilling_txn_executor.rs::expected_ingest_worker_count` 及其独立测试，而不是在本文件复制算法；`resource_modified` 必须保持“无运行子任务则可重试失败”的契约。
- 修改 `SortedKvMeta::merge` 会影响 scheduler、merge-sort 和 cloud import，不只是 read-index；应同步检查 `backfilling_dist_scheduler_test.rs`、`backfilling_merge_sort_test.rs` 和 `backfilling_dist_executor_test.rs`。
- 修改本文件行为应扩展独立测试 `pkg/ddl/backfilling_read_index_test.rs`，不要把测试内嵌进生产源文件。至少覆盖初始化失败的状态、非法/空范围、meta 与 index 数量不匹配、cleanup 后重入、运行中资源调整及本地/云模式差异。
- 兼容性风险集中在持久化元数据字段顺序/分组位置、Go 环绕计数语义、非空 URI 判定和分区升级回退；性能风险集中在 worker 比例、重复克隆 `generated_meta`、汇总结构的锁粒度以及对象存储请求量。
- 该阶段只负责 reorg/backfill 数据面；schema state、job persistence、取消/回滚和 schema version 同步应继续由 DDL job 框架承担，不能在此文件私自建立第二套状态机。

## 验证依据

- 目标源码：`pkg/ddl/backfilling_read_index.rs`，已核对全部 428 行及其中 34 个 RustCodeGraph 符号。
- crate/模块边界：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`；确认 `astersql-ddl-ingest` 路径依赖和模块导出。
- 下游 Rust 证据：`pkg/ddl/backfilling_dist_executor.rs`、`backfilling_dist_scheduler.rs`、`backfilling_merge_sort.rs`、`backfilling_import_cloud.rs`、`backfilling_clean_s3.rs`、`backfilling_merge_temp.rs`，以及 `backfilling_txn_executor.rs::expected_ingest_worker_count`。
- Go 对照：`pkg/ddl/backfilling_read_index.go` 全文件及 `pkg/ddl/backfilling_dist_executor.go::newBackfillStepExecutor` 的 read-index 分支。
- 独立 Rust 测试：`pkg/ddl/backfilling_read_index_test.rs`，覆盖非空 URI、云模式调参、element IDs 回写和 Go 风格转换/溢出；相邻共享类型测试见 `backfilling_dist_executor_test.rs`、`backfilling_dist_scheduler_test.rs`、`backfilling_merge_sort_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点、1,848,419 边；`files --filter` 命中目标；文件 `node` 给出源码及 5 个直接使用文件。精确 `callers ReadIndexStepExecutor` 在 30 秒内没有返回结果，因此生产接线结论又用仓库全文符号搜索复核；没有把超时当作“无调用”的唯一证据。
- 按任务约束未运行 Cargo。文档结构通过任务规定的 11 标题检查后方可交付。
