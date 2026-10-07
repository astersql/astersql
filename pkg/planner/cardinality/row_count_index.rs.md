# `pkg/planner/cardinality/row_count_index.rs`

## 文件定位

[对应 Rust 源文件](row_count_index.rs)属于 `astersql-planner-cardinality` crate，是按索引范围估算命中行数的实现。crate 由 `pkg/planner/cardinality/Cargo.toml` 定义，`lib.rs` 以私有模块 `mod row_count_index` 装载本文件，再通过 `pub use row_count_index::*` 向 crate 使用者公开其中的 `pub` 符号。它处在规划器“访问路径已经形成范围、需要给路径计算基数”的阶段，不负责构造索引范围，也不读取存储数据。

应用主链中的直接入口位于 `pkg/planner/core/operator/logicalop/logical_datasource.rs`：逻辑数据源为索引路径截取已声明索引列的范围后调用 `GetRowCountByIndexRanges`，把返回的 `Est/MinEst/MaxEst` 写入访问路径；当非唯一索引范围还追加了整型主键 handle 时，再调用 `AdjustRowCountForAppendedHandleColumns`。此外，`pkg/planner/cardinality/selectivity.rs` 用入口结果计算索引谓词选择率，`cross_estimation.rs` 用它估算相关性修正后的扫描量，`row_count_column.rs` 复用本文件的均匀分布估算器。

## 核心职责

1. `GetRowCountByIndexRanges` 在完整范围快速路径、无效索引统计的伪估算、StatsVer1 和 StatsVer2 四类路径间分派，并统一返回 `statistics::RowEstimate`。
2. StatsVer1 路径把等值前缀交给 CM Sketch/等值选择率，把后续范围列交给列或索引统计，再限制总数不超过索引统计总行数。
3. StatsVer2 路径区分点范围与一般区间，组合唯一索引约束、TopN、直方图、指数退避、实时增长因子和 out-of-range 修正。
4. `estimateRowCountWithUniformDistribution` 和 `StatsProvider` 为列、索引共用的“TopN/桶未覆盖值”提供统一估算，并纳入 `RiskEqSkewRatio`。
5. `AdjustRowCountForAppendedHandleColumns` 给非唯一索引物理键末尾追加的完整 handle 谓词记入收益，同时用退避权重避免把索引列与主键列当成完全独立。
6. 辅助函数处理 NULL 编码、范围边界、前缀误判、全范围快速路径与列统计可用性。

## 主要符号

- `GetRowCountByIndexRanges(sctx, coll, idxID, indexRanges, idxCols) -> Result<RowEstimate, Error>`：公开总入口。`idxCols` 在索引统计无效时支持部分列统计伪估算，并供 exp-backoff 识别虚拟列。
- `getIndexRowCountForStatsV1(...) -> Result<f64, Error>`：旧统计版本实现。它枚举足够小的末列范围值，以编码后的等值前缀查询选择率，并用下一范围列的统计继续缩放。
- `getIndexRowCountForStatsV2(...) -> Result<RowEstimate, Error>`：新版统计核心。对每个 `Range` 估算并累加，最终通过 `Clamp(1.0, realtimeRowCount)` 限定结果。
- `StatsProvider`：抽象 `Histogram`、可选 `TopN`、统计总行数和实时增长因子；本文件分别为 `statistics::Column` 与 `statistics::Index` 实现。
- `estimateRowCountWithUniformDistribution(...) -> RowEstimate`：用非 NULL 桶行数除以扣除 TopN 后的 NDV；直方图信息不足时转入 `outOfRangeFullNDV`，并可按会话风险变量计算偏斜上下界。
- `equalRowCountOnIndex(...) -> RowEstimate`：索引点值估算，优先处理单列 NULL、V1 out-of-range/CM Sketch、V2 TopN、桶末 `Repeat`，最后回退均匀分布。
- `expBackoffEstimation(...) -> Result<(sel, minSel, maxSel, success), Error>`：逐列估算选择率、从小到大排序，最多取 `MaxExponentialBackoffCols` 个并调用 `ApplyExponentialBackoff`。
- `AdjustRowCountForAppendedHandleColumns(...) -> RowEstimate`：从完整物理范围抽取 handle 各维范围，合并重复范围，按 `sel^(1/2), sel^(1/4), ...` 衰减前缀估算；完整点范围再按范围数封顶。
- `outOfRangeOnIndex`、`matchPrefix`、`betweenRowCountOnIndex`：分别处理直方图范围外判定、编码前缀例外和 `[l, r)` 的直方图加 TopN 计数。
- `getOrdinalOfRangeCond`：返回第一个上下界不相等的列序号；比较错误时返回 0，使上层放弃等值前缀优化。
- `canSkipIndexEstimation`、`isFullRangeIncludingNulls`：仅对非 partial、非 MV 索引，且范围真正覆盖包含 NULL 的 `[NULL, +inf]` 时允许直接返回实时行数。
- `hasColumnStats`：判断传入索引列中是否至少一个具有有效列统计。

