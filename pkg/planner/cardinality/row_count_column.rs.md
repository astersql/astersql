# `pkg/planner/cardinality/row_count_column.rs`

## 文件定位

本文件属于 `astersql-planner-cardinality` crate，是把 `ranger::Range` 转换成列访问路径基数（`statistics::RowEstimate`）的实现层。crate 根 `pkg/planner/cardinality/lib.rs` 以私有模块 `mod row_count_column` 装载它，再通过 `pub use row_count_column::*` 向 crate 使用方导出公开函数；`pkg/planner/cardinality/Cargo.toml` 的 `[package.metadata.porting]` 明确对应 Go 包 `pkg/planner/cardinality`。

它处在“谓词生成范围 -> 统计信息估算 -> 优化器比较路径”的中间：上游包括 `selectivity.rs` 的列谓词选择率、`row_count_index.rs` 的索引缺失回退、`cross_estimation.rs` 的交叉校验，以及 `core/operator/logicalop/logical_datasource.rs` 的表路径估算；下游消费 `statistics::Column` 中的 Histogram、TopN、CM Sketch、NDV、NULL 数和统计版本，并调用 `pseudo.rs`、`selectivity.rs` 等同 crate 估算辅助函数。

## 核心职责

- `GetRowCountByColumnRanges` 是公开总入口：记录统计项使用状态，在统计无效时选择伪估算，在统计有效时进入真实列统计估算。
- `getColumnRowCount` 遍历范围并区分闭合点、小范围枚举、一般区间三条路径，修正开闭边界、NULL、特殊哨兵、统计增长和超出直方图范围的数据。
- `equalRowCountOnColumn` 估算单值：统计版本 1 优先 CM Sketch/Histogram，版本 2 依次使用 TopN、桶末值 Repeat 和均匀分布兜底。
- `betweenRowCountOnColumn` 估算半开区间 `[l, r)`，版本 2 在 Histogram 结果的 `Est` 上补入 TopN 命中数。
- `getPseudoRowCountWithPartialStats` 在索引统计无效、但索引列仍有统计时，把多列索引范围拆成单列范围估算，并同时形成独立性乘积和相关性上界。
- `partialStatsRangeCollator` 固化 Go 移植语义：拆分临时单列范围时始终复用原索引范围的第一个 collator。

该文件不负责生成范围、不维护统计数据，也不决定最终执行计划；其产物是供这些上层决策消费的估算值及上下界。

## 主要符号

- `pub fn init()`：保留 Go 包初始化对应点，但 Rust 中为空实现。Rust 调用方直接调用 cardinality API，以避免 `statistics -> planner` 反向依赖环；因此不能把它解释成已经完成函数指针注册。
- `pub fn GetRowCountByColumnRanges(sctx, coll, colUniqueID, colRanges, pkIsHandle) -> Result<RowEstimate, Error>`：公开入口。`colUniqueID` 查找列统计；`pkIsHandle` 使伪统计选择有/无符号整数范围估算，并使真实统计中的闭合点至多计一行。
- `pub fn equalRowCountOnColumn(...) -> Result<RowEstimate, Error>`：公开但主要在 crate 内复用的等值估算。`encodedVal` 供 TopN 按编码键查询，原始 `Datum` 供 Histogram、CM Sketch 和越界判定使用。
- `pub fn getColumnRowCount(...) -> Result<RowEstimate, Error>`：真实列统计范围估算主循环；返回的 `RowEstimate` 同时维护 `Est`、`MinEst`、`MaxEst`。
- `pub fn betweenRowCountOnColumn(...) -> RowEstimate`：无失败返回的 `[l, r)` 基础估算；仅把 TopN 数加入 `Est`，保留 Histogram 给出的 Min/Max。
- `pub fn getPseudoRowCountWithPartialStats(...) -> Result<(f64, f64), Error>`：返回 `(totalCount, maxCount)`；前者是各列选择率乘积后的估算总数，后者累计每个索引范围中最不选择性列形成的相关性上界。
- `pub(crate) fn partialStatsRangeCollator(indexRange) -> Box<dyn Collator>`：crate 内辅助函数，克隆 `Collators[0]`；独立测试 `row_count_column_test.rs` 直接验证其大小写不敏感语义。

文件没有模块级可变状态、常量、自定义类型、trait 或条件编译项；主要数据结构均来自依赖 crate。

## 执行流程

