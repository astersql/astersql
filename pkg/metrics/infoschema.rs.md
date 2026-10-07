# `pkg/metrics/infoschema.rs`

## 文件定位

本文件属于 `astersql-metrics` crate（见 `pkg/metrics/Cargo.toml`），是 Go 包 `pkg/metrics/infoschema.go` 的 Rust 兼容门面。它由 crate 根 `pkg/metrics/lib.rs` 以公开模块 `infoschema` 暴露，负责把 InfoSchema V2 缓存指标接到 metrics 包的统一初始化、注册流程，并构造 `TableByName` 的 hit/miss 耗时直方图句柄。

它不实现 InfoSchema、表缓存或查询逻辑。缓存指标的实际 collector 定义在依赖 crate `astersql-infoschema-metrics` 的 `pkg/infoschema/metrics/lib.rs`；缓存事件写入由 `pkg/infoschema/metrics.rs::sieveStatusHookImpl` 完成；全局初始化和注册分别由 `pkg/metrics/metrics.rs::InitMetrics`、`RegisterMetrics` 驱动。

## 核心职责

1. 以七个 `pub static mut Option<...>` 保留 Go 包级变量“初始化前无值、初始化后可用”的形状。
2. `InitInfoSchemaV2Metrics` 克隆并暴露 InfoSchema 子 crate 已有的四个缓存 collector：结果计数、对象数、内存用量、内存上限。Prometheus 句柄的克隆共享底层指标状态，因此该门面与 `sieveStatusHookImpl` 写入的是同一组时间序列。
3. 创建 `tidb_infoschema_table_by_name_duration_nanoseconds` `HistogramVec`，变量标签为 `type`，桶边界为 `1 * 2^i`（`i = 0..29`）。
4. 从该 `HistogramVec` 预绑定 `type="hit"` 和 `type="miss"` 两个 `Histogram` observer，避免调用方重复选择标签。
5. 配合 `pkg/metrics/metrics.rs::RegisterMetrics` 注册五个 collector；两个预绑定 observer 是同一直方图向量的子句柄，不单独注册。

## 主要符号

- `InfoSchemaV2CacheCounter: Option<prometheus::CounterVec>`：带 `type` 标签的缓存结果计数器；底层定义的合法业务值为 `evict`、`hit`、`miss`（`pkg/infoschema/metrics/lib.rs::InfoSchemaV2CacheCounter` 与 `pkg/infoschema/metrics.rs::newSieveStatusHookImpl`）。
- `InfoSchemaV2CacheMemUsage: Option<prometheus::Gauge>`：当前缓存内存用量。
- `InfoSchemaV2CacheObjCnt: Option<prometheus::Gauge>`：当前缓存表对象数。
- `InfoSchemaV2CacheMemLimit: Option<prometheus::Gauge>`：配置的缓存内存上限。
- `TableByNameDuration: Option<prometheus::HistogramVec>`：`TableByName` hit/miss 耗时指标族，完整指标名由 namespace、subsystem、name 组合而成。
- `TableByNameHitDuration: Option<prometheus::Observer>`：预绑定 `type="hit"` 的子直方图；兼容层中 `Observer` 是 `prometheus::Histogram` 的别名。
- `TableByNameMissDuration: Option<prometheus::Observer>`：预绑定 `type="miss"` 的子直方图。
- `pub unsafe fn InitInfoSchemaV2Metrics()`：文件唯一函数和公开初始化入口。`unsafe` 来自对上述可变静态量的写入，而不是 collector 构造本身。

文件没有自定义类型、trait、impl、条件编译项或错误返回类型。

## 执行流程

应用级入口 `pkg/metrics/metrics.rs::InitMetrics` 受 `INIT_METRICS_ONCE` 保护，并在依次初始化其他子系统后调用 `InitInfoSchemaV2Metrics`。本函数的执行顺序如下：

1. 获取 `crate::metrics::PACKAGE_INIT_LOCK`；锁中毒时以 `expect("metrics init lock poisoned")` 终止。
2. 从 `astersql_infoschema_metrics` 的四个 `LazyLock` 静态 collector 取值并克隆，依次写入本文件的四个缓存指标 `Option`。
3. 调用兼容工厂 `metricscommon::NewHistogramVec` 创建 TableByName 耗时向量。选项固定为 namespace `tidb`、subsystem `infoschema`、name `table_by_name_duration_nanoseconds`、一个 `type` 标签和 30 个指数桶。
4. 从刚写入的 `TableByNameDuration` 取引用；若初始化顺序被破坏，则以 `expect("TableByNameDuration must be initialized first")` 终止。
5. 调用兼容 trait `MetricCompat::WithLabelValues`，分别产生 hit、miss observer 并保存。
6. 随后的 `RegisterMetrics` 通过 `register_options!` 注册缓存四项和 `TableByNameDuration`；任何 `Option` 仍为 `None` 都会报告“必须先运行 InitMetrics”的 panic。

