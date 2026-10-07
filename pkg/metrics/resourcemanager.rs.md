# `pkg/metrics/resourcemanager.rs`

## 文件定位

本文件属于 `astersql-metrics` crate；crate 的清单是 `pkg/metrics/Cargo.toml`，库入口 `pkg/metrics/lib.rs` 通过 `pub mod resourcemanager` 公开该模块。它是资源管理器观测指标的定义与初始化单元，只创建 Prometheus 句柄，不采样 CPU、不管理线程池，也不自行注册 collector。

包级中枢 `pkg/metrics/metrics.rs::InitMetrics` 调用 `resourcemanager::InitResourceManagerMetrics`，随后 `RegisterMetrics` 才把本文件的两个 collector 注册到默认 Prometheus registry。因此本文件位于“指标声明/构造”和“业务侧更新指标”之间，而不是资源调度主链本身。

## 核心职责

- `InitResourceManagerMetrics` 构造资源管理器的两个指标句柄，并写入包级全局量。
- `EMACPUUsageGauge` 表示 CPU 使用率的指数移动平均值，对外暴露的完整指标名由 namespace、subsystem 和 name 拼成 `tidb_rm_ema_cpu_usage`。
- `PoolConcurrencyCounter` 表示资源池并发度，完整指标名为 `tidb_rm_pool_concurrency`，并声明一个名为 `type` 的可变标签（来自 crate 根经 `session.rs::LblType` 再导出的常量）。虽然名称保留了 `Counter`，实际类型和语义都是可增可减/可直接设值的 `GaugeVec`。

该文件明确不负责 Prometheus 注册、HTTP 暴露、CPU EMA 算法或线程池容量变更；这些行为分别位于 `pkg/metrics/metrics.rs`、指标服务外围、`pkg/util/cpu/cpu.rs` 和资源池实现中。

## 主要符号

- `pub static mut EMACPUUsageGauge: Option<prometheus::Gauge>`：初始化前为 `None`，初始化后保存无可变标签的 gauge。`Option` 对应 Go 包级变量在初始化前没有有效句柄的状态。
- `pub static mut PoolConcurrencyCounter: Option<prometheus::GaugeVec>`：初始化前为 `None`，初始化后保存带单个 `type` 标签的 gauge vector；使用者必须提供恰好一个标签值才能取得具体 gauge。
- `pub fn InitResourceManagerMetrics()`：唯一函数入口。它在一个 `unsafe` 块中顺序替换上述两个全局 `Option`，不返回结果。
- `metricscommon::NewGauge` / `NewGaugeVec`：实际构造器，来源于 `pkg/metrics/common/wrapper.rs` 并经兼容层导入。构造前会用包级常量标签替换 `GaugeOpts` 中的 `const_labels`；无效 descriptor 会在构造器内部 `expect` 并 panic。

本文件没有类型、trait、impl、条件编译项或私有辅助函数。

## 执行流程

1. 应用侧进入 `pkg/metrics/metrics.rs::InitMetrics`。该函数以 `Once` 保证整包初始化只执行一次，并在初始化闭包中调用 `InitResourceManagerMetrics`。
2. `InitResourceManagerMetrics` 先构造 EMA gauge：namespace=`tidb`、subsystem=`rm`、name=`ema_cpu_usage`、help=`exponential moving average of CPU usage`，再写入 `EMACPUUsageGauge`。
3. 它随后构造资源池并发 gauge vector：namespace=`tidb`、subsystem=`rm`、name=`pool_concurrency`、help=`How many concurrency in the pool`，变量标签数组为 `[LblType]`，再写入 `PoolConcurrencyCounter`。
4. `pkg/metrics/metrics.rs::RegisterMetrics` 通过 `register_options!` 读取这两个 `Option`、克隆 collector 并注册。若尚未初始化，注册辅助函数会以 `expect("InitMetrics must run before RegisterMetrics")` panic；registry 拒绝注册时则返回 `prometheus::Error`。
5. 运行期间，`pkg/util/cpu/cpu.rs::CPUObserver::Start` 创建的后台线程约每 100 ms 更新 EMA，并在 `EMACPUUsageGauge` 已初始化时调用 `set(usage)`。未初始化时该分支静默跳过指标写入，但 CPU 状态仍会更新。

## 数据与状态

