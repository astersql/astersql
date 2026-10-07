# `pkg/metrics/topsql.rs`

## 文件定位

[`topsql.rs`](topsql.rs) 属于 `astersql-metrics` crate；该 crate 的入口 [`lib.rs`](lib.rs) 以 `pub mod topsql` 暴露本模块，边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定。本文件是 **TopSQL Prometheus 父指标的声明和构造层**：它只创建三个指标向量，不采集 SQL、执行计划或 RU（Request Unit）数据，也不连接 TopSQL agent。

包级中枢 [`metrics.rs`](metrics.rs) 在 `InitMetrics` 中调用 `crate::topsql::InitTopSQLMetrics`，随后在 `RegisterMetrics` 的 collector 列表中注册三个向量。因此它位于“进程指标初始化/注册”链上，而不是 TopSQL 数据采集与上报的业务链上。

## 核心职责

本文件仅承担三项职责，均可由 `InitTopSQLMetrics` 直接核验：

1. 定义 `tidb_topsql_ignored_total` CounterVec，用 `type` 标签区分被忽略事件。
2. 定义 `tidb_topsql_report_duration_seconds` HistogramVec，用 `type`、`result` 标签区分上报对象和结果。
3. 定义 `tidb_topsql_report_data_total` HistogramVec，用 `type` 标签区分一次上报的数据类别。

这里的“定义”包含指标名、帮助文本、标签维度和桶边界，但不包含注册和观测。注册由 [`metrics.rs`](metrics.rs) 完成；真正对具体标签子序列执行 `inc`/`observe` 的逻辑位于 TopSQL reporter 侧。需要特别注意：当前 Rust reporter 在独立 crate [`../util/topsql/reporter/metrics/lib.rs`](../util/topsql/reporter/metrics/lib.rs) 中另建了同名父向量，并由其 [`metrics.rs`](../util/topsql/reporter/metrics/metrics.rs) 切出子句柄；它没有依赖或读取本文件的三个静态量。因而本文件当前提供的是 `astersql-metrics` 注册表中的 TopSQL 指标定义，不应宣称已与 Rust reporter 的写入路径接通。

## 主要符号

- `pub static mut TopSQLIgnoredCounter: Option<prometheus::CounterVec>`：未初始化时为 `None`；初始化后承载 `namespace=tidb`、`subsystem=topsql`、`name=ignored_total` 的计数向量，唯一标签名为包级常量 `LblType`（值为 `"type"`）。
- `pub static mut TopSQLReportDurationHistogram: Option<prometheus::HistogramVec>`：上报耗时父直方图，标签为 `LblType` 和 `LblResult`，桶由 `ExponentialBuckets(0.001, 2.0, 24)` 生成。
- `pub static mut TopSQLReportDataHistogram: Option<prometheus::HistogramVec>`：上报数据量父直方图，标签为 `LblType`，桶由 `ExponentialBuckets(1.0, 2.0, 20)` 生成。
- `pub unsafe fn InitTopSQLMetrics()`：唯一函数入口，依次构造上述三个向量并写回包级可变静态量。它不返回错误、不注册 collector，也不创建线程、连接或后台任务。

本文件没有类型、trait、`impl` 或条件编译项。导入的 `CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat` 在本文件函数体内没有显式调用；实际构造依赖 `compat_metricscommon::{NewCounterVec, NewHistogramVec}` 和 `compat_prometheus::{CounterOpts, HistogramOpts, ExponentialBuckets}`。

## 执行流程

正常入口链为：

1. 调用方执行 [`metrics.rs`](metrics.rs) 的 `InitMetrics`。其 `INIT_METRICS_ONCE.call_once` 保证整套包级初始化只执行一次，并在同一闭包中调用 `crate::topsql::InitTopSQLMetrics`。
2. `InitTopSQLMetrics` 首先创建 ignored CounterVec，设置固定的 namespace/subsystem/name/help 和单个 `type` 标签，然后将 `Some(CounterVec)` 写入 `TopSQLIgnoredCounter`。
3. 函数创建 duration HistogramVec。24 个边界满足 `0.001 × 2^i`（`i=0..23`），从 1 ms 开始，最后一个显式边界约为 8388.608 秒（约 2.33 小时）；标签维度是 `type × result`。
4. 函数创建 data HistogramVec。20 个边界满足 `1 × 2^i`（`i=0..19`），最后一个显式边界为 524288；标签维度是 `type`。
5. 后续 [`metrics.rs`](metrics.rs) 的 `RegisterMetrics` 通过 `register_options!` 取出三个 `Option` 中的 collector、克隆并注册到默认 Prometheus registry。若未先初始化，`register_option` 会以 `expect("InitMetrics must run before RegisterMetrics")` 终止。