本文件没有模块级可变状态、结构体、枚举或条件编译项；唯一 trait 是 `StatsProvider`，测试通过 `lib.rs` 中独立的 `row_count_index_test.rs` 模块接入。

## 执行流程

`GetRowCountByIndexRanges` 的主流程如下：

1. 从 `PlanContext` 取得求值上下文，从 `HistColl` 按 `idxID` 取索引，并通过 `recordUsedItemStatsStatus` 记录优化器使用的统计项。
2. 若索引存在且 `canSkipIndexEstimation` 判定范围完整，直接使用 `GetScaledRealtimeAndModifyCnt` 的实时计数返回；这避免全范围扫描触发不必要的统计加载或插值。
3. 若索引统计无效：存在可用列统计且范围不是全范围时，调用 `getPseudoRowCountWithPartialStats`；否则调用 `getPseudoRowCountByIndexRanges`。唯一索引会把列数传给伪估算以保持唯一性约束。
4. 若统计有效且为带 CM Sketch 的 StatsVer1，进入 `getIndexRowCountForStatsV1`；其他情况进入 V2。

V1 对每个范围先用 `getOrdinalOfRangeCond` 找等值前缀。可枚举的末列小范围会展开为多个等值编码；首列就是范围条件或单列 `[NULL,NULL]` 时改走 V2。等值部分调用 `getEqualCondSelectivity`，剩余范围列优先复用该列关联的有效索引，否则调用 `GetRowCountByColumnRanges`；最终累加值封顶于 `idx.TotalRowCount()`。

V2 先把上下界编码为索引键。完整点范围上，排除端点直接贡献 0；唯一且非 NULL 的点固定为 1，唯一 NULL 点使用 `NullCount`，非唯一点由 `equalRowCountOnIndex` 估算。一般区间被规范成 `[low, high)`；单列 NULL 下界先加入 NULL 数。若范围具有等值前缀、统计版本至少为 V2 且有 `HistColl`，尝试 exp-backoff，并用多列索引直方图及 TopN 给其估算设置上限；失败则使用索引直方图与 TopN 的区间计数。随后乘实时增长因子，并在尚未接近全表时为直方图边界外部分追加估算；单列范围优先使用原始类型的列直方图，多列仍使用编码索引直方图。所有范围相加后统一 clamp。

`expBackoffEstimation` 为每个索引维构造单列临时范围。有效列统计优先；没有列统计时，多列输入可递归尝试该列关联的索引。递归候选出错会跳过并继续下一个候选。虚拟列既无列统计、又有当前复合索引统计时显式返回 `success=false`，让调用者保留复合索引直方图路径。取得至少两个选择率后，函数用索引 NDV/实时行数建立下界，并对排序后的最强过滤条件应用指数退避。

## 数据与状态

核心输入是只读的 `statistics::HistColl`、`statistics::Index`、`ranger::Range` 和表达式列引用。`HistColl` 提供实时行数、修改行数、索引/列统计，以及 `Idx2ColUniqueIDs`、`ColUniqueID2IdxIDs` 两个映射。`Index` 提供 `Histogram`、`TopN`、可选 `CMSketch`、NDV、NULL 数、统计版本及索引元信息。范围上下界在 V2 中用 `codec::EncodeKey` 转成字节序，以便与索引统计使用相同的排序空间。

输出 `RowEstimate` 同时携带中心估算 `Est`、乐观下界 `MinEst` 和保守上界 `MaxEst`。普通精确计数常通过 `DefaultRowEst` 让三者相同；exp-backoff、偏斜风险和追加 handle 修正会分别维护边界。`AdjustRowCountForAppendedHandleColumns` 保留原 `MaxEst` 作为未计 handle 的上界，仅收紧 `Est/MinEst`；完整物理点范围则把三者限制到点范围数量。

`nullKeyBytes()` 每次使用 UTC 把 NULL datum 编码为索引键。函数局部的 `Vec`、临时范围和编码字节都随调用结束释放；对传入统计对象不做写入。可观察的会话状态只有 `estimateRowCountWithUniformDistribution` 调用 `RecordRelevantOptVar(TiDBOptRiskEqSkewRatio)`，记录本次计划依赖了风险参数；入口还记录已使用统计项。

## 依赖与调用关系

上游直接证据：

