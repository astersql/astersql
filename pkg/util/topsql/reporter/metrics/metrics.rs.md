# `pkg/util/topsql/reporter/metrics/metrics.rs`

## 文件定位

本文件是独立 crate `astersql-util-topsql-reporter-metrics` 的子句柄绑定层。crate 根在同目录 `lib.rs`：其中 `metrics` 模块创建三个 Prometheus 父向量，随后以 `#[path = "metrics.rs"] pub mod reporter_metrics` 暴露本文件。`Cargo.toml` 表明该 crate 只直接依赖 `prometheus = "0.14"`，并以 `pkg/util/topsql/reporter/metrics` 为 Go 移植来源。

在进程级初始化链中，`pkg/util/metricsutil/common.rs::initMetrics` 先调用 `initParentMetricsCollectors`；后者执行 `astersql_util_topsql_reporter_metrics::metrics::init_parent_metrics()`，然后 `initMetrics` 再调用本文件的 `InitMetricsVars()`。因此本文件不创建或注册新的指标族，而是把父向量的特定标签组合缓存成业务路径可直接写入的句柄。

## 核心职责

1. 以 11 个 `prometheus::Counter` 子句柄表示 TopSQL 数据被忽略或丢弃的原因，包括 SQL/Plan/RU 上限、迟到 RU、采集/上报通道满和背压丢弃。
2. 以 10 个 `prometheus::Histogram` 子句柄表示 `all`、`record`、`sql`、`plan`、`ru_record` 五类上报在 `ok`/`error` 两种结果下的耗时。
3. 以 4 个 `prometheus::Histogram` 子句柄表示 `record`、`ru_record`、`sql`、`plan` 四类上报数据量。
4. 通过 `init()` 和 `InitMetricsVars()` 统一完成标签绑定，使调用方不必重复拼写标签，并保持 `metrics.go::InitMetricsVars` 的标签协议。

本文件只持有句柄并负责初始化，不决定何时丢弃数据、如何发送 RPC、如何计算 RU，也不负责把指标注册到 registry。

## 主要符号

- `init() -> ()`：薄入口，仅转发到 `InitMetricsVars()`；供需要显式模拟 Go 包初始化语义的 Rust 调用方和测试使用。
- `InitMetricsVars() -> ()`：唯一实质函数。在一个 `unsafe` 块内依次取得三个父向量，并用 `with_label_values` 填充全部 25 个可变静态句柄。
- `Ignore*Counter: Option<prometheus::Counter>`：11 个公开可变静态计数器。标签分别为 `ignore_exceed_sql`、`ignore_exceed_plan`、`ignore_exceed_ru_keys`、`ignore_exceed_ru_total`、`ignore_late_compacted_ru_keys`、`ignore_late_compacted_ru_total`、`ignore_collect_channel_full`、`ignore_collect_stmt_channel_full`、`ignore_collect_ru_channel_full`、`ignore_report_channel_full`、`ignore_report_data_by_backpressure`。
- `Report*Duration*Histogram: Option<prometheus::Histogram>`：10 个耗时句柄。`type` 为 `all`、`record`、`sql`、`plan` 或 `ru_record`，`result` 使用父模块常量 `metrics::LblOK`/`metrics::LblError`。
- `TopSQLReport*Histogram: Option<prometheus::Histogram>`：4 个数据量句柄，`type` 为 `record`、`ru_record`、`sql` 或 `plan`。
- 文件级 `#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]`：保留 Go 导出名，同时允许当前基于 `static mut` 的全局句柄模型。文件没有 struct、enum、trait、impl、泛型或条件编译项。

## 执行流程

1. `pkg/util/metricsutil/common.rs::initMetrics` 调用 `initParentMetricsCollectors()`。
2. `initParentMetricsCollectors()` 调用同 crate 的 `metrics::init_parent_metrics()`，创建 `TopSQLIgnoredCounter(type)`、`TopSQLReportDurationHistogram(type,result)`、`TopSQLReportDataHistogram(type)` 三个父向量。
3. `initMetrics()` 调用 `reporter_metrics::InitMetricsVars()`；测试也可先初始化父向量，再直接调用 `InitMetricsVars()` 或薄封装 `init()`。
4. `InitMetricsVars()` 对每个父向量执行 `as_ref().expect(...)`，确保初始化顺序正确；然后逐个调用 `with_label_values`，把返回的子句柄写入对应 `Option`。
5. 业务路径在事件发生时读取这些句柄并更新指标。例如 `reporter.rs::takeDataAndSendToReportChan` 在上报队列满时递增 `IgnoreReportChannelFullCounter` 与 `IgnoreReportDataByBackpressureCounter`；`ru_window_aggregator.rs::incrementLateCompactedMetrics` 累加迟到 RU；`stmtstats/aggregator.rs::record_dropped_ru` 累加超过限制的 RU key 和 RU 总量；`single_target.rs` 的 `observeAll`/`observeRecords`/`observeSQL`/`observePlans` 写入发送耗时和数量。

