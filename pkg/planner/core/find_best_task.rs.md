# `pkg/planner/core/find_best_task.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate。该 crate 由 `pkg/planner/core/Cargo.toml` 定义，库入口是 `pkg/planner/core/lib.rs`；后者通过 `pub mod find_best_task` 公开本模块，并只在测试配置下以独立文件 `find_best_task_test.rs` 装配单元测试。文件没有条件编译项，`nextgen` feature 也没有直接控制这里的代码。

它承载一套自包含的 Volcano 风格“逻辑计划 + 物理属性 -> 最优 Task”模型：定义精简的逻辑计划、物理属性和访问路径数据，枚举物理候选，按代价挑选计划；对于 `DataSource`，还负责 skyline 剪枝以及 PointGet、IndexScan、IndexMerge、TableScan、MPP 等 Task 的构造。直接下游主要是同 crate 的 `plan_cost_ver1`、`plan_cost_ver2` 和 `task` 模块。

当前接线必须区分两层：

- `find_best_task.rs` 由 `lib.rs` 导出，并被 `index_join_path.rs`、`planbuilder.rs`、`exhaust_physical_plans.rs` 等复用其数据类型或候选比较辅助函数。
- 全仓 Rust 搜索中，公开函数 `findBestTask` 的直接调用只有本文件对子节点的递归和 `find_best_task_test.rs`；canonical 逻辑计划入口另在 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的 `CanonicalFindBestTaskRouter`/内部递归。因此不能把本文件描述为所有 Rust SQL 规划请求已经统一经过的入口。

## 核心职责

1. 定义本地规划模型：`Datum`、`Range`、`IndexInfo`、`AccessPath`、`PhysicalProperty`、`DataSource`、`PlanAlternative` 和 `LogicalPlan`。
2. 通过 `prepareIterationDownElems` 和四个迭代函数选择普通逻辑节点、GroupExpression、Sequence 及二者组合的子计划遍历方式；共同实现是 `iterate_children`。
3. 由 `findBestTask` 查询属性缓存，分流普通逻辑节点与 `DataSource`，比较直接满足属性的候选和添加 Sort enforcer 后的候选。
4. 由 `skylinePruning` 使用访问条件覆盖、回表、排序匹配、全局索引、风险比例、等值条件以及 Index Join 调整行数等维度淘汰被支配路径。
5. 将保留路径转换为具体 Task：`convertToPointGet`、`convertToBatchPointGet`、`convertToIndexScan`、`convertToIndexMergeScan`、`convertToTableScan` 和 `convertToSampleTable`。
6. 提供生产侧可复用的 Index Join 候选构造接口 `getIndexCandidateForIndexJoin`，以及文件尾部供独立测试使用的 mock 逻辑/物理计划。

## 主要符号

