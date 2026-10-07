# `pkg/dxf/importinto/metrics.rs`

## 文件定位

本文件属于 `astersql-dxf-importinto` crate；crate 入口 `pkg/dxf/importinto/lib.rs` 以 `pub mod metrics` 装配并重新导出本模块。它位于 IMPORT INTO 分布式任务的调度器和节点执行器，与底层 Lightning 通用指标组之间：上游生命周期代码按分布式 `task_id` 取得或释放指标，下游 `pkg/metrics/import.rs` 负责把 `astersql_lightning_metric::Common` 注册到、或从进程级默认 Prometheus registry 注销。

该文件不是指标定义本身，也不负责采集业务值。它存在的原因是同一个 IMPORT INTO 任务的 scheduler 与 task executor 可能同时存在；如果二者各自直接注册带相同常量标签的 collectors，会发生重复注册，而任一方过早注销又会使另一方失去可抓取的指标。`TaskMetricManager` 用“每任务一组指标 + 本地持有者引用计数”协调这两个生命周期。

## 核心职责

- 以 `i64` 类型的分布式任务 ID 为键，在进程内保存唯一的 `Arc<Common>`。
- 首次获取某任务时，以 `dxfproto::TaskIDLabelName` 为标签名、任务 ID 的十进制字符串为标签值，创建并注册一套 import 指标。
- 重复获取同一任务时复用同一个 `Arc`，只增加显式引用计数；不同任务获得不同指标组和不同常量标签。
- 在释放计数归零时先从内部 map 移除，再调用 `UnregisterImportMetrics` 从默认 registry 注销整组 collectors，使相同任务 ID 之后能够重新注册。
- 提供进程级惰性单例 `METRICS_MANAGER`，并用 `metricsManager` 别名兼容 Go 命名，供迁移后的 scheduler/executor 接线使用。

## 主要符号

- `TaskMetrics`：私有槽位，包含 `metrics: Arc<Common>` 和 `counter: usize`。`Arc` 供 scheduler/executor 共享指标句柄，`counter` 记录管理器层面的获取/释放配对，不等同于 `Arc::strong_count`。
- `TaskMetricManager { metrics_map: Mutex<HashMap<i64, TaskMetrics>> }`：公开管理器类型；map 和引用计数始终受同一把互斥锁保护。
- `impl Default for TaskMetricManager`：建立空 map；既用于全局单例初始化，也允许独立测试创建隔离实例。
- `METRICS_MANAGER: LazyLock<TaskMetricManager>`：首次使用时初始化的进程级单例。
- `get_or_create_metrics(&self, task_id: i64) -> Arc<Common>`：公开获取入口。命中时递增计数并克隆 `Arc`；未命中时注册带任务标签的新指标组、以零计数插入，随后统一递增为一。
- `unregister(&self, task_id: i64)`：公开释放入口。存在对应任务时用 `saturating_sub(1)` 递减；结果为零则移除并注销。未知 ID 是无操作。
- `registered_task_count(&self) -> usize`：返回 map 中仍注册的任务数，当前生产代码未调用，直接用途是 `pkg/dxf/importinto/metrics_test.rs` 的黑盒断言。
- `pub use METRICS_MANAGER as metricsManager`：仅为名称兼容的再导出，和 `METRICS_MANAGER` 指向同一个静态对象，不创建第二个管理器。

## 执行流程

1. `pkg/dxf/importinto/scheduler.rs` 的 `importScheduler::withoutMeta` 调用 `metricsManager.get_or_create_metrics(task_id)`，把返回的 `Arc<Common>` 保存到 scheduler；其 `Close` 调用 `unregister(task_id)`。
2. `pkg/dxf/importinto/task_executor.rs` 的 `RegisterImportExecutor` 工厂也对该任务调用一次获取，然后创建 `ImportNodeTaskExecutor`。执行器的 `Close` 或兜底 `Drop` 只会有一个路径通过 `closed.swap(true, Ordering::AcqRel)` 成功释放一次。
3. 第一个本地组件获取任务时，`get_or_create_metrics` 加锁并发现 map 未命中。它构造 `{ TaskIDLabelName: task_id.to_string() }`，调用 `promutil::NewDefaultFactory()` 与 `tidbmetrics::import::GetRegisteredImportMetrics`，后者创建 `Common` 并注册到默认 registry。
4. 第二个组件获取同一任务时命中既有槽位，得到指向同一 `Common` 的 `Arc`，显式计数从一变为二，不会再次注册 collectors。
5. scheduler 或 executor 先结束时，`unregister` 只把计数减为一，map 和 registry 保持不变；最后一个持有者释放时，槽位被移除，并通过 `UnregisterImportMetrics` 注销 `Common` 内的整组 collectors。
6. 注销之后再次用同一任务 ID 获取会建立新的 `Arc<Common>` 并重新注册；`metrics_test.rs::metric_manager_reference_counts_per_task` 直接验证这一流程。

