# `pkg/ddl/schematracker/dm_tracker.rs`

## 文件定位

本文件实现 `astersql-ddl-schematracker` crate 的内存 DDL 元数据跟踪器。crate 入口 `pkg/ddl/schematracker/lib.rs` 通过 `mod dm_tracker; pub use dm_tracker::*;` 导出这里的公开类型，并同时导出 `InfoStore`、解析器 AST 类型和元数据模型类型。`pkg/ddl/schematracker/Cargo.toml` 将该 crate 标记为 Go 包 `pkg/ddl/schematracker` 的移植，并声明它直接依赖 `astersql-ddl`、`astersql-expression-exprstatic`、`astersql-meta-model` 和 `astersql-parser-ast`。

这里不是 TiDB 集群中的持久化 DDL job 执行器：它不创建 job、不推进 schema state、不做 reorg/backfill，也不更新全局 schema version。它接收已经规整为 Rust 规格类型的操作，在内存 `InfoStore` 中模拟库、表、列、索引和分区元数据的变化，供 `pkg/ddl/schematracker/checker.rs` 在真实执行器执行后镜像同一命令并比较结果。按 DDL 预检问题分类，这是 metadata-only fast path；新建或变更的列、索引直接进入 `StatePublic`，没有持久化检查点、回滚 job、delete-range GC、MDL 或 follower schema sync。

## 核心职责

1. 管理 schema 与表的存在性及基本属性：`CreateSchema*`、`AlterSchema`、`DropSchema`、`CreateTable*`、`CreateView`、`DropTable`、`DropView`。
2. 用 `AlterTableSpec.operations` 按顺序修改一份 `TableInfo` 克隆，覆盖列增删改名、索引增删改名/可见性、主键、注释、字符集、storage class 和分区增删；全部成功后才写回 `InfoStore`。
3. 维护元数据内部引用的一致性：列改名同步索引列名，列删除同步裁剪索引，表达式索引生成/删除隐藏列，`normalize_table` 重算列与索引列偏移。
4. 模拟 Go DM schema tracker 的特定可见行为：批量建表在中途失败时保留成功前缀，多表 DROP 在删除存在对象后汇总缺失对象，若干 DM 不关心的 DDL 入口直接成功。
5. 为 storage class 复用真实 DDL 辅助逻辑：`SetStorageClass`、`SetEngineAttribute` 和 `add_partitions` 调用 `astersql_ddl::storage_class`，而不是自行解释 JSON 或分区继承规则。

本文件只覆盖其 `AlterOperation` 能表达的子集。Go 文件中的 AST 构建、会话 charset/collation 解析、完整合法性检查、物化视图日志和更多 ALTER 选项并没有在这里实现；不能把 Go `ddl.Executor` 的完整能力视为 Rust `SchemaTracker` 的现状。

## 主要符号

- `CreateTableSpec`：一次建表/建视图写入所需的 schema、完整 `model::TableInfo` 和 `if_not_exists`。调用者负责先构造好表元数据。
- `CreateIndexSpec`、`IndexSpec`、`IndexPart`：描述目标表、索引名称、唯一性、可见性，以及普通列或字符串表达式键部件。普通列的 `length == -1` 表示整列；表达式稍后物化为隐藏生成列。
- `ColumnPosition`：新增列的尾部、首部或指定列之后三种位置。
- `AlterOperation`：本文件支持的 ALTER 操作闭集。新增操作必须同时接入此 enum 和 `apply_operation`，否则不能被 `AlterTable` 执行。
- `AlterTableSpec`：目标 schema/table 和有序操作列表。
- `SchemaTracker { pub InfoStore }` 与 `NewSchemaTracker`：公开跟踪器及构造入口；`lower_case_table_names` 原样传给 `NewInfoStore` 决定键规范化策略。
- `SchemaTracker::AlterTable`：核心事务边界。先克隆目标表，检查 `ENGINE_ATTRIBUTE` 与 `STORAGE_CLASS` 互斥，再依次调用 `apply_operation`，最终 `normalize_table` 并一次写回。
- `SchemaTracker::RenameTable`/`renameTable`：先校验并收集所有移动副本，再执行删除和插入的两阶段批量重命名。
- `apply_operation`：`AlterOperation` 到各辅助函数或 storage-class API 的总分派器。
- `add_column`、`drop_column`、`rename_column`、`modify_column`：列元数据变更及其索引引用维护。
- `add_index`、`drop_index`、`rename_index`、`set_index_visibility`：索引元数据变更；表达式索引隐藏列使用 `_V$_{索引名}_{部件序号}` 命名。
- `add_partitions`、`drop_partitions`：分区定义增删。新增分区会通过 `CheckAndUpdateAddedPartitionDefinitions` 计算/校验表达式及 storage class。
- `normalize_table`：重排后重算 `ColumnInfo.Offset` 和 `IndexColumn.Offset`，并把 `StateNone` 的列/索引提升到 `StatePublic`。

