# `pkg/statistics/handle/autoanalyze/priorityqueue/analysis_job_factory.rs`

## 文件定位

本文件属于 Cargo crate `astersql-statistics-handle-autoanalyze-priorityqueue`，由同目录 `lib.rs` 以私有模块 `analysis_job_factory` 装配并通过 `pub use analysis_job_factory::*` 重导出。它是 Go 文件 `analysis_job_factory.go` 的 Rust 移植：把会话要求、自动分析阈值、当前 TSO、表/分区统计和索引元信息转换成三种实现 `AnalysisJob` trait 的候选作业，并提供分区统计筛选和每日执行时间窗口判断。

当前接线状态必须与设计职责区分：RustCodeGraph 对 `NewAnalysisJobFactory` 和三个 `Create*AnalysisJob` 的调用者查询只找到本文件自身及 `analysis_job_factory_test.rs`；同 crate 的 Rust `queue.rs` 没有直接引用这些符号。因此这些 API 在 Rust 侧已有可测试实现和 crate 级重导出，但尚未像 Go `queue.go` 那样接入生产队列构建链。

## 核心职责

- `AnalysisJobFactory` 统一保存 `SessionContext.analyze_version`、`auto_analyze_ratio` 和 `current_ts`，避免三类作业各自重复阈值、版本和时间计算。
- `CreateNonPartitionedTableAnalysisJob`、`CreateStaticPartitionAnalysisJob`、`CreateDynamicPartitionedTableAnalysisJob` 先做资格与必要性判断，再分别构造非分区、静态分区和动态分区作业；返回 `None` 表示不应入队。
- `CalculateChangePercentage`、`CalculateTableSize`、`GetTableLastAnalyzeDuration` 和 `CalculateIndicatorsForPartitions` 生成后续优先级计算所需指标。
- `CheckIndexesNeedAnalyze` 与 `CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable` 找出新增但缺统计的索引，使变化率分析被关闭时仍可因缺索引统计创建作业。
- `GetPartitionStats` 通过 `PartitionStatsProvider` 隔离统计存储访问，只保留已找到且具备自动分析资格的物理分区统计。
- `AutoAnalysisTimeWindow::IsWithinTimeWindow` 实现普通和跨午夜的日内窗口判断。

## 主要符号

- 常量 `UNANALYZED_TABLE_DEFAULT_CHANGE_PERCENTAGE = 1.0` 和 `UNANALYZED_TABLE_DEFAULT_LAST_UPDATE_DURATION = -30 分钟`：给从未分析过的对象提供优先级指标。
- 输入模型 `SessionContext`、`IndexInfo`、`TableInfo`、`TableStats`、`PartitionDefinition`：是对 Go 会话、model 与 statistics 类型的局部移植模型。`TableStats` 同时记录实时/修改/分析行数、列数、版本、资格、伪统计、是否已分析，以及已有统计和已分析 ID 集合。
- `PartitionIDAndName`：以名称和物理 ID 标识分区；`Hash`/`Eq` 允许其作为 `HashMap` 键。构造函数为 `NewPartitionIDAndName`。
- trait `PartitionStatsProvider::get_non_pseudo_physical_table_stats`：按物理表 ID 返回可选的非伪统计快照，是 `GetPartitionStats` 的唯一外部读取边界。
- `AnalysisJobFactory` 与 `NewAnalysisJobFactory`：工厂状态及构造入口。方法采用共享借用 `&self`，不在创建过程中修改工厂。
- 三个 `Create*AnalysisJob`：返回 `Option<Box<dyn AnalysisJob>>`，成功时下游分别是 `NewNonPartitionedTableAnalysisJob`、`NewStaticPartitionTableAnalysisJob` 和 `NewDynamicPartitionedTableAnalysisJob`。
- 版本方法 `AnalyzeVersionMatches`、`PartitionedTableAnalyzeVersionMatches`：判断作业是否需要设置 `NeedVersionRewriteWarn`；伪统计、版本 0 或请求版本相同都视为匹配。
- 指标与筛选方法 `CalculateChangePercentage`、`CalculateTableSize`、`GetTableLastAnalyzeDuration`、`FindLastAnalyzeTime`、`CheckIndexesNeedAnalyze`、`CalculateIndicatorsForPartitions`、`CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable`。
- `AutoAnalysisTimeWindow`、`NewAutoAnalysisTimeWindow`、`IsWithinTimeWindow`：按 UTC 小时和分钟判断窗口；内部 `oracle_time` 把 TSO 高位物理毫秒转换为 `SystemTime`。

