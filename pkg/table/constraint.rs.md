# `pkg/table/constraint.rs`

## 文件定位

`pkg/table/constraint.rs` 属于 `astersql-table` crate 的 CHECK 约束边界模块。`pkg/table/lib.rs` 以 `pub mod constraint` 挂载该模块，并通过 `pub use constraint::*` 将公共类型和函数提升到 crate 根。它位于解析器 AST、表元数据与表达式运行时之间：一部分函数清理并包装已经持久化的 `TableInfo.Constraints`，另一部分函数在 DDL 校验阶段检查 CHECK 表达式、AUTO_INCREMENT 和外键引用动作，还提供把约束 SQL 文本构建成可执行表达式的入口。

当前 Rust 生产接线并不等同于 Go 的完整调用面。`pkg/table/tables/tables.rs::table_from_meta` 已调用 `constraint::LoadCheckConstraint`，把清理后的约束装入运行时表对象；代码搜索未发现 `IsSupportedExpr`、`ContainsAutoIncrementCol`、`HasForeignKeyRefAction` 或 `IfCheckConstraintExprBoolType` 的 Rust 生产调用者，它们目前主要由独立 Rust 测试验证。Go 侧则在 `pkg/ddl/create_table.go` 和 `pkg/ddl/executor.go` 中串联这些校验。因此，本文件既包含已接线的元数据加载逻辑，也包含待 Rust DDL 主链消费的 Go 对齐 API。

## 核心职责

1. `LoadCheckConstraint` 在加载表元数据时调用 `removeInvalidCheckConstraintsInfo`，删除引用缺失列或非 Public 列的 CHECK 元数据，然后把每个 `ConstraintInfo` 包装为运行时 `Constraint`。
2. `BuildConstraintExprWithCtx` / `buildConstraintExpression` 把 `ConstraintInfo.ExprString` 交给 `expression_dependency::ParseSimpleExpr`，并通过 `WithTableInfo(database_name, table_info)` 提供列解析环境，返回 `Box<dyn Expression>`。
3. `IsSupportedExpr` 使用 `checkConstraintChecker` 深度优先遍历解析器表达式 AST，拒绝不确定、依赖会话状态或 MySQL 禁止用于 CHECK 的节点。
4. `ContainsAutoIncrementCol` 判断 CHECK 的依赖列是否包含表中带 `AutoIncrementFlag` 的列。
5. `HasForeignKeyRefAction` 同时支持“已有 FK 元数据”和“CREATE TABLE AST”两条路径，禁止 CHECK 依赖带 ON DELETE/ON UPDATE 动作的外键列。
6. `IfCheckConstraintExprBoolType` 构建表达式并检查其类型标志含 `IsBooleanFlag`，否则返回非布尔 CHECK 错误。

该模块不执行行级 CHECK 求值、不维护 DDL 状态机，也不写入持久化存储；它提供的是元数据修复、表达式构建和 DDL 合法性校验原语。

## 主要符号

