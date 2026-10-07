# `pkg/parser/hintparserimpl.rs`

## 文件定位

`hintparserimpl.rs` 是 `astersql-parser` crate 中优化器 Hint 的词法适配层和解析入口实现。它不是独立模块：`pkg/parser/lib.rs` 的私有 `lexer_support` 模块依次 `include!` 生成的 Hint token 表、`hintparser.rs`、本文件、通用 `lexer.rs` 与 `misc.rs`，因此本文件可以直接使用 `Scanner`、`yyhintLexer`、`yyhintSymType`、`yyhintParse`、`hintTokenMap` 及各类 `hint*` token。`lib.rs` 再以 `pub use lexer_support::{ParseHint, Pos}` 对外导出公开入口。

它位于两条调用路径的交汇处：外部调用者可直接调用 `ParseHint`；完整 SQL 解析则由 `pkg/parser/parser_actions/query.rs` 的 `QueryRule::TableOptimizerHintsAlt01` 取得 `hintComment` 文本、当前 SQL mode 和 Hint 起始位置，经 `Parser::parseHint` 复用一个 `hintParser`。两条路径最后都调用本文件的 `hintParser::parse`，产生 `Vec<Box<ast::TableOptimizerHint>>` 和诊断列表。

crate 边界由 `pkg/parser/Cargo.toml` 确认：包名为 `astersql-parser`，库入口是 `lib.rs`；本文件使用的 AST、MySQL 错误码和错误类型分别来自同工作区依赖 `parser-ast`、`parser-mysql`、`parser-terror`，并依赖 crate 内的通用 lexer 与生成解析器，而没有独立 feature 开关或条件编译项。

## 核心职责

本文件承担四项紧密相关的职责。

1. 用六个 `LazyLock<Box<terror::Error>>` 把 MySQL Hint 错误码注册为 parser 类错误模板，供词法层和 `hintparser.rs` 的语义动作共同生成兼容诊断。
2. 以 `hintScanner::Lex` 把通用 SQL `Scanner::scan` 返回的 token 映射为 Hint 文法 token，并把整数、标识符和字符串写入 `yyhintSymType`。
3. 用 `hintSetVarValueState` 限定 decimal/float token 只在 `SET_VAR(name = value)` 的值位置合法，避免放宽 `QB_NAME(1.5)` 等其他 Hint 的语法。
4. 管理 `hintParser` 的输入重置、位置校正、SQL mode、解析结果、诊断优先级与可复用语义栈，并提供公开的一次性入口 `ParseHint`。

本文件不定义 Hint 文法或 AST 规约动作。shift/reduce 循环及 AST 构造在 `pkg/parser/hintparser.rs`，token/动作表在 `pkg/parser/generated/hint_tables.rs`，通用字符扫描、括号深度限制和诊断容器在 `pkg/parser/lexer.rs`。

## 主要符号

- `ErrWarnOptimizerHintUnsupportedHint`、`ErrWarnOptimizerHintInvalidToken`、`ErrWarnMemoryQuotaOverflow`、`ErrWarnOptimizerHintParseError`、`ErrWarnOptimizerHintInvalidInteger`、`ErrWarnOptimizerHintWrongPos`：惰性初始化的 parser 错误模板。当前文件直接使用“不支持 Hint”“非法 token”“非法整数”；`hintparser.rs` 的规约动作使用内存配额溢出；位置与其他解析诊断由同一 include 作用域内的 lexer/解析代码使用。
- `hintScanner { scanner, setVarValueState }`：私有词法适配器。它组合通用 `Scanner`，并实现 `yyhintLexer` 所需的取词、错误格式化、诊断追加和诊断读取接口。
- `hintSetVarValueState::{None, AfterSetVar, AfterLParen, AfterName, ExpectValue, AfterSign}`：单 token 推进的有限状态机。状态只描述“下一个 decimal/float 是否位于 SET_VAR 值槽”，不保存变量名或值。
- `hintScanner::Errorf`：先调用 `Scanner::Errorf` 附加行列上下文，再以 `ErrParse.GenWithStackByArgs` 包装成 “Optimizer hint syntax error at” 诊断。
- `hintScanner::acceptSetVarNumericValue`、`updateSetVarValueState`、`returnToken`：分别判断数值值槽、推进/复位状态、保证正常 token 返回路径统一更新状态。
- `hintScanner::Lex`：本文件的核心词法入口，读取通用 token，执行括号深度检查、语义值填充、关键字映射、SQL mode 判定和非法 token 诊断。
- `impl yyhintLexer for hintScanner`：把生成解析器依赖的 trait 方法转发到 `hintScanner` 或其内部 `Scanner`。trait 定义在 `pkg/parser/hintparser.rs`。
- `hintParser { lexer, result, cache, yylval, yyVAL }`：私有解析状态。`result` 保存本轮 AST；后三项对应 goyacc 语义值/缓存形状，其中 `cache` 初始预分配 50 个槽并由 `yyhintParse` 回收更新。
- `newHintParser`：crate 内可见构造器，为 SQL `Parser` 的延迟复用路径和公开一次性路径创建解析器。
- `hintParser::parse`：重置一轮解析状态并调用 `yyhintParse` 的实际入口。
- `hintParser::warnUnsupportedHint`：构造不支持 Hint 的 warning 并追加到底层 scanner；当前 Rust 规约代码主要直接经 `yyhintLexer::AppendWarn` 实现相同行为，该方法仍保留与 Go `hintParser` 的接口对齐。
- `hintParser::lastErrorAsWarn`：把底层 scanner 最近一个非深度限制错误降级为 warning；`hintparser.rs` 在 `MEMORY_QUOTA` 乘法溢出时调用它。
- `ParseHint`：公开 API，每次新建 `hintParser` 后调用 `parse`；参数要求完整 `/*+ ... */` 文本、SQL mode 与注释起始 `Pos`。

