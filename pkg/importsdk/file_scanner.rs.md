# `pkg/importsdk/file_scanner.rs`

## 文件定位

本文件是 `astersql-importsdk` crate 的数据源扫描实现，crate 边界由
[`pkg/importsdk/Cargo.toml`](Cargo.toml) 定义，模块由 [`lib.rs`](lib.rs) 中的
`mod file_scanner` 装配并通过 `pub use file_scanner::*` 导出。它位于导入 SDK 的前半段：
调用方先通过 `NewImportSDK`（[`sdk.rs`](sdk.rs)）间接创建 `NewFileScanner`，扫描
MyDumper/Aurora 风格的外部文件并得到 `TableMeta`，后续再由 SQL 生成器生成
`IMPORT INTO`，或由 schema importer 在目标数据库执行建库建表。

该文件既是对外 API 边界，也是多个子系统的适配层：向下连接 `astersql-objstore`
（外部存储）、`astersql-lightning-mydump`（文件发现和 schema 导入）、
`astersql-executor-importer`（KV 大小采样）、parser/DDL/meta（从 schema SQL 构造
`TableInfo`）；向上由 `ImportSDK` 委托全部 `FileScanner` 方法。它不是薄门面：URL
脱敏、文件元数据组装、通配路径、大小估算及资源关闭逻辑都在这里实现。

## 核心职责

1. `NewFileScanner` 解析并脱敏源 URL，创建外部存储和 `MDLoader`，把扫描并发、最大
   文件数、文件路由、字符集、Aurora 自动映射及真实大小估算等 `SDKConfig` 选项传给
   loader。
2. `FileScanner` trait 统一暴露建库建表、表元数据查询、源数据总量、导入后 TiKV
   体量估算和关闭资源；`fileScanner` 保存实现这些操作所需的存储、loader、数据库和
   配置状态。
3. `LoaderStorage` 与 `SchemaDatabaseAdapter` 分别把 objstore 和 `JobDatabase` 适配为
   mydump 所需接口；前者支持目录遍历和整文件解压，后者执行 DDL、查询 schema 状态并
   保证行游标关闭。
4. `buildTableMeta`、`processDataFiles`、`createDataFileMeta` 将 loader 元数据转换为
   SDK 模型，计算文件大小，并为 `IMPORT INTO` 生成不改变存储 provider scheme 的
   通配路径。
5. `estimateOneTableSize` 读取并解析建表 SQL、构造公开状态的 `TableInfo`、采样文件
   编码后的 KV 大小，再按“全量源大小 / 样本源大小”外推单副本 TiKV 占用。

## 主要符号

- `FileScanner: Send + Sync`：公开扫描接口。三个 `*Parts` 默认方法把 Rust `Result`
  转成 Go 风格的“值与错误分离”返回；核心方法仍以 `Result` 表达成功或失败。
- `fileScanner`：具体实现，字段包括脱敏后的 `redacted_source_path`、目标数据库 `db`、
  可被 `Close` 取走的 `store: Option<StorageRef>`、`loader`、`logger`、配置和启动时固化的
  `aurora_source` 标记。
- `NewFileScanner`：唯一公开构造函数。loader 创建失败时先关闭已经创建的 store，成功
  后返回 `Box<dyn FileScanner>`。
- `LoaderStorage` / `compressionType`：实现 `mydump::Storage`；`open` 根据
  `Compression` 包装解压 reader，`list` 用 `WalkDir` 返回路径和大小。未知整文件压缩
  类型返回 `MydumpError::Configuration`。
- `SchemaDatabaseAdapter`：实现 `mydump::SchemaDatabase`。`execute` 防御性忽略
  `DROP TABLE`，`query` 把 `SQLValue` 转为字符串矩阵并无论读取成功与否都调用
  `JobRows::Close`。
- `ScannerKVSizeParserService`：实现 `KVSizeParserService::NewParser`，根据采样配置生成
  CSV 配置，并通过与 loader 相同的 `LoaderStorage` 打开样本文件。
