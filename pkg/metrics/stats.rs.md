# `pkg/metrics/stats.rs`

## 文件定位

本文件属于 `astersql-metrics` crate（`pkg/metrics/Cargo.toml`），由 `pkg/metrics/lib.rs` 通过 `pub mod stats` 公开。它是 statistics 与 Plan Replayer 相关 Prometheus collector 的定义和初始化层，不负责统计信息计算、ANALYZE 调度、缓存更新或 Plan Replayer 任务执行。

正常启动链中，`pkg/metrics/metrics.rs::InitMetrics` 在 `INIT_METRICS_ONCE` 保护的初始化序列里调用 `stats::InitStatsMetrics`；随后 `pkg/metrics/metrics.rs::RegisterMetrics` 把本文件的 19 个 collector 注册到默认 Prometheus registry。业务侧只有两个本文件封装的写入口：`pkg/session/runtime/statistics.rs::execute_analyze` 调用 `IncManualAnalyzeCounter`，`pkg/domain/domain.rs` 的自动 ANALYZE 路径调用 `IncAutoAnalyzeCounter`。其余公开 collector 由注册中枢或其他模块直接读取、派生标签句柄。

## 核心职责

1. 声明并初始化 19 个全局 collector，覆盖自动/手动 ANALYZE、统计准确度与伪估计、同步加载、统计缓存、健康度、后台统计作业、历史统计和 Plan Replayer。
2. 在 `InitStatsMetrics` 中保持 Go 版本的 namespace、subsystem、metric name、help、标签顺序和 histogram 桶边界，维持监控时序兼容性。
3. 用 `IncManualAnalyzeCounter(result)` 和 `IncAutoAnalyzeCounter(result)` 封装成功/失败计数；collector 尚未初始化时安全地不执行更新。
4. 向下游提供共享向量：统计缓存模块从 `StatsCacheCounter`/`StatsCacheGauge` 派生固定标签句柄，Domain metrics 从 `HistoricalStatsCounter`、`PlanReplayerTaskCounter` 和 `PlanReplayerRegisterTaskGauge` 绑定业务句柄。

本文件只创建 collector 元数据和句柄。是否产生观测值由调用者决定，是否暴露给抓取端则取决于后续 `RegisterMetrics` 是否成功。

## 主要符号

- `InitStatsMetrics()`: `unsafe` 初始化入口，依次创建并替换全部 19 个 `pub static mut Option<...>`。它本身没有一次性保护，也不返回结果。
- `IncManualAnalyzeCounter(result: &str)`: 若 `ManualAnalyzeCounter` 已初始化，则使用唯一的 `type` 标签写入 `result` 对应时序并加一。
- `IncAutoAnalyzeCounter(result: &str)`: 与手动入口相同，但更新 `AutoAnalyzeCounter`。当前调用点传入 `"succ"` 或 `"failed"`。
- ANALYZE 与估计指标：`AutoAnalyzeHistogram`、`AutoAnalyzeCounter`、`ManualAnalyzeCounter`、`StatsInaccuracyRate`、`PseudoEstimation`。两个准确度 histogram 分别使用 `(0.01, 2, 24)` 和 `(0.01, 2, 14)` 的指数桶。
- 同步加载指标：`SyncLoadCounter`、`SyncLoadTimeoutCounter`、`SyncLoadDedupCounter`、`SyncLoadHistogram`、`ReadStatsHistogram`。两个延迟 histogram 的单位是毫秒，均使用 `(1, 2, 22)` 指数桶。
- 缓存与健康度：`StatsCacheCounter`、`StatsCacheGauge`、`StatsHealthyGauge`，三者均以 `LblType`（值为 `"type"`）作为唯一标签。
- 后台作业耗时：`StatsDeltaLoadHistogram`、`StatsDeltaUpdateHistogram`、`StatsUsageUpdateHistogram`，单位为秒，共用 `(0.01, 2, 24)` 指数桶，但指标名称和帮助文本彼此独立。
- 历史统计与 Plan Replayer：`HistoricalStatsCounter` 和 `PlanReplayerTaskCounter` 的标签顺序均为 `[LblType, LblResult]`；`PlanReplayerRegisterTaskGauge` 是无标签 Gauge。前者 subsystem 为 `statistics`，后两者为 `plan_replayer`。

