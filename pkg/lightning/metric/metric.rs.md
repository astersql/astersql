# `pkg/lightning/metric/metric.rs`

## 文件定位

本文件是 `astersql-lightning-metric` crate 的主体实现；crate 入口 `pkg/lightning/metric/lib.rs` 声明 `metric` 模块并将其全部公开项重导出。`pkg/lightning/metric/Cargo.toml` 表明该 crate 只直接依赖 `astersql-util-promutil` 和 `prometheus`，并以 `pkg/lightning/metric` Go 包为迁移来源。

它位于 Lightning/IMPORT INTO 的观测边界：一方面提供 Lightning 命名空间下的完整 `Metrics`，另一方面提供可指定 namespace、subsystem 与常量标签的 `Common`，供 `pkg/metrics/import.rs`、`pkg/metrics/ddl.rs` 等非 Lightning 路径复用。该文件只定义和管理指标句柄，不负责启动 HTTP exporter、抓取端点或驱动导入任务。

## 核心职责

1. 用字符串常量统一表、引擎、chunk、恢复阶段、SST 操作和块类型的 Prometheus 标签值，例如 `TABLE_STATE_COMPLETED`、`STATE_TOTAL_RESTORE`、`SST_PROCESS_INGEST`。
2. `new_common` 创建导入链公共的 8 个 collector；`new_metrics` 再创建 Lightning 专用的 14 个 collector，并通过 `Arc<Common>` 组合为共 22 个 collector 的完整集合。
3. `Common::{register_to, unregister_from}` 与 `Metrics::{register_to, unregister_from}` 成组维护 collector 的注册生命周期。
4. `Metrics::{record_table_count, record_engine_count}` 将“是否存在错误”规范化为 `success`/`failure` 标签并递增计数。
5. `read_counter`、`read_histogram`、`read_all_counters`、`read_histogram_sum` 提供当前样本读取和聚合能力；其中 `read_all_counters` 明确保留 Go 的“任一给定标签匹配即纳入”语义。
6. `MetricContext` 以 Rust 所有权安全的方式携带完整 `Metrics` 或仅携带 `Common`，供 worker 等下游选择性观测。

## 主要符号

- `Labels = HashMap<String, String>`：对应 Go 的 `prometheus.Labels`，用于 collector 的常量标签和读取过滤条件。
- `opts(...) -> prometheus::Opts`、`histogram_opts(...) -> prometheus::HistogramOpts`：内部构造器，统一设置 namespace、subsystem、名称、帮助文本、常量标签；后者还设置 buckets。
- `Common`：包含 `chunk_counter`、`bytes_counter`、`rows_counter`，三类行/编码/投递耗时直方图，以及按 `kind` 区分的投递字节数和 KV 数直方图，共 8 个 collector。
- `new_common(factory, namespace, subsystem, const_labels) -> Common`：经 `promutil::Factory` 创建 `Common`。可配置命名空间/子系统和任务级常量标签，是 Lightning、IMPORT INTO、DDL 复用的关键入口。
- `Metrics`：包含 importer engine、空闲 worker、KV encoder、表和引擎状态计数，导入/解析/worker/投递/checksum/SST 耗时，以及本地存储占用和阶段进度；`common: Arc<Common>` 嵌入公共集合。
- `new_metrics(factory) -> Metrics`：固定使用 `LIGHTNING_NAMESPACE = "lightning"`、空 subsystem、空常量标签创建完整 Lightning 指标。
- `Common::register_to/unregister_from`、`Metrics::register_to/unregister_from`：注册或注销整组 collector；完整集合总是先处理 `Common` 再处理专用 collector。
- `Metrics::record_table_count`、`Metrics::record_engine_count`：依据 `Option<&dyn Error>` 是否为 `Some` 选择 `TABLE_RESULT_FAILURE` 或 `TABLE_RESULT_SUCCESS`。
- `read_counter`：通过 Prometheus counter 的 `get()` 返回累计值。
- `read_histogram`：从 `collect()` 结果中取第一个 family 的第一个 metric；无样本快照时返回 `None`。
- `metric_has_label`、`read_all_counters`：遍历 `CounterVec` 的全部样本，只要样本的任一 label 键值命中过滤 `Labels` 就累加 counter 值。
- `read_histogram_sum`：读取 histogram 的 `sample_sum`；快照或 histogram 字段不存在时返回 `NaN`。
- `MetricContext`、`with_metric`、`with_common_metric`、`from_context`、`get_common_metric`：分别表示可选指标上下文、两种注入方式和两种读取方式。

