# `pkg/ddl/persistent_create_materialized_view_log.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml`），由 `pkg/ddl/lib.rs` 公开为 `persistent_create_materialized_view_log` 模块。它是普通 DDL worker 执行 `CREATE MATERIALIZED VIEW LOG` 持久化步骤的 Rust 实现：`pkg/ddl/persistent_actions.rs::step` 在 `job.tp == 85`（`ACTION_CREATE_MATERIALIZED_VIEW_LOG`）时调用本文件唯一的公开入口 `step`。对应 Go 实现是 `pkg/ddl/mview_worker.go::onCreateMaterializedViewLog` 及其回滚函数。

这不是 SQL 解析或建 Job 的入口，而是 owner/worker 已取得持久化 `Job` 后的动作执行层。它会创建日志表元数据、在基表上记录日志表 ID、维护 `mysql.tidb_mlog_purge_info`、写 schema diff 和 notifier 事件，最后完成 Job；不执行物化视图数据刷新，也不做 reorg/backfill。

## 核心职责

- 解码并校验 `CreateMaterializedViewLogArgs.TableInfo`，要求其中存在 `MaterializedViewLog`，且 `BaseTableID` 非零（`decode`、`step`）。
- 在元数据事务中验证数据库和基表存在，且基表必须是 public 的普通、非分区、非临时表，不能是 view、sequence、物化视图、shadow 或另一张日志表（`step` 第一次 `with_transaction`）。
- 复用 `persistent_create_table::create_table` 创建日志表，再把 `base.MaterializedViewBase.MLogID` 指向新日志表（`step`）。
- 通过 `JobExecutionContext::derive_create_mlog_schedule` 计算下一次清理时间，并用带标签 `mlog-purge-info-upsert` 的 SQL 写入 `mysql.tidb_mlog_purge_info`（`step`）。
- 生成同时覆盖日志表和基表的 schema diff，发布 create-table notifier，保存更新后的 Job 参数，并以两个表快照结束多表 Job（`step`）。
- 当 Job 已进入 `Rollingback` 时，撤销基表链接、删除日志表及 auto-ID 字段、清理 purge 行并生成回滚 schema diff（`rollback`）。

DDL 预检问题的当前答案是：它是可持久化、可重试的 job-based 动作；没有中间 schema state 提交或 backfill，成功时直接完成为 `Public`；可取消的参数/对象校验把 Job 置为 `Cancelled`，purge 系统表缺失则置为 `Rollingback` 并由后续步骤清理；持久状态涉及日志表元数据、基表 `MaterializedViewBase.MLogID`、schema diff、Job 历史快照和 `mysql.tidb_mlog_purge_info`。

## 主要符号

- `pub fn step(context: &mut dyn JobExecutionContext, job: &mut Job) -> Result<i64, String>`：公开动作入口。返回本步生成的 schema version；错误使用字符串边界与 worker 协议衔接。
- `fn decode(job: &mut Job) -> Result<TableInfo, String>`：调用 `GetCreateMaterializedViewLogArgs`，取出 `TableInfo`，经 `serde_json` 往返转换成当前 Rust `TableInfo`，并验证 `MaterializedViewLog` 存在。解码失败会取消 Job。
- `fn rollback(context, job, args) -> Result<i64, String>`：幂等倾向的补偿路径。实际日志表不存在时仍可依据原始参数清理基表关系；purge 表不存在被视为已经清理。
- `fn cancel(job, error) -> String`：把 `job.state` 设为 `Cancelled` 并保留错误文本。
- `fn invalid(job, reason) -> String`：构造 `ErrInvalidDDLJob`，再经 `cancel` 取消 Job。
- `fn missing_table(job, id) -> String`：生成与 schema/table ID 关联的 1146 错误文本。
- `fn is_missing_purge_table(error: &str) -> bool`：兼容识别包含 1146 与表名的错误，以及 canonical DML 层的精确错误 `unknown DML table tidb_mlog_purge_info`。

文件没有自定义类型、trait、常量或条件编译项；主要状态都来自 `Job`、`TableInfo` 和 `JobExecutionContext`。

## 执行流程

