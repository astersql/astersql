# `pkg/metrics/stmtsummary.rs`

## 文件定位

本文件属于 `astersql-metrics` crate 的 statement-summary 指标子模块。crate 由 `pkg/metrics/Cargo.toml` 定义，入口 `pkg/metrics/lib.rs` 以 `pub mod stmtsummary` 暴露本模块；因此其中的常量、三个指标向量和两个公开函数都可供同一工作区的 Rust crate 使用。

它位于指标生命周期的“声明与更新”层，而不负责生成语句摘要本身。`pkg/metrics/metrics.rs::InitMetrics` 调用 `InitStmtSummaryMetrics` 构造指标，随后同文件的指标注册列表注册三个 collector。语句摘要的聚合、LRU 和持久化逻辑不在本文件：Go v1 位于 `pkg/util/stmtsummary/statement_summary.go`，Go v2 位于 `pkg/util/stmtsummary/v2/`。

当前 Rust 接线范围需要特别说明：仓库搜索只找到 `pkg/metrics/metrics.rs` 对初始化和注册的生产引用，以及 `pkg/metrics/metrics_2_aster_unit_test.rs`、`pkg/metrics/metrics_internal_test.rs` 对更新 API 的测试引用；未找到 Rust 版 statement-summary 生产者调用 `SetStmtSummaryWindowMetrics` 或递增 `StmtSummaryEvictedLogCounter`。所以 Rust crate 已提供并注册真实 Prometheus collector，但业务上报链尚不能仅由本文件证明已经迁入 Rust。

## 核心职责

1. 用四个字符串常量统一指标标签值：`StmtSummaryTypeV1 = "v1"`、`StmtSummaryTypeV2 = "v2"`，以及 v2 淘汰日志结果 `persisted`、`dropped`。
2. 声明三个延迟初始化的全局指标向量：当前窗口记录数、当前窗口累计 LRU 淘汰数，以及 v2 淘汰日志处理结果计数。
3. 在 `InitStmtSummaryMetrics` 中构造与 Go 版本同名、同帮助文本、同标签维度的 collector，并清空旧的 v1/v2 子指标句柄缓存。
4. 在 `SetStmtSummaryWindowMetrics` 中同时更新一类实现的记录数与淘汰数；对常见的 v1/v2 标签复用缓存句柄，避免每次重新做标签查找。

三个最终指标名由构造选项组合得到：`tidb_stmt_summary_window_record_count{type=...}`、`tidb_stmt_summary_window_evicted_count{type=...}` 和 `tidb_stmt_summary_evicted_log_total{type=...,result=...}`。前两者是可升可降的瞬时窗口 Gauge；后者是只能累计的 Counter。

## 主要符号

- `StmtSummaryTypeV1`、`StmtSummaryTypeV2`：公开的实现类型标签，分别代表内存版 v1 与可持久化 v2。
- `StmtSummaryEvictedLogResultPersisted`、`StmtSummaryEvictedLogResultDropped`：公开的 v2 淘汰日志结果标签。它们只定义标签契约；本文件不直接递增计数器。
- `StmtSummaryWindowRecordCount: Option<GaugeVec>`：按 `type` 分组的当前窗口记录数。
- `StmtSummaryWindowEvictedCount: Option<GaugeVec>`：按 `type` 分组的当前窗口 LRU 淘汰数；Go 对照明确该值在窗口轮换时回到 0，因此选用 Gauge 而非 Counter。
- `StmtSummaryEvictedLogCounter: Option<CounterVec>`：按 `type`、`result` 分组的淘汰日志累计结果。
- `StmtSummaryWindowMetrics`：私有缓存状态，把 v1/v2 各自的 record/evicted Gauge 句柄放在同一结构中。
- `stmtSummaryWindowMetricsMu: Mutex<StmtSummaryWindowMetrics>`：保护四个缓存槽位的进程级互斥锁。
- `pub unsafe fn InitStmtSummaryMetrics()`：构造三个向量并在锁内清空句柄缓存。`unsafe` 来自对 `static mut` 的读写，而不是 Prometheus API 本身。
- `pub unsafe fn SetStmtSummaryWindowMetrics(typ, recordCount, evictedCount)`：公开更新入口；v1/v2 走缓存，其他标签直接从向量取得子指标。
- `unsafe fn getStmtSummaryWindowMetricsLocked(typ)`：私有的加锁、惰性取句柄函数，仅接受 v1/v2；返回 clone 后锁即释放。

