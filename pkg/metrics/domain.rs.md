# `pkg/metrics/domain.rs`

## 文件定位

[`domain.rs`](domain.rs) 是 `astersql-metrics` crate 中 Domain 子系统的 Prometheus collector 定义文件。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod domain` 暴露它；包级入口 [`metrics.rs`](metrics.rs) 的 `InitMetrics` 调用 `crate::domain::InitDomainMetrics`，随后 `RegisterMetrics` 将其中多数 collector 注册到默认 Prometheus registry。这里的 “Domain” 指 schema、权限、系统变量和 schema validator 等全局状态相关的观测面，并不是这些业务状态本身的实现。

[`Cargo.toml`](Cargo.toml) 将该目录定义为 `astersql-metrics`，库入口为 `lib.rs`；本文件直接使用 crate 内的 Go API 兼容层，最终 collector 类型来自 `prometheus = "0.14"`。文件没有 feature 或条件编译分支，也不定义类型、trait 或 `impl`。

## 核心职责

本文件承担三项职责：

1. 声明 7 个可变包级 collector 槽位：租约过期时间、schema 加载次数与耗时、InfoCache 读取/命中次数、权限加载次数、系统变量缓存加载次数、schema validator 处理次数。
2. 固定跨模块共享的标签值：InfoCache 的 `get`/`hit`，以及 schema validator 的 `stop`、`restart`、`reset`、`cache_empty`、`cache_miss`。
3. 由 `InitDomainMetrics` 一次构造所有 collector，并在持有 `PACKAGE_INIT_LOCK` 时将它们写入对应的 `Option` 静态槽位。

它只负责 collector 的定义和初始化，不负责采样业务事件，也不直接注册 collector。初始化、注册和简化模式切换分别由 [`metrics.rs`](metrics.rs) 的 `InitMetrics`、`RegisterMetrics`、`ToggleSimplifiedMode` 组织。

## 主要符号

- `LeaseExpireTime: Option<prometheus::Gauge>`：指标全名 `tidb_domain_lease_expire_time`，保存最近一次 lease 过期的 Unix 秒值。
- `LoadSchemaCounter: Option<prometheus::CounterVec>`：`tidb_domain_load_schema_total`，动态标签为 `type`。
- `LoadSchemaDuration: Option<prometheus::HistogramVec>`：`tidb_domain_load_schema_duration_seconds`，动态标签为 `action`；桶为 `ExponentialBuckets(0.001, 2.0, 20)`，即从 1 ms 开始的 20 个二倍指数桶，最大显式上界约 524.288 秒。
- `InfoCacheCounters: Option<prometheus::CounterVec>`：`tidb_domain_infocache_counters`，动态标签按 `action`、`type` 的顺序组成时序身份。
- `LoadPrivilegeCounter: Option<prometheus::CounterVec>`：`tidb_domain_load_privilege_total`，动态标签为 `type`。
- `LoadSysVarCacheCounter: Option<prometheus::CounterVec>`：`tidb_domain_load_sysvarcache_total`，动态标签为 `type`。
- `HandleSchemaValidate: Option<prometheus::CounterVec>`：`tidb_domain_handle_schema_validate`，动态标签为 `type`。
- `InfoCacheCounterGet`、`InfoCacheCounterHit`：InfoCache `action` 标签值 `get`、`hit`。
- `SchemaValidatorStop`、`SchemaValidatorRestart`、`SchemaValidatorReset`、`SchemaValidatorCacheEmpty`、`SchemaValidatorCacheMiss`：schema validator 状态/结果标签值。
- `InitDomainMetrics()`：本文件唯一函数和初始化入口。它先取得 `crate::metrics::PACKAGE_INIT_LOCK`，通过 `metricscommon::NewGauge`、`NewCounterVec`、`NewHistogramVec` 构造局部 collector，最后在一个 `unsafe` 块中一次写入全部静态槽位。

所有上述符号都是公开的；命名刻意保留 Go 风格，因此 crate 根在 [`lib.rs`](lib.rs) 允许 `non_snake_case`、`non_upper_case_globals` 和 `static_mut_refs`。

## 执行流程

1. 应用或测试调用 [`metrics.rs`](metrics.rs) 的 `InitMetrics`。该函数由 `INIT_METRICS_ONCE` 保证全进程只执行一次，并按固定顺序调用各子系统初始化器；Domain 位于 DistSQL 之后、Executor 之前。
2. `InitDomainMetrics` 获取 `PACKAGE_INIT_LOCK`。该包级互斥锁把各个会写 `static mut` 的初始化器串行化；锁中毒会以 `expect("metrics init lock poisoned")` 终止。
3. 函数依次构造 1 个 Gauge、5 个 CounterVec 和 1 个 HistogramVec。每个指标使用固定的 `tidb` namespace 和 `domain` subsystem，并设置与 Go 版本一致的 name、help、标签及直方图桶。
4. 构造全部成功后，函数在 `unsafe` 块中把局部值包装为 `Some`，覆盖 7 个静态槽位。局部先构造、最后集中写入，避免正常路径下只写入前半组 collector。
5. `RegisterMetrics` 从槽位取值并注册 `LoadPrivilegeCounter`、`LeaseExpireTime`、`LoadSchemaCounter`、`LoadSchemaDuration`、`HandleSchemaValidate`、`LoadSysVarCacheCounter`；槽位为 `None` 时会以 “InitMetrics must run before RegisterMetrics” panic。
6. `ToggleSimplifiedMode` 单独使用 `InfoCacheCounters`：进入简化模式时注销，退出时重新注册。当前 Rust `RegisterMetrics` 的注册列表没有 `InfoCacheCounters`，与 Go `RegisterMetrics` 的显式注册不同；因此不能仅根据本文件推断它在 Rust 默认 registry 中已经注册。

## 数据与状态

collector 槽位是进程级 `pub static mut Option<T>`：初始为 `None`，初始化后为 `Some(T)`。`CounterVec`/`HistogramVec` 本身按标签值惰性产生子时序；标签名及顺序是公开的兼容契约，改变它们会改变 Prometheus 时序身份和调用参数个数。

初始化工厂链为 `domain.rs` 的 `metricscommon::*` → [`bindinfo.rs`](bindinfo.rs) 的 `compat_metricscommon` → `astersql_metrics_common`（本仓库实现位于 [`common/wrapper.rs`](common/wrapper.rs)）。最终工厂会用包级常量标签替换调用者传入的常量标签，再调用 `prometheus` crate 构造 collector。这意味着外部设置的全局常量标签也是 collector 状态的一部分。

本文件不缓存业务对象、不维护 schema 版本，也不产生后台任务。所有计数和值都存放在 Prometheus collector 内部；本文件只持有这些 collector 的句柄。

## 依赖与调用关系

上游关系：

- [`lib.rs`](lib.rs) 声明公开 `domain` 模块。
- [`metrics.rs`](metrics.rs) 的 `InitMetrics` 调用 `InitDomainMetrics`；RustCodeGraph 的文件节点也将 `metrics.rs` 列为本文件使用者。
- [`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs) 的 `all_metric_initializers_accept_go_metadata` 直接调用 `InitDomainMetrics`，验证这些 Go 风格元数据可被构造器接受。
- `RegisterMetrics` 读取 6 个 Domain collector；`ToggleSimplifiedMode` 读取 `InfoCacheCounters`。

