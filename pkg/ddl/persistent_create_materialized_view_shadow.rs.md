# `pkg/ddl/persistent_create_materialized_view_shadow.rs`

## 文件定位

源码：[persistent_create_materialized_view_shadow.rs](./persistent_create_materialized_view_shadow.rs)。本文件属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml`），是普通 DDL worker 持久化事务路径中“创建物化视图影子表”的 Rust 动作处理器。模块由 `pkg/ddl/lib.rs` 公开装配；`pkg/ddl/persistent_actions.rs::step` 在 `job.tp == 93`（`ActionCreateMaterializedViewShadow`）时调用本文件的 `step`。上游 Go 接口 `pkg/ddl/materialized_view.go::CreateMaterializedViewShadowTable` 创建该类型的 DDL job，用于 `REFRESH MATERIALIZED VIEW ... COMPLETE OUT OF PLACE` 的构建目标。

它不是完整的物化视图刷新或切换实现：本文件只创建受保护的物理影子表。将影子表原子提升为正式物化视图由 `pkg/ddl/persistent_mview_out_of_place_cutover.rs` 负责，清理由独立的 drop-shadow 动作负责。

## 核心职责

`step` 依次完成四类工作：

1. 从兼容 Go wire 格式的 `CreateTableArgs` 解码表元数据和 `FKCheck`。
2. 验证目标确实是影子物理表，并验证它引用的源对象存在、是物化视图且处于 `Public` 状态。
3. 复用 `persistent_create_table::create_table` 创建表元数据及相关外部资源，但不走普通建表处理器的通知、TTL 注册和多阶段外键状态机。
4. 生成 schema version、写 create-table schema diff、回写 job 参数中的最终表信息，并以 `Done/Public` 结束 job。

文件坚持的核心不变量是：影子表必须带非零 `MaterializedViewShadow.SourceMViewID`，同时自身不能带 `MaterializedView`、`MaterializedViewLog`、`View` 或 `Sequence` 身份；源对象必须是同一 `job.schema_id` 下已经公开的物化视图。

## 主要符号

- `cancel(job: &mut Job, error: impl ToString) -> String`：把 job 状态改为 `JobState::Cancelled`，并把错误转换为持久化执行层使用的字符串。它只负责状态和错误文本，不直接回滚事务。
- `invalid(job: &mut Job, reason: &str) -> String`：构造 `ErrInvalidDDLJob`，统一添加 `create materialized view shadow table:` 前缀，再委托 `cancel`。用于输入元数据形状不合法的确定性错误。
- `pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：唯一公开业务入口。成功时返回新 schema version，并通过 `finish_table_job` 写入终态和历史表快照；失败时返回字符串错误，是否立即取消取决于失败点。

本文件没有类型、trait、常量或条件编译项。`step` 是 crate 外可见的 `pub` 函数，但实际接线入口是 crate 内的 `persistent_actions::step`。

## 执行流程

1. `persistent_create_table::decode_optional(job)` 调用 `GetCreateTableArgs`，兼容 job V1/V2 参数格式，并把 wire 表信息投影为当前 Rust `TableInfo`。参数解码失败会取消 job。
2. 缺少 `TableInfo` 或 `MaterializedViewShadow` 时，以 `ErrInvalidDDLJob` 取消；随后读取 `SourceMViewID`。
3. 如果目标还携带物化视图、物化视图日志、普通视图或序列标记，拒绝将它作为受保护物理表创建；`SourceMViewID == 0` 也被拒绝。
4. 在 `context.with_transaction` 中通过 `TransactionMutator` 检查数据库和源表。数据库/源表不存在、源对象类型错误或源状态非 `Public` 都会取消 job。源表按 `(job.schema_id, source_id)` 查询，因此引用被限定在 job 的 schema 内。
5. 调用 `persistent_create_table::create_table(context, job, &mut table, fk_check)`。该共享助手执行列式副本校验、名称和约束冲突检查、外键检查与编号、表合法性检查、元数据创建、placement/副本/affinity 外部资源配置以及 auto-ID rebase；它将目标表状态设为 `Public`。
6. 再次通过 `with_transaction` 生成 schema version，并调用 `set_create_table_schema_diff(job, version, !table.ForeignKeys.is_empty())` 写差异。这里沿用 create-table diff 格式；有外键时 `old_table_id` 标记为当前 `job.table_id`。
7. `persistent_create_table::save_args` 把最终 `TableInfo` 写回 `job.raw_args`：V1 修改数组第 0 项，V2 修改 `table_info` 字段，并清空解码缓存 `job.args`。
8. `job.finish_table_job(Done, Public, version, Arc<table>)` 更新 job 状态、schema 状态及 `binlog_info` 历史快照，最后返回 version。