这里不存在“本函数直接上报”的步骤。Go 的 reporter 标签绑定可用于解释指标意图；当前 Rust 实际 reporter 标签绑定则发生在独立 reporter-metrics crate 中，不经过本函数。

## 数据与状态

三个全局量都采用 `static mut Option<...>`：`None` 表示尚未构造，`Some` 表示父指标向量可供注册。每次直接调用 `InitTopSQLMetrics` 都会以新向量替换旧值；本函数自身没有幂等保护。生产入口依靠 [`metrics.rs`](metrics.rs) 的 `Once` 将整个 `InitMetrics` 限制为一次，避免替换已被其他代码持有或注册的 collector。

指标的序列基数不是在本文件中固定枚举，而由使用方传入的标签值决定。Go reporter 的直接对照文件 [`../util/topsql/reporter/metrics/metrics.go`](../util/topsql/reporter/metrics/metrics.go) 展示了预期标签集合：ignored 类包括 SQL/plan/RU 超限、采集/上报通道满和背压丢弃；duration 类包括 `all`、`record`、`sql`、`plan`、`ru_record` 与 `ok/error`；data 类包括 `record`、`ru_record`、`sql`、`plan`。这些标签集合是下游约定，不由本文件校验。

直方图还会由 Prometheus 自动维护累计桶、样本数和样本和；本文件只选择桶边界。`report_data_total` 虽以 `_total` 结尾，但类型是 HistogramVec，而不是 CounterVec，这是与 Go 文件一致的既有指标契约。

## 依赖与调用关系

上游和包内接线：

- [`lib.rs`](lib.rs) 声明 `pub mod topsql`，让符号作为 `astersql_metrics::topsql::*` 可见。
- [`metrics.rs`](metrics.rs) 的 `InitMetrics` 是 Rust 直接调用者；RustCodeGraph 的文件关系也显示 `topsql.rs` 被 `metrics.rs` 使用。
- 同一文件的 `RegisterMetrics` 引用三个静态量并将它们加入默认 registry。

下游构造依赖：

- `crate::bindinfo::compat_prometheus` 提供 Go 形状的 opts、指标类型别名和 `ExponentialBuckets`；后者转发到 `prometheus::exponential_buckets`。
- `crate::bindinfo::compat_metricscommon` 将 Go 形状的 opts 转为 Rust `prometheus::{Opts, HistogramOpts}`，再调用 `astersql-metrics-common` 的工厂创建向量。
- `crate::*` 提供 `LblType` 和 `LblResult` 标签名常量。
- [`Cargo.toml`](Cargo.toml) 声明直接依赖 `prometheus = "0.14"` 和本地 `astersql-metrics-common`；本模块没有独有 feature gate。

跨 crate 的实际边界是关键限制：[`../util/topsql/reporter/metrics/Cargo.toml`](../util/topsql/reporter/metrics/Cargo.toml) 仅依赖 `prometheus`，未依赖 `astersql-metrics`，其父向量是在自身 `lib.rs` 中初始化。因此，虽然两边指标描述一致，当前代码搜索没有发现 Rust reporter 对本文件静态量的读取调用边。

## 错误处理与边界

`InitTopSQLMetrics` 没有 `Result` 返回值。兼容层内部用 `expect` 处理不合法配置：指数桶构造失败会触发 `expect("valid exponential histogram buckets")`；CounterVec/HistogramVec 工厂同样将构造失败视为编程错误而 panic。当前常量参数（正起点、倍增因子 2、正桶数，以及合法且不同的标签名）是静态有效配置，因此正常初始化不走可恢复错误路径。

