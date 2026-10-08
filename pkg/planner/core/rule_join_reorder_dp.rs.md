# `pkg/planner/core/rule_join_reorder_dp.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；crate 根 `pkg/planner/core/lib.rs` 通过 `pub mod rule_join_reorder_dp` 将它注册为公开模块，`pkg/planner/core/Cargo.toml` 则声明该 crate 对应 Go 包 `pkg/planner/core`。它是 Join Reorder 规则的小规模连接组求解器：上游 `rule_join_reorder.rs` 的 `JoinReOrderSolver::optimizeRecursive` 先递归优化子树、抽取连续 Inner Join 组，再在节点数不超过 `dpThreshold.max(2)` 时构造 `joinReorderDPSolver` 并调用 `solve`；更大的组交给贪心求解器。

这里操作的是 Rust 迁移层自己的 `JoinPlan`/`JoinEdge`/`Expression` 数据模型，不直接操作 Go 版的 `base.LogicalPlan`、统计信息接口或表达式树。文件已接入规划器规则主链，并非占位文件；但相对同路径 Go 实现仍存在明确的语义简化，详见“与 Go 版本的对应关系”。

## 核心职责

1. `solve` 把 `base.eqEdges` 中的列级等值条件映射为 Join 组节点之间的无向边，并建立邻接表。
2. 对邻接图的每个连通分量调用 `bfsGraph` 得到稳定的局部节点顺序，再由 `dpGraph` 对该分量枚举子集拆分。
3. `dpGraph` 只接受左右子集之间存在等值边的组合，通过 `newJoinWithEdge` 构造候选计划，并为每个子集保留 `row_count` 最小的候选。
4. `solve` 最后调用 `makeBushyJoin`，把互不连通的分量按轮次两两合并为灌木式笛卡尔连接树。
5. 两个列定位函数是到 `rule_join_reorder.rs` 共享实现的公开转发，确保 DP 文件与其它重排逻辑使用相同的“列属于哪个组节点”规则。

核心不变量是：单个 `dpGraph` 只处理一个等值边连通分量；位掩码的第 `bit` 位对应 `order[bit]`，而不是原始 `group` 下标；一个候选拆分必须已经有左右最优子计划且至少有一条跨越两侧的等值边。

## 主要符号

- `joinGroupEqEdge { node1, node2, edge }`：将原始 `JoinEdge` 附上两个 Join 组节点下标，供建图、连通性判断和候选 Join 构造使用。三个字段均公开，类型可克隆并可调试打印。
- `joinGroupNonEqEdge { nodes, condition }`：表示涉及多个节点的非等值条件。当前文件仅声明该公开类型，没有在 `solve`/`dpGraph` 中实例化或消费；不能据此声称 Rust DP 已实现 Go 版的非等值边位图分配。
- `joinReorderDPSolver { base }`：DP 求解器及共享状态容器。`baseSingleGroupJoinOrderSolver` 保存等值边、其它条件、Join 类型信息和列分配器。
- `findNodeIndexInGroup(group, column)`：公开薄转发；返回首个 schema 含该列的节点，找不到时返回字符串错误。
- `findNodeIndexForColumns(group, columns)`：公开薄转发；要求列集合非空且全部属于同一节点，否则返回错误。
- `solve(&mut self, joinGroup)`：求解入口，完成边映射、连通分量划分、分量内 DP 和分量间笛卡尔合并。
- `bfsGraph(&self, startNode, adjacents)`：对单个连通分量做 BFS，返回原始节点下标序列。它使用函数内局部 `visited`；跨分量去重由 `solve` 的外层 `visited` 完成。
- `dpGraph(&mut self, order, group, edges)`：子集 DP。`best: HashMap<usize, JoinPlan>` 保存每个已构造掩码的当前最优计划。
- `nodesAreConnected(leftMask, rightMask, order, edges)`：将边端点从原始节点下标映射到 `order` 位号，判断是否有边跨越左右掩码。
- `newJoinWithEdge(left, right, edges, otherConds)`：筛出真正跨越左右计划 schema 的等值边，委托 `base.makeJoin` 构造 Inner Join。
- `makeBushyJoin(group, otherConds)`：先把参数追加到 `base.otherConds`，再委托共享实现按轮次两两创建笛卡尔 Join。

