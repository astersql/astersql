# `pkg/timer/metrics/metrics.rs`

## 文件定位

本文件是 `astersql-timer-metrics` crate 的指标实现文件。crate 边界由 `pkg/timer/metrics/Cargo.toml` 定义，`pkg/timer/metrics/lib.rs` 公开 `metrics` 模块并用 `pub use metrics::*` 再导出本文件的全部公开符号。它不负责定时器调度、事件执行或持久化，只定义 timer 子系统共享的 Prometheus 事件计数器及其标签选择入口。

Rust 生产接线位于全局指标 crate：`pkg/metrics/metrics.rs::InitMetrics` 调用 `astersql_timer_metrics::InitTimerMetrics`，`register_external_metrics` 随后克隆并注册 `TimerEventCounter`。因此本文件处于“timer 事件产生者—全局 Prometheus 注册/采集”边界。当前 Rust `pkg/timer/runtime/runtime.rs` 与 `worker.rs` 尚未调用本文件：它们分别使用本地 `Counter(Arc<AtomicU64>)` 和 `WorkerCounters` 记录进程内计数；不能把 Go runtime 对本包的直接调用误写成已经完成的 Rust 生产接线。

## 核心职责

- 用 `TimerEventCounter` 保存唯一的 timer `CounterVec` 句柄，允许全局指标初始化代码和注册代码跨 crate 访问同一对象。
- 用 `InitTimerMetrics` 创建与 Go 同名、同 help、同可变标签顺序的指标：完整名称为 `tidb_server_timer_event_count`，标签依次为 `scope`、`type`。
- 用 `TimerScopeCounter` 将 `(scope, event)` 映射为具体 `prometheus::Counter`；参数 `event` 实际填入名为 `type` 的标签。
- 用 `TimerHookWorkerCounter` 统一 Hook worker 的 scope 命名，固定生成 `hook.<hookClass>`，避免各调用方重复拼接。
- 通过 `metricscommon::NewCounterVec` 注入创建时刻的包级常量标签，保持与其 Go 包装层的指标约定一致。

本文件不注册 collector，也不主动增加任何计数。注册发生在 `pkg/metrics/metrics.rs::register_external_metrics`，增加动作由取得具体 `Counter` 的调用方执行。

## 主要符号

- `TimerEventCounter: OnceLock<RwLock<Option<prometheus::CounterVec>>>`：公开的全局存储。`OnceLock` 只负责一次性建立外层 `RwLock<Option<_>>`，并不限制 `InitTimerMetrics` 只调用一次；每次初始化都会在写锁内以新的 `CounterVec` 替换 `Option`。
- `InitTimerMetrics()`：以 `prometheus::Opts` 构造 namespace=`tidb`、subsystem=`server`、name=`timer_event_count`、help=`Counter of timer event.` 的向量，声明 `scope`、`type` 两个动态标签，然后写入全局存储。
- `TimerHookWorkerCounter(hookClass: &str, event: &str) -> prometheus::Counter`：薄包装，将 scope 格式化为 `hook.<hookClass>` 后委托 `TimerScopeCounter`。它不缓存额外对象，也不增加计数。
- `TimerScopeCounter(scope: &str, event: &str) -> prometheus::Counter`：读取已初始化的 `CounterVec`，按 `[scope, event]` 的顺序调用 `with_label_values`，返回对应标签组合的可克隆计数器句柄。
- `#![allow(non_snake_case, non_upper_case_globals)]`：保留 Go 移植后的公开命名，使 Rust 调用点与 Go API 名称直接对应。

本文件没有自定义类型、trait、`impl`、条件编译项或私有函数；四个业务符号全部是公开 API。

## 执行流程

