# `pkg/planner/core/subquery_plan_builder.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate；crate 根在 [`pkg/planner/core/lib.rs`](lib.rs) 中以 `pub mod subquery_plan_builder;` 公开模块，crate 清单是 [`pkg/planner/core/Cargo.toml`](Cargo.toml)。它实现一组“窗口表达式中的子查询需要外层相关列时，为 SELECT 列表补辅助字段”的 Rust 侧辅助逻辑，数据模型来自 [`logical_plan_builder.rs`](logical_plan_builder.rs) 的 `SelectField`、[`task.rs`](task.rs) 的 `Expression`/`PlanNode`，构建器与错误类型来自 [`planbuilder.rs`](planbuilder.rs) 的 `PlanBuilder`/`Result`。

当前接线状态需要特别区分：模块已经进入 crate，但仓库内对 Rust `appendAuxiliaryFieldsForSubqueries` 的全文检索只命中其定义和内部调用，未发现生产调用者；相对地，Go 同名方法在 [`logical_plan_builder.go`](logical_plan_builder.go) 的 `resolveWindowFunction` 中处理 SELECT 字段、窗口定义和带窗口函数的 ORDER BY 三类节点。因此，本文件是已编译进模块树的迁移实现，尚不能据此断言 Rust 主规划链已经使用这套辅助字段逻辑。

## 核心职责

文件把工作拆为四层：

1. `subqueryExprExtractor::Enter` 依据 `Expression.name` 的前缀识别四类子查询占位表达式，并把命中的表达式克隆到 `exprs`。
2. `correlated_ids` 从表达式名称中的一个或多个 `corr=<整数>` 片段提取外层列唯一 ID。
3. `findColumnNameByUniqueID` 从计划节点的字符串标签恢复 `schema.table.column`，并兼顾 Join/Apply 的 `full-column` 标签和一元包装节点。
4. `PlanBuilder::appendAuxiliaryFieldsForSubqueries` 去掉已经存在或重复发现的列，把缺失列构造成新的无别名、非通配符 `SelectField` 返回。

这套实现只补投影字段，不负责构建子查询计划、执行解相关或创建 Apply；源文件开头所述“便于后续解相关与 Apply 改写”是它的下游目的，不是本文件直接完成的动作。

## 主要符号

- `subqueryExprExtractor { exprs: Vec<Expression> }`：公开结构体，保存已识别的子查询表达式副本。`Enter(&mut self, &Expression) -> bool` 命中时入队并返回 `true`；`Leave(&self, &Expression) -> bool` 恒为 `true`，但本仓库未发现 Rust 侧遍历器 trait 接线或对 `Leave` 的调用。
- `is_subquery_expression(&Expression) -> bool`：私有的大小写不敏感前缀判定，接受 `subquery:`、`exists-subquery:`、`compare-subquery:`、`in-subquery:`。
- `ColumnName { schema, table, name }`：公开的三段式列名值对象，字段均为 `String`。
- `findColumnNameByUniqueID(&PlanNode, i64) -> Option<ColumnName>`：公开查名入口。先查当前节点的 `column` 标签；仅对 `HashJoin`、`MergeJoin`、`Apply` 再查 `full-column`；仅当恰有一个孩子时递归下探。
- `lookup_column_label`：私有标签解码器，识别键名 `{prefix}:{id}:schema.table.name`，用 `splitn(3, '.')` 保留列名段中的后续点号。
- `correlated_ids`：私有文本协议解析器，允许负号，遇到首个既非数字也非 `-` 的字符即结束当前 ID；无法解析的片段被静默丢弃。
- `field_matches_column`：私有去重判定。带别名字段永不算作已覆盖；无别名字段按不区分大小写的 `column`、`table.column` 或 `schema.table.column` 文本匹配。
- `PlanBuilder::appendAuxiliaryFieldsForSubqueries`：公开主入口，签名返回 `Result<Vec<SelectField>>`。当前函数体没有构造 `BuilderError` 的分支，所以现有实现总是 `Ok(fields)`。

文件没有模块级常量、trait、宏或条件编译项。

## 执行流程

调用者传入外层 `plan`、原始 `selectFields` 和候选 `nodes` 后，主入口先克隆 SELECT 列表，保证不原地修改调用者切片。随后逐个处理候选表达式：它只直接调用一次 `extractor.Enter(node)`；若顶层表达式不是四种前缀之一就跳过，并不会递归访问其子表达式。命中后，从该表达式名称提取所有 `corr=<id>`。

对每个 ID，`findColumnNameByUniqueID` 按如下次序查找：当前节点 `column:{id}:...` 标签；若节点是 HashJoin、MergeJoin 或 Apply，再查 `full-column:{id}:...`；仍未找到且节点只有一个孩子时继续向下。零孩子或多孩子节点不会继续搜索，找不到列名时主流程静默略过该 ID。

恢复列名后，主入口先在“原字段加此前新增字段”的 `fields` 中执行 `field_matches_column`。已有无别名列则不追加；带别名列按照 Go 现有规则仍允许追加原始列。新字段名称按可用限定层级选择 `column`、`table.column` 或 `schema.table.column`，再由局部 `HashSet<String>` 防止同一次调用中重复追加。最终新增的 `Expression` 只有 `name`，其 `column`、`return_type` 为空，函数计数为零且非虚拟列；字段无别名、非 `*`、无表通配符。

