# `pkg/metrics/bindinfo.rs`

## 文件定位

本文件属于 `astersql-metrics` crate。crate 由 [`pkg/metrics/Cargo.toml`](Cargo.toml) 定义，入口 [`pkg/metrics/lib.rs`](lib.rs) 以私有 `mod bindinfo` 装载本模块，再用 `pub use bindinfo::*` 将其公开项提升到 crate 根。它同时承担两类职责：定义执行计划绑定缓存的五个 Prometheus 指标，以及为仍保持 Go API 形状的 sibling metrics 移植代码提供 `compat_prometheus`、`compat_metricscommon` 兼容层。

应用侧初始化入口是 [`metrics::InitMetrics`](metrics.rs)：该函数在其一次性初始化序列中调用 `crate::bindinfo::init_bind_info_metrics()`。这说明本文件处于指标基础设施的初始化链，而不是 SQL 绑定选择、匹配或淘汰算法本身；真正的 Go 业务写入点可见 [`pkg/bindinfo/binding_cache.go`](../bindinfo/binding_cache.go)。

## 核心职责

- `init_bind_info_metrics` 惰性创建 `tidb_server` 子系统下的命中数、未命中数、内存使用、内存上限和绑定条目数五个 collector；对应静态槽位是 `BINDING_CACHE_*` 五个 `OnceLock`。
- `binding_cache_*` 五个访问器返回进程生命周期内稳定的 `&'static Counter/Gauge`，即使调用方没有先执行初始化，也会通过相同构造路径完成惰性初始化。
- `compat_prometheus` 把 `prometheus` crate 的类型包装成 Go 风格名称，提供选项结构、默认注册表适配器、桶生成函数和 `WithLabelValues`/`Add`/`Set`/`Observe` 等兼容 trait。
- `compat_metricscommon` 把 Go 风格选项转换成 Rust `prometheus::Opts`/`HistogramOpts`，并委托 `crate::metricscommon::New*` 工厂注入包级常量标签。它被 `ddl.rs`、`session.rs`、`server.rs` 等多个 sibling 模块复用，不只服务 bindinfo 指标。
- 本文件只创建 collector，不负责将五个 bindinfo collector 注册到 Prometheus registry。当前 [`metrics::RegisterMetrics`](metrics.rs) 的显式清单没有这些访问器；因此“初始化完成”不能等同于“已暴露到默认 registry”。

## 主要符号

- `compat_prometheus::CounterOpts`/`GaugeOpts`：保存静态的 `Namespace`、`Subsystem`、`Name`、`Help`；`GaugeOpts` 是前者的类型别名。
- `compat_prometheus::HistogramOpts`：在上述元数据外保存 `Buckets: Vec<f64>`；空桶由转换逻辑解释为使用库默认值。
- `compat_prometheus::SummaryOpts` 与 `SummaryVec`：保留 Go Summary API 的调用形状，但 `SummaryVec` 实际别名为 `prometheus::HistogramVec`，不提供 Go Summary 的客户端流式分位数。
- `DefaultRegistry`/`DefaultRegisterer`：实现 `crate::promutil::Registry`，把注册、强制注册和注销转发给 `prometheus::default_registry()`；`MustRegister` 遇到注册错误会 panic。
- `ExponentialBuckets`：调用上游 `prometheus::exponential_buckets`，非法参数以 `expect` 失败；`ExponentialBucketsRange` 先断言 `min > 0`、`max > min`、`count >= 2`，再推导倍率；`exponential_buckets` 是 snake_case 转发别名。
- `MetricCompat`：为 `CounterVec`、`GaugeVec`、`HistogramVec` 提供 Go 风格标签取子指标接口。只有 CounterVec/GaugeVec 覆盖删除逻辑；HistogramVec 使用 trait 的默认 `false` 实现。
- `CounterCompat`、`GaugeCompat`、`ObserverCompat`：分别将 `Add`、`Set`、`Observe` 转发给 `inc_by`、`set`、`observe`。
- `compat_metricscommon::LabelNames`：当前仅实现 `Vec<&'static str>` 与 `&[&'static str; N]` 到 `Vec<String>` 的转换，因此调用方标签名必须满足这些实现之一。
- `counter_opts`/`histogram_opts`：设置 namespace、subsystem、名称、帮助文本和空常量标签；后续 `metricscommon::New*` 会以包级常量标签替换该空映射。
- `NewCounter`、`NewGauge`、`NewHistogram`、三个向量构造器与 `NewSummaryVec`：统一委托 `crate::metricscommon`；`NewSummaryVec` 先构造空桶 `HistogramOpts` 再创建 HistogramVec。
- `BINDING_CACHE_HIT_COUNTER`、`BINDING_CACHE_MISS_COUNTER`、`BINDING_CACHE_MEM_USAGE`、`BINDING_CACHE_MEM_LIMIT`、`BINDING_CACHE_NUM_BINDINGS`：五个进程级惰性存储槽位。
- `counter`/`gauge`：内部构造助手，固定 namespace=`tidb`、subsystem=`server`。
- `init_bind_info_metrics`：Rust 风格初始化入口；`InitBindInfoMetrics` 是保持 Go 命名的薄转发入口。
- `binding_cache_hit_counter`、`binding_cache_miss_counter`、`binding_cache_mem_usage`、`binding_cache_mem_limit`、`binding_cache_num_bindings`：公开读写句柄入口。

