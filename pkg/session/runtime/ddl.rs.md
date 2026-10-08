# `pkg/session/runtime/ddl.rs`

## 文件定位

`ddl.rs` 是 `astersql-session` crate 中 `ConcreteSession` 的会话级 DDL 执行层。模块由 [`pkg/session/runtime.rs`](../runtime.rs) 的 `mod ddl;` 私有装配，并由 [`pkg/session/runtime/dispatch.rs`](dispatch.rs) 在完成语句分类、DDL 隐式提交以及部分 SQL 文本兼容检查后调用。它不实现 DDL owner/worker 本身，而是把解析后的 AST 转成元数据对象或 normal DDL action，执行会话变量、权限、现有数据和兼容性校验，再调用 `Domain`/`astersql-ddl` 的持久化接口。

该文件属于 [`pkg/session/Cargo.toml`](../Cargo.toml) 声明的 `astersql-session` crate；直接使用的主要工作区依赖包括 `astersql-ddl`、`astersql-domain`、`astersql-meta-model`、`astersql-meta-metabuild`、`astersql-parser-ast`、`astersql-kv`、`astersql-expression` 和 `astersql-testkit-testfailpoint`。文件没有 feature 条件分支；唯一条件编译项是末尾 `#[cfg(test)] #[path = "ddl_test.rs"] mod tests;`，测试逻辑保存在独立文件 [`pkg/session/runtime/ddl_test.rs`](ddl_test.rs)。

## 核心职责

- 执行数据库、表、序列和表结构 DDL：`execute_create_database`、`execute_drop_database`、`execute_create_table`、`execute_drop_table`、`execute_rename_table_pairs`、`execute_create_sequence`、`execute_drop_sequence`、`execute_alter_table`、`execute_truncate_table`。
- 在提交元数据变更前实施 TiDB 兼容约束，包括字符集/排序规则、ENUM/SET 长度、AUTO_RANDOM、主键要求、向量列、外键、唯一索引、物化视图依赖、分区和 TTL 等检查。
- 在两种后端路径间接线：`persistent_actions_enabled()` 为真时通过 `submit_normal_action` 提交带 action type 和 JSON 参数的 owner 作业；否则调用 `Domain::ddl_*` 接口，并用运行时 DDL job/snapshot 模拟作业生命周期与回滚观察面。
- 在建表、截断和部分分区变更后执行 `pre_split_and_scatter`，并在本地运行时登记物理 region 数；在元数据变更后通过 `update_self_version_with_retry` 模拟节点 schema version 更新。
- 维护会话/进程内兼容状态，例如当前数据库、临时表、数据库选项、警告、region 计数以及本地临时表 auto ID；这些状态与 Domain 的共享 catalog 并存。

## 主要符号

- `has_explicit_region_split_config` / `should_pre_split_after_create`：识别表、索引或 `SHARD_ROW_ID_BITS + PRE_SPLIT_REGIONS` 的显式切分配置，并确保 Restore 启动模式不做预切分。
- `ddl_system_table_id`：仅为 `mysql` 库中的系统表选择 `astersql_meta_metadef` 固定 ID；NextGen 特有的 masking policy 还受 `IsNextGen()` 约束。
- `validate_ddl_collation`、`validate_create_table_collations`、`ddl_collation_charset`：拒绝新排序规则模式不支持的 `_roman_ci`，并校验 database/table/column charset 与 collation 的对应关系。
- `auto_random_bits_from_definition`：解析 `AUTO_RANDOM` shard/range bits，要求 BIGINT、合法位数和至少 27 位可分配空间，并拒绝与 `AUTO_INCREMENT`、默认值共存。
- `execute_create_table`：本文件最大的建表入口；处理 `CREATE TABLE LIKE`、临时表、元数据构造、表选项、索引/外键/向量约束、系统 ID、持久化、预切分和 schema version 更新。
- `execute_rename_table_pairs`：把有序 rename 链作为原子操作处理；normal DDL 模式分别使用 action 14（单表）或 47（多表），直接路径调用 `ddl_rename_tables`，保留物理 table ID。
- `validate_unique_index_rows`：发布 UNIQUE/PRIMARY 索引前扫描现有行；普通 UNIQUE 允许包含 NULL，PRIMARY KEY 遇到 NULL 报 1138，重复非 NULL key 报 1062。
- `execute_add_foreign_key` / `execute_drop_foreign_key`：验证现有子行、补建支持索引、更新 `FKInfo`；删除外键时故意保留支持索引。
- `execute_alter_table` / `ddl_should_analyze` / `execute_alter_table_inner`：外层完成跨表权限、通用校验、作业和可选 ANALYZE 生命周期；内层按 `AlterTableType` 执行具体元数据变更。
- `pre_split_and_scatter`、`update_self_version_with_retry`：分别负责物理 region 初始化和最多 10 次的 schema version 更新重试。
- `check_base_table_materialized_view_dependency_constraints`、`check_exchange_partition_materialized_view_constraints`、`check_embedding_column_dependencies`：集中保护物化视图、日志表和 EMBED_TEXT 生成列依赖。

