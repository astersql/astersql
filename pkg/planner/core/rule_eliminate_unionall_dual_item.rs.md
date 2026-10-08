# `pkg/planner/core/rule_eliminate_unionall_dual_item.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate（`pkg/planner/core/Cargo.toml` 的 `[lib] path = "lib.rs"`）中，并由 `pkg/planner/core/lib.rs` 以 `pub mod rule_eliminate_unionall_dual_item` 公开。它是 Go 文件 `pkg/planner/core/rule_eliminate_unionall_dual_item.go` 的轻量 Rust 移植，操作的是 `crate::task::{PlanNode, PlanKind}` 值类型计划树，而不是生产优化器使用的 `Box<dyn logicalop::LogicalPlan>`。

当前接线必须特别区分：仓库搜索和 RustCodeGraph 调用边只找到 `pkg/planner/core/rule_eliminate_unionall_dual_item_test.rs` 调用本文件的 `EliminateUnionAllDualItem::Optimize`；完整应用的逻辑优化流水线在 `pkg/planner/core/optimizer_runtime.rs` 中通过 `LogicalRule::EliminateUnionAllDualItem => eliminate_union_all_dual_items(plan)` 执行另一份 `dyn LogicalPlan` 实现。因此，本文件目前是可独立验证的移植/模型实现，不是 SQL 主链上该规则的实际执行入口。

## 核心职责

`unionAllEliminateDualItem` 在一棵拥有所有权的 `PlanNode` 树中处理 `PlanKind::UnionAll`：

- 删除直接子节点中“类型为 `PlanKind::Other("TableDual")` 且 `stats.row_count == 0.0`”的空分支。
- 删除形如 `Projection -> 零行 TableDual` 的直接子分支；只检查 Projection 的第一个子节点。
- 若过滤后 UnionAll 没有任何子节点，则用一个新的零行 `TableDual` 替代它，并保留原 UnionAll 的 `schema`。
- 对仍保留的子节点递归执行相同规则，并汇总后代替换产生的 `changed` 标志。

文件顶部注释称“若剔除后仅剩一个分支，则整个 UnionAll 可退化为该分支”，但本文件实现没有执行这一折叠；实际代码只在子节点数变为零时替换为 `TableDual`。扩展或排错时应以 `unionAllEliminateDualItem` 的当前实现和独立测试为准。

## 主要符号

- `pub struct EliminateUnionAllDualItem`：无字段的规则门面，派生 `Default`，本身不保存配置或运行状态。
- `EliminateUnionAllDualItem::Name(&self) -> &'static str`：返回与 Go 规则一致的稳定名称 `union_all_eliminate_dual_item`。命名采用 Go 风格，并由 crate 根的 `#![allow(non_snake_case)]` 容纳。
- `EliminateUnionAllDualItem::Optimize(&self, p: PlanNode) -> (PlanNode, bool)`：公开方法，按值接收并返回计划，将全部工作委托给 `unionAllEliminateDualItem`。
- `fn is_zero_row_dual(p: &PlanNode) -> bool`：文件私有判定器；要求 `kind` 精确匹配字符串编码的 `Other("TableDual")`，且估算行数精确等于 `0.0`。
- `pub fn unionAllEliminateDualItem(mut p: PlanNode) -> (PlanNode, bool)`：公开递归实现；先处理当前节点，再处理保留下来的后代。

本文件没有模块常量、trait、条件编译项或错误类型。

## 执行流程

1. `Optimize` 把拥有所有权的根节点交给 `unionAllEliminateDualItem`。
2. 若当前节点不是 `PlanKind::UnionAll`，跳过当前层过滤，直接进入子树递归。
3. 若当前节点是 UnionAll，`Vec::retain` 按原顺序检查每个直接子节点：直接零行 Dual 被删除；Projection 的第一个孩子若是零行 Dual，该 Projection 也被删除；其余节点保留。
4. 过滤后若 `children.is_empty()`，构造 `PlanNode::new(PlanKind::Other("TableDual".to_owned()))`，把原节点的 `schema` 移入新节点，并立即返回 `(dual, true)`。由于提前返回，新 Dual 不再递归。
5. 否则初始化 `changed = false`，消费剩余 `children`，逐个递归优化。每个返回节点重新收集到当前节点的子列表；所有 `child_changed` 通过按位或赋值合并。
6. 返回当前节点和累计标志。仅删除当前 UnionAll 的部分分支不会把 `changed` 设为 `true`；只有“当前 UnionAll 变空并被替换”或后代发生这种替换时才报告变化。这一语义由 `removes_direct_and_projected_empty_duals_without_collapsing_union` 固定。

处理顺序是“当前层过滤在先、保留子树递归在后”。因此，一个 Projection 在当前层检查时若包裹的是尚未优化成 Dual 的嵌套 UnionAll，它会先被保留；随后内部 UnionAll 才可能变成 Dual，外层不会回头再次删除该 Projection。`filtering_precedes_recursion_like_go` 专门覆盖这一点。