## 数据与状态

所有状态都局限于一次同步调用：`fields` 是输入 SELECT 列表的拥有型副本，`appended` 只记录本次调用已经生成的限定名，单个 extractor 只服务一个候选节点。函数不会写入 `PlanBuilder` 字段，虽然接收的是 `&mut self`；也不会修改传入的计划树或表达式。

该实现依赖两套非类型化编码约定：相关列 ID 编码在 `Expression.name` 的 `corr=` 片段中，列元数据编码在 `PlanNode.labels` 的键中。仓库全文检索未在其他 Rust 生产文件发现这些完整标签协议的明确生产者；因此这些约定当前主要由本文件自身定义，接线时必须同时验证上游确实生成同格式数据。

列名匹配是 ASCII 小写化后的字符串比较，不采用解析器的标识符、引号或排序规则语义。`HashSet` 的键则使用原始大小写的限定名，不过在插入前已经用大小写不敏感匹配扫描 `fields`，通常可阻止仅大小写不同的重复字段。

## 依赖与调用关系

Rust 内部调用链为：`appendAuxiliaryFieldsForSubqueries` → `subqueryExprExtractor::Enter` → `is_subquery_expression`，以及主入口 → `correlated_ids` / `findColumnNameByUniqueID` / `field_matches_column`；`findColumnNameByUniqueID` → `lookup_column_label`，并可能递归调用自身。RustCodeGraph 对这些内部边有记录，但对 Rust 主入口没有生产调用边；名称相同的 Go 符号会造成查询歧义，故调用者结论另以限定在 `*.rs` 的 `rg` 结果核实。

上游数据类型来自同 crate 的 `logical_plan_builder`、`planbuilder` 和 `task`，唯一标准库容器依赖为 `HashSet`；该文件没有直接使用 `Cargo.toml` 中的外部 crate 依赖，也不受 `nextgen` feature 条件控制。

Go 应用主链是 [`logical_plan_builder.go`](logical_plan_builder.go) 的 `resolveWindowFunction` → [`subquery_plan_builder.go`](subquery_plan_builder.go) 的 `appendAuxiliaryFieldsForSubqueries` → `rewrite` / `coreusage.ExtractCorrelatedCols4LogicalPlan` → `findColumnNameByUniqueID`。这是理解本文件目标位置的对照证据，不是 Rust 当前已存在的调用链。

## 错误处理与边界

当前 Rust 主入口声明 `Result`，实际没有错误分支。无法识别的表达式前缀、非法 `corr=`、找不到的列 ID 都被跳过，调用者无法区分“没有相关列”和“编码不符合约定”。这与 Go 版本在 `rewrite` 失败时返回经 `errors.Trace` 包装的错误不同。

边界行为包括：空 `nodes` 原样返回字段；一个表达式可以携带多个相关 ID；负 ID 可解析；`corr=--1` 会因整数解析失败而忽略；带别名的同列不会阻止追加；只有 HashJoin、MergeJoin、Apply 会检查 `full-column`；一元链可穿透 Selection/Projection/Window 一类包装，多孩子的非指定节点不会搜索孩子。`lookup_column_label` 遍历 `HashMap` 键，若同一前缀与 ID 出现多个标签，选中哪一个没有稳定顺序保证。

最重要的迁移边界是遍历深度：Go 用 `ast.Walk` 递归访问节点，Rust 当前只对每个传入 `Expression` 调一次 `Enter`，而 `Expression` 本身也没有子节点字段。因此只有候选节点本身带子查询前缀时才有效，不能等同于完整 AST visitor。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有容器在栈上的函数作用域内创建并随返回释放；计划与输入字段通过共享借用读取，返回值拥有自身数据。没有全局可变状态，因此单次调用之间不存在本文件引入的同步或缓存一致性问题。

成本主要来自克隆原字段列表和每个命中表达式，以及对每个相关 ID 线性扫描 SELECT 字段、递归扫描一元计划链；粗略可表示为字段克隆加 `相关 ID 数 × (计划链深度 + 当前字段数)`。深度极大的单子节点链使用递归查找，理论上存在栈深限制；正常计划树深度下不是独立资源生命周期风险。

## 与 Go 版本的对应关系

Rust 文件对应 [`subquery_plan_builder.go`](subquery_plan_builder.go)。符号大体一一对应：两个版本都有 `subqueryExprExtractor`、`Enter`/`Leave`、`findColumnNameByUniqueID` 和 `PlanBuilder.appendAuxiliaryFieldsForSubqueries`，都保留 USING/NATURAL JOIN 相关列的完整 schema 回退、穿透一元包装、忽略已有无别名列并追加辅助列的意图。

