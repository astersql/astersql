# `pkg/planner/core/plan_cost_ver2.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate（见 `pkg/planner/core/Cargo.toml`），是规划器第二版代价模型的集中实现。它同时服务两套计划表示：文件前半部的 `GetCanonicalPlanCostVer2` 面向 `base::PhysicalPlan` trait object 和真实物理算子层次；后半部的 `GetPlanCostVer2`/`GetPlanCost` 面向 `crate::task::PlanNode` 兼容树。两套路径都以 CPU、内存、磁盘、网络和请求等资源为成本依据，但使用的 `CostVer2` 类型不同：canonical 路径使用 `costusage_dependency::CostVer2`，兼容路径定义本文件自己的 `CostVer2`。

真实物理计划路径在 `pkg/planner/core/optimizer_runtime.rs` 中通过 `physicalop::InstallPlanCostVer2Router(GetCanonicalPlanCostVer2)` 安装；`pkg/planner/core/operator/physicalop/lib.rs` 为具体算子实现 `PhysicalPlan::get_plan_cost_ver2` 时优先调用该路由。任务候选的选择点位于 `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：读取 `TiDBCostModelVersion`，版本为 2 时取 v2 的标量总成本。兼容 `PlanNode` 路径由 `pkg/planner/core/find_best_task.rs::getTaskPlanCost` 调用本文件 `GetPlanCost`。

crate 根 `pkg/planner/core/lib.rs` 以 `pub mod plan_cost_ver2` 暴露该模块；同目录没有 `doc.go`，包级 Go 语义依据来自 `pkg/planner/core/plan_cost_ver2.go`。

## 核心职责

1. 为真实物理算子计算 Go 对齐的 v2 成本。`get_canonical_plan_cost_ver2_inner`按运行时类型覆盖 reader/exchange、Index Join/Hash Join、scan、projection/selection、聚合、sort/top-N 等算子，并在未命中特化分支时用通用的“子成本之和 + 本地成本”回退。
2. 生成可复核的成本公式。`canonical_term`、`canonical_net_cost` 和 `costusage_dependency::new_cost_ver2` 同时构造数值与 trace 表达式；部分分支特意保留 Go trace 的括号或组合形状，例如 ExchangeSender 与 root HashAgg。
3. 避免深物理计划递归耗尽 Rust 线程栈。`GetCanonicalPlanCostVer2` 使用显式 `Vec<CostRequest>` 工作栈和 `HashMap<CostRequestKey, CostVer2>` 缓存，按需先计算未完成子节点。
4. 提供兼容 `PlanNode` 的完整 v2 资源分量、基础公式与 v1/v2 分派。`GetPlanCostVer2` 按 `PlanKind` 分派，`GetPlanCost` 根据 `model_version` 选择 v1 或 v2。
5. 提供成本追踪扩展点。兼容路径的 `SetGenPlanCostTrace` 管理进程内全局 hook，节点成本算完后调用它。

## 主要符号

- `CardinalityContextAdapter<'a>`：把 `base::PlanContext` 适配为 cardinality crate 所需上下文，转发 session、expression 与 ranger 上下文。
- `canonical_row_size`、`canonical_avg_row_size`、`canonical_trace_row_size`、`canonical_schema_width`：从直方图、schema、表达式类型或回退类型宽度估算行宽；所有结果至少为 1，个别扫描公式再提升到 2。
- `canonical_number`、`canonical_factor`、`canonical_term`、`canonical_net_cost`：统一 Go 风格浮点格式、因子和值/公式的构造。
- `canonical_num_functions`：顶层 `ScalarFunction` 计 1，列/常量等计经验值 0.01，与 Go `numFunctions` 规则一致。
- `canonical_index_lookup_cost`：为 Index Join 内侧动态 lookup 计算 TableScan、Selection、TableReader 的扫描、过滤与网络成本；`canonical_index_join_batch_ratio` 固定返回经验批比 6。
- `CostRequestKey`、`CostRequest`、`CostWorklist`、`cost_of_child`：canonical 显式求值器的请求键、请求体、已算值/待算子节点状态。键由计划对象地址、任务类型和 INL 标志共同组成。
- `GetCanonicalPlanCostVer2`：真实 `PhysicalPlan` 的公开 v2 入口；负责工作栈调度、子节点缓存和环检测。
- `get_canonical_plan_cost_ver2_inner`：canonical 算子分派与公式主体，错误类型为 `expression_dependency::Error`。
- `CostTrace`、`CostVer2`、`CostVer2Factor`、`costVer2Factors`：兼容路径的 trace 条目、五维成本容器、单因子和默认因子集合。`CostVer2::GetCost` 返回五维之和，`Add`/`AddAssign` 同时拼接 trace，`div` 用至少 1 的除数分摊并发成本。
- `scanCostVer2`、`netCostVer2`、`filterCostVer2`、`aggCostVer2`、`groupCostVer2`、`orderCostVer2`、`hashBuildCostVer2`、`hashProbeCostVer2`、`doubleReadCostVer2`、`indexJoinSeekingCostVer2`：兼容树复用的基础公式。
- `getTableScanPenalty`：只给非临时 TiFlash 表扫增加小扫描、宽列和保序惩罚。
- `getNumberOfRanges`：递归精确累加自身与后代的 range 数，不为零 range 人为补 1。
- `GetPlanCostVer2`、`GetPlanCost`：兼容 `PlanNode` v2 主分派与 model version 总入口。
- `TraceHook`、`GEN_PLAN_COST_TRACE`、`SetGenPlanCostTrace`：线程安全的全局兼容 trace hook。