1. 全局指标启动：`pkg/metrics/metrics.rs::InitMetrics` 在其 `INIT_METRICS_ONCE.call_once` 闭包中调用 `InitTimerMetrics`。后者先通过 `metricscommon::NewCounterVec` 创建未注册的 `CounterVec`，再取得或创建全局 `RwLock<Option<_>>`，持有写锁并把向量放入 `Some`。
2. 全局注册：`pkg/metrics/metrics.rs::RegisterMetrics` 调用 `register_external_metrics`；该函数要求存储和内部 `Option` 已初始化，持有读锁克隆 `CounterVec`，再向指定 `prometheus::Registry` 注册克隆句柄。
3. 普通取值：调用方传入 scope 和 event；`TimerScopeCounter` 依次检查 `OnceLock` 与 `Option`，恢复可能被毒化的读锁，最后以固定标签顺序取得具体 counter。相同标签值组合返回指向同一指标子项的句柄，调用方可对它 `inc`/`inc_by`/`get`。
4. Hook 取值：`TimerHookWorkerCounter` 先分配格式化字符串 `hook.<hookClass>`，再走普通取值流程，因此 Hook counter 与直接调用 `TimerScopeCounter("hook.<class>", event)` 共享同一个向量和标签序列。
5. 采集：注册表在 gather 时从已经注册的 `CounterVec` 收集 metric family；本文件本身没有后台采集循环。

## 数据与状态

指标描述符的稳定部分是 `tidb_server_timer_event_count` 和 help 文本；动态状态按 `(scope, type)` 二元标签划分。`metricscommon::NewCounterVec` 会用 `GetConstLabels()` 替换 options 中的常量标签，因此全局常量标签是在 `InitTimerMetrics` 创建向量时拍快照，之后修改常量标签不会反向修改已创建的向量。

`CounterVec` 位于 `Option` 中，以显式区分“存储容器已经建立但指标尚未初始化”和“已有可用指标”。正常生产路径由 `pkg/metrics/metrics.rs::InitMetrics` 的一次性门闩保证先初始化再注册。`Counter` 和 `CounterVec` 的克隆是共享底层指标状态的句柄克隆，不是计数值快照；这由单元测试中通过两个入口观察同一标签值的行为得到验证。

重复调用 `InitTimerMetrics` 会替换全局句柄并丢弃旧向量在本全局中的引用，计数从新的向量重新开始。若旧向量已被注册表或调用方克隆持有，它仍独立存活；此后从本文件取得的新 counter 不再写入那个已注册的旧向量。因此重复初始化只适合受控测试，生产必须维持“初始化一次、随后注册和取值”的顺序。

标签基数没有在本文件中设上限。`scope` 或 `event` 每出现一个新组合，底层向量就可能新增时间序列；特别是 scope 不应包含无界请求 ID、时间戳等高基数字段。

## 依赖与调用关系

直接下游依赖只有两类：标准库 `OnceLock`/`RwLock` 提供全局初始化和同步；`prometheus` 提供 `Opts`、`CounterVec`、`Counter` 及标签选择。`metricscommon::NewCounterVec` 来自 `astersql-metrics-common`，会附加包级常量标签并在 options 或标签名非法时 panic。`pkg/timer/metrics/Cargo.toml` 只声明这两个依赖，且用 `package.metadata.porting.go-package = "pkg/timer/metrics"` 标明 Go 对照包。

Rust 直接上游包括：

- `pkg/metrics/metrics.rs::InitMetrics` 初始化本向量；该函数用全局 `call_once` 约束生产重入。
- `pkg/metrics/metrics.rs::register_external_metrics` 克隆并注册本向量；`RegisterMetrics` 通过它把 timer collector 接到目标 registry。
- `pkg/metrics/metrics_internal_test.rs::test_external_subsystem_metrics_are_initialized_and_registered` 通过 `TimerScopeCounter` 写入并验证注册表可采集完整指标名。
- `pkg/timer/metrics/migration_aster_unit_test.rs` 直接覆盖初始化、描述符、标签、计数值和 Hook scope 共享语义。

RustCodeGraph 对本文件给出的文件级反向使用者是 `pkg/metrics/metrics.rs` 与 `pkg/metrics/metrics_internal_test.rs`；其符号级 `callers` 未解析出跨 crate 调用，以上调用边由精确源码引用检索补证。Go 上游另见 `pkg/timer/runtime/runtime.go` 与 `worker.go`，但它们不是 Rust 调用边。

