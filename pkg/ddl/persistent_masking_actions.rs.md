# `pkg/ddl/persistent_masking_actions.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/ddl/lib.rs` 公开为 `persistent_masking_actions` 模块。它不是脱敏策略 CREATE/ALTER/DROP DDL 的主实现；它把表级持久化 DDL 动作与 `mysql.tidb_masking_policy` 的伴随维护放在同一条 worker 执行路径中，并同时承载 DROP、RENAME、TRUNCATE 的表元数据状态推进。

直接分派入口是 `pkg/ddl/persistent_actions.rs::step`：action 11 进入 `truncate_table`，14/47 进入 `rename_tables`，4/87/88/94 进入 `drop_table`。此外，`pkg/ddl/persistent_drop_column.rs::step` 调用 `policies_on_column`，`pkg/ddl/persistent_modify_column.rs::sync_policy` 调用 `policies_on_table`。因此本文件既是持久化表动作处理器，也是列动作复用的策略查询边界。

## 核心职责

- `policies_on_table`、`policies_on_column` 和私有 `read_policies` 从系统表读取完整 14 列记录，并在任何写入前验证行宽、状态、限制操作、时间和整数列。这保留了 Go `queryMaskingPoliciesFromSysTable`/`maskingPolicyFromSysTableRow` 的“先完整解码、后变更”失败顺序。
- `drop_table` 推进表的 `Public -> WriteOnly -> DeleteOnly -> None` 状态机；在最终状态清理物化视图、TTL、TiFlash、亲和性、脱敏策略等关联资源，写 schema diff，并封装完成参数。
- `rename_tables` 支持单表重命名（action 14）和多表重命名（action 47），移动表元数据、调整外键引用、更新标签规则及脱敏策略中的库表名称，并写入包含受影响表的 schema diff。
- `truncate_table` 用新表 ID 和新分区 ID 重建表，迁移脱敏策略的 `table_id`，重建 placement/TiFlash/affinity/TTL 资源，发布 truncate 事件并完成 job。
- 私有 `update_materialized_view_dependencies` 与 `check_materialized_view` 保护物化视图依赖关系；`sql_string` 负责本文件动态 SQL 字符串值的反斜线和单引号转义。

## 主要符号

- `SELECT_POLICY: &str`：固定选择 `tidb_masking_policy` 的 14 个字段；字段位置随后被 `read_policies`、策略删除和更新逻辑按索引使用。
- `pub fn policies_on_table(context, table) -> Result<Vec<Vec<String>>, String>`：按 `table_id`、`policy_id` 顺序查询；供本文件、列修改和表级策略清理复用。
- `pub fn policies_on_column(context, table, column) -> Result<Vec<Vec<String>>, String>`：把过滤范围限定为一个表列，避免其他列的坏记录阻断该列清理。
- `fn read_policies(context, predicate)`：统一执行 SQL 和逐行校验。合法状态为 `ENABLE[D]`/`DISABLE[D]`；合法 `restrict_on` token 为 `NONE`、`INSERT_INTO_SELECT`、`UPDATE_SELECT`、`DELETE_SELECT`、`CTAS` 或空值；时间列 11/12 以严格上下文解析，列 0/4/6 必须是 `i64`。
- `pub fn drop_policies_on_table`：先读完并验证全部策略，再逐个按 `policy_id` 删除。
- `pub fn drop_table(context, job) -> Result<i64, String>`：处理 DROP TABLE 及物化视图相关 drop action；返回生成的 schema version。
- `fn update_materialized_view_dependencies(meta, job, dropping)`：删除物化视图或其日志时，维护 base/log 表中的双向 ID 集合并返回需要进入 schema diff 的表 ID。
- `fn update_policy_names`：大小写不敏感比较现有名称；仅在名称变化时读取时间戳并更新 `db_name`、`table_name`、`updated_at`。
- `pub fn rename_tables(context, job) -> Result<i64, String>`：解码单表/多表参数，完成元数据移动、伴随资源更新与第二阶段 job 完成。
- `fn txn_delete_table`：重命名时对 `TransactionMutator::drop_table_only` 的局部语义包装，不删除 auto ID。
- `pub fn truncate_table(context, job) -> Result<i64, String>`：解码 truncate 参数并以新 ID 重建表及关联资源。
- `fn check_materialized_view(t, op)`：拒绝对物化视图、日志、shadow 表或仍带物化视图依赖的 base 表执行不支持的操作。

