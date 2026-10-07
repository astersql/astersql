# `pkg/planner/core/operator/logicalop/logical_plans_misc.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-logicalop` crate，是逻辑算子层的共享辅助函数集合。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 以私有模块 `logical_plans_misc` 装配它，再通过 `pub use logical_plans_misc::*` 将全部函数重新导出，因此同 crate 的逻辑算子可以直接调用，物理算子等下游 crate 则可通过 `logicalop::...` 调用。

它不定义新的计划节点，而是操作 `LogicalPlan` trait 对象、`LogicalPlanRef` 子树、`Expression` 和聚合描述符。当前生产接线中，`LogicalUnionAll::PredicatePushDown` 调用 `AddSelection`；多个物理计划生成路径调用 `GetHasTiFlash`。其余函数在当前 Rust 搜索结果中没有跨文件生产调用，或只被本文件内部递归调用，必须视为已导出但尚未充分接线的迁移接口，而不是完整主链能力。

crate 边界由 `pkg/planner/core/operator/logicalop/Cargo.toml` 确认：本文件直接用到本 crate 的计划抽象，以及 `aggregation`、`expression` 和 `rule_util` 三个依赖；manifest 的 `package.metadata.porting.go-package` 指向同路径 Go 包 `pkg/planner/core/operator/logicalop`。

## 核心职责

本文件包含八个公开函数，职责可分为五组：

1. `AddSelection`：把谓词简化后的残余条件物化为 `LogicalSelection`，或在条件恒假/NULL 时替换为零行 `LogicalTableDual`，最后写回父节点指定的 child 槽位。
2. `pushDownTopNForBaseLogicalPlan`：把通用计划的 TopN 下推委托给 `BaseLogicalPlan::PushDownTopN`。
3. `pruneByItems`：按表达式 `HashCode()` 对排序表达式做稳定去重。
4. `GetHasTiFlash`：读取计划基类已经准备好的 TiFlash 可能性缓存；它不递归计算，也不触发属性准备。
5. 树与聚合辅助：`RecursiveMemoryUsage` 递归累计计划节点 trait object 的浅层大小，`FlattenTreePlan` 前序收集节点 ID，`GetPlanIDsHash` 对该 ID 序列计算 64 位哈希，`GetDupAgnosticAggCols` 判断聚合是否完全由重复无关聚合函数组成并抽取参数列。

这些职责来自 `logical_plans_misc.rs:25-157` 的实际分支。文件注释所称“共享辅助逻辑”是准确定位，但共享并不等于每项均已接入生产路径。

## 主要符号

- `AddSelection(parent, child_index, child, conditions) -> Result<()>`（第 25 行）：唯一返回 `Result` 的函数。它验证 child 下标、要求父计划存在上下文，并在必要时构造替代子树。
- `pushDownTopNForBaseLogicalPlan(plan, top_n) -> Option<LogicalPlanRef>`（第 79 行）：薄委托，直接调用 `plan.base_mut().PushDownTopN(top_n)`。当前 Rust 全仓精确搜索只找到定义，没有生产调用。
- `pruneByItems(items) -> Vec<Expression>`（第 88 行）：使用 `HashSet<Vec<u8>>` 保存表达式哈希；`Iterator::filter` 保证保留首次出现项的原顺序。它接收裸 `Expression`，而不是 Go 的 `ByItems`。
- `GetHasTiFlash(plan) -> bool`（第 97 行）：`None` 返回 `false`，`Some` 时读取 `plan.base().PreparePossiblePropertiesValue()`。
- `RecursiveMemoryUsage(plan) -> i64`（第 102 行）：自身 `size_of_val(plan)` 加全部子树递归和。对 `&dyn LogicalPlan` 而言，该值是 trait object 所指具体值的浅层大小，不包含节点内部堆分配容量，因此只能称估算。
- `FlattenTreePlan(plan, output)`（第 112 行）：以前序顺序写入当前 `ID()`，随后按 `Children()` 顺序递归。
- `GetPlanIDsHash(plan) -> u64`（第 120 行）：先扁平化，再以常量 `1469598103934665603` 为初值、`1099511628211` 为乘数，用 `wrapping_mul` 折叠每个 `i32` ID 转成的 `u64`。这是源码声明的 FNV-1a 风格实现；不要与 `BaseLogicalPlan::GetPlanIDsHash` 混淆，后者只读取 `plan_ids_hash` 缓存字段。
- `GetDupAgnosticAggCols(plan, old_agg_cols) -> (bool, Vec<Column>)`（第 130 行）：非 `LogicalAggregation` 返回 `(false, empty)`；聚合节点复用传入向量的分配并清空内容。所有描述符都必须是 `DISTINCT`，或名称为 `first_row`、`max`、`min`、`approx_count_distinct`，否则返回 `(true, empty)`；全部合格时抽取所有参数表达式中的列。

