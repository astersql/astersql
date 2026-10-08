# `pkg/planner/core/rule/rule_predicate_simplification.rs`

## 文件定位

本文件属于 `astersql-planner-core-rule` crate；crate 入口 `pkg/planner/core/rule/lib.rs` 以 `pub mod rule_predicate_simplification` 公开该模块。可执行部分从 `rule_init` 引入精简逻辑计划 IR（`Plan`、`Expr`、`Value`、`FieldType`）和 `LogicalRule` trait，用标准库的 `BTreeMap`/`BTreeSet` 保存按列归组的约束。

文件在迁移期同时保留两层内容：第 16—630 行是被整块注释的 Go 来源及设计说明，不参与 Rust 编译；第 631 行之后才是当前可执行 Rust 实现。当前实现是基于精简 IR 的局部规则及其单元测试表面，并不是完整 Go 谓词简化器的逐函数 Rust 实现。真实 Rust 优化器的规则枚举位于 `pkg/planner/core/optimizer_runtime.rs`，其 `LogicalRule::PredicateSimplification` 分支调用真实 `logicalop::LogicalPlanRef` 的 `PredicateSimplification()`；该路径依靠 `rule_init::init()` 安装到 `rule/util` 的钩子和 `logicalop` 内实现，不调用本文件的 `PredicateSimplification::optimize` 或 `simplify_cnf`。因此，本文件当前在完整应用中的作用更接近可执行的迁移模型与独立行为规格。

同目录未提供 `doc.go`；最近的包级边界证据是 `pkg/planner/core/rule/lib.rs`、`Cargo.toml` 和 `rule_init.rs`。

## 核心职责

可执行 Rust 部分完成三类工作：

1. `PredicateSimplification` 以 `LogicalRule` 形式提供稳定规则名 `predicate_simplification`，并在精简 `Plan` 树上自底向上改写每个节点的 `predicates`。
2. `simplify_cnf` 简化单个 CNF 谓词列表：删除恒真项、遇恒假立即把整个列表收敛为 `[false]`、递归化简布尔表达式、删除确定性重复项，并检测同一列的 `=`/`!=` 常量冲突。
3. `simplify_expr` 对表达式树执行局部布尔折叠：`NOT` 常量取反，`AND`/`OR` 短路并移除中性常量，`OR` 去除确定性重复分支，两个相同且非 `NULL` 的常量执行 `eq`/`ne` 自比较折叠。

该范围明显小于顶部注释保留的 Go 实现。当前可执行 Rust 未实现 Go 的 `IN` 与 `!=` 合并、`IS NULL`/`IN` 矛盾、基于其他标量谓词的 OR 分支裁剪、collation 兼容性检查、常量传播、`PushDownNot`、计划缓存跳过标记及 Join 专用入口。

## 主要符号

- `pub struct PredicateSimplification`：无字段规则对象；公开给同 crate/外部调用方构造。
- `impl LogicalRule for PredicateSimplification`：`name()` 返回固定注册名；`optimize(Plan)` 调用 `simplify_plan`，返回 `Ok((plan, false))`。`false` 对齐 Go `Optimize` 的 `planChanged := false` 合约，并不表示计划内容一定未变化。
- `fn simplify_plan(&mut Plan) -> bool`：后序递归孩子，再用 `std::mem::take` 取出本节点谓词并交给 `simplify_cnf`；内部返回真实结构变化标志，但 `optimize` 有意忽略它。
- `pub fn simplify_cnf(Vec<Expr>) -> Vec<Expr>`：模块唯一公开辅助函数，是独立单元测试的直接入口。
- `fn simplify_expr(Expr) -> Expr`：递归处理 `Expr::Scalar`；非标量表达式原样返回。
- `fn truth(&Expr) -> Option<bool>` 与 `fn bool_expr(bool) -> Expr`：只识别、构造 `Expr::Constant(Value::Bool)`；整数、`NULL` 等不被当作布尔常量。
- `fn column_constant(&Expr, &str) -> Option<(i64, Value)>`：只识别返回类型为 `FieldType::Bool`、函数名匹配、恰有两个参数且为“列/常量”任一顺序的二元表达式。
- `fn value_key(&Value) -> String`：用 `Debug` 表示作为 `!=` 排除集合键。

文件没有模块级常量、条件编译项或异步入口。顶部注释中的 `predicateType`、`FindPredicateType` 等名字不是当前 Rust 符号。

## 执行流程

精简 IR 的调用链为 `PredicateSimplification::optimize` → `simplify_plan` → `simplify_cnf` → `simplify_expr`。

`simplify_plan` 先递归每个子节点，确保子树先于父节点被处理；随后克隆旧谓词用于比较，用 `take` 避免额外克隆输入向量，再写回简化结果。该函数累计整棵子树是否变化，但上层 `optimize` 仍固定报告 `false`，以保持 Go 规则接口语义。

