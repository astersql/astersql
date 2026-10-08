# `pkg/statistics/handle/metrics/metrics.rs` 逻辑说明

## 文件定位

该文件是独立 crate `astersql-statistics-handle-metrics` 的实现文件；crate 边界由 `pkg/statistics/handle/metrics/Cargo.toml` 定义，入口 `pkg/statistics/handle/metrics/lib.rs` 通过 `#[path = "metrics.rs"] mod implementation` 装载并用 `pub use implementation::*` 对外再导出全部公开符号。它只直接依赖 `prometheus = "0.14"`，不持有统计表或会话对象，而是为 statistics handle 子系统提供进程级指标句柄和与 Go 一致的健康度分桶协议。

应用侧的显式初始化链为 `pkg/util/metricsutil/common.rs::initMetrics` → `astersql_statistics_handle_metrics::InitMetricsVars`。健康度指标的 Rust 生产消费方位于 `pkg/statistics/handle/cache/statscache.rs::UpdateStatsHealthyMetrics`；历史统计 dump 计数器目前只有本 crate 的迁移测试直接读写，仓库 Rust 生产代码中未检索到递增调用。因而本文件既是已接线的健康度指标契约，也是尚待 storage Rust 路径消费的历史统计指标接口。

## 核心职责

本文件承担三项职责。

1. 用 `StatsHealthyBucket0To50` 至 `StatsHealthyBucketPseudo`、`StatsHealthyBucketCount` 和 `HEALTHY_BUCKET_CONFIGS` 固化健康度桶的下标、排他上界及 Prometheus `type` 标签。顺序是 ABI 式的跨模块约定：`statscache.rs` 以这些下标累计数组并按同序写入 Gauge。
2. 构造 `tidb_statistics_stats_healthy{type=...}` 的十个 Gauge 子句柄，以及 `tidb_statistics_historical_stats{type="dump",result=...}` 的成功/失败 Counter 子句柄。
3. 通过 `InitMetricsVars` 提供可重复调用的显式初始化入口，配合 `std::sync::LazyLock` 保证全局对象只初始化一次。

本文件不计算表健康度、不遍历统计缓存、不执行历史统计导出，也不包含 Prometheus registry 的注册调用。健康度分类和更新由 `pkg/statistics/handle/cache/statscache.rs` 完成；Go 的历史导出计数发生在 `pkg/statistics/handle/storage/stats_read_writer.go::DumpHistoricalStatsBySnapshot`，对应 Rust 生产接线当前未找到。

## 主要符号

- `StatsHealthyBucket0To50` … `StatsHealthyBucket100To100`：普通健康度桶下标 0–6。上界采用排他语义，因此满分桶配置为 `upper_bound = 101`，使合法值 100 能落入 `[100,100]`。
- `StatsHealthyBucketTotal`、`StatsHealthyBucketUnneededAnalyze`、`StatsHealthyBucketPseudo`：下标 7–9 的特殊桶；其 `upper_bound` 为 0，不参与数值区间查找。
- `StatsHealthyBucketCount`：固定为 10，是配置表长度和调用方定长数组的共同约束。
- `HealthyBucketConfig { index, upper_bound, label }`：可复制的只读配置记录。字段均公开，供 cache crate 的分类函数读取。
- `HEALTHY_BUCKET_CONFIGS`：十项静态切片，顺序必须与所有桶常量一致。标签 `[0,100]` 为兼容旧版本而保留，并非普通闭区间分类桶。
- 私有 `StatsHealthyGauge: LazyLock<GaugeVec>`：创建名称为 `tidb_statistics_stats_healthy`、标签维度为 `type` 的父向量。
- 私有 `HistoricalStatsCounter: LazyLock<CounterVec>`：创建名称为 `tidb_statistics_historical_stats`、标签维度为 `type`/`result` 的父向量。
- `StatsHealthyGauges: LazyLock<Vec<Gauge>>`：按配置顺序绑定十个 `type` 标签后的公开 Gauge 句柄数组；初始化时断言配置数量等于 `StatsHealthyBucketCount`。
- `DumpHistoricalStatsSuccessCounter` / `DumpHistoricalStatsFailedCounter`：分别绑定 `("dump", "success")` 和 `("dump", "fail")` 的公开 Counter。
- `init()`：普通公开 Rust 函数，不是 Rust 运行时自动执行的特殊钩子；函数体只转调 `InitMetricsVars`。
- `InitMetricsVars()`：显式 force 三个公开 `LazyLock`，由 metricsutil 的总初始化链调用。

