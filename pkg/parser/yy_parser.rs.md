# `pkg/parser/yy_parser.rs`

## 文件定位

`yy_parser.rs` 是 `astersql-parser` crate 的公开解析门面和 goyacc 运行时适配层。它本身不保存完整 SQL 文法或 LALR 表：`pkg/parser/lib.rs` 在 `parser_impl` 中先 `include!("yy_parser.rs")`，再包含 `generated/main_tables.rs`、`parser.rs` 和其他实现，因此本文件中的私有辅助函数、`Parser` 状态以及后续包含进来的 `yyParse`、词法器和语义动作处于同一 Rust 模块中。

它位于 SQL 文本与 AST 之间：上游（例如 `pkg/session/runtime.rs`、`pkg/session/hint_runtime.rs`、`dumpling/export/schema_projection.rs`）创建 `Parser` 并调用 `ParseSQL` 或 `ParseOneStmt`；下游由 `Scanner` 产出 token，`pkg/parser/parser_runtime.rs` 的 `yyParse` 根据生成表移进/规约，`pkg/parser/parser_actions/*` 构造 `parser_ast` 节点。本文件最终负责收集警告/错误、检查深度、设置表达式标志并返回 AST。

crate 边界由 `pkg/parser/Cargo.toml` 确认：库入口为 `lib.rs`，AST、字符集、MySQL 常量、terror 错误、类型与 parser driver 分别来自同工作区的 `astersql-parser-*` 子 crate；此外直接使用 `regex`。本文件没有条件编译项；测试由 `lib.rs` 中独立的 `yy_parser_test.rs`、`yy_parser_4_aster_unit_test.rs` 等模块包含，测试逻辑未内嵌在生产文件中。

## 核心职责

1. 定义解析器可见的 MySQL/TiDB 错误类：`ErrSyntax`、`ErrParse`、字符集/排序规则错误、字段宽度/精度错误、弃用语法警告等，均通过 `LazyLock` 延迟建立 terror 错误对象。
2. 管理可复用的 `Parser` 实例及其配置、词法器、AST 结果、goyacc 符号缓存、最近一次警告和 hint parser。
3. 实现公开入口 `New`、`ParseSQL`、`ParseSQLWithWarnings`、`Parse`、`ParseOneStmt`，把参数应用、词法分析、LALR 规约、诊断和 AST 后处理串成完整流程。
4. 提供语法动作所需的兼容辅助：特殊注释裁剪、源码偏移、hint 解析、整数/十进制/浮点/十六进制/位字面量转换，以及动态数值向 `u64`/`i64` 的转换。
5. 在解析前后限制用户可控的 AST 深度，避免生成规约或后续递归访问在极深输入上耗尽 Rust 调用栈。
6. 用 `ParseParam` 把连接字符集、连接排序规则和客户端字符集按调用次序施加到每次解析。

## 主要符号