- `type ConstraintError = errors_dependency::SharedError`：模块统一错误类型。表达式解析错误通过 `ConstraintError::new` 包装，约束规则错误由 `dbterror_dependency` 的规范错误生成器产生。
- `pub struct Constraint { pub ConstraintInfo: Box<constraint_model::ConstraintInfo> }`：规范 CHECK 元数据的运行时包装；`Clone` 会克隆其 boxed 元数据。
- `pub fn LoadCheckConstraint(&mut constraint_model::TableInfo) -> Result<Vec<Box<Constraint>>, ConstraintError>`：公开加载入口。虽然签名可失败，当前函数体只有清理、克隆和收集，没有显式错误分支。
- `fn removeInvalidCheckConstraintsInfo(...)`：内部原地修复函数。对每项约束要求 `ConstraintCols` 中每个列名都能被 `FindPublicColumnByName` 找到；空依赖列集合按 `all` 的真空真规则保留。
- `pub fn BuildConstraintExprWithCtx(...) -> Result<Box<dyn Expression>, ConstraintError>`：公开表达式构建入口；`fn buildConstraintExpression` 是实际调用 `ParseSimpleExpr` 的内部实现。
- `pub fn IsSupportedExpr(&ast::Constraint) -> (bool, Option<ConstraintError>)`：公开 AST 支持性检查。缺少 `Expr` 时返回 `(true, None)`。
- `const UNSUPPORTED_FUNCTIONS`：禁止函数的小写名称表，包括时间、用户/连接状态、锁、文件、随机数、UUID、休眠和 `embed_text` 等函数。
- `struct checkConstraintChecker`：保存 `allowed`、首个拒绝原因和约束名。`check` 递归分派全部 `ExprKind`，`check_all` 在首次失败后短路。
- `pub fn ContainsAutoIncrementCol(...) -> bool`：查找第一个带自增标志的列，再按 `CIStr.L` 与依赖列比较。
- `pub fn HasForeignKeyRefAction(...) -> Result<(), ConstraintError>`：`Some(foreign_keys)` 必走持久化元数据路径，包括 `Some(Vec::new())`；只有 `None` 才扫描传入的 AST 约束。
- `fn checkForeignKeyRefActionByFKInfo`、`fn foreign_key_action_error`：元数据路径与统一错误构造器。
- `fn hasSpecifiedCol`：按 `CIStr.L` 比较列名；当前仅是内部辅助，生产实现中的两个 FK 分支直接使用等价的迭代判断。
- `pub fn IfCheckConstraintExprBoolType(...) -> Result<(), ConstraintError>`：使用当前数据库构建表达式并验证布尔类型标志。

文件没有 feature 条件编译项、异步函数或 trait/impl 公共扩展点；唯一 `impl` 是 `checkConstraintChecker` 的内部遍历实现。

## 执行流程

运行时表加载流程如下：

1. `pkg/table/tables/tables.rs::table_from_meta` 完成列和生成/默认表达式的前置处理后，调用 `table_dependency::constraint::LoadCheckConstraint(meta)`。
2. `LoadCheckConstraint` 原地过滤 `table_info.Constraints`。任一依赖列缺失或不是 Public，整项约束被移除。
3. 剩余 `ConstraintInfo` 被逐项克隆、装入 `Constraint`，随后进入运行时表对象；因此调用后传入的 `TableInfo` 与返回列表反映同一组有效约束，但各自持有克隆值。

表达式构建与类型检查流程如下：

1. `IfCheckConstraintExprBoolType` 从 `BuildContext.GetEvalCtx().CurrentDB()` 取得数据库名。
2. `BuildConstraintExprWithCtx` 读取 `ConstraintInfo.ExprString`，转交 `buildConstraintExpression`。
3. `buildConstraintExpression` 用 `WithTableInfo` 把数据库名和表元数据加入简单表达式解析选项，返回可执行表达式或传播包装后的解析错误。
4. 类型检查读取 `Expression::GetType(eval_ctx).GetFlag()`；未含 `HasIsBooleanFlag` 时生成 `ErrNonBooleanExprForCheckConstraint`。

AST 合法性检查从 `IsSupportedExpr` 开始。检查器按深度优先顺序遍历函数参数、二元/一元操作数、IN/BETWEEN/LIKE/REGEXP、行、CASE、聚合函数参数与排序项、窗口函数参数等。遇到禁止函数、变量、任一子查询形态或 DEFAULT 后立即记录第一个错误并停止继续遍历；字面量、列、参数标记等叶节点本身不拒绝。

外键冲突检查有明确的分支不变量：`foreign_keys = Some(...)` 表示调用者已经选择持久化 FK 元数据，即使集合为空也不会回退扫描 AST；`None` 才代表 CREATE TABLE 阶段，应扫描 `constraints` 中的 ForeignKey 节点。两条路径都先忽略无引用动作的 FK，再仅在 CHECK 的依赖列与 FK 本地列相交时返回错误。

## 数据与状态

