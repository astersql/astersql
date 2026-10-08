# `pkg/planner/core/plan_cost_ver1.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate，是第一版（Ver1）物理计划代价模型的 core 侧实现。`pkg/planner/core/lib.rs` 以公开模块 `plan_cost_ver1` 暴露它；`pkg/planner/core/Cargo.toml` 表明该 crate 直接依赖物理算子、计划基类、基数估计、统计信息、会话变量、物理属性和代价选项等 crate。

文件同时服务两种计划表示：

- `GetCanonicalPlanCostVer1` 面向真正的 `dyn base::PhysicalPlan` 树，是运行时优化链和 Explain 使用的入口。`optimizer_runtime.rs` 将它安装到 `physicalop::InstallPlanCostVer1Router`，物理算子的 `get_plan_cost_ver1` 会优先经过该全局路由。
- `GetPlanCostVer1` 面向 `crate::task::PlanNode`，使用本文件定义的 `PlanCostOption`/`CostFactors`。它被 `plan_cost_ver2.rs::GetPlanCost` 用作版本不为 2 时的标量回退，并由同目录独立测试直接验证。

因此，阅读或修改本文件时必须先区分 canonical 真实计划路径与 `PlanNode` 路径；两者目标相同，但类型、配置来源、错误模型和覆盖的算子细节并不完全相同。

## 核心职责

1. 对真实物理计划递归计算 Ver1 代价。`GetCanonicalPlanCostVer1` 先取得输出行数、行宽和子树代价，再按具体物理算子叠加扫描、CPU、内存、网络、seek 和并发调度成本。
2. 为 canonical 路径提供行宽、seek、DistSQL 并发和网络因子的适配函数：`canonical_row_size`、`canonical_seek_cost`、`canonical_dist_sql_scan_concurrency`、`canonical_network_factor` 和 `canonical_children_cost`。
3. 对轻量 `PlanNode` 表示提供可配置的 Ver1 公式及递归分派，包括读算子、扫描、连接、聚合、排序、点查、UnionAll 和 ExchangeReceiver。
4. 实现缓存与重算语义。`PlanNode.cost_v1` 已有值且未设置 `CostFlagRecalculate` 时直接返回；重算完成后写回缓存。
5. 实现真实基数选择。`getCardinality` 在 `CostFlagUseTrueCardinality` 下使用 `actual_rows / actual_probe_count`，否则使用估计行数。

## 主要符号

- `DISTINCT_FACTOR = 0.8`：canonical 聚合与 `PlanNode` 聚合公式使用的 DISTINCT 内存折算常量。
- `CardinalityContextAdapter`：把 `base::PlanContext` 适配为基数模块所需的 `CardinalityContext`，转发会话、表达式和 ranger 上下文。
- `GetCanonicalPlanCostVer1(plan, task, option) -> Result<f64, expression_dependency::Error>`：真实物理计划的公开入口；通过 `as_any` 向下转型识别算子。
- `modelVer1`、`modelVer2`：模型版本号；本文件的 `getCardinality` 保留 Ver1 的零估计行语义。
- `CostFlagRecalculate`、`CostFlagUseTrueCardinality` 与 `hasCostFlag`：`PlanNode` 路径的位标志协议。
- `CostFactors`：CPU、内存、磁盘、网络、seek、请求、扫描因子及多类并发度的集中配置；`Default` 给出稳定的本地默认值。
- `PlanCostOption`：携带标志、`CostFactors` 和 trace 开关。当前文件的 `PlanNode` 公式读取前两者；`trace` 在此文件中不产生记录。
- `getCardinality`、`getOperatorActRows`：分别选择估计/真实基数，以及从 `actual_rows` 标签读取实际行数。
- `getCost4Physical*` 系列：Projection、IndexLookup、三类 IndexJoin、Apply、MergeJoin、HashJoin、StreamAgg、HashAgg、Sort 的自身代价公式。
- `estimateNetSeekCost`、`getTableNetFactor`：分别递归累计范围 seek，以及为临时表将网络因子归零。
- `GetPlanCostVer14PhysicalIndexMergeReader`：IndexMergeReader 的专用 `PlanNode` 公式。
- `GetPlanCostVer1(plan, task, option) -> f64`：`PlanNode` 主分派入口，负责缓存、递归和最终写回。

