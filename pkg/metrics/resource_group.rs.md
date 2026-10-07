# `pkg/metrics/resource_group.rs`

## 文件定位

`resource_group.rs` 属于 `astersql-metrics` crate，由 `pkg/metrics/lib.rs` 通过 `pub mod resource_group` 公开。它是 Resource Group runaway 可观测性的 collector 定义层：为 checker、flusher 和 syncer 构造 Prometheus 句柄，但不执行查询判定、批量落盘或节点同步。Crate 边界和直接依赖见 `pkg/metrics/Cargo.toml`：本文件通过 `crate::bindinfo` 的兼容层使用 `astersql-metrics-common` 和 `prometheus` 0.14。

应用内的装配链是 `pkg/metrics/metrics.rs::InitMetrics` → `init_resource_group_metrics` → `pkg/metrics/metrics.rs::RegisterMetrics`。前一步构造并存入包级静态量，后一步才将这些 collector 注册到默认 registry。

## 核心职责

- 统一声明 10 个 runaway collector，并保持指标名、Help、标签顺序和 histogram bucket 与 `pkg/metrics/resource_group.go::InitResourceGroupMetrics` 一致。
- 在 `init_resource_group_metrics` 中一次性建立 checker、flusher、syncer 三组句柄，供统一指标初始化/注册流程使用。
- 以 `Option<...>` 表达 Go 包级指针“初始化前为 nil”的状态。它不包含记录指标的业务 API，也不管理 runaway 记录、watch 或定时任务。

## 主要符号

- `RUNAWAY_CHECKER_COUNTER: Option<prometheus::CounterVec>`：`tidb_server_query_runaway_check`，标签依次为 `resource_group` / `type` / `action`，区分资源组、命中类型和处置动作。
- `RUNAWAY_FLUSHER_COUNTER`：`tidb_server_runaway_flusher_total{name,result}`；`RUNAWAY_FLUSHER_ADD_COUNTER`：`tidb_server_runaway_flusher_add_total{name}`。
- `RUNAWAY_FLUSHER_BATCH_SIZE_HISTOGRAM`：批大小桶从 1 开始、倍率 2、共 10 个，即 1–512。
- `RUNAWAY_FLUSHER_DURATION_HISTOGRAM`：秒制耗时桶从 0.001 开始、倍率 2、共 15 个；`RUNAWAY_FLUSHER_INTERVAL_HISTOGRAM` 从 0.1 开始、倍率 2、共 12 个。三者都以 `name` 分组。
- `RUNAWAY_SYNCER_DURATION_HISTOGRAM` 和 `RUNAWAY_SYNCER_INTERVAL_HISTOGRAM`：分别复用 flusher 的耗时桶和间隔桶，以 `type` 分组。
- `RUNAWAY_SYNCER_CHECKPOINT_GAUGE`：`tidb_server_runaway_syncer_checkpoint{type}`，保存下一扫描窗口的 Unix 毫秒下界；`type` 在 Go syncer 中区分 `watch` 的 `start_time` 与 `watch_done` 的 `done_time`。
- `RUNAWAY_SYNCER_COUNTER`：`tidb_server_runaway_syncer_total{type,result}`。
- `unsafe fn init_resource_group_metrics()`：本文件唯一函数，按 checker、flusher、syncer 顺序覆盖上述 10 个静态量；无参数、无返回值、无条件编译分支。

## 执行流程

1. `pkg/metrics/metrics.rs::InitMetrics` 由 `INIT_METRICS_ONCE.call_once` 保证进程内只执行一次，并在其子系统初始化序列中调用 `init_resource_group_metrics`。
2. `init_resource_group_metrics` 通过 `metricscommon::NewCounterVec` / `NewHistogramVec` / `NewGaugeVec` 依次构造 4 个 counter vec、5 个 histogram vec 和 1 个 gauge vec，并写入对应 `static mut Option`。
3. Histogram 在构造时通过 `prometheus::ExponentialBuckets` 生成固定桶边界；其余 descriptor 字段和 label 名由本文件直接给出。
4. 后续 `pkg/metrics/metrics.rs::RegisterMetrics` 通过 `register_options!` 从每个 `Option` 取出句柄、clone，再注册到 Prometheus 默认 registry。
5. 目前 Rust 代码库中这 10 个静态量的直接引用只出现在初始化/注册链；尚未看到 Rust runaway checker/flusher/syncer 调用 `.with_label_values(...)` 并记录数值的消费链。

