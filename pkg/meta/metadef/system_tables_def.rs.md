# `pkg/meta/metadef/system_tables_def.rs`

## 文件定位

本文件属于 `astersql-meta-metadef` crate，是 `mysql` 与 `sys` 持久系统对象的权威 SQL 定义层。crate 根模块 `pkg/meta/metadef/lib.rs` 通过 `pub mod system_tables_def` 声明模块并用 `pub use system_tables_def::*` 重导出全部公开项，因此调用方通常直接以 `astersql_meta_metadef::CreateUserTable` 等路径引用。

它位于“固定对象 ID”与“执行建表”之间：相邻的 `pkg/meta/metadef/system.rs` 为系统表分配稳定 ID，本文件提供与表名对应的完整 DDL；`pkg/session/runtime/session.rs`、`pkg/session/ddl_tables.rs` 等调用方负责解析或执行这些字符串。文件自身不连接存储、不创建 schema，也不实现升级调度。

`pkg/meta/metadef/Cargo.toml` 表明该 crate 仅直接依赖 `astersql-parser-ast` 和 `astersql-parser-mysql`；本文件自身只使用 Rust 静态数据类型，没有第三方 API、feature gate 或条件编译项。

## 核心职责

1. 保存系统对象的完整 SQL 文本。主体是 `Create*Table`、`Create*View` 与 `DropMySQLIndexUsageTable` 等 `pub const`，覆盖权限、统计、GC、绑定、DDL/DXF、TTL、分布式任务、资源控制、导入恢复、物化视图和存储层级迁移等元数据。
2. 用 `BootstrapSystemTableDefinitions` 把 53 个首次 bootstrap 所需的基础 `mysql` 表名与 DDL 配成单一权威清单，避免调用方只保留表名或不完整占位 SQL。
3. 用 `SysSchemaSupportedObjects` 和 MySQL 8 计数/抽样常量表达 AsterSQL 的窄 `sys` 兼容边界：当前声明 `schema_unused_indexes` 视图和 `sys_config` 基表，不把未实现对象注册为空壳。
4. 保持与 `pkg/meta/metadef/system_tables_def.go` 的表结构语义对齐，供 Rust bootstrap 与 Go 路径使用同一类系统表契约。

这里的“权威”限于建表文本和清单关系，不意味着每个常量都会在同一阶段执行。例如基础表由 `BootstrapSystemTableDefinitions` 驱动，DDL/DXF 表由专门的初始化路径使用，`DropMySQLIndexUsageTable` 则是升级清理语句。

## 主要符号

- `SystemTableDefinition { name, create_sql }`：两个字段均为 `&'static str` 的轻量描述符；派生 `Clone`、`Copy`、`Debug`、`PartialEq`、`Eq`，没有方法或可变状态。
- `BootstrapSystemTableDefinitions`：53 项静态切片，按表名关联基础 `mysql` 表 DDL。它从 `user`、`password_history` 等权限表开始，覆盖统计、TTL、任务、恢复、索引顾问、内核选项、工作负载值，结束于 `tidb_masking_policy`。
- 权限与基础元数据 DDL：`CreateUserTable`、`CreateGlobalPrivTable`、`CreateDBTable`、`CreateTablesPrivTable`、`CreateColumnsPrivTable`、`CreateGlobalVariablesTable`、`CreateTiDBTable` 等。`mysql.user` 的列演进受 BR 兼容性约束；权限对象的列名、枚举值、主键和校对规则属于外部兼容契约。
- 统计与优化 DDL：`CreateStatsMetaTable`、`CreateStatsHistogramsTable`、`CreateStatsBucketsTable`、`CreateStatsTopNTable`、`CreateStatsFMSketchTable`、`CreateStatsExtendedTable`、`CreateColumnStatsUsageTable`、`CreateAnalyzeOptionsTable`、`CreateStatsHistoryTable` 等。
- 运维任务 DDL：`CreateTiDBTTL*`、`CreateTiDBGlobalTask*`、`CreateDistFrameworkMetaTable`、`CreateTiDBRunaway*`、`CreateTiDBTimersTable`、`CreateRequestUnitByGroupTable`、`CreateTiDBImportJobsTable`、`CreateTiDBPITRIDMapTable` 和 `CreateTiDBRestoreRegistryTable`。
- DDL/DXF DDL：`CreateTiDBDDLJobTable`、`CreateTiDBReorgTable`、`CreateTiDBDDLHistoryTable`、`CreateTiDBMDLTable`、`CreateTiDBBackgroundSubtask*`、`NotifierTableName`、`CreateTiDBDDLNotifierTable`。这些常量不在基础 53 项切片中，由专门初始化路径管理。
- 物化视图与存储迁移 DDL：`CreateTiDBMViewRefreshInfoTable`、`CreateTiDBMLogPurgeInfoTable`、`CreateTiDBMViewRefreshHistTable`、`CreateTiDBMViewRefreshAlertTable`、`CreateTiDBMLogPurgeHistTable`、`CreateTiDBStorageClassTransitionHistoryTable`。
- `SysSchemaCompatibilityObject { name, object_type, source }` 与 `SysSchemaSupportedObjects`：声明当前可见的两个 `sys` 对象及其来源；`MySQL80NativeSysViewCount`、`MySQL80NativeSysRoutineCount` 和两个 `Unsupported*Samples` 记录未实现边界的规模与抽样。
- `CreateSchemaUnusedIndexesView`、`CreateSysConfigTable`：分别定义 `sys.schema_unused_indexes` 视图与 `sys.sys_config` 基表。前者聚合 `information_schema.cluster_tidb_index_usage`，后者保存工具配置。

