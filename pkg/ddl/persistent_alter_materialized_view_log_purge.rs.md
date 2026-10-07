# `pkg/ddl/persistent_alter_materialized_view_log_purge.rs`

## 文件定位

本文件属于 `astersql-ddl` crate，是普通 DDL worker 的持久化 action 处理器之一。`pkg/ddl/lib.rs` 将其公开为模块；`pkg/ddl/persistent_actions.rs::step` 在 job 类型为 `ACTION_ALTER_MATERIALIZED_VIEW_LOG_PURGE` 时调用本文件唯一的公开入口 `step`。它对应 Go 的 `pkg/ddl/mview_worker.go::onAlterMaterializedViewLogPurge`，处理的不是 purge 数据任务本身，而是修改 materialized view log（MLog）的 purge 配置元数据。

从 DDL 生命周期看，这是 owner worker 执行持久化 job 的一个元数据步骤。它不创建 reorg/backfill 任务，不扫描用户数据，也不改变表的 schema state；成功时表保持 `SchemaState::Public`。提交 SQL 后另外更新 `mysql.tidb_mlog_purge_info.NEXT_PURGE_UNIX_SECONDS` 的逻辑位于 Go 前端 `pkg/ddl/materialized_view.go::alterMaterializedViewLogPurge`，不属于本文件职责。

## 核心职责

`step` 完成四件事：解析 job 中的 `AlterMaterializedViewLogPurgeArgs`；在 DDL 事务内取得并校验 public 表；修改 `TableInfo.MaterializedViewLog` 中的 purge 方法、起始表达式、后续表达式及有条件更新的 SQL mode；持久化表和 schema diff 后发布 schema-change event，并把 job 完成在 `Done/Public`。

文件刻意保留 Go 处理器的阶段划分。若是仍可回滚的 multi-schema job，本轮只调用 `job.mark_non_revertible()`，不写表、不发通知、不结束 job；下一轮才真正修改元数据。这是不可回滚边界，而不是“无事可做”的成功捷径。

## 主要符号

