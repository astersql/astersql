# `pkg/planner/core/task.rs`

源文件：[`task.rs`](./task.rs)

## 文件定位

`task.rs` 属于 `astersql-planner-core` crate；crate 根在 [`lib.rs`](./lib.rs) 中以 `pub mod task` 公开该模块，清单 [`Cargo.toml`](./Cargo.toml) 指定库入口为 `lib.rs`、默认 feature 为空，并用 `package.metadata.porting.go-package = "pkg/planner/core"` 标明 Go 对照包。该文件位于“选出物理计划之后、形成可执行任务之前”的接线层：它用较轻量的 `PlanNode` 表示计划树，用 `Task` 表示 Root、Cop、MPP 三种执行落点，并把物理算子附着到子任务。

当前 Rust 仓库同时存在更完整、基于 trait object 的物理算子任务体系（例如 `operator/physicalop/task.rs` 及各物理算子的 `Attach2Task`）。本文件不是那套类型的别名，而是 `find_best_task.rs` 中简化候选计划路径实际使用的独立模型：`mockPhysicalPlan4Test::Attach2Task` 以及 `find_best_task.rs` 的多处搜索路径直接调用本文件的 `attach2Task`。因此阅读时不能把 Go `task.go` 的全部能力默认视为这里已经实现。

## 核心职责

- 定义执行位置与计划载体：`StoreType`、`TaskType`、`PlanKind`、`PlanNode`、`Task` 以及表达式、字段类型、统计信息和警告等辅助结构。
- 维护任务当前计划槽。`Task::plan`、`take_plan`、`into_root` 与 `attachPlan2Task` 统一处理 Root/MPP 的单一计划，以及 Cop 在 `index_finished` 前后的 `index_plan`/`table_plan` 选择。
- 按物理算子分发附着。`attach2Task` 根据 `PlanKind` 调用 UnionScan、Join、Limit、TopN、Projection、聚合、Window、CTE、Sequence 等专用路径。
- 保留关键优化语义：UnionScan 与 Projection/Selection 的重排、Cop/MPP/Root 落点转换、Limit/TopN 下推、MPP Join 分区键类型协商、重函数排序键物化、统计和网络代价辅助估算。
- 用 `Task::Invalid` 将缺子任务、错误任务类型等结构性失败显式带回调用方，避免这些路径因索引越界而 panic。

## 主要符号

- `StoreType::{TiDb, TiKv, TiFlash}` 与 `TaskType::{Root, CopSingleRead, CopMultiRead, Mpp}` 描述存储/执行域。`TaskType` 在本文件中只作为公开分类值，实际状态由 `Task` 变体承载。
- `TypeCode`、`FieldType` 和 `Expression` 是简化类型与表达式模型。`Expression` 用名称、列下标、函数计数、虚拟列标记和返回类型支持下推判定；它并不等价于 Go 的完整 `expression.Expression` 树。
- `StatsInfo` 保存 `row_count`、`avg_row_size` 和可选 `histogram_row_size`；`PlanNode::rows`、`row_size` 都把结果钳制到非负值。
- `PlanKind` 列出本分发器识别的物理算子；`Other(String)` 为未专门建模的节点保留名称。
- `PlanFlags` 汇集保序、分页、MPP 强制、临时表、分区、缓存、细粒度 shuffle、部分有序等布尔状态；`PlanNode` 另保存 schema、表达式、排序/分组项、连接参数、并发度、offset/count、存储类型、标签和代价。
- `Task::{Root, Cop, Mpp, Invalid}` 是核心状态机。Root 还保存 `index_join`，Cop 区分索引计划与表计划，MPP 保存 `partition_keys`；三种有效任务都拥有 `TaskWarnings`。
- `attachPlan2Task` 是一元节点的公共附着原语；`root_binary` 是二元算子的内部公共原语；`attach2Task` 是公开分发入口。
- `needConvert`、`decimal_bucket`、`negotiateCommonType` 为 MPP 分区键类型对齐服务；`convertPartitionKeysIfNeed4PhysicalHashJoin` 在发生转换时关闭相关子计划的 `fine_grained_shuffle`。
- `HeavyFunctionNameMap`、`ContainHeavyFunction`、`getPushedDownTopN`、`tryReturnDistanceFromIndex` 识别向量距离/全文匹配等重函数，并在 Local TopN 前插入 Projection 只计算一次。
- `extractRows`、`calcPagingCost`、`getAvgRowSize`、`collectRowSizeFromMPPPlan`、`accumulateNetSeekCost4MPP` 提供简化统计和代价辅助值。

