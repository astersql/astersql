# `pkg/statistics/handle/cache/metrics/metrics.rs`

## 文件定位

本文件是统计信息缓存指标的“标签绑定层”：它不创建或注册 Prometheus collector，而是把父级 `StatsCacheCounter`、`StatsCacheGauge` 中固定 `type` 标签对应的 time series 绑定为缓存代码可直接使用的全局句柄。源码由同 crate 的 [`lib.rs`](./lib.rs) 以 `cache_metrics` 模块载入；crate 名为 `astersql-statistics-handle-cache-metrics`，仅直接依赖 `prometheus = "0.14"`，其移植元数据指向 Go 包 `pkg/statistics/handle/cache/metrics`（[`Cargo.toml`](./Cargo.toml)）。

应用初始化链位于 `pkg/util/metricsutil/common.rs`：`initParentMetricsCollectors` 先调用该 crate 的 `metrics::init_parent_metrics()` 创建父向量，`initMetrics` 再调用本模块 `InitMetricsVars()`。缓存 crate 和 LFU 子 crate 通过各自的 Cargo 路径依赖消费这些句柄。

## 核心职责

- 声明六个操作计数器：`MissCounter`、`HitCounter`、`UpdateCounter`、`DelCounter`、`EvictCounter`、`RejectCounter`。
- 声明两个数值仪表：`CostGauge`、`CapacityGauge`。
- 由 `InitMetricsVars` 使用固定标签值把上述句柄绑定到父 `CounterVec`/`GaugeVec`，保持指标名、标签和值域与 Go 实现一致。
- 由 `init` 提供一个薄初始化入口。Rust 不会像 Go 包那样自动执行包级 `init()`；当前完整应用的实际接线直接调用 `InitMetricsVars`。

本文件不负责缓存算法、指标注册或父向量构造，也不直接递增/设置指标。

## 主要符号

- `pub static mut MissCounter/HitCounter/UpdateCounter/DelCounter/EvictCounter/RejectCounter: Option<prometheus::Counter>`：分别绑定 `type=miss/hit/update/del/evict/reject`。初始值均为 `None`，初始化后为对应父向量 time series 的克隆句柄。
- `pub static mut CostGauge/CapacityGauge: Option<prometheus::Gauge>`：分别绑定 `type=track/capacity`。名称中的 `CostGauge` 表示缓存当前内存成本；Go 注释写成 “cost time” 与真实调用点不一致，Rust LFU 和缓存主体都把缓存 cost 写入它。
- `pub fn init()`：只调用 `InitMetricsVars()`，用于显式模拟 Go 包初始化入口。
- `pub fn InitMetricsVars()`：唯一实质函数；先要求两个父向量存在，再依固定标签取得八个子句柄并覆盖相应全局 `Option`。
- crate 级 `#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]`：允许保留 Go 导出名并访问可变全局，从而维持迁移 API 形状；这也是本文件最主要的 Rust 安全边界。

## 执行流程

