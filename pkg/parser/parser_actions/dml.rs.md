# `pkg/parser/parser_actions/dml.rs`

## 文件定位

本文件是 `astersql-parser` crate 的 DML 语义动作实现。它不负责词法分析或 LR 状态迁移，而是在 `pkg/parser/parser_runtime.rs` 完成一次语法归约时，由 `parser_actions::apply` 按稳定的 `RuleId` 分派到这里，把归约栈中的 `yySymType` 值组装成 `parser_ast` 节点。模块入口位于 `pkg/parser/parser_actions/mod.rs`；语法来源是 `pkg/parser/grammar/main.astergram`，Go 对照实现是 `pkg/parser/parser.y` 中相同产生式的动作块。

文件覆盖 INSERT、REPLACE、DELETE、UPDATE、LOAD DATA、IMPORT INTO、非事务批量 DML、CANCEL IMPORT，以及这些语句共用的赋值、列/用户变量列表、RETURNING、字段和行格式选项。它是 crate 私有实现：对父模块只暴露 `owns` 和 `apply`，不会成为解析器的外部 API。`pkg/parser/Cargo.toml` 表明本 crate 名为 `astersql-parser`，AST、MySQL 常量、字面量和字符集等能力分别来自同 workspace 的 `parser-ast`、`parser-mysql`、`parser-test-driver` 等依赖；本文件主要直接构造 `parser_ast` 类型。

## 核心职责

- `identify` 把生成器提供的稳定字符串 `RuleId` 映射为私有 `DmlRule`。映射涵盖 95 个需要动作的 DML 产生式；`dml_rule_coverage` 会把它与 `main.astergram` 的实际产生式集合比较，并检查它不与 DDL、表达式、查询动作重复认领。
- `apply_rule` 按语句族把动作路由到 `apply_general_rule`、`apply_insert_rule` 或 `apply_load_rule`，避免一个巨大 `match` 混合三类不同的数据形态。
- 各动作从 `Rhs` 的相对位置读取或移动动态语义值，构造 `Assignment`、`InsertStmt`、`DeleteStmt`、`UpdateStmt`、`LoadDataStmt`、`ImportIntoStmt`、`NonTransactionalDMLStmt` 等 AST，并写入 `Context.output.item`、`statement` 或 `ident`。
- 本文件保留 Go yacc 动作的兼容语义，例如：单表 DELETE 的修饰符和 RETURNING、LOCAL LOAD DATA 默认把重复键策略从 Error 改为 Ignore、括号子查询标记 `IsInBraces`、REPLACE 禁止 INSERT 行别名、IMPORT FROM SELECT 禁止用户变量和 SET 子句。

## 主要符号

- `DmlRule`：私有枚举，一项对应一个带动作的语法候选分支；`Alt01` 等后缀区分同一非终结符的不同产生式。它只作为文件内分派键，不进入 AST。
- `identify(rule_id: RuleId) -> Option<DmlRule>`：稳定规则 ID 的唯一登记表。未知 ID 返回 `None`，由父分派器继续尝试其他动作模块。
- `owns(rule_id) -> bool`：供 `parser_actions/mod.rs` 判定所有权；不执行动作。
- `apply(rule_id, rhs, context) -> Option<Result<bool, isize>>`：模块入口。非本模块规则返回 `None`；已认领规则返回动作结果。
- `apply_rule(...) -> Result<bool, isize>`：按规则集合选择三个实现函数。集合与实现函数的 `match` 必须同步，错误路由会触发 `unreachable!`。
- `apply_general_rule`：处理赋值及列表、DELETE/UPDATE、CANCEL IMPORT、BATCH 非事务 DML 和其布尔/枚举选项。
- `apply_insert_rule`：处理 INSERT/REPLACE 的值、SELECT、SET、行别名、ON DUPLICATE KEY UPDATE 和 RETURNING。
- `mark_select_in_braces`：只接受 `SelectStmt` 或 `SetOprStmt`，为括号子查询设置 `IsInBraces`；其他节点返回 `Err(0)`。
- `apply_load_rule`：处理 LOAD DATA/IMPORT INTO 及字段、行、字符集、格式、SET、WITH 选项等中间语义值。
- `dml_numeric_isize`：兼容动作栈中直接的 `i32` 以及 `semantic_numeric_isize` 支持的其他数字表示，用于优先级、dry-run 模式和行数等数值提取。

