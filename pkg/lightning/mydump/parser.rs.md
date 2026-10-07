# `pkg/lightning/mydump/parser.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-mydump`（`pkg/lightning/mydump/Cargo.toml`），由 crate 根 `pkg/lightning/mydump/lib.rs` 以 `mod parser; pub use parser::*;` 纳入并公开。它位于 Lightning/mydump 输入侧：把 SQL dump 中的 `INSERT ... VALUES ...` 字节流解析成行，抽象 SQL/CSV 共用的行解析器接口，并提供按源文件偏移切分导入任务的工具。

直接上游中，`pkg/importsdk/file_scanner.rs::ScannerKVSizeParserService::NewParser` 调用本文件的 `OpenReader`，为 IMPORT INTO 的 KV 大小采样创建 `dyn Parser`。RustCodeGraph 还确认 `pkg/executor/importer/table_import.rs` 使用这里公开的 `Parser`、`SourceFileMeta` 等类型；该文件通过自身的 `ParserProvider` 抽象把解析器交给导入管线，而不是直接调用 `ChunkParser::ReadRow`。

SQL 的词法扫描不在本文件内实现：`ChunkParser::ReadRow` 下沉到 `pkg/lightning/mydump/parser_generated.rs::lex`。后者维护跨读取块的词法状态，跳过注释和分隔符，并产生本文件定义的 `Token`。CSV 的具体实现则位于 `pkg/lightning/mydump/csv_parser.rs`，通过实现同一个 `Parser` trait 接入。

## 核心职责

1. 定义解析结果及切分元数据：`Datum`、`Row`、`Chunk`。
2. 用 `BlockParser` 统一管理底层 `PooledReader`、未消费缓冲、逻辑偏移、最近一行、列名和行向量复用池。
3. 用 `ChunkParser::ReadRow` 的四态状态机，把 `parser_generated::lex` 产生的 token 组装成一行 `Datum`。
4. 用 `Parser` trait 为 SQL 与 CSV 解析器提供定位、逐行读取、回收行、列名和关闭资源的共同协议。
5. 用 `ReadChunks`、`ReadUntil` 支持导入分片规划和从检查点推进；用 `OpenReader` 按 `SourceType` 构造 CSV 或 SQL 解析器。
6. 实现字符串转义和十六/二进制字面量转换，并把 I/O、配置和语法失败统一为 `MydumpError`。

本文件是宽松的 dump 数据解析器而不是完整 SQL 校验器。与 Go 注释所述设计相同，词法器会忽略 `INSERT`、`INTO`、逗号、分号和注释等内容；目标是快速、稳定地识别 VALUES 行和安全切分，不保证拒绝所有非标准 SQL。

## 主要符号

