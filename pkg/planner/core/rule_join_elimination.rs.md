# `pkg/planner/core/rule_join_elimination.rs`

## 文件定位

该文件属于 Cargo 包 `astersql-planner-core`（见 `pkg/planner/core/Cargo.toml`），由 `pkg/planner/core/lib.rs` 以公开模块 `rule_join_elimination` 导出。它在简化的 Rust `JoinPlan` 计划树上实现外连接消除规则：当外连接的内表不会影响父节点所需列，并且重复行不会改变结果时，用外表子计划替换该连接。

当前接线范围必须与 Go 主实现区分：Rust 仓库内对 `OuterJoinEliminator` 的直接使用仅见 `pkg/planner/core/rule_join_elimination_test.rs`；没有证据表明它已加入 Rust 优化器的规则序列。Go 对照实现则在 `pkg/planner/core/optimizer.go` 的 `optRuleList` 中注册 `&OuterJoinEliminator{}`，位于连接键类型改写之后、分区处理之前。

## 核心职责

- `tryToEliminateOuterJoin` 判断单个 `LeftOuter` 或 `RightOuter` 是否可消除。它先确定外表与内表，再检查父层是否引用内表列；随后允许两类消除：不引用内表参数的重复无关聚合，或内表连接键具有唯一性。
- `doOptimize` 自顶向下传播父节点所需列和聚合相关列，并递归改写 `Aggregation`、`Projection`、`Join` 三类节点。成功消除某个 Join 后，它继续递归处理返回的外表，以支持连续外连接消除。
- `extractInnerJoinKeys`、`isInnerJoinKeysContainUniqueKey`、`isInnerJoinKeysContainIndex` 和 `isSelectionPartitionedRowNumberWindowOneUnique` 提供连接键抽取与三条唯一性证据路径。
- `buildOuterJoinNullExtendedProjection` 可构造“保留外表、内表列填 `NULL`”的投影，但当前 Rust 消除主流程没有调用它；不能把它描述成已生效的零行内表处理。
- `appendUniqueCorrelatedCols`、`isColEqConst` 和 `_default_values` 也是当前文件内未被主流程调用的辅助函数，分别表达相关列去重、列等于常量和空默认值映射。

## 主要符号

- `pub struct OuterJoinEliminator`：无字段、可 `Default` 构造的规则对象；状态完全来自方法参数。
- `Optimize(plan) -> Result<(JoinPlan, bool)>`：公开入口，以根计划的 `schema` 作为初始父层所需列，并把空聚合列集合传给 `doOptimize`。
- `Name() -> &'static str`：返回规则名 `outer_join_eliminate`，与 Go 注册名一致。
- `doOptimize(plan, aggCols, parentCols)`：递归驱动器。返回重写后的计划和是否发生过消除。
- `tryToEliminateOuterJoin(plan, aggCols, parentCols)`：局部判定器。非 Join、非左右外连接、父层引用内表列、聚合参数引用内表列或无法证明唯一时返回 `(None, false)`；成功时返回外表克隆和 `true`。
- `extractInnerJoinKeys(conditions, innerChildIdx)`：从 `JoinEdge` 的左列或右列抽取内表连接键，并单独记录 `null_equal` 键。
- `isInnerJoinKeysContainUniqueKey`：只识别 `JoinNode::Leaf.unique_keys`，要求某一组唯一键的每列都在连接键中，且没有一列使用 NULL-safe 等值。
- `isInnerJoinKeysContainIndex`：当前仅转调唯一键判定，不读取独立索引元数据。
- `isSelectionPartitionedRowNumberWindowOneUnique`：识别 `Selection(Window)` 形态；窗口必须有 `row_number_column`，全部分区列必须包含在连接键中，并且窗口自带 `upper_bound == 1` 或过滤条件提供相同上界。
- `hasRowNumberUpperBoundOne`：用字符串编码识别行号列上的 `eq:1`、`le:1`、`lt:2`。
- `buildOuterJoinNullExtendedProjection`：按原输出 schema 生成 `Expression`；内表列名称写成 `null`，其余列写成 `col_<id>`，投影沿用外表 `id`、把 `row_count` 置为 `0.0`。

