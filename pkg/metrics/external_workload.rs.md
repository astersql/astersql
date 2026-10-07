# `pkg/metrics/external_workload.rs`

## 文件定位

本文件属于 `astersql-metrics` crate；`pkg/metrics/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/metrics/lib.rs` 通过公开模块 `external_workload` 暴露本文件。它负责定义外部工作负载管理器的 Prometheus 指标描述和动作标签，不实现任务调度或 RPC。

在指标包内部，`pkg/metrics/metrics.rs::InitMetrics` 调用 `InitExternalWorkloadMetrics` 创建 collector，随后 `RegisterMetrics` 把 `ExternalWorkloadTaskCounter` 注册到默认 Prometheus registry。当前 Rust 业务接线并未完整复刻 Go：`astersql-extworkload` 的 `Cargo.toml` 不依赖 `astersql-metrics`，`pkg/extworkload/manager.rs` 使用的是 `pkg/extworkload/lib.rs::metrics` 中同名的本地空操作桩。因此，本文件当前已接入指标初始化与暴露链，但外部工作负载 RPC 路径尚未向这里的真实 counter 打点。

## 核心职责

- 用 `WorkerActionInit`、`WorkerActionRegister`、`WorkerActionRecycle`、`WorkerActionAbort` 固定 worker 生命周期的 `action` 标签值，分别为 `init`、`register`、`recycle`、`abort`。
- 用 `ExternalWorkloadTaskCounter` 保存按 worker 类型和动作分组的 `prometheus::CounterVec`。
- 用 `InitExternalWorkloadMetrics` 构造指标 `tidb_external_workload_task_total`，并固定变量标签顺序为 `type`、`action`。
- 只负责 collector 的定义与初始化；注册由 `pkg/metrics/metrics.rs::RegisterMetrics` 统一完成，业务事件的标签注入和递增按 Go 设计应由外部工作负载管理器完成。

## 主要符号

- `pub const WorkerActionInit: &str = "init"`：GCV2 worker 初始化事件标签；Go 调用点是 `manager.InitializeGCV2`。
- `pub const WorkerActionRegister: &str = "register"`：GCV2、TTL 和 auto-analyze worker 注册事件标签。
- `pub const WorkerActionRecycle: &str = "recycle"`：上述 worker 回收/完成事件标签。
- `pub const WorkerActionAbort: &str = "abort"`：中止全部 GCV2 任务的事件标签。
- `pub static mut ExternalWorkloadTaskCounter: Option<prometheus::CounterVec>`：初始化前为 `None`，初始化后保存真实 collector。它是公开的可变全局状态，访问方必须遵守初始化先于读取、并自行满足 `unsafe` 访问约束。
- `pub fn InitExternalWorkloadMetrics()`：持有 `crate::metrics::PACKAGE_INIT_LOCK` 后构造并写入全局 counter。该函数本身不返回结果；无效指标描述会在兼容工厂内部 panic，锁中毒也会 panic。

本文件没有类型、trait、`impl` 或条件编译项。导入的 `CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat` trait 用于 Go 风格 Prometheus 兼容方法的统一可见性；本函数直接依赖的构造入口是 `compat_metricscommon::NewCounterVec`。

## 执行流程

1. `pkg/metrics/metrics.rs::InitMetrics` 由 `INIT_METRICS_ONCE` 保证进程内只执行一次，并在各子系统初始化序列中调用 `InitExternalWorkloadMetrics`。
2. `InitExternalWorkloadMetrics` 获取包级 `PACKAGE_INIT_LOCK`，使各个会写 `static mut` 的指标初始化函数串行执行。
3. 函数将 namespace、subsystem、name、help 分别设置为 `tidb`、`external_workload`、`task_total` 和对应帮助文本，再以 `[LblType, LblAction]` 构造 `CounterVec`。
4. `pkg/metrics/bindinfo.rs::compat_metricscommon::NewCounterVec` 把 Go 形状的选项转换为 `prometheus::Opts`；`pkg/metrics/common/wrapper.rs::NewCounterVec` 注入包级常量标签并调用 Rust `prometheus::CounterVec::new`。
5. 新 collector 在 `unsafe` 块中写入 `ExternalWorkloadTaskCounter`。
6. `pkg/metrics/metrics.rs::RegisterMetrics` 从该 `Option` 取出并克隆 collector，注册到默认 registry。若未先初始化，`register_option` 以 `InitMetrics must run before RegisterMetrics` panic。
7. Go 完整业务链在 RPC 前用 `withMetric` 写入 worker type/action，`metricsInterceptor` 读取标签并执行 `WithLabelValues(...).Inc()`，之后无论是否打点都调用 RPC invoker。Rust 管理器保留了同样的标签流与拦截器形状，但其同名 metrics 来自本地桩，尚未连接本文件。

## 数据与状态

