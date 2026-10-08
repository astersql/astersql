# `pkg/planner/core/rule_topn_push_down.rs`

## 文件定位

该文件位于 `astersql-planner-core` crate 内，定义一个面向简化计划树 `crate::task::PlanNode` 的 TopN/Limit 下推器 `PushDownTopNOptimizer`。模块由 `pkg/planner/core/lib.rs` 公开导出，独立测试模块 `rule_topn_push_down_test` 仅在 `cfg(test)` 下装配。crate 边界及模块归属分别由 `pkg/planner/core/Cargo.toml` 的 `[package] name = "astersql-planner-core"`、`[lib] path = "lib.rs"` 和 `lib.rs` 的 `pub mod rule_topn_push_down` 确认。

当前文件不是 Rust 优化主链中的实际执行节点：`pkg/planner/core/optimizer.rs::logicalOptimize` 的规则名数组包含 `"topn_push_down"`，但循环只调用通用 `normalize(&mut plan, rule)`；该函数当前只对 `projection_eliminate` 有行为，不会构造或调用 `PushDownTopNOptimizer`。因此，本文件现状是“已公开、可直接调用并有单测覆盖的局部规则实现”，不能据此宣称 `DoOptimize` 已执行该规则。

## 核心职责

- `PushDownTopNOptimizer::Optimize` 先递归处理全部子树，再识别当前节点是否为单孩子的 `PlanKind::TopN` 或 `PlanKind::Limit`（`rule_topn_push_down.rs:15-24`）。
- 对 `Projection` 或 `Selection` 子节点，将 TopN/Limit 与该子节点交换，使截断节点更靠近数据源（`rule_topn_push_down.rs:27-34`）。
- 对 `UnionAll` 子节点，向每个分支复制 TopN/Limit；分支副本把 `offset` 清零，并将 `count` 设置为原 `offset + count`，同时保留原 TopN/Limit 在 UnionAll 之上完成最终偏移与截断（`rule_topn_push_down.rs:36-47`）。
- 对其他节点形态保持父子关系，只保留递归处理子树所得结果（`rule_topn_push_down.rs:49-53`）。
- `Name` 返回与 Go 规则注册名、优化器黑名单名一致的 `"topn_push_down"`（`rule_topn_push_down.rs:55-58`、`optimizer.go`、`logical_operator_test.go`）。

## 主要符号

- `pub struct PushDownTopNOptimizer`：无字段的规则对象，派生 `Default`；它不持有会话、统计或配置状态。
- `pub fn Optimize(&self, mut p: PlanNode) -> (PlanNode, bool)`：取得计划树所有权并返回重写后的树以及 `planChanged` 标志。方法名保留 Go 风格大写。返回元组不含错误类型。
- `pub fn Name(&self) -> &'static str`：返回静态规则名。
- 本文件无模块级常量、trait、条件编译项或私有辅助函数。所操作的 `PlanKind` 与 `PlanNode` 定义在 `pkg/planner/core/task.rs`；关键字段为 `kind`、`children`、`offset`、`count`，而 `PlanNode` 的 `Clone` 派生使 UnionAll 分支复制成为可能。

## 执行流程

1. `Optimize` 消耗 `p.children`，对每个孩子递归调用自身，再收集回 `p.children`。这使改写顺序为后序/自底向上；孩子返回的布尔标志被丢弃，因为契约最终固定返回 `false`。
2. 仅当当前节点是 TopN/Limit 且恰有一个孩子时进入本层下推；零孩子或多孩子的异常形态直接保持结构。
3. 若孩子是 Projection/Selection，先从当前节点移走唯一孩子，再从该孩子移走第一个孙节点；TopN/Limit 接管该孙节点，Projection/Selection 再接管 TopN/Limit，随后立即返回。
4. 若孩子是 UnionAll，对每个分支克隆当前 TopN/Limit，设置分支副本 `offset = 0`、`count = wrapping_add(offset, count)`，并用该副本包裹原分支；最后把改写后的 UnionAll 放回原 TopN/Limit 下方并返回。
5. 其他孩子重新挂回当前节点；函数统一返回 `(p, false)`。

