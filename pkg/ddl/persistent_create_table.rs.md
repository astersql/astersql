# `pkg/ddl/persistent_create_table.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[package] name = "astersql-ddl"`），实现普通 DDL worker 在持久事务路径上执行 `CREATE TABLE` 与批量建表的核心步骤。上游统一入口是 `persistent_actions::step`：动作号 `3` 分派给 `step`，动作号 `60` 分派给 `batch_step`（`pkg/ddl/persistent_actions.rs:59-100`）。它不是 SQL AST 构造层，也不负责提交/调度 DDL job；它消费已持久化到 `Job.raw_args` 的表元数据，在 owner worker 上创建目录元数据、生成 schema version/diff，并完成外部资源通知。

RustCodeGraph 将本文件识别为 22 个符号，并报告其被 `executor.rs`、`job_worker.rs`、`partition.rs`、`persistent_actions.rs`、`persistent_create_materialized_view.rs` 等 6 个文件使用。精确调用边查询在当前索引上把 symbol id 错当成普通名称，因而直接调用关系又以源码引用核验；可确认 `create_table` 还被物化视图、物化视图日志和 shadow-table 持久路径复用（`persistent_create_materialized_view*.rs`）。

从 DDL 生命周期看，这是 job-based、owner-driven 路径，不是 metadata-only 快速通道。普通无外键表直接发布 `Public`；含外键表跨两次 worker step 经 `DeleteOnly` 到 `Public`。文件不做 reorg/backfill。取消通常通过把 `job.state` 设为 `Cancelled` 后返回错误；已写入的表元数据和外部 PD/TTL 资源是否清理由更高层 job 生命周期及对应补偿接口负责，本文件只对批量 TTL 注册实现显式逆序补偿。

## 核心职责

1. `decode` / `decode_optional` 从持久 job 参数中恢复 `TableInfo` 与 `FKCheck`；`save_args` 将状态推进后的表信息按 job V1/新版本编码布局写回，支持下一步恢复执行。
2. `create_table` 完成单张物理表的共同创建阶段：检查列存能力、库/表/约束名、外键及 placement policy，分配外键 ID，写入表元数据，并配置 TiFlash、placement bundle、affinity 与 auto-ID。
3. `step` 编排单表 schema 状态与版本发布。无外键表一次完成；含外键表先发布 `DeleteOnly`，后续 step 再转为 `Public`。
4. `batch_step` 复用 `create_table` 创建多张表，只生成一个 schema version 和包含所有表的 schema diff，逐表发送 notifier 事件并注册 TTL；TTL 失败时逆序删除已注册项。
5. `check_foreign_keys` / `check_foreign_key` 在 owner 看到的公开目录上校验父子表关系、列兼容性和父索引条件。

## 主要符号

- `cancel(job, error) -> String`：统一将 job 置为 `JobState::Cancelled` 并保留错误文本。它只改变 job 内存状态，不执行元数据回滚。
- `decode(job) -> Result<(TableInfo, bool), String>`：要求建表参数必须含 `TableInfo`；缺失时产生 `[ddl:1105]missing create-table metadata` 并取消 job。
- `decode_optional(job)`：公开给 crate 内物化视图 shadow 路径使用的可选解码器；调用 `group_2::GetCreateTableArgs`，再经 JSON 投影为本 crate 使用的 `TableInfo`。
- `save_args(job, table)`：兼容 `JobVersion::V1` 的数组第 0 项和新版对象的 `table_info` 字段，重写 `raw_args` 后清空已解码的 `job.args` 缓存。
- `Policies<'a, 'b>`：把 `TransactionMutator::get_placement_policy` 适配为 `astersql_ddl_placement::PolicyGetter`；策略不存在时返回 `[schema:8249]`。
- `create_table(context, job, table, fk_check)`：crate 内共享的物理表创建函数。注释明确它不发布 schema version、不发 notifier、不注册 TTL、也不结束 job。
- `step(context, job) -> Result<i64, String>`：单表公开入口，返回本步生成的 schema version。
- `check_foreign_keys`：建立当前所有 `Public` 表的目录快照，检查新表声明的外键以及既有子表指向新父表的反向关系。
- `check_foreign_key`：校验临时表、TTL 父表、分区表、父列存在性/虚拟生成列、子列存在性、类型/unsigned/charset/collation 和父索引。
- `batch_step(context, job) -> Result<i64, String>`：批量动作入口，克隆 stub job 并按表 ID 执行公共创建路径，最后一次性发布版本与结果。

## 执行流程

单表流程如下：