## 执行流程

`drop_table` 的主流程如下：

1. 用 `GetDropTableArgs` 解码 job；失败即把 `job.state` 设为 `Cancelled`。
2. 在事务中确认库表存在，执行对象类型、物化视图依赖和外键检查，再按一次调用推进一个 schema state。
3. 若刚进入 `WriteOnly` 且表有 TTL，删除 TTL 注册；随后在新事务中生成 schema version、更新表元数据，并在 `None` 阶段删除表及 auto ID。物化视图依赖变化决定写普通 table diff 还是 drop-mview diff。
4. 仅在 `None` 阶段清理物化视图系统行、TiFlash/affinity 等资源，发送 drop 事件，删除表上的所有脱敏策略，收集分区和 rule ID，调用 `finish_table_job` 并序列化 V1 或当前版本完成参数。

`rename_tables` 的主流程如下：

1. action 14 用 `GetRenameTableArgs` 形成单元素列表，其他路径用 `GetRenameTablesArgs`；`job.schema_state == Public` 表示元数据移动已完成，本次仅收尾。
2. 对每个条目，在事务中检查目标库和名称冲突，从旧库 `drop_table_only`，按跨库情况维护 `AutoIDSchemaID`，改名后在新库创建表，并在启用外键时扫描和更新引用该表的 child table。
3. 非收尾阶段在事务外更新 PD 标签和脱敏策略名称；全部条目处理后生成 schema version，直接写 `Diff:{version}`，将额外重命名条目和受影响外键表放入 `affected_options`。
4. 把 job schema state 置为 `Public`；下一次进入收尾分支时调用单表或多表 `finish_*_job`。

`truncate_table` 的主流程如下：

1. 解码参数并读取 public 表，拒绝 view、sequence 和 `check_materialized_view` 判定的不支持对象；启用 FK 检查时拒绝仍被其他表引用的目标。
2. 迁移 TTL 注册并提供恢复旧注册的补偿尝试；删除旧表及 auto ID，清理旧 TiFlash 资源，为分区补齐新 ID。
3. 更新标签规则和 TiFlash 配置，将表 ID 换为 `NewTableID`，逐条把脱敏策略的 `table_id` 和 `updated_at` 更新为新值。
4. 计算并发布 placement bundles，创建新表，迁移 affinity，写包含旧/新表与分区 ID 映射的 schema diff。
5. 发布 truncate 通知，完成 job，按 job version 序列化 finished args，并清空内存中的 `job.args`。

## 数据与状态

核心持久状态有三类：`astersql_meta_model::group_3::Job`、`TableInfo`/数据库元数据，以及 `mysql.tidb_masking_policy` 行。`drop_table` 显式维护表 schema state；`rename_tables` 用 `job.schema_state == Public` 区分“已移动、待 finish”；`truncate_table` 保持新表为 `Public`，但更换表和分区 ID。

策略查询结果目前以 `Vec<Vec<String>>` 表示，字段位置与 `SELECT_POLICY` 严格耦合：0/4/6 是 policy/table/column ID，2/3 是库表名，8 是状态，10 是限制操作，11/12 是时间。扩展 SELECT 列或改变顺序时必须同步所有索引访问和 Go 行解码语义。

schema version 由 `TransactionMutator::gen_schema_version` 生成。DROP 通过 `set_table_schema_diff` 或 `set_drop_mview_schema_diff` 写 diff；RENAME/TRUNCATE 在 `Diff:{version}` key 写 JSON。job 的 `raw_args` 在 DROP/TRUNCATE 完成时按 `JobVersion::V1` 与当前结构分别编码，以保留持久化兼容性。

## 依赖与调用关系

上游调用边经 RustCodeGraph 与源码核对如下：

- `pkg/ddl/persistent_actions.rs::step -> {drop_table, rename_tables, truncate_table}`；这是 owner worker 的持久化 action 分派路径。
- `pkg/ddl/persistent_drop_column.rs::step -> policies_on_column`；列到达最终删除阶段后清理绑定策略。
- `pkg/ddl/persistent_modify_column.rs::sync_policy -> policies_on_table`；修改/重命名列时筛选并更新关联策略。

