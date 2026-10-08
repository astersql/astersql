# `pkg/planner/core/rule_join_reorder_greedy.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate 的经典 Join Reorder 路径。crate 由 `pkg/planner/core/Cargo.toml` 定义，库入口是 `pkg/planner/core/lib.rs`；后者以 `pub mod rule_join_reorder_greedy` 暴露本模块，并仅在 `cfg(test)` 下装配独立测试 `rule_join_reorder_greedy_test.rs`。

生产入口位于 `pkg/planner/core/rule_join_reorder.rs` 的 `JoinReOrderSolver::optimizeRecursive`：它先抽取连续 Inner Join 组，在组内节点数大于 `dpThreshold.max(2)` 时构造 `joinReorderGreedySolver` 并调用 `solve`；较小的组走 `joinReorderDPSolver`。因此，本文件负责大连接组的近似顺序搜索，不负责 Join 组抽取、DP 枚举或最终 schema 恢复。

## 核心职责

- 将 `joinNodePlans` 转为带累计代价的 `jrNode`，按初始累计代价升序选择每个连通分量的起点（`solve`、`baseSingleGroupJoinOrderSolver::generateJoinOrderNode`）。
- 从当前树可连接的候选中，反复选择合并后累计代价最小者，形成一个连通 Join 树（`construct_connected_join_tree`）。
- 连通分量之间没有可用连接条件时，不在贪心循环中强行连接，而是把每个分量交给 `makeBushyJoin`，最终以 bushy 笛卡尔 Join 合并（`solve`）。
- 提供两个公开的兼容/测试入口：`constructConnectedJoinTree` 从完整输入生成一个连通分量；`checkConnectionAndMakeJoin` 判断两个计划能否非笛卡尔地合并。仓库 Rust 调用搜索显示，这两个入口目前没有目标文件之外的生产调用者。

这里的“代价”不是物理算子成本。`calcJoinCumCost` 在 `rule_join_reorder.rs` 中定义为左右累计代价加本次 Join 的估计行数（且至少按 1 计），所以它是基于基数的启发式指标。

## 主要符号

- `pub struct joinReorderGreedySolver`：求解器状态。`base: baseSingleGroupJoinOrderSolver` 持有等值边、其它条件、Join 类型和列 ID 分配器；`joinNodePlans: Vec<JoinPlan>` 是待重排叶子或不可继续展开的子树。类型派生 `Clone`、`Debug`、`Default`。
- `pub fn solve(&mut self) -> Result<JoinPlan>`：完整求解入口。空组返回 `"empty join group"`；非空组被拆成连通分量，最后由 `makeBushyJoin` 汇合。
- `fn construct_connected_join_tree(&mut self, nodes: &mut Vec<jrNode>) -> Result<jrNode>`：核心贪心循环。它从已排序列表头部取起点，枚举剩余节点，试建候选 Join 并保留最低累计代价者。
- `pub fn constructConnectedJoinTree(&mut self) -> Result<jrNode>`：重新从 `joinNodePlans` 生成并排序节点，然后只构造一个连通分量。它不返回未消费节点，当前只适合兼容接口或聚焦测试，不能替代 `solve` 完成多分量求解。
- `pub fn checkConnectionAndMakeJoin(&mut self, left, right) -> (Option<JoinPlan>, Vec<Expression>, bool)`：检查等值边或其它 Join 条件；若两侧完全不连通，返回 `(None, [], true)`，否则调用共享基类的 `makeJoin`，返回计划、剩余条件和 `false`。

命名保留了 Go 移植风格（例如类型名和公开方法不是 Rust 惯用的 CamelCase/snake_case），调用或扩展时应优先保持现有跨语言映射，除非进行有完整调用面验证的统一重命名。

## 执行流程

1. `JoinReOrderSolver::optimizeRecursive` 抽取 Join 组，并在节点数超过阈值时把 `eqEdges`、`otherConds`、`joinTypes` 和计划列表交给本求解器。
2. `solve` 调用 `generateJoinOrderNode`。每个叶子节点的 `cumCost` 是该计划及其全部后代的 `row_count` 总和；随后使用 `f64::total_cmp` 升序排序，避免普通浮点偏序无法覆盖 NaN 的问题。
3. 只要排序后的 `nodes` 非空，`solve` 就调用 `construct_connected_join_tree`。该函数移除最低初始代价节点作为 `current`。
4. 对每个剩余节点，函数克隆 `self.base` 为 `candidate_base`，再调用 `checkConnection`。如果既无等值边、又无 `hasOtherJoinCondition` 认可的其它条件，则视为笛卡尔候选并跳过。
5. 对可连接候选调用 `candidate_base.makeJoin`。该调用可能消费 `otherConds` 并推进 `columnAllocator`，所以候选必须在独立 clone 上试算；随后用 `calcJoinCumCost` 计算新树代价。
6. 一轮枚举结束后，若有候选，就从 `nodes` 删除获胜节点，把 `self.base` 替换为获胜候选的状态，并令新 Join 成为 `current`；若没有候选，则当前连通分量完成。
7. `solve` 收集每个连通分量的计划；全部节点消费完毕后调用 `makeBushyJoin`，按轮次两两创建无等值边 Join，直到得到单一计划树。

