# `pkg/parser/parser_actions/misc.rs` 逻辑说明

## 文件定位

`pkg/parser/parser_actions/misc.rs` 是 `astersql-parser` crate 内部的语义动作分片之一，不是独立解析器入口。crate 边界由 `pkg/parser/Cargo.toml` 定义，库入口为 `pkg/parser/lib.rs`；`pkg/parser/parser.rs` 私有引入 `parser_actions`，`pkg/parser/parser_runtime.rs` 在 LR 归约时把稳定的 `RuleId`、右侧语义值 `Rhs` 和可变 `Context` 交给 `parser_actions::apply`，再由 `pkg/parser/parser_actions/mod.rs` 按所有权分派到本文件。

本文件负责 153 个“杂项”文法归约动作，数量由 `pkg/parser/parser_actions/remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 固定验证。范围包括事务控制、`EXPLAIN`/`TRACE`、预处理语句、语句列表、`USE`、过程调用和存储过程控制结构；词法分析、LR 状态机、其他 DDL/DML/表达式/查询动作分别位于相邻模块，不属于这里。

## 核心职责

1. `identify` 将生成器提供的稳定字符串 `RuleId` 映射为私有枚举 `MiscRule`，避免依赖易漂移的 yacc 数字规则号。
2. `owns` 向总分派器声明某条规则是否归本模块所有；`apply` 只对已识别规则调用 `apply_rule`。
3. `apply_rule` 从 `Rhs` 的 `yySymType` 槽位读取 `ident`、`expr`、`statement` 或动态类型 `item`，构造 `parser_ast` 节点或中间向量并写入归约输出。
4. 对顶层 `StatementList`，除构造 AST 外还把 lexer 保存的原始语句文本和 SQL mode 写入节点，并更新 `Parser::result`、`reducedStatementCount`、`allStatementsSemanticallyComplete`。

这些职责对应 Go 文法动作 `pkg/parser/parser.y` 中同名产生式；本文件不是执行 SQL 的地方，只在解析阶段组织 AST 和解析器状态。

## 主要符号

- `MiscRule`（私有枚举，`misc.rs:21`）：用一项表示一个具体文法备选分支，例如 `BeginTransactionStmtAlt09`、`ExplainStmtAlt17`、`ProcedureDeclAlt03`。它只在本文件内使用。
- `identify(RuleId) -> Option<MiscRule>`（`misc.rs:177`）：匹配稳定规则字符串；未知 ID 返回 `None`。规则清单是本模块所有权的单一来源。
- `owns(RuleId) -> bool`（`misc.rs:494`，`pub(super)`）：供 `parser_actions/mod.rs` 的分派和所有权测试调用。
- `apply(RuleId, Rhs, Context) -> Option<Result<bool, isize>>`（`misc.rs:498`，`pub(super)`）：未知规则返回 `None`；已知规则返回动作结果。
- `apply_rule(MiscRule, Rhs, Context) -> Result<bool, isize>`（`misc.rs:506`）：主体分派。成功完成归约返回 `Ok(true)`；必需子节点或动态类型不匹配时若干分支返回 `Ok(false)`。当前文件没有产生 `Err(isize)` 的分支，但签名与总动作接口一致。
- `Context`、`Rhs`（定义于 `pkg/parser/parser_actions/mod.rs`）：前者借用归约输出、`Parser` 与 `yyLexer`，后者包装当前产生式右侧语义槽；索引实现会以 `expect("semantic RHS position")` 保护位置契约。

## 执行流程

主流程如下：

1. `pkg/parser/parser_runtime.rs` 根据 LR 表完成一次归约，取得 `RULE_IDS_BY_REDUCTION` 中的稳定 `RuleId`，创建默认 `yySymType` 输出并计算右侧语义完整性。
2. `parser_actions::apply` 按 `mview → ddl → dml → expression → query → security → admin → misc` 顺序询问 `owns`；命中本模块后调用 `misc::apply`。
3. `identify` 把字符串 ID 转为 `MiscRule`，`apply_rule` 再按规则族处理：
   - 事务族构造 `BeginStmt`、`CommitStmt`、`RollbackStmt`、保存点节点和 `CompletionType`，并把隔离级别/只读属性转换为系统变量赋值；`READ ONLY AS OF` 转为 `tx_read_ts`。
   - `EXPLAIN`/`TRACE` 族包装已有 statement，区分 `Analyze`、`Explore`、格式、SQL/plan digest、replayer 文件、连接 ID与 trace-plan 目标；`EXPLAIN table [column]` 被改写为 `ExplainStmt(ShowStmt::Columns)`。
   - prepare/execute/deallocate、`DO`、`HELP`、`USE`、`CALL`、values-list 等分支构造对应 AST 或累积表达式/行向量。
   - 存储过程族逐层累积参数、声明、handler 条件、游标操作、语句列表、IF/ELSEIF/ELSE、两类 CASE、WHILE/REPEAT、标签块和 `ITERATE`/`LEAVE`，最终形成 `ProcedureInfo` 或 `DropProcedureStmt`。
   - 子查询可作为普通/traceable/explainable/bindable/过程语句时，经 `take_subquery_statement` 取出 `SelectStmt` 或 `SetOprStmt` 并设置 `IsInBraces = true`。
4. 顶层 `StatementList` 将非空语句附加到 `Parser::result`；空语句不产生节点。动作未处理时，runtime 才尝试 goyacc 的 `$$ = $1` 默认语义，并结合 `has_semantic_action` 标记语义完整性。

## 数据与状态

本文件没有全局可变状态。每次归约的状态都通过借用传入：`out: &mut yySymType` 是唯一输出槽，`parser_state: &mut Parser` 仅在语句列表归约时累计最终结果，`yylex: &mut dyn yyLexer` 提供语句文本和 SQL mode。

`yySymType` 承载多种语义通道：标识符在 `ident`，表达式在 `expr`，AST 语句在 `statement`，异构中间值在 `item: Box<dyn Any>`。`apply_rule` 对 `item` 使用 `downcast_ref`/`downcast`，并在组装向量时多用 `take()` 转移所有权，避免复制 trait object。存储过程名称、游标名和局部变量名按 Go 行为在相应分支调用 `to_ascii_lowercase`；标签则保留解析出的文本用于首尾比较。

关键不变量是 `RuleId`、产生式右侧位置和动态类型三者必须同步。`remaining_aster_unit_test.rs` 还验证每个生成动作规则恰有一个 owner，且 misc 恰好拥有 153 条规则，并禁止回退到 `legacy_rule_number`、`apply_numeric` 或 `rhs_index`。

## 依赖与调用关系

上游调用边为 `pkg/parser/parser_runtime.rs` → `pkg/parser/parser_actions/mod.rs::apply` → `misc::owns`/`misc::apply` → `apply_rule`。`pkg/parser/parser_actions/mod.rs::has_semantic_action` 也调用 `misc::owns`，用于判断默认归约是否语义完整。RustCodeGraph 的文件视图确认本文件由 `parser_actions/mod.rs` 及所有权测试模块使用；独立行为测试通过 `pkg/parser/lib.rs` 的 `#[path = "parser_actions/misc_test.rs"]` 注册，从公共 `Parser` 路径间接覆盖动作。