## 数据与状态

文件不持有 SQL、资源组配额或 runaway 记录；唯一可变状态是 10 个进程级 collector 句柄。每个静态量初值为 `None`，初始化后为 `Some(Vec)`。标签集是 descriptor 协议的一部分：标签数或顺序不一致会使调用方取得时序列失败或改变仪表盘查询含义。

Counter 只累加事件，histogram 累积观测值到桶，gauge 允许 checkpoint 前进时设置新值。本文件未预绑定任何 label value，因而也不会在初始化时自动生成带标签的时序列。

## 依赖与调用关系

- 上游：RustCodeGraph 的文件边显示 `pkg/metrics/metrics.rs` 是唯一使用本文件的 Rust 文件；`InitMetrics` 调用初始化函数，`RegisterMetrics` 引用所有 10 个句柄。
- 下游：`crate::bindinfo::{compat_metricscommon, compat_prometheus}` 提供 Go 风格构造器和 opts；`crate::*` 提供 `LblResourceGroup`、`LblType`、`LblAction`、`LblName`、`LblResult` 等标签名。RustCodeGraph `callees init_resource_group_metrics` 还确认了到 `pkg/metrics/bindinfo.rs::ExponentialBuckets` 的调用边。
- Crate 依赖：`pkg/metrics/Cargo.toml` 的包名是 `astersql-metrics`，其 `[lib]` 入口为 `lib.rs`，并声明 `astersql-metrics-common` 路径依赖和 `prometheus = "0.14"`。未定义与本文件相关的 Cargo feature。
- 业务语义对照：Go 的 `pkg/resourcegroup/runaway/checker.go`、`flusher.go` 和 `syncer.go` 分别绑定这些指标的 label value 并更新序列；这些是目前业务使用方的直接证据，不是 Rust 已接线的证据。

## 错误处理与边界

`init_resource_group_metrics` 本身不返回 `Result`，也没有显式错误分支；它依赖兼容构造器建立 descriptor。注册错误在本文件之外由 `RegisterMetrics() -> Result<(), prometheus::Error>` 向上传播。若未先执行 `InitMetrics`，`register_option` 会以 `expect("InitMetrics must run before RegisterMetrics")` panic；重复注册相同 descriptor 则由 Prometheus registry 返回错误。

输入边界不在本文件内：它不校验 label value、时长、批量大小或 checkpoint 的单调性。`RUNAWAY_SYNCER_CHECKPOINT_GAUGE` 的数值单位由 Help 约定为 Unix 毫秒，而 duration/interval 指标名明确约定为秒；消费方必须维持这些单位。

## 并发与资源生命周期

`static mut` 读写需要 `unsafe`，类型本身不能阻止并发覆盖或读写竞争。实际包级生命周期由 `pkg/metrics/metrics.rs` 约束：`INIT_METRICS_ONCE` 保证统一入口只初始化一次，`INIT_METRICS_DONE` / `INIT_METRICS_ERROR` 记录首次结果。本函数自身仍是 `pub unsafe`，若调用方绕过 `InitMetrics` 并发或重复调用，安全性与旧 collector 的生命周期不受它保护。

本文件不创建线程、异步任务、通道、锁、事务或计时器。Collector 在全局静态量和 Prometheus registry 中存活至进程结束；克隆用于注册的句柄与原句柄共享指标状态，这是 `metrics.rs::register_clone` 的资源交接方式。

## 与 Go 版本的对应关系

Rust `init_resource_group_metrics` 是 `pkg/metrics/resource_group.go::InitResourceGroupMetrics` 的直接移植：10 个指标的 namespace (`tidb`)、subsystem (`server`)、name、Help、label 顺序和三组指数桶参数逐项一致。Go 使用可为 nil 的 `*prometheus.*Vec` 包级变量，Rust 使用 `static mut Option<...>` 表达同一初始化状态。Go `metrics.go` 同样先调用 `InitResourceGroupMetrics`，再对 10 个 collector 逐个 `MustRegister`；Rust 将注册失败保留为 `Result`。

