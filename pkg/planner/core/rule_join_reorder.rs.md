# `pkg/planner/core/rule_join_reorder.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate。`pkg/planner/core/Cargo.toml` 将 `lib.rs` 定为 crate 根，`pkg/planner/core/lib.rs` 公开声明 `rule_join_reorder`，并同时公开相邻的 `rule_join_reorder_dp`、`rule_join_reorder_greedy` 和 `rule_join_reorder_projection_inline` 模块。文件定义一套自包含的简化逻辑计划模型，以及 Join 组抽取、共享建树能力和 DP/贪心选择入口；DP 与贪心算法的主体分别位于相邻文件。

RustCodeGraph 对 `JoinReOrderSolver::Optimize` 的查询只确认了本文件内 `Optimize -> optimizeRecursive`，没有发现它被 Rust 生产优化器管线调用。仓库中的直接使用者主要是 `pkg/planner/core/casetest/ch/ch_test.rs`、`pkg/planner/core/casetest/rule/rule_join_reorder_test.rs` 等测试，以及若干相邻 Rust 规则对 `JoinPlan`、`JoinNode`、`JoinEdge` 的复用。因此它是已公开、可独立执行和测试的移植模块，但不能据此认定它已经替代 `pkg/planner/core/rule_join_reorder.go` 在 Go 优化主链中的位置。

## 核心职责

1. 用 `JoinNode`/`JoinPlan` 表达本模块可处理的逻辑计划树，并以整数列号表示 schema。
2. `extractJoinGroup`/`extractJoinGroupImpl` 展开连续的 Inner Join，把不可继续展开的子树保留为原子叶子，同时收集等值边、其它条件、Join 类型扩展信息和原始 schema。
3. `JoinReOrderSolver::optimizeRecursive` 先自底向上优化所有子树，再按组内节点数和 `dpThreshold` 选择 DP 或贪心求解器，最后恢复原输出列序。
4. `baseSingleGroupJoinOrderSolver` 为两种求解器提供连接性检查、条件消费、表达式投影注入、提示传播、Join 构造、累计代价和笛卡尔分量拼接等共同操作。
5. `findNodeIndexInGroup`、`findNodeIndexForColumns` 和 `canReuseInjectedJoinExpr` 提供求解器需要的边界判定辅助函数。

本文件保证的是这套简化模型内的结构重排；它不负责 SQL 解析、真实统计信息派生、会话变量读取或物理执行。

## 主要符号

- `Result<T> = std::result::Result<T, String>`：模块统一错误类型，错误只携带文本。
- `JoinEdge { left_column, right_column, null_equal }`：列号级等值连接边；`null_equal` 被保存，但本文件的行数估计和连接性判断不区分其语义。
- `JoinNode`：计划节点枚举，覆盖 `Leaf`、`Join`、`Projection`、`Selection`、`Aggregation`、`Apply`、`Window`、`UnionAll`。它使递归遍历不依赖真实 planner trait。
- `JoinPlan { id, node, schema, row_count }`：节点身份、算子、输出列序和行数估计的载体；`columns` 生成集合，`contains_column` 做列归属判断。
- `basicJoinGroupInfo`：待求解叶子、等值边和其它条件；`joinGroupResult` 额外保存原 Join 类型信息及原始 schema。
- `joinTypeWithExtMsg`：保存 `JoinType`、偏好物理 Join 方法和原计划 id，供重建时恢复 Join 类型/提示。
- `jrNode`：把子计划和累计代价绑定，供贪心选择和链式拼接使用。
- `baseSingleGroupJoinOrderSolver`：可变共享状态。`eqEdges` 是候选连接边；`otherConds` 会在 `makeJoin` 中被消费；`joinTypes` 用于类型/提示；`columnAllocator` 同时分配注入列和新节点 id。
- `JoinReOrderSolver { dpThreshold }`：总控入口；`Name` 返回稳定注册名 `join_reorder`。
- 私有函数 `estimate_join_rows`：有连接边时使用 `sqrt(left*right)` 与较小输入行数的较大者，无边时使用笛卡尔积；`map_children` 对所有含子节点的枚举分支应用递归变换。

这些 Rust 名称保留了 Go 风格大小写（如 `Optimize`、`dpThreshold`、`makeJoin`），是当前公开 API 事实，不应在只做文档或小范围扩展时顺手重命名。

