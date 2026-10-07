# `pkg/dxf/framework/dxfmetric/metric.rs`

## 文件定位

本文件属于 Cargo crate `astersql-dxf-framework-dxfmetric`，crate 根 `pkg/dxf/framework/dxfmetric/lib.rs` 以 `pub mod metric` 声明本模块，并用 `pub use metric::*` 将其公共 API 再导出。它不采集任务快照（该职责在同 crate 的 `collector.rs`），而是定义 DXF（Distributed eXecution Framework）运行路径主动写入的五组 Prometheus `GaugeVec`/`CounterVec`、事件标签常量，以及统一初始化和注册入口。

应用级接线位于 `pkg/metrics/metrics.rs`：`InitMetrics` 调用 `InitDistTaskMetrics`，`register_external_metrics` 调用 `Register`。因此本文件位于“DXF 业务事件 -> Prometheus 向量 -> TiDB 全局指标注册表”的边界，自己不启动后台任务，也不决定何时发生某个业务事件。

`pkg/dxf/framework/dxfmetric/Cargo.toml` 指明 crate 的 Go 对照包为 `pkg/dxf/framework/dxfmetric`。本文件直接使用 `prometheus` 和内部 `metricscommon`；清单中的 `proto`、`uuid` 是整个 crate（尤其 `collector.rs`）的依赖，不是本文件的直接依赖。

## 核心职责

1. 用 `namespaceTiDB`、`subsystemDXF`、标签名和公开事件常量固定指标协议，避免调用点自行拼接字符串。
2. 由 `InitDistTaskMetrics` 构造并缓存唯一一份 `DistTaskMetrics`，使不同 DXF 子系统写入同一组进程内指标向量。
3. 由 `Register` 将五个向量按 Go 声明顺序注册到给定 `prometheus::Registry`，并向上返回注册错误。
4. 保留一个有意的命名例外：`UsedSlotsGauge` 使用 `disttask` subsystem，其余四组使用 `dxf` subsystem。这由源码构造参数及迁移测试中的完整指标名共同确认。

本文件只定义与暴露指标，不负责重置、删除某个任务的标签序列、控制标签值是否合法，或保证业务调用点一定写入所有已声明事件。

## 主要符号

- `LblTaskID: &str`：公开标签名 `task_id`，供事件向量和调用者保持相同标签键。
- `EventSubtaskScheduledAway`、`EventSubtaskRerun`、`EventSubtaskSlow`、`EventRetry`、`EventTooManyIdx`、`EventMergeSort`、`EventCleanupFailed`、`EventExpiredFileCleanupFailed`、`EventMeterWriteFailed`：公开的事件标签值。当前 Rust 搜索可直接确认清理、过期文件清理、计量写入失败、索引过多和归并排序等调用点；其余常量仍是与 Go 公共协议对齐的 API，不能仅因当前 Rust 调用点较少而删除。
- `DistTaskMetrics`：公开聚合结构。`UsedSlotsGauge: GaugeVec` 使用标签 `service_scope`；`WorkerCount: GaugeVec` 使用 `type`；`FinishedTaskCounter: CounterVec` 使用 `state`；`ScheduleEventCounter` 和 `ExecuteEventCounter` 均按 `task_id,event` 两个标签分组。
- `METRICS: OnceLock<DistTaskMetrics>`：模块私有的进程级惰性单例。
- `opts(namespace, subsystem, name, help) -> Opts`：模块私有构造辅助函数，为每个指标统一设置 namespace 和 subsystem。
- `labels(values) -> Vec<String>`：把静态标签名切片转换成 `metricscommon::NewGaugeVec`/`NewCounterVec` 所需的字符串切片载体；调用完成后临时 `Vec` 即可释放，因为向量构造函数持有自己的描述信息。
- `InitDistTaskMetrics() -> &'static DistTaskMetrics`：公共初始化/访问入口。名称沿用 Go 风格；返回静态共享引用，让调用者可直接选择字段并写值。
- `Register(&Registry) -> prometheus::Result<()>`：公共注册入口，逐个 clone 指标向量句柄后交给 Registry；任一步失败即停止并返回错误。

## 执行流程

全局启动流程如下：

1. `pkg/metrics/metrics.rs::InitMetrics` 调用 `InitDistTaskMetrics`。
2. 首次调用进入 `METRICS.get_or_init` 闭包，依次构造 `tidb_disttask_used_slots`、`tidb_dxf_worker_count`、`tidb_dxf_finished_task_total`、`tidb_dxf_schedule_event_total`、`tidb_dxf_execute_event_total`。后续调用直接取得同一 `DistTaskMetrics`。
3. `pkg/metrics/metrics.rs::register_external_metrics` 将目标 Registry 传给 `Register`。
4. `Register` 先取得单例，然后按照上述顺序注册五个 clone 句柄；全部成功后返回 `Ok(())`。

