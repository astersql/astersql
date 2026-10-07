# `pkg/planner/cardinality/cross_estimation.rs`

## 文件定位

本文件属于 `astersql-planner-cardinality` crate。`pkg/planner/cardinality/Cargo.toml` 将 `lib.rs` 设为 crate 根，`lib.rs` 以私有模块 `mod cross_estimation` 装载本文件，并通过 `pub use cross_estimation::*` 导出其公开项。它位于优化器基数估算层：输入规划上下文、逻辑/表统计和 `AccessPath`，为带 `LIMIT`、排序要求的表扫描或索引扫描估算“为了取得期望输出行数需要扫描多少候选行”。它只读统计信息与会话变量，不访问存储，也不执行物理计划。

Rust 生产主链中，`pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的原始物理表扫描构造逻辑调用 `AdjustRowCountForTableScanByLimit`，随后以结果缩放 `StatsInfo`。仓库搜索没有发现 `AdjustRowCountForIndexScanByLimit` 的 Rust 生产调用者；该入口目前由 crate 公开，但索引扫描侧的实际接线仍应以调用点搜索结果为准。

## 核心职责

- `AdjustRowCountForTableScanByLimit`：以均匀分布估算为基线，条件允许时尝试利用过滤列与 handle 的相关性修正；若不能精确修正，则按相关系数做指数回退；有排序属性时再应用 `OptOrderingIdxSelRatio` 风险补偿。
- `AdjustRowCountForIndexScanByLimit`：尝试索引扫描的跨列估算，并对“索引负责顺序、过滤发生在索引外”的风险增加扫描量。当前 `crossEstimateIndexRowCount` 向公共逻辑传入 `None` 相关列和 `0.0` 相关系数，因此公共逻辑立即报告不可精确估算，实际落到均匀分布/排序比例启发式；这与同路径 Go 文件当前参数形状一致。
- `crossEstimateRowCount`：把可用于相关列的过滤条件转成范围，利用直方图求出从扫描起点到命中 `expectedCnt` 行所需覆盖的前缀/后缀，再换算为访问路径扫描行数。
- `getColumnRangeCounts`、`convertRangeFromExpectedCnt`、`getMostCorrCol4Handle`：分别负责逐范围计数、按扫描方向截取范围边界、挑选与 handle 绝对相关性最大的过滤列。

## 主要符号

- `pub const SelectionFactor: f64 = 0.8`：范围条件之外仍有残余过滤时，用于反推原始扫描行数的默认选择率。
- `pub fn AdjustRowCountForTableScanByLimit(...) -> f64`：表扫描公开入口。`isMatchProp` 表示路径是否匹配所需顺序，`desc` 表示扫描方向。
- `fn crossEstimateTableRowCount(...) -> (f64, bool, f64)`：表扫描包装层；返回值依次为估算、是否得到精确结果、可供回退使用的相关系数。
- `pub fn AdjustRowCountForIndexScanByLimit(...) -> f64`：索引扫描公开入口，始终记录排序选择率变量的使用，并在存在索引过滤或表过滤时按该比例补偿。
- `fn crossEstimateIndexRowCount(...) -> (f64, bool, f64)`：合并 `TableFilters` 与 `IndexFilters`；当前没有选择相关列。
- `fn crossEstimateRowCount(...) -> (f64, bool, f64)`：公共核心。只有全范围访问、存在相关列、能分离列条件、能建立范围且直方图有效时才产生精确估算。
- `fn getColumnRangeCounts(...) -> (Vec<f64>, f64, f64, bool)`：逐个范围调用索引或列范围估算；索引分支还保存最后一次估算的 `MinEst`/`MaxEst`，当前调用方不消费这两个值。
- `pub(crate) fn convertRangeFromExpectedCnt(...) -> (Vec<Range>, f64, bool)`：按升序或降序累加范围计数，返回覆盖目标前已累计的行数及是否必须扫描完整访问范围。`pub(crate)` 可见性使同 crate 的独立测试能够直接验证它。
- `fn getMostCorrCol4Handle(...) -> (Option<Column>, f64)`：仅当条件恰好涉及一列且其绝对相关系数达到阈值时返回列；多列条件只返回最大相关系数，供启发式回退使用。

## 执行流程

表扫描入口先令结果等于 `path.CountAfterAccess`。仅当 `expectedCnt < dsStatsInfo.RowCount` 时，才计算 `uniformEst = min(CountAfterAccess, expectedCnt / selectivity)` 并尝试相关性估算。`crossEstimateTableRowCount` 会在伪统计、没有表过滤器或会话关闭 `EnableCorrelationAdjustment` 时立即失败；否则由 `getMostCorrCol4Handle` 选择候选列并进入公共核心。精确估算成功时取 `max(uniformEst, corrEst)`，避免相关性估算过于激进；失败但 `abs(corr) < 1` 时，以 `(1 - abs(corr)) ^ CorrelationExpFactor` 放大均匀估算。若路径匹配排序属性，最后按 `OptOrderingIdxSelRatio` 向原始访问行数插值。

公共核心首先拒绝缺少相关列或已有 `AccessConds` 的路径，因为整表直方图不能直接代表非全范围扫描。负相关会反转 `desc`。随后 `DetachCondsForColumn` 分出目标列范围条件和残余条件，`BuildColumnRange` 建立范围；成功得到空范围时返回成功的零行估算。它从 `dsStatsInfo.HistColl.ColUniqueID2IdxIDs` 选择目标列关联的第一个索引 ID，并用 `dsTableStats` 的表级直方图逐范围估算。`convertRangeFromExpectedCnt` 从扫描起点累加范围：升序构造从最小值到命中范围下界的前缀，降序构造从命中范围上界到最大值的后缀。若所有范围仍不足 `expectedCnt`，直接返回 `path.CountAfterAccess`；否则再次估算转换后范围，并用 `rangeCount + expectedCnt - count` 得到扫描量。有残余过滤时再除以 `SelectionFactor`，最终不超过 `CountAfterAccess`。

索引入口先调用 `crossEstimateIndexRowCount`。由于当前包装层传入 `None`，精确分支不会成立，入口按均匀选择率和相关性因子计算；之后若仍需索引外过滤，则把结果按 `OptOrderingIdxSelRatio` 朝 `CountAfterAccess` 提升。

## 数据与状态

本文件不拥有长期状态。所有计算围绕借用输入和局部值展开：`StatsInfo.RowCount` 表示过滤后预期输出规模，`AccessPath.CountAfterAccess` 是访问条件后的候选行数，`Table`/`HistColl` 提供伪统计标志、列相关性和列/索引直方图。`AccessPath.TableFilters`、`IndexFilters`、`AccessConds` 决定哪些条件仍可用于跨列估算。

可观察的唯一状态性副作用是调用 `SessionVars.RecordRelevantOptVar(TiDBOptOrderingIdxSelRatio)`，让 EXPLAIN EXPLORE 能报告该会话变量参与了估算。范围、过滤表达式及 collator 均通过克隆形成局部所有权；`convertRangeFromExpectedCnt` 特意克隆源范围的全部 `Collators`，避免新范围丢失排序/比较语义。

## 依赖与调用关系

上游直接证据是 `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：它在表路径存在、`CountAfterAccess > 0` 且 LIMIT/顺序条件需要修正时构造统计表，调用 `cardinality::AdjustRowCountForTableScanByLimit`，再用结果缩放表统计。`pkg/planner/cardinality/lib.rs` 提供 `planctx`、`property`、`statistics`、`ranger`、`types`、`util`、`vardef` 等再导出命名空间，并将本模块的公开入口提升到 crate 根。

