# `pkg/lightning/mydump/csv_parser.rs`

## 文件定位

本文件属于 `astersql-lightning-mydump` crate。crate 由 `pkg/lightning/mydump/Cargo.toml` 定义，`lib.rs` 通过 `mod csv_parser; pub use csv_parser::*;` 将这里的配置、构造函数和解析器公开给 Lightning、IMPORT/LOAD DATA 与 session 运行时。它实现 `parser.rs` 中统一的 `Parser` trait，是 `OpenReader` 针对 `SourceType::Csv` 选择的 CSV 分支；SQL dump 则走 `ChunkParser`。

直接生产调用证据包括 `pkg/lightning/mydump/parser.rs::OpenReader`、`pkg/lightning/mydump/region.rs::openCSVParser`、`lightning/pkg/importer/chunk_process.rs`、`lightning/pkg/importer/get_pre_info.rs`、`pkg/session/runtime/import_compression.rs` 和 `pkg/session/runtime/load_data.rs`。因此该文件不是通用 CSV 工具的孤立副本，而是把 mydumper/LOAD DATA 风格文本转为统一 `Row` 的导入边界。

## 核心职责

- `CsvConfig` 描述字段分隔符、引号、行终止符、`STARTING BY` 前缀、转义符、NULL 文本和若干兼容开关；`Default` 提供逗号、双引号、换行、反斜杠及 `\\N` 的默认方言。
- `NewCSVParser` 接管一个 `ReadSeekCloser`，从起点读取全部字节，准备经过字符集编码的字段/引号/行终止符，并校验空字段分隔符及不合法的 `STARTING BY`/行终止符组合。
- `ReadRow`/`readRecord`/`read_field` 把输入拆成字段，处理多字节分隔符、引号字段、双引号、MySQL 反斜杠转义、空行、行前缀、尾部分隔符和文件末尾无终止符的最后一行。
- `ReadColumns` 消费表头并保存小写列名；`Parser` 实现提供位置、行号、最近一行、列名及行向量回收能力。
- `LargestEntryLimit` 对单条逻辑记录的累计扫描字节设限，防止异常大行无限扩张内存。

## 主要符号

- `LargestEntryLimit: AtomicUsize`：进程内共享的行大小上限，默认 `120 * 1024 * 1024`。`ensure_entry_limit` 从 `row_start` 到当前 `pos` 计算累计长度；名字沿用 Go，但实际约束对象是整行而非单字段，`csv_parser_test.rs::row_limit_counts_all_fields` 固化了这一点。
- `CsvConfig`：公开的 CSV 方言结构。`header` 供上层规划使用；真正决定构造后是否先消费表头的是 `NewCSVParser` 的 `should_parse_header` 参数，见测试 `constructor_header_argument_controls_header_consumption`。
- `Field`：内部字段结果，保存已去定界符并展开转义的 `content`、是否带引号 `quoted`、是否匹配 NULL 标记 `is_null`。
- `CsvParser`：核心有状态解析器。`data`/`pos` 是全量输入和游标；`row_start`/`row_id` 是当前记录位置；`columns`/`last_row` 是对外状态；`comma`/`quote`/`newline`/`starting`/`escape` 是配置派生值；`convertor` 负责字符集；`header_pending` 控制延迟表头；`recycled_rows` 复用 `Vec<Datum>`。
- `encodeSpecialSymbols`：通过 `CharsetConvertor::Encode` 编码字段分隔符、引号和行终止符。`lines_starting_by` 与转义符没有通过此函数转换。
- `NewCSVParser` 与 `new_csv_parser`：公开构造入口及 snake_case 别名。构造函数先 `seek(0)`、再 `read_to_end`，所以读取/seek 错误在构造阶段返回。
- `ReadColumns`、`ReadRow`、`readRecord`、`read_field`：分别承担表头、行级输出、记录边界和字段级状态机。
- `unescapeString`/`append_escape`：展开 `\\0`、`\\b`、`\\n`、`\\r`、`\\t`、`\\Z`，未知转义保留转义符后的字节；末尾孤立转义返回语法错误。
- `ReadUntilTerminator` 及字节级辅助函数：用于跳行、探测和 Go API 对齐；`find_subslice` 提供多字节模式的朴素窗口搜索。
- `impl Parser for CsvParser`：把具体解析器适配到 `parser.rs::Parser`，包含 `Pos`、`SetPos`、`ScannedPos`、`Close`、`LastRow`、`RecycleRow`、列名及行号接口。

