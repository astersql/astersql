# `pkg/importsdk/sql_generator.rs`

## 文件定位

本文件属于 `astersql-importsdk` crate（`pkg/importsdk/Cargo.toml`），实现从扫描结果 [`TableMeta`](model.rs) 和调用方提供的 [`ImportOptions`](model.rs) 生成 TiDB `IMPORT INTO` SQL 的能力。crate 根 `pkg/importsdk/lib.rs` 声明并公开重导出 `sql_generator`；统一门面 `ImportSDK` 在 `pkg/importsdk/sdk.rs` 中持有 `Box<dyn SQLGenerator + Send + Sync>`，由 `NewImportSDK` 调用 `NewSQLGenerator` 完成装配。

它处于“发现数据文件”与“提交导入作业”之间：上游扫描器提供库名、表名和唯一通配路径；本文件把这些元数据和导入选项序列化为 SQL 字符串；作业提交本身由 `JobManager` 负责，本文件不执行 SQL、访问数据库或访问对象存储。

## 核心职责

- 通过 `SQLGenerator` trait 定义可替换、可 mock 的 SQL 生成接口，并用无状态的 `sqlGenerator` 提供默认实现。
- 按固定顺序拼装 `IMPORT INTO <库>.<表> FROM '<路径>' [FORMAT ...] [WITH ...]`，保证输出与 Go `pkg/importsdk/sql_generator.go` 的顺序和文本约定一致。
- 将 `ResourceParameters` 追加到绝对 URL 或 Go `net/url.Parse` 可接受的相对引用中，同时保留已有 query 和 fragment 的相对位置。
- 把通用导入字段和仅限小写 `Format == "csv"` 的 CSV 字段转换为 `WITH` 选项；拒绝多个 `FIELDS_DEFINED_NULL_BY` 值。
- 为标识符、部分 SQL 字符串选项提供与 Go 对应的转义。它不是通用 SQL AST/参数绑定器，不负责验证所有字段是否为合法 SQL、URL 或 TiDB 选项。

## 主要符号

- `pub trait SQLGenerator: Send + Sync`：公开抽象。`GenerateImportSQL(&TableMeta, &ImportOptions) -> Result<String, errors::SharedError>` 是主入口；`GenerateImportSQLParts` 是 Rust 侧的便利适配，成功返回 `(sql, None)`，失败返回 `(String::new(), Some(error))`。
- `pub struct sqlGenerator`：零字段默认实现。类型本身不公开，但通过 trait object 使用。
- `pub fn NewSQLGenerator() -> Box<dyn SQLGenerator>`：构造默认实现；RustCodeGraph 显示直接调用者包括 `NewImportSDK`、`generate_import_sql_matches_go_table` 和 `file_scanner_preserves_storage_scheme_in_import_sql`。
- `sqlGenerator::GenerateImportSQL`：按库表、路径、格式、选项的顺序生成完整 SQL，是本文件的主流程。
- `sqlGenerator::buildOptions`：按结构体字段的固定顺序收集通用选项，并在格式精确等于 `"csv"` 时接入 `buildCSVOptions`。
- `sqlGenerator::buildCSVOptions`：生成字段/行分隔、包围、转义和 NULL 定义选项；多个 NULL 标记返回 `ErrMultipleFieldsDefinedNullBy`。
- `escapeIdentifier`：用反引号包围数据库/表名，并把内部反引号加倍。
- `escapeString`：依次把反斜杠变为双反斜杠、单引号变为两个单引号。
- `isValidRelativeURL`：补足 Rust `url::Url::parse` 只接受绝对 URL而 Go `net/url.Parse` 接受相对引用的差异；拒绝控制字符、畸形 `%xx` 和首个相对路径段中的冒号，但允许 `//` 开头的 scheme-relative 引用。

文件没有模块级常量、枚举、条件编译项或可变静态状态。

## 执行流程

1. `GenerateImportSQL` 创建 `"IMPORT INTO "`，用 `escapeIdentifier` 处理 `TableMeta.Database` 与 `TableMeta.Table`，中间写入点号。
2. 克隆 `TableMeta.WildcardPath` 为局部 `path`。当 `ResourceParameters` 非空时，优先用 `url::Url::parse` 处理绝对 URL：已有非空 query 时用 `&` 追加，否则直接设置 query；`url` crate 负责重新序列化。
3. 若绝对 URL 解析失败但 `isValidRelativeURL` 接受该字符串，则在 fragment 之前写入资源参数：已有非空 query 用 `&`，空 query 直接续写，无 query 用 `?`。若两种检查都失败，保持原路径且不报错，复现 Go “URL 解析失败不阻断生成”的行为。
4. 写入 `FROM '<path>'`；`Format` 非空时原样写入 `FORMAT '<Format>'`。
5. `buildOptions` 按如下稳定顺序追加非默认字段：`THREAD`、`DISK_QUOTA`、`MAX_WRITE_SPEED`、`SPLIT_FILE`、`RECORD_ERRORS`、`DETACHED`、`CLOUD_STORAGE_URI`、`GROUP_KEY`、`SKIP_ROWS`、`CHARACTER_SET`、`CHECKSUM_TABLE`、`DISABLE_TIKV_IMPORT_MODE`、`DISABLE_PRECHECK`。
6. 仅当 `CSVConfig` 存在且 `Format == "csv"` 时，`buildCSVOptions` 继续追加 `FIELDS_TERMINATED_BY`、`FIELDS_ENCLOSED_BY`、`FIELDS_ESCAPED_BY`、`LINES_TERMINATED_BY`、`FIELDS_DEFINED_NULL_BY`。
7. 选项为空时直接返回基础 SQL；否则以 `", "` 连接并加上 `WITH`。CSV 校验错误经 `?` 立即返回，不返回已拼装的半成品 SQL。