- `TableInfo.Constraints` 是本文件唯一被原地修改的外部状态。过滤使用克隆后整体替换，既不会修改约束内部字段，也不会持久化这次“惰性修复”；是否写回由更上层负责。
- `Constraint.ConstraintInfo` 拥有一份 boxed 克隆，而不是借用 `TableInfo`。这简化运行时表对象的生命周期，但会复制约束名、表达式字符串和依赖列等元数据。
- `checkConstraintChecker` 是单次调用内的可变状态：初始 `allowed = true`、`reason = None`；首次拒绝后 `allowed = false`，后续递归立即返回，因此错误具有“首个按遍历顺序发现的违规节点”语义。
- 名称比较统一使用 `CIStr.L`，即规范化小写形式。`ContainsAutoIncrementCol`、两条 FK 检查路径和无效列清理都依赖该大小写不敏感身份。
- `UNSUPPORTED_FUNCTIONS` 是静态只读切片。查找为线性 `contains`；列表当前规模很小，若显著扩展需评估改用集合或解析器提供的规范分类。
- 本文件不缓存构建后的表达式。每次 `BuildConstraintExprWithCtx` / `IfCheckConstraintExprBoolType` 调用都会重新解析 `ExprString`。

## 依赖与调用关系

crate 边界由 `pkg/table/Cargo.toml` 确认：本文件直接使用 `astersql-expression`、`astersql-meta-model`、`astersql-parser-ast`、`astersql-parser-mysql`、`astersql-util-dbterror` 和 `astersql-errors` 的依赖别名。`Cargo.toml` 没有为本模块声明专属 feature；模块通过 `pkg/table/lib.rs` 无条件编译并再导出。

已确认的 Rust 调用关系：

- 上游：`pkg/table/tables/tables.rs::table_from_meta` -> `LoadCheckConstraint`。
- 内部：`LoadCheckConstraint` -> `removeInvalidCheckConstraintsInfo` -> `TableInfo::FindPublicColumnByName`。
- 内部：`BuildConstraintExprWithCtx` -> `buildConstraintExpression` -> `expression_dependency::ParseSimpleExpr`，并调用 `WithTableInfo`。
- 内部：`IfCheckConstraintExprBoolType` -> `BuildConstraintExprWithCtx` -> `Expression::GetType` -> MySQL 布尔标志检查。
- 内部：`IsSupportedExpr` -> `checkConstraintChecker::check` / `check_all` -> 相应 `dbterror` 错误生成器。
- 内部：`HasForeignKeyRefAction` -> `checkForeignKeyRefActionByFKInfo` 或 AST 扫描 -> `foreign_key_action_error`。

RustCodeGraph 的文件节点报告 `pkg/table/constraint.rs` 被 7 个文件使用，其中包括 `pkg/ddl/create_table.rs` 和若干测试；但对本文件具体函数运行 `callers/callees` 未返回符号边。后续用精确 `rg` 核验后，只确认上述 `LoadCheckConstraint` 生产调用；`create_table.rs` 对 `ast::Constraint` 的一般使用不等于调用本模块的校验 API，因此不能据文件级依赖推断完整 DDL 接线。

## 错误处理与边界

