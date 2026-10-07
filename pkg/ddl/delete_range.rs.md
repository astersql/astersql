# `pkg/ddl/delete_range.rs`

对应源码：[delete_range.rs](delete_range.rs)。

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 公开为 `pub mod delete_range`。它位于持久化 DDL 作业完成链的末端：`pkg/ddl/table_mode.rs` 中 `NormalDdlExecutor::finish` 先用 `persistent_job_need_gc` 判断作业是否需要垃圾回收范围，再通过 `DurableJobSession::register_delete_ranges` 注册范围；`pkg/session/runtime/system_session.rs` 的实现最终调用 `add_persistent_delete_range_job`，将记录写入 `mysql.gc_delete_range`。这些记录描述旧表、旧分区或旧索引的 KV 前缀范围，实际物理删除由后续 GC/兼容存储流程执行，本文件本身不扫描或删除业务 KV。

文件同时保留一套较轻量的、字段已展开的模型：`DeleteRangeAction`、`DeleteRangeJob`、`DeleteRangeTask` 和内存 `DeleteRangeManager`。仓库精确调用搜索显示，生产主链调用的是持久化 `Job` 路径；`add_delete_range_job` 和内存管理器没有生产调用者，前者目前由 `pkg/ddl/delete_range_test.rs` 直接验证。因此阅读和扩展时不能把内存队列当作当前服务器的 GC 队列实现。

从 DDL 生命周期看，这是 job-based 流程的完成阶段接线，不负责 schema state 转换、reorg 扫描或 schema version 推进。删除范围注册失败会阻止作业从活动队列移入历史；注册成功后，范围记录可独立于外层 DDL worker 事务存活（见 `DeleteRangeExecutor` 注释及 `normal_ddl_plan_delete_range_worker_conflict_preserves_gc_and_retry`）。

## 核心职责

1. `persistent_job_need_gc` 对持久化 `astersql_meta_model::group_3::Job` 做动作、状态和特殊参数过滤，决定完成作业是否必须登记 GC 范围。
2. `add_persistent_delete_range_job`/`persist_job_ranges` 解码完成态作业参数，把每个旧物理对象转换为 `[start_key, end_key)`，以独立执行器批量执行 `INSERT IGNORE INTO mysql.gc_delete_range`。
3. `finished_range_batches` 覆盖 Go 版本的动作矩阵，包括删库/表/分区/索引、truncate、列变更、索引回滚、分区重组和物化视图清理，并兼容 V1 数组参数与 V2 对象参数。
4. `table_tasks`、`index_tasks` 统一生成 tablecodec 前缀范围，并通过 `ElementIdAllocator` 在同一父作业内稳定、去重地分配 `element_id`。
5. `DeleteRangeJob` 路径提供不依赖完整持久化 `Job` 的范围生成模型；`DeleteRangeManager` 提供启动、重试、完成归档和移除记录的内存语义模型，但目前不是生产后台 worker。

这里的“删除”是登记异步清理意图，而不是在 DDL 提交路径逐行删除。表范围使用 `[EncodeTablePrefix(id), EncodeTablePrefix(id + 1))`，索引范围使用 `[EncodeTableIndexPrefix(table, index), EncodeTableIndexPrefix(table, index + 1))`；Rust 使用 `wrapping_add(1)` 显式保持 Go 有符号整数溢出语义。

## 主要符号