初始化与注册有严格顺序边界：三个静态量初始为 `None`，先注册会在 `register_option` 的 `expect` 处 panic。反复绕过 `InitMetrics` 直接调用本函数则会替换父向量，可能使已经取得的子句柄或已注册 clone 与全局变量指向不同的底层状态；调用者不应依赖这种用法。

本文件也不校验标签值数量。Prometheus 向量的 `with_label_values` 在标签数与声明不一致时会失败或 panic（取决于所用 API）；安全扩展必须同步父向量声明、所有绑定点和测试。

## 并发与资源生命周期

本文件使用 `static mut`，因此函数标记为 `unsafe`，它本身不提供锁、原子操作或并发重入保证。安全生命周期由外层 [`metrics.rs`](metrics.rs) 管理：`INIT_METRICS_ONCE` 串行且只执行一次初始化，成功后以 `INIT_METRICS_DONE` 发布结果；注册函数随后读取稳定的 `Option`。`PACKAGE_INIT_LOCK` 是其他包级初始化场景可用的互斥量，但 `InitTopSQLMetrics` 自身没有获取它。

CounterVec/HistogramVec 是 Prometheus collector 句柄；注册时会 clone，具体样本状态由 Prometheus 类型内部共享和同步。本文件不持有网络连接、文件描述符、通道、锁守卫、异步任务或显式销毁逻辑。collector 的生命周期随全局静态量和 registry 延续到进程结束。

由于直接重写 `static mut` 会与并发读取形成数据竞争，新增调用点必须复用一次性包级入口，不能把 `InitTopSQLMetrics` 当作运行时重载 API。相关测试若需要重建全局指标，应像 [`../util/topsql/reporter/metrics/migration_aster_unit_test.rs`](../util/topsql/reporter/metrics/migration_aster_unit_test.rs) 一样使用测试互斥锁隔离；不过该测试覆盖的是 reporter-metrics crate 的镜像父向量，而非本文件静态量。

## 与 Go 版本的对应关系

直接 Go 对照是 [`topsql.go`](topsql.go)。Rust 保留了三个全局句柄、初始化顺序、完整 metric descriptor、标签维度和桶参数：

| 语义 | Go | Rust |
| --- | --- | --- |
| 未初始化状态 | `nil` 指针 | `Option::None` |
| ignored | `*prometheus.CounterVec`，`type` | `Option<CounterVec>`，`LblType` |
| duration | 1 ms、×2、24 桶，`type/result` | 参数和标签一致 |
| data | 1、×2、20 桶，`type` | 参数和标签一致 |
| 初始化入口 | `func InitTopSQLMetrics()` | `pub unsafe fn InitTopSQLMetrics()` |

主要差异如下：

- Go 包的 `init()` 自动调用 `InitMetrics`；Rust 没有模块加载时自动执行，而由应用/测试显式调用可返回 `Result` 的 `metrics::InitMetrics`。
- Go 全局指针的可变性由语言运行时约束；Rust 用 `static mut Option` 表达迁移期形状，并把读写安全责任暴露为 `unsafe`。
- Go reporter 的 [`../util/topsql/reporter/metrics/metrics.go`](../util/topsql/reporter/metrics/metrics.go) 直接对 `metrics.TopSQL*` 调用 `WithLabelValues`。当前 Rust reporter-metrics crate 为避免跨 crate 依赖，在自身 [`lib.rs`](../util/topsql/reporter/metrics/lib.rs) 重复创建父向量。因此“指标定义语义已对齐”，但“reporter 写入本文件注册的父向量”尚未从代码中得到证实，现状反而显示两套独立 collector。

## 扩展指南

修改现有 metric 名称、namespace、subsystem、help、标签顺序或桶边界都属于外部可观测性契约变更，可能破坏 dashboard、告警、PromQL 和时序连续性；优先新增指标而不是原地重命名。扩展时至少同步检查：