- 错误与正则：公开的 `ErrSyntax`、`ErrParse`、`ErrUnknownCharacterSet`、`ErrUnknownCollation` 等静态错误保持 MySQL 错误码映射；`SpecFieldPattern` 识别特殊注释边界，私有 `specCodeStart`/`specCodeEnd` 供 `TrimComment` 去掉 `/*!...*/` 包装。
- `ParserConfig`：四个公开开关分别控制窗口函数、严格 DOUBLE 类型检查、位置记录和“TiDB 不支持但工具可能需要接受”的 MySQL 语法。`SetParserConfig` 将这些值写入解析器和词法器。
- `Parser`：核心状态容器。`charset`/`collation` 和 `src` 保存当前解析上下文；`lexer`、惰性 `hintParser` 负责扫描；`result` 保存本轮 AST；`reducedStatementCount` 与 `allStatementsSemanticallyComplete` 校验规约是否真的产生了完整语句；`cache`、`yylval`、`yyVAL` 是 goyacc 栈与临时值；`lastWarnings` 保留最近一轮诊断。字段私有，外部通过方法配置和读取结果。
- `New() -> Box<Parser>`：初始化 parser driver，建立容量为 200 的符号缓存，然后调用内部 `reset` 载入默认 SQL mode 与开关。`Reset` 还会清空缓存中已复用的 `yySymType`。
- `ParseSQL`：主入口，返回“语句列表 + 警告”或错误；`ParseSQLWithWarnings` 在失败时也返回保存于 `lastWarnings` 的警告，模拟 Go 的三返回值契约；`Warnings` 返回警告克隆，避免暴露内部 Vec。
- `Parse`：用 `CharsetConnection` 和 `CollationConnection` 包装传统参数后委托 `ParseSQL`；`ParseOneStmt` 同样委托，但强制结果恰为一条，否则生成 `ErrSyntax`。
- 深度保护：`MAX_AST_DEPTH` 为 10,000 加 64 层语句包装余量；`check_expression_depth_before_parse` 通过继承的 Scanner 预检 CASE、连续一元 `!` 和连续 `+` 表达式；`AstDepthChecker`/`check_ast_depth_limit` 在 AST 生成后执行权威检查，并对连续括号链做迭代下钻。
- 语法动作辅助：`yySetOffset` 只在规约值含表达式时写入原文位置；`yyhintSetOffset` 当前为空操作；`startOffset`、`endOffset` 保持 Go 字节下标语义；`parseHint` 复用 hint parser，并接收当前 mode/位置。
- 字面量辅助：`toInt` 在 `u64` 范围内区分 `i64`/`u64`，正溢出转 `toDecimal`；`toFloat` 拒绝无穷值；`toHex`/`toBit` 委托 parser driver；`getUint64FromNUM` 与 `getInt64FromNUM` 模拟 Go 类型 switch。
- `ParseParam` 及三个实现：`CharsetConnection` 设置字面量默认字符集及 lexer connection encoding，`CollationConnection` 设置排序规则，`CharsetClient` 设置输入解码。`resetParams` 确保每次 `ParseSQL` 先恢复默认 charset/collation。
- `stmtTexter`：包内接口，只约定 `stmtText() -> String`，供同模块包含的生成/迁移代码使用；不是 crate 公开 API。

## 执行流程

典型多语句解析按以下顺序进行：

1. 调用者通过 `New` 获得默认 parser；可先调用 `SetSQLMode`、`SetMariaDB`、`SetStrictDoubleTypeCheck` 或 `SetParserConfig`。
2. `ParseSQL` 调用 `resetParams`，清空 `lastWarnings`，以 SQL 重置 Scanner，再按切片顺序执行每个 `ParseParam::ApplyOn`。任一参数失败会在语法分析前返回，并清除本轮警告。
3. 保存 `src`，清空旧 `result`，把规约计数归零并假设语义完整；随后 `check_expression_depth_before_parse` 用继承 Scanner 检查容易在 AST 形成前触发深递归的 token 链。
4. 为满足 Rust 可变借用规则，暂时用 `mem::take` 将 `lexer` 移出 `Parser`，调用 `yyParse(&mut lexer, self)`。运行时读取生成表并调用 `parser_actions/*`；语句列表动作会递增 `reducedStatementCount` 并更新 `allStatementsSemanticallyComplete`。
5. 从 lexer 提取 warnings 与 errors，将警告克隆到 `lastWarnings`，再把 lexer 放回 Parser。状态码非零或存在词法/语法错误时，只返回第一项解析错误；若状态码失败但没有具体错误，则退回 `ErrSyntax`。
6. 只有当所有规约语义完整且 `result.len() == reducedStatementCount` 时，逐语句执行 `check_ast_depth_limit` 和 `parser_ast::SetFlag`，然后用 `mem::take` 把结果所有权交给调用者。否则返回 `goyacc semantic reduction produced no statement`，防止“语法接受但没有真实 AST”的假成功。
7. `ParseOneStmt` 在上述流程之后检查数量；零条或多条都返回 `ErrSyntax`。`ParseSQLWithWarnings` 则把 `Result` 展开为 `(statements, warnings, Option<error>)`。

词法器识别数值 token 时会从 `pkg/parser/lexer.rs` 调用本文件的 `toInt`/`toFloat`/`toDecimal`/`toHex`/`toBit`，把类型化值放入 `yySymType.item`；规约动作再通过 `getUint64FromNUM` 等函数读取。query hint 动作位于 `pkg/parser/parser_actions/query.rs`，调用 `Parser::parseHint` 并合并 hint 诊断。

## 数据与状态