- `schemaImporter`、`CreateSchemasAndTables`、`CreateSchemaAndTableByName`：构造并运行
  `SchemaImporter`；后者只克隆选中表并清空 views，避免创建未请求对象。
- `GetTableMetas` / `GetTableMetaByName` / `GetTotalSize`：分别枚举全部表、精确查找单表、
  累加所有数据文件大小。
- `EstimateImportDataSize` / `estimateOneTableSize` / `buildEstimateSampleConfig` /
  `buildEstimateTableInfo`：形成大小估算主链。
- `buildEstimateCreateTableStmt` / `estimateCreateTableStmtMatchesMeta`：从可能包含多条
  SQL 的 schema 中选择目标 `CREATE TABLE`；匹配时表名和可选 schema 都忽略大小写。
- `buildWildcardPath` / `encodeAuroraWildcardPath`：连接存储 URI 与 glob；Aurora 远端路径
  会将已有 `%` 先变成 `%25`，使对象键中的百分号序列经过 URL 解析后仍保持字面含义。
- `sourceTypeToImportFormat`：只接受 CSV、SQL、Parquet，返回 importer 使用的格式名。
- `IMPORTANT_VARIABLE_DEFAULTS` / `IMPORT_VARIABLE_DEFAULTS`：为采样补齐与 Lightning /
  IMPORT 行为一致的系统变量默认值。

## 执行流程

构造阶段从 `NewImportSDK -> NewFileScanner` 开始。`NewFileScanner` 先调用 `redactURL`
生成只用于日志和错误的安全路径；若输入甚至不能被 `url::Url` 解析，则解析错误只显示
`<redacted-invalid-source>`。随后 `objstore::parse::ParseBackend` 解析真实源地址，
`objstore::storage::New` 创建存储。根据 `SDKConfig` 构造 `LoaderConfig` 和可选 loader
参数，最后 `NewLoaderWithStore` 扫描文件并判断是否为 Aurora 来源。

元数据查询时，`GetTableMetas` 遍历 `loader.GetDatabases()` 的库表，逐表调用
`buildTableMeta`。该函数把每个 `FileInfo` 转为 `DataFileMeta`，依据配置选择压缩后的存储
大小或估算解压大小；无数据文件时返回不带通配路径的合法元数据。存在数据文件时先由
`generateWildcardPath` 求能唯一覆盖该表文件的 glob，再将 store URI 与 glob 连接；本地
`file://` 被去掉，远端 scheme（如 `s3://`、`oss://`）被保留，Aurora 路径额外编码一次。

建库建表时，`CreateSchemasAndTables` 要求 loader 至少发现一个数据库，然后将全部
`MDDatabaseMeta` 交给 `SchemaImporter::Run`。按名创建先定位精确 schema/table，只提交
该表；找不到表与找不到 schema 使用不同哨兵错误。执行 SQL 经过
`SchemaDatabaseAdapter`，其中 `DROP TABLE` 不下发，查询结果会完整消费并关闭游标。

估算阶段由 `EstimateImportDataSize` 逐表调用 `estimateOneTableSize`：

1. 空表直接返回 0；缺 schema 文件立即报错；首个数据文件的类型决定导入格式。
2. `buildEstimateTableInfo` 读取 schema，按配置 SQL mode 解析，并选择匹配的
   `CREATE TABLE` 后调用 `ddl::BuildTableInfoFromAST`。
3. Rust DDL builder 新建的列和索引状态被显式提升为 `StatePublic`；若
   `PKIsHandle` 但没有独立主键索引，则从带主键 flag 的列补出 `PRIMARY` `IndexInfo`，使
   Rust sampler 与 Go `MockTableFromMeta` 的可见元数据一致。
4. `SampleFileImportKVSizeWithTableInfo` 用 schema、CSV/SQL 配置和源文件做实际样本解析。
   样本源大小与 KV 大小都为零时结果为零；只有一侧非正数时退回源文件总大小；否则按
   样本 KV/源大小比例外推全表大小。