1. `step` 调用 `decode`，判断 `table.ForeignKeys` 是否为空。
2. 首次执行（无外键，或表状态为 `None/Public`）调用 `create_table`。该函数先调用 `JobExecutionContext::check_create_table_columnar`，随后把状态重置为 `None`。
3. 在一个 `with_transaction` 闭包内确认数据库存在、同名表不存在、跨表 check constraint 名不重复；校验外键，分配递增 FK ID 并设为 `Public`；随后把表设为 `Public`、写入事务起始时间、调用 `check_table_info_valid` 并执行 `TransactionMutator::create_table`。表级与分区级 placement policy 引用也在此事务中确认存在。
4. 元数据写入后，按需配置 TiFlash replica；在只读元数据事务中通过 `NewFullTableBundles` 构造 placement bundles，非空时发给 PD；按需创建 affinity group；最后 rebase auto-ID。由此可见外部资源调用不与表元数据处于同一事务。
5. 回到 `step`：含外键表先将状态改为 `DeleteOnly`，用 `update_version_and_table` 持久化并更新 `job.schema_state`；无外键表直接生成版本并设置 table schema diff。
6. 若这是含外键表的后续执行，`DeleteOnly` 或兼容旧作业的 `WriteOnly` 都直接推进到 `Public` 并更新版本和表元数据；其他状态返回 `ErrInvalidDDLJob`。
7. `set_create_table_schema_diff` 写入最终 diff。仅在无外键或已到 `Public` 时发送 `NewCreateTableEvent`、注册 TTL 并调用 `finish_table_job(Done, Public, ...)`。最后 `save_args` 保存当前表状态；因此含外键首步会留下可恢复的 `DeleteOnly` 参数而不会结束 job。

批量流程如下：

1. `batch_step` 解码 `GetBatchCreateTableArgs`，克隆一个 stub job；每张表必须有 `TableInfo`，然后用其 ID 更新 `stub.table_id` 并调用 `create_table`。
2. 全部创建成功后生成一个 schema version，直接写入 `Diff:{version}`；`affected_options` 为每张表记录 schema/table ID。
3. 按表序号发送 create-table notifier 事件。
4. 逐表注册 TTL。失败时把此前已成功注册且 TTL 启用的表 ID 逆序传给 `delete_drop_table_ttl`；补偿失败只记录 warning，原始注册错误取消整个 job。
5. 全部成功后调用 `finish_multiple_table_job(Done, Public, version, tables)`。

## 数据与状态

- 持久输入是 `Job.raw_args` 中的 `CreateTableArgs` 或 `BatchCreateTableArgs`，核心载荷为 `TableInfo` 与 `FKCheck`。`save_args` 保持 V1 与新版编码兼容，是含外键 job 跨 step 恢复的关键检查点。
- `TableInfo.State` 在公共创建阶段被强制重置并最终写成 `Public`；单表外键路径随后将其降为对外不可直接使用的 `DeleteOnly`，下一 step 再升为 `Public`。接受遗留 `WriteOnly` 是升级兼容分支，依据见 Go `createTableWithForeignKeys` 注释与 Rust `step` 的匹配分支。
- `Job.state` 在确定性输入/环境错误上经 `cancel` 变为 `Cancelled`；成功终态由 `finish_table_job` 或 `finish_multiple_table_job` 写为 `Done`。首个外键阶段只更新 `job.schema_state = DeleteOnly`。
- 每个新外键以 `MaxForeignKeyID += 1` 分配 ID并直接标为 `Public`；表可见性仍由表级 schema state 控制。
- schema version 由 `gen_schema_version` 或 `update_version_and_table` 产生。批量建表共享一个版本，diff 的 `affected_options` 列出全部表。
- `UpdateTS` 取元数据事务的 `start_ts`。placement bundles 是事务外临时向量；TTL 补偿列表只记录确实启用了 TTL 的表 ID。

## 依赖与调用关系

上游主链为 `persistent_actions::step` → `persistent_create_table::{step,batch_step}`。更高层由正常 DDL worker 调用 `persistent_actions`；本文件通过 `JobExecutionContext` 抽象事务及外部服务，使具体 worker 提供 PD、TTL、auto-ID 和 notifier 能力。`pkg/ddl/job_worker.rs` 中这些方法的默认实现均返回“resource unavailable”，说明真实运行上下文必须覆盖它们，不能把默认 trait 行为视为可工作的降级方案。

主要下游依赖为：

- `astersql-meta` 的 `TransactionMutator`：数据库/表/placement policy 查询，表创建、schema version 与 diff 持久化。
- `astersql-meta-model`：`Job`、`TableInfo`、`SchemaState`、job 参数解码及外键模型。
- `astersql-ddl-placement`：通过 `Policies` 查询策略并生成完整表 bundle。
- `persistent_actions::{update_version_and_table, async_notify_event}`：状态持久化与异步 DDL 事件。
- `astersql-ddl-notifier::NewCreateTableEvent`：下游 schema 变更通知。
- `JobExecutionContext`：列存检查、TiFlash/PD/affinity、auto-ID、TTL 注册与补偿的资源边界。
- `serde_json`：跨近似模型投影、job 参数重写及批量 schema diff 编码。