## 数据与状态

主要数据来自 `pkg/planner/core/task.rs`：`PlanNode` 持有 `kind`、`children`、`stats`、`schema` 以及表达式、代价和执行属性等字段。本规则只读取 `kind` 与 `stats.row_count`，重写 `children`，并在生成替代 Dual 时搬移 `schema`；其他字段不会复制到替代节点，而是采用 `PlanNode::new`/`Default` 的默认值。

规则按值消费整棵树，因此没有共享别名：`retain` 原地缩短当前节点的子列表，随后 `into_iter` 转移各子节点所有权并重建列表。保留下来的兄弟节点顺序不变。新建 Dual 的默认 `StatsInfo` 使 `row_count` 为默认零值，独立测试也验证该值为 `0.0`。

空分支判定依赖两个表示约定：TableDual 以自由字符串 `PlanKind::Other("TableDual")` 编码，而不是专用枚举变体；行数必须与 `0.0` 精确相等。负数、正数、`NaN` 或名称不同的 `Other` 均不会被本判定器删除。

## 依赖与调用关系

本文件唯一的直接 Rust 依赖是同 crate 的 `crate::task::{PlanKind, PlanNode}`，不直接使用 `pkg/planner/core/Cargo.toml` 中列出的外部 crate。`PlanNode::new` 提供替代 Dual 的默认构造，`PlanKind::{UnionAll, Projection, Other}` 提供节点分类，`PlanNode.children`、`stats.row_count` 和 `schema` 承载变换所需数据。

静态调用链为：

`rule_eliminate_unionall_dual_item_test.rs` → `EliminateUnionAllDualItem::Optimize` → `unionAllEliminateDualItem` → `is_zero_row_dual`，其中 `unionAllEliminateDualItem` 还有一条对子节点的自递归边。

生产主链则是独立路径：`optimizer_runtime.rs::logical_optimize_in_place` 遍历 `LOGICAL_RULES`，匹配 `LogicalRule::EliminateUnionAllDualItem` 后调用同文件内的 `eliminate_union_all_dual_items`。仓库内没有从该主链到本文件符号的调用边；两者不可视为同一实现。

## 错误处理与边界

接口不返回 `Result`，内部也没有显式错误分支；输入不符合识别模式时保持原样返回。Projection 的孩子通过 `first()` 安全读取，所以零孩子 Projection 不会 panic，也不会被删除。这一点比 Go 对照中直接读取 `proj.Children()[0]` 更防御性。

需要注意以下边界：

- 当前层删除一部分 Dual 子节点时，返回的布尔值仍可能为 `false`；调用方不能把该标志理解为“树结构是否发生任何修改”。这是对 Go 当前行为的刻意保留。
- 过滤后只剩一个有效孩子时仍保留 UnionAll，尽管文件注释描述了可折叠性。
- 新 Dual 仅继承 `schema`；原 UnionAll 的表达式、条件、标签、代价和其他 `PlanNode` 元数据都会丢弃。当前独立测试只证明 schema 与零行统计的预期。
- 递归深度与计划树高度一致；极深的人工计划可能消耗较多调用栈，本文件没有深度限制或迭代化保护。
- 判定依赖浮点数精确等于零及字符串类型标签，新增专用 `TableDual` 变体或统计规范变化时必须同步调整。

## 并发与资源生命周期

`EliminateUnionAllDualItem` 是无状态零大小类型；优化函数既不访问锁、原子变量、线程、异步任务、通道、事务，也不持有外部资源。每次调用完全拥有传入的 `PlanNode`，在当前线程同步完成树重写后返回，因此不同调用之间没有本文件引入的共享状态或竞争条件。