## 执行流程

1. `parser_runtime.rs` 根据生成表完成归约，建立 `Rhs` 和默认 `yySymType`，再调用 `parser_actions::apply(rule_id, ..., Context::new(...))`。
2. 父模块依次调用各动作模块的 `owns`；DML 稳定 ID 被 `identify` 命中后进入本文件的 `apply`。动作成功通常返回 `Ok(true)`；缺少必须的动态值会返回 `Ok(false)`，显式语法错误返回负载于 `Err(status)` 的 yacc 状态码。
3. `apply_rule` 将 INSERT/REPLACE/RETURNING 相关规则送到 `apply_insert_rule`，将 LOAD/IMPORT 及格式选项送到 `apply_load_rule`，其余送到 `apply_general_rule`。
4. 通用路径逐步构造列表或最终语句。DELETE/UPDATE 会把单表来源包成 `TableSource -> Join -> TableRefsClause`；带 WITH 的外层产生式取出已建语句，再补上 `WithClause`。多表 DELETE 设置 `IsMultiTable`，并用 `BeforeFrom` 区分两种语法位置。BATCH 语句组合 dry-run 模式、可选分片列、限制值和内部可分片 DML。
5. INSERT 路径先由 `InsertValues` 产生一个尚未绑定目标表的 `InsertStmt`：输入可为 VALUES 行、SELECT/集合操作/括号子查询，或 SET 列值对；随后 `InsertIntoStmtAlt01` 补目标表、分区、优先级、IGNORE、提示、重复键赋值和 RETURNING。REPLACE 复用相同中间节点，先拒绝行/列别名，再设置 `IsReplace` 和目标表信息。
6. LOAD 路径先生成可选项和列表，再由 `LoadDataStmtAlt01` 汇总文件位置、格式、重复键策略、表、字符集、字段/行分隔规则、忽略行数、列或用户变量、SET 赋值及 WITH 选项。IMPORT 文件形式复用这些中间值；SELECT 形式提取查询节点并施加额外限制。
7. 结果写入归约输出；运行时将它压回语义值栈并继续 LR 解析。若动作未处理，运行时仅在规则没有登记语义动作时才安全采用 goyacc 的默认 `$$ = $1` 语义。

## 数据与状态

`Rhs` 是当前产生式右侧语义值的可变视图，索引采用 `rhs_len - back` 的相对位置以对应 Go 动作中的 `$n`。简单、仍会被其他字段读取的值通常经 `as_deref`、`downcast_ref` 后克隆；拥有动态 AST 或大列表的值会用 `take` 和 `downcast` 移出，避免复制 trait object。类型不匹配的可选值大多回落到空列表、默认标量或 `None`，而构造最终节点不可缺少的值（例如 INSERT 的中间语句、目标表或 UPDATE 的 Join）缺失时返回 `Ok(false)`。

`Context.output` 是本次归约唯一的输出槽；本文件不使用 `parser_state`。`lexer` 只在需要记录用户可见解析错误时使用。中间值以 `Box<dyn Any>` 保存，包括 `Vec<Assignment>`、`InsertStmt`、`FieldsClause`、`Option<String>` 等；最终语句放在 `yySymType.statement`，表达式和辅助对象放在 `item`/`expr`，终止符文本可放在 `ident`。

重要不变量包括：INSERT 的 `Columns` 与 VALUES/SET 语义保持对应；SET 模式以 `Setlist = true` 标记且值集中在 `Lists[0]`；括号 SELECT/集合操作必须保留 `IsInBraces`；LOAD DATA 同时保留 `ColumnsAndUserVars`，并另行派生只有真实列的 `Columns`；无选项产生式使用明确的空向量或 `None`，其选择需与下游 AST 语义一致。

## 依赖与调用关系

