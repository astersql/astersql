# `pkg/ddl/persistent_alter_materialized_view_attributes.rs`

## 文件定位

事务、owner 调度和后续 schema 同步由外层普通 DDL worker 负责，本文件只实现该 job 的动作步骤。

## 核心职责

- 从 `Job` 解码 `AlterMaterializedViewAttributesArgs`，兼容 V1 旧格式中缺少 `AlertRefreshFailed` 的情况；解码实现位于 `pkg/meta/model/job_args.rs::GetAlterMaterializedViewAttributesArgs`。
- 在 worker 提供的事务中查找并校验目标表：数据库和表必须存在，job 中的非空表名必须匹配，表状态必须为 `SchemaState::Public`，且 `TableInfo.MaterializedView` 必须存在。
- 对可回滚的 multi-schema 子 job 只调用 `Job::mark_non_revertible`，本次不写表元数据；后续步骤再真正修改属性。这保持了 Go 实现的阶段边界。
- 原子替换 `AlertWarningSec`、`AlertOverdueSec` 和 `AlertRefreshFailed`，生成 schema version/schema diff（除非 multi-schema 指定 `skip_version`），并写回 `TableInfo`。
- 发布包含新旧完整 `TableInfo` 的 `NewAlterMaterializedViewAttributesEvent`，再以 `Done/Public` 完成 job。

本文件不负责解析或验证 SQL 属性字符串。诸如未知 key、yes/no 值以及 warning/overdue 大小关系的校验发生在创建 job 之前，相关 Go 入口和回归用例见 `pkg/ddl/materialized_view.go` 与 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go::TestAlterMaterializedViewAttributesUpdatesAlertThresholds`。

## 主要符号

- `pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：本文件唯一函数和公开动作入口。成功返回本步产生的 schema version；仅完成 multi-schema 的“转为不可回滚”阶段或 `skip_version` 写入时可返回 `0`。
- `JobExecutionContext`（`pkg/ddl/job_worker.rs`）：为动作提供同一 worker 会话上的 KV 事务和 SQL 查询能力。本函数使用 `with_transaction` 修改元数据，通知辅助函数使用同一 context 执行通知表写入。
- `GetAlterMaterializedViewAttributesArgs`（`pkg/meta/model/job_args.rs`）：读取三个属性。V1 三字段解码失败时回退为两字段，并把 `AlertRefreshFailed` 置为 `false`。
- `persistent_actions::public_table`：读取目标数据库/表并执行存在性、名称和 `Public` 状态检查；失败时把 job 置为 `Cancelled`。
- `persistent_actions::update_version_and_table`：按 `multi_schema_info.skip_version` 决定是否生成版本和 schema diff，然后持久化表元数据。
- `persistent_actions::async_notify_event`：为非系统库在 `mysql.tidb_ddl_notifier` 路径写入 schema change event；传入 `-1` 时，multi-schema job 使用其 `seq` 作为 sub-job ID。

本文件没有模块级常量、类型、trait、`impl` 或条件编译项。

## 执行流程

1. `persistent_actions::step` 识别 `job.tp == ACTION_ALTER_MATERIALIZED_VIEW_ATTRIBUTES`，调用本文件的 `step`。
2. `step` 解码 job 参数。失败时立即设置 `job.state = Cancelled`，将底层错误字符串向上传播，且不进入元数据事务。
3. 初始化 `output = None` 和 `version = 0`，通过 `context.with_transaction` 进入 worker 的当前事务。
4. 构造 `TransactionMutator`，调用 `public_table` 按 `schema_id/table_id` 获取 Public 表，并验证 job 表名。
5. 若 `MaterializedView` 为空，则取消 job 并返回 `ErrWrongObject(..., "MATERIALIZED VIEW")`。
6. 若当前 multi-schema job 仍可回滚，只将其标记为不可回滚并返回；此轮保持 `output = None`，不更新 schema version、不发通知、不调用 `finish_table_job`。
7. 克隆修改前的 `TableInfo`，随后只覆盖物化视图中的三个告警字段，其他字段保持不变。
8. `update_version_and_table` 生成 schema version 和 diff（`skip_version` 时版本为 0），并把新表信息写入元数据；新旧表信息保存到 `output`。
9. 事务回调成功后，构造带新旧表快照的通知事件。通知成功后调用 `finish_table_job(JobState::Done, SchemaState::Public, version, table)`。
10. 返回 `version`。任一 `?` 错误都直接上抛，由外层持久化 worker 决定事务提交、回滚、重试和 job 错误持久化。

## 数据与状态

输入状态来自 `Job`：`schema_id`、`table_id`、名称、版本化 `raw_args`/已填充参数、`multi_schema_info` 以及当前 job state。持久化目标是 `TableInfo.MaterializedView` 内的三个字段：告警阈值 `AlertWarningSec`、过期阈值 `AlertOverdueSec` 和刷新失败告警开关 `AlertRefreshFailed`。

正常写入时，`TableInfo.State` 始终要求并保持 `Public`；不存在 delete-only/write-only/reorg 等 schema state 迁移。`update_version_and_table` 同时写表元数据和 schema diff，之后 `finish_table_job` 记录最后 schema version 与完成表快照。multi-schema 可回滚阶段是一个显式两步边界：第一步只翻转 `revertible`，第二次调用才写属性。