上述计划结构来自 `pkg/planner/core/rule_join_reorder.rs`：`JoinPlan` 保存 `id`、`JoinNode`、列编号 schema 与行数估计；`JoinEdge` 保存左右列和 `null_equal`。表达式及连接类型来自 `pkg/planner/core/task.rs`，其中 `Expression` 是名称/列号等字段组成的简化表示，`JoinType` 提供 `Inner`、左右外连接及半连接类型。

## 执行流程

1. `Optimize` 克隆根 `schema`，调用 `doOptimize(plan, &[], &schema)`。
2. `doOptimize` 遇到聚合时，把 `group_by` 作为下层 `aggCols`，并把聚合自身输出 schema 作为 `parentCols`；遇到投影时，从带 `column` 的表达式提取下层所需列。
3. 遇到 Join 时，先重建一个保持原 `id`、schema、行数和所有 Join 字段的候选计划，再调用 `tryToEliminateOuterJoin`。
4. 局部判定仅接受左右外连接。左外连接以内表为右孩子，右外连接以内表为左孩子；若 `parentCols` 中存在任何内表列，立即拒绝。
5. 若 `aggCols` 非空，且其参数均不来自内表，则直接返回外表；只要有一个参数来自内表就拒绝，不再尝试唯一性路径。
6. 无聚合列时，规则抽取内表等值连接键，并依次检查叶子唯一键、“索引”辅助方法和 `Selection(Window(row_number))` 模式。任一路径成立即返回外表。
7. 消除成功后，`doOptimize` 对返回的外表再次递归，因此嵌套的连续可消除外连接会被逐层去掉；当前层直接返回 `changed = true`。
8. 当前 Join 不能消除时，把其输出 schema、所有等值边两端列以及 `other_conditions` 中显式携带的列合并、排序、去重，作为两个孩子的 `parentCols`。这一不变量避免仅因某列不在 Join 输出中，就错误删除仍被本层连接谓词引用的子 Join。
9. 其它 `JoinNode` 变体不递归，原样保留。最后返回重建后的计划与累计变更标记。

## 数据与状态

该规则没有可变对象状态或全局缓存。列由 `usize` 编号表示；schema 是 `Vec<usize>`，临时成员关系使用 `HashSet<usize>`。`JoinPlan` 及其孩子通过值和 `Box` 所有权传递；消除判定接收借用，但成功结果会克隆外表计划。

唯一性信息来自 `JoinNode::Leaf.unique_keys: Vec<Vec<usize>>`。每个内层向量代表一组复合唯一键；连接键覆盖其中任意完整一组即可。`null_equal` 键被排除，是因为可空唯一列在 NULL-safe 等值下可能匹配多行，不能据此证明“每个外表行至多匹配一个内表行”。

窗口唯一性依赖 `Window.partition_by`、`row_number_column`、`upper_bound` 和外层 `Selection.conditions`。条件不是通用表达式求值，而是读取 `Expression.column` 和 `Expression.name` 的约定字符串，因此新增谓词表示时必须同步识别逻辑与测试。

## 依赖与调用关系

- 模块装配：`pkg/planner/core/lib.rs` 公开 `rule_join_elimination`，并在 `cfg(test)` 下装配独立的 `rule_join_elimination_test.rs`。
- 上游：仓库索引显示 Rust 文件由该独立测试使用；精确搜索未发现 Rust 生产规则列表构造 `OuterJoinEliminator`。因此目前可确认的 Rust 入口是直接调用 `Optimize`、`doOptimize` 或 `tryToEliminateOuterJoin`，而不是完整 SQL 规划主链。
- 下游：本文件直接依赖同 crate 的 `rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, Result}` 与 `task::{Expression, JoinType}`，以及标准库 `HashMap`/`HashSet`。这些都是 crate 内部依赖；`Cargo.toml` 没有为本文件声明专属 feature，默认 feature 为空，`nextgen` feature 也不条件编译本文件。
- 内部调用边：`Optimize -> doOptimize`；`doOptimize -> tryToEliminateOuterJoin` 且递归调用自身；`tryToEliminateOuterJoin -> extractInnerJoinKeys -> {isInnerJoinKeysContainUniqueKey, isInnerJoinKeysContainIndex, isSelectionPartitionedRowNumberWindowOneUnique}`；窗口判定再调用 `hasRowNumberUpperBoundOne`。
- Go 生产链：`pkg/planner/core/optimizer.go` 的 `optRuleList -> OuterJoinEliminator.Optimize -> doOptimize -> tryToEliminateOuterJoin`。这是 Go 版本在完整 SQL 逻辑优化中的实际位置，不能反推 Rust 已经接线。

