# `pkg/metrics/metrics.rs`

## 文件定位

[`metrics.rs`](metrics.rs) 是 `astersql-metrics` crate 的包级指标中枢。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod metrics` 暴露本模块，并把 DDL、DistSQL、Domain、Executor、Session、Server、Telemetry、TiKV client 等指标模块汇总在同一 crate 内。该 crate 的边界和依赖由 [`Cargo.toml`](Cargo.toml) 定义：核心依赖是 `prometheus`，channelz 实现使用 `grpcio`，并直接连接 DXF、ingestor、timer、intest、promutil 等本地 crate。

它负责四类工作：定义跨子系统共用的标签和两个包级 collector；按固定顺序初始化各子系统 collector；把选定 collector 注册到默认 Prometheus registry；管理简化模式和进程级 gRPC channelz collector。它不是每个指标的具体定义文件，实际指标大多位于同目录的 `ddl.rs`、`server.rs`、`session.rs` 等模块。

当前 Rust 应用接线需要与模块能力区分开看。代码搜索显示，服务启动入口 [`cmd/tidb-server/main.rs`](../../cmd/tidb-server/main.rs) 调用的是 `pkg/util/metricsutil/common.rs::RegisterMetrics`；该函数初始化拆分到多个 owner crate 的父 collector，并未直接调用本文件的 `InitMetrics` 或 `RegisterMetrics`。本文件的这两个入口目前主要由 metrics/server 测试环境调用。另一方面，生产 HTTP 指标响应 [`pkg/server/http_status.rs`](../server/http_status.rs) 直接调用本文件的 `GatherText` 采集默认 registry。`ToggleSimplifiedMode` 未检出 Rust 生产调用者，因此不能把 Go 侧系统变量接线当作 Rust 已有事实。

## 核心职责

1. `PanicCounter` 与 `MemoryUsage` 用 `LazyLock` 构造包级 `CounterVec`/`GaugeVec`，保留 Go 指标名、namespace、subsystem 和标签定义。
2. `InitMetrics` 一次性调用各子模块的 `Init*Metrics`，并初始化 DXF、ingestor、timer 等外部 owner 的指标状态。
3. `RegisterMetrics` 把本 crate 选定的 collector、外部 owner collector、TiKV client collector 和 channelz collector 注册到默认 registry。
4. `GatherText` 把默认 registry 的快照编码为 Prometheus text exposition format，供 HTTP 状态服务返回。
5. `ToggleSimplifiedMode` 在完整模式与简化模式之间切换，注销或重新注册 Grafana 不使用的一组高开销 collector。
6. `ChannelzState` 及其辅助函数保证 channelz collector 只创建、注册一个实例，并在测试中提供可控清理。

## 主要符号

- `PACKAGE_INIT_LOCK: Mutex<()>`：供同 crate 各子系统的初始化函数串行化对 `static mut Option<Collector>` 的赋值；本文件声明锁，具体加锁发生在子模块。
- `INIT_METRICS_ONCE`、`INIT_METRICS_DONE`、`INIT_METRICS_ERROR`：分别控制一次执行、发布成功状态、缓存首次失败字符串。组合后使并发调用者复用首次结果。
- `PanicCounter: LazyLock<CounterVec>`：`tidb_server_panic_total{type=...}`；`MemoryUsage: LazyLock<GaugeVec>`：`tidb_server_memory_usage{module=...,type=...}`。
- `LabelSession` 至 `TiKVClient`：与 Go `metrics.go` 对齐的模块名、scope、成功/失败等标签常量；`OP_SUCC`/`OP_FAILED` 仅 crate 内可见。
- `RetLabel<E>(Option<&E>) -> &'static str`：无错误返回 `"ok"`，有错误返回 `"err"`；泛型参数只表达错误是否存在，不读取错误内容。
- `InitMetrics() -> Result<(), prometheus::Error>`：一次性初始化入口。它是 `unsafe fn`，因为下游迁移代码仍使用包级 `static mut Option<Collector>`。
- `register_clone`、`register_option`、`register_options!`：内部注册适配层。前者克隆 collector 后注册；后两者要求对应 `Option` 已被初始化，并用宏展开长注册表。
- `RegisterMetrics() -> Result<(), prometheus::Error>`：默认 registry 的集中注册入口；先注册本地 collector，再注册外部和 TiKV collector，最后尝试安装 channelz。
- `register_external_metrics(&Registry)`：支持向指定 registry 注册 DXF、ingestor、timer collector，便于隔离测试。
- `Register(Vec<Box<dyn Collector>>)` / `Unregister(Vec<Box<dyn Collector>>)`：批量注册/注销任意 collector；前者遇错立即返回，后者忽略单项注销失败。
- `GatherText() -> Result<(Vec<u8>, String), prometheus::Error>`：返回编码后的 body 和 encoder 的 content type。
- `ToggleSimplifiedMode(bool)`：由 `MODE: Mutex<bool>` 保护的幂等模式切换。
- `ChannelzState { collector, registered }`、`GRPC_CHANNELZ_COLLECTOR`：channelz collector 的进程级单例及注册状态。
- `init_grpc_channelz_collector_locked`、`setup_channelz_collector`、`stop_grpc_channelz_collector_locked`：要求调用方持锁的创建、生产安装与清理流程。
- `is_internal_channelz_target` / `is_internal_channelz_socket`：过滤 collector 自己的 `bufnet` 通道与无 remote 的内部 socket。
- `collect_channelz_snapshots_for_test`、`GRPC_CHANNELZ_TEST_LOCK`、`cleanup_grpc_channelz_collector_for_test`、`with_grpc_channelz_collector_locked`：仅测试或以测试辅助为目的的接口；测试逻辑仍位于独立测试文件中。

