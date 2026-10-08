# `pkg/planner/core/operator/physicalop/base_physical_plan.rs`

## 文件定位

本文件位于 `astersql-planner-core-operator-physicalop` crate，是 Rust 物理计划层的公共基座和“逻辑计划 → 物理任务”规范路由中心。crate 根文件 `pkg/planner/core/operator/physicalop/lib.rs` 以 `mod base_physical_plan` 装载它，并通过 `pub use base_physical_plan::*` 向 planner core、executor 和 DDL 等上层重新导出公开接口。

它处在物理优化主链的中段：`pkg/planner/core/optimizer_runtime.rs::physical_optimize_without_post` 安装 `CanonicalFindBestTaskRouter`，清空本线程缓存，然后经 `FindBestTask` 进入本文件；本文件枚举物理候选、递归优化孩子、挂接成 `Task` 并按代价选优。优化器后处理还直接调用这里的 `FlattenNestedMPPReadersBelowRoot`、`PopulateIndexJoinInnerPlans` 以及多组 `Align*PlanIDs` 函数，整理最终执行树。

因此它不只是 Go 同名文件的机械翻译。前约 780 行承接 `base_physical_plan.go` 的公共能力；其余大部分 Rust 代码集中承载规范路由、MPP/IndexJoin 挂接、访问路径选择和 Go 可观察 PlanID 对齐。实际算子结构仍分别定义在同 crate 的 `physical_*.rs` 中。

## 核心职责

1. 以 `BasePhysicalPlan` 保存所有物理算子共享的上下文、孩子、孩子所需属性、统计、Schema、代价缓存、探测父节点和 TiFlash shuffle 信息，并实现 `base::Plan`、`base::PhysicalPlan`。
2. 提供通用操作：隐藏物理分区列追加、统计版本收集、安全克隆、IndexJoin 属性准入、成本汇总、内存估算和探测次数估算。
3. 通过 `FindBestTaskRouter`/`InstallFindBestTaskRouter` 建立依赖倒置边界，使 physicalop crate 不反向依赖 planner core；`CanonicalFindBestTaskRouter` 是 planner core 安装的规范实现。
4. 以 `canonical_find_best_task_router_inner` 为主循环，根据 `PhysicalProperty` 枚举候选、递归生成孩子任务、执行算子专用挂接、计算成本并缓存最优任务。
5. 针对 DataSource、TiKV/TiFlash reader、MPP exchange、HashJoin、IndexJoin、聚合、Sort/TopN/Limit、Projection/Window、Lock 和 CTE 等形态完成 Rust 侧的枚举与树形整合。
6. 在 Rust 的“先枚举模板再克隆挂接”模型与 Go 的构造顺序之间对齐 PlanID、列 ID、reader 边界、统计与分区元数据，保证执行和 EXPLAIN 可观察语义一致。

## 主要符号

