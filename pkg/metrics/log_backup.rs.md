# `pkg/metrics/log_backup.rs`

## 文件定位

本文件属于 `astersql-metrics` crate（`pkg/metrics/Cargo.toml`），由 crate 入口 `pkg/metrics/lib.rs` 以公开模块 `pub mod log_backup` 暴露。它只负责定义和构造日志备份（PiTR）观测指标，不负责推进 checkpoint、扫描 Region、访问外部存储或执行备份。

包级初始化链为 `pkg/metrics/metrics.rs::InitMetrics` 调用 `log_backup::InitLogBackupMetrics`，随后 `pkg/metrics/metrics.rs::RegisterMetrics` 将本文件的 10 个 collector 注册到默认 Prometheus registry。指标统一使用 `tidb` namespace 和 `log_backup` subsystem，因此最终名称形如 `tidb_log_backup_last_checkpoint`。

## 核心职责

`InitLogBackupMetrics` 一次构造三组指标：任务 checkpoint 水位、advancer 的所有权/循环行为，以及 Region checkpoint 请求和订阅事件。具体包括 5 个 gauge（其中 2 个带 `task` 标签）、3 个 histogram vector、2 个 counter vector。

文件保留 Go 包级变量的公开命名和初始化顺序，便于迁移代码按原名访问；Rust 侧以 `Option<T>` 表达初始化前尚不存在，以 `static mut` 保留可替换的包级全局形状。它没有自行注册指标，也没有更新任何样本值。

## 主要符号

- `LastCheckpoint: Option<GaugeVec>`：每个 `task` 最近一次全局 checkpoint。
- `ExternalStorageCheckpoint: Option<GaugeVec>`：每个 `task` 已成功持久化到外部存储的全局 checkpoint。
- `AdvancerOwner: Option<Gauge>`：本节点是否为 advancer owner，约定 1/0 表示是/否。
- `AdvancerTickDuration: Option<HistogramVec>`：按 `step` 记录一次 advancer tick 各步骤的秒级耗时；桶由 `ExponentialBuckets(0.01, 3.0, 8)` 生成，即从 0.01 秒开始、倍率 3、共 8 桶。
- `GetCheckpointBatchSize: Option<HistogramVec>`：按 `type` 记录扫描 Region 或获取 checkpoint 的批大小；桶从 1 开始、倍率 2、共 12 桶。
- `RegionCheckpointRequest: Option<CounterVec>`：按 `result` 累计 Region checkpoint 请求成功或失败次数。
- `RegionCheckpointFailure: Option<CounterVec>`：按 `reason` 累计请求失败原因。
- `RegionCheckpointSubscriptionEvent: Option<HistogramVec>`：按 `store` 记录 Region flush/订阅事件规模；桶从 8 开始、倍率 2、共 12 桶。
- `LogBackupCurrentLastRegionID: Option<Gauge>`：当前任务中 checkpoint 最小的 Region ID。
- `LogBackupCurrentLastRegionLeaderStoreID: Option<Gauge>`：上述 Region leader 所在的 store ID。
- `unsafe fn InitLogBackupMetrics()`：本文件唯一函数；在包级锁保护下依次构造并覆盖上述全局量。

这些符号均为公开 API；文件中没有类型、trait、`impl` 或条件编译分支。

## 执行流程

1. 调用者进入 `InitLogBackupMetrics` 后先获取 `crate::metrics::PACKAGE_INIT_LOCK`；锁中毒会立即 panic。
2. 函数按 Go 版本顺序构造两个 task checkpoint gauge vector 和 owner gauge。
3. 构造 advancer tick 耗时、批大小、请求结果、失败原因和订阅事件指标，并固定各自标签维度及 histogram 桶。
4. 构造最小 checkpoint Region 及其 leader store 的两个无变量标签 gauge。
5. 每个构造结果写入相应 `static mut Option`，替换此前值；函数返回时释放互斥锁。
6. 正常应用入口由 `InitMetrics` 的 `Once` 保证整套初始化只执行一次；之后 `RegisterMetrics` 通过 `register_options!` 要求每个 `Option` 已为 `Some`，克隆并注册 collector。

本函数本身不调用 `RegisterMetrics`。绕过 `InitMetrics` 直接重复调用虽然由锁串行化，但会替换 collector，可能使已注册或被其他代码持有的旧 clone 与新全局量分离，因此不属于安全的常规生命周期。