## 执行流程

1. 进程或测试调用 [`metrics::InitMetrics`](metrics.rs)，其 `Once` 保护的初始化闭包首先调用 `init_bind_info_metrics`；也可直接调用 `InitBindInfoMetrics`。
2. `init_bind_info_metrics` 依次对五个 `OnceLock` 执行 `get_or_init`。Counter 走 `counter`，Gauge 走 `gauge`，并固定生成 `tidb_server_<name>` 全名。
3. `counter`/`gauge` 构造 `prometheus::Opts` 后委托 `metricscommon::NewCounter`/`NewGauge`。[`common/wrapper.rs`](common/wrapper.rs) 中的工厂读取包级常量标签并创建上游 collector。
4. 业务调用方应通过相应 `binding_cache_*` 访问器取得静态引用，再调用 counter 的 `inc`/`inc_by` 或 gauge 的 `set`。访问器自身再次使用 `get_or_init`，所以初始化顺序不会产生空引用。
5. 若 sibling 移植模块使用兼容构造器，调用从 `compat_prometheus`/`compat_metricscommon` 的 Go 风格 API 进入，转换选项与标签名后落到同一 `metricscommon::New*` 工厂。

当前接线边界必须单独看待：初始化流程只创建对象；默认 registry 的暴露由 [`metrics::RegisterMetrics`](metrics.rs) 的显式注册清单决定，而该清单当前未列出本文件的五个访问器。

## 数据与状态

五个 `OnceLock` 是本文件唯一的 bindinfo 运行时状态，每个槽位至多成功初始化一次，之后访问器总是返回同一 collector。两个 Counter 表示单调累计事件数；三个 Gauge 表示可升可降的当前值。指标元数据与 Go 文件一致：

| 访问器 | 类型 | Prometheus 全名 | 含义 |
| --- | --- | --- | --- |
| `binding_cache_hit_counter` | Counter | `tidb_server_binding_cache_hit_total` | 缓存命中累计次数 |
| `binding_cache_miss_counter` | Counter | `tidb_server_binding_cache_miss_total` | 缓存未命中累计次数 |
| `binding_cache_mem_usage` | Gauge | `tidb_server_binding_cache_mem_usage` | 当前缓存内存用量 |
| `binding_cache_mem_limit` | Gauge | `tidb_server_binding_cache_mem_limit` | 当前缓存内存上限 |
| `binding_cache_num_bindings` | Gauge | `tidb_server_binding_cache_num_bindings` | 当前绑定条目数 |

collector 构造时会快照 [`common/wrapper.rs`](common/wrapper.rs) 的包级常量标签；之后修改全局标签不会重建已被 `OnceLock` 固定的 collector。`DefBuckets` 是空向量占位而非实际边界集合。兼容 Summary 也持有 Histogram 的桶、计数和总和语义，而非 Summary quantile 状态。

## 依赖与调用关系