## 执行流程

初始化流程从 `InitMetrics` 开始。`Once::call_once` 只让第一个调用者执行闭包；闭包严格按源文件顺序调用 bindinfo、DDL、DistSQL、domain、executor、GC、日志备份、meta、owner、RawKV、资源管理、server、session、RU v2、SLI、stats、telemetry、TopSQL、TTL、外部工作负载、语句摘要、DXF、ingestor、资源组、global sort、InfoSchema、memory、timer 和 BR 初始化。其中 `InitTelemetryMetrics()?` 是当前序列中显式传播 `prometheus::Error` 的步骤。成功时以 `SeqCst` 写入完成标志；失败时把错误转为字符串存入互斥量。所有调用者随后读取同一结果，失败不会自动重试。

注册流程要求初始化已经完成。`RegisterMetrics` 先注册 `PanicCounter`、`MemoryUsage`，再由 `register_options!` 依序取出大量 `static mut Option` collector。任何未初始化项都会触发 `expect("InitMetrics must run before RegisterMetrics")`；注册重名等 Prometheus 错误通过 `?` 返回。列表完成后，它调用 `register_external_metrics(default_registry)`、`tikv_client_metrics::RegisterMetrics(default_registry)`，最后调用 `setup_channelz_collector`。

channelz 安装先判断 `cfg!(test)` 或 `astersql_util_intest::InTest`，测试环境直接成功返回。生产环境锁住单例；若还没有 collector，则通过 `ChannelzCollector::new()` 创建。创建失败按 Go 的 best-effort 语义吞掉并返回 `Ok(())`；创建成功且尚未注册时才注册并置 `registered = true`。重复调用不会重复创建或注册。

简化模式切换先锁住 `MODE`，相同目标状态立即返回。切到简化模式时，逐项注销 session/server/SLI/domain/owner 的选定指标、`MemoryUsage` 和 `tikv_client_metrics::unused_collectors()`；注销结果被忽略。切回完整模式时按同一集合逐项注册，首个错误立即返回。状态值在实际注册/注销之前更新，因此恢复过程若中途失败，`MODE` 已是 `false`，再次以 `false` 调用会被幂等分支跳过，这是扩展或修复时必须保留意识的现有边界。

导出流程由 `GatherText` 调用 `prometheus::gather()` 取得默认 registry 快照，再由 `TextEncoder` 写入字节缓冲区；编码错误向上返回。`pkg/server/http_status.rs::metrics_response` 将成功结果作为 HTTP 200 body，并设置 content type；失败则返回 HTTP 500 文本。

## 数据与状态

指标描述符和实际时序数据由 `prometheus` collector 持有；本模块主要保存 collector 句柄与注册状态。`LazyLock` 保证 `PanicCounter`、`MemoryUsage` 和 channelz 状态容器按需构造。大量子系统 collector 仍是其他模块里的 `static mut Option<T>`，所以 `InitMetrics`/`RegisterMetrics` 标为 `unsafe`，初始化顺序是重要不变量。

初始化状态是一台不可回退的状态机：初始为“未运行”，第一次闭包后成为“成功”或“失败”；`Once` 不允许失败后重试。错误只缓存 `to_string()`，后续调用构造新的 `prometheus::Error::Msg`，不保留原错误类型。

简化模式状态由 `MODE` 独立维护，不读取 registry 判断真实状态；外部若绕过本模块注册或注销同一 collector，布尔值可能与 registry 实际状态不同。channelz 则同时保存 `Option<ChannelzCollector>` 和 `registered`，并在同一个 mutex 临界区内维护二者。

标签常量是跨模块协议。更改 `"ok"`/`"err"`、模块名、scope 或 namespace 会改变时序标签或指标全名，可能造成仪表盘查询不兼容。

## 依赖与调用关系

