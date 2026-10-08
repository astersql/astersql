# `pkg/statistics/column.rs`

## 文件定位

`column.rs` 位于 `astersql-statistics` crate 内。`pkg/statistics/lib.rs` 以私有模块 `mod column` 装配它，再通过 `pub use column::*` 把本文件的公开类型、方法和函数重导出为 crate 级 API。`pkg/statistics/Cargo.toml` 指定 crate 根为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/statistics"` 标明其 Go 对照包。

本文件定义“单列统计”的聚合对象，而不是负责生成、持久化或异步加载统计的服务。它把 `Histogram`、`CMSketch`、`TopN`、`FMSketch`、列元数据、统计版本和加载/驱逐状态收拢到 `Column`，供表级统计缓存、同步加载器和优化器基数估算读取。实际直方图算法与加载状态定义分别在 `pkg/statistics/histogram.rs`，表级容器在 `pkg/statistics/table.rs`。

## 核心职责

- 用 `ColumnInfo` 保存本文件所需的最小列元数据：列 ID、名称、字段类型和主键标志。
- 用 `Column` 聚合一列的直方图、三种可选草图/高频值结构、物理表 ID、统计版本、handle 标志和 `StatsLoadedStatus`。
- 统一提供行数、非空行数、增长因子、内存占用、统计可用性与加载状态查询。
- 在缓存驱逐时通过 `DropUnnecessaryData` 丢弃桶边界、桶、标量缓存和 TopN，并按统计版本决定是否同时丢弃 CMSketch。
- 用 `ColumnStatsIsInvalid` 为规划器提供无副作用的有效性门槛；用 `EmptyColumn` 为伪统计或尚无 ANALYZE 结果的列建立空骨架。

本文件不执行 I/O、不调度 ANALYZE、不访问 session，也不直接把加载请求写入队列。Rust 当前调用链把“统计是否无效”和“触发加载”拆开；不能根据 Go 同名函数推断 Rust 版本具有异步加载副作用。

## 主要符号

- `pub struct ColumnInfo`：包含 `ID: i64`、`Name: String`、`FieldType: types::FieldType`、`IsPrimaryKey: bool`。它是统计层本地元数据快照，不是 `pkg/meta/model` 的完整列描述。
- `pub struct Column`：核心列统计对象。`CMSketch`、`TopN`、`FMSketch` 和 `Info` 均为 `Option`；`Histogram` 与 `StatsLoadedStatus` 总是存在；`PhysicalID` 标识物理表或分区；`StatsVer` 决定 V1/V2 兼容分支；`IsHandle` 标记该列是否作为 handle。
- `Deref<Target = Histogram>` / `DerefMut`：允许 `Column` 透明调用 `Histogram` 的只读或可变方法。阅读调用点时需注意，形如 `column.NDV` 或 `column.OutOfRange(...)` 可能来自解引用后的直方图，而非 `Column` 自身字段/方法。
- `Copy(&self) -> Column`：调用 `Clone` 复制整个对象。Rust 所含 `String`、`Vec` 和可选子结构按其 `Clone` 语义复制；返回值不是共享引用。
- `String(&self) -> String`：委托 `Histogram::ToString(0)` 输出直方图文本。
- `TotalRowCount` / `NotNullCount`：当 `StatsVer >= Version2` 时，在直方图计数上叠加 `TopN::TotalCount`；TopN 缺失按 0 处理。V1 只读直方图计数。
- `GetIncreaseFactor(realtime_row_count)`：返回实时行数与统计总行数之比；统计总数为 0 时固定返回 `1.0`，避免除零。
- `MemoryUsage`：汇总 `Histogram`、CMSketch、TopN、FMSketch 的内存估算，缺失组件贡献 0；返回单个 `i64` 总量，不计 `ColumnInfo` 等元数据，也不返回分类明细。
- `ItemID`：优先返回 `Info.ID`，`Info` 缺失时退回 `Histogram.ID`。
- `DropUnnecessaryData`：释放可重载的大块数据并把 `evictedStatus` 设为 `AllEvicted`。V1 清除 CMSketch，V2 保留 CMSketch；两者均清除 TopN、`Histogram.Bounds`、`Buckets`、`Scalars`，但保留直方图的 NDV、NullCount 等标量元数据和 FMSketch。
- `IsAllEvicted`、`GetEvictedStatus`、`IsStatsInitialized`、`IsLoadNeeded`、`IsEssentialStatsLoaded`、`IsFullLoad`：查询或委托 `StatsLoadedStatus`。其不变量来自 `histogram.rs`：只有 `statsInitialized` 为真时才可能“需加载”“必要数据已加载”“全驱逐”或“全加载”。
- `GetStatsVer`、`IsCMSExist`、`IsAnalyzed`、`StatsAvailable`：暴露版本和统计可用性。`IsAnalyzed` 只依据版本；`StatsAvailable` 还接受 `NDV > 0` 或 `NullCount > 0` 的合成统计，因此前者成立集合是后者的子集。
- `GetHistogram` / `GetTopN`：分别返回直方图引用与可选 TopN 引用，不转移所有权。
- `ColumnStatsIsInvalid(column, pseudo)`：列缺失、表为伪统计、总行数为 0，或 `NDV > 0` 且必要统计未加载时返回真。`NDV == 0` 时不会仅因必要数据未加载而判无效。
- `EmptyColumn(physical_id, primary_key_is_handle, info)`：用 `NewHistogram` 建立零 NDV/零空值/零桶的直方图，所有草图为空、状态为默认未初始化、版本为 0；只有表以主键为 handle 且列自身为主键时，`IsHandle` 才为真。

