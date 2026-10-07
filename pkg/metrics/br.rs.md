# `pkg/metrics/br.rs`

## 文件定位

本文件属于 `astersql-metrics` crate，是 BR（Backup & Restore）观测指标的 Rust 定义与初始化模块。crate 入口 `pkg/metrics/lib.rs` 以 `pub mod br` 暴露该模块；包级中枢 `pkg/metrics/metrics.rs::InitMetrics` 在其他子系统之后调用 `br::InitBRMetrics`，随后 `RegisterMetrics` 将其中 13 个 collector 注册到默认 Prometheus registry。

它不执行备份、恢复或 PiTR，也不负责主动采样；它只建立可供业务代码调用的 collector。当前仓库中这些 collector 的真实业务消费证据主要位于 Go BR 实现（如 `br/pkg/restore/log_client/client.go`、`br/pkg/restore/snap_client/import.go`），Rust 生产代码尚未直接读写本模块的静态指标。因而本文件是已移植的指标边界，而不是 Rust BR 主流程实现。

crate 边界由 `pkg/metrics/Cargo.toml` 确认：本文件直接使用 crate 内的 Go 兼容封装，并最终依赖 `prometheus = "0.14"`；没有 BR crate 依赖，也没有 feature 条件控制。

## 核心职责

1. 声明 17 个公开、包级、延迟赋值的 Prometheus collector，保持 Go `pkg/metrics/br.go` 的变量名和分组。
2. 在 `InitBRMetrics()` 中一次构造全部 collector，固定 metric namespace、subsystem、name、help、桶边界和标签名。
3. 借助 `crate::metrics::PACKAGE_INIT_LOCK` 串行化对 `static mut` 的整组写入，降低并发初始化时的数据竞争风险。
4. 为 `pkg/metrics/metrics.rs` 的统一初始化与注册流程提供 BR 子系统入口。

本文件只创建 collector，不产生观测值、不选择标签值，也不在构造失败时返回 `Result`。指标是否有数据取决于调用方是否调用 `observe`、`inc`、`set` 或带标签的 child collector。

## 主要符号

唯一函数是 `pub fn InitBRMetrics()`，其公开 API 语义是“重新构造并替换本模块全部 17 个 `Option<collector>`”。通常应经 `metrics.rs::InitMetrics()` 调用；测试 `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata` 也会直接调用它作为元数据构造冒烟检查。

公开静态量按观测阶段分为四组：

- 恢复入口：`RestoreImportFileSeconds`、`RestoreUploadSSTForPiTRSeconds`、`RestoreUploadSSTMetaForPiTRSeconds` 三个 `Histogram`，以及 `RestoreTableCreatedCount` 计数器。
- Meta KV 批处理：`MetaKVBatchFiles`、`MetaKVBatchFilteredKeys`、`MetaKVBatchKeys`、`MetaKVBatchSize` 四个带 `cf` 标签的 `HistogramVec`。
- KV apply 批处理：`KVApplyBatchDuration`、`KVApplyBatchFiles`、`KVApplyBatchRegions`、`KVApplyBatchSize`、`KVApplyRegionFiles` 五个 `Histogram`。
- 事件与内存：带 `event` 标签的 `KVApplyTasksEvents` 和 `KVApplyRunOverRegionsEvents`，带 `status` 标签的 `KVLogFileEmittedMemory`，以及 `KVSplitHelperMemUsage` gauge。

所有静态量初始均为 `None`。前 13 个 collector 在 `metrics.rs::RegisterMetrics` 的 `register_options!` 列表中；最后四个事件/内存 collector 不在 Rust 注册列表中。Go 的 `RegisterMetrics` 同样只逐项注册前 13 个，因此不能仅凭定义推断后四项会出现在默认 registry。

## 执行流程

典型 Rust 初始化链如下：

1. 上层调用 `pkg/metrics/metrics.rs::InitMetrics()`。
2. `INIT_METRICS_ONCE` 保证整个包级初始化闭包只执行一次。
3. 闭包按固定顺序初始化各子系统，最后调用 `br::InitBRMetrics()`。
4. `InitBRMetrics` 获取 `PACKAGE_INIT_LOCK`；锁中毒会由 `expect("metrics init lock poisoned")` 触发 panic。
5. 在一个 `unsafe` 块内依次构造并写入 17 个全局 `Option`。前三组主要经 `metricscommon::NewCounter/NewHistogram/NewHistogramVec` 创建；最后四项直接经兼容层 `prometheus::NewCounterVec/NewGauge` 创建。
6. 上层再调用 `metrics.rs::RegisterMetrics()` 时，`register_option` 要求前 13 个静态量已为 `Some`，克隆 collector 后注册到默认 registry；未初始化会以 `InitMetrics must run before RegisterMetrics` panic。

