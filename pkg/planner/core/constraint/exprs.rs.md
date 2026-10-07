# [`pkg/planner/core/constraint/exprs.rs`](exprs.rs)

## 文件定位

本文件属于 Rust crate `astersql-planner-core-constraint`，crate 根由同目录的 `Cargo.toml` 将 `lib.rs` 指定为入口。`lib.rs` 以私有模块 `mod exprs` 装入本文件，再通过 `pub use exprs::*` 对外暴露两个公开函数 `DeleteTrueExprs` 和 `DeleteTrueExprsBySchema`。它处在规划器的约束/谓词清理层：接收一组已经构造好的表达式，在谓词继续简化或下推前删除能够安全证明为真的项。

当前 Rust 实现是迁移期隔离 crate，而不是完整表达式子系统的直接消费者。`lib.rs` 内定义了精简的 `expression` 兼容模块，并从 parser、session context 和 datum crate 再导出所需类型。全仓 Rust 引用显示该 crate 在 `pkg/planner/core/rule/Cargo.toml` 中仅作为 Windows 条件依赖出现；`pkg/planner/core/rule/rule_predicate_simplification.rs` 目前只有注释形式的 Go 对照调用。RustCodeGraph 的文件关系只发现独立测试 `migration_aster_unit_test.rs` 使用本文件，因而不能把 Go 侧已经接入的规划主链误写成 Rust 侧当前已接线。

## 核心职责

文件提供两种“恒真谓词删除”，共同遵守保守优化原则：只有证明充分时才删除，否则保留原表达式。

- `DeleteTrueExprs` 处理常量条件。只有表达式确实是常量、不会因计划缓存而在执行期改变，并且 `Datum::ToBool` 成功返回恰好 `1` 时才删除。非零但不等于 `1` 的结果也不会由这里自行推断为真。
- `DeleteTrueExprsBySchema` 处理由列约束推出的恒真形式。它仅识别精确的 `NOT(ISNULL(column))` 结构；当该列能从给定 `Schema` 按 `UniqueID` 找回，并且 schema 中列类型带 MySQL `NOT NULL` 标志时删除。
- `isNullWithNotNullColumn` 封装第二种判断的内层结构检查，避免公开入口将“不像目标形状”和“列约束不足”混为可删除条件。

两个公开函数均以过滤方式生成新的 `Vec<Expression>`，保持所有未删除表达式的相对顺序。它们不执行通用常量传播、布尔代数化简或表达式重写。

## 主要符号

- `pub fn DeleteTrueExprs(build_ctx: &expression::BuildContext, stmt_ctx: &stmtctx::StatementContext, conds: Vec<expression::Expression>) -> Vec<expression::Expression>`：消费条件向量并移除可安全求值为 `true` 的常量。空向量直接原样返回。公开 API 名称保留 Go 风格，以便迁移对照；crate 根允许 `non_snake_case`。
- `pub fn DeleteTrueExprsBySchema(ctx: &expression::EvalContext, schema: &expression::Schema, conds: Vec<expression::Expression>) -> Vec<expression::Expression>`：消费条件向量，删除由 schema 的非空约束证明恒真的 `NOT(ISNULL(column))`。
- `fn isNullWithNotNullColumn(ctx: &expression::EvalContext, schema: &expression::Schema, expr: &expression::Expression) -> bool`：文件私有辅助函数。依次验证 `IS NULL` 函数名、单参数形状、参数为列、列存在于 schema、找回列的类型带 `NOT NULL` 标志。
- 依赖的兼容符号定义于 `lib.rs`：`Expression::{Constant, Column, ScalarFunction, Other}`、`MaybeOverOptimized4PlanCache`、`Schema::RetrieveColumn`、`ScalarFunction::GetArgs` 与各 `as_*` 类型判别方法。

本文件没有模块级常量、类型、trait、`impl` 或条件编译项；条件编译仅出现在 crate 根，用于挂载测试模块。

## 执行流程

`DeleteTrueExprs` 的流程如下：

1. 若 `conds` 为空，立即返回原向量，避免无意义的迭代与分配。
2. 对每个条件调用 `as_constant`；不是常量就保留。
3. 对常量调用 `MaybeOverOptimized4PlanCache(build_ctx, constant)`。在当前兼容实现中，仅当启用计划缓存且常量标记为 `mutable` 时返回真；这种条件必须保留。
4. 对剩余常量执行 `constant.Value.ToBool(stmt_ctx.TypeCtx())`。只有结果严格匹配 `Ok(1)` 时，过滤闭包返回“不保留”；`Ok(0)`、`Ok` 的其它值、`NULL` 对应结果和 `Err` 均保留。
5. `collect` 按输入遍历顺序构造结果向量。

`DeleteTrueExprsBySchema` 的流程如下：