本文件没有 trait 或 enum 定义，也没有条件编译项；公开 API 与上述私有辅助函数均为普通模块级项。

## 执行流程

真实物理计划的主链如下：

1. `optimizer_runtime.rs` 初始化时安装 `GetCanonicalPlanCostVer2`；具体物理算子的 `get_plan_cost_ver2` 经 `routed_plan_cost_ver2` 进入本文件。
2. `GetCanonicalPlanCostVer2` 把根计划、任务类型和 INL 标志组成根请求，压入显式工作栈。
3. 对栈顶调用 `get_canonical_plan_cost_ver2_inner`。算子先取得非负基数与行宽，再按实际运行时类型进入特化公式。
4. 若公式需要尚未计算的子计划，`cost_of_child` 将该请求写入 `CostWorklist.pending` 并返回内部“pending”错误；外层捕获后把子请求压栈。若同一键已在栈上，则报告物理计划环。
5. 子请求成功后写入 `values`，再次处理父请求；根请求完成后移出缓存并返回。
6. `base_physical_plan.rs` 对返回的 `costusage::CostVer2` 调用 `get_cost()`，将标量用于候选比较。

canonical 分派的主要公式族为：reader 把存储侧子成本与网络成本相加并按 DistSQL 并发分摊；ExchangeReceiver 增加 MPP 网络成本且广播乘 3；Index Join 组合外侧、批量 lookup、启动/过滤、哈希或排序项；Hash Join 按 build/probe、任务引擎、过滤/键数、并发和 FullOuterJoin 尾扫计费；Index/Table Scan 按 `rows * log2(row width)` 与 TiKV/TiFlash 因子计费，并包含 TiFlash 启动/延迟物化分支；投影、过滤、聚合、排序和 TopN 则用输入基数、表达式复杂度、行宽、并发及内存项构造成本。未特化算子递归求子成本，并按识别到的基础类别补本地项。

兼容路径从 `find_best_task.rs::getTaskPlanCost` 进入 `GetPlanCost`：版本 2 调 `GetPlanCostVer2(...).GetCost()`，否则转到 `GetPlanCostVer1`。`GetPlanCostVer2` 依据 `PlanKind` 递归求子树，应用对应公式，最后在读锁下调用可选 trace hook。

## 数据与状态

canonical 路径的输入状态来自 `PhysicalPlan`：`stats_count`/`stats_info` 提供基数与直方图，`schema` 和表达式类型提供行宽，`children` 提供树结构，`s_ctx().GetSessionVars()` 提供投影、HashAgg、DistSQL 等并发变量与默认值。`PlanCostOption` 控制是否生成 trace；`inl: &[bool]` 是 Index Lookup/Join 上下文的一部分，因而也进入缓存键。

`CostWorklist.values` 只在单次 `GetCanonicalPlanCostVer2` 调用内存在，键中的 `plan` 是该次计算期间稳定借用的 trait object 数据地址；没有跨调用缓存。`pending` 每轮至多保存一个当前缺失子请求，用错误返回把控制权交回工作栈调度器。

兼容 `CostVer2` 保存 `cpu`、`memory`、`disk`、`network`、`request` 五个 `f64` 与 `Vec<CostTrace>`。构造时 `units.max(0.0)` 防止负单位进入成本；因子 `Name` 决定维度，未知名称默认归 CPU。`defaultVer2Factors` 返回固定经验权重。兼容 `PlanNode` 还携带行数、行宽、ranges、条件、分组/排序项、join 属性、并发和标志，这些字段直接影响分派公式。