全文件共有 78 个公开常量、2 个公开结构体；没有函数、trait、impl、宏定义或条件编译分支。72 个常量的类型是 `&str`，其中 `NotifierTableName` 是名称而非 SQL，其余还包括静态切片和计数常量。

## 执行流程

首次或规范化 bootstrap 的主链见 `pkg/session/runtime/session.rs::BootstrapCanonicalDomain`：

1. 创建 `mysql`、`sys`、`test` 数据库。
2. 遍历 `BootstrapSystemTableDefinitions`；若 domain 目录中缺少 `mysql.<name>`，则把 `create_sql` 交给 session 执行。已有表会跳过；已升级集群还对 `tidb_masking_policy` 保留专门的跳过条件。
3. `ensure_canonical_ddl_system_tables` 单独检查并创建 DDL、MDL、后台子任务、notifier 与物化视图维护表。这解释了这些常量为何没有并入 53 项基础切片。
4. 执行 `CreateSysConfigTable` 并填入默认配置行。
5. 若 `sys.schema_unused_indexes` 尚不存在，`build_bootstrap_view_table` 解析 `CreateSchemaUnusedIndexesView`，再通过 domain DDL 接口持久化视图元数据。

另一条 DDL 初始化链位于 `pkg/session/ddl_tables.rs`：`DDLJobTables`、`MDLTables`、`BackfillTables`、`DDLNotifierTables` 将本文件的 SQL 与 `system.rs` 中的固定 ID 配对；`create_tables` 用 parser 将 SQL 解析成 `CreateTableStmt`、构造表信息、校验解析出的表名，再由 meta mutator 写入。版本门控由 `InitDDLTables` 负责，不在本文件中。

因此本文件的常量求值发生在编译期，真正可能失败的解析、目录检查、建表和持久化均发生在调用方。

## 数据与状态

所有定义都是进程只读的 `'static` 数据，不持有运行时句柄。DDL 字符串使用 Rust 原始字符串，保留 Go 侧 SQL 的引号、注释、换行和大小写；这对默认值、枚举成员、前缀索引、字符集/校对规则和 SQL 注释尤其重要。

主要持久状态可按用途划分为：权限及账号、全局变量/bootstrap 标志、统计与优化器辅助数据、GC/DDL 进度、TTL 和分布式任务状态、runaway/资源组记录、导入及恢复登记、物化视图刷新/清理历史。主键、唯一键和辅助索引直接编码各子系统的不变量，例如任务键唯一性、历史记录排序和恢复登记冲突检测。

`BootstrapSystemTableDefinitions` 的顺序是稳定审查线索，但测试将它转换为集合比较，因此运行正确性主要依赖名称/DDL 一一对应与无重复，而不是切片顺序。固定表 ID 不在本文件内；新增表时必须同时核对 `pkg/meta/metadef/system.rs` 及消费方的表目录。

## 依赖与调用关系