- 上游初始化边：`metrics::InitMetrics -> bindinfo::init_bind_info_metrics -> counter/gauge -> metricscommon::NewCounter/NewGauge`。RustCodeGraph 对目标文件的被调用关系也识别出 `init_bind_info_metrics -> counter/gauge` 与 `InitBindInfoMetrics -> init_bind_info_metrics`。
- 上游使用边：`pkg/metrics/bindinfo_1_aster_unit_test.rs` 直接调用五个访问器并写入/读回值。全仓 `rg` 未发现这五个 Rust 访问器在非测试生产文件中的调用。
- 兼容层调用者：`pkg/metrics/ddl.rs`、`distsql.rs`、`domain.rs`、`executor.rs`、`server.rs`、`session.rs`、`ttl.rs` 等直接导入 `compat_prometheus` 和/或 `compat_metricscommon`；因此修改兼容 API 的影响范围远大于五个 bindinfo 指标。
- 下游 crate：直接使用 `prometheus = "0.14"`、`astersql-metrics-common` 和经 `lib.rs` 再导出的 `astersql-util-promutil`。Cargo 未为本文件设置条件 feature。
- Go 业务边：[`pkg/bindinfo/binding_cache.go`](../bindinfo/binding_cache.go) 会设置 `BindingCacheMemUsage`、`BindingCacheMemLimit` 和 `BindingCacheNumBindings`；Rust 同名访问器当前尚未在 `pkg/bindinfo` 生产实现中接线。
- 注册边：Go [`metrics.go`](metrics.go) 显式 `MustRegister` 五个全局 collector；Rust [`metrics.rs`](metrics.rs) 的 `RegisterMetrics` 当前未包含它们。这是可观测性接线差异，不应由本文档假定为已完成。

## 错误处理与边界

- `OnceLock::get_or_init` 保证重复初始化无害，但初始化闭包若 panic 不会留下已初始化值，下一次仍可重试。
- `ExponentialBuckets` 对上游返回的错误使用 `expect`；`ExponentialBucketsRange` 对非法范围直接 `assert!`。这些 API 不返回 `Result`，调用方必须预先保证参数有效。
- 所有 `New*` 构造器最终都对无效 Prometheus 元数据使用 `expect`；名称、标签或桶配置错误会 panic，而非向上传播错误。
- `DefaultRegistry::Register` 保留 `prometheus::Result<()>`；`MustRegister` 将错误升级为 panic；`Unregister` 只返回成功布尔值并丢弃具体错误。
- `MetricCompat::DeleteLabelValues` 对 HistogramVec 返回默认 `false`，不是实际删除尝试；调用方不能据此推断标签子序列已被清除。
- `NewSummaryVec` 是明确的兼容降级，不能提供 Go Summary 的 quantile 行为。若调用方依赖 quantile 输出，需要改用可验证的 Histogram 桶策略或引入有对应语义的实现。
- 本文件无 I/O、SQL、事务或可恢复业务错误；主要失败模式是构造/注册时 panic、注册重复错误以及尚未接线造成指标不暴露。

## 并发与资源生命周期

`OnceLock` 为五个 collector 提供线程安全、一次性的发布；返回的 `&'static` 引用与进程同生命周期，无释放或替换路径。Counter/Gauge 的数值更新由 `prometheus` collector 自身实现并发安全，本文件不额外加锁。

全局初始化还有外层 [`metrics::InitMetrics`](metrics.rs) 的 `Once`，但访问器自身的惰性初始化使其不依赖外层调用时序。两层一次性保护不会创建两个 collector：它们访问相同静态槽位。包级常量标签则由 [`common/wrapper.rs`](common/wrapper.rs) 的 `OnceLock<RwLock<Labels>>` 保护，并在 collector 创建时读取快照。