- `pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：唯一公开 API。返回本轮生成的 schema version；参数解析失败、对象不合法、元数据读写或通知失败时返回字符串错误。
- `args`：由 `astersql_meta_model::group_2::GetAlterMaterializedViewLogPurgeArgs(job)` 解码，提供 `PurgeMethod`、`PurgeStartWith`、`PurgeNext`、`PurgeScheduleSQLMode` 和 `UpdatePurgeSchedule`。
- `output: Option<(TableInfo, TableInfo)>`：把事务内形成的新表快照和修改前快照带到事务闭包之后。`None` 表示本轮只越过 multi-schema 不可回滚边界，不能发布事件或完成 job。
- `version`：初始为 `0`。实际写表时由 `persistent_actions::update_version_and_table` 设置；若 multi-schema 的 `skip_version` 为真，该辅助函数也会合法返回 `0`。

本文件没有常量、自定义类型、trait、`impl` 或条件编译项。

## 执行流程

1. `step` 解码 job 参数。失败时先把 `job.state` 设为 `Cancelled`，再传播解码错误。
2. 通过 `JobExecutionContext::with_transaction` 进入 worker 的 DDL 事务，并用底层 transaction 构造 `astersql_meta::TransactionMutator`。
3. `persistent_actions::public_table` 按 `schema_id/table_id` 读取表，并检查数据库存在、表存在、job 表名匹配且表状态为 `Public`；不满足时取消 job。
4. 若目标表没有 `MaterializedViewLog`，把 job 取消并返回 `ErrWrongObject(..., "MATERIALIZED VIEW LOG")`。
5. 若 `job.multi_schema_info.revertible` 为真，只调用 `mark_non_revertible` 并结束本轮事务；此时 `output` 保持 `None`、`version` 保持 `0`。
6. 否则先 clone 出 `old`，再更新 MLog 的 `PurgeMethod`、`PurgeStartWith` 和 `PurgeNext`。只有 `UpdatePurgeSchedule` 为真时才用参数中的值覆盖 `PurgeScheduleSQLMode`。
7. `update_version_and_table` 根据 `skip_version` 决定是否生成 schema version 和 table schema diff，并把新 `TableInfo` 写回 metadata。
8. 事务闭包成功且 `output` 为 `Some` 时，构造 `NewAlterMaterializedViewLogPurgeEvent(new, old)`，通过 `async_notify_event` 发布；`-1` 会被解析为普通 job 的 `-1` 或 multi-schema 的序号。
9. 通知成功后调用 `job.finish_table_job(JobState::Done, SchemaState::Public, version, Arc<table>)`，最后返回 version。

## 数据与状态

持久数据是目标 MLog 表的 `TableInfo.MaterializedViewLog`：三个 purge 字段总是按 job 参数覆盖；`PurgeScheduleSQLMode` 仅在调度表达式被更新时覆盖。Go 测试 `pkg/ddl/tests/materializedviewlog/materialized_view_basic_test.go::TestAlterMaterializedViewLogPurgeScheduleUTC` 证明这一条件很重要：单独执行 `PURGE` 清空表达式时保留原 SQL mode，而提供新的 `PURGE NEXT` 时记录当前 session SQL mode。

正常写入产生表级 schema diff 和新的表元数据；`multi_schema_info.skip_version` 时不生成版本/diff但仍更新表。job 状态存在三条关键路径：参数或对象错误进入 `Cancelled`；multi-schema 第一次执行由 `revertible` 变为不可回滚但尚未完成；实际更新成功后进入 `Done/Public`。事件同时携带新旧完整表快照，供下游区分 purge 元数据变化。

## 依赖与调用关系

上游链路为 `pkg/ddl/materialized_view.go::alterMaterializedViewLogPurge` 创建 `ActionAlterMaterializedViewLogPurge` job，owner worker 的持久化执行器最终经 `pkg/ddl/table_mode.rs` 调用 `persistent_actions::step`，再分发到本文件 `step`。RustCodeGraph 对本文件入口给出的直接调用者是 `pkg/ddl/persistent_actions.rs::step`。

主要下游依赖如下：

- `astersql-meta-model`：job、job/schema state、参数解码和 `TableInfo` 数据模型。
- `JobExecutionContext::with_transaction` 与 `astersql-meta::TransactionMutator`：提供 worker 事务和 metadata 读写。
- `persistent_actions::{public_table, update_version_and_table}`：集中实现 public 表校验、schema version/diff 及表更新。
- `astersql-util-dbterror::ErrWrongObject`：生成与 Go 相同类别的错误。
- `astersql-ddl-notifier::NewAlterMaterializedViewLogPurgeEvent` 与 `persistent_actions::async_notify_event`：发布新旧表快照。后者会跳过内存/系统 schema，并把通知写入失败作为本步骤错误传播。
- `astersql-parser-mysql::const::SQLMode`：把参数中的整数 SQL mode 还原为模型字段类型。

`pkg/ddl/Cargo.toml` 证明这些均为 `astersql-ddl` 的直接 path dependency；本文件没有 feature gate，也没有平台条件分支。

## 错误处理与边界

参数解码失败和确定性的对象错误会显式取消 job。`public_table` 还覆盖未知 schema、缺失/重命名表和非 public 状态；若读写 metadata、生成版本/diff、发布通知或 transaction 本身失败，错误用 `?` 传播，由外层持久化 worker 记录错误次数并按其策略重试或转入取消流程。

`MaterializedViewLog` 在 `is_none` 检查后才 `unwrap`，因此只要没有并发修改本地 `table` 值就不会 panic。此处理器假定 job 参数已经由前端校验并规范化，不在这里重新解析 purge SQL 表达式，也不计算下一次 purge 时间。通知失败发生在 metadata 更新逻辑之后，但仍位于 worker 管理的同一阶段；调用者会清理 staged statement，不能把“表字段已在内存中修改”误判为已完成 job。

取消/回滚语义受 multi-schema 边界约束：在 `revertible` 阶段只标记不可回滚；真正写表后处理器直接完成 job，没有反向 schema-state 转换或 delete-range GC。错误不会触发 reorg 清理，因为本操作没有 reorg 状态。

## 并发与资源生命周期

本文件不创建线程、异步 task、channel、锁或后台资源。并发和 owner 排他性由外层 durable DDL worker/lease 管理；`step` 借用可变的 `context` 与 `job`，每次只处理一个 job step。

表读取、schema version/diff 和表更新位于 `with_transaction` 闭包内。新旧表快照通过所有权移出闭包后用于通知和完成 job；最终表以 `Arc<TableInfo>` 存入 history 信息，避免再次复制。没有长时间数据扫描、checkpoint 或资源回收点。MDL 注册及 schema version 同步由 `pkg/ddl/table_mode.rs` 在本步骤返回 version 后统一完成，不由本文件直接执行。

## 与 Go 版本的对应关系

直接对应 `pkg/ddl/mview_worker.go::onAlterMaterializedViewLogPurge`：两者都按相同顺序解码参数、读取并验证目标表、拒绝非 MLog 对象、处理 multi-schema 不可回滚边界、clone 旧表、更新四个字段、持久化版本与表、发布新旧表事件并 `FinishTableJob(Done, Public, ...)`。

Rust 把 Go 的 `jobContext/metaMut/session` 抽象为 `JobExecutionContext` 和显式 transaction，把 Go error 转成 `String`，并用 `Option`/`Arc` 表达事件与完成信息。Go 的 `updateVersionAndTableInfo(jobCtx, job, tblInfo, true)` 中 `shouldUpdateVer=true` 的可观察语义由 Rust `update_version_and_table` 保留，包括 multi-schema `skip_version`。Go 前端在 DDL job 完成后 best-effort 更新运行时 schedule info；该后续动作未迁入此 Rust 文件，因此不能把本处理器描述为已经独立完成 `NEXT_PURGE_UNIX_SECONDS` 更新。

## 扩展指南

新增 purge 元数据字段时，应同步修改参数模型、`step` 中旧快照之后的字段赋值、Go `onAlterMaterializedViewLogPurge`，以及事件序列化兼容性；若字段影响 schedule 求值，还要检查 `materialized_view.go` 与 `mview_schedule_expr.go` 的 job 后更新路径。不要在这里引入数据 purge、表扫描或独立提交事务；那会改变当前 metadata-only、可由 owner 重试的边界。

测试应继续放在独立文件中：用户可见语义优先扩展 `pkg/ddl/tests/materializedviewlog/materialized_view_basic_test.go` 或 `materialized_view_alter_test.go`；事件新旧快照扩展 `pkg/ddl/notifier/events_test.rs`（以及 Go 对照测试）；若要覆盖 Rust handler 的取消、multi-schema 两阶段或事务失败路径，应新增/扩展同目录独立的 `*_test.rs` 模块并在 `pkg/ddl/lib.rs` 中以 `#[cfg(test)] mod ...` 接入，不能把测试内嵌进本生产文件。