- `PhysicalProperty`：规划需求的值对象，包含 `task_type`、排序项、期望行数、enforcer 开关、MPP 任意分区标志、部分有序需求、Index Join 列数和向量 TopK。`without_order` 清空硬排序、把期望行数恢复为无穷并放宽 MPP 分区，但保留其余字段。
- `AccessPath`：表或索引访问路径，集中保存 range、访问/索引/表过滤条件、行数上下界、索引属性、存储类型、强制保序标志以及 IndexMerge 子路径。
- `LogicalPlan`：拥有子节点、按切片组织的 `PlanAlternative`、可选 `DataSource`、属性到 Task 的缓存和 sequence 标志。这里使用值递归和克隆，而不是 Go 版的 planner interface 树。
- `candidatePath`：skyline 比较包装，缓存列覆盖 map、属性匹配结果、IndexMerge 各分支结果、等值谓词数量，以及 Index Join 单次 probe 的调整后行数。
- `findBestTask`：本模型的主入口，缓存键由 `property_key` 编码完整属性和代价模型版本。
- `enumeratePhysicalPlans4TaskHelper` / `enumeratePhysicalPlans4Task`：先给每个物理候选规划子 Task 并 `attach2Task`，再在 hinted、preferred、普通候选槽位内比较代价；hinted 候选优先于其他候选，preferred 次之。
- `getTaskPlanCost` / `compareTaskCost`：按 Root、MPP、Cop Task 形态选择代价计算任务类型；无效 Task 的代价为 `f64::MAX`，有效 Task 即使代价达到最大值也不因此失效。
- `matchProperty` / `matchPartialOrderProperty`：验证路径列顺序、等值前缀、升降序一致性、前缀索引限制和 grouped ranges，返回不匹配、匹配或需 MergeSort。
- `compareCandidates` / `skylinePruning`：实现多维支配关系和增量保留集合。多值索引直接视为不可比较；当 Index Join 调整行数不稳定时，不启用经验性的 1000 倍行数剪枝。
- `findBestTask4LogicalDataSource`：先尝试常量假谓词对应的 TableDual，再剪枝并按路径类型转换，最后以代价选择 Task。
- `GroupRangesByCols`、`validateTableSamplePlan`：本文件中显式返回 `Result` 的边界校验函数。
- `mockLogicalPlan4Test`、`mockPhysicalPlan4Test`、`ExhaustPhysicalPlans4MockLogicalPlan`：为 `find_best_task_test.rs` 提供 mock；虽位于生产文件且公开，但用途由注释和调用位置限定为测试支持。

## 执行流程

普通逻辑计划的主流程如下：

1. `findBestTask` 用 `property_key(prop, model_version)` 查 `LogicalPlan.cache`。键包含任务类型、硬/提示排序、期望行数、enforcer、MPP 分区、部分有序、Index Join、向量 TopK 和代价模型版本，防止不同属性错误复用。
2. 如果节点含 `data_source`，立即转入 `findBestTask4LogicalDataSource` 并缓存结果。
3. 非 DataSource 节点只接受 Root 或 MPP 请求；其他 Task 类型返回带原因的 `Task::Invalid`。
4. `enumeratePhysicalPlans4Task` 遍历候选切片；helper 通过 `prepareIterationDownElems` 选迭代函数，递归调用 `findBestTask` 获取每个孩子的 Task，任何孩子无效都会淘汰该候选。
5. 候选计划通过 `attach2Task` 挂接孩子；Root 属性会调用 `into_root`。候选若需要强制属性，`enforce_property` 在根部插入 `PlanKind::Sort`。
6. 先比较无需 enforcer 的候选；若没有 preferred 结果，且属性允许 enforcer 或有排序需求，则以 `without_order` 重新枚举并比较。有效 hinted/preferred 候选具有选择优先级，普通候选才单纯以代价决定。
7. 最优 Task 写入当前逻辑节点的属性缓存。

DataSource 分支的主流程如下：

1. `tryToGetDualTask` 发现无列引用且名字为 `false` 的已下推条件时，直接返回零行 `TableDual` Root Task。
2. `skylinePruning` 将每条 `AccessPath` 包装成 table、index 或 IndexMerge candidate；对每个新候选与已保留候选逐一执行 `compareCandidates`，丢弃被支配者并移除它支配的旧候选。
3. `findBestTask4LogicalDataSource` 按优先判定顺序选择转换函数：TABLESAMPLE、IndexMerge、单 range PointGet、多 range BatchPointGet、TableScan、IndexScan。
4. 各转换函数先验证 Task 类型、排序匹配、保序强制、存储能力或 PointGet 唯一性；不满足时返回 Invalid Task。满足时创建 `PlanNode`，设置统计、schema、range、条件、store 与 flags，并以 `selection` 包裹剩余过滤。
5. 每个有效转换结果由 `compareTaskCost` 与当前最优项比较。函数最终调用 `validateTableSamplePlan`，但当前代码丢弃其 `Result`，随后返回 best Task。

