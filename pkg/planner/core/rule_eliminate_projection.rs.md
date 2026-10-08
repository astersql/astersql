# `pkg/planner/core/rule_eliminate_projection.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 由 [`pkg/planner/core/Cargo.toml`](Cargo.toml) 声明，并由 [`pkg/planner/core/lib.rs`](lib.rs) 通过公开模块 `pub mod rule_eliminate_projection` 暴露。它在通用的 [`task::PlanNode`](task.rs) 树上提供一组投影消除辅助 API，包括逻辑树重写与物理树的严格透传投影删除。

这里需要区分“公开可调用的轻量移植接口”和“完整应用的实际优化主链”。RustCodeGraph 的调用轨迹显示，本文件的 `ProjectionEliminator` 当前由独立测试导入，`eliminatePhysicalProjection` 也只在本文件递归引用和规则测试中直接使用；完整逻辑/物理优化流程目前由 [`optimizer_runtime.rs`](optimizer_runtime.rs) 中的 `eliminate_identity_projections`、`eliminate_redundant_physical_projections` 等更完整实现接线。因此不能仅凭本文件公开导出，就认定其直接驱动所有 SQL 请求的生产优化。

## 核心职责

1. `canProjectionBeEliminatedLoose` 判断逻辑投影是否只有一个孩子，且每个表达式都是列引用；它允许列重排，不要求输出 schema 与孩子相同。
2. `canProjectionBeEliminatedStrict` 在宽松条件上检查逐位置透传，供物理投影删除使用；空 schema 是特殊可消除情形。
3. `doPhysicalProjectionElimination` / `eliminatePhysicalProjection` 自底向上删除严格透传的物理 Projection，同时保留返回孩子节点中已有的属性，例如 `keep_order`。
4. `ProjectionEliminator::Optimize` / `eliminate` 按父算子边界递归删除逻辑 Projection，并用列位置到表达式的映射重写祖先节点的 `expressions` 和 `conditions`。

这些职责直接依赖 `task.rs` 中简化的 `PlanKind`、`PlanNode` 和 `Expression` 模型，而不是 Go 实现使用的多态逻辑/物理算子体系。

## 主要符号

- `canProjectionBeEliminatedLoose(p: &PlanNode) -> bool`：要求 `p.kind == PlanKind::Projection`、恰有一个孩子，且所有 `p.expressions` 的 `column` 均为 `Some(_)`。空表达式集合满足 `all`，所以只要节点是单孩子 Projection，也会通过宽松判断。
- `canProjectionBeEliminatedStrict(p: &PlanNode) -> bool`：先调用宽松判断；若 `p.schema.is_empty()` 直接返回 `true`，否则要求 schema 长度等于孩子 schema 长度，并要求第 `offset` 个表达式引用 `Some(offset)`。它只比较长度和位置，不比较父子 `FieldType` 内容。
- `doPhysicalProjectionElimination(mut p: PlanNode) -> PlanNode`：递归调用 `eliminatePhysicalProjection` 处理全部孩子，然后在当前节点满足严格条件时以 `remove(0)` 取出并返回唯一孩子。
- `eliminatePhysicalProjection(p: PlanNode) -> PlanNode`：物理消除的公开入口，目前只是转发给 `doPhysicalProjectionElimination`。
- `ProjectionEliminator`：无字段的规则类型，派生 `Default`；本身不保存跨次优化状态。
- `ProjectionEliminator::Optimize(&self, p: PlanNode) -> (PlanNode, bool)`：为每次调用创建空 `HashMap<usize, Expression>`，以根节点 `canEliminate = false` 开始递归；无论内部是否删除节点，对外的第二个返回值固定为 `false`，与 Go 规则的 `planChanged` 约定一致。
- `ProjectionEliminator::eliminate(...)`：实际逻辑重写。内部返回的布尔值用于递归聚合删除事实，但 `Optimize` 会丢弃它。
- `ProjectionEliminator::Name(&self) -> &'static str`：返回稳定规则名 `projection_eliminate`。

本文件没有模块级常量、trait、条件编译分支或可变静态状态。

## 执行流程

物理路径从 `eliminatePhysicalProjection` 进入。`doPhysicalProjectionElimination` 先消费当前节点的 `children`，逐个递归并收集重写后的孩子，形成自底向上的遍历；随后调用严格判定。若当前节点是单孩子、逐位置列透传的 Projection，则返回该孩子，否则返回当前节点。由此，连续多层严格 Projection 会从最深层开始依次消除。

逻辑路径从 `ProjectionEliminator::Optimize` 进入：