## 执行流程

直接入口的流程是 `ParseHint -> newHintParser -> hintParser::parse -> yyhintParse -> yyhintlex1 -> hintScanner::Lex`。完整 SQL 流程则是通用 lexer 在合法位置扫描到 `/*+ ... */` 后返回 `token::hintComment` 并记录 `lastHintPos`，`TableOptimizerHintsAlt01` 调用 `Parser::parseHint`，然后进入相同的 `hintParser::parse`；解析出的 boxed AST 被解箱后写入 SQL AST，诊断逐项转成外层 parser warning。

`hintParser::parse` 每轮按以下顺序工作：

1. 清空旧 `result`，以 `input[3..]` 重置 scanner，跳过开头的三个字节 `/*+`；调用者必须满足这一输入前置条件。
2. 把 `setVarValueState` 重置为 `None`，设置 SQL mode，并把行号继承自 `initPos`、列号前移 3、切片内 offset 归零。
3. 设置 `inBangComment = true`，让保留下来的结尾 `*/` 按注释结束规则跳过，同时仍可用于错误/警告定位。
4. 暂时用 `std::mem::take` 移出 `self.lexer`，把 `&mut lexer` 和 `&mut self` 同时传给 `yyhintParse`，解析结束后再放回；这是为满足 Rust 可变借用规则，不是并发转移。
5. 若 scanner 有 error，只返回 errors；没有 error 时才返回 warnings。最后用 `std::mem::take` 移交本轮 `result`，保证复用解析器不会把 AST 留到下一轮。

`hintScanner::Lex` 先调用 `Scanner::scan` 并记录 `lastScanOffset`，随后调用 `updateParenthesesDepth`；深度超限时立即返回 `hintInvalid`，具体深度错误由通用 scanner 保存。正常分支中：

- `intLit` 必须能解析为 `u64`，成功写入 `lval.number`，失败追加非法整数错误并返回 `hintInvalid`。
- identifier 写入 `lval.ident`，以大写形式查 `hintTokenMap`；命中时变成专用 Hint 关键字，否则为 `hintIdentifier`，因此关键字匹配大小写不敏感但原文本仍保留。
- string literal 在 `ANSI_QUOTES` 且源字节确为双引号时作为 identifier，否则作为 `hintStringLit`。
- `0b...`、`0x...` 形式的 bit/hex literal 作为 identifier；引号形式 `b'...'`、`x'...'` 被归类为非法 token。
- `eq` 映射为 ASCII `'='`；ASCII 范围内的其他标点原样交给文法。
- decimal/float 仅在 `acceptSetVarNumericValue` 为真时写入 `lval.ident` 并返回 `hintNumericLit`，否则追加带有 token 分类、原文本和底层编号的非法 token 错误。

`yyhintParse` 在 `pkg/parser/hintparser.rs` 中消费这些 token。规约规则把结果写入 `parser.result`，构造 `TableOptimizerHint` 的 `HintData`、表和索引等字段；不支持的 Hint 产生 warning 且不产出 AST，`MEMORY_QUOTA` 溢出则先追加错误再由 `lastErrorAsWarn` 降级为 warning。

