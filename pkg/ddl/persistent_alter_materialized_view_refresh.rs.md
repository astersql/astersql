# `pkg/ddl/persistent_alter_materialized_view_refresh.rs`

## 文件定位

本文件属于 `astersql-ddl` crate，是持久化普通 DDL worker 对 `ACTION_ALTER_MATERIALIZED_VIEW_REFRESH`（动作值 89）的单步执行器。模块由 `pkg/ddl/lib.rs` 公开声明，并由 `pkg/ddl/persistent_actions.rs::step` 在识别到该动作后调用 `persistent_alter_materialized_view_refresh::step`。上游 SQL 构造逻辑位于 Go 的 `pkg/ddl/materialized_view.go::(*executor).alterMaterializedViewRefresh`；Rust 侧当前负责消费已经持久化的 `Job`，不负责解析 SQL、校验刷新表达式类型或计算运行时的下一次刷新时间。

从 DDL 生命周期看，这是一个 job-based、metadata-only 的快速路径：它不进行 reorg/backfill，也不让表经历 `delete only`、`write only` 等 schema state；目标表在读取和完成时都保持 `SchemaState::Public`。持久队列的 owner/lease、提交、回滚和提交后的 schema 同步由 `pkg/ddl/job_scheduler.rs::schedule_persisted` 与 `pkg/ddl/job_worker.rs::transit_persisted_job_step` 统一管理，而不是由本文件自行实现。

## 核心职责

唯一公开函数 `step` 完成以下职责：解码刷新配置参数；在 DDL 元数据事务中取得并校验 public 状态的目标表；确认对象确实是物化视图；处理 multi-schema job 从可回滚到不可回滚的阶段切换；更新 `MaterializedViewInfo` 中的刷新方法、调度表达式及按条件更新的 SQL mode；生成 schema version/schema diff 并持久化表信息；发布包含修改前后表快照的 schema-change 事件；最后将 job 标记为 `Done/Public` 并写入 history table info。

本文件只更新结构化元数据。`mysql.tidb_mview_refresh_info.NEXT_REFRESH_UNIX_SECONDS` 的派生和 best-effort 更新，以及禁用计划后告警行的处理，位于 Go 的 `pkg/ddl/materialized_view.go::alterMaterializedViewRefresh` 后半段，明确与 DDL 元数据事务解耦，不能误认为由 Rust `step` 完成。

## 主要符号

