# `pkg/ttl/metrics/metrics.rs`

## 文件定位

本文件是 `astersql-ttl-metrics` crate 中的 TTL 运行时指标适配层。crate 根 `pkg/ttl/metrics/lib.rs` 以 `#[path = "metrics.rs"] pub mod ttl_metrics` 暴露它；同一 crate 的 `metrics` 模块负责定义 Prometheus `HistogramVec`、`CounterVec` 和 `GaugeVec`，本文件则预绑定常用标签，并实现 worker 相位计时与 TTL 水位延迟聚合。

`pkg/ttl/metrics/Cargo.toml` 表明该 crate 仅直接依赖 `prometheus = "0.14"`，没有 feature 开关。Rust 全局指标初始化链已经接入：`pkg/util/metricsutil/common.rs::initMetrics` 调用 `ttl_metrics::InitMetricsVars`。但截至当前源码，`pkg/ttl/ttlworker` 和 `pkg/ttl/session` 的 Rust 生产模块没有调用本文件的 worker tracer、行计数或水位更新 API；这些 API 的完整生产使用链仍见同路径 Go 文件。故本文件是已实现且有独立测试的迁移模块，不能据此宣称 Rust TTL worker 已完整接线。

## 核心职责

1. 用 `LazyLock` 为查询耗时、处理行数、Job/Task 状态等向量指标预绑定稳定的标签组合，向调用方提供可直接 `observe`、`inc_by` 或 `set` 的句柄。
2. 用 `initWorkerPhases` 为 scan/delete worker 的九种相位建立 `phase -> Counter` 映射，并由 `PhaseTracer` 在相位切换时累计上一相位的秒数。
3. 用 `PhaseContext` 在 Rust 内传递一个可共享、可变的 tracer；该类型是对 Go `context.WithValue` 用法的轻量迁移适配，并非通用异步 context。
4. 用 `WaterMarkScheduleDelayNames`、`getWaterMarkScheduleDelayName` 和 `UpdateDelayMetrics` 将每表调度相对延迟聚合成 Prometheus Gauge 分桶；`ClearDelayMetrics` 删除该 GaugeVec 的全部样本。

指标值只承担观测职责，不参与 TTL 扫描、删除或调度决策。

## 主要符号

- 相位常量 `PhaseIdle`、`PhaseBeginTxn`、`PhaseCommitTxn`、`PhaseQuery`、`PhaseCheckTTL`、`PhaseWaitRetry`、`PhaseDispatch`、`PhaseWaitToken`、`PhaseOther`：作为 `TTLPhaseTime` 的 `phase` 标签值，也作为 tracer 状态字符串。
- 预绑定指标 `SelectSuccessDuration`、`SelectErrorDuration`、`DeleteSuccessDuration`、`DeleteErrorDuration`：分别绑定 `sql_type={select,delete}` 与 `result={ok,error}`。
- 行计数 `ScannedExpiredRows`、`DeleteSuccessExpiredRows`、`DeleteErrorExpiredRows`：绑定扫描成功、删除成功和删除失败三个标签组合。
- 状态 Gauge `RunningJobsCnt`、`CancellingJobsCnt`、`ScanningTaskCnt`、`DeletingTaskCnt`：绑定 Job/Task 的状态标签。
- `init` / `InitMetricsVars`：强制求值所有上述 `LazyLock` 以及 scan/delete 相位表。Rust 不会像 Go 包那样自动调用普通 `init` 函数；真实全局入口是 `pkg/util/metricsutil/common.rs::initMetrics` 对 `InitMetricsVars` 的显式调用。
- `initWorkerPhases(workerType: &str) -> HashMap<String, Counter>`：为指定 worker 类型绑定九个 `TTLPhaseTime(worker_type, phase)` Counter。
- `PhaseTracer`：保存可注入的单调时钟 `getTime`、上报回调 `recordDuration`、当前相位 `phase` 和进入时刻 `phaseTime`。生产构造器是 `NewScanWorkerPhaseTracer` 与 `NewDeleteWorkerPhaseTracer`；`newPhaseTracer` 为测试和 crate 外调用保留可注入构造能力。
- `PhaseTracer::Phase`、`EnterPhase`、`EndPhase`：读取当前相位、切换相位并结算旧相位、以及通过进入空相位完成结算。
- `PhaseContext`、`CtxWithPhaseTracer`、`PhaseTracerFromCtx`：保存并克隆 `Arc<Mutex<PhaseTracer>>`；缺失时返回 `None`。
- `DelayMetricsRecord`：一张表的 `TableID`、`LastJobTime`、`AbsoluteDelay` 和 `ScheduleRelativeDelay`。当前文件的聚合逻辑只读取 `ScheduleRelativeDelay`，其他字段保留了 Go 数据模型和上游采集语义。
- `getWaterMarkScheduleDelayName`、`UpdateDelayMetrics`、`ClearDelayMetrics`：执行分桶查找、全桶重写和全样本清除。

