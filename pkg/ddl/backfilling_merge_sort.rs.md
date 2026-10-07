# `pkg/ddl/backfilling_merge_sort.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod backfilling_merge_sort` 暴露。它描述分布式 DDL 全局排序回填中的归并阶段：读取 `BackfillSubTaskMeta.data_files` 所列的有序但范围可能重叠的文件，通过后端归并后，把汇总的 `SortedKvMeta` 写回子任务元数据，供后续写入与摄取阶段使用。

阶段位置可由 `pkg/ddl/backfilling_dist_executor.rs::BackfillStep` 核对：`ReadIndex` 之后是 `MergeSort`，再进入 `WriteAndIngest`。不过当前 Rust 生产接线尚不完整：`BackfillDistExecutor::get_step_executor` 对 `MergeSort` 只校验并返回阶段枚举，并未构造或调用 `MergeSortExecutor`；RustCodeGraph 也只找到 `pkg/ddl/backfilling_merge_sort_test.rs` 对本文件的直接引用。因此，本文件目前是可测试的阶段实现模型，不应表述为已经接入 Rust 生产调度链。

## 核心职责

- `MergeSortBackend` 隔离实际外部排序引擎，提供文件归并、线程池调节和线程池大小查询三个能力。
- `MergeSortExecutor::run_subtask` 管理一次子任务的状态转换，生成按 `task_id/subtask_id` 隔离的输出前缀，调用后端，并将多个结果摘要折叠为一个 `SortedKvMeta`。
- 后端重复键错误被翻译为带可选索引名的 `MergeSortError::DuplicateKey`；普通后端错误和外部元数据写入错误保持分类。
- `resource_modified` 在子任务运行时同步调整后端工作池；`cleanup` 和 `reset_summary` 分别清理运行态缓存与统计。

本文件不负责读取或反序列化外部子任务元数据，也不直接实现对象存储、排序算法或后续摄取；这些边界分别由调用方、`ExternalMetaStorage`/`write_external_backfill_subtask_meta`、`MergeSortBackend` 和云端导入阶段承担。

## 主要符号

- `MergeSortError`：执行器级错误。`NotInitialized` 和 `NoSubtaskRunning` 是生命周期错误；`Merge(String)` 保存普通后端错误文本；`DuplicateKey { index_name }` 表示唯一键冲突；`ExternalMeta(MetaError)` 表示持久化元数据失败。
- `MergeBackendError`：后端错误的最小分类，仅区分 `DuplicateKey` 与 `Other(String)`。
- `MergeSortBackend`：同步 trait。`merge_overlapping_files(files, memory_per_core, output_prefix, concurrency)` 返回若干输出批次摘要；`tune_worker_pool_size(concurrency, wait)` 和 `worker_pool_size()` 支持运行期资源调整。
- `MergeSortExecutor<B>`：泛型执行器。`task_id` 用于路径，`job_id`、`indexes`、`cloud_storage_uri` 保存任务上下文，`backend` 持有实际实现，`summary` 保存统计，`subtask_sorted_kv_meta` 是完成过程中的短期缓存，`running` 与私有 `initialized` 表示生命周期状态。
- `new`：建立未初始化、未运行、空摘要和空缓存的执行器。
- `init`：只把 `initialized` 置为 `true`。
- `run_subtask`：本文件的主要行为入口。
- `cleanup`：将 `running` 置为 `false` 并丢弃缓存，不改变 `initialized`、后端或摘要。
- `reset_summary`：调用 `SubtaskSummary::reset` 清零统计。
- `resource_modified`：仅在 `running == true` 时比较并调整并发度；调整时固定传入 `wait = true`。
- `From<ImportError> for MergeSortError`：把导入侧错误的调试表示包装成 `Merge`；当前文件内没有调用该转换。

## 执行流程

1. 调用方用 `new` 构造执行器并必须先调用 `init`；否则 `run_subtask` 立即返回 `NotInitialized`，且不触碰后端和元数据。
2. `run_subtask` 以 `format!("{}/{}", task_id, subtask_id)` 生成输出前缀，把 `running` 置为 `true`，然后把 `meta.data_files`、每核内存、前缀和并发度原样交给 `MergeSortBackend::merge_overlapping_files`。
3. 后端正常返回或返回 `Result::Err` 后，`running` 都会被置回 `false`。普通错误转成 `MergeSortError::Merge`；重复键错误会用 `get_index_info_and_id(meta.element_ids, indexes)` 尽力定位索引，再返回 `DuplicateKey`。
4. 成功时从 `SortedKvMeta::default()` 开始，逐项调用 `SortedKvMeta::merge`：起始键取非空最小值，结束键取最大值，文件数和 KV 字节数使用 wrapping 加法累计。
5. 合并结果先同时放进 `subtask_sorted_kv_meta` 和 `meta.meta_groups`；随后在持久化前清空短期缓存。这一顺序刻意保持 Go 行为，即使后续外部写入失败，已完成子任务缓存也仍为空。
6. 最后调用 `write_external_backfill_subtask_meta`，路径为 `task_id/subtask_id/meta`。提供存储时，该函数设置 `meta.external_path` 并写出外部字段；未提供存储时直接成功。写入错误映射为 `ExternalMeta`。