## 数据与状态

- 可变持久状态仅在 `LogicalPlan.cache: HashMap<String, Task>`；缓存归属于单个 `LogicalPlan` 值，没有全局缓存。递归时 `iterate_children` 克隆子逻辑计划，所以对子克隆缓存的写入不会回写原始 `children`。
- `property_key` 使用 Debug/字符串格式编码浮点值及列表，作用域仅为进程内当前结构；它不是稳定序列化格式，不应持久化或作为跨版本协议。
- `candidatePath` 克隆完整 `AccessPath`。`skylinePruning` 维护局部 `Vec<candidatePath>`，按输入路径顺序增量比较；多维不可比较的路径会同时保留。
- `getCountAfterAccess4SkylinePruning` 在普通场景返回 `count_after_access`；Index Join 只有 `countAfterAccess4IndexJoinOK` 为真才返回按 probe 调整的行数，否则明确禁用相关经验剪枝。
- 扫描转换会从 `DataSource.stats` 派生 `PlanNode.stats`，通常以 `expected_count`、range 数或 1 为上界。`Task` 形态保存 Root、Cop、MPP 或 Invalid 状态，Cop 还区分 index/table plan 和 index 是否结束。
- `Range`、`AccessPath`、`PhysicalProperty` 等均是普通拥有所有权的 Rust 值；函数以借用读取，只有 IndexMerge 分组和 mock 状态等局部路径会显式克隆或修改。

## 依赖与调用关系

上游与装配：

- `pkg/planner/core/lib.rs` 公开模块，并将 `find_best_task_test.rs` 作为独立测试模块装配。
- `pkg/planner/core/index_join_path.rs` 在构建普通索引内侧路径和整型主键路径时调用 `getIndexCandidateForIndexJoin`；对应测试在 `index_join_path_test.rs` 构造同类 candidate。
- `pkg/planner/core/planbuilder.rs`、`indexmerge_path.rs`、`indexmerge_unfinished_path.rs`、`exhaust_physical_plans.rs` 和谓词下推代码复用 `AccessPath`、`Datum`、`IndexInfo`、`Range`、`PhysicalProperty` 等模型。
- 对 `findBestTask(` 的 Rust 全仓搜索只找到本文件递归与 `find_best_task_test.rs` 的缓存测试；canonical planner 的独立实现/路由在 `operator/physicalop/base_physical_plan.rs`，这是判断当前接线范围的重要限制。

下游：

- `crate::task::{PlanNode, PlanKind, Task, TaskType, StoreType, Expression, FieldType, StatsInfo, attach2Task}` 提供计划节点、Task 容器和挂接操作。
- `crate::plan_cost_ver2::GetPlanCost` 是 `getTaskPlanCost` 实际调用的统一代价入口；`PlanCostOption` 从 `plan_cost_ver1` 提供默认选项。`model_version` 随调用向下传递并进入缓存键。
- `HashMap` 用于缓存和谓词列覆盖，`HashSet` 记录伪统计索引列；除标准库外，本文件没有直接使用 `Cargo.toml` 中的第三方依赖。

经 RustCodeGraph，目标文件被报告为由 `find_best_task_test.rs`、`index_join_path.rs`、`index_join_path_test.rs` 和 `planbuilder.rs` 使用；原始 import 搜索补充了上述其他类型复用点。RustCodeGraph 的 `callers`/`callees` 命令对关键函数未输出边，因此这里没有把缺失图边当作“无调用”的唯一证据，而是同时采用了直接调用搜索。

## 错误处理与边界