文件没有 trait、枚举、条件编译项或本地测试模块。测试通过 `pkg/metrics/lib.rs` 中的 `#[cfg(test)] #[path = ...]` 独立文件装配，符合生产逻辑与测试分文件的仓库约定。

## 执行流程

初始化流程如下：

1. `pkg/metrics/metrics.rs::InitMetrics` 的 `Once` 初始化闭包调用 `InitStmtSummaryMetrics`。
2. `InitStmtSummaryMetrics` 通过 `compat_metricscommon::NewGaugeVec` 创建两个带 `type` 标签的 GaugeVec，通过 `NewCounterVec` 创建带 `type`、`result` 标签的 CounterVec。
3. 构造器最终委托 `astersql-metrics-common` 和上游 `prometheus` crate，因而创建的是真实 collector，并会注入 metrics-common 管理的常量标签。
4. 初始化函数取得 `stmtSummaryWindowMetricsMu`，把四个缓存槽位重置为 `None`，防止重新构造向量后继续写入旧向量的子句柄。
5. `pkg/metrics/metrics.rs` 的注册阶段把三个全局 `Option` 解包并注册到默认 registry。

窗口更新流程如下：

1. 调用方把实现类型、当前记录数和当前窗口淘汰数传给 `SetStmtSummaryWindowMetrics`。
2. 当 `typ` 是 `v1` 或 `v2` 时，函数调用 `getStmtSummaryWindowMetricsLocked`。
3. 私有函数持锁检查对应缓存；首次访问时分别从两个 GaugeVec 以同一 `type` 标签创建子 Gauge，此后复用 clone 的句柄。
4. 公开函数在锁已释放后依次 `Set(recordCount)` 和 `Set(evictedCount)`。
5. 对未知 `typ`，公开函数跳过专用缓存，直接按该标签获取两个子 Gauge 并设置值；这保留了 Go 的可扩展默认分支。

淘汰日志 Counter 不经过上述窗口更新函数。Go v2 的 `pkg/util/stmtsummary/v2/stmtsummary.go::onEvict` 在非阻塞发送失败时递增 `dropped`，`pkg/util/stmtsummary/v2/logger.go::logEvicted` 在写出记录后按成功条数增加 `persisted`。

## 数据与状态

三个公开 `static mut Option<...>` 的初值均为 `None`。初始化之后，Option 内保存向量句柄；向量在首次按标签访问时产生时序。`pkg/metrics/metrics_internal_test.rs::test_stmt_summary_metric_labels` 验证初始化后采集条目数为 0，写 v1 后两个 GaugeVec 各有 1 条，写 v2 后各有 2 条，说明“声明向量”和“创建标签序列”是分开的。

缓存结构只保存四个常用 Gauge 子句柄，不缓存 Counter 子句柄，也不缓存未知类型。Gauge 句柄的 clone 指向同一底层指标，而非复制数值；这使缓存复用不改变观测结果。缓存与全局向量必须成对更新，所以 `InitStmtSummaryMetrics` 每次重建向量后都清空缓存。

`recordCount` 与 `evictedCount` 使用 `f64`，与 Prometheus Gauge 及 Go API 一致。文件本身不校验非负、整数或有限值，也不保存窗口起止时间；这些语义由上游 statement-summary 生产者负责。淘汰日志 Counter 的标签约定定义在此处，计数值由外部调用者更新。

## 依赖与调用关系

上游接线：

- `pkg/metrics/lib.rs` 公开 `stmtsummary` 模块，并通过 `pub use session::*` 提供本文件使用的 `LblType`、`LblResult` 标签名常量。
- `pkg/metrics/metrics.rs::InitMetrics` 调用 `InitStmtSummaryMetrics`；同文件的注册元组读取并注册三个全局向量。
- Rust 直接行为测试位于 `pkg/metrics/metrics_internal_test.rs::test_stmt_summary_metric_labels`；`pkg/metrics/metrics_2_aster_unit_test.rs` 还在全包初始化测试中验证 v1 写入可读回。

下游依赖：