文件没有条件编译项、模块级常量或 trait 实现；上述类型和函数均采用 Go 风格名称，crate 根通过 `#![allow(non_snake_case)]` 和 `#![allow(non_camel_case_types)]` 接受该命名。

## 执行流程

`JoinReOrderSolver::optimizeRecursive` 是实际入口。它从原计划抽取 `basicJoinGroupInfo`，复制出 `baseSingleGroupJoinOrderSolver`；当组规模位于 DP 阈值内时，调用本文件的 `solve`，求解后再由 `restoreSchemaIfChanged` 恢复原输出列顺序。

`solve` 的步骤如下：

1. 拒绝空组；同时要求 `joinGroup.len() <= usize::BITS - 1`，避免后续 `1usize << order.len()` 超出位图容量。
2. 遍历 `self.base.eqEdges`。每条边的左右列分别经 `findNodeIndexForColumns` 定位到组节点；定位失败立即用 `?` 返回。左右列落在同一节点时，该边被忽略，否则生成 `joinGroupEqEdge`。
3. 以这些等值边建立双向邻接表。
4. 从原组下标 0 开始扫描未访问节点。`bfsGraph` 返回该节点所在的完整连通分量，外层将分量节点标记为已访问，并调用 `dpGraph` 得到一个分量级最优计划。孤立节点形成长度为 1 的分量。
5. 将全部分量计划交给 `makeBushyJoin`。因此全笛卡尔组不会进入一次缺少中间连通子计划的大 DP，而是每个节点先独立成分量，再成对合并。

`dpGraph` 先把每个单节点子集写入 `best`，再按子集大小从 2 到 `order.len()` 枚举所有掩码。对每个掩码，以 `(subset - 1) & mask` 枚举非空真子集；只有 `subset < other` 时才处理，以消除左右交换产生的重复拆分。若任一半没有已知计划，或 `nodesAreConnected` 判定没有跨边，则跳过。其余情况由 `newJoinWithEdge` 构造候选，并仅在该掩码尚无候选或候选 `row_count` 更小时替换。最终移出全集掩码；若不存在则报错。

按所有子集和拆分计，分量内搜索具有典型子集 DP 的指数级时间/空间开销（时间上界可按约 `O(3^n)` 理解，`best` 最多含 `2^n-1` 个掩码），因此只能由上游阈值限制在小连接组使用。

## 数据与状态

- 输入 `joinGroup: &[JoinPlan]` 只读；叶子和中间计划通过 `Clone` 进入候选表，原切片不被修改。
- `eq` 与 `adjacency` 是单次 `solve` 的局部图状态。边保存完整 `JoinEdge`，邻接表只保存端点下标。
- `order` 是某连通分量内“局部位号到原始组节点下标”的映射。`nodesAreConnected` 每次用线性 `position` 反查位号；不在该分量的端点映射为 0 位掩码值并自然不匹配。
- `best` 以 `usize` 位图为键。单节点初始化为 `1 << bit`，全集为 `(1 << order.len()) - 1`。
- 选择指标仅为 `JoinPlan.row_count`，不是共享模块中 `calcJoinCumCost` 所定义的“左右累计代价加当前行数”。相同行数时保留先遇到的候选。
- `newJoinWithEdge` 会可变借用 `self.base`。共享 `makeJoin` 会分配新计划 id、合并 schema、估算行数，并可能从 `base.otherConds` 中排出条件；所以一次求解会推进 `columnAllocator`，也可能消费 `otherConds`。
- `makeBushyJoin` 先把显式传入的其它条件追加到 `base.otherConds`。当前 `solve` 传入空向量，实际剩余条件来自求解器原有共享状态。

## 依赖与调用关系

上游调用链为 `JoinReOrderSolver::Optimize` → `optimizeRecursive` → `joinReorderDPSolver::solve`，证据在 `pkg/planner/core/rule_join_reorder.rs`。`lib.rs` 同时把生产模块公开，并在 `#[cfg(test)]` 下以独立文件 `rule_join_reorder_dp_test.rs` 挂载测试，符合测试与源文件分离要求。

本文件的直接下游依赖全部位于同一 crate：

