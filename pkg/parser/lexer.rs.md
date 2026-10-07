# `pkg/parser/lexer.rs`

## 文件定位

`pkg/parser/lexer.rs` 是 `astersql-parser` crate 的 SQL 词法扫描实现。它不是独立模块：`pkg/parser/lib.rs` 在私有的 `parser_impl::lexer_support` 中依次 `include!` 生成 token、Hint 解析器、关键字表、本文件和 `misc.rs`，因此本文件可以直接使用 `yySymType`、`yyLexer`、`token`、`ruleTable`、`toInt` 等同一模块内符号。crate 边界由 `pkg/parser/Cargo.toml` 定义，字符集、MySQL SQL Mode、错误体系、AST 驱动值分别来自 `parser-charset`、`parser-mysql`、`parser-terror`、`parser-test-driver` 等依赖。

应用主链中的位置是 `Parser::ParseSQL`（`pkg/parser/yy_parser.rs`）重置 `Scanner`、应用连接参数和解析配置，再把扫描器交给 `yyParse`；生成的语法分析器通过 `yyLexer::Lex` 逐个取得 token 和语义值。`Scanner` 位于私有 `lexer_support` 中且没有从 crate 根重新导出，正常外部入口是 `Parser`；`Pos` 则由 `parser_impl` 重新导出，供 Hint 位置等接口使用。

## 核心职责

- `Scanner::scan` 完成原始分词：跳过空白，识别扩展标识符，沿 `misc.rs` 的 `ruleTable` trie 做最长前缀匹配，并把特殊首字节分派给本文件的 `startWith*`/`scan*` 函数。
- `Scanner::Lex` 在原始 token 上叠加语法相关语义：关键字识别、复合 token 合并、SQL Mode 改写、括号深度限制、字面量值构造、字符集转换，并填写 `yySymType`。
- `startWithSlash`、`startWithDash`、`startWithSharp`、`startWithStar` 处理普通注释、版本注释、TiDB 特性注释、优化器 Hint 和 JSON 路径运算符等有上下文的词法分支。
- `scanString`、`scanQuotedIdent`、`startWithNumber`、`scanFloat`、`startWithAt` 等实现 MySQL 兼容的字符串、标识符、数值及用户/系统变量规则。
- `reader` 与 `Pos` 维护字节偏移、行和列；`Scanner` 同时收集 warning/error，并保存关键字历史、Hint 位置、字符集和 SQL Mode 等跨 token 状态。

## 主要符号

- `Pos { Line, Col, Offset }`：token 起点的位置。`Offset` 是 SQL 字节偏移；`reader::inc` 在换行时同步维护行列。
- `Scanner`：核心有状态扫描器。公开行为包括 `Errors`、`Errorf`、`AppendError`、`AppendWarn`、`Lex`、`LexLiteral`、`SetSQLMode`、`GetSQLMode`、`EnableWindowFunc` 和 `InheritScanner`；`reset`、`scan` 及各扫描辅助函数仅供解析器模块内部使用。
- `NewScanner`、`Scanner::empty`、`Scanner::reset`：分别负责构造、零状态初始化和一条新 SQL 的瞬时状态重置。`reset` 保留调用方配置的 `sqlMode`、`supportWindowFunc`、`skipPositionRecording` 等，但重设输入、默认字符集、诊断、关键字历史和括号深度。
- `Scanner::Lex`：主词法入口，返回 `i32` token；`impl yyLexer for Scanner` 的 `Lex` 仅做类型桥接，把结果转成生成解析器需要的 `isize`。
- `Scanner::LexLiteral`：扫描一个 token，并以 `Box<dyn Any>` 保留 `i64`、`u64`、`f64`、decimal、hex/bit 驱动值或字符串，而不是统一字符串化。
- `Scanner::scan`：底层调度器；其 trie 来自 `pkg/parser/misc.rs::ruleTable`。`ruleTable` 把引号、数字、点号、`@`、注释起始符等连接到本文件函数。
- `startWithSlash`：注释总入口。它借助 `lastKeyword*` 和 `keepHint` 判断 Hint 是否处于合法位置，用 `tidbfeature::CanParseFeature` 判断 `/*T![...]` 正文是否展开。
- `MAX_PARENTHESES_DEPTH`、`updateParenthesesDepth`：限制词法层括号嵌套为 10,000；超限时追加解析错误并返回 `invalid`。
- `reader`：持有输入 `String`、当前位置和总字节数；`skipRune` 结合客户端编码宽度与 Rust UTF-8 字符边界推进，避免按非法边界切片。
- `lazyBuf`：与 Go 文件一样保留了延迟缓冲数据结构，但当前两端的 `scanString` 都实际使用扫描器自带的复用缓冲；扩展时不能假定 `lazyBuf` 已接入活跃路径。

