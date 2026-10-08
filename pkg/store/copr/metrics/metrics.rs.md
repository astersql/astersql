# [`pkg/store/copr/metrics/metrics.rs`](metrics.rs)

## 文件定位

本文件属于独立 crate `astersql-store-copr-metrics`，crate 根由同目录的 [`lib.rs`](lib.rs) 指定并通过 `copr_metrics` 模块加载本文件。它位于 DistSQL/Coprocessor 缓存的可观测性边界：上游创建一个带 `type` label 的 Prometheus `CounterVec`，本文件再把 `evict`、`hit`、`miss` 三个固定 label 绑定成调用方可直接递增的 `Counter`。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，运行时仅直接依赖 `prometheus = "0.14"`。工作区根 `Cargo.toml` 以 `facade_store_copr_metrics` 暴露该 crate；`pkg/util/metricsutil/Cargo.toml` 直接依赖它，`pkg/store/copr/Cargo.toml` 则把它列为可选依赖。

## 核心职责

- 暴露 `CoprCacheCounterEvict`、`CoprCacheCounterHit`、`CoprCacheCounterMiss` 三个进程级计数器句柄，分别对应 coprocessor 缓存淘汰、命中和未命中事件。
- 用 `LazyLock<Counter>` 把句柄的首次绑定变成线程安全的一次性初始化，避免本文件再引入未同步的可变全局变量。
- 通过 `init`/`InitMetricsVars` 提供显式预热入口，使全局指标初始化流程能在服务处理请求前发现父 collector 尚未初始化的问题。
- 保留 Go 包的名称和 label 语义，便于迁移代码按相同指标序列 `tidb_distsql_copr_cache{type="..."}` 聚合。

本文件不创建或注册 Prometheus collector，也不实现缓存策略。父 `CounterVec` 的本地定义和构造位于同 crate 的 `lib.rs::metrics::DistSQLCoprCacheCounter` 与 `init_dist_sql_metrics`。

## 主要符号

- `pub static CoprCacheCounterEvict: LazyLock<Counter>`：首次求值时调用 `copr_cache_counter("evict")`，得到淘汰 label 的计数器句柄。
- `pub static CoprCacheCounterHit: LazyLock<Counter>`：绑定 `hit` label。
- `pub static CoprCacheCounterMiss: LazyLock<Counter>`：绑定 `miss` label。
- `pub fn init()`：Rust 风格的初始化入口，仅转调 `InitMetricsVars`。
- `pub fn InitMetricsVars()`：Go 兼容入口，按 evict、hit、miss 的顺序对三个 `LazyLock` 调用 `LazyLock::force`。它不重建已完成初始化的计数器。
- `fn copr_cache_counter(label: &str) -> Counter`：文件内私有绑定函数；从 `crate::metrics::DistSQLCoprCacheCounter` 取父向量，并用 `with_label_values(&[label])` 返回具体子计数器。

文件没有类型、trait、`impl`、条件编译项或自定义常量；公开 API 仅为三个静态句柄和两个初始化函数。

## 执行流程