但实现并非等价移植。Go 按真实 AST 类型识别 `SubqueryExpr`、`ExistsSubqueryExpr`、`CompareSubqueryExpr` 和具有非空 `Sel` 的 `PatternInExpr`，Rust按名称前缀识别且没有 `IN` 的 `Sel != nil` 检查；Go 递归 `ast.Walk`，Rust不递归；Go 通过 `rewrite` 生成逻辑计划并调用 `ExtractCorrelatedCols4LogicalPlan`，Rust直接解析 `corr=` 文本；Go 从 `Schema/OutputNames` 与 `FullSchema/FullNames` 取列，Rust从浮点值 `labels` 映射的键取列；Go 新字段显式设 `Auxiliary: true`，Rust `SelectField` 类型没有这个字段；Go 可传播重写错误，Rust当前不会报错。

测试证据也要分层理解。[`casetest/windows/window_with_exist_subquery_test.go`](casetest/windows/window_with_exist_subquery_test.go) 的 `TestWindowSubqueryRewrite` 和 `TestWindowSubqueryOuterRef` 验证 Go 主链中窗口子查询、IN/ANY 与外层引用结果；对应 Rust 文件 [`casetest/windows/window_with_exist_subquery_test.rs`](casetest/windows/window_with_exist_subquery_test.rs) 验证 SQL 结果和后续解相关形状。[`lateral_join_test.go`](lateral_join_test.go) 与 [`lateral_join_test.rs`](lateral_join_test.rs) 覆盖 USING/NATURAL 合并列和 Apply 保留。但没有测试直接调用本文件的 Rust 符号，所以这些只能证明邻近规划行为，不能证明当前 Rust 主入口的标签解析、去重和错误边界。

## 扩展指南

若要把该实现接入 Rust 规划主链，首选修改点是 Rust 侧 `resolveWindowFunction` 的等价路径，在 SELECT 窗口字段、窗口规格、含窗口函数的 ORDER BY 三处调用主入口；接线前必须先决定是继续字符串协议，还是改用真实 Rust AST 与逻辑计划 schema。后者更接近 Go，也能消除 `corr=`/label 生产者不明和非递归遍历问题。

修改子查询种类时同步更新 `is_subquery_expression`，修改标签格式时同步更新 `lookup_column_label` 及上游标签生产者，修改 Join/Apply schema 行为时同步检查 `findColumnNameByUniqueID`，修改 `SelectField` 表示时应考虑补上等价的 auxiliary 标识。不要把测试嵌入本源文件；按仓库约定在同目录独立测试文件中新增直接单元测试，至少覆盖四种前缀、嵌套非顶层节点、多个/非法/负相关 ID、普通与 full-column 标签、一元链与多孩子边界、别名/限定名/大小写去重、重复 ID 和缺失列。

兼容性风险集中在标识符匹配与 Go 语义偏差；正确性风险是静默跳过导致外层列被投影裁剪；性能风险是若未来接入真实 AST 后重复重写每个子查询，可能重现 Go 文件中的 TODO 成本。接线或修正行为时应同步运行窗口子查询和 LATERAL USING/NATURAL 的独立测试，并增加本模块的直接单元测试，不能只依赖 SQL 端到端用例。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`explore "pkg/planner/core/subquery_plan_builder.rs SubqueryPlanBuilder build_handle_subquery optimize_subquery"` 返回目标文件与内部 blast radius；`node --file pkg/planner/core/subquery_plan_builder.rs --offset 1 --limit 260` 完整读取 192 行；`query/callers/callees` 核对内部边，并发现同名 Go/Rust 符号存在歧义。
- 源码与边界：完整阅读 [`subquery_plan_builder.rs`](subquery_plan_builder.rs)、[`task.rs`](task.rs) 的 `Expression`/`PlanKind`/`PlanNode`、[`logical_plan_builder.rs`](logical_plan_builder.rs) 的 `SelectField`、[`planbuilder.rs`](planbuilder.rs) 的 `Result`。
- crate 与接线：阅读 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)；用 `rg` 核实模块公开声明、Rust 侧无生产调用者，以及 `corr=`/标签协议没有其他明确生产者。
- Go 对照：完整阅读 [`subquery_plan_builder.go`](subquery_plan_builder.go)，并阅读 [`logical_plan_builder.go`](logical_plan_builder.go) 中 `resolveWindowFunction` 与 `resolveHavingAndOrderBy` 的调用上下文；Git 历史 `6d7c23bd9d` 证明 Go 文件由逻辑计划构建器抽取而来，Rust 文件则由 `fac562eba9` 初次加入。
- 测试：阅读 Go/Rust 窗口子查询对照测试 [`casetest/windows/window_with_exist_subquery_test.go`](casetest/windows/window_with_exist_subquery_test.go) 与 [`casetest/windows/window_with_exist_subquery_test.rs`](casetest/windows/window_with_exist_subquery_test.rs)，以及 USING/NATURAL/LATERAL 相关的 [`lateral_join_test.go`](lateral_join_test.go) 与 [`lateral_join_test.rs`](lateral_join_test.rs)。仓库中没有直接针对本 Rust 文件的独立测试。
- 本任务为纯文档分析，按计划不运行 Cargo 或代码测试；交付前运行任务规定的 11 章节结构命令，并对链接目标、唯一产物和 diff 范围做静态复核。