## 错误处理与边界

三个函数都不返回 `Result`。`InitTimerMetrics` 使用固定且合法的 descriptor/标签定义；如果未来把它们改为非法值，`metricscommon::NewCounterVec` 内部的 `expect("invalid counter vector options")` 会 panic。写锁或读锁曾被其他线程 panic 毒化时，本文件通过 `poisoned.into_inner()` 继续访问内部值，而不是再次 panic。

`TimerScopeCounter` 对初始化顺序采用 fail-fast：外层 `OnceLock` 未建立，或锁内仍为 `None`，都会以 `TimerEventCounter should be initialized before use` panic。它不做惰性指标初始化，这保证配置常量标签和全局注册顺序仍由 `InitMetrics` 管理。调用 `with_label_values` 的标签数固定为两个，所以本实现不会触发标签数量不匹配；若未来更改向量标签定义而未同步此函数，则可能在 Prometheus API 内 panic。

空字符串 scope/event 在本文件中没有被拒绝，会形成相应标签组合。`hookClass` 为空时 Hook scope 为 `hook.`；字符串中的点、空白或其他内容也不校验。是否允许这些业务值必须由调用方约束。计数器只能单调增加；本文件没有 reset、删除标签组合或注销 collector 的接口。

## 并发与资源生命周期

全局访问由 `OnceLock<RwLock<Option<_>>>` 同步：首次初始化容器是线程安全的；替换向量持有写锁；取 counter 与全局注册读取向量时持有读锁。锁只覆盖取得/克隆句柄的短临界区，计数增量由 `prometheus::Counter` 自身的线程安全实现承担，不需要继续持有本文件的锁。

`pkg/timer/metrics/migration_aster_unit_test.rs` 使用静态 `TEST_LOCK: Mutex<()>` 串行化本 crate 内两个会重复初始化全局向量的测试，避免它们相互替换状态。这个测试锁不是生产 API，也不能协调其他 crate 中并行运行的测试。全局生产初始化则由上层 `pkg/metrics/metrics.rs` 的 `INIT_METRICS_ONCE` 管理。

本文件不创建线程、异步任务、通道、定时器或 I/O 资源，也没有 `Drop`/关闭流程。向量及 counter 的生命周期由静态全局、注册表和各克隆句柄共同延长。最关键的生命周期约束是不要在注册后再次替换全局向量，否则“注册表持有的旧向量”和“调用方通过全局取得的新向量”会分叉。

## 与 Go 版本的对应关系

`pkg/timer/metrics/metrics.go` 是逐项语义来源：Go 的 `TimerEventCounter *prometheus.CounterVec` 对应 Rust 的带初始化状态和锁的全局存储；两边 `InitTimerMetrics` 都经 metrics common 包装创建 namespace `tidb`、subsystem `server`、name `timer_event_count`、help `Counter of timer event.`、标签 `scope/type` 的向量；两边 Hook 包装都生成 `hook.%s`；普通包装都按 scope、event 顺序选择标签。

实现差异主要来自全局状态模型。Go 包变量可直接赋值，未初始化时调用会发生 nil pointer panic；Rust 用 `OnceLock + RwLock + Option` 明确表示两个未就绪阶段，并给出固定 expect 文本，同时从锁毒化中恢复。Go 的 `InitMetrics` 直接初始化并用 `MustRegister` 注册，Rust 的全局 metrics crate 把初始化和向指定 registry 注册拆开，注册失败可作为 `prometheus::Error` 返回。

生产调用迁移尚不完全对称：Go `pkg/timer/runtime/runtime.go::NewTimerRuntimeBuilder` 使用 `TimerScopeCounter` 创建刷新计数器，`worker.go::newHookWorker` 使用 `TimerHookWorkerCounter` 创建六类 Hook 事件计数器；当前对应 Rust runtime 使用独立原子计数结构，未依赖 `astersql-timer-metrics`。因此本文件已对齐指标定义、全局初始化和注册，但不能据此声称 Rust timer runtime 的全部事件已经写入 Prometheus。

