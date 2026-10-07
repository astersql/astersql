# `pkg/lightning/mydump/schema_import.rs`

## 文件定位

本文件是 `astersql-lightning-mydump` crate 的 schema 执行层。crate 根模块 [`lib.rs`](lib.rs) 以 `mod schema_import; pub use schema_import::*;` 导出这里的公开 API；crate 元数据在 [`Cargo.toml`](Cargo.toml) 中把它对应到 Go 包 `pkg/lightning/mydump`，直接依赖中本文件实际使用 `regex`，其余核心类型通过 `crate::*` 来自同一 crate。

应用侧已验证的直接入口位于 [`pkg/importsdk/file_scanner.rs`](../../importsdk/file_scanner.rs)：`fileScanner::schemaImporter` 用数据库适配器、loader 的 `Storage` 和配置并发度调用 `NewSchemaImporter`，`FileScanner::CreateSchemasAndTables` 随后把 `loader.GetDatabases()` 交给 `SchemaImporter::Run`。RustCodeGraph 将本文件列为被 `pkg/importsdk/file_scanner.rs` 和独立测试 [`schema_import_test.rs`](schema_import_test.rs) 使用。

本文件不负责扫描 dump 目录，也不负责解析视图依赖图；它消费 loader 生成的 `MDDatabaseMeta`，调用 [`view_import.rs`](view_import.rs) 的 `NewSchemaImportPlan` 获得视图拓扑计划，再查询下游对象并执行 DDL。

## 核心职责

- `SchemaImporter::Run` 固定按“数据库 → 物理表 → 视图”三阶段导入，保证视图只在表阶段完成后创建。
- `importDatabases` 查询下游已有数据库，按 ASCII 小写比较并跳过已有项；其余数据库 schema 会经 `createIfNotExistsStmt` 改写后执行。
- `importTables` 从每个 `MDTableMeta` 的 schema 文件读取 SQL。若路径为空，它以 `SHOW CREATE TABLE` 的查询是否成功作为对象已存在的判据；有文件时则改写、规范化并逐句执行。
- `importViews` 用下游对象类型补足视图计划的外部依赖，拒绝缺失依赖和同名非视图冲突，跳过已有视图，并按 `ViewImportPlan::ordered` 串行创建其余视图。
- `createIfNotExistsStmtWithMode` 是本文件的 SQL 文本处理核心：拆分语句，限制可接受的 schema 语句类别，选择性忽略破坏性 DDL，重写数据库、表或视图限定名，并规范化 `CREATE TABLE` 顶层列定义。
- 查询辅助方法把 `SchemaDatabase` 隔离为可替换边界；自由函数包装器保留了对应方法的公开调用形式。

## 主要符号

- `SchemaStmtType`：公开枚举，区分建库、建表、建视图；`Display` 生成执行错误中的操作描述。
- `SchemaJob`：公开任务值，携带 `db_name`、可为空的 `tbl_name`、语句类型和原始 SQL。它不拥有连接或异步状态。
- `SchemaDatabase: Send + Sync`：公开数据库抽象。`execute` 执行单条 SQL，`query` 返回字符串矩阵；`file_scanner.rs` 提供真实适配器，测试提供 `RecordingDatabase`。
- `SchemaImporter`：持有 `Arc<dyn SchemaDatabase>`、`Arc<dyn Storage>` 和 `concurrency`。`NewSchemaImporter` 把零并发度钳制为 1；但当前本文件没有读取 `concurrency`，执行仍为串行。
- `Run`、`importDatabases`、`importTables`、`importViews`：公开的阶段编排与分阶段入口。
- `runJobWithFailedStatement`：内部逐句执行器，失败时保留语句索引，使 `runCreateTableJob` 只对失败的 `CREATE TABLE` 做“表已存在”恢复。
- `runCreateTableJob`、`runCommonJob`、`runJob`：公开执行辅助方法。当前 `importTables` 直接调用 `runJob`，没有调用 `runCreateTableJob`；后者主要由直接调用者或测试使用。
- `getExistingDatabases`、`isTableExist`、`getExistingObjectTypes`、`getExistingSchemas`、`queryStringRows`：下游状态查询方法；文件末尾另有同名自由函数转发器。
- `tableKey`、`collectDumpTables`、`unionTableNames`：构造或合并 `TableName`/`TableNameSet`。其中本文件的视图导入使用 `unionTableNames`，另两个符号供 crate 级调用者复用。
- `createIfNotExistsStmt`、`createIfNotExistsStmtWithMode`：公开 SQL 改写入口；内部依赖 `split_sql`、`normalizeCreateTableColumns`、`quote_ident` 和 `isCreateTableStmt`。
- `String` 与本地 `normalizeTableName`：私有兼容包装，分别转发枚举显示和 `view_import::normalizeTableName`。