5. 外层累加每表 `SourceSize`、`TiKVSize` 及全局总量。

## 数据与状态

`fileScanner` 的长期状态在构造完成后基本只读：`db`、`loader`、`logger`、`config` 和
`aurora_source` 在所有操作间复用。唯一显式状态转移是 `store: Some -> None`；
`Close` 使用 `take`，因此重复关闭是幂等的。依赖 store 的 `buildTableMeta` 和
`estimateOneTableSize` 假定调用发生在 `Close` 前，并以 `expect` 表达此生命周期不变量。

大小字段有两种来源。`fileRealSize` 优先使用 loader 已填充的 `real_size`，测试或未填充
场景回退 `file_size`；实例方法 `dataFileSize` 则尊重 `estimate_real_size`，未压缩文件或
显式关闭估算时直接用 `file_size`，压缩文件调用 `EstimateRealSizeForFile`。因此
`GetTotalSize`、`TableMeta::TotalSize` 和估算结果都遵循当前 scanner 配置，而公开辅助函数
`processDataFiles` 使用传入元数据已有的真实大小语义。

`buildEstimateSampleConfig` 从 `SDKConfig.csv_config` 深拷贝字段；空 null 定义默认
`\\N`，启用反斜杠转义但未给转义符时默认 `\\`，CSV 有表头时 `IgnoreLines=1`。系统变量
映射在每次估算配置构造时新建，不与调用者共享可变状态。

## 依赖与调用关系

已确认的上游链是 `pkg/importsdk/sdk.rs::NewImportSDK -> NewFileScanner`；`ImportSDK` 的
`FileScanner` 实现逐项转发到内部 trait object。更上层的真实调用包括
`lightning/pkg/importinto/importer.rs` 对 SDK `GetTableMetas` 的消费，以及
`tests/realtikvtest/importintotest/sdk_test.rs` 的集成覆盖。`lib.rs` 将本文件公开符号重新导出。

主要下游关系如下：

- 构造：`NewFileScanner -> objstore::ParseBackend/storage::New -> mydump::NewLoaderWithStore`。
- schema：`Create* -> schemaImporter -> mydump::NewSchemaImporter/Run ->
  SchemaDatabaseAdapter::{execute,query} -> JobDatabase`。
- 元数据：`GetTableMetas -> buildTableMeta -> createDataFileMeta/dataFileSize ->
  generateWildcardPath -> buildWildcardPath`。
- 估算：`EstimateImportDataSize -> estimateOneTableSize -> buildEstimateTableInfo ->
  parser::ParseSQL -> buildEstimateCreateTableStmt -> ddl::BuildTableInfoFromAST ->
  execimporter::SampleFileImportKVSizeWithTableInfo`。
- 文件读取：`MDLoader` 和 `ScannerKVSizeParserService` 都经 `LoaderStorage` 访问同一
  `StorageRef`，压缩输入再委托 `objstore::objectio::compressedio::new_reader`。

`Cargo.toml` 直接声明了上述 DDL、errors、executor-importer、lightning mydump/log、meta、
objstore、parser/AST 及 `url` 依赖；独立测试另使用 `flate2` 和 `tempfile`。RustCodeGraph
索引能定位本文件及 61 个符号，但本次精确 ID 的 `callers/callees` 查询未在 30 秒内返回，
所以上述跨文件入口另由模块源码和仓库引用搜索核验，而非把无结果解释成“没有调用者”。

## 错误处理与边界

构造错误使用 `ErrParseStorageURL`、`ErrCreateExternalStorage`、`ErrCreateLoader` 等共享哨兵
并通过 `annotate` 添加安全上下文。敏感 URL 参数按协议和大小写归一化后替换为
`xxxxxx`；无法解析的原始字符串不进入 outward-facing parse error。loader 初始化失败会
关闭已打开的 store，避免构造半成品泄漏资源。

