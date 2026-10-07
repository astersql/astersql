# `pkg/planner/core/exhaust_physical_plans.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；`pkg/planner/core/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `pkg/planner/core/lib.rs` 通过 `pub mod exhaust_physical_plans;` 公开本模块。它把逻辑算子摘要与父节点要求的 `PhysicalProperty` 转换成若干 `PlanAlternative` 候选，职责位于逻辑优化之后、`FindBestTask` 代价比较之前。

需要区分“模块可见”与“生产主链已接线”：RustCodeGraph 将文件识别为已索引模块，但仓库文本调用搜索只找到 `pkg/planner/core/exhaust_physical_plans_test.rs` 对 `getHashJoins` 的直接使用；没有生产 Rust 文件调用顶层 `exhaustPhysicalPlans`。当前 `pkg/planner/core/find_best_task.rs` 的 mock 路径调用其自身的 `ExhaustPhysicalPlans4MockLogicalPlan`。因此本文件是可复用的 Rust 物理候选枚举实现及移植承载点，尚不能等同于 Go 优化器中已接入 `findBestTask` 的完整枚举主链。

文件没有条件编译项。它只直接使用同 crate 的 `find_best_task`、`task` 模块与标准库 `HashMap`/`HashSet`；`Cargo.toml` 的 `nextgen` feature 没有在本文件中形成分支。

## 核心职责

1. `exhaustPhysicalPlans` 按 `LogicalOperator` 分派：Join 与 Apply 进入专门枚举；TopN/Limit 生成本地与下推形态；Aggregation 生成 HashAgg/StreamAgg；其余一元算子用默认物理节点或 `base_alternatives` 包装。
2. Join 路径同时构造 TiDB HashJoin、IndexJoin 家族、MergeJoin 与 TiFlash MPP HashJoin，并用属性、统计、存储能力及 Hint 做可行性筛选。
3. Index Join 辅助逻辑负责扫描比例剪枝、内表形态准入、DataSource 路径选择、扫描任务构造和反馈标签回填。
4. Hint 辅助逻辑计算候选是否命中偏好、处理强制 Index Join、记录部分不可满足告警，并把“Hint 能否工作”返回给上层。
5. MPP 辅助逻辑判断 TiFlash 副本与表达式下推能力，估算 Broadcast/Shuffle 交换规模并选择广播侧。

该文件不计算最终代价，也不执行计划；它输出候选和子属性，最终选择应由 `find_best_task` 一类的任务枚举/代价模块完成。

## 主要符号

- 常量 `indexJoinPruneMinProbeRows`、`indexJoinPruneMinBuildRows` 分别设置 100000 与 100 的扫描比例剪枝门槛；`indexJoinMethod`、`indexHashJoinMethod`、`indexMergeJoinMethod` 是 Index Join 家族的稳定方法编号。
- `JoinHints` 汇总 Hash/Merge/Index、Broadcast/Shuffle 偏好与 Hash Join Build/Probe 侧约束。它是布尔摘要，不包含 Go 的完整 hinted-table、query block 或 session warning 上下文。
- `IndexJoinRuntimeProp` 保存内外 join key、额外条件、平均内表行数和 table-range/index-range 选择。
- `LogicalJoin` 保存 Join 类型、两侧统计/schema、join key、Hint、MPP/TiFlash 能力、伪统计、交换限制与告警。`Default` 给出可枚举但偏宽松的默认能力值。
- `LogicalAggregation`、`LogicalLimitTopN` 是聚合及 Limit/TopN 的轻量输入；`LogicalOperator` 是本模块支持的逻辑算子枚举；`ExhaustLogicalPlan` 将算子、已有候选与告警组合成入口对象。
- `exhaustPhysicalPlans` 是总分派入口，返回 `(Vec<Vec<PlanAlternative>>, bool)`；二维分组保持不同候选族/比较组，布尔值表示 Hint 是否可满足。
- `getHashJoin`/`getHashJoins` 构造 HashJoin 并处理左右 Build、Semi/AntiSemi、Hash Join v1/v2 以及冲突 Hint。
- `enumerateIndexJoinByOuterIdx`、`tryToEnumerateIndexJoin` 枚举两种外表方向的 IndexJoin/IndexHashJoin；`constructIndexJoinStatic`、`constructIndexHashJoinStatic` 建立节点骨架。当前 Rust 枚举没有在这里生成 `IndexMergeJoin`，但若外部候选包含该类型，Hint/方法识别辅助函数能够识别它。
- `shouldPruneIndexJoinByScanRatio` 实现统计门槛剪枝；`admitIndexJoinInnerChildPattern` 与 `checkIndexJoinInnerTaskWithAgg` 限制可作为内表的子树形态。
- `buildDataSource2IndexScanByIndexJoinProp`、`buildDataSource2TableScanByIndexJoinProp` 选择访问路径并调用 `convertToIndexScan`/`convertToTableScan`；没有可用路径时返回 `Task::invalid`。
- `applyLogicalHintVarEigen`、`handleForceIndexJoinHints`、`recordWarnings` 是 Hint 匹配、过滤与告警入口；`hasNormalPreferTask` 还维护 TopN/Limit Cop 候选只优先一次的枚举状态。
- `checkChildFitBC`、`preferMppBCJ`、`canTryMPPJoinForJoin`、`tryToGetMppHashJoin` 组成 MPP Join 可行性与 Broadcast/Shuffle 选择链。
- `exhaustPhysicalPlans4LogicalJoin` 与 `exhaustPhysicalPlans4LogicalApply` 是两个复杂逻辑算子的专门入口；`pushLimitOrTopNForcibly` 判定 Limit/TopN 的存储下推偏好与阈值满足情况。

