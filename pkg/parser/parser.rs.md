# `pkg/parser/parser.rs`

## 文件定位

`pkg/parser/parser.rs` 是 `astersql-parser` crate 内 SQL 主语法分析器的组装边界，而不是公开入口本身。`pkg/parser/lib.rs` 的 `parser_impl` 模块先引入词法器、AST、错误、MySQL mode 等上下文，再以 `include!("parser.rs")` 把本文件展开到同一模块；公开的 `Parser`、`New`、`ParseSQL` 等随后由 `pub use parser_impl::*` 导出。crate 边界由 `pkg/parser/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认。

文件保留 goyacc 生成文件的版权与生成来源说明，但当前主体已被拆成四个稳定组成件：`parser_value.rs`、`parser_semantic_support.rs`、`parser_actions` 和 `parser_runtime.rs`。因此它“为何存在”的核心答案是：在不改变 `parser_impl` 词法/AST 上下文的前提下，固定纯 Rust LALR(1) 解析器各部分的展开顺序与可见域。

## 核心职责

1. 通过 `include!("parser_value.rs")` 定义规约栈携带的 `yySymType` 和一基索引的 `Rhs` 访问器。
2. 通过 `include!("parser_semantic_support.rs")` 提供跨语法类别共享的中间语义结构及校验/转换辅助，例如生成列、分区定义和 Go duration 校验。
3. 通过 `mod parser_actions` 建立按稳定 `RuleId` 分派的语义动作层；动作被拆到 `admin`、`ddl`、`dml`、`expression`、`misc`、`mview`、`query`、`security` 八个子模块。
4. 通过 `include!("parser_runtime.rs")` 提供 token 到符号的映射、稀疏动作表查询、词法接口，以及执行移进、规约、接受和错误恢复的 `yyParse`。

本文件不直接定义 token/状态表，也不直接暴露业务 API。表数据来自 `pkg/parser/generated/main_tables.rs`（在 `lib.rs` 中先于本文件展开），公开解析会话和入口来自 `pkg/parser/yy_parser.rs`。

## 主要符号

- `yySymType`（`parser_value.rs`）：goyacc 语义值，保存状态 `yys`、源码偏移、动态 `item`、标识符、表达式、语句及 `semantic_complete` 标记。动态字段不要求 `Clone`，规约时以移动语义取值。
- `Rhs<'a>`（`parser_value.rs`）：对一次规约右部语义值的可变切片封装；`borrow`、`borrow_mut`、`take` 使用与 yacc `$1..$n` 一致的一基位置。
- 共享语义结构和辅助函数（`parser_semantic_support.rs`）：如 `CreateTableSemantic`、`SetOprSemantic`、`validate_column_def`、`validate_partition_clause`、`validate_partition_options`。它们承接多个动作模块共同需要的中间态和 MySQL 兼容校验。
- `parser_actions::Context`：把规约输出、当前 `Parser` 和 `yyLexer` 聚合后交给语义动作。
- `parser_actions::apply` / `has_semantic_action`：按 `RuleId` 路由语义动作，并区分“该规则没有动作”与“应有动作但没有产出”。
- `yyLexer` / `yyLexerEx`（`parser_runtime.rs`）：runtime 所需的词法、错误/警告和可选规约回调契约。
- `yylex1`：调用 `Lex`，将非正 token 统一折叠为 `yyEOFCode`，并保留调试输出。
- `yyParse(yylex: &mut dyn yyLexer, parser_state: &mut Parser) -> isize`：LALR 状态机入口；`0` 表示接受，`1` 表示解析失败，扩展规约回调要求提前停止时可返回 `-1`。
- `Parser::ParseSQL`（`yy_parser.rs`）：真正的公开主入口；负责重置参数/词法器、调用 `yyParse`、提取警告和错误、检查语义完整性与 AST 深度并设置 AST flag。

`pkg/parser/parser.rs` 自身没有常量、类型、trait、函数、`impl` 或条件编译项；这些符号通过 `include!` 在编译后属于相同模块作用域。唯一普通子模块声明是 `mod parser_actions`。

## 执行流程

