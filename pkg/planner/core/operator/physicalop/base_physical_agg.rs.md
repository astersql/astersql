# `pkg/planner/core/operator/physicalop/base_physical_agg.rs`

## 文件定位

本文件位于 `astersql-planner-core-operator-physicalop` crate 中，是物理聚合算子的公共实现与逻辑聚合物理化入口。crate 根模块 `pkg/planner/core/operator/physicalop/lib.rs` 以 `mod base_physical_agg` 装载它，并通过 `pub use base_physical_agg::*` 向 planner 与 executor 暴露其公开类型和函数；`Cargo.toml` 的 `[package.metadata.porting]` 又把该 crate 对应到 Go 包 `pkg/planner/core/operator/physicalop`。

它处于“逻辑聚合 → 物理候选 → task attach/执行计划”的中段：`base_physical_plan.rs` 中的物理计划路由把 `logicalop::LogicalAggregation` 交给 `ExhaustPhysicalPlans4LogicalAggregation`；生成的 `PhysicalHashAgg`/`PhysicalStreamAgg` 随后可能在下推阶段调用 `CheckAggCanPushCop`、`NewPartialAggregate` 和 `ConvertAvgForMPP`。本文件不执行数据聚合，真正的执行器构建在 executor 层完成。

## 核心职责

1. 用 `BasePhysicalAgg` 保存 HashAgg 与 StreamAgg 共用的 schema producer、聚合描述、分组表达式及 MPP 状态。
2. 将公共基座初始化为具体的 `PhysicalHashAgg` 或 `PhysicalStreamAgg`，并设置算子类型、输出 schema、统计信息和子节点物理属性。
3. 为 MPP 改写 `AVG`，并把单阶段聚合拆成 partial/final 两阶段描述。
4. 判断聚合能否下推至指定存储类型，解析表达式到子节点 schema，并生成 EXPLAIN 文本、代价因子、相关列与内存估算。
5. 从 `LogicalAggregation` 枚举 Root、TiKV Cop 与 TiFlash MPP 上的 HashAgg/StreamAgg 候选，处理分区键、提示、CTE、锁读、缓存表、标量子查询及若干计划选择边界。

## 主要符号

- `AggMppRunMode`：MPP 聚合阶段枚举。`NoMpp` 表示非 MPP；`Mpp1Phase` 为单阶段；`Mpp2Phase` 为全 MPP 两阶段；`MppTiDB` 表示 TiFlash partial 后回 TiDB final；`MppScalar` 表示标量聚合在单一 MPP task 汇总。`scalar_mpp_run_mode` 根据 DISTINCT/ORDER BY 选择 `MppScalar` 或 `MppTiDB`。
- `BasePhysicalAgg`：公共状态容器。`PhysicalSchemaProducer` 提供基础计划、schema 和统计；`AggFuncs` 保存 `AggFuncDesc`；`GroupByItems` 保存分组表达式；`MppPartitionCols` 保存带排序规则 ID 的 MPP 分区列。
- `New`、`Init`、`InitForHash`、`InitForStream`：构造与具体化入口。`InitForHash`/`InitForStream` 分别写入 `HashAgg`/`StreamAgg` 类型名，并附加 schema 与 child required properties。
- `IsFinalAgg`、`NumDistinctFunc`、`Scale3StageForDistinctAgg`：读取聚合阶段及 DISTINCT 条件。当前 Rust 的三阶段判断只接受“无 GROUP BY、恰好一个 DISTINCT、全部无 ORDER BY 且为 `CompleteMode`”。
- `CloneWithSelf`、`CloneForPlanCacheWithSelf`：深拷贝聚合描述、表达式与 MPP 分区列，并将基础计划换绑到新上下文；plan-cache 版本把错误折叠为 `None`。
- `GetAggFuncCostFactor`：按函数名累计代价权重；空函数列表在 MPP 下取 `0.1`，其他场景取 `1.0`。
- `ConvertAvgForMPP`：把每个 `AVG` 扩展为 COUNT 与 SUM，并返回恢复原输出 schema 的 `PhysicalProjection`。
- `ResolveIndices`：先解析 schema producer，再把聚合参数、ORDER BY 和 GROUP BY 绑定到唯一子节点 schema；额外兼容 projection、下层聚合及 CTE 列 ID 漂移。
- `AggInfo`、`BuildFinalModeAggregation`：描述一个聚合阶段并构造 partial/final 两套函数、分组项和 schema。
- `CheckAggCanPushCop`：拒绝含虚拟列、相关列或存储引擎不支持函数的聚合。
- `RemoveUnnecessaryFirstRow`：仅当 `first_row` 参数等于某个非常量 GROUP BY 项时删除冗余函数。
- `ExhaustPhysicalPlans4LogicalAggregation`：本文件的规划主入口，返回符合所需 `PhysicalProperty` 的物理聚合候选。