反向复用关系包括 `persistent_create_materialized_view.rs`、`persistent_create_materialized_view_log.rs` 和 `persistent_create_materialized_view_shadow.rs` 调用 `create_table`；log/shadow 路径还使用 `save_args` 或 `decode_optional`。修改公共创建阶段会同时影响这些非普通建表动作。

## 错误处理与边界

可取消错误包括参数解码/元数据缺失、数据库或同名表不存在/冲突、check constraint 重名、外键不合法、placement policy 缺失、PD/affinity/TTL 外部调用失败。错误以字符串传播并尽量保留 TiDB 错误码，例如 `[schema:1049]`、`[schema:1050]`、`[schema:3822]`、`[schema:8249]`。

外键开关 `EnableForeignKey` 关闭时跳过 owner 校验；`FKCheck=false` 时允许引用的父表暂不存在，但父表存在时仍执行结构兼容检查。只扫描 `Public` 表。规则禁止临时表参与、TTL 表作为父表、任一侧分区；要求父列存在且非虚拟生成列，子列存在，类型、unsigned、字符集、排序规则一致，并要求父列由适用索引覆盖（单列 clustered primary-key handle 是例外）。

事务边界是重要限制：表元数据先提交/落入事务路径，TiFlash、PD bundle、affinity、auto-ID 随后逐项执行；本文件没有为这些单表外部副作用实现局部逆向补偿。批量创建同样逐表调用 `create_table`，并非一个覆盖所有表及外部资源的原子事务；只有 schema diff 是一次发布，且只有 TTL 注册有显式逆序补偿。因此扩展错误路径时必须结合 worker 的 job rollback/重试语义检查幂等性，不能假定返回 `Err` 会自动撤销此前所有副作用。

批量路径要求每项 `TableInfo` 存在，但当前 Rust 实现没有 Go `onCreateTables` 对 `tableInfo.Sequence != nil` 的 `createSequenceWithCheck` 特判；这是可从源码确认的移植差异，不应将批量 sequence 创建描述为已由本文件支持。

## 并发与资源生命周期

文件自身不创建线程、任务、锁或通道；并发串行化、owner 选举、job 重试和 schema sync 由外层 DDL worker 框架负责。所有元数据读写通过 `JobExecutionContext::with_transaction` 获得短生命周期事务，`TransactionMutator` 不逃逸闭包。

资源生命周期顺序为：持久 job 参数 → 表元数据事务 → schema version/diff → 外部通知/TTL → job history 终态。含外键 job 在 `DeleteOnly` 检查点跨 worker step 存活，`save_args` 使重试能读回推进后的状态。外部 PD/affinity/auto-ID 调用位于表元数据事务之外，必须具备重试幂等性；批量 TTL 注册维护会话内的成功 ID 列表，错误时逆序清理并仅记录清理失败。notifier 事件先于 TTL 注册，故 TTL 失败取消 job 时事件可能已经发出；消费者和上层恢复逻辑必须能容忍这一顺序。

