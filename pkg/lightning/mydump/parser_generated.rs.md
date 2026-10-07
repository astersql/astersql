# `pkg/lightning/mydump/parser_generated.rs`

## 文件定位

本文件是 `astersql-lightning-mydump` crate 内部的 SQL dump 词法扫描器。crate 由 `pkg/lightning/mydump/Cargo.toml` 定义，`lib.rs` 以私有模块 `mod parser_generated` 装配它；因此文件唯一公开给 crate 外部的函数 `lex` 实际仍受模块可见性限制，只由同 crate 的行解析逻辑使用。

它位于 Lightning 读取 mydumper SQL 文件的前半段：`OpenReader`/`NewChunkParser` 创建 `ChunkParser`，`ChunkParser::ReadRow` 反复调用 `parser_generated::lex` 得到 token，再在 `parser.rs` 中把 token 转成 `Datum` 和 `Row`。文件头明确说明实现由 `parser.rl` 的 Ragel 规则语义迁移而来；它保留生成文件的命名和状态常量，但用可读的 Rust 游标函数表达状态机，而不是复制 Go 的大段跳转表。

## 核心职责

1. 从 `ChunkParser.block_parser.buf` 的开头识别一个 token，并返回 token 及其未经解码的原始字节。
2. 跳过不产生 token 的输入：空白、`,`、`;`、块注释、行注释，以及 mydumper JSON 输出中的 `CONVERT(` 和 `USING UTF8MB4)` 固定片段。
3. 识别行括号、大小写不敏感关键字、十进制整数、十六/二进制字面量、单引号/双引号/反引号字符串与普通未加引号文本。
4. 在 token 横跨读取块时保留尚未确定的缓冲内容，通过 `BlockParser::read_block` 追加数据后继续扫描。
5. 保持 Ragel 的“最长匹配优先、等长时专用规则优先”语义，例如 `values` 是 `Token::Values`，而 `valuesx` 是完整的 `Token::Unquoted`。

本文件只完成词法切分，不解释 INSERT 的表名、列清单或行结构，也不把字面量转换成最终值；这些语法状态和解码工作属于 `parser.rs` 的 `ChunkParser::ReadRow`、`decode_text` 与 `parse_based`。

## 主要符号

- `CHUNK_PARSER_START`、`CHUNK_PARSER_FIRST_FINAL`、`CHUNK_PARSER_ERROR`、`CHUNK_PARSER_EN_MAIN`：与 Go/Ragel 生成器相同的状态编号。当前可读实现只用前三个非错误状态值组成 `_initial_state`，并用错误常量做调试断言；真正分派由 Rust 分支完成。
- `ScanResult`：单次扫描的内部结果。`Token(Token, usize)` 和 `Skip(usize)` 携带缓冲消费终点；`NeedMore` 表示当前块不足以确定最长匹配；`Eof` 表示缓冲为空且底层已结束；`Error` 表示输入无法匹配或构造未闭合。
- `pub fn lex(&mut ChunkParser)`：唯一词法入口。它循环调用 `scan_one`，负责消费缓冲、更新逻辑字节位置、续读和把内部结果映射为 `Result<(Token, Vec<u8>), Error>`。
- `scan_one`：规则总分派器，按注释/忽略片段、括号与引号、关键字、进制字面量、整数、普通文本的顺序尝试规则。
- `Prefix` 与 `ascii_prefix`：区分大小写不敏感前缀的完全匹配、块尾部分匹配和不匹配，防止跨块关键字被过早当成普通文本。
- `find_block_comment_end`：查找第一个闭合 `*/`，对应 `parser.rl` 的块注释规则。
- `scan_quoted`：扫描三类引号；成对定界符表示转义，单/双引号是否接受反斜杠由 `EscapeFlavor` 决定，反引号始终不使用反斜杠规则。
- `scan_based_quoted`、`scan_based_digits`、`is_radix_digit`：处理 `x'AB'`/`b'01'` 与 `0xAB`/`0b01` 两组进制字面量及合法数字集合。
- `scan_integer`：识别 `-` 可选、至少一位数字的十进制整数。
- `scan_unquoted`、`is_unquoted_delimiter`：识别普通文本，终止字符与 `parser.rl` 的排除集合一致。
- `prefer_over_unquoted`：比较专用规则长度和普通文本可匹配长度，是最长匹配语义的集中实现。

## 执行流程

`ChunkParser::ReadRow` 在 `parser.rs` 中调用 `lex(self)`。一次 `lex` 的流程如下：

