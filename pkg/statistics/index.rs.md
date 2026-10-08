# `pkg/statistics/index.rs`

## 文件定位

`index.rs` 定义单个数据库索引的统计信息载体及其基础查询、内存核算和驱逐操作。它属于 `astersql-statistics` crate：`pkg/statistics/Cargo.toml` 将 crate 根设为 `lib.rs`，`pkg/statistics/lib.rs` 以私有 `mod index` 装配本文件，再通过 `pub use index::*` 将其中的类型和函数暴露给 planner、统计缓存与统计加载模块。

该文件位于优化器统计链的中间层：下游组合 `Histogram`、`CMSketch`、`TopN`、`FMSketch` 和 `StatsLoadedStatus`；上游由 `pkg/planner/cardinality/row_count_index.rs`、`pkg/planner/cardinality/selectivity.rs` 等读取，用于索引范围行数与等值条件选择率估算。`pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs` 会通过这里的驱逐和内存接口保留低成本统计壳，`pkg/statistics/handle/syncload/stats_syncload.rs` 则读取完整加载状态决定是否补载。

本文件不是存储读取器，也不负责 ANALYZE、异步加载调度或加锁；它只描述一个已经构造出来的索引统计对象及其本地派生行为。源码没有条件编译项。独立 Rust 测试位于 `pkg/statistics/index_test.rs`，另有 `pkg/statistics/histogram_test.rs` 覆盖查询优先级；测试没有内嵌在生产源文件中。

## 核心职责

- 用 `IndexColumnInfo`、`IndexInfo` 保存估算所需的索引元信息：索引 ID、名称、列及前缀长度、多值索引、唯一索引和条件表达式标志。
- 用 `Index` 将索引元信息、直方图、三类 sketch、物理表 ID、统计版本和加载/驱逐状态聚合为一个值对象。
- 通过 `Deref<Target = Histogram>` / `DerefMut` 让调用者直接访问内嵌直方图字段和方法，保持与 Go 匿名嵌入 `Histogram` 相近的调用形态。
- 为 planner 提供总行数、等值编码键频次、实时行数放大系数、有效性判断和加载状态查询。
- 为统计缓存提供可丢弃数据清理及内存占用估算；这些操作明确保留部分元数据和 `FMSketch`，不是销毁整个对象。

## 主要符号

- `IndexColumnInfo { Name, Length }`：单个索引列的名称与前缀长度。`Length` 被 `pkg/planner/cardinality/selectivity.rs::findAvailableStatsForCol` 用来排除前缀索引，只有 `types::UnspecifiedLength` 的单列索引才可替代列统计。
- `IndexInfo { ID, Name, Columns, MVIndex, Unique, ConditionExprString }`：Rust 本地的索引定义摘要。`Unique` 和 `Columns` 参与点范围估算，`MVIndex` 参与表分析行数选择，其他字段供识别与兼容信息保存。
- `Index`：核心公开结构。`CMSketch`、`TopN`、`FMSketch` 均可缺失；`Info` 也为 `Option`；`Histogram` 是范围分布主体；`StatsLoadedStatus` 表示是否初始化以及是否驱逐；`PhysicalID` 标识物理表/分区；`StatsVer` 控制 V1/V2 兼容分支。
- `InfoRef`：把可选元信息收窄为必有引用；缺失时以 `expect("initialized index statistics require IndexInfo")` 立即 panic。需要元信息的生产调用应先保证对象已经正确初始化。
- `Copy`：以 `Clone` 深拷贝当前 Rust 值。当前组成字段都是值或拥有型容器，因此副本不与原对象共享可变统计载荷。
- `ItemID`：优先返回 `Info.ID`；缺失 `Info` 时退回 `Histogram.ID`，避免测试或部分构造对象因取 ID 而 panic。
- `DropUnnecessaryData`：丢弃可重建的大块数据。统计版本低于 `Version2` 时额外丢弃 `CMSketch`；所有版本都丢弃 `TopN`、直方图边界、桶和 scalar，并设置 `AllEvicted`。它保留 `Info`、`FMSketch`、直方图概要字段以及 V2 的 `CMSketch`。
- `EvictAllStats`：测试所需的较窄驱逐操作，只清空 `CMSketch`、`TopN` 和直方图桶并标为 `AllEvicted`；它刻意保留 bounds、scalars、`FMSketch` 与元信息，见 `pkg/statistics/index_test.rs::evict_all_stats_only_drops_go_index_payloads`。
- `TotalRowCount`：V2 及以上返回直方图行数加 `TopN::TotalCount`；旧版本只返回直方图行数。`TopN` 缺失时按零处理。
- `QueryBytes`：按 `TopN`、`CMSketch`、`Histogram::EqualRowCount` 的顺序查询编码键频次，命中前一级即返回；直方图回退会按 `StatsVer >= Version2` 选择 V2 语义。
- `MemoryUsage`：累加 `Histogram`、`CMSketch` 和 `TopN` 的动态用量；与 Go 一致，不计 `FMSketch` 及其他元数据，测试 `memory_usage_ignores_fm_sketch_like_go` 固化了这一边界。
- `GetIncreaseFactor`：将实时行数除以统计总行数；统计总数为零时返回 `1.0`，避免除零并表示“不放大”。
- `IsAllEvicted`、`IsEvicted`、`IsStatsInitialized`、`IsEssentialStatsLoaded`、`IsFullLoad`：委托 `StatsLoadedStatus`。其定义位于 `pkg/statistics/histogram.rs`：只有已初始化状态才会被认为需要加载/已驱逐；`AllLoaded = 0`、`AllEvicted = 1`。
- `IndexStatsIsInvalid`：索引不存在、集合处于 pseudo 模式或索引总行数为零时返回 `true`；它不要求统计已经全量加载。