文件没有 trait、impl、枚举、条件编译项或可返回错误的函数。

## 执行流程

初始化主流程如下。

1. `pkg/util/metricsutil/common.rs::initMetrics` 在父 collector 初始化之后调用 `statshandler_metrics::InitMetricsVars()`。
2. `InitMetricsVars` 首先 force `StatsHealthyGauges`。其闭包检查 `HEALTHY_BUCKET_CONFIGS.len() == StatsHealthyBucketCount`，然后逐项调用私有 `StatsHealthyGauge.with_label_values(&[cfg.label])`，保持配置顺序收集十个句柄。
3. 随后 force 两个历史统计 Counter，分别从 `HistoricalStatsCounter` 绑定 dump 成功与失败标签。之后重复调用 `InitMetricsVars` 不会重建或清零对象。

健康度采集流程在直接下游 `StatsCacheImpl::UpdateStatsHealthyMetrics` 中完成：为十个桶建立 `[i64; StatsHealthyBucketCount]`，每张表先增加 total；pseudo 表和无需 ANALYZE 的表进入特殊桶并提前继续；其余表取得健康度后由 `statsHealthyBucketIndex` 扫描 `HEALTHY_BUCKET_CONFIGS` 中 `upper_bound > 0` 的项，按第一个满足 `healthy < upper_bound` 的配置分类；最终按下标把累计值写入 `StatsHealthyGauges`。

历史 dump 的预期 Go 流程是 `DumpHistoricalStatsBySnapshot` 用 defer 检查最终 `err`，成功递增 `DumpHistoricalStatsSuccessCounter`，失败递增 `DumpHistoricalStatsFailedCounter`。Rust 搜索只确认句柄定义、初始化及测试读写，未确认生产 dump 路径已有同等调用。

## 数据与状态

所有状态都是进程级、常驻的 Prometheus metric 对象。`HEALTHY_BUCKET_CONFIGS` 是编译期静态只读切片；两个父向量与三个公开句柄集合由 `LazyLock` 管理。`StatsHealthyGauges` 内 Gauge 的值可被反复 `set`，表达最近一次缓存扫描得到的瞬时分布；total 同时计数所有表，而 pseudo/unneeded analyze 还会进入各自特殊桶，因此这些特殊桶不是互斥地分割 total。

Counter 只能单调增加，表达进程生命周期内的历史 dump 结果次数。`InitMetricsVars` 只确保句柄存在，不重置 Gauge 或 Counter。`migration_aster_unit_test.rs::repeated_init_reuses_go_global_metric_handles` 先把 pseudo Gauge 设为 17，再次初始化后仍读到 17，直接验证了这一不变量。

配置顺序具有数据协议意义：下游以 `buckets[idx]` 与 `StatsHealthyGauges[idx]` 对齐，并用 `HEALTHY_BUCKET_CONFIGS[idx]` 取得标签；新增或重排桶时不能只改标签表中的一处。

## 依赖与调用关系

- crate 装配：`pkg/statistics/handle/metrics/lib.rs` 私有装载实现并公开再导出；同文件仅在 `cfg(test)` 下装载 `migration_aster_unit_test.rs`。
- 外部库：`prometheus::{Gauge, GaugeVec, Counter, CounterVec, Opts}` 提供描述符、标签子句柄和数值操作；`std::sync::LazyLock` 提供一次初始化及线程安全共享。
- 上游初始化：`pkg/util/metricsutil/common.rs::initMetrics` 在固定的子系统顺序中调用 `InitMetricsVars`。该 util crate 的 Cargo manifest 显式依赖本 crate。
- 健康度生产调用边：`pkg/statistics/handle/cache/statscache.rs::UpdateStatsHealthyMetrics` 读取桶常量、配置表和 Gauge；cache crate 的 Cargo manifest 通过 `path = "../metrics"` 依赖本 crate。
- 测试调用边：`pkg/statistics/handle/metrics/migration_aster_unit_test.rs` 验证配置、初始化和复用；`pkg/statistics/handle/cache/statscache_test.rs` 验证真实分类、边界和 Gauge 更新。
- 历史统计关系：storage crate 的 Cargo manifest 在 `cfg(any())`（恒假配置）依赖组中列出本 crate，但当前 Rust 生产源码未引用两个 dump Counter。Go 的 `pkg/statistics/handle/storage/stats_read_writer.go` 是已实现行为的直接对照。