上游方面，`lib.rs` 公开 `metrics` 模块。RustCodeGraph 将目标文件识别为 58 个符号，并显示它被 `pkg/metrics/executor.rs`、`pkg/metrics/memory.rs` 以及若干执行/导入模块引用。精确源码搜索补充了图工具未能输出的调用点：`GatherText` 的生产调用者是 `pkg/server/http_status.rs::metrics_response`；`InitMetrics`/`RegisterMetrics` 的直接 Rust 调用集中在 `pkg/metrics/*_test.rs`、`pkg/server/*/main_test.rs` 等测试初始化；未检出 `ToggleSimplifiedMode` 的 Rust 生产调用者。

服务进程启动链是 `cmd/tidb-server/main.rs::runServer`（附近启动逻辑）调用 `pkg/util/metricsutil/common.rs::RegisterMetrics`。后者的 `registerMetrics -> initMetrics -> initParentMetricsCollectors` 初始化拆分 owner crate 的 collector。这条链解释了为什么不能仅凭 Go 的包 `init()` 就断言本文件 `InitMetrics` 已在 Rust 生产启动时执行。

下游方面，`InitMetrics` 调用本 crate 几乎全部指标子模块，以及 `astersql-dxf-framework-dxfmetric`、`astersql-ingestor-ingestmetric`、`astersql-timer-metrics`。`RegisterMetrics` 依赖 `prometheus` 默认 registry、上述外部 crate、`tikv_client_metrics` 和 `channelz`。`GatherText` 依赖 `prometheus::gather` 与 `TextEncoder`。channelz 的测试/生产环境判断依赖 `astersql-util-intest`。

## 错误处理与边界

- collector 构造中的静态描述符错误使用 `expect`，因为名称和标签被视为编译期固定配置；若配置非法，会在首次访问时 panic。
- `InitMetrics` 只允许一次尝试；首次错误会永久缓存。锁 poison 通过 `unwrap` 触发 panic，不做恢复。
- `RegisterMetrics` 要求先初始化，未初始化的 `Option` 会 panic；重复注册或描述符冲突以 `prometheus::Error` 返回。调用方必须保证默认 registry 的隔离与调用顺序。
- `register_external_metrics` 对 ingestor/timer 未初始化使用 `expect`；它对 poisoned timer 读锁使用 `into_inner()` 恢复，但 ingestor 读锁 poison 会 panic，二者策略并不完全一致。
- `Register` 发生错误时停止，之前已成功注册的 collector 不回滚。`Unregister` 和简化模式的注销分支均忽略失败。
- `ToggleSimplifiedMode` 的恢复分支不是事务性的，部分成功后出错不会回滚，且模式布尔值已提前更新。
- `setup_channelz_collector` 在测试环境完全跳过；创建失败被视为非致命，注册失败则向上传播。注册成功后重复调用幂等。
- `is_internal_channelz_target` 只匹配两个精确字符串；`is_internal_channelz_socket` 只在 remote 缺失且 remote name 为空时判定为内部 socket。
- `GatherText` 只采集默认 registry。注册在自定义 `Registry` 的 collector 不会出现在该 HTTP 输出中。

## 并发与资源生命周期

`INIT_METRICS_ONCE` 为整个初始化序列提供进程级一次性同步，`AtomicBool` 使用 `SeqCst` 发布成功结果，错误字符串由 mutex 保护。`PACKAGE_INIT_LOCK` 则服务于多个子模块的可变静态量写入，避免并发测试重入时替换仍被读取的 collector。

`MODE` 在整个简化模式注册/注销循环期间持锁，因此两个切换调用不会交错；代价是 Prometheus registry 操作也发生在临界区内。该锁不协调其他直接调用 `prometheus::register/unregister` 的代码。

channelz collector 的创建、注册、读取测试状态和清理都通过 `GRPC_CHANNELZ_COLLECTOR: Mutex<ChannelzState>` 串行化。`init_grpc_channelz_collector_locked` 的命名明确表达“调用方必须持锁”。测试另用 `GRPC_CHANNELZ_TEST_LOCK` 防止会修改进程级单例的测试并行执行。`stop_grpc_channelz_collector_locked` 注销已注册 collector 后把整个状态恢复默认值；底层资源的具体所有权和 Drop 行为位于 `channelz.rs::ChannelzCollector`，本文件只持有其克隆句柄。

## 与 Go 版本的对应关系

Rust 常量、`RetLabel`、初始化顺序、集中注册列表、简化模式指标集合和 channelz 过滤条件均以同路径 [`metrics.go`](metrics.go) 为语义基准。`PanicCounter`/`MemoryUsage` 在 Go 中由 `InitMetrics` 赋值，在 Rust 中改为 `LazyLock`，因此可在显式 `InitMetrics` 前访问；其描述符保持一致。