重复调用 `InitMetricsVars()` 会重新取得相同标签对应的子句柄并覆盖静态 `Option`，不会在本文件内清零父序列。

## 数据与状态

全部状态是进程级 `pub static mut Option<...>`：初始为 `None`，成功初始化后为 `Some(Counter/Histogram)`。子句柄与父 `CounterVec`/`HistogramVec` 的同标签时间序列共享底层指标状态；`migration_aster_unit_test.rs` 通过子句柄写入后读取父向量，验证了这种共享关系。

标签集合是对外可观测协议。改名会改变 Prometheus series；增减标签值会影响时序基数、仪表盘和告警。耗时值由调用方以秒写入，数据量直方图写入本批条数，RU 丢弃总量计数器可用 `inc_by(f64)` 累加非整数值。

当前 Rust 生产引用可确认：`IgnoreExceedRUKeysCounter`、`IgnoreExceedRUTotalCounter`、两个迟到 RU 计数器、两个上报/背压计数器，以及 `all`/`record`/`sql`/`plan` 的数据量和耗时句柄。文本搜索未发现 `IgnoreExceedSQLCounter`、`IgnoreExceedPlanCounter`、三个 collect-channel 句柄、`TopSQLReportRURecordCounterHistogram` 和两个 RU record 耗时句柄在 Rust 生产文件中的消费者；它们已绑定并有部分 crate 级测试，但不能据此声称对应 Rust 业务打点已完整接线。

## 依赖与调用关系

- 上游初始化：`pkg/util/metricsutil/common.rs::initMetrics` → `initParentMetricsCollectors` → `metrics::init_parent_metrics`，随后调用本文件 `InitMetricsVars`。
- 父级依赖：`crate::metrics::{TopSQLIgnoredCounter, TopSQLReportDurationHistogram, TopSQLReportDataHistogram, LblOK, LblError}`，定义于同 crate 的 `lib.rs`。
- 外部依赖：`prometheus::Counter`、`prometheus::Histogram` 以及父向量提供的 `with_label_values`。
- Rust 消费者：`pkg/util/topsql/reporter/reporter.rs`、`single_target.rs`、`ru_window_aggregator.rs` 和 `pkg/util/topsql/stmtstats/aggregator.rs`；`stmtstats/lib.rs` 还提供测试初始化辅助函数。
- crate 接线：`pkg/util/topsql/reporter/Cargo.toml` 以 `reporter_metrics` 别名依赖本 crate；`pkg/util/topsql/stmtstats/Cargo.toml` 以 `reporter-metrics-dependency` 别名依赖它；根 `Cargo.toml` 还提供 workspace facade 依赖。

RustCodeGraph 将目标文件列为 29 个符号，并报告它被 reporter、RU 聚合器、其测试、single-target 与 stmtstats 聚合器等文件使用。由于多个 crate 均存在同名 `metrics.rs::InitMetricsVars`，图的直接 callers 查询出现名称歧义；上述精确调用点由 `rg` 对符号引用补证。

## 错误处理与边界

`InitMetricsVars()` 不返回 `Result`。如果任一父向量仍为 `None`，对应 `expect` 会立即 panic，错误文本分别指出 `TopSQLIgnoredCounter`、`TopSQLReportDurationHistogram` 或 `TopSQLReportDataHistogram` 必须先初始化。这是初始化顺序不变量，而非可恢复的运行期错误。

`with_label_values` 的标签数量在本文件中与父向量定义一致：ignored/data 各 1 个标签，duration 2 个标签。若将父向量标签 schema 改动而不同步本文件，Prometheus 库可能在初始化时失败或 panic，因此两处必须原子更新。

消费方通常使用 `if let Some(...)`：句柄在早期启动阶段未就绪时跳过指标写入，但数据路径本身继续运行。该容错只避免观测代码阻断业务，并不补记初始化前丢失的指标。静态变量公开且可变，没有类型系统层面的“仅初始化一次”约束。

## 并发与资源生命周期

Prometheus 的克隆子句柄负责并发安全的计数/观测；本文件不创建线程、任务、通道、锁、事务或网络连接。真正需要额外约束的是 `static mut Option` 的读写：初始化和消费都在 `unsafe` 中，正确性依赖进程启动阶段先完成单线程式初始化、之后只读取句柄这一约定。

生产初始化路径没有在本文件中用 `Once` 防止并发重绑。crate 级测试使用 `migration_aster_unit_test.rs::TEST_LOCK` 串行化三个用例，防止测试并行重置全局父向量和子句柄。句柄生命周期为进程全程；文件没有显式释放或注销逻辑。

