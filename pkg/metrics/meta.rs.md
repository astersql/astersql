# `pkg/metrics/meta.rs`

## 文件定位

当前 Rust 状态必须区分“指标已构造”与“端到端已接线”：仓库检索中，这些 Rust 静态量除本文件赋值外没有生产读取点，`pkg/metrics/metrics.rs::RegisterMetrics` 的 Rust 注册列表也未包含它们。

## 核心职责

- 保留 Go `pkg/metrics/meta.go` 的指标名、子系统名、帮助文本、标签顺序和桶边界，使 Rust 与 Go 产生兼容的 Prometheus 元数据。
- 创建 AutoID 操作耗时直方图、Meta 操作耗时直方图和 AutoID 客户端连接重置计数器。
- 对外暴露 Go 同值的操作类型字符串，供未来观测点构造稳定 label value。

当前 Rust 状态必须区分“指标已构造”与“端到端已接线”：仓库检索中，这些 Rust 静态量除本文件赋值外没有生产读取点，`pkg/metrics/metrics.rs::RegisterMetrics` 的 Rust 注册列表也未包含它们。Go 版的生产观测点与注册行只能作为对照，不能证明 Rust 已完成同等接线。

## 主要符号

- `GLOBAL_AUTO_ID: &str = "global"`：全局 AutoID 操作的 `type` 标签值。
- `TABLE_AUTO_ID_ALLOC: &str = "alloc"` 与 `TABLE_AUTO_ID_REBASE: &str = "rebase"`：表级分配与 rebase 操作的 `type` 标签值。
- `GET_SCHEMA_DIFF`、`SET_SCHEMA_DIFF`、`GET_HISTORY_DDL_JOB`：Meta 读写耗时的三个 `type` 标签值。
- `AUTO_ID_HISTOGRAM: Option<prometheus::HistogramVec>`：`tidb_autoid_operation_duration_seconds`，标签顺序为 `[type, result]`。
- `META_HISTOGRAM: Option<prometheus::HistogramVec>`：`tidb_meta_operation_duration_seconds`，标签顺序同上。
- `RESET_AUTO_ID_CONN_COUNTER: Option<prometheus::Counter>`：`tidb_meta_autoid_client_conn_reset_total`，无可变标签。
- `pub unsafe fn init_meta_metrics()`：本文件唯一函数；在包级互斥锁内构造并替换上述三个 `Option` 的值。

本文件没有自定义类型、trait、`impl` 或条件编译分支。它导入的 `CounterCompat`、`GaugeCompat`、`MetricCompat` 和 `ObserverCompat` 在本文件中没有实际方法调用；真正用到的兼容 API 是 `compat_metricscommon::{new_histogram_vec, new_counter}`、`compat_prometheus` 选项类型与 `exponential_buckets`。

## 执行流程

1. `pkg/metrics/metrics.rs::InitMetrics` 由 `std::sync::Once` 保证包级总初始化最多执行一次，然后在子系统顺序中调用 `crate::meta::init_meta_metrics()`。
2. `init_meta_metrics` 首先锁住 `crate::metrics::PACKAGE_INIT_LOCK`；锁中毒会立即 `expect` panic，不继续部分初始化。
3. 函数创建 AutoID `HistogramVec`：namespace=`tidb`，subsystem=`autoid`，name=`operation_duration_seconds`，并设置 29 个 `0.0005 * 2^n` 的指数桶。
4. 以同样的桶和 `[LBL_TYPE, LBL_RESULT]` 标签创建 Meta `HistogramVec`，仅 subsystem 和 help 不同。
5. 创建 Meta 子系统下的 AutoID 连接重置 `Counter`。
6. 三个 collector 被写入包级 `static mut Option`；函数本身不向 Prometheus registry 注册，也不记录样本。

## 数据与状态

六个 `&'static str` 常量只表示稳定标签值，不携带运行状态。可变状态是三个 `static mut Option`：初始为 `None`，初始化后为 `Some(collector)`。两个直方图共用不可变量是：

- 标签个数和顺序必须为 `type`、`result`；观测时少传、多传或交换都会破坏 API/时序语义。
- 29 个桶从 0.0005 秒开始、倍率 2，最后一个显式边界约为 134217.728 秒（约 1.55 天），另有 Prometheus `+Inf` 桶。
- 再次直接调用 `init_meta_metrics` 会在锁内以新 collector 替换旧值，因此不具备 `OnceLock` 式的自身幂等性；正常生产路径依赖外层 `InitMetrics` 的 `Once`。

## 依赖与调用关系

上游直接证据：

- `pkg/metrics/metrics.rs::InitMetrics -> pkg/metrics/meta.rs::init_meta_metrics`，这是生产初始化入口。
- `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata -> init_meta_metrics`，这是当前唯一找到的 Rust 测试调用，仅验证构造器能接受这组 Go 形状元数据。