## 执行流程

canonical 路径的主流程如下：

1. `GetCanonicalPlanCostVer1` 从 `stats_count` 取得非负行数，由 `canonical_row_size` 估算输出宽度，并调用 `canonical_children_cost` 递归累计子树代价。
2. `canonical_children_cost` 根据父算子调整子任务类型：MPP 继续使用 MPP，IndexLookupReader 使用 `CopMultiReadTaskType`，TableReader/IndexReader 使用 `CopSingleReadTaskType`，其他算子沿用调用者任务类型。
3. 主入口按动态类型匹配算子。扫描按行数、行宽和扫描因子计费；Reader 加网络、seek 后除以 DistSQL 并发；Projection/Selection、Sort/TopN、Hash/StreamAgg、HashJoin、Exchange 节点分别叠加相应资源成本。
4. TableReader/IndexReader 会使用 `GetTblStats` 与 `cardinality::GetAvgRowSize` 估算网络行宽。TiFlash BatchCop 下若子树含无分组、同时含 `COUNT` 和 `SUM` 的 partial AVG 形态，则 Reader 成本乘以 `0.4`。
5. 不在显式分支中的叶子返回至少 `1` 的自身行数成本；未知的非叶节点自身成本为 `0`，但仍保留已递归得到的子树成本。

`PlanNode` 路径的主流程如下：

1. `GetPlanCostVer1` 首先检查 `cost_v1` 缓存；仅 `CostFlagRecalculate` 能绕过缓存。
2. 它计算当前基数以及 Root/MPP 布尔状态，再按 `PlanKind` 分派。
3. Reader 会切换 Cop/MPP 子任务、加入网络与 seek 并按 DistSQL 并发折算；扫描使用 `scan_factor`；连接和聚合委托相应 `getCost4Physical*`；UnionAll 取最大子代价而不是求和。
4. TableReader 与 UnionAll 在 MPP 强制执行且未要求重算时把代价除以 `1_000_000_000`，用于强烈偏向被强制的 MPP 计划。
5. 未显式覆盖的 `PlanKind` 只累加全部子代价。结果写入 `plan.cost_v1` 后返回。

## 数据与状态

- canonical 路径主要读取真实计划树的统计信息、schema、扫描 range、存储类型、聚合函数、连接方向及会话变量，不在本文件中写入计划缓存。其配置来自 `costusage_dependency::PlanCostOption` 和 `SessionVars`。
- `PlanNode` 路径从 `StatsInfo`、`labels`、`flags`、`ranges`、`row_size`、表达式/条件/聚合列表及 `inner_child` 等字段取数，并唯一地修改 `cost_v1`。
- `CostFactors::default` 中的重要默认值包括 seek `20`、DistSQL 并发 `15`、Projection 并发 `4`、HashJoin 并发 `5`、IndexLookup 并发 `4`、批大小 `20_000`。所有并发均经 `concurrency` 至少归一为 `1`，避免除零。
- `getCardinality` 的真实基数分支把 `actual_probe_count == 0` 定义为零；非零时将每次 probe 的实际行数截断到非负。估计分支直接保留 `plan.rows()`，与 Go Ver1 保留零基数的意图一致。
- canonical 的会话并发解析失败时回退 `DefDistSQLScanConcurrency`，且至少为 `1`；网络因子解析失败时回退 `DefOptNetworkFactor`。

## 依赖与调用关系

上游调用关系：

- `pkg/planner/core/optimizer_runtime.rs` 在物理优化入口安装 `GetCanonicalPlanCostVer1` 路由；随后物理计划的 `get_plan_cost_ver1` 经路由计算最终候选成本。
- `pkg/planner/core/operator/physicalop/lib.rs` 的 `PLAN_COST_VER1_ROUTER: OnceLock` 保存路由；具体物理算子优先调用 `routed_plan_cost_ver1`，未安装时才回退各算子本地实现。
- `pkg/session/runtime/explain_query.rs::explain_physical_plan_cost` 直接调用 canonical 入口；成功且有限的结果用于 Explain，否则回退非负统计行数。
- `pkg/planner/core/plan_cost_ver2.rs::GetPlanCost` 按模型版本选择 `PlanNode` 的 Ver2 或本文件的 `GetPlanCostVer1`。

