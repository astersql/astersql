# `pkg/infoschema/metrics/metrics.rs`

## 文件定位

本文件属于独立 crate `astersql-infoschema-metrics`，crate 根是同目录的 `lib.rs`，后者通过 `#[path = "../../../pkg/infoschema/metrics/metrics.rs"]` 装入本文件并公开重导出其符号。`Cargo.toml` 表明该 crate 只有 `prometheus = "0.14"` 一个直接外部依赖，并以 `pkg/infoschema/metrics` 作为 Go 对照包。

它位于“父指标向量”与 InfoSchema 业务调用之间：父级 `CounterVec`/`HistogramVec` 定义在 `pkg/infoschema/metrics/lib.rs::metrics`，本文件把固定 label 组合绑定成可复用的全局句柄。全局指标初始化链 `pkg/util/metricsutil/common.rs::initMetrics` 会在父 collector 初始化后调用 `infoschema_metrics::InitMetricsVars`。本文件不是 `INFORMATION_SCHEMA.METRICS_*` 虚拟表定义；后者位于 `pkg/infoschema/metrics_schema.rs`。

当前迁移状态需要特别区分：Rust 全局初始化和句柄行为已有实现及独立测试，但仓库内生产 Rust 代码没有直接引用这些十个固定句柄；对应的业务消费点仍能在 Go 的 `pkg/domain/domain.go` 与 `pkg/infoschema/issyncer/loader.go` 中看到。因此这些句柄目前主要提供 Go API 对齐和后续 Rust 业务接线边界。

## 核心职责

1. `Counter` 和 `Observer` 将 `std::sync::LazyLock` 包装成接近 Go Prometheus API 的句柄，分别提供 `Inc`/`Get` 与 `Observe`/`GetSampleCount`，并通过 `Deref` 暴露底层 `prometheus::Counter` 或 `prometheus::Histogram`。
2. 十个私有构造函数把固定 label 值绑定到三个父向量：`InfoCacheCounters`、`LoadSchemaCounter`、`LoadSchemaDuration`。
3. 十个公开静态句柄保存这些子序列，避免业务调用方反复拼写 label，并保持同一 time series 的身份。
4. `init` 与 `InitMetricsVars` 提供包级初始化入口；后者显式 force 所有 `LazyLock`，使 Rust 的初始化时机与 Go 包加载时绑定变量的效果对齐。

本文件不创建或注册父 collector，也不负责 Prometheus registry。父向量的名字、帮助文本、label schema 和直方图桶由 `pkg/infoschema/metrics/lib.rs::metrics` 决定。

## 主要符号

- `pub struct Counter(LazyLock<prometheus::Counter>)`：计数器惰性句柄。`Counter::new` 仅在本文件内构造静态值；`Inc` 加一，`Get` 读取当前浮点计数；`Deref<Target = prometheus::Counter>` 允许调用底层 API。
- `pub struct Observer(LazyLock<prometheus::Histogram>)`：直方图惰性句柄。`Observe(value)` 记录一个以秒为单位的样本，`GetSampleCount` 返回已记录样本数；同样实现到 `prometheus::Histogram` 的 `Deref`。
- `get_latest_counter`、`get_ts_counter`、`get_version_counter`：绑定 `InfoCacheCounters` 的 `("get", "latest|ts|version")`。
- `hit_latest_counter`、`hit_ts_counter`、`hit_version_counter`：绑定 `InfoCacheCounters` 的 `("hit", "latest|ts|version")`。
- `load_schema_counter_snapshot`：绑定 `LoadSchemaCounter` 的 `("snapshot")`。
- `load_schema_duration_total`、`load_schema_duration_load_diff`、`load_schema_duration_load_all`：绑定 `LoadSchemaDuration` 的 `("total|load-diff|load-all")`。
- `GetLatestCounter`、`GetTSCounter`、`GetVersionCounter`、`HitLatestCounter`、`HitTSCounter`、`HitVersionCounter`：六个公开 InfoCache 计数句柄。
- `LoadSchemaCounterSnapshot`：公开的 snapshot schema 加载次数句柄。
- `LoadSchemaDurationTotal`、`LoadSchemaDurationLoadDiff`、`LoadSchemaDurationLoadAll`：三个公开的 schema 加载耗时句柄。
- `pub fn init()`：薄入口，只调用 `InitMetricsVars`。
- `pub fn InitMetricsVars()`：按上述静态声明顺序 force 十个句柄；是 `pkg/util/metricsutil/common.rs::initMetrics` 的下游调用。

## 执行流程

全局初始化路径如下：

1. `pkg/util/metricsutil/common.rs::initMetrics` 先调用 `initParentMetricsCollectors`，再依次初始化各 metrics 子 crate。
2. 该函数调用 `infoschema_metrics::InitMetricsVars`。
3. `InitMetricsVars` 对十个静态句柄逐一执行 `LazyLock::force`。
4. 每个句柄首次 force 时运行自己的私有构造函数，调用父向量的 `with_label_values`，校验 label 数量并取得或创建对应子序列。
5. 后续 `Inc`、`Get`、`Observe`、`GetSampleCount` 或 `Deref` 直接访问已经缓存的同一子序列；重复调用 `InitMetricsVars` 不会重建它。