- `pub type Error = MydumpError`：本模块错误别名。`MydumpError::Eof` 表示正常耗尽；`Syntax`、`Io`、`Configuration` 等定义在 `pkg/lightning/mydump/common.rs`。
- `BUFFER_SIZE_SCALE: i64 = 2`：`makeBlockParser` 分配块缓冲时的放大倍数。传入大小先以 `max(1)` 限制为至少一个字节。
- `Datum::{Null, I64, Bytes, Binary}`：一格值的最小表示。十进制整数仅在可解析为 `i64` 时进入 `I64`；文本、引号字符串及溢出整数进入 `Bytes`；十六/二进制字面量进入 `Binary`。
- `Row { row, row_id, length }`：最近一次成功解析的行。`row_id` 在识别到行左括号时递增；`length` 是本次 `ReadRow` 消费到行右括号为止的 token 原始字节长度之和。
- `Chunk { offset, end_offset, real_offset, prev_row_id_max, row_id_max, columns }`：连续导入片段。行号可分配范围语义为 `(prev_row_id_max, row_id_max]`；本实现生成普通 SQL/CSV 分片时令 `real_offset == offset`。
- `EscapeFlavor::{None, MySql, MySqlWithNull}`：反斜杠转义策略。`NewChunkParser` 当前只选择 `None` 或 `MySql`；`MySqlWithNull` 在本文件没有构造点。
- `BlockParser`：持有 `PooledReader`、未消费 `buf`、块容量模板 `block_buf`、EOF 标记、列名、最后一行、逻辑偏移和 `row_pool`。`beginRowLenCheck`/`endRowLenCheck` 及其字段在当前 Rust 模块中没有调用者，应视为为接口/迁移保留的状态，而不是活跃流程。
- `makeBlockParser` / `make_block_parser`：构造基础解析器；后者是 snake_case 转发别名。
- `Parser` trait：公开 `Pos`、`SetPos`、`ScannedPos`、`Close`、`ReadRow`、`LastRow`、`RecycleRow`、列名和行号操作。与 Go 接口不同，Rust trait 不含 `SetLogger`；日志器只能通过具体 `BlockParser` 或本文件的同名转发函数设置。
- `ChunkParser` / `NewChunkParser`：SQL VALUES 解析器及构造器。`no_backslash_escapes=true` 时选择 `EscapeFlavor::None`，否则采用 MySQL 反斜杠转义。
- `Token`：词法器与行状态机之间的协议，覆盖行括号、`VALUES`、NULL/布尔、整数、进制字面量、三类引号和普通文本。
- `unescapeString`：先折叠成对定界符，再按 escape 字节解释 `0/b/n/r/t/Z`；未知转义去掉 escape 并保留后一字节，悬空 escape 保留原样。
- `ChunkParser::ReadRow`：核心四态解析函数。
- `ReadChunks` / `ReadUntil`：分别完整扫描并按最小逻辑字节跨度切片，或逐行推进到目标逻辑偏移。`read_chunks`/`read_until` 是命名风格别名。
- `OpenReader`：从 `Storage` 打开并完整读入文件，根据 `SourceType::{Csv, Sql}` 返回相应的 `Box<dyn Parser>`；其余类型返回 `Configuration` 错误。
- `acquireDatumSlice`、`logSyntaxError`、`readBlock`、`SetLogger`、`String`、`unescape`：为 Go 风格调用或兼容命名暴露的轻量入口。

## 执行流程

### 构造与补充缓冲

`NewChunkParser` 调用 `makeBlockParser`，后者用 `MakePooledReader` 包装输入流并初始化容量为 `max(size, 1) * BUFFER_SIZE_SCALE` 的 `block_buf`。第一次 `ReadRow` 发现 `buf` 为空且尚未到末尾时调用 `BlockParser::read_block`。该函数读取一块，读到零字节时设置 `is_last_chunk`；首次读取若以 UTF-8 BOM `EF BB BF` 开头，则删除 BOM 并把逻辑 `pos` 前移 3，之后把新字节追加到 `buf`。

词法器 `parser_generated.rs::lex` 每识别或跳过一段数据都会从 `buf` 前端删除对应字节并增加 `block_parser.pos`。token 跨块时返回内部的 `NeedMore`，保留未定型字节并再次调用 `read_block`；最终空缓冲且 `is_last_chunk=true` 才映射为 `Error::Eof`。

### 单行解析

`ChunkParser::ReadRow` 每次调用从 `State::Values` 开始：

1. `Values` 状态遇到普通/双引号/反引号文本，认为进入新 INSERT 的表名部分，清空上一条 INSERT 的列名并转到 `TableName`；遇到 `RowBegin` 则递增 `row_id`、从对象池取得空向量并转到 `Row`；重复 `VALUES` token 不改变状态。
2. `TableName` 接受表名片段；遇到 `RowBegin` 转入 `Columns`，遇到 `Values` 直接转回 `Values`。因此既支持带列清单，也支持无列清单的 INSERT。
3. `Columns` 将普通、双引号或反引号 token 经 `decode_text` 解引号、解转义并转小写后追加到 `columns`；右括号结束列清单。
4. `Row` 把 NULL 映射为 `Datum::Null`，TRUE/FALSE 映射为 `I64(1/0)`，合法 `i64` 映射为 `I64`，溢出整数退化为原始 `Bytes`，进制字面量经 `parse_based` 转为 `Binary`，文本 token 经 `decode_text` 转为 `Bytes`。
5. 遇到 `RowEnd` 时，把本地 values 向量和累计长度写入 `last_row` 并返回成功。下次调用从当前位置继续读取下一行。