## 执行流程

典型索引范围估算从 `pkg/planner/cardinality/row_count_index.rs::GetRowCountByIndexRanges` 开始：

1. `HistColl::GetIdx` 取得 `Option<&Index>`，并记录本次统计使用情况。
2. 若完整索引可直接跳过估算，则使用实时行数；否则调用 `IndexStatsIsInvalid`。缺失、pseudo 或零行统计进入伪估算分支；有效统计继续进入 V1/V2 路径。
3. 点范围估算通过 `IndexInfo.Unique`、列数和 NULL 情况决定是否直接返回至多一行，或进入 `equalRowCountOnIndex`。
4. 旧版本且有 `CMSketch` 时，`equalRowCountOnIndex` 调用 `QueryBytes`；V2 路径直接优先查 `TopN`，未命中再使用直方图和 NDV 逻辑。`GetIncreaseFactor` 最后把历史统计缩放到实时行数。
5. `pkg/planner/cardinality/selectivity.rs::getEqualCondSelectivity` 同样用 `QueryBytes / TotalRowCount` 形成等值选择率，并在唯一索引、越界值和跨列校验分支中使用 `InfoRef`、`NDV` 与总行数。

`QueryBytes(data)` 自身的决策顺序是：先调用 `TopN::QueryTopN`，命中即返回精确重频值；否则若有 `CMSketch`，调用 `CMSketch::QueryBytes` 返回近似频次；两者都不存在时，把字节包装为 `types::NewBytesDatum` 并调用 `Histogram::EqualRowCount`。`pkg/statistics/histogram_test.rs::index_query_bytes_prefers_topn_then_cms` 直接验证 TopN 优先于 CMSketch，Go 的 `pkg/statistics/histogram_test.go::TestIndexQueryBytes` 还验证了无 sketch 时的直方图回退。

缓存降级流程由 `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs::retainEvictedShell` 驱动：复制表统计，筛选“已初始化且尚未全部驱逐”的索引，调用 `DropUnnecessaryData`，再按 `Table::MemoryUsage` 核算降级后的缓存成本。同步加载侧的 `TableStats::IndexIsLoadNeeded` 以 `!index.IsFullLoad()` 判断已有统计是否需要补载。

## 数据与状态

`Index` 的统计数据分为四层。`Info`、`PhysicalID`、`StatsVer` 是身份与兼容元数据；`Histogram` 持有 NDV、NULL 数、桶、上下界和 scalar 等分布；`TopN` 保存高频编码键，`CMSketch` 保存长尾近似频次，`FMSketch` 保存 NDV sketch；`StatsLoadedStatus` 则是独立的加载状态机。

关键不变量如下：