下游主要依赖来自 `super::super::*`：`RuleId`、`yySymType`、`Parser`、`yyLexer`、`semantic_numeric_isize`、`take_subquery_statement`、`mysql`、`parser_ast` 和 `parser_types`；直接 crate 依赖由 `pkg/parser/Cargo.toml` 声明，其中 AST 和类型分别对应本地包 `astersql-parser-ast`、`astersql-parser-types`，MySQL 语义来自 `astersql-parser-mysql`。本文件没有 feature 或条件编译项。

## 错误处理与边界

未知 `RuleId` 在 `identify`/`apply` 层返回 `None`，让总分派器继续或最终报告未处理。对于已拥有规则，缺失必需 statement、错误的 `Any` 动态类型或无法把子查询降型为 `SelectStmt`/`SetOprStmt` 时，相关分支返回 `Ok(false)`；可选字段和列表则常用 `unwrap_or_default` 形成 Go 零值语义。数值通过 `semantic_numeric_isize` 读取，连接 ID 和过程错误号在转为无符号数前以 `max(0)` 截断负值。

右侧位置由 `Rhs` 的索引实现保护，越界会 panic，因此修改产生式后必须同步这里的反向偏移。动态类型错误有些会降级为空列表/默认值而非显式 parser error，这使规则/类型同步测试尤其重要。标签首尾不一致不在此处报 parser error，而是写入 `LabelError` 与 `LabelEnd` 供后续检查；首尾一致或无结束标签时 `LabelEnd` 保持空串。当前实现没有直接调用 `AppendError`，也没有返回 `Err(isize)`。

## 并发与资源生命周期

解析动作是单次解析调用内的同步、串行状态变换；本文件不创建线程、异步任务、锁、通道、文件或网络资源。`Context<'_>` 和 `Rhs<'_>` 的借用把可变访问限制在一次归约期间，避免同一 `Parser`、lexer 或输出槽的并发写入。