主要下游依赖是 `JobExecutionContext`（`pkg/ddl/job_worker.rs`）提供的 SQL、事务、TTL、PD label/placement、TiFlash、affinity 和通知资源接口；`astersql-meta::TransactionMutator` 提供数据库、表、schema version/diff 与 auto-ID 元数据操作；`astersql-meta-model` 提供 job 参数、状态和表模型；`astersql-ddl-notifier` 发布 drop/truncate 事件；`astersql-ddl-placement` 重建 truncate 后的规则；`astersql-types` 严格校验系统表时间。

`pkg/ddl/Cargo.toml` 直接声明了上述本地 crate 依赖，包括 `astersql-meta`、`astersql-meta-model`、`astersql-ddl-notifier`、`astersql-ddl-placement`、`astersql-kv`、`astersql-tablecodec`、`astersql-types`、`astersql-sessionctx-vardef`、`serde`/`serde_json`。该模块没有自己的 feature gate 或条件编译项。

## 错误处理与边界

函数统一返回 `Result<_, String>`。job 参数无效、库表不存在、目标名称冲突、不合法 schema state、FK/物化视图约束、外部资源更新以及序列化错误都会向 worker 传播；若错误意味着当前 job 不可继续，多处分支同时把 `job.state` 设为 `Cancelled`。系统表不存在的精确消息会在 `read_policies` 中改写为 `[schema:1146]...`，DROP 最终清理再保留错误码前缀并增加目标 table ID 上下文。

并非所有清理失败都阻断 job：物化视图 refresh alert 删除、TiFlash drop 清理和 affinity 删除被显式忽略或仅在下层记录；其余关键更新通常传播错误。`drop_policies_on_table`、rename/truncate 策略更新与 Go 一致，缺少 `mysql.tidb_masking_policy` 时应失败，而不是当作无策略成功；Go 的 `TestMaskingPolicyOperationsRequireSysTable` 固定了这一边界。

本文件通过 `sql_string` 转义动态字符串值，但数值直接来自已解析的 `i64`；predicate 仅由内部数值参数拼接。新增自由文本 SQL 参数时不能绕过该转义边界，优先改为上下文支持的参数化执行。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道；并发串行化和 owner 生命周期由外层 DDL scheduler/worker 负责。单次 job step 可被持久化状态机再次调用，因此状态判断和完成参数必须保持可恢复、可重入的阶段边界。

`JobExecutionContext::with_transaction` 提供短元数据事务；trait 注释明确 SQL 执行前会释放事务借用。本文件的元数据事务、系统表 SQL、PD/TTL/TiFlash/affinity 回调并非一个跨资源原子事务：例如 truncate 先删除旧表元数据，再更新策略、placement，最后创建新表；TTL 注册失败时仅对旧注册做显式补偿。扩展时必须保留 Go 的事务边界、错误传播和可重试顺序，不能假设一个回滚会撤销所有外部副作用。

DROP 的状态机把可见性变更分散到多个 worker step，最终 `None` 才释放资源；RENAME 用 `Public` 作为第二阶段完成标记；TRUNCATE 则在一条处理函数中按严格顺序迁移资源。通知通过 `persistent_actions::async_notify_event` 交给外层持久化通知设施，本文件本身不管理通知任务生命周期。

## 与 Go 版本的对应关系

Go 对照不是单一同名文件，而是 `pkg/ddl/table.go` 与 `pkg/ddl/masking_policy.go` 的组合：

- Rust `read_policies` 对应 Go `queryMaskingPoliciesFromSysTable` 及其固定 SELECT；Rust显式验证字符串行，Go 交给 `maskingPolicyFromSysTableRow` 构造强类型 `MaskingPolicyInfo`。
- Rust `drop_policies_on_table` 对应 Go `dropMaskingPoliciesOnTable`；两者都先取得全部策略再逐条按 ID 删除。
- Rust `update_policy_names` 对应 Go `updateMaskingPolicyNamesAfterRename`；两者按大小写不敏感名称判断跳过，并只更新库名、表名和更新时间。
- Rust `truncate_table` 中的策略迁移对应 Go `updateMaskingPolicyTableIDAfterTruncate`：更新表 ID 和更新时间，保留 column ID。Go `TestMaskingPolicyTruncateKeepsPolicy` 验证策略仍可启停和删除。
- Rust DROP/RENAME/TRUNCATE 的大流程分别对应 `table.go` 的 drop 收尾、`onRenameTable`/`onRenameTables`、truncate 流程。Rust还在同一文件中包含物化视图依赖、TTL、placement、TiFlash、affinity 等必要接线，不应把它误解为只维护 masking policy 的小工具。

