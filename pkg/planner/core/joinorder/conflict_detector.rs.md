# [`pkg/planner/core/joinorder/conflict_detector.rs`](./conflict_detector.rs)

## 文件定位

本文件是 `astersql-planner-core-joinorder` crate 的冲突检测与建边实现，由同目录 `lib.rs` 公开为 `conflict_detector` 模块。上游 `JoinOrder::optimize`（`join_order.rs`）先调用 `ConflictDetector::build` 把原始连接树拆成叶节点和连接边，再由 DP 或贪心枚举器反复调用 `check_connection`、`make_join`；最终用 `has_remaining_edges` 拒绝遗漏真实谓词边的结果。因此它位于“连接组抽取之后、候选连接树枚举之中”，负责保证外连接、半连接等非自由可交换连接的重排合法性。

文件第 16—594 行是整段注释化的早期移植草稿，不参与编译。当前可执行实现从 `use crate::util` 开始，使用同 crate 的 `PlanNode`/`Expr` 简化模型；说明中的行为均以第 595 行后的有效代码为准。

## 核心职责

1. `ConflictDetector::build` 自底向上扫描 `PlanNode` 连接树，把非 Join 节点变成枚举叶子，把每个 Join 变成一条或多条 `Edge`。
2. 内连接的每个等值/非等值合取项拆成独立边，扩大可重排空间；非内连接的全部谓词保留为单条原子边，避免拆分后改变语义（`build_recursive`）。
3. 为每条边计算 TES（Total Eligibility Set）并依据 `ASSOC_RULE_TABLE`、`LEFT_ASSCOM_RULE_TABLE`、`RIGHT_ASSCOM_RULE_TABLE` 生成 `Rule { from, to }`，约束不可结合/不可交换的候选组合。
4. `check_connection` 检查边是否未消费、TES 是否跨越候选两侧、冲突规则是否满足，并为非内连接恢复原始左右方向。
5. `make_join` 把所有适用边合成一个新 `PlanNode`，对齐等值表达式参数、传播 hint、估算行数和累计代价，并合并 `used_edges`。
6. `cartesian_join`、`has_remaining_edges` 和 `has_remaining_edges_in_subset` 分别支持枚举器的笛卡尔回退与完整性检查。

## 主要符号

- `Rule { from, to }`：蕴含约束；候选顶点并集只要触及 `from`，就必须完整包含 `to`。
- `Edge`：保存唯一 `index`、`join_type`、原始 `left`/`right` 顶点、谓词 `conditions`、`tes`、冲突 `rules` 以及 `has_equality`。边索引也是防重复消费的身份。
- `Node`：枚举状态，组合逻辑计划 `plan`、覆盖顶点 `vertexes`、已消费边 `used_edges` 和 `cumulative_cost`。`Node::leaf` 从叶计划初始化这些字段。
- `CheckConnectionResult`：一次候选连接的结果，包含定向后的左右节点、全部适用内连接边和至多一条非内连接边；`connected` 判断是否有边，`no_equality_edge` 判断是否需要笛卡尔代价惩罚。
- `ConflictDetector { edges }`：构图结果及枚举期查询入口；公开方法为 `build`、`check_connection`、`make_join`、`cartesian_join`、`has_remaining_edges`、`has_remaining_edges_in_subset`。
- `edge_applicable`：统一执行 TES、跨两侧、非内连接方向和冲突规则检查；无谓词边有避免额外集合分配的快速路径。
- `right_to_left_rule` / `left_to_right_rule`：从子边的原始两侧及 TES 派生蕴含规则。
- `join_type_index` 与三张规则表：把 `JoinType` 映射到五类规则矩阵；`RightOuter` 与 `FullOuter` 共用索引 2。

## 执行流程

1. `JoinOrder::optimize(root)` 调用 `ConflictDetector::build(&root)`。
2. `build_recursive` 遇到非 Join 即用 `Node::leaf` 记录叶子；遇到 Join 则先递归左右孩子，并要求两侧顶点集合不重叠。
3. 它收集谓词引用的叶 ID。内连接把每个谓词各建一条边，无谓词时仍建空条件边；非内连接把全部谓词聚合成一条边。TES 初始来自该边谓词的 `Expr::leaf_ids`，若没有触及原始某一侧，则补入该侧全部顶点，防止退化谓词过早连接无关子图。
4. 新边与左右子树边逐一查三张结合/交换规则表；性质不成立时加入相应方向的 `Rule`。边按追加顺序获得稳定 `index`。
5. DP 或贪心枚举器提交两个互不重叠的 `Node` 给 `check_connection`。方法跳过任一输入已消费的边，收集所有适用内连接边；非内连接同时试正反方向，只接受唯一方向和至多一条边，必要时交换结果左右节点。
6. `make_join` 让非内连接边排在首位，以其类型作为新 Join 类型，再汇总适用边谓词。标记为等值的表达式经 `align_join_edge_args` 对齐左右列；失败即返回错误，其他谓词原样归入 `other_conditions`。
7. 新节点合并列集合、子计划和 `used_edges`。内连接估计行数为左右行数乘积，存在等值条件时再乘 `0.1`；其他连接取左右估计行数最大值。累计代价为两个子节点累计代价加当前估计行数。
8. 没有合法边时，调用方可使用 `cartesian_join`，再在 `join_order.rs` 中通过 `apply_cartesian_factor` 放大代价。优化完成后 `has_remaining_edges` 确认所有含谓词边均已消费。