1. `GetRowCountByColumnRanges` 从 `PlanContext` 取得表达式求值上下文，从 `HistColl` 查找列，并把 unique ID 映射成统计系统使用的 column-info ID。映射表非空而键缺失时使用 `0`，保持 Go map 缺失键的零值行为，然后调用 `recordUsedItemStatsStatus`。
2. 若 `ColumnStatsIsInvalid(c, coll.Pseudo)`：
   - PK handle 且范围为空时直接返回零；否则依据第一个下界的 Datum kind 选择 signed/unsigned 整数伪估算。
   - 非 PK handle 调用 `getPseudoRowCountByColumnRanges`，使用类型上下文和实时表行数。
   - 结果包装成 `DefaultRowEst` 返回。
3. 若统计有效，断言列必然存在并调用 `getColumnRowCount`；错误经 `errors::Trace` 传播。
4. `getColumnRowCount` 对每个范围克隆首列的上下界。字符串先转成相应 collation key，再以 binary collator 比较并通过 `codec::EncodeKey` 编码。
5. 上下界相等时只处理双闭点范围：PK handle 加一；普通列调用 `equalRowCountOnColumn`，再乘 `Column::GetIncreaseFactor`。任一端开区间的空点直接跳过。
6. 对 StatsVer 1，先尝试 `EnumRangeValues` 把小范围枚举为点；每一点执行等值估算、按增长因子缩放并累加。无法枚举才进入一般区间。
7. 一般区间先由 `betweenRowCountOnColumn` 得到 `[low, high)`：排除有效普通低端点时减去其等值行数；包含 NULL 下界时补 `NullCount`；包含非哨兵高端点时补其等值行数。随后把三个估算字段限制到 `[0, realtimeRowCount]` 并乘增长因子。
8. 若缩放后尚未近似覆盖实时全集，且任一非 NULL 边界超出统计范围，则以 Histogram、NDV、modify count 和会话变量 `RiskRangeSkewRatio` 补充 out-of-range 估算。StatsVer 2 的 Histogram NDV 要扣除 TopN 项数，避免重复覆盖。
9. 所有范围累加后，`totalCount.Clamp(1.0, realtimeRowCount)` 保持 Go 的最小一行启发式；因此“输入范围为空”与入口的 PK 伪统计空范围特判并不完全等价，调用方应理解该下限。
10. 等值估算中，NULL 直接使用 `NullCount`。StatsVer 1 对越界值使用 `outOfRangeEQSelectivity`，否则优先 CM Sketch，最后用 Histogram；StatsVer 2 则依次查 TopN、正数 Repeat，零 Repeat 或被判定为低估时转入 `estimateRowCountWithUniformDistribution`。
11. 部分列统计回退中，单列索引直接复用公开入口；多列索引把每个索引范围逐列投影到一个临时单列范围，最后一列才继承原范围开闭属性。每列选择率既相乘形成独立性估算，又取最小值形成相关性上界；总估算最终 clamp 到 `[1, tableRowCount]`。

## 数据与状态

- 主要只读输入为 `HistColl::{RealtimeCount, ModifyCount, Pseudo, PhysicalID, UniqueID2colInfoID}` 和 `Column::{Histogram, TopN, CMSketch, NDV, NullCount, StatsVer, IsHandle}`。
- `RowEstimate` 的三个字段随 `Add`、`AddAll`、`Subtract`、`MultiplyAll`、`Clamp` 共同演化。`betweenRowCountOnColumn` 有意只向 `Est` 加 TopN，Min/Max 仍来自 Histogram；扩展时不能无意改变这一契约。
- Range 当前只读取 `LowVal[0]`、`HighVal[0]`；本文件估算的是单列投影。多列索引仅由 `getPseudoRowCountWithPartialStats` 主动拆分后逐列调用。
- 字符串边界会在局部克隆上替换为 collation key，原始 `ranger::Range` 不被修改。多列回退复用同一个 `tmpRan`，但每次调用都同步覆盖当前 Datum 和最后一列的开闭标志。
- 唯一可观察的外部记账副作用是 `recordUsedItemStatsStatus`；统计对象、Range 和上下文均以共享借用传入，本文件不持久化缓存或全局状态。

## 依赖与调用关系

直接上游（由 RustCodeGraph 文件使用关系和 `rg` 符号引用共同核对）：

