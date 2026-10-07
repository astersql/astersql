# `pkg/metrics/server.rs`

## 文件定位

`server.rs` 是 `astersql-metrics` crate 的 Server 层 Prometheus 指标定义文件，由 [`pkg/metrics/lib.rs`](lib.rs) 以公开模块 `server` 暴露。它负责创建和保存连接、SQL/命令耗时、扫描、计划缓存、慢查询、Token、TiFlash、PD API、TLS 等服务器入口侧 collector，但不负责监听网络、执行 SQL 或把 collector 注册到 Prometheus 注册表。

包级总入口 [`metrics::InitMetrics`](metrics.rs) 调用 `server::InitServerMetrics` 创建句柄，随后 [`metrics::RegisterMetrics`](metrics.rs) 才逐项注册这些句柄。crate 边界与依赖见 [`pkg/metrics/Cargo.toml`](Cargo.toml)：本文件使用本 crate 的标签常量和兼容层，底层依赖 `prometheus = "0.14"` 与路径依赖 `astersql-metrics-common`；没有本文件专属 feature 或条件编译分支。

## 核心职责

- 声明 49 个 `Option<prometheus::...>` 包级可变句柄，以及测试开关 `ResettablePlanCacheCounterFortTest`。`None` 表示尚未执行 `InitServerMetrics`，`Some` 表示 collector 已构造，但不等同于已经注册。
- 通过私有工厂函数 `counter`、`counter_vec`、`gauge`、`gauge_vec`、`histogram`、`histogram_vec` 统一设置 `tidb` namespace，并将构造委托给 `metricscommon::New*`，从而继承包级常量标签行为。
- `InitServerMetrics` 按 Go 版本的名称、subsystem、帮助文本、标签和桶边界构造所有 Server collector。
- `RecordQueryDuration`、`RecordCommandDuration` 和 `RecordQueryScanMetrics` 提供不会因尚未初始化而失败的观测入口；它们只在对应句柄为 `Some` 时写入。
- `ExecuteErrorToLabel` 把上游已经提取的 RFC 错误码映射为指标标签，缺失时使用稳定值 `"unknown"`。
- 常量 `ServerStart`、`ServerStop`、`EventKill` 定义 `ServerEventCounter` 的生命周期事件标签值。

## 主要符号

- `InitServerMetrics()`: 唯一的批量构造入口。它直接覆盖全部包级 `Option`，不会注册 collector，也不自行加锁；调用方必须遵守包级初始化协议。
- `RecordQueryDuration(sql_type, database, resource_group, seconds)`: 向 `QueryDurationHistogram` 观测单条 SQL 总耗时，标签顺序固定为 `sql_type`、`db`、`resource_group`。
- `RecordCommandDuration(sql_type, database, resource_group, seconds)`: 与上一函数结构相同，但写入独立的 `CommandDurationHistogram`，用于协议命令或受限 SQL 操作，避免与单条语句直方图混算。
- `RecordQueryScanMetrics(sql_type, database, request_count, scan)`: 总是尝试记录 RPC 数；仅当 `scan` 为 `Some((processed_keys, ia_cache_hits, remote_count, remote_bytes, remote_wait))` 时继续记录处理键数、IA 缓存命中、远端 segment 数/字节及等待秒数。
- `ExecuteErrorToLabel(Option<&str>) -> String`: `Some(code)` 原样复制，`None` 返回 `unknown`；它不负责识别 Rust 错误类型或展开错误链。
- 私有 `counter*`/`gauge*`/`histogram*`: 把重复的 `Namespace = "tidb"`、subsystem、名称、帮助文本、标签和桶配置收敛到六个构造器。
- 句柄按用途分组：流量与查询（`PacketIOCounter`、`Query*`、`IA*`）、连接与错误（`ConnGauge`、`DisconnectionCounter`、`PreparedStmtGauge`、`*ErrorCounter`）、生命周期与计划缓存（`ServerEventCounter`、`PlanCache*`）、慢查询与并发（`Total*`、`CopMVCCRatioHistogram`、`SlowQueryCounter`、`TokenGauge`）、外部组件与配置（`TiFlash*`、`PDAPI*`、`ConfigStatus`）、运行时和安全（`MaxProcs`、`GOGC`、`MemoryLimit`、`InternalSessions`、`ActiveUser`、`TLSVersion`、`TLSCipher`）。

## 执行流程