运行期间，业务代码再次调用 `InitDistTaskMetrics` 获取同一实例。例如 `scheduler.rs` 在任务终态形成后对 `FinishedTaskCounter` 的 `all` 和具体状态各加一；`scheduler_manager.rs` 在清理失败时增加 `ScheduleEventCounter`；`importinto/scheduler.rs` 记录 `too-many-idx` 与 `merge-sort`；`metering.rs` 记录 `meter-write-failed`。这些调用不会重新创建或重新注册指标。

## 数据与状态

唯一模块状态是 `METRICS`。其内容初始化后不再替换；变化发生在 Prometheus 向量内部的标签子序列和值上。Gauge 表示可升降的当前量，Counter 表示只增的累计事件量。

指标及标签协议为：

| 字段 | Prometheus 全名 | 标签（按构造顺序） | 语义 |
| --- | --- | --- | --- |
| `UsedSlotsGauge` | `tidb_disttask_used_slots` | `service_scope` | 执行节点按服务范围统计的已用槽位 |
| `WorkerCount` | `tidb_dxf_worker_count` | `type` | 按 worker 类型统计的数量 |
| `FinishedTaskCounter` | `tidb_dxf_finished_task_total` | `state` | 按任务终态统计的完成次数 |
| `ScheduleEventCounter` | `tidb_dxf_schedule_event_total` | `task_id,event` | 调度侧事件次数 |
| `ExecuteEventCounter` | `tidb_dxf_execute_event_total` | `task_id,event` | 执行侧事件次数 |

`ExecuteEventCounter` 明确使用任务 ID 而不是子任务 ID，以限制时间序列基数。无法关联到具体任务的清理或计量事件在现有调用点使用 `"-"` 作为 `task_id`。标签值及序列的删除策略属于调用者；本文件不保存任务对象、调度状态或持久化数据。

## 依赖与调用关系

上游初始化与注册调用者是 `pkg/metrics/metrics.rs::InitMetrics` 和 `register_external_metrics`。直接业务调用证据包括：

- `pkg/dxf/framework/scheduler/scheduler.rs`：任务完成分类后写 `FinishedTaskCounter`。
- `pkg/dxf/framework/scheduler/scheduler_manager.rs`：外部文件过期清理和批量清理失败时写 `ScheduleEventCounter`。
- `pkg/dxf/importinto/scheduler.rs`：索引数量告警和进入 merge-sort 步骤时写调度事件。
- `pkg/dxf/framework/metering/metering.rs`：计量数据写入失败时写执行事件。
- `pkg/metrics/metrics_internal_test.rs`：经全局初始化/注册路径确认 `tidb_disttask_used_slots` 可被采集。

下游依赖是 `metricscommon::NewGaugeVec`、`metricscommon::NewCounterVec` 和 `prometheus::{Opts, Registry}`。`opts` 先生成带 namespace/subsystem 的描述，`metricscommon` 包装器再构造具体向量；`Register` 最终调用 `Registry::register`。

RustCodeGraph 的文件关系将 `metric.rs` 指向 `scheduler_manager.rs`、`scheduler_manager_nokit_test.rs`、`importinto/scheduler.rs` 和 `metrics_internal_test.rs`；其精确 `callers/callees` 查询未生成函数级结果，因此上述更完整的调用关系由符号引用搜索及相邻源码核验补足，而不是假定图中无边即无调用。

## 错误处理与边界

`InitDistTaskMetrics` 没有可恢复错误返回；Prometheus 向量构造采用 `metricscommon` 提供的构造接口。`Register` 则保留每次 `Registry::register` 的 `prometheus::Error`，通过 `?` 立即传播：例如向同一 Registry 重复注册相同描述时，调用者会收到错误，后续向量不会继续注册。与 Go 的 `MustRegister` 相比，Rust 版本不会在此处主动 panic，而是把失败交给上层处理。

调用者必须严格提供正确数量且顺序一致的标签值；本文件定义标签顺序，但不会把业务枚举或 ID 转为标签。Counter 也不能用于减值。`Register` 的逐项操作不是事务：若中途失败，之前成功注册的向量仍留在 Registry 中，调用者不能假设失败会回滚。

事件常量只保证字符串协议，不证明相应 Rust 业务路径已经全部移植。尤其对只在 Go 调用搜索中出现的事件，文档不能宣称 Rust 已产生这些事件。

## 并发与资源生命周期

`OnceLock` 保证并发首次访问时初始化闭包只成功执行一次，并安全发布 `DistTaskMetrics`；返回的 `&'static` 引用与进程生命周期一致，不需要调用者持锁或释放。Prometheus 的 `GaugeVec`/`CounterVec` 句柄可 clone；注册的是共享底层指标的句柄，而不是一组与业务写入脱离的新数值。