`Parser` 是有状态且可复用的。配置状态（SQL mode、窗口函数、MariaDB、严格 DOUBLE、位置记录、unsupported syntax）与单轮状态并不完全同寿命：`ParseSQL` 每轮重置 charset/collation、Scanner 输入、结果、警告和语义计数，但不会像公开 `Reset` 那样恢复全部默认配置。`Reset` 清空 `cache` 内对象并调用 `reset`；`reset` 恢复 explicit charset、strict DOUBLE、unsupported syntax、窗口函数和默认 SQL mode，但当前代码没有在其中重置 `enableMariaDB`，因此调用者不应假设所有字段都被恢复。

AST 结果以 `Vec<Box<dyn parser_ast::Node>>` 持有；成功时 `mem::take` 转移所有权，使后续 Parser 复用不会覆盖已返回语句。`yySymType.item` 使用 `Box<dyn Any>` 承载语法动作的动态类型；数值转换必须与动作侧预期类型严格匹配。`src` 和符号偏移采用字节位置，`endOffset` 刻意逐字节回退 ASCII 空白，遇到多字节空白可能停在字符中间，这是对 Go `src[offset-1]` 行为的兼容而不是 Unicode 字符索引。

错误静态量和正则使用线程安全的 `LazyLock`，首次访问时初始化；每个 Parser 的其余可变状态不共享。hint parser 在首次 `parseHint` 时创建，并在同一 Parser 后续解析中复用。`cache` 初始只预留容量，实际栈增长及 `yyVAL` 管理由 `yyParse` 负责。

## 依赖与调用关系

上游公开调用边包括：

- `pkg/session/runtime.rs` 用 `SetSQLMode` 后调用 `ParseSQL`，把解析器放在会话 SQL 主链中；`pkg/session/hint_runtime.rs` 用 `ParseOneStmt` 解析 binding SQL。
- `pkg/session/runtime/system_session.rs`、`pkg/session/runtime/mlog_purge.rs` 通过 `ParseOneStmt` 解析表达式或建表定义。
- `dumpling/export/schema_projection.rs` 用 `ParseOneStmt` 解析/投影建表语句；`pkg/domain/extract.rs`、`pkg/expression/simple_rewriter.rs` 也直接消费公开入口。
- RustCodeGraph 对 `ParseOneStmt` 的 Rust 调用边能定位到 planner 集成测试，并对 Go 同名入口显示大量生产调用者；原始 Rust 搜索补足了 session、dumpling、domain 与 expression 的实际调用位置。

关键下游边包括：

- `ParseSQL -> check_expression_depth_before_parse -> Scanner::InheritScanner/Lex`，随后 `ParseSQL -> yyParse`。
- `yyParse` 位于 `pkg/parser/parser_runtime.rs`，使用 `generated/main_tables.rs` 并分派到 `pkg/parser/parser_actions/*`；`parser_actions/misc.rs` 维护规约计数与语义完整标志。
- `ParseSQL -> Scanner::Errors` 收集诊断，成功后 `ParseSQL -> check_ast_depth_limit -> parser_ast::Walk`，再调用 `parser_ast::SetFlag`。
- `lexer.rs -> toInt/toDecimal/toFloat/toHex/toBit`；`parser_actions/admin.rs`、`ddl.rs`、`dml.rs`、`expression.rs`、`query.rs` 调用数值读取辅助；`parser_actions/query.rs -> parseHint`。
- `CharsetConnection`/`CharsetClient -> charset::encoding::FindEncoding`；错误构造依赖 `parser_terror` 与 `parser_mysql` 错误码；十进制、hex 和 bit 实例由 `parser_test_driver` 建立。

## 错误处理与边界

