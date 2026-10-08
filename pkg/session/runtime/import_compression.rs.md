# `pkg/session/runtime/import_compression.rs`

## 文件定位

本文件属于 `astersql-session` crate 的具体会话运行时，由 `pkg/session/runtime.rs` 以私有模块 `import_compression` 装配。它实现的是 `IMPORT INTO ... FROM '<对象路径>'` 的同步、非 `split_file` 文件导入分支：`pkg/session/runtime/dispatch.rs` 识别 `ast::ImportIntoStmt` 并完成物化视图日志表、TTL 表和 `IMPORT INTO ... SELECT` 分流检查后，调用 `ConcreteSession::execute_import_file`；`pkg/session/runtime/import_file.rs::execute_import_file` 在选项中没有 `split_file` 时再调用本文件的 `ConcreteSession::execute_import_compression`。

该分支把已注入的对象存储作为输入边界，负责文件匹配、按扩展名解压、CSV/SQL dump 解析、行值转换和一次性会话插入。它不是 Go 生产版分布式 IMPORT INTO 的完整调度器，也不创建导入任务、引擎或 SST；带 `split_file` 的路径由 `pkg/session/runtime/import_file.rs` 的其余实现处理。

## 核心职责

- `execute_import_compression` 校验该同步路径支持的 AST 形态和选项，并从 `ConcreteSession.import_files.storage` 取得对象存储。
- `source_matches` 对存储列举结果执行轻量 `*` 通配匹配，同时要求模式与候选路径的查询串完全相等。
- 根据文件名末尾的 `.gz`/`.gzip`、`.zst`/`.zstd` 或 `.snappy` 选择 `astersql_objstore_compressedio` 解码器；无这些后缀时按未压缩输入处理。
- 根据显式 `statement.Format` 或去掉压缩后缀后的 `.sql` 扩展名选择 Lightning mydump 的 CSV 或 SQL chunk parser。
- 将解析结果转换成 `ast::ValueExpr` 列表，所有匹配文件成功解析且关闭 parser 后，只调用一次 `ConcreteSession::execute_insert`，并返回单列 `Imported_Rows` 结果集。

## 主要符号

- `fn import_error(error: impl Display) -> SessionError`：统一把底层显示文本包装为当前 session 运行时的错误类型。它不保留具体底层错误类型，只保留字符串。
- `fn source_matches(pattern: &str, source: &str) -> bool`：私有字节级动态规划匹配器。它把第一个 `?` 之前视为路径、之后视为查询串；查询串必须完全相等，路径中的 `*` 可匹配任意长度字节序列，其他字节必须逐字相等。算法使用两个长度为 `source.len() + 1` 的布尔向量，空间复杂度为 `O(|source|)`，时间复杂度为 `O(|pattern| * |source|)`。
- `pub(super) fn ConcreteSession::execute_import_compression(&self, statement: &ast::ImportIntoStmt) -> SessionResult<ConcreteRecordSet>`：模块内可见的唯一入口。它读取会话当前数据库、运行时表元数据和注入存储，最终构造 `ast::InsertStmt` 复用正常 DML 写路径。
- `#[cfg(test)] #[path = "import_compression_test.rs"] mod tests`：测试逻辑放在独立文件 `pkg/session/runtime/import_compression_test.rs`，没有与生产实现混写。

## 执行流程

1. 入口先拒绝 `SELECT`、列赋值或列/用户变量映射；这些形态在本分支会返回 `file import column mapping is not configured`。其中 `SELECT` 正常情况下已由 `dispatch.rs` 分流到 `execute_import_query`，此处仍作防御性校验。
2. 遍历 `statement.Options`，通过 `crate::dml_runtime::EvalExpr` 在空变量映射下求值。支持 `thread`、`skip_rows` 和四个 CSV 分隔/包围/转义选项；`thread` 只校验为正整数，并不在本实现内创建并行工作线程；未知选项立即报错。
3. 从 `ImportFiles.storage` 克隆存储句柄，调用 `Storage::list`，用 `source_matches` 过滤并按路径字典序排序。没有匹配项时返回源文件不存在错误。
4. 用显式 schema 或 `current_database()` 解析目标表；找不到表即失败。
5. 依序处理每个匹配文件：从路径去掉查询串并转小写，识别压缩后缀，同时保留去掉压缩后缀后的名字用于格式推断；随后以 `dump::Compression::None` 打开原始对象，因为实际解压由本文件显式完成。
6. 压缩文件交给 `new_reader`，Zstd 解码并发固定为 `1`；然后 `read_to_end` 将单个文件完整解压到内存。显式 `FORMAT` 优先，否则基础文件名以 `.sql` 结尾时选 SQL，其余选 CSV。
7. CSV 使用配置后的 `dump::NewCSVParser`，SQL 使用块大小 `64 * 1024` 的 `dump::NewChunkParser`。循环调用 `ReadRow` 直到 `dump::Error::Eof`；`skip_rows` 对每个文件分别从零计数。
8. 每行必须与目标表非隐藏列数量完全相等。`Null` 转为无值，`I64` 转十进制字符串，`Bytes`/`Binary` 必须能转成 UTF-8；之后统一构造字符集 `utf8mb4`、排序规则 `utf8mb4_bin` 的值表达式并追加到内存中的 `lists`。
9. 每个 parser 无论解析闭包成功与否都会调用一次 `Close`；实现先保存解析结果和关闭结果，再按“解析错误优先、关闭错误其次”的顺序传播。
10. 所有文件都成功后，若至少有一行，则构造无显式列清单的 `ast::InsertStmt`，一次调用 `execute_insert`。最后返回 `Imported_Rows = lists.len()`；零行输入不会执行 insert，但仍成功返回 `0`。