Go 通过包级 `init()` 自动调用 `InitMetrics`，且 `RegisterMetrics` 使用 `MustRegister`，冲突时 panic。Rust 没有对应包 `init()`；`InitMetrics` 是显式、一次性且返回 `Result` 的 `unsafe fn`，`RegisterMetrics` 也返回 `Result`。Rust 当前生产启动由拆分 crate 的 `metricsutil` 路径接线，不能视为 Go 包初始化的逐句等价调用。

Go `RegisterMetrics` 还替换 Go runtime collector；Rust 使用 rust-prometheus 的平台默认 process collector，不存在 Go runtime collector 替换步骤。Rust 额外提供 `GatherText` 供自身 HTTP server 编码默认 registry。

Go 简化模式恢复注册时记录错误并中断，函数无返回值；Rust 将首个注册错误返回给调用者。两者都先更新模式状态，且都不做事务回滚。Go 系统变量层存在 `metrics.ToggleSimplifiedMode(...)` 调用，而 Rust 搜索未发现对应生产接线。

Go channelz 状态显式拥有 bufconn listener、gRPC server 和 client connection，并在清理时逐项关闭；Rust 把这些细节封装进 `channelz.rs::ChannelzCollector`，本文件只管理 collector/registered。两边都跳过测试环境、复用单例、过滤自身 bufnet 通道和无 remote socket，并将初始化失败视为 best-effort。Go 会记录 warning，Rust 当前只丢弃该错误，没有日志输出。

## 扩展指南

新增包级指标时，先在所属独立模块定义并初始化 collector，再根据 Go 行为决定是否加入 `RegisterMetrics` 的 `register_options!` 列表。若指标属于 DXF/ingestor/timer 一类外部 owner，应修改 `register_external_metrics`，避免复制 collector 定义。所有新行为测试应放在独立的 `metrics_test.rs`、`metrics_internal_test.rs` 或新建独立测试文件，不要嵌入 `metrics.rs`。

新增或删除简化模式指标时，需要同时更新 `ToggleSimplifiedMode` 的本 crate 列表、`tikv_client_metrics::unused_collectors()`（若属于 TiKV client）和 Go `unusedMetricsByGrafana` 对照，并扩展 `metrics_internal_test.rs` 的描述符/注册往返测试。要特别验证恢复注册中途失败后的状态语义，避免 `MODE` 与 registry 漂移。

扩展初始化序列时，应保持 Go 顺序与所有前置依赖，确认新初始化是否可能返回错误，并评估“一次失败永久失败”的影响。新增 `static mut` collector 必须复用 `PACKAGE_INIT_LOCK`，不能让读线程观察到替换中的句柄。

修改 channelz 时，应在 `channelz.rs` 实现采集细节，在本文件维持单例与注册生命周期；同步覆盖重复初始化、测试环境跳过、cleanup、内部目标过滤和真实 leaf-subchannel/socket 快照。不要在并行测试中绕过 `GRPC_CHANNELZ_TEST_LOCK`。

若目标是让 Rust 生产路径直接使用本文件 `InitMetrics/RegisterMetrics` 或接通 `ToggleSimplifiedMode`，这属于跨 crate 接线变更，必须先审查 `pkg/util/metricsutil/common.rs` 现有拆分 owner 设计和重复注册风险，不能仅在本文件增加调用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录中的 `metrics.rs` 有 58 个符号；`node --file pkg/metrics/metrics.rs` 完整读取 1–631 行并报告目标被 17 个文件使用；精确 `query` 确认 `InitMetrics`、`RegisterMetrics`、`ToggleSimplifiedMode`、`GatherText`、`setup_channelz_collector` 的目标符号。`callers/callees` 对这些内部 ID 未输出可用边，因此调用者部分以精确 `rg` 和入口源码补证，未把空结果解释为“无调用者”。
- 源码与 crate 边界：`pkg/metrics/metrics.rs`、`pkg/metrics/lib.rs`、`pkg/metrics/Cargo.toml`。
- Rust 直接入口证据：`cmd/tidb-server/main.rs`、`pkg/util/metricsutil/common.rs`、`pkg/server/http_status.rs`。
- Go 对照：`pkg/metrics/metrics.go`，覆盖初始化/注册顺序、简化模式列表、channelz 生命周期和过滤条件。
- 独立 Rust 测试：`pkg/metrics/metrics_test.rs` 验证初始化注册与可采集指标；`pkg/metrics/metrics_internal_test.rs` 验证 `RetLabel`、外部 collector、channelz 单例/测试跳过/快照和 TiKV 简化 collector；`pkg/metrics/metrics_2_aster_unit_test.rs` 验证初始化成功边界与 channelz 内部目标判断。
- 本任务为纯文档分析，按计划不运行 Cargo；结构检查用于确认文件存在且恰好包含规定的十一个二级章节。