若调用方在全局初始化前直接使用某个句柄，访问 `self.0` 也会触发该句柄的惰性初始化，所以单个句柄可独立工作。显式初始化的价值是一次性建立完整固定 label 集，并与 Go 的包初始化语义保持一致。

业务语义可由 Go 调用链复核：`pkg/domain/domain.go` 对 snapshot 加载调用 `LoadSchemaCounterSnapshot.Inc()`；`pkg/infoschema/issyncer/loader.go` 分别在总体、增量和全量加载路径对三个 duration 句柄调用 `Observe(time.Since(...).Seconds())`。这些是语义对照证据，不代表同名 Rust 业务路径已经完成接线。

## 数据与状态

- 每个公开句柄内部保存一个进程级 `LazyLock`；初始化后持有由父向量返回的 cloneable Prometheus 子句柄。子句柄共享父向量中的同一指标序列，而不是复制计数值。
- `InfoCacheCounters` 的 label schema 是 `action,type`，本文件穷举 `get/hit × latest/ts/version` 六种固定组合。
- `LoadSchemaCounter` 的 label schema 是 `type`，本文件只预绑定 `snapshot`。同一父向量在其他模块还可能使用 `failed`、`succ` 等值，那些不属于本文件的固定句柄。
- `LoadSchemaDuration` 的 label schema 是 `action`，本文件预绑定 `total`、`load-diff`、`load-all`。父向量在 `lib.rs` 中配置 20 个指数桶，起点 `0.001` 秒、倍率 `2.0`。
- `Counter::Get` 返回 `f64`，`Observer::GetSampleCount` 返回 `u64`；二者是观测辅助接口，不会清零指标。
- 文件没有可变普通全局、缓存淘汰、配置快照或持久化状态。指标生命周期随进程及父 collector；本文件也没有 reset API。

## 依赖与调用关系

上游调用者：

- `pkg/util/metricsutil/common.rs::initMetrics` 是已确认的生产 Rust 初始化调用者，在父 collector 初始化之后调用 `infoschema_metrics::InitMetricsVars`。
- `pkg/infoschema/metrics/migration_aster_unit_test.rs` 调用 `InitMetricsVars`/`init` 及全部公开句柄，验证 label、观测和幂等性。
- `pkg/infoschema/metrics/lib_test.rs` 调用 `InitMetricsVars`，并检查父 collector 的完整名称与直方图桶。
- 直接固定句柄的生产 Rust 消费者在当前仓库搜索中未发现；Go 消费者见 `pkg/domain/domain.go` 和 `pkg/infoschema/issyncer/loader.go`。

下游依赖：

- `crate::metrics::InfoCacheCounters`、`LoadSchemaCounter`、`LoadSchemaDuration`，定义于 `pkg/infoschema/metrics/lib.rs`。
- `prometheus::Counter`、`Histogram` 及其 `inc`、`get`、`observe`、`get_sample_count`、`with_label_values` 行为。
- `std::sync::LazyLock` 提供线程安全的一次初始化；`std::ops::Deref` 提供底层类型兼容入口。

crate 边界由 `pkg/infoschema/metrics/Cargo.toml` 确认。该 crate 被 `pkg/util/metricsutil`、`pkg/domain`、`pkg/infoschema`、`pkg/infoschema/issyncer` 和 `pkg/metrics` 等 Cargo manifest 引用，但 manifest 依赖本身不能证明每个固定句柄都有运行时调用。

## 错误处理与边界

- 本文件的公开操作不返回 `Result`。`Inc`、`Get`、`Observe` 和读取样本数直接委托给 Prometheus 句柄。
- `with_label_values` 在 label 数量不匹配时会失败；这里所有 label 数量和顺序均为编译期固定字符串，并由迁移测试核对。若父向量的 label schema 改变而本文件未同步，首次惰性初始化或 `InitMetricsVars` 会暴露该不兼容。
- 父 collector 的构造在 `lib.rs` 使用 `unwrap`，选项或桶配置非法会 panic；该风险不在本文件内恢复。
- `Observe` 接受任意 `f64`，本文件不验证负值、NaN 或无穷值，也不自行换算单位。调用方必须传入秒，Go loader 使用 `Duration.Seconds()` 提供这一契约证据。
- 本文件不注册 collector；只调用 `InitMetricsVars` 不能保证指标已经进入某个 registry。注册责任属于更高层指标设施。
- `Get` 与 `GetSampleCount` 主要支持状态核验；并发更新时它们只能给出读取时刻的观测值，不提供跨多个指标的一致快照。

## 并发与资源生命周期

`LazyLock` 保证每个句柄的构造闭包至多成功执行一次，多个线程同时首次访问会在初始化点同步。初始化完成后，Prometheus 的计数器和直方图实现承担并发更新安全性；本文件没有额外锁、线程、异步任务、通道或事务。

