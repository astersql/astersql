# `pkg/metrics/gc_worker.rs`

## 文件定位

本文件属于 `astersql-metrics` crate 的 `gc_worker` 公共模块；模块由 `pkg/metrics/lib.rs` 的 `pub mod gc_worker` 暴露。它不是 GC 算法或 MVCC 清理实现，而是 GC Worker 的 Prometheus 指标定义与初始化适配层：负责构造 collector，并将它们发布到包级静态槽位，供后续 GC 业务代码记录动作、耗时、配置和失败情况。

全局指标初始化入口 `pkg/metrics/metrics.rs::InitMetrics` 在 `Once` 保护的初始化序列中调用 `gc_worker::InitGCWorkerMetrics`。当前 Rust `RegisterMetrics` 的 collector 列表没有包含本文件的 GC 指标；全仓 Rust 引用检索也只找到初始化/测试引用，没有找到 GC 业务侧写入这些 collector 的调用。因此，本文件已经具备描述符和初始化语义，但不能仅凭本文件认定指标已在 Rust 服务中完成注册与采样接线。

## 核心职责

1. `InitGCWorkerMetrics` 构造六类与 Go 同名同标签的 GC Worker collector：动作次数、阶段耗时、配置值、任务失败、Region 级动作结果，以及单个 Region 多次扫锁次数。
2. 同一初始化函数还构造 `GCUnsafeDestroyRangeFailuresCounterVec` 的本地等价 collector，弥补 Rust TiKV client 当前没有直接导出 Go `client-go` collector 的差异。
3. `StageTotal` 统一规定 `GCHistogram` 的整轮 GC 耗时标签值为 `"total"`。
4. `init_gc_unsafe_destroy_range_metric` 允许调用方用上游 collector 替换本地 unsafe-destroy-range 失败计数器。

本文件只拥有 collector 的定义和发布，不决定何时递增、设置或观察，也不负责向默认 Prometheus registry 注册 collector；这些职责应由调用方和 `pkg/metrics/metrics.rs` 的注册流程承担。

## 主要符号

- `GCWorkerCounter: Option<CounterVec>`：指标名 `tidb_tikvclient_gc_worker_actions_total`，变量标签为 `type`，累计 GC Worker 动作。
- `GCHistogram: Option<HistogramVec>`：指标名 `tidb_tikvclient_gc_seconds`，变量标签为 `stage`；桶边界由 `ExponentialBuckets(1.0, 2.0, 20)` 生成，即 20 个从 1 秒开始、逐次翻倍的桶，最后一个上界为 524,288 秒。
- `GCConfigGauge: Option<GaugeVec>`：指标名 `tidb_tikvclient_gc_config`，变量标签为 `type`，承载当前 GC 配置值。
- `GCJobFailureCounter: Option<CounterVec>`：指标名 `tidb_tikvclient_gc_failure`，变量标签为 `type`，累计 GC 任务失败。
- `GCActionRegionResultCounter: Option<CounterVec>`：指标名 `tidb_tikvclient_gc_action_result`，变量标签为 `type`，累计 Region 粒度的 GC 动作结果。
- `GCRegionTooManyLocksCounter: Option<Counter>`：指标名 `tidb_tikvclient_gc_region_too_many_locks`，无变量标签，累计同一 Region 需要多次 scan-lock 的事件。
- `GCUnsafeDestroyRangeFailuresCounterVec: Option<CounterVec>`：指标名 `tidb_tikvclient_gc_unsafe_destroy_range_failures`，变量标签为 `type`；初始值由本文件创建，也可由 `init_gc_unsafe_destroy_range_metric` 替换。
- `InitGCWorkerMetrics()`：本文件的主要公开初始化入口。它先取得 `metrics::PACKAGE_INIT_LOCK`，完成全部 collector 构造后，再在一个 `unsafe` 块中批量写入上述静态槽位。
- `StageTotal: &str = "total"`：`GCHistogram` 表示“一整轮 GC”时的 `stage` 标签值。
- `init_gc_unsafe_destroy_range_metric(counter: CounterVec)`：直接把调用方提供的 collector 移入 unsafe-destroy-range 静态槽位；当前索引和全仓 Rust 搜索没有发现生产调用者。

文件没有自定义 struct、enum、trait 或条件编译项。导入的 `CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat` 扩展 trait 在本文件自身没有直接调用，主要维持同类 Go 迁移模块的兼容导入形状。

## 执行流程

`metrics::InitMetrics` 的相关主流程如下：

1. `INIT_METRICS_ONCE.call_once` 保证整个 metrics crate 的总初始化序列最多执行一次。
2. 初始化序列调用 `gc_worker::InitGCWorkerMetrics`。
3. `InitGCWorkerMetrics` 获取 `PACKAGE_INIT_LOCK`；锁中毒时以 `expect("metrics init lock poisoned")` 终止。
4. 函数先在局部变量中依次构造六个 Go 对齐 collector，再构造 unsafe-destroy-range 的本地 collector。构造通过 `bindinfo::compat_metricscommon` 转换 Go 风格选项，最终委托 `astersql-metrics-common` 和 `prometheus` crate。
5. 所有构造完成后，函数在一个 `unsafe` 块内把七个局部 collector 写入 `static mut Option<_>`。这种“先完整构造、后集中发布”避免在正常返回路径上留下部分槽位已更新、部分未更新的状态。
6. 如果上层拥有 TiKV client 提供的等价 collector，可再调用 `init_gc_unsafe_destroy_range_metric` 覆盖第七个槽位。