本文件没有模块级常量、类型、trait、`impl` 或条件编译项；八个函数均因 crate 根的通配 re-export 成为公开 API。

## 执行流程

`AddSelection` 的主流程如下：

1. 先比较 `child_index` 与 `parent.Children().len()`；越界直接返回 `PlannerError`，不修改计划树。
2. 初始条件为空时，直接把 `child` 写入该槽位。
3. 从父节点取得并克隆 `SCtx()`，缺失上下文时报错；随后调用 `rule_util::ApplyPredicateSimplification(context, conditions, true, None)`。
4. 简化后条件为空，或 child 已是 `RowCount == 0` 的 `LogicalTableDual` 时，直接挂回原 child。
5. `Conds2TableDual(&conditions)` 为真时，新建零行 `LogicalTableDual`；否则新建持有条件的 `LogicalSelection` 并把原 child 设为其唯一子节点。
6. 两种新节点都以父节点上下文和 `QueryBlockOffset()` 初始化，并复制原 child 的 schema 与 output names，最后替换父节点槽位。

真实上游 `LogicalUnionAll::PredicatePushDown`（`logical_union_all.rs:38-52`）对每个分支克隆谓词，调用 `PredicatePushDownPlan`，用占位 `LogicalTableDual` 暂时取出 child，再交给 `AddSelection` 物化未消费条件。这个 `mem::replace` 顺序也说明 `AddSelection` 必须可靠地回填指定槽位。

`GetHasTiFlash` 的流程只有一次可选值判断和一次缓存读取。`logical_plans_misc_test.rs:58-68` 证明：仅准备 child 不会让 parent 返回真；必须在 parent 自身调用 `PreparePossibleProperties` 后才能读到真。这与物理计划选择路径只消费“已经准备的属性”相符。

树辅助函数均为深度优先前序：`GetPlanIDsHash` 先通过 `FlattenTreePlan` 生成根在前、子树按 child 顺序排列的 ID 向量，再顺序折叠哈希，因此节点 ID 或 child 顺序变化都会改变指纹。

`GetDupAgnosticAggCols` 先做动态类型检查，再逐个聚合描述符验证资格；任何一个普通非 DISTINCT 聚合都会清空先前抽取结果并立即返回。只有所有描述符均合格，才返回累积列。

## 数据与状态

本文件本身没有全局状态。状态变化仅发生在传入对象：

- `AddSelection` 修改 `parent.Children_mut()[child_index]`，并可能把原 child 移入新的 Selection；schema 使用 `Clone()`，输出名使用 `Shallow()`，说明计划结构获得独立容器，但输出名遵循既有浅复制语义。
- `pushDownTopNForBaseLogicalPlan` 取得 `base_mut()`，具体变更由基类实现决定。
- `pruneByItems` 消耗输入向量并返回新向量；`seen` 只在调用栈内存在。
- `GetHasTiFlash` 只读基类缓存，不修改或重新计算属性。
- `RecursiveMemoryUsage`、`FlattenTreePlan` 和 `GetPlanIDsHash` 只读计划树；其中 `FlattenTreePlan` 追加到调用者提供的向量，不会先清空已有内容。
- `GetDupAgnosticAggCols` 按值接收旧向量，先 `clear()` 以复用容量。失败分支返回空向量，但 `bool == true` 仍表示“输入是聚合节点”，不是“聚合重复无关”。调用者必须同时解释两个返回值。

关键不变量包括：计划子节点顺序决定树指纹；Selection/TableDual 替换时保留 child 的输出 schema/name；重复无关聚合要求每个聚合函数均满足白名单或 DISTINCT 条件；排序去重以 `HashCode()` 字节完全相等为判据。

## 依赖与调用关系

直接下游依赖如下：

- 计划抽象：`LogicalPlan::{Children, Children_mut, SCtx, QueryBlockOffset, Schema, OutputNames, ID, base, base_mut}` 及 `LogicalPlanRef`。
- 节点构造：`LogicalSelection::Init`、`LogicalTableDual::Init`、`SetSchema`、`SetOutputNames`、`SetChildren`。
- 规则层：`rule_util::ApplyPredicateSimplification`；条件判定由 crate 内 `Conds2TableDual` 提供。
- 表达式层：`Expression::HashCode` 与 `expression::ExtractColumns`。
- 聚合层：`aggregation::ast` 的函数名常量和 `LogicalAggregation::AggFuncs` 描述符。
- 标准库：`HashSet`、`size_of_val` 和整数 `wrapping_mul`。

RustCodeGraph 的 `node --file` 结果确认文件共有 11 个索引符号，并显示它被 8 个文件使用；精确全仓搜索把可确认的生产上游缩小为：