1. 应用初始化进入 `metrics::InitMetrics`；其 `Once` 保证整包初始化只成功执行一次，并在既定子系统顺序中调用 `server::InitServerMetrics`。
2. `InitServerMetrics` 使用六个私有工厂逐一创建 collector，将结果写入对应 `static mut Option`。直方图显式保留 Go 的指数桶，例如 SQL/命令耗时为 `ExponentialBuckets(0.0005, 2.0, 29)`，IA 远端等待为 `ExponentialBuckets(0.00005, 2.0, 20)`。
3. `metrics::RegisterMetrics` 要求初始化已经完成；它通过 `register_option` 对所有 Server 句柄取值、克隆并注册。未初始化时该层会以 `expect("InitMetrics must run before RegisterMetrics")` 失败。
4. SQL 完成时，`pkg/session/runtime/scan_adapter_runtime.rs` 的 `ObserveStatementDuration` 跳过 restricted SQL，规范化空语句类型为 `LblGeneral`，遍历语句涉及的数据库，调用 `RecordQueryDuration` 和 `RecordQueryScanMetrics`。请求数先以 `max(0)` 截断再转为 `u64`，扫描明细缺失时传 `None`。
5. 记录函数取得 `PACKAGE_INIT_LOCK`，复制或借用已初始化句柄，然后按固定标签顺序调用 `observe`/`inc_by`。`RecordQueryScanMetrics` 将 `Duration` 转成秒；若扫描明细缺失，它在记录 RPC 数后立即返回。
6. 简化指标模式由 `metrics::ToggleSimplifiedMode` 管理；该函数会注销或重新注册本文件中的部分高开销 collector（`LoadTableCacheDurationHistogram`、`ReadFromTableCacheCounter`、两个 `TiFlash*` 和 `TokenGauge`），但不会重建句柄。

## 数据与状态

所有 collector 都是进程级包状态。Counter 只累计事件或数量，Gauge 表示可增减或可覆盖的当前状态，Histogram/HistogramVec 保存样本数、总和和桶计数。向量标签是指标时序身份的一部分，调用者必须严格匹配初始化时声明的标签个数与顺序，否则底层 Prometheus API 会失败或 panic。

`InitServerMetrics` 中大多数指标的 subsystem 是 `server`；例外是 `TimeJumpBackCounter` 使用 `monitor`，`ConfigStatus` 使用 `config`。所有指标仍使用 `tidb` namespace。`GetTokenDurationHistogram` 的指标名以 `_seconds` 结尾，但 Go/Rust 沿用的帮助文字写 `us`，桶值也原样为从 1 开始的二倍指数桶；在确认上游实际观测单位前不应擅自“纠正”它。

`ResettablePlanCacheCounterFortTest` 只声明测试兼容开关，本文件没有读取或修改它。其名称中的 `FortTest` 是 Go 侧既有拼写，属于兼容 API，不应在单文件内改名。全文件没有 trait、struct、enum 或条件编译项。

## 依赖与调用关系

上游关系经 RustCodeGraph 与源码核验如下：

- `pkg/metrics/metrics.rs::InitMetrics -> server::InitServerMetrics`：构造所有句柄。
- `pkg/metrics/metrics.rs::RegisterMetrics -> server::*`：注册全部 collector；`ToggleSimplifiedMode` 管理其中五类可选高开销指标。
- `pkg/session/runtime/scan_adapter_runtime.rs::ObserveStatementDuration -> RecordQueryDuration` 与 `RecordQueryScanMetrics`：当前生产查询完成主链。
- `RecordCommandDuration` 当前只在 `pkg/metrics/metrics_test.rs` 被调用；没有找到生产调用者，因此文档不宣称命令耗时已接入服务器协议主链。
- `ExecuteErrorToLabel` 当前由 `pkg/metrics/metrics_test.rs` 和 `pkg/metrics/metrics_2_aster_unit_test.rs` 验证；它是公开兼容辅助函数。

下游依赖为 `crate::bindinfo::{compat_metricscommon, compat_prometheus}` 和 crate 根标签常量。兼容层最终调用 `pkg/metrics/common/wrapper.rs` 的 `New*`，注入包级常量标签并使用 rust-prometheus 构造 collector。该文件只构造和观测，不执行注册、抓取或编码；抓取文本由 `metrics::GatherText` 负责。

## 错误处理与边界

- 三个 `Record*` 函数在指标未初始化时静默跳过，因此可以安全处理初始化前的观测，但这也意味着样本会丢失且没有返回错误。
- 记录入口用 `PACKAGE_INIT_LOCK` 串行化与初始化之间的访问；锁中毒时通过 `PoisonError::into_inner` 继续，而不是 panic。
- `RecordQueryScanMetrics` 不校验负的 `processed_keys`，会按 `f64` 原样观测；其他计数输入为 `u64`。调用方当前只对 RPC 数执行非负截断。
- `ExecuteErrorToLabel` 的 Rust 签名与 Go 不同：Rust 不接收 `error`，因此无法自行执行 Go 的 `errors.Cause` 或 `*terror.Error` 类型判断；调用者必须先提取 RFC code。
- collector 构造器没有返回 `Result`。`metricscommon::New*` 对非法名称、帮助文本、标签或桶配置使用 `expect`，因此定义错误会在初始化阶段 panic。注册冲突则发生在 `RegisterMetrics` 并以 `prometheus::Error` 返回。
- `InitServerMetrics` 可被直接重复调用并覆盖句柄；正常路径依赖 `InitMetrics` 的 `Once` 防止已注册或正被使用的 collector 被替换。

## 并发与资源生命周期