## 数据与状态

输入均以共享借用传入，生成器只读取字段。`TableMeta` 中本文件实际使用 `Database`、`Table` 和 `WildcardPath`；`DataFiles`、`TotalSize`、`SchemaFile` 不参与 SQL 生成。`ImportOptions` 的每个生成字段均按“正数、非空或 true 才输出”的规则处理；负数 `Thread`、`RecordErrors` 或 `SkipRows` 与零值一样被忽略。

中间状态只有局部 `String` 和 `Vec<String>`：路径先克隆后修改，选项按固定顺序进入向量，最终创建一个新的 SQL 字符串。输入对象不被修改，函数调用之间也不保存缓存或累积状态。`CSVConfig.FieldNullDefinedBy` 为向量，但 TiDB `IMPORT INTO` 当前只接受零个或一个值。

需要注意，`Format`、路径以及 `DISK_QUOTA`、`MAX_WRITE_SPEED`、`CLOUD_STORAGE_URI` 当前按 Go 实现直接插入单引号上下文；只有 `GROUP_KEY`、`CHARACTER_SET`、`CHECKSUM_TABLE` 和 CSV 字符串字段经过 `escapeString`。这是当前代码契约和潜在输入信任边界，不应在文档中误写为“所有输入都已安全转义”。

## 依赖与调用关系

上游装配链由 RustCodeGraph 和 `pkg/importsdk/sdk.rs` 共同确认：`NewImportSDK` 创建 `NewSQLGenerator()` 并保存到 `ImportSDK.sql_generator`；`impl SQLGenerator for ImportSDK` 将两个生成方法委托给该 trait object。crate 根 `pkg/importsdk/lib.rs` 再将本模块公开 API 重导出。测试及扫描集成点可以直接构造 `NewSQLGenerator`。

本文件的直接 crate 内依赖为 `CSVConfig`（`config.rs`）、`TableMeta`/`ImportOptions`（`model.rs`）和 `ErrMultipleFieldsDefinedNullBy`（`error.rs`）。外部依赖只有 `astersql-errors` 的共享错误类型以及 `url` crate 的绝对 URL 解析/序列化；二者均在 `pkg/importsdk/Cargo.toml` 声明，且该 manifest 没有为本逻辑设置 feature gate。

RustCodeGraph 的主要内部调用边为：`GenerateImportSQL → escapeIdentifier`、`GenerateImportSQL → isValidRelativeURL`、`GenerateImportSQL → buildOptions → buildCSVOptions`，以及两个选项构造器对 `escapeString` 的调用。生成结果可交给 `JobManager::SubmitJob`，但当前仓库中二者是 SDK 暴露的独立能力，本文件没有静态调用提交接口。

## 错误处理与边界

显式业务错误只有 `buildCSVOptions` 在 `FieldNullDefinedBy.len() > 1` 时克隆并返回包级惰性哨兵 `ErrMultipleFieldsDefinedNullBy`，消息为 `IMPORT INTO only supports one FIELDS_DEFINED_NULL_BY value`。该错误经 `buildOptions` 和 `GenerateImportSQL` 的 `?` 原样传播；`GenerateImportSQLParts` 将其转换成 Go 风格的值/错误二元组并保证 SQL 为空串。

URL 解析失败不是错误：只有满足 `isValidRelativeURL` 的相对引用才手工追加参数，包含控制字符、错误百分号编码或非法首段冒号的路径保持原样。资源参数本身被当作已经编码好的 query 片段，不进行键值解析或再编码。路径和若干字符串字段也不统一执行 SQL 字符串转义，因此调用方必须遵守这些字段已有的可信/编码约定。

`Format` 的 CSV 判断区分大小写；`"CSV"` 即使附带 `CSVConfig` 也不会生成 CSV 选项。空字符串和非正数通常表示“不输出”，函数不额外报告无效数值。除多个 NULL 标记外，语法和选项合法性留给后续 TiDB SQL 解析/执行阶段。

## 并发与资源生命周期

`SQLGenerator` 要求 `Send + Sync`，所以 trait object 可作为 `ImportSDK` 的跨线程安全组件；默认 `sqlGenerator` 没有字段和内部可变性，所有工作都发生在调用栈上的局部值中，可并发复用而无需锁。