- `StatsVer >= Version2` 时，TopN 已从直方图主体分离，因此 `TotalRowCount` 必须把两者相加；旧版本只读取直方图。缺失 TopN 按零计数，使部分加载对象也可安全求和。
- `QueryBytes` 的数据是已经按索引键规则编码的字节，不在本文件中进行列值编码；planner 在 `row_count_index.rs` 中先用 `codec::EncodeKey` 生成该输入。
- `Info` 在类型上可缺失，便于测试和中间状态构造；但 `String` 及大量 planner 路径通过 `InfoRef` 要求它存在。仅 `ItemID` 对缺失元信息提供回退。
- `DropUnnecessaryData` 与 `EvictAllStats` 都是原地突变，状态标记与载荷清理必须作为同一操作完成；清理后对象仍保留可识别的统计壳。
- `MemoryUsage` 是与 Go 缓存策略对齐的“被追踪统计载荷”口径，不是 `Index` 的完整堆内存测量，也不会把 `FMSketch`、`Info` 或容器固定开销计入。

`DerefMut` 允许调用者直接修改直方图，因此 `Index` 自身无法强制直方图字段与 sketch、统计版本或加载状态始终同步；构建器、加载器和扩展代码必须共同维护这些跨字段不变量。

## 依赖与调用关系

直接 Rust 依赖均由 crate 内重导出提供：`Histogram` 与 `StatsLoadedStatus` 来自 `histogram.rs`，`CMSketch` 来自 `cmsketch.rs`，`TopN` 来自 `cmsketch.rs` 中的 TopN 实现，`FMSketch` 来自 `fmsketch.rs`，版本和驱逐常量来自 `histogram.rs`。唯一直接引用的外部 crate 是 `types`，用于在直方图回退时构造 bytes datum；`pkg/statistics/Cargo.toml` 将它绑定为本地包 `astersql-types-datum`。

主要调用边经 RustCodeGraph 文件引用和 `rg` 交叉核验如下：

- `pkg/planner/cardinality/row_count_index.rs` → `IndexStatsIsInvalid`、`InfoRef`、`TotalRowCount`、`GetIncreaseFactor`、`QueryBytes`：索引范围行数估算主链。
- `pkg/planner/cardinality/selectivity.rs` → `IndexStatsIsInvalid`、`IsFullLoad`、`InfoRef`、`QueryBytes`、`TotalRowCount`：可用统计选择与等值条件选择率。
- `pkg/planner/cardinality/cross_estimation.rs` 和 `pseudo.rs` → `IndexStatsIsInvalid`：决定是否退回伪统计。
- `pkg/statistics/table.rs` → `DropUnnecessaryData`、`IsFullLoad`、`TotalRowCount`、`MemoryUsage`：表级统计驱逐、分析行数和缓存成本汇总。
- `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs` → `DropUnnecessaryData`、加载状态、`MemoryUsage`：LFU 缓存降级。
- `pkg/statistics/handle/syncload/stats_syncload.rs` → `IsFullLoad`：索引同步加载判定。

RustCodeGraph 的 `node --file pkg/statistics/index.rs` 报告本文件被 7 个文件引用，并明确列出 planner/cardinality 与 `index_test.rs`、`histogram_test.rs` 等；当前索引未为该 Rust `impl` 的 PascalCase 方法生成可查询方法节点，精确 `callers/callees` 未返回可用边，所以上述方法级边使用这些调用点源码补齐验证。

## 错误处理与边界

本文件没有返回 `Result` 的 API，也不吞掉下游错误；所有操作均是内存内的确定性查询或突变。唯一显式失败方式是 `InfoRef` 在 `Info == None` 时 panic，`String` 会间接触发同一失败。生产构造器不得把缺少 `Info` 的 `Index` 传到这些路径。

`IndexStatsIsInvalid(None, _)` 立即返回无效；对存在的对象只检查 pseudo 和零总行数，不因驱逐或未全量加载而无效。`pkg/statistics/index_test.rs::evicted_nonempty_index_remains_valid_like_go` 明确验证“已全部驱逐但仍有非零 TopN/直方图总数”的对象保持有效。这与加载判定是两个维度：需要加载由 `IsEvicted` / `IsFullLoad` 表达，不能用有效性函数替代。

`GetIncreaseFactor` 的零统计保护返回 `1.0`。`TotalRowCount` 与 `MemoryUsage` 对缺失 sketch 使用零值。`QueryBytes` 的直方图 `f64` 结果以 `as u64` 转换，调用约定要求行数估计非负；若未来允许负数或超过 `u64` 的异常估计，应在转换处增加显式约束和回归测试。