## 错误处理与边界

`Result<T>` 是 `pkg/planner/core/rule_join_reorder.rs` 定义的 `std::result::Result<T, String>`。当前文件所有可失败方法都只返回 `Ok`，没有构造 `Err`；`?` 保留了未来唯一性元数据查询失败时的传播接口。普通“不适用”不是错误，而是 `(None, false)` 或 `Ok(false)`。

边界条件包括：非 Join 和非左右外连接不处理；父层仍需内表列时不处理；重复无关聚合的参数一旦涉及内表列就不处理；唯一键必须被完整覆盖；NULL-safe 键不能证明可空唯一列唯一；窗口模式只接受直接的 `Selection -> Window` 结构。空的 `partition_by` 会因 `all` 的空集语义通过分区覆盖检查，但仍需行号上界证据。

还需注意几个当前限制：`isInnerJoinKeysContainIndex` 没有独立索引语义；`buildOuterJoinNullExtendedProjection` 没有进入消除路径；`isColEqConst` 没有被 `hasRowNumberUpperBoundOne` 使用；`doOptimize` 不遍历 `Selection`、`Apply`、`Window`、`UnionAll` 的孩子。文档据此只陈述现有代码行为，不把 Go 的完整能力视作 Rust 已支持功能。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道、事务或外部资源。一次优化调用拥有输入 `JoinPlan`，递归过程中移动并重建节点；临时集合和列向量在栈帧结束时释放。规则对象本身无状态，因此代码层面不存在跨调用共享状态；但尚无生产接线或并发调用测试，不能据此宣称完整规划器环境中的并发行为已验证。

资源方面的主要成本是计划克隆：`tryToEliminateOuterJoin` 成功时克隆整个外表子树，Join 候选也克隆左右孩子和条件。深层计划还会使用递归栈。扩展时应关注大计划上的额外内存、克隆次数和递归深度。

## 与 Go 版本的对应关系

Rust 符号名称总体与 `pkg/planner/core/rule_join_elimination.go` 对齐，但数据模型与覆盖范围明显简化：

- 两边都支持左右外连接、父层内表列检查、重复无关聚合、唯一键/唯一索引思路，并返回规则名 `outer_join_eliminate`。
- Go 在内表为零行 `LogicalTableDual` 时调用 `buildOuterJoinNullExtendedProjection`，保留原输出 schema、类型和输出名；Rust 虽定义同名帮助函数，却没有零行节点判定或调用点，而且其表达式只用字符串 `null` 表示。
- Go 的唯一键检查覆盖 `PKOrUK` 与 `NullableUK`，索引检查遍历 `DataSource.AllPossibleAccessPaths` 并检查唯一索引、句柄路径、索引列及可空标志；Rust 只读取叶子的 `unique_keys`，所谓索引检查是同一逻辑的别名。
- Go 的窗口模式严格要求单个 `row_number` 函数、完整的 `ROWS CURRENT ROW ... CURRENT ROW` frame、单个窗口结果列，并用真实表达式分析 `= 1` 或上界；Rust 仅检查简化节点字段和谓词名称编码。
- Go 的递归覆盖 CTE、Apply、Projection、Aggregation、Join 及所有孩子；它还处理会话变量控制的相关子查询列、聚合参数及排序参数、Join 的左右/其它/NAEQ 条件。Rust 只特殊处理三类节点，Join 下游所需列只收集等值边与 `other_conditions` 的单一可选列。
- Go `Optimize` 的 `planChanged` 当前始终为 `false`，但返回重写计划；Rust 返回 `doOptimize` 的实际变更标记。调用方若需要跨语言一致的变更标志，应先明确契约再调整。

