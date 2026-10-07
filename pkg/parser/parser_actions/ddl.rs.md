# `pkg/parser/parser_actions/ddl.rs` 逻辑说明

## 文件定位

`ddl.rs` 是 `astersql-parser` crate 内部的 DDL 语义动作分片，由 [`parser_actions/mod.rs`](./mod.rs) 私有声明为 `mod ddl`。它不负责词法分析或 LR 状态转移，而是在 [`parser_runtime.rs`](../parser_runtime.rs) 完成一次规约时，把已识别的稳定 `RuleId` 和右部语义值转换成 `parser_ast` 节点、选项值或列表。

该文件直接对应 Rust 主语法 [`grammar/main.astergram`](../grammar/main.astergram) 中标有 `@action` 的 DDL 产生式，并以 Go 版 [`parser.y`](../parser.y) 的产生式动作为移植语义基线。[`Cargo.toml`](../Cargo.toml) 将它归入 `astersql-parser` 库，AST 主要来自本地路径依赖 `astersql-parser-ast`，错误、MySQL 常量、字符集和类型能力通过 crate 根模块及其他本地 parser crate 间接引入。

## 核心职责

- 为 596 个需要显式语义动作的 DDL 产生式建立稳定 `RuleId -> DdlRule` 映射，避免依赖会随语法重排而变化的数字 rule number。
- 把规约右部 `Rhs` 中的 `ident` / `expr` / `item` / `statement` 动态语义值组装到 `Context.output`，覆盖 ALTER/CREATE/DROP/RENAME/TRUNCATE/RECOVER/FLASHBACK/SPLIT/DISTRIBUTE，以及列、索引、外键、表选项、分区、视图和 placement policy 等子结构。
- 在构造 AST 前执行 Go 规则中的局部语义校验，包括数值/长度、重复选项、分区定义、MariaDB/MySQL 兼容开关和严格 DOUBLE 类型等边界；错误通过 `yyLexer` 记录，必要时用 `Err(1)` 终止解析。
- 保持语法所有权与执行一致：`owns` 和 `apply` 共用 `identify`，因此被声称所有的 rule 一定有对应 action。

## 主要符号

- `DdlRule`（文件第 21 行起）：私有、可比较的枚举。每个变体名由非终结符和候选序号组成，例如 `AlterTableSpecAlt01` 或 `CreateTableStmtAlt02`。它是文件内部的类型化分派键，不是对外 AST API。
- `identify(rule_id: RuleId) -> Option<DdlRule>`（第 620 行起）：将 `RuleId::as_str()` 的“规则签名 + 哈希”匹配到 `DdlRule`；未归属本分片时返回 `None`。
- `owns(rule_id: RuleId) -> bool`（第 1914 行）：只检查 `identify` 是否成功，供总调度器和所有权测试使用。
- `apply(rule_id, rhs, context) -> Option<Result<bool, isize>>`（第 1918 行）：模块边界入口。`None` 表示非 DDL rule；`Some(Ok(true))` 表示动作已完成；`Some(Ok(false))` 表示 rule 已识别但必要的动态语义值缺失/类型不符；`Some(Err(1))` 表示已记录解析错误。
- `apply_rule(rule, rhs, context) -> Result<bool, isize>`（第 1927 行）：主执行器。它拆出 `Context { output, parser_state, lexer }`，在单个大 `match` 中实现全部 DDL 产生式动作，默认尾部返回 `Ok(true)`。

文件没有公开 `pub` API、trait、struct、模块常量、static 或条件编译项；对父模块可见的只有 `pub(super)` 的 `owns` 和 `apply`。

## 执行流程

1. [`parser_runtime.rs`](../parser_runtime.rs) 在 LR 规约时从 `RULE_IDS_BY_REDUCTION` 取得稳定 `RuleId`，计算 RHS 切片，创建默认 `yySymType` 和 `Context`，调用 `parser_actions::apply`。
2. [`parser_actions/mod.rs`](./mod.rs) 按 action 分片询问 `owns`；`ddl::owns(rule_id)` 为真时调用 `ddl::apply(...).expect("owned DDL rule has an action")`。`identify` 共用保证这个 `expect` 的前提。
3. `apply_rule` 依据 `DdlRule` 读取 RHS 的固定倒数位置。可选值常写入 `out.item = None` 或一个装箱值；列表规则克隆已有 `Vec` 后追加；一级 DDL 语句则写入 `out.statement`。
4. 主要规则族按以下顺序聚集：placement 与 ALTER TABLE（约 1935–3627 行）；RENAME/RECOVER/FLASHBACK/DISTRIBUTE/SPLIT（约 3636–4051 行）；列与约束/外键（约 4052–4772 行）；索引、库、表和视图（约 4773–5512 行）；分区/子分区（约 5513–6120 行）；表选项和 SQL 数据类型（约 6121–7700 行）；最后是 DROP/TRUNCATE 与 placement policy 语句。
5. action 返回 `Ok(true)` 时，运行时将新语义值压回栈。若返回 `Ok(false)`，运行时按 goyacc 的 `$$ = $1` 默认规则移动 RHS 首值，并依 `has_semantic_action` 标记语义完整性；`Err(status)` 直接成为解析器返回码。