- `DEL_RANGE_EMULATOR_TASK_DEL_BATCH = 65_536`：仅供内存 `DeleteRangeManager::do_delete_range_work` 传给删除回调的批量上限，对应 Go `delBatchSize`。
- `BATCH_INSERT_DELETE_RANGE_SIZE = 256`：`finished_range_batches` 对 `DROP SCHEMA` 的大量物理表 ID 分批，避免单条插入无限增长，对应 Go `batchInsertDeleteRangeSize`。
- `TEMPORARY_INDEX_PREFIX`：直接取 `astersql_tablecodec::TempIndexPrefix`；AddIndex/AddPrimaryKey 完成或回滚时用于定位临时索引键空间。
- `DeleteRangeAction`：轻量动作枚举。它覆盖主要表、分区、索引和列变更，并用 `MultiSchemaChange` 递归代理子作业；`Other` 明确不生成范围。它不等同于完整的 `Job.tp` 动作集合。
- `IndexArgument`：轻量索引元数据，区分全局索引、列存索引及所属表。
- `DeleteRangeJob`：轻量作业投影，包含 job/table 身份、取消/回滚状态、物理表/分区/索引集合和子作业。
- `DeleteRangeTask`：最终范围记录的内存表示，主键语义由 `(job_id, element_id)` 提供，范围为半开区间。
- `add_delete_range_job`：创建新的 `ElementIdAllocator`，把一个轻量作业转换成任务列表。
- `insert_job_into_delete_range`：轻量动作分发器；取消作业直接返回空，多 schema 子作业继承父作业的 `id` 和 `table_id`。
- `table_tasks`/`index_tasks`：共享的范围编码器，也被持久化路径复用。
- `DeleteRangeManager`：包含 `pending`、`completed`、`started` 的同步内存状态机；`do_delete_range_work` 对失败任务保留重试。
- `DeleteRangeExecutor`：生产持久化边界所需的最小接口。`current_version` 获取 TSO，`execute` 在调用方提供的独立系统会话执行 SQL。
- `persistent_job_need_gc`：对应 Go `JobNeedGC` 的持久化判断器。取消、`[ddl:1091]` 缺失字段/索引警告和列存索引会被过滤；MultiSchemaChange 递归检查代理作业。
- `add_persistent_delete_range_job`：生产公开入口；同一父作业及其全部子作业共享一个 `ElementIdAllocator`。
- `persist_job_ranges`：每个（代理）作业先取一次当前版本，再逐批生成十六进制键并执行 `INSERT IGNORE`。
- `finished_range_batches`：完整 `Job` 的动作解码与范围批次生成核心。
- `legacy_finished_table_ids`、`finished_index_args`、`normalized_legacy_gc_job`：V1 兼容层；在临时副本上校验旧 StartKey 字节编码、把 Go `nil` slice 的 JSON `null` 归一成数组，并保持原始历史参数不变。

## 执行流程

生产流程如下：

1. `NormalDdlExecutor::finish` 将 Done 作业推进为 Synced，并处理取消/回滚作业的 RU；随后调用 `persistent_job_need_gc`。
2. 若需要清理，`DurableJobSession::register_delete_ranges` 经 `pkg/session/runtime/system_session.rs` 取得独立系统 session，构造实现 `DeleteRangeExecutor` 的 `SystemSessionLease`，进入 `add_persistent_delete_range_job`。
3. 普通作业直接进入 `persist_job_ranges`；MultiSchemaChange 则按顺序调用 `SubJob::to_proxy_job`，只处理 `persistent_job_need_gc` 为真的子作业，并让所有子作业共享一个 allocator。
4. `persist_job_ranges` 先调用一次 `current_version` 得到该代理作业所有批次共用的 `ts`，再由 `finished_range_batches` 解码完成态参数。
5. `finished_range_batches` 依动作生成任务：表/分区用 `table_tasks`，索引用 `index_tasks`；DropSchema 每 256 个表 ID 一批；Drop/Truncate Table 除旧分区外总会加入逻辑表范围以覆盖全局索引区域；分区重组先登记被替换的全局索引，再登记旧分区；AddIndex 为普通索引逐物理分区生成临时索引范围，为全局索引只用逻辑表 ID，RollbackDone 时还加入正式索引范围；Drop/Modify Column 从专门模块读取完成态索引 ID。
6. 每批任务编码为 `(job_id, element_id, start_key, end_key, ts)`，键转为小写十六进制文本，使用 `INSERT IGNORE` 写入 `mysql.gc_delete_range`。空批次跳过。
7. 全部登记成功后，外层 `finish` 才继续写历史并删除活动作业；任一步返回错误都会中止完成流程，保留作业供后续重试。

轻量流程则是 `add_delete_range_job → insert_job_into_delete_range → table_tasks/index_tasks`。内存管理器的 `add_job` 把结果加入 `pending`；启动后，`do_delete_range_work` 对每项调用回调，成功移入 `completed`、失败留在 `pending`；`remove_from_gc_delete_range` 按 job ID 删除完成记录。该流程没有 SQL、TSO 或真实 GC worker 接线。