全局可变状态只有兼容 trace hook：`OnceLock<RwLock<Option<Arc<dyn Fn + Send + Sync>>>>`。canonical 路径自身无全局可变状态，其 trace 随返回的 `costusage_dependency::CostVer2` 传播。

## 依赖与调用关系

上游调用与接线：

- `pkg/planner/core/optimizer_runtime.rs` 两处安装 `GetCanonicalPlanCostVer2` 路由。
- `pkg/planner/core/operator/physicalop/lib.rs` 的 `PhysicalPlan::get_plan_cost_ver2` 优先调用已安装路由；未安装时才回落到算子本地实现。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 在物理任务选择中根据 `TiDBCostModelVersion` 调 v2 或 v1，并把资源成本化为标量。
- `pkg/planner/core/find_best_task.rs::getTaskPlanCost` 对兼容 `Task::{Root,Mpp,Cop}` 调 `GetPlanCost`，进而用于 `compareTaskCost`。
- `pkg/planner/core/main_test.rs` 直接调用 `GetCanonicalPlanCostVer2` 验证 full outer join 等 canonical 成本。

下游 crate 依赖由 `pkg/planner/core/Cargo.toml` 明确声明：`base-dependency` 和 `physicalop-dependency` 提供计划接口/算子；`costusage-dependency` 提供 canonical 成本值与组合函数；`cardinality-dependency`、`statistics-dependency`、`chunk-dependency` 提供行数、直方图和类型宽度；`expression-dependency` 提供表达式、错误与列抽取；`property-dependency`、`kv-dependency`、`vardef-dependency` 分别提供任务类型、存储类型和会话变量。`nextgen` feature 只转发配置 feature，本文件没有 feature gate。

RustCodeGraph 将目标文件标为被 `base/plan_base.rs`、`find_best_task.rs`、`main_test.rs`、`operator/physicalop/base_physical_plan.rs`、`physical_index_hash_join.rs` 等文件使用；精确查询还显示 `GetCanonicalPlanCostVer2` 调用 `get_canonical_plan_cost_ver2_inner` 和请求键逻辑，`GetPlanCost` 调用 v1/v2 入口及 `CostVer2::GetCost`。

## 错误处理与边界

canonical 路径返回 `Result<costusage_dependency::CostVer2, expression_dependency::Error>`。子节点缺失一般使用零成本继续，例如无 IndexLookup index/table plan、无一元子节点；公式或子计划的真实错误通过 `?` 向上传播。显式调度用“physical child cost pending”作为内部控制信号，但仅当 `pending` 已设置时由外层消费；没有 pending 的错误原样返回。检测到重复在栈请求时报“cyclic physical plan during cost calculation”，根值丢失时报“physical plan cost was not calculated”。

关键数值边界包括：基数常夹到非负；并发从会话变量解析且只接受正数，否则使用默认值；兼容 `CostVer2::div` 将除数夹到至少 1；scan 的行宽通常至少 1，要求 Go 最小行宽语义的分支使用 2；兼容 `scanCostVer2` 允许 `log2(1)=0`，不会强制正成本；IndexJoin seeking 仅当 build rows 与 range 数都大于 1 时收费；HashAgg 的兼容内存项不随并发整除。

Go/Rust 浮点计算都可能产生很大有限值；canonical 路径对非法嵌套 reader 显式返回 `1e100` 成本而非结构错误，以使优化器避开该候选。文件未提供 I/O、序列化或用户输入校验，所有输入均假定来自已构造的规划器内部计划。

## 并发与资源生命周期

代价计算本身同步执行，不启动线程、异步任务、通道、事务或外部 I/O。canonical 工作栈、缓存和公式临时值都受单次函数调用栈管理；`PhysicalPlan` 仅不可变借用，结束即释放容器。

公式中的“并发”表示成本摊销参数而非本文件创建的运行时 worker：DistSQL、Projection、HashAgg、HashJoin 等项通过会话变量或算子字段做除法；内存、启动成本等是否分摊严格按分支处理。修改这些除法位置会改变计划选择，不能视为普通性能优化。