## 执行流程

1. `SchemaImporter::Run(dbs)` 调用 `NewSchemaImportPlan(self.store.as_ref(), dbs)`。该函数在 `view_import.rs` 中读取所有视图 schema、抽取依赖、构建拓扑序，并克隆数据库元数据到 `SchemaImportPlan::db_metas`；计划构造失败时尚未执行任何 DDL。
2. `importDatabases` 先通过 `information_schema.SCHEMATA` 得到小写名称集合。未存在的数据库调用 `MDDatabaseMeta::GetSchema`，经 `createIfNotExistsStmt` 把名称重写为目标库且补上 `IF NOT EXISTS`，随后逐句执行。
3. `importTables` 按数据库和表的输入顺序遍历。有 schema 文件时，`MDTableMeta::GetSchema` 从 `Storage` 取内容；`createIfNotExistsStmt` 用 `split_sql` 尊重单引号、双引号和反引号内的分号，验证每条语句类别，再改写表限定名。`normalizeCreateTableColumns` 只拆分表体顶层逗号，为未引用的普通列名加反引号，约束定义、字符串、注释和类型参数内的逗号保持原样。各语句由 `runJobWithFailedStatement` 顺序交给 `SchemaDatabase::execute`。
4. 无 schema 文件的表执行 `SHOW CREATE TABLE <db>.<table>`。在 Rust 当前契约中，只要 `query` 返回 `Ok`，即使行集为空也直接跳过；查询错误会包装为 `checking existing table ...` 后终止。
5. `importViews` 收集计划节点本身、内部依赖和 `external_deps` 涉及的 schema，再由 `getExistingObjectTypes` 把下游对象分为视图与非视图。`validateViewImportPlan` 要求所有外部依赖存在于两类对象的并集。
6. 对 `ViewImportPlan::ordered` 中每个节点：已有视图则跳过；同名非视图则立即报错；否则把 `node.create_sql` 中的视图名称重写为目标限定名，并逐句执行。排序、环检测和外部依赖分类属于 `view_import.rs`，本文件只执行并做下游状态校验。

单独调用 `runCreateTableJob` 时，失败恢复还有一条支路：仅当实际失败索引对应 `CREATE TABLE`，才查询 `isTableExist`；对象存在则吞掉原执行错误，不存在则返回原错误。后续 `SET` 等语句失败不会触发此恢复。

## 数据与状态

`SchemaImporter` 的两个资源句柄均由 `Arc` 共享，构造后不替换；方法全部借用 `&self`。`SchemaJob` 和传入的元数据切片是短生命周期输入，执行器不会持久化队列或检查点。`SchemaImportPlan` 在 `Run` 栈上存在，包含克隆的 `db_metas` 及可选视图图。

名称比较有两套明确规则：查询到的数据库、表和视图名称用 `to_ascii_lowercase`；视图依赖键通过 `view_import::normalizeTableName` 归一化。生成 SQL 时，`quote_ident` 把反引号翻倍；information schema 查询把单引号翻倍；`isTableExist` 的 `LIKE` 表名同样转义单引号。

`concurrency` 会在构造时保存并至少为 1，但当前没有生产方法读取它。因而 `Run` 的三个阶段、各库、各表、各视图和每个任务内的多条语句都是调用线程上的串行过程。它不是完成计数、信号量或线程池大小的现行实现。

## 依赖与调用关系

上游主链为：`pkg/importsdk/file_scanner.rs::CreateSchemasAndTables` → `fileScanner::schemaImporter` → `mydump::NewSchemaImporter` → `SchemaImporter::Run`。同一调用方的 `CreateSchemaAndTableByName` 还会构造仅含指定表的 `MDDatabaseMeta`，因此本文件必须保持单库/单表切片同样有效。

同 crate 下游关系如下：

- loader/元数据层提供 `MDDatabaseMeta`、`MDTableMeta`、`GetSchema` 和 `Storage`；本文件不解释目录布局。
- `view_import.rs::NewSchemaImportPlan`、`validateViewImportPlan` 和 `normalizeTableName` 提供视图图构造、外部依赖验证和统一键格式。
- `MydumpError::Schema` 承载执行、查询和对象冲突错误，`MydumpError::Syntax` 承载本文件文本拆分、白名单和表体完整性错误。
- 外部 crate `regex` 用于识别允许语句、DDL 类型及列标识符；Rust 当前没有依赖 TiDB SQL parser，因此文本级处理能力是明确边界。