- `pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：文件的唯一入口。成功时返回本步生成的 schema version；multi-schema 的首次不可回滚转换返回 `0`，并把实际修改留给后续一步。
- `JobExecutionContext`（`pkg/ddl/job_worker.rs`）：向动作实现提供 `with_transaction` 和 `query` 等执行能力。本函数借它开启元数据回调，并在回调外发布 notifier 事件。
- `GetAlterMaterializedViewRefreshArgs`（`pkg/meta/model/job_args.rs`）：从 `Job` 解码 `AlterMaterializedViewRefreshArgs`。参数包含 `RefreshMethod`、`RefreshStartWith`、`RefreshNext`、`RefreshScheduleSQLMode` 和 `UpdateRefreshSchedule`；Rust/Go 编解码一致性由 `pkg/meta/model/go_merge_15_test.rs` 与 `pkg/meta/model/job_args_test.go` 覆盖。
- `persistent_actions::public_table`：验证 schema 存在、table 存在、可选表名匹配且表状态为 `Public`；失败路径会把 job 置为 `Cancelled`。
- `persistent_actions::update_version_and_table`：除 multi-schema `skip_version` 外生成 schema version、写 schema diff，随后调用 meta mutator 更新表信息。
- `persistent_actions::async_notify_event`：将 notifier 事件写入当前 DDL 执行上下文；系统库/内存库会跳过通知，`-1` 子任务号会按 multi-schema 序号归一化。
- `Job::mark_non_revertible` 与 `Job::finish_table_job`：前者关闭 multi-schema 回滚窗口；后者设置 `Done/Public` 并把带版本的最终 `TableInfo` 放入 `binlog_info`。

本文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项。

## 执行流程

1. `persistent_actions::step` 根据 `job.tp == ACTION_ALTER_MATERIALIZED_VIEW_REFRESH` 分派到本函数。动作此前由 SQL 层构造成持久 DDL job，并由 owner 侧 `schedule_persisted` 从 `mysql.tidb_ddl_job` 读取。
2. `GetAlterMaterializedViewRefreshArgs(job)` 解码参数。解码失败立即将 `job.state` 设为 `Cancelled`，返回字符串化错误，不打开元数据修改事务。
3. `context.with_transaction` 创建 `TransactionMutator`，通过 `public_table` 取得当前表。schema/table 缺失、名称不匹配或非 public 均直接失败并取消 job。
4. 检查 `table.MaterializedView`。普通表或其他对象没有该字段时，返回 `ErrWrongObject(schema_name, table_name, "MATERIALIZED VIEW")` 并取消 job。
5. 若这是 multi-schema job 且仍处于 `revertible` 阶段，只调用 `mark_non_revertible` 后结束本次事务回调；此时不克隆表、不改字段、不生成版本、不发事件，也不完成 job。调度器下一次执行才进入实际修改。
6. 克隆当前 `TableInfo` 为 `old`，取得 `MaterializedView` 的可变引用，并无条件覆盖刷新方法、`START WITH` 与 `NEXT` 表达式；仅当 `UpdateRefreshSchedule` 为真时覆盖 `RefreshScheduleSQLMode`。这一条件保证只清除/保留调度表达式而未显式提交新 schedule 时，不会错误改写已有调度 SQL mode。
7. `update_version_and_table` 生成版本和 schema diff（或按 `skip_version` 返回 0），再把修改后的整张表信息写回 meta。事务回调仅把新旧表快照留在局部 `output` 中。
8. 事务回调成功且确实修改了表时，使用 `NewAlterMaterializedViewRefreshEvent(new, old)` 发布 schema-change 事件。之后 `finish_table_job(Done, Public, version, table)` 记录完成状态与 history 信息。
9. 返回 schema version。外层 `transit_persisted_job_step` 还会检查 owner lease、更新持久 job 行并提交；`schedule_persisted` 随后调用 `wait_synced`，因此本函数返回版本不等于完整集群同步已经完成。

## 数据与状态

主要输入状态是持久化 `Job` 及其参数。`AlterMaterializedViewRefreshArgs` 的三个字符串字段表示规范化后的刷新方式和调度表达式；`RefreshScheduleSQLMode` 是表达式以后重新解释时所需的 SQL mode；`UpdateRefreshSchedule` 是区分“此次语句显式更新 schedule”与“保留原 schedule 解释环境”的关键位。

持久修改集中在 `TableInfo.MaterializedView`：`RefreshMethod`、`RefreshStartWith`、`RefreshNext` 总是按参数覆盖，`RefreshScheduleSQLMode` 条件覆盖。函数在更新前完整克隆 `TableInfo`，使 notifier 同时获得 old/new 快照；最终的新表则通过 `Arc<TableInfo>` 写入 job history。函数本身不维护全局缓存，infoschema 可见性依赖生成的 schema version、diff 以及外层同步屏障。

job 状态存在三类显式变化：参数或对象校验错误进入 `Cancelled`；multi-schema 的第一阶段只把 `multi_schema_info.revertible` 从 `true` 改为 `false`；实际更新成功后进入 `Done`，schema state 固定为 `Public`。这里没有中间 schema state、reorg checkpoint 或 delete-range 数据。

## 依赖与调用关系

上游主链为：`pkg/ddl/materialized_view.go::alterMaterializedViewRefresh` 构造 `ActionAlterMaterializedViewRefresh` job → 持久 DDL 队列/owner scheduler → `pkg/ddl/job_worker.rs::transit_persisted_job_step` → durable executor → `pkg/ddl/persistent_actions.rs::step` → 本文件 `step`。`pkg/ddl/Cargo.toml` 将本文件归入 `astersql-ddl`，其直接使用的 crate 依赖包括 `astersql-meta-model`、`astersql-meta`、`astersql-parser-mysql`、`astersql-util-dbterror` 和 `astersql-ddl-notifier`。

下游关系包括：`TransactionMutator` 读取和写入 schema/table 元数据；`public_table` 提供对象与状态保护；`update_version_and_table` 生成全局 schema version/diff；`NewAlterMaterializedViewRefreshEvent` 构建携带 old/new `TableInfo` 的动作 89 事件；`async_notify_event` 在 DDL 执行上下文发布事件；`finish_table_job` 写 job 完成状态和 history 信息。

Go 入口还有一条不经过本文件的后续支线：DDL job 成功后派生下一刷新时间，并 best-effort 更新 `mysql.tidb_mview_refresh_info`，必要时清理 `mysql.tidb_mview_refresh_alert`。因此扩展 Rust worker 时应保持元数据提交与运行时 schedule-info 更新的边界，除非整体执行架构也同时迁移并验证。

## 错误处理与边界

- 参数解码失败：将 job 取消并返回解码错误；`pkg/meta/model/job_args_test.go::TestGetAlterMaterializedViewRefreshArgs` 和 Rust `go_merge_15_test.rs` 验证 V1/V2/跨语言参数形状。
- schema/table 不存在、名称不匹配、表非 public：由 `public_table` 返回相应错误并取消 job，且不会产生表更新。
- 目标不是物化视图：显式返回 `ErrWrongObject`，期望对象名为 `MATERIALIZED VIEW`；在 `unwrap` 前已有 `is_none` 保护。
- multi-schema 可回滚阶段：不是错误，而是阶段屏障；本次返回 0 且不产生通知。若删除这个早退，会破坏组合 DDL 的回滚语义。
- meta 版本、diff 或表写入失败：`?` 向上传播；外层持久事务失败时回滚，job 行不会以部分更新提交。
- notifier 失败：错误向上传播，`finish_table_job` 不执行。事件发布通过当前执行上下文完成，外层事务/持久 worker 负责保证失败不留下可见的半完成 job。
- 本文件不校验 `START WITH`/`NEXT` 的表达式类型、时区或计算结果；这些前置/后续边界由 SQL 层处理。Go 测试 `TestAlterMaterializedViewRefreshExprTypeValidation`、`TestAlterMaterializedViewRefreshScheduleUTC` 与 `TestAlterMaterializedViewRefreshBestEffortInfoUpdateWarning` 证明了该分层。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。目标表读取、字段更新、schema version/diff 生成及 meta 写回都在 `JobExecutionContext::with_transaction` 提供的事务作用域内完成；局部 `TransactionMutator` 和表可变引用随闭包结束释放。新旧表快照被移出事务闭包，供随后构造 notifier 事件和 job history 使用。

并发正确性依赖外层 durable worker：`transit_persisted_job_step` 在事务前后检查 owner lease，校验数据库中的 `job_meta` 仍等于调度时读取的字节，失败时回滚；提交后由 scheduler 对返回版本执行 `wait_synced`。`schedule_persisted` 还用 involving-schema 冲突集合限制同对象作业并发。因此本函数不应自行开启第二个元数据事务，也不应绕开外层 lease/commit/sync 生命周期。

运行时刷新信息表可能被正在运行的 refresh 持锁，Go 层刻意将其放在 DDL 元数据完成之后做 best-effort 更新。`TestAlterMaterializedViewRefreshBestEffortInfoUpdateWarning` 证明锁竞争不会回滚已经成功的物化视图元数据，只会保留旧的 `NEXT_REFRESH_UNIX_SECONDS` 并产生 warning；这个语义不属于本 Rust 事务的原子边界。

## 与 Go 版本的对应关系

直接对照实现是 `pkg/ddl/mview_worker.go::onAlterMaterializedViewRefresh`。Rust 与 Go 按相同顺序执行：解码参数并在失败时取消 job；读取表并校验物化视图类型；multi-schema 可回滚阶段调用 `MarkNonRevertible` 后早退；克隆旧表；覆盖 `RefreshMethod`、`RefreshStartWith`、`RefreshNext`，按 `UpdateRefreshSchedule` 覆盖 SQL mode；更新 schema version/table；发布 `NewAlterMaterializedViewRefreshEvent`；以 `Done/Public` 完成 table job。

实现形态上的差异主要来自运行框架：Go 函数直接接收 `jobContext` 与 session，并调用 `updateVersionAndTableInfo`；Rust 函数通过 trait 化的 `JobExecutionContext` 打开事务，使用 `TransactionMutator` 和 `persistent_actions` 公共帮助函数。Go 使用 `errors.Trace` 保留错误链，Rust 当前统一向上传递 `String`。语义上 Rust 的 `output: Option<(TableInfo, TableInfo)>` 明确区分 multi-schema 仅切换不可回滚标记的步骤，避免误发事件或提前完成 job。

Go SQL 入口 `pkg/ddl/materialized_view.go::alterMaterializedViewRefresh` 在 worker 完成后还更新运行时 schedule-info；这不是 `onAlterMaterializedViewRefresh` 的职责，也尚未包含在本 Rust 文件中。文档中的“已对齐”仅指 worker 元数据步骤，不表示整条 Go SQL 后处理链已由此文件迁移。

## 扩展指南

新增或改变刷新元数据字段时，优先同时检查并修改：`pkg/meta/model/job_args.rs`/`.go` 的参数结构和 V1/V2 编解码、SQL job 构造端、本文 `step`、Go `onAlterMaterializedViewRefresh`、`TableInfo.MaterializedView` 模型以及 notifier 消费者。新增字段应明确是无条件覆盖还是由独立 update flag 控制，避免旧版本 job 的默认值意外清空已持久化配置。

保持以下不变量：对象必须是 public 的物化视图；multi-schema 在真实写入前先跨过不可回滚屏障；old snapshot 必须在修改前克隆；表更新、schema version/diff 与事件持久化必须留在 durable worker 的事务/提交协议内；成功结束仍为 `Done/Public`。若新逻辑需要 reorg、外部 I/O 或第二张系统表的强原子更新，本文件的 metadata-only 模式已不够，应先设计 checkpoint、取消/回滚、owner failover 和 schema sync 行为，而不是直接塞入 `step`。

测试应放在独立文件而非源文件内。当前最接近的 Rust 覆盖是 `pkg/meta/model/go_merge_15_test.rs`（参数 round-trip）和 `pkg/ddl/notifier/events_test.rs::materialized_view_alter_event_constructors_preserve_old_and_new_tables`（事件 old/new 快照）；本文件没有同名专属 Rust 单元测试。新增 worker 分支时，建议在独立 `pkg/ddl/persistent_alter_materialized_view_refresh_test.rs` 中以 mock `JobExecutionContext` 覆盖：错误参数取消、非物化视图错误、multi-schema 两步行为、SQL mode 条件更新、meta/通知失败回滚和最终 history 信息；SQL 可见行为继续同步 Go 的 `pkg/ddl/tests/materializedview/materialized_view_alter_test.go`。

兼容性风险集中在 job 参数 wire 格式和旧 job 默认值；正确性风险集中在不可回滚边界、old/new 事件顺序及通知失败的事务语义；性能风险较低，因为本步只读写单个 `TableInfo`，但完整克隆表信息和事件 JSON 大小会随表元数据规模增长。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 11,467 个文件；`node --file pkg/ddl/persistent_alter_materialized_view_refresh.rs` 确认文件共 81 行、唯一函数为 `step`，并显示由 `persistent_actions.rs` 使用。
- RustCodeGraph 调用证据：`persistent_actions.rs::step` 在动作 89 分支调用本文件；`job_scheduler.rs::schedule_persisted` 调用 `job_worker.rs::transit_persisted_job_step` 并随后 `wait_synced`；辅助符号 `public_table`、`update_version_and_table`、`async_notify_event`、`mark_non_revertible`、`finish_table_job` 的源码与调用轨迹均已核对。
- Rust 源与 crate 证据：已读 `pkg/ddl/persistent_alter_materialized_view_refresh.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/job_scheduler.rs`、`pkg/ddl/lib.rs`、`pkg/meta/model/job_args.rs`、`pkg/meta/model/job.rs`、`pkg/ddl/notifier/events.rs` 和 `pkg/ddl/Cargo.toml`。
- Go 对照证据：已读 `pkg/ddl/mview_worker.go::onAlterMaterializedViewRefresh`、`pkg/ddl/job_worker.go` 的动作分派、`pkg/ddl/materialized_view.go::alterMaterializedViewRefresh` 及 `pkg/meta/model/job_args.go`。
- 测试证据：已核对 `pkg/meta/model/job_args_test.go::TestGetAlterMaterializedViewRefreshArgs`、`pkg/meta/model/go_merge_15_test.rs`、`pkg/ddl/notifier/events_test.rs`，以及 `pkg/ddl/tests/materializedview/materialized_view_alter_test.go` 中调度禁用、锁竞争 warning、权限、SQL mode/时区测试和 `pkg/ddl/tests/materializedview/materialized_view_basic_test.go` 的元数据断言/表达式类型测试。
- 本任务为只写文档的静态分析，按计划未运行 Cargo 或 Go 测试；结构校验命令与结果在任务交付时单独记录。