如果词法器在表名、列清单或行数据的中间返回 EOF，函数把它改写为带当前偏移的 premature-EOF `Syntax`；只有处于初始 `Values` 状态的 EOF 才作为正常结束向上传递。

### 分块与打开文件

`ReadChunks` 记录初始 `(offset, row_id)`，持续 `ReadRow`。每次成功后，若当前位置与当前片段起点之差达到 `min_size`，就在完整行边界生成 `Chunk`，复制当时的列名，再把当前偏移/行号作为下一片段的起点。EOF 时仅在仍有未归档跨度时添加尾片；其他错误原样返回。由此片段偏移连续、相邻片段行号边界相接，但实际大小可以大于阈值。

`ReadUntil` 只在 `Parser::Pos().0 < target` 时继续读行；提前 EOF 视为成功，其他错误传播。`SetPos` 则先 seek 到绝对字节位置，验证底层实际位置完全一致，再清空缓冲、复位 EOF 状态并同步逻辑偏移和行号。

`OpenReader` 通过 `Storage::open(path, compression)` 获得可读流，当前实现用 `read_to_end` 将其全部装入内存 `StringReader`。CSV 分支调用 `NewCSVParser(cfg, reader, cfg.header, None)`，SQL 分支以 64 KiB 块大小、无 WorkerPool、启用 MySQL 反斜杠转义构造 `ChunkParser`。

## 数据与状态

- `BlockParser::pos` 是已经由词法器消费的逻辑字节位置，不等同于底层 reader 的预读位置。`ScannedPos` 返回底层流位置，可能领先于 `Pos`；该区别用于进度和检查点。
- `buf` 保存已读取但尚未被词法器消费的字节。`block_buf` 的长度决定单次读取上限，但 Rust `read_block` 当前每次按该长度重新分配临时向量，并未直接复用 `block_buf` 的存储。
- `is_last_chunk` 只有在一次 `read` 返回 0 时才置真。短读本身不代表 EOF，后续词法需要更多字节时会再次读取。
- `columns` 是最近 INSERT 的小写列名快照；开始识别下一条带表名 INSERT 时清空。`ReadChunks` 在生成每个片段时复制该状态。
- `last_row` 是解析器持有的最近成功行。`LastRow` 返回深拷贝；调用方在不再需要行时可调用 `RecycleRow`，把其中的 `Vec<Datum>` 清空后放回 `row_pool`。
- `row_pool` 是解析器实例私有的 LIFO 向量池，空池时分配容量 16；它不跨解析器共享，也没有内部同步。
- `row_start_pos` 与 `check_row_len` 可由公开方法设置，但当前词法和行组装流程没有读取它们。不能据此声称 Rust 已实现 Go 端任何额外行长检查行为。
- `parse_based` 对奇数位十六进制文本左补零；二进制文本从低位反向聚合为字节再反转。非法十六进制由 `hex::decode` 报错；二进制合法字符范围主要由上游词法器保证。

## 依赖与调用关系

核心调用链为：

`ScannerKVSizeParserService::NewParser`（`pkg/importsdk/file_scanner.rs`） → `OpenReader` → `Storage::open` → `NewCSVParser` 或 `NewChunkParser` → `ChunkParser::ReadRow` → `parser_generated::lex` → `BlockParser::read_block` / `unescapeString` / `parse_based`。

分片链为：导入规划或测试持有 `&mut dyn Parser` → `ReadChunks` → 重复 `Parser::ReadRow` 与 `Parser::Pos` → 形成 `Vec<Chunk>`。恢复链为调用方先 `SetPos(offset, row_id)`，再通过 `ReadUntil` 或逐行读取推进。

crate 内部依赖包括：