- `ScanCardinalityContext<'a>`：把 `base::PlanContext` 适配为 `cardinality::CardinalityContext`，向 scan 行数调整逻辑提供 session、表达式和 ranger 上下文。
- `CANONICAL_TASK_CACHE`、`cached_canonical_task`、`store_canonical_task`、`ResetCanonicalTaskCache`：线程本地最优任务缓存。键包含逻辑对象地址、Plan ID、类型、Schema 列唯一 ID 和属性哈希，值以 `Task::copy` 存取，避免返回共享可变任务。
- `BasePhysicalPlan`：共享状态实体。重要字段包括 `Plan`、`children_req_props`、`children`、两版代价缓存、`probe_parents`、`schema`、`stats`、`stats_table_name`、`store_type` 和 `TiFlashFineGrainedShuffleStreamCount`。
- `BasePhysicalPlan::{New, SetChildren, SetChild, SetChildrenReqProps, SetXthChildReqProps, CloneWithNewCtx}`：构造、树结构/属性维护与深克隆入口；修改孩子会使代价缓存失效。
- `BasePhysicalPlan::{GetPlanCostVer1, GetPlanCostVer2}`：基座自身成本为零，只汇总孩子；除非设置 `COST_FLAG_RECALCULATE`，否则复用缓存。
- `AddExtraPhysTblIDColumn`：给分区表 Schema 和 `ColumnInfo` 同步追加一次隐藏的 `ExtraPhysTblID` 列，并从 session 分配唯一列 ID。
- `CollectPlanStatsVersion`、`GetStatsInfo`：递归收集扫描表的统计版本；IndexLookup 只遍历 index plan，符合其 table plan 不单独设置统计的约定。
- `SafeClone`：捕获 `clone_physical` 的 panic 并转换为 `expression::Error`，对应 Go 的 `recover` 边界。
- `AdmitIndexJoinProps`、`AdmitIndexJoinProp`、`AdmitIndexJoinTypes`：只允许非 MPP 的孩子/任务类型继承 `IndexJoinProp`。
- `FindBestTaskRouter`、`FIND_BEST_TASK_ROUTER`、`InstallFindBestTaskRouter`、`FindBestTask`：一次安装的全局路由函数表和稳定调用入口；未安装会返回显式错误。
- `CanonicalFindBestTaskRouter`、`canonical_find_best_task_router_inner`：规范寻优入口与内部递归实现；覆盖缓存、特例分派、候选枚举、孩子寻优、挂接、成本比较和结果提交。
- `ExhaustPhysicalPlans`：按具体 `logicalop` 类型分派到各算子的枚举函数，并直接处理 gather、scan、reader 等需要集中接线的路径。
- `PopulateIndexJoinInnerPlans`、`build_index_join_lookup_scan`、`build_lookup_scan_from_logical`：为已选 IndexJoin 补齐 lookup scan/reader 内层执行计划。
- `FlattenNestedMPPReaders`、`FlattenNestedMPPReadersBelowRoot`、`convert_canonical_mpp_task_to_root`：消除不合法的嵌套 MPP reader，同时保留根 reader 和 IndexJoin 等边界。
- `attach_canonical_*`、`enforce_canonical_*`：算子专用挂接和属性强制器，包括 Root/MPP HashJoin、Lock、IndexJoin、scan filter、Sort、MPP 分区、Projection/Window、TopN/Limit 和聚合。
- `AlignSelectedRootCandidatePlanIDs`、`AlignScalarMppAggregationPlanIDs`、`AlignScalarAntiSemiHashJoinPlanIDs`、`AlignFinalSemiIndexJoinPlanIDs`、`AlignGroupedMultiKeyIndexHashJoinPlanIDs`、`AlignFinalMppSemiHashJoinPlanIDs`、`AlignNestedSemiIndexJoinPlanIDs`、`AlignDecorrelatedSemiIndexJoinReaderPlanIDs`：只在已识别的严格树形上重建或重编号，复现 Go 枚举/挂接顺序。
- `task_hash_cols_satisfy_output`、`hash_columns_equivalent_on_inner_join`、`index_join_keys_match_children`：MPP 分区键和 IndexJoin 键的结构性校验辅助函数。

## 执行流程

完整应用中的主流程如下：

1. `optimizer_runtime.rs::physical_optimize_without_post` 调用 `InstallFindBestTaskRouter(CanonicalFindBestTaskRouter)`。`OnceLock` 使安装幂等；随后 `ResetCanonicalTaskCache` 隔离本次优化。
2. 优化器构造根 `PhysicalProperty`，调用 `FindBestTask`。入口先检查路由是否安装，再把逻辑节点和属性交给规范路由。
3. `CanonicalFindBestTaskRouter` 调用内部递归；若根任务实际包含 grouped two-phase MPP 聚合，则把 MPP task 转为带 reader/exchange 边界的 Root task。
4. `canonical_find_best_task_router_inner` 先处理强制 Root 的 Lock，再查线程缓存。DataSource 进入 `find_best_data_source_task`；CTE/CTETable、空 Selection 走专用短路。一般逻辑节点调用 `ExhaustPhysicalPlans` 得到候选。
5. 每个候选依据其 `GetChildReqProps` 递归调用孩子的最优任务路由。代码在候选间保存/恢复 PlanID 与 PlanColumnID 前沿，避免失败或落选候选污染最终编号。
6. 候选通过 `attach_canonical_*` 专用路径或 `PhysicalPlan::attach_to_task` 组装。必要时插入 Sort、Exchange、reader、Projection、TopN/Limit 或聚合阶段，并修正分区、Schema、统计和访问条件。
7. 对有效任务计算代价，保留最低成本者；失败候选的错误被暂存，只有没有可用方案时才上抛。胜出任务再做嵌入 Limit 统计提升、列 ID 前沿提交，并写入线程缓存。
8. `optimizer_runtime.rs` 从任务克隆最终物理树，执行索引解析及后处理。本文件的 reader 展平、IndexJoin 内层填充和 PlanID 对齐函数在此阶段被再次调用，之后树才交给执行器或 EXPLAIN。