`Cargo.toml` 把本目录声明为独立库 `astersql-lightning-mydump`，根 workspace 以 `facade_lightning_mydump` 注册；`pkg/importsdk/Cargo.toml` 通过路径依赖接入它。文档中的主链仅声称已核验的直接使用方，不把其他仅声明依赖的 crate 推断为本文件调用者。

## 错误处理与边界

- `Run` 使用 `?` 短路：计划、建库、建表或视图任一阶段失败都会停止后续工作；已成功执行的 DDL 不回滚。
- `runJobWithFailedStatement` 遇到首个执行错误即停止，并把错误包装为“语句类型 + 库名 + 表名”；没有重试、补偿或批量错误聚合。
- `split_sql` 能避免在三类引号内按分号拆分，并拒绝未闭合引号，但只检查前一个字节是否为反斜杠，且不实现完整 MySQL 转义、注释或 delimiter 语法。
- `createIfNotExistsStmtWithMode` 只接受含 `CREATE`、`DROP`、`SET`、`ALTER`、`USE`、`GRANT`、`REVOKE`、`RENAME`、`TRUNCATE` 的语句或单条版本注释；其他文本返回 `MydumpError::Syntax`。这是正则白名单，不等价于 AST 语法验证。
- `ignore_destructive_ddl` 只跳过以 `DROP TABLE` 或 `DROP DATABASE` 开头的语句。默认入口 `createIfNotExistsStmt` 传 `false`；当前 `importTables` 因此不会自动忽略 dump 中的破坏性 DDL。
- `normalizeCreateTableColumns` 找不到左括号时原样返回；找不到配对右括号时报 `unterminated CREATE TABLE body`。它只规范化顶层普通列，不重写约束定义。
- `getExistingObjectTypes` 忽略少于两列的行；`getExistingSchemas` 忽略空行并只取首列。数据库接口返回的字符串形状由适配器保证，生产代码没有更强的类型检查。
- 视图目标名若已是 view 则幂等跳过；若是任意非 view 对象则硬失败，避免覆盖表或 sequence。缺失的外部依赖在执行视图 SQL 前失败。

## 并发与资源生命周期

`SchemaDatabase` 要求 `Send + Sync`，`db` 和 `store` 使用 `Arc`，因此类型层面允许跨线程共享；但当前实现没有创建线程、任务、channel、锁或异步 future。`concurrency` 只被保存，没有控制任何循环，这一点与 Go 的 worker pool 不同。

连接的申请、释放、重试和事务边界不在本文件中：每次 `execute`/`query` 的资源策略由 `SchemaDatabase` 实现决定。本文件也不显式开启事务，所以跨语句、跨对象和跨阶段没有原子性；错误发生前已经成功的 DDL 会保留。`Storage` 由 `Arc` 保活至 importer 释放，读取 schema 时只通过共享引用访问。

若未来引入并发，必须仍保持三阶段栅栏，并且视图至少遵守拓扑层次；同一 job 的语句顺序不能改变。还需定义首错取消、已经开始的 DDL、连接限额及确定性测试，而不能仅开始读取现有 `concurrency` 字段。

## 与 Go 版本的对应关系

直接对照文件为 [`schema_import.go`](schema_import.go)，Go 回归在 [`schema_import_test.go`](schema_import_test.go)。两版都保留 `SchemaImporter`/job/语句类别、三阶段 `Run`、既有数据库跳过、视图拓扑顺序、下游对象类型验证、逐句执行，以及为目标库表重写 schema SQL 的总体结构。Rust 测试中的 `TestSchemaImporter`、`TestNewSchemaImportPlan`、`TestSchemaImporterManyTables` 和视图占位清理用例对应这些主要意图。

已核验的现行差异包括：

- Go 用 `concurrency` 个 worker 并发导入库和表，Rust 串行且字段未读；Go 的视图阶段也按拓扑总序串行。
- Go 通过 TiDB parser 按配置的 SQL mode 解析并通过 AST restore；Rust 构造器没有 logger、context 或 SQL mode，使用正则与自有拆分器，因此 ANSI_QUOTES、注释、复杂转义及可接受语法范围不完全等价。
- Go `runCreateTableJob` 以 `ignoreDestructiveDDL=true` 过滤 drop，并在解析失败或 `CREATE TABLE` 执行失败时检查既有表；Rust 的 `importTables` 当前直接走默认改写和 `runJob`，不会使用这些恢复分支。公开的 Rust `runCreateTableJob` 仅覆盖执行失败后的既有表检查，也未处理改写失败恢复。
- Go 的无 schema 文件路径通过 `isTableExist` 区分“对象不存在”和其他查询错误；Rust 直接执行 `SHOW CREATE TABLE`，任何 `Ok`（包括空行）都视为存在，任何错误都终止，无法从抽象错误中识别 no-such-table。
- Go `runJob` 获取并关闭连接、通过 `SQLWithRetry` 执行且接受 context 取消；Rust 把这些职责交给 `SchemaDatabase`，本文件自身没有重试、取消或日志任务。
- Rust `loadExistingViewDependencies` 还显式遍历 `external_deps` 的 schema；Go 所示实现遍历节点及 `deps`。两者都最终用 information schema 结果校验外部对象，但查询 schema 集合的形成需保持测试覆盖。