十个句柄各自独立初始化，因此 `InitMetricsVars` 不是十项原子事务：若某一项初始化 panic，之前已 force 的句柄仍保持初始化状态。固定 label 与已测试父向量使正常路径不应触发该情况，但扩展 label schema 时需要考虑这一边界。

静态句柄存活到进程退出。重复调用 `InitMetricsVars` 只 force 已有 `LazyLock`，不会替换底层子序列；`repeated_initialization_keeps_existing_metric_series` 通过前后指针相等验证了这一点。文件没有显式清理或注销逻辑。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/metrics/metrics.go`：Go 也声明六个 InfoCache counter、一个 snapshot counter 和三个 duration observer，并在包 `init` 中调用 `InitMetricsVars`。Rust 的固定 label 字符串和绑定顺序与 Go 第 45—57 行一致。

主要实现差异是生命周期表达方式：

- Go 的包变量先为空接口值，再由 `InitMetricsVars` 赋值；再次初始化会重新赋句柄。
- Rust 用不可变 `static` 包裹 `LazyLock`，首次访问绑定句柄，重复初始化保持原对象身份，避免 `static mut`。测试证明这仍指向父向量的同一 time series。
- Go 直接使用 `prometheus.Counter`/`Observer` 接口；Rust 通过 `Counter`/`Observer` 包装补出 Go 风格方法名，并提供 `Deref` 访问具体类型。
- Go 父指标定义位于 `pkg/metrics/domain.go` 并由中心 registry 注册；该 Rust 子 crate 在 `lib.rs` 中持有本地父向量，仓库注释说明这是 Rust 拆分 owner crate 后的当前结构。

Go 测试没有为本薄绑定文件设置同名独立测试；Rust 的 `migration_aster_unit_test.rs` 和 `lib_test.rs` 补充验证 label 矩阵、共享序列、重复初始化、指标全名和桶配置。生产行为接线方面，Go 已有 domain/issyncer 调用，Rust 当前只确认了初始化调用。

## 扩展指南

新增固定指标句柄时，应按以下边界修改：

1. 若只是给既有父向量新增一个固定 label 组合，在本文件增加私有构造函数、公开静态包装句柄，并在 `InitMetricsVars` 中 force；不要在业务调用点重复裸写 label。
2. 若新增指标名、label schema 或桶，先修改实际父 collector 所在的 `pkg/infoschema/metrics/lib.rs::metrics`，再同步本文件绑定。还应检查 Go 对照 `pkg/metrics/domain.go` 与 `pkg/infoschema/metrics/metrics.go`，明确是保持兼容还是有意产生差异。
3. 在独立测试文件中扩展测试，优先使用 `pkg/infoschema/metrics/migration_aster_unit_test.rs` 验证绑定值、time series 身份和操作行为，使用 `pkg/infoschema/metrics/lib_test.rs` 验证指标全名与桶；不要把测试嵌入本生产文件。
4. 新增生产 Rust 消费点时，应接入真实业务阶段并保持单位契约。例如 duration 必须传秒；计数器分支必须与 Go 的 snapshot/get/hit 语义一致。
5. 兼容风险包括 dashboard/告警依赖的完整指标名和 label 值变化；正确性风险包括 label 顺序错位或漏 force；性能风险主要是高基数 label 或热路径额外观测。本文件当前只使用有限固定 label，不应引入请求 ID、表名等无界值。
6. 若改变初始化模型，应继续保证并发首次访问安全与重复初始化不丢失已有序列，并同步 `pkg/util/metricsutil/common.rs::initMetrics` 的全局顺序假设。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件；`files --filter pkg/infoschema/metrics` 确认目标 crate 的 `lib.rs`、`metrics.rs`、Go 对照和两个独立 Rust 测试均已索引。
- RustCodeGraph `node --file pkg/infoschema/metrics/metrics.rs`：读取目标文件完整 156 行，确认 `Counter`、`Observer`、十个构造函数、十个静态句柄、`init` 与 `InitMetricsVars`。
- RustCodeGraph `node`：读取 `pkg/infoschema/metrics/lib.rs`、`metrics.go`、`lib_test.rs`、`migration_aster_unit_test.rs`，核对父向量、重导出、Go 绑定和测试不变量。
- RustCodeGraph `node --file pkg/util/metricsutil/common.rs --offset 280`：确认 `initMetrics` 在父 collector 初始化后调用 `infoschema_metrics::InitMetricsVars`。
- RustCodeGraph `callers` 对公开静态名报告“定义未找到”，说明当前索引不能为这些 static 产生符号调用边；因此使用精确 `rg` 搜索补充直接引用证据。搜索确认生产 Rust 仅有全局初始化调用，固定句柄引用集中在独立测试；Go 业务消费位于 `pkg/domain/domain.go` 和 `pkg/infoschema/issyncer/loader.go`。
- `pkg/infoschema/metrics/Cargo.toml` 及相关 Cargo manifest：确认 crate 名、唯一直接外部依赖和上游 crate 依赖边。
- 未运行 Cargo：任务是纯文档分析，计划明确要求不运行 Cargo。最终只执行固定 11 章节的结构检查和文档差异自审。