- `pkg/planner/core/operator/logicalop/logical_datasource.rs`：索引访问路径的主调用者；先估算声明列前缀，再视需要调用追加 handle 修正。
- `pkg/planner/cardinality/selectivity.rs`：把行数除以 `RealtimeCount`，形成 `StatsNode` 的索引选择率及上下界。
- `pkg/planner/cardinality/cross_estimation.rs`：相关性/期望扫描量计算中，存在索引统计时使用本入口估算转换后的范围。
- `pkg/planner/cardinality/ndv.rs`、`pkg/statistics/histogram.rs`：RustCodeGraph 将其识别为目标文件使用方；本任务未把它们扩展为新的分析对象。

主要下游依赖：

- 统计层：`HistColl::{GetIdx,GetCol,GetScaledRealtimeAndModifyCnt}`，`Histogram::{EqualRowCount,BetweenRowCount,OutOfRangeRowCount,LocateBucket}`，`TopN::{QueryTopN,BetweenCount}`，以及统计有效性判断。
- 同 crate 估算器：`getPseudoRowCountWithPartialStats`、`getPseudoRowCountByIndexRanges`、`GetRowCountByColumnRanges`、`getEqualCondSelectivity`、`outOfRangeEQSelectivity`、`outOfRangeFullNDV`、`IsLastBucketEndValueUnderrepresented`、`ApplyExponentialBackoff`。
- 范围与编码：`codec::EncodeKey`、`kv::Key::PrefixNext`、`ranger::UnionRanges` 和 `Range::IsPoint`。
- 上下文：`PlanContext` 的表达式、ranger 和会话变量接口；`Cargo.toml` 直接声明 `statistics`、`ranger`、`codec`、`collate`、`kv`、`expression`、`planctx`、`types`、`vardef`、`chrono-tz` 与带 failpoints feature 的 `fail` 依赖。

RustCodeGraph 的文件查询显示本文件被 5 个文件使用，并确认主要入口和核心函数同时存在于 Go/Rust 两侧。对精确 `callers/callees` 的命令未产生边列表，因此上述调用关系又以同仓库符号搜索和调用点源码核实，不把空图结果解释成“没有调用者”。

## 错误处理与边界

所有需要编码或调用其他可失败估算器的主路径使用 `Result`。V1 首次编码错误会转为 `NewNoStackError`，其他编码/估算错误通过 `?` 返回；V2 的上下界编码错误也直接向上传播。入口假设“统计有效意味着索引存在”，因此在有效性检查后用 `expect` 固化该不变量。V2 的 StatsVer2 out-of-range 路径同样要求 `coll` 存在；V1 仅在不需要该分支的位置以 `None` 调 V2。

有三类故意降级而非报错的边界：`getOrdinalOfRangeCond` 比较失败返回 0；exp-backoff 的递归索引候选失败后尝试下一候选；追加 handle 时范围合并或列估算失败会忽略该维并保留前缀估算。测试 failpoint `afterRecursiveIndexEstimation` 验证了候选错误不会逃逸。`cleanEstResults` 可清空退避输入，用于验证空结果返回 `success=false` 的回退契约。

重要数值边界包括：完整结果限制在 `[1, realtimeRowCount]`；V1 总量不超过分析时索引总行数；唯一非 NULL 点至多一行；追加完整 handle 的每个点范围至多一行；无约束、无统计、实时行数非正或选择率不在 `(0,1)` 时不应用 handle 衰减。`matchPrefix` 防止编码值只是首桶下界前缀时被误判为 out-of-range。partial index 与 MV index 禁止使用全范围实时行数快速路径，因为前者不覆盖所有行、后者一行可能产生多个索引项。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或 I/O 资源。函数同步执行，借用共享统计快照并只创建调用内临时数据，因此并发安全性由 `PlanContext`、`HistColl` 和统计类型的上游共享约束决定，本文件不额外串行化。

递归只出现在两处：V1 为剩余范围列复用单列索引估算，以及 exp-backoff 在多列输入缺少列统计时尝试关联索引。exp-backoff 以“仅多列输入才递归索引”阻止单列索引无限递归；成功找到第一个候选即停止。临时 failpoint 的生命周期由独立测试中的 `FailScenario` 与 `fail::remove` 管理，生产函数本身不持有 failpoint 资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cardinality/row_count_index.go`。Rust 保留了 Go 的同名入口、V1/V2 分派、NULL/唯一点逻辑、TopN/CM Sketch/直方图优先级、exp-backoff、out-of-range、完整范围跳过估算和追加 handle 修正。`Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向 `pkg/planner/cardinality`。