- `crate::rule_join_reorder::{JoinEdge, JoinPlan, Result, baseSingleGroupJoinOrderSolver}` 提供计划/边模型、错误别名和 Join 构造共享状态。
- `crate::rule_join_reorder::findNodeIndexInGroup` 与 `findNodeIndexForColumns` 提供列归属校验。
- `baseSingleGroupJoinOrderSolver::makeJoin` 创建带等值条件的 Join，合并 schema 并调用本地行数估计。
- `baseSingleGroupJoinOrderSolver::makeBushyJoin` 按轮次合并无等值边的分量。
- `crate::task::Expression` 是其它条件的数据类型；本文件只传递它，不解释表达式。
- 标准库 `HashMap` 保存 DP 表，`VecDeque` 实现 BFS 队列。

RustCodeGraph 的 `explore` 结果确认了本文件内部 `solve → dpGraph`、`dpGraph → nodesAreConnected/newJoinWithEdge`、`solve → makeBushyJoin`，并把 Rust `solve` 的上游定位到 `rule_join_reorder.rs` 的 `optimizeRecursive`。精确 `files --filter` 未命中目标路径，且部分带歧义的 `callers/callees` 查询长时间无结果，因此模块注册、测试挂载和精确 Rust 调用位置另以 `rg` 及源码读取核验。

## 错误处理与边界

- 空输入在 `solve` 返回 `"empty join group"`；`makeBushyJoin` 的共享实现也会拒绝空分量列表，但正常 `solve` 已在更早处拦截。
- 超过 `usize::BITS - 1` 个节点返回 `"join group is too large for DP bitmap"`。该限制按整个组而非单个连通分量检查，且仍不能避免实际指数资源消耗；可用规模主要依赖上游 `dpThreshold`。
- 等值边列不存在、列集合为空或跨越多个 Join 节点时，列定位共享函数返回字符串错误，`solve` 原样传播。
- 一条等值边的两端落在同一节点时 Rust 当前静默丢弃；Go 当前实现将其视为内部错误。这是需要调用方和扩展者注意的兼容差异。
- `bfsGraph` 假定 `startNode` 及邻接表内下标均有效；这些值由 `solve` 自己生成时成立，直接公开调用若传非法下标会 panic。
- `dpGraph` 假定 `order` 非空、下标属于 `group` 且位移合法。若无法为全集生成计划，返回 `"DP could not build join tree"`，而不是解引用空结果。
- `newJoinWithEdge` 的返回类型虽为 `Result`，当前共享 `makeJoin` 本身不返回错误，因此该路径总是 `Ok`；未来若增加统计派生或表达式物化失败，应在这里保留错误传播。
- 浮点 `row_count` 没有显式验证。若出现 NaN，`join.row_count < old.row_count` 为假，候选选择取决于先后顺序；测试只覆盖最终值为有限正数的正常输入。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄。BFS 队列、邻接表、分量向量和 DP `HashMap` 均由一次同步调用拥有，函数退出时自动释放。