## 执行流程

### 物理候选枚举

1. `base_physical_plan.rs` 的路由器识别 `LogicalAggregation` 后调用 `ExhaustPhysicalPlans4LogicalAggregation`。
2. 入口先取得上下文、统计、schema 与 query-block offset，并递归检查子树特征：极端大小差异或小型 root join、TiFlash 可用性、`FOR UPDATE`、半连接/Apply、运行时标量表达式和缓存表。
3. 若允许 HashAgg，函数构造可接受的 child task 类型。普通请求会考虑 CopSingleRead、CopMultiRead、Root，并在允许时额外枚举 MPP；明确的 MPP 请求只生成 MPP 候选。
4. MPP 分支从逻辑计划潜在分区键或 GROUP BY 列构造 `MPPPartitionColumn`，校验父属性要求的 Hash/Any/SinglePartition，并据此生成一阶段、两阶段、TiDB-final 或 scalar 候选。`max_count`/`min_count` 的分组聚合禁止两阶段，但标量情形允许 `MppScalar`。
5. 非 MPP 分支为每个 task 类型建立 HashAgg，并把 `NoCopPushDown`、CTE 状态和标量 AVG 的 TiFlash 偏好传给子属性。
6. StreamAgg 仅在非 MPP、非 root-index 特例、GROUP_CONCAT 无 ORDER BY，且 GROUP BY 都是列时考虑；父排序方向必须一致，且所需排序列均被 GROUP BY 覆盖。它只枚举 CopSingleRead 与 Root 子属性。
7. 最后按 HASH_AGG/STREAM_AGG hint 位筛选候选；冲突提示保留两类候选。

### AVG 的 MPP 改写

`ConvertAvgForMPP` 保存原 schema。对每个 `AVG`，它分别克隆并推导 COUNT、SUM 的返回类型，创建中间列，再构造 `sum / CAST(CASE count = 0 THEN 1 ELSE count END)`，最后把除法表达式返回类型恢复为原 AVG 输出类型。改写后的 aggregate 持有新函数与中间 schema，返回的 projection 持有原 schema。若 schema 未扩展且当前不是 final aggregate，则无需 projection；final MPP aggregate 即使没有 AVG 也保留 identity projection，以隔离 TiDB/TiFlash 聚合状态布局。

### partial/final 拆分

`NewPartialAggregate` 直接委托 `BuildFinalModeAggregation`。后者先为原 GROUP BY 建 partial group schema，再逐个处理聚合函数：

- DISTINCT 参数被纳入 partial GROUP BY，final 函数继续保留 DISTINCT，并改为引用 partial 分组列。
- GROUP BY 已提供同一值时，`first_row(group_key)` 不再产生 partial 聚合列，final 参数改为对应分组列。
- AVG 产生 COUNT 与 SUM 两个 partial 函数和两列中间状态。
- `max_count`/`min_count` 产生计数和极值两列，极值列保留原参数类型及排序规则。
- MPP 的非 DISTINCT COUNT 在 final 阶段改名为 SUM；非 MPP 路径保持 COUNT。
- 最后将分组列追加到 partial schema，并构造 final GROUP BY 对这些列的引用。

### 索引解析

`ResolveIndices` 先解析公共 producer；没有子节点时直接成功。存在子节点时，它收集 projection 输出表达式，或下层 HashAgg/StreamAgg 的 GROUP BY 表达式到输出列映射。参数可按表达式相等、稳定字符串、唯一生成列类型或 CTE 列替换回子 schema。`resolve_cte_column_ids` 先走普通 `ResolveIndices`，失败后才按 index、显示名及唯一匿名同类型列替换，仍失败时返回带子 schema 列清单的上下文错误。

## 数据与状态

- `AggFuncs` 的顺序与输出 schema 前缀一一对应；`explain`、`ConvertAvgForMPP` 和 `BuildFinalModeAggregation` 都依赖这一不变量。
- partial schema 的布局是“聚合中间列在前、分组列在后”。final 函数参数保存的是指向这些 partial 列的 `Column`，其 `Index` 必须与布局一致。
- `MppPartitionCols` 不只是列集合，还携带 `CollateID`；选择或重映射父级 hash partition key 时必须同时保持列类型与排序规则语义。
- `Init` 只更新已有具体候选的统计信息，不重新分配基础计划；这是为了避免重复 PlanID。`InitForHash`/`InitForStream` 则在同一具体基座上设置正确算子名。
- `CloneWithSelf` 深拷贝表达式和描述，避免候选之间共享可变聚合参数；上下文及 schema 的语义被保留。
- 文件没有全局可变状态。唯一可观察的外部状态变化是经 `ContextRef` 分配 PlanColumnID，以及对正在构造的计划节点字段进行修改。