Go 回归 `pkg/planner/core/logical_plans_test.go::TestOuterJoinEliminator` 通过 SQL 与 golden 计划覆盖无列引用、左右外连接、`distinct`/重复无关聚合、复杂嵌套、`ORDER BY` 阻止消除和自然连接。对应输入/输出位于 `pkg/planner/core/testdata/plan_suite_unexported_{in,out}.json`。

## 扩展指南

- 扩充 Rust 生产能力前，先决定该规则应接入哪个 Rust 逻辑优化规则序列；仅公开模块或让单元测试通过不等同于进入 SQL 主链。接线后应增加独立集成测试证明真实入口调用它。
- 实现零行内表消除时，应在 `tryToEliminateOuterJoin` 中加入可证明的零行节点分支，再接入 `buildOuterJoinNullExtendedProjection`；同时验证输出列顺序、列类型可空性、计划 id、schema 与行数估计，不能直接沿用当前占位字段而不核对语义。
- 实现真实唯一索引判定时，应替换 `isInnerJoinKeysContainIndex` 的转调，根据 Rust 数据源访问路径模型核对唯一性、全部索引列、可空列及 NULL-safe 等值；保持复合键必须完整覆盖的不变量。
- 扩展窗口识别时，应修改 `isSelectionPartitionedRowNumberWindowOneUnique`、`hasRowNumberUpperBoundOne` 和必要的表达式模型，防止把其它窗口函数、宽松 frame 或不能证明上界的谓词误判为唯一。
- 扩展递归节点时，应修改 `doOptimize` 的列需求传播；尤其是 Join 新增条件类别、Apply 相关列、聚合函数参数和排序项时，必须把下层仍需使用的列纳入 `parentCols`。
- 测试逻辑应继续放在独立的 `pkg/planner/core/rule_join_elimination_test.rs`，不要嵌入生产文件。至少同步增加：左右外连接、父层引用内表、复合/可空唯一键、NULL-safe 等值、重复无关聚合引用内表、连续消除、Join 条件保活、窗口边界和零行内表测试；需要证明完整 SQL 行为时再扩展集成测试或 fixture。
- 兼容性风险主要是错误消除导致结果集行数或 NULL 语义变化；性能风险主要是遗漏消除以及计划克隆开销。任何放宽条件的修改都应先写一个修复前失败、修复后通过的回归测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust/Go 文件均可读取。
- RustCodeGraph 文件节点：`pkg/planner/core/rule_join_elimination.rs`（完整 281 行）、`pkg/planner/core/rule_join_reorder.rs`（`JoinEdge`、`JoinNode`、`JoinPlan`、`Result`）、`pkg/planner/core/task.rs`（`JoinType`、`Expression`）。
- RustCodeGraph 查询：`OuterJoinEliminator`、`tryToEliminateOuterJoin`、`JoinPlan`、`JoinNode`、`JoinEdge`、`JoinType`、`isSelectionPartitionedRowNumberWindowOneUnique`；索引的 `callers`/`callees` 命令未输出边，因此调用边又以文件节点和精确符号搜索交叉核对，没有据此虚构生产调用者。
- crate 与装配：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。
- Rust 独立测试：`pkg/planner/core/rule_join_elimination_test.rs` 覆盖聚合引用内表时拒绝、连续外连接消除、Join 条件列阻止错误消除和规则名。
- Go 对照与生产注册：`pkg/planner/core/rule_join_elimination.go`、`pkg/planner/core/optimizer.go`。
- Go 回归：`pkg/planner/core/logical_plans_test.go::TestOuterJoinEliminator` 及 `pkg/planner/core/testdata/plan_suite_unexported_in.json`、`plan_suite_unexported_out.json`；`pkg/planner/core/main_test.rs` 还验证该 fixture 分组存在且输入输出数量相等。
- 本任务是纯文档分析，按计划未运行 Cargo、Go 测试或代码构建；最终仅执行任务指定的 11 章节结构验证，并人工复核上述事实与当前实现一致。
