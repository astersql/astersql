# `pkg/metrics/ttl.rs`

## 文件定位

`pkg/metrics/ttl.rs` 属于 `astersql-metrics` crate（见 `pkg/metrics/Cargo.toml`），由 crate 根 `pkg/metrics/lib.rs` 以 `pub mod ttl` 暴露。它是 TTL（Time To Live）观测面的指标定义层：声明进程级 Prometheus 句柄，并在 `InitTTLMetrics` 中配置指标名称、帮助文本、桶和变量标签。它不扫描表、不删除过期行，也不调度 TTL Job/Task。

包级中枢 `pkg/metrics/metrics.rs::InitMetrics` 调用 `crate::ttl::InitTTLMetrics`，随后 `RegisterMetrics` 将本文件的 8 个根 collector 注册到默认 registry。两个预绑定事件 counter（`TTLSyncTimerCounter`、`TTLFullRefreshTimersCounter`）是 `TTLEventCounter` 的带标签子句柄，不会被重复注册。

## 核心职责

本文件只有两类职责：

1. 以 10 个 `Option<prometheus::...>` 可变静态量保存 Go 包变量的 Rust 对应物；初始化前为 `None`，初始化后为具体句柄。
2. 由 `InitTTLMetrics` 一次性构造 8 个根 collector，并从事件向量派生两个常用事件子句柄。

所有根指标都使用 `tidb` namespace 和 `server` subsystem，因此导出的全限定名称形如 `tidb_server_ttl_query_duration`。构造统一经过 `metricscommon::New*` 兼容封装；该封装会注入包级 const labels，并在选项或标签非法时通过 `expect` 失败（`pkg/metrics/common/wrapper.rs::NewCounter`、`NewCounterVec`、`NewGaugeVec`、`NewHistogramVec`）。

## 主要符号

| 符号 | 类型 | 指标名/标签 | 语义 |
| --- | --- | --- | --- |
| `TTLQueryDuration` | `Option<HistogramVec>` | `ttl_query_duration`; `sql_type,result` | SELECT/DELETE 等 TTL SQL 的处理耗时（秒）。使用 `ExponentialBuckets(0.01, 2.0, 20)`，桶上界从 10 ms 按 2 倍增长到 `0.01 * 2^19` 秒，约 1.46 小时。 |
| `TTLProcessedExpiredRowsCounter` | `Option<CounterVec>` | `ttl_processed_expired_rows`; `sql_type,result` | 按 SQL 类型和结果累计处理过的过期行。 |
| `TTLJobStatus` | `Option<GaugeVec>` | `ttl_job_status`; `type` | 各状态 Job 的当前数量，例如 `running`、`cancelling`。 |
| `TTLTaskStatus` | `Option<GaugeVec>` | `ttl_task_status`; `type` | 各状态 Task 的当前数量，例如 `scanning`、`deleting`。 |
| `TTLPhaseTime` | `Option<CounterVec>` | `ttl_phase_time`; `type,phase` | 按 worker 类型和 phase 累计耗时。 |
| `TTLInsertRowsCount` | `Option<Counter>` | `ttl_insert_rows`; 无变量标签 | 累计写入 TTL 表并按事务语义最终生效的行数。 |
| `TTLWatermarkDelay` | `Option<GaugeVec>` | `ttl_watermark_delay`; `type,name` | 按类型和命名分桶表示 TTL watermark 调度延迟。尽管 help 中保留 “Bucketed”，实现类型是 GaugeVec，分桶由调用方以 `name` 标签表达。 |
| `TTLEventCounter` | `Option<CounterVec>` | `ttl_event_count`; `type` | TTL 内部事件计数向量。 |
| `TTLSyncTimerCounter` | `Option<Counter>` | `ttl_event_count{type="sync_one_timer"}` | 从 `TTLEventCounter` 预绑定的单 timer 同步事件句柄。 |
| `TTLFullRefreshTimersCounter` | `Option<Counter>` | `ttl_event_count{type="full_refresh_timers"}` | 从 `TTLEventCounter` 预绑定的全量 timer 刷新事件句柄。 |
| `InitTTLMetrics` | `pub unsafe fn()` | 初始化入口 | 顺序构造上述根 collector，再派生两个事件 counter。 |