1. `scan_one` 查看当前缓冲。空缓冲在末块返回 `Eof`，否则返回 `NeedMore`。
2. 空白和 `,;` 每次跳过一个字节；块注释跳到闭合符之后；行注释跳到换行前；两个 JSON 包装片段按 ASCII 大小写不敏感方式整体跳过。
3. `(`、`)` 立即产生行边界 token；三类引号进入 `scan_quoted`。
4. `values/null/true/false` 先检查前缀，再交给 `prefer_over_unquoted` 判断是否被更长普通文本覆盖。
5. 扫描带引号或 `0x`/`0b` 前缀的进制字面量；随后尝试整数；其余输入由 `scan_unquoted` 处理。
6. `Token` 结果让 `lex` 复制 `buf[..end]` 作为原始值，删除该前缀并增加 `block_parser.pos`；`Skip` 只删除和计数后继续扫描。
7. `NeedMore` 在非末块调用 `BlockParser::read_block`，保留现有 token 前缀并追加下一块；在末块则返回 `unexpected EOF`。`Error` 会先调用 `log_syntax_error`，再返回普通语法错误。
8. 上层 `ChunkParser::ReadRow` 根据 token 维护 `TableName/Columns/Values/Row` 四态语法机，并把原始字节解码为行数据。

关键不变量是：`block_parser.pos` 只增加已经确定可消费的字节数；可能影响最长匹配的块尾内容不会提前消费。因此块大小变化不应改变 token 序列。

## 数据与状态

扫描器没有全局可变状态。持久状态全部位于借用的 `ChunkParser`：

- `block_parser.buf` 保存未消费字节；续读通过追加而不是替换来保护跨块 token。
- `block_parser.is_last_chunk` 区分“暂时没有更多字节”和真实 EOF。
- `block_parser.pos` 是已消费源码字节的逻辑位置；返回 token 或跳过输入时同步推进。
- `esc_flavor` 决定单、双引号内反斜杠是否转义。`EscapeFlavor::None` 禁止该规则；反引号不受此字段影响。

内部扫描只使用局部索引 `usize`，并返回消费终点，不保存跨调用游标。`lex` 为 token 创建 `Vec<u8>` 副本，随后用 `Vec::drain` 删除缓冲前缀；这简化了所有权，但意味着每个 token 都有一次复制，缓冲前缀删除也可能移动剩余字节。

## 依赖与调用关系

上游直接调用边有两类：

- 生产路径：`parser.rs` 的 `ChunkParser::ReadRow` 调用 `crate::parser_generated::lex(self)`；`Parser::ReadRow`、`ReadChunks`、`ReadUntil` 和 `OpenReader(SourceType::Sql)` 再把它接入 Lightning 的统一读取链。
- 测试路径：`parser_generated_test.rs::lex_once` 直接调用 `lex`，隔离验证 Ragel 规则优先级。

下游直接依赖是同 crate 的 `ChunkParser`、`BlockParser::read_block`、`BlockParser::log_syntax_error`、`EscapeFlavor`、`Token` 与 `MydumpError`（经 `parser.rs::Error` 别名）。扫描辅助函数均为本文件私有函数，没有线程、网络或存储依赖。

`Cargo.toml` 没有为该模块设置 feature 开关；模块始终编入库。该文件本身只使用 crate 内类型和标准库字节/切片能力；Cargo 中的 `hex` 等依赖由后续 `parser.rs::parse_based` 使用，不是词法器的直接依赖。

RustCodeGraph 已索引目标文件并列出 28 个符号；精确文件节点确认 `parser.rs` 第 339 行的生产调用和测试文件第 7 行的直接调用。通用名 `lex` 的 `callers/callees` 查询在当前索引后端存在名称消歧噪声，因此调用关系以已索引文件节点中的确切调用表达式复核，而没有把噪声结果当作证据。

## 错误处理与边界

- 缓冲为空且已经是末块时返回 `Error::Eof`，供 `ReadRow`/`ReadChunks` 作为正常结束处理。
- 未闭合块注释、未闭合引号、末尾悬空反斜杠、未闭合带引号进制字面量，在末块返回语法错误；非末块返回 `NeedMore`。
- 不匹配任何规则的起始分隔字符（例如不能组成 `/*` 的独立 `/` 或 `*`）进入 `ScanResult::Error`；`lex` 会记录最多 256 字节的缓冲前缀和当前位置。
- `scan_based_quoted` 遇到非法进制数字直接报错；无引号 `0x`/`0b` 若没有合法数字则落回普通文本。若专用前缀后还有更长普通文本，如 `0x12g`，最长匹配使整个输入成为 `Unquoted`。
- 仅有 `-` 不是整数；它会落入普通文本或与 `--` 行注释规则组合。
- 对未结束的关键字前缀必须等待下一块。例如非末块的 `val` 不能提前返回 `Unquoted`，因为后续可能组成 `values`。
- 词法器按字节工作，不验证 UTF-8；普通文本和引号内容的损失/校验策略由上层解码决定。

## 并发与资源生命周期

`lex` 接收 `&mut ChunkParser`，同一解析器在一次调用期间具有独占可变借用，不能并发扫描同一缓冲。文件不创建线程、任务、锁、通道或事务；并行导入发生在更上层按 `Chunk` 切分之后。

底层 reader 的生命周期由 `BlockParser`/`PooledReader` 管理。词法器只在 `NeedMore` 时同步调用 `read_block`，并原样传播 I/O 错误；它既不关闭 reader，也不回收行对象。EOF 由 `is_last_chunk` 持久化，一旦底层读取返回 0，后续扫描只消费缓冲余量，耗尽后返回 `Error::Eof`。

