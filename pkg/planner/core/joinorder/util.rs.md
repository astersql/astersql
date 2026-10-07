# `pkg/planner/core/joinorder/util.rs`

## 文件定位

本文件是 `astersql-planner-core-joinorder` crate 的共享模型与辅助算法层，由同目录 [`lib.rs`](lib.rs) 以 `pub mod util` 暴露。crate 边界见 [`Cargo.toml`](Cargo.toml)：它对应 Go 包 `pkg/planner/core/joinorder`，当前活跃 Rust 实现只依赖标准库的 `BTreeMap`、`BTreeSet`；Cargo 中列出的 TiDB 风格内部依赖均位于 `cfg(windows)` 目标段，不能据此声称当前自包含模型已经接入完整 planner 对象体系。

源码第 22—290 行是被块注释包围的迁移设计稿，不参与编译。真正的生产代码从 `use std::collections::{BTreeMap, BTreeSet};` 开始，向 `conflict_detector.rs`、`join_order.rs`、`ordered_leading.rs` 提供连接类型、表达式、计划节点、hint 及若干构造/规范化函数。完整优化入口位于 `JoinOrder::optimize`，本文件本身不负责枚举连接顺序。

## 核心职责

1. 用 `JoinType`、`Expr`、`PlanKind`、`PlanNode` 建立连接重排所需的最小逻辑计划模型，并通过 `PlanNode::leaf`、`PlanNode::vertexes` 维护叶节点及代价基础数据。
2. 用 `JoinMethodHint` 表示顶点级连接算法偏好，用 `set_new_join_with_hint` 把两个直接孩子的偏好合并到新 Join。
3. 用 `LeadingTree` 与 `build_leading_tree_from_list` 表示并递归落实嵌套的 LEADING 顺序。
4. 提供表达式分析和改写：列/叶集合收集、确定性判断、派生列递归替换、外连接多叶过滤器检查。
5. 提供等值连接边拆解与左右 schema 对齐，供 `ConflictDetector::make_join` 在重建 Join 时规范化条件方向。

这些职责都服务于连接重排的中间表示；它们不直接读 SQL AST、session 变量或真实 `LogicalJoin`。这些完整 TiDB 对象只存在于同路径 Go 实现及 Rust 文件前部的非编译注释中。

## 主要符号

- `JoinType`：覆盖 `Inner`、左右/全外连接、半连接和反半连接。`ConflictDetector` 根据它推导边应用与冲突规则，`JoinOrder` 还用它判断是否允许笛卡尔积回退。
- `Expr`：简化表达式 AST。`Column` 同时保存列 `unique_id` 和来源 `leaf_id`；`Constant` 与 `Other` 保存确定性；`Eq`、`And` 形成递归树。`Other.columns` 能参与 `column_ids`，但其 `leaf_ids` 当前返回空集。
- `Expr::{column_ids, leaf_ids, deterministic}`：分别归并列 ID、叶 ID 和确定性。`Eq`/`And` 递归聚合，列永远视为确定性表达式。
- `JoinMethodHint`：四个布尔偏好位，分别表示 hash、merge、index、broadcast；允许同时为真，文件内不处理冲突优先级。
- `PlanKind`：`Table` 保存库表名和索引列前缀；`Join` 保存连接类型、等值/其它条件及算法 hint。
- `PlanNode`：保存节点 ID、节点种类、孩子、输出列集、估算行数和累计代价。`leaf` 将行数和初始代价下限钳制为 `1.0`；`vertexes` 以“无孩子”为叶判据递归收集 ID。
- `LeadingTree`：`Table(String)` 或二叉 `Join`，是活跃 Rust 实现采用的 LEADING 表示。
- `build_leading_tree_from_list`：泛型递归构树；由调用者注入表查找/移除闭包和 Join 合并闭包。
- `substitute_columns`：按 UniqueID 递归替换列，沿替换链继续展开，并以 `visiting` 集合阻断循环映射。
- `outer_join_side_filters_touch_multiple_leaves`：汇总过滤器的叶 ID，超过一个不同叶时返回 `true`。
- `get_eq_edge_args_and_columns` / `align_join_edge_args`：前者只接受 `Expr::Eq`，后者依据左右列集合是否分别为 schema 子集来保持或交换参数。
- `set_new_join_with_hint`：只查新 Join 的前两个直接孩子 ID，将命中的布尔 hint 以 OR 合并到 Join hint。