- `common.rs`：`MydumpError`、`FileInfo`/`FileMeta`。
- `reader.rs`：`ReadSeekCloser`、`PooledReader`、`WorkerPool`、`Storage`、`StringReader`。`PooledReader` 在实际 read/非查询 seek 期间临时获取 worker 令牌。
- `csv_parser.rs`：CSV 的 `Parser` 实现和 `NewCSVParser`。
- `parser_generated.rs`：SQL token 扫描及逻辑偏移推进。
- `router.rs` 等公开模块：`SourceType`、`Compression` 等文件元数据枚举。

外部依赖中，本文件直接使用标准库 `Read`/`Seek`/`Arc`，并用 Cargo 依赖 `hex = "0.4"` 解码十六进制。`thiserror` 用于相邻 `common.rs` 的错误枚举；其余 Cargo 依赖不应归因于本文件的直接执行路径。

RustCodeGraph 对精确符号的结果显示：`NewChunkParser` 的 Rust 调用者包括 `OpenReader` 与测试辅助函数；`ReadChunks`、`ReadUntil` 的直接 Rust 调用者主要是同名 snake_case 别名和 `parser_test.rs`；`OpenReader` 的生产调用者为 `pkg/importsdk/file_scanner.rs::NewParser`。这说明部分 Go 生产链尚未在 Rust 端一一接线，不能用 Go 的 37 个文件使用关系替代 Rust 当前调用事实。

## 错误处理与边界

- I/O 错误通过 `From<std::io::Error>` 转成 `MydumpError::Io`。`SetPos` 对“seek 成功但实际位置不同”也显式返回 `Io`。
- 正常输入耗尽用 `Error::Eof` 表示；`ReadChunks` 以此结束并保留尾片，`ReadUntil` 以此提前成功。二者都传播非 EOF 错误，`parser_test.rs::read_chunks_propagates_syntax_errors` 专门约束这一点。
- token 与当前状态不匹配时，`unexpected_token` 报告 token、原始内容、逻辑偏移和期望类别。词法器自身失败时会记录最多 256 字节的缓冲前缀，再返回 `Syntax`。
- 引号、块注释或行结构未闭合会产生语法错误；在非初始状态提前 EOF 会带上当前位置。空文件、空白、分隔符和纯注释最终正常 EOF，不产生行。
- 文本以 `String::from_utf8_lossy` 转换，非法 UTF-8 会被替换字符代替而不是返回编码错误；整数 token 的 `raw_str` 则要求有效 UTF-8。
- `parse_based` 的十六进制错误显式传播；二进制分支自身仅把 `1` 置位，依赖词法器拒绝其他数字。
- `OpenReader` 仅接受 CSV 和 SQL。Parquet、schema 等来源进入 `Configuration("no row parser for ...")`，且在分支判断前已经将整个对象读入内存。
- `min_size <= 0` 时，`ReadChunks` 会在每个成功行后切片；代码没有主动拒绝该参数。
- `SetPos` 将有符号 `i64` 直接转换为 `u64`；调用契约要求非负有效偏移，函数本身没有单独检查负值。
- `Close` 释放 `PooledReader` 内的底层 reader；之后 read/seek 会以 BrokenPipe 形式返回 I/O 错误。重复关闭由 `PooledReader::Close` 作为空操作处理。

## 并发与资源生命周期

`BlockParser`/`ChunkParser` 使用 `&mut self` 逐步改变缓冲、偏移、行号和对象池，设计为单个消费者串行驱动；本文件没有锁、任务或通道，也没有声明解析器可并发共享。并行性发生在更高层：`ReadChunks` 先在完整行边界产出互不重叠的 `Chunk`，导入器再按各片段建立独立解析器或定位状态。

可选 `Arc<WorkerPool>` 只由 `PooledReader` 在 read 和真正改变位置的 seek 周期内获取 RAII 令牌，操作结束即归还；查询当前位置的 `SeekFrom::Current(0)` 不占令牌。`NewChunkParser` 接受该池，而 `OpenReader` 当前传入 `None`，因此该入口没有 I/O 并发限流。