1. 对每个条件尝试取 `ScalarFunction`；其它表达式直接保留。
2. 要求外层函数名等于 `ast::UnaryNot` 且恰有一个参数；不满足则保留。
3. 将唯一参数交给 `isNullWithNotNullColumn`。
4. 辅助函数要求内层同样是标量函数、函数名为 `ast::IsNull` 且恰有一个参数，再要求该参数为列。
5. 使用 `Schema::RetrieveColumn` 按列 `UniqueID` 查找 schema 中的权威列；未知列保留条件。
6. 读取找回列的 `FieldType` 标志，仅当 `mysql::HasNotNullFlag(...)` 为真时，外层条件被删除。

## 数据与状态

函数拥有并消费输入 `Vec<expression::Expression>`，通过 `into_iter` 移动每个元素，再把保留项收集到新向量；没有就地修改表达式对象。过滤只借用 `BuildContext`、`StatementContext`、`EvalContext` 和 `Schema`，不会修改这些上下文。

影响判定的状态只有三类：常量中的 `Datum` 值、计划缓存相关的 `BuildContext::use_plan_cache` 与 `Constant::mutable`，以及 schema 列的 `UniqueID` 和字段类型 flags。传入表达式列自身携带的 `RetType` 不是最终非空依据；代码先按 `UniqueID` 找回 schema 列，再读取找回列的类型标志。这保证判定使用当前 schema 元数据，而非表达式上可能不完整的列副本。

未被删除项保持原有相对顺序，独立 Rust 测试 `retains_non_matching_function_shapes_in_original_order` 明确验证了这一属性。输出容量和分配策略由标准库 `collect` 决定，本文件没有缓存或跨调用状态。

## 依赖与调用关系

直接下游依赖均经 crate 根兼容模块进入：

- `expression`：表达式变体判别、计划缓存保护、schema 列查找、标量函数参数和求值上下文。
- `stmtctx::StatementContext`：通过 `TypeCtx()` 为 `Datum::ToBool` 提供语句类型上下文。
- `ast::{UnaryNot, IsNull}`：按规范化的小写函数名识别目标表达式形状。
- `mysql::HasNotNullFlag`：检查找回列的字段类型标志。
- `types_crate::Datum`：由 `lib.rs` 的常量表达式持有，提供布尔转换；`Cargo.toml` 将它映射到 `pkg/types/internal/datum`。

Cargo 边界由 `pkg/planner/core/constraint/Cargo.toml` 给出：直接依赖 parser AST、parser MySQL、session statement context 和 datum 四个本地 crate。根 workspace 在 `Cargo.toml` 中以 `facade_planner_core_constraint` 登记该 crate。

Go 侧的真实上游调用包括：`logical_selection.go` 的 `LogicalSelection.PredicatePushDown` 在向子节点下推前调用 `DeleteTrueExprs`；`logical_join.go` 在分别向左右子节点下推条件前，以各自 child schema 调用 `DeleteTrueExprsBySchema`；`rule_predicate_simplification.go` 在谓词简化流水线末尾调用 `DeleteTrueExprs`。Rust 侧目前未找到等价的生产调用边，只有 crate 公开导出、条件依赖、注释对照和独立迁移测试，因此其生产主链接线仍未验证。

## 错误处理与边界

本文件没有 `Result` 返回值，也不向调用者传播求值错误。`Datum::ToBool` 的任何错误都被转化为“无法证明为真”，相应条件原样保留。这是安全退化，而不是吞掉后继续删除。`NULL` 同样不会匹配 `Ok(1)`，因此保留。

计划缓存是另一条显式安全边界：可变常量在启用缓存时即使当前值为 `1` 也不删除，避免复用计划时参数值变化导致语义错误。非标量函数、函数名不匹配、参数数量不是一、内层不是列、列不在 schema、列可空等情况全部返回“保留”。函数不会验证 schema 指针有效性，因为 Rust API 使用非空引用；也没有数组越界风险，因为每次索引 `[0]` 前都先检查参数数恰为一。

当前兼容表达式模型较窄，`Expression::Other` 代表尚未迁移的其它表达式形态。这些形态一律保留。扩展模型时不能把未知变体默认当作可删除条件。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有输入上下文均以共享不可变引用传入；表达式向量由函数独占消费，返回的新向量把保留表达式的所有权交还调用者。因此函数本身没有共享可变状态和显式并发协议。

资源生命周期限定在一次调用内：过滤闭包只在迭代期间临时借用上下文及当前表达式，`collect` 完成后这些借用结束；被删除的表达式在迭代中被丢弃，保留的表达式移动进返回向量。是否能在线程间传递取决于这些表达式和上下文类型自身的 `Send`/`Sync` 实现，本文件没有声明或验证该能力。