- 表达式解析失败由 `ParseSimpleExpr` 返回，经 `ConstraintError::new` 包装后原样传播。与 Go `buildConstraintExpression` 相比，Rust 当前没有同位置的后台错误日志；调用者只能从返回错误获知失败。
- `IsSupportedExpr` 返回 `(bool, Option<ConstraintError>)` 而非 `Result`。有效表达式应为 `(true, None)`；禁止节点应为 `(false, Some(error))`。`Expr = None` 被视为支持，调用者若要求 CHECK 必须有表达式，需要在更早阶段保证该结构不变量。
- 禁止具名函数返回 `ErrCheckConstraintNamedFuncIsNotAllowed`，变量返回 `ErrCheckConstraintVariables`，子查询返回 `ErrCheckConstraintFuncIsNotAllowed`，DEFAULT 复用具名函数错误并以 `default` 为函数名。
- AST 遍历明确覆盖当前 `ExprKind` 的所有变体。增加新的 AST 变体时，Rust 的穷尽匹配通常会迫使这里作出决定；不能简单把新节点归为允许，必须核对 MySQL/TiDB CHECK 规则。
- AST 外键路径对 `Refer = None` 或索引键 `Column = None` 做跳过处理，避免崩溃；持久化路径把 `OnDelete == 0 && OnUpdate == 0` 视为无引用动作。
- `ContainsAutoIncrementCol` 只使用找到的第一个自增列，这与表只能有一个 AUTO_INCREMENT 列的模型不变量一致；若上游允许非法的多个自增标志，本函数不会检查后续列。
- 非布尔表达式通过 `ErrNonBooleanExprForCheckConstraint` 报告，并使用约束原始名 `info.Name.O`。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。所有检查在调用线程同步完成；AST 检查器和表达式对象都是单次调用局部值。

并发安全的主要边界是 `LoadCheckConstraint` 要求 `&mut TableInfo`：Rust 借用规则保证清理期间不存在同一 `TableInfo` 的并发读写。返回的 `Constraint` 拥有克隆元数据，不依赖该可变借用的后续生命周期。表达式构建仅借用 `BuildContext` 和 `TableInfo` 完成解析，返回的 trait object 不携带显式借用生命周期。