运行时缓存事件的另一条链路是 `SieveStatusHook` 回调 → `pkg/infoschema/metrics.rs::sieveStatusHookImpl::{on_hit,on_miss,on_evict,on_update,on_update_limit}` → `astersql_infoschema_metrics` 中的共享 collector。本文件负责初始化门面和注册，不位于每次缓存访问的热路径。

## 数据与状态

七个可变静态量是进程级状态，初值全部为 `None`。初始化后，四个缓存句柄指向 InfoSchema metrics crate 的共享 collector；直方图向量及两个预绑定子句柄由本函数新建。`CounterVec` 只单调累加事件，三个 `Gauge` 表示可升可降的当前快照，`HistogramVec` 累积观测次数、总和和桶计数。

`PACKAGE_INIT_LOCK` 串行化包级指标初始化，外层 `InitMetrics` 的 `Once` 又保证正常应用初始化只发生一次。不过 `InitInfoSchemaV2Metrics` 本身仍是公开的 `unsafe` 函数，测试可以直接重复调用；直接重入会替换本文件保存的 TableByName 直方图句柄，因此调用者必须遵守“初始化完成后不再并发重置”的约束。

桶配置严格保存 Go 参数 `ExponentialBuckets(1, 2, 30)`。名称声明为 `duration_nanoseconds`，Go 调用点也直接把 `time.Since(start)` 转成浮点数；文档不额外推断或改写其单位语义。

## 依赖与调用关系

上游调用者：

- `pkg/metrics/metrics.rs::InitMetrics`：生产初始化入口，在 `INIT_METRICS_ONCE` 内调用本函数。
- `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata`：隔离进程中的初始化冒烟测试，直接在 `unsafe` 块调用。
- Go 对照的 `pkg/metrics/metrics.go::InitMetrics` 同样调用 `InitInfoSchemaV2Metrics`。

下游依赖：

- `astersql-infoschema-metrics`（Cargo 路径 `../infoschema/metrics`）：提供四个 `LazyLock` 缓存 collector。
- `crate::bindinfo::compat_prometheus`：提供 Go 风格选项、`ExponentialBuckets`、`MetricCompat::WithLabelValues` 和 `Observer` 别名。
- `crate::bindinfo::compat_metricscommon`：`NewHistogramVec` 最终委托 `prometheus::HistogramVec::new`，同时注入 metrics-common 的包级常量标签。
- `crate::metrics::PACKAGE_INIT_LOCK`：保护直接初始化的临界区。
- `crate::LblType`：变量标签名 `type`。

`pkg/metrics/metrics.rs::RegisterMetrics` 是下游注册方。当前 Rust 源码搜索没有发现 `TableByNameHitDuration` 或 `TableByNameMissDuration` 在本文件外被调用；`pkg/infoschema/infoschema_v2.rs::TableByName` 当前也没有耗时观测。故直方图已初始化、可注册，但 Rust 查询路径尚未接线，不能描述为已经记录 hit/miss 耗时。

## 错误处理与边界

本函数没有 `Result` 返回值，构造或状态错误采用 panic：初始化锁中毒会 panic；直方图构造器对非法描述、标签或桶配置使用 `expect`；内部初始化顺序异常也会 panic。当前固定参数满足 Prometheus 构造约束。

边界约束包括：

- 调用 `RegisterMetrics` 前必须完成 `InitMetrics`，否则 `register_option` 对 `None` panic。
- `WithLabelValues` 必须恰好提供一个 `type` 标签；本文件固定提供一个值。
- 本文件不校验或限制缓存计数器的标签值；合法值由实际写入端约定。
- 重复向同一 Prometheus registry 注册相同描述符会由注册层返回错误；本文件只负责构造，不处理重复注册。
- 当前没有针对本文件直方图名称、桶边界、observer 写入或重入行为的专门 Rust 断言；现有 metrics 测试只提供初始化冒烟和共享缓存指标行为证据。

## 并发与资源生命周期

Prometheus 的 `CounterVec`、`Gauge`、`HistogramVec` 及其克隆句柄可共享底层状态；collector 生命周期为进程级，不需要显式释放。缓存业务侧通过 `LazyLock` 完成线程安全的首次构造，SIEVE 回调可从多个访问路径更新相同指标。

本文件的风险点是 `static mut`：读写本身不受 Rust 类型系统同步保证，所以 API 标为 `unsafe`，crate 根还允许 `static_mut_refs`。外层 `INIT_METRICS_ONCE` 是生产调用的主要安全边界，`PACKAGE_INIT_LOCK` 则串行化直接初始化。新增消费者不应长期持有跨越再次初始化的静态引用，也不应绕过初始化约束并发读写这些 `Option`。长期演进若移除 Go 兼容形状，应优先迁移到 `OnceLock`/`LazyLock` 等安全的一次性容器。