## 执行流程

主链由相邻模块驱动：

1. `JoinOrder::optimize` 调用 `ConflictDetector::build`，把原始 `PlanNode` Join 树拆为叶 `Node` 与约束边。
2. `ConflictDetector::build_recursive` 使用 `Expr::leaf_ids` 计算谓词涉及的顶点和边的 Total Eligibility Set；内连接谓词拆成可独立消费的边，非内连接保持原子性。
3. DP 或贪心枚举调用 `ConflictDetector::check_connection` 选择合法边，再进入 `ConflictDetector::make_join`。
4. `make_join` 对标记为等值的条件调用 `align_join_edge_args`。条件两侧分别落入左右输出列集时保持原方向，反向落入时交换；无法归属两侧则返回错误。
5. 新 `PlanNode` 构造完成后，`make_join` 调用 `set_new_join_with_hint`，只从两个直接孩子 ID 恢复顶点 hint；随后合并已消费边与累计代价。
6. `ordered_leading.rs` 复用 `Expr`、`PlanKind`、`PlanNode` 来收集叶、等价列和确定性固定列，并选择可满足排序前缀的叶表。

独立辅助流程中，`build_leading_tree_from_list` 对 `LeadingTree::Table` 调用 `finder`；对 `LeadingTree::Join` 先用原列表构建左树，再用左树返回的剩余列表构建右树，最后调用 `joiner(left, right)`。任一查找或合并错误通过 `Result<_, String>` 返回。`substitute_columns` 则深度优先遍历 `Column`、`Eq`、`And`；遇到映射列先登记正在访问的 ID，递归展开映射值，返回前再移除该 ID。

## 数据与状态

所有状态均由调用者拥有并以值或共享引用传入，本文件没有全局可变状态。

- `BTreeSet` 使列 ID、叶 ID 和顶点集合去重且迭代顺序稳定；`BTreeMap` 用于确定性的 UniqueID 替换及顶点 hint 查找。
- `PlanNode.children` 承载树所有权；`vertexes` 每次递归重新计算集合，不缓存结果。
- `estimated_rows` 表示当前节点估算行数，`cumulative_cost` 表示含子树的累计代价。只有 `PlanNode::leaf` 在本文件中初始化并钳制它们，Join 代价由 `ConflictDetector::make_join` 计算。
- `Expr::Other` 的 `columns` 与 `leaf_ids` 信息并不对称：它能报告列 UniqueID，但不能报告来源叶。新增依赖叶判断的谓词形态时必须补充来源表达能力，否则多叶检测与 TES 可能低估依赖。
- `JoinMethodHint` 是可组合布尔位而非单选枚举；`set_new_join_with_hint` 对已有 hint 和两个孩子 hint 只做 OR，不清空旧值。
- `substitute_columns` 的 `visiting` 只在一次递归调用链内存在。循环映射命中时保留当前列作为安全不动点，避免无限递归。

## 依赖与调用关系

上游活跃调用关系由 RustCodeGraph、源码符号查询及同目录引用交叉核验：

- `conflict_detector.rs` 导入 `Expr`、`JoinMethodHint`、`JoinType`、`PlanKind`、`PlanNode`、`align_join_edge_args`、`set_new_join_with_hint`；其中 `ConflictDetector::make_join` 是后两个函数的直接生产调用者。
- `join_order.rs` 导入 `JoinMethodHint` 与 `PlanNode`；`JoinOrder::optimize` 以它们作为优化输入与 hint 状态，DP/贪心再通过 `ConflictDetector` 间接使用表达式对齐和 hint 合并。
- `ordered_leading.rs` 导入 `Expr`、`PlanKind`、`PlanNode`，用于索引前缀、等价列和固定列推理。
- `join_order_test.rs` 直接覆盖 `substitute_columns` 的链式替换，并广泛用 `Expr`/`PlanNode` 构造优化回归；`util_test.rs` 直接覆盖外连接过滤器的单叶/多叶判定。

