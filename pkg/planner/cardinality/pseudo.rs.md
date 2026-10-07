# `pkg/planner/cardinality/pseudo.rs`

## 文件定位

本文件属于 `astersql-planner-cardinality` crate。`pkg/planner/cardinality/Cargo.toml` 将 `lib.rs` 设为库入口且关闭自动测试发现；`lib.rs` 以私有模块 `mod pseudo` 装配本文件，再通过 `pub use pseudo::*` 导出其中的公开项。它位于优化器基数估算的兜底层：当直方图、TopN、CM Sketch 或索引统计缺失/无效时，为选择率、列范围和索引范围提供确定性的伪统计估算，而不负责收集或持久化统计信息。

直接入口分别来自 `selectivity.rs::Selectivity`、`row_count_column.rs::GetRowCountByColumnRanges` 和 `row_count_index.rs::GetRowCountByIndexRanges`。因此它处在“谓词/Range 已构造完成、真实统计不可用”与“向规划器返回估算行数或选择率”之间。

## 核心职责

- 用固定经验分母定义无统计时的基准：等值或 `IN` 为 `1 / pseudoEqualRate`（1/1000），单边比较为 `1 / pseudoLessRate`（1/3），有界区间为 `1 / pseudoBetweenRate`（1/40）。
- `pseudoSelectivity` 从一组表达式中选择最严格的可识别因子，并对单列唯一键、主键或谓词完整覆盖的复合唯一索引收紧为“一行占全表的比例”。
- 两个整型 Range 函数为主键 handle 估算行数，区分有符号和无符号边界，并用整数值域宽度限制结果。
- 通用列 Range 与索引 Range 函数处理 NULL、无穷边界、点查、区间、复合索引等值前缀，并把所有 Range 的结果限制在表行数内。
- `PseudoAvgCountPerValue` 为没有直方图时提供每个值的平均伪频次。

这些函数给出的是启发式估算，不承诺反映真实数据分布；其意义是让代价优化在统计缺失时仍能继续工作并保持与 Go 实现一致的相对选择性。

## 主要符号

- `pseudoEqualRate: f64 = 1000.0`：crate 内可见，既供本文件使用，也被 `selectivity.rs` 的其他回退分支复用。
- `pseudoLessRate`、`pseudoBetweenRate`：本模块私有，分别控制单边比较和双边区间。
- `PseudoAvgCountPerValue(&statistics::Table) -> f64`：公开 API，以 `RealtimeCount / 1000` 返回平均频次。
- `pseudoColumnHasUniqueKey(&statistics::ColumnInfo) -> bool`：crate 内辅助函数，同时接受 `IsPrimaryKey` 和字段类型上的 `mysql::UniqueKeyFlag`。独立 Rust 测试直接覆盖该组合语义。
- `pseudoSelectivity(&dyn planctx::PlanContext, &statistics::HistColl, &[expression::ExprBox]) -> f64`：选择率回退入口，只识别可转为 `ScalarFunction` 且能由 `getConstantColumnID` 找到常量比较列的表达式。
- `getPseudoRowCountBySignedIntRanges` / `getPseudoRowCountByUnsignedIntRanges`：主键 handle 的整型 Range 估算，返回 `f64` 且不产生错误。
- `getPseudoRowCountByIndexRanges(&types::Context, &[&ranger::Range], f64, usize) -> Result<f64, errors::Error>`：索引伪估算，可能传播前缀比较或 Datum 比较错误。
- `getPseudoRowCountByColumnRanges(&types::Context, f64, &[&ranger::Range], usize) -> Result<f64, errors::Error>`：指定 Range 列位置的通用估算核心。

文件没有类型、trait、`impl` 或条件编译项；状态全部来自参数和局部变量。

## 执行流程

选择率路径如下：

1. `selectivity.rs::Selectivity` 在谓词超过 63 个，或集合同时没有列统计和索引统计时调用 `pseudoSelectivity`；空表和空谓词已在调用前返回 1.0。
2. `pseudoSelectivity` 以会话变量 `SelectivityFactor` 为初值，跳过非标量函数以及无法确定列 ID 的表达式。
3. 对 `EQ`、`NullEQ`、`In`，候选因子收紧至不高于 1/1000；若列元信息表示主键或唯一键，立即返回 `1 / RealtimeCount`。同时记录已被等值谓词覆盖的列名。
4. 对 `GE`、`GT`、`LE`、`LT`，候选因子收紧至不高于 1/3；当前不把成对上下界合并为 BETWEEN，源文件保留了与 Go 相同的 FIXME。
5. 若存在等值列，遍历所有索引；只有索引的每一列都出现在集合中且索引标记为 `Unique`，才按唯一键返回 `1 / RealtimeCount`。首列开始匹配时会调用 `IndexStatsIsInvalid`，保留统计有效性检查/加载的副作用。

列与主键 Range 路径如下：