1. 调用方构造 `Parser`（`New` 或 `Default`），设置 SQL mode、MariaDB、窗口函数等配置，再调用 `ParseSQL`、`Parse` 或 `ParseOneStmt`。
2. `Parser::ParseSQL` 恢复连接字符集/排序规则默认值，清空上次警告与结果，重置 `Scanner`，按顺序应用 `ParseParam`，并在进入状态机前防御可导致深递归的表达式链。
3. `ParseSQL` 暂时取出 `self.lexer`，调用 `yyParse(&mut lexer, self)`；这样 runtime 同时获得词法器和解析会话的独占可变引用。
4. `yyParse` 从 `Parser.cache` 取出并清空语义值栈，压入基状态。每轮用 `yylex1` 获取 lookahead，经 `GENERATED_MAIN_XLAT` 和 `GENERATED_MAIN_PARSE_TABLE` 决定动作。
5. 正动作执行移进：把 `parser_state.yylval` 移入栈并前进状态；负动作执行规约：查 `GENERATED_MAIN_REDUCTIONS`、旧规则映射和 `RULE_IDS_BY_REDUCTION`，切出右部语义值。
6. 规约调用 `parser_actions::apply(rule_id, Rhs, Context)`。已覆盖规则由对应动作模块构建 AST 或更新 `Parser`；无显式动作的规则沿用 yacc 的 `$$ = $1`，通过 `Rhs::take_default` 移动值，避免克隆动态 AST。
7. runtime 计算 goto 状态，按配置记录表达式偏移，并可调用 `yyLexerEx::Reduced`。遇到无动作状态时采用 goyacc 三阶段恢复：首次记录 scanner 格式化的错误、回退栈寻找可移入 `error` 的状态、恢复期间丢弃 lookahead；EOF 或找不到恢复状态时失败。
8. 无论成功失败，runtime 清空栈并归还 `Parser.cache`。`ParseSQL` 随后保存警告；失败时返回首个错误，成功时还要求已规约语句数、结果数及 `semantic_complete` 一致，再检查 AST 深度、设置节点 flag 并转移结果。
9. `ParseOneStmt` 在上述流程之外再要求结果恰好一条，否则返回 `ErrSyntax`。

## 数据与状态

解析持久状态位于 `Parser`（`yy_parser.rs`）：字符集/排序规则、源 SQL、`Scanner`、可复用 hint parser、最近警告、语法开关、AST 结果、规约计数，以及 `cache`、`yylval`、`yyVAL` 等 yacc 状态。每次 `ParseSQL` 会清理本次结果和计数，但 SQL mode 等显式配置由调用方管理；`Reset` 才恢复默认解析开关。

runtime 栈是 `Vec<yySymType>`。`New` 预分配 200 个元素的容量，`yyParse` 用 `std::mem::take` 临时取得它，结束时清空并放回；`pkg/parser/parser_3_aster_unit_test.rs` 的缓存复用用例验证长 INSERT 第二次解析保持相同指针和容量。

`semantic_complete` 是 Rust 拆分语义动作后的完整性护栏：规约结果继承右部完整性；若某个已登记语义动作未实际处理，标记会变为不完整。顶层 `ParseSQL` 只有在全部语句完整且结果数等于规约语句数时才返回 AST，否则返回 `goyacc semantic reduction produced no statement`。

生成表和错误模板是只读静态数据；语义辅助里的 MySQL 错误对象使用 `LazyLock` 延迟初始化。AST 节点通过 `Box<dyn Node>` / `ExprNode` 传递，动态中间值通过 `Box<dyn Any>` 传递。

## 依赖与调用关系

上游主链为 `Parser::ParseSQL`（`pkg/parser/yy_parser.rs`）→ `yyParse`（`pkg/parser/parser_runtime.rs`）→ `parser_actions::apply`（`pkg/parser/parser_actions/mod.rs`）。RustCodeGraph 对 `ParseSQL` 的 callee 查询确认其调用 `yyParse`、`Scanner::Errors`、深度检查、`ParseParam::ApplyOn` 和 `parser_ast::SetFlag`；对 `yyParse` 的 callee 查询确认其依赖 `yylex1`、`yyLexer::{Errorf, AppendError}`、`yyLexerEx::Reduced` 等。

应用侧直接调用示例包括 `pkg/session/hint_runtime.rs` 的绑定 SQL 解析、`pkg/session/ddl_tables.rs` 的建表 SQL 解析、`pkg/session/runtime/mlog_purge.rs` 的表达式解析。RustCodeGraph 的通用 callers 查询因 `ParseSQL`/`yyParse` 同时存在 Go 与 Rust 同名定义而未给出精确 caller 列表，以上调用点由针对 Rust crate 名和方法名的仓库搜索补证。