## 执行流程

`JoinReOrderSolver::Optimize(plan)` 的主流程如下：

1. 调用 `optimizeRecursive`。
2. `map_children` 先递归改写当前节点的每个子计划；任一子树返回错误则立即向上传播。
3. `extractJoinGroup` 从当前根向下展开连续 Inner Join。每遇到 Inner Join，先遍历左右子树，再收集其等值条件、其它条件和 `joinTypeWithExtMsg`；其它节点整体进入 `joinNodePlans`。
4. 若组内不足两个原子计划，返回当前计划和 `changed = false`。
5. 构造 `baseSingleGroupJoinOrderSolver`。节点数不超过 `max(dpThreshold, 2)` 时调用 `joinReorderDPSolver::solve`，否则调用 `joinReorderGreedySolver::solve`。
6. DP 求解器把列级边映射到节点，按等值边连通分量做 BFS 和位图 DP，再由 `makeBushyJoin` 连接互不连通的分量。贪心求解器按 `baseNodeCumCost` 排序，反复选择与当前树连通且累计代价最低的候选，最后同样用 bushy 笛卡尔 Join 合并分量。
7. `restoreSchemaIfChanged` 比较重排结果与 `originalSchema`；顺序或内容不同就包装 `Projection`，投影表达式按原列号生成。
8. 成功返回重排计划和 `changed = true`。此布尔值表示进入了重排路径，不表示树形或估算代价一定发生变化。

共享建树流程中，`checkConnection` 选择跨越左右 schema 的等值边；`makeJoin` 在没有显式边时再次检查连接，把当前左右列集合能够覆盖的 `otherConds` 移入新 Join，其余条件写回求解器状态，然后合并去重后的 schema 并调用 `estimate_join_rows`。

## 数据与状态

- `JoinPlan.schema` 是有序 `Vec<usize>`：顺序是外部可见契约，而 `columns()` 产生的 `HashSet` 只用于成员判断。`restoreSchemaIfChanged` 正是为避免重排泄漏列序变化。
- `row_count` 驱动所有代价选择。叶子值由调用者提供；新 Join 由 `estimate_join_rows` 估算；本文件新建的 Projection 把它设为 `0.0`。因此这是启发式模型，不是完整统计信息系统。
- `otherConds` 是消耗型状态：`makeJoin` 使用 `drain(..)`，仅把暂时无法归属的条件放回。贪心求解器在试探候选时克隆整个 base，只提交获胜候选的状态，避免试探过程丢失条件。
- `columnAllocator` 单调递增，但默认从 0 开始，调用者必须保证它不会与既有列号/计划 id 冲突。本文件没有全局唯一 id 服务，也没有碰撞检查。
- `generateLeadingJoinGroup` 按给定计划 id 排序，重复 id 只取一次，未点名计划保持原相对顺序追加；`generateNestedLeadingJoinGroup` 依次应用多组排序。它只处理 id 列表，不实现 Go 的完整 AST hint 验证与 warning 机制。
- `mergeMap` 直接以 `src` 覆盖 `dst` 的同键值；当前文件内部没有调用它，是供相邻移植逻辑使用的辅助 API。

## 依赖与调用关系

上游与装配关系：

- `pkg/planner/core/lib.rs` 公开本模块，并在 `#[cfg(test)]` 下挂接 `rule_join_reorder_test.rs`。
- `pkg/planner/core/casetest/ch/ch_test.rs` 直接构造计划并以不同 `dpThreshold` 调用 `JoinReOrderSolver::Optimize`，覆盖 DP 和贪心分支。
- `pkg/planner/core/rule_correlate.rs`、`rule_decorrelate.rs`、`rule_semi_join_rewrite.rs`、`rule_join_elimination.rs` 等复用这里的计划模型或结果类型；`rule_join_reorder_projection_inline.rs` 复用 Join 组抽取结果。

下游关系：

- `optimizeRecursive` 调用 `crate::rule_join_reorder_dp::joinReorderDPSolver::solve` 或 `crate::rule_join_reorder_greedy::joinReorderGreedySolver::solve`。
- 两个求解器都持有 `baseSingleGroupJoinOrderSolver`，并回调 `makeJoin`、`makeBushyJoin`、节点定位、连接判断和累计代价函数。
- 本文件唯一直接的跨模块类型依赖是 `crate::task::{Expression, JoinType}`；其余使用标准库 `HashMap`/`HashSet`。