本文件没有 trait、枚举、条件编译项或异步函数；公开 API 主要是上述常量、两个结构体、构造/读取函数和结构体方法，`opts`、`histogram_opts`、`metric_has_label` 为内部实现。

## 执行流程

完整 Lightning 路径的典型流程如下：

1. 上层以某个 `promutil::Factory` 调用 `new_metrics`。
2. `new_metrics` 逐个创建 14 个 Lightning collector，并调用 `new_common` 创建 8 个公共 collector；各 collector 的名称、label 维度和指数 buckets 在构造时固定。
3. 若需要向 Prometheus 暴露，上层调用 `Metrics::register_to`；它先调用 `Common::register_to`，随后把 14 个专用 collector 一次传给 `Registry::MustRegister`。
4. 调用方持有 cloneable 的 collector 句柄并在业务节点执行 `inc`、`set` 或 `observe`。例如 `pkg/lightning/worker/worker.rs::NewPool/Apply/Recycle` 从 `MetricContext` 取得完整指标，更新 `idle_workers_gauge` 和 `apply_worker_seconds_histogram`。
5. 业务或测试可用读取辅助函数取得累计值/快照；结束时调用 `Metrics::unregister_from`，按与注册相同的分组逐个注销。

复用 `Common` 的 IMPORT INTO 路径不同：`pkg/metrics/import.rs::GetRegisteredImportMetrics` 合并全局与 task 常量标签后调用 `new_common(factory, "tidb", "import", labels)`，立即注册到默认 registry；`pkg/dxf/importinto/metrics.rs::TaskMetricManager` 按 task ID 引用计数持有该集合，并在引用归零时经 `UnregisterImportMetrics` 注销。`pkg/dxf/importinto/scheduler.rs::nextImportSubtasksBatch` 则通过 `bytes_counter` 和 `STATE_TOTAL_RESTORE` 写入计划总文件大小。

上下文流程中，`with_metric` 会同时写入 `metrics` 与其共享的 `common`；`with_common_metric` 只写 `common`。因此需要完整 Lightning collector 的调用方使用 `from_context`，只依赖公共导入指标的调用方可使用 `get_common_metric`。

## 数据与状态

- 所有实际指标状态保存在 `prometheus` collector 内，`Common`/`Metrics` 只持有可克隆句柄。clone 不复制计数值，而是共享底层指标状态；`Metrics.common` 使用 `Arc` 明确共享所有权。
- Counter 只累加；Gauge 可被设置为当前值；Histogram 累积样本数、总和与 buckets。文件本身不重置指标。
- `new_common` 的可变维度为：chunks/bytes 的 `state`，rows 的 `state, table`，两类 block histogram 的 `kind`。`new_metrics` 的可变维度包括 engine `type`、worker `name`、表/引擎 `state, result`、SST `kind`、存储 `medium`、进度 `phase`。
- buckets 是兼容契约的一部分：行读取从 `0.001` 秒、因子约 `3.1623`、7 桶；多类耗时从 `0.001` 秒、同因子、10 桶；投递字节从 `512`、因子 2、10 桶；KV 数从 `1`、因子 2、10 桶；导入耗时从 `0.125` 秒、因子 2、6 桶；checksum/SST 从 `1` 秒、因子约 `2.2679`、10 桶。
- `MetricContext` 的初始状态由 `background()`/`Default` 产生，两个 `Option` 均为 `None`。后续注入按值返回新上下文，不使用全局可变上下文。
- `read_all_counters` 的过滤不是“所有标签均满足”，而是“至少一个键值对满足”；空过滤映射不会匹配任何样本，结果为 `0.0`。

## 依赖与调用关系

下游依赖：

- `astersql_util_promutil`（在 crate 中重导出为 `promutil`）提供可替换的 `Factory`、`Registry` 和默认实现，使指标构造/注册可在测试 registry 与进程默认 registry 间复用。
- `prometheus` 提供 `CounterVec`、`GaugeVec`、`Histogram(Vec)`、`Opts`、`HistogramOpts`、指数 buckets、collector `collect()` 以及 protobuf `proto::Metric`。
- 标准库的 `HashMap`、`Arc` 和 `Error` 分别承载标签、共享所有权与错误存在性判断。