## 执行流程

1. `find_best_task.rs` 生成 `PlanAlternative` 后，把子候选递归转为 `Vec<Task>`，再调用 `attach2Task(plan, child_tasks)`；RustCodeGraph 和源码均显示 `find_best_task.rs::Attach2Task` 到该入口的调用边。
2. `attach2Task` 检查 `plan.kind`。有专用语义的节点进入对应 `attach2Task4*`；未列出的节点若有子任务，就走 `attachPlan2Task`，无子任务则直接创建 `Task::root(plan)`。
3. 一元普通节点通过 `attachPlan2Task` 从任务的活动槽取出原计划作为唯一 child，再把新节点写回同一槽；Invalid 原样传播。Cop 的活动槽由 `index_finished` 决定。
4. Apply、Index Join、普通 Hash Join、Merge Join 等二元 Root 路径走 `root_binary`：必须恰有两个有效任务，双方先 `into_root`，警告合并，计划成为左右子节点。Index Join 额外把 Root 的 `index_join` 置为 `true`。
5. TiFlash Hash Join 在两个子任务都是 MPP 时走 `attach2TaskForMpp4PhysicalHashJoin`，保留 MPP 形态并合并警告；否则回退为 Root 二元计划。分区键类型需对齐时，`negotiateCommonType` 选择公共类型，发生转换的一侧取消细粒度 shuffle；列数不符时 `enforceExchangerByBackup4PhysicalHashJoin` 清空键并插入 `ExchangeSender`。
6. Limit 对 Cop 下推一个 `offset = 0、count = offset + count` 的局部节点，对 MPP 原位附着，其余提升到 Root。`sinkIntoIndexLookUp` 和 `sinkIntoIndexMerge` 可把同样的局部 Limit 写入 Cop 计划槽。
7. TopN 先以 `canPushDownToTiKV`/`canPushDownToTiFlash` 检查任务落点、表达式 protobuf 可转换性与虚拟列。可下推时创建 Local TopN，并在 Root 保留 Global TopN；`getPushedDownTopN` 的更细路径还会在有 offset 或重函数时保留 Global，并由 `tryReturnDistanceFromIndex` 插入 Projection、把重函数排序项改写成列引用。
8. UnionAll 在所有子任务均为 MPP 时保持 MPP，否则把所有分支提升到 Root；两条路径都汇总警告。HashAgg 只在 MPP 输入时保持 MPP；CTEStorage 强制 Root；Window 的专用 MPP 入口要求 MPP 并开启细粒度 shuffle。

## 数据与状态

`PlanNode` 是拥有所有权的树结构，`children: Vec<PlanNode>` 直接嵌套子树。附着函数通常消费 `PlanNode` 和 `Task`，通过 `Option::take`/`std::mem::take` 移动计划，而不是共享可变节点；只有为了构造 local/global 两份计划或保留调用方模型时才显式 `clone`。

`Task::Cop` 的不变量是“当前活动计划由 `index_finished` 选择”：未完成索引阶段时读写 `index_plan`，完成后读写 `table_plan`。`into_root` 会优先取活动槽，活动槽为空时才回退到另一槽，并保留 warnings。`Task::Mpp.partition_keys` 是 Hash Join 分区兼容性的状态；重建交换器后会被清空，表示原分区保证不再成立。

`TaskWarnings` 只是字符串列表。二元 Root、MPP Join 和 UnionAll 显式合并子任务警告；一元附着与 `into_root` 保留原警告。UnionScan 的三种重排分支还显式保留 Root 的 `warnings` 与 `index_join`，该不变量由 `task_test.rs` 回归覆盖。

计数使用 `u64`，多数 Limit/TopN 路径用 `saturating_add` 防止 `offset + count` 溢出；`pushLimitDownToTiDBCop` 例外地使用普通加法，应在扩展该路径时注意 debug/release 下溢出行为差异。统计值是 `f64`，只在 `rows`、`row_size` 和 `getAvgRowSize` 的出口做非负钳制。

## 依赖与调用关系

本文件的直接外部代码依赖仅为标准库 `std::collections::{HashMap, HashSet}`；其余类型都在文件内定义。所在 crate 则通过 `Cargo.toml` 依赖 expression、kv、planner property、statistics、physicalop 等多个工作区 crate，以及固定 revision 的 `tipb`，但本简化模块没有直接 import 它们。