## 数据与状态

全部运行时状态是 10 个进程级 `static mut Option<collector>`。初值均为 `None`，初始化后为 `Some`。vector 指标的标签集合是其时序基数契约：`task`、`step`、`type`、`result`、`reason`、`store` 分别只有一个变量标签；两个 Region ID gauge 和 owner gauge 没有变量标签。

Gauge 保存可升可降的当前值，Counter 只适合累计事件，Histogram 将观察值累计进预设桶。checkpoint、Region ID 和 store ID 在 Prometheus 中都以 `f64` gauge 表示；当原始整数超过 `f64` 的精确整数范围时存在精度风险，本文件没有额外转换或校验逻辑。

`PACKAGE_INIT_LOCK` 只保护构造时对全局 `Option` 的写入。collector 自身的样本并发安全由 `prometheus` crate 提供；本文件没有额外缓存、通道、任务、事务或文件句柄。

## 依赖与调用关系

上游直接调用边是 `pkg/metrics/metrics.rs::InitMetrics → pkg/metrics/log_backup.rs::InitLogBackupMetrics`。`InitMetrics` 通过 `INIT_METRICS_ONCE` 复用首次结果；注册边是 `RegisterMetrics → register_options! → register_option → register_clone → prometheus::register`，列表中包含本文件全部 10 个静态量。

构造路径为 `InitLogBackupMetrics → crate::bindinfo::compat_metricscommon::{NewGauge, NewGaugeVec, NewHistogramVec, NewCounterVec}`。兼容层在 `pkg/metrics/bindinfo.rs` 中把 Go 形状的 opts 转成上游 `prometheus` crate 的 opts，再委托 `astersql-metrics-common` 工厂。`ExponentialBuckets` 则直接委托 `prometheus::exponential_buckets`。

`pkg/metrics/Cargo.toml` 声明了直接依赖 `prometheus = "0.14"` 和路径依赖 `astersql-metrics-common`；本文件通过兼容层间接使用二者。全仓 Rust 引用搜索只找到初始化/注册中枢和初始化冒烟测试，未找到 `br/**/*.rs` 对这些全局指标的样本写入，因此当前可证实的 Rust 状态是“定义并注册”，不能据此宣称 Rust 日志备份执行链已经上报这些指标。

## 错误处理与边界

`InitLogBackupMetrics` 没有 `Result` 返回值。获取 `PACKAGE_INIT_LOCK` 使用 `expect("metrics init lock poisoned")`；兼容构造器和指数桶构造也以 `expect` 处理非法 metadata/桶配置，所以失败表现为 panic，而非向调用者传播可恢复错误。

注册阶段不在本文件内：若未初始化就调用 `RegisterMetrics`，`register_option` 会以 `expect("InitMetrics must run before RegisterMetrics")` panic；若 registry 拒绝重复或冲突 collector，`RegisterMetrics` 返回 `prometheus::Error`。这些边界由 `pkg/metrics/metrics.rs` 定义。

本文件不限制标签值集合，也不控制时序清理。调用方若给 `task`、`reason` 或 `store` 等标签传入无界高基数字符串，会增加内存和导出开销；任务结束时是否删除带标签时序属于业务调用方职责。

## 并发与资源生命周期

常规生命周期应是：进程内首次 `InitMetrics` 在 `Once` 中构造 collector，随后调用 `RegisterMetrics` 注册 clone，业务线程再更新样本。`PACKAGE_INIT_LOCK` 使多个子模块的 `static mut` 初始化串行，但读取公开 `static mut` 仍要求调用方使用 `unsafe` 并自行满足“不与替换写入并发”的不变量。