## 执行流程

`exhaustPhysicalPlans` 首先匹配逻辑算子。普通一元算子产生一个子属性与父属性相同的 `PlanAlternative`；Mock 候选以零子节点包装。TopN/Limit 生成两组计划：下推形态把 `offset` 归零并把 `count` 饱和加为 `offset + count`，本地形态保留原值。Aggregation 为同一分组列同时产生 HashAgg 与 StreamAgg，并携带各自 Hint 偏好。

Join 主流程在 `exhaustPhysicalPlans4LogicalJoin` 中完成：

1. `getHashJoins` 先拒绝有排序要求的属性，再根据 Join 类型、Hash Join 版本和 Build/Probe Hint 枚举 TiDB HashJoin。冲突的左右强制 Hint 会写入 `LogicalJoin.warnings` 并撤销强制；`no_hash` 仅在没有更高优先级强制 Hint 时清空候选。
2. `tryToEnumerateIndexJoin` 分别令左右子树作为外表。每个方向先检查排序方向一致且排序列属于外表 schema，再计算 `avg_inner = join_rows / build_rows`，可选地按扫描比剪枝，最后生成 table-range 与 index-range 两种 IndexJoin/IndexHashJoin 骨架。
3. `handleForceIndexJoinHints` 按强制的 Index Join 家族过滤候选；过滤为空且不允许加 enforcer 时返回不可强制。
4. 排序属性为空或显式偏好 MergeJoin 时生成 MergeJoin 骨架。
5. `preferMppBCJ` 根据 Hint 或行数/字节限制决定先试 Broadcast；若失败但 MPP 基本条件成立，则回退 Shuffle。`tryToGetMppHashJoin` 要求无排序属性、MPP 已启用、两侧都有 TiFlash 副本且表达式可下推。
6. 每一族转为两个子节点、复制父属性的 `PlanAlternative`；`applyLogicalJoinHint` 标记命中候选。最终 `hint_can_work` 只有在没有强制偏好，或至少一个候选命中 Hint 时为真。

Apply 流程总是先产生 `PlanKind::Apply`；属性无排序要求时再附加一个右侧为 inner 的 TiDB HashJoin。Limit/TopN 的独立辅助 `pushLimitOrTopNForcibly` 则要求阈值满足、存储是 TiKV/TiFlash，且物理节点不是 Apply/Window，才认为偏好的下推实际可行。

Index Join 的 DataSource 转换是另一条辅助链：先筛选最小 `count_after_access` 的合法 index path 或 table path，构造 candidate，再转为扫描 `Task`；`completeIndexJoinFeedBackInfo` 把平均内表行数和索引列数写入计划标签。`constructDS2*ScanTask` 可按 `keep_order`/`desc` 临时构造排序属性。

## 数据与状态

输入和输出主要是拥有所有权的轻量 Rust 值，没有共享全局可变状态。`PhysicalProperty` 传入排序项、任务类型、期望行数、Index Join 属性和是否允许 enforcer；`alternative` 为每个子节点克隆该属性，因此后续子任务枚举可以独立消费。

统计状态集中在 `StatsInfo.row_count`/平均行宽与 `LogicalJoin.pseudo_stats`。扫描比例公式为：Index Join 估算扫描量 `build_rows + build_rows * probe_rows_one`，Hash Join 估算扫描量 `build_rows + inner_full_scan_rows`；只有阈值为正、Build 行数不少于 100、估计 Probe 行数不少于 100000、双方都不是伪统计且内表全扫行数为正时才剪枝。