1. `step` 先由 `decode` 恢复日志表信息。如果 Job 已是 `Rollingback`，立即进入 `rollback`，不会重复正常创建流程。
2. 正常路径读取 `MaterializedViewLog.BaseTableID`。零 ID 是不可恢复的无效参数，Job 被取消。
3. 第一个 `with_transaction` 检查数据库与基表。数据库/表缺失、对象类型错误、分区表、非 public 状态或基表已经绑定 MLog 都会停止创建；校验通过后保留基表快照。
4. `persistent_create_table::create_table(context, job, &mut log, false)` 执行通用建表检查和元数据创建，把日志表推进到 `Public`；`false` 表示不启用 FK 检查。
5. 在基表的 `MaterializedViewBase` 上建立或复用结构，并令 `MLogID = log.ID`，随后通过 `update_table` 持久化。
6. `derive_create_mlog_schedule` 返回 `(next, should_update)`。需要更新计划时使用 `INSERT ... ON DUPLICATE KEY UPDATE NEXT_PURGE_UNIX_SECONDS`；否则使用 `INSERT IGNORE`，仅保证 purge 行存在。
7. purge 写入若报告系统表不存在，`step` 把 Job 改为 `Rollingback` 并返回 `ErrInvalidDDLJob`；其他 SQL 错误原样返回，交给 worker 的重试/失败协议处理。
8. 成功路径生成 schema version，并用 `set_create_mlog_schema_diff(job, version, &[base.ID], false)` 记录新日志表和受影响基表。
9. `async_notify_event` 发布 `NewCreateTableEvent(log)`；`save_args` 把最终日志表信息写回 V1 或 V2 Job raw args；`finish_multiple_table_job` 将 Job 置为 `Done/Public`，历史信息包含 `[base, log]` 两个快照。
10. 回滚路径清除仍指向当前 Job 日志表的基表 `MLogID`；若同时没有依赖物化视图，则移除整个 `MaterializedViewBase`。存在实际日志表时删除表元数据与 `Table/TID/IID/TARID` auto-ID 字段，删除 purge 行，最后置为 `RollbackDone/None` 并写回滚 schema diff。

## 数据与状态

核心输入是 `Job` 中的 `schema_id`、`schema_name`、`table_id`、`state`、`version` 与 raw args，以及 args 内的日志表 `TableInfo`。日志表的 `MaterializedViewLog.BaseTableID` 是定位基表的主关联；基表的 `MaterializedViewBase.MLogID` 是反向关联。正常完成后，两边必须分别指向正确对象，且历史 `multiple_table_infos` 同时保存基表和日志表快照。

状态变化只有终态，没有本文件自行提交的 `DeleteOnly/WriteOnly/Reorganization` 中间状态：正常完成为 `Done/Public`；参数或对象校验失败为 `Cancelled`；缺失必需 purge 系统表时先进入 `Rollingback`；补偿完成为 `RollbackDone/None`。`persistent_create_table::create_table` 会在实际创建前把日志表从 `None` 设置到 `Public`，但本文件只在所有附属元数据完成后发布最终 schema version。

持久数据包括：数据库内的日志表记录及 auto-ID 字段、基表 `MaterializedViewBase`、`mysql.tidb_mlog_purge_info` 的 MLog 行、`Diff:<version>` schema diff、notifier 表事件以及 Job raw args/历史信息。`set_create_mlog_schema_diff` 在创建时令 diff 的主表为日志表并把基表作为 affected option；回滚时用 `table_id = 0`、`old_table_id = job.table_id` 表达删除，并仅列出实际受影响的基表 ID。

## 依赖与调用关系

上游直接调用边是 `pkg/ddl/persistent_actions.rs::step -> persistent_create_materialized_view_log::step`，由 action type 85 选择；`pkg/meta/model/job.rs` 与 `pkg/meta/model/internal/group1/lib.rs` 均将该动作定义为 `ACTION_CREATE_MATERIALIZED_VIEW_LOG = 85`。更外层由普通持久 DDL worker 提供 `JobExecutionContext`，本文件不拥有调度器或 session 生命周期。

下游关键依赖如下：

- `astersql-meta-model`：`Job`、`JobState`、`SchemaState`、`TableInfo`、Job args 解码和完成历史信息。
- `astersql-meta::TransactionMutator`：数据库/表读取、基表更新、日志表及 auto-ID 删除、schema version 与 diff 写入。
- `persistent_create_table::{create_table, save_args}`：通用建表语义及完成后参数持久化。
- `JobExecutionContext::{with_transaction, query, derive_create_mlog_schedule}`：事务访问、系统 SQL 执行与 UTC purge 计划求值。trait 默认的计划求值会报“unavailable”，实际 worker 必须提供实现。
- `persistent_actions::async_notify_event` 与 `astersql-ddl-notifier::NewCreateTableEvent`：事务型 schema-change 通知；系统库会由 notifier 辅助逻辑跳过。
- `astersql-meta-metadef::IsMemOrSysDB` 和 `astersql-util-dbterror`：对象合法性与兼容错误构造。

