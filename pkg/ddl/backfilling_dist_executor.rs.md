# `pkg/ddl/backfilling_dist_executor.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；模块由 [`pkg/ddl/lib.rs`](lib.rs) 以 `pub mod backfilling_dist_executor` 公开。它位于 DDL reorg 的分布式回填边界，负责定义任务/子任务元数据协议、过大子任务元数据的外置读写规则、回填阶段枚举，以及一个轻量的阶段选择门面。它不负责 DDL job 的持久化、schema state 推进、Region 扫描或索引 KV 的实际生成/导入。

从完整链路看，DDL job 仍是上层生命周期和恢复单位；[`LitBackfillScheduler`](backfilling_dist_scheduler.rs) 根据任务元数据规划阶段和 `BackfillSubTaskMeta`，具体工作分别由 `backfilling_read_index.rs`、`backfilling_merge_sort.rs`、`backfilling_import_cloud.rs`、`backfilling_merge_temp.rs` 承担。本文件的 [`BackfillDistExecutor`](backfilling_dist_executor.rs) 当前只返回阶段枚举来表示选择结果，并未像 Go 的 `backfillDistExecutor` 那样实现 DXF `TaskExecutor`、构造真实 `StepExecutor` 或持有 DDL/runtime/session 资源，因此应视为仍在迁移中的协议与门面层。

## 核心职责

1. 用 `BackfillTaskMeta` 表达一个分布式回填任务的稳定输入：DDL job、schema/table、待回填元素、全局排序 URI、行大小估计、临时索引合并、摘要、版本、批大小和限速。
2. 用 `BackfillSubTaskMeta` 表达一个物理表键区间及各阶段产物，包括快照 `ts`、range job/split keys、数据/统计文件、按索引分组的 `SortedKvMeta` 与兼容旧版本的单组元数据。
3. 通过 `marshal`/`unmarshal` 和私有 `put_*`/`take_*` 辅助函数维护 Rust 自定义二进制协议；通过 `decode_backfill_subtask_meta`、`write_external_backfill_subtask_meta` 在内联元数据和 `ExternalMetaStorage` 之间拆分/合并字段。
4. 用 `BackfillStep` 和 `BackfillDistExecutor::get_step_executor` 校验初始化状态、索引存在性以及本地/全局导入模式的阶段合法性。
5. 明确执行器契约：`is_idempotent` 恒为真；`IndexInfoNotFound` 不可重试，其他 `ExecutorError` 当前均可重试；`close` 只维护本地关闭标志。

## 主要符号

- `BACKFILL_TASK_META_VERSION_0` / `BACKFILL_TASK_META_VERSION_1`：任务元数据版本常量。当前文件定义版本值但不在 `BackfillTaskMeta` 上执行升级逻辑；版本 1 还被 [`backfilling_clean_s3.rs`](backfilling_clean_s3.rs) 用于清理任务校验。
- `BackfillTaskSummary { index_kv_size }`：记录全局排序路径生成的索引 KV 总字节数；[`LitBackfillScheduler::plan_global_sort_ingest`](backfilling_dist_scheduler.rs) 在写入计划成功后填充它。
- `BackfillTaskMeta`：任务级配置。`cloud_storage_uri` 是否为空决定 scheduler 的 `global_sort`，`merge_temporary_index` 决定初始阶段是否直接进入临时索引合并；`batch_size`、`max_write_speed` 可被 scheduler 的运行时修改项更新。
- `BackfillSubTaskMeta`：跨阶段传递的子任务载荷。`physical_table_id`、`row_start`、`row_end`、`ts` 是内联字段；文件列表、range keys、`meta_groups`、`element_ids` 和 `legacy_sorted_kv_meta` 可被外置。
- `BackfillSubTaskMeta::marshal` / `marshal_all` / `unmarshal`：前者在存在 `external_path` 时清空外置字段再编码“指针载荷”，后者按完整布局解码；协议头固定为 `ABFM` 加版本字节 `0x01`。
- `BackfillSubTaskMeta::apply_compatibility`：若 `row_start` 为空，以 `legacy_sorted_kv_meta` 的起止键补齐行范围；若 `meta_groups` 为空，将旧单组元数据作为唯一分组。
- `MetaError`：区分截断、非法 UTF-8、协议版本不符和外部存储失败。
- `ExternalMetaStorage`：同步的最小读写 trait；具体对象存储/内存实现由调用方注入。
- `decode_backfill_subtask_meta`：解码内联载荷；当存在路径且提供 storage 时再读外部载荷，只把外置字段合并回来，最后统一应用兼容修正。
- `write_external_backfill_subtask_meta`：设置外部路径，并把清空路径及内联字段后的副本写入外部存储；未提供 storage 时是无操作成功。
- `BackfillStep`：`Init`、`ReadIndex`、`MergeSort`、`WriteAndIngest`、`MergeTemporaryIndex`、`Done` 六个阶段。
- `BackfillDistExecutor`：持有 `task_id`、可选任务元数据、当前节点可用索引 ID 和关闭标志；公开 `new`、`init`、`get_step_executor`、`is_idempotent`、`is_retryable_error`、`close`、`is_closed`。