计划节点上的可扩展状态通过 `labels: HashMap<String, f64>` 表达，例如 `use_outer_to_build`、`index_join_prop_child`、`extracted_other_eq`、`avg_inner_rows`、`index_columns` 和 `broadcast`。这些字符串标签是本实现内部约定，修改名称会影响消费这些标签的代码或测试。

可变状态只有调用者显式传入的对象：`getHashJoins` 可向 `LogicalJoin.warnings` 追加告警；`hasNormalPreferTask` 修改 `enumerateState.topNCopExist`/`limitCopExist`，防止同类 Cop 候选被重复标为普通偏好；`completeIndexJoinFeedBackInfo` 修改任务内的计划标签。

## 依赖与调用关系

上游方面，`pkg/planner/core/lib.rs` 暴露模块，独立测试 `pkg/planner/core/exhaust_physical_plans_test.rs` 直接调用 `getHashJoins`。截至本次核验，生产 Rust 源中没有 `exhaustPhysicalPlans` 的直接调用；RustCodeGraph 对文件的模块级 “used by” 信息不能替代精确的符号调用边。Go 对应入口则由 `pkg/planner/core/find_best_task.go` 的优化过程调用，是 Go 生产规划链的一部分。

模块内部的关键调用边经 RustCodeGraph 核验为：`exhaustPhysicalPlans -> exhaustPhysicalPlans4LogicalJoin/exhaustPhysicalPlans4LogicalApply`；`exhaustPhysicalPlans4LogicalJoin -> getHashJoins/tryToEnumerateIndexJoin/handleForceIndexJoinHints/preferMppBCJ/canTryMPPJoinForJoin/tryToGetMppHashJoin/alternative`；`enumerateIndexJoinByOuterIdx -> shouldPruneIndexJoinByScanRatio/constructIndexJoinStatic/constructIndexHashJoinStatic`；`tryToGetMppHashJoin -> canTryMPPJoinForJoin/isJoinChildFitMPPBCJ/hash_join_plan`。

下游类型与函数主要来自 `find_best_task.rs`（`AccessPath`、`DataSource`、`PhysicalProperty`、`PlanAlternative`、路径候选及扫描转换）和 `task.rs`（`PlanNode`、`Task`、`StatsInfo`、`JoinType`、`StoreType` 等）。本文件自身没有直接第三方 crate API 调用；其 crate 归属和可选 feature 由 `pkg/planner/core/Cargo.toml` 定义。

## 错误处理与边界

公开入口不返回 `Result`：不可行候选通常用空 `Vec` 表示，路径缺失用 `Task::invalid("no index join index path")` 或 `Task::invalid("no index join table path")` 表示，Hint 问题通过布尔返回值、`Option<String>` 或 `LogicalJoin.warnings` 表示。这与 Go 入口返回 `(plans, hintCanWork, error)` 的接口不同，调用者不能期待 Rust 入口传播 Go 式错误。

数值边界有显式防护：零 Build 行数使平均内表行数为零；非正全扫行数禁用比例剪枝；Broadcast/Shuffle 规模用 `is_finite` 拒绝 NaN/无穷；store 数量至少按 1 计算；TopN/Limit 用 `saturating_add` 避免 `u64` 溢出。`getColsNDVLowerBoundFromHistColl` 没有直方图值时返回 1。

结构边界包括：混合升降序或越出外表 schema 的排序不能用于 Index Join；多值索引不参与 `buildDataSource2IndexScanByIndexJoinProp`；TiFlash 偏好会拒绝 DataSource 作为 Index Join 内表模式；OuterJoin/Fence 永不作为可接纳内表模式；MPP Join 要求两侧都有 TiFlash 副本且表达式可下推。

`exhaustPhysicalPlans` 的兜底匹配含 `unreachable!()`，安全性依赖 `LogicalOperator` 前面的显式分支保持穷尽。未来添加枚举成员时必须同步更新分派，否则可能在运行时 panic。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有候选在调用栈内同步生成，`Vec`、`HashMap`、`HashSet` 及克隆的计划/属性由 Rust 所有权自动释放。