本文件没有模块级常量、trait、自定义错误、异步函数或条件编译项。

## 执行流程

典型规划路径如下：

1. `pkg/planner/core/operator/logicalop/logical_datasource.rs::build_pseudo_hist_coll` 将表元数据裁剪为本文件的 `ColumnInfo`，调用 `EmptyColumn`，随后用 `NewPseudoHistogram` 替换空直方图，并按优化器列的 `UniqueID` 放入 `HistColl.Columns`。
2. 基数估算入口（例如 `pkg/planner/cardinality/row_count_column.rs::GetRowCountByColumnRanges`）先调用 `ColumnStatsIsInvalid`。无效时走伪统计估算；有效时取得非空的 `Column`，进入真实直方图/CMSketch/TopN 估算。
3. 真实估算通过 `TotalRowCount`、`NotNullCount` 与 `GetIncreaseFactor` 把 ANALYZE 时刻的分布扩展到实时表行数。V2 的 TopN 值从直方图桶中独立维护，因此总量必须额外叠加；V1 不叠加。
4. 表统计缓存需要回收内存时，`pkg/statistics/table.rs::HistColl::DropEvicted` 或 `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs` 调用 `DropUnnecessaryData`。之后状态查询会报告全驱逐/需加载，规划器不能把已清空的桶当作完整统计。
5. `pkg/statistics/handle/syncload/stats_syncload.rs` 使用 `IsFullLoad` 与 `StatsAvailable` 判断缓存数据是否足够，以及加载结果能否替换现有列统计。

`ColumnStatsIsInvalid` 的判断顺序具有可观察意义：`None` 立即无效；其余条件是逻辑或。它调用的 `TotalRowCount` 会按版本包含 TopN，因此 V2 中“直方图为零但 TopN 非零”仍可能有效。

## 数据与状态

`Column` 同时持有分布数据与生命周期元数据：

- `Histogram` 保存 NDV、NullCount、桶、边界和标量缓存，是区间估算的主体。
- `CMSketch` 用于频率估计；旧版本驱逐时会清除它，V2 驱逐时保留。
- `TopN` 保存高频值。V2 的 `TotalRowCount`/`NotNullCount` 必须把其计数加回；驱逐会清除 TopN。
- `FMSketch` 用于 NDV 草图，本文件仅计入复制和内存估算，驱逐函数不会清除它。
- `StatsLoadedStatus` 是两个字段组成的值类型：是否初始化和驱逐等级。默认值表示未初始化；`AllLoaded` 表示完整加载；`AllEvicted` 表示必要载荷已清除。
- `StatsVer` 不只是标签：它控制计数口径和 CMSketch 驱逐策略。修改版本条件时必须同时核对 `index.rs` 的平行实现以及 Go 行为。
- `PhysicalID` 由调用方设置，本文件不验证其正负值。Go 注释说明 `-1` 可代表统计不可用且不应触发加载；Rust 本文件没有触发加载逻辑，因此这里只保存该值。

重要不变量是：V2 的直方图计数不含 TopN 计数；调用方需要列级总量时应使用 `Column::TotalRowCount`，不能只用 `Histogram::TotalRowCount`。另一个不变量是驱逐后桶向量为空，但 NDV 等摘要仍可保留，所以有效性还必须结合 `StatsLoadedStatus` 判断。

## 依赖与调用关系

