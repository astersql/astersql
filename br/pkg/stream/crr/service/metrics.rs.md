# `br/pkg/stream/crr/service/metrics.rs`

## 文件定位

本文件是 `astersql-br-pkg-stream-crr-service` library crate 的私有指标适配层。crate 入口 [`lib.rs`](lib.rs) 以 `mod metrics` 挂载它，但不向 crate 外导出；状态层 [`status.rs`](status.rs) 才是生产调用方。它把 `StatusSnapshot` 的当前值投影到默认 Prometheus registry，形成 `tidb_br_crr_*` 指标，供监控系统抓取；自身不推进 checkpoint、不维护 CRR 状态机，也不提供 HTTP endpoint。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：本 crate 直接依赖 `prometheus = "0.14"`，并从相邻的 `astersql-br-pkg-stream-crr-internal-checkpoint` crate 引入 `EventType`。目录下没有 `doc.go`；Rust crate 入口 `lib.rs` 和 Go 同路径实现 [`metrics.go`](metrics.go) 是最近的模块契约证据。

## 核心职责

1. `GaugeMap::new` 定义并注册 16 个 `GaugeVec`，统一使用 namespace `tidb`、subsystem `br_crr`；最终指标名形如 `tidb_br_crr_service_live`。
2. `observe_status_metrics` 在状态快照每次变化后全量刷新指标。它把 `State` 的 4 个候选值和 `Phase` 的 7 个候选值写成 one-hot gauge，既将当前项写为 `1`，也将旧项明确清零。
3. 其余字段按 `task` 标签直接写为标量，包括存活/就绪、轮次、水位、文件计数和连续失败次数。
4. `skipped_store_synced_meta_file_count_metric` 只为 crate 内测试暴露单项读取能力，不是生产公共 API。

本文件只镜像快照；指标何时变化以及字段值的业务含义由 `StatusStore` 决定。特别是 `status.rs` 在构造、启动、停止、持久状态更新、清除失败、开始轮次和应用 checkpoint 事件之后调用本模块。

## 主要符号

### `GaugeMap`

`GaugeMap { values: HashMap<&'static str, GaugeVec> }` 是指标短名到 collector 的内部索引。短名仅用于本模块分派，实际导出名称由 `Opts::new(...).namespace("tidb").subsystem("br_crr")` 组合生成。

- `GaugeMap::new() -> Self`：逐项构造并向 `prometheus::default_registry()` 注册 16 个 collector；定义或注册失败会 panic。
- `GaugeMap::set(&self, name, labels, value)`：忽略标签元组中的名字，只按传入顺序提取值，因此调用方顺序必须与定义时的 `variable_labels` 一致。
- `GaugeMap::get(&self, name, labels) -> f64`：供测试读取已有 gauge；与 Go 的 `promtest.ToFloat64` 用途对应。

### `metrics`

`fn metrics() -> &'static GaugeMap` 使用函数内 `static METRICS: OnceLock<GaugeMap>`。第一次观察或测试读取时注册 collectors，之后返回同一进程级实例。

### `observe_status_metrics`

`pub(crate) fn observe_status_metrics(snapshot: &StatusSnapshot)` 是唯一生产写入口。它写入：

- `service_live`、`service_ready`；
- `service_state{task,state}` 的 `starting/running/degraded/stopped` 四项；
- `service_phase{task,phase}` 的 `idle` 和六个 `EventType` 阶段；
- `current_round`、`last_loop_iteration`、`last_upstream_checkpoint`、`safe_checkpoint`、`synced_ts`；
- `alive_store_count`、`pending_file_count`、`consecutive_failures`；
- `upstream_read_meta_file_count`、`skipped_store_synced_meta_file_count`、`estimated_sync_log_file_count`、`downstream_check_file_count`。

### 测试辅助与转换函数

`skipped_store_synced_meta_file_count_metric(task)` 读取指定 task 的 skipped gauge。`bool_to_float` 将 `true/false` 映射为 `1.0/0.0`，保持 Prometheus gauge 与 Go 实现的数值约定。

## 执行流程