`pkg/ddl/table_mode.rs` 在动作执行前建立 statement stage；本函数成功时释放 stage，出错时清理 stage。更外层 `JobWorker::transit_persisted_job_step` 在同一持久化 worker 会话中更新 `mysql.tidb_ddl_job` 并提交，lease 丢失或提交失败则回滚。

## 数据与状态

- 输入 job：关键字段为 `tp=93`、`schema_id`、`schema_name`、`table_id`、`raw_args` 和 `version`。`raw_args` 承载 `CreateTableArgs { TableInfo, FKCheck }`。
- 目标 `TableInfo`：`MaterializedViewShadow.SourceMViewID` 建立影子表到源物化视图的身份联系；创建过程中状态由共享助手设置为 `None`，通过元数据校验后变为 `Public`。
- 源 `TableInfo`：只读校验 `MaterializedView.is_some()` 和 `State == Public`，本文件不修改源对象。
- 持久化元数据：共享助手写表记录及 auto-ID 等元数据；本文件随后写递增 schema version 对应的 `Diff:<version>`。
- job 终态：确定性输入/对象错误设置 `Cancelled`；成功设置 `Done/Public`，并把最终表快照写进 `HistoryInfo`。执行框架之后负责持久化 job、移入历史及 schema 同步。

本动作没有自己的中间 schema 状态或 reorg checkpoint；一次成功执行直接公开影子表。这与它作为内部构建目标、随后再由 cutover 动作提升的角色一致。

## 依赖与调用关系

上游调用链为：`materialized_view.go::CreateMaterializedViewShadowTable` → DDL job（`ActionCreateMaterializedViewShadow`/93）→ 普通 owner worker 持久化执行 → `table_mode.rs` → `persistent_actions.rs::step` → 本文件 `step`。

直接下游依赖包括：

- `crate::persistent_create_table::{decode_optional, create_table, save_args}`：参数兼容、物理建表及最终参数回写。
- `crate::job_worker::JobExecutionContext`：提供活动事务、PD/placement/affinity、auto-ID 等真实 worker 能力。
- `astersql_meta::TransactionMutator`：读取数据库/表、生成 schema version、写 schema diff。
- `astersql_meta_model::{Job, JobState, SchemaState}`：Go wire 兼容的 job 与表状态模型。
- `astersql_util_dbterror`：生成 `ErrInvalidDDLJob`、`ErrWrongObject` 和 `ErrInvalidDDLState`。

`pkg/ddl/Cargo.toml` 将 `astersql-meta`、`astersql-meta-model`、`astersql-util-dbterror` 声明为路径依赖，并以 `lib.rs` 为 crate 入口；本文件没有 feature gate。成功创建的表被后续 `persistent_mview_out_of_place_cutover.rs` 按 `MaterializedViewShadow` 身份校验和提升。

## 错误处理与边界

会立即把 job 标为 `Cancelled` 的错误包括：参数解码失败、影子元数据缺失、目标混入其他逻辑对象身份、源 ID 为零、schema/源表不存在、源对象不是物化视图、源对象未处于 `Public`，以及共享建表助手中显式经 `cancel` 映射的校验或外部资源错误。

错误类型尽量对齐 Go：非法 job 使用 `ErrInvalidDDLJob`，错误对象类型使用 `ErrWrongObject(schema, table, "MATERIALIZED VIEW")`，非法状态使用 `ErrInvalidDDLState("table", state)`。Rust 对 schema/源表缺失直接构造包含 ID 的 1049/1146 文本；语义与 Go `getTableInfo` 的取消规则一致，但展示文本不依赖名称查找。

并非所有 `?` 路径都会在本函数内调用 `cancel`。例如底层事务错误、schema version/diff 写入失败、`save_args` 序列化失败，以及共享助手末端未映射的错误可能保持 `Running`，交给 `table_mode` 的错误计数和重试/取消策略处理。任何错误都会触发 statement stage cleanup；因此不能把返回错误误解为部分 KV 元数据已提交。

边界上，本文件不负责检查用户权限、生成影子表名、构造刷新 SQL、导入构建数据、执行 cutover、发布 create-table notifier、注册 TTL，或允许用户直接访问/删除影子表。这些职责位于上游 executor、共享资源适配器、cutover/drop 动作及访问控制路径。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。并发串行化来自外层 DDL owner、job scheduler、`InvolvingSchemaInfo` 和持久化 job 行；`CreateMaterializedViewShadowTable` 会把目标表作为涉及对象，并在能解析源物化视图时以 shared 模式加入源对象。

KV 生命周期由 worker 管理：动作运行在已开始的持久化 DDL 会话中，`table_mode.rs` 使用 statement stage 保证动作失败时清理本次写入，`transit_persisted_job_step` 在提交前后检查 owner lease；失去 owner、会话失败或提交失败会回滚。`with_transaction` 回调只是借用该活动事务，不代表每次调用独立提交。