已验证的上游关系：

- `pkg/lightning/metric/lib.rs` 公开重导出本文件全部 API。
- `pkg/lightning/worker/worker.rs` 通过 `from_context` 取得 `Metrics`，使用空闲 worker gauge 与申请耗时 histogram。
- `pkg/metrics/import.rs` 和 `pkg/metrics/ddl.rs` 通过 `new_common` 建立带不同 namespace/subsystem 的公共指标，并成组注册/注销。
- `pkg/dxf/importinto/metrics.rs` 以 task ID 为常量标签管理 `Common` 的引用计数生命周期；`pkg/dxf/importinto/scheduler.rs` 持有 `Arc<Common>` 并记录待恢复总字节数。
- `pkg/metrics/lib.rs::metric` 再导出本文件 API，使 TiDB 指标模块可以共享同一实现。

RustCodeGraph 将 `metric.rs` 标为被 19 个文件使用，并索引出 24 个符号；对 `new_metrics`、`register_to` 等常见名称执行 callers/callees 未产生可消歧的静态调用边，因此上述调用点又以精确源码搜索和相邻文件读取核验，没有将模糊图结果当作事实。

## 错误处理与边界

- `new_common`/`new_metrics` 内的 `prometheus::exponential_buckets(...).unwrap()` 依赖当前硬编码参数合法；参数若被扩展为动态值，必须改为显式传播或验证错误，不能保留无条件 `unwrap`。
- `register_to` 使用 `MustRegister`，重复 descriptor、标签冲突等注册失败会按 registry 实现触发 panic；该 API 适合配置错误即不可继续的初始化路径。`unregister_from` 忽略每个 `Unregister` 的布尔返回值，因此重复注销是无报告的 best effort。
- `record_*` 不检查错误内容，只判断 `Option` 是否存在；调用方传入任意错误都记为 `failure`，错误本身不被保存或上报。
- `read_counter` 直接使用 `get()`，不像 Go 的 `Write` 路径那样有可返回的编码错误。`read_histogram` 只取第一个采集结果；它适用于单一 `Histogram`，不应被推广为多 family 聚合工具。
- `read_histogram_sum` 在缺少快照或 histogram 字段时返回 `NaN`，调用方需避免把它当作零。
- `read_all_counters` 对缺失 counter protobuf 字段按 `0.0` 处理，并保持 Go 的“任意标签匹配”行为。若调用者误以为是 AND 过滤，会发生过度聚合。
- `with_metric` 会用传入完整指标覆盖上下文中已有的 `metrics` 和 `common`；`with_common_metric` 只覆盖 `common`，可形成“完整 Metrics 与单独 Common 不属于同一集合”的上下文，当前实现不强制二者一致。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。并发安全主要由 `prometheus` collector 的内部实现和 `Arc` 提供；多个 worker 可以克隆同一 collector/`Arc<Metrics>` 并并发更新。

资源生命周期是显式的“创建—注册—观测—注销”：构造函数不自动注册，`register_to` 才把 collector 交给 registry；注销不销毁仍被 `Metrics`、`Common` 或其他 clone 持有的句柄，只移除 registry 对其的公开采集。调用方必须保证同一组 descriptor 不被重复注册，并在任务级动态指标结束时注销，避免默认 registry 长期积累。`pkg/dxf/importinto/metrics.rs::TaskMetricManager` 用 `Mutex<HashMap<task_id, TaskMetrics>>` 和引用计数落实这一外围生命周期；锁并不属于本文件。