下游关键调用包括 `PlanContext::GetSessionVars`/`GetRangerCtx`、`expression::ExtractColumnsMapFromExpressions`、`ranger::DetachCondsForColumn`、`ranger::BuildColumnRange`、`GetRowCountByIndexRanges`、`GetRowCountByColumnRanges`，以及统计有效性检查 `IndexStatsIsInvalid`/`ColumnStatsIsInvalid`。Cargo 依赖对应 `planctx-dependency`、`property-dependency`、`statistics-dependency`、`ranger-dependency`、`expression-dependency`、`types-dependency`、`planutil-dependency`、`vardef-dependency` 和 `variable-dependency`。

RustCodeGraph 能列出文件被 `base_physical_plan.rs`、相关测试和若干物理算子文件引用，但对精确函数 ID 的 `callers`/`callees` 查询没有返回边；因此函数级调用关系以目标源码和 `rg` 的直接调用点为补充证据。仓库内未发现索引公开入口的 Rust 生产调用点。

## 错误处理与边界

公开 API 返回 `f64`，内部不向上抛出错误。任何不可靠条件——伪统计、功能关闭、缺少过滤/相关列、非全范围访问、缺失返回类型、range 构建失败、直方图类型下转失败、统计失效或行数估算错误——都转换为 `(0.0, false, corr)`，由入口退回启发式或原始 `CountAfterAccess`。这使统计缺失不会阻断优化，但可能降低估算精度。

空范围是特殊的成功结果 `(0.0, true, corr)`；目标数量超过全部候选范围时，`isFull` 令结果回到完整 `CountAfterAccess`。最终精确估算使用 `min(CountAfterAccess)` 封顶。代码假设用于除法的 `CountAfterAccess` 与派生选择率处于规划器可接受域；本文件没有显式处理零值或 NaN。相关系数绝对值达到 1 但无法精确估算时不会进入指数回退，以避免 `(1 - |corr|)` 为零导致除零。