## 数据与状态

`execute_import_compression` 本身不持有跨调用状态。持久输入边界是 `ConcreteSession.import_files: RefCell<ImportFiles>` 中的 `storage`，该句柄在函数开始阶段被克隆，借用随即结束；目标 schema 来自会话当前数据库或语句显式 schema，目标列数来自 `resolve_runtime_table` 的运行时表元数据。

每次调用创建一个 `dump::CsvConfig`、匹配文件列表、单文件解压缓冲区和跨文件的 `lists`。文件按路径排序，因此输入拼接顺序确定；但最终 SQL 表中的可观察顺序仍应由查询的 `ORDER BY` 决定。`skip_rows` 是全局配置、逐文件局部计数，测试 `canonical_compressed_csv_and_sql_preserve_rows_and_skip_per_file` 明确验证每个匹配文件各跳过一行。

所有行先转成 AST 值并累计，之后才进入一次 `execute_insert`。因此在解压、解析、字段数检查或 UTF-8 转换阶段失败时，本函数尚未写入目标表；独立测试 `canonical_corrupt_file_keeps_table_empty` 验证“先读到一个有效文件、后遇到损坏 gzip”仍保持空表。进入 `execute_insert` 后的事务性和键冲突行为由正常 DML 路径负责，不由本文件另行定义。

## 依赖与调用关系

上游调用链为：

`ConcreteSession` 语句分派（`pkg/session/runtime/dispatch.rs`）→ `ConcreteSession::execute_import_file`（`pkg/session/runtime/import_file.rs`）→ 未指定 `split_file` 时的 `ConcreteSession::execute_import_compression`。

主要下游依赖如下：

- `astersql_parser_ast`（经 `super::*` 引入的 `ast`）：提供 `ImportIntoStmt`、`InsertStmt`、表引用和值表达式。
- `astersql-lightning-mydump`：提供 `Storage`、`StringReader`、`CsvConfig`、CSV/SQL parser、`Datum` 与 EOF 错误。
- `astersql-objstore-compressedio`：提供 `CompressType`、`DecompressConfig` 和 `new_reader`，实际处理 gzip、zstd、snappy 字节流。
- 会话运行时方法 `current_database`、`resolve_runtime_table` 与 `execute_insert`：分别解析 schema、取得表元数据并复用正常 KV/DML 写路径。
- `crate::dml_runtime::EvalExpr`：求值 IMPORT 选项表达式。

`pkg/session/Cargo.toml` 将本模块归入 crate `astersql-session`，并以工作区路径依赖声明 `astersql-lightning-mydump`、`astersql-objstore-compressedio`、`astersql-parser-ast` 等。该行为不受唯一 crate feature `nextgen` 的条件编译控制；内核相关路径预处理发生在更上游的 `dispatch.rs::prepare_import_path_for_kernel`。

## 错误处理与边界

以下情况在写入前失败：不支持的列映射形态；选项缺值、数值解析失败、`thread == 0` 或未知选项；未配置对象存储；列举失败或无匹配文件；目标表不存在；解码器创建/读取失败；格式不是 CSV/SQL；parser 读行或关闭失败；字段数不等于非隐藏列数；字节字段不是合法 UTF-8。

压缩识别只看不含查询串、转为小写后的末尾扩展名，支持 gzip、zstd、snappy；其他压缩格式会被当作原始字节交给 parser。格式推断只区分去压缩后缀后的 `.sql` 和默认 CSV，显式格式可覆盖推断。`source_matches` 只把 `*` 当通配符，不实现 Go `filepath.Match` 的 `[]` 等语法，并把 `?` 固定解释为查询串分隔符而非单字符通配符。

`fields_enclosed_by` 和 `fields_escaped_by` 在这里直接接收字符串，没有复刻 Go 对长度及分隔符前缀冲突的全部校验。`skip_rows` 解析为 `u64`，负数自然解析失败；非 `split_file` 分支没有 Go 对 split-file 场景 `skip_rows <= 1` 的限制。扩展或对齐时不能把该同步分支现有的成功范围误称为 Go IMPORT INTO 的完整兼容面。

## 并发与资源生命周期

本文件是同步串行实现。路径按字典序逐个打开、完整读取、解析和关闭；`thread` 选项只被验证，未驱动并发。Zstd 的 `zstd_decode_concurrency` 固定为 `1`。因此没有本地任务、线程、通道或锁的生命周期。