下游依赖包括：

- `generated/main_tables.rs` 提供符号名、token 翻译、动作/规约表、稳定规则标识映射；
- `lexer.rs` 的 `Scanner` 实现 `yyLexer`，负责 token、SQL mode、位置以及错误/警告；
- `parser_actions/*` 构造 `parser_ast` 节点并调用共享校验；
- `parser_ast`、`parser_mysql`、`parser_terror`、`parser_charset`、`parser_types` 与 `parser_test_driver` 等本地 crate 由 `pkg/parser/Cargo.toml` 声明；外部直接依赖仅见 `regex`、`sha2`、`hex`。

## 错误处理与边界

- 表索引或规则映射缺失、规约弹栈越界、goto 非法等内部一致性问题不会 panic，而是经 `yyLexer::AppendError` 记录明确错误并返回失败状态。
- 语法错误由 scanner 根据当前行、列和 near 文本格式化；runtime 故意忽略 goyacc 自动生成的通用消息，以保持 TiDB/MySQL 兼容诊断。`ParseSQL` 返回首个解析错误，同时 `Warnings()` 仍可取得失败解析产生的警告。
- `ParseParam::ApplyOn` 在状态机启动前失败会立即返回；`ParseOneStmt` 拒绝零条或多条语句。
- 深度边界同时在解析前检测高风险 `CASE`、一元和二元链，并在 AST 形成后以 10,064 的上限遍历复核；括号链采用迭代跳过以避免 Rust 调用栈先溢出。
- 未显式动作的规则只有在 `has_semantic_action` 为假时才能安全采用默认规约值；这防止尚未完成的动作被伪装成成功。
- `Rhs` 的一基索引若越界返回 `None`，但动作模块的 `Index`/`IndexMut` 实现会以 `expect("semantic RHS position")` 抛出内部错误；规则与动作必须同步生成和维护。
- 本文件顶部仍标注 goyacc 生成来源，但当前 runtime 和动作分派是 Rust 拆分实现；不能仅重新生成 Go `parser.go` 后覆盖这些 Rust 文件。

## 并发与资源生命周期

解析 API 需要 `&mut Parser`，单个实例的一次调用对内部 scanner、结果和缓存拥有独占访问；代码没有为同一实例的并发解析提供锁或共享接口。需要并行解析时应由调用方为每个任务持有独立 `Parser`，不要在外层用不安全共享绕过该约束。

一次调用中，lexer 被暂时移出 `Parser`，runtime 栈缓存也被暂时移出并在返回前归还；AST 结果成功时用 `mem::take` 交给调用方。该生命周期避免重复分配，也保证失败后不会把不完整 AST 当作成功结果。动态语义值在规约时移动或截断释放；hint parser 在 `Parser` 内延迟创建并跨解析复用。

全局只读解析表天然可共享；错误模板的 `LazyLock` 初始化是线程安全的。`yyDebug` 是 `static mut`，只被 runtime 读取且没有公开安全写入口；若未来增加运行时调试开关，必须先解决并发数据竞争风险。

## 与 Go 版本的对应关系

Go 对照主文件是 `pkg/parser/parser.go`：`yySymType`、`yyLexer`、`yyLexerEx`、`yylex1` 和 `yyParse` 的总体协议一致，状态机都执行移进/规约/接受及三阶段错误恢复。Rust 将 Go 单个巨大生成文件拆为值类型、共享语义辅助、分类动作和 runtime，并使用 `generated/main_tables.rs` 的稀疏表及稳定 `RuleId` 路由。

公开生命周期对照位于 `pkg/parser/yy_parser.go` 与 `pkg/parser/yy_parser.rs`：两侧的 `New`、`Reset`、`ParseSQL`、`Parse`、`ParseOneStmt`、`SetSQLMode`、`EnableWindowFunc` 保持同一职责。Rust 以 `Result<(statements, warnings), error>` 表示 Go 的三返回值，并额外提供 `ParseSQLWithWarnings` 保留失败时的 warnings 合约。