`ExternalWorkloadTaskCounter` 的状态只有 `None` 和 `Some(CounterVec)`：静态初始化为 `None`，`InitExternalWorkloadMetrics` 用新实例整体替换为 `Some`。正常总入口通过 `Once` 避免重复替换；若外部直接重复调用公开初始化函数，则旧 collector 会被替换，旧计数不会迁移，新实例也不自动重新注册。

指标完整名由三段组成：`tidb` + `external_workload` + `task_total`，最终为 `tidb_external_workload_task_total`。两个变量标签的顺序是接口契约：第一个值是 worker `type`，第二个值是生命周期 `action`。独立测试使用 `type="gc"`、`action="init"` 创建并递增子 counter。包级常量标签由 `astersql-metrics-common` 的 wrapper 在构造时注入，不由本文件维护。

四个动作常量只约束推荐值；`CounterVec` 本身不会限制调用方传入其他字符串。每个新的 type/action 组合都会形成一个时间序列，因此动态或无界标签值会造成基数增长。

## 依赖与调用关系

上游装配与调用：

- `pkg/metrics/lib.rs`：公开声明 `pub mod external_workload`，并在测试配置下装配独立文件 `external_workload_test.rs`。
- `pkg/metrics/metrics.rs::InitMetrics -> InitExternalWorkloadMetrics`：初始化真实 counter。
- `pkg/metrics/metrics.rs::RegisterMetrics -> ExternalWorkloadTaskCounter`：把 collector 注册到默认 registry。
- `pkg/metrics/external_workload_test.rs`：验证描述符、标签顺序、递增和注册后的 gather 可见性。

下游依赖：

- `crate::LblType`（由 `pkg/metrics/session.rs` 再导出）和 `crate::LblAction`（定义于 `pkg/metrics/lib.rs`）提供标签名。
- `pkg/metrics/bindinfo.rs` 的 `compat_prometheus` 与 `compat_metricscommon` 把 Go 风格的 `CounterOpts`/`NewCounterVec` 转换到 Rust `prometheus` crate。
- `pkg/metrics/common/wrapper.rs::NewCounterVec` 合并常量标签并创建实际 collector。
- `pkg/metrics/metrics.rs::PACKAGE_INIT_LOCK` 串行化对包级可变指标的初始化写入。

预期业务消费者可由 Go 版本确认：`pkg/extworkload/manager.go::metricsInterceptor` 递增该计数器，GCV2、TTL、auto-analyze 方法提供标签。Rust 对应方法位于 `pkg/extworkload/manager.rs`，但当前通过 `pkg/extworkload/lib.rs::metrics` 桩处理，不能视为本文件的实际调用者。

## 错误处理与边界

- `InitExternalWorkloadMetrics` 没有 `Result` 返回值。`PACKAGE_INIT_LOCK.lock().expect(...)` 在锁中毒时 panic；底层 `NewCounterVec` 对非法选项调用 `expect("invalid counter vector options")`，也会 panic。
- 初始化本身不执行 Prometheus 注册，因此不会在这里产生重复注册错误。注册阶段由 `RegisterMetrics` 返回 `prometheus::Error`；重复向同一 registry 注册相同描述符通常在该阶段失败。
- 在 `ExternalWorkloadTaskCounter` 仍为 `None` 时调用 `RegisterMetrics` 会因明确的前置条件检查而 panic。独立测试通过先调用 `InitMetrics` 遵守此顺序。
- `with_label_values` 要求值的数量与声明的两个标签一致；本文件固定标签顺序，调用者改变顺序会产生语义错标，数量不符则由 Prometheus API 报错或兼容方法的策略处理。
- 该 counter 只统计事件发生次数，不记录 RPC 成功/失败、耗时或活动任务数；Go 拦截器在 invoker 之前递增，所以它表示尝试次数而非成功次数。
- `UpdateGCLifeTime`、删除 TTL 表信息和更新 TTL 开关在 Go/Rust 管理器中都未附加这些动作标签，不应从本指标推断这些操作次数。

## 并发与资源生命周期

正常生命周期是“进程初始化一次、注册一次、进程期间持续累加”。`InitMetrics` 的 `Once` 和 `PACKAGE_INIT_LOCK` 分别保护总初始化幂等性及多个 `static mut` 写入的串行化；注册后 registry 持有 collector 克隆，业务侧按标签取得子 counter 并原子递增。

本文件的 `static mut Option<CounterVec>` 不是一个面向任意并发访问的安全抽象。局部互斥锁只覆盖本初始化函数中的写操作，公开静态变量的读取和直接重初始化仍依赖调用方纪律。安全扩展不应在 worker 运行后再次调用本函数，也不应把对该全局的裸引用跨越可能写入的阶段。