例如 `TopN(Selection(Scan))` 变为 `Selection(TopN(Scan))`；`Limit(offset=o,count=c, UnionAll(A,B))` 变为 `Limit(o,c, UnionAll(Limit(0,o+c,A), Limit(0,o+c,B)))`。第二种形态保留外层节点，保证分支预裁剪不代替最终全局排序/偏移语义。

## 数据与状态

规则自身无可变成员和全局状态；全部变化发生在按值传入的 `PlanNode` 树上。`children` 通过 `into_iter` 转移所有权，Projection/Selection 路径通过 `Vec::remove(0)` 移动节点；UnionAll 路径通过 `p.clone()` 为每个分支复制完整节点及其附属字段，再替换分支。

TopN/Limit 的分页量是 `u64`（`pkg/planner/core/task.rs::PlanNode`）。分支所需行数明确使用 `wrapping_add`，溢出按模 2^64 回绕；独立 Rust 测试以 `u64::MAX + 2 == 1` 固化这一行为。该实现不会更新 `stats`、`schema`、表达式、代价或其他节点元数据，只随整节点移动/克隆保留原值。

## 依赖与调用关系

直接生产依赖只有 `crate::task::{PlanKind, PlanNode}`，均属于同一 crate；该文件未直接使用 `Cargo.toml` 中的外部 crate 或 feature。`nextgen` feature 不改变本文件编译内容。

已验证的上游关系如下：

- `pkg/planner/core/lib.rs` 公开声明该模块，并在测试配置下声明对应测试模块。
- `pkg/planner/core/rule_topn_push_down_test.rs` 直接构造 `PushDownTopNOptimizer` 并调用 `Optimize`；RustCodeGraph 的文件关系也把该测试列为使用者。
- RustCodeGraph 对同名 `Optimize` 的精确 caller 查询没有返回可靠调用者；结合仓库文本搜索，生产 Rust 代码中没有 `PushDownTopNOptimizer` 的构造或调用点。
- `pkg/planner/core/optimizer.rs::logicalOptimize` 只把 `topn_push_down` 当作追踪/开关名称传给 `normalize`，不是本类型的动态注册或调用。

下游关系是对 `PlanNode.children` 的递归遍历、`PlanKind` 模式匹配、`Vec::remove`、`PlanNode::clone` 及整数 `wrapping_add`。RustCodeGraph 对该方法只识别到 `remove`，未完整解析递归、自带 trait 方法及字段操作，因此这些细节以目标源码为直接证据。

## 错误处理与边界

该 API 不返回 `Result`，也不显式生成错误。以下结构边界决定其行为：

- 当前 TopN/Limit 必须恰有一个孩子，否则不下推。
- Projection/Selection 路径默认其自身至少有一个孩子，并直接执行 `child.children.remove(0)`；若输入是无孩子的 Projection/Selection，会 panic。它也未要求该孩子“恰有一个”孙节点；若存在多个孙节点，只移动第一个，其余仍留在 Projection/Selection 下。因此调用方必须维护这些一元算子的合法树形不变量。
- UnionAll 可有零个或多个分支；零分支只会保留外层 TopN/Limit 和空 UnionAll，不 panic。
- 非 Projection、Selection、UnionAll 的算子形成下推边界。
- `wrapping_add` 是刻意的无符号回绕，而非饱和或报错；`rule_topn_push_down_test.rs::union_branch_count_uses_go_unsigned_addition_semantics` 是直接证据。
- 返回的 `changed` 永远为 `false`，即使树已改变；这是对 Go `planChanged := false` 契约的兼容，不可把该标志解释为结构是否相等。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。`Optimize` 只在当前调用栈中同步递归；生命周期由 Rust 所有权管理：输入树被消耗，临时节点在重挂接后进入返回树，未保留外部引用。