## 数据与状态

`hintScanner` 的长期状态来自内部 `Scanner`：输入 reader、SQL mode、当前位置、括号深度、errors/warnings 与注释模式；本文件新增的唯一词法状态是 `setVarValueState`。状态机的有效路径为 `None --SET_VAR--> AfterSetVar --(--> AfterLParen --name--> AfterName --=--> ExpectValue`，若随后是正负号则进入 `AfterSign`。`ExpectValue` 或 `AfterSign` 恰好让下一个 decimal/float 合法；任何消费后的值 token，以及不符合预期的标点、EOF 或其他 token，都会复位到 `None`。连续 `SET_VAR` token 在 `AfterSetVar` 保持该状态，以兼容 lexer/token 流形状。

`yyhintSymType` 是 token 与规约之间的语义载体：本文件填充 `number` 或 `ident`，`hintparser.rs` 再把这些值转成 `HintData::{Unsigned, Signed, SetVar, Name, CIStr, TimeRange, Leading, ...}`。`hintParser::result` 只属于当前一轮解析；`parse` 开始清空、结束移出。`cache`、`yylval`、`yyVAL` 是为生成解析器保留的状态，其中 `cache` 以 50 个默认值初始化，并在接受、错误或提前退出时接收解析值栈；它不影响跨请求的业务语义。

位置有两个坐标约定：`Line` 和 `Col` 延续外层 SQL 中注释起点，列号加 3 表示已跳过 `/*+`；`Offset` 对切片后的 Hint 正文重新从 0 开始。所有字符串 offset 都按字节解释，ANSI_QUOTES 分支也通过 `as_bytes()` 检查原始引号。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 与源码交叉确认：

- `pkg/parser/lib.rs` 公开再导出 `ParseHint`，`pkg/parser/hintparser_test.rs`、`pkg/parser/go_merge_29_test.rs` 和外部 crate 用户可直接调用。
- `pkg/parser/yy_parser.rs::Parser::parseHint` 延迟创建并复用 `hintParser`。
- `pkg/parser/parser_actions/query.rs::TableOptimizerHintsAlt01` 是完整 SQL 文法的直接桥接点，向 `parseHint` 传入 comment 文本、`yylex.sql_mode_bits()` 和 `yylex.hint_position()`，再把返回诊断追加为外层 warning。

下游依赖关系为：

- `Scanner::scan`、`updateParenthesesDepth`、`SetSQLMode`、`Errors`、`lastErrorAsWarn` 和 reader 位置更新来自 `pkg/parser/lexer.rs`。
- `hintTokenMap` 来自 `pkg/parser/misc.rs`，把诸如 `SET_VAR`、`HASH_JOIN` 的大写名称映射到专用 token。
- `yyhintLexer`、`yyhintSymType`、`yyhintParse` 和规约动作来自 `pkg/parser/hintparser.rs`；数值 token 常量及 LR 表来自 `pkg/parser/generated/hint_tables.rs`。
- AST 目标类型是 `parser_ast::TableOptimizerHint` 及其 `HintData`；SQL mode 和错误码来自 `parser_mysql`，错误实例与堆栈参数化来自 `parser_terror`。

RustCodeGraph 的符号 trail 明确记录：Rust `ParseHint` 调用 Rust `newHintParser` 和 `hintParser::parse`；`newHintParser` 还被 `yy_parser.rs::parseHint` 调用；`acceptSetVarNumericValue` 由 `Lex` 调用，`updateSetVarValueState` 由 `returnToken` 调用。对 include 文件中同名 Go/Rust 符号，图有少量跨语言或同名边误配，因此调用结论以带路径的 trail 和实际源码共同限定。

## 错误处理与边界

词法/语法问题不通过 Rust `Result` 返回，而是累积在 `Scanner` 的 `errs`/`warns` 中。语法错误由 `hintScanner::Errorf` 增加外层 Hint 语境；非法整数、非法 token 和括号深度超限均进入 errors。`parse` 的返回规则是“errors 优先”：只要存在 error，warnings 不会出现在本次返回诊断切片；没有 error 时返回 warnings。这与 Go 实现中 `if len(errs) == 0 { errs = warns }` 一致。

明确边界包括：