相同代价不会替换先发现的候选，因为选择条件是严格的 `cost < old.cumCost`。配合稳定的初始排序，这使并列情况下的选择受原输入顺序约束，扩展代价规则时应保留或明确改变这一确定性。

## 数据与状态

`joinNodePlans` 在 `solve` 中只被借用并克隆为工作节点，原列表不会被消费。实际变化集中在 `base`：`makeJoin` 会从 `otherConds` 中吸收当前两侧可用的条件，把未用条件写回，并通过 `columnAllocator` 分配新 Join ID。核心循环只提交获胜候选的 `candidate_base`，未获胜候选的条件消费与 ID 分配随 clone 丢弃，这是避免枚举顺序污染最终状态的关键不变量。

`jrNode` 同时携带 `JoinPlan` 和 `cumCost`。`JoinPlan` 包含节点 ID、算子树、输出 schema 与 `row_count`；本文件不直接估算行数，而由 `makeJoin` 调用 `rule_join_reorder.rs::estimate_join_rows`，再由 `calcJoinCumCost` 聚合。

连接性由两类状态判断：`eqEdges` 中跨越左右 schema 的边，或 `otherConds` 中被 `hasOtherJoinCondition` 识别的列引用。当前基类对“其它条件”的判断只检查表达式的单个可选 `column` 是否落在任一侧，这是一种简化表示；不能据此推断已经覆盖 Go 表达式系统的全部列依赖语义。

## 依赖与调用关系

上游链路为：`JoinReOrderSolver::Optimize` → `optimizeRecursive` →（大组分支）`joinReorderGreedySolver::solve`。模块由 `pkg/planner/core/lib.rs` 公开装配，相关数据结构来自同 crate 的 `rule_join_reorder` 与 `task` 模块。

主要下游调用为：

- `generateJoinOrderNode`：生成叶子累计代价；
- `checkConnection`、`hasOtherJoinCondition`：判定候选连接性；
- `makeJoin`：创建 Join、吸收条件、合并 schema、估计行数并推进 ID；
- `calcJoinCumCost`：计算贪心比较指标；
- `makeBushyJoin`：合并互不连通的最终分量。

RustCodeGraph 对 `solve` 的被调用函数给出了上述边，对 `constructConnectedJoinTree` 和 `checkConnectionAndMakeJoin` 也分别解析到对应基类方法；调用者查询未直接解析出 Rust 方法边，因此又以仓库符号搜索核验了唯一生产构造点位于 `rule_join_reorder.rs`。`Cargo.toml` 没有为本文件声明专属 feature；`nextgen` feature 只转发配置依赖，当前模块本身没有条件编译分支，也没有直接使用外部 crate。

## 错误处理与边界

- `solve` 和 `constructConnectedJoinTree` 都显式拒绝空输入，错误文本为 `empty join group`。
- 正常的“当前分量已无可连接节点”不是错误：内部函数返回当前树，外层继续处理下一个分量。
- 最后的 `makeBushyJoin` 理论上总会收到至少一个分量；其自身仍防御空输入并返回 `empty cartesian join group`。
- 浮点比较使用 `total_cmp` 排序，但候选比较仍使用 `<`。若估算产生 NaN，排序位置是确定的，而 NaN 候选不会优于已记录候选；当前 `estimate_join_rows` 基于 `max`、乘法和开方，调用者仍应避免向计划注入无意义的非有限基数。
- `checkConnectionAndMakeJoin` 的第三个返回值表示“检测到笛卡尔关系”，即使对应计划为 `None`；它与 Go 版本在禁用笛卡尔 Join 时返回的布尔语义并不完全相同，跨语言调用约定不能混用。
- 本文件没有兜底恢复原计划。上层通过 `?` 传播错误；schema 恢复由 `restoreSchemaIfChanged` 在重排成功后处理。

## 并发与资源生命周期

求解完全同步，使用普通 `Vec`、拥有所有权的计划树和短生命周期 clone；没有线程、异步任务、锁、通道、I/O 或事务。`&mut self` 保证同一个求解器实例不能被并发修改。