扫描边界包括：无数据库时建 schema 和大小估算都返回 `ErrNoDatabasesFound`；按名创建
区分 `ErrSchemaNotFound` 与 `ErrTableNotFound`，按名取元数据统一为后者；空数据表仍返回
元数据但没有 wildcard；Aurora store 前缀含 glob 元字符时拒绝拼接，防止前缀被误当模式。
`skip_invalid_files` 只在非 Aurora 来源生效：坏表会记录 warning 并跳过，Aurora 的缺
schema 等错误始终向上传播。

schema 选择允许文件中夹杂 `CREATE DATABASE`、`USE`、`DROP` 等语句；恰有一个
`CREATE TABLE` 时可直接使用，多个时必须有一个名称匹配，否则报告语句数、schema 文件和
目标表。`sourceTypeToImportFormat` 拒绝未知格式。对象存储和解压错误被转换为
`MydumpError`，再在公开边界转换为 `SharedError`；转换保留文字上下文，但不保留原错误
的具体 Rust 类型。

## 并发与资源生命周期

`FileScanner` 要求实现 `Send + Sync`，数据库和存储通过 `Arc` 共享。扫描并发度来自
`SDKConfig.concurrency`：正数传给 loader 的 `WithScanFileConcurrency`，schema importer
至少使用 1；本文件不创建显式线程、任务或通道，实际并发由 mydump/objstore 下游实现。

store 的所有权由 scanner 和若干 `Arc` 适配器共享。构造 loader 时克隆 `StorageRef`；
采样 parser service 也临时克隆它。`Close` 关闭 scanner 保存的 handle 并置空，且不会因
重复调用报错。公开方法接收 `ctx: &dyn Any` 以维持 SDK 接口，但当前实现多数未向下传播：
objstore 使用 background context，schema adapter 给 `JobDatabase` 传 `&()`。因此该文件
当前没有基于调用方 context 的取消保证，这是扩展取消语义时必须正视的边界。

`SchemaDatabaseAdapter::query` 对行读取和关闭分别取结果：读取失败优先返回读取错误，读取
成功但关闭失败则返回关闭错误，确保游标关闭不是静默的。`fileScanner` 没有 `Drop` 实现，
调用者应显式调用 `FileScanner::Close`/`SDK::Close`；`ImportSDK::Close` 已明确委托这里。

## 与 Go 版本的对应关系

直接对照文件是 [`file_scanner.go`](file_scanner.go)，独立测试分别为
[`file_scanner_test.rs`](file_scanner_test.rs) 和
[`file_scanner_test.go`](file_scanner_test.go)。公开接口、构造流程、loader 选项、表元数据、
坏表跳过、schema 选择和 TiKV 样本比例外推均保持 Go 语义。

Rust 为适配现有 crate 边界增加了明确接线：Go 直接使用 `sql.DB`、store 和
`SampleFileImportKVSize`，Rust 使用 `JobDatabase`、`LoaderStorage`、
`ScannerKVSizeParserService` 及 `SampleFileImportKVSizeWithTableInfo`；Rust 还补齐列/索引
`StatePublic` 和 PK-handle 的主键索引元数据，使 sampler 看见与 Go 表对象相同的结构。
Go schema importer 接收 logger、SQL mode 和 context，Rust importer 通过
`SchemaDatabaseAdapter` 接线，并额外过滤可能被转发的 `DROP TABLE`。

两处可观察的实现差异需保留在维护视野内：Go 在 loader 返回“文件过多”且仍有 loader 时
允许继续返回 scanner，而 Rust 当前把任何 `NewLoaderWithStore` 错误都视为构造失败；Rust
当前未把调用方 context 传入存储、数据库和 importer。另一个细节是 Rust 的
`fileRealSize` 在 `real_size == 0` 时回退 `file_size`，以支持手工测试元数据和未压缩文件，
而 Go 辅助函数直接读取 loader 填充的 `RealSize`。