驱逐操作不是完全对称的：`DropUnnecessaryData` 清 bounds、buckets、scalars，且仅在旧版本移除 CMS；`EvictAllStats` 清 buckets 和两个频次结构但保留 bounds/scalars。修改二者时必须先确认 Go 语义和各自调用目的，不能因名称相近而合并。

## 并发与资源生命周期

`Index` 不包含锁、原子、通道、后台任务或外部句柄。查询方法只借用 `&self`，驱逐方法要求独占 `&mut self`，线程安全边界由 Rust 借用规则及持有它的表/缓存容器负责。`Copy` 返回拥有型克隆，可供缓存写时复制流程在不修改共享原对象的前提下驱逐载荷；`lfu_cache.rs::retainEvictedShell` 正是先复制表再逐项原地清理。

资源生命周期从构造/加载开始，经 planner 只读查询，再由内存压力触发 `DropUnnecessaryData` 进入 `AllEvicted` 壳状态，最后由同步或异步统计加载层重新构造/替换载荷。本文件只设置状态和释放容器内容，不主动触发重载。`PhysicalID == -1` 在 Go 注释中表示统计不可用且不应触发加载；Rust 字段保留了该信息，但本文件没有据此分支，调度语义属于外层加载代码。

清空 `Vec` 会将逻辑长度降为零，但 Rust 是否立即归还全部容量取决于 `Vec::clear` 的实现语义；因此 `DropUnnecessaryData` 的实际容量释放效果可能不同于 Go 用新空切片/新 chunk 替换底层存储。当前缓存核算依赖各组件 `MemoryUsage` 的口径；若要保证立刻释放容量，应先测量并同步修改内存测试，而不能仅凭逻辑为空推断物理内存已释放。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/statistics/index.go`，`pkg/statistics/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/statistics"` 也声明了移植来源。Rust 保留了 Go `Index` 的主要字段与方法命名，并以 `Deref` 模拟 Go 匿名嵌入的直方图访问；Rust 的 `IndexInfo` / `IndexColumnInfo` 是本地精简模型，而 Go 使用 `model.IndexInfo`。

已经对齐的核心语义包括：V2 总行数叠加 TopN；`QueryBytes` 按 TopN → CMSketch → histogram 回退；放大系数的零值保护；驱逐后保留统计壳；`MemoryUsage` 不计 `FMSketch`。`pkg/statistics/index_test.rs` 的四个测试分别锁定字符串列数、测试驱逐边界、内存口径和“驱逐不自动无效”；`pkg/statistics/histogram_test.rs` 与 Go `histogram_test.go::TestIndexQueryBytes` 提供查询路径证据。

当前可见差异必须在扩展时保留或显式解决：

- Go `IndexStatsIsInvalid` 接收 session/context、`HistColl` 和索引 ID；遇到缺失或未全量加载统计时会向 `asyncload.AsyncLoadHistogramNeededItems` 登记加载需求，之后才判断 nil、pseudo 或零总数。Rust 签名只有 `(Option<&Index>, pseudo)`，只做纯有效性判断，没有异步加载副作用，也没有检查 restricted SQL、`CanNotTriggerLoad` 或同步加载超时。
- Go `Copy` 支持 nil receiver 并逐字段复制；Rust 无 nil receiver，直接 `Clone`。Go `ItemID` 假定 `Info` 存在，Rust 对缺失 `Info` 回退到直方图 ID。
- Go `TotalRowCount` 在 V2 路径直接解引用 `TopN`；Rust 对 `None` 按零处理，更适合部分加载状态。
- Go `MemoryUsage` 返回带分项的 `CacheItemMemoryUsage`；Rust 返回单个 `i64` 总数，分项信息不可用。
- Go `QueryBytes` 显式计算 Murmur3 哈希并允许传入 plan context 记录 debug trace；Rust 把哈希细节封装在 `CMSketch::QueryBytes`，且没有 trace context。
- Go `IsAllEvicted` 对 nil receiver 返回 true；Rust 必须先有 `Index` 值。Go 的 `IsEvicted` 是 `evictedStatus != AllLoaded`；Rust 委托 `IsLoadNeeded`，额外要求状态已初始化。