## 数据与状态

关键持久状态位于 `mysql.gc_delete_range`，每行含 `job_id`、`element_id`、`start_key`、`end_key`、`ts`。`job_id + element_id` 的稳定组合支持 `INSERT IGNORE` 幂等重试：同一完成作业再次执行不会分配额外范围或覆盖原时间戳。`ElementIdAllocator` 同时维护物理表 ID 和 `(physical_id, index_id)` 两个映射，但共享从 1 开始的单调编号空间；同一对象重复出现会复用编号，多 schema 子作业也共享该空间。

范围是不包含结尾的半开区间。表范围覆盖整个 tablecodec 表前缀；索引范围只覆盖给定表/分区下的一个索引前缀。全局索引用逻辑表 ID，普通分区索引用分区物理 ID。临时索引 ID 通过 `TEMPORARY_INDEX_PREFIX | index_id` 形成。

持久化输入 `Job` 的 `version` 决定参数布局：V2 调用 `GetFinished*Args` 读取对象字段；V1 先在克隆的作业上把特定 slice 位置的 `null` 归一化，再复用解码器。原作业的 `raw_args` 不被兼容转换覆盖。旧 Drop/Truncate Table 的 `StartKey` 虽不再决定范围，仍验证其 Go `[]byte` JSON 表示，防止接受不兼容的历史载荷。

内存 `DeleteRangeManager` 的 `started` 只是执行门闩，`clear` 会停止并清空 `pending`，但保留 `completed`；该状态没有锁，也没有跨进程持久性。

## 依赖与调用关系

上游生产边：

- `pkg/ddl/table_mode.rs::NormalDdlExecutor::finish` → `persistent_job_need_gc`。
- `NormalDdlExecutor::finish` → `DurableJobSession::register_delete_ranges` → `pkg/session/runtime/system_session.rs` → `add_persistent_delete_range_job`。
- `pkg/ddl/lib.rs` 将模块公开给 `pkg/session` crate。

主要下游边（RustCodeGraph 的 `callees` 与源码核对）：

- `add_persistent_delete_range_job` → `persistent_job_need_gc`、`persist_job_ranges`。
- `persist_job_ranges` → `DeleteRangeExecutor::{current_version, execute}`、`finished_range_batches`。
- `finished_range_batches` → `table_tasks`、`index_tasks`、`legacy_finished_table_ids`、`finished_index_args`、`normalized_legacy_gc_job`，并调用 `astersql_meta_model::group_2` 的各类 `GetFinished*Args`。
- DropColumn/ModifyColumn 分支分别下沉到 `pkg/ddl/persistent_drop_column.rs::finished_range_ids` 与 `pkg/ddl/persistent_modify_column.rs::finished_range_ids`。
- `table_tasks`/`index_tasks` → `pkg/ddl/delete_range_util.rs::ElementIdAllocator` 与 `astersql_tablecodec::{EncodeTablePrefix, EncodeTableIndexPrefix}`。

crate 依赖依据见 `pkg/ddl/Cargo.toml`：直接使用 `astersql-meta-model`、`astersql-tablecodec`、`serde`/`serde_json`；`Key` 来自 crate 内 `backfilling`，element allocator 来自 crate 内 `delete_range_util`。Cargo 没有为本模块定义单独 feature，模块在库入口中无条件编译，只有其独立测试模块受 `#[cfg(test)]` 控制。

## 错误处理与边界

`persistent_job_need_gc` 是保守过滤器而不是错误返回接口：取消作业、不相关动作、缺失字段/索引警告返回 `false`；DropIndex 参数无法解码或缺少第一项时也返回 `false`。这与后续 `finished_range_batches` 的严格解码不同：一旦作业已被判断为需要 GC，完成参数缺失、JSON 无效、V1 字节编码错误或缺少索引参数会返回 `Err(String)`，阻止作业完成。

