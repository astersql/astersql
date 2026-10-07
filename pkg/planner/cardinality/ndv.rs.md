# `pkg/planner/cardinality/ndv.rs`

## 文件定位

本说明对应真实源文件 [`ndv.rs`](ndv.rs)。该文件属于 `astersql-planner-cardinality` crate，是优化器基数估算层的 NDV（Number of Distinct Values，不同值个数）实现。crate 根 `pkg/planner/cardinality/lib.rs` 以私有模块 `mod ndv` 装入本文件，再通过 `pub use ndv::*` 导出公开入口。它位于统计信息与物理/逻辑规划之间：输入主要是 `statistics::Table` 或 `property::StatsInfo`，输出的单列/列组 NDV 用于 Join 行数、聚合分组数、递归 CTE 去重、Index Join 探测和 Apply 缓存命中率等估算。

`pkg/planner/cardinality/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/planner/cardinality`，并直接依赖 expression、planctx、property、statistics、session variable/vardef 和 types 等本地 crate。本文件经 `use crate::*` 使用这些由 crate 根重导出的命名空间，并调用相邻 `exponential.rs` 的 `ApplyExponentialBackoff`。

## 核心职责

1. `EstimateColumnNDV` 从原始列直方图取得单列 NDV，并按分析时总行数与当前 `RealtimeCount` 的比例校正；统计不可用时使用 `RealtimeCount * 0.8` 的伪估算。
2. `EstimateColsNDVWithMatchedLen` / `EstimateColsNDVWithSessionVars` 估算一组列的联合 NDV：精确 `GroupNDV` 优先，否则取最大单列 NDV作为保守值，并可依据会话风险参数混入指数退避结果。
3. `ScaleNDV` 根据筛选前后行数缩放 NDV，在均匀分布概率模型和偏斜数据线性模型之间插值。
4. `init` 将 `ScaleNDV` 适配为 property 层的全局回调，以打破 cardinality 与 property 的依赖环；当前优化器运行入口也在 `pkg/planner/core/optimizer_runtime.rs` 中直接安装等价回调。

所有估算均为同步、纯计算或只读统计访问；除记录相关优化变量以及安装全局回调外，不修改表统计或计划节点。

## 主要符号

- `const distinctFactor: f64 = 0.8`：没有已初始化列统计时，按实时行数估算 NDV 的比例。
- `pub fn init()`：调用 `property::SetScaleNDVFunc` 注册 `ScaleNDV`。仓库中没有发现 `cardinality::init()` 的 Rust 调用点；正式逻辑优化入口目前直接注册同一闭包，因此不能把此函数描述为自动执行的 Rust 包初始化器。
- `pub fn EstimateColumnNDV(tbl, colID) -> f64`：单列入口。已初始化直方图走统计缩放，否则走伪估算。
- `fn getTotalRowCount(statsTbl, colHist) -> i64`：寻找与目标列相同 `LastUpdateVersion` 的分析总行数；目标列完整加载时直接返回自身总行数，否则先查完整索引，再查完整列，均无匹配时返回 `0`。
- `pub fn EstimateColsNDVWithMatchedLen(sctx, cols, schema, profile) -> (f64, usize)`：接受 object-safe `planctx::PlanContext` 的公开列组入口，只提取 `SessionVars` 后转交完整实现。
- `pub fn EstimateColsNDVWithSessionVars(vars, cols, schema, profile)`：列组估算的完整分支实现，也允许只有会话变量而没有完整规划上下文的调用者使用。
- `fn estimateNaiveNDV(...) -> f64`：对能在 schema 中匹配且 NDV 为正的列取最大值，默认下界为 `1.0`。
- `fn estimateNDVWithExponentialBackoff(...) -> f64`：收集正的单列 NDV、降序排列，以最大单列 NDV 为下界、`profile.RowCount` 为上界调用 `ApplyExponentialBackoff`。
- `pub(crate) fn estimateColsNDVBySkewRatio(...)` 与 `fn calculateGroupNDVWithSkewRatio(...)`：前者处理风险比是否启用，后者执行 `conservative + (exponential - conservative) * ratio` 的线性插值。
- `pub fn EstimateColsDNVWithMatchedLenFromUniqueIDs(...)`：把 UniqueID 包装为临时 `expression::Column` 后复用列组入口；名称中的 `DNV` 是沿用 Go API 的既有拼写。
- `pub fn ScaleNDV(...) -> f64`：读取 `RiskScaleNDVSkewRatio`（无会话变量时使用默认值），组合均匀与偏斜模型。
- `fn estimateUniformNDV(...)`：按“每个值出现次数相同、每行被选概率相同”计算至少出现一次的 distinct value 数，并在有效筛选场景中裁剪到 `[1, selectedRows]`。
- `fn estimateSkewedNDV(...)`：以 `originalNDV * selectedRows / originalRows` 线性缩放，`originalRows <= 0` 时返回 `0`。