## 执行流程

1. 上层根据文件类型和 SQL/导入选项组装 `CsvConfig`，将 reader、表头开关及可选 `CharsetConvertor` 交给 `NewCSVParser`。
2. 构造函数把 reader 定位到零并全部读入 `data`；有转换器时编码三个特殊符号，否则直接取 UTF-8 字节。随后初始化游标、空结果、配置派生字节与回收池。
3. 每次 `ReadRow` 先递增 `row_id`。若 `header_pending`，它先调用 `ReadColumns` 消费一个记录、解码并小写化列名，然后读取数据记录；即便后续遇到 EOF，行号仍已递增，测试 `row_id_advances_on_failed_read` 明确验证该契约。
4. `readRecord` 先处理 `lines_starting_by`：只保留物理行中首次前缀之后的内容，缺少前缀的整行跳过。随后忽略不允许的空行/纯 ASCII 空白行，并循环调用 `read_field`。
5. `read_field` 若当前位置以引号开头，则进入引号状态机：双定界符产生一个字面引号，配置允许时字段内部非边界引号按文本保留，转义符交给 `append_escape`；否则扫描到字段或行分隔符，再由 `unescapeString` 展开转义。
6. 字段后必须出现字段分隔符、行终止符或 EOF。`trim_last_separators` 在行终止符前直接消费末尾分隔符而不产生空字段；普通连续分隔符则产生空字段。
7. `ReadRow` 解码每个字段；满足 NULL 标记且不受 `quoted_null_is_text` 保护时生成 `Datum::Null`，否则生成 UTF-8 `Datum::Bytes`。`Row.length` 是字段内容长度之和，不含分隔符、引号和行终止符。
8. 上层通过 `LastRow` 取克隆结果；不再使用时可调用 `RecycleRow`，把其中的 `Vec<Datum>` 清空后放回池供下一行复用。

## 数据与状态

`pos` 是下一待解析字节的位置，`Pos()` 返回 `(pos, row_id)`；`ScannedPos()` 当前与 `pos` 相同。`row_start` 在通过行前缀筛选后设置，用于整行长度限制。`SetPos` 只校验范围并更新 `pos`/`row_id`，不会重置 `columns`、`last_row`、`header_pending` 或回收池，调用方恢复分片位置时必须自行保证这些状态与位置一致。

`last_row` 在成功读取后整体替换。它含 `Vec<Datum>`、逻辑 `row_id` 和字段原始内容总长度。`LastRow` 会克隆整行；`RecycleRow` 只回收传入行的值向量。解析器本身不共享可变行对象，也没有后台读取任务。

特殊符号中 `comma`、`quote`、`newline` 支持多字节序列；`escape` 仅取 `fields_escaped_by` 的首字节。`newline` 为空时，`is_newline_at`/`next_newline_offset` 把 CR 或 LF 各自视为换行；CRLF 因连续两个换行字节而被消费两次，但空行过滤使外部行结果与测试期望一致。

## 依赖与调用关系

上游调用链可概括为：文件/语句配置 → `NewCSVParser` → `Box<dyn Parser>` → importer、采样器或 session 循环调用 `ReadRow` → `LastRow` 交给编码/写入阶段。`pkg/lightning/mydump/region.rs` 还直接调用 `ReadColumns` 获取 CSV 表头和数据起始偏移；`pkg/executor/importer/sampler.rs` 使用 `Pos` 与 `Row.length` 估算源数据和 KV 大小。

文件内主要下游边为：`NewCSVParser` → `ReadSeekCloser::{seek, read_to_end}`、`encodeSpecialSymbols`；`ReadRow` → `ReadColumns`/`readRecord`/`decode`；`readRecord` → 行前缀与换行辅助函数/`read_field`；`read_field` → `append_escape`/`unescapeString`/`ensure_entry_limit`；`decode` → `CharsetConvertor::Decode` 或 `String::from_utf8`。