所有 `execute_*` 方法均为 `pub(super)`，只向 `runtime` 父模块开放；校验辅助函数保持私有。文件没有自定义 struct、enum、trait、模块级业务常量或公开 crate API。

## 执行流程

1. [`runtime/dispatch.rs`](dispatch.rs) 识别 DDL AST。除本地临时表创建外，若当前存在事务，先调用 `finish_transaction(true)` 执行 MySQL/TiDB 的 DDL 隐式提交语义；随后分派到本文件的对应 `execute_*` 方法。`CREATE/DROP INDEX` 会被转换为 ALTER TABLE 或直接落到相同的 Domain 元数据接口。
2. 各入口解析未限定 schema，通常以 `current_database()` 补齐，并先验证 database/table 是否存在、对象是否为 base table，以及权限、会话变量和语句选项。
3. `execute_create_table` 安装 planner expression factory，构造或复制 `TableInfo`，清零 LIKE 来源的 ID/allocator/foreign key 状态，处理本地临时表的进程内高位 ID；共享表则建立 runtime job，调用 `Domain::ddl_create_table`，执行预切分，更新 schema version，并记录 AUTO_RANDOM 可用空间提示。
4. `execute_alter_table` 先对 RENAME/EXCHANGE 执行源表和目标表的有序权限检查，验证 collation 和大行限制，然后建立 runtime job。`execute_alter_table_inner` 逐个匹配 `AlterTableType`：覆盖分区方式切换与增删/截断/重组/交换分区、列增改名删、索引增改名/可见性/删除、外键、TiFlash 副本、TTL、placement、storage class、auto ID 和 shard bits；不支持的分支明确返回“requires the full DDL session ABI”。成功且 `ddl_should_analyze` 为真时，外层再运行 `ANALYZE TABLE` 并更新 job detail。
5. `execute_drop_table` 先在会话内清除 local temporary table 及其数据/allocator；共享表在直接路径保存旧 `TableInfo` snapshot 后调用 `ddl_drop_tables`。`execute_truncate_table` 同样保存旧表 snapshot，替换表元数据后重新预切分。
6. `RuntimeDdlJobGuard::finish` 根据 `SessionResult` 收束作业；失败路径可借助已附加 snapshot 恢复或向管理语句暴露失败状态。直接元数据路径在需要时调用 `update_self_version_with_retry`，持久化 action 路径则由 DDL owner 接管。

## 数据与状态

- 输入数据以 `astersql_parser_ast` 的 `CreateTableStmt`、`AlterTableStmt`、`DropTableStmt` 等 AST 为主；持久化形态是 `astersql_meta_model::TableInfo`、`ColumnInfo`、`IndexInfo`、`FKInfo`、`TTLInfo` 和 `PolicyRefInfo`。
- `ConcreteSession.state` 使用 `RefCell` 管理会话状态：`current_warnings`、`databases`、`local_temporary_tables`、`local_temporary_auto_ids`、`foreign_key_checks`、`sql_require_primary_key`、`scatter_region`、snapshot database 可见性等。借用均限定在局部作用域，构造 metabuild context 后会显式 `drop(state)`，避免后续可变借用冲突。
- `domain` 是共享 `Arc`，提供 catalog 查询、DDL 元数据写入、auto ID、region splitter、cross-keyspace 记录和 privilege handle。`runtime_domain_id` 或 `Arc::as_ptr` 被用作进程内兼容 map 的隔离键。
- 共享 map 包括 `RUNTIME_DATABASES`、`RUNTIME_DATABASE_OPTIONS`、`RUNTIME_REGION_COUNTS`、`RUNTIME_PLACEMENT_POLICIES` 和全局 txn entry size 限制；锁中毒时多数 map 选择 `PoisonError::into_inner` 继续，region count 写入则使用 `expect`。
- DDL 作业状态由 `begin_runtime_ddl_job`、`attach_runtime_ddl_snapshot`、`update_runtime_ddl_detail` 和 `RuntimeDdlJobGuard` 管理。局部临时表不进入共享 InfoSchema/DDL job，而保存在 session extended state。

## 依赖与调用关系