上游主链为 `parser_runtime.rs` 的归约循环 → `parser_actions/mod.rs::apply` → `dml::owns`/`dml::apply` → `apply_rule`。RustCodeGraph 已将目标识别为 107 个符号，并精确定位 `apply_rule`、`apply_insert_rule`、`mark_select_in_braces`、`apply_load_rule` 和 `dml_numeric_isize`；当前 CLI 的 callers/callees 子命令未输出边，因此上述调用边以相邻模块和运行时代码直接核对。

下游主要是 `parser_ast` 数据类型和父模块导入的解析辅助类型/函数：`RuleId`、`Rhs`、`Context`、`yyLexer`、`SubquerySemantic`、`TextStringSemantic`、`take_subquery_statement`、`semantic_numeric_isize`、`getUint64FromNUM`、`ErrSyntax`。`parser_test_driver::{HexLiteral, BitLiteral}` 为十六进制和位字面量提供终止符字符串转换。语法层依赖 `main.astergram` 中对应的 RHS 排列；任何产生式顺序变化都会影响本文件的相对栈位置。

没有数据库、网络、执行器或存储层调用。该文件只把 token/中间语义转换成 AST；AST 的执行、权限和事务含义由解析器下游负责。

## 错误处理与边界

- 动态类型或必需节点缺失时，动作通常返回 `Ok(false)`；这代表本次动作未能产生有效语义值，不等价于业务校验错误。父运行时还会结合 `has_semantic_action` 设置 `semantic_complete`，因此不能把这些分支随意改成默认 AST。
- `REPLACE ... InsertValues` 若携带行别名或列别名，`ReplaceIntoStmtAlt01` 通过 `AppendError(ErrSyntax...)` 记录语法错误并返回 `Err(1)`。
- `ENCLOSED BY`、`OPTIONALLY ENCLOSED BY`、`ESCAPED BY` 的终止符除单个字符外只允许反斜杠；违反时追加 `Wrong field terminators` 并返回 `Err(1)`。
- `IMPORT INTO ... FROM SELECT` 禁止用户变量，也禁止非空 SET 赋值；两个分支均向 lexer 追加具体错误并返回 `Err(1)`。空的 `LoadDataSetSpecOpt` 在 Rust 中表现为空向量，校验因此明确检查“非空”，等价于没有 SET 子句。
- `mark_select_in_braces` 仅支持 `SelectStmt`/`SetOprStmt`，其他节点返回 `Err(0)`。调用方必须维持 `SubSelect` 的节点种类约束。
- 数字提取失败会使用默认值；IGNORE LINES 还会以 `max(0)` 阻止负 `isize` 转成极大的 `u64`。`getUint64FromNUM` 的进一步解析语义由父模块辅助函数负责。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或全局可变状态。动作在单次解析器归约调用内同步执行；`Rhs<'_>`、`Context<'_>` 的借用生命周期把栈切片、输出槽、解析器状态和 lexer 限定在该次调用。