## 执行流程

1. `Parser::ParseSQL` 调用 `Scanner::reset`，应用 `CharsetConnection`、`CollationConnection` 等参数，并用 `InheritScanner` 做深表达式的预扫描；随后 `yyParse` 经 `yyLexer` trait 驱动正式扫描（`pkg/parser/yy_parser.rs`）。
2. `Scanner::Lex` 调用 `scan`。`scan` 跳过空白，记录起始 `Pos`，对非 ASCII 起始字节走标识符路径，否则沿 `ruleTable` 查找固定符号、复合运算符或扫描函数。
3. `Lex` 先用 `updateParenthesesDepth` 维护深度，再滚动 `lastKeyword3/2/1`。标识符先经 `handleIdent` 识别 `_charset` introducer，再由 `misc.rs::isTokenIdentifier` 区分普通关键字、内建函数和可选窗口函数 token。
4. 需要前瞻的语法形态通过克隆 `reader` 实现无副作用预读：`getNextToken`/`getNextTwoTokens` 扫描后恢复位置，用于 `AS OF`、`MEMBER OF`、`FULL OUTER JOIN`、`TO TIMESTAMP/TSO` 和 `OPTIONALLY ENCLOSED BY` 等复合 token。
5. SQL Mode 在 token 层改写行为：`ANSI_QUOTES` 把双引号字符串视为标识符，未启用 `PIPES_AS_CONCAT` 时把 `||` 改为 OR，`HIGH_NOT_PRECEDENCE` 改写 `NOT`，`NO_BACKSLASH_ESCAPES` 控制字符串转义。
6. 数值 token 交给 `pkg/parser/yy_parser.rs` 的 `toInt`、`toDecimal`、`toFloat`、`toHex`、`toBit` 构造具体语义值；标识符经 `convert2System` 解码为系统 UTF-8，字符串经 `convert2Connection` 按客户端/连接字符集转换。
7. `yyParse` 消费 token 并规约 AST；完成后 `Parser::ParseSQL` 从扫描器取回 warnings/errors，若词法/语法状态异常则返回首个错误，否则继续 AST 深度校验和标志设置。

注释分支有独立子流程：`#` 和满足 MySQL 空白条件的 `--` 消费到换行；普通 `/*...*/` 被跳过；`/*!` 跳过最多五位版本号后展开正文；`/*T![ids]` 仅在特性列表可解析时展开；合法位置的 `/*+...*/` 返回 `hintComment` 并记录 `lastHintPos`，非法位置产生 warning 或被当普通注释跳过。

## 数据与状态

`Scanner` 的状态可分为四组：输入与缓冲（`r`、`buf`）、编码和模式（`client`、`connection`、`sqlMode`、`supportWindowFunc`）、诊断与位置（`errs`、`warns`、`stmtStartPos`、`lastScanOffset`、`lastHintPos`、`skipPositionRecording`），以及上下文判定（`inBangComment`、三层 `lastKeyword`、`identifierDot`、`keepHint`、`parenDepth`、`depthLimitError`）。