资源生命周期由 Rust 所有权自然约束：被 `retain` 删除的子树立即离开集合并析构；保留子树在递归中按值移动；当 UnionAll 变空时，原节点除被移入新 Dual 的 `schema` 外随提前返回被析构。主要资源风险不是泄漏，而是递归栈深度和重建子列表所需的线性遍历/分配。对含 `n` 个节点的树，正常遍历时间为 `O(n)`；每层只保留当前路径的递归栈。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/rule_eliminate_unionall_dual_item.go`。名称、入口委托、当前层先过滤再递归、直接零行 Dual、Projection 包裹零行 Dual、空 UnionAll 转换为 Dual并保留 schema，以及“部分过滤不设置 changed”这些核心语义在本文件中均有对应实现。

两侧表示和接口不同：Go 接收 `context.Context` 与 `base.LogicalPlan` 接口并返回 `(plan, bool, error)`；本文件接收拥有所有权的具体 `PlanNode`，没有 context 或 error。Go 通过类型断言识别 `LogicalUnionAll`、`LogicalProjection`、`LogicalTableDual`，Rust 本文件通过 `PlanKind` 和字符串识别。Go 替代 Dual 使用原 UnionAll 的 session context 初始化，Rust 模型没有等价上下文字段。

Rust 对空 Projection 使用 `first()`，而 Go 代码假定 Projection 至少有一个孩子。Rust 通过消费并重建 `Vec` 安装递归结果；Go 仅在 `changed` 为真时把递归返回的孩子写回。就当前规则返回约定而言，两者对“后代 UnionAll 变空”的可见结果一致。

还应与生产 Rust 实现区分：`optimizer_runtime.rs::eliminate_union_all_dual_items` 使用真实 `logicalop` 类型，采用先递归后过滤，只删除直接 `LogicalTableDual`，并在只剩一个孩子时折叠 UnionAll；它没有复刻本文件/Go 文件的 Projection+Dual、零孩子转 Dual和 changed 返回协议。这是现有仓库的实现分流事实，不应由本说明推断二者已经语义等价。

没有发现按规则名或符号名命名的独立 Go 单元测试；本文件的直接回归证据来自同目录 Rust 测试 `rule_eliminate_unionall_dual_item_test.rs`。Go 优化器注册顺序可由 `optimizer.go` 的 `optRuleList` 与 `optRuleFlags` 核对。

## 扩展指南

若扩展本文件的模型规则，应首先明确是否也要同步生产主链的 `optimizer_runtime.rs::eliminate_union_all_dual_items`；当前两份 Rust 实现的数据模型和行为并不相同，修改其中一份不会自动影响另一份。与 Go 提交对齐时，应以 `rule_eliminate_unionall_dual_item.go` 的增量为边界，逐项记录表示差异，避免顺手重构整个优化框架。

常见改动位置及配套验证如下：

- 新增可消除形态：修改 `unionAllEliminateDualItem` 的 `retain` 判定，并在独立文件 `pkg/planner/core/rule_eliminate_unionall_dual_item_test.rs` 增加正例、近似但不得删除的反例及顺序测试。
- 改变 Dual 表示或零行判定：优先收敛 `is_zero_row_dual`，覆盖负数、非零数、`NaN`、错误名称和新枚举变体等边界。
- 改变 `changed` 契约：同时更新所有提前返回和递归汇总逻辑，并明确是否有调用方依赖“只报告节点替换、不报告分支过滤”的 Go 兼容行为。
- 增加单子节点折叠：必须先解决文件注释、Go 当前行为与生产 Rust 行为之间的差异，并验证 schema、节点元数据及调用方标志语义，而不能只为减少节点数直接返回孩子。
- 调整替代 Dual 的元数据：检查 `PlanNode::default` 的所有字段，明确哪些应从 UnionAll 继承，并为每项新增独立断言。

测试逻辑应继续放在同目录的独立 `*_test.rs` 文件，不嵌入生产源文件。此类变更还应评估兼容风险（规则标志与返回协议）、正确性风险（错误删除非空分支、schema/元数据丢失）和性能风险（重复遍历或克隆整棵子树）。

## 验证依据

本说明依据以下直接证据整理：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `explore "pkg/planner/core/rule_eliminate_unionall_dual_item.rs EliminateUnionAllDualItem unionAllEliminateDualItem"`：定位 Rust/Go 实现，显示 Rust 递归函数的两个调用者为 `Optimize` 与自身，并给出独立 Rust 测试源码。
- RustCodeGraph `query`/`node`：确认 `EliminateUnionAllDualItem`、`unionAllEliminateDualItem`、`task.rs::PlanNode` 的定义，以及 `lib.rs` 的模块声明。
- `pkg/planner/core/rule_eliminate_unionall_dual_item.rs`：规则符号、过滤顺序、返回标志和 schema 转移的主要事实来源。
- `pkg/planner/core/task.rs`：`PlanKind`、`PlanNode`、默认构造和孩子建造器的数据模型来源。
- `pkg/planner/core/rule_eliminate_unionall_dual_item_test.rs`：规则名、直接/投影 Dual 删除、空 UnionAll 替换、schema 保留、changed 协议和过滤先于递归的回归证据。
- `pkg/planner/core/rule_eliminate_unionall_dual_item.go`、`optimizer.go`：Go 语义与规则注册顺序的对照证据；仓库范围符号检索未发现该规则的专门 Go 测试。
- `pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`：crate 归属、公开模块和独立测试装配依据。
- `pkg/planner/core/optimizer_runtime.rs`：生产 Rust `LOGICAL_RULES`、规则分派和真实 `logicalop` 实现的依据，证明目标文件当前未接入该主链。

本任务是纯文档分析，按计划不运行 Cargo。交付前另以规定命令验证本文恰好包含十一个固定二级章节，并人工检查所有“已支持”陈述均能回指以上源码、图查询或测试证据。