## 执行流程

初始化时，`pkg/util/metricsutil/common.rs::initMetrics` 调用 `InitMetricsVars`；后者依次 `LazyLock::force` 预绑定指标和两组 worker 相位 Counter。即使某个指标尚无业务更新，这一步也建立了后续可复用的句柄。

相位追踪流程如下：

1. `NewScanWorkerPhaseTracer` 或 `NewDeleteWorkerPhaseTracer` 通过 `newPhaseTracer` 记录初始 `Instant`，但初始相位为空。
2. 第一次 `EnterPhase` 只设置相位与时间，不上报初始空相位。
3. 后续每次 `EnterPhase(next)` 取得当前时间，将 `now - phaseTime` 交给回调记录到旧相位 Counter，再更新相位与起点。同名相位再次进入也会结算并重新开始一个区间。
4. `EndPhase` 等价于 `EnterPhase("")`：结算当前非空相位并清空状态；它不是析构钩子，调用方必须显式执行。
5. 生产回调只对预建映射内的相位累加秒数；未知相位不会报错，而是被静默忽略。

水位延迟更新流程如下：

1. `UpdateDelayMetrics` 先以所有展示名建立值为 `0.0` 的临时表，因此空输入也会把已有标准分桶归零。
2. 对每条 `DelayMetricsRecord`，`getWaterMarkScheduleDelayName` 按声明顺序选择第一个满足 `ScheduleRelativeDelay <= Delay` 的桶，并累计表数。
3. 将每个桶写入 `TTLWatermarkDelay{type="schedule",name=<桶名>}`。
4. 非 leader 等不应保留任何样本的场景可调用 `ClearDelayMetrics`，其语义是 `GaugeVec::reset`，不同于把标准桶设置为零。

Go 生产链的直接证据是：`pkg/ttl/ttlworker/scan.go` 和 `del.go` 创建 tracer、写入 context 并在 SQL/重试等边界切相位；`job_manager.go::reportMetrics` 仅由 leader 周期收集并更新水位，非 leader 清空；`task_manager.go::reportMetrics` 更新 Task Gauge。Rust 对应 worker 文件目前没有这些调用。

## 数据与状态

所有预绑定 Prometheus 句柄和相位映射都是进程级 `LazyLock`。首次访问只初始化一次，此后 Counter/Gauge/Histogram 的状态由 Prometheus 类型内部维护。`InitMetricsVars` 可重复调用，但重复调用只是强制访问已初始化对象，不会重建或清零指标。

`PhaseTracer` 的状态是单个连续区间：`phase` 为空表示未处于可记录相位，`phaseTime` 始终保存最近一次构造或切换的 `Instant`。它不保存历史；历史立即通过回调累计。`PhaseContext` 克隆时只克隆 `Arc`，因此多个 context 指向同一个 tracer；调用者需锁住 `Mutex` 后才能切换相位。

水位分桶刻意保持 Go 表的顺序和阈值，包括两个看似异常但已由迁移测试锁定的重复阈值：`"02 hour"` 与 `"01 hour"` 都是 1 小时，`"one week"` 与 `"72 hour"` 都是 72 小时。由于采用“第一个上界”规则，前者之后的重复桶不会接收记录：超过 1 小时会进入 `"06 hour"`，超过 72 小时会进入 `"others"`。修改这些值属于兼容性变更，不能当作普通纠错。

## 依赖与调用关系

