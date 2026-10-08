# `pkg/session/runtime/modify_column_cloud_executor.rs` 逻辑说明

## 文件定位

`modify_column_cloud_executor.rs` 属于 `astersql-session` crate（`pkg/session/Cargo.toml`），但没有在 `pkg/session/runtime.rs` 中直接声明模块，而是由 `pkg/session/runtime/modify_column_dist_backfill.rs` 通过 `#[path = "modify_column_cloud_executor.rs"] mod cloud;` 纳入分布式 MODIFY COLUMN / 索引回填实现。`modify_column_dist_backfill.rs` 的 `Extension::GetStepExecutor` 只在任务步骤为 `BackfillStepMergeSort` 或 `BackfillStepWriteAndIngest` 时构造 `cloud::CloudStep`；读取索引和临时索引归并仍由 `ReadIndex` 执行。

本文件是 DXF 云端回填后半段的步骤执行器：第一阶段把读取阶段产出的重叠 KV 文件归并为有序对象，第二阶段把对象存储中的有序文件注册成 Lightning 外部引擎并导入 TiKV。它定义两个私有/包内类型 `Active`、`CloudStep`，一个清理守卫 `ActiveGuard`，以及 `CloudStep` 的构造、归并、导入和 `executor::StepExecutor` 实现；没有 feature gate。唯一条件编译项是 `#[cfg(test)] #[path = "modify_column_cloud_executor_test.rs"] mod tests;`，测试保持在独立文件。

## 核心职责

- `BackfillStepMergeSort`：读取外置 `ExternalFields`，使用全局排序组件归并 `data_files`，累计真实 writer summary，成功后把单个 `meta_groups` 写回对象存储并更新 `subtask.Meta`。
- `BackfillStepWriteAndIngest`：从持久元数据恢复文件、范围、时间戳和索引信息，建立外部引擎，应用导入限速/并发配置，将排序结果导入 TiKV，记录已导入行数并清理已注册引擎。
- 实现 DXF `StepExecutor` 生命周期，包括导入后端初始化与释放、取消检查、实时进度、任务元数据热更新和 CPU/内存资源热更新。
- 在运行中的归并算子或外部引擎周围维护短生命的 `Active` 状态，使框架的资源修改回调只能调节当前真实执行对象，子任务结束后不会留下过期句柄。

## 主要符号

- `enum Active` 表示当前可调节的执行对象。`Merge(Arc<sort::merge::MergeOperator>)` 保存归并算子；`Import(Arc<sort::engine::ExternalEngineAdapter>, CancellationToken)` 同时保存外部引擎适配器和其资源更新所需的取消令牌。
- `CloudStep` 是包内步骤执行器。`base: ReadIndex` 复用 Domain、DDL job、索引 ID、云存储 URI、资源和写入限速器；`step` 决定归并或导入路径；`backend` 只为 WriteAndIngest 保存本地导入后端；`active` 保存正在运行的对象；`rows: AtomicU64` 保存最近一次已完成/中止导入采集到的行数。
- `CloudStep::new(base, step)` 拒绝空 `cloud_storage_uri`，初始化空后端、空活动对象和零行数。步骤合法性由上游 `Extension::GetStepExecutor` 保证，因此 `RunSubtask` 中“非 MergeSort 即导入”的分支不能脱离该工厂约束理解。
- `CloudStep::backend()` 在互斥锁下克隆后端 `Arc`；未执行导入步骤的 `Init` 或已 `Cleanup` 时返回 `local backend not found`。
- `run_merge` 创建 `NewMergeOperator`，以 `Memory / max(CPU, 1)` 计算每核内存、以 `max(CPU, 1)` 设置并发、以 `OnDuplicateKey::Error` 拒绝重复键；writer close 回调通过 `SortedMeta::from_merge` 汇总输出元数据。
- `run_import` 合并 `meta_groups`（为空时兼容旧任务内联的 `legacy` 字段），解码 job/split keys，读取当前表和索引元数据，创建并注册外部引擎，调用 `import_external_native`，采集统计并执行 `cleanup_external`。
- `ActiveGuard<'a>(&'a CloudStep)` 的 `Drop` 将 `active` 清空；它覆盖 `RunSubtask` 的成功和所有 `?`/错误返回路径。
- `StepExecutor::{Init, RunSubtask, Cleanup, RealtimeSummary, ResetSummary, TaskMetaModified, ResourceModified}` 把上述业务接入 DXF 生命周期。

## 执行流程