本文件不执行 reorg/backfill，也不直接等待全节点 schema version 同步。它返回 version，由外层 worker 完成版本同步与 job 生命周期收尾。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/ddl/create_table.go` 与 `pkg/ddl/foreign_key.go`：

- Rust `create_table` 对应 Go `createTable`。两者都声明为单表/批量共享内部函数，不负责更新 schema version、结束 job 或发送事件；都依次检查列存、名称与外键，写表元数据，处理 TiFlash、placement、affinity 和 auto-ID。
- Rust `step` 对应 Go `onCreateTable` 加 `createTableWithForeignKeys`。无外键一次完成；有外键先 `None/Public → DeleteOnly`，再兼容 `DeleteOnly/WriteOnly → Public`，只在最终公开时通知、注册 TTL 和结束 job。
- Rust `batch_step` 对应 Go `onCreateTables` 与 `registerTTLTablesToExternalWorkload`：共享一个版本、逐表通知、TTL 失败逆序补偿。Rust 直接构造 `Diff:{version}`，而 Go 通过 `updateSchemaVersion`；Rust 未包含 Go 的 sequence 分支。
- Rust `check_foreign_keys` / `check_foreign_key` 对应 Go `checkTableForeignKeyValidInOwner` / `checkTableForeignKey`，核心禁止项与类型/索引规则一致。Go 使用 infoschema 最新快照，Rust 从 `TransactionMutator` 枚举数据库及公开表构造目录。
- Go 对部分错误区分可重试与不可重试，再决定是否取消；Rust 当前多数校验与外部资源错误直接经过 `cancel`。扩展错误处理时应对照 Go 的 retryable 语义，不应仅复制错误文本。

独立 Rust 测试证据主要位于 `pkg/ddl/tests/fk/foreign_key_test.rs`，覆盖父表缺失、关闭 foreign-key checks、自引用及临时表/TTL/分区/列兼容矩阵；`pkg/ddl/db_table_test.rs` 覆盖普通与批量创建的 ID、重复名和 on-exist 行为；`pkg/ddl/tests/fastcreatetable/fastcreatetable_test.rs` 覆盖合并批量 job；`pkg/ddl/tests/fail/fail_db_test.rs` 覆盖快速建表失败场景。未发现以 `persistent_create_table` 命名的独立单元测试文件。

## 扩展指南

- 新增普通建表的持久校验时，优先放在 `create_table` 的元数据写入前，并同步对照 Go `createTable`；若只属于发布阶段，则放在 `step`/`batch_step`，避免物化视图复用路径意外继承。
- 新增外键规则应同步修改 `check_foreign_keys`/`check_foreign_key` 与 `pkg/ddl/tests/fk/foreign_key_test.rs`，同时核对 `foreign_key.go` 的 owner 校验、`fk_check=false` 和反向子表检查。
- 新增 `TableInfo` 状态字段时，要更新 `save_args` 的 V1/新版编码兼容测试，并验证跨 `DeleteOnly` 重试恢复；不能只改内存对象。
- 新增外部资源时，应通过 `JobExecutionContext` 暴露明确接口，说明调用在表元数据提交前后的位置、幂等键和 rollback/补偿策略，并为默认 trait 实现与真实 worker 实现分别检查。
- 修改批量逻辑要保留“一个 schema version + 每表 affected option + 每表 notifier”的契约，并给部分成功、通知失败、TTL 中途失败及补偿失败增加独立测试。若补齐 sequence 支持，应明确移植 Go `createSequenceWithCheck`，而不是让 sequence 走普通 `create_table`。
- placement、TiFlash、affinity、TTL 或 auto-ID 变化可能引入 PD/控制器兼容与失败恢复风险；外键目录全量枚举还具有随库表数量增长的性能风险。优化时须保持只接受 `Public` 元数据与错误码语义。
- 按仓库约定，Rust 测试继续放在独立 `*_test.rs` 或 `tests/` 文件中，不把测试内嵌进本生产文件。

## 验证依据

- 源码：`pkg/ddl/persistent_create_table.rs` 全部 441 行；直接入口 `pkg/ddl/persistent_actions.rs:59-100`；资源接口 `pkg/ddl/job_worker.rs`；复用者 `pkg/ddl/persistent_create_materialized_view.rs`、`persistent_create_materialized_view_log.rs`、`persistent_create_materialized_view_shadow.rs`。
- crate 边界：`pkg/ddl/Cargo.toml`，确认 crate 名、`[lib] path = "lib.rs"`，以及 `astersql-meta`、`astersql-meta-model`、`astersql-ddl-placement`、`astersql-ddl-notifier`、`astersql-sessionctx-vardef`、`astersql-util-dbterror`、`serde_json` 等直接依赖。
- Go 对照：`pkg/ddl/create_table.go` 的 `createTable`、`onCreateTable`、`createTableWithForeignKeys`、`onCreateTables`、`registerTTLTablesToExternalWorkload`；`pkg/ddl/foreign_key.go` 的 `checkTableForeignKeyValidInOwner` 与 `checkTableForeignKey`。
- 测试：`pkg/ddl/tests/fk/foreign_key_test.rs`、`pkg/ddl/db_table_test.rs`、`pkg/ddl/tests/fastcreatetable/fastcreatetable_test.rs`、`pkg/ddl/tests/fail/fail_db_test.rs`。这些是相关行为证据；本任务按计划不运行 Cargo，也不声称执行了测试。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 边；`files --filter pkg/ddl/persistent_create_table.rs` 定位目标；`node --file ... --offset 1 --limit 460` 返回全文件和 22 个符号；`query persistent_create_table --json` 核对 `cancel`、`decode(_optional)`、`save_args`、`Policies::GetPolicy`、`create_table`、`step`、两级外键检查与 `batch_step` 的签名。精确 callers/callees 命令在当前 CLI 中误将 symbol id 解释成名称并返回无关候选，因此直接边由上述源码引用补证，未据此虚构图关系。
- 结构验证按任务指定命令执行；人工复核重点为固定 11 个二级标题、真实符号/调用关系、Go 差异、错误与事务边界，以及未把批量 sequence 或自动回滚描述为已支持。