- `crate::bindinfo::compat_metricscommon` 把 Go 风格构造参数转换为 `prometheus::Opts`，再调用 `astersql_metrics_common` 的工厂。
- `crate::bindinfo::compat_prometheus` 再导出真实 `prometheus::{GaugeVec, Gauge, CounterVec}`，并用 `MetricCompat`、`GaugeCompat` 等 trait 提供 `WithLabelValues`、`Set` 这类 Go 风格方法名。
- `pkg/metrics/Cargo.toml` 直接依赖 `prometheus = "0.14"` 和本地 `astersql-metrics-common`；本文件没有 feature gate。

业务调用关系的当前事实：Go v1 的 `stmtSummaryByDigestMap::updateMetricsLocked` 上报 `v1`，Go v2 的 `StmtSummary::updateMetrics` 上报 `v2`；Go v2 的淘汰与日志路径递增两类结果 Counter。RustCodeGraph 将目标文件列为被 `metrics.rs`、`metrics_2_aster_unit_test.rs`、`metrics_internal_test.rs` 使用，但其精确 callers/callees 查询没有给出函数级边，因此这些调用点由仓库 `rg` 搜索补证。

## 错误处理与边界

- `InitStmtSummaryMetrics` 和 `getStmtSummaryWindowMetricsLocked` 对互斥锁使用 `expect("statement summary metrics mutex poisoned")`。一旦持锁线程 panic 导致锁中毒，后续调用也会 panic；这里没有可恢复错误返回。
- 两个公开函数都假定全局向量已经初始化。若先调用 `SetStmtSummaryWindowMetrics`，内部 `Option::unwrap()` 会 panic。正常入口通过 `metrics::InitMetrics` 保证初始化先发生。
- 指标构造器把无效 Prometheus 选项视为编程错误并 `expect`；本文件使用固定合法名称和标签，因此没有 `Result` 传播路径。
- `getStmtSummaryWindowMetricsLocked` 的未知类型分支是 `unreachable!`。公开函数只在 v1/v2 时调用它，未知类型在外层走直接标签查找，所以按现有封装不可达；若未来新增直接内部调用，必须维持此前置条件。
- 未知 `typ` 并非错误，会创建新标签序列。调用者若传入无界动态字符串，可能造成 Prometheus 标签基数持续增长；扩展实现类型时应使用稳定常量。
- 文件不限制 Gauge 的负数、NaN 或无穷值，也不验证 record/eviction 的业务一致性。安全扩展不能把指标层当作输入校验层。

## 并发与资源生命周期

`stmtSummaryWindowMetricsMu` 只保护四个缓存槽位以及“检查后创建”的原子区间。`getStmtSummaryWindowMetricsLocked` 在返回前释放锁，真正的 Gauge `Set` 在锁外执行；上游 `prometheus` 指标句柄负责数值更新的线程安全，避免把全局缓存锁扩展到热路径写入。

`metrics::InitMetrics` 使用一次性初始化保护正常全局生命周期，生产路径预期为“构造一次、注册一次、长期更新”。本文件的 `InitStmtSummaryMetrics` 自身仍是可重复调用的，主要便于隔离测试；它在重建 collector 时清缓存，但对 `static mut` 的替换没有本地同步保护。因而调用方不得让初始化/重初始化与指标读取或更新并发发生，`unsafe` API 把这个不变量交给调用者。

缓存句柄随进程静态状态存活，不需要显式关闭。重新初始化会丢弃缓存中的旧 clone，但已经注册或被其他位置持有的旧 collector 不会由本文件注销。测试使用 `run_in_isolated_process` 避免全局 registry、重复初始化及并发测试互相污染；新增相关测试也应沿用独立测试文件和隔离策略。

## 与 Go 版本的对应关系

`pkg/metrics/stmtsummary.go` 是逐符号的直接对照：四个标签常量、三个指标向量、初始化函数、窗口更新函数、四个缓存槽位和一把 mutex 均有 Rust 对应物。指标 namespace、subsystem、name、help 和标签顺序保持一致，`pkg/metrics/metrics_internal_test.go::TestStmtSummaryMetricLabels` 与 Rust 同名语义测试也使用相同的 0→v1→v2→淘汰结果断言。

实现形态上的差异包括：

- Go 的全局指针零值映射为 Rust 的 `Option<...>`，因此 Rust 在使用点显式 `unwrap`。
- Go 的四个独立缓存变量合并进 `StmtSummaryWindowMetrics`，仍由一把 mutex 保护。
- Go Gauge 接口值复制对应 Rust Gauge 句柄 `clone`，两者都引用共享底层序列。
- Go 未知类型的私有函数返回两个 `nil`；Rust 标为 `unreachable!`，因为公开入口已分流未知类型。
- Rust 对可变全局访问显式标注 `unsafe`，而 Go 依靠包初始化约定和 mutex 表达生命周期。