存储以 `Arc<dyn dump::Storage>` 克隆后使用，避免在 I/O 期间持有 `RefCell` 借用。原始 reader 被包装进可选解压 reader，并在循环迭代结束时释放；parser 接管内存 reader，且代码显式调用 `Close`。主要资源风险是峰值内存：单文件 `read_to_end` 加上所有文件全部行的 `lists` 同时存活，规模约随最大解压文件字节数与全部待插入行增长。若要支持大文件流式导入或并行化，应优先重新设计批次提交、失败原子性和资源上限，而不是让现有 `thread` 参数直接启动线程。

## 与 Go 版本的对应关系

Go 仓库没有 `pkg/session/runtime/import_compression.go` 的一一同路径实现。生产 Go 主链位于 `pkg/executor/importer/import.go`：`LoadDataController`/`Plan` 解析选项、初始化数据文件，使用 `mydump.ParseCompressionOnFileExtension` 标记压缩类型，并为通配路径通过对象存储遍历与 `filepath.Match` 选择文件；后续导入由完整 importer/DXF 流程执行。`pkg/session/session.go::validateStatementInTxn` 还明确禁止在显式事务中执行 `IMPORT INTO`。

本 Rust 文件保留的共同语义包括：默认 CSV 字段配置来源于 mydump；支持 CSV 和 SQL；压缩类型从文件扩展名推导；支持通配文件集合；`skip_rows` 非负且按文件生效；`thread` 必须为正；字段映射形态受到限制。Rust 独立测试用真实压缩 writer 验证 gzip/zstd/snappy 下 CSV 与 SQL 的行一致性，并验证损坏压缩流不会产生部分写入。

需要明确的差异是：Go 版本支持更完整的选项验证、格式与存储规划、压缩大小估算、并行处理、任务调度和分布式写入；Go 通配路径使用转义后的 `filepath.Match`，而本文件只有 `*`；本 Rust 分支把文件和行全部缓存在内存，且 `thread` 不控制并发。文档中的“对应”因此是语义参照，而不是宣称实现架构完全等价。

## 扩展指南

- 新增压缩后缀或压缩算法时，修改 `execute_import_compression` 的后缀识别与 `CompressType` 映射，并在 `pkg/session/runtime/import_compression_test.rs` 使用 `new_buffer` 增加成功和损坏流用例；同时确认格式推断使用的是去掉压缩后缀后的名字。
- 新增导入格式时，在 parser 分派处接入对应 `dump::Parser`，补充显式格式、自动推断、空文件、parser 错误和 `Close` 错误测试；不要绕过字段数与统一 insert 路径。
- 扩展 option 时，应与 `pkg/executor/importer/import.go::Plan::initOptions` 的 Go 语义逐项对齐，包括缺值、空字符串、数值范围和互斥约束，并把测试留在独立的 `import_compression_test.rs`。
- 若增强 glob，应先决定是否与 Go 的 `filepath.Match`、对象存储 URI 查询参数和转义规则完全兼容，再替换 `source_matches` 并添加查询串、多个 `*`、空匹配及特殊字符用例。
- 若实现真正并行或流式写入，必须显式设计文件顺序、内存上限、部分失败回滚、parser 关闭和 `execute_insert` 批次边界；这会改变当前“解析全部成功后才开始写”的重要性质。
- 若增加列映射、隐藏列、默认值或二进制非 UTF-8 支持，入口校验、字段计数和 `Datum`→`ValueExpr` 转换必须一起调整，并与正常 DML 类型转换和 Go importer 控制器做兼容验证。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`node --file pkg/session/runtime/import_compression.rs` 核对了完整 230 行实现；`query` 唯一定位 `execute_import_compression`（第 38 行）和 `source_matches`（第 14 行）。精确 `callers/callees` 没有返回边，因此通过已索引文件节点和文本引用补足直接调用证据。
- `pkg/session/runtime/dispatch.rs:1754-1784`：确认 `ImportIntoStmt` 的前置校验、query 分流及 `execute_import_file` 调用。
- `pkg/session/runtime/import_file.rs:96-107`：确认没有 `split_file` 时直接调用 `execute_import_compression`；同文件 `ImportFiles` 定义确认 storage 状态来源。
- `pkg/session/runtime.rs:59-60`：确认 `import_compression` 与 `import_file` 都是具体运行时的私有模块。
- `pkg/session/Cargo.toml`：确认 crate 名、`nextgen` feature 以及 mydump、compressedio、parser AST 等工作区依赖。
- `pkg/session/runtime/import_compression_test.rs`：`canonical_compressed_csv_and_sql_preserve_rows_and_skip_per_file` 覆盖 gzip/zstd/snappy、CSV/SQL、混合压缩与逐文件跳行；`canonical_corrupt_file_keeps_table_empty` 覆盖后续压缩文件损坏时无部分写入。
- `pkg/executor/importer/import.go` 与 `pkg/executor/importer/import_test.go::TestInitCompressedFiles`：核对 Go 的默认字段配置、option 校验、通配文件发现、压缩扩展解析和压缩文件测试；`pkg/session/session.go::validateStatementInTxn` 核对显式事务限制。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核本文未把同步 Rust 分支描述为完整 Go 分布式导入实现。