`InitGCWorkerMetrics` 本身不调用 `RegisterMetrics`。当前 `metrics.rs::RegisterMetrics` 的 `register_options!` 列表没有列出上述 GC collector，因此初始化成功与对外暴露到默认 registry 是两个不同状态。

## 数据与状态

七个 collector 都保存在 `pub static mut Option<_>` 中，初始为 `None`，初始化后为 `Some`。`Option` 显式表示“尚未初始化”，测试在调用 `InitGCWorkerMetrics` 后用 `unwrap` 验证槽位已填充。collector 的 namespace 固定为 `tidb`、subsystem 固定为 `tikvclient`；向量标签名称和顺序属于监控兼容契约，变更会影响查询、仪表盘和告警。

构造器会经过 `pkg/metrics/common/wrapper.rs` 注入 metrics 包当前的全局常量标签。计数器表达单调累计事件，Gauge 表达可变配置值，Histogram 保存观测计数、总和与桶分布。`StageTotal` 只是标签字符串，不保存运行状态。

重复直接调用 `InitGCWorkerMetrics` 会创建新 collector 并替换槽位中的旧值；公开函数自身没有 `Once`。生产总入口用 `INIT_METRICS_ONCE` 防止这种替换，但单独调用者必须自行保证初始化时序。替换 `GCUnsafeDestroyRangeFailuresCounterVec` 也会丢弃槽位中原先的 handle；已经克隆或注册的旧 collector 不会自动随槽位替换而变化。

## 依赖与调用关系

- 上游模块：`pkg/metrics/lib.rs` 声明并公开 `gc_worker`；`pkg/metrics/metrics.rs::InitMetrics` 是已验证的生产初始化调用者。
- 测试调用者：`pkg/metrics/gc_worker_test.rs` 的两个测试直接调用 `InitGCWorkerMetrics`；`pkg/metrics/bindinfo_1_aster_unit_test.rs` 也在兼容层初始化检查中调用它。
- 下游适配：`crate::bindinfo::compat_metricscommon` 提供 Go 风格 `NewCounterVec`、`NewGaugeVec`、`NewHistogramVec`、`NewCounter`，并转发到 `crate::metricscommon`。
- 下游库：`crate::bindinfo::compat_prometheus` 暴露真实 `prometheus` collector 类型、选项类型以及 `ExponentialBuckets`；`pkg/metrics/Cargo.toml` 直接依赖 `prometheus = "0.14"` 和路径依赖 `astersql-metrics-common`。
- 同步依赖：`crate::metrics::PACKAGE_INIT_LOCK` 串行化 metrics 子模块对可变静态槽位的写入。
- 当前缺口：RustCodeGraph 把 `metrics.rs::InitMetrics -> InitGCWorkerMetrics` 识别为调用边；对 `init_gc_unsafe_destroy_range_metric` 未识别到调用者。补充全仓 `rg` 后，仍未发现这些静态 collector 的 Rust 业务写入，也未发现 GC collector 出现在 Rust `RegisterMetrics` 列表中。

## 错误处理与边界

本文件的公开函数没有 `Result` 返回值。可能的失败都表现为 panic：初始化锁中毒时 `expect`；指数桶参数非法时 `ExponentialBuckets` 的兼容封装 `expect`；collector 名称、帮助文本或标签配置非法时，各 `New*` 构造器最终 `expect`。本文件使用的是固定常量参数，现有测试覆盖描述符和桶边界，但没有模拟这些 panic 分支。

调用方在初始化前读取 `Option` 必须处理 `None`；现有测试选择 `unwrap`，生产使用者不应把该测试假设替代初始化顺序保证。`init_gc_unsafe_destroy_range_metric` 不验证传入 collector 的名称、标签或帮助文本，因此错误形状的 collector 也能覆盖静态槽位。若要开放生产注入，调用方或该函数需要建立描述符兼容检查及相应独立测试。

指标标签值没有在此文件中枚举或校验，只有标签键被固定。高基数 `type`/`stage` 值会增加时间序列数量，属于调用方必须控制的边界。

## 并发与资源生命周期

`PACKAGE_INIT_LOCK` 只覆盖 `InitGCWorkerMetrics` 的构造和批量赋值，使采用同一锁的 metrics 子模块不会同时改写包级状态。总入口外层的 `INIT_METRICS_ONCE` 才提供“生产初始化一次”的保证。`init_gc_unsafe_destroy_range_metric` 没有获取该锁，因此它若与初始化或读取并发执行，会对 `static mut` 产生未同步访问；其安全前提是仅在单线程启动接线阶段调用。