上游主链是 `ConcreteSession::execute`/内部分派 -> [`runtime/dispatch.rs`](dispatch.rs) -> 本文件 `execute_*`。源码可直接确认 `dispatch.rs` 对 CREATE/ALTER/DROP/TRUNCATE/RENAME/DATABASE/SEQUENCE 的调用；RustCodeGraph 对这些 `pub(super)` 方法的 `callers` 查询返回空，属于当前索引的调用边缺失，不能解释为无调用者。

下游分为四类：

- AST/元数据构造：`astersql-parser-ast`、`astersql-meta-metabuild::NewContext`、`astersql-ddl::BuildTableInfoFromAST`。
- catalog 与持久化：`Domain::stats_table`/`table_by_name` 读取，`Domain::ddl_create_table`、`ddl_drop_tables`、`ddl_rename_tables`、`ddl_truncate_table`、`ddl_add_index`、`ddl_replace_foreign_keys`、各 partition/TTL/placement 接口写入。
- owner action 路径：[`runtime/normal_ddl_submit.rs`](normal_ddl_submit.rs) 的 `submit_normal_action` 接收 action type 与 Go ABI 形状的 JSON 参数。
- 物理/表达式能力：`astersql-ddl::split_region::split_table_regions`、`astersql-kv::WithInternalSourceType(...InternalTxnDDL)`、planner expression factory、generated-column/TTL/storage-class 检查器。

RustCodeGraph 的精确 `callees` 为 `execute_alter_table` 找到 `stats_table`、`InstallPlannerExpressionFactory`、`runtime_privilege_handle`、`begin_runtime_ddl_job`、`update_runtime_ddl_detail`、`ddl_should_analyze` 等边；为 `execute_drop_table` 找到 `ddl_drop_tables`、`stats_table`、`begin_runtime_ddl_job`、`attach_runtime_ddl_snapshot`、`session_error` 和 `update_self_version_with_retry` 等边。图查询对 `execute_create_table`/`execute_truncate_table` 的下游识别不完整，因此其余关系以源文件和 `dispatch.rs` 的直接调用为依据。

## 错误处理与边界

- 所有入口返回 `SessionResult<()>`，下游错误通常经 `session_error("操作上下文", error)` 包装；兼容错误直接构造带 TiDB/MySQL error code 的 `SessionError`，例如 unknown database、重复表、非法 AUTO_RANDOM、外键 1452、主键要求 3750 和不支持 DDL 8200。
- `IF EXISTS`/`IF NOT EXISTS` 并非一律静默：建表已存在和删表不存在会按语义写入 Note/Warning；无保护条件时返回错误。空当前数据库且引用未限定名称时返回 1046。
- `CREATE TABLE LIKE` 不复制 allocator 水位、foreign keys 或物理 ID；temporary table 还拒绝 partition、placement、auto_random 和预切分等不兼容属性。
- 唯一索引校验以字符串拼接现有行值形成检测 key，属于此运行时的兼容实现；表达式 UNIQUE key 若未物化会拒绝，而不是猜测执行表达式。
- ALTER TABLE 只支持 `execute_alter_table_inner` 明确列出的 `AlterTableType`。未覆盖类型返回完整 ABI 未实现错误；`CACHE/NOCACHE` 在 mock runtime 中接受但没有异步 cache population 副作用，文档不能据此宣称完整生产缓存流程已移植。
- failpoint 覆盖建表回滚、owner version 等待、schema version 更新、truncate version 错误、外键检查和 analyze 阶段；它们是测试/故障注入边界，不是正常分支。

## 并发与资源生命周期

- 会话内部用 `RefCell`，假设 `ConcreteSession` 的可变状态由当前会话串行访问；跨会话/Domain 状态通过 `Mutex` map、原子 ID 和 `Arc<Domain>` 协调。
- 本地临时表 ID 由 `NEXT_RUNTIME_LOCAL_TEMPORARY_TABLE_ID.fetch_add(..., Ordering::AcqRel)` 分配；region split 全局开关按 `SeqCst` 读取；测试用 RAII reset guard 在退出时恢复开关。
- runtime DDL job 在变更前创建 guard，结果无论成功失败都传给 `finish`；DROP/TRUNCATE 在写入前附加旧表 snapshot。扩展新失败点时必须维持“建立 job -> 附 snapshot（若需回滚）-> 变更 -> finish”的顺序。
- `pre_split_and_scatter` 对 temporary table 和 Restore 模式提前返回；region splitter 调用完成后才登记 `RUNTIME_REGION_COUNTS`。表级/索引级 split policy 取最大 `Regions`，否则以最多 20 位的 `PreSplitRegions` 计算 `1 << bits`，避免不受控位移。
- `update_self_version_with_retry` 是同步且有界的 10 次循环，没有 sleep 或后台任务；耗尽后返回错误。真正的异步 DDL owner 生命周期位于下游 DDL 子系统，不在本文件实现。