Cargo 清单声明了 planner core 的大量真实子系统依赖，但本文件自身并未直接使用 logicalop、statistics、hint 或 session variable crate。这进一步表明其当前模型与 Go 主实现之间仍有接线层级差异。

## 错误处理与边界

- `generateLeadingJoinGroup` 在 hint 引用不存在的计划 id 时返回 `leading hint references plan {id}`；重复 id 被去重，不报错。
- `connectJoinNodes` 和 `makeBushyJoin` 拒绝空输入，分别返回 `empty join group` 与 `empty cartesian join group`。
- DP 下游还会拒绝空组、超过位图宽度的组、无法覆盖全部节点的 DP；错误通过 `?` 一路传到 `Optimize`。
- `findNodeIndexInGroup` 在列不属于任何节点时返回错误；`findNodeIndexForColumns` 还拒绝空列集和跨多个节点的列集。
- `extractJoinGroupImpl` 只展开 `JoinType::Inner`。Outer Join、Apply、Projection 等全部作为原子子树，不会跨越其边界重排。
- `hasOtherJoinCondition` 只检查条件的单个 `Expression.column` 是否位于左右任一侧；`makeJoin` 也采用“命中任一侧”的吸收规则。这比 Go 对“条件完整依赖于合并 schema、且真正跨两侧”的判断弱，不能据此推导完整谓词下推语义。
- `restoreSchemaIfChanged` 不验证原列是否仍存在，只生成列引用；`injectExpr` 对已有 `expression.column` 直接复用，也不检查该列是否属于传入计划。
- `makeJoin` 默认 Inner Join；若传入 `joinType`，则使用其中类型和偏好方法。`checkConnection` 当前只返回 `joinTypes.first()`，没有像 Go 实现那样按边索引精确对应 Join 类型，Outer Join 重排也不在抽取范围内。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件句柄、网络连接或事务。所有权沿 `JoinPlan` 值移动，树边用 `Box<JoinPlan>` 独占；需要保留原树或试探候选时显式 `clone`。因此单次求解没有共享可变全局状态，也没有跨调用资源清理要求。

需要关注的是内存和计算生命周期：抽取阶段克隆原子子树和条件；递归映射重建节点；贪心候选枚举会克隆 base 与计划；DP 使用位图枚举子集，其时间和空间随组大小指数增长。`dpThreshold` 是限制该成本的主要保护，但代码用 `max(2)`，所以配置为 0 或 1 仍会让两节点组进入 DP。`makeBushyJoin` 每轮两两合并，奇数尾节点进入下一轮，直到只剩根节点。

## 与 Go 版本的对应关系

Rust 文件以 `pkg/planner/core/rule_join_reorder.go` 为同路径语义对照，保留了 `JoinReOrderSolver`、`jrNode`、`joinTypeWithExtMsg`、`basicJoinGroupInfo`、`extractJoinGroup`、`optimizeRecursive`、`restoreSchemaIfChanged`、`baseNodeCumCost`、`checkConnection`、`injectExpr`、`makeJoin`、`makeBushyJoin` 和 `calcJoinCumCost` 等核心概念。

已对齐的骨架包括：递归处理计划、抽取 Join 组、按阈值选择 DP/贪心、累计子树行数作为基础代价、把互不连通分量组成 bushy 树、重排后恢复输出 schema，以及为非列 Join 表达式预留投影注入能力。

仍存在的重要差异：

- Go 操作真实 `base.LogicalPlan`、`logicalop.LogicalJoin/Projection/Selection`、真实表达式 schema 与统计信息；Rust 使用本文件定义的简化枚举和整数列号。
- Go `Optimize` 读取会话变量，可切换 advanced join reorder，并按实际阈值选择算法；Rust 只读取结构体字段 `dpThreshold`。
- Go 抽取支持受会话变量控制的 Selection/Projection 穿透、部分 Outer Join 重排、NullEQ/straight join/算法 hint 限制、null-extended 列保护、LEADING hint 警告与方法 hint 映射；Rust 仅展开连续 Inner Join，未实现这些完整约束。
- Go 的谓词分类区分左条件、右条件、跨侧条件、可变副作用和 outer-bind 条件；Rust 只按一个可选列号做近似归属。
- Go 的 schema 恢复支持 projection inlining 的表达式映射并在缺失映射时回退；Rust 仅按原列号生成 Projection，且不提供失败回退。
- Go 从真实计划上下文分配 id、派生统计并维护 query block；Rust 用本地 `columnAllocator` 和启发式 `estimate_join_rows`。