- `logical_union_all.rs:49` 调用 `AddSelection`。
- `physical_limit.rs:334`、`physical_window.rs:754`、`physical_selection.rs:343,356`、`physical_projection.rs:653`、`base_physical_agg.rs:1041,1130,1187` 与 `base_physical_plan.rs:9946,10081,10175,10251,11157` 调用 `GetHasTiFlash`，用于 TiFlash/MPP 相关物理计划选择判断。
- `logical_plans_misc_test.rs` 调用 `GetHasTiFlash` 和 `GetDupAgnosticAggCols`。
- `FlattenTreePlan` 由自身递归并被 `GetPlanIDsHash` 调用；`RecursiveMemoryUsage` 只自递归。

没有搜索到 Rust 生产侧对 `pushDownTopNForBaseLogicalPlan`、`pruneByItems`、`RecursiveMemoryUsage`、自由函数 `GetPlanIDsHash` 或 `GetDupAgnosticAggCols` 的跨文件调用。因此扩展时不能仅因函数公开就假定优化器主链会执行它们。

## 错误处理与边界

显式错误只有 `AddSelection` 的两个边界：child 下标越界返回 `PlannerError("child index ... out of bounds")`；父节点没有计划上下文返回 `PlannerError("logical parent has no plan context")`。错误发生在写回前，因此原 parent 子槽位不由本函数改动；但调用者若已像 `LogicalUnionAll` 那样先用占位节点取走 child，仍需依赖上层错误路径丢弃整次变换，不能把局部 parent 当作可继续使用的成功结果。

其余函数不返回错误：

- `GetHasTiFlash(None)` 明确为 `false`；未准备属性也会读到基类当前缓存值，而非推导值。
- 递归树函数没有环检测或深度限制，依赖计划树无环且递归深度可承受线程栈。
- `RecursiveMemoryUsage` 可能低估堆拥有数据；总和使用普通 `i64` 加法，没有溢出保护。
- `pruneByItems` 假设哈希足以代表表达式等价；哈希碰撞会误删后项，也没有保留 Go 侧 `ByItems` 的排序方向等包装信息。
- `GetDupAgnosticAggCols` 不对空聚合函数列表作特殊拒绝，因此空列表会返回 `(true, empty)`；参数中的同一列也不会去重。

## 并发与资源生命周期

文件没有锁、原子变量、通道、异步任务、事务或外部 I/O。所有临时集合和向量都受函数调用栈或返回值所有权管理，计划节点由 `LogicalPlanRef` 的所有权移动组织。

这些函数没有声明线程安全保证；是否可跨线程使用取决于 `LogicalPlan`、表达式和计划上下文的具体实现。`AddSelection` 需要独占 `&mut dyn LogicalPlan`，编译期阻止同一 parent 的并发可变访问；其 child 和 conditions 按值移入。只读树函数接收共享引用，但递归期间假设计划树不会通过其他内部可变性改变 child 列表。