下游直接证据：

- `pkg/metrics/bindinfo.rs::compat_metricscommon` 将 Go 形状选项转换为 `prometheus` crate 的 `Opts`/`HistogramOpts`，再调用 `astersql-metrics-common` 工厂。
- `pkg/metrics/lib.rs` 提供 `LBL_TYPE = "type"` 和 `LBL_RESULT = "result"`。
- `prometheus::exponential_buckets` 创建桶边界；无效参数在兼容层被 `expect("valid exponential histogram buckets")` 转为 panic。本文件的常量参数是有效的。

`pkg/metrics/Cargo.toml` 的相关边界是直接依赖 `prometheus = "0.14"` 与路径依赖 `astersql-metrics-common = { path = "common" }`。当前 Rust 生产搜索未找到三个 collector 的观测点；`pkg/meta/meta.rs` 使用的 `metrics::META_HISTOGRAM` 来自该 crate 自身的 metrics/harness 抽象，不是对本文件 `Option<HistogramVec>` 的直接引用。

## 错误处理与边界

`init_meta_metrics` 没有 `Result` 返回值，所有构造失败均由下游兼容层的 `expect` 处理，因此边界策略是启动/初始化时 fail-fast，而非向上返回可恢复错误。互斥锁中毒同样 panic。

函数是 `unsafe` 的根本原因是对 `static mut` 赋值。`PACKAGE_INIT_LOCK` 仅约束使用同一锁的初始化函数；类型系统无法强制所有读取者持有这把锁。不得在初始化前对 `Option` 做 `unwrap`/注册，也不得在其他线程正在持有该 collector 引用时直接重新初始化。

## 并发与资源生命周期

collector 内部计数/观测操作由 `prometheus` crate 提供线程安全实现，但本文件的 collector 容器是进程级 `static mut`。初始化写入由 `PACKAGE_INIT_LOCK` 串行化，正常主链又由 `INIT_METRICS_ONCE` 限定一次。测试中直接调用则可以替换 collector，所以 `all_metric_initializers_accept_go_metadata` 通过隔离子进程运行，避免并行测试共享全局指标状态。

指标对象在被全局 `Option` 持有期间与进程同生命周期。本文件没有后台任务、通道、事务、显式注销或 drop 流程。如果未来补上 `RegisterMetrics` 接线，重新初始化还需考虑 registry 持有的 clone 与新全局值分离的风险。

## 与 Go 版本的对应关系

主要差异和迁移缺口是：

- Go 包初始化模式依赖单线程包初始化；Rust 额外使用 `PACKAGE_INIT_LOCK` 和外层 `Once`，但仍保留 `static mut`/`unsafe`。
- Go `pkg/metrics/metrics.go::RegisterMetrics` 显式注册 `AutoIDHistogram`、`MetaHistogram` 和 `ResetAutoIDConnCounter`；Rust `RegisterMetrics` 当前的 `register_options!` 列表未包含三者。
- Go 观测点分布在 `pkg/meta/autoid/autoid.go`、`pkg/meta/autoid/autoid_service.go`、`pkg/autoid_service/autoid.go` 和 `pkg/meta/meta.go`；Rust 全库检索未找到本文件三个 collector 的对应生产读取。
- Go 的 `ResetAutoIDConnCounter.Add(1)` 依赖 Go Counter API；Rust 兼容层虽提供 `CounterCompat::Add`，本文件与当前 Rust 调用链尚未使用它。

## 扩展指南

- 增加 Meta/AutoID 操作类型时，优先新增标签值常量，不要改动已有 label name/order，否则会改变 Prometheus 时间序列约定。
- 修改 metric namespace/subsystem/name 会改变公开时序列名；修改桶边界会影响分位数准确性、时序列数与 Go 仪表盘兼容性。两类改动都应先核对 `pkg/metrics/meta.go` 和现有监控查询。
- 补齐 Rust 生产功能时，最小必要接线点包括 `pkg/metrics/metrics.rs::RegisterMetrics`、AutoID 分配/rebase 路径、Meta schema diff/历史 DDL 路径以及 AutoID 连接重置路径；需分别核对 Go 对照点，不应只通过增加注册行就宣称完成。
- 如果消除 `unsafe`，应将三个全局值改为 `OnceLock`/`LazyLock` 或返回显式所有者，并同步调整注册、读取与测试隔离策略，避免 registry clone 指向旧 collector。
- 测试应保持在独立 Rust 文件中，不嵌入 `meta.rs`。可扩展 `pkg/metrics/bindinfo_1_aster_unit_test.rs` 或新建同目录 `meta_test.rs` 并在 `lib.rs` 以 `#[cfg(test)]` 挂载；应断言三个公开 metric 名、两个标签名、桶边界、计数/观测可写入，以及注册后可被 gather。