桶配置体现各数据量级：文件导入耗时从 `0.01` 秒开始、倍率 `4`、共 14 桶；PiTR 元数据上传从 `0.01` 秒开始、倍率 `2`、共 14 桶；Meta KV 文件数用 `1 × 2^n` 的 12 桶，键数用 18 桶，字节数从 256 字节开始用 20 桶；KV apply 耗时从 1ms 开始用 21 桶，大小从 1KiB 开始用 21 桶。

## 数据与状态

状态由 17 个 `pub static mut Option<...>` 保存。`None` 表示尚未执行本模块初始化，`Some` 持有可克隆的 Prometheus collector handle。`InitBRMetrics` 会替换旧 handle，而不是清零既有 handle 的数据；包级 `InitMetrics` 的 `Once` 防止正常入口重复替换，但直接调用公开的 `InitBRMetrics` 不具备幂等保护。

指标身份由初始化时的元数据决定：除 `RestoreTableCreatedCount` 使用 namespace `BR`、name `table_created` 且无 subsystem 外，其余指标均使用 namespace `tidb` 和 subsystem `br`。这会分别形成 `BR_table_created` 与 `tidb_br_*` 系列。

向量标签是不变量：四个 Meta KV 向量只有 `cf`；apply 任务和跨 Region 事件向量只有 `event`；日志文件元数据内存向量只有 `status`。标签的合法值并未由类型或本文件校验，而是调用方约定。Go 对照说明给出的值包括任务事件 `skipped/submitted/started/finished`、内存阶段 `0-loaded/1-split/2-applied`、跨 Region 事件 `request-region/retry-region/retry-range/region-success`。

## 依赖与调用关系

上游 Rust 调用边经 RustCodeGraph 确认为：

- `pkg/metrics/metrics.rs::InitMetrics -> pkg/metrics/br.rs::InitBRMetrics`；
- `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata -> InitBRMetrics`；
- `pkg/metrics/metrics.rs::RegisterMetrics` 读取并注册前 13 个静态量。

下游依赖包括：

- `crate::bindinfo::compat_prometheus`：提供 Go 风格的 `CounterOpts`、`HistogramOpts`、构造函数以及 `MetricCompat`/`ObserverCompat` 等兼容 trait；
- `crate::bindinfo::compat_metricscommon`：包装 collector 构造，并承接 metrics common 的一致配置；
- `crate::metrics::PACKAGE_INIT_LOCK`：保护包内 `static mut` 初始化；
- 外部 `prometheus` crate：由 `pkg/metrics/Cargo.toml` 固定为 0.14。

RustCodeGraph 与全仓 `rg` 未发现 Rust 生产代码消费这些静态量；出现的 Rust BR 引用 `br/pkg/restore/log_client/import.rs` 指向该目录自己的 `metrics` 桩模块，而不是 `astersql_metrics::br`。Go 侧则存在完整消费链，例如文件导入耗时在 `br/pkg/restore/snap_client/import.go` 观测，Meta KV 批次在 `br/pkg/restore/log_client/client.go` 观测，PiTR 上传在 `pitr_collector.go` 观测，表创建计数用于 `snap_client/client.go` 的速率跟踪。

## 错误处理与边界

本函数没有返回值和可恢复错误通道。兼容构造器把非法 Prometheus 描述符视为编程错误；本模块给出的静态名称、桶和标签属于编译期附近的固定元数据。包级互斥锁若中毒会 panic，注册前未初始化也会在 `metrics.rs::register_option` 中 panic，而 registry 的重复名称等注册错误由 `RegisterMetrics() -> Result<(), prometheus::Error>` 向上传播。

本文件不限制传入的标签值，也不验证观测值的单位或符号；调用者必须遵循 help 与 Go 约定。尤其 `KVApplyBatchSize` 的 help 文本写成 “number of KV files”，但变量语义、桶范围与 Go 注释均表明其记录总字节数；这是忠实保留的现有元数据差异，不应在本文件说明中擅自改写为已修复。

当前边界还包括：事件/内存四项只被初始化，未被 Rust 默认注册流程引用；Rust BR 生产代码也未连接本模块。因此“collector 已定义”不等于“Rust 服务已暴露并产生数据”。

## 并发与资源生命周期

Prometheus collector handle 本身可克隆并可被并发更新，但其全局容器采用 `static mut Option`，访问需要 `unsafe` 且必须遵守初始化先于读取的顺序。`PACKAGE_INIT_LOCK` 只覆盖 `InitBRMetrics` 执行期间的替换，不能保护锁外直接读取；正常生命周期依赖 `InitMetrics` 的 `Once`：进程内构造一次，注册后存活至进程结束。