1. `modify_column_dist_backfill.rs` 的调度器在云存储 URI 非空时按 `ReadIndex -> MergeSort -> WriteAndIngest` 推进；执行器工厂为后两个步骤构造 `CloudStep`。
2. `Init` 对 MergeSort 不做额外工作；对 WriteAndIngest 读取当前 CPU 和存储 keyspace，选择 v1 或带 keyspace 的 v2 `KeyCodec`，再以编码后的空 key 前缀创建 `import_sst::Backend`。
3. `RunSubtask` 先检查 `ctx.Done()`，再由 `ReadIndex::import_control`取得共享取消标记和导入选项，打开 `CloudStore`，并从同一取消标记创建 globalsort `CancellationToken`。随后建立 `ActiveGuard`，保证离开函数时清空运行句柄。
4. MergeSort 路径调用 `wire::read` 合并 SQL 行内元数据与对象存储外置元数据，反序列化 `ExternalFields`，创建 `MergeOperator` 并发布为 `Active::Merge`。`MergeOverlappingFiles` 成功后将 callback 累积的 `SortedMeta` 设为唯一 `meta_groups`，把外置元数据写到 `<TaskID>/<SubtaskID>/meta.json`，最后替换 `subtask.Meta`。归并失败时替换动作不会发生。
5. WriteAndIngest 路径读取并解析元数据，合并所有排序组；旧任务没有 `meta_groups` 时改用扁平 `legacy`。它强制取得 `ts`，优先使用 `range_job_keys`、为空则兼容 `range_split_keys`，并分别解码 job keys、split keys 和总范围起止 key。
6. 导入路径从 Domain 存储的最新快照读取目标表，按 `base.index_ids` 找到索引；`ele_ids` 为一个元素时选对应索引 ID，为空时兼容旧任务并使用第一个索引，超过一个元素则报错。表名和索引 ID 经 `MakeUUID` 生成稳定 engine ID。
7. `NewExternalEngine` 使用文件列表、范围、分片 key、CPU/内存、TS、总文件大小及重复键报错策略创建 engine。后端应用当前 import options，注册 engine 后发布 `Active::Import`，执行 native import；无论导入成败都读取 `ImportedStatistics` 到 `rows` 并尝试 `cleanup_external`。
8. 框架可在运行中调用 `TaskMetaModified` 更新 WriteAndIngest 的最大写速率，或调用 `ResourceModified` 调节归并 worker 数/导入 engine CPU 和内存。步骤完成后调用 `Cleanup` 释放 `CloudStep` 持有的后端。

## 数据与状态

持久输入/输出是 `executor::Subtask.Meta`。`wire::read` 读取 JSON，并在存在 `ExternalPath` 时从 `CloudStore` 合并外置字段；`wire::write` 保留行内的表范围、物理表 ID、`ts` 等字段，同时把大字段写入对象存储。归并阶段只在完整成功后发布新的 `meta_groups` 和元数据指针，测试明确验证失败/取消时原 `subtask.Meta` 不变。

`CloudStep` 的共享可变状态分为三类：`Mutex<Option<Arc<Backend>>>` 管理步骤级本地后端；`Mutex<Option<Active>>` 管理子任务级可调执行对象；`AtomicU64 rows` 用 Release 写、Acquire 读保存导入行数。`base.resource` 与 `base.job.reorg_meta` 是从 `ReadIndex` 复用的共享状态，资源更新在实际执行对象调节成功后才写回，写速率更新同时作用于 job 元数据和 `write_limiter`。

`RealtimeSummary` 在导入进行中直接读取 engine 的 `ImportedStatistics().1`，否则读取原子缓存；归并步骤没有写 `rows`，因此这里只报告零或先前重置后的值，并不报告归并 KV 数。`ResetSummary` 仅清零 `rows`。对象存储访问统计和 Go 版本中的 Lightning metric collector 没有在本文件内对应实现，不能据此宣称 Rust 路径已提供同等计量。

## 依赖与调用关系

上游直接调用者是 `modify_column_dist_backfill.rs` 的 `Extension::GetStepExecutor`：它从任务 Meta 构造 `ReadIndex`，注入节点资源，然后为 MergeSort / WriteAndIngest 返回 `Arc<dyn StepExecutor>`。同文件的 `Planner::next_step` 和 `on_next_subtasks_batch` 决定云回填步骤顺序及子任务元数据。RustCodeGraph 的文件节点也将 `modify_column_dist_backfill.rs`、`runtime/session.rs` 和 `normal_ddl_test.rs` 列为使用者；真实构造边由前述工厂源码确认。