## 执行流程

初始化与注册流程如下：

1. `pkg/metrics/metrics.rs::InitMetrics` 通过 `INIT_METRICS_ONCE.call_once` 进入各子系统初始化序列。
2. 序列调用 `stats::InitStatsMetrics`，本函数按类别创建 histogram、counter、counter vector、gauge 和 gauge vector，并逐项写入全局 `Option`。
3. `InitMetrics` 成功后设置完成标志；失败信息只来自序列中其他返回 `Result` 的初始化器，因为 `InitStatsMetrics` 自身不返回错误。
4. `RegisterMetrics` 通过 `register_option` 读取每个 `Option`，未初始化时以 `expect("InitMetrics must run before RegisterMetrics")` panic，已初始化时克隆并注册 collector；注册冲突等错误以 `prometheus::Error` 返回。
5. 运行期手动 ANALYZE 完成后，`execute_analyze` 只对非 restricted SQL 调用 `IncManualAnalyzeCounter`，并根据执行结果写入 `succ` 或 `failed`。
6. 自动 ANALYZE 无论走已注册的系统会话 executor，还是回退到逐表 `analyze_stats_table`，都会在成功或失败分支调用 `IncAutoAnalyzeCounter`。

共享向量的后续绑定与采集是另一条链：例如 `pkg/domain/metrics/metrics.rs::InitMetricsVars` 先要求本文件的历史统计与 Plan Replayer collector 已初始化，再按 `generate/dump/capture` 和 `success/fail/send/discard` 标签组合派生业务句柄；统计缓存 metrics 以同样方式从缓存向量派生 `miss/hit/update/del/evict/reject` counter 和 `track/capacity` gauge。

## 数据与状态

19 个全局量初始均为 `None`，调用 `InitStatsMetrics` 后变成 `Some(collector)`。指标状态存储在 Prometheus collector 内部，本文件没有数据库事务、统计缓存内容或持久化数据。Counter 单调增加，Gauge 可增减或设置，Histogram 保存桶计数、样本数与样本和。

外部可见的时序标识由 `Namespace`、`Subsystem`、`Name` 和标签集合共同决定。例如 `StatsCacheCounter` 导出 `tidb_statistics_stats_cache_op` 指标族，`PlanReplayerTaskCounter` 导出 `tidb_plan_replayer_task`。这些名称、标签数量、标签顺序和桶边界都是兼容性契约；改动可能破坏 Grafana、告警或已有查询。

派生句柄与父向量共享同一 time series，而不是复制数值。`pkg/domain/metrics/migration_aster_unit_test.rs` 验证从 Domain 句柄写入后能从父向量的同标签读取；`pkg/statistics/handle/cache/metrics/migration_aster_unit_test.rs` 对缓存标签做了相同验证。

## 依赖与调用关系

- crate 与模块边界：`pkg/metrics/Cargo.toml` 定义 `astersql-metrics`，入口是 `lib.rs`，直接依赖 `astersql-metrics-common` 和 `prometheus = "0.14"`；`pkg/metrics/lib.rs` 公开 `stats` 并提供 `LblType`、`LblResult` 等标签常量。
- 下游构造依赖：本文件经 `crate::bindinfo::{compat_metricscommon, compat_prometheus}` 使用 Go 风格的 `NewCounter`、`NewCounterVec`、`NewGauge`、`NewGaugeVec`、`NewHistogram`、`ExponentialBuckets`、`with_label_values`、`inc` 等兼容接口。
- 上游初始化与注册：`pkg/metrics/metrics.rs::InitMetrics -> InitStatsMetrics`；`RegisterMetrics` 注册本文件全部 19 个 collector。
- 上游业务写入：`pkg/session/runtime/statistics.rs::ConcreteSession::execute_analyze -> IncManualAnalyzeCounter`；`pkg/domain/domain.rs` 自动 ANALYZE 分支 `-> IncAutoAnalyzeCounter`。
- 下游绑定：`pkg/domain/metrics/metrics.rs::InitMetricsVars` 消费历史统计和 Plan Replayer 指标；`pkg/statistics/handle/cache/metrics/metrics.rs::InitMetricsVars` 展示缓存向量的同类标签绑定模式。后者在其独立 crate 测试中使用本地父向量，不能据此推断生产构建必然直接引用本 crate 的静态量。
- 当前 Rust 搜索显示，若干 statistics/planner 子模块拥有自己 crate 内的同名 collector；同名不等于共享本文件实例。没有直接证据的跨 crate 写入不能作为本文件已经接线的结论。