下游依赖集中在同 crate 的 `crate::metrics`：`TTLQueryDuration`、`TTLProcessedExpiredRowsCounter`、`TTLJobStatus`、`TTLTaskStatus`、`TTLPhaseTime` 和 `TTLWatermarkDelay` 均定义于 `pkg/ttl/metrics/lib.rs`；外部 `prometheus` crate 提供具体句柄类型。标准库提供 `LazyLock`、`HashMap`、`Arc<Mutex<_>>` 以及单调/墙钟时间类型。

Rust 上游的已确认生产调用边只有 `pkg/util/metricsutil/common.rs::initMetrics -> InitMetricsVars`。Cargo 层面，`pkg/util/metricsutil/Cargo.toml` 直接依赖本 crate；`pkg/ttl/ttlworker/Cargo.toml` 仅在 `cfg(windows)` 下声明依赖，`pkg/ttl/session/Cargo.toml` 仅在永假条件 `cfg(any())` 下声明依赖，源码检索未发现两者对本文件 API 的 Rust 生产调用。

Go 对照调用边提供预期业务位置：scan worker 使用扫描 tracer 和 SELECT 指标；delete worker 使用删除 tracer 和 DELETE 指标；TTL session 在事务边界切换 begin/commit/other 相位；JobManager 更新 Job Gauge 与水位；taskManager 更新 Task Gauge。文档中的这些关系是 Go 当前行为与 Rust 扩展落点，不是 Rust 已接线声明。

## 错误处理与边界

本文件没有 `Result` 返回路径。指标构造与标签数量依赖 `pkg/ttl/metrics/lib.rs` 中固定定义；若未来标签定义与本文件的 `with_label_values` 参数数量失配，初始化阶段可能失败，因此两处必须同步修改。

`EnterPhase` 使用 `Instant::duration_since`。生产 `Instant::now` 单调递增；测试注入时钟若倒退会触发 panic。未知相位仍会成为 `PhaseTracer::phase`，但 scan/delete 生产回调因映射查找失败而不记录，这一静默丢弃是当前边界。`EndPhase` 在空相位上调用不会上报。

`PhaseTracerFromCtx` 对缺失值返回 `None`，但 Go 生产调用方通常直接使用返回 tracer；Rust 接线时必须显式处理 `None` 或保证先经过 `CtxWithPhaseTracer`。共享 tracer 的 `Mutex` 若被持锁线程 panic 会中毒，解锁策略属于未来调用方责任。

分桶比较包含等号，边界值进入较早的桶。最后一个 `"others"` 的阈值是 `Duration::from_nanos(i64::MAX as u64)`；更大的 Rust `Duration` 即使没有匹配，也由 `last().unwrap()` 回落到 `"others"`。该 `unwrap` 依赖静态分桶表永不为空这一不变量。

## 并发与资源生命周期

`LazyLock` 保证全局句柄和映射的线程安全一次性初始化；Prometheus Counter/Gauge/Histogram 句柄可克隆并由库内部协调并发更新。`UpdateDelayMetrics` 的临时 `HashMap` 是调用栈局部状态，但对全局 Gauge 的“逐桶 set”不是跨所有桶的原子快照，并发调用可能互相覆盖；设计上应由单一 TTL leader 的周期汇报路径串行调用。

`PhaseTracer` 自身通过 `&mut self` 切换，`PhaseContext` 用 `Arc<Mutex<PhaseTracer>>` 才能跨所有权边界共享。锁的获取与持有不在本文件实现；接线时应只在快速切相位操作期间持锁，避免把业务 I/O 包在锁内。回调在 `EnterPhase` 内同步执行，慢回调会直接延长切相位路径；当前 Prometheus `inc_by` 回调很短。

tracer 没有 `Drop` 实现，离开作用域不会自动结算最后相位。Go worker 通过 defer 调用 `EndPhase`；未来 Rust worker 需要用显式收尾或可靠 guard 保持这一生命周期语义。`ClearDelayMetrics` 会移除 GaugeVec 的全部当前样本，调用前应确认调用者拥有全局 TTL 水位指标的管理权。

## 与 Go 版本的对应关系

