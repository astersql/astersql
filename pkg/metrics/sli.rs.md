# `pkg/metrics/sli.rs` 逻辑说明

## 文件定位

`pkg/metrics/sli.rs` 属于 Cargo 包 `astersql-metrics`（见 `pkg/metrics/Cargo.toml`），由 crate 根 `pkg/metrics/lib.rs` 以 `pub mod sli` 暴露。它是 SLI（Service Level Indicator，服务水平指标）的 **Prometheus 指标定义层**：声明并构造两个事务写入体验直方图，但不负责累计事务状态、判断事务大小、观测样本或直接注册 collector。

在包级主链中，`pkg/metrics/metrics.rs::InitMetrics` 调用 `crate::sli::InitSliMetrics` 完成句柄初始化；同文件的 `RegisterMetrics` 再注册 `SmallTxnWriteDuration` 与 `TxnWriteThroughput`。因此，本文件位于“包级指标初始化”与“事务写入 SLI 观测”之间的指标句柄边界，而不是 SQL 执行或事务提交路径本身。

当前 Rust 迁移还存在一条需要区分的平行实现：事务累计与上报规则位于 `pkg/util/sli/sli.rs::TxnWriteThroughputSLI`，但其 `metrics` 名称解析到 `pkg/util/sli/lib.rs` 内的轻量观察器，并未直接引用本文件的两个 Prometheus 句柄。故不能仅凭本文件断言 Rust SQL 事务运行时已把样本写入这里定义的 collector。

## 核心职责

本文件只有三项生产职责：

1. 以 `Option<prometheus::Histogram>` 包级可变静态量保存小事务写耗时直方图 `SmallTxnWriteDuration`。
2. 以相同形态保存非小事务写吞吐直方图 `TxnWriteThroughput`。
3. 由 `InitSliMetrics` 按 Go 版本相同的命名、帮助文本和指数桶参数构造两个句柄。

它明确不做以下工作：不调用 Prometheus 注册表，不采集当前事务数据，不区分小事务与大事务，不处理 commit，也不发起数据库或网络 I/O。注册职责在 `pkg/metrics/metrics.rs::RegisterMetrics`，事务分类与观测公式在 `pkg/util/sli/sli.rs::TxnWriteThroughputSLI::reportMetric`。

## 主要符号

- `pub static mut SmallTxnWriteDuration: Option<prometheus::Histogram>`：小事务写入耗时（秒）的无标签直方图。完整指标名由 namespace、subsystem、name 组合为 `tidb_sli_small_txn_write_duration_seconds`。初始值为 `None`，初始化后为 `Some(Histogram)`。
- `pub static mut TxnWriteThroughput: Option<prometheus::Histogram>`：非小事务写吞吐（bytes/second）的无标签直方图，完整指标名为 `tidb_sli_txn_write_throughput`，生命周期同上。
- `pub fn InitSliMetrics()`：唯一函数和公开初始化入口。它在内部 `unsafe` 块中依次替换上述两个静态 `Option`；没有返回值，也不自行加锁、判重或注册。

文件没有自定义类型、trait、`impl`、条件编译项或私有辅助函数。`CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat` 的匿名 trait 导入来自共享的 Go 兼容层；本文件自身的构造表达式没有调用这些 trait 方法，它们主要维持迁移代码统一的导入形状。

## 执行流程

正常包级流程如下：

1. 调用方进入 `pkg/metrics/metrics.rs::InitMetrics`。该入口用 `INIT_METRICS_ONCE` 保证整套指标初始化只执行一次。
2. `InitMetrics` 按固定子系统顺序调用 `crate::sli::InitSliMetrics`。
3. `InitSliMetrics` 为 `SmallTxnWriteDuration` 创建 `HistogramOpts`：`tidb` namespace、`sli` subsystem、`small_txn_write_duration_seconds` name，并调用 `ExponentialBuckets(0.001, 2.0, 28)` 生成 28 个指数桶边界。
4. 它再为 `TxnWriteThroughput` 创建选项，调用 `ExponentialBuckets(64.0, 1.3, 40)` 生成 40 个吞吐桶边界。
5. 两次构造都经过 `bindinfo::compat_metricscommon::NewHistogram`；兼容函数最终委托 `astersql_metrics_common::NewHistogram`，后者注入包级常量标签并用 `prometheus::Histogram::with_opts` 创建 collector。
6. 稍后的 `pkg/metrics/metrics.rs::RegisterMetrics` 从两个 `Option` 取出并克隆 collector，注册到默认 Prometheus registry。若启用简化指标模式，`ToggleSimplifiedMode(true)` 会注销它们；恢复普通模式时重新注册。

指数桶的稳定契约是起点、倍率和数量，而不是源码注释中的近似终点描述；扩展或核验边界时应以 `ExponentialBuckets(start, factor, count)` 实际生成的 28/40 个边界为准，另有 Prometheus 的 `+Inf` 桶承接更大值。