## 错误处理与边界

`InitStatsMetrics` 不返回 `Result`，collector 工厂在该兼容 API 中也表现为直接返回 collector，因此本函数没有显式错误传播路径。初始化与注册是分离的：初始化成功不代表注册成功；重复名称、重复注册等 registry 错误由 `RegisterMetrics() -> Result<(), prometheus::Error>` 报告。

两个 `Inc*AnalyzeCounter` 在对应全局量为 `None` 时静默 no-op，这允许轻量测试或初始化前路径不 panic，但也会丢失该次观测且不会向调用者报告。它们接受任意 `&str`，类型系统不限制为 `succ/failed`；新增标签值会创建新时序，调用者必须控制拼写和基数。

直接读取共享向量的下游初始化器通常使用 `expect`：若未先初始化父 collector，会 panic。直接调用 `InitStatsMetrics` 会替换已有全局句柄；如果旧句柄已经注册或被下游克隆，新旧 collector 的数值与注册身份可能分离。因此生产代码应走一次性的包级 `InitMetrics`，测试中的重初始化必须串行并重新绑定下游句柄。

## 并发与资源生命周期

正常包级入口由 `INIT_METRICS_ONCE` 保证只初始化一次，collector 随全局静态句柄存活至进程结束，运行期计数、Gauge 更新和 Histogram 观测依赖 Prometheus collector 自身的并发安全实现。本文件不创建线程、异步任务、channel、锁、文件或网络资源。

风险边界是 `static mut`。`InitStatsMetrics` 自身没有锁，也没有一次性约束；并发调用它，或在其他线程读取静态引用时替换 `Option`，不具备本文件内的同步保证。两个计数助手也通过 `unsafe` 读取共享静态量，其安全前提是生产初始化完成后不再替换 collector。测试若需要重初始化，应像 `pkg/domain/metrics/migration_aster_unit_test.rs` 一样用互斥锁串行化，并在替换父向量后重新执行子模块的 `InitMetricsVars`。