标签键来自 crate 根通过 `use crate::*` 引入的 `LblSQLType`、`LblResult`、`LblType`、`LblPhase`、`LblName`；其字符串值在 `pkg/metrics/session.rs` 中分别定义为 `sql_type`、`result`、`type`、`phase`、`name`。

## 执行流程

1. 应用启动路径调用 `pkg/metrics/metrics.rs::InitMetrics`。该函数受 `INIT_METRICS_ONCE.call_once` 保护，并在其他子系统指标之后调用 `crate::ttl::InitTTLMetrics`。
2. `InitTTLMetrics` 依次构造查询耗时、过期行、Job 状态、Task 状态、phase 时间、插入行、水位延迟和事件共 8 个根 collector，写入对应 `static mut Option`。
3. 查询耗时直方图通过 `prometheus::ExponentialBuckets(0.01, 2.0, 20)` 生成桶；其余向量按表中标签顺序创建。
4. 初始化事件向量后，函数以 `as_ref().unwrap()` 取得刚写入的 `TTLEventCounter`，用 `WithLabelValues` 缓存 `sync_one_timer` 与 `full_refresh_timers` 两个子句柄。
5. `pkg/metrics/metrics.rs::RegisterMetrics` 注册 8 个根 collector。之后业务层取得具体标签子句柄并执行 `observe`、`inc`、`inc_by`、`set` 或 `reset`。

消费侧的直接例子包括：`pkg/ttl/metrics/metrics.rs` 预绑定查询耗时、过期行、Job/Task 状态和 worker phase，并在 `UpdateDelayMetrics` 中写 `TTLWatermarkDelay`；`pkg/session/runtime.rs::increment_ttl_insert_rows_metric` 更新 `TTLInsertRowsCount`。Go 侧 timer 同步消费点位于 `pkg/ttl/ttlworker/timer_sync.go`。

## 数据与状态

本文件不维护 TTL 业务实体，只维护进程级 Prometheus collector 句柄。8 个根句柄与 2 个派生句柄的状态迁移都是 `None -> Some(handle)`；没有清理或回退路径，生命周期预期与进程一致。

Counter/CounterVec（过期行、phase 时间、插入行、事件）只累计；GaugeVec（Job、Task、水位延迟）表达可升可降或可重置的当前状态；HistogramVec 保存观测次数、总和和固定桶计数。`TTLEventCounter` 与两个派生 counter 共享同一底层指标族和标签序列，不是三份独立数据。

标签的顺序是调用契约的一部分。例如 `TTLQueryDuration` 必须传 `[sql_type, result]`，`TTLWatermarkDelay` 必须传 `[type, name]`；传入数量不符会在 Prometheus API 取子句柄时失败。高基数值也会长期增加时序数量，因此 `type`、`phase` 和 `name` 应继续使用有限枚举值。

## 依赖与调用关系