`simplify_cnf` 按输入顺序逐项处理。每项先递归简化：结果为真则丢弃，为假则立即返回唯一假谓词。对布尔型 `eq` 的列/常量形态，它检查该列是否已有不同等值或该值已在 `ne` 排除集合中；对 `ne` 则检查它是否排除了既有等值，并把值加入该列的排除集合。发现冲突即把整个 CNF 化为假。最后用 `Vec::contains` 删除完全相等的顶层重复表达式，保留首次出现顺序。

`simplify_expr` 先递归所有参数，再按函数名处理：`not` 仅在单参数且参数为布尔常量时折叠；`and` 遇假短路、移除真、空参数变真、单参数解包；`or` 遇真短路、移除假，并仅删除确定性表达式的重复项，随后处理空/单参数；`eq`/`ne` 仅在两个常量相同且该值不是 `Value::Null` 时折叠。其他函数或不满足形态的表达式以已递归简化的参数重建。

完整应用的真实路径不同：`pkg/planner/core/optimizer_runtime.rs::logical_optimize_in_place` 按 `LOGICAL_RULES`/flag 选择 `LogicalRule::PredicateSimplification`，随后调用 `plan.PredicateSimplification()`。`pkg/planner/core/operator/logicalop` 的 Selection、DataSource、Join 等实现再通过 `rule_util::ApplyPredicateSimplification*` 钩子简化真实表达式；这一调用链是对照本文件迁移状态时必须区分的边界。

## 数据与状态

输入/输出均按值所有权传递。`optimize` 获取 `Plan` 所有权，`simplify_plan` 原地遍历，最终返回改写后的计划；没有全局可变状态。`simplify_cnf` 的 `equalities: BTreeMap<i64, Value>` 记录每列唯一等值，`exclusions: BTreeMap<i64, BTreeSet<String>>` 记录每列已见的不等值键，生命周期仅限一次 CNF 调用。

表达式相等性依赖 `Expr`/`Value` 派生的 `PartialEq`。顶层去重是线性 `result.contains`，因此最坏可达到二次比较；列约束查找使用有序映射/集合。`Value::Float(f64)` 只有 `PartialEq` 而非全序，所以排除集合不直接保存 `Value`，而保存 `value_key` 生成的调试字符串。该选择可支持集合键，但其语义并非 SQL Datum/collation 等价关系，扩展类型时必须重新审视。

`Expr::deterministic()`（定义在 `rule_init.rs`）把 `rand`、`uuid`、`now` 视为非确定函数，并递归检查参数；`OR` 去重据此保留每一次非确定或副作用候选表达式。当前精简 IR 仅把这三个名称视为非确定，不能推断已覆盖真实表达式系统的全部易变函数。

## 依赖与调用关系

直接代码依赖只有 `crate::rule_init::{Expr, FieldType, LogicalRule, Plan, Value}` 与 `std::collections::{BTreeMap, BTreeSet}`。`pkg/planner/core/rule/Cargo.toml` 将该 crate 定义为 `astersql-planner-core-rule`，常规依赖包括 `astersql-planner-core-operator-logicalop`、`astersql-planner-core-rule-util` 和 `astersql-meta-model`；大量完整 planner/expression 依赖仅配置在 Windows target 下，但本文件可执行部分没有直接导入它们。

RustCodeGraph 将本文件标为被 `pkg/planner/core/rule/rule_predicate_simplification_test.rs` 和 `pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 使用，并定位到 `simplify_plan`、`simplify_cnf`、`simplify_expr`。源码级直接引用进一步表明：本文件的两个公开符号仅由同目录单元测试直接调用；优化器入口测试使用的是真实 `optimizer_runtime`/`logicalop` 路径，而不是精简 IR 规则对象。

下游内部调用关系为：`simplify_plan` 递归调用自身并调用 `simplify_cnf`；`simplify_cnf` 调用 `simplify_expr`、`truth`、`bool_expr`、`column_constant`、`value_key`；`simplify_expr` 递归调用自身并调用 `truth`、`bool_expr` 以及 `Expr::deterministic`。

## 错误处理与边界

`optimize` 的签名允许 `Result<_, String>`，但当前路径不构造错误，始终返回 `Ok`。无法识别或无法安全证明的表达式不会报错，而是保守保留：非 `Scalar`、参数个数不匹配、非布尔字段类型、非列/常量比较、未知函数以及含 `NULL` 的相同常量比较都不做相应折叠。

SQL 三值逻辑边界体现在 `NULL = NULL` 与 `NULL != NULL` 均不被折叠；同样，两个相同列的 `ne` 也不会被错误地折叠为真或假。单元测试 `sql_nullable_self_comparisons_are_not_boolean_folded` 固化了这两个保守条件。

冲突检查只覆盖同列常量 `eq`/`ne`，不推导范围关系、`IN`、cast、collation 或跨列等价类。`column_constant` 接受常量在左或右，但函数名仍原样匹配；对方向敏感的比较若未来复用该辅助函数，不能假设交换参数保持语义。`value_key` 使用调试字符串，存在表示稳定性与 SQL 等价性风险，不应直接外推到生产 Datum 比较。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、I/O 或外部资源。规则对象无状态，可被独立构造；每次调用的映射、集合和结果向量均为栈上所有权管理的局部值，调用结束自动释放。

计划遍历需要 `&mut Plan` 独占借用，因此单次改写不会并发修改同一计划。递归深度与计划树/表达式树深度线性相关；异常深的树可能消耗调用栈。`simplify_plan` 的旧谓词克隆用于变化检测，`column_constant` 克隆常量用于状态记录，可能放大大型文本值的内存开销，但不会跨调用保留资源。

真实优化器路径的 `OnceLock` 钩子注册位于 `rule_init.rs`/`rule/util`，不是本文件状态；不要把其进程级生命周期归因于这里的精简规则。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/rule/rule_predicate_simplification.go`。两者共同保留规则名，并且 `Optimize` 都允许改写计划却固定返回 `planChanged=false`。Rust 的后序遍历也概括了 Go 通过各逻辑算子 `PredicateSimplification()` 递归处理计划树的意图。