两个全局量都是进程级、可替换的 `static mut Option<...>`。初始状态为 `None`；成功执行初始化后为 `Some(collector)`。函数每次调用都会构造新 collector 并覆盖旧值，但正常入口 `metrics.rs::InitMetrics` 的 `Once` 将生产路径限制为一次初始化。

指标元数据是稳定契约：EMA 无变量标签；并发度有且仅有 `type` 标签。`metricscommon` 构造器还会注入当时的包级常量标签快照，因此必须在初始化前完成常量标签配置；初始化后再修改包级配置不会重写已经构造的 descriptor。

本文件自身不保存 EMA 历史，也不计算池容量。EMA 数值保存在 CPU observer 状态中并被复制到 gauge；线程池容量属于资源池对象。collector 被默认 registry 克隆注册后，与全局句柄共享 Prometheus 内部状态，业务侧更新句柄即可反映到采集结果。

## 依赖与调用关系

上游直接调用边只有 `pkg/metrics/metrics.rs::InitMetrics -> pkg/metrics/resourcemanager.rs::InitResourceManagerMetrics`；RustCodeGraph 对该符号给出的调用者也是 `InitMetrics`。同一中枢的 `RegisterMetrics` 直接引用 `EMACPUUsageGauge` 与 `PoolConcurrencyCounter` 完成注册。

下游依赖包括：

- `crate::bindinfo::compat_metricscommon`：提供与 Go 风格名称兼容的 `NewGauge`、`NewGaugeVec` 工厂。
- `crate::bindinfo::compat_prometheus`：提供 `Gauge`、`GaugeVec`、`GaugeOpts` 及兼容 trait，使机械移植的字段名和方法调用可用。
- `crate::*`：取得 crate 根再导出的 `LblType` 等包级符号；`LblType` 的值为 `"type"`。
- `pkg/util/cpu/cpu.rs`：Rust 中已确认的 EMA 指标业务写入者。

`pkg/metrics/Cargo.toml` 声明 crate 名为 `astersql-metrics`，直接依赖 `prometheus = "0.14"` 和本地 `astersql-metrics-common`。清单没有控制本模块的 feature，模块在常规构建中无条件编译。

## 错误处理与边界

`InitResourceManagerMetrics` 没有 `Result` 返回值。指标选项无效时，`NewGauge` 或 `NewGaugeVec` 在 `pkg/metrics/common/wrapper.rs` 中通过 `expect` panic；这些静态名称与标签当前是固定值，所以正常配置下不应失败。注册错误不在本文件处理，而由 `metrics.rs::RegisterMetrics` 向上传播。

调用顺序是重要边界：注册前必须完成初始化，否则 `register_option` 会 panic。反过来，CPU observer 对未初始化句柄采取容错策略：它检查 `Option`，为 `None` 时不写 gauge，不会因此终止采样线程。

`PoolConcurrencyCounter` 要求一个 `type` 标签；标签数量不匹配会触发 Prometheus `GaugeVec` 的使用错误。目标文件本身没有调用 `with_label_values`，所以标签值的命名与基数控制属于消费方责任。

## 并发与资源生命周期

本文件的 `static mut` 不提供自身同步，直接并发调用初始化或在覆盖过程中读取句柄会形成不安全访问风险。受支持的包级路径由 `metrics.rs` 的 `INIT_METRICS_ONCE` 串行化且只执行一次；该文件还定义了 `PACKAGE_INIT_LOCK` 供包内初始化场景协调。扩展者不应绕过中枢反复调用初始化，也不应新增无同步的运行期替换。

EMA 的并发写入发生在 `CPUObserver::Start` 启动的 worker 中；worker 持有 observer 状态锁计算数值，然后通过 Prometheus gauge 的线程安全句柄更新。`CPUObserver::Stop` 关闭通道并 `join` worker，决定写入者的生命周期；collector 自身则作为包级句柄/registry 条目存活到进程结束。

本文件不创建线程、锁、通道、事务或网络连接，也没有显式清理函数。Prometheus collector 的所有权由全局 `Option` 与注册表内部克隆共同持有。

## 与 Go 版本的对应关系

`pkg/metrics/resourcemanager.go` 与本文件在两个指标的 namespace、subsystem、name、help、标签数量及初始化顺序上逐项一致。Go 使用非空类型的包级变量，Rust 用 `Option` 显式表达初始化前状态；Go 的 `*prometheus.GaugeVec` 对应 Rust 的 `prometheus::GaugeVec` 值。两侧都把并发度声明为 gauge vector，尽管变量名含 `Counter`。