1. 在 `InitTopSQLMetrics` 中添加或修改父向量，并确保静态量仍只由一次性初始化入口写入。
2. 在 [`metrics.rs`](metrics.rs) 的 `RegisterMetrics` collector 列表中同步注册新增父向量；否则指标不会暴露。
3. 若新增标签或类别，同时修改所有具体 `with_label_values` 绑定点。当前 Rust reporter 使用独立实现，需要同步 [`../util/topsql/reporter/metrics/lib.rs`](../util/topsql/reporter/metrics/lib.rs) 与其 [`metrics.rs`](../util/topsql/reporter/metrics/metrics.rs)，否则两套描述会漂移。
4. 同步 Go 对照 [`topsql.go`](topsql.go) 和 Go reporter 标签绑定，除非变更明确只面向 Rust；文档或评审中应解释偏离原因。
5. 测试必须放在独立测试文件，不能内嵌到 `topsql.rs`。最近的指标标签/父序列行为测试是 [`../util/topsql/reporter/metrics/migration_aster_unit_test.rs`](../util/topsql/reporter/metrics/migration_aster_unit_test.rs)；若测试本文件本身，应在 `pkg/metrics` 新增或扩展独立 `*_test.rs`，并从 [`lib.rs`](lib.rs) 的 `#[cfg(test)]` 区域接入。

若目标是消除 Rust 的两套 TopSQL 父向量，需先设计 crate 依赖方向，避免 reporter 与 `astersql-metrics` 形成环；这超出本文件的局部修改范围，不能仅通过删除 reporter 侧定义完成。迁移时还应验证注册表中 descriptor 唯一、子句柄写入能由已注册父向量采集，并评估标签基数与 histogram 内存成本。

## 验证依据

本说明基于以下直接证据（均为只读分析，未运行 Cargo）：

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件、4415 个 Go 文件；`files --filter pkg/metrics` 确认 `topsql.rs`、`topsql.go`、crate 入口和相关测试均已索引。
- RustCodeGraph `node --file pkg/metrics/topsql.rs`：核对三个静态量、唯一初始化函数、descriptor、标签和桶参数；文件共 73 行。
- RustCodeGraph 对 `InitTopSQLMetrics` 的 `query/callers/callees`：查询识别 Go/Rust 同名函数；通用名称的 callers 结果只返回 Go `InitMetrics`，而带 Rust 节点标识的调用边未返回结果，因此又以 RustCodeGraph 的文件使用关系和 [`metrics.rs`](metrics.rs) 源码第 132、402–404 行直接核验 Rust 初始化与注册边。
- RustCodeGraph `node`：读取 [`topsql.go`](topsql.go)、[`metrics.rs`](metrics.rs)、[`bindinfo.rs`](bindinfo.rs)、Rust reporter 的 [`metrics.rs`](../util/topsql/reporter/metrics/metrics.rs) 及其 [`migration_aster_unit_test.rs`](../util/topsql/reporter/metrics/migration_aster_unit_test.rs)。
- 配置与模块证据：读取 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs) 和 reporter-metrics 的 [`Cargo.toml`](../util/topsql/reporter/metrics/Cargo.toml)、[`lib.rs`](../util/topsql/reporter/metrics/lib.rs)；`pkg/metrics` 不存在 `doc.go`。
- `rg` 全仓引用核验：Rust 中本文件三个静态量只在本文件和 `pkg/metrics/metrics.rs` 直接出现；同名引用还出现在 reporter-metrics 独立 crate，源码及 Cargo 依赖证明它是另一组静态量。Go 中 `pkg/metrics/metrics.go` 负责初始化/注册，`pkg/util/topsql/reporter/metrics/metrics.go` 负责具体标签绑定。
- 相关独立测试：没有 `pkg/metrics/topsql_test.rs` 或同目录同名测试；[`../util/topsql/reporter/metrics/migration_aster_unit_test.rs`](../util/topsql/reporter/metrics/migration_aster_unit_test.rs) 验证 ignored/duration/data 的标签集合及子句柄与其父向量共享 series，但由于使用 reporter-metrics 自有父向量，只能证明镜像实现和 Go 标签语义，不能证明本文件与 reporter 的运行时接线。

人工复核结论：本文件存在的原因是为 `astersql-metrics` 定义并注册 TopSQL 的三类父指标；执行由一次性包级初始化触发；安全扩展必须同时维护 descriptor、注册、标签绑定和独立测试，并正视当前 Rust reporter 指标所有权分离这一兼容性风险。
