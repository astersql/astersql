# `pkg/metrics/distsql.rs`

源码：[distsql.rs](distsql.rs)；Go 对照：[distsql.go](distsql.go)。

## 文件定位

本文件属于 `astersql-metrics` crate（[`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"`），由 [`lib.rs`](lib.rs) 以公开模块 `pub mod distsql` 暴露。它是 DistSQL/Coprocessor 指标的**定义与初始化层**：创建七个 Prometheus collector，并把它们保存到包级静态槽位；它不执行分布式查询，也不负责采样时机。

包级主链是 [`metrics::InitMetrics`](metrics.rs) 调用 `distsql::InitDistSQLMetrics`，随后 [`metrics::RegisterMetrics`](metrics.rs) 将七个 collector 注册到默认 Prometheus registry。当前 Rust 仓库搜索只发现初始化、注册和初始化冒烟测试对这些符号的引用，未发现 DistSQL Rust 业务路径对这些 collector 执行 `observe`/`inc`；因此当前事实是“collector 已定义并可注册”，不能据此声称七项运行时数据都已在 Rust 主链采集。

## 核心职责

`InitDistSQLMetrics` 一次构造并发布以下观测面：

- 查询处理耗时：`tidb_distsql_handle_query_duration_seconds`，由 `type`、`sql_type`、`copr_type` 三个标签分维度。
- 扫描规模：每个 partial result 的扫描 key 数 `scan_keys_partial_num`，以及每次查询的扫描 key 总数 `scan_keys_num`。
- 查询拆分规模：每次查询的 partial result 数 `partial_num`。
- Coprocessor 行为：按 `type` 统计缓存 hit/evict/miss，按 `type` 统计 closest read 结果，按 `store` 统计响应体大小。

职责边界很窄：本文件只描述指标元数据、桶和标签，不注册 collector、不选择具体标签值，也不从响应或执行状态计算观测值。注册由 `metrics.rs` 完成；Go 侧的真实采样点位于 `pkg/distsql/select_result.go`、`pkg/store/copr/coprocessor.go` 和 `pkg/store/copr/metrics/metrics.go`。

## 主要符号

| 符号 | 类型 | 语义与关键配置 |
| --- | --- | --- |
| `DistSQLQueryHistogram` | `Option<HistogramVec>` 可变静态量 | 查询耗时（秒）；标签依次为 `LblType`、`LblSQLType`、`LblCoprType`；29 个指数桶 `0.0005 * 2^i`。 |
| `DistSQLScanKeysPartialHistogram` | `Option<Histogram>` 可变静态量 | 单个 partial result 扫描 key 数；未指定桶，兼容层保留 `prometheus` crate 的默认直方图桶。 |
| `DistSQLScanKeysHistogram` | `Option<Histogram>` 可变静态量 | 单次查询扫描 key 总数；使用默认桶。 |
| `DistSQLPartialCountHistogram` | `Option<Histogram>` 可变静态量 | 单次查询产生的 partial result 数；使用默认桶。 |
| `DistSQLCoprCacheCounter` | `Option<CounterVec>` 可变静态量 | Coprocessor 缓存事件；标签名 `type`，标签值由消费方选择。 |
| `DistSQLCoprClosestReadCounter` | `Option<CounterVec>` 可变静态量 | closest/local read 判定；标签名 `type`。 |
| `DistSQLCoprRespBodySize` | `Option<HistogramVec>` 可变静态量 | Coprocessor 响应体大小；标签名 `store`；10 个指数桶 `1 * 2^i`。源码帮助文本声明单位为 bytes。 |
| `InitDistSQLMetrics()` | 公共函数 | 在包级初始化锁内构造全部 collector，最后一次性写入七个静态槽位。 |

标签常量并非在本文件定义，而是通过 `use crate::*` 取得；真实定义位于 [`session.rs`](session.rs) 的 `LblType`、`LblSQLType`、`LblCoprType`、`LblStore`。文件导入的四个 `*Compat` trait 在当前函数体没有直接方法调用，属于机械迁移遗留的兼容导入，不应解释成额外行为。

## 执行流程

1. `InitDistSQLMetrics` 先获取 [`metrics::PACKAGE_INIT_LOCK`](metrics.rs)。锁中毒时立即 panic，避免在无法确认初始化一致性时继续发布部分状态。
2. 依次在局部变量中创建查询耗时、两类扫描 key、partial 数、缓存事件、closest read 和响应大小 collector。构造通过 [`bindinfo::compat_metricscommon`](bindinfo.rs) 把 Go 风格 options 转为 `prometheus` crate 的真实 options，再调用 `astersql-metrics-common` 工厂。
3. `NewHistogramVec`/`NewCounterVec` 固化标签的**名称和顺序**；消费方必须按相同数量和顺序提供标签值。
4. 七个构造都完成后，函数进入一个 `unsafe` 块，把局部 collector 包装为 `Some` 并写入对应 `static mut`。因为发布发生在所有构造之后，不会因某个较早构造成功而留下本函数产生的半套新状态。
5. 正常应用路径由 `metrics::InitMetrics` 的 `Once` 间接调用本函数；注册阶段的 `register_options!` 再逐项读取 `Option`、克隆 collector 并注册。

`InitDistSQLMetrics` 自身没有 `Once`：直接重复调用会在锁内重新创建并替换全部静态值。正常主入口避免了重复初始化，但公开 API 的直接调用者需要理解这一差异。

## 数据与状态

七个静态量初始均为 `None`，表示 Go 包级变量尚未初始化；成功返回后均为 `Some`。这种 `Option + static mut` 是对 Go 可写包级变量的机械映射，不是 Rust 的惰性、安全单例模式。

collector 本身保存 Prometheus 累积状态。`Histogram`/`HistogramVec` 累积样本数、总和和桶计数；`CounterVec` 按标签组合维护只增不减的子计数器。向量指标的标签基数由业务方提供的值决定，尤其 `store` 若直接使用高变动地址会扩大时间序列数量。

查询耗时桶的最小上界为 0.0005 秒，指数倍增 29 次；响应大小桶为 1 到 512 的十个指数上界。Go 运行时观测点 `pkg/store/copr/coprocessor.go` 实际传入 `len(data) / 1024`，而本文件 Help 写的是 bytes，两者存在单位表述差异；当前 Rust 尚无该 collector 的业务观测点，扩展时必须先决定保持 Go 实际 KiB 数值还是修正元数据/采样语义，不能仅凭 Help 猜测。

## 依赖与调用关系

上游调用关系（RustCodeGraph 与源码交叉核验）：

- [`metrics::InitMetrics`](metrics.rs) → `InitDistSQLMetrics`：正常包级初始化；外层 `Once` 保证这条路径只执行一次。
- [`all_metric_initializers_accept_go_metadata`](bindinfo_1_aster_unit_test.rs) → `InitDistSQLMetrics`：独立进程中的构造冒烟检查。
- [`metrics::RegisterMetrics`](metrics.rs) → 七个静态 collector：要求初始化在先，然后克隆并注册。

下游构造依赖：

- [`bindinfo::compat_prometheus`](bindinfo.rs) 提供 Go 字段形状的 `HistogramOpts`/`CounterOpts` 和指数桶函数。
- [`bindinfo::compat_metricscommon`](bindinfo.rs) 转换 options，并调用 `crate::metricscommon`；后者由 [`lib.rs`](lib.rs) 再导出 `astersql-metrics-common`。
- `prometheus = "0.14"` 是 [`Cargo.toml`](Cargo.toml) 的直接依赖；`astersql-metrics-common` 是路径依赖。

Go 对照中的业务消费链包括：`pkg/distsql/select_result.go` 记录查询耗时与 partial 数，`pkg/store/copr/coprocessor.go` 记录 closest-read 和响应大小，`pkg/store/copr/metrics/metrics.go` 取得缓存 hit/evict/miss 子计数器。Rust 的 [`pkg/store/copr/metrics/lib.rs`](../store/copr/metrics/lib.rs) 则在另一个 crate 内独立创建同名 `copr_cache` CounterVec；它不是本文件 `DistSQLCoprCacheCounter` 的引用，这反映出当前 Rust workspace 的父 collector 仍有拆分边界。

## 错误处理与边界

- 获取 `PACKAGE_INIT_LOCK` 使用 `expect("metrics init lock poisoned")`；锁中毒属于不可恢复的初始化不变量破坏，会 panic。
- 指数桶生成和 collector 构造在兼容层中使用 `expect`；无效桶参数、无效指标元数据或标签配置会 panic，而不是由 `InitDistSQLMetrics` 返回 `Result`。
- `InitDistSQLMetrics` 没有返回值，也不执行 registry 注册，所以无法报告重复注册错误。注册错误由 `metrics::RegisterMetrics() -> Result` 传播。
- `RegisterMetrics` 在任何槽位仍为 `None` 时通过 `expect("InitMetrics must run before RegisterMetrics")` panic，故初始化先于注册是硬前置条件。
- 本文件不验证消费方标签数量；`prometheus` 的向量访问会在标签数量不匹配时失败或 panic，扩展时应沿用既有标签顺序。
- 本文件没有针对 NaN、负观测值、极大响应或标签高基数的业务约束；这些应由采样点和独立测试定义。

## 并发与资源生命周期

`PACKAGE_INIT_LOCK` 只串行化各 `Init*Metrics` 函数对包级可变静态量的写入。正常 `InitMetrics` 又由 `INIT_METRICS_ONCE` 包裹，因此标准初始化路径不会并发替换 collector。

该锁没有保护所有读者：七个 `static mut` 的读取仍要求 `unsafe`，公开的 `InitDistSQLMetrics` 也允许外部代码在业务读取期间重新赋值。直接重复初始化可能使 registry 中保留旧 collector 的 clone，而静态槽位指向新 collector，随后观测不会出现在已注册实例中。因此安全扩展应复用 `metrics::InitMetrics`，不要在服务运行后直接重调本函数；若重构，应优先改为 `OnceLock`/`LazyLock` 或提供只读 accessor，并同步消除读写竞态。

collector 由静态槽位持有到进程结束；注册表持有注册 clone。函数不启动线程、不创建异步任务、不打开文件或网络连接，也没有显式清理阶段。测试通过独立子进程隔离默认 registry 与全局静态状态，见 [`main_test.rs`](main_test.rs) 和 [`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs)。

## 与 Go 版本的对应关系

Rust 的七个变量、构造顺序、Namespace/Subsystem/Name/Help、标签名和两组显式指数桶均逐项对应 [`distsql.go`](distsql.go) 的 `InitDistSQLMetrics`。主要语言映射为：

- Go 的 nil 包级指针/接口值 → Rust 的 `Option<collector>`。
- Go 的包初始化顺序 → Rust `metrics::InitMetrics` 的显式调用顺序与 `Once`。
- Go 的 `[]string` 标签名 → Rust `Vec<&'static str>`，再由兼容层转为 `Vec<String>`。
- Go 的直接包级赋值 → Rust 先构造局部变量，再在锁内 `unsafe` 写 `static mut`。

差异与迁移状态：Go 侧存在完整的业务采样调用，Rust 主 `astersql-metrics` crate 当前搜索不到这七个静态量的业务写入；现有 Rust 冒烟测试只证明构造参数可接受，包级注册测试只证明整体注册路径可工作，并未逐项验证 DistSQL 指标名、标签、桶或观测值。另一个 Rust copr metrics crate 复制了缓存 collector，这与 Go 共享 `pkg/metrics` 父 collector 的结构不同。

Go 对照还暴露一个既有语义风险：`DistSQLCoprRespBodySize` 的 Help 声称 bytes，但 Go 采样值除以 1024。本文只记录差异，不把它改写为任何一方已经修复。

## 扩展指南

新增或修改 DistSQL 指标时，应按以下接入点同步处理：

1. 在 `distsql.rs` 增加/修改静态槽位与 `InitDistSQLMetrics` 的局部构造、最终发布；保持先完整构造后发布。
2. 若是新 collector，把它加入 [`metrics::RegisterMetrics`](metrics.rs) 的 `register_options!` 列表，否则仅初始化不会暴露给抓取端。
3. 在真实 Rust 业务采样点接入 `observe`/`inc`，不要把 Go 消费点的存在当作 Rust 已接线。若跨 crate，共享 `astersql-metrics` collector 或清楚记录独立父 collector 的原因，避免产生同名但不同状态的时间序列。
4. 标签名、顺序、指标全名和单位是监控兼容契约；新增无界标签（SQL 文本、动态地址、请求 ID 等）前评估基数和内存开销。
5. 在独立 Rust 测试文件中增加回归测试，建议扩展同目录 [`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs) 或新增独立 `*_test.rs`，验证完整指标名、标签顺序、桶边界和一次实际观测；不要把测试嵌入 `distsql.rs`。
6. 同步检查 [`distsql.go`](distsql.go) 及其消费点，明确是保持 Go 兼容还是有意改变。响应大小单位尤其需要成对验证。

兼容风险主要是 dashboard/告警依赖的指标名和标签变化；正确性风险主要是初始化顺序、重复初始化造成注册实例与写入实例分裂，以及 `static mut` 的并发访问；性能风险主要是高基数标签和过于频繁的 histogram 观测。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、目标文件含 2 个索引符号；`files --filter pkg/metrics/distsql.rs` 确认目标已索引；`explore "pkg/metrics/distsql.rs InitDistSQLMetrics DistSQLQueryHistogram"` 返回 Rust/Go 源码及 Rust 调用者 `metrics.rs::InitMetrics`、`bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata`；`query InitDistSQLMetrics --json` 区分 Rust 与 Go 两个同名函数。精确 `callers/callees` 未返回更多边，故用源码搜索补齐静态量引用。
- 已读实现与配置：[`distsql.rs`](distsql.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`metrics.rs`](metrics.rs)、[`bindinfo.rs`](bindinfo.rs)、[`session.rs`](session.rs)。同目录不存在 `doc.go`。
- 已读 Rust 测试：[`bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs)、[`metrics_test.rs`](metrics_test.rs)、[`metrics_internal_test.rs`](metrics_internal_test.rs)；前者只直接冒烟调用本初始化器，后两者覆盖包级初始化/注册但没有 DistSQL 专项断言。
- 已读 Go 对照与消费点：[`distsql.go`](distsql.go)、[`metrics.go`](metrics.go)、`pkg/distsql/select_result.go`、`pkg/store/copr/coprocessor.go`、`pkg/store/copr/metrics/metrics.go`，以及 closest-read 回归测试 `pkg/executor/distsql_test.go`。
- 已读 Rust 拆分边界：[`pkg/store/copr/metrics/lib.rs`](../store/copr/metrics/lib.rs)、[`pkg/store/copr/metrics/metrics.rs`](../store/copr/metrics/metrics.rs) 及其独立测试 [`migration_aster_unit_test.rs`](../store/copr/metrics/migration_aster_unit_test.rs)。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构检查，并检查 Git diff 只包含本文档与任务文件删除。