下游关系：

- `crate::metrics::PACKAGE_INIT_LOCK` 提供初始化写入的互斥边界。
- `crate::bindinfo::compat_metricscommon` 负责将 Go 风格 opts 和标签集合转换为 Rust Prometheus opts。
- `crate::bindinfo::compat_prometheus` 提供 Go 风格选项结构、`ExponentialBuckets` 及兼容 trait；本文件导入的 `CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat` 在当前文件中没有方法调用，仅作为迁移期兼容导入存在。
- crate 根的 `LblType`、`LblAction` 提供标签名。

仓库搜索表明，Rust 业务侧没有直接读写这些 `static mut` collector；已迁移的 InfoSchema 指标句柄使用独立 crate [`../infoschema/metrics/metrics.rs`](../infoschema/metrics/metrics.rs) 中的 `LazyLock` collector。相反，Go 业务调用仍分布在 `pkg/domain/domain.go`、`pkg/infoschema/isvalidator/validator.go`、`pkg/infoschema/issyncer/syncer.go` 和 `pkg/infoschema/metrics/metrics.go`。这是当前迁移接线边界，不应把 Go 调用者描述成 Rust 调用者。

## 错误处理与边界

`InitDomainMetrics` 不返回 `Result`。无效的指标元数据会在 [`common/wrapper.rs`](common/wrapper.rs) 的构造工厂中经 `expect("invalid ... options")` panic；初始化锁中毒也会 panic。当前固定 name、help 和 label 配置是有效常量，冒烟测试覆盖了构造路径，但没有把构造失败转换为可恢复错误。

注册错误属于 [`metrics.rs`](metrics.rs) 的责任：重复/冲突注册可从 `RegisterMetrics` 返回 `prometheus::Error`；未初始化的槽位则会 panic。简化模式注销时忽略单项注销错误，重新注册时传播错误。

调用方绑定标签时必须严格匹配向量声明的标签数量和顺序。尤其 `InfoCacheCounters` 是 `[action, type]`，`LoadSchemaDuration` 是 `[action]`；不匹配会在 Prometheus API 层失败或 panic，不能通过本文件自动修正。Counter 只能单调递增，Gauge 用于时间点值，Histogram 的单位由名称和 help 约定为秒。

## 并发与资源生命周期

生命周期分为构造、注册、采样、可选注销四段。`InitMetrics` 外层的 `Once` 防止正常主链重复初始化；`InitDomainMetrics` 内层的 `PACKAGE_INIT_LOCK` 只串行化赋值过程。直接绕过 `InitMetrics` 多次调用 `InitDomainMetrics` 仍会替换槽位中的 collector，因此公开函数本身不具备幂等或一次性语义。

