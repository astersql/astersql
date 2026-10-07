# `pkg/metrics/memory.rs`

## 文件定位

本文件属于 `astersql-metrics` crate（见 `pkg/metrics/Cargo.toml`），由 crate 根 `pkg/metrics/lib.rs` 以 `pub mod memory` 暴露。它不是内存仲裁算法本身，而是全局内存仲裁器的 Prometheus 指标定义与更新辅助层：负责创建 collector、保存包级句柄、缓存动态 `type` 标签对应的子指标，并把这些能力交给执行/会话等上层使用。

应用级入口位于 `pkg/metrics/metrics.rs`：`InitMetrics` 调用 `memory::InitMemoryMetrics`，`RegisterMetrics` 随后注册本文件的八个顶层 collector。当前 Rust 生产代码还会在 `pkg/session/runtime/control.rs` 直接递增预绑定的 `GlobalMemArbitratorSubTasks.ForceKillPlan`；代码搜索未发现三个动态更新辅助函数在其他 Rust 生产文件中的调用。Go 版本则由 `pkg/util/memory/global_arbitrator.go` 调用这些辅助函数，说明 Rust 的动态指标接线尚未完全达到 Go 的使用范围。

## 核心职责

- 以固定的 `namespace = "tidb"`、`subsystem = "memory"` 创建一组内存仲裁指标（`InitMemoryMetrics`）。
- 预绑定高频事件标签和任务标签，避免调用方重复执行 `WithLabelValues`（`GlobalMemArbitratorSubEvents`、`GlobalMemArbitratorSubTasks`）。
- 按 `taskType` 缓存动态 `Counter`/`Gauge` 子句柄，并分别执行累加或设置（`AddGlobalMemArbitratorCounter`、`SetGlobalMemArbitratorGauge`）。
- 将已经进入 gauge 缓存的全部动态序列归零（`ResetGlobalMemArbitratorGauge`）。

该文件只定义采集与更新机制，不决定仲裁模式、配额算法、OOM 判定或任务调度策略；这些行为属于 `pkg/util/memory` 等模块。

## 主要符号

- `GlobalMemArbitrationDuration: Option<Histogram>`：仲裁耗时，指标名 `tidb_memory_arbitration_duration_seconds`；使用从 `0.00005` 秒到 `86400` 秒的 17 个指数区间。
- `GlobalMemArbitratorWorkMode`、`GlobalMemArbitratorQuota`、`GlobalMemArbitratorWaitingTask`、`GlobalMemArbitratorRootPool`：均为带 `type` 标签的 `GaugeVec`，分别表示模式、配额字节数、等待任务数和根池状态。
- `GlobalMemArbitratorRuntimeMemMagnifi: Option<Gauge>`：无标签 gauge，描述运行时 heap-in-use 相对配额的放大比率。
- `GlobalMemArbitratorEventCounter`、`GlobalMemArbitratorTaskExecCounter`：带 `type` 标签的 `CounterVec`，分别记录事件和任务执行次数。
- `GlobalMemArbitratorSubEventsMetrics`：保存 `pool-init-hit-digest`、`pool-init-reserve`、`pool-init-medium-quota`、`pool-init-none` 四个预绑定事件 counter。
- `GlobalMemArbitratorSubTasksMetrics`：保存 `force-kill-parse`、`force-kill-plan`、`nolimit` 三个预绑定任务 counter。
- `counters`、`gauges`：`LazyLock<RwLock<HashMap<String, ...>>>`，键为 `taskType`，值为可克隆的 Prometheus 子指标句柄。
- `unsafe fn InitMemoryMetrics()`：创建所有 collector，并在父 `CounterVec` 就绪后创建固定标签子句柄。
- `AddGlobalMemArbitratorCounter(&CounterVec, &str, i64)`：查找/创建动态 counter 后调用 `Add(count as f64)`。
- `SetGlobalMemArbitratorGauge(&GaugeVec, &str, i64)`：查找/创建动态 gauge 后调用 `Set(value as f64)`。
- `ResetGlobalMemArbitratorGauge()`：遍历 gauge 缓存并将每个缓存句柄设为 `0.0`。

本文件没有 trait、impl 或条件编译分支；公开 API 是上述公开静态量、两个公开结构体及四个公开函数。

## 执行流程

1. `pkg/metrics/metrics.rs::InitMetrics` 在全包一次性初始化闭包内调用 `InitMemoryMetrics`。
2. `InitMemoryMetrics` 先取得 `metrics::PACKAGE_INIT_LOCK`，依次构造一个 histogram、五类 gauge/gauge-vector 和两个 counter-vector，并写入相应 `Option` 全局量。
3. 初始化完成父级 `GlobalMemArbitratorEventCounter` 和 `GlobalMemArbitratorTaskExecCounter` 后，函数用固定标签派生七个常用 counter，写入两个子指标结构体。父级先于子级是明确的不变量。
4. `metrics.rs::RegisterMetrics` 通过 `register_options!` 取出并注册八个顶层 collector；预绑定 counter 共享父 `CounterVec` 的底层序列，不单独注册。
5. 动态更新时，`AddGlobalMemArbitratorCounter` 或 `SetGlobalMemArbitratorGauge` 先在读锁下按 `taskType` 查缓存；命中则克隆句柄，未命中则从传入的 vector 派生句柄并在写锁下发布；锁释放后执行指标更新。
6. 需要清除瞬时 gauge 状态时，`ResetGlobalMemArbitratorGauge` 持读锁遍历缓存，将已缓存的每个 gauge 设为零；它不会删除标签序列，也不会影响从未经过本文件缓存创建的 gauge。