兼容性风险包括：无条件覆盖 `PurgeScheduleSQLMode` 会改变旧 schedule 的解释环境；调整 `mark_non_revertible` 时机会破坏 multi-schema 回滚协议；遗漏旧表快照会影响 notifier/CDC 消费者；绕开 `update_version_and_table` 会破坏 schema diff 和集群同步。此路径只做少量 metadata clone/write，通常无数据规模相关性能成本，但增加完整表 clone 或同步外部调用会放大 DDL transaction 延迟。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`node persistent_alter_materialized_view_log_purge.rs::step` 定位入口；`callers` 确认直接调用者为 `persistent_actions.rs::step`。其通用 callee 解析出现同名误配，因此下游关系以精确源码和 `rg` 复核。
- Rust 源码：`pkg/ddl/persistent_alter_materialized_view_log_purge.rs`；分发和辅助函数 `pkg/ddl/persistent_actions.rs`；外层持久化执行与 schema sync 接线 `pkg/ddl/table_mode.rs`；trait 契约 `pkg/ddl/job_worker.rs`；模块声明 `pkg/ddl/lib.rs`；事件构造测试 `pkg/ddl/notifier/events_test.rs`。
- crate 边界：`pkg/ddl/Cargo.toml` 的 package、lib path、直接 dependencies 与 dev-dependencies。
- Go 对照：`pkg/ddl/mview_worker.go::onAlterMaterializedViewLogPurge`；job 创建和 job 后 schedule 更新 `pkg/ddl/materialized_view.go::alterMaterializedViewLogPurge`；worker action 分发 `pkg/ddl/job_worker.go`；事件测试 `pkg/ddl/notifier/events_test.go`。
- 行为测试：`pkg/ddl/tests/materializedviewlog/materialized_view_basic_test.go::TestAlterMaterializedViewLogPurgeScheduleUTC` 覆盖 SQL mode 的条件更新与时间语义；`pkg/ddl/tests/materializedviewlog/materialized_view_alter_test.go::TestAlterMaterializedViewLogPurgeUpdatesNextUnixSecondsWithMLogAlterPrivilege` 覆盖权限、purge 表达式持久化及 job 后 schedule 行更新。未发现直接调用本 Rust `step` 的独立同名测试。
- 人工复核结论：本文区分了 metadata job step 与 job 后 runtime schedule 更新，说明了 job 状态、schema version/diff、通知、事务、不可回滚边界以及安全扩展位置；未把 Go 集成测试等同于 Rust handler 的直接单元覆盖。