直接依赖均通过 crate 根重导出：`Histogram`/`NewHistogram`、`CMSketch`、`TopN`、`FMSketch`、`StatsLoadedStatus`、`AllEvicted`、`Version2`、`IsAnalyzed` 和 `IsColumnAnalyzedOrSynthesized`。`ColumnInfo::FieldType` 依赖 `astersql-types-datum` 在 Cargo 中别名为 `types` 的 `FieldType`。

主要上游调用者/持有者包括：

- `pkg/statistics/table.rs::HistColl` 以 `HashMap<i64, Box<Column>>` 持有列统计，并调用加载状态、计数和驱逐接口。
- `pkg/planner/cardinality/{selectivity,row_count_column,row_count_index,cross_estimation}.rs` 调用 `ColumnStatsIsInvalid`、行数和增长因子接口，决定伪估算或真实统计估算。
- `pkg/planner/core/operator/logicalop/logical_datasource.rs` 调用 `EmptyColumn` 构造伪直方图集合。
- `pkg/statistics/handle/syncload/stats_syncload.rs` 调用 `IsFullLoad` 与 `StatsAvailable` 管理同步加载后的缓存替换。
- `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs` 在缓存回收时调用 `DropUnnecessaryData`。

下游调用集中在内存内的数据结构方法：直方图负责计数、文本和内存估算；各 sketch/TopN 负责自己的内存或计数；加载状态对象负责状态谓词。本文件没有网络、存储、锁或 channel 依赖。

## 错误处理与边界

本文件所有 API 都是非 `Result` 接口，没有显式错误传播；边界主要通过默认值与布尔判断表达：

- 缺失列统计在 `ColumnStatsIsInvalid` 中判无效；缺失 TopN/sketch 在计数和内存估算中按 0 处理。
- 总行数为 0 时增长因子返回 1，避免无穷大或 NaN。
- `ItemID` 在 `Info` 缺失时回退到直方图 ID，避免解包失败；这与 Go 直接访问 `c.Info.ID` 不同。
- `EmptyColumn` 不校验 ID、字段类型或物理表 ID；调用方负责提供一致元数据。
- `DropUnnecessaryData` 是破坏性操作且没有恢复机制。恢复完整桶/TopN 必须由上层重新加载并替换或填充统计对象。
- `DerefMut` 允许调用者直接修改内嵌直方图，因此本文件无法自行维护跨字段一致性；修改桶、NDV、NullCount、StatsVer 或 TopN 的代码必须共同维护 V2 计数口径。
- `ColumnStatsIsInvalid` 当前不接收列 ID、`HistColl` 或 plan context，故不会执行 Go 版的 restricted SQL 短路、内部列过滤或异步加载登记。

## 并发与资源生命周期

`Column` 自身没有内部锁、原子变量、引用计数、后台任务或异步生命周期。所有可变操作都要求 `&mut self`，并发安全由持有它的表统计缓存/handle 层负责。本文件也不声明 `Send`/`Sync` 的额外保证；实际能力由字段类型自动决定。