- `pkg/planner/cardinality/selectivity.rs`：构造列范围后调用 `GetRowCountByColumnRanges` 得到选择率；交叉校验路径直接调用 `getColumnRowCount`。
- `pkg/planner/cardinality/row_count_index.rs`：索引统计无效但列统计可用时调用 `getPseudoRowCountWithPartialStats`；多处索引/handle 修正也调用列范围入口。
- `pkg/planner/cardinality/cross_estimation.rs`：把索引范围转换为单列范围后调用公开入口。
- `pkg/planner/core/operator/logicalop/logical_datasource.rs`：构造表访问范围后，把估算的 Est/MinEst/MaxEst 写入访问路径计数。

直接下游：

- `statistics`：Histogram/TopN/CM Sketch 查询、统计有效性、范围枚举、`RowEstimate` 运算和 out-of-range 估算。
- `ranger`：范围边界、开闭属性与 collator。
- `types`、`collate`、`codec`：Datum kind、比较、排序键和编码键；时区/位置来自表达式求值上下文。
- 同 crate 的 `pseudo.rs` 与 `selectivity.rs`：伪范围估算、越界等值选择率、均匀分布估算、桶末值低估判定和 used-stats 记账。
- `cost::ToleranceFactor` 防止浮点误差导致已覆盖全集的范围重复补算；`mathutil::Clamp` 限制部分统计总估算。

`Cargo.toml` 明确声明了上述本地 crate 依赖；本文件通过 `use crate::*` 使用 crate 根的 re-export 命名空间，没有自行引入 feature 或可选依赖。

## 错误处理与边界

- 可失败点包括 Datum 比较、key 编码、CM Sketch 查询、伪列范围估算及递归列估算；函数以 `errors::Error` 返回，部分边界用 `errors::Trace` 或 `NewNoStackError` 重新包装。
- 代码假设每个输入 Range 至少有一个 LowVal、HighVal 和（部分统计路径所需的）Collator；直接使用 `[0]`，不在本层校验畸形 Range。多列回退还假设 `idxCols.len()` 足以覆盖每个范围的 Datum 数。
- 统计有效性为 false 后使用 `expect` 取得列对象，依赖 `ColumnStatsIsInvalid(None, ...)` 必须返回 true 的不变量；破坏该不变量会 panic。
- `partialStatsRangeCollator` 对空 Collators 会 panic。这与 Go 直接读取 `indexRange.Collators[0]` 对齐，范围构造方必须保证存在 collator。
- 版本分界并非完全对称：等值路径以 `< Version2` 和否则分支处理，区间 TopN 路径以 `<= Version1` 判旧版；目前 Version1/Version2 常量使二者一致，新增统计版本时需重新审视。
- 特殊 Datum `MaxValue`、`MinNotNull` 不作为普通端点增减等值计数；NULL 仅在包含下界时补计。点范围的任一开端使该范围贡献零。
- `realtimeRowCount == 0` 时，部分统计入口显式返回 `(0, 0)`；真实统计主循环最终使用 `[1, 0]` clamp 的具体行为依赖 `RowEstimate::Clamp`，调用方通常应避免以零实时行数进入此路径。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel、事务、文件句柄或网络资源。所有计算在调用线程同步完成；共享统计与上下文均为不可变借用，Range 边界在栈上克隆后修改，编码缓冲区和临时范围由函数局部所有。

`getPseudoRowCountWithPartialStats` 的 `tmpRan` 在循环间复用以减少分配，但不会逸出函数；其内容在每一列调用前覆盖。`Box<dyn Collator>` 的克隆随临时 Range 或返回值按 Rust 所有权正常释放。并发安全因此主要取决于传入 `PlanContext` 与统计对象的实现，本文件不额外同步，也不延长其生命周期。

## 与 Go 版本的对应关系

主要函数与 `pkg/planner/cardinality/row_count_column.go` 一一对应，分支次序、StatsVer 语义、边界修正、增长因子、TopN 补算和部分统计计算均保持一致。Rust 特有适配包括：