本文件不创建线程、异步任务、通道、事务、网络连接或需要显式释放的资源。`CounterVec` 生命周期随全局和 registry 延续到进程结束；标签子 counter 会随遇到的标签组合保留，故调用方必须控制标签基数。

## 与 Go 版本的对应关系

`pkg/metrics/external_workload.go` 与本文件在四个动作字符串、全局变量角色、指标 namespace/subsystem/name/help 以及 `[LblType, LblAction]` 标签顺序上逐项一致。Rust 额外使用 `Option` 表示 Go 的 nil 指针状态，用 `unsafe static mut` 保存 collector，并增加 `PACKAGE_INIT_LOCK` 以串行化写入；Go 函数是直接赋值。

Go 的 `pkg/metrics/metrics.go` 在包初始化序列调用 `InitExternalWorkloadMetrics`，并在 `RegisterMetrics` 中执行 `prometheus.MustRegister(ExternalWorkloadTaskCounter)`。Rust 对应总入口用 `Once` 缓存初始化结果，注册接口返回 `Result`，但对未初始化的 `Option` 仍会 panic。

最大迁移差异在消费者侧：Go 的 `pkg/extworkload/manager.go` 导入 `pkg/metrics`，拦截器实际递增本 counter；Go 的 `pkg/extworkload/manager_test.go` 验证各业务方法携带正确标签。Rust 的 `pkg/extworkload/manager.rs` 复刻了标签注入位置，但 `astersql-extworkload` crate 目前仅依赖 client、Tokio、Tonic，并使用自身 `lib.rs::metrics` 的空操作 `Counter`。所以不能宣称 Rust 已完成端到端指标采集；当前独立 metrics 测试只证明 collector 本身可初始化、递增、注册和被 gather。

## 扩展指南

- 新增生命周期动作时，应在本文件和 `external_workload.go` 同步新增稳定常量，并在独立 Rust 测试 `pkg/metrics/external_workload_test.rs` 及 Go 管理器测试中覆盖对应业务方法；避免把任务 ID、表 ID等无界值作为 action。
- 改动指标名、帮助文本或标签集合属于监控接口兼容性变化，会影响 dashboard、告警和已有查询。尤其不能随意调整 `type`、`action` 顺序；需要先检索指标消费者并准备迁移方案。
- 若完成 Rust 业务接线，应先设计 crate 依赖方向，避免 `astersql-metrics` 与 `astersql-extworkload` 形成环；可考虑由 manager 注入计数接口或把共享指标句柄置于无环的公共 crate。接线后要用独立测试证明真实 RPC 拦截器递增默认 registry 中的 `tidb_external_workload_task_total`，而不只是验证 context 标签。
- 若改善全局安全性，优先评估 `OnceLock`/`LazyLock` 或锁保护句柄，并同步调整 `InitMetrics`、`RegisterMetrics` 和所有访问方；不能只局部替换类型后留下裸 `static mut` 读取。
- 测试必须继续放在独立的 `pkg/metrics/external_workload_test.rs`，不要内嵌进生产源文件。若验证 extworkload 接线，则相应扩展同目录独立的 `pkg/extworkload/manager_test.rs` 或迁移测试文件。
- 性能审查重点是标签基数和热路径递增开销；正确性审查重点是初始化/注册顺序、事件是在 RPC 前还是成功后计数，以及动作标签与 Go 语义一致。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引为 64 行、2 个图符号，并显示由 `pkg/metrics/metrics.rs` 和一个测试文件使用。
- RustCodeGraph `node --file pkg/metrics/external_workload.rs`：核对全部源代码、四个常量、全局 counter 和初始化函数；`query InitExternalWorkloadMetrics` 同时定位 Rust 与 Go 定义。
- RustCodeGraph `node --file`：核对 `pkg/metrics/metrics.rs` 的初始化/注册链、`pkg/extworkload/manager.rs` 的标签注入与拦截器、`pkg/extworkload/lib.rs` 的 metrics 桩、`pkg/metrics/bindinfo.rs` 和 `pkg/metrics/common/wrapper.rs` 的构造链。精确 `callers`/`callees` 查询未返回边，因此相关调用关系又以这些索引源码片段和精确符号搜索交叉确认，未臆造图边。
- crate 与模块边界：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/extworkload/Cargo.toml`。
- Go 对照：`pkg/metrics/external_workload.go`、`pkg/metrics/metrics.go`、`pkg/extworkload/manager.go`、`pkg/extworkload/manager_test.go`。
- Rust 测试与消费者证据：`pkg/metrics/external_workload_test.rs`、`pkg/extworkload/manager_test.rs`、`pkg/extworkload/migration_aster_unit_test.rs`。

按任务约束未运行 Cargo。交付前运行任务指定的结构命令，确认本文件存在且恰好包含上述十一个固定二级标题；同时人工复核文档明确回答了文件存在目的、初始化与注册流程、当前未完成的 Rust 业务接线及安全扩展位置。