- 主规划函数以 `Task::invalid(reason)` 表达不可实现，而不是返回 `Result`。常见边界包括：子 Task 无效、非 Root/MPP 的完整逻辑算子下推、排序不匹配、PointGet 条件不足、MPP 分区不兼容、TiFlash 无法保序或 cop 被禁用。
- `getTaskPlanCost` 仅把 `Task::Invalid` 视为 invalid；有效计划成本为 `f64::MAX` 仍可参与选择。`find_best_task_test.rs::cost_overflow_does_not_turn_a_task_invalid` 固化了此行为。
- `GroupRangesByCols` 在分组列下标越界时返回带列号的字符串错误；调用它的 `matchPropForIndexMergeAlternatives` 使用 `unwrap_or_default`，即在该路径上将错误降级为空分组，而不是继续传播。
- `validateTableSamplePlan` 能在 TABLESAMPLE 得到非 TableSample 有效 Task 时返回错误，但 `findBestTask4LogicalDataSource` 当前用 `let _ = ...` 丢弃错误。因此调用者看不到该错误；扩展时不能假定验证失败会使主流程失败。
- `splitIndexFilterConditions` 仅按表达式的单一 `column` 下标是否在 `index_columns` 中划分，不等同于 Go 版完整的索引覆盖表达式分析。
- `isPointGetPath` 要求非空 range、非空唯一索引（整型 handle 除外）、无前缀索引、range 宽度等于索引列数、schema 无 Vector，且每个 range 都是无 NULL 的闭区间点。
- `matchProperty` 要求所有排序项方向一致；等值/单值 IN 前缀之后才匹配排序列，并拒绝涉及前缀索引的排序。存在 `grouped_ranges` 时返回 `MatchedNeedMergeSort`。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件句柄或网络资源。规划完全同步执行，临时候选和 `PlanNode` 由 Rust 所有权在函数返回或离开作用域时释放。

缓存也没有内部同步：调用者需要以 `&mut LogicalPlan` 进入 `findBestTask`，编译期独占借用防止同一 `LogicalPlan` 被并发修改。`DataSource` 分支只读借用源数据，再把结果克隆到缓存。大对象的主要资源风险来自 `AccessPath`、子逻辑计划和 Task 的克隆，而不是后台生命周期泄漏。

`Task::into_root` 和 `attach2Task` 消费或拥有 Task/计划值；转换函数不会保留指向调用栈局部值的引用。MPP/Cop 在这里只是计划形态，不会启动远程执行或持有存储连接。

## 与 Go 版本的对应关系

同路径 `pkg/planner/core/find_best_task.go` 是语义对照源。两者共享的主干包括：物理属性缓存、物理计划穷举、hint/enforcer 优先级、Task 代价比较、DataSource skyline、PointGet/BatchPointGet/IndexScan/IndexMerge/TableScan 转换、TiKV/TiFlash Task 分流以及独立 mock 测试意图。

Rust 独立测试与 Go `find_best_task_test.go` 对齐了三类核心回归：代价溢出不使 Task 无效、只有允许或 hint 要求时才通过 Sort 强制属性、hint 不可应用时回退并记录一次告警。Rust 还增加了 MPP 分区缓存键区分，以及 Index Join 调整行数稳定/不稳定两种 skyline 行为。

但当前 Rust 文件是精简模型，不应宣称与 Go 完全等价：

- Go `findBestTask` 操作真实 planner interface、memo `GroupExpression`、session/statement context，返回 `(Task, error)`，并处理 IndexJoin property admission、hint 枚举中的更多特殊情况；Rust 本地 `LogicalPlan` 是值模型，主流程只返回 `Task`。
- Go skyline 与 IndexMerge alternative 选择包含 session variable、表达式 hash、覆盖索引、plan-cache 安全、intersection/MV index、hint 及不可缓存原因等完整规则；Rust 使用字符串名称与列下标作简化判断。
- Go 扫描转换会构造真实 physical operator、histogram、partition info、root residual filters、MPP/vector 元数据并传播错误；Rust 构造通用 `PlanNode`，多个失败场景降级为 Invalid Task。
- Go 的 `getTaskPlanCost` 会处理 IndexMerge partial plan 成本和未知 Task 错误；Rust 对 Cop 的 index/table plan 做简化求和，没有错误通道。
- Rust 的 `findBestTask` 当前没有被 canonical Rust planner 直接调用；生产侧已确认的直接接线主要是类型共享和 Index Join candidate 构造。功能迁移时必须同时核对 `operator/physicalop/base_physical_plan.rs`，避免维护两套行为却只修改一处。