- `parse` 无条件执行 `input[3..]`，因此它依赖调用者传入至少三字节且以 ASCII `/*+` 开头的完整 Hint 注释；当前公开 API 和 SQL lexer 路径都遵守此约定，但函数本身不验证前缀，错误输入可能触发切片 panic。
- 整数语义槽是 `u64`；超范围输入产生 `ErrWarnOptimizerHintInvalidInteger` 并继续以 `hintInvalid` 交给 parser 恢复。
- decimal/float 的放行严格受 `SET_VAR` 状态控制。`pkg/parser/hintparser_test.rs` 验证 `SET_VAR(timestamp = 1.5)` 成功，而 `QB_NAME(1.5)` 同时得到 “Cannot use decimal number” 和语法错误。
- bit/hex 仅允许 `0b`/`0x` 前缀形态充当 identifier；引号形态被拒绝。`ANSI_QUOTES` 会改变双引号字符串的 token 类别。
- 括号最大深度由通用 scanner 实施；`go_merge_29_hint_depth_limit_public_api` 验证 10,000 层输入返回深度错误而非无限递归。
- `MEMORY_QUOTA` 的单位乘法由规约动作检查 `i64::MAX / unit`；溢出诊断被降级为 warning 且该 Hint 不进入 AST。通用 `lastErrorAsWarn` 不会降级深度限制错误。
- 不支持的、但被文法识别的 Hint 形态只产生 warning 并省略 AST；真正未知或结构错误的输入走语法 error。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有解析数据由调用栈上的可变引用和拥有所有权的 `String`/`Vec`/`Box` 管理；`LazyLock` 只负责错误模板的一次性线程安全初始化。

`ParseHint` 每次创建全新的 `hintParser`，天然没有请求间共享状态。完整 SQL `Parser` 会在自身内部复用一个 boxed `hintParser`，但 `Parser` 以 `&mut self` 串行调用它；`parse` 在每轮清空 `result`、重置 scanner 输入与 `SET_VAR` 状态，并在结束时移出结果。`std::mem::take(&mut self.lexer)` 只是在一次同步调用内临时转移所有权，以便同时把 lexer 与 parser 可变传给生成状态机，调用返回后立即恢复。