`pkg/ddl/Cargo.toml` 直接声明上述 `astersql-meta`、`astersql-meta-model`、`astersql-meta-metadef`、`astersql-ddl-notifier`、`astersql-kv`、`astersql-util-dbterror`、`serde`/`serde_json` 依赖；本模块没有专属 feature gate。

## 错误处理与边界

会取消 Job 的错误包括：args 解码/JSON 转换失败、缺少日志元数据、零基表 ID、数据库或基表不存在、基表对象类型不合格、分区表、不处于 `Public`、或基表已有 MLog。错误文本尽量沿用 Go/MySQL 类别：1049 数据库不存在、1146 表不存在、1050 表已存在、`ErrWrongObject`、`ErrGeneralUnsupportedDDL`、`ErrInvalidDDLState` 和 `ErrInvalidDDLJob`。

purge 表缺失是特殊边界：创建已可能写入日志表和基表关系，因此不能只取消 Job，而是显式进入 `Rollingback`；下一次执行由 `rollback` 撤销元数据。回滚删除 purge 行时相同缺失错误被忽略，因为目标状态已经满足。其他 query 或元数据错误不被吞掉，返回给外层 worker；本文件本身不决定它们最终是重试、持久化错误还是终止。

防御性边界包括：只有当基表当前 `MLogID == job.table_id` 时回滚才清零，避免删掉后来建立的其他关系；只有 `MLogID == 0` 且 `MViewIDs` 为空时才移除整个 `MaterializedViewBase`；实际日志表已不存在时不会重复 drop；schema diff 的 affected ID 由实际找到并更新的基表产生。正常路径的 `base.ok_or("base table metadata unavailable")` 是内部不变量保护，理论上只有闭包未保存已校验基表时才触发。

## 并发与资源生命周期

本文件不创建线程、锁、channel 或后台任务。并发控制、owner lease、Job 重试、事务提交和 schema 同步由外层持久 DDL worker 管理；入口只通过借用的 `&mut dyn JobExecutionContext` 顺序执行。

源码注释声明日志表、基表关系、purge 注册与 notifier 参与外层 worker 的事务语义。实现层面这些操作表现为多次 `with_transaction` 和一次或多次 `query`/notifier 调用，因此具体是否复用同一底层事务由 `JobExecutionContext` 实现保证，不能仅从本文件断言每次调用都会独立提交。安全扩展时必须保持所有调用使用同一 worker context，不能另开会话提前提交附属行，否则在 schema diff 或 notifier 失败时可能留下孤立元数据。

日志表元数据的资源生命周期从 `create_table` 创建开始，在基表反向链接、purge 行、schema diff 与 notifier 成功后随 Job 完成；异常进入回滚后，`drop_table_and_auto_ids` 负责同时删除对象和四类 allocator 字段。该动作没有 reorg checkpoint、扫描任务或 delete-range GC；回滚是元数据删除，不产生本文件自己的异步清理任务。

## 与 Go 版本的对应关系

Rust `step` 对应 `pkg/ddl/mview_worker.go::onCreateMaterializedViewLog`，Rust `rollback` 对应 `rollbackCreateMaterializedViewLog`。主要语义保持一致：解码 args、验证基表、调用通用 `createTable`、写 `MLogID`、计算并 upsert purge 时间、更新 schema version、发 notifier、以多表历史结束，以及缺失 purge 表时进入回滚。

Rust 将 Go 的若干辅助逻辑收拢到边界接口或元数据层：Go `upsertCreateMaterializedViewLogPurgeInfo`/`buildCreateMaterializedViewLogPurgeInfoUpsertSQL` 在 Rust 中由 `derive_create_mlog_schedule` 加本文件 SQL 拼接承担；Go `updateSchemaVersion` 的 MLog 特例由 `TransactionMutator::set_create_mlog_schema_diff` 表达；Go `DropTableOrView` 加 auto-ID `Del` 由 `drop_table_and_auto_ids` 合并。