AST 子节点和动态中间值由 `Box`/`Vec` 拥有。代码使用 `take()` 把 statement/item 从 RHS 移入父节点或结果向量，归约结束后 runtime 截断已消费的 value stack；只读字段才通过 `clone()` 复制。因而扩展时应优先延续移动所有权的模式，避免克隆大型 AST，也不能在移动后再次依赖相同 RHS 槽。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/parser.y`，生成的 Go 结果可在 `pkg/parser/parser.go` 中看到。主要语义保持一致：事务模式与 completion type、`EXPLAIN` 的格式/analyze/explore/digest 分支、statement-list 的非空过滤、子查询括号标记、过程声明/handler/游标/控制流、参数 IN/OUT/INOUT 以及标识符小写化都复刻了同名 Go 动作。

可见差异主要来自运行时表示：Go 用接口类型断言和切片，Rust 用 `yySymType` 的专用字段、`Box<dyn Any>` 降型和 `Box<dyn parser_ast::Node>`；Go 动作中的 `return 1` 错误协议统一保留在 Rust 动作接口的 `Result<bool, isize>`，但本文件当前只返回 `Ok(true/false)`。Rust 的 `StatementList` 还显式维护 `reducedStatementCount` 和 `allStatementsSemanticallyComplete`，并从 lexer 同时设置 `NoBackslashEscapes`；这是 Rust runtime 为跟踪移植完整性和节点文本所需的局部接线。

一个已知的移植边界是 `CreateProcedureStmtAlt01`：Go 动作还从原始源码切片设置过程体文本和 `ProcedureParamStr`，本文件只组装 `ProcedureInfo` 的存在标志、名称、参数和 body；不能在没有进一步证据时声称 Rust 此处已覆盖那两项文本提取。相关 Go 恢复用例集中在 `pkg/parser/ast/procedure_test.go`。

## 扩展指南

新增或修改本组文法时，应同时处理以下位置：

1. 先在语法/生成链确定稳定 `RuleId`，在 `MiscRule` 和 `identify` 中一一登记，再在 `apply_rule` 增加与 `pkg/parser/parser.y` 对应的分支；不要引入数字规则号回退。
2. 逐项核对 RHS 反向位置及其语义通道/动态类型。必需值应显式拒绝缺失，真正可选的字段才使用默认值；集合优先 `take()` 后追加。
3. 若新增规则属于其他动作域，应放入对应 `ddl`、`dml`、`expression`、`query`、`security`、`admin` 或 `mview` 模块，避免 owner 重叠。
4. 同步 `pkg/parser/parser_actions/remaining_aster_unit_test.rs` 的 owner 数量，并在独立测试文件增加行为回归。misc 行为首选 `pkg/parser/parser_actions/misc_test.rs`；广泛 SQL/恢复兼容性可对照 `pkg/parser/parser_test.rs`、Go 的 `pkg/parser/parser_test.go` 与过程测试 `pkg/parser/ast/procedure_test.go`。
5. 风险重点是：规则字符串或 RHS 偏移漂移导致动作静默落空，错误降型产生默认 AST，statement-list 文本/SQL mode 丢失，以及大型 AST 不必要克隆带来的性能损失。涉及过程语法时还应检查标签错误、handler 条件节点可遍历性、名称大小写和 Go 的源码文本字段。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/parser/parser_actions` 定位 15 个动作/测试文件；`node --file pkg/parser/parser_actions/misc.rs` 读取 1597 行源码并报告模块使用关系；`query MiscRule/identify/apply_rule/owns` 确认主要符号。按文件消歧的 `callers/callees` 未返回边，故调用关系由下列源码直接补证。
- Rust 源与装配：`pkg/parser/parser_actions/misc.rs`、`pkg/parser/parser_actions/mod.rs`、`pkg/parser/parser_runtime.rs`、`pkg/parser/parser.rs`、`pkg/parser/lib.rs`。
- crate 边界：`pkg/parser/Cargo.toml`，确认库入口、本地 AST/types/mysql 依赖和无本文件专属 feature。
- Go 对照：`pkg/parser/parser.y` 的 `BeginTransactionStmt`、`ExplainStmt`、`RollbackStmt`、`CompletionTypeWithinTransaction`、`StatementList` 和存储过程产生式；生成文件 `pkg/parser/parser.go` 仅用于确认同名动作存在。
- 测试证据：`pkg/parser/parser_actions/misc_test.rs` 覆盖匹配标签的空 `LabelEnd`、过程声明表达式遍历、handler 条件保留；`pkg/parser/parser_actions/remaining_aster_unit_test.rs` 覆盖唯一 owner、153 条规则和无数字回退；Go 的 `pkg/parser/parser_test.go` 与 `pkg/parser/ast/procedure_test.go` 提供事务、EXPLAIN、保存点和存储过程兼容样例。
- 本任务只新增说明文档，按任务要求不运行 Cargo；最终以固定 11 章节的结构命令验证，并人工复核上述定位、流程、边界与扩展入口均能回指真实符号或路径。