1. 建立空替换表，并以 `canEliminate = false` 处理根节点，所以根 Projection 不会被本规则直接删除。
2. `eliminate` 遇到 `PlanKind::Cte` 立即返回，不进入其孩子，也不重写 CTE 节点自身。
3. 计算传给孩子的许可：`UnionAll` 强制为 `false`；`HashAgg`、`StreamAgg`、`Projection`、`Window` 强制为 `true`；其他算子继承父层许可。
4. 递归处理所有孩子并合并内部 `changed`。
5. 若当前节点在许可范围内且满足宽松判定，把每个输出位置 `i` 映射到对应表达式克隆，随后删除当前 Projection 并返回其唯一孩子。
6. 若当前节点未被删除，则遍历 `expressions` 与 `conditions`；当表达式是列引用且替换表含对应位置时，以克隆的替代表达式覆盖它。

测试 [`rule_eliminate_projection_test.rs`](rule_eliminate_projection_test.rs) 验证了根节点保护、Projection 下的子 Projection 删除与父表达式改写、`UnionAll` 边界、`Cte` 边界，以及对外始终返回 `changed == false`。

## 数据与状态

输入计划按值传入并按值返回，重写过程拥有整棵 `PlanNode`，不依赖内部可变性或共享引用。节点类型、孩子、schema、表达式与条件分别来自 `PlanNode.kind`、`children`、`schema`、`expressions` 和 `conditions`；列引用由 `Expression.column: Option<usize>` 表示。

逻辑路径唯一的临时状态是 `HashMap<usize, Expression>`。键是输出位置而不是稳定列 ID，值通过 `clone` 保存；同一个映射在整次深度优先遍历中跨层、跨兄弟共享，且没有进入/退出子树时的作用域恢复。当前模型因此依赖位置编号在相关重写区域内不会产生歧义。相比 Go 使用列哈希标识，这是一项需要扩展者特别关注的简化边界。

物理路径不建立额外状态。节点被消除时直接返回原孩子，因此孩子上的统计、schema、顺序标志和其他字段保持原值；本文件不会把被删 Projection 的 schema 或属性合并到孩子。

## 依赖与调用关系

直接 Rust 依赖只有 `crate::task::{Expression, PlanKind, PlanNode}` 和标准库 `std::collections::HashMap`。`Cargo.toml` 将该文件归入 `astersql-planner-core`，本文件自身未直接使用该 manifest 中的外部依赖或 `nextgen` feature。

RustCodeGraph 记录的内部边包括：`eliminatePhysicalProjection -> doPhysicalProjectionElimination`、`doPhysicalProjectionElimination -> eliminatePhysicalProjection`（递归），以及 `ProjectionEliminator::Optimize -> ProjectionEliminator::eliminate`。图上的 Rust 上游主要是 [`rule_eliminate_projection_test.rs`](rule_eliminate_projection_test.rs) 和 [`casetest/rule/rule_eliminate_projection_test.rs`](casetest/rule/rule_eliminate_projection_test.rs)；[`casetest/rule/rule_common_handle_ordering_test.rs`](casetest/rule/rule_common_handle_ordering_test.rs) 直接验证物理入口保留有序索引扫描属性。

完整应用中的对应接线位于 [`optimizer_runtime.rs`](optimizer_runtime.rs)：`LogicalRule::EliminateProjection` 分派到 `eliminate_identity_projections`，物理计划后处理调用更完整的 `eliminate_redundant_physical_projections`。这些函数与本文件语义相关但不是本文件函数的调用者。

## 错误处理与边界

本文件没有 `Result`、显式错误类型或日志；不满足消除条件时原样返回计划。以下结构前提由判定顺序保证：两个消除路径只有在 `children.len() == 1` 后才访问或移除第一个孩子，因此正常 API 路径不会因零孩子 Projection 在这里越界。

仍需注意这些边界：

- 宽松判定仅确认表达式是列引用，不确认列下标有效，也不确认与 schema 一一对应。
- 严格判定的空 schema 特例不检查表达式数量；非空时也不比较父子字段类型。
- 表达式改写只覆盖 `expressions` 和 `conditions` 中“整个表达式就是列引用”的情况，不递归进入复合表达式，也不处理 `by_items`、`group_items`、`agg_funcs` 或 schema 列。
- `UnionAll` 只禁止其直接孩子区域开始消除；`Cte` 则完全停止遍历。
- 共享的位置替换表可能让不同兄弟子树的相同位置互相覆盖；当前测试未覆盖这一风险。
- 本文件未实现 Go 版本的 failpoint、TiFlash reader 内部计划处理、MPP 最终聚合保护、`CalculateNoDelay`、update 列引用保护、相邻 Projection 表达式折叠和副作用判断。

## 并发与资源生命周期

规则不启动线程、异步任务或通道，不持有锁、事务、文件句柄或网络资源。所有计划节点和替换表达式都由当前调用栈拥有，递归返回后自动释放被替换的 Projection；保留的表达式通过克隆进入映射或父节点。

`ProjectionEliminator` 是零大小、无状态类型，同一个实例可被独立调用；不过单次 `Optimize` 内的可变替换表只应在该同步递归中使用。递归深度与计划树深度一致，极深计划会消耗相应栈空间；本文件没有深度限制或迭代化保护。