`MetricContext` 按值克隆只增加 `Arc` 引用计数，不复制 collector。`Metrics` 与 `Common` 的 `Clone` 同样共享 Prometheus 句柄，因此克隆后的注册/注销针对相同 collector descriptor，不能把 clone 当成独立指标实例重复注册。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/lightning/metric/metric.go`：标签常量、`Common`/`Metrics` 字段、collector 名称、help、label 维度、buckets、注册顺序、`Record*Count` 的 success/failure 选择以及上下文的完整/公共两级访问均保持一致。`pkg/lightning/metric/metric_test.go` 的计数、直方图、22 个 collector 注册注销和 context 用例在 `metric_test.rs` 中有对应覆盖；`migration_aster_unit_test.rs` 额外固定了标签聚合语义。

主要语言适配差异如下：

- Go 返回 `*Common`/`*Metrics` 并匿名嵌入 `*Common`；Rust 返回拥有值的 `Common`/`Metrics`，以 `Arc<Common>` 字段表达共享，字段访问显式经过 `common`。
- Go 的 `context.Context` 用私有 key 保存动态类型；Rust 用专用 `MetricContext` 的两个 `Option<Arc<_>>` 表达，类型更静态，但不是通用请求 context，也没有父链查找。
- Go 的 `ReadCounter`、`ReadHistogram`、`ReadHistogramSum` 通过 `Write` 可能返回 `NaN`/`nil`；Rust counter 用 `get()` 无写出错误，histogram 用 `collect()`，仅缺失快照/字段时返回 `None` 或 `NaN`。
- Go `ReadAllCounters` 启动 goroutine、经 channel 收集通用 `MetricVec`，写出失败返回 `NaN`；Rust 同步遍历具体 `CounterVec::collect()`，无通道，也不接受其他 MetricVec 类型，但保留“任意标签命中”的聚合结果。
- Go context key 借助类型和值隔离；Rust 不暴露 key，而由结构字段保证隔离。

这些差异是当前实现事实，不代表 Rust 已覆盖 Go `prometheus.MetricVec` 的全部泛型能力。

## 扩展指南

- 新增指标时，应在 `Common` 或 `Metrics` 中选择正确归属，同时修改对应构造函数、`register_to` 和 `unregister_from`；若属于完整 `Metrics`，还需重新核对当前 22 个 collector 的测试期望。漏掉注销会造成动态任务指标泄漏，漏掉注册会让更新存在但无法被抓取。
- 名称、namespace、subsystem、help、label 顺序和 bucket 边界均是监控兼容面。修改前应对照 `metric.go`、告警/仪表盘调用者及 `pkg/metrics/import.rs`、`pkg/metrics/ddl.rs` 的复用途径，避免时间序列改名或基数膨胀。
- 新增 label 时必须同步所有 `with_label_values` 调用；尤其注意 `rows_counter` 的顺序是 `state, table`，表名等高基数字段会直接影响 Prometheus 内存与查询成本。
- 修改 `read_all_counters` 时不要未经迁移决策把 OR 语义改成 AND；应同步更新 `migration_aster_unit_test.rs::read_all_counters_preserves_go_any_label_match_semantics` 并说明与 Go 的偏差。
- 修改上下文行为时，应同步独立测试 `metric_test.rs::test_context`、`migration_aster_unit_test.rs::metric_context_exposes_full_and_common_metrics_like_go_context`，并检查 `pkg/lightning/worker/worker.rs` 的完整指标读取。
- 测试逻辑应继续放在同目录独立的 `metric_test.rs` 或迁移测试文件中，不要内嵌回本生产文件；Rust 与 Go 测试应尽可能保持同一行为断言。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/lightning/metric` 列出 `metric.rs`、crate 入口、Go 对照和独立测试；`node --file pkg/lightning/metric/metric.rs` 完整读取 552 行并报告 24 个符号、19 个使用文件；`query new_metrics` 与 `query read_all_counters` 确认定义和迁移测试符号。对关键符号运行 `callers`/`callees` 没有返回可用边，故没有据此虚构调用链。
- 生产实现：`pkg/lightning/metric/metric.rs`；crate 边界：`pkg/lightning/metric/Cargo.toml`、`pkg/lightning/metric/lib.rs`。
- Go 对照：`pkg/lightning/metric/metric.go`；Go 测试：`pkg/lightning/metric/metric_test.go`。
- Rust 独立测试：`pkg/lightning/metric/metric_test.rs`、`pkg/lightning/metric/migration_aster_unit_test.rs`；覆盖读取、success/failure、8/22 collector 注册注销、OR 标签聚合和两级上下文。
- 直接调用证据：`pkg/lightning/worker/worker.rs`、`pkg/metrics/import.rs`、`pkg/metrics/ddl.rs`、`pkg/metrics/lib.rs`、`pkg/dxf/importinto/metrics.rs`、`pkg/dxf/importinto/scheduler.rs`。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；交付验证只检查文档存在且恰含规定的 11 个二级章节，并人工复核符号、调用点和 Go/Rust 差异均可回溯到上述文件。