主要上游是 `find_best_task.rs`：该文件在候选计划附着、强制排序和 mock 物理计划接口中调用 `attach2Task`。此外，逻辑构建、代价模型、规则与测试广泛导入 `Expression`、`PlanKind`、`PlanNode`、`StatsInfo`、`StoreType`、`Task` 等数据类型。`lib.rs` 的 `pub mod task` 让 case test crate 也可通过 `astersql_planner_core::task::*` 使用这些类型。

主要下游都在本文件内部：`attach2Task → attach2Task4* → attachPlan2Task/root_binary`；TopN 路径调用表达式可下推判断和重函数物化；MPP Join 路径调用公共类型协商；递归代价函数沿 `PlanNode.children` 向下遍历。RustCodeGraph 对 `attach2Task` 的 callee 列表确认了这些专用分支，并确认 `negotiateCommonType → needConvert`、`tryReturnDistanceFromIndex → ContainHeavyFunction`。

需要特别区分另一条并存链：`operator/physicalop/*.rs` 的各具体算子使用 `Box<dyn Task>` 和自身的 `Attach2Task` 方法；它们不是本文件分发器的直接下游。二者都在移植 Go 规划器语义，但抽象层次和完成度不同。

## 错误处理与边界

本文件不返回 `Result`。结构错误被编码为 `Task::Invalid { reason }`，典型情况包括一元算子缺少 child、二元算子不是两个有效任务、MPP Hash Join 收到非 MPP 输入、MPP UnionAll 混入非 MPP、MPP Window 收到非 MPP，以及 UnionScan 子计划为空。`attachPlan2Task` 对 Invalid 直接短路，`Task::plan` 对 Invalid 返回 `None`。

边界行为包括：`root_binary` 会拒绝任何 Invalid 子任务；UnionAll 只检查列表非空，但会忽略无法取出计划的子项；Sequence 也会跳过空计划。`convertPartitionKeysIfNeed4PhysicalHashJoin` 对非 MPP 输入直接无操作，并只处理两侧键数的最小值；调用方若需要严格等长，必须另行使用交换器校正。表达式下推会拒绝虚拟列，且 TiFlash 额外拒绝名称以 `tikv_only` 开头的排序表达式。

Go 实现大量使用真实执行类型、session context、failpoint、fix control、schema 复制和具体错误/警告机制；Rust 文件当前用简化值对象表达这些行为。新增功能不能仅依据 Go 函数同名就假定 Rust 已具备相同错误语义。

## 并发与资源生命周期

本文件没有线程、锁、异步任务、channel、网络句柄或事务对象，也没有条件编译项。所有变换在调用线程同步执行，资源生命周期由 Rust 所有权管理：函数消费任务/计划，`take_plan` 临时清空槽位并把所有权移入新树，返回值重新拥有完整计划。

MPP 在这里是计划的执行形态，不表示本模块直接启动并行工作。`concurrency`、`fine_grained_shuffle`、`partition_keys` 和 `ExchangeSender` 都是后续执行阶段读取的规划元数据。递归函数 `collectRowSizeFromMPPPlan`、`accumulateNetSeekCost4MPP` 与深计划树的调用栈深度相关，但不分配后台资源。

警告通过克隆或合并转移；计划树也可能因 local/global TopN、Projection 或下推节点而克隆。扩展大计划场景时应关注这些深克隆的内存与 CPU 成本，不能把所有权安全误解为零拷贝。

## 与 Go 版本的对应关系

直接对照文件是 [`task.go`](./task.go)。Rust 保留了主要函数族和命名，包括 `attachPlan2Task`、UnionScan/Join/Limit/TopN/Agg/Window/CTE/Sequence 附着、`needConvert`/`negotiateCommonType`、重函数 TopN 优化，以及 MPP 行大小和 seek 代价辅助函数。

已对齐的关键语义包括：UnionScan 把 Projection 保持在最外层；Limit/TopN 下推时局部 count 使用 `offset + count`；字符串公共类型无需转换，Decimal 以 9/18/38/65 精度区间判断 TiFlash 表示；重函数 TopN 通过 Projection 物化排序表达式；二元任务汇总警告；MPP 分区类型改变后取消细粒度 shuffle。