资源方面需要关注两点：跨块 token 会让 `buf` 持续增长到 token 完整，故极长或永不闭合且尚未到 EOF 的字段会占用与字段长度成正比的内存；token 返回时又会复制原始字节。修改缓冲策略时必须同时保护位置计算、跨块最长匹配和上层对原始字节的所有权。

## 与 Go 版本的对应关系

Go 语义来源是 `pkg/lightning/mydump/parser.rl`，生成物是 `parser_generated.go`。两端共享以下契约：

- 相同的忽略规则、括号、四个关键字、整数、进制字面量和三类引号规则。
- 相同的状态编号常量 `21/21/0/21`。
- token 成功后都消费到 token 终点并增加逻辑位置；块不足时都保留未定型 token、续读后恢复扫描。
- 状态错误先记录语法上下文；真实 EOF 与意外 EOF 使用不同错误路径。
- Ragel 扫描器的最长匹配由 Rust 的 `prefer_over_unquoted` 显式表达，等长时保持专用规则顺序。

实现形态不同：Go 文件是真正由 Ragel 生成的跳转状态机，使用 `cs/ts/te/act/p` 恢复游标；Rust 文件用 `ScanResult` 和多个辅助函数重新表达同一规则。Go 在续读前消费 `ts` 之前的已确认前缀，而 Rust 的单次规则从缓冲开头重扫并只在 `Skip`/`Token` 后消费；二者外部结果应一致，但 Rust 对很长跨块 token 可能重复扫描已有前缀，存在潜在的时间复杂度风险。

独立 Rust 测试 `parser_generated_test.rs` 专门固定 `valuesx`、`123abc`、`0x12g` 等最长匹配以及等长专用规则。`parser_test.rs` 和 Go `parser_test.go` 则从完整行解析层覆盖块大小为 1 的续读、注释、`CONVERT` 包装、引号、进制字面量、伪关键字和语法错误。

## 扩展指南

- 新增或修改词法规则时，先更新语义源 `parser.rl`，再同步 Rust 的 `scan_one` 或对应辅助函数，并检查规则顺序与最长匹配；不要只修改状态常量或 Go 生成表。
- 新增 token 需要同步 `parser.rs::Token`、`ChunkParser::ReadRow` 的四态处理、Go token/解析逻辑，以及独立的 `parser_generated_test.rs`。若会改变最终 `Datum`，还应扩展同目录 `parser_test.rs` 和 Go `parser_test.go`。
- 新增大小写不敏感固定串应使用 `ascii_prefix` 并明确部分匹配的跨块行为；若它可能是普通文本前缀，必须通过 `prefer_over_unquoted` 或等价比较保护最长匹配。
- 修改引号或转义语义时要分别覆盖 `EscapeFlavor::None` 与 MySQL 模式、成对引号、块边界上的反斜杠/闭合符，并确认反引号仍不错误启用反斜杠转义。
- 修改分隔符集合时必须同时审查注释起始字符、`scan_unquoted` 与 Go `unquoted` 规则，否则专用规则和普通文本会产生不同切分。
- 性能优化应优先避免跨块重扫和 `drain` 搬移，但不能让返回的原始 token 借用随后会变化的缓冲；需要用独立基准或大 token 测试证明内存和时间改进。
- 测试逻辑继续放在独立文件 `parser_generated_test.rs`/`parser_test.rs`，不要内嵌进生产源文件。

## 验证依据

本说明基于以下直接证据：

- `pkg/lightning/mydump/parser_generated.rs`：`lex`、`scan_one`、全部扫描辅助函数与状态常量的完整实现。
- `pkg/lightning/mydump/parser.rs`：`BlockParser`/`ChunkParser`/`Token` 数据结构，`ReadRow` 第 339 行的直接调用，以及上层 `ReadChunks`、`ReadUntil`、`OpenReader` 链路。
- `pkg/lightning/mydump/lib.rs`：私有模块装配和 `parser_generated_test.rs` 的独立测试挂载。
- `pkg/lightning/mydump/Cargo.toml`：crate 名称、`lib.rs` 入口、无 feature 条件以及 Go 包移植元数据。
- `pkg/lightning/mydump/parser.rl` 与 `parser_generated.go`：Ragel 规则、状态编号、成功消费、续读、EOF 和错误分支的 Go 基准语义。
- `pkg/lightning/mydump/parser_generated_test.rs`：专用规则与普通文本之间的最长匹配、等长优先级。
- `pkg/lightning/mydump/parser_test.rs`、`pkg/lightning/mydump/parser_test.go`：完整行解析、最小块续读、注释/包装剥离、字面量与错误边界。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/lightning/mydump` 确认目标、入口、Go 对照和测试均已索引；精确 `node --file` 查询读取目标 371 行及上述调用文件。由于本任务是纯文档分析，按计划未运行 Cargo。

人工复核结论：该文件存在是为了以 Rust 实现 mydumper INSERT 输入的 Ragel 词法契约；它通过 `lex → scan_one → 辅助扫描函数` 运行，并由 `ChunkParser::ReadRow` 消费；安全扩展必须同步语义源、token 消费者、最长匹配及跨块独立测试。