## 扩展指南

- 新增事件类型通常不需要修改本文件，只需从调用方以稳定、低基数的 event 值调用 `TimerScopeCounter` 或 `TimerHookWorkerCounter`；同步在独立测试中验证标签和增量，不要把测试放进 `metrics.rs`。
- 修改 namespace、subsystem、name、help 或标签顺序时，必须同时核对 Go `metrics.go`、`migration_aster_unit_test.rs::init_matches_go_metric_descriptor_and_labels`、全局注册测试及依赖该指标名的监控规则。标签顺序必须与 `TimerScopeCounter` 的参数顺序同步。
- 若要把 Rust timer runtime 接到 Prometheus，应在 `runtime.rs`/`worker.rs` 的构造路径替换或桥接当前本地原子计数，并迁移现有独立 runtime/worker 测试；必须保留其可测试的计数观察能力，评估动态 group/hook class 带来的基数，不能只删除本地计数让编译通过。
- 若需要支持安全重初始化，应重新设计注册表与全局句柄的一致性；仅在现有写锁内替换向量会使已注册 collector 分叉。更稳妥的生产约束仍是由上层 `call_once` 保证一次初始化。
- 新增 reset、删除标签或注销功能时，应明确并发语义以及注册表所有权；这些操作不应绕过 `pkg/metrics/metrics.rs` 的集中注册流程。
- 回归测试应优先扩展 `pkg/timer/metrics/migration_aster_unit_test.rs`；跨 crate 初始化/注册行为扩展 `pkg/metrics/metrics_internal_test.rs`。Go 语义调整还应同步检查 `pkg/timer/metrics/metrics.go` 以及 runtime/worker 的 Go 测试意图。

## 验证依据

- 目标源码与模块边界：`pkg/timer/metrics/metrics.rs`、`pkg/timer/metrics/lib.rs`、`pkg/timer/metrics/Cargo.toml`。
- 下游构造语义：`pkg/metrics/common/wrapper.rs::{GetConstLabels, NewCounterVec}`；确认常量标签快照、标签切片转换和非法 options 的 panic 行为。
- Rust 生产接线：`pkg/metrics/metrics.rs::{InitMetrics, RegisterMetrics, register_external_metrics}`；确认一次性初始化、外部 collector 注册及初始化先决条件。
- Rust 独立测试：`pkg/timer/metrics/migration_aster_unit_test.rs::{init_matches_go_metric_descriptor_and_labels, hook_worker_counter_uses_go_scope_format_and_shares_the_vector}`；跨 crate 注册测试：`pkg/metrics/metrics_internal_test.rs::test_external_subsystem_metrics_are_initialized_and_registered`。
- Go 对照与生产调用：`pkg/timer/metrics/metrics.go`、`pkg/metrics/metrics.go`、`pkg/timer/runtime/runtime.go::NewTimerRuntimeBuilder`、`pkg/timer/runtime/worker.go::newHookWorker`。同目录未发现直接断言本指标的 Go 测试。
- Rust 迁移现状反证：对 `pkg/timer/runtime/**/*.rs` 检索只发现 `runtime.rs::Counter` 和 `worker.rs::WorkerCounters` 等本地原子计数，没有 `TimerScopeCounter`/`TimerHookWorkerCounter` 调用；Rust Cargo 生产依赖仅由 `pkg/metrics/Cargo.toml` 指向本 crate，Go Bazel runtime 依赖则保留在 `pkg/timer/runtime/BUILD.bazel`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file` 读取目标、Go 对照、模块入口、测试和上层指标文件；`query` 定位三个函数的 Rust/Go 定义；`callees` 确认 `TimerHookWorkerCounter -> TimerScopeCounter`。符号级 `callers` 未返回跨 crate 边，调用者以文件级 used-by 信息和精确 `rg` 引用补证。
- 按计划未运行 Cargo。验证范围是代码/配置/测试事实复核与 Markdown 固定章节结构检查，不代表执行了运行时回归。