## 扩展指南

- 新增物理属性字段时：同步更新 `PhysicalProperty`、`Default`、`without_order`（判断应保留还是放宽）和 `property_key`；补充 `find_best_task_test.rs` 的缓存隔离回归，否则不同需求可能复用错误 Task。
- 新增访问路径类型时：更新 `AccessPath` 判别、candidate 构造、`skylinePruning` 的可比维度，以及 `findBestTask4LogicalDataSource` 的转换分派；转换失败应保持 Invalid Task 语义。若生产路径也经过 canonical router，同时核对 `operator/physicalop/base_physical_plan.rs` 和 Go 对照。
- 修改 skyline 规则时：优先在 `compareCandidates` 或 `candidatePath::getCountAfterAccess4SkylinePruning` 做局部变更；同步覆盖伪统计、不可比较、多值索引、风险比例、expected count 和 Index Join 稳定性。Index Join 相关回归应放在独立的 `find_best_task_test.rs` 或 `index_join_path_test.rs`，不要内嵌到生产文件。
- 修改排序匹配时：联动 `matchProperty`、`matchPartialOrderProperty`、IndexMerge 分支匹配、`enforce_property` 和各 scan 转换的 keep-order 限制；重点检查混合升降序、等值前缀、前缀索引、分组 range 和 TiFlash/分区表。
- 新增错误场景时：先决定是继续使用 Invalid Task 作为“候选不可行”，还是需要把主 API 改为 `Result`。不要像当前 TABLESAMPLE 校验那样无意丢弃必须上报的错误；若改变错误模型，应对照 Go 的传播语义并更新所有调用者。
- 性能风险集中在路径两两比较、递归克隆和计划代价重复计算。增加 candidate 维度时须保持支配关系方向一致；增加缓存字段时须确保键完整；增加 clone 时需评估大计划树和大量 IndexMerge 子路径。
- 按仓库约定，Rust 测试继续放在独立 `*_test.rs` 文件；本模块通过 `lib.rs` 的测试专用 `#[path]` 声明接入测试。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/planner/core/find_best_task.rs`（1–1736 行），包括全部类型、函数、impl 和文件内调用。
- crate 与装配：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件和 4415 个 Go 文件；`node --file` 读取目标源码；`query findBestTask`、`query skylinePruning`、`query convertToTableScan` 定位 Rust/Go 同名符号；目标文件关系报告列出四个使用文件。关键符号的 `callers`/`callees` 查询未返回明细，此限制已在调用关系章节披露。
- Rust 上游与测试：`pkg/planner/core/index_join_path.rs`、`pkg/planner/core/index_join_path_test.rs`、`pkg/planner/core/find_best_task_test.rs`；另以全仓调用搜索确认 `findBestTask` 的直接 Rust 调用范围。
- Go 对照：`pkg/planner/core/find_best_task.go` 的 `findBestTask`、skyline/IndexMerge、`isPointGetPath`、`convertToTableScan` 等段落，以及 `pkg/planner/core/find_best_task_test.go`。
- 下游接口：源码 import 指向 `pkg/planner/core/task.rs`、`plan_cost_ver1.rs`、`plan_cost_ver2.rs`；RustCodeGraph 查询确认 `GetPlanCost`/计划挂接相关符号存在。

人工复核结论：本文区分了本地精简模型与 canonical planner 接线，覆盖了文件存在原因、主流程、数据/缓存、边界、资源生命周期、Go 差异及安全扩展位置；没有把未返回的调用图边或 Go 的完整能力描述成当前 Rust 已支持。