本模块不创建线程、异步任务、通道、锁守卫或外部连接。并发更新和采集的同步由 Prometheus 指标实现承担。高基数是主要资源风险：每个新 `task_id,event` 组合都可能形成时间序列，因此执行事件按任务 ID 聚合，未知任务使用 `"-"`。清理过期标签序列的生命周期策略必须由拥有任务生命周期的调用方实现。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/dxfmetric/metric.go`。两版保持五个指标的 namespace、subsystem、名称、Help 文本、标签顺序、事件字符串和注册顺序一致；`migration_aster_unit_test.rs::migration_metric_names_labels_and_registration_match_go` 对完整指标名及标签组合做了实际采集断言。

主要结构差异如下：

- Go 使用五个可重新赋值的包级指针变量，`InitDistTaskMetrics()` 负责赋值且不返回对象；Rust 用 `DistTaskMetrics` 加 `OnceLock`，初始化函数返回共享静态引用，因此初始化是幂等且不可替换的。
- Go `Register(prometheus.Registerer)` 使用 `MustRegister`，失败会 panic，并接受较宽的 `Registerer` 接口；Rust `Register(&Registry)` 的参数更具体，并返回 `prometheus::Result<()>`。
- Go 调用点直接引用包级字段；Rust 调用点先调用 `InitDistTaskMetrics()` 再访问字段。
- Go 注释与 Rust 源码都说明执行事件用 task ID 限制基数。Rust 当前只迁移了部分事件生产调用点；公共常量集合仍与 Go 保持一致。

## 扩展指南

新增指标时，应同时修改 `DistTaskMetrics` 字段、`InitDistTaskMetrics` 构造闭包和 `Register` 注册列表，并在 `migration_aster_unit_test.rs` 的注册测试中写入一个样本、采集 Registry、断言完整指标名与标签。若 Go 仍是对照实现，还应同步核对 `metric.go` 的名称、Help、标签顺序和注册顺序，避免监控查询静默失配。

新增事件类型通常只需在本文件增加与 Go 对齐的公开字符串常量，并在实际生产路径使用；测试应放在独立 `*_test.rs` 文件，不能嵌入本源文件。涉及任务终态分类或清理行为时，优先扩展 `scheduler_nokit_test.rs` 或 `scheduler_manager_nokit_test.rs`；涉及指标描述与注册协议时扩展同 crate 的 `migration_aster_unit_test.rs`。

修改标签必须视为监控兼容性变更：标签名称、顺序或 subsystem 改动会改变时间序列及仪表盘查询；加入无界 ID 会增加内存和采集成本。新增注册项还需考虑 `Register` 部分成功的错误边界。若希望支持自定义 Registry 包装器，需要有意评估是否把参数从 `&Registry` 泛化，而不能只改调用点。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/dxf/framework/dxfmetric` 确认同 crate 的 Rust/Go/测试文件；`node --file pkg/dxf/framework/dxfmetric/metric.rs` 读取 150 行完整实现并给出四个文件级使用者。对 `InitDistTaskMetrics`、`Register` 的精确 `callers/callees --file ...` 未返回函数级边，已用直接引用核验补齐。
- 目标实现与边界：`pkg/dxf/framework/dxfmetric/metric.rs`、`lib.rs`、`Cargo.toml`，以及 `pkg/metrics/common/wrapper.rs` 中 `NewCounterVec`/`NewGaugeVec` 的签名。
- Go 对照：`pkg/dxf/framework/dxfmetric/metric.go`；业务侧补充对照包括 `pkg/dxf/framework/scheduler/*.go`、`taskexecutor/*.go`、`metering/metering.go` 与 `pkg/dxf/importinto/scheduler.go` 的引用搜索结果。
- Rust 调用者：`pkg/metrics/metrics.rs`、`pkg/dxf/framework/scheduler/scheduler.rs`、`scheduler_manager.rs`、`pkg/dxf/framework/metering/metering.rs`、`pkg/dxf/importinto/scheduler.rs`。
- 独立测试：`pkg/dxf/framework/dxfmetric/migration_aster_unit_test.rs` 验证五个指标的名称、标签和值；`scheduler_nokit_test.rs` 验证完成状态分类与终态累加；`scheduler_manager_nokit_test.rs` 验证清理失败事件；`pkg/metrics/metrics_internal_test.rs` 验证全局初始化和注册接线。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认目标文件存在且恰有规定的十一个二级标题；人工复核确认本文区分了已验证 Rust 行为、Go 对照协议和当前未见 Rust 调用的公共事件常量。