1. `row_count_column.rs::GetRowCountByColumnRanges` 先判断列统计是否无效。主键 handle 根据首个低边界 Datum 类型选择有符号或无符号实现；普通列调用 `getPseudoRowCountByColumnRanges`。
2. 整型实现把 NULL/`MinNotNull` 映射到最小端，把 `MaxValue` 映射到最大端；全域返回全表、单边区间返回 1/3、点范围返回 1、有界范围返回 1/40。
3. 每段结果再受端点差值约束，多段求和后以 `tableRowCount` 封顶。有符号版本使用 `wrapping_sub`，保留 Go 在极值相减溢出时不会进入宽度收紧分支的效果。
4. 通用列实现将 `[NULL, MaxValue]` 视为全域；`[MinNotNull, MaxValue]` 排除估算的 NULL 数；单边上界使用 1/3；有限端点通过 `Datum::Compare` 区分点查（1/1000）与区间（1/40）。

索引路径由 `row_count_index.rs::GetRowCountByIndexRanges` 在索引统计无效、且不能利用部分列统计时进入。每个索引 Range 先由 `PrefixEqualLen` 求完整等值前缀；完整覆盖唯一索引且端点均包含时直接计一行，否则由首个非等值列的通用列估算决定剩余比例，再对每个等值前缀额外除以 100。所有 Range 相加若超过全表，不是简单封顶，而是回退到 `tableRowCount / 3`，与 Go 行为一致。

## 数据与状态

核心输入是 `statistics::HistColl` 的 `RealtimeCount`、`Pseudo` 标志、列/索引元信息，以及 `ranger::Range` 的 `LowVal`、`HighVal`、排除端点标志和逐列 Collator。`types::Context` 为 Datum 比较提供类型语义；会话的 `SelectivityFactor` 是无法识别更具体谓词时的默认上限。

`pseudoSelectivity` 的 `HashSet<String>` 只保存被等值类谓词命中的列名，供复合唯一索引完整覆盖判断使用；它不缓存跨调用状态。Range 函数的计数均是局部 `f64` 累加器。文件不修改表统计本身，但 `ColumnStatsIsInvalid`、`IndexStatsIsInvalid` 调用保留了统计检查或按需加载的语义副作用。

重要不变量是：Range 至少含有所访问的 `colIdx`，索引 Range 的 `LowVal` 非空，`Collators[colIdx]` 存在；这些由上游 ranger 构造流程保证，本文件直接索引而不自行校验。`pseudoSelectivity` 在返回唯一键比例时假定 `RealtimeCount > 0`；当前直接调用者 `Selectivity` 已对零行表提前返回。

## 依赖与调用关系

上游调用边经 RustCodeGraph 与源码核对：

- `selectivity.rs::Selectivity -> pseudoSelectivity`：统计节点不足或谓词数量过多时的整组选择率回退。
- `row_count_column.rs::GetRowCountByColumnRanges -> getPseudoRowCountBySignedIntRanges / getPseudoRowCountByUnsignedIntRanges / getPseudoRowCountByColumnRanges`：列统计无效时，按 handle 类型或普通列分流。
- `row_count_index.rs::GetRowCountByIndexRanges -> getPseudoRowCountByIndexRanges`：索引统计无效且部分统计路径不适用时回退。
- `getPseudoRowCountByIndexRanges -> getPseudoRowCountByColumnRanges`：复合索引首个非等值列复用通用 Range 规则。

下游依赖由 `lib.rs` 的 re-export 别名提供：`expression`（表达式分类和参数）、`statistics`（表、直方图集合和元信息）、`ranger`（Range 与等值前缀）、`types`（Datum、Kind、比较上下文）、`planctx`（会话因子）、`ast`（函数名）、`mysql`（唯一键标志）和 `errors`（错误追踪）。这些分别对应 `Cargo.toml` 中的本地 workspace 依赖；该 crate 没有专门控制本文件的 feature。

## 错误处理与边界

- `getPseudoRowCountByIndexRanges` 将 `Range::PrefixEqualLen` 的错误包装为 `errors::Trace` 后立即返回；它调用的列 Range 比较错误通过 `?` 继续传播。
- `getPseudoRowCountByColumnRanges` 的唯一可恢复错误来自 `Datum::Compare`，常见于不兼容类型或排序规则比较失败；同样用 `errors::Trace` 保留错误链。
- `tableRowCount == 0` 时索引函数显式返回 `Ok(0)`，避免后续比例计算除零。选择率函数依赖上游零行短路，不适合作为不经契约检查的独立零行入口。
- 多 Range 的结果不会超过表行数；索引总和异常超过全表时按 1/3 回退。负的 `tableRowCount` 不属于正常统计契约，代码没有专门防御。
- 空主键 Range 由 `GetRowCountByColumnRanges` 在进入本文件前返回 0；本文件对空切片自然累加为 0。相反，畸形的空 `LowVal` 或缺失 Collator 会因直接索引而 panic，属于 ranger 输入不变量而非本模块的错误返回面。
- 当前比较选择率没有识别 BETWEEN 对，仍按单个比较的 1/3 处理；这是明确记录的精度限制。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务、文件或网络资源。所有估算器都是同步调用，拥有的集合和计数器在函数返回时释放；传入的统计、表达式与 Range 仅借用。