## 数据与状态

`Rhs<'_>` 借用规约栈上的 `yySymType` 切片。标量字符串多从 `ident` 取值，表达式从 `expr` 取值，复合值则保存在类型擦除的 `item` 中，通过 `downcast_ref`/`downcast` 恢复为 `Vec<TableOption>`、`ColumnDef`、`PartitionOptions` 等真实类型。对可转移的所有权值，部分分支使用 `borrow_mut(...).item.take()` 避免不必要的深克隆；列表累积分支则常克隆旧 `Vec` 再追加当前元素。

`Context.output` 是本次规约的唯一主输出；`Context.lexer` 承载可累积的错误/警告；`Context.parser_state` 只在少数兼容性分支读取 `enableMariaDB`、`enableUnsupportedMySQLSyntax` 和 `strictDoubleFieldType`。本文件不持有跨规约的全局状态，分区列表、表选项、列定义等都作为当前规约值逐层上传。

## 依赖与调用关系

- 上游主链：`parser_runtime.rs::yyParse` 规约 -> `parser_actions/mod.rs::apply` -> `ddl::owns` -> `ddl::apply` -> `apply_rule`。`parser_actions/mod.rs::has_semantic_action` 也调用 `ddl::owns` 来判定默认传值后的语义完整性。
- 规则来源：[`grammar/main.astergram`](../grammar/main.astergram) 定义 Rust 产生式和 `@action`；生成的 `RULE_IDS_BY_REDUCTION` 为 `identify` 提供运行时 RuleId。
- AST 依赖：`use super::super::*` 带入 `parser_ast`、`RuleId`、`yySymType`、语义辅助函数和 MySQL 错误定义；`use super::{Context, Rhs}` 带入 action 边界。主要下游类型是 `AlterTableSpec/Stmt`、`CreateTableStmt`、`ColumnDef/Option`、`Constraint/ReferenceDef`、`IndexOption/CreateIndexStmt`、`PartitionOptions/Definition`、`CreateViewStmt` 以及各种 Drop/Recover/Flashback/Placement 节点。
- crate 边界：[`Cargo.toml`](../Cargo.toml) 的 `[lib] path = "lib.rs"`；`parser-ast` 是本地路径依赖，同时使用 `parser-mysql`、`parser-types`、`parser-terror`、`parser-charset` 等 parser 子 crate。本文件自身没有 feature gate。
- RustCodeGraph 对该文件建立了 603 个符号，可精确读取 `pkg/parser/parser_actions/ddl.rs::apply_rule`。索引的文件级关系另指向 `query.rs` 和 `remaining_aster_unit_test.rs`；主运行时调用边以上述 `mod.rs`/`parser_runtime.rs` 源码为准。

## 错误处理与边界

`apply_rule` 区分三类非正常情况。第一类是语法/action 内部不变量：固定 RHS 位置缺失时使用 `expect("DDL RHS position")` 失败，这表示生成语法与手写 action 已失配，而不是普通 SQL 输入错误。第二类是动态 `item` 缺失或 downcast 失败：关键分支返回 `Ok(false)`，交由运行时的默认传值与 `semantic_complete` 机制处理；一些可选列表则明确降级为空值或默认枚举。

第三类是用户可见的语义错误：通过 `AppendError(Errorf(...))` 或 MySQL 兼容错误对象记录，再返回 `Err(1)`。已确认的边界包括 `FOLLOWERS` 必须为正数、不支持的 ALTER `ALGORITHM`/`LOCK`、非法 TSO、重复 `COLLATE`、不合法的列/索引/分区组合、当前仅允许的 VECTOR 类型、MariaDB 专有语法开关、不支持 MySQL 语法开关以及严格 DOUBLE 类型检查。某些 Go 兼容性情形会先追加错误再调用 `LastErrorAsWarn()`，因此扩展时不能简单将它们改为硬错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、文件、网络连接或事务，也没有 `unsafe`。生命周期限于一次 LR 规约：`Rhs<'_>` 和 `Context<'_>` 都是对解析器栈、当前 parser state 与 lexer 的独占/短期借用；action 在返回前将所有权值放入 `output`，然后运行时截断 RHS 栈。

因为 parser 实例和 lexer 通过 `&mut` 传入，同一次解析内 action 是串行的。若需并发解析，应由调用方为每个任务创建独立 parser/lexer，不能共享当前 `Context`。性能敏感点主要是大量 AST/`Vec` 克隆和 `Box<dyn Any>` downcast，而非同步或 I/O。

## 与 Go 版本的对应关系