`build_leading_tree_from_list` 和 `LeadingTree` 在当前 Rust crate 中没有找到活跃调用者；它们是公开但尚未接到当前 `JoinOrder::optimize` 主链的辅助 API。相对地，Go 的 `BuildLeadingTreeFromList` 被 `join_order.go` 与 `rule_join_reorder.go` 直接调用。当前 Rust 调用关系不能从 Go 接线反推为已经存在。

## 错误处理与边界

- `build_leading_tree_from_list` 对表不存在返回 `Err("leading table ... not found")`，对 `joiner` 失败原样传播字符串错误。它没有 Go 版本的 `(ok=false, 原 availableGroups)` 软失败语义：左侧成功消费而右侧失败时，虽然切片输入本身不被修改，但返回值只保留错误，不返回原可用组。
- `get_eq_edge_args_and_columns` 对任何非 `Expr::Eq` 返回 `None`；空列集合本身并不使等式无效。
- `align_join_edge_args` 使用集合子集判断。当两侧表达式都不含列时，正向分支会成立；当列跨越 schema、schema 信息不全或表达式不是等式时返回 `None`。生产调用者将此转为 `eq condition does not match join sides`。
- `outer_join_side_filters_touch_multiple_leaves` 对空条件、常量及无法报告叶 ID 的 `Other` 返回 `false`；它只根据表达式携带的 `leaf_id`，不执行 Go 版本的派生列替换或真实 schema 查找。
- `PlanNode::leaf` 对 `NaN` 的 `rows.max(1.0)` 结果取决于 Rust 浮点 `max` 语义，本函数没有单独拒绝非法统计值；Join 构造阶段另由 `ConflictDetector` 检查负数、NaN 和负无穷累计代价。
- `set_new_join_with_hint` 在非 Join 节点上静默不做修改；孩子少于两个时只处理现有孩子，孩子多于两个时忽略第三个以后。主链的 `ConflictDetector::build_recursive` 会拒绝非二叉 Join，但本函数自身不验证该不变量。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源，因此没有跨任务资源清理流程。所有递归函数均同步完成，临时 `BTreeSet`/`Vec`/克隆值在调用栈退出时释放。

主要资源风险是计算与克隆成本：`Expr::{column_ids, leaf_ids}` 每层构造并合并集合；`PlanNode::vertexes` 递归重算；`align_join_edge_args` 返回完整表达式克隆；`build_leading_tree_from_list` 要求 `T: Clone`，具体复制量由 `finder`/`joiner` 决定。`substitute_columns` 的循环保护保证恶意循环映射不会耗尽调用栈，但极深的无环表达式或替换链仍使用递归栈。

## 与 Go 版本的对应关系

同路径 [`util.go`](util.go) 是语义参照，但两者不是逐类型可替换实现：

- Rust `JoinMethodHint` 用四个布尔位；Go 用 `PreferJoinMethod uint` 加 `*hint.PlanHints`，还能保留 hint 来源信息。Rust 合并只能表达算法偏好，无法复现 Go 的 `HintInfo` 覆盖与 `SetPreferredJoinType()` 归一化。
- Rust `LeadingTree` 是名称二叉树；Go `BuildLeadingTreeFromList` 直接消费 parser 的嵌套 `ast.LeadingList`，支持 warning、`ok` 软失败和失败时返回原列表。Rust 当前 API 只保留递归消费/合并的核心形状。
- Rust `Expr`/`PlanNode` 是自包含值模型；Go 使用 `expression.Expression`、`base.LogicalPlan`、`logicalop.LogicalJoin`、session query-block alias 等真实 planner 状态。
- Rust `substitute_columns` 与 Go `SubstituteColsInExpr` 都沿替换链递归；Rust额外显式阻断循环。Go 采用 ScalarFunction 写时复制并清理 canonical hash 缓存，Rust表达式没有该缓存。
- Rust 多叶过滤器只接收已解析的 `&[Expr]` 并汇总 `leaf_id`；Go 同时检查 Other/左右侧/等值条件，先替换投影派生列，再逐个真实叶 schema 判定。因此 Rust 函数是窄化后的判定核心，不能视为 Go API 的完整移植。
- Rust 等值边只接受 `Expr::Eq`；Go `GetEqEdgeArgsAndCols` 接受任意恰有两个参数的 `ScalarFunction`。两者的对齐原则相同，但 Go 另行返回 `swapped` 标记。
- Rust 文件活跃区没有 Go 的 `CheckAndGenerateLeadingHint`、`FindAndRemovePlanByAstHint`、`IsDerivedTableInLeadingHint` 等 session/AST 辅助逻辑；同名草稿只存在于注释块，属于未接线迁移信息。