包级 collector 使用 `static mut Option`，本身没有 Rust 类型系统提供的同步保证。正常生命周期是“`InitMetrics` 一次构造 -> `RegisterMetrics` 注册克隆 -> 多线程记录 -> 进程结束”；`InitMetrics` 的 `Once` 和 `PACKAGE_INIT_LOCK` 是该协议的关键部分。

`RecordQueryDuration` 与 `RecordCommandDuration` 在锁内克隆 `HistogramVec`，释放时只销毁句柄克隆，底层共享 collector 状态仍由注册表和全局句柄持有。`RecordQueryScanMetrics` 在锁内直接借用多个静态句柄并完成全部写入，所以一次扫描指标更新不会与重新初始化交错，但持锁时间覆盖多次 label 查找和观测。Prometheus collector 自身支持并发更新；额外包锁主要保护 `static mut` 初始化/读取边界。

本文件不创建线程、异步任务、通道、网络连接或文件句柄。注册表资源的注销/重注册由 `ToggleSimplifiedMode` 管理；本文件不实现显式清理。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/metrics/server.go`](server.go)。Rust 的全局名称、三种事件常量、指标 namespace/subsystem/name/help、标签集合和直方图桶基本逐项复刻 Go 的 `InitServerMetrics`，包括 `RCCheckTSWriteConfilictCounter`、`ResettablePlanCacheCounterFortTest` 等历史拼写和 Token 耗时单位文字的不一致。

主要迁移差异有三类：

- Go collector 全局变量是指针或接口值；Rust 用 `Option<T>` 显式表达初始化前状态，并因机械迁移允许 `static mut`。
- Go 文件只有构造和 `ExecuteErrorToLabel(error)`；Rust 额外提供三个 `Record*` 安全入口，以集中处理包锁和可选句柄。查询耗时与扫描入口已经接到 Rust 会话完成路径，命令耗时入口目前仅有测试证据。
- Go 的 `ExecuteErrorToLabel` 先 `errors.Cause`，再对 `*terror.Error` 返回 RFC code；Rust 接收 `Option<&str>`，只负责“已有 code/无 code”的最终映射。`pkg/metrics/metrics_test.go::TestExecuteErrorToLabel` 与 Rust 两个测试文件共同记录这一签名适配。

## 扩展指南

新增 Server 指标时，应同时完成以下局部接线：在本文件声明 `Option` 句柄并在 `InitServerMetrics` 以准确的名称、单位、标签与桶构造；在 `metrics::RegisterMetrics` 加入注册列表；若属于简化模式高开销指标，再同步 `ToggleSimplifiedMode`；最后在真实生产路径记录，而不能只完成声明和初始化。

修改标签时要同步所有 `with_label_values` 调用和仪表盘/告警消费者，并评估标签基数。数据库名、资源组、地址等动态标签尤其可能扩大时序数量。修改桶或单位会改变监控解释和聚合结果，应先与 Go 行为及现有 dashboard 对齐。若扩展记录函数，继续通过 `PACKAGE_INIT_LOCK` 或迁移到具备等价同步保证的状态容器，不应新增裸 `static mut` 竞态。

测试逻辑必须继续放在独立文件，不要嵌入 `server.rs`。优先扩展 `pkg/metrics/metrics_test.rs` 验证注册后的完整指标名、标签、样本和桶；错误标签兼容同时更新 `pkg/metrics/metrics_2_aster_unit_test.rs`；若 Go 契约变化，也核对 `pkg/metrics/metrics_test.go`。生产接线的行为测试应放在对应调用模块的独立 `*_test.rs` 中。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/metrics/server.rs`；`files --filter pkg/metrics/server.rs` 与 `node --file ...` 核对 613 行、67 个符号；`query`/`explore` 确认 `InitServerMetrics`、三个 `Record*`、`ExecuteErrorToLabel` 及调用边。图结果显示该文件被 `pkg/metrics/metrics_test.rs` 与 `pkg/session/runtime/scan_adapter_runtime.rs` 使用。
- Rust 源码：`pkg/metrics/server.rs`；crate/模块和生命周期入口：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`；生产调用点：`pkg/session/runtime/scan_adapter_runtime.rs::ObserveStatementDuration`；构造失败语义：`pkg/metrics/bindinfo.rs` 与 `pkg/metrics/common/wrapper.rs`。
- Go 对照：`pkg/metrics/server.go::InitServerMetrics`、`ExecuteErrorToLabel`，以及 `pkg/metrics/metrics.go::InitMetrics`；Go 测试：`pkg/metrics/metrics_test.go::TestExecuteErrorToLabel`。
- Rust 独立测试：`pkg/metrics/metrics_test.rs` 覆盖初始化/注册、错误标签、IA 扫描 collector 和 SQL/命令独立直方图；`pkg/metrics/metrics_2_aster_unit_test.rs` 补充初始化成功边界和错误标签映射。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文恰有上述 11 个固定二级章节；事实复核还确认本文没有把句柄构造误写为注册，也没有把仅在测试中调用的 `RecordCommandDuration` 描述为已接入生产链路。