- 参数错误优先于语法分析；`ParseSQL` 不保留参数内部可能通过嵌套解析产生的警告，`yy_parser_test.rs::rejected_params_do_not_leak_nested_parse_warnings` 固化了该边界。
- 词法/语法错误由 Scanner 累积；主入口返回第一项 error，但警告仍可从 `ParseSQLWithWarnings` 或 `Warnings` 取得。没有具体 Scanner error 的非零 `yyParse` 状态回退为 `ErrSyntax`。
- `ParseOneStmt` 只接受恰好一条语句；空 SQL 和多语句输入都走 `ErrSyntax`。
- `ParseErrorWith` 先按 `mysql::ErrTextLength` 截断原始字节，再以有损 UTF-8 显示，因此多字节字符可能产生替换字符；这保留了 Go “先截字节”的限制。
- 深度限制既有 token 预检，也有 AST 后检。预检只识别若干高风险模式，不是完整语法判定；AST visitor 才是其他形状的权威检查。连续括号链采用迭代遍历以避免检查器自身先栈溢出。
- `toInt` 的非范围格式错误、`toFloat` 的格式错误、hex/bit 构造错误会向 lexer 追加错误并返回 `invalid`；无限/溢出的浮点数使用 `ErrIllegalValueForType`。`getUint64FromNUM` 对非 `i64/u64` 返回 0，`getInt64FromNUM` 对非 `i64` 返回 -1 和 Go 风格范围文本，调用动作必须检查相应语义。
- 当前 Rust `toDecimal` 在 parser driver 失败时追加 `decimal literal` 错误并放入默认 decimal；Go 版本则对 `ErrDataOutOfRange` 追加截断 warning 并用 `mysql.DefaultDecimal`。因此不能笼统宣称这一分支已完全等价，应以测试和后续迁移任务为准。
- `CharsetConnection("")` 将 parser 字符集恢复默认值，但仍把空字符串传给 `FindEncoding`，与 Go 当前实现一致；未知编码如何表示由 charset 子 crate 决定，本文件不主动生成 `ErrUnknownCharacterSet`。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部 I/O。`LazyLock` 只负责全局只读错误模板和正则的安全一次性初始化。

`Parser` 需要 `&mut self` 执行解析，表示单个实例面向串行复用；代码没有为同一 Parser 的并发调用提供同步。不同 Parser 实例可独立工作，但能否跨线程移动仍取决于其 AST、Scanner、动态 `Any` 值及错误类型的 trait 实现，本文件没有声明额外并发保证。