关键不变量如下：

- `reader.p.Offset` 以字节计数，所有 `data` 切片必须落在 Rust `String` 的 UTF-8 边界；`skipRune` 为非 ASCII 输入校验编码报告的宽度，必要时退回实际 UTF-8 字符宽度。
- `getNextToken` 和 `getNextTwoTokens` 必须恢复整个 `reader`，所以前瞻不能消费正式输入；它们只用于判定复合 token。
- `identifierDot` 使点号后的数字按标识符处理，避免限定名中的数字段被误判为数值；读取星号时会清除此状态。
- `Errors` 的语义是先 warnings 后 errors。`lastErrorAsWarn` 只移动最近一个普通错误，括号/AST 深度错误必须保持致命；`depthLimitError` 记录本扫描器产生的括号深度错误索引。
- `InheritScanner` 只继承客户端编码、SQL Mode 和窗口函数开关；子扫描器的输入、诊断、缓冲和位置均为新状态。连接编码没有在该函数中继承，这是当前实现和 Go 对照共同约定的实际行为。

## 依赖与调用关系

上游主调用边：

- `pkg/parser/yy_parser.rs::Parser::ParseSQL` → `Scanner::reset` → `check_expression_depth_before_parse`/`yyParse`。
- `check_expression_depth_before_parse` → `Scanner::InheritScanner` → `Scanner::Lex`，用于正式规约前的高深度表达式保护。
- 生成解析器 `yyParse` → `yyLexer::Lex` → `Scanner::Lex`；`yyLexer` 的错误、语句文本、Hint 位置和 SQL Mode 方法也由本文件实现。
- `pkg/parser/hintparser.rs` 的 Hint lexer 复用 `Scanner::Lex`；`hintparserimpl.rs` 把 SQL Mode 传给内部扫描器。

主要下游边：

- `Scanner::scan` → `pkg/parser/misc.rs::ruleTable` → 本文件的 `startWith*`/`scan*` 分派函数；关键字解析调用 `isTokenIdentifier`，字符分类调用 `isIdentChar`、`isIdentExtend`、`isUserVarChar`、`isDigit`。
- `Scanner::Lex` → `pkg/parser/yy_parser.rs::{toInt,toDecimal,toFloat,toHex,toBit}`，后者依赖 `parser-test-driver` 构造精确字面量值。
- 编码转换调用 `parser_charset::encoding::EncodingRef::Transform/MbLen`；SQL Mode 和默认字符集来自 `parser-mysql`；错误由 `errors`/`terror` 体系构造。
- 特性注释调用 `pkg/parser/tidb/features.rs::CanParseFeature`；字符串转义调用 `pkg/parser/util/escape.rs::UnescapeChar`。

RustCodeGraph 的文件查询确认 `lexer.rs` 被 `lib.rs`/`yy_parser.rs` 所在解析模块使用，并给出 `Lex` 到 `scan`、前瞻、编码转换、深度检查及各 `to*` 函数的调用边。由于图对同名 trait 方法的 caller 解析存在歧义，主链同时以 `lib.rs` 的 `include!` 关系和 `yy_parser.rs::ParseSQL`/`yyParse` 源码核验。

## 错误处理与边界