## 执行流程

非分区表和静态分区的流程相同：先把缺失统计 `None` 或 `eligible == false` 拒绝；读取会话请求版本；计算版本是否匹配、变化率和缺统计索引；若变化率为零且索引集合为空则返回 `None`；否则计算规模和距上次分析时长，并把请求版本、是否需要版本重写警告及全部指标传给对应作业构造器。静态分区额外传递全局表 ID 和物理分区 ID。

动态分区流程先要求全局统计存在且合格，然后要求全局及所有给定分区的统计版本均匹配。`CalculateIndicatorsForPartitions` 只纳入变化率非零的分区，汇总其变化率、以“分区实时行数 × 全局列数”估算的规模、距上次分析时长和分区 ID，最后按入选分区数求三个平均值。独立地，`CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable` 为每个合格索引收集缺统计的分区 ID。只要“变化率入选分区”或“缺统计索引”任一非空，就创建动态分区作业。

`GetPartitionStats` 遍历 schema 分区定义，经 provider 按 ID 拉取统计，忽略不存在或不合格的结果，并以 `PartitionIDAndName` 建表。时间窗口则先拒绝 `UNIX_EPOCH` 哨兵；把三个时间都折算成 UTC 日内分钟，普通窗口使用闭区间 `start <= current <= end`，跨午夜窗口使用 `current >= start || current <= end`。

## 数据与状态

工厂自身只有三个只读字段，不缓存表统计，也不拥有队列。输入 `TableStats` 是调用时快照；输出作业持有由构造器复制进去的 ID、索引集合、版本和指标。`HashMap<i64, ()>` 用作 Go `map[int64]struct{}` 的集合等价物；动态分区索引结果为“索引 ID -> 缺统计分区 ID 列表”。遍历 `HashMap` 不保证顺序，因此这些列表和分区集合不应被消费者当作稳定排序结果。

未分析表的变化率固定为 1.0，最后分析时间按“当前 TSO 的物理时间减 30 分钟”构造。已分析表变化率优先以 `analyze_row_count` 为分母，否则用 `realtime_count`；只有严格大于阈值才保留，比阈值相等时返回 0。阈值为 0 明确关闭变化率触发，但不关闭缺索引统计触发。

## 依赖与调用关系

直接 Rust 下游依赖仅来自同 crate：`job::{AnalysisDuration, AnalysisJob}` 规定返回接口和有符号时长；三个具体作业模块提供构造器。标准库提供 `HashMap`/`HashSet` 和 `SystemTime`/`Duration`。同目录 `Cargo.toml` 唯一声明的 crate 依赖是 `astersql-statistics-handle-logutil`，本文件本身未直接使用外部 crate。

RustCodeGraph 核对的内部调用边包括：动态创建入口调用 `CalculateIndicatorsForPartitions`、`CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable` 和 `NewDynamicPartitionedTableAnalysisJob`；分区指标计算调用 `CalculateChangePercentage` 与 `GetTableLastAnalyzeDuration`；后者调用 `FindLastAnalyzeTime`；`GetPartitionStats` 调用 `NewPartitionIDAndName`。Rust 上游目前只发现 `analysis_job_factory_test.rs`。相对地，Go `queue.go` 在全量扫描、`tryCreateJob`、`tryUpdateJob` 和作业重建路径中调用 Go 工厂及 `GetPartitionStats`，这是未来核对 Rust 生产接线时的直接对照。

## 错误处理与边界

本文件不返回 `Result`：预期的“不存在、不合格或无需分析”使用 `Option::None` 或空集合表达。`CalculateTableSize` 要求 `column_count != 0`；动态分区指标要求全局 `column_count != 0`，违反时 `assert!` panic。`AnalyzeVersionMatches` 用 `debug_assert_eq!` 声明当前请求版本应为 2，该检查仅 debug 构建生效。

已分析但行数分母为零时，浮点除法保留 Go 对齐行为：正修改数产生正无穷大，测试明确覆盖。TSO 转换丢弃低 18 位逻辑计数，仅使用物理毫秒。未来分析时间会产生负 `AnalysisDuration`；单次时差先夹紧到 `i64` 纳秒范围。分区时长求和刻意使用 `wrapping_add`，再做整数平均，以匹配 Go `time.Duration` 溢出语义。时间窗口忽略日期和秒，边界包含起止分钟；早于 Unix epoch 的输入在折算时回退为零时长，这是 Rust `SystemTime` 适配边界。