注册使用 collector clone；clone 与派生 label 句柄共享底层状态。collector 没有显式注销流程，重新构造一个同名 collector不会自动替换 registry 中旧的实例。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metrics/stats.go`。Rust 的 19 个全局量、`InitStatsMetrics` 创建顺序、namespace/subsystem/name/help、标签顺序和全部指数桶参数与 Go 保持一致。Go 的全局接口或指针变量对应 Rust 的 `Option<...>`，用 `None` 明确表达初始化前状态。

主要差异有三点：

1. Go 文件只有 collector 定义与 `InitStatsMetrics`；Rust 额外提供 `IncManualAnalyzeCounter` 和 `IncAutoAnalyzeCounter`，把当前迁移后的业务写入封装为初始化前可 no-op 的入口。
2. Go 包初始化语义通常在启动阶段赋值一次；Rust 的包级 `InitMetrics` 有 `Once`，但公开的 `InitStatsMetrics` 本身可重入并会替换全部句柄。`pkg/domain/metrics/migration_aster_unit_test.rs` 明确验证了重初始化后旧值清零、下游重新绑定的测试语义。
3. 指标定义已对齐不代表所有 Go 采集点都已接入这组 Rust 静态量。当前可确认的直接业务接线只有手动/自动 ANALYZE helper，以及已读取到的 Domain 标签绑定；其他同名指标在若干 Rust 子 crate 中存在独立实现，应逐调用点核实，不能仅按名称推断共享。

Go 同路径没有独立 `stats_test.go`。Rust 也没有 `pkg/metrics/stats_test.rs`；相关行为分散在 ANALYZE、统计缓存和 Domain metrics 的独立测试中。

## 扩展指南

- 新增 collector：在本文件新增全局 `Option` 与 `InitStatsMetrics` 赋值，同时将它加入 `pkg/metrics/metrics.rs::RegisterMetrics`；同步核对 `pkg/metrics/stats.go`，避免无意改变已有名称、标签顺序或桶。
- 新增固定业务标签：优先从现有 CounterVec/GaugeVec 派生句柄，不要另建同名 collector，也不要重复注册派生句柄。标签值应是有限集合，防止高基数时序造成内存与抓取成本上升。
- 新增业务写入口：仿照两个 `Inc*AnalyzeCounter` 处理未初始化状态，并明确调用点的成功/失败边界。若观测不能容忍静默丢失，应在更高层建立初始化不变量，而不是随意在热路径 panic。
- 调整 histogram 桶或单位：同时检查 Grafana、告警与查询表达式；`seconds` 与 `millis` 指标不能混用。桶变化会改变导出的 `_bucket{le=...}` 时序，属于监控兼容性变更。
- 调整重初始化：必须处理 `static mut` 同步、registry 中旧 collector 和下游派生句柄三者的一致性；不要假设重新赋父 `Option` 会自动更新已有 clone。
- 测试必须放在独立文件。初始化/注册和元数据适合扩展 `pkg/metrics/metrics_test.rs` 或 `metrics_internal_test.rs`；ANALYZE 计数扩展 `pkg/statistics/handle/handletest/analyze/analyze_test.rs`；缓存绑定扩展 `pkg/statistics/handle/cache/metrics/migration_aster_unit_test.rs`；历史统计/Plan Replayer 绑定扩展 `pkg/domain/metrics/migration_aster_unit_test.rs`。

## 验证依据

- RustCodeGraph 索引：`status` 显示 11,467 个文件、307,296 个节点、1,848,419 条边，目标文件已索引；`files --filter pkg/metrics` 显示 `stats.rs` 含 4 个索引符号；`explore "pkg/metrics/stats.rs StatsCacheCounter StatsCacheMissCounter"` 给出完整文件上下文，并报告 `InitStatsMetrics` 的 5 个调用者、两个 `Inc*AnalyzeCounter` 各 1 个业务调用者；`query` 确认 Go/Rust 两个 `InitStatsMetrics` 定义及两个 Rust helper。精确 `callers/callees --file` 在本次索引上未输出文本，因此调用边又用索引文件片段和精确 `rg` 交叉验证。
- 源码与入口：`pkg/metrics/stats.rs`（19 个全局 collector、3 个公开函数）；`pkg/metrics/lib.rs`（公开模块与标签常量）；`pkg/metrics/metrics.rs`（一次性初始化、注册前置条件及 19 个注册项）；`pkg/session/runtime/statistics.rs`（手动 ANALYZE 结果计数）；`pkg/domain/domain.rs`（自动 ANALYZE 两条执行路径的结果计数）。目标包没有 `doc.go`。
- crate 与 Go 对照：`pkg/metrics/Cargo.toml`（crate 名、入口和依赖）；`pkg/metrics/stats.go`（全局量与初始化配置逐项对照）。
- 下游绑定：`pkg/domain/metrics/metrics.rs`（历史统计、Plan Replayer 标签组合）；`pkg/statistics/handle/cache/metrics/metrics.rs`（缓存标签组合及初始化顺序约束）。
- 独立测试：`pkg/statistics/handle/handletest/analyze/analyze_test.rs::TestAnalyzeMetricsCounters` 验证手动和自动成功计数；`pkg/statistics/handle/cache/statscache_test.rs::global_configuration_metrics_and_failpoints` 验证缓存 hit/miss/update 以及 `StatsDeltaLoadHistogram` 样本数；`pkg/statistics/handle/cache/metrics/migration_aster_unit_test.rs` 验证缓存派生句柄共享父时序；`pkg/domain/metrics/migration_aster_unit_test.rs` 验证历史统计/Plan Replayer 标签绑定与重初始化。Go 同路径未发现独立 `stats_test.go`。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验收由固定 11 个二级标题检查完成；文档中的路径、符号、标签和调用边均以以上源码与测试为直接证据。