可见差异需谨慎维护：Go 通过 `sqlescape.MustEscapeSQL` 参数化转义，Rust 当前把内部生成的整数 ID/时间直接格式化进 SQL；Rust 可接受的缺表错误还包括 canonical DML 层字符串。Go 测试含 failpoint 精确模拟 purge 表缺失，Rust 本文件未发现同名独立单元测试；因此不能把 Go 覆盖等同于 Rust 已执行覆盖。Rust `decode` 的 JSON 往返是模型兼容转换，而 Go 直接使用指针 args。

## 扩展指南

新增或改变创建 MLog 行为时，优先定位以下接入点：基表资格在 `step` 的第一次事务闭包；建表共同行为应进入 `persistent_create_table::create_table` 而非复制；purge 表字段/计划规则应同步修改 `JobExecutionContext::derive_create_mlog_schedule`、SQL 生成和实际 context；schema 传播字段应修改 `set_create_mlog_schema_diff`；历史参数变化应同步 `save_args` 与 args 版本兼容；新附属资源必须同时补充 `rollback` 的幂等清理。

必须保持的兼容约束包括 action type 85、V1/V2 Job raw args、MySQL 兼容错误类别、日志表与基表双向关系、创建/回滚 schema diff 形状、缺系统表触发补偿而非直接取消，以及 notifier 在最终 Job 完成前成功。性能方面当前只有少量点查和元数据写；不要在此入口加入基表扫描或长事务数据处理，若需要 backfill 应进入独立 reorg 机制。

测试应放在独立测试文件，不能内嵌到生产 `.rs`。Rust 侧最接近的新增回归位置应是 `pkg/ddl` 下独立 `*_test.rs`，通过 mock `JobExecutionContext` 覆盖：有效创建、各种基表拒绝、purge 表缺失后第二步回滚、普通 query 错误保留、已有/缺失实际日志表的幂等回滚，以及 schema diff/notifier/历史快照。Go 对照测试需同步审阅 `pkg/ddl/ddl_test.go::TestBuildCreateMaterializedViewLogPurgeInfoUpsertSQL`、`pkg/ddl/tests/materializedviewlog/materialized_view_create_test.go::TestCreateMaterializedViewLogPurgeInfoFailureRollback`，以及 `pkg/ddl/mview_worker_test.go` 中关联元数据清理场景。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/ddl/persistent_create_materialized_view_log.rs` 读取完整 240 行源码；`query` 确认 `step`、`rollback`、`decode` 的签名。通用名的 `callers/callees` 查询产生跨仓库重名噪声，因此直接调用边进一步用精确源码引用核验。
- Rust 入口与下游：`pkg/ddl/persistent_actions.rs`（action 85 分派）、`pkg/ddl/job_worker.rs`（`JobExecutionContext`）、`pkg/ddl/persistent_create_table.rs`（创建与保存 args）、`pkg/ddl/persistent_actions.rs`（notifier）、`pkg/meta/reader.rs`（删除 auto-ID 与 MLog schema diff）、`pkg/meta/model/job.rs`（多表 Job 完成）。
- crate 与模块：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`。
- Go 对照：`pkg/ddl/mview_worker.go::onCreateMaterializedViewLog`、`rollbackCreateMaterializedViewLog`、purge upsert/delete helpers；`pkg/ddl/job_worker.go` 的 action 分派；`pkg/ddl/schema_version.go::SetSchemaDiffForCreateTable`。
- 测试证据：`pkg/ddl/ddl_test.go::TestBuildCreateMaterializedViewLogPurgeInfoUpsertSQL` 验证三种 purge SQL 形态；`pkg/ddl/tests/materializedviewlog/materialized_view_create_test.go::TestCreateMaterializedViewLogPurgeInfoFailureRollback` 验证缺系统表会删除日志表、purge 行并恢复基表；`pkg/ddl/job_submitter_test.go::TestCreateMaterializedViewLogJobTableIDs` 验证 Job 涉及的表 ID；`pkg/ddl/mview_worker_test.go` 覆盖相关 MLog 关系清理。仓库搜索未发现直接调用本 Rust `step` 的独立 Rust 测试。
- 本任务是纯文档分析，按任务约束未运行 Cargo。结构验证要求目标文档恰有本文的十一个固定二级标题；最终交付前另行执行并记录退出码。