索引筛选只接受 public、非列存索引；分区版本还排除 `is_special_global`。已有 `index_stats` 或 `analyzed_ids` 任一命中即视为不缺统计。非分区未分析表会整表分析，因此不单列索引。

## 并发与资源生命周期

`AnalysisJobFactory` 没有锁、原子量、通道、后台任务或析构逻辑；所有计算都基于调用者提供的不可变引用和工厂的只读标量。它不保证跨多个统计快照的事务一致性，调用者必须提供属于同一决策时点的 schema 与统计数据。`PartitionStatsProvider` 返回拥有所有权的 `TableStats`，因此 `GetPartitionStats` 结束后不保留 provider 借用。

Go 源码显式注明工厂非线程安全；Rust 类型当前字段本身可共享读取，但文档不能据此宣称生产并发接线安全，因为 Rust 生产队列尚未调用该工厂，而且 provider 的线程安全约束未由 trait 指定。未来接线时应在队列锁之外准备稳定快照，并只把已构造作业推入受保护队列，避免锁内访问统计存储。

## 与 Go 版本的对应关系

三类创建流程、未分析默认值、严格阈值比较、分析行数优先分母、索引过滤、动态分区平均指标、物理分区统计过滤和跨午夜时间窗口均逐项对应 `analysis_job_factory.go`。Rust 用局部 DTO 替代 Go 的 `sessionctx.Context`、`model.TableInfo` 和 `statistics.Table`，用 `PartitionStatsProvider` 替代 `statstypes.StatsHandle`，并用 `Box<dyn AnalysisJob>` 替代 Go 接口值。

已知适配差异包括：Rust 以 `UNIX_EPOCH` 表示 Go `time.Time{}` 零值；`AnalysisDuration` 保留 Go 有符号和溢出行为；Rust 的版本匹配直接检查 `pseudo` 与 `stats_version`，对应 Go `statistics.AnalyzeVersionMatchesForTableStats`；Rust 的 `is_special_global` 已由输入模型预计算，对应 Go `util.IsSpecialGlobalIndex`。最重要的迁移状态差异是 Rust 生产 `queue.rs` 尚未复用本工厂，而 Go `queue.go` 已在扫描和增量路径接线；扩展时不能假定两侧生产调用覆盖相同。

## 扩展指南

新增作业判定字段时，先扩展对应输入 DTO，再在三类 `Create*` 中决定该字段是资格门槛、入队原因还是仅作业元数据；同步修改独立的 `analysis_job_factory_test.rs`，不要把测试写进本源文件。新增索引过滤条件应同时审查 `CheckIndexesNeedAnalyze` 和 `CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable`，并与 Go `model.IndexInfo`/`util.IsSpecialGlobalIndex` 语义核对，避免非分区与分区行为漂移。

修改指标算法时还需检查 `calculator.rs` 和三个具体作业构造器的消费假设。特别注意阈值是严格大于、动态指标只对入选分区取平均、规模使用全局列数，以及有符号时长的饱和与求和溢出语义；这些会直接影响优先级排序和 Go/Rust兼容性。要完成生产迁移，应在 Rust `queue.rs` 的全量构建、DML 增量、重建/重试路径分别接入并增加队列级独立回归测试，而不是仅增加工厂单测。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`node --file ...analysis_job_factory.rs` 阅读了完整 470 行；`query AnalysisJobFactory`、`query NewAnalysisJobFactory` 与精确 `explore` 核对了符号、内部调用链及 Rust/Go 上游差异。
- Rust 源与模块边界：`analysis_job_factory.rs`、`lib.rs`、`Cargo.toml`；具体作业构造器位于 `non_partitioned_table_analysis_job.rs`、`static_partitioned_table_analysis_job.rs`、`dynamic_partitioned_table_analysis_job.rs`，公共 trait 和时长位于 `job.rs`。
- Go 对照：`analysis_job_factory.go`；生产调用位置由 `queue.go` 中 `NewAnalysisJobFactory`、三个 `Create*AnalysisJob` 和 `GetPartitionStats` 的引用核对。
- 独立 Rust 测试：`analysis_job_factory_test.rs` 覆盖阈值上下界与零分母、分析版本、三类具体作业字段、索引过滤、分区平均、provider 过滤、普通/跨午夜/分钟精度窗口、负时长和 Go 式溢出。Go 对照测试 `analysis_job_factory_test.go` 覆盖对应主要场景。
- 本任务为纯文档分析，按计划未运行 Cargo；交付结构校验要求目标文件存在且恰有本文这 11 个固定二级标题。