`putTableIfNoError` 以及 `addColumn`、`dropColumn`、`alterColumn`、`modifyColumn`、`changeColumn`、`handleModifyColumn`、`renameIndex`、`addTablePartitions`、`dropTablePartitions`、`createPrimaryKey` 等私有方法主要保留 Go 命名和调用形状；当前主分派直接调用模块级辅助函数，其中若干包装器没有生产调用边。

## 执行流程

典型上游链路可由 `pkg/ddl/schematracker/checker.rs` 复核：

1. `NewChecker` 用同一 `lower_case_table_names` 创建 `SchemaTracker`。
2. `Checker` 的公开 DDL 方法先把 `DdlCommand` 交给 `realExecutor.Execute`。
3. 真实执行成功后，`Checker::execute` 或专用方法调用本文件对应的 `SchemaTracker` 方法。
4. `checkDBInfo`/`checkTableInfo` 再读取真实执行器与 `InfoStore`，将 SHOW CREATE 形式归一化后比较。因此本文件是镜像模型，不是最先产生业务副作用的一侧。

一次 `AlterTable` 的内部流程为：从 `InfoStore::TableClonedByName` 获得深拷贝；预扫描操作列表拒绝同时指定 engine attribute 和 storage class；在本地 `TableInfo` 上按输入顺序执行操作；任一步返回错误就丢弃本地克隆；全部成功后规范化 offset/state 并用 `InfoStore::PutTable` 替换旧值。`dm_tracker_test.rs::test_atomic_multi_schema_change` 验证第二步重复加列失败时第一步不会残留。

索引流程中，`add_index` 先生成匿名索引名并检查重名，再解析各键部件。普通列必须存在；表达式部件追加 public hidden column，随后构造 public `IndexInfo`。`drop_index` 先移除索引，再删除该索引引用的隐藏列并规范化。`rename_index` 还会重写符合旧 `_V$_` 前缀的隐藏列名和索引部件名。

分区新增先拒绝非分区表和重名定义，再把新定义放入临时 `PartitionInfo`，调用真实 storage-class 检查/重建函数后追加；删除按名称逐一过滤，遇到不存在名称立即报错。

与单表 ALTER 不同，`BatchCreateTableWithInfo` 逐表直接写入，后项失败不回滚前项；`DropTable` 也会继续删除存在的表，最后才汇总缺失名称。这两种部分成功语义分别由 `batch_create_keeps_successful_prefix_when_a_later_table_fails` 和 `drop_table_removes_existing_tables_before_reporting_missing_ones` 固定。

## 数据与状态

唯一长期状态是 `SchemaTracker.InfoStore`。`InfoStore` 在 `pkg/ddl/schematracker/info_store.rs` 中以两层 `HashMap` 保存 `DBInfo` 和 `TableInfo`；`lowerCaseTableNames == 0` 时使用原始名 `CIStr.O` 作键，否则使用小写名 `CIStr.L`。修改表通常遵循“读取克隆—本地修改—成功写回”，所以旧的 `TableInfo::Clone` 快照不受后续 ALTER 影响，测试 `test_immutable_table_info` 对此有直接断言。

重要不变量包括：列 `Offset` 等于在 `Columns` 中的位置；索引列 `Offset` 指向同名列；本文件新建/修改的列和索引为 `StatePublic`；表至少保留一列；索引名、列名和分区名按 `CIStr.L` 比较；表达式索引的隐藏列带 `Hidden = true` 并保存 `GeneratedExprString`。`CreateTableWithInfo` 则有意保留调用者传入的 schema state，不主动规范化，测试 `create_table_with_info_preserves_caller_supplied_schema_state` 验证了这一点。

本跟踪器不分配真实 DDL job ID、table/column/index ID，也没有磁盘状态。`test_no_num_limit` 和 `test_create_table_long_index` 表明它不会自行施加常规列数、索引数或前缀长度上限；这些检查若需要，应在生成规格的上游或真实执行器中完成。

## 依赖与调用关系

上游直接依赖：