1. `pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 调用 `astersql_statistics_handle_cache_metrics::metrics::init_parent_metrics()`，建立名为 `tidb_statistics_stats_cache_op` 与 `tidb_statistics_stats_cache_val`、标签键均为 `type` 的父向量。
2. `pkg/util/metricsutil/common.rs::initMetrics` 调用 `statscache_metrics::InitMetricsVars()`。
3. `InitMetricsVars` 在 `unsafe` 块中读取 `metrics::StatsCacheCounter`；若尚未初始化则以明确消息 panic。
4. 函数依次用 `with_label_values` 绑定 `miss`、`hit`、`update`、`del`、`evict`、`reject`，并写入六个计数器全局句柄。
5. 函数以同样方式读取 `metrics::StatsCacheGauge`，绑定 `track`、`capacity` 两个 gauge。
6. 运行期缓存操作通过 `pkg/statistics/handle/cache/statscacheinner.rs::count`、`set_cost` 以及 `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs` 的指标辅助函数更新这些句柄；多个句柄与父向量的相应标签共享同一 Prometheus time series。

重复执行初始化只会重新取得并覆盖句柄，不会重建父向量，也不会清零已有 series；`migration_aster_unit_test.rs::cache_metric_handles_match_go_labels_and_share_parent_series` 对此有直接断言。

## 数据与状态

文件持有八个进程级可变全局 `Option`。`None` 表示尚未完成标签绑定，`Some` 中的 `Counter`/`Gauge` 是 Prometheus collector 子 series 的句柄。计数器只能累加，分别记录命中、未命中、更新、删除、驱逐和拒绝；gauge 可覆盖，`track` 表示当前成本，`capacity` 表示容量上限。

标签字符串是监控协议的一部分：父向量只声明一个 `type` 标签，因此 `with_label_values` 必须恰好传一个值。修改标签键、标签数量或这些固定值会创建不同的 series，并可能破坏仪表盘及告警兼容性。

## 依赖与调用关系

上游初始化调用为 `pkg/util/metricsutil/common.rs::initMetrics -> InitMetricsVars`；测试还从 `migration_aster_unit_test.rs` 与 `statscache_test.rs` 直接调用它。`init -> InitMetricsVars` 是文件内唯一函数调用边。

下游依赖是同 crate `lib.rs` 中的 `metrics::StatsCacheCounter`、`metrics::StatsCacheGauge` 以及 `prometheus::{Counter, Gauge}`。生产消费者包括：

- `pkg/statistics/handle/cache/statscacheinner.rs::count`：更新 hit、miss、update、del；`set_cost` 写 cost。
- `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs::{setCostGauge,setCapacityGauge,incrementEvictCounter,incrementRejectCounter}`：更新 LFU 成本、容量、驱逐与拒绝指标。

RustCodeGraph 将目标文件标记为被 `migration_aster_unit_test.rs` 和 `statscache_test.rs` 使用；精确 `callers/callees` 查询没有产出函数边，因此上述应用级调用与消费关系由局部源码引用搜索交叉核验。

## 错误处理与边界

`InitMetricsVars` 没有 `Result` 返回值。父 `StatsCacheCounter` 或 `StatsCacheGauge` 为 `None` 时分别通过 `expect` 立即 panic，这把“父 collector 必须先初始化”定义为启动顺序不变量，而非可恢复运行时错误。标签数量若与向量不匹配，Prometheus 库的 `with_label_values` 同样会 panic。

消费者通常以 `if let Some(...)` 更新指标，因此绑定前发生的缓存操作会静默跳过指标更新；它们不会因为句柄为 `None` 而使业务失败。相反，直接读取句柄的测试会 `unwrap`，用于暴露初始化遗漏。

本文件不验证数值范围：缓存成本和容量由调用方转换为 `f64` 后写入 gauge。它也不注册 collector；注册/父向量所有权属于上级指标初始化层。

## 并发与资源生命周期

所有句柄具有进程生命周期，没有显式释放逻辑。Prometheus `Counter`/`Gauge` 克隆句柄指向共享 series，因此重绑定不会复制或重置指标值。具体指标自身支持并发更新，但保存句柄的 `static mut Option<_>` 不提供 Rust 级同步保证；初始化顺序必须保证绑定发生在并发缓存活动之前，且运行期不得与 `InitMetricsVars` 并发改写这些全局变量。

缓存消费者通过 `unsafe` 读取全局句柄，并在 `None` 时跳过；这降低了未初始化时的业务影响，但不能消除并发重绑定造成数据竞争的风险。相关测试对全局配置与指标的修改使用子进程隔离（`statscache_test.rs::global_configuration_metrics_and_failpoints`），表明这些状态不适合普通并行测试共享。

## 与 Go 版本的对应关系

同路径 [`metrics.go`](./metrics.go) 定义完全相同的八个导出句柄、`init()` 和 `InitMetricsVars()`，标签映射逐项一致。Rust 使用 `Option` 表达 Go 全局接口变量初始化前的空状态，并用 `expect` 明确父向量初始化顺序；Go 的包初始化机制会自动执行 `init()`，Rust 则由 `metricsutil` 显式接线。

父指标的命名空间、子系统、名称、帮助文本和 `type` 标签可在 `pkg/metrics/stats.go::InitStatsMetrics` 与 Rust `pkg/metrics/stats.rs::InitStatsMetrics` 中交叉验证。当前指标更新点也保持 Go 意图：`statscacheinner.go` 更新 hit/miss/update/del，`internal/lfu/lfu_cache.go` 更新 capacity/cost/evict/reject。

一个需注意的实现差异是 Rust 消费者多在句柄为 `None` 时跳过更新，而 Go 依赖包初始化后句柄总是可用；扩展时不能据此把“未初始化也正常”当作完整应用的预期状态。

## 扩展指南

新增缓存指标操作类型时，应同时修改父向量契约、本文件的全局句柄与 `InitMetricsVars` 标签绑定、真实缓存消费点，以及同路径 Go 实现（若仍要求迁移对齐）。若只是增加同一 `type` 标签下的新 counter/gauge 值，不应另建独立 collector；若需要新的标签维度，则必须评估 time-series 基数、仪表盘兼容性及 `with_label_values` 参数数量。

测试应保持在独立文件中：优先扩展 `pkg/statistics/handle/cache/metrics/migration_aster_unit_test.rs` 验证新句柄与父 series 共享、重复初始化不清零；涉及真实缓存路径时扩展 `pkg/statistics/handle/cache/statscache_test.rs`，LFU 行为则在相应独立测试中验证。测试全局 `static mut` 时应沿用串行或子进程隔离，避免并行污染。

若要降低 `static mut` 风险，可评估一次性容器或同步封装，但这会改变公开句柄访问方式和重复绑定语义，不能作为本文件的局部机械替换。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、目标目录四个索引文件；`node --file` 核对了 `metrics.rs`、`lib.rs`、`metrics.go`、`migration_aster_unit_test.rs` 和 `statscache_test.rs`。目标文件索引显示被后两份 Rust 测试使用；精确 `callers/callees` 未返回边，故未据此宣称不存在调用。
- 源码与配置：`pkg/statistics/handle/cache/metrics/metrics.rs`（八个全局句柄、`init`、`InitMetricsVars`）；同目录 `Cargo.toml` 与 `lib.rs`（crate 边界、依赖、父向量）；`pkg/util/metricsutil/common.rs`（完整应用初始化顺序）。
- 生产调用点：`pkg/statistics/handle/cache/statscacheinner.rs` 与 `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`；父 collector 定义还与 `pkg/metrics/stats.rs`、`pkg/metrics/stats.go` 核对。
- Go 对照：`pkg/statistics/handle/cache/metrics/metrics.go`、`pkg/statistics/handle/cache/statscacheinner.go`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache.go`。
- 独立测试：`pkg/statistics/handle/cache/metrics/migration_aster_unit_test.rs::cache_metric_handles_match_go_labels_and_share_parent_series` 验证八个标签、父子 series 共享和重复绑定不清零；`pkg/statistics/handle/cache/statscache_test.rs::global_configuration_metrics_and_failpoints` 验证 hit/miss/update/del/cost 在真实缓存操作中的更新。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付前以规定命令验证目标文档存在且恰好包含十一个固定二级标题，并人工复核无运行时能力臆测。