DataSource 子流程会在 possible access paths 中选择 table/index/lookup 路径，验证排序方向、handle/index 列前缀、MPP 许可和 TiFlash 限制；ordered LIMIT 场景借助 `ScanCardinalityContext` 调整扫描行数。`ExhaustPhysicalPlans` 对非 DataSource 节点按具体逻辑类型调用相邻算子模块的 `ExhaustPhysicalPlans4*` 实现。

## 数据与状态

`BasePhysicalPlan` 同时保存树结构和派生缓存，维护时必须遵守以下不变量：

- `children_req_props` 至少覆盖现有孩子数；`SetChildren`/`SetChild` 会调用 `ensure_child_properties` 并清除 `PlanCostInit`，但直接经 `ChildrenMut` 修改时调用方必须自行维护一致性。
- `stats` 是 Rust 侧权威值，`set_stats` 和 `restore_cached_fields` 同时把克隆值写回 `baseimpl::Plan`，避免 trait 视角不一致。
- `schema()` 优先返回第一个孩子的 Schema，无孩子时才返回本地 `schema`；具体 schema producer 算子通常覆盖或持有自己的 Schema。
- `PlanCostInit` 同时控制 v1/v2 缓存有效性。`CloneWithNewCtx` 深克隆孩子和 probe parents，但明确重置两版代价缓存。
- `probe_parents` 用于 IndexJoin/Apply 内侧算子的显示行数与运行时 probe 次数缩放：估计值乘父节点 `stats_count().max(1)`，实际值乘 runtime stats 中的 act rows 并保证至少为 1。
- 规范任务缓存只存活于当前线程；键包含属性哈希和 Schema 列 ID，防止不同排序、分区或列身份之间误复用。
- `FIND_BEST_TASK_ROUTER` 是进程级一次写入状态；安装后只能读取，重复安装返回原 router 作为 `Err`，调用端当前有意忽略重复安装结果。
- `PlanID`、`PlanColumnID` 是 session 级原子分配前沿。本文件在候选枚举时显式 checkpoint/restore，只提交胜出候选需要的最大值；多个 `Align*` 函数随后按 Go 构造序列重排局部树。

## 依赖与调用关系

crate 边界由 `pkg/planner/core/operator/physicalop/Cargo.toml` 定义。直接依赖可分为：

- 抽象与公共状态：`base`、`baseimpl`、`logicalop`、`property`、`planctx`；
- 代价与基数：`costusage`、`cardinality`、`statistics`；
- 表达式与聚合：`expression`、`aggregation`、`types`、`mysql`、`parser_ast`；
- 元数据与存储协议：`model`、`kv`、`table`、`tipb`；
- range/执行统计和 planner 工具：`ranger`、`rangerctx`、`execdetails`、`planner_util`。

上游直接证据包括：

- `pkg/planner/core/optimizer_runtime.rs` 安装/调用路由，清缓存，并调用 `FlattenNestedMPPReadersBelowRoot`、`PopulateIndexJoinInnerPlans` 与 `Align*PlanIDs`；
- `pkg/planner/core/logical_plan_builder.rs::addExtraPhysTblIDColumn4DS` 调用隐藏分区列辅助；
- `pkg/planner/core/operator/physicalop/logical_plan_route_aster_unit_test.rs` 验证安装边界；
- executor 的 plan walk 测试通过 `NewBasePhysicalPlan` 构建可遍历的物理树。

下游调用以相邻算子模块为主：`ExhaustPhysicalPlans` 分派各种 `ExhaustPhysicalPlans4*`，attach 辅助会构造/克隆 `PhysicalTableReader`、`PhysicalIndexLookUpReader`、Exchange sender/receiver、Join、Agg、Sort、Projection 等具体算子；代价通过 `PhysicalPlan::get_plan_cost_ver1/ver2` 递归下沉；最终任务由 `RootTask`、MPP task 和 `AttachedTask` 承载。

## 错误处理与边界