## 数据与状态

输入主体是 `BackfillSubTaskMeta`：本文件读取 `data_files` 和 `element_ids`，覆盖 `meta_groups`，并可能通过外部元数据写入函数设置 `external_path`。`meta_groups` 总是被替换为恰好一个合并摘要，而不是追加；空后端结果会产生一份默认的空摘要。

`SortedKvMeta` 的范围与计数合并规则位于 `pkg/ddl/backfilling_read_index.rs::SortedKvMeta::merge`。计数溢出采用 wrapping 语义，文档调用者不能假设会返回溢出错误。

执行器状态顺序为 `new: initialized=false, running=false`，`init: initialized=true`，`run_subtask` 的后端调用区间内 `running=true`，正常 `Result` 返回后恢复为 `false`。`subtask_sorted_kv_meta` 仅在成功归并到开始持久化之间短暂为 `Some`，公开字段主要用于与 Go 生命周期对齐和测试观察。当前 `run_subtask` 不更新 `summary`；只有 `reset_summary` 会改变它。`job_id` 与 `cloud_storage_uri` 也由本结构保存但不在当前方法中消费。

## 依赖与调用关系

上游架构入口是 `pkg/ddl/backfilling_dist_executor.rs::BackfillStep::MergeSort`。Rust 当前仅在 `get_step_executor` 中认可该阶段，没有到 `MergeSortExecutor::new/run_subtask` 的生产调用边；直接调用者证据仅见独立测试 `pkg/ddl/backfilling_merge_sort_test.rs`。

下游依赖均为同 crate 模块：

- `backfilling_dist_executor` 提供 `BackfillSubTaskMeta`、`ExternalMetaStorage`、`MetaError` 与 `write_external_backfill_subtask_meta`。
- `backfilling_import_cloud` 提供 `IndexInfo`、`get_index_info_and_id` 和可转换的 `ImportError`。
- `backfilling_read_index` 提供 `SortedKvMeta` 与 `SubtaskSummary`。
- 真正的排序、线程池与输出文件生成全部委托给泛型 `B: MergeSortBackend`；仓库当前只在测试中可见 `BackendStub` 实现，未找到生产后端实现。

`pkg/ddl/Cargo.toml` 将该文件编入 `astersql-ddl` 库；本文件没有直接引用外部 crate，所需类型通过 crate 内模块获得。

## 错误处理与边界

- 未初始化调用是显式错误；重复调用 `init` 是幂等置位，没有额外资源分配。
- `merge_overlapping_files` 返回错误后不会修改 `meta.meta_groups` 或缓存，且 `running` 已恢复为 `false`。
- 重复键定位是“尽力而为”：一个元素 ID 且匹配索引时返回名称；多元素 ID、找不到索引等情况最终得到 `index_name: None`。需要特别注意，`element_ids` 为空时 `get_index_info_and_id` 会无条件访问 `indexes[0]`；若 `indexes` 也为空会 panic，而不是返回 `MergeSortError`。
- 外部写入失败前，`meta.meta_groups` 已更新、缓存已清空，并且写入辅助函数已设置 `meta.external_path`；调用方看到的是部分已变更的内存元数据，不能把错误理解为事务式回滚。
- `external_storage == None` 被视为合法的无外部存储模式，函数仍返回成功；此时只有调用方持有的 `meta` 得到归并摘要。
- 方法未校验零并发或零内存，而是原样传给后端；独立测试明确固定了这一 Go 对齐语义。
- 若后端 panic，`running = false` 没有 RAII guard 保证执行，因此状态可能停留在 `true`；当前接口只对普通 `Result` 错误提供恢复保证。

## 并发与资源生命周期

`MergeSortBackend` 和执行器方法都是同步接口，`run_subtask`、`resource_modified` 均要求 `&mut self`。因此安全 Rust 调用方不能在同一执行器上让这两个方法真正并发执行，除非在外层引入互斥和分段访问；而持有互斥锁执行整个 `run_subtask` 又会阻止同时取得锁来调整资源。`running` 表达了 Go 版运行期调整的状态模型，但当前 Rust 借用接口尚未提供等价的并发接线路径。

当 `resource_modified` 在可调用状态下执行时，没有运行子任务则返回 `NoSubtaskRunning`，目标并发与当前池大小相同则不操作，不同则调用 `tune_worker_pool_size(target, true)` 并等待调整完成。零并发不会在 Rust 层夹紧。

本执行器不自行打开或关闭对象存储，也没有线程、异步任务、锁或通道字段；这些资源必须由后端或调用方拥有。`cleanup` 只清理两个内存状态字段，不关闭 `backend`。`summary` 的对象存储计数也不会由当前实现自动累加。