## 数据与状态

两个全局量都经历 `None → Some(Histogram)` 的初始化转换。`InitSliMetrics` 被再次直接调用时会创建新 collector 并覆盖旧句柄；函数本身不保留旧实例，也不检查旧实例是否已注册。正常主链依赖 `InitMetrics` 的 `Once` 避免重复替换。

直方图没有变量标签。构造时 `metricscommon::NewHistogram` 会用 `pkg/metrics/common/wrapper.rs::GetConstLabels` 的快照覆盖选项中的常量标签，因此标签集合在句柄构造时确定；之后修改全局常量标签不会反向改变已构造的直方图。

本文件不保存每笔事务的 `writeSize`、`writeTime`、读写 key 数或影响行数。这些状态属于 `pkg/util/sli/sli.rs::TxnWriteThroughputSLI`。Go/Rust 对照中的观测意图是：小事务观测写耗时秒数，其他有效事务观测 `writeSize / writeTime.seconds`；这些公式不是本文件执行的逻辑。

## 依赖与调用关系

上游直接关系：

- `pkg/metrics/lib.rs` 声明并公开 `sli` 模块。
- `pkg/metrics/metrics.rs::InitMetrics` 是 RustCodeGraph 与源码检索共同确认的 `InitSliMetrics` 包级调用者。
- `pkg/metrics/metrics.rs::RegisterMetrics` 消费两个静态句柄并注册；`ToggleSimplifiedMode` 消费同一对句柄以切换注册状态。

下游直接关系：

- `crate::bindinfo::compat_prometheus::HistogramOpts` 提供 Go 风格选项结构。
- `crate::bindinfo::compat_prometheus::ExponentialBuckets` 转发到 `prometheus::exponential_buckets`，非法参数会在兼容层 `expect` 处 panic。
- `crate::bindinfo::compat_metricscommon::NewHistogram` 转发到 `crate::metricscommon::NewHistogram`；后者来自 Cargo 路径依赖 `astersql-metrics-common`，注入常量标签并创建真正的 `prometheus::Histogram`。
- `prometheus = "0.14"` 是 `pkg/metrics/Cargo.toml` 中的外部 collector/registry 依赖。

RustCodeGraph 对 `InitSliMetrics` 返回 Go 与 Rust 两个同名定义，并确认 Rust 版本调用 `ExponentialBuckets`；对全局 `static mut` 的精确符号检索未返回本文件定义，因此注册和切换关系由 `rg` 对 `pkg/metrics/metrics.rs` 的直接引用补证。

## 错误处理与边界

`InitSliMetrics` 返回 `()`，没有可传播错误。当前两组桶参数固定且有效；如果将来把起点、倍率或数量改成非法值，`compat_prometheus::ExponentialBuckets` 会因 `.expect("valid exponential histogram buckets")` panic。若指标名称、帮助文本或选项无效，`metricscommon::NewHistogram` 也会在 `.expect("invalid histogram options")` panic。

初始化和注册分离带来明确前置条件：`RegisterMetrics` 或 `ToggleSimplifiedMode` 在 `InitMetrics` 之前访问句柄时，会因 `expect("InitMetrics must run before RegisterMetrics")` 或 `expect("InitMetrics must run first")` panic。重复注册同名 collector 则由 Prometheus registry 返回错误；本文件不吞掉该错误，因为注册发生在返回 `Result` 的外层函数。

直方图会自动把超过最大显式边界的样本计入 `+Inf` 桶；本文件不裁剪负数、零、无穷或 NaN 观测值，因为它根本不执行 `observe`。输入有效性应在观测调用方处理。

## 并发与资源生命周期

`prometheus::Histogram` 本身可克隆并用于并发观测，但保存句柄的 `static mut Option<_>` 不是同步容器。`InitSliMetrics` 虽是安全的公开函数，却在内部执行无锁全局写入；其正确使用约束是只从受 `INIT_METRICS_ONCE` 保护的 `pkg/metrics/metrics.rs::InitMetrics` 初始化主链调用。绕过主链并发调用或在其他线程无同步读取时，没有本文件内的安全保证。

注册时使用 collector clone；注册表持有的 clone 与静态句柄共享 Prometheus collector 内部状态。`ToggleSimplifiedMode` 由包级 `MODE: Mutex<bool>` 串行化模式切换，注销不会销毁静态 `Option` 中的原始句柄，因此之后可重新注册。进程结束前没有显式 drop/清理流程，也没有线程、异步任务、通道、文件句柄或网络连接由本文件管理。

## 与 Go 版本的对应关系

`pkg/metrics/sli.go` 是直接对照源。两边均定义 `SmallTxnWriteDuration`、`TxnWriteThroughput` 和 `InitSliMetrics`，且 namespace、subsystem、name、help 与两组桶参数完全一致：`(0.001, 2, 28)` 和 `(64, 1.3, 40)`。