上游消费关系经 RustCodeGraph 文件节点和代码搜索确认：索引直接报告 `pkg/meta/metadef/system_test.rs`、`pkg/session/runtime/session.rs`、`pkg/session/test/bootstraptest/boot_test.rs` 使用本文件；仓库搜索还定位到经 crate 重导出使用常量的 `pkg/session/ddl_tables.rs`、`pkg/session/bootstrap.rs`、`pkg/session/test/meta/session_test.rs` 和 `pkg/session/mysql_sys_schema_compat_test.rs`。

- `pkg/meta/metadef/lib.rs`：模块声明及公开重导出边界。
- `pkg/session/runtime/session.rs`：基础表遍历、专用 DDL 表补建、`sys_config` 建表与 unused-index 视图注册，是 Rust 完整应用中的主要执行入口。
- `pkg/session/ddl_tables.rs`：把 DDL 字符串解析为 AST 并与固定 ID 配对写入 meta。
- `pkg/session/bootstrap.rs`：将部分新增表定义接入版本化 bootstrap schema。
- `pkg/meta/metadef/system.rs`：提供本文件各系统表对应的稳定保留 ID。
- `pkg/meta/metadef/system_tables_def.go`：Go 语义基线；`pkg/session/bootstrap.go` 与 `pkg/session/session.go` 展示 Go 侧执行和 DDL 表接线。

本文件内部没有函数调用边；其“下游依赖”是 SQL 引擎对字符串所含语法、表名、列定义与索引定义的解释。RustCodeGraph 对 `BootstrapSystemTableDefinitions`、`SystemTableDefinition`、`SysSchemaSupportedObjects` 的精确查询找到了定义，但当前索引未为静态常量解析出可供 `callers/callees` 子命令使用的定义节点，因此调用证据以文件级 `used by` 与直接引用搜索互相补足。

## 错误处理与边界

本文件没有返回值和错误类型，无法自行报告无效 SQL。错误在消费点暴露：session 执行失败会附加 `bootstrap mysql.<name> from authoritative DDL` 等上下文；`ddl_tables.rs::create_tables` 会分别报告解析失败、不是 `CREATE TABLE`、解析表名不匹配、构建元数据或持久化失败。

兼容边界包括：

- 多数基础定义采用 `CREATE TABLE IF NOT EXISTS`，但部分 DDL 专用表没有该子句，幂等性由上游的存在检查或版本控制保证。
- `CreateTiDBMDLView` 与 `CreateSchemaUnusedIndexesView` 是视图语句，不应被要求下转为 `CreateTableStmt`；`DropMySQLIndexUsageTable` 是删除语句，只能用于相应升级路径。
- `mysql.user` 的删除/重命名列可能破坏 BR 兼容；`mysql.db`、`tables_priv`、`columns_priv` 的对象名列显式采用大小写不敏感校对，以维持 GRANT/REVOKE 语义。
- `stats_feedback` 虽已废弃仍需建表，属于磁盘格式兼容面，不能仅因运行时不再使用而删除。
- `SysSchemaSupportedObjects` 是允许清单而非 MySQL 8 全量承诺；未支持视图/例程必须保持不存在，而非返回空壳结果。
- 文件没有在编译期校验 SQL，也没有自动保证 `name`、DDL 内表名和固定 ID 三者一致；这些不变量依靠调用方解析和独立测试守护。

## 并发与资源生命周期

两个结构体只含静态字符串并派生 `Copy`；常量切片不可变、无锁、无原子变量、无通道、无异步任务，也不分配或释放资源。并发安全来自不可变静态数据本身。