Go 对照的真实位置不是同路径 `.go` 文件，而是 [`pkg/parser/parser.y`](../parser.y) 中的 goyacc 产生式动作；[`pkg/parser/parser.go`](../parser.go) 是对应生成结果。Rust [`grammar/main.astergram`](../grammar/main.astergram) 保留产生式形状，而本文件把 Go 中的 `$n`/`$$`、类型断言和 `return 1` 分别移植为 `Rhs` 索引、`Context.output`、downcast 和 `Err(1)`。例如 Go `DirectPlacementOption` 的 `FOLLOWERS > 0` 校验与错误文本，在 Rust `DirectPlacementOptionAlt03` 中逐项保留。

两端在表达机制上有必要差异：Go 直接将 `interface{}` 断言为 AST 指针/切片，Rust 使用 `Box<dyn Any>` 的受检 downcast；Go 候选分支直接嵌入动作，Rust 用稳定 RuleId 先映射成 `DdlRule` 再集中分派；Rust 还使用 `semantic_complete` 显式记录未完整移植的语义值。这些是实现差异，不代表可以删减 Go 分支逻辑；扩展时应以同一 Go 产生式的 AST 字段、默认值、警告/错误和 feature 开关为对齐单元。

## 扩展指南

1. 新增或修改 DDL 语法时，先同时核对 `parser.y` 与 `grammar/main.astergram`，然后为新的 `@action` RuleId 增加 `DdlRule` 变体、`identify` 映射和 `apply_rule` 分支。三者必须同步，否则会破坏 ownership 或在总调度器的 `expect` 处暴露失配。
2. 新分支应复用相邻规则的 RHS 倒数索引和真实 `parser_ast` 类型，对照 Go `$n` 位置逐一核验。不要以 `unwrap_or_default` 掩盖本应是必需值的迁移缺口；必需动态值不可用时，沿用本文件的 `Ok(false)` 与语义完整性约定。
3. 涉及 AST 字段时同步检查 `pkg/parser/ast` 中的根类型和 restore/visitor 行为；涉及错误时保留 MySQL 错误码、文本、警告降级和 parser feature 开关，不只验证“能否解析”。
4. 测试首先更新 [`ddl_aster_unit_test.rs`](./ddl_aster_unit_test.rs) 的 `DDL_NONTERMINALS`/预期 inventory，并保持无 numeric fallback。真实 SQL -> AST 行为应在 [`parser_3_aster_unit_test.rs`](../parser_3_aster_unit_test.rs) 或更靠近的独立 parser 测试中增加回归；分区错误和语义边界可延伸 [`parser_semantic_support_test.rs`](../parser_semantic_support_test.rs)，Go 对照则查看 `parser_test.go` 及与具体 DDL 特性相关的测试。
5. 兼容性风险主要是 Go/Rust AST 字段、默认值和错误级别漂移；正确性风险是 RHS 位置、downcast 类型或 RuleId 漏映射；性能风险是在长列表规约中新增不必要的 AST/`Vec` 克隆。

## 验证依据

- RustCodeGraph：`status` 显示工程已索引 11,467 个文件；`files --filter pkg/parser/parser_actions/ddl.rs` 确认目标文件已索引；`node --file ... --offset 1 --limit 260` 读取枚举开头；`query apply_rule --kind function` 找到本文件定义；`node pkg/parser/parser_actions/ddl.rs::apply_rule` 用完整限定名消除同名 action 歧义。限定 `callers` 查询在本机持续无输出，因此调用边另由 `parser_actions/mod.rs` 与 `parser_runtime.rs` 直接核实。
- 源码/边界：已读 [`ddl.rs`](./ddl.rs) 的全局符号、`identify` 末端、`owns`/`apply`、`apply_rule` 分支索引、错误与 parser-state 使用点；已读 [`Cargo.toml`](../Cargo.toml)、[`lib.rs`](../lib.rs)、[`parser_actions/mod.rs`](./mod.rs) 和 [`parser_runtime.rs`](../parser_runtime.rs)。
- Go/语法对照：已查看 [`parser.y`](../parser.y) 的 ALTER TABLE、placement 等相关产生式，并对照 [`grammar/main.astergram`](../grammar/main.astergram) 的 Rust action 标记；`parser.go` 中的同文本 `FOLLOWERS must be positive` 进一步证明生成 Go 路径保留该语义。
- 测试：[`ddl_aster_unit_test.rs`](./ddl_aster_unit_test.rs) 验证 596 条 DDL RuleId 的精确所有权及无 numeric fallback；[`remaining_aster_unit_test.rs`](./remaining_aster_unit_test.rs) 验证每个 action rule 恰有一个所有者；[`parser_3_aster_unit_test.rs`](../parser_3_aster_unit_test.rs) 覆盖 CREATE/DROP/TRUNCATE TABLE、ALTER TABLE、CREATE/DROP INDEX、RENAME/RECOVER/FLASHBACK/SPLIT、分区和 placement policy 等 SQL -> AST 路径；[`parser_semantic_support_test.rs`](../parser_semantic_support_test.rs) 验证分区错误码等语义边界。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用固定 11 章节命令做结构验证，并人工复核本文能回答文件的存在原因、运行主链与安全扩展点。