## 数据与状态

唯一长期可变状态是 `metrics_map`。键是任务 ID；值把共享指标对象和显式生命周期计数绑定在一起。所有查找、插入、计数修改、移除及底层注册/注销调用均发生在互斥锁保护区内，因此同一进程内并发首次获取不会创建两套相同标签的指标。

指标标签只包含本文件加入的任务 ID；`pkg/metrics/import.rs::GetRegisteredImportMetrics` 还会通过 `GetMergedConstLabels` 合并 metrics 包的全局常量标签。`Common` 本身可克隆，注销路径克隆 map 中 `Arc` 指向的 `Common` 后，将可变克隆传给底层注销函数；collector 句柄的克隆仍表示同一批已注册对象。

显式 `counter` 追踪的是成功调用 `get_or_create_metrics` 的组件数量，而不是所有外部 `Arc` 克隆。因此调用方必须以获取/释放成对为不变量，不能依据任意 `Arc` 的存活自动延迟注销。map 长度是“当前注册任务数”，不是调用方数量或 collector 数量。

## 依赖与调用关系

上游生产调用边由精确源码引用确认：

- `scheduler.rs::importScheduler::withoutMeta -> metricsManager.get_or_create_metrics`，对应 `importScheduler::Close -> metricsManager.unregister`。
- `task_executor.rs::RegisterImportExecutor -> metricsManager.get_or_create_metrics`，对应 `ImportNodeTaskExecutor::{Close, drop} -> metricsManager.unregister`；原子 `closed` 防止 `Close` 与 `Drop` 双重释放。

下游调用边为：

- `get_or_create_metrics -> dxfproto::TaskIDLabelName`：取得 DXF 统一任务标签名。
- `get_or_create_metrics -> promutil::NewDefaultFactory`：创建默认指标 factory。
- `get_or_create_metrics -> tidbmetrics::import::GetRegisteredImportMetrics`：创建带常量标签的 `Common` 并注册到默认 registry。
- `unregister -> tidbmetrics::import::UnregisterImportMetrics`：从默认 registry 注销 `Common` 的 collectors。

`pkg/dxf/importinto/Cargo.toml` 将这些边分别声明为 `astersql-dxf-framework-proto`、`astersql-lightning-metric`、`astersql-metrics` 和 `astersql-util-promutil` 的路径依赖；`nextgen` feature 不改变本文件的编译内容，本文件也没有条件编译项。

## 错误处理与边界

本模块 API 不返回 `Result`。互斥锁若因持锁线程 panic 而中毒，三个方法都会以 `expect("import task metric mutex poisoned")` 继续 panic，避免在状态完整性不确定时静默工作。底层注册/注销 API 同样不向这里返回错误，其重复注册等实际处理由 `Common::register_to`/`unregister_from` 与 registry 封装决定。

`unregister` 对未知任务 ID 静默无操作。对已存在但计数已经为零的异常状态，Rust 使用 `saturating_sub` 保持为零并清理，避免 `usize` 下溢；正常路径中，槽位一旦减到零就立即移除，所以零计数槽位不会跨调用保留。这比 Go 的直接 `tm.counter--` 更防御性，但不能发现调用方多释放的问题。

注册与注销在持有 `metrics_map` 锁时执行，保证 map 状态与 registry 副作用的顺序一致；代价是底层 registry 操作会延长临界区。任务 ID 没有正数校验，任何 `i64` 都会被转换为标签字符串；有效 ID 的约束属于上游 DXF 任务模型。

## 并发与资源生命周期

`LazyLock` 确保全局管理器只初始化一次，`Mutex<HashMap<...>>` 串行化所有任务的管理操作。这里没有异步任务、通道、I/O 句柄或事务。锁的粒度是全局的，即不同任务 ID 的注册/释放也互斥；换取的是简单且原子的“查找或注册”和“归零或注销”过程。

资源生命周期不是由 `Arc` 自动驱动，而由 scheduler/executor 显式配对驱动。scheduler 持有返回的 `Arc<Common>`；节点执行器当前只触发注册并依靠其他运行时路径使用已绑定的指标，其包装器保证释放一次。最后一个显式持有者注销后，外部若仍保留 `Arc`，对象内存仍可存活，但 collectors 已不再由默认 registry 暴露；因此新增调用者必须同时接入可靠的关闭或 `Drop` 路径。