Rust 独立测试没有照抄 Go `sqlmock` 的 SQL 字面量，而以 `CanonicalDatabase` 捕获当前 Rust
schema importer 的真实语句；它仍验证相同意图：CREATE DATABASE/TABLE 被执行、DROP
TABLE 被忽略、未知表报错。其余测试覆盖 Go 中的数据文件汇总、路径脱敏、真实压缩大小、
`skip_invalid_files`、CSV/SQL KV 采样、多语句 schema、Aurora 自动映射及远端路径编码。

## 扩展指南

- 新增源文件格式时，同时修改 `sourceTypeToImportFormat`、`compressionType`（若涉及新的
  整文件压缩）、parser service 配置，并在 `file_scanner_test.rs` 添加正常与不支持分支；
  必须与 Go `file_scanner.go`/测试核对格式名和错误语义。
- 修改文件路由或 wildcard 时，从 `buildTableMeta -> generateWildcardPath ->
  buildWildcardPath/encodeAuroraWildcardPath` 接入，分别测试本地绝对路径、各 provider
  scheme、glob 元字符、字面 `%`、空数据文件和歧义路由；避免对远端对象键重复编码。
- 修改大小估算时保持三项不变量：schema 对应正确表、列/索引对 sampler 可见、全量结果按
  样本 KV/源大小比例外推。同步覆盖无数据、空样本、非法 schema、CSV header、索引增量、
  PK-handle 和 `skip_invalid_files`。
- 引入真正可取消的 context 时，需要同时调整公开 trait、objstore context、
  `SchemaDatabaseAdapter` 和 importer/sampler 调用，不能只改一个入口；评估 API 兼容性和
  后台任务提前退出时的 store/rows 清理。
- 改动 `Close` 或 store 持有方式时，应消除当前“Close 后不得扫描”的 `expect` 假设，或在
  公开 API 上返回可诊断错误；补充重复关闭、关闭后调用和构造中途失败的独立测试。
- 本仓库约定 Rust 测试与生产源码分离；所有回归继续放在同目录
  `pkg/importsdk/file_scanner_test.rs`，不要嵌入本文件。兼容风险主要在 Go 行为偏移和错误
  分类，性能风险主要在全量目录遍历、压缩真实大小估算、schema 解析以及逐表 KV 采样。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/importsdk`
  确认目标、Go 对照和独立测试均已索引；`node --file pkg/importsdk/file_scanner.rs` 读取了
  971 行目标源码并显示 61 个符号。对 `NewFileScanner` 的 `query --json` 精确定位到 Rust
  函数 `function:fd555917014dae6b037f6cef9fc68621`（第 328 行）；精确 ID 的
  `callers/callees` 在 30 秒内无输出，未据此作否定结论。
- 生产源码：完整检查 `pkg/importsdk/file_scanner.rs`；检查 `pkg/importsdk/sdk.rs` 的
  `NewImportSDK` 与全量委托实现、`pkg/importsdk/lib.rs` 的模块装配和再导出。
- crate/调用证据：检查 `pkg/importsdk/Cargo.toml` 的直接及测试依赖；仓库引用搜索确认
  `pkg/importsdk/sdk.rs`、`lightning/pkg/importinto/importer.rs` 与
  `tests/realtikvtest/importintotest/sdk_test.rs` 的上游使用。
- Go 对照：完整检查 `pkg/importsdk/file_scanner.go`，逐段核对 trait/API、构造、schema、
  元数据、大小估算、schema 选择和格式映射。
- 测试证据：完整检查 `pkg/importsdk/file_scanner_test.rs`，并用
  `pkg/importsdk/file_scanner_test.go` 的测试清单核对 Go 原始意图；覆盖数据大小、scheme、
  Aurora、脱敏、DDL、DROP 过滤、KV 采样、压缩真实大小和坏表跳过。
- 本任务只新增说明文档，不运行 Cargo；交付结构检查要求本文恰好包含本计划规定的 11 个
  二级章节，并人工复核没有把图查询超时、Go/Rust 差异或下游并发实现写成未经验证事实。