若绕过 `InitMetrics` 并并发直接调用 `InitBRMetrics` 与读取者，即使写入者彼此被锁串行化，锁外读写仍可能违反 Rust 的别名/同步要求。新增消费者应优先复用包级初始化入口，避免再次暴露裸 `static mut` 读取；若重构存储，应考虑 `LazyLock`/`OnceLock` 等安全一次初始化结构，并同步处理测试对重初始化的需求。

本文件不启动线程、异步任务、channel、事务或外部 I/O，也不拥有需要显式释放的资源。registry 保存的是 collector 克隆，共享底层计量状态；直接替换全局 handle 可能让旧注册项与新 handle 脱离，因此生产路径不应重复初始化。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/metrics/br.go`：17 个变量名、collector 类型、namespace/subsystem/name/help、桶边界、标签名以及 `InitBRMetrics` 的构造顺序均保持 Go 形状。Rust 以 `Option<T>` 表达 Go 中初始化前的 nil/零值，并以兼容层保留 Go 风格字段名和构造 API。

主要实现差异有三点：

1. Rust 增加 `PACKAGE_INIT_LOCK` 和包级 `INIT_METRICS_ONCE` 配合，Go 依赖 package `init` 的单次串行语义。
2. Rust 对可变全局状态使用 `unsafe static mut Option<T>`；Go 使用包级 collector 变量。
3. Go BR 业务代码已经大量调用这些指标；当前 Rust 生产消费者尚未接到 `astersql_metrics::br`，因此功能覆盖只到指标元数据初始化/前 13 项注册，而非完整观测行为。

Go 测试未发现专门针对 `pkg/metrics/br.go` 的独立测试。Rust 同样没有 `br_test.rs`；最近的直接测试是 `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata`，它只证明初始化不拒绝这些元数据，不验证所有名称、桶、标签、注册或业务采样行为。

## 扩展指南

新增或修改 BR 指标时，应同时核对并修改：

1. `pkg/metrics/br.rs` 的静态声明与 `InitBRMetrics` 构造项；若是 Go 对齐工作，同步核对 `pkg/metrics/br.go` 的对应增量。
2. 若指标需要默认暴露，将其加入 `pkg/metrics/metrics.rs::RegisterMetrics` 的 `register_options!` 列表，并确认是否也应与 Go `RegisterMetrics` 保持一致。
3. 在实际 Rust BR 业务节点接入 `observe/inc/set/with_label_values`；不要仅创建 collector 就声称指标可用。
4. 新增独立 Rust 测试文件，而非把测试放入 `br.rs`。建议覆盖完整 metric family 名、help、桶边界、标签维度、初始化后可注册与重复初始化策略，并在必要时验证真实 BR 调用点。

兼容风险集中在 metric family 名和标签集合：更改 namespace、subsystem、name 或标签会破坏 dashboard、告警和查询；桶变化会改变聚合精度及时间序列数量；新增无界标签值会放大基数。并发风险集中在公开 `static mut` 的重初始化和锁外访问。性能风险主要来自过细桶、过多标签组合以及热路径上的频繁观测。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/metrics` 确认 `br.rs`、`br.go`、模块入口及测试布局；`node --file pkg/metrics/br.rs --offset 1 --limit 240` 读取全部 225 行并报告该文件由 `metrics.rs` 与 `bindinfo_1_aster_unit_test.rs` 使用；`query/callers` 与 `explore` 确认 `InitBRMetrics` 的上游入口。
- 源文件：`pkg/metrics/br.rs`，核对 17 个静态 collector、唯一初始化函数、全部元数据、桶和标签。
- crate/入口：`pkg/metrics/Cargo.toml`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`，核对 crate 依赖、模块可见性、一次初始化、注册集合和错误边界。
- Go 对照：`pkg/metrics/br.go`、`pkg/metrics/metrics.go`，核对变量、初始化和注册语义。
- 业务调用：`br/pkg/restore/snap_client/import.go`、`br/pkg/restore/snap_client/pitr_collector.go`、`br/pkg/restore/snap_client/client.go`、`br/pkg/restore/log_client/client.go`、`br/pkg/restore/log_client/import.go`、`br/pkg/task/stream.go`、`br/pkg/task/restore.go`。
- 测试：`pkg/metrics/bindinfo_1_aster_unit_test.rs` 提供直接 Rust 初始化冒烟证据；全仓搜索未发现同名独立 Rust/Go 测试或 Rust 生产消费者。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核“定位、运行、扩展”三类问题均有源码依据。