下游依赖关系：

- `base-dependency` 提供 `PhysicalPlan`、`PlanContext`、schema、统计与 JoinType 等基础接口。
- `physicalop-dependency` 提供具体物理算子类型、Reader/Scan 属性和表统计提取。
- `cardinality-dependency`、`statistics-dependency` 用于 Reader 行宽与直方图回退。
- `variable-dependency`、`vardef-dependency` 提供 CPU/内存/网络因子和并发系统变量。
- `property-dependency`、`costusage-dependency` 提供真实计划路径的任务类型和代价选项。
- `crate::task` 提供第二套 `PlanKind`、`PlanNode`、`TaskType`、`StoreType` 以及 MPP 行宽/seek 聚合工具。

RustCodeGraph 对 `GetCanonicalPlanCostVer1` 的节点解析确认其调用 `canonical_row_size`、`canonical_seek_cost`、`canonical_dist_sql_scan_concurrency`、`canonical_network_factor`、`canonical_children_cost` 和局部 `contains_avg_partial`；图的调用者结果只直接识别到递归边和 `main_test.rs`，运行时路由安装与 Explain 调用由源码搜索补充确认。

## 错误处理与边界

- canonical 路径返回 `Result`，目前显式可传播的错误来自递归的 `GetCanonicalPlanCostVer1`/物理计划代价调用；`canonical_children_cost` 使用 `Iterator::sum<Result<...>>()` 在任一子节点失败时立即返回错误。
- 系统变量无法取得或解析时使用定义好的默认值，不把配置解析失败升级为代价计算错误。
- 行宽始终至少为 `1`；行数多数按 `max(0.0)` 归一；并发至少为 `1`。这些边界避免负资源量和除零，但不能自动消除输入中的非有限浮点值。
- `PlanNode` 入口不返回错误。缺失的指定子节点在 `child_cost` 中按零成本处理，`child_rows` 则回退当前节点基数；这是一种容错行为，也意味着畸形树不会在本入口中显式报错。
- `IndexReader`/`TableReader` 的 `PlanNode` 公式允许缺少子节点并回退当前节点；canonical Reader 则在无子节点时使用自身行数/行宽。
- `PlanCostOption.trace` 在本文件没有消费方，不能据此声称 Ver1 会生成 trace。
- `canonical_seek_cost` 对 Scan 的空 ranges 仍按一个 range 计费，而 Go `estimateNetSeekCost` 直接使用 `len(Ranges)`；这是需要兼容性测试约束的现存差异。

## 并发与资源生命周期

本文件没有启动线程、异步任务、通道或事务，也不持有外部资源。递归计算在调用线程内同步完成，临时 `Vec`、直方图回退值和适配器随栈帧释放。

共享状态位于物理算子 crate 的 `OnceLock<PlanCostVer1Router>`，本文件只提供可安装的函数。安装使用 `OnceLock::set`，重复安装返回已有函数；`optimizer_runtime.rs` 明确忽略重复安装结果，因此路由一旦设置，在进程生命周期内保持不变。

`PlanNode.cost_v1` 是树节点内缓存，要求调用方以 `&mut PlanNode` 独占访问；重新计算需要显式设置 `CostFlagRecalculate`。修改统计、因子或树结构后若复用已缓存节点却不要求重算，会得到旧值，这是扩展时最重要的生命周期约束。