扩展时必须维持“每轮重置所有跨输入状态”的不变量。若给 `hintScanner` 或 `hintParser` 新增字段，应同时决定它是可复用缓存还是请求语义状态；后者必须在 `parse` 开头重置，避免前一条 SQL 的 mode、诊断或 token 上下文泄漏。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/hintparserimpl.go`。Rust 保留了 Go 的六个错误模板、`hintScanner`/`hintParser` 角色、六态 `SET_VAR` 有限状态机、token 映射分支、50 槽初始缓存、跳过 `/*+` 的位置规则、`inBangComment` 行为、errors 优先返回规则以及 `ParseHint`/`warnUnsupportedHint`/`lastErrorAsWarn` 接口意图。

主要语言层差异如下：

- Go 通过嵌入 `Scanner` 暴露方法和字段；Rust 通过 `scanner: Scanner` 组合并在 `yyhintLexer` impl 中显式转发。
- Go 的局部 `returnToken` 闭包在 Rust 中变为方法，以避免闭包对 `&mut self` 的借用与其他分支冲突。
- Go 以 map 查询大写关键字；Rust 当前在 `hintTokenMap` 切片上 `iter().find`。语义一致，但关键字数增长时这是需要关注的线性查找性能点。
- Go 的 `yyVAL` 是语义栈指针；Rust 字段是 `Option<usize>`，实际纯 Rust shift/reduce 循环主要维护本地 `states`/`values`，同时保留字段和 `cache` 以维持生成解析器形状。
- Rust 为同时把 lexer 与 parser 传入 `yyhintParse` 使用 `mem::take`；Go 可直接传 `&hp.lexer, hp`。
- Go 的 `warnUnsupportedHint` 被生成 `hintparser.go` 直接调用；当前 Rust `reduce_hint` 对相关规则直接调用 trait 的 `AppendWarn`，所以本文件同名方法主要是对齐保留，而 `lastErrorAsWarn` 仍被 Rust 规约直接调用。

Rust 独立测试 `pkg/parser/hintparser_test.rs` 是 `hintparser_test.go` 的对应测试，覆盖主要 Hint AST、SQL mode、非法 token、整数溢出和 `SET_VAR` 小数/符号；`pkg/parser/go_merge_29_test.rs` 对应 Go 的最大 Hint 深度回归；`pkg/parser/yy_parser_test.rs` 与 `pkg/parser/parser_test.rs` 进一步验证完整 SQL 路径传递实时 mode/位置、产生 warnings 并把 Hint 写入 `SelectStmt.TableHints`。这些测试是独立文件，符合 Rust 生产逻辑与测试分离要求。

## 扩展指南

新增或修改 Hint 时，应先判断改动属于哪一层：

- 只增加 Hint 关键字：更新产生 `hintTokenMap`/token 的语法或生成输入及生成表，不应在 `Lex` 堆叠专用字符串分支；随后核对 `pkg/parser/generated/hint_tables.rs` 和 `pkg/parser/hintparser.rs` 的生成结果。
- 改变字面量准入规则：修改 `hintScanner::Lex`；若规则依赖上下文，扩展 `hintSetVarValueState`、`acceptSetVarNumericValue` 和 `updateSetVarValueState`，并保证所有 token 返回路径仍经 `returnToken`。不要全局允许 decimal/float，否则会破坏 `QB_NAME(1.5)` 等兼容边界。
- 改变 AST 载荷或语法：修改文法/生成器对应的规约动作，而非把 AST 构造塞进本文件；本文件应继续只负责 lexer 适配和解析生命周期。
- 改变诊断：保持 MySQL 错误码、错误与 warning 级别、errors 优先规则以及行列/字节 offset 与 Go 一致。新增降级路径时确认不会吞掉深度限制错误。
- 改变 parser 复用：在 `hintParser::parse` 明确重置新增请求状态，并验证连续解析不会复用旧结果、warning、SQL mode 或 `SET_VAR` 上下文。

测试应同步放在独立文件：词法/公开 API 与 AST 形状优先扩展 `pkg/parser/hintparser_test.rs`；完整 SQL 接线路径扩展 `pkg/parser/parser_test.rs` 或 `pkg/parser/yy_parser_test.rs`；Go 语义变化同时更新 `pkg/parser/hintparserimpl.go` 与 `pkg/parser/hintparser_test.go` 的对应断言。关键兼容风险是 token 分类、诊断顺序/级别、字节位置和 AST 字段；主要性能风险是关键字线性查找和解析栈分配。任何生成表变化还应走 parser 专用生成与验证流程，但本分析任务本身不修改或运行生成器。

## 验证依据

本说明使用以下直接证据核对，而未把架构概览当作实现事实：

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`pkg/parser/hintparserimpl.rs` 被识别为含 31 个符号的 Rust 文件。通过 `node --file` 阅读了该文件全部 326 行。
- RustCodeGraph 符号查询：查询了 `ParseHint`、`newHintParser`、`hintScanner`、`Lex`、`acceptSetVarNumericValue`、`updateSetVarValueState`、`warnUnsupportedHint`、`lastErrorAsWarn` 和 `parseHint`；trail 显示公开入口、复用入口、状态机和词法方法之间的上述调用边。
- crate/装配证据：`pkg/parser/Cargo.toml`，以及 `pkg/parser/lib.rs` 中 `lexer_support` 的 include 顺序和 `ParseHint` 再导出。
- 运行链证据：`pkg/parser/yy_parser.rs::Parser::parseHint`、`pkg/parser/parser_actions/query.rs::TableOptimizerHintsAlt01`、`pkg/parser/hintparser.rs::{yyhintLexer, yyhintParse, reduce_hint}`、`pkg/parser/generated/hint_tables.rs`、`pkg/parser/lexer.rs` 与 `pkg/parser/misc.rs`。
- Go 对照证据：完整阅读 `pkg/parser/hintparserimpl.go`，并核对 `pkg/parser/hintparser_test.go::TestParseHint`、`TestMaxOptimizerHintDepth` 及生成 `pkg/parser/hintparser.go` 的相关规约动作。
- Rust 测试证据：完整阅读 `pkg/parser/hintparser_test.rs`；核对 `pkg/parser/go_merge_29_test.rs::go_merge_29_hint_depth_limit_public_api`、`pkg/parser/yy_parser_test.rs::hint_uses_live_scanner_mode_and_position`、`pkg/parser/parser_test.rs::test_optimizer_hints` 及相邻不支持 Hint/嵌套 SELECT 测试。
- `pkg/parser` 下不存在 `doc.go`，因此没有可额外读取的包级 Go 契约文件。

本任务是纯文档分析，按计划不运行 Cargo 或代码测试。交付前以任务指定命令确认目标文件存在且恰好包含上述十一个固定二级标题，并人工复核本文能够回答文件存在原因、执行链、状态与诊断边界、Go 对照和安全扩展位置。