## 执行流程

任务阶段由 [`LitBackfillScheduler::get_next_step`](backfilling_dist_scheduler.rs) 决定，而不是由本文件自行推进：

1. `Init` 在 `merge_temporary_index=true` 时进入 `MergeTemporaryIndex` 并结束；否则进入 `ReadIndex`。
2. `ReadIndex` 扫描 scheduler 生成的 `[row_start, row_end)` 快照范围。无云存储 URI 时走本地 ingest 并直接结束；全局排序模式进入 `MergeSort`。
3. `MergeSort` 归并重叠文件并更新 `meta_groups`，随后调用本文件的 `write_external_backfill_subtask_meta` 将大字段写到类似 `<task_id>/<subtask_id>/meta` 的路径，再进入 `WriteAndIngest`。
4. `WriteAndIngest` 消费全局排序产物并结束。`BackfillDistExecutor::get_step_executor` 明确拒绝本地模式的这一阶段，防止 scheduler/执行器模式不一致。

子任务元数据外置的往返流程是：调用者先写外部字段并令 `external_path` 生效，再调用 `marshal` 得到仅含内联字段和路径的框架载荷；接收方调用 `decode_backfill_subtask_meta`，先解内联载荷，再按路径读取外部载荷并合并外置字段。外部载荷本身清空 `external_path`，避免递归引用自己。

当前 Rust 生产代码中，`write_external_backfill_subtask_meta` 的直接调用者是 `MergeSortStepExecutor::run_subtask`；仓库搜索未发现 `decode_backfill_subtask_meta` 的生产调用者，它目前由 `backfilling_dist_executor_test.rs` 验证。`BackfillDistExecutor::new/get_step_executor` 同样只在 Rust 测试中直接使用，因此文档不能把这个轻量门面描述为已接入 DXF 主循环。

## 数据与状态

- 编码协议使用小端整数，所有变长字节串、字符串和列表用小端 `u64` 长度前缀。`usize` 字段编码为 `u64`，解码时通过 `usize::try_from` 检查目标平台容量。
- `unmarshal` 只要求按定义顺序读出已知字段，没有检查输入尾部是否仍有额外字节；因此当前协议能容忍尾随数据，但不能自动识别字段插入或重排。
- `external_path` 是内外载荷分界开关。内联载荷保留 `physical_table_id`、行范围和 `ts`；外部载荷保留 range keys、文件列表、分组元数据、元素 ID 和旧版单组元数据。
- `ts` 对应 Go 注释中的幂等快照依据；本文件只携带它，不分配或校验它。时间戳由 [`try_generate_plan_for_physical_table`](backfilling_dist_scheduler.rs) 的 `alloc_ts` 回调为每个子任务生成。
- `meta_groups` 与 `element_ids` 按位置对应的约束来自 Go `BackfillSubTaskMeta`；本文件不自行校验长度。汇总/计划阶段在相邻 scheduler 代码中承担一致性检查。
- `BackfillDistExecutor` 的状态机很小：`new` 后 `raw_meta=None, closed=false`；`init` 替换元数据并重新打开；`close` 仅置 `closed=true`。它不拥有线程、task handle、channel、session pool 或存储连接。

## 依赖与调用关系

crate 边界由 [`pkg/ddl/Cargo.toml`](Cargo.toml) 确认：crate 名为 `astersql-ddl`、入口为 `lib.rs`。本文件直接依赖同 crate 的 `backfilling::Key` 和 `backfilling_read_index::SortedKvMeta`，没有直接使用外部 crate；其上层/下层关系主要通过数据类型连接：

- 上游规划：[`backfilling_dist_scheduler.rs`](backfilling_dist_scheduler.rs) 创建 `BackfillTaskMeta`/`BackfillSubTaskMeta`、决定 `BackfillStep`、聚合 `BackfillTaskSummary`。
- 下游阶段：[`backfilling_merge_sort.rs`](backfilling_merge_sort.rs) 调用 `write_external_backfill_subtask_meta`；`backfilling_merge_temp.rs` 和 `backfilling_import_cloud.rs` 消费 `BackfillSubTaskMeta`。
- 清理路径：[`backfilling_clean_s3.rs`](backfilling_clean_s3.rs) 消费任务元数据和版本常量以清理全局排序产物。
- 模块装配：[`pkg/ddl/lib.rs`](lib.rs) 公开生产模块，并以独立 `#[cfg(test)] mod backfilling_dist_executor_test` 挂载测试，符合测试与源文件分离要求。