初始化与注册链也保持一致：Go 的 `pkg/metrics/metrics.go` 调用 `InitResourceManagerMetrics`，并在 `RegisterMetrics` 中注册两个句柄；Rust 的 `pkg/metrics/metrics.rs` 做同样接线，但额外用 `Once` 和错误缓存约束初始化。

消费侧仍存在需要注意的迁移差异。Go 的 `pkg/resourcemanager/pool/spool/spool.go::NewPool` 使用 `metrics.PoolConcurrencyCounter.WithLabelValues(name)`，并在创建和 `Tune` 时更新它；当前 Rust `pkg/resourcemanager/pool/spool/spool.rs::Pool::new` 没有引用本文件的 `PoolConcurrencyCounter`，而是自行创建名为 `tidb_rm_pool_concurrency`、带常量标签 `pool=<name>` 的 gauge。因而不能声称 Rust 全局 `PoolConcurrencyCounter` 已接入线程池更新；这也是扩展或对齐时需要优先核查的兼容点。

## 扩展指南

- 新增资源管理器指标时，在本文件增加独立句柄和构造逻辑，并同步把句柄加入 `pkg/metrics/metrics.rs::RegisterMetrics`；只构造而不注册不会出现在默认采集结果中。
- 修改现有名称、help 或标签会改变监控契约，需同步核查 dashboard、告警和消费方。尤其不要在高基数数据上直接增加标签。
- 若接通线程池并发指标，应先决定是复用本文件的 `GaugeVec(type)` 还是保留 spool 的本地 `Gauge(pool)`，避免在同一 registry 注册同名但 descriptor 不一致的 collector；同时对齐 `Pool::new`、`Pool::tune` 和释放后的状态语义。
- 若改变初始化模型，应优先消除或封装 `static mut`，并保持 `InitMetrics`/`RegisterMetrics` 的先后契约；不要让业务线程看到替换中的句柄。
- 测试必须放在独立测试文件而非本源文件中。可扩展 `pkg/metrics/metrics_test.rs` 验证初始化/注册后两个 metric family、help 与标签；EMA 更新行为放在 `pkg/util/cpu/cpu_test.rs`；线程池创建与调容更新行为放在 `pkg/resourcemanager/pool/spool/spool_test.rs`。Go 对照测试入口是 `pkg/metrics/metrics_test.go` 及相应 CPU/资源池测试。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件；目标文件被识别为 61 行、2 个符号，并显示由 `pkg/metrics/metrics.rs` 使用。
- RustCodeGraph 符号/调用证据：`InitResourceManagerMetrics` 位于 `pkg/metrics/resourcemanager.rs:39`，其调用者为 `pkg/metrics/metrics.rs::InitMetrics`；文件读取确认 `EMACPUUsageGauge`、`PoolConcurrencyCounter` 的定义和构造参数。
- Rust 源码证据：`pkg/metrics/lib.rs`（模块公开与 `LblType` 再导出）、`pkg/metrics/metrics.rs`（初始化和注册）、`pkg/metrics/common/wrapper.rs`（常量标签及 panic 语义）、`pkg/util/cpu/cpu.rs`（EMA 写入和 worker 生命周期）、`pkg/resourcemanager/pool/spool/spool.rs`（当前独立并发 gauge）。
- crate 证据：`pkg/metrics/Cargo.toml` 的 `[lib]`、Prometheus 与 metrics-common 依赖，以及 `go-package = "pkg/metrics"` 移植元数据。
- Go 对照证据：`pkg/metrics/resourcemanager.go`、`pkg/metrics/metrics.go`、`pkg/resourcemanager/pool/spool/spool.go`、`pkg/util/cpu/cpu.go`。
- 测试证据：`pkg/metrics/metrics_test.rs` 覆盖整包 `InitMetrics`/`RegisterMetrics` 冒烟路径；`pkg/util/cpu/cpu_test.rs` 覆盖 observer 启停与采样；`pkg/resourcemanager/pool/spool/spool_test.rs` 覆盖创建、调容和并发行为。检索未发现直接按 `EMACPUUsageGauge`、`PoolConcurrencyCounter` 或两个完整指标名断言的 Rust 专项测试，因此指标值与标签契约目前只有间接覆盖。