求解器要求 `&mut self` 的方法会修改 `base`（计划 id 分配与其它条件集合），因此同一个实例不能在没有外部同步的情况下并发求解，也不应假定可无状态复用。`JoinPlan` 候选以深克隆方式保存在 DP 表中，指数枚举可能带来明显的瞬时内存与克隆成本；这也是上游必须保持较小 DP 阈值的资源约束。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/rule_join_reorder_dp.go`，Go 测试是 `pkg/planner/core/rule_join_reorder_dp_test.go`。两版都遵循“等值边建图 → BFS 拆连通分量 → 分量内子集 DP → 分量间 bushy 笛卡尔连接”的主结构，也都通过 `sub`/补集枚举并去除左右镜像。Rust 测试 `dp_reorder_builds_bushy_tree_for_four_cartesian_nodes` 对应 Go `TestDPReorderAllCartesian`，验证四个孤立节点形成左右两侧均为 Join 的灌木树。

当前 Rust 并非 Go 实现的完整等价移植，主要差异如下：

- Go 在 DP 前调用 `generateJoinOrderNode` 派生统计，并用 `calcJoinCumCost` 比较累计代价；Rust 直接比较候选输出 `row_count`。
- Go `joinGroupEqEdge` 支持表达式边，并通过 `AlignJoinEdgeArgs`/`buildJoinEdge` 对齐、物化复杂表达式；Rust `JoinEdge` 只含左右列下标和 `null_equal`。
- Go 为非等值条件建立 `joinGroupNonEqEdge` 位图，在首次跨越左右两侧且列已覆盖时附着；Rust 虽声明 `joinGroupNonEqEdge`，但 DP 不使用它。共享 `makeJoin` 只基于简化 `Expression.column` 消费 `base.otherConds`。
- Go 会对空列列表、同节点等值边、无法对齐边等情况生成带栈的 planner 内部错误；Rust 使用 `Result<T, String>`，且同节点边被忽略。
- Go `newJoinWithEdge` 会递归派生 Join 统计并传播错误；Rust 的行数由 `rule_join_reorder.rs::estimate_join_rows` 以启发式公式同步计算。
- Go TPCH Q5 测试断言一个精确最优树；Rust 的对应测试使用较小四表链，只断言列/叶子集合完整及输出行数有限为正，尚未证明与 Go 精确树选择一致。

因此，当前文件可以作为 Rust 简化规划模型中的可运行 DP 求解器理解，但不能把 Go 已覆盖的完整表达式、统计代价和谓词安放能力直接归因于 Rust 版本。

## 扩展指南

- 若改变候选优劣标准，修改 `dpGraph` 的比较点，并优先复用 `baseSingleGroupJoinOrderSolver::calcJoinCumCost` 或新增明确的代价状态；同步在独立的 `rule_join_reorder_dp_test.rs` 增加能区分“最小输出行数”和“最小累计代价”的回归用例。
- 若补齐非等值条件语义，应把 `joinGroupNonEqEdge` 接入 `solve` 和 `dpGraph`：记录覆盖节点掩码、按连通分量重映射、只在条件首次同时跨越左右侧时附着，并保留跨分量条件到最终 bushy 阶段。Go 的 `solve`、`nodesAreConnected` 和 `makeBushyJoin` 是直接行为基准。
- 若支持复杂等值表达式，不能只扩大 `JoinEdge` 字段；还需在列归属、左右参数对齐、表达式注入、非确定函数安全性和统计派生之间保持一致，参考 Go `newJoinWithEdge` 及现有 Rust `rule_join_reorder.rs` 的表达式注入辅助逻辑。
- 若优化 `nodesAreConnected` 性能，可在 BFS 后预建“原节点下标 → 位号”映射，避免每条边重复线性搜索；必须保持分量外节点不匹配和双向边判断不变。
- 若调整位图或阈值，必须同时覆盖空组、单节点、全连通、多个连通分量、全笛卡尔、接近位宽上限和较大但合法输入的资源风险。
- Rust 单元测试必须继续放在独立 `pkg/planner/core/rule_join_reorder_dp_test.rs`，不要内嵌回生产源文件。任何对 Go 语义的移植应同步阅读并对齐 `rule_join_reorder_dp_test.go`，尤其是 `TestDPReorderTPCHQ5` 与 `TestDPReorderAllCartesian`。

## 验证依据

- 生产实现：`pkg/planner/core/rule_join_reorder_dp.rs`，逐项核对两个边类型、求解器、两个转发函数以及 `solve`、`bfsGraph`、`dpGraph`、`nodesAreConnected`、`newJoinWithEdge`、`makeBushyJoin`。
- 上游和共享实现：`pkg/planner/core/rule_join_reorder.rs`，核对 `JoinReOrderSolver::optimizeRecursive` 的 DP/贪心选择、`baseSingleGroupJoinOrderSolver::makeJoin`/`makeBushyJoin`、列定位、行数估计和累计代价辅助函数。
- crate 边界：`pkg/planner/core/Cargo.toml` 的 `[package]`、`[lib]` 和 `[package.metadata.porting]`；`pkg/planner/core/lib.rs` 的生产模块声明及独立测试挂载。
- Rust 测试：`pkg/planner/core/rule_join_reorder_dp_test.rs`，覆盖连通四表链、缺失列错误、无边两节点求解及四节点 bushy 笛卡尔树。
- Go 对照：`pkg/planner/core/rule_join_reorder_dp.go` 与 `pkg/planner/core/rule_join_reorder_dp_test.go`，核对完整 DP 公式、非等值边安放、表达式边物化、累计代价、TPCH Q5 精确树及全笛卡尔结果。
- RustCodeGraph：`status` 显示本地索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query joinReorderDPSolver` 命中 Rust/Go 类型及 Go 方法；`explore` 给出 Rust 文件内部调用边和上游 `optimizeRecursive`。文件过滤未命中与部分精确调用图查询无响应的限制，已由 `rg` 和直接源码证据补足。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的结构命令确认文件存在且恰有十一个固定二级标题，并人工复核所有“当前支持/尚未对齐”陈述均能回指上述符号或文件。