已核对的 Go 回归包括 `pkg/ddl/masking_policy_internal_test.go::TestMaskingPolicyOperationsRequireSysTable`、`pkg/ddl/masking_policy_test.go::{TestMaskingPolicyRenameTable, TestMaskingPolicyRenameTableCrossDatabase, TestMaskingPolicyRenameTableNoPolicy, TestMaskingPolicyTruncateKeepsPolicy}`。Rust 仓库搜索未发现直接引用本模块或这些公开函数的同名独立测试；`pkg/ddl/masking_policy_test.rs` 测试的是内存 `MaskingPolicyStore`/表达式逻辑，不覆盖本文件的 durable worker 接线。

## 扩展指南

- 增减策略字段：同步修改 `SELECT_POLICY`、`read_policies` 的行宽和字段索引、所有消费者、Go `queryMaskingPolicyFromSysTable`/行解码，并在独立 Rust 测试文件中覆盖坏行与兼容数据；不要把测试嵌入本源文件。
- 新增表动作：在 `persistent_actions::handler_available/step` 建立明确 action 分派，决定 schema-state/reorg/rollback 语义，再选择复用这里的策略读取/更新辅助函数。应补独立 worker 上下文测试和对应 Go 行为对照。
- 调整 DROP：重点维护 `Public -> WriteOnly -> DeleteOnly -> None`、物化视图双向依赖、schema diff 类型、finished args 版本兼容，以及最终阶段才执行的资源清理。新增失败点要区分阻断错误与 best-effort 清理。
- 调整 RENAME：同时考虑跨库 auto-ID 归属、外键 child table、PD label、masking policy 名称与 `affected_options`；多表 action 47 还要保持 job 临时 table ID/name 和最终完成列表一致。
- 调整 TRUNCATE：保持旧/新表与分区 ID 映射、策略 column ID 不变、TTL 补偿、placement/TiFlash/affinity 顺序及 finished args。外部资源操作存在部分完成风险，新增步骤必须设计重试或补偿。
- 测试应优先新增/扩展 `pkg/ddl` 下独立 `*_test.rs`，构造可记录 SQL、事务和资源回调的 `JobExecutionContext`；并继续对照 Go 的 masking policy 与 table 测试。当前直接 Rust 回归缺口值得在后续实现任务中补齐，但不属于本纯文档任务。

## 验证依据

- RustCodeGraph：`status` 显示目标仓库已索引；`files --filter pkg/ddl/persistent_masking_actions.rs` 命中该文件并报告 37 个符号；`node --file ... --offset 1/500` 读取 760 行全貌并显示由 `pkg/ddl/persistent_actions.rs` 使用。
- RustCodeGraph 调用查询：`policies_on_table/policies_on_column -> read_policies`；`drop_table -> async_notify_event, drop_policies_on_table, update_materialized_view_dependencies`；`rename_tables -> update_policy_names, txn_delete_table`；`truncate_table -> policies_on_table, check_materialized_view, async_notify_event`，以及各 `JobExecutionContext` 资源回调。图对上游 caller 未返回边，因此又由已索引源码 `persistent_actions.rs` 的明确分派和相邻 Rust 源直接核验。
- 已读 Rust 路径：`pkg/ddl/persistent_masking_actions.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/persistent_drop_column.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- 已读 Go/测试路径：`pkg/ddl/table.go`、`pkg/ddl/masking_policy.go`、`pkg/ddl/masking_policy_internal_test.go`、`pkg/ddl/masking_policy_test.go`；另用仓库搜索确认 Rust `*_test.rs` 中没有本模块的直接持久化动作测试。
- 本任务只新增说明文档，未运行 Cargo 或代码测试；最终以固定 11 章节结构命令和人工事实复核验收。