## 数据与状态

顶点集合、已用边集合、TES 和规则两侧均为有序的 `BTreeSet<usize>`，提供确定性遍历与集合运算；hint 映射为 `BTreeMap<usize, JoinMethodHint>`。`ConflictDetector::build` 创建全新检测器，构图后 `edges` 只读；枚举过程中状态随 `Node` 值传递，连接成功时合并两侧的 `used_edges` 并加入本次全部边索引。

重要不变量包括：Join 恰有两个孩子；左右候选顶点不重叠；每条边索引等于插入时 `edges.len()`；一条真实边最多消费一次；非内连接必须保持原始左右集合方向；冲突规则采用“触及 `from` 则完整包含 `to`”的蕴含语义；空条件边不计入“剩余真实边”。

成本允许正无穷（例如调用方用它压制笛卡尔方案），但拒绝 NaN、负无穷和负值。`cartesian_join` 还拒绝 NaN 或任意无穷的 `factor`；实际放大在 `join_order.rs` 完成，本方法只按传入因子计算自身代价。

## 依赖与调用关系

直接依赖只有标准库 `BTreeMap`/`BTreeSet` 和同 crate `util.rs` 导出的 `Expr`、`JoinMethodHint`、`JoinType`、`PlanKind`、`PlanNode`、`align_join_edge_args`、`set_new_join_with_hint`。`Cargo.toml` 将 crate 根设为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/planner/core/joinorder"` 标记 Go 对照包；清单中的跨 crate 依赖目前位于 Windows 条件节，而本文件有效实现没有直接导入它们。

主要上游是 `join_order.rs`：`JoinOrder::optimize` 构图；`optimize_dp`、`optimize_greedy`、`greedy_connect` 和 `make_bushy_cartesian` 调用连接检查、建 Join 或笛卡尔回退。下游为 `util.rs` 的表达式列归属与 hint 接线。独立测试入口由 `lib.rs` 的 `#[cfg(test)] mod conflict_detector_test`、`join_order_test` 和 `bitset_bench_test` 接入。

RustCodeGraph 的文件节点确认本文件已被 `lib.rs` 模块装配；精确源码追踪确认调用主链为 `JoinOrder::optimize -> ConflictDetector::build`，枚举循环再进入 `check_connection -> edge_applicable` 以及 `make_join -> align_join_edge_args/set_new_join_with_hint`。图工具对 Rust impl 方法的名称查询未返回独立方法节点，因此方法级调用边同时以 `join_order.rs` 直接调用点核验。

## 错误处理与边界

所有可失败入口返回 `Result<_, String>`。构图拒绝非二叉 Join 和左右顶点重叠；连接检查拒绝输入节点重叠、同一非内连接边正反方向均适用，以及一对节点同时命中多条非内连接边；建 Join 拒绝无边结果、等值条件无法匹配左右列，以及非法累计代价；笛卡尔连接拒绝非法因子和非法代价。

`build_recursive` 将所有非 Join 节点直接视为叶子，不像 Go 实现那样显式穿透并恢复 Selection 边，也没有 Go 版本的 `NAEQConditions`、NOT NULL 标志重写、可变副作用条件分类和真实 `LogicalPlan.RecursiveDeriveStats`。这些是当前简化模型的明确边界，不能仅凭文件前半部注释草稿宣称已经支持。

`check_connection` 返回“未连接”不等同于错误：可能确实受冲突规则禁止，也可能只是没有共享边。是否允许笛卡尔积由 `join_order.rs` 根据全内连接、枚举阶段和成本因子决定。无谓词边用于合法连接，但 `has_remaining_edges`/`has_remaining_edges_in_subset` 有意忽略它们。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。`ConflictDetector` 构建后由枚举器以共享不可变引用读取；候选 `Node`、`Edge` 和结果通过克隆或所有权移动隔离，因此没有内部可变共享状态。

生命周期是“每个连接组构建一次检测器和叶节点—枚举期间产生并丢弃候选节点—选出最终计划后整体释放”。主要资源风险来自 `BTreeSet` 集合运算、边/节点克隆和 DP 子集数量，而不是同步。大规模覆盖由 `bitset_bench_test.rs` 的 16/32/64/128 叶贪心收敛测试提供；当前实现相对 Go 的 `FastIntSet` 可能具有更高常数开销，扩展规则时应避免在热路径重复分配集合。