表扫描精确结果尚未实现“至少扫描一个 region”的下界；Rust 注释和 Go 原实现均明确保留这一限制。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务、文件句柄或网络资源。函数同步完成，借用的规划上下文和统计对象只在调用期间有效。局部 `Vec`、克隆的表达式/范围/collator 在函数返回后按 Rust 所有权规则释放。

`RecordRelevantOptVar` 是通过共享会话上下文发生的状态记录，其并发保证由 `SessionVars` 实现负责；本文件没有额外同步。新增缓存或共享统计状态时不能沿用当前“纯局部计算”的并发假设，必须在其所有者处定义生命周期和同步策略。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cardinality/cross_estimation.go`。Rust 保留了 `SelectionFactor`、两个公开入口、三个核心辅助函数、相关性阈值判断、负相关反转扫描方向、范围逐项计数、残余条件除以 0.8、结果封顶以及排序索引风险补偿等主逻辑。`Cargo.toml` 的 `[package.metadata.porting] go-package = "pkg/planner/cardinality"` 也声明了移植归属。

主要表示差异来自 Rust 安全边界：可空列改为 `Option<&Column>`，统计直方图由 `Any` 下转，Go 的 `error` 被匹配并转换为失败标志，范围与表达式通过克隆管理所有权。Rust 构造转换范围时逐个 `Clone()` collator；独立测试专门覆盖这一点。Go 和 Rust 的索引包装层目前都传入空相关列，因此两者的精确索引跨列路径都不会由该包装层触发。

端到端对应测试是 Go `pkg/planner/core/casetest/cbotest/cbo_test.go::TestLimitCrossEstimation` 与 Rust `pkg/planner/core/casetest/cbotest/cbo_test.rs::test_cbo_limit_cross_estimation_matches_go_suite_fixture`，二者读取同一 `analyze_suite` 用例，覆盖伪统计、正相关、负相关以及 index join 外层计划中的正确列 ID。Rust `pkg/planner/core/integration_test.rs::ordered_table_limit_uses_expected_scan_count` 还直接验证排序表扫描应用 `OptOrderingIdxSelRatio` 后的行数。

## 扩展指南

若要改变表扫描 LIMIT 估算，应从 `AdjustRowCountForTableScanByLimit` 和 `crossEstimateTableRowCount` 切入，并同步检查 `base_physical_plan.rs` 的触发条件。若要真正启用索引跨列精确估算，不能只改公开入口；需要为 `crossEstimateIndexRowCount` 明确选择“与第一索引列相关”的过滤列、传递相关系数，并在物理索引扫描构造链补齐调用，同时保持 Go 语义或明确记录差异。

修改范围截取时应维护以下不变量：升/降序方向与负相关翻转一致；边界排除标志取源边界的逻辑反值；collator 数量和语义不丢失；累计 `count` 只包含目标范围之前已经扫描完的范围。对应单元测试应放在独立的 `pkg/planner/cardinality/cross_estimation_test.rs`，不要嵌回生产文件。规划行为变更还应同步 Go/Rust `TestLimitCrossEstimation` 共享夹具或增加聚焦的 planner 测试。

兼容风险集中在计划选择变化和 EXPLAIN 估算变化；性能风险来自对每个范围分别查询直方图以及克隆过滤表达式/范围。新增统计错误传播策略时要保留优化器在缺失或损坏统计下可回退的性质，避免把估算失败升级为查询失败。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列出 `cross_estimation.rs`、Go 对照和独立 Rust 测试。`query cross_estimation --json` 识别到本文件的 1 个常量和 8 个函数；`node --file` 完整读取了 423 行目标源码。
- RustCodeGraph 文件引用结果：目标文件被 `cross_estimation_test.rs`、`base_physical_plan.rs`、`physical_apply.rs` 等文件引用；精确函数 ID 的 `callers`/`callees` 未返回边，因此又用源码及文本调用点核验函数级关系。
- 已读取路径：`pkg/planner/cardinality/cross_estimation.rs`、`pkg/planner/cardinality/lib.rs`、`pkg/planner/cardinality/Cargo.toml`、`pkg/planner/cardinality/cross_estimation.go`、`pkg/planner/cardinality/cross_estimation_test.rs`、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`、`pkg/planner/core/integration_test.rs`、Go/Rust 两份 `cbo_test` 及 `testdata/analyze_suite_{in,out}.json`。目标包没有 `doc.go`。
- 独立 Rust 单测 `test_convert_range_preserves_source_collators` 验证升序与降序转换都保留两个 collator；共享 CBO 夹具验证 LIMIT 计划在伪统计、正/负相关和子查询场景下与 Go 预期一致。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核公开/私有符号、回退分支、当前未接线状态和测试位置均有直接证据。