- EOF 返回 token `0`；未闭合字符串、反引号标识符、非法 `X'...'`/`B'...'` 返回 `invalid`。未闭合块注释追加 `ParseErrorWith` 后返回 EOF。
- `Errorf` 从 `lastScanOffset` 生成 line/column/near 上下文，附近 SQL 最多取 2048 字节；这与 `ParseErrorWith` 的另一条错误格式化路径不同。
- 客户端编码解码失败时，`convert2System` 使用替换输出并记录 warning；`convert2Connection` 先记录 error，在严格模式且客户端/连接编码相同时直接返回 `invalid`，其他情况把最近错误降级为 warning 后继续替换转换。
- 整数超过 `u64` 时转为 decimal；decimal 构造失败保留默认 decimal 并记录错误；非有限浮点、非法 hex/bit 返回 `invalid` 并追加错误。具体逻辑位于 `yy_parser.rs` 的 `to*` 函数。
- 括号深度第 10,001 层返回 `invalid`，错误不能被 `lastErrorAsWarn` 降级。多余右括号不会让计数下溢，只有深度大于零才递减。
- `--` 只有在其后为 EOF 或空白时才是注释；否则首个 `-` 是普通减号。优化器 Hint 只有在 `hintedTokens` 指定关键字后或显式 `keepHint` 时返回 token，错误位置会产生 warning。
- 数字后紧邻标识符字符时可能回退为 identifier；`.digits` 的非法指数路径由 `startWithDot` 转为 `invalid`。这些兼容分支不可用通用 Rust 数值解析提前替代。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。`Scanner` 以可变独占引用推进，单个实例代表一次顺序扫描状态；代码没有声明或承诺同一实例可并发使用。包级 `ruleTable` 使用 `std::sync::LazyLock`，首次访问时一次性构建，之后只读共享。