RustCodeGraph 对目标文件报告 4 个索引符号和一个使用文件（同目录迁移测试）；精确 `callers`/`callees` 未返回变量级跨 crate 边，因此上述公开句柄消费关系由精确 `rg` 和对应 Cargo manifest 补证。

## 错误处理与边界

本文件没有 `Result` 返回路径。可见失败均发生在初始化阶段：

- `GaugeVec::new` 或 `CounterVec::new` 若描述符非法，会由 `.expect(...)` panic。指标名、帮助文本和标签集合均为编译期常量，正常情况下不会触发。
- `StatsHealthyGauges` 初始化时若配置项数不是 10，会由 `assert_eq!` panic，避免生成与调用方数组错位的句柄集合。
- `with_label_values` 传入的标签数量由本文件固定为一项或两项，与父向量定义一致；不存在运行时外部输入。

健康度数值边界由下游而非本文件检查：`statsHealthyBucketIndex` 仅用 `debug_assert!` 声明合法范围 `0..=100`，release 构建中越界正数仍按顺序分类，超过所有上界时回退到满分桶。文档消费者不应把 `upper_bound <= 0` 的特殊配置当成数值桶。标签和值是 Prometheus 对外契约，改名会造成时序断裂或监控查询不兼容。

另一个重要边界是注册所有权：本文件创建私有父向量，但没有调用 registry 的 `register`/`register_*`。仓库另有 `pkg/metrics/stats.rs` 中的同名中心 collector 及 `pkg/metrics/metrics.rs` 注册链；当前文件的句柄并非从该中心 collector 获取。仅凭本文件可以确认句柄可被直接读写，不能据此声称这些私有 collector 已被默认 Prometheus registry 暴露。

## 并发与资源生命周期

`LazyLock` 保证每个父向量和句柄集合至多初始化一次，多个线程同时首次访问时由标准库同步；成功初始化后对象存活至进程结束，没有显式 drop、关闭或注销流程。Prometheus `Gauge`/`Counter` 句柄可克隆并支持并发数值操作，本文件不额外加锁。

初始化顺序依赖 `LazyLock::force`，但不存在循环：公开 Gauge 列表只依赖私有 GaugeVec，公开 Counter 各自只依赖私有 CounterVec。重复 `InitMetricsVars` 是幂等的句柄初始化，不是指标值幂等操作。

测试侧 `migration_aster_unit_test.rs` 用 `METRICS_TEST_LOCK: Mutex<()>` 串行化会修改全局指标值的用例；cache 测试用 `HealthyGaugeGuard` 的 `Drop` 在作用域结束时归零 Gauge，避免全局状态污染后续测试。这些锁和守卫仅属于测试隔离，不是生产调用的前置要求。

## 与 Go 版本的对应关系

Rust 的桶常量、`HealthyBucketConfig` 三字段、十项顺序、排他上界和标签逐项对应 `pkg/statistics/handle/metrics/metrics.go`。特别是：100 分桶用上界 101；total 保留历史标签 `[0,100]`；三个特殊项的 Go 零值 `UpperBound` 在 Rust 中显式写为 0。

生命周期实现有所不同。Go 包级 `init()` 会自动执行并由 `InitMetricsVars` 每次重建公开 slice、从 `pkg/metrics` 的共享 `StatsHealthyGauge` 与 `HistoricalStatsCounter` 取子句柄；Rust 的 `init()` 只是可调用函数，实际由 metricsutil 显式调用，且使用 `LazyLock` 复用首次创建的句柄。