`DefaultRegistry` 没有内部状态；注册表资源由上游 `prometheus` 全局 registry 持有。注册需要传入 boxed clone，注销也要求能描述同一 collector；重复注册可能返回错误或在 `MustRegister` 中 panic。文件不创建线程、异步任务、通道或事务。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/metrics/bindinfo.go`](bindinfo.go)。两侧保持完全相同的五组 namespace、subsystem、name、help 和 Counter/Gauge 类型。Go 使用可重新赋值的包级变量，由 `InitBindInfoMetrics` 每次覆盖；Rust 用私有 `OnceLock` 加访问器，初始化后不可替换，并额外提供 snake_case 与 Go 风格两个入口。

Go 的 `metrics.InitMetrics` 调用 `InitBindInfoMetrics`，Rust 的 `metrics::InitMetrics` 调用 `init_bind_info_metrics`，初始化顺序一致。Go `metrics.RegisterMetrics` 显式注册五个 collector，Rust 当前注册清单缺少对应项。Go `pkg/bindinfo/binding_cache.go` 会更新三个 Gauge；Rust 全仓生产代码未找到五个访问器的调用，因此目前只能确认“collector 定义与元数据已迁移”，不能确认“业务更新和暴露链已完整迁移”。

两个 `compat_*` 模块不是 Go `bindinfo.go` 的逐行内容，而是为更广泛 Go-to-Rust metrics 移植增加的局部基础设施。其中 SummaryVec 到 HistogramVec 的映射是语义降级，必须在新增指标时显式评估。

## 扩展指南

- 新增 bindinfo 指标时，应同步增加静态 `OnceLock`、初始化语句、公开访问器，并保持 `counter`/`gauge` 的 `tidb/server` 命名约定；若 Go 仍是对照基准，也要核对 [`bindinfo.go`](bindinfo.go) 的名称、帮助文本和类型。
- 要让指标实际可抓取，必须同时把 collector clone 接入 [`metrics::RegisterMetrics`](metrics.rs)，并检查重复注册/注销测试；仅加入 `init_bind_info_metrics` 不足以完成暴露。
- 要让指标反映真实业务状态，应在 `pkg/bindinfo` 对应缓存命中、未命中、容量变化或条目变化路径调用访问器。接线前要核对 Go [`binding_cache.go`](../bindinfo/binding_cache.go) 的更新时机，避免只在初始化或部分分支更新。
- 修改 `compat_prometheus` 或 `compat_metricscommon` 前，应搜索所有 sibling metrics 调用者；其公开构造器和 trait 是 crate 内迁移兼容面，标签类型、panic 行为或 Summary 降级的变化可能广泛影响指标元数据。
- 测试应继续放在独立文件 [`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs)，不要内嵌进生产源。新增 collector 至少验证全名、类型行为、重复初始化和注册可见性；新增业务接线则在 `pkg/bindinfo` 的独立测试中验证状态变化。
- 性能上不要在热路径重复拼装标签名或创建 collector；复用静态访问器。兼容性上保持现有 Prometheus 全名和标签集合，否则会破坏仪表盘、告警和查询。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/metrics` 确认 `bindinfo.rs`、Go 对照和独立测试均被索引；`node --file pkg/metrics/bindinfo.rs --offset 1 --limit 500` 读取了完整 372 行与 52 个符号。
- RustCodeGraph 精确查询：`query InitBindInfoMetrics` 同时定位 Go `bindinfo.go:32` 与 Rust `bindinfo.rs:370`；`query init_bind_info_metrics`、`query binding_cache_hit_counter`、`query binding_cache_mem_usage` 定位 Rust 公开入口。目标文件调用图确认 `InitBindInfoMetrics -> init_bind_info_metrics`、初始化函数到 `counter/gauge`、构造器到 `counter_opts/histogram_opts/metricscommon::New*` 的边。
- 已读 Rust/Cargo 路径：[`bindinfo.rs`](bindinfo.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`metrics.rs`](metrics.rs)、[`common/wrapper.rs`](common/wrapper.rs)。`pkg/metrics` 当前没有 `doc.go`。
- 已读对照与测试：[`bindinfo.go`](bindinfo.go)、[`metrics.go`](metrics.go)、[`../bindinfo/binding_cache.go`](../bindinfo/binding_cache.go)、[`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs)。独立 Rust 测试验证五个 `tidb_server_*` 全名及 Counter/Gauge 写入读回；Go 搜索确认三个 Gauge 的业务写入和五个 collector 的注册位置。
- 全仓直接引用搜索：五个 Rust `binding_cache_*` 访问器仅出现在定义与 `bindinfo_1_aster_unit_test.rs`；`compat_*` 被多个 `pkg/metrics/*.rs` 生产模块导入；`metrics::InitMetrics` 被 server/session/statistics 等测试初始化入口调用。
- 本任务为纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证，确保本文恰有十一个固定二级标题；行为陈述则由上述源码、调用边、Cargo 和 Go/测试对照人工复核。