主要实现逐项对应 `pkg/ttl/metrics/metrics.go`：相位字符串、预绑定标签、scan/delete 相位表、`PhaseTracer` 切换规则、context 存取、水位数据结构、首个上界分桶算法、更新与清空语义均保持一致。`pkg/ttl/metrics/metrics_test.go::TestPhaseTracer` 与 Rust `metrics_test.rs::test_phase_tracer` 使用可控时钟验证了相同的首次进入、切换、同名再进入和结束行为。

语言差异包括：Go 使用包初始化和可重赋值全局变量，Rust 使用 `LazyLock` 并由全局 metricsutil 显式强制初始化；Go tracer 是可为 `nil` 的指针且方法对 `nil` 安全，Rust `PhaseTracer` 是值类型，不存在 nil receiver；Go 使用通用 `context.Context` 的私有 key，Rust 使用专用 `PhaseContext` 和 `Arc<Mutex<_>>`；Go 延迟记录使用指针 map，Rust 使用借用的 `HashMap<i64, DelayMetricsRecord>`。

`migration_aster_unit_test.rs` 额外锁定重复分桶阈值、空 map 归零、`ClearDelayMetrics` 删除样本、context 往返和指标 descriptor/bucket 配置。Rust 的测试覆盖比原 Go `metrics_test.go` 更宽，但不等于 Rust TTL worker 已完成运行时接线。

## 扩展指南

- 新增 worker 相位时，同时更新相位常量、`initWorkerPhases`、Go 对照（若仍要求兼容）及独立测试；必须决定旧版本 dashboard 对新标签值的处理，并避免高基数动态相位。
- 新增预绑定指标时，在 `pkg/ttl/metrics/lib.rs` 定义向量，在本文件绑定标签并加入 `InitMetricsVars`，再在 `migration_aster_unit_test.rs` 验证名称、help、标签和桶。标签次序或名称变化会影响抓取与 dashboard 兼容性。
- 接入 Rust scan/delete/session/job manager 时，应优先复用现有 API，并以 Go 的 `scan.go`、`del.go`、`session.go`、`job_manager.go` 和 `task_manager.go` 为行为基线；测试必须放在独立 `*_test.rs` 文件，不得内嵌到本源文件。
- 调整水位桶前先确认是否要修复还是保持 Go 的重复阈值；任何修复都应同步 dashboard/告警预期，并更新 `migration_aster_unit_test.rs::watermark_buckets_preserve_go_thresholds_and_order`。桶展示名是指标标签，重命名会形成新的时间序列。
- 若要让 tracer 自动收尾，可引入 guard，但需避免 `Drop` 中锁中毒或 panic，并证明与 Go defer 的异常/取消路径等价。不要以自动收尾替代明确的相位边界测试。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ttl/metrics` 确认目标、crate 根、Go 对照和三份测试材料均已索引；`node --file pkg/ttl/metrics/metrics.rs --offset 1/260` 读取了完整 331 行并显示目标文件被索引引用；`query` 核对了 `initWorkerPhases`、`NewScanWorkerPhaseTracer`、`NewDeleteWorkerPhaseTracer`、`newPhaseTracer`、`CtxWithPhaseTracer`、`PhaseTracerFromCtx` 和 `UpdateDelayMetrics` 的 Go/Rust同名符号。批量 `callers/callees` 查询在等待窗口内未返回，因此调用边另以直接源码检索核验。
- Rust 源与 crate：`pkg/ttl/metrics/metrics.rs`、`pkg/ttl/metrics/lib.rs`、`pkg/ttl/metrics/Cargo.toml`、`pkg/util/metricsutil/common.rs`、`pkg/util/metricsutil/Cargo.toml`，以及作为当前接线状态证据的 `pkg/ttl/ttlworker/Cargo.toml`、`pkg/ttl/session/Cargo.toml`。
- Go 对照与生产调用：`pkg/ttl/metrics/metrics.go`、`pkg/ttl/session/session.go`、`pkg/ttl/ttlworker/scan.go`、`del.go`、`job_manager.go`、`task_manager.go`。
- 独立测试：`pkg/ttl/metrics/metrics_test.rs`、`pkg/ttl/metrics/migration_aster_unit_test.rs`、`pkg/ttl/metrics/metrics_test.go`。本任务是纯文档分析，按计划未运行 Cargo。