Rust 并非逐对象完整复刻。Go 使用 `base.PhysicalPlan`、`base.Task` 和 `physicalop` 的具体类型，带 session context、直方图、fix control、failpoint、索引路径、schema/column ID、partition property 及更完整代价公式；Rust 本文件用 `PlanNode`、字符串表达式名、标签和简化统计替代。若 Go 后续分支、hint、warning 或 schema 修复在 Rust 中没有对应字段，应记录为移植差异，不能通过删除 Go 分支或弱化断言来“对齐”。

测试对应关系：Go `task_test.go::TestPhysicalUnionScanAttach2Task` 验证重排不破坏原计划；Rust `task_test.rs` 进一步验证三种形状、缺 child 返回 Invalid、warnings/index_join 保留以及类型转换边界。Go `task_heavy_function_optimize_test.go::TestGetPushedDownTopNHeavyFunctionNotFirstByItem` 与 Rust `task_heavy_function_optimize_test.rs::pushed_down_topn_handles_heavy_function_after_first_by_item` 都覆盖重函数不是首个排序项时的物化和列下标改写。

## 扩展指南

- 新增 `PlanKind` 时，先判断它是叶节点、一元、二元还是多分支节点，再在 `attach2Task` 增加明确分发；不要依赖默认分支掩盖子任务数量或执行落点约束。
- 修改任务槽语义时，应集中审查 `Task::plan`、`take_plan`、`into_root` 与 `attachPlan2Task`，确保 Cop 的 index/table 阶段选择一致，并保持 warnings、`index_join` 和 MPP 分区信息。
- 扩展 Go 对齐逻辑时，逐项映射 `task.go` 中对应函数的分支依据；若简化模型缺少 session、schema、property 或具体算子能力，应先补正确数据模型或明确限制，不应把分支静默删掉。
- 新增重函数或类型时，同步检查 `HeavyFunctionNameMap`、`ContainHeavyFunction`、`TypeCode`、`needConvert`、`negotiateCommonType`、`getAvgRowSize` 和 TopN 下推条件。重函数返回列的 schema 与 local/global `by_items` 必须同时更新。
- 改变 Limit/TopN 计数时统一使用饱和加法，并覆盖 `u64::MAX`、offset 非零、虚拟列、不可下推表达式以及 Cop/MPP/Root 三种输入。
- 测试逻辑继续放在独立文件：核心附着/类型边界写入 `task_test.rs`，重函数物化写入 `task_heavy_function_optimize_test.rs`；对应 Go 语义分别核对 `task_test.go` 与 `task_heavy_function_optimize_test.go`。若新增跨优化器调用，还应扩展 `find_best_task_test.rs` 或最接近的独立规则测试，而不要在 `task.rs` 内嵌 `#[cfg(test)]` 模块。
- 兼容性风险集中在计划形状、schema 列下标、分区键类型、warning 传播和 Root/Cop/MPP 落点；性能风险集中在不必要的 Root 回退、深克隆、重复重函数求值以及错误的网络/行大小估算。

## 验证依据

- 源码全貌：`pkg/planner/core/task.rs`；符号清单包含 7 个主要数据类型族、`Task` 实现、公共附着分发器及全部专用函数，文件内没有 `#[cfg]` 项。
- crate 与模块入口：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`；后者公开 `pub mod task`，并以独立模块挂载 `task_test.rs` 和 `task_heavy_function_optimize_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query` 精确定位 `task.rs::attachPlan2Task`、`attach2Task4PhysicalTopN`、`convertPartitionKeysIfNeed4PhysicalHashJoin`、`tryReturnDistanceFromIndex`；调用图确认 `find_best_task.rs::Attach2Task → task.rs::attach2Task` 以及分发器到各专用附着函数的边。由于 `task`/`Task` 重名很多，歧义结果用源码位置和 `rg` 交叉核验。
- 上游源码：`pkg/planner/core/find_best_task.rs`，其候选计划流程和 `mockPhysicalPlan4Test::Attach2Task` 直接调用 `attach2Task`。
- Go 对照：`pkg/planner/core/task.go`；测试对照：`pkg/planner/core/task_test.go`、`pkg/planner/core/task_heavy_function_optimize_test.go`。
- Rust 独立测试：`pkg/planner/core/task_test.rs`、`pkg/planner/core/task_heavy_function_optimize_test.rs`；测试挂载位置为 `pkg/planner/core/lib.rs`。
- 本任务只生成说明文档，按计划不运行 Cargo。交付前执行任务文件给定的结构命令，要求目标存在且固定二级标题恰好为 11 个；同时人工复核本文能回答文件为何存在、主流程如何工作、与 Go 的差距以及安全扩展位置。