资源成本主要来自约束元数据克隆、AST 递归和表达式重复解析。递归深度由输入表达式深度决定；代码没有显式深度限制，此边界通常应由解析器或上游 SQL 限制承担。`HasForeignKeyRefAction` 的最坏扫描成本约为 FK 数量、依赖列数量和 FK 列数量的乘积；常规表规模下较小，但扩展时不应在内层增加昂贵操作。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/table/constraint.go`。Rust 保留了 Go 的 `Constraint`、六个公开函数以及主要内部辅助的职责和命名，核心语义对应如下：

- 两版 `LoadCheckConstraint` 都先惰性删除引用不到 Public 列的约束，再生成运行时包装。Rust 模型使用值向量，因此通过克隆构造独立 boxed 值；Go 使用指针切片。
- 两版表达式构建都调用 `ParseSimpleExpr` 并附带 `WithTableInfo`。Go 会对失败执行 `errors.Trace` 并记录“wrong check constraint expression”日志；Rust 只包装并返回错误，这是可观测性差异。
- Go 通过 `ast.Walk` 的 visitor 检查禁止节点；Rust 对 `ExprKind` 做显式递归。禁止函数集合与 Go 对齐，并由 `pkg/table/go_merge_49_test.rs::go_merge_49_rejects_embed_text_in_check_constraint` 特别覆盖 `embed_text`。
- Go 只需处理其 AST 中的子查询节点；Rust 同时明确拒绝 `Subquery`、`CompareSubquery`、`InSubquery`、`ExistsSubquery`，使本地枚举的所有子查询形态落入同一错误语义。
- Go 用 `nil fkInfos` 在 AST 路径和元数据路径之间切换；Rust 用 `Option<Vec<Box<FKInfo>>>` 精确保留该语义。`None` 对应 nil，`Some(empty)` 对应非 nil 空切片。
- 两版都按规范化小写名比较列，并以 ON DELETE/ON UPDATE 是否为零或 NoOption 判定有无引用动作。
- Go 通过 `GetAutoIncrementColInfo` 获取自增列；Rust直接扫描 `Columns` 的标志。两者在合法表元数据下等价。
- 两版布尔检查都使用当前数据库构建表达式，再检查 MySQL 的布尔类型标志。

最大的迁移状态差异不在本文件内部，而在上游接线：Go 的 `pkg/ddl/create_table.go` 和 `pkg/ddl/executor.go` 会依次使用支持性、自增、外键动作和布尔类型检查；当前 Rust 搜索只确认这些函数的测试调用，没有确认等价生产调用。扩展或宣称 DDL CHECK 完整支持前，应先补齐并验证这条上游链路，而不是仅依据这些 API 已存在。

## 扩展指南

- 增删禁止函数时，修改 `UNSUPPORTED_FUNCTIONS`，同时在独立测试文件中加入允许/拒绝和错误类型断言。不要把测试写回 `constraint.rs`；优先扩展 `pkg/table/constraint_migration_aster_unit_test.rs` 或面向 Go 提交对齐的 `pkg/table/go_merge_49_test.rs`。
- parser AST 新增表达式节点时，审查 `checkConstraintChecker::check` 的对应分支：递归节点必须遍历所有子表达式，叶节点需明确说明为何安全，涉及子查询、变量、DEFAULT 或非确定性行为的节点应产生与 Go/MySQL 一致的错误。
- 修改表达式构建时，保持 `BuildConstraintExprWithCtx` 返回可执行 `Expression` 的公共契约；`pkg/table/constraint_test.rs::build_constraint_expression_returns_executable_expression` 是编译期回归保护。若添加缓存，必须定义数据库名、表 schema 版本、表达式上下文和类型标志变化时的失效规则。
- 修改 FK 分支时，必须保留 `None` 与 `Some(empty)` 的区别，并同步 `foreign_key_ast_action_obeys_none_vs_non_none_metadata_branch`。不要用“空集合则回退 AST”的简化破坏 Go nil 语义。
- 修改无效约束清理时，注意函数会原地改变 `TableInfo`；应同时验证返回包装和源元数据的结果，并覆盖缺失列、非 Public 列、空依赖和多个依赖列。
- 若将校验 API 接入 Rust DDL，最可能的接入点是创建/变更 CHECK 元数据的 Rust DDL 构建流程。应逐项对照 Go `pkg/ddl/create_table.go` 与 `pkg/ddl/executor.go` 的调用顺序，并增加独立 DDL 测试，覆盖禁止表达式、自增列、FK 引用动作和非布尔表达式；不可只靠本模块单测声称端到端完成。
- 性能改动应重点衡量表达式重复解析、约束元数据克隆与 FK 嵌套扫描；兼容性改动则需保持错误码、错误参数顺序、`CIStr.L/O` 的选择和 Go nil 语义。

## 验证依据

本说明使用了以下直接证据：

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件且索引时间晚于目标源文件；`node --file pkg/table/constraint.rs --offset 1 --limit 500` 读取完整 400 行源码并报告文件级使用者；对六个公开函数执行 `query`，确认 Rust/Go 同名定义；对精确符号执行 `callers/callees` 未返回边，因此按技能规则用源码搜索补齐调用证据。
- 源码与装配：`pkg/table/constraint.rs`、`pkg/table/lib.rs`、`pkg/table/tables/tables.rs`。
- crate 声明：`pkg/table/Cargo.toml`，确认 crate 名、库入口、直接依赖别名与 Go 包迁移元数据。
- Go 对照：`pkg/table/constraint.go`；上游调用证据来自 `pkg/ddl/create_table.go`、`pkg/ddl/executor.go`、`pkg/table/table.go` 和 `pkg/table/tables/tables.go`。
- Rust 独立测试：`pkg/table/constraint_test.rs` 验证表达式构建返回可执行表达式；`pkg/table/constraint_migration_aster_unit_test.rs` 验证无效列清理、大小写不敏感自增列匹配、FK 元数据动作冲突及 `None`/`Some(empty)` 分支；`pkg/table/go_merge_49_test.rs` 验证 `embed_text` 被拒绝。
- 人工复核：文档已区分已接线行为、公开但未发现生产调用的 API 和 Go 侧完整主链；没有把文件级依赖误写成符号调用，也没有把测试覆盖误写成端到端支持。

本任务是纯文档分析，依计划不运行 Cargo。交付前使用任务规定的正则结构检查，要求本文件存在且恰有 11 个固定二级标题。