候选求值的资源边界是每次循环创建的 `candidate_base`：失败或落选即在该次迭代后释放，获胜时整体移动到 `self.base`。计划节点通过 `Box<JoinPlan>` 形成拥有所有权的树，删除 `nodes` 中的获胜元素后不会留下共享可变别名。大连接组的主要资源风险是枚举过程反复 clone 基类状态和计划树，时间近似二次增长，且 clone 成本随条件/计划大小增加。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/rule_join_reorder_greedy.go`。Rust 保留了 `joinReorderGreedySolver`、`solve`、`constructConnectedJoinTree`、`checkConnectionAndMakeJoin` 以及“累计代价最小候选 + 最终 bushy 笛卡尔合并”的主干算法，但当前只实现了 Go 行为的一个简化子集。

主要差异如下：

- Go 的 `solve` 接收计划切片，并维护 `curJoinGroup`、leading hint 分组及不适用警告；Rust 把计划存为字段，没有 leading hint/session warning 逻辑。
- Go 会读取 `CartesianJoinOrderThreshold`，允许带惩罚比率的笛卡尔候选，并记录相关优化变量；Rust 注释明确按默认阈值 0 处理，在构造连通分量时一律跳过笛卡尔候选。
- Go 用 `allInnerJoin` 禁止不安全的外连接笛卡尔重排；Rust 上游目前只抽取连续 Inner Join，求解器结构没有该字段。未来若扩大抽取范围，必须先补齐外连接语义约束，不能直接复用现有贪心循环。
- Go 对候选调用 `RecursiveDeriveStats`，可能返回统计推导错误；Rust 使用本地 `estimate_join_rows`，没有对应统计递归及错误面。
- Go 在异常的“存在可用 Join 却未选中”分支包含测试断言和生产兜底；Rust 通过隔离候选状态和简单选择逻辑没有对应分支。
- Rust 对每个候选 clone `base`，仅提交获胜状态，显式防止 `makeJoin` 移动 `otherConds` 污染后续枚举；这是当前 Rust 数据模型所需的状态隔离实现。

因此，Go 测试能说明目标语义和未来对齐方向，但不能作为 Rust 已支持会话变量、hint、外连接或完整统计派生的证据。

## 扩展指南

- 修改起点、候选选择或代价公式时，集中在 `solve`、`construct_connected_join_tree` 和 `rule_join_reorder.rs::calcJoinCumCost`；同步扩充独立测试 `pkg/planner/core/rule_join_reorder_greedy_test.rs`，不要把测试内嵌到生产文件。
- 增加会话可调的笛卡尔阈值时，需要把配置上下文显式接入求解器，并移植 Go 的笛卡尔/非笛卡尔惩罚比较、相关变量记录和边界测试；不能只删除当前的 `continue`，否则会改变外连接安全性和计划稳定性。
- 扩展到 outer join 或 leading hint 前，先补齐 `allInnerJoin`、Join 顺序约束、警告行为和不可应用 hint 的条件保留语义。Go 回归入口包括 `pkg/planner/core/casetest/rule/rule_join_reorder_test.go`；issue 63290 的阈值行为在 `pkg/planner/core/casetest/integration_test.go` 有直接计划断言。
- 若增强其它条件的连接性判断，应同步修改 `baseSingleGroupJoinOrderSolver::hasOtherJoinCondition`/`makeJoin`，并覆盖条件引用单侧、跨侧、无法下推以及候选落选后状态不被消费的场景。
- 性能修改要关注每轮对 `base` 和 `JoinPlan` 的 clone，以及从 `Vec` 中间删除的移动成本；任何缓存都必须维持“落选候选无副作用”和并列选择确定性。
- `constructConnectedJoinTree`、`checkConnectionAndMakeJoin` 虽公开但当前无生产调用者；改变签名之前仍须检查 crate 外部使用面，改变语义则至少增加直接单元测试。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件的 `node --file` 读取覆盖 117 行全文件。
- RustCodeGraph 符号/调用查询：`query joinReorderGreedySolver`、`query solve`、`query constructConnectedJoinTree`、`query checkConnectionAndMakeJoin`；`callees` 核对到 `generateJoinOrderNode`、`construct_connected_join_tree`、`makeBushyJoin`、`checkConnection`、`hasOtherJoinCondition` 和 `makeJoin`。图的 callers 结果为空，因此使用精确仓库搜索补证生产入口。
- 已读 Rust 生产代码：`pkg/planner/core/rule_join_reorder_greedy.rs`、`pkg/planner/core/rule_join_reorder.rs`、`pkg/planner/core/lib.rs`。目标目录根部没有 `doc.go`；最近检出的 `pkg/planner/core/base/doc.go` 属于 `base` 子包，不是当前 crate 根模块契约。
- 已读 crate 声明：`pkg/planner/core/Cargo.toml`，确认包名、`lib.rs` 入口、feature 和依赖边界。
- 已读独立 Rust 测试：`pkg/planner/core/rule_join_reorder_greedy_test.rs`。`disconnected_components_are_combined_as_a_bushy_cartesian_tree` 验证四个不连通节点最终形成两侧均为 Join 的 bushy 根；`connected_component_starts_with_lowest_cumulative_cost` 验证低累计代价节点先与其连通邻居组成子树。
- 已读 Go 对照与回归：`pkg/planner/core/rule_join_reorder_greedy.go`、`pkg/planner/core/rule_join_reorder.go`、`pkg/planner/core/casetest/rule/rule_join_reorder_test.go`，以及 `pkg/planner/core/casetest/integration_test.go` 中 issue 63290 的笛卡尔阈值计划断言。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核所有“当前支持”陈述均能回指上述符号或测试。