兼容 trace hook 的生命周期跨调用：首次访问初始化 `OnceLock`，`SetGenPlanCostTrace` 用写锁替换 `Option<Arc<...>>`，成本完成时用读锁调用。hook 必须 `Send + Sync`；锁中毒通过 `expect` panic。由于回调在读锁仍持有时执行，扩展 hook 时不应在回调内调用 `SetGenPlanCostTrace`，否则存在自阻塞风险。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/plan_cost_ver2.go`。Go 的 `GetPlanCost/getPlanCost` 依据 session `CostModelVersion` 分派，并在 cost-trace explain 时设置 trace flag；各 `getPlanCostVer24Physical*` 函数按物理算子保存/复用成本。Rust canonical 路径把这些分散的 Go 算子公式集中到运行时类型分派中，并通过 physicalop 全局路由接入真实算子。

已核对的等价意图包括：扫描使用 `rows * log2(rowSize) * factor`；列/常量复杂度为 0.01，顶层标量函数为 1；reader 合并存储子成本、网络与并发；TiKV/TiFlash 使用不同 CPU/内存/扫描/网络因子；TiFlash 表扫有启动与延迟物化项；IndexLookup/IndexJoin 有双读、批量与请求成本；HashAgg CPU 可按并发分摊而特定内存项不分摊；FullOuterJoin 对 build 未匹配行增加尾扫；cost trace 的组合形状在可观察处保持 Go 顺序与括号。

Rust 不是机械逐函数翻译：`GetCanonicalPlanCostVer2` 用显式工作栈取代 Go 递归，以支持很深的 Q64 类物理树；Rust 对环给出明确错误。后半部 `PlanNode` 兼容模型则是另一套本地表示及简化因子表，不等同于 Go 真实物理计划对象，不能用其单元测试单独证明 canonical 全算子语义。Go 的算子缓存字段与 recalculate flag 主要由 Rust physicalop/`costusage` 接线承担，不应误认为全在本文件后半部实现。

## 扩展指南

新增或修改真实物理算子成本时，应优先扩展 `get_canonical_plan_cost_ver2_inner`：先明确任务类型、输入/输出基数、行宽、引擎因子、并发摊销和 trace 形状，再通过 `cost_of_child` 请求子成本，避免恢复递归调用。若新算子只是通用包装器，应确认通用回退足够；若有 reader/MPP 边界、缓存或特殊网络语义，则需新增显式分支。需要新的行宽规则时集中修改 `canonical_*row_size`，并核对 Go `getAvgRowSize` 语义。

任何公式修改都应同步独立测试文件 `pkg/planner/core/plan_cost_ver2_test.rs`；涉及真实物理计划接线或复杂树时，还应扩展 `pkg/planner/core/main_test.rs` 或对应 `operator/physicalop/*_test.rs`，不要把测试嵌入本源文件。Go 对照测试位于 `pkg/planner/core/plan_cost_ver2_test.go`，其中扫描行宽/trace、优化因子、TiFlash、真实基数、HashAgg 内存与 full-join 用例是优先复核点。

扩展兼容 `PlanNode` 时，在 `GetPlanCostVer2` 增加 `PlanKind` 分支，并尽量复用基础公式；若增加资源维度，必须同步 `CostVer2::{GetCost,div}`、`AddAssign`、trace 及 hook 消费者。修改 `SetGenPlanCostTrace` 或回调时要保持独立测试串行隔离全局状态。

主要风险是成本数值或 trace 字符串的细小变化改变候选计划、跨引擎选择或 golden 输出；其次是把本应仅分摊 CPU 的成本整体除以并发、重复乘 IndexJoin 内侧基数、用声明 `flen` 代替统计行宽，以及破坏 explicit-worklist 的子请求/环检测不变量。此类修改必须同时比较 Rust 与 Go 的公式和边界测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/planner/core/plan_cost_ver2.rs` 读取了 1–2637 行；精确 `callers/callees` 查询用于核对 `GetCanonicalPlanCostVer2` 与 `GetPlanCost`。宽泛 `explore` 因符号重名返回噪声，未将其推断作为结论。
- Rust 源与接线：`pkg/planner/core/plan_cost_ver2.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/find_best_task.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/operator/physicalop/lib.rs`、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`。
- crate 边界：`pkg/planner/core/Cargo.toml`；确认 crate 名、库根、依赖与 `nextgen` feature。目标目录不存在 `doc.go`。
- 独立 Rust 测试：`pkg/planner/core/plan_cost_ver2_test.rs`，覆盖扫描公式/trace、资源分量求和、因子单调性、TiFlash penalty、range 精确累加、HashAgg 内存并发、函数计数、排序/哈希与 IndexJoin seeking 边界；`pkg/planner/core/main_test.rs` 含 canonical 直接测试。
- Go 对照：`pkg/planner/core/plan_cost_ver2.go` 与 `pkg/planner/core/plan_cost_ver2_test.go`；另参考 `pkg/planner/core/casetest/fulljoin/full_join_test.go` 的 v2 full outer join 测试。
- 本任务仅新增说明文档，未运行 Cargo 或代码测试；完成判据为上述事实交叉核对、人工审阅以及任务指定的 11 章节结构验证。