## 与 Go 版本的对应关系

同路径 `conflict_detector.go` 实现论文 CD-C 算法，是主要语义基准。Rust 保留了核心结构对应关系：`ConflictDetector`、边/节点/规则、内连接逐谓词拆边、非内连接原子边、退化谓词补全 TES、三张规则表、方向恢复、边消费和剩余边检查。`conflict_detector_test.rs` 验证左外连接即使反向提交也恢复原始左右顺序，以及空谓词边不被报告为剩余；`join_order_test.rs` 进一步验证内连接谓词成为独立边并全部消费、TES 按表达式引用而非整个原始侧计算，以及非等值边接受笛卡尔代价惩罚。

当前 Rust 不是 Go 类型系统的一比一移植：它使用自包含 `PlanNode`/`Expr`，边统一放在 `Vec<Edge>`，没有 `ctx`、`groupRoot`、`allInnerJoin`、Selection 穿透、NAEQ 拒绝、真实统计派生和 NOT NULL 对齐。Go 的 `TryCreateCartesianCheckResult` 只允许全内连接组创建回退边；Rust 则把 `cartesian_join` 暴露给枚举器，由 `join_order.rs` 的 `all_inner`/`allow_cartesian` 条件约束调用。Go 注释说 TES 尚未按冲突规则扩展，Rust 同样把规则保留为运行时检查，而不是物化进 TES。

规则类型也有映射差异：Go 当前不支持 Full Outer Join，而 Rust `JoinType` 含 `FullOuter` 并将其与 `RightOuter` 映射到同一个规则表索引；这只证明当前查表行为，不等于完成了完整 Full Outer 语义验证。

## 扩展指南

- 新增连接类型时，必须同时更新 `util.rs::JoinType`、`join_type_index` 和三张规则表，并为该类型与其他类型的结合/交换组合增加独立测试；错误映射会静默放宽或收紧合法搜索空间。
- 修改谓词拆分或 TES 时，优先改 `build_recursive`，并同步覆盖退化谓词、跨嵌套子树引用、空条件边、内连接多合取项和非内连接原子性。需与 Go `makeInnerEdge`、`makeNonInnerEdge`、`makeEdgeInternal` 的增量语义逐项核对。
- 修改边适用条件时，集中在 `edge_applicable` 和两个规则构造函数，并保留非内连接方向恢复、规则蕴含语义与空条件热路径。至少同步 `conflict_detector_test.rs` 和 `join_order_test.rs`，不要把测试内嵌进生产文件。
- 扩展真实表达式或计划能力时，应在 `util.rs` 的 `Expr`/`PlanNode` 边界接入；若要追平 Go 的 Selection、NAEQ、NOT NULL 或统计派生，应作为明确移植任务实现并增加对应测试，不能只启用前半段注释代码。
- 调整成本模型或笛卡尔策略时，需要同时审查 `make_join`、`cartesian_join` 与 `join_order.rs::apply_cartesian_factor`，防止重复放大、NaN 传播或误把合法正无穷当错误。
- 性能敏感修改应运行 `bitset_bench_test.rs` 覆盖的规模矩阵；`BTreeSet` 到位集合的替换会影响集合语义、确定性和克隆成本，必须保持结果一致。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含目标文件；`node --file pkg/planner/core/joinorder/conflict_detector.rs` 分段核对了 1—1129 行；`query ConflictDetector --kind struct` 定位有效结构体于第 687 行。自然语言 `explore` 同时确认 Go 的 `MakeJoin -> makeNonInnerJoin/makeInnerJoin -> newCartesianJoin` 对照流。方法名查询未形成独立节点，故没有据此臆造调用边。
- Rust 源与装配：`conflict_detector.rs`、`join_order.rs`、`util.rs`、`lib.rs`；其中 `JoinOrder::optimize`、DP、贪心和 bushy 回退是直接调用证据。
- crate 边界：`pkg/planner/core/joinorder/Cargo.toml` 的 package、lib path、porting metadata 与条件依赖。
- Go 对照：`pkg/planner/core/joinorder/conflict_detector.go`、`join_order.go`；核对 CD-C 概念、构图、规则表、连接适用、计划构造和剩余边语义。
- 测试证据：`conflict_detector_test.rs`、`join_order_test.rs`、`bitset_bench_test.rs`，分别覆盖方向恢复/空谓词边、谓词拆边/TES/成本，以及 DP 与贪心在多规模下完整覆盖顶点。
- 本任务是纯文档分析，按计划未运行 Cargo；结构验证用于确认目标文件存在且恰有十一项规定章节，人工复核用于确认没有把注释草稿或 Go 独有能力写成当前 Rust 已实现行为。