- Go `init` 给 `statistics.GetRowCountByColumnRanges/GetRowCountByIndexRanges` 赋函数变量；Rust `init` 故意为空并依赖显式 API，避免 crate 依赖环。
- Go 的 `coll.UniqueID2colInfoID[colUniqueID]` 缺失时得到零；Rust 用 `get(...).copied().unwrap_or_default()` 显式复现。
- Go 的可空 TopN 方法调用由 crate 根 `OptionalTopNExt` 复现，Rust 中 `Option<TopN>` 的 `Num/TotalCount` 对 None 返回零。
- Rust 的 `ColumnStatsIsInvalid` 接口只传列和 `coll.Pseudo`，而当前 Go 版本还接收 `sctx/coll/colUniqueID`；本文档仅确认本文件实际调用契约，不推断两端未展示的异步加载副作用完全相同。
- Go 编码错误会经过 statement context 的 `HandleError`；Rust 直接用 `?` 传播 `codec::EncodeKey` 错误。两端都停止估算，但警告降级或错误处理策略是否完全等价需由 codec/eval context 的独立文档验证。
- Rust `Histogram::OutOfRangeRowCount` 额外显式传入布尔参数与 `RiskRangeSkewRatio`；这是当前 Rust API 的接线，不应从旧 Go 调用签名推断为算法差异，需结合 statistics 实现单独核验。
- `partialStatsRangeCollator` 将 Go 的 `Collators[0]` 选择抽成可独立测试的辅助函数；它并不按列索引 `i` 选择 collator。

Go 的 `pkg/statistics/statistics_test.go` 验证伪/真实列统计、NULL、开闭区间、PK handle、有符号/无符号范围和实时行数增长；`pkg/planner/cardinality/selectivity_test.go` 进一步覆盖 collation、未知值、out-of-range、TopN、桶末值和风险比例。Rust 的对应独立测试目前分布在 `row_count_column_test.rs` 与 `row_count_index_test.rs`，覆盖面较 Go 少，不能据此宣称所有 Go 回归场景已在 Rust 独立复现。

## 扩展指南

- 新增等值数据源或改变优先级时修改 `equalRowCountOnColumn`，同时在独立的 `row_count_column_test.rs` 或 `row_count_index_test.rs` 增加 StatsVer、NULL、TopN 命中/未命中、零/正 Repeat、越界和错误传播用例。
- 改变范围语义时以 `getColumnRowCount` 为接入点，必须成对验证点/枚举/一般区间、四种开闭组合、NULL、`MinNotNull/MaxValue`、字符串 collation、增长因子和最终 clamp；测试逻辑不得内嵌回生产源文件。
- 改变 TopN 与 Histogram 合并方式时同步审查 `betweenRowCountOnColumn` 的“只更新 Est”不变量及 `histNDV` 扣除规则，避免重复计数或悄然改变 Min/Max 上下界。
- 扩展部分索引统计时同步审查 `row_count_index.rs` 的回退条件、独立性乘积与相关性上界含义、最后一列开闭继承，以及首 collator 的 Go 兼容约束。
- 修改公开入口签名或错误类型时检查 `selectivity.rs`、`row_count_index.rs`、`cross_estimation.rs` 和 `logical_datasource.rs` 四类调用方；特别留意逻辑数据源把三个估算字段分别写入路径成本输入。
- 性能风险集中在每范围两次编码、Datum/collator 克隆、StatsVer 1 小范围枚举和多列部分统计的嵌套循环。优化时应保持编码与 collation 语义，并用真实调用规模证明收益。
- 兼容风险包括 Go 零值 map 行为、空 TopN 行为、至少一行启发式、PK 点范围至多一行、错误处理以及 used-stats 记账；这些都应作为回归断言，而不是仅验证函数可编译。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/cardinality` 确认目标、Go 对照及独立测试均已索引；`node --file pkg/planner/cardinality/row_count_column.rs --offset 1 --limit 500` 完整读取 384 行并报告直接使用文件。精确 `query` 还确认 Go/Rust 同名的 `GetRowCountByColumnRanges`、`equalRowCountOnColumn`、`getColumnRowCount`。
- 生产源码：`pkg/planner/cardinality/row_count_column.rs`；crate 装配与可空 TopN 适配：`pkg/planner/cardinality/lib.rs`；依赖和 Go 包映射：`pkg/planner/cardinality/Cargo.toml`。
- Rust 调用证据：`pkg/planner/cardinality/selectivity.rs`、`row_count_index.rs`、`cross_estimation.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs`。
- Rust 测试证据：`pkg/planner/cardinality/row_count_column_test.rs` 验证首 collator 复用；`row_count_index_test.rs` 验证零 Repeat 均匀回退、正 Repeat 精确值、TopN/Histogram 区间合并及列范围边界调用。
- Go 对照与测试：`pkg/planner/cardinality/row_count_column.go`、`pkg/planner/cardinality/selectivity_test.go`、`pkg/statistics/statistics_test.go`、`pkg/statistics/merge_global_test.go`、`pkg/statistics/handle/handletest/handle_test.go`。
- 本任务只新增说明文档，未运行 Cargo 或代码测试；按计划以源码/调用图/对照测试事实复核和固定章节结构检查作为验收。