直接下游包括：`modify_column_cloud_meta.rs` 的 `read`/`write`/`decode`、`ExternalFields` 与 `SortedMeta`；`modify_column_cloud_store.rs` 的 `CloudStore::open` 及 globalsort `Storage` 实现；`modify_column_dist_backfill.rs` 的 `ReadIndex`/`ImportControl`；`runtime/import_sst.rs` 的 `Backend`；globalsort 的 merge、reader 和 external-engine API；Domain/KV snapshot 与 meta `SnapshotReader`；Lightning `MakeUUID`；以及 keyspace-aware `KeyCodec`。

`pkg/session/Cargo.toml` 明确声明本文件使用的 `astersql-ingestor-globalsort`、`astersql-ingestor-ingestctrl`、`astersql-ingestor-simplesst`、`astersql-lightning-backend`、`astersql-store-copr`、`astersql-dxf-framework-proto`、`astersql-dxf-framework-taskexecutor*` 和对象存储 crates。crate 只有 `nextgen` feature，本文件没有按 feature 改变控制流。

## 错误处理与边界

所有业务入口返回 `executor::Result`，外部库错误统一通过本模块可见的 `error(...)` 转换。构造时拒绝空云存储 URI；导入时显式检查后端、时间戳、目标表、目标索引、索引集合、`ele_ids` 数量、base64 key 和排序元数据。CPU 均以 `max(1)` 约束实际并发，避免零 CPU 导致除零或零 worker；内存值则按资源对象原样传递。

重复键策略在归并和导入引擎两端都是 `OnDuplicateKey::Error`。归并 writer summary 转 wire metadata 的回调使用 `unwrap`：源码注释给出的不变量是 summary 中的 raw byte key 总能编码为合法 base64；互斥锁中毒也被视为不可恢复。其他锁操作通过 `map_err(error)` 正常返回错误。

导入结束时先保存 import 结果，再采集行数并尝试清理 engine，最终使用 `result.and(cleanup)`：导入成功但清理失败会返回清理错误；导入已经失败时仍执行清理，但保留原导入错误。`Cleanup` 只丢弃步骤持有的 backend，子任务 engine 的显式清理由 `run_import` 负责。

`ResourceModified` 没有活动子任务时返回 `no subtask running`，由框架重试。WriteAndIngest 若 CPU 与当前资源相同，会按 Go 行为忽略仅内存变化，包括空闲期；否则必须先更新活动 engine，再更新 backend worker concurrency 和缓存资源。`TaskMetaModified` 对 MergeSort 是 no-op，对导入步骤要求新旧 job 都有 reorg metadata。

## 并发与资源生命周期

`CloudStep` 实现的 `StepExecutor: Send + Sync` 可能同时接收运行、进度和资源回调。`backend`、`active` 和 `base.resource` 分别由互斥锁保护；行数用 Acquire/Release 原子序保证跨线程可见。`Active` 中的 `Arc` 允许资源回调短暂克隆运行对象而不依赖锁的整个调用期。

归并 writer close callback 可由 worker 并发调用，因此通过 `Arc<Mutex<SortedMeta>>` 串行合并 summary。归并算子和外部引擎使用从 `ImportControl.cloud_cancelled` 派生的同一取消信号，`CloudStore` 的 reader/writer 也检查该信号；此外 `RunSubtask` 在启动工作前检查 DXF context 是否已取消。

`ActiveGuard` 在子任务任何退出路径清空 `active`，防止完成后继续 Tune；Rust 测试验证成功、重复键失败和取消失败后三种情况下均为空。WriteAndIngest 后端从 `Init` 存活到 `Cleanup`，每个外部 engine 则从 `register_external` 存活到 `cleanup_external`。调用方必须保持 DXF 的 Init -> RunSubtask -> Cleanup 顺序，并避免对同一个 `CloudStep` 并发运行多个子任务，因为单一 `active` 槽和单一 engine 生命周期没有表达多子任务并行所有权。

## 与 Go 版本的对应关系

Rust 的一个 `CloudStep` 按 `step` 合并了 Go 的两个执行器：`pkg/ddl/backfilling_merge_sort.go` 的 `mergeSortExecutor` 对应 `run_merge` 和 Merge 分支资源调节；`pkg/ddl/backfilling_import_cloud.go` 的 `cloudImportExecutor` 对应 `Init`、`run_import`、导入摘要、任务 Meta 更新及 Import 分支资源调节。

核心语义保持一致：归并使用每核内存、对象前缀 `<task>/<subtask>`、writer summary、重复键报错，成功后才写外置 Meta；导入合并排序组、兼容缺失 job keys 的旧 Meta、以表名和索引 ID 生成 engine UUID、注册 external engine 并动态调节 CPU/内存；空闲期资源修改返回可重试错误，CPU 未变时忽略 import 的 memory-only 更新；任务 Meta 修改只更新最大写速率。