资源复杂度：`pruneByItems` 的时间和附加空间均近似 O(n)；`FlattenTreePlan`/`GetPlanIDsHash`/`RecursiveMemoryUsage` 对节点数为 O(N)，递归栈为 O(树高)，其中 `GetPlanIDsHash` 还分配 O(N) 的 ID 向量；`GetDupAgnosticAggCols` 与聚合函数数和参数表达式规模线性相关，并复用传入向量容量。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/operator/logicalop/logical_plans_misc.go`，但并非所有 Rust 函数都与其中实现完全等价：

- `AddSelection` 保留“空条件直挂、谓词简化、空 TableDual 短路、恒假/NULL 转零行 dual、否则包 Selection”的核心流程。Rust 额外显式检查 child 下标和缺失上下文并返回错误；Go 签名不返回错误。Rust 直接调用 `Conds2TableDual(&conditions)` 后自行复制 schema/output names，Go 的 `Conds2TableDual(child, conditions)` 返回完整 dual。
- Go `pushDownTopNForBaseLogicalPlan` 会先逐子节点调用 `PushDownTopN(nil)`，再将可选 `LogicalTopN` 附到当前计划；Rust 只委托 `base_mut().PushDownTopN(top_n)`。是否最终等价取决于基类实现，本文没有找到该自由函数的 Rust 调用证据，不能宣称完整对齐。
- Go `pruneByItems` 不只是去重：它操作 `[]*util.ByItems`，保留排序项元数据，剔除确定性常量和 NULL 类型表达式，并返回仍需父层保留的列；运行时常量和非确定性表达式还有专门分支。Rust 版本仅对 `Vec<Expression>` 按哈希去重，也没有生产调用，属于明显的部分迁移。
- Go `GetHasTiFlash` 会做 nil 和基类类型断言后读 `hasTiFlash`；Rust trait 直接保证可访问基类，并通过 `Option` 表达 nil，语义同为只读已准备缓存。Rust 测试专门验证了“不递归读取 child”。
- Go `GetDupAgnosticAggCols` 的白名单、全体资格要求、旧缓冲复用和非聚合返回值与 Rust 基本一致；Rust 还接受 `approx_count_distinct`，与当前 Go 条件一致。Rust 独立测试覆盖 DISTINCT SUM + MAX、普通 SUM 和非聚合三类。
- Go 的计划 ID 哈希当前在 `base_logical_plan.go` 以缓存字段 `planIDsHash` 暴露，相关 cascades 测试验证缓存值；本 Rust 文件的自由函数则现场遍历 ID 并计算 FNV-1a 风格哈希。Rust `base_logical_plan.rs` 同时也有只读缓存的同名方法，因此两者用途不可混用。
- `RecursiveMemoryUsage`、`FlattenTreePlan` 和现场计算的自由函数 `GetPlanIDsHash` 在同路径 Go 文件中没有对应实现，属于 Rust 侧新增辅助；当前也未见生产接线。

## 扩展指南

修改 `AddSelection` 时，应保持三类短路顺序、schema/output names 传播以及 `LogicalUnionAll::PredicatePushDown` 的取出/回填契约。新增可失败分支要考虑调用者已临时放入占位 child 的状态，并在独立的 `logical_plans_misc_test.rs` 增加越界、缺上下文、谓词简化为空、恒假 dual、已有空 dual 和正常 Selection 用例；不要把测试内嵌进源文件。

若要让 `pruneByItems` 对齐 Go，应先把 API 提升到能表达 `ByItems`、计划表达式上下文和返回依赖列，再分别覆盖重复项、普通常量、运行时常量、非确定性函数、NULL 类型和列引用。仅增加哈希规则无法补齐 Go 语义，且修改后应同步检查 Sort、TopN 和 Aggregation 的列裁剪接线。

修改 TiFlash 属性时，应在属性准备阶段写缓存，而不是让 `GetHasTiFlash` 隐式递归；同步更新 `logical_plans_misc_test.rs` 的 parent/child 缓存边界，并审查所有物理算子调用点对真假值的决策。

修改树指纹时，应明确它与 `BaseLogicalPlan::plan_ids_hash` 缓存的关系，增加独立测试覆盖 child 顺序、负 ID 转换、空子树和溢出回绕；若用于缓存键，还需评估哈希碰撞与跨版本稳定性。若计划树可能很深，考虑把 `FlattenTreePlan` 和 `RecursiveMemoryUsage` 改为显式栈遍历。

扩展重复无关聚合白名单时，必须先核对 Go `GetDupAgnosticAggCols` 与聚合函数对重复行的数学性质，再同步独立测试；不能仅根据函数名推断。所有 Rust 生产代码修改完成后应按仓库规则运行 `cargo fmt --all`，但本次纯文档任务不修改 Rust，也不运行 Cargo。

## 验证依据

已读取并交叉核验以下直接证据：

- 目标源码 `pkg/planner/core/operator/logicalop/logical_plans_misc.rs:1-157`：八个函数的完整实现。
- crate 边界 `pkg/planner/core/operator/logicalop/Cargo.toml` 与模块入口 `pkg/planner/core/operator/logicalop/lib.rs`：依赖、Go 包映射、模块装配与公开 re-export。
- Rust 独立测试 `pkg/planner/core/operator/logicalop/logical_plans_misc_test.rs:1-106`：TiFlash 缓存读取和重复无关聚合边界；crate 根以 `#[cfg(test)] mod logical_plans_misc_test` 装配测试。
- Rust 真实入口 `pkg/planner/core/operator/logicalop/logical_union_all.rs:38-52`，以及 `physicalop` 下对 `GetHasTiFlash` 的调用点。
- Rust 基类 `pkg/planner/core/operator/logicalop/base_logical_plan.rs:576-588`：计划 ID 哈希缓存的 setter/getter，证明它与本文件现场计算函数并存。
- Go 对照 `pkg/planner/core/operator/logicalop/logical_plans_misc.go:85-168,299-331`、`base_logical_plan.go:404-417`，以及 Go 的 Sort、TopN、Aggregation、CTE 调用点。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点和 1,848,419 条边；`files --filter` 定位目标为含 11 个符号的已索引 Rust 文件；`node --file ... --offset 1 --limit 400` 返回完整源码并报告 8 个使用文件。带 `--file` 的精确 callers/callees 查询未返回可用文本，因此调用边又以 `rg` 精确符号搜索核实，未采用模糊 `explore` 的同名噪声。

结构验证应使用任务指定命令，要求文件存在且恰好命中上述十一个固定二级标题。本说明仅描述静态代码事实，未运行 Cargo 或运行时测试；未验证的重点是未接线辅助函数在未来调用场景中的行为，以及 `pushDownTopNForBaseLogicalPlan` 委托与 Go 展开实现是否在所有计划类型上等价。