`persist_job_ranges` 对 `current_version`、参数解码或任一 SQL 批次错误立即 `?` 返回。前面已成功提交的独立批次不会被外层 DDL worker 回滚；重试依赖相同 element ID 和 `INSERT IGNORE` 收敛。`normal_ddl_plan_delete_range_gc_sql_failure_retries_before_history` 证明系统表缺失时作业仍留在队列、不会进入历史；修复表后可重试。

边界分支包括：取消作业为空；DropIndex 的列存索引不登记 KV 范围；CreateMaterializedView 仅在 RollbackDone 且已有非零 table ID 时清理；AddIndex 正常完成只清临时索引，回滚完成同时清正式与临时索引；空 index ID 集合不生成记录；Drop/Truncate Table 始终包含逻辑表范围；`wrapping_add` 保留 `i64::MAX → i64::MIN` 的 Go 溢出行为。

轻量分发器对 DropIndex 缺失首个 `IndexArgument` 返回空而非错误。内存管理器的删除回调只有布尔成功/失败，不能携带错误细节；这进一步说明它不应被当作生产错误模型。

## 并发与资源生命周期

生产范围注册使用独立、autocommit 的系统 session。`SystemSessionLease` 从池中取得 context，在 `register_delete_ranges` 返回时随 lease 生命周期归还；每个代理作业只获取一次 TSO。该设计让已成功的 GC 范围批次不受包围它的 DDL worker 事务回滚影响。`normal_ddl_plan_delete_range_worker_conflict_preserves_gc_and_retry` 验证 worker 提交冲突后范围仍存在、作业仍可重新调度，重试不会复制记录。

同一调用内没有线程共享 allocator：它是栈上的可变对象，并按作业/子作业顺序确定性分配 ID。多 schema 子作业必须共用该实例，否则相同父 job 下可能产生主键碰撞或不稳定编号。当前文件不创建线程、异步任务、锁或通道；实际 Go `delRange` 的 goroutine、通知 channel、WaitGroup 和不支持原生 DeleteRange 存储时的模拟删除器没有在这个 Rust 生产文件中复刻。