公式中的“并发”是代价折算参数而非本文件创建的实际执行并发：Reader 除以 DistSQL 并发，Projection/HashJoin/HashAgg 还会加入调度成本或按 worker 分摊 CPU。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/plan_cost_ver1.go`。Rust 保留了 Go 的主要结构：标志判断、基数选择、按物理算子递归、Reader 网络/seek/并发折算、三类 IndexJoin、Apply、Merge/Hash Join、两类聚合、Sort/TopN、点查、UnionAll、ExchangeReceiver，以及 MPP 强制计划的极小代价偏置。

已由源码和测试明确对齐的例子包括：

- Ver1 估计基数允许为零；`plan_cost_ver1_test.rs::ver1_zero_cardinality_and_small_sort_match_go_flooring_rules` 同时验证小 Sort 的 `max(2)` 下限。
- UnionAll 使用“最大子代价 + `(1 + 子数) * concurrencyFactor`”，并非子代价总和；独立 Rust 测试期望结果为 `30.0`。
- ExchangeReceiver 使用“子成本 + 子基数 * networkFactor”，不乘行宽；独立 Rust 测试期望结果为 `33.0`。
- `main_test.rs::go_merge_46_full_join_tail_scan_increases_both_cost_models` 验证 FullOuter HashJoin 的 Ver1 成本高于普通内连接。

不能把两套实现视为逐行等价：canonical Rust 是面向真实类型的集中路由，`PlanNode` 是可配置的简化表示；Go 则由每个具体物理计划的 `GetPlanCostVer1` 委托同文件中的算子函数。Rust 的默认因子表也把部分 Go 会话变量固化为本地默认值。已观察到的细节差异还包括 canonical 空 range 至少计一次 seek，以及未知非叶算子仅保留子成本。后续对齐应以 Go 公式、真实算子字段和独立回归测试共同决定，不能只修改其中一套 Rust 路径。

## 扩展指南

- 新增真实物理算子的 Ver1 公式时，在 `GetCanonicalPlanCostVer1` 增加精确的 downcast 分支，并确认 `canonical_children_cost` 是否需要新的任务类型映射；同时检查算子自己的 `get_plan_cost_ver1` 路由回退行为。
- 新增 `PlanKind` 时，在 `GetPlanCostVer1` 添加对应分支；若 canonical 也存在对应真实算子，应同步两条路径并说明允许的差异。
- 调整资源公式时优先复用 `CostFactors` 或会话变量来源，明确单位和并发折算顺序。扫描/Reader 修改要同时审查行宽、网络、seek、临时表与 TiFlash/MPP 分支。
- 任何可能改变已缓存结果的上下文或配置，都要确认调用方何时设置 `CostFlagRecalculate`，避免缓存跨配置复用。
- 测试必须继续放在独立文件。轻量公式与边界应扩展 `pkg/planner/core/plan_cost_ver1_test.rs`；真实 `PhysicalPlan` 路由行为可扩展 `pkg/planner/core/main_test.rs` 或相应 `operator/physicalop/*_test.rs`，不要把测试内嵌回生产文件。
- 与 Go 对齐时应逐项核对 `pkg/planner/core/plan_cost_ver1.go` 的同名函数，并关注 session factor、true cardinality、range/row-size、FullOuterJoin、MPP 强制偏置和错误传播。性能风险主要来自递归重复遍历（特别是 seek 与 AVG partial 检测）和错误的并发缩放；兼容风险主要来自 canonical 与 `PlanNode` 公式漂移。

## 验证依据

- 完整阅读：`pkg/planner/core/plan_cost_ver1.rs`、`pkg/planner/core/plan_cost_ver1_test.rs`、`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`、`pkg/planner/core/plan_cost_ver1.go`。
- 直接调用证据：`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/operator/physicalop/lib.rs`、`pkg/session/runtime/explain_query.rs`、`pkg/planner/core/plan_cost_ver2.rs`、`pkg/planner/core/main_test.rs`。
- RustCodeGraph：`status` 显示索引可用；`query GetCanonicalPlanCostVer1 --kind function` 定位到本文件第 172 行；`node`/`callees GetCanonicalPlanCostVer1` 确认 canonical 辅助函数调用。初次 `files --filter pkg/planner/core/plan_cost_ver1` 没有命中，且部分 callers 解析不完整，因此模块、路由与 Explain 边使用 `rg` 和源码读取补齐。
- 符号清单通过 `rg` 提取本文件的常量、结构体、实现和函数，并与 Go 文件全部顶层函数名对照。
- 独立测试证据：`plan_cost_ver1_test.rs` 的三个测试覆盖零基数/Sort、UnionAll 和 ExchangeReceiver；`main_test.rs` 覆盖真实计划 FullOuter HashJoin 的相对代价。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构校验，并人工检查唯一生产物、源码链接、无整段源码复制和未验证结论的边界表述。