1. `pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 调用 `astersql_store_copr_metrics::metrics::init_dist_sql_metrics()`，构造 `tidb` namespace、`distsql` subsystem、`copr_cache` name 且 label 名为 `type` 的 `CounterVec`。
2. `pkg/util/metricsutil/common.rs::initMetrics` 随后调用 `copr_metrics::InitMetricsVars()`；这个固定顺序是本文件可安全运行的前置条件。
3. `InitMetricsVars` 依次强制求值三个 `LazyLock`。每次首次求值进入 `copr_cache_counter`，读取父 `CounterVec` 并选择一个固定 label。
4. `CounterVec::with_label_values` 返回与相应 label 时间序列共享状态的 `Counter` 克隆句柄；后续通过公开静态量调用 `inc`/`inc_by` 会更新父向量中的同一序列。
5. 再次调用 `init` 或 `InitMetricsVars` 时，`LazyLock` 已完成求值，不会重复运行闭包，也不会重新绑定到另一个父向量。

当前 Rust 生产代码搜索只确认了上述全局初始化接线；未找到 `pkg/store/copr` 的 Rust 缓存执行路径递增这三个句柄。实际 hit/miss/evict 事件接线目前可在 Go 的 `coprocessor.go::handleCopCache` 与 `coprocessor_cache.go::newCoprCache` 中看到。因此本文件已提供指标句柄，但不能据此断言 Rust coprocessor 缓存路径已完整上报这些事件。

## 数据与状态

父状态是 `lib.rs::metrics::DistSQLCoprCacheCounter: Option<CounterVec>`。`None` 表示尚未初始化，`Some` 中的向量按唯一 label `type` 管理多条时间序列。本文件的三个 `LazyLock` 各自保存一个 `Counter` 句柄，不保存缓存内容、请求信息或 Region 信息。

三个 label 的意义为：

| label 值 | 事件 | Go 侧直接证据 |
| --- | --- | --- |
| `evict` | Ristretto 淘汰缓存项 | `pkg/store/copr/coprocessor_cache.go` 的 `OnEvict` 回调 |
| `hit` | TiKV 返回可接受的缓存命中并复用缓存数据 | `pkg/store/copr/coprocessor.go::handleCopCache` |
| `miss` | 响应未命中，之后可能满足条件而写入缓存 | `pkg/store/copr/coprocessor.go::handleCopCache` |

Prometheus `Counter` 只允许单调累加；本文件不提供重置和减计数接口。`migration_aster_unit_test.rs::migration_initializes_distinct_copr_cache_label_counters` 先保存当前值再分别增加 1、2、3，说明测试考虑了进程级指标可能已被先前操作累加，而非假设初值恒为零。

## 依赖与调用关系

上游初始化链为 `metricsutil::initParentMetricsCollectors -> metrics::init_dist_sql_metrics`，然后 `metricsutil::initMetrics -> copr_metrics::InitMetricsVars -> LazyLock::force -> copr_cache_counter`。RustCodeGraph 对目标文件的文件级关系显示直接测试使用者为 `migration_aster_unit_test.rs`；精确源码查询同时确认了 `pkg/util/metricsutil/common.rs` 的跨 crate 初始化调用。

下游依赖只有标准库 `std::sync::LazyLock`、`prometheus::Counter` 和同 crate 的 `crate::metrics::DistSQLCoprCacheCounter`。`with_label_values` 把句柄连接到父 `CounterVec`；本文件没有 I/O、网络、存储或异步调用。

Go 生产调用边是 `coprocessor_cache.go::newCoprCache -> CoprCacheCounterEvict.Add`，以及 `coprocessor.go::handleCopCache -> CoprCacheCounterHit.Add/CoprCacheCounterMiss.Add`。这些 Go 调用说明指标的业务触发点，但不是当前 Rust 生产接线已经完成的证据。

## 错误处理与边界

`copr_cache_counter` 使用 `expect("DistSQL coprocessor cache counter must be initialized first")`。若任何静态量在 `init_dist_sql_metrics` 之前首次解引用，进程会 panic；这里没有可恢复的 `Result`，因为初始化顺序被视为应用启动不变量。label 数量固定为一个，并与父 `CounterVec` 的 `type` label 对齐；将来若父向量改变 label 数量而本文件未同步，`with_label_values` 也会失败。

`LazyLock` 的一次性语义形成另一条边界：若父 `DistSQLCoprCacheCounter` 在三个句柄求值后被替换，已绑定的句柄不会自动重绑。当前 `lib.rs::metrics::init_dist_sql_metrics` 仅在父状态为 `None` 时赋值，避免正常路径替换；扩展时应保持这一约束。

指标本身不决定何时算命中、未命中或淘汰，也不验证 label 字符串。业务边界仍由缓存实现负责。本文件没有返回业务错误，不会影响缓存请求的成功或失败；其显式失败仅发生在启动/接线不变量被破坏时。

## 并发与资源生命周期

每个 `LazyLock` 的闭包在并发首次访问时最多成功执行一次，其他线程等待完成并共享同一个 `Counter` 句柄。Prometheus `Counter` 克隆句柄共享内部计数状态，测试通过从父 `CounterVec` 再取同 label 并读取相同增量验证了这一点。

三个静态量拥有进程生命周期，没有显式析构、后台任务、锁句柄、通道、事务或异步资源。唯一的 `unsafe` 块用于读取 `static mut Option<CounterVec>`；`LazyLock` 只保护三个子句柄的初始化，并不保护父变量本身。安全性依赖应用在并发请求和指标访问之前完成父 collector 初始化，且之后不再并发修改父变量。

## 与 Go 版本的对应关系

同目录 `metrics.go` 定义三个 `prometheus.Counter` 包变量，Go `init()` 调用 `InitMetricsVars()`，后者分别执行 `metrics.DistSQLCoprCacheCounter.WithLabelValues("evict"|"hit"|"miss")`。Rust 保留了三个变量名、`InitMetricsVars` 名称、初始化顺序和 label 字符串；`init` 是对 Go 包初始化语义的显式入口。

主要实现差异是：Go 在包初始化时给可变包变量赋值，Rust 则以 `LazyLock` 延迟且一次性地生成句柄。Rust 的父 collector 是本地 crate 中的 `Option<CounterVec>`，需由 `metricsutil` 显式先初始化；Go 的中央 `pkg/metrics` 初始化与注册流程负责其父变量。Rust 当前还没有找到与 Go `handleCopCache`/`OnEvict` 等价的生产递增调用，所以迁移状态应描述为“指标定义和启动绑定已接线，Rust 缓存事件消费尚未由直接证据确认”。

## 扩展指南

- 新增缓存事件 label 时，应同时修改父 collector 的帮助文本/label 约定、本文件的静态句柄与 `InitMetricsVars`，并同步 Go 对照或明确记录差异。不要把动态高基数字段（SQL、key、Region ID）加入 label。
- 若在 Rust coprocessor 缓存路径接入计数，应复用这些静态 `Counter`，把增量放在与 Go 相同的语义点：确认命中后、确认未命中后、真实淘汰回调中；不要仅以查找失败或写入尝试近似事件。
- 不应把单元测试嵌入本文件。指标绑定测试继续放在同 crate 的 `migration_aster_unit_test.rs`；缓存触发语义应放在 `pkg/store/copr` 的独立测试文件中，并分别覆盖 hit、miss、evict 及初始化顺序失败边界。
- 修改初始化协议时需同时审查 `pkg/util/metricsutil/common.rs` 中父 collector 与子句柄的调用顺序。若要支持父向量重建，现有 `LazyLock` 设计无法重绑，必须先定义并发访问、注册冲突和旧句柄存活的兼容策略。
- 性能上，正常事件路径只应执行计数器原子增量；避免在热路径重复查 `CounterVec` label。兼容性上必须保留现有指标全名和 `type={evict,hit,miss}`，否则监控面板和告警会断裂。

## 验证依据

- 目标源码：`pkg/store/copr/metrics/metrics.rs`，完整核对三个 `LazyLock`、`init`、`InitMetricsVars` 和 `copr_cache_counter`。
- crate 与模块边界：`pkg/store/copr/metrics/Cargo.toml`、`pkg/store/copr/metrics/lib.rs`、工作区根 `Cargo.toml`、`pkg/store/copr/Cargo.toml`、`pkg/util/metricsutil/Cargo.toml`。
- Rust 启动调用链：`pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 与 `initMetrics`。
- Rust 独立测试：`pkg/store/copr/metrics/migration_aster_unit_test.rs::migration_initializes_distinct_copr_cache_label_counters`，覆盖三条 label 的独立增量和与父向量共享状态。
- Go 对照与业务触发点：`pkg/store/copr/metrics/metrics.go`、`pkg/store/copr/coprocessor.go::handleCopCache`、`pkg/store/copr/coprocessor_cache.go::newCoprCache`；缓存相关边界另见独立的 `coprocessor_cache_test.go`。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/store/copr/metrics` 返回 `lib.rs`、`metrics.go`、`metrics.rs`、`migration_aster_unit_test.rs`；`node --file` 核对目标源码与 `metricsutil` 初始化片段；`query`/`callers`/`callees` 因 `InitMetricsVars` 重名存在歧义，故以路径限定的文件节点和 `rg` 精确引用搜索补证。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按计划以源代码、调用图、对照实现和结构检查作为验证证据。