## 依赖与调用关系

上游调用关系：

- `base_physical_plan.rs` 的逻辑到物理路由调用 `ExhaustPhysicalPlans4LogicalAggregation`。
- 同文件的 MPP attach 路径调用 `ConvertAvgForMPP`；Cop/MPP 聚合拆分路径调用 `CheckAggCanPushCop` 与 `NewPartialAggregate`。
- `physical_utils_test.rs` 直接验证 `BuildFinalModeAggregation` 的 count-extrema schema；`canonical_router_aster_unit_test.rs` 直接验证 MPP 候选模式与分区约束。

下游依赖：

- `aggregation`：`AggFuncDesc`、模式常量、类型推导、拆分与函数下推能力。
- `expression`：schema/column、表达式克隆和比较、索引解析、列替换、相关列/虚拟列检测，以及 AVG 恢复表达式构造。
- `base` 与本 crate 的 `BasePhysicalPlan`/`PhysicalSchemaProducer`：计划上下文、统计、子属性、schema 与具体物理计划 trait。
- `property`：task 类型、排序属性、MPP 分区类型与列、统计信息。
- `logicalop`：逻辑聚合及递归检查的 DataSource、Join、Apply、Selection、Lock 等节点。
- `kv`、`planner_util`、`parser_ast`：存储类型、TiFlash 下推判断、聚合函数名与 hint 位语义。

`Cargo.toml` 对上述模块均使用 workspace 内路径依赖；例外是 `tipb` 的固定 Git revision，但本文件本身不直接构造 protobuf 聚合表达式。

## 错误处理与边界

- 可失败的表达式类型推导、cast/函数构造、基础计划克隆和索引解析通过 `Result<_, expression::Error>` 向上传播；错误消息在缺少 COUNT/SUM 返回类型或 CTE 解析失败时补充具体语境。
- `ExhaustPhysicalPlans4LogicalAggregation` 以空候选集表示当前属性不可满足，例如无上下文、MPP 遇到被强制 root-index 的子树、运行时标量子查询，或分区要求无法匹配且禁止 enforcer。
- `CheckAggCanPushCop` 是保守布尔门：任何函数参数、ORDER BY 或 GROUP BY 中出现虚拟列/相关列都会拒绝；函数本身还必须通过 `aggregation::CheckAggPushDown`。
- `RemoveUnnecessaryFirstRow` 不会因为“存在 GROUP BY”就删除函数：常量分组项、参数不等、列 UniqueID 不同或非唯一表达式均保留。
- `ResolveIndices` 假定至多使用第一个 child；聚合没有 child 时返回成功。对生成列的按类型兜底仅在候选唯一时采用，避免错误绑定。
- `explain` 按函数下标读取输出 schema，因此构造者必须保证 schema 至少包含每个聚合函数对应的输出列。
- `BuildFinalModeAggregation` 的 `max_count`/`min_count` 分支从已推导返回类型和第一个参数取类型；调用前必须完成聚合描述类型推导并提供参数。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel、文件句柄或网络连接。所有主要操作均在优化器单次计划构造期间同步完成，资源由 Rust 所有权和 `Vec`/`Box` 生命周期管理。