一次解析的临时资源生命周期由 Parser 持有并复用：Scanner 在调用 `yyParse` 时暂时移出，结束后无论成功与否都会在诊断处理阶段放回；符号缓存跨解析保留以减少分配；成功 AST 通过 `mem::take` 脱离 Parser；warnings 通过克隆同时保留在 Parser 与返回值中。hint parser 也跨轮次复用，并在 Parser 被丢弃时一并释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/yy_parser.go`。错误变量、三种特殊注释正则、`ParserConfig`、`Parser` 的主要状态、`New/Reset/reset`、传统解析入口、SQL mode/窗口函数设置、偏移辅助、hint parser、字面量转换、`ParseParam` 及三个参数类型均保持同名或一一对应。Rust 用 `Box<dyn Node>`/`Box<dyn Any>` 表达 Go 接口值，用 `Option<Box<_>>` 表达 nil，用 `Vec` 表达切片，用 `Result` 加 `ParseSQLWithWarnings` 表达 Go 可同时返回 warnings 与 error 的契约。

需要特别注意的当前差异：

- Rust `Parser` 增加 `lastWarnings`、`reducedStatementCount` 和 `allStatementsSemanticallyComplete`，并在返回前拒绝不完整或没有对应 AST 的规约；Go 对照文件没有这三个字段和门槛。
- Rust 增加解析前表达式深度扫描，并对括号链做迭代式 AST 检查；Go 文件只有解析后的 `checkASTDepth`，括号上限还由 `lexer.go` 维护。
- Rust `New` 显式调用 `parser_test_driver::init_test_driver`；Go `New` 检查 AST driver 函数指针是否已注册并在缺失时 panic。
- Rust 成功时转移 `result`，保证 Parser 复用不覆盖旧返回值；Go 返回其切片，并依赖现有切片/节点所有权行为。Rust 的 `ParseOneStmt` 不重复调用 `SetFlag`，因为 `ParseSQL` 已统一设置。
- Go 文件中的 `setNodeText`、`setLastSelectFieldText` 以及角色/权限转换辅助未出现在本 Rust 文件；对应语义由 Rust 的拆分动作与 AST 接口承担，不能仅凭文件名假设逐函数同位移植。
- Rust 额外公开 `ErrUnknownCollation`；十进制失败分支与 Go 的截断 warning 路径存在上述差异。

因此，该文件是按 Go API 与行为迁移的解析门面，但不是机械逐行翻译；评审兼容性时应同时查看 Scanner、parser runtime、parser actions 和独立测试。

## 扩展指南

- 新增公开解析配置时，在 `ParserConfig`、`Parser` 状态、`SetParserConfig` 与 `reset/Reset` 的生命周期语义间保持一致，并同步 Go 对照。若开关影响 token 化，应下沉到 `Scanner`；若影响规约，应接入相应 `parser_actions/*`，不要在公开入口伪造 AST。
- 修改主解析流程时，必须保持诊断顺序、lexer 归还、语义完整性校验、AST 深度检查和 `SetFlag`。尤其要覆盖“warnings 与 error 同时存在”“Parser 失败后复用”“返回 AST 不被下一轮覆盖”。
- 新增字面量类型时，应同时接入 `lexer.rs` token 分派、`yySymType.item` 的具体动态类型、消费它的语义动作和 parser driver；避免只令 token 可接受而规约无法正确 downcast。
- 修改深度限制时，同时评估 Scanner 预检、AST visitor、Go `maxParenthesesDepth/maxASTDepth` 及递归 visitor 的栈安全。预检不可替代后检，也不应扩大 SQL 接受范围。
- 字符集/排序规则改动应落在 `ParseParam` 和 charset encoding 层，保持空字符串默认值及“每轮 resetParams”语义；添加可能失败的参数实现时测试失败前后的警告和 Parser 状态。
- hint 行为改动应从 `parseHint`、`hintparser.rs`、`hintparserimpl.rs` 和 `parser_actions/query.rs` 联合验证，确保使用实时 SQL mode 与词法位置。
- 测试继续放在独立文件。优先扩展 `pkg/parser/yy_parser_test.rs`（警告、AST flag、错误上下文与复用）、`pkg/parser/yy_parser_4_aster_unit_test.rs`（辅助函数、参数、特殊注释、Go 边界）、深度相关的 `pkg/parser/go_merge_35_test.rs`，以及需要完整文法覆盖时的 `pkg/parser/parser_3_aster_unit_test.rs`；不要把 `#[cfg(test)]` 测试加入本生产文件。

## 验证依据

- 生产源码：`pkg/parser/yy_parser.rs`（公开入口、状态、辅助函数、错误与深度保护），`pkg/parser/lib.rs`（include 顺序、模块可见性与 re-export），`pkg/parser/parser_runtime.rs`（`yyParse`），`pkg/parser/lexer.rs`（字面量辅助调用），`pkg/parser/parser_actions/misc.rs` 与 `query.rs`（规约完整性计数、hint 调用）。
- crate 配置：`pkg/parser/Cargo.toml`（库入口、工作区子 crate、`regex` 与 parser driver 依赖、Go package 移植元数据）。`pkg/parser` 下未发现 `doc.go`，所以包契约取自 `lib.rs` 和上述实现。
- Go 对照：`pkg/parser/yy_parser.go`；并以 `pkg/parser/lexer.go` 核对 Go 括号深度上限。本文明确记录了状态字段、driver 初始化、深度保护、结果所有权和 decimal 失败路径的差异。
- 独立测试：`pkg/parser/yy_parser_test.rs` 覆盖失败警告、实时 hint mode/位置、AST flag、浮点溢出、UTF-8 字节截断、位置项与参数失败隔离；`pkg/parser/yy_parser_4_aster_unit_test.rs` 覆盖特殊注释、数字辅助、ParseParam、SQL mode、字节偏移、TiDB 文法和复用；`pkg/parser/go_merge_35_test.rs` 覆盖 AST 深度限制。
- RustCodeGraph：`status` 显示索引包含 Rust/Go；`node check_ast_depth_limit` 给出其调用者 `ParseSQL` 和下游 `ast_depth_error`；`node CharsetConnection` 给出 Rust `Parse`/`ParseOneStmt` 的实例化边；`node ParseOneStmt` 给出 Rust `ParseSQL`、错误和参数构造边；`node yyParse` 确认生成状态机入口。`files --filter pkg/parser/yy_parser` 未返回文件，且部分重名方法查询优先命中 Go，因此文件级和缺失调用边按技能约定用精确源码/`rg` 补充。
- 结构验证使用任务指定命令，要求目标文件存在且恰好包含本文 11 个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