事件保存修改后的表和修改前的克隆，因此订阅者可以比较完整的新旧元数据。`pkg/session/runtime/normal_ddl_test.rs::normal_ddl_plan_notifier_persistent_worker_publishes_full_event_once` 验证事件类型为 91、旧快照准确、新快照与实际元数据一致，且不改动已有用户行或无关物化视图字段。

## 依赖与调用关系

上游直接调用边为 `pkg/ddl/persistent_actions.rs::step -> pkg/ddl/persistent_alter_materialized_view_attributes.rs::step`；模块声明位于 `pkg/ddl/lib.rs`。更外层由普通持久化 DDL worker 驱动 job，并在 owner/lease、提交、schema barrier 和历史迁移流程中调用统一的 `persistent_actions::step`。

主要下游依赖如下：

- `astersql-meta-model`：`Job`、`JobState`、`SchemaState`、参数解码、`TableInfo` 与 `MaterializedViewInfo`。
- `astersql-meta`：`TransactionMutator` 读取数据库/表、生成 schema version、记录 diff 并更新表。
- `astersql-util-dbterror`：生成对象类型不匹配的 `ErrWrongObject`。
- `astersql-ddl-notifier`：创建并持久化属性变更事件。
- crate 内 `job_worker` 与 `persistent_actions`：提供执行上下文、公共校验、版本写入和通知桥接。

这些依赖均由 `pkg/ddl/Cargo.toml` 声明；本文件没有 feature gate。Cargo 中 `package.metadata.porting.go-package = "pkg/ddl"` 也确认其 Go 对照边界。

## 错误处理与边界

- 参数不能解码：job 置为 `Cancelled`，返回解码错误；V1 两字段旧格式不是错误，第三字段默认为 `false`。
- schema 不存在、表不存在或 job 表名过期：`public_table` 取消 job并返回相应 1049/1146 错误。
- 表不是 `Public`：取消 job并返回 DDL 8210 错误。
- 表存在但不是物化视图：取消 job并返回 `ErrWrongObject`（1347）。
- 元数据版本/diff/表写入失败：错误经 `?` 原样向上传播；实际事务不应提交。
- 通知写入失败（包括重复键）：不调用 `finish_table_job`，错误交给外层事务回滚。`normal_ddl_plan_notifier_duplicate_commit_error_rolls_back_metadata` 验证延迟到 commit 的重复键错误会同时回滚 schema version 和表属性。
- 并发 owner 写冲突：本函数不吞掉冲突；`normal_ddl_plan_notifier_metadata_conflict_rolls_back_then_owner_retries` 验证首次事务整体回滚，重试从竞争者提交后的表快照继续并只产生一条通知。
- 系统库：`async_notify_event` 明确跳过通知，但属性修改和 job 完成仍可成功；Rust 边界测试覆盖 `schema_name = "mysql"`。

本函数不重复做前端属性语义校验，也没有补偿性 delete-range 或回填逻辑。调用者必须传入动作 91 的 job；动作类型匹配由分派器保证。

## 并发与资源生命周期

函数本身不创建线程、异步任务、锁或通道。其资源边界是 `JobExecutionContext` 所属的 worker 会话：元数据修改发生在 `with_transaction` 回调中；回调结束后借用释放，随后通知通过相同 context 的 SQL 路径写入。外层 session 决定最终 commit/rollback，使表元数据、schema version/diff、通知记录和 job 状态能够作为同一持久化步骤回滚。

旧表克隆只存活到事件构造结束，新表值在事件构造时克隆一次，并最终通过 `Arc<TableInfo>` 交给 `finish_table_job`。没有长期缓存或后台资源。owner failover、lease 检查、schema barrier 与历史 job 迁移属于外层 scheduler/executor 生命周期；本文件依靠幂等的事务回滚和通知唯一键处理重试，而不自行协调并发。

## 扩展指南

- 新增持久化告警属性时，应同步修改 `AlterMaterializedViewAttributesArgs`、`MaterializedViewInfo`、创建 job 的前端、Go/Rust 参数兼容解码、本文件的字段赋值以及 notifier 序列化；必须决定旧 V1 job 缺字段时的默认值。
- 若新增字段需要校验，优先保持 SQL/job 创建阶段的用户输入校验，同时在持久化执行端补足恢复旧 job 所必需的不变量检查；不要只依赖前端，因为 owner failover 会重放已持久化参数。
- 若行为需要 schema state 迁移或数据扫描，本文件当前的元数据快速路径不再足够，应接入完整 job 状态机和独立 reorg/checkpoint 设施，而不是在此事务回调中执行长任务。
- 保持 `old` 在首次修改前克隆，且事件发布必须发生在 job 完成之前；否则订阅者会丢失准确差异或看到已完成但没有事件的状态。
- 测试逻辑应继续放在独立测试文件，不嵌入本生产文件。至少同步 `pkg/session/runtime/normal_ddl_test.rs` 的持久化 worker 回归、`pkg/meta/model/go_merge_15_test.rs` 的参数兼容用例，以及 Go 对照测试 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go`/`pkg/meta/model/job_args_test.go`。
- 兼容性风险集中在持久化 job 编码、事件 JSON 和旧版本默认值；正确性风险集中在事务原子性、multi-schema 两阶段和系统库通知跳过；此路径只改小型元数据，没有按数据量增长的扫描性能成本。