但当前 Rust 可执行范围只是 Go 语义的子集。Go 入口 `applyPredicateSimplificationHelper` 依次执行 `PushDownNot`、可选的普通/Join 常量传播、逻辑常量短路、`IN`/`!=` 合并、重复 OR 分支删除、与其他谓词矛盾的 OR 分支裁剪及 `DeleteTrueExprs`；它还检查参数标记导致的计划缓存过度优化、字符串 collation 兼容性和表达式构造错误。上述逻辑目前只存在于本文件顶部的注释副本，不能作为 Rust 已接线证据。

Rust `simplify_cnf` 额外以精简 IR 直接维护列等值/排除集合，这不是 Go 文件同名函数的逐行翻译，而是对部分矛盾检测的局部模型。真实 Rust SQL 行为由 `logicalop` 钩子路径承担；`pkg/planner/core/casetest/rule/rule_predicate_simplification_test.rs` 复用 Go fixture，覆盖计划、warning、plan-cache 形状以及矛盾折叠，但其通过完整 testkit/优化器运行，不能证明本文件的精简实现参与执行。

## 扩展指南

若扩展本文件的精简 IR 行为，应优先修改 `simplify_expr`（单表达式布尔规则）、`simplify_cnf`（跨 CNF 项约束）或 `simplify_plan`（遍历范围），并在独立的 `pkg/planner/core/rule/rule_predicate_simplification_test.rs` 增加回归；不要把测试内嵌到生产文件。新增易变函数时应同步 `rule_init.rs::Expr::deterministic`，确保 OR 去重不删除有副作用或非确定调用。

若目标是补齐 Go 功能，先判断行为应进入本精简 IR，还是进入真实 `logicalop`/`rule_util` 钩子路径。完整 SQL 语义应优先沿真实优化器链接入，并同步 `pkg/planner/core/casetest/rule/rule_predicate_simplification_test.rs` 与 Go fixture；不能只增加本文件单元测试后宣称完整应用已支持。

具体风险包括：`NULL` 三值逻辑、参数化计划缓存、字符串 charset/collation、可变/副作用表达式、常量位于比较式左侧时的方向性、浮点/文本值键的 SQL 等价性，以及 `optimize` 固定 false 的迭代合约。性能上应注意当前顶层去重的二次扫描、递归深度和大常量克隆。任何扩展都应保持 `LogicalRule::name()` 与优化器注册名一致，并验证真实与精简两条路径没有产生互相矛盾的规则语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 11,467 个文件；`files --filter pkg/planner/core/rule` 收录目标 Rust、Go 对照和独立测试；`node --file ... --offset 1/497` 读取了目标文件全貌；`query` 精确定位 `simplify_plan`（653）、`simplify_cnf`（666）、`simplify_expr`（706）。`callers`/`callees` 查询在 30 秒内未返回，因此调用边由直接源码引用补证。
- 生产源码：`pkg/planner/core/rule/rule_predicate_simplification.rs`、`rule_init.rs`、`lib.rs`、`Cargo.toml`；真实接线路径对照 `pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/operator/logicalop/base_logical_plan.rs`、`logical_selection.rs`、`logical_datasource.rs`、`logical_join.rs` 和 `pkg/planner/core/rule/util/misc.rs`。
- Go 对照：`pkg/planner/core/rule/rule_predicate_simplification.go`、`rule_init.go`，以及真实算子调用点 `pkg/planner/core/operator/logicalop/logical_selection.go`、`logical_datasource.go`、`logical_join.go`。
- 测试证据：`pkg/planner/core/rule/rule_predicate_simplification_test.rs` 验证固定 changed 标志、非确定 OR 重复保留和 `NULL` 自比较边界；`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 验证真实规则枚举顺序/实际计划改写；`pkg/planner/core/casetest/rule/rule_predicate_simplification_test.rs` 与同名 `.go` 测试提供完整 SQL fixture 对照。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工检查文档只陈述上述源码、调用搜索与测试能够支持的事实。