Go 业务侧的具体 label 契约可从以下文件复核：`checker.go` 使用资源组名、match type 和 action；`flusher.go::newBatchFlusher` 使用 flusher name 及 `ok`/`error`；`syncer.go::newSyncer` 使用 `sync`、`watch`、`watch_done` 及 `ok`/`error`。Rust 对应的 runaway 业务模块尚未消费这些 collector，因此当前达到“定义与注册对齐”，不能据此声称 Rust 已完成运行时指标上报对齐。

## 扩展指南

- 新增指标时，在 `resource_group.rs` 增加独立静态句柄并在 `init_resource_group_metrics` 构造，同时把它加入 `metrics.rs::RegisterMetrics` 的 `register_options!` 列表；否则只会构造而不会暴露。
- 必须同步核对 `resource_group.go`、真实业务消费方和仪表盘/告警所依赖的指标名与 label 顺序。改名或改 label 是时序 API 兼容性变更，会影响查询和序列基数；引入高基数 label（如原始 SQL）会带来明显内存与抓取性能风险。
- 修改 bucket 前要核对单位和实际分布；duration/interval 是秒，checkpoint 是 Unix 毫秒，batch size 是记录数。
- Rust 测试应放在独立文件，例如新建 `pkg/metrics/resource_group_test.rs` 并从 `pkg/metrics/lib.rs` 以 `#[cfg(test)] #[path = "resource_group_test.rs"] mod resource_group_test;` 接入；不应在生产文件内嵌 `#[cfg(test)] mod tests`。建议覆盖 descriptor 名/Help/label、bucket 边界、`InitMetrics` 后的句柄可用性以及 `RegisterMetrics` 后的 gather 结果。
- 当 Rust runaway checker/flusher/syncer 实现接线时，可以 `pkg/resourcegroup/runaway/flusher_test.go` 为 flusher 边界测试参考，并为 Rust 记录路径增加独立回归测试；仅测试 collector 能构造不足以证明业务消费已对齐。

## 验证依据

- RustCodeGraph：`status` 显示本工作区已索引；`node --file pkg/metrics/resource_group.rs --offset 1 --limit 260` 读取了 161 行全文；`query init_resource_group_metrics --kind function` 定位唯一函数；`callees init_resource_group_metrics` 确认 `ExponentialBuckets` 调用边。`callers` 在本地查询未输出结果，因此上游边额外由已索引的 `metrics.rs` 源码和文件 used-by 边复核。
- Rust 源码/Crate：`pkg/metrics/resource_group.rs`、`pkg/metrics/metrics.rs`、`pkg/metrics/lib.rs`、`pkg/metrics/session.rs`、`pkg/metrics/bindinfo.rs`、`pkg/metrics/Cargo.toml`。
- Go 对照：`pkg/metrics/resource_group.go`、`pkg/metrics/metrics.go`、`pkg/resourcegroup/runaway/checker.go`、`pkg/resourcegroup/runaway/flusher.go`、`pkg/resourcegroup/runaway/syncer.go`。
- 测试证据：`rg` 未找到直接覆盖这 10 个 Rust 静态量或 `init_resource_group_metrics` 的独立 Rust 测试；`pkg/metrics/metrics_internal_test.rs` 只证明统一 `InitMetrics` / `RegisterMetrics` / gather 测试模式，其 RU 断言不直接覆盖本文件。Go 消费行为的直接测试位于 `pkg/resourcegroup/runaway/flusher_test.go`；同目录的 checker/syncer Go 测试及 Rust `pkg/resourcegroup/runaway/syncer_test.rs` 可用于业务边界背景，但不直接验证这些 Rust collector。
- 本任务为纯文档分析，按计划不运行 Cargo；交付检查限于固定章节结构、路径/符号存在性、Go/Rust 定义对照和人工事实复核。