## 与 Go 版本的对应关系

同路径 `exprs.go` 是直接语义基准。Rust `DeleteTrueExprs` 对应 Go 的 `slices.DeleteFunc`：Go 删除条件为“常量、没有计划缓存过优化风险、`ToBool` 无错误且结果为 1”；Rust 的过滤谓词取反后以 `matches!(..., Ok(1))` 表达完全相同的删除条件。Rust 额外对空向量提前返回，与 Go 对空 slice 的结果一致。

Rust `DeleteTrueExprsBySchema` 和 `isNullWithNotNullColumn` 逐层对应 Go 的类型断言与参数长度检查：外层 `UnaryNot`、单一参数、内层 `IsNull`、单一列参数、schema 成功找回列、列类型带 `NOT NULL`。Rust 的 `Option` 分支代替 Go 的 `(value, ok)` 与 `nil` 判断，保留逻辑相同。

需要注意的迁移差异是类型环境而非本文件算法：Go 使用完整的 `pkg/expression` 接口对象和 planner schema；Rust 当前使用 `lib.rs` 内的精简 enum/struct 兼容模型。Go 已有三个生产调用点，Rust 的同类调用尚未实际启用；`rule_predicate_simplification.rs` 中仅保留了注释调用。因此可以确认单文件算法对齐，但不能据此宣称完整 Rust 优化器行为已经对齐。

## 扩展指南

若新增可删除的恒真模式，应优先在相应公开入口旁增加小型私有判定函数，保持“识别失败即保留”的保守默认值。修改常量规则时必须同步考虑 `MaybeOverOptimized4PlanCache` 和 `StatementContext::TypeCtx`，不能绕过计划缓存可变性或把转换错误当真值。修改 schema 规则时必须继续使用 schema 找回后的列元数据，不能仅相信表达式列携带的 flags；新增函数形状也必须验证精确参数数量后再索引。

测试应继续放在独立文件 `pkg/planner/core/constraint/migration_aster_unit_test.rs`，不要嵌入生产源文件。至少同步覆盖：可删除案例、每个结构检查失败分支、NULL 与求值错误、计划缓存可变常量、未知/可空/非空列、多个条件的顺序保持。若把 crate 接入真实 Rust planner，还应在实际调用 crate 增加规划器级测试，验证 Selection、Join 左右 child schema 和谓词简化的调用时机，并与 Go 的 `pkg/planner/core/casetest/rule/rule_predicate_simplification_test.go` 及相关 plan testdata 对照。

兼容风险主要是错误删除导致查询结果变化；性能风险则来自过度保守导致冗余谓词继续参与估算与下推。新增优化必须先证明语义恒真，再考虑减少分配或扩大识别范围。若将精简 `expression` 模型替换为完整表达式 crate，还需核对 `Expression` 动态类型、函数名规范化、schema 列身份和 `Datum::ToBool` 错误语义没有漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/core/constraint` 列出 `exprs.rs`、`exprs.go`、`lib.rs` 和 `migration_aster_unit_test.rs`；`node --file pkg/planner/core/constraint/exprs.rs` 读取 1–103 行并报告该文件由迁移测试使用；`query` 分别确认三个 Rust 函数及其 Go 同名对应；`callers`/`callees` 未返回额外 Rust 函数边。
- 生产源码：[`exprs.rs`](exprs.rs)（两个公开入口和一个私有辅助函数）；[`lib.rs`](lib.rs)（兼容表达式模型、模块装配与公开再导出）。
- crate 配置：[`Cargo.toml`](Cargo.toml)（crate 名、入口、四项直接依赖和 Go package 元数据）；根 [`Cargo.toml`](../../../../Cargo.toml)（workspace facade 登记）；[`pkg/planner/core/rule/Cargo.toml`](../rule/Cargo.toml)（Windows 条件依赖）。
- Go 对照与调用点：[`exprs.go`](exprs.go)；[`logical_selection.go`](../operator/logicalop/logical_selection.go)；[`logical_join.go`](../operator/logicalop/logical_join.go)；[`rule_predicate_simplification.go`](../rule/rule_predicate_simplification.go)。
- 独立 Rust 测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `deletes_only_safely_true_constants`、`deletes_not_isnull_only_for_schema_not_null_column` 和 `retains_non_matching_function_shapes_in_original_order`。
- Go 规划器测试入口：[`rule_predicate_simplification_test.go`](../casetest/rule/rule_predicate_simplification_test.go) 的 `TestPredicateSimplification` 运行 SQL、检查 plan、warnings 和 plan-cache 复用状态；它提供集成层背景，但并非对这三个 helper 的逐分支白盒测试。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付前使用任务规定的命令验证文档存在且固定二级标题恰为 11 个，并另行检查 Markdown 路径引用与 diff 格式。