需要避免误读源码第 27 行的旧注释：“不会注册或上报 Prometheus 指标”与当前直接证据不一致。兼容构造器已经委托真实 `prometheus` crate，`metrics.rs` 也实际注册这些 collector，Rust 测试可读回写入值。可确认的限制不是“指标为空壳”，而是仓库中尚未找到 Rust statement-summary 业务生产者接线。

## 扩展指南

- 新增稳定的 statement-summary 实现类型时，先增加公开标签常量。若该类型位于高频路径并需要专用缓存，应扩展 `StmtSummaryWindowMetrics`、初始化清理逻辑和 `getStmtSummaryWindowMetricsLocked` 的匹配分支；若调用频率低，可保留未知类型默认路径，但须控制标签基数。
- 修改指标名、帮助文本或标签维度时，必须同步 `InitStmtSummaryMetrics`、`pkg/metrics/metrics.rs` 的注册契约、Go 对照文件及 dashboard/告警消费者；标签维度改变还要求修改全部 `WithLabelValues` 调用，否则运行时可能 panic。
- 调整窗口语义时，应在真正的 v1/v2 生产者中保持调用发生在状态一致的时点。Go 证据显示 v1 在持有 map 锁时上报，v2 的 `updateMetrics` 要求持有 `windowLock`；未来 Rust 生产者应保留“先稳定读取窗口，再一次提交两个 Gauge”的不变量。
- 新增淘汰结果时应使用固定结果常量，并在事件实际完成或确定丢弃的位置递增 Counter，不能在排队前预先计数。
- 测试应继续放在独立文件。指标声明/标签/缓存行为优先扩展 `pkg/metrics/metrics_internal_test.rs`；全局初始化与注册覆盖可扩展 `pkg/metrics/metrics_2_aster_unit_test.rs`。若迁移 v1/v2 业务调用，还需在对应独立 Rust statement-summary 测试中覆盖窗口轮换、LRU 淘汰、异步日志成功/丢弃以及并发更新。
- 主要风险是全局初始化顺序、重复注册、`static mut` 并发访问、标签维数不匹配和高基数。任何把安全 API 包在这些函数外层的改造，都应把“初始化一次且早于读写”编码为类型或同步约束，而不是只隐藏 `unsafe`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/metrics` 确认目标文件、Go 对照与测试位置；`node --file pkg/metrics/stmtsummary.rs --offset 1 --limit 240` 读取完整 164 行并报告使用者为 `metrics.rs`、`metrics_2_aster_unit_test.rs`、`metrics_internal_test.rs`；`query` 确认 Rust/Go 两套 `InitStmtSummaryMetrics`、`SetStmtSummaryWindowMetrics`、`getStmtSummaryWindowMetricsLocked`。精确 callers/callees 未返回函数级边，未把空结果臆测成“无调用”。
- 源码与配置：`pkg/metrics/stmtsummary.rs`、`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`、`pkg/metrics/bindinfo.rs`、`pkg/metrics/common/wrapper.rs`。
- Go 对照与真实生产者：`pkg/metrics/stmtsummary.go`、`pkg/util/stmtsummary/statement_summary.go::updateMetricsLocked`、`pkg/util/stmtsummary/v2/stmtsummary.go::{updateMetrics,onEvict}`、`pkg/util/stmtsummary/v2/logger.go::logEvicted`。
- 独立测试：`pkg/metrics/metrics_internal_test.rs::test_stmt_summary_metric_labels`、`pkg/metrics/metrics_2_aster_unit_test.rs` 的全局指标初始化断言，以及 Go 对照 `pkg/metrics/metrics_internal_test.go::TestStmtSummaryMetricLabels`。仓库没有 `stmtsummary_test.rs` 同名测试文件。
- 人工复核结论：本文件存在是为了集中声明、初始化、注册并高效更新 statement-summary 可观测指标；运行时先初始化和注册，再由生产者按窗口设置 Gauge、按淘汰结果累加 Counter；安全扩展必须维护标签契约、缓存与向量同步、初始化先行以及有界标签集合。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另执行任务指定的 11 章节结构检查，并检查变更范围只含本说明文档与任务文件清理。