`ContextRef` 是共享计划上下文；函数通过它分配单调递增的 PlanColumnID，因此候选枚举和改写顺序会影响可观察 ID。代码中特别保留 Go 构造顺序并避免 `Init` 二次分配 PlanID。计划候选中的表达式和 schema 在克隆时使用 `CloneExpr`、`AggFuncDesc::Clone`、`Schema::Clone`，防止一个候选的改写污染另一个候选。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/physicalop/base_physical_agg.go`。Rust 保留了同名核心类型与方法：`BasePhysicalAgg`、初始化、克隆、代价、相关列、内存估算、AVG 改写、partial/final 拆分、下推判断、EXPLAIN、索引解析和物理候选枚举。

已对齐的关键语义包括：final/complete 判定；空聚合在 MPP 下使用 first-row 级低成本；AVG 拆成 COUNT+SUM 并用 projection 恢复；MPP final COUNT 改 SUM；DISTINCT 参数成为 partial 分组键；冗余 first_row 仅对相同非常量分组键删除；HashAgg/StreamAgg hint 选择。

仍需把 Rust 视为独立移植实现而非逐行等价：

- Go 的 `CheckAggCanPushCop` 还检查参数/分组表达式是否能编码下推、能否转 protobuf，并向 statement context 写 warning；当前 Rust 版本只检查虚拟列、相关列及 `CheckAggPushDown`。
- Go 的三阶段 DISTINCT 支持由 session 变量控制，并包含多 DISTINCT grouping sets；当前 Rust 的 `Scale3StageForDistinctAgg` 只是更窄的纯条件判断。
- Go 的 `BuildFinalModeAggregation` 处理 GROUP_CONCAT separator/ORDER BY、Partial1/Partial2 及 first-row map 等更完整状态；Rust 目前集中覆盖 DISTINCT、AVG、count-extrema、COUNT 和 GROUP BY/first_row 的现有调用需求。
- Rust 的 `ResolveIndices` 比所读 Go 实现多了 projection/下层聚合重映射与 CTE 列 ID 恢复逻辑。
- Rust 的候选枚举内含针对 root index join、CTE、运行时标量与 TiFlash 路径的显式递归判断；验证时应以当前 Rust 测试和最终计划为准，不能仅凭 Go 函数体推断完全一致。

## 扩展指南

- 新增聚合函数或中间状态时，优先检查 `GetAggFuncCostFactor`、`BuildFinalModeAggregation`、`ConvertAvgForMPP`、`CheckAggCanPushCop` 和 executor attach 端是否都理解其列数、类型及阶段模式。
- 修改 partial schema 布局时必须同步 final 参数 index、DISTINCT/group 列位置、`ResolveIndices` 映射，以及 `physical_utils_test.rs` 中的 schema 断言；不能只让编译通过。
- 扩展 MPP 模式或分区策略时，应在 `ExhaustPhysicalPlans4LogicalAggregation` 保持父属性可满足性，并同步 `base_physical_plan.rs` 的 attach 逻辑和 `canonical_router_aster_unit_test.rs` 的候选/最终 task 验证。
- 增强下推判定时，应与 Go 的表达式可编码检查、protobuf 转换和 warning 行为逐项核对，避免把“函数名支持”误当成“整个表达式可下推”。
- 修改 `ResolveIndices` 时必须覆盖普通列、projection 表达式、两阶段聚合 group 输出、CTE 列 ID 漂移、同名/同类型歧义与失败错误文本。
- 测试逻辑应继续放在独立文件。最直接的单元测试位置是 `base_physical_agg_test.rs`；两阶段 schema 使用 `physical_utils_test.rs`；候选路由与 MPP 模式使用 `canonical_router_aster_unit_test.rs`。涉及最终执行树时还应选择 `base_physical_plan` 或 executor 的独立测试文件。
- 兼容风险主要是 Go/Rust 计划形状、PlanColumnID、EXPLAIN 文本和 hint 行为差异；性能风险主要是错误选择一/两阶段、漏用 MPP 或引入多余 exchange/projection；正确性风险主要是中间列类型、排序规则和 final 参数索引错误。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件可按行读取，共 1,524 行。查询确认 Rust 与 Go 均存在 `BasePhysicalAgg`、`BuildFinalModeAggregation`、`CheckAggCanPushCop`、`RemoveUnnecessaryFirstRow`、`ExhaustPhysicalPlans4LogicalAggregation` 和 `ConvertAvgForMPP`。
- 目标实现：`pkg/planner/core/operator/physicalop/base_physical_agg.rs`，已核对全部源码范围及主要符号。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml`；模块装载和公开再导出：`pkg/planner/core/operator/physicalop/lib.rs`。
- 上游与下游调用证据：`pkg/planner/core/operator/physicalop/base_physical_plan.rs` 中 AVG 改写、下推检查、partial/final 拆分和 LogicalAggregation 路由。
- Go 对照：`pkg/planner/core/operator/physicalop/base_physical_agg.go`，核对公共结构、AVG 改写、first_row 删除、下推判断、两阶段拆分、EXPLAIN、索引解析及物理候选入口。
- Rust 独立测试：`pkg/planner/core/operator/physicalop/base_physical_agg_test.rs` 验证匹配的非常量分组键才删除 first_row；`physical_utils_test.rs` 验证 `max_count`/`min_count` partial schema 保留计数列和带 collation 的值列；`canonical_router_aster_unit_test.rs` 验证 MPP 标量边界、分区要求及 count-extrema 运行模式。
- Go 测试入口由 RustCodeGraph 查询定位到 `pkg/planner/core/plan_test.go` 的 `TestBuildFinalModeAggregation` 与 `TestBuildFinalModeAggregationMaxMinCountSchema`，作为拆分语义的直接回归参考。
- 结构验收使用任务指定命令，要求文件存在且恰有十一个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