因此扩展或修复时应以 Go 行为为对齐目标，但不能在文档中把尚未接线的 Go 能力表述为 Rust 已支持。

## 扩展指南

- 修改导入阶段或调用顺序：从 `SchemaImporter::Run` 接入，并同步 [`schema_import_test.rs`](schema_import_test.rs) 的端到端顺序断言；视图依赖规则应改在 `view_import.rs` 及其独立 `view_import_test.rs`，不要在本文件重建第二套图算法。
- 补齐 Go 并发语义：使用现有 `concurrency`，保留阶段栅栏、单 job 语句顺序及首错传播；新增并发上限、取消和部分执行测试。不能仅并发整个 `Run`，否则视图可能早于表。
- 补齐 SQL parser/SQL mode 行为：优先替换 `createIfNotExistsStmtWithMode` 的文本级识别边界，并针对引号、注释、多个 statement、drop 过滤、复杂列定义及 AST restore 差异扩充独立测试；注意 crate 当前只有 `regex` 直接依赖，新增依赖会扩大 Cargo 变更范围。
- 修正表错误恢复：统一 `importTables` 与 `runCreateTableJob` 的接线，覆盖解析失败、首条 `CREATE TABLE` 失败、后续 session directive 失败、对象存在/不存在和查询失败。不要吞掉非建表语句错误。
- 新增下游对象查询：通过 `SchemaDatabase` 扩展可测试边界，并继续转义标识符/字符串；若改变行形状，需同步 `getExistingSchemas` 或 `getExistingObjectTypes` 的防御逻辑。
- Rust 单元测试必须继续放在独立的 `schema_import_test.rs`，由 `lib.rs` 的 `#[path]` 挂载；不要把测试嵌入生产文件。任何行为移植都应同时核对 `schema_import.go` 和 `schema_import_test.go`，避免用简化实现替代 Go 语义。

兼容性风险主要是 SQL mode 与错误分类，正确性风险主要是破坏性 DDL、部分执行和视图依赖，性能风险主要是当前串行导入在大量表上的时延。`TestSchemaImporterManyTables` 只验证执行数量，不证明配置并发度已经生效。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`node --file pkg/lightning/mydump/schema_import.rs --offset 1 --limit 500` 与后续 `--offset 495` 覆盖本文件 571 行；`query` 确认 Rust `NewSchemaImporter` 与 `createIfNotExistsStmtWithMode` 的签名和符号 ID；文件关系显示直接使用方为 `pkg/importsdk/file_scanner.rs` 与 `pkg/lightning/mydump/schema_import_test.rs`。`callers`/`callees` 查询在限定时间内未返回，因此调用链结论改由直接入口源码核验。
- 生产源码：[`schema_import.rs`](schema_import.rs)、[`lib.rs`](lib.rs)、[`view_import.rs`](view_import.rs)、[`pkg/importsdk/file_scanner.rs`](../../importsdk/file_scanner.rs)。
- crate/工作区证据：[`Cargo.toml`](Cargo.toml)、仓库根 `Cargo.toml` 的 `facade_lightning_mydump` 路径项、`pkg/importsdk/Cargo.toml` 的 `astersql-lightning-mydump` 路径依赖。
- Rust 独立测试：[`schema_import_test.rs`](schema_import_test.rs)，覆盖建表执行失败恢复、后续语句失败、三阶段与视图顺序、延迟视图校验、大量表计数、SQL 改写与语法错误、已有数据库/无 schema 表、集合归一化、空语句、已有视图及非视图冲突。
- Go 对照：[`schema_import.go`](schema_import.go) 与 [`schema_import_test.go`](schema_import_test.go)，用于核验 worker pool、parser/SQL mode、重试/连接、表存在恢复和对应回归意图。
- 本任务是纯文档分析，未运行 Cargo；交付前使用任务指定命令确认本文档存在且恰有 11 个固定二级章节，并人工复核未把未验证的图调用边或 Go 能力写成 Rust 当前事实。