重要实现差异是 Rust 不能克隆 `Box<dyn Node>`，所以默认规约通过 `Rhs::take_default` 移动 `$1`；同时用 `semantic_complete` 和规约计数显式拒绝缺失语义动作。Go 栈以固定长度切片起步并按需扩容，Rust 以容量 200 的 `Vec` 起步并跨调用复用。Rust 还在 AST 生成前增加深表达式链防护，以避免在达到 Go 语义深度限制前发生宿主栈溢出，但最终 AST 深度检查仍是权威边界。

`pkg/parser/parser_test.rs` 是独立 Rust 主测试集，对应 Go parser 测试意图而非内嵌在生产文件；它覆盖 SELECT/DDL/DML、SQL mode、特殊注释、hint warning、错误文本、字符集、MariaDB、窗口函数、多语句、内存分配等。更贴近拆分 runtime 的用例在 `parser_3_aster_unit_test.rs`、`parser_runtime_aster_unit_test.rs` 和 `parser_semantic_support_test.rs`。

## 扩展指南

- 新增或修改 SQL 语法时，应先改语法源/生成链并同步 `generated/main_tables.rs` 的规则映射，再在对应的 `parser_actions/{ddl,dml,expression,query,...}.rs` 添加或调整动作；不要在 `parser.rs` 中插入孤立特判。
- 新的跨类别中间语义或校验应放在 `parser_semantic_support.rs`；仅单类动作使用的逻辑应留在对应动作模块，避免扩大共享状态。
- 修改 runtime 时必须保持 `yyLexer` 合约、错误恢复阶段、`Reduced` 回调、偏移记录和 cache 归还路径；任何提前返回都应保证缓存最终回到 `Parser`。
- 新的公开配置或入口应接入 `yy_parser.rs` 的 `Parser`/`ParserConfig`/`ParseParam`，同时确认每次解析的重置语义以及 Go 同名 API。
- 测试必须继续放在独立文件：通用解析行为扩展 `pkg/parser/parser_test.rs`；状态机/缓存扩展 `parser_3_aster_unit_test.rs` 或 `parser_runtime_aster_unit_test.rs`；共享校验扩展 `parser_semantic_support_test.rs`；具体动作模块使用其同目录独立测试文件。
- 兼容风险主要是 token/规则编号漂移、MySQL 错误码和文本变化、SQL mode 分支变化以及 Go/Rust AST 形状不一致；性能风险主要是语义值克隆、缓存失效、深链递归和动作分派热路径。变更后应同时核对 Go 对照和 Rust 合约测试，不得用“能解析”代替 AST/错误/警告一致性。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标 `pkg/parser/parser.rs` 已索引，但因其只有 `include!`/模块装配，图中仅识别 1 个文件级符号。
- RustCodeGraph 文件/源码查询：`pkg/parser/parser.rs`（41 行）、`parser_value.rs`（11 个符号）、`parser_semantic_support.rs`（34 个符号）、`parser_runtime.rs`（43 个符号）、`parser_actions`（15 个文件）和 `yy_parser.rs`（70 个符号）。
- RustCodeGraph 调用查询：`callees yyParse` 确认 Rust runtime 到 `yylex1`、错误接口和规约回调的边；`callees ParseSQL` 确认 Rust 入口到 `yyParse`、深度检查、参数应用和 AST flag 的边。同名 Go/Rust 定义使无限定 callers 结果为空，未据此推断上游。
- 直接读取并核对：`pkg/parser/Cargo.toml`、`pkg/parser/lib.rs`、`pkg/parser/parser.go`、`pkg/parser/yy_parser.go`、`pkg/parser/parser_test.rs`；并以 `rg` 核对 Rust 应用侧调用点和独立测试覆盖面。
- 相关直接实现证据：`pkg/parser/parser_value.rs`、`pkg/parser/parser_semantic_support.rs`、`pkg/parser/parser_actions/mod.rs`、`pkg/parser/parser_runtime.rs`、`pkg/parser/yy_parser.rs`、`pkg/parser/generated/main_tables.rs`。
- 相关独立测试证据：`pkg/parser/parser_test.rs`、`pkg/parser/parser_3_aster_unit_test.rs`、`pkg/parser/parser_runtime_aster_unit_test.rs`、`pkg/parser/parser_semantic_support_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；验收以 11 个固定章节的结构检查、链接/路径检查和上述源码事实复核为准。