collector 所有权也不完全相同：Go 直接绑定 `pkg/metrics` 已注册的共享父向量；Rust 本文件自行创建同名私有父向量。仓库搜索没有发现本文件对其执行注册。健康度的分类与写入已有 Rust 对应实现和边界测试；历史 dump 成功/失败递增只在 Go storage 路径和 Rust metrics 单测中得到证据，不能宣称 Rust storage 生产路径已完全对齐。

## 扩展指南

新增或修改健康度桶时，应把它视为跨 crate 协议变更：同步调整桶常量、`StatsHealthyBucketCount`、`HEALTHY_BUCKET_CONFIGS`、`pkg/statistics/handle/cache/statscache.rs::statsHealthyBucketIndex` 的语义，以及 `metrics/migration_aster_unit_test.rs` 和 `cache/statscache_test.rs` 的期望。保持普通桶按排他上界升序排列，特殊桶的 `upper_bound <= 0`，并评估 Prometheus 标签兼容性和现有 dashboard/query 的时序连续性。增加桶还会增加常驻 time series，需评估基数和采集成本。

新增历史统计操作或结果标签时，应先确认父 `CounterVec` 的标签维度仍是 `type`/`result`，再新增命名明确的 `LazyLock<Counter>` 子句柄和测试；不要在热路径重复创建动态标签。若补齐 Rust dump 生产接线，最可能修改 storage 的实际启用实现，在成功/错误最终确定的位置各递增一次，并增加能覆盖两条结果路径的独立 storage 测试。

若目标是让指标被抓取，必须先解决 collector 所有权：复用 `pkg/metrics/stats.rs` 中已注册的共享 collector，或明确为本地 collector 增加唯一且只执行一次的注册路径；不能同时注册两个同名 descriptor。该改动涉及 metricsutil 初始化顺序、重复注册风险和兼容性，应同步添加 registry gather 测试，而不仅验证句柄 `.get()`。

## 验证依据

- RustCodeGraph：`status` 显示索引可用（11,467 文件、307,296 节点）；`files --filter pkg/statistics/handle/metrics` 确认目标、入口、Go 对照和迁移测试；`node --file` 阅读了 `metrics.rs` 全 171 行及 `migration_aster_unit_test.rs` 全 82 行；目标文件被索引为 4 个符号，精确 callers/callees 无跨文件输出。
- crate 与入口：`pkg/statistics/handle/metrics/Cargo.toml`、`pkg/statistics/handle/metrics/lib.rs`。
- Rust 初始化上游：`pkg/util/metricsutil/common.rs::initMetrics` 及 `pkg/util/metricsutil/Cargo.toml`。
- Rust 健康度下游与行为测试：`pkg/statistics/handle/cache/statscache.rs::UpdateStatsHealthyMetrics`、`statsHealthyBucketIndex`，以及 `pkg/statistics/handle/cache/statscache_test.rs::stats_healthy_bucket_index_matches_go_bounds` 和相邻 Gauge 分布断言。
- 本 crate 独立测试：`pkg/statistics/handle/metrics/migration_aster_unit_test.rs::{healthy_bucket_configs_match_go_order_bounds_and_labels, init_binds_each_gauge_and_historical_counter_labels, repeated_init_reuses_go_global_metric_handles}`。
- Go 语义对照：`pkg/statistics/handle/metrics/metrics.go`、`pkg/statistics/handle/cache/statscache.go`、`pkg/statistics/handle/cache/statscache_test.go`、`pkg/statistics/handle/storage/stats_read_writer.go::DumpHistoricalStatsBySnapshot`。
- 注册边界补证：全仓精确检索 `tidb_statistics_stats_healthy`、`tidb_statistics_historical_stats`、`StatsHealthyGauge`、`HistoricalStatsCounter`，对照 `pkg/metrics/stats.rs` 与 `pkg/metrics/metrics.rs`；本文件及其调用链中未发现私有父向量注册语句。
- 未运行 Cargo 或代码测试：任务是纯文档分析，计划明确排除 Cargo；完成依据是源码/调用边事实复核与任务规定的 11 章节结构验证。