## 扩展指南

- 新增表达式变体时，必须同步审查 `column_ids`、`leaf_ids`、`deterministic`、`substitute_columns::rewrite`，以及 `ordered_leading.rs` 的等值/固定列收集。遗漏任一处会造成 TES、外连接安全判断或有序索引选择不一致。
- 修改 Join 结构时，保持“恰有两个孩子”的主链不变量，并同步检查 `PlanNode::vertexes`、`ConflictDetector::build_recursive`、`set_new_join_with_hint`。若要支持多叉节点，应先定义 hint 来源与边方向，而不能只放宽孩子数量。
- 扩充 hint 时，应在 `JoinMethodHint`、`PlanKind::Join`、`set_new_join_with_hint`、`ordered_leading::try_annotate_ordered_leading` 和相关独立测试中一起处理冲突与优先级。需要完整 Go parity 时还必须设计 `HintInfo` 来源语义。
- 接线 `build_leading_tree_from_list` 前，应明确采用 Rust 的硬错误契约还是 Go 的软失败/回滚契约，并为嵌套成功、表缺失、joiner 拒绝、重复表、剩余节点顺序增加独立测试。
- 修改列替换时，保留循环映射终止性；若加入函数缓存或共享节点，需要像 Go 一样处理写时复制和缓存失效。
- 相关测试应继续放在独立文件：直接工具回归放 [`util_test.rs`](util_test.rs)，跨 `JoinOrder`/`ConflictDetector` 的流程回归放 [`join_order_test.rs`](join_order_test.rs) 或 [`conflict_detector_test.rs`](conflict_detector_test.rs)，不要把测试内嵌进生产源文件。
- 兼容风险主要来自 Rust 简化模型与 Go 真实 planner 对象的差距；性能风险主要来自递归集合分配和深克隆。扩展后应分别验证 Go 对应边界与大连接组上的集合/克隆开销。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 源与测试均已索引。
- RustCodeGraph 源码/符号查询：`node --file pkg/planner/core/joinorder/util.rs`（完整 1—556 行），以及对 `build_leading_tree_from_list`、`substitute_columns`、`outer_join_side_filters_touch_multiple_leaves`、`align_join_edge_args` 的 `query`。`callers` 查询没有产出可用结果，因此调用边又以同目录精确符号引用核验，没有据此臆造图边。
- 已读生产路径：[`util.rs`](util.rs)、[`conflict_detector.rs`](conflict_detector.rs)、[`join_order.rs`](join_order.rs)、[`ordered_leading.rs`](ordered_leading.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。当前目录无 `doc.go`；最近的 `pkg/planner/core/base/doc.go` 属于另一个子包，不作为本 crate 合同依据。
- Go 对照：[`util.go`](util.go)；并通过 `join_order.go`、`pkg/planner/core/rule_join_reorder.go`、`rule_join_reorder_dp.go`、`rule_join_reorder_projection_inline.go` 的精确调用位置核对 Go 主链接线。
- 已读 Rust 测试：[`util_test.rs`](util_test.rs)、[`join_order_test.rs`](join_order_test.rs)、[`ordered_leading_test.rs`](ordered_leading_test.rs)。直接证据包括多叶过滤器真/假两例、列替换链 `1 -> 2 -> 3`、hint 冲突不覆盖以及 Join 优化完整消费边。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前另以规定命令检查目标文件存在且固定二级标题恰好为 11 个，并人工复核唯一生产物、源码链接和未接线能力表述。