系统表的真实生命周期由外层 bootstrap/升级协调：先检查目录是否已有对象，再解析/执行 DDL，最后由 domain/meta 层持久化和刷新目录。多节点 bootstrap 的 owner 锁、事务性和重试策略不在本文件中；不能从 `IF NOT EXISTS` 推断所有专用 DDL 都天然支持并发重复执行。表内的 owner、heartbeat、state、timestamp 等列只是其他子系统的持久化协议，本文件不驱动其状态机。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/metadef/system_tables_def.go`。Rust 保留了 Go 中的主要建表/视图/删表常量及 DDL/DXF 分组，包括物化视图维护表和 `tidb_storage_class_transition_history`；`pkg/meta/metadef/system_test.rs::go_merge_12_system_ids_and_sql` 专门检查近期合入字段和六个新增表名。

Rust 侧为运行时接线增加了两类组织信息：

- `SystemTableDefinition` 与 `BootstrapSystemTableDefinitions` 将 Go bootstrap 中分散的 `{Name, SQL}` 表目录集中为 53 项权威清单。
- `SysSchemaCompatibilityObject`、`SysSchemaSupportedObjects`、MySQL 8 原生对象计数及未支持样本明确了 Rust 当前的 `sys` 暴露边界；`CreateSysConfigTable` 对应 Rust bootstrap 需要的 MySQL 8 兼容基表，不是当前 Go 对照文件中的同名常量。

Go 侧 `pkg/session/bootstrap.go` 直接调用 `CreateSchemaUnusedIndexesView`，并在 bootstrap 表列表中组合固定 ID、名称和 SQL；Rust 将相同职责拆到 metadef 清单、`session/bootstrap.rs`、`session/runtime/session.rs` 和 `session/ddl_tables.rs`。这种接线差异不应被误写为表结构语义差异。

## 扩展指南

新增或修改系统表时应按以下边界接入：

1. 在本文件新增或修改完整 DDL，逐列与 `system_tables_def.go` 核对字符集、默认值、可空性、键和注释。若是从 Go 移植，不要缩减字段以只满足当前测试。
2. 若是首次 bootstrap 的基础 `mysql` 表，把唯一的 `{ name, create_sql }` 项加入 `BootstrapSystemTableDefinitions`；若属于 DDL/DXF 版本表，则接入 `pkg/session/ddl_tables.rs` 的相应版本组；特殊升级对象应接到明确的 upgrade 路径，而不是盲目加入基础清单。
3. 为需要固定 ID 的表同步更新 `pkg/meta/metadef/system.rs` 及 `pkg/session/bootstrap.rs` 等目录映射，确保名称、DDL 内限定表名和 ID 一致。
4. 修改 `sys` 暴露面时同步更新 `SysSchemaSupportedObjects`，并在 `pkg/session/mysql_sys_schema_compat_test.rs` 验证目录可见性、对象类型和未支持对象仍缺失。
5. 测试逻辑保持在独立文件：优先扩展 `pkg/meta/metadef/system_test.rs`、`pkg/session/test/bootstraptest/boot_test.rs`、`pkg/session/test/meta/session_test.rs` 或相应 package 测试，不要把 `#[cfg(test)]` 测试内嵌到本生产文件。

主要风险是磁盘 schema/升级兼容、BR 对权限表的读取兼容、重复 bootstrap 的幂等性和新增索引带来的写入/存储成本。修改字段顺序或删除兼容表前需要检查旧集群升级与 Go 路径，不能只验证新集群建表。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter pkg/meta/metadef/system_tables_def.rs` 确认目标已索引；`node --file ... --offset/--limit` 读取完整 1,360 行并报告三个直接使用文件；对 `BootstrapSystemTableDefinitions`、`SystemTableDefinition`、`SysSchemaSupportedObjects` 做了精确 `query`，并尝试 `callers/callees`。自然语言 `explore` 在 30 秒内未返回，常量级 callers/callees 也未解析出定义节点，故用文件级引用和 `rg` 补证。
- 源与边界：`pkg/meta/metadef/system_tables_def.rs`、`pkg/meta/metadef/lib.rs`、`pkg/meta/metadef/Cargo.toml`、`pkg/meta/metadef/system.rs`。
- Rust 调用链：`pkg/session/runtime/session.rs::BootstrapCanonicalDomain`、`ensure_canonical_ddl_system_tables`，以及 `pkg/session/ddl_tables.rs::create_tables`、`InitDDLTables`。
- Go 对照：`pkg/meta/metadef/system_tables_def.go`、`pkg/session/bootstrap.go`、`pkg/session/session.go`。
- 独立测试：`pkg/meta/metadef/system_test.rs::go_merge_12_system_ids_and_sql`；`pkg/session/test/bootstraptest/boot_test.rs::bootstrap_schema_catalog_uses_every_authoritative_system_table_definition_once` 及物化视图 rebootstrap 测试；`pkg/session/mysql_sys_schema_compat_test.rs::sys_schema_and_routine_catalog_respect_compatibility_manifest`；`pkg/session/test/meta/session_test.rs` 的 DDL 表定义/固定 ID 断言。
- 静态清点：源码包含 78 个 `pub const`、2 个 `pub struct`，其中 `BootstrapSystemTableDefinitions` 含 53 项；未发现函数、trait、impl 或条件编译项。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务指定的 11 章节结构检查，并人工复核所有路径、符号和边界陈述。