RustCodeGraph 对 `BackfillDistExecutor`、`decode_backfill_subtask_meta`、`write_external_backfill_subtask_meta` 建立了符号节点；图查询未返回后两者的完整调用边，因此对具体调用点使用仓库文本搜索补证。不要把同名 Go 符号的调用边自动归给 Rust 实现。

## 错误处理与边界

- 少于所需字节、长度无法转换为 `usize`、列表元素中途耗尽都归为 `MetaError::Truncated`；字符串字节非法时返回 `InvalidUtf8`。
- 协议头不是 `ABFM\x01` 时返回 `InvalidVersion`，错误值只携带输入第 5 字节（缺失时为 0），不会区分魔数错误与版本错误。
- 外部 storage 的字符串错误被包装为 `MetaError::External`；写入前已经修改 `subtask.external_path`，所以写失败时调用者会看到路径已设置。调用者若要重试或回滚，应显式考虑这一部分更新状态。
- 如果内联载荷带 `external_path` 但调用者没有提供 storage，解码仍成功，外置字段保持内联载荷中的值（正常 `marshal` 会清空这些字段）。这是一种允许延迟解析指针的边界，不等同于已得到完整元数据。
- `apply_compatibility` 以空 `row_start`/空 `meta_groups` 判断旧格式；合法的新格式若确实使用空起点，也会从 legacy 字段补值。修改兼容判据必须同时核对 v7.5 单索引数据。
- `get_step_executor` 在任何阶段选择前验证全部 `element_ids`；缺失返回 `IndexInfoNotFound(id)`。`Init`、`Done` 和未来未知阶段都返回 `UnknownStep`；本地模式的 `WriteAndIngest` 单独返回 `LocalImportHasNoWriteAndIngest`。
- 重试策略只把 `IndexInfoNotFound` 判为不可重试。它比 Go 的完整分类更粗：Go 还委托 `common.IsRetryableError`/`isRetryableError`，Go 测试中确认的“磁盘空间确定不足不可重试”没有对应的 Rust `ExecutorError` 变体；扩展错误模型时不可简单沿用“除索引缺失外全部重试”。

## 并发与资源生命周期

本文件本身没有并发原语。所有结构均以拥有值或借用 trait object 的方式同步执行；`ExternalMetaStorage::write` 需要 `&mut self`，在类型层面防止同一 storage 借用被并发写。实际跨节点并行、阶段调度、CPU/内存预算和执行资源由 DXF/scheduler 及各 step executor 管理。

资源生命周期的关键点是数据而非线程：外部元数据路径必须在后续阶段完成前保持可读；merge-sort 在调用外部写入前已清除自己的完成子任务缓存，即使外部写失败也不会恢复该缓存。`BackfillDistExecutor::close` 只设置布尔标志，没有关闭 Go `BaseTaskExecutor` 所拥有的上下文或资源，因此调用方不能依赖它完成实际资源回收。