## 与 Go 版本的对应关系

`pkg/metrics/infoschema.go` 与本文件具有同名的七个变量和同名初始化函数。两者对 TableByName 直方图使用相同的 namespace、subsystem、name、help、`type` 标签、指数桶以及 hit/miss 预绑定方式；两者也都由各自的 `metrics.InitMetrics` 初始化并由注册入口注册 collector。

主要差异有三点：

1. Go 在本函数中重新构造四个缓存 collector；Rust 改为克隆 `pkg/infoschema/metrics/lib.rs` 的共享 collector，以便 `sieveStatusHookImpl` 的写入和全局 metrics 注册落到同一状态。
2. Go 包变量是未显式包装的 collector 接口/指针；Rust 用 `Option` 表达初始化前状态，并因 `static mut` 暴露 `unsafe` 初始化。
3. Go 的 `pkg/infoschema/infoschema_v2.go` 在 `TableByName` 命中和未命中路径分别调用两个 observer；Rust 的 `pkg/infoschema/infoschema_v2.rs::TableByName` 当前只执行查找、缓存 Get/Set 和错误返回，未发现对应观测调用。这是迁移接线缺口，不应由本说明宣称已实现。

共享缓存指标的 Rust 行为由 `pkg/infoschema/metrics_test.rs::sieve_status_hook_updates_shared_prometheus_metrics` 验证：evict/hit/miss 各递增一次，内存、对象数和上限 Gauge 更新为指定值。

## 扩展指南

- 新增缓存结果标签时，应同时修改真实写入端 `pkg/infoschema/metrics.rs::newSieveStatusHookImpl`、钩子行为测试和 Go 对照；仅在本门面创建子句柄不会产生业务观测。
- 修改 metric 名称、namespace、subsystem、help、标签或桶会破坏仪表盘和告警兼容性。应同步检查 `pkg/metrics/infoschema.go`、Prometheus 注册清单及相关 dashboard/rule，并在独立测试文件中断言完整指标名、标签和桶。
- 若要补齐 Rust TableByName 耗时观测，应在 `pkg/infoschema/infoschema_v2.rs::TableByName` 的实际 hit/miss 分支接线，并把测试放在独立的 `*_test.rs` 文件；需要明确 special-table、元数据缺失、缓存 miss 后加载失败等分支是否计入 miss，严格对照 Go 行为。
- 新 collector 必须加入 `InitInfoSchemaV2Metrics` 的初始化顺序和 `pkg/metrics/metrics.rs::RegisterMetrics` 的注册清单；若真实所有者仍是 infoschema 子 crate，还需更新 `pkg/infoschema/metrics/Cargo.toml`/`lib.rs` 对应定义。
- 不应在本生产文件内嵌测试；优先扩展 `pkg/metrics/bindinfo_1_aster_unit_test.rs`（初始化契约）或新增同目录独立测试文件，并用 `pkg/infoschema/metrics_test.rs` 覆盖缓存写入端。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件已索引为 84 行、11 个符号。
- RustCodeGraph `node --file pkg/metrics/infoschema.rs`：核对七个静态量、初始化锁、四个共享 collector 克隆、直方图参数和 observer 派生顺序。
- RustCodeGraph `query InitInfoSchemaV2Metrics` 与 `explore "InitInfoSchemaV2Metrics in pkg/metrics/metrics.rs"`：确认 Go/Rust 两个定义，并识别 Rust `InitMetrics` 和初始化冒烟测试调用者。精确 `callers`/`callees` 对该 Rust 节点返回空边，因此又以索引的 `explore` 结果和直接引用搜索补齐图未覆盖的边。
- `pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`：核对 crate 边界、公开模块和 `astersql-infoschema-metrics`/`prometheus` 依赖。
- `pkg/metrics/metrics.rs::{InitMetrics,RegisterMetrics}`：核对一次性生产初始化、调用顺序以及五个 collector 的注册清单。
- `pkg/metrics/bindinfo.rs::compat_prometheus`、`pkg/metrics/common/wrapper.rs::NewHistogramVec`：核对 `Observer`、标签绑定、指数桶和常量标签注入语义。
- `pkg/infoschema/metrics/lib.rs`、`pkg/infoschema/metrics.rs`、`pkg/infoschema/metrics_test.rs`：核对缓存 collector 的真实所有权、写入钩子及共享状态测试。
- `pkg/infoschema/infoschema_v2.rs::TableByName`、`pkg/infoschema/infoschema_v2.go`：核对 Rust 当前未接耗时 observer、Go 已在 hit/miss 分支观测的差异。
- `pkg/metrics/infoschema.go`、`pkg/metrics/metrics.go`：核对 Go 指标元数据、初始化和注册语义。
- `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata`：核对本初始化函数已有的 Rust 冒烟覆盖；本任务按计划只做文档分析，未运行 Cargo。