递归深度与计划树高度一致，极深的人工计划树可能消耗较多栈空间。UnionAll 下推会对每个分支克隆 TopN/Limit 节点，其时间与分支数及节点附属数据大小相关，且会增加计划节点数；没有共享可变状态，所以同一无状态优化器可被多个调用者并行借用，但调用者各自必须拥有输入树。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/rule_topn_push_down.go`。两端一致之处是类型名、规则名，以及即使发生改写也返回 `planChanged = false`。Go 的 `Optimize` 还接受 `context.Context`、返回 `error`，并把行为委托给 `p.PushDownTopN(nil)`；Rust 签名省略上下文和错误通道，直接操作简化 `PlanNode`。

两者的覆盖范围并不等价。Go 的 `PushDownTopN` 是逻辑计划接口上的多态操作，真实实现分布在 `operator/logicalop` 的 base、projection、limit、sort、union-all、join、lock、CTE、mem-table、partition-union-all 等文件中。Projection 实现还会检查/改写排序表达式；`operator/logicalop/logicalop_test/logical_operator_test.go::TestLogicalProjectionPushDownTopN` 验证表达式经投影内联后的真实 SQL 计划及规则黑名单效果。当前 Rust 文件仅处理 Projection、Selection 与 UnionAll 三种 `PlanKind`，不做表达式替换，也尚未接入 `DoOptimize` 主链，因此应视为局部迁移基线，而非 Go 完整规则的等价移植。

## 扩展指南

- 若增加可穿透算子或新的停止条件，主要修改点是 `Optimize` 的 `match child.kind`；先核对相应 Go `LogicalPlan.PushDownTopN` 实现，避免把只对特定表达式、Join 类型或存储能力安全的下推泛化。
- 扩展 Projection 下推时，应在移动节点前完成 `by_items`/表达式列引用重写与可下推性检查；Go 的 `logical_projection.go::PushDownTopN` 和 `TestLogicalProjectionPushDownTopN` 是直接语义基准。
- 扩展 UnionAll 时必须保留外层 TopN/Limit，并维持分支 `offset=0,count=offset+count`；需要明确继续采用 Go `uint64` 回绕语义还是引入可报告的溢出策略。
- 若要让规则进入 Rust 优化主链，应在 `optimizer.rs::logicalOptimize` 的实际规则分派处接线，不能只保留字符串；同时验证 `disabled_rules`、追踪顺序与 flag 位语义。
- 测试应继续放在独立的 `pkg/planner/core/rule_topn_push_down_test.rs`，不要内嵌进生产文件。至少覆盖合法一元树、零/多孩子非法形态、下推边界、UnionAll 多/零分支、TopN 与 Limit 两类、表达式/元数据保留，以及接入主链后的开关行为。
- 性能审查重点是递归栈深度和 UnionAll 分支克隆成本；兼容性审查重点是 Go 多态实现中尚未迁移的各算子语义。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件，目标文件被索引且识别出 5 个符号；`files --filter pkg/planner/core/rule_topn_push_down.rs` 与 `node --file ... --offset 1 --limit 500` 核对了文件全貌；`query push_down_top_n`、`explore`、`callers/callees` 用于检查相邻符号和调用边。同名 `Optimize` 导致图查询噪声，故生产调用缺失又以仓库文本搜索和入口源码交叉验证。
- Rust 源与入口：`pkg/planner/core/rule_topn_push_down.rs`、`pkg/planner/core/task.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/optimizer.rs`。
- crate 配置：`pkg/planner/core/Cargo.toml`。
- 独立 Rust 测试：`pkg/planner/core/rule_topn_push_down_test.rs`，覆盖 changed 标志和无符号溢出回绕。
- Go 对照与主链：`pkg/planner/core/rule_topn_push_down.go`、`pkg/planner/core/optimizer.go`。
- Go 算子与测试证据：`pkg/planner/core/operator/logicalop/logical_projection.go`、`logical_union_all.go`、`logical_limit.go`、`logical_sort.go`、`logical_plans_misc.go`，以及 `pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.go::TestLogicalProjectionPushDownTopN`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构校验，并人工检查上述路径、符号、边界和未接线结论。