## 与 Go 版本的对应关系

主要 Go 对照位于 [`pkg/ddl/executor.go`](../../ddl/executor.go)：`(*executor).CreateTable`、`CreateTableWithInfo`、`preSplitAndScatter`、`AlterTable`、`DropTable`、`TruncateTable`；外键父表校验对应 [`pkg/ddl/foreign_key.go`](../../ddl/foreign_key.go) 的 `checkTableForeignKeysValid`。Rust 版本保留了建表元数据构造、DDL 前置校验、临时表跳过预切分、Restore 模式跳过预切分、外键检查、分区/TTL/placement 等关键分支。

两者的架构边界不同：Go `pkg/ddl/executor.go` 直接处于完整 DDL executor/owner 体系；Rust 文件还承担 mock/runtime catalog、局部临时表和运行时作业快照的兼容接线，并在 `persistent_actions_enabled()` 下把 Go action type 参数序列化后交给 normal DDL owner。直接 `Domain::ddl_*` 路径与 action 路径并存，说明当前处于渐进迁移状态。

已确认的限制包括：部分 ALTER 类型明确要求 full DDL session ABI，CACHE/NOCACHE 无完整异步实现，schema version retry 是同步模拟。以上均以 Rust 当前代码为准，不把 Go 已有能力自动视为 Rust 已支持。

## 扩展指南

- 新增一种 DDL 语句时，先在 [`runtime/dispatch.rs`](dispatch.rs) 补充 AST 分派与隐式提交判定，再在本文件增加会话校验和执行入口；若属于 ALTER，优先扩展 `execute_alter_table_inner` 的精确 `AlterTableType` 分支。
- 若下游支持 persistent action，必须同步 [`runtime/normal_ddl_submit.rs`](normal_ddl_submit.rs) 所需的 action type/参数 ABI，并与 Go `model.Job` 参数形状逐字段核对；不要仅在直接 `Domain::ddl_*` 路径实现，否则两种运行模式行为会分叉。
- 任何会改变多个表的操作都应先完成全部权限、存在性和依赖校验，再以一个 owner transaction/job 提交；RENAME 链和 EXCHANGE PARTITION 是现成范例。
- 会改变物理 table/partition ID 的操作应评估 snapshot、auto ID、region split/scatter、cross-keyspace 记录和 schema version 更新；失败后必须保证 `RuntimeDdlJobGuard` 收束。
- 回归测试应继续放在独立的 [`pkg/session/runtime/ddl_test.rs`](ddl_test.rs)，不能内嵌到生产源文件；还应按行为补充邻近 session runtime 测试。涉及 Go 对齐时同步核对 `pkg/ddl/*_test.go` 的原始边界，但不要为了文档或单一增量递归补齐整个 DDL 子系统。
- 兼容风险集中在 error code/message、IF EXISTS warning、权限访问顺序和 action JSON ABI；性能风险集中在 `scan_registered_table` 的全表唯一/外键校验、分区批量变更和同步预切分。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`files --filter pkg/session/runtime/ddl.rs` 确认目标文件已索引；`node --file ...` 核对源码；对 `execute_create_table`、`execute_alter_table`、`execute_drop_table`、`execute_truncate_table`、`execute_create_database`、`execute_rename_table_pairs`、`execute_add_foreign_key` 执行了 `query`、`callers`、`callees`。图中 callers 缺边，已用 `runtime/dispatch.rs` 源码补证并在上文披露限制。
- 生产源码：[`pkg/session/runtime/ddl.rs`](ddl.rs)、[`pkg/session/runtime.rs`](../runtime.rs)、[`pkg/session/runtime/dispatch.rs`](dispatch.rs)、[`pkg/session/runtime/normal_ddl_submit.rs`](normal_ddl_submit.rs)。
- crate 边界：[`pkg/session/Cargo.toml`](../Cargo.toml) 的 package、feature 和 workspace dependencies。
- 独立 Rust 测试：[`pkg/session/runtime/ddl_test.rs`](ddl_test.rs) 当前覆盖 persisted empty database 的 `IF NOT EXISTS`、temporary table 跳过 split、显式 table/index split policy 覆盖全局关闭，以及 Restore 模式跳过 split。该文件当前只有 6 个直接单元测试，不能据此声称本文列出的全部 DDL 分支均有本地测试覆盖。
- Go 对照：[`pkg/ddl/executor.go`](../../ddl/executor.go) 的 Create/Alter/Drop/Truncate 与 pre-split 入口，以及 [`pkg/ddl/foreign_key.go`](../../ddl/foreign_key.go) 的 `checkTableForeignKeysValid`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、链接/路径和事实一致性。