因此，扩展 Rust 时应以 Go 行为作为语义目标，但必须逐项移植约束和测试，不能把相同符号名当作行为完全等价的证据。

## 扩展指南

- 新增可重排算子或允许穿透 Selection/Projection 时，首先修改 `extractJoinGroupImpl`，同时设计表达式替换、非确定性函数、输出 schema 和失败回退语义；同步 `rule_join_reorder_projection_inline_test.rs` 与 case 测试。
- 修改 DP/贪心切换条件时，修改 `JoinReOrderSolver::optimizeRecursive`，并在 `pkg/planner/core/casetest/ch/ch_test.rs` 同时覆盖阈值两侧。注意 `max(2)` 是现有不变量。
- 扩展 Join 类型时，必须同时修改抽取边界、`checkConnection` 的边到类型映射、`makeJoin` 的条件安置以及 null 扩展约束；应对照 Go Outer Join 逻辑新增独立测试，不能只放宽 `extractJoinGroupImpl` 的匹配条件。
- 改变代价模型时，修改 `estimate_join_rows`、`baseNodeCumCost` 或 `calcJoinCumCost`，并同步 `rule_join_reorder_test.rs`、`rule_join_reorder_dp_test.rs` 和 `rule_join_reorder_greedy_test.rs`。性能风险主要是 DP 指数复杂度和候选克隆量。
- 改变条件吸收逻辑时，重点审查 `makeJoin` 对 `otherConds` 的消耗、贪心候选克隆隔离及剩余条件最终落点；必须覆盖单侧条件、跨侧条件、无列条件和非确定性表达式。
- 调整注入列/id 分配时，应先接入不会与既有计划冲突的分配器，再修改 `injectExpr` 与 `restoreSchemaIfChanged`；不能继续假设默认 0 安全。
- Rust 单元测试应继续放在独立 `*_test.rs` 文件，由 `lib.rs` 的 `#[cfg(test)] mod ...` 挂接，不应内嵌回生产文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；本次通过索引读取了目标文件全部 579 行。
- RustCodeGraph 源码/调用查询：`node --file pkg/planner/core/rule_join_reorder.rs`；`query Optimize --kind method`；`callers/callees JoinReOrderSolver::Optimize`；`callers/callees baseSingleGroupJoinOrderSolver::makeJoin`。确认 `Optimize -> optimizeRecursive -> extractJoinGroup/map_children -> DP 或 greedy -> restoreSchemaIfChanged`，以及求解器回调 `makeJoin`/`estimate_join_rows` 的关系。
- crate 与模块证据：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。
- Rust 下游实现：`pkg/planner/core/rule_join_reorder_dp.rs`、`pkg/planner/core/rule_join_reorder_greedy.rs`、`pkg/planner/core/rule_join_reorder_projection_inline.rs`。
- 独立 Rust 测试：`pkg/planner/core/rule_join_reorder_test.rs` 验证累计代价包含后代以及四叶 bushy 树；`pkg/planner/core/rule_join_reorder_dp_test.rs` 验证节点定位与错误；`pkg/planner/core/casetest/ch/ch_test.rs` 直接覆盖 DP/贪心入口；`pkg/planner/core/casetest/rule/rule_join_reorder_test.rs` 覆盖公开入口和节点定位。
- Go 对照：`pkg/planner/core/rule_join_reorder.go`；相关 Go 测试包括 `pkg/planner/core/rule_join_reorder_dp_test.go` 与 `pkg/planner/core/casetest/rule/rule_cdc_join_reorder_test.go`。
- 本任务为只读行为分析和单文档新增，按计划未运行 Cargo，也未把现有测试结果当作本次验证证据；最终结构检查用于确认 11 个固定章节完整存在。