## 与 Go 版本的对应关系

Go 对照实现为 `pkg/ddl/backfilling_merge_sort.go::mergeSortExecutor`，生产构造边位于 `pkg/ddl/backfilling_dist_executor.go::newBackfillStepExecutor` 的 `BackfillStepMergeSort` 分支；调度顺序由 `pkg/ddl/backfilling_dist_scheduler.go::GetNextStep` 和 `OnNextSubtasksBatch` 确认。Go 调度测试 `pkg/ddl/backfilling_dist_scheduler_test.go::TestGetNextStep` 及相邻全局排序场景确认归并阶段位于读取和写入摄取之间。

Rust 保留的主要语义包括：任务/子任务路径前缀、每核内存与并发度透传、归并摘要聚合、重复键索引名定位、完成缓存先清理再持久化、空闲资源调整请求报错、并发变化时等待工作池调整，以及零并发不做 Rust 特有夹紧。

尚未等价或有意抽象化的部分包括：Go 版直接创建带请求记录的对象存储、解码子任务元数据、构造 `globalsort.MergeOperator`、通过 writer 回调在互斥锁下汇总、以原子指针支持并发资源调整、注入 failpoint、合并对象存储计量、转换真实 ingest 键冲突错误，并在完成时重新序列化 `subtask.Meta`。Rust 版改为调用方传入已解码的可变元数据、可选存储和泛型后端；`init` 有实际的初始化门禁，而 Go `Init` 仅记录日志；Rust `cleanup` 清状态，而 Go `Cleanup` 是日志型空操作。这些差异说明当前 Rust 文件是局部移植，不是 Go 执行器的完整生产替代。

## 扩展指南

- 接入生产调度时，应在 `BackfillDistExecutor` 的阶段选择处构造具体 `MergeSortBackend` 和 `MergeSortExecutor`，并明确初始化、元数据解码/编码、对象存储所有权、计量与清理责任；不能仅凭 `BackfillStep::MergeSort` 已存在就认为接线完成。
- 若要实现运行中资源调整，需要重新设计共享状态边界，例如把可调后端句柄独立放入线程安全容器；应保持 Go 版“无运行算子则请求框架重试”和减少并发时等待完成的语义。
- 修改错误转换时，应补齐未初始化、普通后端错误、重复键（匹配、缺失、多 ID、空索引）和外部写失败后的部分状态测试；尤其应先决定空 `element_ids`/空 `indexes` 是否继续严格保持 Go 的 panic 前置条件。
- 修改摘要合并或持久化顺序时，应同步检查 `SortedKvMeta::merge`、`write_external_backfill_subtask_meta` 和后续云端导入对 `meta_groups`/`external_path` 的契约，避免破坏旧元数据兼容或重试幂等性。
- 测试必须继续放在独立的 `pkg/ddl/backfilling_merge_sort_test.rs`，不要内嵌到生产源文件。新增生产后端后，还应增加覆盖真实后端边界的独立测试，而不只依赖 `BackendStub`。
- 性能敏感点是归并并发、每核内存、同步等待资源缩容以及输出摘要聚合；接口演进应保留零值透传行为，除非 Go 侧契约也同步改变。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点、1,848,419 条边。
- RustCodeGraph `explore "pkg/ddl/backfilling_merge_sort.rs MergeSortExecutor run_subtask resource_modified"`：确认 Rust 文件源码、Go 对照源码和调用爆炸半径；其中 Rust 文件仅由 `pkg/ddl/backfilling_merge_sort_test.rs` 使用，Go `newMergeSortExecutor` 由 `newBackfillStepExecutor` 调用。
- RustCodeGraph `node --file`：完整核对 `pkg/ddl/backfilling_merge_sort.rs`、`pkg/ddl/backfilling_merge_sort_test.rs`、`pkg/ddl/backfilling_dist_executor.rs`、`pkg/ddl/backfilling_read_index.rs`、`pkg/ddl/backfilling_import_cloud.rs`、Go 执行器/调度器及调度测试的相关源码区间。
- `pkg/ddl/lib.rs`：确认生产模块公开声明与独立测试模块声明。
- `pkg/ddl/Cargo.toml`：确认 crate 名称、库入口和包级 Go 移植元数据。
- `pkg/ddl/backfilling_merge_sort_test.rs::zero_concurrency_is_forwarded_like_go`：确认零并发与零内存原样传递，以及运行态资源调整使用 `wait = true`。
- `pkg/ddl/backfilling_merge_sort_test.rs::failed_external_meta_write_drops_finished_subtask_cache_like_go`：确认外部写失败仍清空完成缓存。
- `pkg/ddl/backfilling_dist_scheduler_test.go::TestGetNextStep` 及其相邻全局排序测试：确认 Go 生产流程的阶段顺序和归并元数据向摄取阶段传递。
- 人工边界复核：文档将 Rust 当前可执行语义、Go 生产链和尚未接线部分分开陈述；未把测试桩或预期设计写成已具备的生产能力。