## 数据与状态

顶层 collector 与预绑定子句柄使用 `static mut` 保存，并以 `Option` 表达 Go 包变量初始化前的零值状态。读取或改写这些全局量需要调用方遵守 unsafe 初始化协议：先成功执行 `InitMetrics`/`InitMemoryMetrics`，再注册或访问。crate 根的 `#![allow(static_mut_refs)]` 只是允许这种迁移形态，并未把它变成安全的并发发布机制。

动态缓存是进程级、惰性且不会主动淘汰的。key 只有 `taskType`，不包含传入的 `CounterVec`/`GaugeVec` 身份；因此同一个标签字符串若跨不同 vector 调用，会复用第一次缓存的句柄。这与 `pkg/metrics/memory.go` 的 map 设计一致，也是扩展调用方必须保持的隐含约束。句柄的 `Clone` 指向同一底层时间序列，不复制数值。

## 依赖与调用关系

- crate 内依赖：`crate::bindinfo::compat_prometheus` 提供 Go 风格的 `Add`、`Set`、`WithLabelValues` 等兼容方法；`compat_metricscommon` 提供 `NewHistogram`、`NewGauge(Vec)`、`NewCounterVec`；`crate::LblType` 提供统一的 `type` 标签名；`crate::metrics::PACKAGE_INIT_LOCK` 串行化包级 collector 初始化。
- 外部依赖：`prometheus = "0.14"`（`pkg/metrics/Cargo.toml`）提供 collector 类型和注册能力；标准库 `LazyLock`、`RwLock`、`HashMap` 管理动态句柄缓存。
- 上游初始化/注册：`pkg/metrics/metrics.rs::InitMetrics` → `InitMemoryMetrics`；`RegisterMetrics` 注册全部八个顶层 collector。
- 当前 Rust 消费者：`pkg/session/runtime/control.rs::begin_compile_memory_arbitration` 在 OOM 风险分支递增 `GlobalMemArbitratorSubTasks.ForceKillPlan`。
- Go 对照调用链：`pkg/util/memory/global_arbitrator.go` 使用 `SetGlobalMemArbitratorGauge` 更新等待数、配额和根池，使用 `AddGlobalMemArbitratorCounter` 汇总任务/事件；这些 Rust 动态调用边经仓库搜索未发现，不能视为已经接线。

RustCodeGraph 将 `memory.rs::InitMemoryMetrics`、`AddGlobalMemArbitratorCounter`、`SetGlobalMemArbitratorGauge`、`ResetGlobalMemArbitratorGauge` 与同名 Go 符号分别建模；精确源码节点位于第 102、215、246、271 行。对歧义符号的普通 callers 查询会混合语言，因此本说明以精确节点加仓库引用搜索核验真实 Rust 调用边。

## 错误处理与边界

- `InitMemoryMetrics`、缓存读写和批量归零均以 `expect` 处理锁中毒；发生过持锁 panic 后再次访问会 panic，而不是返回可恢复错误。
- 初始化函数对父 counter 使用 `expect`，但父值在同一持锁函数中刚刚赋值；只要顺序不被修改，该断言成立。新增预绑定子项时必须继续遵守“先父后子”。
- `RegisterMetrics` 在初始化前访问 `Option` 会以 `InitMetrics must run before RegisterMetrics` panic；注册冲突等 Prometheus 错误则由 `Result` 向上传播（实现位于 `pkg/metrics/metrics.rs`）。
- 更新函数把 `i64` 转成 `f64`；极大整数可能丢失整数精度。counter 语义要求调用方不要传负增量；本文件不自行校验业务值。
- `ResetGlobalMemArbitratorGauge` 只覆盖 `gauges` map 中已有的动态子句柄，不重置 counter、不清空缓存，也不自动枚举 vector 的所有可能标签。
- 任意新 `taskType` 都会成为持久缓存 key 和 Prometheus 标签值；无限基数输入会造成内存和时序数量增长，调用方应使用受控枚举标签。

## 并发与资源生命周期

包级初始化有两层约束：外层 `metrics.rs::INIT_METRICS_ONCE` 保证完整指标包只初始化一次；本函数内部 `PACKAGE_INIT_LOCK` 还与其他可单独调用的初始化器串行化。不过 `static mut` 的安全性依赖调用协议，若绕过统一入口并与读取并发，Rust 类型系统无法提供保证。