`static mut` 的读取和写入需要调用侧遵守时序并进入 `unsafe`；互斥锁并未封装所有后续读取。代码依赖“初始化完成后再注册/使用，且不再直接重初始化”的进程启动不变量。collector 克隆共享内部原子状态，注册表持有的 clone 与槽位句柄指向同一采样状态；退出进程时由 Rust 正常释放，无显式 shutdown。

`ToggleSimplifiedMode` 通过自己的 `MODE: Mutex<bool>` 串行化注册状态转换，但该锁与 `PACKAGE_INIT_LOCK` 不同。安全扩展时不能假设两个锁形成统一临界区，也不要在采样并发进行时重新调用 `InitDomainMetrics` 替换 collector。

## 与 Go 版本的对应关系

直接对照文件是 [`domain.go`](domain.go)。Rust 保留了 Go 的 7 个 collector、7 个标签字符串、完整指标名/help、标签排列，以及 `LoadSchemaDuration` 的 `0.001 × 2^n`、20 桶配置。Go 使用已初始化的包级具体值/指针；Rust 为配合分阶段初始化，改为 `static mut Option<T>`，并增加 `PACKAGE_INIT_LOCK` 和集中 `unsafe` 写入。

Go 包的 `init()` 自动调用 `InitMetrics`；Rust 没有等价的自动包初始化，必须由 Rust 应用入口显式调用 `InitMetrics`。Go `RegisterMetrics` 注册 `InfoCacheCounters`，当前 Rust `RegisterMetrics` 未列出它，但 Rust 简化模式切换仍操作它。这一差异影响默认 registry 可见性，扩展或修正时需先确认期望，而不能仅机械复制调用。

Go 业务侧真实使用包括：Domain 加载权限/系统变量后的成功失败计数，schema validator 的状态计数和 lease 时间设置，schema syncer 的成功失败计数，以及 InfoSchema 的缓存与 schema 加载耗时句柄。当前 Rust 对应业务接线主要迁入独立 `astersql-infoschema-metrics` crate；本文件的多数 collector 暂未被 Rust 业务代码直接采样。

## 扩展指南

新增 Domain 指标时，应在 `domain.rs` 增加槽位并在 `InitDomainMetrics` 中构造和集中赋值，同时检查 [`metrics.rs`](metrics.rs) 的注册列表、简化模式列表及应用初始化顺序。若指标属于已经拆出的 InfoSchema metrics crate，应优先确认所有权，避免在两个 crate 中创建同名 collector 导致注册冲突。

修改现有指标时必须保持 Prometheus 兼容性：namespace/subsystem/name 决定全名，label 名称及顺序决定时序，桶边界决定直方图聚合语义。名称、标签或桶的变化都可能破坏 dashboard、告警和历史数据连续性。新增标签还会增加基数，必须评估内存和采集成本。

测试逻辑应继续放在独立测试文件，不要内嵌到 `domain.rs`。构造/初始化冒烟测试可扩展 [`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs)；指标全名、标签和桶的语义测试可参考或扩展 [`../infoschema/metrics/lib_test.rs`](../infoschema/metrics/lib_test.rs) 与 [`../infoschema/metrics/migration_aster_unit_test.rs`](../infoschema/metrics/migration_aster_unit_test.rs)。若修正 `InfoCacheCounters` 注册差异，应在 `pkg/metrics` 的独立测试中验证 `InitMetrics` → `RegisterMetrics` → `gather` 以及简化模式往返，避免只验证对象构造。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 `pkg/metrics/domain.rs` 已收录；文件节点读取了完整 158 行，并报告使用者为 `pkg/metrics/metrics.rs` 与 `pkg/metrics/bindinfo_1_aster_unit_test.rs`。
- RustCodeGraph 精确查询：`InitDomainMetrics` 在 Go/Rust 对照文件中各有一个定义；目标节点确认本文件包含 7 个 collector 槽位、7 个标签常量和 1 个初始化函数。通用名称的 `callers/callees` 查询未在时限内返回，因此调用边又由索引文件节点及下列源码位置交叉验证。
- Rust 源码：[`domain.rs`](domain.rs)、[`lib.rs`](lib.rs)、[`metrics.rs`](metrics.rs)、[`bindinfo.rs`](bindinfo.rs)、[`common/wrapper.rs`](common/wrapper.rs)、[`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs)。
- crate/迁移边界：[`Cargo.toml`](Cargo.toml)；InfoSchema 独立实现与测试为 [`../infoschema/metrics/metrics.rs`](../infoschema/metrics/metrics.rs)、[`../infoschema/metrics/lib_test.rs`](../infoschema/metrics/lib_test.rs)、[`../infoschema/metrics/migration_aster_unit_test.rs`](../infoschema/metrics/migration_aster_unit_test.rs)。
- Go 对照与真实调用：[`domain.go`](domain.go)、[`metrics.go`](metrics.go)、`pkg/domain/domain.go`、`pkg/infoschema/isvalidator/validator.go`、`pkg/infoschema/issyncer/syncer.go`、`pkg/infoschema/metrics/metrics.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行固定 11 章节结构检查和人工事实复核。