生命周期上的主要成本是候选复制与扩张：父属性会按子节点数克隆；Join 最多组合多个算法族和左右方向；Index Join 每个外表方向当前产生 table-range/index-range × IndexJoin/IndexHashJoin 四个骨架。扩展候选时应关注候选数乘法效应以及 `PlanNode`/统计克隆成本。`warnings` 与 `enumerateState` 由调用者持有，因此同一个对象被重复枚举会保留累积状态；调用者若需要独立尝试，应创建新摘要或明确重置状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/exhaust_physical_plans.go`。Rust 保留了 Go 文件中的主要命名和功能分区：Hash Join Build/Probe 枚举、Index Join 扫描比公式与 100/100000 门槛、内表准入与扫描构造、Hint 处理、MPP Broadcast/Shuffle 规模估算、Join/Apply 入口和 Limit/TopN 下推判断。两个 Rust 单测也明确验证 Semi Join Build 侧与 Hash Join v1 Hint 的 Go 语义。

Rust 当前是摘要化移植而非逐字段等价实现。Go `exhaustPhysicalPlans` 接收真实 `base.LogicalPlan`/`memo.GroupExpression`，调用各物理算子的完整 `ExhaustPhysicalPlans4Logical*`，并返回 `error`；Rust 使用本地 `LogicalOperator` 摘要和统一 `PlanNode`。Go Join 路径还处理 MPP Hint 冲突、Flash 属性、Full Outer Join、NAAJ、failpoint、session warning、真实 schema/property 初始化和更丰富的 IndexMergeJoin/Index Join path 信息；Rust 对应代码只保留核心候选与标签。Go Apply 还计算相关列 NDV、缓存命中率、内存配额与有序输出期望行数，Rust Apply 仅生成 Apply/HashJoin 骨架。

另一个关键迁移差异是接线：Go 文件的入口处于生产 `findBestTask` 调用链，Rust 顶层入口目前没有生产调用者。文档或新功能不得仅凭同名函数就宣称完整 Go 行为已经接通。

## 扩展指南

新增逻辑算子时，应同时扩充 `LogicalOperator`、`exhaustPhysicalPlans` 分派、子属性数量和必要的 Hint/告警处理；不要依赖兜底 `unreachable!()`。新增 Join 算法时，应在 `exhaustPhysicalPlans4LogicalJoin` 中建立独立候选族，并同步 `applyLogicalJoinHint`、强制 Hint 过滤、`getIndexJoinSideAndMethod`（若属于 Index Join 家族）及候选爆炸控制。

调整 Index Join 时，优先修改 `enumerateIndexJoinByOuterIdx`、`shouldPruneIndexJoinByScanRatio`、内表准入和 DataSource 转换这一完整链条；统计公式或阈值变化必须对照 Go 同名函数，避免 Rust/Go 选择漂移。路径标签变化还要检查 `find_best_task.rs`、`task.rs` 的消费者。MPP 变化则应从 `canTryMPPJoinForJoin` 到 `preferMppBCJ`、`checkChildFitBC`、`tryToGetMppHashJoin` 成组检查。

测试必须放在独立文件，优先扩展同目录 `pkg/planner/core/exhaust_physical_plans_test.rs`，不要把 `#[cfg(test)]` 测试嵌入本源文件。当前该测试文件直接覆盖的本模块行为只有两个 `getHashJoins` 场景；新增剪枝、MPP、Hint、Apply、Limit/TopN 或路径构造行为时应补相应定向测试，并与 `pkg/planner/core/exhaust_physical_plans_test.go` 及 Go 同名函数的边界保持一致。若未来把顶层入口接入生产 Rust 规划链，还应增加从真实逻辑计划到候选/最优任务的集成级验证。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/planner/core/exhaust_physical_plans.rs` 分段读取 1–1099 行；确认常量、类型、函数、分派、剪枝、Hint、MPP 和 Apply/Limit 边界。
- 符号与调用图：RustCodeGraph `explore "exhaust_physical_plans.rs ExhaustPhysicalPlans physical plan enumeration"`，并对 `exhaustPhysicalPlans`、`exhaustPhysicalPlans4LogicalJoin`、`enumerateIndexJoinByOuterIdx`、`tryToGetMppHashJoin`、`buildDataSource2IndexScanByIndexJoinProp` 执行 `callers`/`callees`。同名 Go/Rust 符号造成 caller 输出不完整，因此再用精确 `rg` 核对生产引用。
- crate 与模块边界：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`。
- Rust 上游与测试：`pkg/planner/core/find_best_task.rs`、`pkg/planner/core/exhaust_physical_plans_test.rs`；全仓测试符号搜索确认只有该独立测试中的两个测试直接调用本模块 API，其余测试主体属于 `index_join_path`。
- Go 语义对照：`pkg/planner/core/exhaust_physical_plans.go`、`pkg/planner/core/exhaust_physical_plans_test.go`，重点复核总分派、扫描比例剪枝、Join/Apply、MPP、Hint 与 Limit/TopN 下推。
- 人工结论：本文件为何存在、候选如何生成和筛选、哪些状态被修改、错误如何表达、尚未接入何处以及安全扩展时需同步哪些独立测试，均可由上述符号和文件反查；未把 Go 的完整生产行为误写为 Rust 现状。