动态缓存采用先读后写的 `RwLock` 模式。两个线程可同时未命中、各自从同一 vector 获取等价子句柄，再依次覆盖 map 中的缓存值；由于 Prometheus vector 按相同标签返回同一逻辑时间序列，这保留了 Go 实现允许的竞态窗口。计数/设值在锁外进行，缩短临界区。批量归零在整个遍历期间持读锁，阻止新 key 写入，但已取得句柄的线程仍可并发更新底层指标，因此“重置后保持为零”不是原子屏障。

缓存与 collector 均为进程生命周期资源，没有关闭、删除或标签回收路径。重复注册由 Prometheus 注册表报告错误；测试因此使用隔离进程执行全局初始化/注册路径（见 `pkg/metrics/metrics_test.rs`）。

## 与 Go 版本的对应关系

`pkg/metrics/memory.rs` 按 `pkg/metrics/memory.go` 保留了相同的八个 collector、指标 namespace/subsystem/name/help、`type` 标签、17 个耗时桶、七个固定标签值，以及动态缓存的读锁查询—写锁发布—锁外更新流程。Rust 用：

- `Option<T>` 表达 Go collector 包变量初始化前的零值；
- 两个具名结构体代替 Go 的匿名结构体；
- `LazyLock<RwLock<HashMap<...>>>` 代替带 `sync.RWMutex` 的惰性 map；
- 借用参数代替 Go 的 vector 值传递，并把 `i64` 显式转换为 `f64`；
- `expect` 明确暴露锁中毒与初始化顺序失败。

行为差距主要在调用接线而不在本文件算法：Go 的 `pkg/util/memory/global_arbitrator.go` 已调用三个更新/重置辅助函数；Rust 仓库搜索只确认初始化、注册和 `ForceKillPlan` 预绑定 counter 的直接消费。后续移植不能仅凭本文件 API 已存在就断言运行时会持续填充全部内存仲裁指标。

## 扩展指南

- 新增顶层指标时：在 `InitMemoryMetrics` 创建并保存 collector，同时在 `pkg/metrics/metrics.rs::RegisterMetrics` 加入注册列表；同步 `pkg/metrics/memory.go` 语义或清楚记录有意差异。
- 新增固定事件/任务类型时：扩展相应具名结构体及静态初值，并在父 `CounterVec` 初始化之后预绑定标签；标签拼写属于监控兼容契约，变更会产生新的时间序列。
- 新增动态调用方时：优先使用有限、稳定的 `taskType`；不要让相同字符串跨不同 vector 复用共享缓存。若确需支持这种场景，应同时重设计 cache key，而不是只改调用点。
- 改并发模型时：应优先消除或封装 `static mut`，并保持完整初始化、子句柄发布、注册之间的可见性；不能只依赖 crate 级 lint allow。
- 测试应放在独立文件，不嵌入 `memory.rs`。当前没有同名 `memory_test.rs`；可扩展 `pkg/metrics/metrics_test.rs` 验证注册后的 metric family/标签/桶，或新建独立 `pkg/metrics/memory_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 区域接入。并发缓存测试应覆盖首次未命中竞争、重复标签复用、reset 与并发 set 的边界。
- 若继续对齐 Go 运行时，应在 `pkg/util/memory/global_arbitrator.rs` 等真实仲裁路径补齐调用，并同步其独立 Rust 测试；这属于后续实现工作，不是本文档任务的现状修改。

## 验证依据

- 源码全貌：`pkg/metrics/memory.rs`（全部 277 行）；主要节点为 `InitMemoryMetrics`、`AddGlobalMemArbitratorCounter`、`SetGlobalMemArbitratorGauge`、`ResetGlobalMemArbitratorGauge`。
- crate/模块边界：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`；确认 crate 名、`prometheus 0.14` 依赖、`pub mod memory` 和兼容层再导出。
- 初始化与注册：`pkg/metrics/metrics.rs`；确认 `InitMetrics` 的调用次序、`PACKAGE_INIT_LOCK` 及八个 collector 的注册列表。
- Go 对照：`pkg/metrics/memory.go`；调用侧对照为 `pkg/util/memory/global_arbitrator.go`。
- Rust 直接消费证据：`pkg/session/runtime/control.rs` 的 `GlobalMemArbitratorSubTasks.ForceKillPlan`；仓库级精确 `rg` 未发现 Rust 生产代码调用三个动态更新辅助函数。
- 测试证据：`pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata` 冒烟调用 `InitMemoryMetrics`；`pkg/metrics/metrics_test.rs::test_register_metrics` 覆盖全包初始化/注册，但未对本文件各 metric family、动态缓存或 reset 做专项断言。`pkg/executor/test/memtest/mem_test.rs` 验证仲裁器行为，但不是本指标文件的直接单元测试。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/metrics` 确认目标与对照/测试文件；`query --json` 消除 Go/Rust 同名符号歧义；`node --file pkg/metrics/memory.rs` 核对完整定义。结构检查按任务指定命令执行。