crate 外部依赖间接来自同 crate 的 `CharsetConvertor`，其实现使用 `encoding_rs`；本文件直接只使用标准库 I/O、字节向量和原子类型。`Cargo.toml` 的 `[package.metadata.porting] go-package = "pkg/lightning/mydump"` 明确了对应 Go 包。

RustCodeGraph 将该文件识别为 58 个符号并报告 14 个使用文件；精确源码检索进一步确认 `NewCSVParser` 的 Rust 生产调用点位于 Lightning importer、mydump parser/region、session 导入路径等位置。图的 `callers` 子命令本次未在限定时间内返回，因此调用点以这些精确路径复核，没有推断未观测的边。

## 错误处理与边界

- 正常耗尽用 `Error::Eof` 表示；读取循环应把它当作结束而不是失败。最后一行没有行终止符时，只要含内容仍成功返回一次。
- 构造期空字段分隔符、不合法的 `STARTING BY` 组合和超大行返回 `Error::Configuration`；底层 seek/read 失败经 `From<std::io::Error>` 变为 `Error::Io`。
- 未闭合引号、悬空转义、严格模式下非引号字段中的引号、字段结束后出现意外字节返回 `Error::Syntax`。`replaceEOF` 可把辅助读取的 EOF 映射成更具体错误。
- 无字符集转换器时，字段必须是 UTF-8，否则 `decode` 返回 `Error::Encoding`；有转换器时错误由 `CharsetConvertor` 传播。
- `SetPos` 拒绝负位置和超过 `data.len()` 的位置。字节探测函数大多通过切片边界或 `Option`/`Error::Eof` 防止越界。
- `readUntil(target)` 对空 `target` 会立即成功且不推进游标；它是公开辅助函数，扩展调用者必须避免在无进展循环中反复调用。
- NULL 当前是单个 `cfg.null` 字符串。是否为 NULL 在字段原始/去定界符内容上判断，再结合 `quoted_null_is_text`；修改转义或 NULL 次序时必须重跑相应兼容测试。

## 并发与资源生命周期

`CsvParser` 通过 `&mut self` 串行推进，不在内部启动线程、任务或通道。`ReadSeekCloser` 要求 `Send`，但构造函数读取完毕后不把 reader 保存到结构体中：reader 在 `NewCSVParser` 返回前离开作用域并被释放，之后 `Close()` 是无操作。与 Go 的块读取器相比，这意味着 Rust 版资源生命周期更短，但输入内存占用与整个文件大小成正比。

全局 `LargestEntryLimit` 是唯一跨实例共享的可变状态，解析时以 `Ordering::Relaxed` 读取。生产代码通常只读取默认值；测试用 `swap/store` 临时修改并恢复。并行修改该原子值会同时影响所有解析器，因此新增测试或运行时配置若需改值，应隔离并发影响。