## 与 Go 版本的对应关系

直接对照文件为同目录 `metrics.go`。两版都提供 25 个同名全局句柄、`init` 和 `InitMetricsVars`，所有 ignored/type/result 标签字符串逐项一致，绑定顺序也一致。

主要差异如下：

- Go 全局变量类型为非可空 `prometheus.Counter`/`prometheus.Observer`，包 `init()` 自动执行；Rust 使用 `Option<Counter/Histogram>`，并由 `metricsutil` 显式维持“先父向量、后子句柄”的初始化顺序。
- Go 的 `Observer` 是接口；Rust 保存具体 `Histogram`，因此这些句柄只表达当前直方图实现。
- Go 消费方通常直接 `Inc`/`Add`/`Observe`；Rust 消费方先判断 `Option`，允许初始化前跳过观测。
- Go 的 `pubsub.go` 对 `ru_record` 数量和耗时有完整打点；当前 Rust 搜索只确认这些句柄的绑定和测试引用，未确认对应生产消费者。类似地，Go 的 datamodel/reporter 路径消费部分 SQL/Plan/collect-channel 计数器，而当前 Rust 生产引用不完整。

Go 行为测试证据包括：`reporter_test.go` 验证 report-channel/backpressure 丢弃计数，`ru_window_aggregator_test.go` 验证迟到 RU 计数，`stmtstats/aggregator_test.go` 验证超限 RU 计数，`single_target_test.go` 验证整批成功/失败耗时分流。

## 扩展指南

新增一种 ignored 原因时，应同时修改同 crate `lib.rs` 的父向量约束（若标签维度变化）、本文件的静态句柄与 `InitMetricsVars` 标签绑定、直接业务消费点，以及独立的 `migration_aster_unit_test.rs`；还应核对 Go `metrics.go` 和实际 Go 消费点，避免只增加一个永远不写入的句柄。

新增上报对象类型时，通常需要成对增加成功/失败耗时句柄，并视需要增加数据量句柄；标签字符串必须与 exporter 查询、仪表盘和 Go 语义保持兼容。高基数动态值不得直接成为这里的标签值。

测试必须继续放在独立 Rust 测试文件中，不应内嵌到 `metrics.rs`。至少验证：每个新标签能成功绑定、子句柄写入反映到父向量、成功/失败映射正确、并发测试用全局互斥锁隔离。当前测试的 ignored cases 没有覆盖 `IgnoreReportDataByBackpressureCounter`，后续修改该组绑定时应补齐这一直接回归项；生产接线缺口应在其所属业务迁移任务中处理，而不是在本说明任务中改行为。

兼容风险主要是指标名/标签变化导致历史时序断裂；正确性风险是父子初始化顺序或 `static mut` 并发访问；性能风险是新增高基数标签和热路径额外观测。若未来重构全局状态，优先考虑一次初始化容器和安全共享句柄，但需保持现有指标名、标签及初始化入口兼容。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标目录的 `lib.rs`、`metrics.rs`、`metrics.go`、`migration_aster_unit_test.rs`；目标文件节点列出 29 个符号和 5 个使用文件。`node --file` 核对了目标源码、crate 根、Rust 独立测试及主要消费函数。
- 目标实现：`pkg/util/topsql/reporter/metrics/metrics.rs`，重点为 25 个静态句柄、`init`、`InitMetricsVars` 及三组 `with_label_values`。
- crate/初始化：`pkg/util/topsql/reporter/metrics/Cargo.toml`、同目录 `lib.rs`、`pkg/util/metricsutil/common.rs::{initParentMetricsCollectors, initMetrics}`、根 `Cargo.toml` 及 reporter/stmtstats 的 Cargo manifest。
- Rust 调用点：`pkg/util/topsql/reporter/reporter.rs::takeDataAndSendToReportChan`、`single_target.rs::{incrementIgnoreReportChannelFull, observeAll, observeRecords, observeSQL, observePlans}`、`ru_window_aggregator.rs::incrementLateCompactedMetrics`、`pkg/util/topsql/stmtstats/aggregator.rs::record_dropped_ru`。
- Rust 测试：`pkg/util/topsql/reporter/metrics/migration_aster_unit_test.rs`；相关业务测试还包括 `ru_window_aggregator_test.rs` 和 stmtstats 聚合器测试。crate 级测试覆盖 ignored（除背压句柄）、全部 duration 标签和四类 data 标签与父序列共享。
- Go 对照：`pkg/util/topsql/reporter/metrics/metrics.go`；相关行为测试为 `reporter_test.go`、`ru_window_aggregator_test.go`、`single_target_test.go`、`pkg/util/topsql/stmtstats/aggregator_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令和人工事实复核验收。