当前没有为 scheduler 实现自动 `Drop` 释放，本文件也没有 RAII guard。若构造过程在获取指标后、交付可关闭对象前失败，新接线必须显式回滚，避免计数泄漏。现有 scheduler 的 `withoutMeta` 后续只进行字段组装，而 executor 工厂在获取后立即组装包装器，降低了该窗口。

## 与 Go 版本的对应关系

`pkg/dxf/importinto/metrics.go` 中的 `taskMetrics`、`taskMetricManager`、全局 `metricsManager`、`getOrCreateMetrics` 和 `unregister` 分别对应 Rust 的同名/蛇形命名符号。两边都按 task ID 加锁查找，首次创建带 `proto.TaskIDLabelName` 标签的 Lightning `Common`，重复获取增加计数，归零时注销并删除 map 项。

Go scheduler 与 executor 也各获取、各释放一次，说明引用计数的设计意图在迁移中得到保留。Rust 额外使用 `Arc` 表达共享所有权、`LazyLock` 初始化全局值，并用 `metricsManager` 再导出维持 Go 名称接线；执行器还以 `AtomicBool` 协调显式关闭与析构释放。

可观察的差异包括：Go 管理器嵌入 `sync.RWMutex`，但两个修改方法实际都取写锁；Rust 直接使用 `Mutex`。Go 对计数直接减一，Rust 使用饱和减法。Go 测试替换全局默认 registry，并断言每任务八个 metric family；Rust 测试没有直接断言 family 数量，而是以成功注销后同 ID 可重新创建来验证 registry 清理，并额外断言返回 `Arc` 的指针同一性。`registered_task_count` 是 Rust 为这种隔离测试增加的观察入口。

## 扩展指南

新增使用任务指标的生产组件时，应在组件成功建立生命周期所有权的位置调用 `get_or_create_metrics`，并在每条结束、失败回滚和析构路径中确保恰好一次 `unregister`。若对象同时提供 `Close` 与 `Drop`，应复用 `ImportNodeTaskExecutor::closed` 这种一次性门闩，避免双重释放。不要绕过管理器直接为相同 task ID 注册指标。

如果新增标签，必须同步检查 `dxfproto::TaskIDLabelName` 约定、`pkg/metrics/import.rs` 的常量标签合并行为及 Go `metrics.go`，并评估 Prometheus label cardinality 和重复注册兼容性。如果改变计数或锁策略，应保持“同任务只注册一次”“最后释放才注销”“归零后可重新注册”三项不变量，并关注底层 registry 调用位于临界区时的性能。

测试应继续放在独立文件 `pkg/dxf/importinto/metrics_test.rs`，不要内嵌到生产源文件。至少同步覆盖同 ID 复用、不同 ID 隔离、部分释放不注销、最终释放后可重建、未知 ID 释放；涉及 scheduler/executor 接线时，还应在各自的独立测试文件验证获取与关闭路径配对。与 Go 行为对齐的断言同时参考 `pkg/dxf/importinto/metrics_test.go`。

## 验证依据

- 目标实现：`pkg/dxf/importinto/metrics.rs`；确认了两个结构体、`Default`、全局单例、三个方法和兼容别名，且不存在 trait、宏、异步函数或条件编译项。
- crate 边界：`pkg/dxf/importinto/lib.rs` 与 `pkg/dxf/importinto/Cargo.toml`；确认模块公开装配、独立测试装配、四项直接依赖及本文件不受 `nextgen` feature 分支影响。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query TaskMetricManager` 和 `node TaskMetricManager` 定位到本文件第 38 行；`explore`/调用查询识别 `get_or_create_metrics`、`registered_task_count` 的 Rust 测试调用。通用名 `unregister` 的图结果存在歧义，因此生产调用边又用精确源码引用核验。
- 生产上游：`pkg/dxf/importinto/scheduler.rs` 的 `withoutMeta`/`Close`，以及 `pkg/dxf/importinto/task_executor.rs` 的 `RegisterImportExecutor`、`ImportNodeTaskExecutor::Close`/`Drop`。
- 下游实现：`pkg/metrics/import.rs::{GetRegisteredImportMetrics, UnregisterImportMetrics}` 与 `pkg/lightning/metric/metric.rs::Common`，确认默认 registry 副作用和 `Common: Clone`。
- 对照实现与测试：`pkg/dxf/importinto/metrics.go`、`pkg/dxf/importinto/metrics_test.go`、`pkg/dxf/importinto/metrics_test.rs`；Rust 测试确认同任务共享、跨任务隔离、引用计数、最终注销和同 ID 重建。
- 本任务是纯文档分析，按计划不运行 Cargo；交付只执行固定章节结构检查，并人工复核上述符号、调用边、边界差异和扩展不变量均能由所列路径追溯。