本文件不创建线程、异步任务、通道、锁、事务、文件句柄、网络连接或数据库会话，也没有 `Drop`/`Close` 责任。输入借用只持续到调用返回，返回值拥有其字符串和错误。`ImportSDK::Close` 关闭的是文件扫描资源，与本生成器无关。

## 与 Go 版本的对应关系

Rust `SQLGenerator`、`sqlGenerator`、`NewSQLGenerator`、`GenerateImportSQL`、`buildOptions`、`buildCSVOptions`、`escapeIdentifier` 和 `escapeString` 分别对应 `pkg/importsdk/sql_generator.go` 的同名接口、实现与辅助函数。SQL 片段顺序、默认字段省略规则、CSV 的小写格式门槛、转义次序和多个 NULL 标记错误均保持一致；`pkg/importsdk/sql_generator_test.rs` 逐表复刻 Go `TestGenerateImportSQL` 的输入和期望。

Rust 有两个为语言/API 适配增加的点：`GenerateImportSQLParts` 把 `Result` 暴露为 Go 风格二元返回；`isValidRelativeURL` 在 `url::Url::parse` 失败时模拟 Go `net/url.Parse` 的相对引用接受范围。Rust 测试额外包含“带 query/fragment 的相对路径追加参数”用例，Go 当前表驱动测试没有该用例，但逻辑对应 Go 标准库行为。

两侧都在 URL 解析错误时静默保留原路径，也都只转义选定的 SQL 字符串字段。Rust 的错误是 `astersql_errors::SharedError`，由 `LazyLock` 哨兵克隆得到；Go 直接返回包级 `error`。这些是类型系统和运行库层面的表达差异，不改变成功 SQL 或错误文案契约。

## 扩展指南

- 新增通用 `IMPORT INTO` 选项时，应先在 `ImportOptions`（`model.rs`）增加字段，再在 `buildOptions` 的 Go 对齐位置插入，避免无意改变现有选项顺序；同步修改 Go 实现/语义依据和独立的 `sql_generator_test.rs` 表驱动用例。
- 新增 CSV 解析选项应进入 `CSVConfig` 与 `buildCSVOptions`，并继续受 `Format == "csv"` 门控。若 TiDB 开始支持多个 NULL 标记，需要同时调整 `ErrMultipleFieldsDefinedNullBy` 契约和错误测试，而不能只删除校验。
- 修改 URL 参数行为时，应同时覆盖绝对 URL、已有/空 query、fragment、相对引用、scheme-relative 引用、非法 `%`、控制字符及首段冒号；尤其要对照 Go `net/url.Parse`，避免 `url` crate 行为差异造成跨语言输出漂移。
- 修改转义策略前应区分标识符、SQL 字符串和已编码 URL query 三种语境，并评估兼容性：给当前未转义字段新增转义可能改变合法历史输入的字面结果，但保持现状又要求调用方提供可信值。
- 测试逻辑继续放在独立的 `pkg/importsdk/sql_generator_test.rs`，不要内嵌到生产源文件；扫描到 SQL 的集成约束位于 `file_scanner_test.rs`。默认实现可替换性还可通过 `pkg/importsdk/mock/sdk_mock.rs` 的 `MockSQLGenerator` 验证。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/importsdk/sql_generator.rs` 已索引为 13 个符号。通过 `files`、`node --file`、`query SqlGenerator` 及各精确符号 `node` 查询核对了定义、源码与调用 trail。
- 生产代码：`pkg/importsdk/sql_generator.rs`（完整 256 行）、`pkg/importsdk/sdk.rs`（构造与委托）、`pkg/importsdk/model.rs`（`TableMeta`/`ImportOptions`）、`pkg/importsdk/config.rs`（`CSVConfig`）、`pkg/importsdk/error.rs`（哨兵错误）、`pkg/importsdk/lib.rs`（模块和重导出）。
- crate 边界：`pkg/importsdk/Cargo.toml` 的 package 名为 `astersql-importsdk`，`[lib]` 指向 `lib.rs`，并声明本文件直接使用的 `astersql-errors` 与 `url = "2"`。
- Go 对照：`pkg/importsdk/sql_generator.go`；相关 Go 测试：`pkg/importsdk/sql_generator_test.go`。两侧主表用例覆盖基础 SQL、资源参数、全部通用选项、CSV 选项、云存储 URI、错误、转义和标识符。
- Rust 独立测试：`pkg/importsdk/sql_generator_test.rs`；直接集成证据：`pkg/importsdk/file_scanner_test.rs::file_scanner_preserves_storage_scheme_in_import_sql`；可替换实现证据：`pkg/importsdk/mock/sdk_mock.rs::MockSQLGenerator`。
- 本任务是只新增说明文档的静态分析，没有运行 Cargo。最终结构检查要求本文恰好包含本计划规定的 11 个二级标题；对内容还人工复核了定位、主流程、调用边、错误、生命周期、Go 对照和扩展入口。