内存 `DeleteRangeManager` 要求调用者以 `&mut self` 串行访问；`do_delete_range_work` 先 drain `pending`，再按回调结果重建失败列表，因此一次调用中每项只处理一次。它没有持久化、崩溃恢复或并发同步保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/delete_range.go`，GC 判定在 `pkg/ddl/job_worker.go::JobNeedGC`。主要对应关系如下：

- Rust `persistent_job_need_gc` 对应 Go `JobNeedGC`；动作、取消状态、1091 warning、列存索引和 multi-schema 递归规则保持一致。
- Rust `add_persistent_delete_range_job` 对应 Go `AddDelRangeJobInternal`；两者都为一个父作业创建一次 element allocator，并将 multi-schema 子作业转为 proxy job。
- Rust `finished_range_batches` + `persist_job_ranges` 对应 Go `insertJobIntoDeleteRangeTable` 及 `doBatchDeleteTablesRange`/`doBatchDeleteIndiceRange`。
- Rust `DeleteRangeExecutor` 是 Go `DelRangeExecWrapper` 的收窄版本：当前生产实现只暴露取版本和执行 SQL，没有 Go 为 BR 提供的 `RewriteTableID`、参数列表构造和消费回调。因此本文件当前不提供 BR 表 ID 重写扩展点。
- Rust `ElementIdAllocator` 对应 Go `elementIDAlloc`，保证对象到 element ID 的幂等映射。
- Rust 的持久化路径包含 Go 动作矩阵中的物化视图动作；前半轻量 `DeleteRangeAction` 没有这些变体，不能作为完整兼容清单。
- Go `delRange` 还负责原生 DeleteRange 不可用时的后台模拟器：加载范围、每批最多 65,536 个键、事务删除、推进 start key、完成迁移记录。Rust 文件仅有布尔回调的内存模型，没有与 Go `LoadDeleteRanges`/`CompleteDeleteRange`/`UpdateDeleteRange` 等价的生产执行链。

Go 测试 `pkg/ddl/delete_range_test.go` 重点覆盖 BR-style 表 ID 重写后跳过行和 SQL 逗号拼接；这些能力不在当前 Rust executor 接口中。Rust 的直接单元测试覆盖整数环绕、multi-schema 父身份和列存过滤，生产持久化语义则主要由 `pkg/session/runtime/normal_ddl_test.rs` 的集成式测试覆盖。

## 扩展指南

新增一种需要 GC 的 DDL 动作时，至少同步检查四处：`persistent_job_need_gc` 的筛选、`finished_range_batches` 的完成参数解码与范围生成、Go `JobNeedGC`/`insertJobIntoDeleteRangeTable` 的语义，以及独立测试。若轻量模型仍要保持可比性，还需更新 `DeleteRangeAction` 与 `insert_job_into_delete_range`；不要只改轻量分支而遗漏生产持久化入口。

新增范围类型时优先复用 `table_tasks`/`index_tasks`，并确保 element allocator 的对象身份可稳定复现。改变生成顺序、allocator 生命周期或 batch 边界会影响 `(job_id, element_id)` 幂等键，必须验证 worker 冲突和部分批次重试。键边界必须继续使用 tablecodec，且保持半开区间及有符号溢出兼容；不要用手工拼接替代。

修改 V1 参数兼容时只应在克隆作业上归一化，并补充 null slice、旧 `[]byte` Base64/数组编码和错误载荷测试；历史 `raw_args` 是兼容边界，不应为便于解码而原地改写。新增 V2 动作应优先使用 `astersql_meta_model::group_2` 的完成态参数函数，而不是在此重复定义 JSON 结构。

需要 BR 表 ID 重写时，必须先设计并扩展 `DeleteRangeExecutor`/任务生成边界，不能假定现有 Rust 路径已具备 Go `RewriteTableID` 语义。需要真实后台模拟删除器时，应连接 `pkg/ddl/util/util.rs` 的持久任务加载/完成接口并设计停止、重试和事务边界，而不是直接提升当前无锁内存 `DeleteRangeManager`。

测试应继续放在独立文件：纯范围规则放 `pkg/ddl/delete_range_test.rs`；完成作业、系统表、独立 session、历史迁移和重试放 `pkg/session/runtime/normal_ddl_test.rs`。涉及 Go 对齐时同步核对 `pkg/ddl/delete_range_test.go` 与 `pkg/ddl/job_worker_test.go::TestJobNeedGC`。

## 验证依据

- 源码与模块：`pkg/ddl/delete_range.rs`（全部 691 行）、`pkg/ddl/lib.rs`、`pkg/ddl/delete_range_util.rs`、`pkg/ddl/persistent_drop_column.rs`、`pkg/ddl/persistent_modify_column.rs`。
- crate 与包契约：`pkg/ddl/Cargo.toml`、`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`。DDL 文档只作为入口，本文行为结论均回查源码/测试。
- 生产上游：`pkg/ddl/table_mode.rs::NormalDdlExecutor::finish`、`pkg/session/runtime/system_session.rs::register_delete_ranges`。
- Go 对照：`pkg/ddl/delete_range.go`、`pkg/ddl/job_worker.go::JobNeedGC`；Go 测试为 `pkg/ddl/delete_range_test.go`、`pkg/ddl/job_worker_test.go::TestJobNeedGC`。
- Rust 测试：`pkg/ddl/delete_range_test.rs`；持久化动作矩阵、V1、multi-schema、取消/回滚/warning、SQL 失败、worker 冲突与提交冲突证据来自 `pkg/session/runtime/normal_ddl_test.rs` 中 `normal_ddl_plan_delete_range_*` 测试。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、索引时间戳 `1791342965170`；`node --file pkg/ddl/delete_range.rs` 读取全文件；对 `add_delete_range_job`、`insert_job_into_delete_range`、`persistent_job_need_gc`、`add_persistent_delete_range_job`、`finished_range_batches` 执行了 `query`/`callers`/`callees`。图确认 `finished_range_batches` 到 table/index helpers、V1 helpers、持久列变更模块及 `GetFinished*Args` 的下游边；公开入口 caller 边为空，因此使用精确 `rg` 补证了 `table_mode.rs` 与 `system_session.rs` 的生产接线。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构检查要求文档恰含十一个固定二级标题。