本文件没有析构或注销逻辑。注册后的 collector 由 Prometheus registry 持有；vector 内动态创建的标签时序需要调用方通过兼容 trait 的 `DeleteLabelValues`/`Reset` 管理。Go 侧 `br/pkg/streamhelper/advancer.go` 会在任务移除时删除两个 checkpoint vector 的 task label，`advancer_daemon.go` 在失去 owner 时重置相关 vector；当前 Rust BR 文件中未检索到对应指标调用，迁移时必须补足并发与清理语义，而不能只写入数值。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metrics/log_backup.go`。Rust 与 Go 都声明相同的 10 个指标，使用相同 namespace、subsystem、name、help、变量标签、桶参数和初始化顺序。Go 的 `ConstLabels: map[string]string{}` 在 Rust 兼容层中体现为构造 opts 时注入空 const-label map，语义仍是“没有固定标签”。

主要语言差异是：Go 包级变量在 `init()` 调用 `InitMetrics` 后直接持有 collector；Rust 用 `Option` 表达前置初始化状态，用 `unsafe` 标记全局可变访问，并由 `Once` 与 `Mutex` 补上初始化时序约束。Go `RegisterMetrics` 使用 `prometheus.MustRegister`，注册错误 panic；Rust `RegisterMetrics` 对 registry 错误返回 `Result`，但对未初始化 `Option` 仍 panic。

Go 业务侧直接证据包括：`br/pkg/streamhelper/advancer.go` 更新两个 checkpoint、tick duration 和两个最小 Region gauge，并在任务移除时删除 task labels；`collector.go` 更新批大小、请求结果及失败原因；`regioniter.go` 更新重试失败原因；`flush_subscriber.go` 观察订阅事件；`advancer_daemon.go` 设置 owner 并在退出时清理。Rust 同路径业务文件目前没有这些指标名的引用，属于迁移覆盖差异，而不是本指标定义文件可单独补齐的行为。

## 扩展指南

新增或修改日志备份指标时，应同步调整 `InitLogBackupMetrics` 的静态声明、metadata/标签/桶，以及 `pkg/metrics/metrics.rs::RegisterMetrics` 的注册列表；同时对照修改 `pkg/metrics/log_backup.go`，除非任务明确改变跨语言兼容契约。新增标签前需评估基数，修改指标名、类型、标签或桶边界属于监控查询和告警面板的兼容性变化。

行为接线应放在真正产生事件的 BR Rust 模块，而不是在本文件模拟数据。移植 Go 写入点时至少核对 `br/pkg/streamhelper/{advancer,advancer_daemon,collector,regioniter,flush_subscriber}.go`，并在对应独立 Rust 测试文件中验证更新值、标签、失败分类、owner 退出和任务删除后的清理。依照仓库约束，测试不可嵌入 `log_backup.rs`。

现有 `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata` 只冒烟调用初始化函数，未验证本文件的完整指标名、标签、桶和值。安全扩展宜新增同目录独立 `log_backup_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 区域挂载；若增加新的顶层测试函数或改动 Go import，还需按仓库 Build Flow 判断并运行 `make bazel_prepare`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/metrics` 确认目标、Go 对照、crate 入口和测试文件；`node --file pkg/metrics/log_backup.rs --offset 1 --limit 220` 读取完整 165 行并报告其被 `pkg/metrics/metrics.rs` 与 `pkg/metrics/bindinfo_1_aster_unit_test.rs` 使用；`query LOG_BACKUP --limit 20` 定位 Rust/Go 同名初始化函数。精确 `callers/callees InitLogBackupMetrics` 在 30 秒内未产生结果，故调用边由下列源码引用直接核验。
- Rust 源码：`pkg/metrics/log_backup.rs`（10 个静态量及唯一初始化函数）、`pkg/metrics/metrics.rs`（初始化锁、`Once`、初始化与注册链）、`pkg/metrics/bindinfo.rs`（兼容 opts、构造器、标签操作）、`pkg/metrics/lib.rs`（公开模块和测试挂载）。目标包没有 `doc.go`。
- crate 边界：`pkg/metrics/Cargo.toml`（crate 名、lib 入口、`prometheus` 与 `astersql-metrics-common` 依赖）。
- Go 对照与业务证据：`pkg/metrics/log_backup.go`、`pkg/metrics/metrics.go`，以及 `br/pkg/streamhelper/advancer.go`、`advancer_daemon.go`、`collector.go`、`regioniter.go`、`flush_subscriber.go`。
- 测试证据：`pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata` 覆盖初始化冒烟；`br/pkg/streamhelper/advancer_test.go` 验证外部存储 checkpoint 等 Go 侧业务写入。全仓检索未发现同名 Rust 专测或 Rust BR 业务写入引用。
- 本任务是纯文档分析，依照计划不运行 Cargo；交付前以任务指定命令验证本文恰有 11 个固定二级章节，并人工检查每项职责、调用边和迁移差异均有上述直接证据。