1. `new_status_store` 创建 `starting/idle` 快照，并在仍持有状态读锁时首次调用 `observe_status_metrics`。
2. `observe_status_metrics` 调用 `metrics()`；进程内首次调用会执行 `GaugeMap::new`，构建并注册全部 16 个 collectors，后续调用复用 `OnceLock` 中的实例。
3. 函数先遍历全部合法 state，再遍历全部合法 phase。每个候选标签都与快照当前字符串比较并写 `1.0` 或 `0.0`，因此切换状态/阶段不会留下旧标签值为 1。
4. 函数再刷新两个布尔探针、八个服务进度/健康标量和四个 `StatusStatistic` 文件统计标量。整数通过 `as f64` 转成 gauge 值。
5. 后续 `StatusStore::start`、`stop`、`set_persistent_state`、`clear_failure`、`begin_round`、`apply_event` 都在更新同一快照后再次执行上述完整刷新。
6. Prometheus 抓取通过默认 registry 读取 collector；`service_test.rs::test_status_metrics_are_registered_for_prometheus_gathering` 以 `prometheus::gather()` 验证实际注册和 task 标签。

## 数据与状态

指标状态由全局 `OnceLock<GaugeMap>` 持有，生命周期与进程一致。每个 `GaugeVec` 又按标签值保存时间序列：大多数指标只有 `task`；`service_state` 增加 `state`；`service_phase` 增加 `phase`。因此 task 名、state 或 phase 的新标签值会产生新的序列，本文件没有删除标签序列的逻辑。

本模块不保存一份独立业务快照，而是每次从借用的 `&StatusSnapshot` 重建当前指标值。state/phase 使用 one-hot 全量覆盖；其他 gauge 使用最新快照覆盖同一 task 的旧值。`SyncedByStore`、错误文本、时间字段以及两个按后缀计数的 map 不导出为指标，只存在于状态/JSON 表达中。

整数到 `f64` 的转换与 Go 的 `float64(...)` 一致，但极大的 `u64` 超过 IEEE-754 精确整数范围后可能失去低位精度；当前实现没有范围检查。这对 timestamp/checkpoint gauge 的精确查询是扩展时需要保留或明确改变的兼容边界。

## 依赖与调用关系

上游生产调用链由 RustCodeGraph 核对为：

- `new_status_store -> observe_status_metrics`：初始化时发布 starting/idle 零值快照；
- `StatusStore::{start, stop, set_persistent_state, clear_failure, begin_round, apply_event} -> observe_status_metrics`：状态写路径完成后同步发布；
- `observe_status_metrics -> metrics -> GaugeMap::new`：首次调用惰性初始化；
- `observe_status_metrics -> GaugeMap::set`，并通过 `bool_to_float` 转换两个布尔值。

下游直接依赖是 `prometheus::{Opts, GaugeVec, default_registry}`。phase 标签还依赖 checkpoint crate 的 `EventType::as_str()`，因此事件字符串变化必须与 Go、状态 JSON、指标和测试同步。`http.rs` 不直接调用本文件，但 `/livez`、`/readyz` 读取的 `Live/Ready` 与 `service_live/service_ready` 来自同一快照，二者语义同源。

测试调用链为 `service_test.rs` 和 `parity_test.rs -> skipped_store_synced_meta_file_count_metric -> metrics -> GaugeMap::get`；另有 `service_test.rs` 直接调用 `prometheus::gather()` 检查默认 registry。

## 错误处理与边界

本模块没有 `Result` 返回路径，配置错误采用启动期/首次观察期快速失败：

- `GaugeVec::new(...).expect("valid CRR gauge definition")` 在标签定义非法时 panic；
- 默认 registry 注册失败（例如同名 collector 已注册）时以 `expect("register CRR gauge")` panic；
- `set/get` 收到未知短名时以 `expect("known CRR gauge")` panic；
- 标签数量不匹配由 Prometheus API 的 `with_label_values` 触发 panic。

当前所有名字和标签均为本文件内静态清单，正常调用路径不会传入任意外部指标名。快照的 `State` 或 `Phase` 若不是固定候选之一，所有已知 one-hot 序列都会被写为 0，且不会为未知字符串创建序列；这能避免无界 label，但也会把未知状态表现为“无当前项”。本模块不校验负数：来自 `i32` 的 store/file 计数会原样转换为负 gauge；对应状态编码测试已明确覆盖有符号数。

## 并发与资源生命周期

`OnceLock` 保证多个线程首次并发观察时只初始化一个 `GaugeMap`。注册后的 `GaugeVec` 可被共享引用更新，`observe_status_metrics` 本身不创建线程、任务、channel 或事务，也不持有额外锁。

调用方 `StatusStore` 使用 `Arc<RwLock<...>>` 保护快照，并在持有写锁时调用 `observe_status_metrics`；初始化路径则持有读锁。结果是同一个 store 的“修改快照 + 刷新指标”不会与该 store 的其他写路径交错，但全局指标更新并不构成对多个 store 或多个同名 task 的事务。若并发存在两个 `StatusStore` 使用相同 `TaskName`，它们会写同一标签序列，最后一次写入胜出。