Rust 的类型化适配包括：Go 的可空 `*HistColl` 在 V2 内部表示为 `Option<&HistColl>`；可空 TopN 通过 crate 的 `OptionalTopNExt` 提供 nil-safe 的 `Num/MinCount`；Go 接口 `StatsProvider` 对应 Rust trait 及两个显式 impl；Go 包级 `nullKeyBytes` 对应无可变全局状态的 `nullKeyBytes()`；切片指针改为借用切片。Rust 对缺失 `Idx2ColUniqueIDs`、越界列映射和空候选使用显式检查，保持 Go nil slice/失败回退语义而避免 panic。

两侧当前都包含 `AdjustRowCountForAppendedHandleColumns`：其前置条件是调用者先把范围截断到已声明索引列做前缀估算，不能把追加 handle 的完整编码键直接送入索引直方图。两侧也都要求 fast path 排除 partial/MV 索引，并要求低界为包含 NULL 而非 `MinNotNull`。Rust 测试 `go_merge_46_zero_repeat_index_upper_uses_uniform_estimate` 明确验证桶末 `Repeat=0` 不表示零行，而要回退均匀分布；正 `Repeat` 仍采用精确值。

## 扩展指南

- 新增统计版本或改变主分派时，从 `GetRowCountByIndexRanges` 接入，并同步核对伪统计、实时缩放、完整范围快速路径和 `RowEstimate` 三个边界值；不要只改变 `Est`。
- 新增点值数据源（例如新 sketch）时修改 `equalRowCountOnIndex`，保持单列 NULL、版本差异、TopN 和桶末欠表示检查的顺序；同时检查 `row_count_column.rs` 是否需要共享逻辑。
- 调整多列相关性模型时修改 `expBackoffEstimation`/`ApplyExponentialBackoff`，必须保留缺映射、虚拟列、递归候选失败、MV 缩放、NDV 下界和复合直方图上限。
- 扩展追加 handle 支持时修改 `AdjustRowCountForAppendedHandleColumns` 及其调用者。当前“追加维度构成完整单列整型 handle”的契约不能直接外推到复合主键前缀；点范围封顶依赖完整物理键唯一。
- 改动范围端点语义时同时检查 `PrefixNext`、NULL 计数、`LowExclude/HighExclude` 只作用于最后维度的规则、`UnionRanges` 与 `isFullRangeIncludingNulls`。
- Rust 测试必须继续放在独立的 `pkg/planner/cardinality/row_count_index_test.rs`，不要内嵌进生产文件。优先扩展现有测试：`exp_backoff_treats_missing_index_column_mapping_as_empty`、`go_merge_46_zero_repeat_index_upper_uses_uniform_estimate`、`appended_handle_selectivity_merges_bounds_damps_and_caps_points`、`virtual_column_recursive_index_estimates_propagate_and_retry`、`appended_handle_missing_stats_keeps_prefix_and_point_cap` 和组合统计回归。用户可见的选择率/fast-path 行为还应同步核对 `selectivity_test.rs` 及 Go 同路径测试。
- 兼容风险集中在 Go/Rust 数值顺序、空映射与错误降级语义；性能风险集中在范围逐项编码、末列枚举、递归索引候选和 handle 范围合并。修改后应避免扩大递归深度或对全范围失去快速路径。

## 验证依据

本说明读取并核对了以下直接证据：

- 目标源码：`pkg/planner/cardinality/row_count_index.rs`（841 行；RustCodeGraph `node --file` 分段读取）。
- crate 边界：`pkg/planner/cardinality/Cargo.toml`、`pkg/planner/cardinality/lib.rs`。
- Go 对照：`pkg/planner/cardinality/row_count_index.go`。
- 独立 Rust 测试：`pkg/planner/cardinality/row_count_index_test.rs`；相邻选择率测试调用点：`pkg/planner/cardinality/selectivity_test.rs`。
- 应用调用点：`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`pkg/planner/cardinality/selectivity.rs`、`pkg/planner/cardinality/cross_estimation.rs`、`pkg/planner/cardinality/row_count_column.rs`。
- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/cardinality` 确认目标、Go 对照与独立测试均已索引；文件节点列出 5 个使用文件；`query` 精确确认 `GetRowCountByIndexRanges`、`getIndexRowCountForStatsV2`、`expBackoffEstimation`、`AdjustRowCountForAppendedHandleColumns` 的 Go/Rust 对应定义。

人工复核结论：该文件存在的原因是把索引范围及多种统计结构转换为规划器可比较的行数区间；运行时由索引路径、选择率与相关性估算调用；安全扩展必须维持版本分派、范围边界、错误降级、递归终止、Go 对齐及独立测试约束。此任务为纯文档分析，按计划未运行 Cargo 或代码测试；验收采用固定十一章节的结构命令与源码证据复核。