并发安全主要取决于借用对象及 `PlanContext`/统计有效性检查的实现。本文件自身没有可变全局状态；常量是只读的。调用 `ColumnStatsIsInvalid` 与 `IndexStatsIsInvalid` 可能触发统计加载相关行为，但本文件不持有加载任务或资源句柄，也不改变其生命周期。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/cardinality/pseudo.go`。三个经验分母、平均频次、选择率分支、整数边界映射、范围宽度限制、复合索引每列除以 100、总量超过全表后回退 1/3，以及 Datum 比较错误追踪均保持 Go 算法顺序和数值语义。

可见差异如下：

- Go 的 `pseudoSelectivity` 通过 `mysql.HasUniKeyFlag(col.Info.GetFlag())` 判定唯一列；Rust 将该语义封装为 `pseudoColumnHasUniqueKey`，并同时显式接受 `ColumnInfo::IsPrimaryKey`。`selectivity_test.rs::test_pseudo_unique_key_uses_unique_key_flag` 分别验证唯一键标志、主键和普通列。
- Go 的统计有效性调用携带 session context、列/索引 ID；当前 Rust 统计 API 为 `ColumnStatsIsInvalid(None, coll.Pseudo)` 和 `IndexStatsIsInvalid(None, coll.Pseudo)`。文档只能确认 Rust 保留了调用点，不能据此声称其按需加载细节与 Go 参数级行为完全等价。
- Rust 用 `Result<_, errors::Error>` 和 `map_err/errors::Trace` 表达 Go 的 `(value, error)`；用 `wrapping_sub` 明确模拟有符号极值相减的溢出效果。
- Rust 参数以借用切片和 trait object 表达，算法没有复制 Range 或统计对象；这属于语言层表示差异，不改变估算规则。

Go 的 `selectivity_test.go` 通过公开的列/索引估算入口覆盖多种真实统计与伪统计场景，但未发现直接点名这些私有 pseudo 函数的同名测试。Rust 当前直接测试仅覆盖唯一键辅助判断；其他分支主要由 `selectivity_test.rs`、`row_count_index_test.rs` 经公开入口间接触达，不能把存在调用断言等同于每个伪分支已有完整单元覆盖。

## 扩展指南

- 调整默认倍率时，应修改对应常量并同步检查 `selectivity.rs` 对 `pseudoEqualRate` 的复用；数值变化会影响计划选择与 Go 兼容性，需在独立 `*_test.rs` 中增加精确行数/选择率回归，不能把测试写进本源文件。
- 增加新的谓词类别时，入口是 `pseudoSelectivity` 的函数名匹配。需明确它是否贡献等值列集合、是否可证明唯一性，并与 Go `pseudo.go` 保持分支顺序。
- 改进 BETWEEN 识别需要联合分析成对谓词，不能只把单个比较改成 1/40；应覆盖顺序互换、开闭端点、同列/异列和不可识别表达式。
- 扩展 Range Kind 或复合索引算法时，优先复用 `getPseudoRowCountByColumnRanges`，同时维护 `colIdx`、`LowVal/HighVal/Collators` 长度不变量。任何新增可失败比较都应继续返回并追踪错误，而非静默降级。
- 若允许直接对零行集合调用 `pseudoSelectivity`，需先定义零行表的选择率契约，避免唯一键分支除零；当前契约由 `Selectivity` 上游短路保证。
- 测试应优先补到 `pkg/planner/cardinality/selectivity_test.rs` 或与入口对应的 `row_count_column_test.rs` / `row_count_index_test.rs` 独立文件；对照 Go 时同步检查 `pseudo.go` 和 `selectivity_test.go`，尤其关注 NULL、无穷端点、唯一复合索引、重叠多 Range、比较错误及极值溢出。

## 验证依据

- RustCodeGraph：索引状态检查后读取了 `pseudo.rs` 全部 261 行；查询确认 `pseudoSelectivity`、`getPseudoRowCountByIndexRanges`、`getPseudoRowCountByColumnRanges` 及 `pseudoColumnHasUniqueKey`，并报告本文件被 `row_count_column.rs`、`row_count_index.rs`、`selectivity.rs`、`selectivity_test.rs` 使用。
- 装配与调用源码：`pkg/planner/cardinality/lib.rs`、`selectivity.rs::Selectivity`、`row_count_column.rs::GetRowCountByColumnRanges`、`row_count_index.rs::GetRowCountByIndexRanges`。
- crate 边界：`pkg/planner/cardinality/Cargo.toml`，确认包名、`lib.rs` 入口、依赖别名、开发依赖和 `go-package` 映射。
- Go 对照：完整读取 `pkg/planner/cardinality/pseudo.go`；相关公开入口测试位于 `pkg/planner/cardinality/selectivity_test.go`。
- Rust 测试：读取 `pkg/planner/cardinality/selectivity_test.rs::test_pseudo_unique_key_uses_unique_key_flag`，并检索 `selectivity_test.rs`、`row_count_index_test.rs` 中公开估算入口的调用。目录中不存在 `doc.go`，因此没有额外的 Go package contract 文件可读。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核所有行为结论均可回指上述符号或文件。