可见差异也必须保留：Go `cloudImportExecutor` 持有物理表和指标/对象存储计量，能尝试把重复键错误转换成带索引/表信息的 `ErrKeyExists`；Rust 当前从最新 snapshot 重取表，仅统一传播 backend/globalsort 错误，本文件没有 Go 的 metric collector、对象存储请求摘要或重复键错误再包装。Go 对旧 Meta 的 `EleIDs` 为空处理默认依赖索引列表，Rust 显式拒绝空索引列表；Rust 还显式支持 `legacy` 扁平排序元数据。仓库搜索未找到直接命名这些 Go 执行器的 `*_test.go`，因此 Go 对照依据是生产实现而非同名测试。

## 扩展指南

新增云回填步骤时，应先在 `modify_column_dist_backfill.rs` 的工厂、步骤推进和 planner 子任务生成中建立显式分支，再扩展 `CloudStep`；不要依赖当前“非 MergeSort 即 import”的内部二分逻辑接受第三种步骤。若新步骤需要动态资源调节，应为 `Active` 增加能够完整表达其运行句柄和取消上下文的变体，并确保 `ActiveGuard` 继续覆盖所有返回路径。

修改归并元数据时，应同步检查 `modify_column_cloud_meta.rs` 的 Go JSON 字段名、旧任务兼容字段和 `wire::write` 的行内/外置合并规则；只有完整成功后才能替换 `subtask.Meta`。修改导入选择索引或 engine ID 时，应同时核对 Go `getIndexInfoAndID`、Lightning `MakeUUID` 和旧版本 `EleIDs` 为空的行为，避免升级期间无法恢复既有任务。

资源与限速扩展应保持“先调真实对象，成功后提交缓存状态”的顺序，并为 idle、CPU 不变但内存变化、取消和锁失败定义行为。进度扩展若要覆盖 MergeSort，不能复用当前仅表示 import rows 的 `rows` 而混淆单位。

回归测试应扩展独立文件 `pkg/session/runtime/modify_column_cloud_executor_test.rs`，不要把测试逻辑写入生产文件。现有用例应继续覆盖归并成功、重复键、取消和 Meta 原子发布；新增导入行为至少应覆盖 Init/Cleanup、旧/新 Meta、表或索引缺失、engine 注册/清理、导入失败、行数摘要、TaskMetaModified 及 ResourceModified 的运行中/空闲/CPU 不变分支。正确性风险集中在错误发布 Meta、错误索引 UUID 和清理遗漏；兼容风险集中在 Go JSON/旧任务 Meta；性能风险集中在每核内存、worker 并发和大规模 metadata 合并。

## 验证依据

本说明直接核对了生产源 `pkg/session/runtime/modify_column_cloud_executor.rs`、模块入口与工厂 `pkg/session/runtime.rs` 和 `pkg/session/runtime/modify_column_dist_backfill.rs`、crate 声明 `pkg/session/Cargo.toml`、独立 Rust 测试 `pkg/session/runtime/modify_column_cloud_executor_test.rs`、元数据与存储边界 `pkg/session/runtime/modify_column_cloud_meta.rs` 和 `pkg/session/runtime/modify_column_cloud_store.rs`、StepExecutor 契约 `pkg/dxf/framework/taskexecutor/interface.rs`，以及 Go 对照 `pkg/ddl/backfilling_merge_sort.go`、`pkg/ddl/backfilling_import_cloud.go`。`pkg/session` 下不存在 `doc.go`；仓库搜索也未发现直接覆盖这两个 Go 执行器名称/故障注入点的 `*_test.go`。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime` 确认目标源和独立测试均已索引，`query CloudStep` 定位类型，`query RunSubtask` / `ResourceModified` / `TaskMetaModified` 同时定位 Rust 实现、StepExecutor trait 与两份 Go 对照，`node --file pkg/session/runtime/modify_column_cloud_executor.rs` 读取了完整 350 行源码并给出使用文件。精确 callers/callees 查询未在限定时间返回，因此具体构造边和下游调用由索引源码、`rg` 模块引用及直接文件读取交叉确认，不把查询超时解释为没有调用关系。

本任务仅新增说明文档，按计划不运行 Cargo。交付前运行任务指定的结构命令确认文档存在且恰有 11 个固定二级章节，并人工复核本文能回答文件为何存在、两个步骤如何运行、状态何时发布/清理、与 Go 的差异以及如何安全扩展。