每次 `Parser::ParseSQL` 都调用 `reset` 清理输入相关状态，但复用扫描器对象和缓冲容量；正式解析时 `std::mem::take` 暂时把扫描器移出 `Parser`，`yyParse` 结束后再放回。`getNextToken`/`getNextTwoTokens` 克隆 `reader` 并在前瞻结束时恢复；由于 `reader` 直接拥有 `String`，当前克隆会复制输入字符串，而不只是复制位置。`InheritScanner` 为预扫描创建独立扫描器。所有字面量、错误和字符串缓冲均由 Rust 所有权自动释放，无显式 close 生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/lexer.go`，测试基线是 `pkg/parser/lexer_test.go`。Rust 保留了 Go 的主要符号顺序和行为：`Pos`/`Scanner` 字段、`Lex` 两层扫描、规则 trie、注释和 Hint 状态机、SQL Mode、字符集转换、变量扫描、版本/特性注释、动态字面量类型以及 10,000 层括号限制。`pkg/parser/lexer_5_aster_unit_test.rs` 逐表复刻 Go token、值、位置和模式用例；`pkg/parser/lexer_test.rs` 通过公开 `Parser`/digester 做集成覆盖；`pkg/parser/go_merge_29_lexer_test.rs` 验证后续 Go 合并带来的括号深度与 token 映射。

需要明确的实现差异：

- Go `NewScanner` 返回指针，Rust 返回拥有所有权的 `Scanner`；Rust 再通过 `yyLexer` trait 适配生成解析器。
- Go 输入字符串允许任意字节并由编码器决定 rune 宽度；Rust 输入是合法 UTF-8 `String`。Rust `reader::skipRune` 增加字符边界回退，相关测试覆盖 GBK/GB18030 配置下不切裂 UTF-8 输入。
- Go 的复用缓冲是 `bytes.Buffer`，Rust 对应为 `Vec<u8>`；两端当前 `scanString` 都走各自的扫描器缓冲。两份文件虽都定义 `lazyBuf`，但它不在当前活跃调用路径上。
- Go 用 `parserDepthLimitError` 类型识别不可降级错误；Rust 用 `depthLimitError: Option<usize>` 并辅以错误消息检查，保证深度错误保持 fatal。
- Rust 的公开集成测试因 `Scanner` 所在模块私有，部分断言通过 `Parser` 和 digester 间接验证；精确 token/offset/type 断言放在 `lexer_5_aster_unit_test.rs` 等独立测试文件中，而不是嵌入生产源码。

## 扩展指南

- 新增固定符号或多字符运算符：修改 `pkg/parser/misc.rs::buildRuleTable`；若需要状态机，新增本文件 `startWith*` 函数并在 rule table 注册。同步更新生成 token/语法定义所要求的来源，而不是只在 lexer 中发明 token。
- 新增或改变关键字：优先修改关键字/token 的生成来源和 `misc.rs` 映射；确认 `supportWindowFunc`、内建函数括号判定及三层关键字历史是否受影响。
- 新增复合关键字：在 `Scanner::Lex` 中沿用无副作用前瞻模式，保证恢复 reader，并同步维护 `v.ident`、`v.offset`、`lastScanOffset` 和 `lastKeyword`。
- 修改字符串、数字或字符集行为：同时检查 `scanString`/`startWithNumber`/`scanFloat`、`convert2System`/`convert2Connection`、`reader::skipRune` 和 `yy_parser.rs::to*`；风险包括 UTF-8 切片 panic、MySQL 类型差异、严格模式错误等级变化和大字面量精度丢失。
- 修改注释或 Hint：检查 `inBangComment`、`lastKeyword*`、`keepHint`、`lastHintPos` 以及 `tidbfeature::CanParseFeature`；普通注释、版本注释、特性注释和 Hint 的结束符行为彼此不同。
- 修改状态重置或继承：明确哪些字段属于调用方配置、哪些属于单条 SQL 瞬时状态；遗漏清理会让复用的 `Parser` 泄漏前一条语句状态，过度清理则会丢失 SQL Mode/窗口函数配置。
- 测试必须放在独立文件。首选同步 `pkg/parser/lexer_5_aster_unit_test.rs` 的精确内部断言及 `pkg/parser/lexer_test.rs` 的公开入口回归；Go 语义变化还要对照 `pkg/parser/lexer_test.go`，深度/token 合并场景检查 `pkg/parser/go_merge_29_lexer_test.rs`。本仓库规则禁止把测试模块写入 `lexer.rs`。

兼容性风险集中在 token 编号/分类、语义值具体类型、字节位置、warning/error 等级及 SQL Mode 分支；性能风险集中在输入字符串克隆、每 token 分配、字符集转换和 trie/关键字查找。扩展后应优先添加最小的 token 序列、语义值类型、offset 和诊断顺序断言。

## 验证依据

- 完整阅读：`pkg/parser/lexer.rs`（1157 行）；模块与 crate 边界：`pkg/parser/lib.rs`、`pkg/parser/Cargo.toml`。
- 主链与字面量转换：`pkg/parser/yy_parser.rs::check_expression_depth_before_parse`、`Parser::ParseSQL`、`toInt`、`toDecimal`、`toFloat`、`toHex`、`toBit`；分派与字符分类：`pkg/parser/misc.rs::buildRuleTable`、`ruleTable`、`isTokenIdentifier`。
- Go 对照：`pkg/parser/lexer.go`、`pkg/parser/lexer_test.go`；Rust 独立测试：`pkg/parser/lexer_test.rs`、`pkg/parser/lexer_5_aster_unit_test.rs`、`pkg/parser/go_merge_29_lexer_test.rs`。
- RustCodeGraph：`status` 显示索引含目标文件；`node --file pkg/parser/lexer.rs --symbols-only` 枚举 `Pos`、`Scanner`、`Lex`、所有扫描分支、`reader` 等符号；分段 `node --file` 核对全部源码；`callees Lex --file pkg/parser/lexer.rs` 验证到 `scan`、前瞻、编码转换、括号深度及 `yy_parser.rs::to*` 的边；`callees NewScanner` 验证构造后调用 `reset`。图未解析出的同名 trait caller 以 `lib.rs` 与 `yy_parser.rs` 源码补证。
- 人工核对的边界用例来自上述测试：变量前缀、字符串转义、ANSI_QUOTES/NO_BACKSLASH_ESCAPES、非法未闭合 token、普通/版本/特性注释、Hint 合法位置、Unicode/客户端编码、数值类型与 10,001 层括号错误。
- 本任务为纯文档分析，按计划不运行 Cargo；结构验收使用任务文件指定的 11 章节检查命令。