共享建表助手可能配置 TiFlash replica、placement bundle、affinity group 并 rebase auto-ID，这些资源动作通过 `JobExecutionContext` 完成。它们的失败处理和补偿能力属于对应适配器/共享助手；本文件本身没有额外补偿循环。影子表成功后一直作为受保护表存在，直到 cutover 接管或内部 drop-shadow 清理；Go 测试验证普通 SQL 不能读写或直接删除该对象。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/ddl/create_table.go::(*worker).onCreateMaterializedViewShadow`。两者都：解码 `CreateTableArgs`；拒绝缺失 shadow 信息、混合对象身份和零源 ID；校验源物化视图存在、类型正确且为 `Public`；复用普通物理建表逻辑；更新 schema version；最后以 `Done/Public` 完成 job。

Rust 将 Go 的 `getTableInfo` 展开为 `TransactionMutator::{get_database,get_table}`，将 `createTable` 拆分复用为 `persistent_create_table::create_table`，并显式调用 `set_create_table_schema_diff` 与 `save_args`。Go 的 `updateSchemaVersion` 在统一 schema-diff 路径中对该 action 使用 create-table diff；Rust在本文件内直接写同类 diff。Go executor 的 job 构造、查询字符串伪装和建表后处理仍位于 Go `materialized_view.go`，不属于本 Rust 文件。

现有 Go 测试 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go` 多次直接创建 shadow，并覆盖成功 cutover、cutover 失败保持旧元数据、外部 affinity 生命周期以及“用户不可读写/直接删除，只能内部清理”。`pkg/ddl/ddl_test.go::TestGetJobCheckIntervalForCreateMaterializedView` 还确认 shadow action 使用 fast DDL 轮询策略。仓库中未找到直接调用本 Rust `step` 的独立 Rust 测试，因此异常分支目前主要由 Go 实现和共享建表测试间接约束。

## 扩展指南

- 新增 shadow 身份约束时，应同时修改本文件 `step` 的前置验证与 Go `onCreateMaterializedViewShadow`，并在独立 Rust 测试文件中覆盖缺失元数据、混合对象类型、零/失效源 ID、错误源类型和非 `Public` 状态；不要把测试嵌入生产 `.rs`。
- 改变建表行为时优先评估 `persistent_create_table::create_table` 是否是正确公共接入点。若只适用于 shadow，应在本文件调用前后局部实现，避免改变普通 CREATE TABLE、批量建表和其他复用者。
- 修改 schema diff 或 job 参数时必须兼容 Job V1/V2，保持 `save_args` 的数组/对象布局，并核对 Go `updateSchemaVersion`/`SetSchemaDiffForCreateTable`。
- 引入多阶段状态或可重组工作会改变当前“一步 Public、无 checkpoint”的模型，需要同步设计取消/回滚、owner failover、schema sync 和持久化恢复，不能只在本函数增加分支。
- 增加外部资源操作时必须明确其事务边界和补偿方式；statement rollback 只保证活动 KV stage，不自动撤销已经成功的 PD/affinity 等外部副作用。
- 性能上应保持源对象校验为按 ID 点查，避免在此引入全 schema 扫描；兼容性上必须维持 action 93、错误类别、历史表快照和 create-table schema diff 的 Go wire 语义。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库索引可用（目标文件包含 10 个索引符号）；`query persistent_create_materialized_view_shadow --json` 确认 `cancel`、`invalid`、`step` 及导入；`query createMaterializedView --json` 定位 Go handler、executor 和相关测试。索引未返回本地函数的 callers/callees，因此调用边用下列精确源码搜索补齐。
- 目标与接线：`pkg/ddl/persistent_create_materialized_view_shadow.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/table_mode.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/lib.rs`。
- 共享实现与模型：`pkg/ddl/persistent_create_table.rs`、`pkg/meta/reader.rs::set_create_table_schema_diff`、`pkg/meta/model/job.rs::finish_table_job`。
- crate 边界：`pkg/ddl/Cargo.toml`。
- Go 对照：`pkg/ddl/create_table.go::onCreateMaterializedViewShadow`、`pkg/ddl/materialized_view.go::CreateMaterializedViewShadowTable`、`pkg/ddl/schema_version.go`、`pkg/ddl/rollingback.go`。
- 测试证据：`pkg/ddl/tests/materializedview/materialized_view_basic_test.go`（out-of-place cutover、失败原子性、affinity、shadow 保护与内部清理），以及 `pkg/ddl/ddl_test.go::TestGetJobCheckIntervalForCreateMaterializedView`。未发现本文件同名或直接调用 `step` 的独立 Rust 测试。
- 结构校验使用任务指定命令，要求目标文件存在且恰好包含本文 11 个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