- 上游装配：`pkg/metrics/lib.rs` 声明 `pub mod ttl`；`pkg/metrics/metrics.rs::InitMetrics` 调用 `InitTTLMetrics`；同文件 `RegisterMetrics` 注册根 collector。
- 构造依赖：`crate::bindinfo::compat_metricscommon` 提供 `NewHistogramVec`、`NewCounterVec`、`NewGaugeVec`、`NewCounter`；`compat_prometheus` 提供 Prometheus 类型、选项、指数桶和 Go 风格兼容方法。
- 配置依赖：共享标签常量来自 `crate::*`，实际定义在 `pkg/metrics/session.rs`。crate 的 Prometheus 依赖是 `prometheus = "0.14"`，见 `pkg/metrics/Cargo.toml`。
- 相邻 TTL worker 实现：独立 crate `pkg/ttl/metrics/metrics.rs` 使用自己 crate 内的同名查询、过期行、状态、phase 和 watermark 指标；其测试 `pkg/ttl/metrics/migration_aster_unit_test.rs` 校验那套同名描述符、桶与水位更新。它们可用于核对 TTL 侧所需语义，但不会直接执行本文件的静态句柄。
- Session 消费：`pkg/session/runtime.rs::increment_ttl_insert_rows_metric` 在事务/savepoint 语义确定后增加插入行 counter；`pkg/session/test/meta/session_test.rs` 覆盖累加结果。
- Go timer 消费：`pkg/ttl/ttlworker/timer_sync.go` 增加两个事件子句柄，`pkg/ttl/ttlworker/integrationtest/timer_sync_test.go` 会替换并恢复它们以验证调用次数。当前仓库搜索未发现这两个子句柄的 Rust 业务消费点，因此不能据本文件声称 Rust timer 同步链已经接线。

另有独立 crate `pkg/ttl/metrics` 自己声明部分同名 `LazyLock` collector，用于其 TTL worker 移植代码；它并不是本文件的定义位置。扩展时应先确认目标消费者使用 `astersql-metrics` 还是 `astersql-ttl-metrics`，避免只改一侧造成描述符漂移。

## 错误处理与边界

`InitTTLMetrics` 没有返回值，也没有恢复性错误分支。底层 `metricscommon::New*` 对 Prometheus 构造错误使用 `expect`，所以非法 metric 名、标签或桶配置会 panic；这是启动期配置错误，不是运行时 TTL 作业错误。

事件子句柄处的 `TTLEventCounter.as_ref().unwrap()` 依赖函数内先初始化事件向量这一顺序；若重排代码使派生发生在赋值前，会 panic。调用方还必须保证在读取这些 `Option` 前完成 `InitMetrics`；例如 session 更新函数对 `None` 采取跳过策略，以兼容未执行应用启动流程的轻量单测。

本文件不捕获 registry 的重复注册或注册失败。注册发生在 `RegisterMetrics`，错误由其 `Result<(), prometheus::Error>` 向上传播。根 collector 应只注册一次；派生标签句柄不能再次注册。

## 并发与资源生命周期

10 个句柄是 `static mut`，因此读写本身需要 `unsafe` 协议。真正的并发保护位于 `pkg/metrics/metrics.rs`：`INIT_METRICS_ONCE.call_once` 确保初始化只发生一次，并避免测试并发时替换仍被其他线程读取的 HistogramVec。`InitTTLMetrics` 单独公开为 `unsafe`，表示调用者若绕开中枢重复调用，就必须自行保证没有并发读写或旧句柄仍在使用。