## 与 Go 版本的对应关系

直接对照文件是 [`rule_eliminate_projection.go`](rule_eliminate_projection.go)。名称和主干控制流保持一致：逻辑根从不可消除开始，`UnionAll` 阻断许可，聚合/Projection/Window 开启许可，CTE 独立优化，规则名相同，逻辑规则对外不设置 `planChanged`；物理路径同样先递归孩子再判断删除，并保留空 schema 特例。

Rust 当前是简化移植，不等价于 Go 完整行为：

- Go 宽松判定拒绝 `Proj4Expand`，Rust `PlanNode` 没有对应字段。
- Go 严格判定保护 TiFlash MPP 最终聚合与 `CalculateNoDelay`，还检查真实列相等性；Rust 只检查枚举种类、长度和位置。
- Go 物理流程会进入 TiFlash `PhysicalTableReader.TablePlan`、重建扁平计划，并保护 update 计划引用及某些子 Projection schema；Rust 只遍历 `children`。
- Go 逻辑流程按列哈希更新 schema 和所有计划表达式，特殊重建 Apply schema，并合并无副作用的相邻 Projection、替换表达式和折叠常量；Rust 只按位置改写两个表达式列表。

Go 的规则注册证据在 [`optimizer.go`](optimizer.go) 的 `optRuleList`，物理入口证据在同文件 `postOptimize`。Go 回归 [`logical_plans_test.go`](logical_plans_test.go) 的 `TestProjectionEliminator` 覆盖相邻投影/聚合场景；[`casetest/rule/rule_eliminate_projection_test.go`](casetest/rule/rule_eliminate_projection_test.go) 覆盖 Apply 与表达式索引查询。Rust 的同名 case test 保留了这些 SQL 意图，并额外提供通用 `PlanNode` 的规则级断言。

## 扩展指南

若继续完善这个轻量 API，应从语义缺口对应的最小符号切入：宽松资格规则修改 `canProjectionBeEliminatedLoose`，物理保护条件修改 `canProjectionBeEliminatedStrict` 或 `doPhysicalProjectionElimination`，逻辑边界与替换策略修改 `ProjectionEliminator::eliminate`。不要把 Rust 测试写入本源文件；应同步更新同目录独立测试 [`rule_eliminate_projection_test.rs`](rule_eliminate_projection_test.rs)，跨 crate/SQL 回归放在 [`casetest/rule/rule_eliminate_projection_test.rs`](casetest/rule/rule_eliminate_projection_test.rs)，物理顺序属性可沿用 `rule_common_handle_ordering_test.rs` 的模式。

扩展前应先决定本文件是否仍需作为公开轻量接口，还是应复用/委托 `optimizer_runtime.rs` 的生产实现，避免形成两套漂移规则。若保留本实现，优先引入稳定列标识和有作用域的替换环境，并补充多兄弟、嵌套复合表达式、空 schema 带表达式、无效列下标、Expand、MPP 聚合、update schema 与副作用表达式测试。兼容风险主要是误删为类型对齐或执行语义专门保留的 Projection；性能风险主要来自表达式深克隆和深树递归。

## 验证依据

- 源码全貌：[`rule_eliminate_projection.rs`](rule_eliminate_projection.rs)；数据模型：[`task.rs`](task.rs)；模块导出：[`lib.rs`](lib.rs)；crate 边界：[`Cargo.toml`](Cargo.toml)。
- RustCodeGraph：`status` 显示仓库索引可用；`query ProjectionEliminator`、`query eliminatePhysicalProjection`、`query canProjectionBeEliminatedStrict` 定位 Rust/Go 定义；`node ProjectionEliminator` 与 `node eliminatePhysicalProjection` 给出源码和调用轨迹。宽泛的文件过滤/探索未返回目标，故按技能规则用源码和 `rg` 补齐上游证据。
- Rust 测试：[`rule_eliminate_projection_test.rs`](rule_eliminate_projection_test.rs)、[`casetest/rule/rule_eliminate_projection_test.rs`](casetest/rule/rule_eliminate_projection_test.rs)、[`casetest/rule/rule_common_handle_ordering_test.rs`](casetest/rule/rule_common_handle_ordering_test.rs)。
- Go 对照与测试：[`rule_eliminate_projection.go`](rule_eliminate_projection.go)、[`optimizer.go`](optimizer.go)、[`logical_plans_test.go`](logical_plans_test.go)、[`casetest/rule/rule_eliminate_projection_test.go`](casetest/rule/rule_eliminate_projection_test.go)。
- 应用接线复核：[`optimizer_runtime.rs`](optimizer_runtime.rs) 中 `LogicalRule::EliminateProjection`、`eliminate_identity_projections` 和 `eliminate_redundant_physical_projections`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查，并人工复核链接、符号名和现状/Go 差异表述。