collector 和已经出现过的 task 标签序列存活到进程结束。服务 `stop` 只把 live/ready/state 更新为停止值，不 unregister collector，也不删除该 task 的指标。这与本文件的进程级单例设计一致，但动态、高基数 task 名会带来常驻内存与监控基数风险。

## 与 Go 版本的对应关系

Go 对照文件是 [`metrics.go`](metrics.go)。两端都有相同的 16 个指标名、help 文本、namespace/subsystem、标签集合、四个 state 和七个 phase，也都在每次状态变化后全量写指标。Rust 的 `GaugeMap` 只是把 Go 的 16 个包级变量集中放进短名 map；`bool_to_float` 对应 `boolToFloat`，`observe_status_metrics` 对应 `observeStatusMetrics`。

已确认的实现差异是注册时机：Go `init()` 在包初始化时 `MustRegister`；Rust 在 `metrics()` 首次被调用时通过 `OnceLock` 向默认 registry 惰性注册。由于 `new_status_store` 构造时立即观察一次，正常服务构造后指标仍会注册；但仅链接 crate 而从不创建/观察状态时，Rust 不会注册这些指标，而 Go 会。

Go `service_test.go::TestStatusStoreTracksFileStatistic` 用 `promtest.ToFloat64` 验证 skipped 值为 4；Rust 的同名语义测试使用本模块测试辅助函数做相同断言。Rust 另外以 `test_status_metrics_are_registered_for_prometheus_gathering` 明确验证 `tidb_br_crr_service_live` 能被默认 gatherer 收集。`parity_test.rs::contract_resource_cleanup_on_shutdown` 也验证 RoundPlanned 的 skipped 统计能更新指标，并随后核对 stopped 生命周期状态。

## 扩展指南

新增一个从 `StatusSnapshot` 导出的指标时，应保持以下最小闭环：

1. 在 `GaugeMap::new` 的静态清单中增加 name、help 和有界标签；避免把 checkpoint、store ID 或任意错误文本作为 label。
2. 在 `observe_status_metrics` 中从快照写入该指标，并严格保持标签值顺序与定义顺序一致。
3. 同步修改 Go `metrics.go` 的 collector、注册和 `observeStatusMetrics`，除非任务明确记录跨语言差异。
4. 若指标需要新状态字段，业务事实应先在 `status.rs`/Go `status.go` 产生；不要在 metrics 层复制状态机或自行推导异步状态。
5. 在独立测试文件 `service_test.rs` 或 `parity_test.rs` 增加断言；Rust 测试不要内嵌到 `metrics.rs`。对应 Go 行为应在 `service_test.go` 保持一致。

修改 state/phase 枚举时，必须同时更新 one-hot 候选、checkpoint `EventType::as_str()` 或状态常量、Go 候选清单及测试。修改指标名、help、namespace、subsystem 或标签属于监控 API 兼容变化，可能破坏 dashboard/告警；修改惰性注册策略则需评估默认 registry 重复注册和测试进程共享 registry 的风险。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标 `metrics.rs` 已索引为 9 个符号。
- RustCodeGraph `node --file br/pkg/stream/crr/service/metrics.rs`：核对完整 286 行源码、16 项 Gauge 定义和全部写入字段。
- RustCodeGraph `node observe_status_metrics`：核对 callees 为 `metrics`、`GaugeMap::set`、`bool_to_float`，callers 为 `status.rs` 的模块引用、`new_status_store` 及 `start/stop/set_persistent_state/clear_failure/begin_round/apply_event`。
- 源码/配置：[`metrics.rs`](metrics.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`status.rs`](status.rs)、[`http.rs`](http.rs)。
- Go 对照：[`metrics.go`](metrics.go)、[`status.go`](status.go)、[`service_test.go`](service_test.go)。
- Rust 独立测试：[`service_test.rs`](service_test.rs) 中 `test_status_store_tracks_file_statistic`、`test_status_metrics_are_registered_for_prometheus_gathering`；[`parity_test.rs`](parity_test.rs) 中 `contract_resource_cleanup_on_shutdown`。
- checkpoint 统计来源：[`../internal/checkpoint/progress.rs`](../internal/checkpoint/progress.rs) 增加 skipped 计数，[`../internal/checkpoint/calculator.rs`](../internal/checkpoint/calculator.rs) 将其放入 `FileStatistic`；对应 Rust 测试位于 [`../internal/checkpoint/checkpoint_calculator_test.rs`](../internal/checkpoint/checkpoint_calculator_test.rs)。
- 本任务是纯文档分析，按任务约束未运行 Cargo；结构验证命令及退出状态在交付前单独执行并记录。