资源管理的重点是所有权移动：`take()` 从 RHS 取走 AST、列表或语句，避免动态节点克隆；失败的 `downcast` 会使已取值不再留在原槽，因此应只在规则保证类型时使用。构建完的 `Box<dyn parser_ast::Node>` 随解析结果向下游转移。本文件不持有文件句柄或外部资源；`LOAD DATA` 的 Path/FileLocRef 只是 AST 描述，并不在解析阶段打开数据源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/parser.y`，生成后的 `pkg/parser/parser.go` 也包含同一组动作。Rust 将 Go yacc 动作从产生式旁移入稳定 `RuleId` 分派，但数据流保持对应：Go 的 `$n`/`$$` 对应 Rust 的 `rhs[rhs_len - back]`/`Context.output`，Go 的类型断言对应 Rust `Any` 下转型，Go 指针 AST 对应 Rust 的 owned/boxed AST。

已核对的关键等价点包括：INSERT 先创建值或 SELECT 形态再补目标表；REPLACE 拒绝行别名；DELETE/UPDATE 保留优先级、IGNORE、提示、ORDER/LIMIT/RETURNING 和 WITH；LOAD DATA LOCAL 将默认重复键策略改为 Ignore，并从列/用户变量混合列表派生纯列列表；字段终止符执行相同长度校验；IMPORT FROM SELECT 禁止用户变量与 SET；BATCH DML 保留 dry-run 三态、分片列、限制和内部语句。

实现层存在语言表示差异而非预期行为差异：Rust 用空向量表示部分 Go `nil` slice，用 `Option<T>` 表示指针可选值，用 `Default` 填充未涉及字段，并通过 `Result<bool, isize>` 把“已处理”“未构造”和 yacc 返回码分开。错误文本中 Rust 的用户变量限制信息不插入变量名，而 Go 文本会包含名称；这是当前代码事实，若要求错误消息逐字兼容需单独评估。

## 扩展指南

新增或修改 DML 语法时，应先更新 `pkg/parser/grammar/main.astergram`，取得生成器稳定规则 ID，再同步 `DmlRule`、`identify`、`apply_rule` 的路由集合和对应实现函数。不要恢复基于数值归约号的分派；`dml_module_has_no_numeric_fallback` 明确保护这一约束。调整 RHS 时必须逐项重算相对索引，并与 `pkg/parser/parser.y` 的 `$n` 数据流核对。

若新增 INSERT/REPLACE 形态，优先扩展 `apply_insert_rule` 并维持“中间 InsertStmt 后绑定目标表”的两阶段结构；若新增 LOAD/IMPORT 格式项，保持明确的 `Option`/空列表语义，并确认文件形式和 SELECT 形式是否都适用；若新增最终语句字段，要同时检查 AST restore/visitor 行为以及 Go 动作是否设置相同字段。

测试逻辑继续放在独立文件：行为回归扩展 `pkg/parser/parser_actions/dml_test.rs`；规则所有权、数量和禁止数值回退等结构约束扩展 `pkg/parser/parser_actions/dml_aster_unit_test.rs`。建议覆盖成功 AST 字段、拒绝路径的错误码/错误文本、空选项与显式选项、单表/多表和括号 SELECT。兼容风险主要来自 Go/Rust AST 字段遗漏、nil/空集合差异和错误消息差异；性能风险主要来自把现有 `take` 改成深克隆或在长列表归约中反复复制。

## 验证依据

- 目标实现：`pkg/parser/parser_actions/dml.rs`，完整读取 1–1621 行；主要实现位置为 `identify`（119）、入口（305/309）、三段动作（399/851/1194）及 `mark_select_in_braces`（1176）。
- 运行时与模块入口：`pkg/parser/parser_runtime.rs` 归约循环（约 180–230 行）；`pkg/parser/parser_actions/mod.rs` 的 `Context`、`apply`、`has_semantic_action`。
- crate 边界：`pkg/parser/Cargo.toml`；语法事实：`pkg/parser/grammar/main.astergram` 中 DML 产生式。
- Go 对照：`pkg/parser/parser.y` 的 `insertRowAlias`、DELETE、CANCEL IMPORT、INSERT/REPLACE、UPDATE、LOAD DATA/IMPORT INTO、BATCH DML 动作；生成文件 `pkg/parser/parser.go` 的对应动作位置通过搜索确认。
- 独立 Rust 测试：`pkg/parser/parser_actions/dml_test.rs` 验证 DELETE、INSERT、REPLACE、LOAD DATA、IMPORT INTO 的关键字段；`pkg/parser/parser_actions/dml_aster_unit_test.rs` 验证 95 条规则的完整且唯一所有权，以及无数值规则回退。Go AST 相关测试位于 `pkg/parser/ast/dml_test.go`。
- RustCodeGraph：`status` 显示目标仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/parser_actions` 确认目标及独立测试已索引；`query` 精确定位本文件的五个关键函数。`explore/node/callers/callees` 在本次 CLI 调用中未返回正文/边，故调用关系改由上述源码直接核验，未把缺失图输出当作结论。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档存在且恰有 11 个固定二级章节，并人工检查只新增本说明、链接指向真实路径、未建议把测试嵌入生产源文件。