`static mut` 不能为读写提供 Rust 层面的线程安全。collector 类型内部支持并发采样，但装载 collector 的 `Option` 槽位本身仍要求外部同步。扩展时优先考虑以 `OnceLock`、`LazyLock` 或其他不可替换的线程安全容器承载 collector，而不是增加新的裸 `static mut`。

collector 随进程存活，没有显式释放逻辑。初始化覆盖会 drop 槽位中的旧 handle；若旧 collector 已被 registry 或其他调用者克隆持有，其底层生命周期由那些持有者继续决定。Histogram 子对象通过标签值取得，观测值持续累计到所属 collector 生命周期结束。

## 与 Go 版本的对应关系

`pkg/metrics/gc_worker.go` 是直接语义基准。Rust 保留了六个 Go 初始化 collector 的名称、namespace、subsystem、帮助文本、标签键以及直方图桶 `ExponentialBuckets(1, 2, 20)`；`StageTotal = "total"` 也完全一致。Rust 使用 `Option<Collector>` 表达 Go 的 nil 指针初始态，且额外使用 `PACKAGE_INIT_LOCK` 和总入口 `Once` 约束测试及并发初始化。

主要差异是 unsafe-destroy-range collector 的来源。Go 的包级 `init()` 直接把 `github.com/tikv/client-go/v2/metrics.TiKVUnsafeDestroyRangeFailuresCounterVec` 赋给本包变量；Rust 没有相应 client crate 导出，所以 `InitGCWorkerMetrics` 创建同名、同标签的本地 collector，并额外提供 `init_gc_unsafe_destroy_range_metric` 作为注入点。这保证描述符兼容，不等同于已经接入 TiKV client 内部的失败事件。

另一个当前差异是注册接线：Go `pkg/metrics/metrics.go::RegisterMetrics` 显式注册 `GCActionRegionResultCounter`、`GCConfigGauge`、`GCHistogram`、`GCJobFailureCounter`、`GCRegionTooManyLocksCounter` 和 `GCWorkerCounter`；Rust `pkg/metrics/metrics.rs::RegisterMetrics` 当前未列出这些项。Go 也没有在该列表中注册 client-go 拥有的 unsafe-destroy-range collector。

## 扩展指南

- 新增或修改指标时，先在 `InitGCWorkerMetrics` 中保持 Go 的完整描述符语义：名称、help、标签顺序和桶边界都应同步核对 `gc_worker.go`；不要为了通过 Rust 测试简化 Go 行为。
- 新 collector 仍应先构造到局部变量，再与相关槽位集中发布，避免部分初始化。若继续使用可变静态量，必须与 `PACKAGE_INIT_LOCK` 和 `InitMetrics` 的一次性初始化约束一致。
- 若目标是让指标真正对外采集，还需检查并按 Go 语义更新 `metrics.rs::RegisterMetrics`，并检查实际 GC Worker 写入点；仅新增槽位和描述符测试不构成完整接线。
- 接入上游 unsafe-destroy-range collector 时，最可能修改 `init_gc_unsafe_destroy_range_metric` 及启动初始化调用点。需要验证传入 collector 的全名和 `type` 标签，并避免与 `InitGCWorkerMetrics` 并发替换。
- 同步扩展独立测试 `pkg/metrics/gc_worker_test.rs`，覆盖描述符、标签、桶边界、初始化后非空，以及新增的注入/注册行为；Rust 单元测试不要内嵌回生产源文件。
- 兼容风险主要是 Prometheus 时间序列重命名、标签变化和重复注册；性能风险主要是无界标签值造成基数膨胀，以及重复初始化造成 collector 状态割裂。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/metrics/gc_worker.rs` 报告本文件 13 个符号。
- RustCodeGraph `node --file pkg/metrics/gc_worker.rs`：核对了七个静态槽位、`InitGCWorkerMetrics`、`StageTotal` 和注入函数的完整源码；图中 `InitGCWorkerMetrics` 的测试调用边指向 `gc_worker_test.rs`，并列出对各槽位和 Prometheus 选项的引用。
- RustCodeGraph `node InitGCWorkerMetrics`：同时显示 Go/Rust 两个定义；Go 图边显示 `metrics.go::InitMetrics` 调用 Go 版本。Rust 文件级查询及 `metrics.rs` 源码确认 Rust 总入口的直接调用。
- 已读 crate/模块与适配证据：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`、`pkg/metrics/bindinfo.rs`、`pkg/metrics/common/wrapper.rs`。
- 已读对照与测试：`pkg/metrics/gc_worker.go`、`pkg/metrics/gc_worker_test.rs`；另以全仓 Rust/Go 引用搜索核对生产调用、注册列表和缺失接线。
- `gc_worker_test.rs::gc_worker_metrics_match_go_descriptors_and_initialization` 验证七个 collector 的全名/变量标签及 `StageTotal`；`gc_histogram_uses_go_exponential_buckets` 验证 20 个桶、首桶 1 秒、末桶 524,288 秒。本文档任务不运行 Cargo，未重新执行这些单元测试。