初始化完成后，Prometheus 句柄可以 clone/派生并由多线程更新；本文件不创建线程、锁、异步任务、通道、事务或 I/O 资源。句柄没有显式销毁流程，随进程存活。TTL worker 使用 `LazyLock` 缓存带标签句柄，避免每次记录时重新解析标签；事件的两个常用标签也在本文件初始化时预绑定。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metrics/ttl.go`。逐项源码对照表明，Rust 保留了 Go 的 10 个包变量、`InitTTLMetrics` 初始化顺序、8 个根指标的 namespace/subsystem/name/help、变量标签及查询耗时桶。相邻独立 crate 的 `pkg/ttl/metrics/migration_aster_unit_test.rs::metric_descriptors_and_query_buckets_match_go` 验证了同名 TTL worker collector 的关键描述符和桶，但它不直接覆盖本文件；仓库搜索未发现专门直接断言 `pkg/metrics/ttl.rs` 全部描述符的 Rust 测试。

主要语言差异如下：

- Go 指针或接口句柄初始化前为 `nil`；Rust 用 `Option<...> = None` 表达同一状态。
- Go 包变量可以直接赋值；Rust 因 `static mut` 要求初始化函数与读取路径进入 `unsafe`，部分读取还使用裸指针规避 Rust 2024 对可变静态量共享引用的限制。
- Go 的 `WithLabelValues("value")` 在兼容层中对应 Rust 的 `WithLabelValues(&["value"])`。
- Go 依赖包初始化约定；Rust 中枢显式使用 `Once` 约束初始化。Prometheus collector 的运行时语义仍由 Rust `prometheus` crate 提供。

Go 测试 `pkg/session/test/meta/session_test.go` 和 Rust 对应测试 `pkg/session/test/meta/session_test.rs` 都验证 TTL 插入行 counter 的事务相关累计。Go timer 集成测试覆盖事件子句柄；仓库中未发现等价 Rust timer 消费测试，属于当前迁移覆盖差异，而不是本文件证明已支持的行为。

## 扩展指南

- 新增 TTL 指标时，应在本文件增加根句柄及 `InitTTLMetrics` 构造逻辑，并把根 collector 加入 `pkg/metrics/metrics.rs::RegisterMetrics`；若 Go 源仍是兼容基准，应同步 `pkg/metrics/ttl.go` 的名称、help、标签和桶。
- 新增或调整本文件描述符时，应在 `pkg/metrics` 下新增或扩展独立 `*_test.rs`，直接经 `astersql_metrics::metrics::InitMetrics` 检查这些句柄；若同名 TTL worker collector 也要保持一致，再同步 `pkg/ttl/metrics/migration_aster_unit_test.rs::metric_descriptors_and_query_buckets_match_go`。不要把测试内嵌进 `ttl.rs`。
- 修改标签时必须同步所有 `with_label_values`/`WithLabelValues` 调用点。标签顺序变化会破坏现有消费者；加入无界表名、任务 ID 等值会带来 Prometheus 高基数和内存风险。
- 若添加常用派生句柄，只注册其根向量，并在根向量初始化后派生。为避免别名重复注册，不要把子句柄加入 `RegisterMetrics`。
- 若要消除 `static mut`，需统一评估 `InitMetrics`、注册宏/元组、session 裸指针读取以及跨 crate 消费，不能仅在本文件改成另一种全局容器。
- 对同名指标的修改还应审查独立 `pkg/ttl/metrics` crate；当前它只覆盖查询、过期行、Job/Task、phase 与 watermark，未包含本文件的插入行和事件指标。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标查询期间索引可用。
- RustCodeGraph `query InitTTLMetrics --kind function`：定位 `pkg/metrics/ttl.rs:53` 的 Rust 入口和 `pkg/metrics/ttl.go:46` 的 Go 对照入口。
- RustCodeGraph `node --file pkg/metrics/ttl.rs --offset 1 --limit 180`：读取目标文件全部 131 行，确认 10 个静态句柄、唯一函数、桶、标签与派生顺序；节点报告该文件由 `pkg/metrics/metrics.rs` 使用。
- RustCodeGraph `explore "pkg/metrics/ttl.rs TTL metrics symbols callers callees"`：确认 `InitTTLMetrics` 的直接调用方是 `pkg/metrics/metrics.rs::InitMetrics`。精确 `callers` 子命令在本地索引上持续无输出后被终止，因此调用边又以精确 `node` 和 `rg` 引用结果交叉验证。
- RustCodeGraph 节点：读取独立 `pkg/ttl/metrics` crate 的标签预绑定、phase、watermark 消费及其描述符/桶/水位测试，用作相邻语义证据而非本文件直接测试；读取 `pkg/session/runtime.rs::increment_ttl_insert_rows_metric`；读取 `pkg/metrics/common/wrapper.rs` 的构造失败行为。
- 原始文件核验：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`、`pkg/metrics/session.rs`、`pkg/metrics/ttl.go`、`pkg/ttl/metrics/Cargo.toml`、`pkg/ttl/metrics/lib.rs`，以及 `rg` 找到的 Rust/Go 消费与独立测试路径。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构验证，并人工复核文档只描述可由上述源码、调用边、Cargo 或测试支持的事实。