- `pkg/ddl/schematracker/lib.rs` 声明并公开再导出本模块。
- `pkg/ddl/schematracker/checker.rs` 的 `Checker` 持有 `SchemaTracker`；`execute` 将 `DdlCommand` 映射到 Create/Drop/Alter/Rename 方法，并在真实 DDL 执行后比较元数据。
- `pkg/ddl/schematracker/dm_tracker_test.rs` 直接构造规格并覆盖本文件行为。
- RustCodeGraph 的文件关系还列出 `pkg/ddl/storage_class.rs`、`pkg/ddl/storage_class_test.rs`；真正从本文件向下的关键边是 `apply_operation`/`add_partitions` 对 `astersql_ddl::storage_class` API 的调用。

主要下游依赖：

- `InfoStore::{SchemaByName, PutSchema, DeleteSchema, TableByName, TableClonedByName, PutTable, DeleteTable}` 提供查询和提交边界。
- `model::{DBInfo, TableInfo, ColumnInfo, IndexInfo, IndexColumn, PartitionDefinition}` 是被修改的数据模型。
- `ast::CIStr` 同时保留原始名和规范化小写名。
- `astersql_ddl::storage_class::{GetEngineAttributeFromStorageClassTableOptions, handle_create, rebuild_partitions, CheckAndUpdateAddedPartitionDefinitions}` 负责 storage class 语义。
- `astersql_expression_exprstatic::NewExprContext` 为新增分区表达式检查提供表达式上下文。

RustCodeGraph 对 `NewSchemaTracker`、`apply_operation` 和 `BatchCreateTableWithInfo` 的 `callers`/`callees` 查询在当前索引上没有打印边；上述链路因此由图的 file-usage 结果和 `checker.rs`/源码中的直接调用共同核验，而不是据空输出推断“没有调用者”。

## 错误处理与边界

所有可失败操作返回 crate 的 `Error`。存在性错误区分库、表、列、索引和分区；对象类型错误使用 `WrongObject`；storage class 下游的字符串错误映射为 `Error::Mismatch`。`if_exists`/`if_not_exists` 只在对应入口显式处理，不能推广到所有错误：例如 `DropIndex(..., true)` 特别吞掉 schema/table 不存在，`DropView` 遇到存在但不是 view 的对象仍快速失败。

原子性边界需要逐 API 判断：

- `AlterTable` 在克隆上执行，失败不写回，因而对单表操作列表原子。
- `createIndex`/`dropIndex` 和列/索引辅助逻辑也是先改克隆再提交。
- `RenameTable` 在变更前先验证所有目标并收集克隆，但提交阶段是逐项 delete/put；理论上的 `PutTable` 失败仍可能留下部分移动，当前没有事务日志。
- `BatchCreateTableWithInfo` 和多对象 `DropTable` 明确允许部分成功。
- `drop_partitions` 对本地克隆逐名修改；后续名称不存在时整个 `AlterTable` 克隆被丢弃，所以持久状态仍不变。

以下入口当前是有意 no-op 且返回成功：恢复表/库、集群闪回、截断表、表锁、表模式、副本信息、修复表、序列、脱敏策略、放置策略、资源组和刷新元数据等。它们只表示 DM 跟踪器的成功契约，不表示实际集群完成了相应操作。

## 并发与资源生命周期

`SchemaTracker` 的变更方法使用 `&mut self`，`InfoStore` 内部是普通 `HashMap`，本文件没有锁、原子变量、线程、异步任务、channel 或后台资源；调用者必须负责串行可变访问。`Checker` 自身只有启停检查用的 `AtomicBool`，并未把 tracker 变成并发容器。