## 执行流程

单列路径从 `EstimateColumnNDV` 开始。函数用 `Table::GetCol` 查列：若列存在且统计已初始化，先取 `Histogram.NDV`；随后 `getTotalRowCount` 选择与该列同一分析版本的总行数，找到正值时乘以 `RealtimeCount / analyzeCount`，找不到时保留直方图原值。若列不存在或统计未初始化，则直接返回实时行数的 `0.8` 倍。

列组路径按以下优先级运行：

1. 空列组立即返回 `(1.0, 1)`。
2. `StatsInfo::GetGroupNDV4Cols` 精确命中时，返回 `max(group.NDV, 1.0)`，第二个返回值为该组覆盖的列数。
3. 未命中时，`estimateNaiveNDV` 取各匹配列正 NDV 的最大值；单列到此直接返回。
4. 多列时，`estimateNDVWithExponentialBackoff` 对降序 NDV 使用 `v0 * v1^(1/2) * v2^(1/4) * v3^(1/8)`，实际最多使用四列，并裁剪到最大单列 NDV与计划行数之间（公式和四列上限定义于 `exponential.rs`）。
5. 有会话变量时读取并记录 `RiskGroupNDVSkewRatio`；比值大于零便在保守值和指数值间插值，否则返回保守值。没有会话变量同样返回保守值。非精确路径的 matched length 固定为 `1`。

缩放路径中，`ScaleNDV` 同时计算两个端点。均匀模型令选择率为 `selectedRows / originalRows`、每个值平均行数为 `originalRows / originalNDV`，用 `originalNDV * (1 - (1-selectivity)^rowsPerValue)` 估算至少出现一次的值数；偏斜模型只按行数比例线性缩放。最终以 `skewRatio` 加权：`skewed * ratio + uniform * (1-ratio)`。

## 数据与状态

列级统计来自 `statistics::Table`、`statistics::Column` 及其 `Histogram.NDV`、`LastUpdateVersion`、加载状态和总行数。`getTotalRowCount` 的版本相等约束避免用另一轮 analyze 的统计缩放当前 NDV；索引优先于其他列只是替代总行数的搜索顺序。

派生计划统计来自 `property::StatsInfo`：`RowCount` 是指数估算上界，`ColNDVs` 以列 `UniqueID` 为键，`GroupNDVs` 提供精确列组统计。schema 的 `ColumnsIndices` 把输入列映射回计划 schema；映射整体失败时返回安全默认值 `1.0`，而不是使用部分位置。

会话状态只涉及 `RiskGroupNDVSkewRatio`、`RiskScaleNDVSkewRatio` 和 `RecordRelevantOptVar`。前者控制联合 NDV 对指数退避的信任程度；后者控制筛选缩放对偏斜模型的权重。代码没有在本地把 ratio 限制到 `[0,1]`，合法范围依赖系统变量层校验；直接构造 `SessionVars` 或直接调用内部算术函数时，越界值会产生外插而非插值。

## 依赖与调用关系

上游直接证据包括：