- `FindBestTask` 在 router 未安装时返回 `physical plan task router has not been installed`，不会静默采用默认实现。
- 递归寻优广泛返回 `expression::Error`：缺失 plan context/CTE seed、任务类型或排序属性不兼容、MPP 被禁用、无法枚举候选、克隆或索引解析失败都会沿调用链传播。
- 候选寻优允许单个候选失败；只有全部候选无效时才返回最后错误或“无法找到任务”类错误。这使“不适用”候选不阻塞其他实现。
- `SafeClone` 是刻意的 panic 隔离边界；普通克隆、挂接和对齐函数仍以 `Result` 报错。`attach_to_task` 中的 `expect` 表示基座挂接要求孩子计划和自身必须可克隆，违反这一内部契约会 panic。
- `to_pb` 对 `BasePhysicalPlan` 总是报错，因为基座不是可编码执行器；具体算子必须覆盖 PB 转换。
- `GetChildReqProps`、`SetChild` 采用下标访问，越界会 panic；调用者须先保持孩子与属性数组一致。
- `AddExtraPhysTblIDColumn` 以列信息中的 `ExtraPhysTblID` 判断幂等；它假设传入 Schema 与 columns 对应，返回的布尔值用于调用者区分是否新增。
- 各 `Align*PlanIDs` 先严格匹配节点类型、孩子数和语义条件，不符合目标形态就返回 `false`/`None`，避免对一般计划树误改。
- 当前 `CollectPlanStatsVersion` 对 IndexLookup 明确只递归 `IndexPlan`；这是兼容约定而不是漏遍历。若改变 reader 统计写入策略，此处和测试必须同步。

## 并发与资源生命周期

本文件没有异步任务或网络资源。主要生命周期约束来自共享状态和所有权：