因此本文件属于功能可用但并非 Go 全签名/全副作用等价的移植。尤其不能把当前 `IndexStatsIsInvalid` 描述为已经完成异步加载调度。

## 扩展指南

新增估算字段或 sketch 时，首先修改 `Index`，再逐项审查 `Copy`（通常由 `Clone` 自动覆盖）、`DropUnnecessaryData`、`EvictAllStats`、`MemoryUsage`、总行数以及查询优先级；新字段若参与缓存成本，必须同步 `pkg/statistics/table.rs` 的汇总口径和 LFU 缓存测试。不要把 Rust 单元测试放回生产文件，应扩展同目录的 `pkg/statistics/index_test.rs`，跨直方图行为可扩展 `histogram_test.rs`。

调整统计版本语义时，应以 `Version2` 分支为兼容边界，同时核对 `pkg/planner/cardinality/row_count_index.rs` 的 V1/V2 路径、`selectivity.rs` 的分母与 TopN 使用，以及 Go `pkg/statistics/index.go`。总行数与频次查询必须维持同一数据拆分模型，否则会发生 TopN 重复计数或漏计。

若补齐 Go 的异步加载副作用，最可能修改 `IndexStatsIsInvalid` 的签名和调用点，但这会跨越本值对象与 session/asyncload 边界。应先复用现有 Rust 加载接口，覆盖缺失索引、部分加载、restricted SQL、禁止触发加载、同步加载超时以及重复登记等 Go 测试意图，而不是在 `Index` 内引入全局可变状态。

若改变 `InfoRef` 的 panic 策略，应统一审计 `String`、planner 的唯一索引/列数判断和所有测试构造器；可选元信息是当前类型事实，不能只在某个调用点静默填默认值。若改变驱逐逻辑，应分别为 `DropUnnecessaryData` 与 `EvictAllStats` 添加版本化载荷断言，并验证 `MemoryUsage` 的前后差值和重新加载判定。

性能风险主要在热路径的克隆、内存计算和等值查询。`QueryBytes` 必须维持常见高频值优先命中 TopN；`Copy` 会深拷贝所有载荷，不宜无意加入 planner 查询热路径；驱逐清理若保留容器容量，应结合真实缓存压力测试决定是否改为释放容量。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/statistics/index.rs --offset 1 --limit 500` 读取了目标文件 1–229 行，并报告 7 个引用文件，包括 `pkg/planner/cardinality/cross_estimation.rs`、`row_count_index.rs`、`selectivity.rs`、`pkg/statistics/index_test.rs` 和 `histogram_test.rs`。`query IndexStatsIsInvalid --kind function --json` 同时定位 Go 与 Rust 定义。Rust `impl` 方法节点/精确调用边查询未返回可用结果，故没有据此臆造边。
- 源码与模块边界：完整阅读 `pkg/statistics/index.rs`；读取 `pkg/statistics/lib.rs` 的 `mod index`、`pub use index::*` 和独立测试装配；读取 `pkg/statistics/Cargo.toml` 的 crate 根、本地 `types` 依赖与 Go 包映射；目标包未发现 `doc.go`。
- 上游/下游证据：检查 `pkg/planner/cardinality/row_count_index.rs`、`pkg/planner/cardinality/selectivity.rs`、`pkg/statistics/table.rs`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`、`pkg/statistics/handle/syncload/stats_syncload.rs` 的直接调用点，并读取 `pkg/statistics/histogram.rs` 中版本、驱逐常量和 `StatsLoadedStatus` 判定。
- Go 对照：完整读取 `pkg/statistics/index.go`；读取 `pkg/statistics/histogram_test.go::TestIndexQueryBytes`，并检索 Go 测试中的 `IndexStatsIsInvalid`、`TotalRowCount` 与加载行为调用点。
- Rust 测试：完整读取 `pkg/statistics/index_test.rs`；读取 `pkg/statistics/histogram_test.rs::index_query_bytes_prefers_topn_then_cms`。它们覆盖本文件最关键的列数格式化、驱逐边界、内存口径、有效性与查询优先级，但没有覆盖 `InfoRef` panic、V1/V2 `DropUnnecessaryData` 差异、零总数放大系数及纯直方图回退，属于后续可补的独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文档存在且恰有 11 个固定二级章节，并人工复核所有“已支持”和差异说明都能回溯到以上代码或测试。