主要表示差异是 Go 包级变量直接持有 `prometheus.Histogram` 接口值，Rust 使用 `static mut Option<prometheus::Histogram>` 表达“尚未初始化”。Go 的包初始化和测试运行模型隐含串行初始化；Rust 通过外层 `InitMetrics` 的 `Once` 补充一次性约束。Go 的 `metricscommon.NewHistogram` 与 Rust 兼容包装都负责应用包级常量标签。

Go 运行时的真实消费链是 `pkg/util/sli/sli.go::TxnWriteThroughputSLI::reportMetric → pkg/metrics` 两个句柄。Rust 的 `pkg/util/sli/sli.rs` 保留了相同分类和计算语义，但当前导入的是自身 crate 的 `metrics` 模块；`pkg/util/sli/lib.rs` 提供的是用于记录次数/总和的轻量观察器。因此，“定义和注册 parity”已经存在于本文件及 `pkg/metrics/metrics.rs`，而“事务运行时观测直达本 collector”的 Rust 接线不能从现有代码得到证明。

## 扩展指南

新增或修改 SLI 指标时，应保持职责分层：在本文件声明和构造句柄，在 `pkg/metrics/metrics.rs::RegisterMetrics` 接入注册；若指标应受简化模式控制，还要同步 `ToggleSimplifiedMode`。调整现有名字、help、桶参数或单位会影响仪表盘、告警和时序连续性，应先核对 `pkg/metrics/sli.go` 及监控兼容要求，不能只改单侧。

若要让 Rust 事务运行时真正上报到本文件，应优先解决 `astersql-util-sli` 与 `astersql-metrics` 的 crate 依赖方向，避免制造循环依赖；不能把当前 `pkg/util/sli/lib.rs` 的测试观察器误当成生产 Prometheus 接线。接线后应在独立测试文件中覆盖：完整指标名、help、28/40 个桶边界、常量标签快照、初始化前置条件、注册/简化模式切换，以及小事务和大事务分别只写入对应直方图。

本文件没有同名独立 Rust 测试。最接近的现有测试是：

- `pkg/metrics/metrics_test.rs::test_register_metrics`：间接覆盖整包初始化和注册不报错，但只显式断言 DDL 指标名。
- `pkg/metrics/metrics_2_aster_unit_test.rs::package_metrics_initialization_covers_go_success_boundary_and_error_paths`：间接覆盖 `InitMetrics` 成功路径，未逐项断言本文件两个 collector。
- `pkg/util/sli/migration_aster_unit_test.rs`：覆盖小/大事务边界、无效事务、观测公式和 reset/failpoint，但使用 `pkg/util/sli/lib.rs` 的轻量观察器，不是本文件 collector 的集成测试。
- `pkg/executor/executor_failpoint_test.go::TestTxnWriteThroughputSLI`：Go 侧端到端行为依据。

## 验证依据

本说明读取并核对了以下直接证据：`pkg/metrics/sli.rs`、`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`、`pkg/metrics/sli.go`、`pkg/metrics/bindinfo.rs`、`pkg/metrics/common/wrapper.rs`、`pkg/util/sli/sli.rs`、`pkg/util/sli/lib.rs`、`pkg/util/sli/sli.go`、`pkg/metrics/metrics_test.rs`、`pkg/metrics/metrics_internal_test.rs`、`pkg/metrics/metrics_2_aster_unit_test.rs`、`pkg/util/sli/migration_aster_unit_test.rs` 与 `pkg/executor/executor_failpoint_test.go`。`pkg/metrics` 下不存在 `doc.go`，因此没有额外包契约文件可读。

RustCodeGraph 证据包括：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/metrics/sli.rs` 展示目标文件 61 行全貌并指出由 `pkg/metrics/metrics.rs` 使用；`query InitSliMetrics` 命中 Go/Rust 两个定义；`callees InitSliMetrics` 确认 Rust 定义下游包含 `ExponentialBuckets`；对 `pkg/metrics/metrics.rs`、`pkg/metrics/lib.rs`、`pkg/util/sli/sli.rs` 和相关测试的 `node` 查询补齐初始化、注册、切换和观测边界。精确 `rg` 又确认 `metrics.rs` 第 129、378–379、497–498 行分别承担初始化、注册和简化模式切换，并确认没有直接引用把 Rust `pkg/util/sli` 的观测写入本文件句柄。

本任务为纯文档分析，按计划未运行 Cargo。文档结构由任务规定的 11 个固定二级标题命令验证；事实人工复核重点是：本文件为何存在、初始化与注册如何分离、共享静态状态的并发前提、Go parity，以及当前 Rust 运行时接线仍未由代码证明的限制。