- `CANONICAL_TASK_CACHE` 使用 `thread_local! + RefCell<HashMap<...>>`，无跨线程共享和锁竞争；同一线程内的重入借用仍受 `RefCell` 动态规则约束。每轮物理优化前必须清空，避免逻辑对象地址复用带来的陈旧命中。
- `FIND_BEST_TASK_ROUTER` 使用 `OnceLock`，发布后可无锁读取；全进程只允许一个实现。它要求 router 是普通 `fn`，不能捕获临时状态。
- plan context、统计等共享只读对象使用 `Arc`；逻辑 CTE 等局部可变图使用相邻类型提供的 `Rc<RefCell<_>>`。代码在借出 seed 后会将其放回 CTE 状态，再传播优化结果，避免永久移走共享计划。
- `Box<dyn PhysicalPlan>` 和 `Box<dyn Task>` 表达计划树独占所有权。缓存、候选隔离及后处理通过 `copy`/`clone_physical` 生成独立树，避免一个候选的孩子、ID 或统计修改泄漏到另一个候选。
- MPP reader/exchange 的资源语义由树边界表达：转换到 Root task 时插入 reader/sender，展平时只消除嵌套边界，不启动执行线程，也不持有远端连接。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/base_physical_plan.go`，Cargo 的 `[package.metadata.porting].go-package` 也指向同一 Go 包。

共同语义包括：`AddExtraPhysTblIDColumn`、`CollectPlanStatsVersion`、`SafeClone`、`BasePhysicalPlan` 的孩子/成本/统计/probe 逻辑、IndexJoin 属性准入、`GetStatsInfo` 和 `FindBestTask` 的逻辑到物理入口。Rust 独立测试 `base_physical_plan_test.rs` 已锁定两项关键对应：v2 成本只有显式 Recalculate 才重算；IndexLookup 统计版本只采用 index side。

重要实现差异如下：

- Go 的 `BasePhysicalPlan` 通过 `Self` 指回具体算子；Rust 用 trait object、具体算子委托和 `clone_physical` 表达多态，不保存 `Self` 字段。
- Go 基座默认 `Clone` 报“不支持”；Rust 基座实现了 `CloneWithNewCtx` 深克隆，并把 panic 转换能力单独放在 `SafeClone`。
- Go 同文件的 `FindBestTask` 主要按 logical 类型转发到其他文件；Rust 文件把规范递归、DataSource 寻优、候选挂接和大量兼容整形集中到一起。因此不能以 Go 文件较短推断 Rust 后半段是冗余逻辑。
- Go 的 reader/attach 构造顺序自然产生稳定 ID；Rust 会先枚举并克隆候选，所以需要 `Align*PlanIDs` 和 PlanColumnID checkpoint 显式复现 Go 的最终可观察编号。
- Rust `schema()` 对无孩子基座提供本地 Schema 回退；Go 基座直接返回首孩子 Schema。该回退服务于 Rust 集中构造和缓存恢复场景。
- Go `GetStatsInfo(any)` 还识别 Insert/Update/Delete 包装；Rust 本文件的 `GetStatsInfo(Option<&dyn PhysicalPlan>)` 只接受已提取的物理计划，DML 包装提取应由上游完成。

## 扩展指南

- 新增逻辑算子的物理枚举时，优先在对应独立算子文件实现 `ExhaustPhysicalPlans4*`，只在 `ExhaustPhysicalPlans` 增加最小分派；不要继续把具体算子定义塞入本文件。同步在独立 `*_test.rs` 或现有 `canonical_router_aster_unit_test.rs` 增加候选与属性测试。
- 新增物理算子必须正确实现/委托 `Plan` 与 `PhysicalPlan`，尤其是 `clone_physical`、children、child properties、stats、cost、Schema 和 PB 转换。若可出现在通用 attach 路径，验证 `SetChildren` 会使成本缓存失效。
- 修改缓存键时，必须证明所有影响任务选择的属性都被编码；不得缓存借用对象。新增 statement 优化入口时必须像 `optimizer_runtime.rs` 一样在开始前调用 `ResetCanonicalTaskCache`。
- 扩展 MPP 或 IndexJoin 路径时，重点检查 Root/Mpp task 类型、partition type/hash cols、exchange/reader 边界和 join key 所属 Schema。对应测试首选 `canonical_router_aster_unit_test.rs`、`mpp_join_hash_cols_aster_unit_test.rs`、`physical_hash_agg_aster_unit_test.rs`，不要把测试嵌入本源文件。
- 若新增 Go/Rust 构造顺序差异，PlanID 对齐函数只能匹配足够窄的树形，并需测试“不匹配时不修改”和“匹配时 ID/列 ID/Schema 同步”。避免用全树无条件重编号掩盖枚举错误。
- 修改统计收集或 probe 缩放时，同步更新 `base_physical_plan_test.rs`；修改 cost 缓存时同时覆盖普通、TRACE 和 `COST_FLAG_RECALCULATE`。
- 性能风险主要是候选爆炸、重复深克隆和递归树扫描。新增候选/后处理前应确认缓存命中粒度，并避免在每个候选上重复遍历整棵树。
- 兼容风险集中在 EXPLAIN ID、隐藏列身份、TiKV/TiFlash 任务边界与 Go 输出。所有新逻辑应先从 Go 对应实现或现有 Rust 测试取得证据；没有证据的预期行为应标为未验证。

## 验证依据

本说明依据以下本地事实完成，未运行 Cargo（任务明确为纯文档分析）：

- RustCodeGraph 状态：索引可用，覆盖 7,032 个 Rust 文件；`files --filter pkg/planner/core/operator/physicalop` 确认目标、Go 对照和测试均在索引内。
- RustCodeGraph 源码读取：`node --file pkg/planner/core/operator/physicalop/base_physical_plan.rs` 核对了公共基座（1–999）、规范路由（1640 起）、候选完成/IndexJoin 接线（2800 起）、MPP/Root 挂接（3460 起）和枚举分派（9250 起）。
- RustCodeGraph 符号查询：唯一定位了 `CanonicalFindBestTaskRouter:1658`、`InstallFindBestTaskRouter:764`、`PopulateIndexJoinInnerPlans:3182`、`FlattenNestedMPPReadersBelowRoot:3530`；`FindBestTask`/`ExhaustPhysicalPlans` 的同名结果同时显示 Go、旧 core 路径与本文件实现，文档据文件路径消歧。
- `rustcodegraph callers CanonicalFindBestTaskRouter` 在 60 秒内无输出并被中止；随后只用精确 `rg` 补齐直接调用点，确认生产入口位于 `pkg/planner/core/optimizer_runtime.rs`。
- 已读路径：目标 Rust 源、`pkg/planner/core/operator/physicalop/Cargo.toml`、`lib.rs`、Go 对照 `base_physical_plan.go`、独立测试 `base_physical_plan_test.rs`，以及直接关联的 router/MPP/聚合测试文件与 `optimizer_runtime.rs` 调用区段。
- 测试证据：`base_physical_plan_test.rs` 覆盖成本缓存和 IndexLookup 统计版本；`logical_plan_route_aster_unit_test.rs` 覆盖 router 安装；`canonical_router_aster_unit_test.rs` 覆盖候选枚举、DataSource、MPP reader、IndexJoin 和 PlanID；`physical_hash_agg_aster_unit_test.rs` 与 `mpp_join_hash_cols_aster_unit_test.rs` 覆盖聚合分区/列 ID 提交和 join hash keys。
- 包契约：阅读了最近的 `pkg/planner/core/base/doc.go`；其中要求公共接口保持抽象、避免依赖具体实现和循环依赖，与本文件通过 router/trait 建立的依赖倒置一致。

结构验收以任务指定命令为准；人工复核项为：恰好 11 个固定二级标题、所有路径和符号可在仓库定位、未把测试建议写入生产 Rust 文件、未宣称运行时测试已执行。