资源生命周期完全随 Rust 所有权：表修改克隆在栈/堆所有值中暂存，成功时移动进 `InfoStore`，失败时自动丢弃；没有需要显式关闭的句柄。storage class 的表达式上下文按调用临时创建。由于没有持久化层，进程退出或 tracker 被 drop 后全部跟踪状态消失，也不存在 owner failover 或恢复点。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/schematracker/dm_tracker.go`，Rust 测试也以 Go 行为为兼容基准。两者共同点包括：`SchemaTracker` 包装 `InfoStore`；按 `lower_case_table_names` 建立存储；ALTER 前使用表克隆；表达式索引生成 public hidden column；多 schema ALTER 失败恢复旧表；多表 DROP 和批量建表的部分成功语义；大量 DM 不关心的 Executor 方法为 no-op。

Rust 当前是收窄后的模型层，而非逐签名实现 Go `ddl.Executor`：

- Go `CreateSchema`/`CreateTable`/`CreateView` 接收 AST 与 session context，并调用 DDL builder、charset/collation 和合法性检查；Rust 接收已构造的 `DBInfo`/`TableInfo` 或规格。
- Go `AlterTable` 先 `ResolveAlterTableSpec`，支持更多 AST 分支及会话语义；Rust 仅支持 `AlterOperation` 中列出的操作。
- Go 修改列使用 `GetModifiableColumnJob`、ID 分配、位置移动和完整索引标志维护；Rust 直接替换 `ColumnInfo`，保留 offset 并更新索引列名。
- Go 主键路径包含重复主键、表达式主键、生成列、not-null 和 ID 等校验；Rust 用名为 `PRIMARY` 的唯一可见索引模拟。
- Go 分区增删使用完整 builder/checker；Rust 新增分区复用 storage-class 检查，删除只验证存在性。
- Go 已包含物化视图日志及更多 DDL 接口；Rust 本文件没有对应规格和实现。

因此扩展 Rust 时应以 Go 对应方法的实际语义为准，但只能移植任务所需的增量，不能用当前简化接口推断 Go 的完整行为已对齐。

## 扩展指南

新增 ALTER 能力时，优先扩展 `AlterOperation`，在 `apply_operation` 增加唯一分派，并把具体校验/修改放入独立模块级辅助函数；若会改变列顺序、隐藏列或索引引用，成功路径末尾应调用或依赖 `normalize_table`。新增公开 DDL 入口还需同步 `DdlCommand`、`Checker::execute`/专用核对方法与 `pkg/ddl/schematracker/lib.rs` 的错误模型。

涉及 storage class、分区表达式或复杂列/索引合法性时，应复用 `astersql-ddl` 现有辅助函数，避免在 tracker 中另写一套语义。任何可能部分修改多个对象的功能都要先明确原子性：单表操作宜在克隆上完成后一次写回；跨表操作若不能事务提交，必须通过测试记录部分成功或预校验保证。

测试必须继续放在独立的 `pkg/ddl/schematracker/dm_tracker_test.rs`，不要内嵌到生产文件。至少覆盖成功状态、精确错误、失败后的 `InfoStore` 状态、大小写键、引用/offset 维护及 Go 对照边界。需要 AST/session 语义时还应读取并对照 `pkg/ddl/schematracker/dm_tracker_test.go`，而不能只添加能通过当前 Rust 规格的简化测试。

兼容风险主要在错误优先级、IF EXISTS/IF NOT EXISTS、匿名索引/隐藏列命名、操作顺序和部分成功语义；性能风险主要来自每次 ALTER 深克隆整张 `TableInfo`、`normalize_table` 对每个索引列线性查找列位置，以及隐藏列清理中的线性包含检查。除非有基准和行为测试，不要为了优化改变这些可见顺序或原子性。

## 验证依据

- RustCodeGraph `status`：当前仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 `files --filter` 列出 Rust/Go 源及独立测试。
- RustCodeGraph `node --file pkg/ddl/schematracker/dm_tracker.rs`：读取 1–1028 行，确认全部规格类型、公开/私有方法、分派和辅助函数。
- RustCodeGraph `query`：确认 Rust/Go `SchemaTracker`、`NewSchemaTracker`、`AlterTable`、`add_index`、`apply_operation` 和 `BatchCreateTableWithInfo` 的精确位置；对三个 Rust 关键入口执行了 `callers`/`callees`，当前 CLI 未打印边。
- 调用链：`pkg/ddl/schematracker/lib.rs` 的模块再导出；`pkg/ddl/schematracker/checker.rs` 中 `NewChecker`、`execute`、`CreateTable`、`AlterTable` 和元数据核对方法；`pkg/ddl/schematracker/info_store.rs` 的存储与克隆接口。
- crate 边界：`pkg/ddl/schematracker/Cargo.toml`；workspace 成员与依赖别名由根 `Cargo.toml`、`pkg/ddl/Cargo.toml` 的路径声明交叉确认。
- Go 对照：`pkg/ddl/schematracker/dm_tracker.go` 的 `SchemaTracker`、建删库表、索引、列/分区、`AlterTable`、`RenameTable`、no-op 入口及 `BatchCreateTableWithInfo`。
- Rust 测试：`pkg/ddl/schematracker/dm_tracker_test.rs` 的对象类型、原子 ALTER、部分成功、表达式索引、offset/位置、storage class、分区及 no-op 合同；Go 测试路径为 `pkg/ddl/schematracker/dm_tracker_test.go`。
- DDL 范围判断参考 `docs/agents/ddl/README.md`，并以本文件、调用者和测试重新核验：本模块不进入持久 DDL job/state/reorg/schema-sync 主链。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构检查要求文档恰有本页所示 11 个固定二级标题。