资源生命周期以“构造或加载 → 只读估算 → 缓存驱逐 → 上层重新加载/替换”为主。`Copy` 生成独立所有权的克隆；`GetHistogram`/`GetTopN` 只借用当前对象，借用不能超过 `Column` 生命周期。`DropUnnecessaryData` 立即释放向量和可选 TopN 的所有权，并将状态推进到 `AllEvicted`；它不会清除全部摘要字段，也不会主动发起重载。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/statistics/column.go`。主要保持一致的语义包括：Column 聚合结构、V2 计数叠加 TopN、零行数增长因子、按版本驱逐 CMSketch、加载状态代理、分析/合成统计判断，以及空列骨架的用途。

已验证的当前差异如下：

- Rust `ColumnStatsIsInvalid(Option<&Column>, bool)` 是纯判断；Go 版本还接收 `PlanContext`、`HistColl`、列 ID，在统计缺失/未初始化/需加载时可能登记异步加载，并处理 restricted SQL、内部列 ID 与 `CanNotTriggerLoad`。这些副作用不能视为已移植到本文件。
- Rust `MemoryUsage() -> i64` 只给总数；Go 返回 `CacheItemMemoryUsage`/`ColumnMemUsage`，包含列 ID 和各组件明细。Rust 也安全处理 `Info == None`，Go 构造明细时直接读取 `Info.ID`。
- Rust `Copy` 由派生 `Clone` 实现并返回值；Go 手工逐字段深拷贝并保留 nil receiver 返回 nil 的约定。Rust 方法不能在 `None` 上调用，调用方应复制 `Option<Column>` 而非期待 nil receiver 语义。
- Rust `ItemID` 在 `Info` 缺失时退回 `Histogram.ID`；Go 假定 `Info` 存在。
- Rust `EmptyColumn` 接收拥有所有权的精简 `ColumnInfo`，主键判断直接使用 `IsPrimaryKey`；Go 接收 `*model.ColumnInfo` 并从 MySQL flag 判断主键。
- Go 的 `Column` 匿名嵌入 `Histogram`/`StatsLoadedStatus`；Rust 使用具名字段，并用 `Deref`/显式代理方法模拟常用访问。两者 API 外形并不完全等价。

因此，后续对齐 Go 提交时应逐项确认是补齐本文件、在 handle/planner 层接线，还是保持 Rust 分层设计，不能只按同名函数机械复制。

## 扩展指南

- 新增列级统计组件时，至少同步检查 `Column` 字段、`Copy`/`Clone` 语义、`MemoryUsage`、`DropUnnecessaryData`、表级内存汇总和缓存驱逐路径；若影响总量，还要同步 V1/V2 计数规则。
- 修改有效性规则时，以 `ColumnStatsIsInvalid` 为入口，并同步核对所有 planner cardinality 调用点。若要移植 Go 的加载副作用，应先决定依赖注入边界，避免把 session/全局队列直接耦合进基础数据结构 crate。
- 修改加载状态时，应优先改 `pkg/statistics/histogram.rs::StatsLoadedStatus` 的谓词，再确认本文件的代理方法、`table.rs::ColumnIsLoadNeeded`、同步加载器和 LFU 缓存仍一致。
- 修改 `EmptyColumn` 时，同步检查 `logical_datasource.rs::build_pseudo_hist_coll`，确保列 ID、UniqueID、物理表 ID、字段类型和 handle 判定没有混淆。
- 修改 V2 TopN 口径时，必须同时核对 `index.rs` 的平行实现和所有直接读取 `Histogram::TotalRowCount` 的调用者，防止漏加或重复加 TopN。
- 回归测试应放在独立测试文件，不放回 `column.rs`。优先扩展 `pkg/statistics/integration_test.rs`（计数与加载生命周期）、`pkg/statistics/table_test.rs`（表级装配/驱逐）或新增同目录独立 `column_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 区域注册。涉及规划结果时同步扩展 `pkg/planner/cardinality/*_test.rs`；涉及真实异步加载时参考 `pkg/statistics/handle/storage/read_test.rs` 与 `tests/realtikvtest/statisticstest/statistics_test.rs`。
- 重点风险是 V1/V2 兼容、TopN 重复计数、驱逐后误用空桶、合成统计被误判无效，以及把 Go 的异步副作用遗漏或错误放入基础层。性能风险集中在克隆大型直方图/草图和内存估算遗漏新增字段。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/statistics/column.rs` 确认目标文件含 27 个符号；`node --file pkg/statistics/column.rs` 阅读了完整 231 行并显示该文件被 26 个文件使用；`query` 分别定位了 Rust/Go 的 `ColumnStatsIsInvalid`、`EmptyColumn`、`DropUnnecessaryData`、`TotalRowCount`；`node` 核验了 `histogram.rs::StatsLoadedStatus`、`IsColumnAnalyzedOrSynthesized` 及其状态谓词。
- 源与装配：`pkg/statistics/column.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/Cargo.toml`。
- 直接调用链：`pkg/statistics/table.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`pkg/planner/cardinality/row_count_column.rs`、`pkg/planner/cardinality/selectivity.rs`、`pkg/statistics/handle/syncload/stats_syncload.rs`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`。
- Go 对照：`pkg/statistics/column.go`；加载状态的 Go 定义位于 `pkg/statistics/histogram.go`。
- 独立 Rust 测试：`pkg/statistics/integration_test.rs` 覆盖列总行数与懒加载状态切换；`pkg/statistics/histogram_test.rs` 覆盖加载/驱逐状态谓词；`pkg/statistics/table_test.rs` 覆盖列是否需要加载；`pkg/statistics/handle/storage/read_test.rs` 记录 Go 异步加载契约在 Rust 测试层的对应场景；`tests/realtikvtest/statisticstest/statistics_test.rs` 覆盖真实统计加载/驱逐状态。当前未发现同名独立 `column_test.rs`。
- 人工核对结果：文档区分了本文件的纯数据结构职责与上层加载/规划职责；对 Go 扩展语义均明确标注为差异，没有将未接线能力写成已支持。