幂等性由 `is_idempotent=true` 和子任务 `ts` 契约共同表达，但当前文件没有实施去重、事务提交或 checkpoint。安全重试依赖具体 step executor 和存储写入语义；新增阶段时必须在实际执行模块验证，而不能仅凭这个返回值推断。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/ddl/backfilling_dist_executor.go`](backfilling_dist_executor.go)。已对齐的语义包括：任务/子任务元数据的大体字段；全局排序大字段外置；外部载荷不递归携带自身路径；旧单索引 `SortedKVMeta` 向行范围和 `MetaGroups` 的兼容补齐；四个可执行 backfill 阶段；索引信息缺失不可重试；子任务幂等。

重要差异如下：

- Go 使用 JSON 和 `globalsort.BaseExternalMeta` 的标签驱动拆分；Rust 使用私有的 `ABFM\x01` 二进制格式。两者不是字节级互操作协议。
- Go 的 `BackfillTaskMeta` 内嵌完整 `model.Job`；Rust 将其缩为 `job_id/schema_id/table_id` 等标量，并额外含 `batch_size/max_write_speed`。Rust 当前不能仅凭该结构重建 Go `newBackfillStepExecutor` 所需的完整 job/table/runtime 上下文。
- Go `backfillDistExecutor` 嵌入 `BaseTaskExecutor`，`Init` 解码框架任务，`GetStepExecutor` 构造真实 read-index/merge-sort/cloud-import/merge-temp executor，`Close` 释放基类资源；Rust 门面只保存元数据、验证已有索引 ID、返回 `BackfillStep` 并设置关闭标志。
- Go `decodeBackfillSubTaskMeta`/`writeExternalBackfillSubTaskMeta` 已面向 `storeapi.Storage`；Rust 用本地 trait 解耦，且解码函数当前没有生产接线。
- Go 的重试分类能识别通用可重试错误和确定的环境失败；Rust 类型模型只特判 `IndexInfoNotFound`。因此 Rust 是聚焦移植而不是 Go 文件的完整等价实现。

## 扩展指南

- 新增/调整元数据字段时，必须同步修改 `marshal_all` 和 `unmarshal` 的同一顺序，并决定字段属于内联还是外置集合；若破坏旧布局，应增加协议版本和显式升级分支，而不是继续复用 `0x01`。
- 修改外置字段时同时检查 `BackfillSubTaskMeta::marshal` 的清空集合、`write_external_backfill_subtask_meta` 的内联字段清空集合、`decode_backfill_subtask_meta` 的合并集合，三者必须互为补集/逆操作。
- 新增阶段时，至少同步 `BackfillStep`、`BackfillDistExecutor::get_step_executor`、`LitBackfillScheduler::get_next_step` 及真实 step executor 模块；不要只让门面返回新枚举。
- 完善 DXF 接线时应以 Go `newBackfillStepExecutor`/`Init`/`GetStepExecutor` 为行为基准，引入真实 task/runtime、table/index 解析、session pool 和可取消上下文，而不是在当前轻量结构里假装资源已经存在。
- 扩展错误类型时，分别确定永久错误、瞬态错误和环境错误，补齐 Go 的 `common.IsRetryableError`/`isRetryableError` 语义；尤其保护“索引元数据缺失”和“确认磁盘不足”等不可重试分支。
- 测试继续放在独立的 [`backfilling_dist_executor_test.rs`](backfilling_dist_executor_test.rs)，覆盖协议版本、截断/UTF-8、无 storage 的外部指针、写失败后的部分状态、旧版兼容、新字段往返和阶段/重试矩阵；涉及真实阶段接线时同步相邻 step executor 测试，不把测试内嵌到生产文件。
- 兼容性风险集中在持久化元数据布局和 Go/Rust 差异；性能风险集中在大列表克隆、无上限长度前缀导致的内存分配，以及 `available_index_ids.contains` 的线性查找。优化时须先保留现有协议和错误行为。

## 验证依据

- 完整阅读：[`pkg/ddl/backfilling_dist_executor.rs`](backfilling_dist_executor.rs)、[`pkg/ddl/Cargo.toml`](Cargo.toml)、[`pkg/ddl/lib.rs`](lib.rs)、[`pkg/ddl/doc.go`](doc.go)。
- Go 对照：[`pkg/ddl/backfilling_dist_executor.go`](backfilling_dist_executor.go)；重试边界补证来自 [`pkg/ddl/backfilling_test.go`](backfilling_test.go) 的 `TestBackfillRetryableErrors`。
- Rust 独立测试：[`pkg/ddl/backfilling_dist_executor_test.rs`](backfilling_dist_executor_test.rs) 验证内外元数据往返、外部载荷不自引用以及当前重试分类；[`pkg/ddl/backfilling_test.rs`](backfilling_test.rs) 还覆盖初始化后的阶段选择。
- 直接调用/数据流证据：[`pkg/ddl/backfilling_dist_scheduler.rs`](backfilling_dist_scheduler.rs) 的 `LitBackfillScheduler`、`try_generate_plan_for_physical_table`、`plan_global_sort_ingest`；[`pkg/ddl/backfilling_merge_sort.rs`](backfilling_merge_sort.rs) 的 `MergeSortStepExecutor::run_subtask`。
- RustCodeGraph：`status` 报告索引包含 7,032 个 Rust 文件；`query BackfillDistExecutor --kind struct`、`query decode_backfill_subtask_meta --kind function`、`query write_external_backfill_subtask_meta --kind function` 和 `node BackfillDistExecutor` 确认目标符号与位置。`callers/callees` 未给出完整 Rust 调用边，故再以 `rg` 全仓搜索确认生产/测试调用点。
- 人工边界复核：当前文件是协议与轻量门面，不是完整 DXF executor；未把 DDL 设计文档概览或 Go 行为当作已接入 Rust 的事实。
- 本任务仅新增说明文档，不修改运行时代码；按任务约束不运行 Cargo。结构验证应确认文件存在且恰有上述 11 个固定二级标题。