- `pkg/planner/cardinality/join.rs::EstimateFullJoinRowCount` 分别估算左右连接键 NDV，以两侧最大 NDV 除笛卡尔积；matched length 还决定剩余连接键的 `0.9` 相关性因子次数。
- `pkg/planner/core/operator/logicalop/logical_aggregation.rs` 用列组入口估算 `GROUP BY` 输出行数。
- `pkg/planner/core/operator/logicalop/logical_cte.rs` 用其估算 distinct 递归 CTE 的输出行数。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs` 用其估算非伪统计下等值前缀的 NDV。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 直接调用 `EstimateColsNDVWithSessionVars`，以 `1 - ndv / RowCount` 估算 Apply 缓存命中率。
- `pkg/planner/core/optimizer_runtime.rs` 把 `ScaleNDV` 注册到 property 回调。

下游依赖包括 `Schema::ColumnsIndices`、`StatsInfo::GetGroupNDV4Cols`、`SessionVars::RecordRelevantOptVar`、统计表遍历接口，以及 `exponential.rs::ApplyExponentialBackoff`。`property::StatsInfo::Scale` 通过 `property::ScaleNDVFunc` 间接消费 `ScaleNDV`；该回调采用 `RwLock<Option<fn(...)>>` 保存，以避免 property 反向依赖 cardinality。

RustCodeGraph 对本文件的文件节点报告了 planner core 的直接使用文件，但精确 `callers/callees` 查询因 Go/Rust 同名符号没有输出完整边；上述关系因此又由各调用点源码逐一核对。仓库搜索未发现 Rust 侧 `EstimateColumnNDV` 和 `EstimateColsDNVWithMatchedLenFromUniqueIDs` 的生产调用点，不能据此声称它们已进入当前 Rust 主链。

## 错误处理与边界

本文件不返回 `Result`，所有缺失或不可靠输入都通过数值回退处理：缺少列统计时使用 `0.8 * RealtimeCount`；找不到同版本分析总行数时不缩放原直方图 NDV；空列组、schema 映射失败或没有正单列 NDV时返回 `1.0`；计划行数不大于最大单列 NDV时直接返回该下界。

精确 `GroupNDV` 和保守列组结果至少为 `1.0`，以保护 Join 等下游除法。均匀缩放在任一输入非正时返回 `0`，筛选行数不少于原行数时返回原 NDV；正常筛选结果不会少于 `1` 或超过选中行数。偏斜缩放只保护 `originalRows <= 0`，不会自行裁剪负的 `selectedRows`、过大的选择行数或超范围风险比，因此调用方和变量层必须保持输入契约。

与 Go 版本相比，Rust 在 schema 映射失败时没有写诊断日志，只返回默认值；这是可观测性差异。Rust 类型使用引用而非可空 `profile`，所以 Go 的 `profile == nil` 防护在 Rust API 中由类型系统替代。

## 并发与资源生命周期

估算函数不创建线程、异步任务、通道、事务或外部资源；临时列向量、NDV 向量均在调用栈内创建并在返回时释放。统计表和 schema/profile 均以共享引用读取，本文件不持有跨调用缓存。

唯一共享可变状态位于 `pkg/planner/property/stats_info.rs` 的 `ScaleNDVFunc`：`init` 或优化器初始化通过写锁替换函数指针，统计缩放通过读锁取得回调。锁中毒时 property 层使用 `PoisonError::into_inner` 继续访问。重复注册会覆盖旧值，测试若替换回调需注意进程级共享状态与执行顺序；本文件自身没有恢复旧回调的生命周期管理。

`RecordRelevantOptVar` 是列组多列、未命中精确 GroupNDV 且存在会话变量时的可见副作用；空列组、精确 GroupNDV、单列和无会话变量路径均在此之前返回，不会记录该变量。

## 与 Go 版本的对应关系

Rust 实现逐段对应 `pkg/planner/cardinality/ndv.go`：`distinctFactor`、单列直方图缩放、同版本完整统计查找、GroupNDV 优先、最大单列保守估计、指数退避、risk ratio 插值、UniqueID 包装入口以及两种 NDV 缩放模型的公式均保持一致。公开函数采用 Go 风格名称，`EstimateColsDNVWithMatchedLenFromUniqueIDs` 也保留原有 `DNV` 拼写。

Rust 为 object safety 增加了 `EstimateColsNDVWithSessionVars`：完整 `PlanContext` 入口只取会话变量，物理计划中已有调用者可直接传 `SessionVars`。`estimateColsNDVBySkewRatio` 也是 Rust 为隔离算术和便于测试增加的局部层次，不改变 Go 分支语义。

主要差异有三点：Go 的包 `init()` 自动运行，Rust `init()` 必须显式调用，而当前实际优化器入口选择直接注册闭包；Go 在列不属于 schema 时记录错误日志，Rust 静默回退 `1.0`；Go 接受可空 `*StatsInfo` 并显式防御 nil，Rust 用非空引用消除了该分支。

`pkg/planner/cardinality/ndv_test.rs` 对齐 `ndv_test.go` 的核心数值：均匀缩放边界、风险比 `0/0.5/1`、issue 54812、单列、精确三列 GroupNDV、二/三列指数退避和空键。Go 测试还通过 SQL/EXPLAIN 覆盖真实 session 与计划链，Rust 测试以轻量 `TestContext` 和直接函数调用替代存储支持；因此 Rust 测试证明算法对齐，但不等价于完整 SQL 集成覆盖。

## 扩展指南

新增列组估算策略时，应优先在 `EstimateColsNDVWithSessionVars` 中保持现有优先级：空输入和精确 GroupNDV 不应被启发式覆盖；单列不应无故进入多列组合；任何可用作除数的结果应维持正下界。若修改指数退避权重或列数上限，应同步检查 `pkg/planner/cardinality/exponential.rs` 及其独立测试，而不是在本文件复制公式。

增加统计来源或改变分析版本选择时，应修改 `EstimateColumnNDV` / `getTotalRowCount`，并新增独立的 `pkg/planner/cardinality/ndv_test.rs` 回归用例，覆盖完整列、同版本索引替代、同版本列替代、版本不匹配和统计未初始化；当前 Rust 测试没有覆盖这组路径。

改变风险参数语义时，需同时核对 vardef/variable 的范围校验、相关优化变量记录、`ScaleNDVFunc` 注册点和 Go 对照。尤其不要只修改未被调用的 `init()` 而遗漏 `pkg/planner/core/optimizer_runtime.rs` 的实际注册闭包。若开放 `EstimateColsDNVWithMatchedLenFromUniqueIDs` 的新调用，需确认临时列只依赖 `UniqueID` 的假设仍被 schema 匹配实现满足。

测试应继续放在独立 `ndv_test.rs`，不得嵌入生产源文件。性能方面，列组估算会分配并排序至多输入列数个 NDV；若用于更热路径，可考虑在不改变排序和四列退避语义的前提下减少分配，并用基准或调用侧证据证明收益。

## 验证依据

- 生产实现：`pkg/planner/cardinality/ndv.rs`（完整读取 290 行）及 `pkg/planner/cardinality/exponential.rs::ApplyExponentialBackoff`。
- crate 边界：`pkg/planner/cardinality/Cargo.toml` 与 `pkg/planner/cardinality/lib.rs`。
- Rust 独立测试：`pkg/planner/cardinality/ndv_test.rs`；Go 对照实现与测试：`pkg/planner/cardinality/ndv.go`、`pkg/planner/cardinality/ndv_test.go`。
- 上游/下游直接证据：`pkg/planner/cardinality/join.rs`、`pkg/planner/core/operator/logicalop/logical_aggregation.rs`、`logical_cte.rs`、`pkg/planner/core/operator/physicalop/index_join_probe.rs`、`base_physical_plan.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/property/stats_info.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/planner/cardinality/ndv.rs` 列出完整源码和 planner core 使用文件；`query` 同时定位 Rust/Go 的 `EstimateColumnNDV`、`EstimateColsNDVWithMatchedLen`、`ScaleNDV`。精确 `callers/callees --file` 未返回边，故调用边再以仓库搜索和调用点源码验证，未将空结果推断为“无调用者”。
- 人工事实复核：文档分别回答文件存在目的、三条计算主流程、回退与数值边界、共享回调生命周期、Go/Rust 差异和安全扩展位置；未运行 Cargo，符合纯文档任务约束。