底层 reader 从构造起由 `PooledReader` 独占；`Close` 取走并丢弃它以释放资源。`OpenReader` 的临时存储 reader 在 `read_to_end` 完成后离开作用域，返回的解析器实际拥有内存中的 `StringReader`，所以其资源成本主要是整文件字节向量加解析缓冲，而不是持续持有对象存储流。

行向量的复用需要调用方遵循所有权顺序：先取得 `LastRow` 的副本并完成消费，再把该 `Row` 交给 `RecycleRow`；回收后该行的 values 被清空。由于 `LastRow` 当前会克隆整个 `Row`，复用池降低的是后续向量容量分配，不能消除行副本成本。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/mydump/parser.go`，行为测试为 `pkg/lightning/mydump/parser_test.go`；Rust 独立测试是 `pkg/lightning/mydump/parser_test.rs`，没有把测试内嵌在生产源文件中。

保持一致的主要语义：

- `blockParser`/`BlockParser`、`ChunkParser`、`Chunk`、`Parser`、token 集合及四态 `ReadRow` 状态机一一对应。
- 列名被解引号后转小写；下一条带表名但无列清单的 INSERT 会清空旧列名。
- NULL、布尔、超长整数回退文本、十六/二进制字面量、三类引号、反斜杠转义和双定界符折叠规则均有对应实现。
- `ReadChunks` 只在完整行结束后按最小跨度切片，EOF 正常收尾，其他错误传播；`ReadUntil` 接受目标前提前 EOF。
- `Pos` 表示已解析的非压缩逻辑位置，`ScannedPos` 表示底层读取位置；行向量可通过池回收。

当前可见差异与迁移限制：

- Go 正整数先用 `ParseUint`，可保存完整 `uint64`；Rust `Datum` 只有 `I64`，超过 `i64::MAX` 的正整数会退化为 `Bytes`。Rust 测试只覆盖远超 64 位整数的回退，未证明 `i64::MAX+1..=u64::MAX` 与 Go 等价。
- Go `OpenReader` 只负责按压缩配置打开并返回流，解析器类型由更高层选择；Rust 同名函数会整文件读入内存并直接选择 CSV/SQL parser。两者 API 和内存生命周期并非逐句移植。
- Go `Parser` 包含 `SetLogger`，Rust trait 不包含；Rust 仅给具体 `BlockParser` 暴露设置入口。
- Go `readBlock` 使用 `ReadFull`、复用缓冲，并区分截断压缩流的 `ErrUnexpectedEOF`；Rust `read_block` 使用普通 `read`、每次新建临时向量，并以零字节读取确认 EOF。Rust 的压缩错误是否等价取决于 `Storage::open` 返回的 reader，当前 `parser_test.rs` 未覆盖截断压缩流。
- Go 保存 metrics、logger 上下文以及 `remainBuf`/`appendBuf`；Rust 只有简化的 logger 和 `Vec<u8>` 追加缓冲，没有同等 metrics 观测。
- Go 的 `Chunk` 生成逻辑默认不写 `RealOffset` 和列名（零值/空值），Rust `ReadChunks` 显式令 `real_offset=offset` 并复制 `Columns()`。这是当前 Rust 行为，调用方不能假定 Go 零值语义。
- Go 使用 TiDB `types.Datum` 与 collation 信息；Rust 用本地精简 `Datum`，文本只保存字节，没有 Go Datum 的无符号整数和类型元数据。

测试对应关系方面，Rust 的 `TestReadRow`、`TestReadChunks`、`TestNestedRow`、`TestVariousSyntax`、`TestSyntaxError`、`TestContinuation`、`TestPseudoKeywords`、`TestMoreEmptyFiles` 覆盖 Go 同名/同意图场景的核心子集；额外的 Rust 测试约束列状态机、溢出整数回退、错误传播、提前 EOF 和逐字节转义。Go 测试规模更大，因此不能把 Rust 当前测试集描述为完全覆盖 Go parser 的全部输入矩阵。

## 扩展指南

- 新增 SQL token 或改变词法优先级时，应以 `pkg/lightning/mydump/parser.rl` 的规则为语义来源，并同步 `parser_generated.rs`；随后在独立的 `parser_generated_test.rs` 和/或 `parser_test.rs` 增加跨块、EOF 和冲突 token 测试。不要只改 `Token` 或 `ReadRow` 而让扫描器协议失配。
- 新增值类型时，先扩展 `Datum`，再更新 `ChunkParser::ReadRow` 的 `State::Row` 分支，并核对 Go `types.Datum` 的有符号性、字面量格式和错误行为。尤其要决定 `u64` 范围是新增变体还是继续文本回退。
- 改变转义规则时，同时检查 `unescapeString`、`decode_text`、`EscapeFlavor` 的构造点，以及 `parser_test.rs::TestUnescapeBytePairs`、`TestUnescapeDelimiterBeforeEscape`。反引号目前强制 `EscapeFlavor::None`，不应被普通字符串模式意外覆盖。
- 改变分片策略时修改 `ReadChunks`，并保持相邻 `end_offset == offset`、相邻行号边界衔接、只在完整行边界切分及错误不被 EOF 吞掉等不变量；同步 `parser_test.rs::TestReadChunks` 和错误传播测试。若调整 `real_offset`/`columns`，还需检查 region/导入侧消费者。
- 扩展新 `SourceType` 时在 `OpenReader` 增加明确分支，并评估是否允许 `read_to_end`。Parquet 等随机访问或大文件格式更适合独立 parser/opener，不应无意复制整个对象。
- 优化内存时，优先评估 `read_block` 的临时分配、`buf.drain(..end)` 的前移成本、`LastRow` 深拷贝和 `OpenReader` 整文件加载；任何优化都必须保留跨块 token、逻辑偏移和 BOM 语义。
- 增加进度/检查点功能时，明确区分 `Pos` 与 `ScannedPos`，并在 seek 后清理所有可能保存旧字节或旧 EOF 的状态。负偏移校验也应放在 `SetPos` 边界。
- 测试继续放在同目录独立文件 `parser_test.rs` 或生成词法器测试文件中，不要放回 `parser.rs`。若目标是与 Go 对齐，应先扩展当前 Rust 测试到对应 Go 场景，再修改实现。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；目标文件可通过 `node --file pkg/lightning/mydump/parser.rs` 完整读取。文件关系显示其被 `pkg/importsdk/file_scanner.rs`、`pkg/executor/importer/table_import.rs`、`pkg/lightning/mydump/parser_test.rs` 和 `pkg/dumpformat/parquetfile/parser_test.rs` 使用。
- RustCodeGraph 符号查询：`NewChunkParser`、`ReadChunks`、`OpenReader`、`unescapeString` 均同时定位到 Rust 与 Go 同路径实现；精确 explore 显示 Rust `NewChunkParser <- OpenReader/parser_test::parser`、`ReadChunks <- parser_test/别名`、`ReadUntil <- parser_test/别名`、`OpenReader <- pkg/importsdk/file_scanner.rs::NewParser`。
- 已读 Rust 生产路径：`pkg/lightning/mydump/parser.rs`、`lib.rs`、`parser_generated.rs`、`common.rs`、`reader.rs`、`pkg/importsdk/file_scanner.rs`，并检查 `pkg/executor/importer/table_import.rs` 对公开解析接口的引用。
- 已读 crate 声明：`pkg/lightning/mydump/Cargo.toml`，确认 crate 名、`lib.rs` 入口、`hex` 等依赖及 `go-package = "pkg/lightning/mydump"` 的移植元数据。
- 已读 Go 对照：`pkg/lightning/mydump/parser.go` 的基础状态、读取/转义、`ReadRow`、`ReadChunks`、`ReadUntil`、`OpenReader`；已读 Go 测试 `pkg/lightning/mydump/parser_test.go` 的逐行、偏移、列名、分片、嵌套语法和错误场景。
- 已读 Rust 独立测试：`pkg/lightning/mydump/parser_test.rs` 全文，确认多块大小、值映射、列状态、错误传播、提前 EOF、对象语义和转义边界。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收要求本文恰好包含任务指定的 11 个二级标题；最终交付前以任务文件给定命令验证。