`recycled_rows` 只属于单个解析器。它减少 `Vec<Datum>` 重复分配，但 `LastRow` 先克隆再回收，调用者必须显式把不再需要的 `Row` 传给 `RecycleRow`；不可把仍被业务持有的行误当成可复用缓冲。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/mydump/csv_parser.go`，独立对照测试是 `pkg/lightning/mydump/csv_parser_test.go`。Rust 保留了 Go 的核心名称和语义骨架：`CSVParser/CsvParser`、`NewCSVParser`、`ReadRow`、`ReadColumns`、`readRecord`、定界符/转义辅助函数、行号先递增、字段长度求和、表头小写化、宽松未转义引号、空行与 `STARTING BY` 处理。

当前实现并非逐机制等价，扩展时不能直接假设 Go 行为全部存在：

- Go 通过 `blockParser`、块缓冲和 worker pool 流式读取；Rust 构造期 `read_to_end`，无块缓冲、worker、metrics 或日志器。
- Go 配置来自更完整的 `config.CSVConfig`，支持 `NotNull`、多个 `FieldNullDefinedBy`、`HeaderSchemaMatch` 等；Rust `CsvConfig` 只有单个 `null`，且 `ReadColumns` 总会记录小写列名。
- Go 的 `ReadUntilTerminator` 返回内容与位置并支持跨块错误语义；Rust 版本只推进游标并返回 `Result<()>`。
- Go 的 `Close`/seek/缓冲状态由底层 reader 与 block parser 管理；Rust 只在内存字节向量上定位，`Close` 无操作。
- Rust 的 `trim_last_separators` 名称和处理位置与 Go `TrimLastEmptyField` 不同，但都覆盖常见的行尾空字段裁剪场景；复杂多个尾部分隔符需要以测试明确期望。

Rust 测试刻意记录了多项 Go 契约，例如行号在失败读取前递增、`Row.length` 不含结构符号、空行终止符自动识别和 `STARTING BY` 行内匹配。尚未由 Rust 测试覆盖或接口缺失的 Go 能力应标记为迁移差异，而不是宣称已支持。

## 扩展指南

- 新增 CSV 方言开关时，先扩展 `CsvConfig`，再检查 `NewCSVParser` 的派生字节、`readRecord` 的记录状态机和 `read_field` 的字段状态机；同时核对 `pkg/session/runtime/load_data.rs`、`pkg/executor/importer/import.rs` 等配置映射入口。
- 改动引号、分隔符或转义优先落在 `read_field`、`append_escape`、`unescapeString`，并在独立文件 `pkg/lightning/mydump/csv_parser_test.rs` 增加回归测试；不要把 Rust 测试内嵌到生产源文件。
- 改动 NULL 语义需覆盖：未加引号、加引号、`quoted_null_is_text`、自定义转义符、自定义 NULL 文本及字符集转换，并逐项与 Go 的 `unescapeString` 和测试对照。
- 若要恢复 Go 的大文件流式特性，需要重构 `data`/`pos` 与 reader 生命周期，连带审查 `SetPos`、`ScannedPos`、跨块多字节分隔符、行大小限制和 `ReadUntilTerminator`；这不是局部替换 `read_to_end` 即可完成的变更。
- 修改位置或表头语义时同步检查 `region.rs::getHeaderColumn` 的 CRLF 边界修正、`Parser` trait 调用者和 importer 分片恢复逻辑。
- 性能优化应保留多字节符号、宽松引号及密集转义行为；可用 `TestCSVParserUnescapeDenseRows`、两个 benchmark harness 和大行限制测试防止分配/状态回归。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标目录文件列表确认 `csv_parser.rs`、Rust/Go 对照及测试均被索引；`node --file pkg/lightning/mydump/csv_parser.rs` 读取完整 599 行；`query` 确认 `CsvParser`、Rust/Go `NewCSVParser`、`ReadColumns`、`unescapeString` 等符号。`explore`/`callers` 在限定时间内没有返回结果，故未用其输出证明调用关系。
- 生产源码：`pkg/lightning/mydump/csv_parser.rs`；统一接口和工厂 `pkg/lightning/mydump/parser.rs`；reader 契约 `pkg/lightning/mydump/reader.rs`；错误类型 `pkg/lightning/mydump/common.rs`；crate 装配 `pkg/lightning/mydump/lib.rs`；表头/region 入口 `pkg/lightning/mydump/region.rs`。
- crate/config：`pkg/lightning/mydump/Cargo.toml`；直接上游还核对了 `lightning/pkg/importer/chunk_process.rs`、`lightning/pkg/importer/get_pre_info.rs`、`pkg/executor/importer/sampler.rs`、`pkg/session/runtime/import_compression.rs`、`pkg/session/runtime/load_data.rs`。
- Go 对照：`pkg/lightning/mydump/csv_parser.go` 与 `pkg/lightning/mydump/csv_parser_test.go`，重点核对构造、`unescapeString`、`readRecord`、`readQuotedField`、`ReadRow`、`ReadColumns` 和终止符读取。
- Rust 独立测试：`pkg/lightning/mydump/csv_parser_test.rs`，覆盖 RFC4180/MySQL/TSV/CRLF、多字节符号、行前缀、空行、NULL、表头、字符集、EOF、语法/I/O 错误、整行限制、行号、长度和缓冲复用。本任务依约不运行 Cargo；完成判断采用源码/调用证据、人工事实复核和文档结构验证。
