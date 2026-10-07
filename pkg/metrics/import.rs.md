# `pkg/metrics/import.rs`

## 文件定位

[`pkg/metrics/import.rs`](import.rs) 属于 `astersql-metrics` crate（见 [`pkg/metrics/Cargo.toml`](Cargo.toml)），由 [`pkg/metrics/lib.rs`](lib.rs) 通过公开模块 `pub mod import` 暴露。它位于 SQL 导入路径与 Lightning 通用指标实现之间：本文件不采集业务事件，而是为调用方创建一组 `astersql_lightning_metric::metric::Common` 指标，将其注册到进程级 Prometheus 默认注册表，并提供配对的注销入口。

当前 Rust 仓库中没有发现 `GetRegisteredImportMetrics` 或 `UnregisterImportMetrics` 的目标文件外调用，因此该门面已经公开但尚无可确认的 Rust 业务接线。Go 对照实现则在 `pkg/executor/importer/chunk_process_testkit_test.go` 中被测试辅助流程调用。

## 核心职责

- 用常量 `importMetricSubsystem` 固定指标 subsystem 为 `"import"`，与命名空间常量 `crate::TiDB`（值为 `"tidb"`）组合出 `tidb_import_*` 指标名前缀。
- `GetRegisteredImportMetrics` 先调用 `metricscommon::GetMergedConstLabels` 合并调用方标签与 metrics 包全局常量标签，再通过调用方提供的 `promutil::Factory` 构造 `metric::Common`。
- 构造完成后，将 `Common` 内的 8 个 collector 注册到 `prometheus::DefaultRegisterer`，并返回同一组指标的句柄供导入流程写入。
- `UnregisterImportMetrics` 将该组 collector 从同一个默认注册表逐一移除。注册和注销是显式生命周期操作，本文件没有自动清理或 `Drop` 保护。

## 主要符号

- `const importMetricSubsystem: &str = "import"`：私有模块常量，只参与 `metric::new_common` 的名称构造。
- `pub fn GetRegisteredImportMetrics(factory: Box<dyn promutil::Factory>, constLabels: prometheus::Labels) -> metric::Common`：公开构造/注册入口。`Factory` 使用 trait object，允许默认工厂或测试替身决定具体 collector 的创建方式；`Labels` 是 `HashMap<String, String>` 兼容别名；返回值拥有 `Common` 句柄。
- `pub fn UnregisterImportMetrics(metrics: &mut metric::Common)`：公开注销入口。它不取得对象所有权，调用后 `Common` 仍然存在，但其 collector 不再由默认注册表导出。参数采用可变借用，尽管底层 `Common::unregister_from` 只要求共享借用，这会在调用期间阻止其他借用。
- 文件顶部以 `_` 引入 `CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat`。本文件没有直接调用这些 trait 方法；它们是迁移兼容导入，不产生独立运行时状态或控制流。

## 执行流程

1. 调用方准备一个 `Box<dyn promutil::Factory>` 和本次导入任务的常量标签。
2. `GetRegisteredImportMetrics` 调用 `metricscommon::GetMergedConstLabels(constLabels)`。依据 `pkg/metrics/common/wrapper.rs`，函数读取包级标签快照，并用包级同名键覆盖调用方值。
3. 函数调用 `metric::new_common(factory.as_ref(), TiDB, importMetricSubsystem, mergedCstLabels)`。`pkg/lightning/metric/metric.rs` 据此创建 chunk、bytes、rows 三组计数器，以及 row read、row encode、block deliver 耗时、block bytes、block KV pairs 五组直方图/向量，共 8 个 collector。
4. `metrics.register_to(&prometheus::DefaultRegisterer)` 通过 `Common::register_to` 将这 8 个 collector 的 clone 传给默认注册表；成功后返回 `Common`。
5. 调用方在导入上下文中使用返回句柄记录进度或耗时。Go 侧示例是 `pkg/executor/importer/chunk_process_testkit_test.go`：注册后通过 `metric.WithCommonMetric` 放入 context，并以 `defer` 注销。
6. 生命周期结束时，调用方把同一 `Common` 传给 `UnregisterImportMetrics`；后者调用 `Common::unregister_from`，按相同的 8 个 collector 逐项注销。

## 数据与状态

本文件自身没有可变静态变量。持久状态分布在三个边界：调用方传入的标签映射、`pkg/metrics/common/wrapper.rs` 中由 `OnceLock<RwLock<Labels>>` 保存的包级常量标签，以及 Prometheus 全局默认注册表。

`GetMergedConstLabels` 返回标签快照，不把调用方的映射保存在本模块中；全局标签在合并后发生的变化不会追溯修改已创建的 collector。返回的 `metric::Common` 拥有 3 个 `CounterVec`、3 个普通 `Histogram` 和 2 个 `HistogramVec`。动态标签分别包括 `state`、`state/table` 或 `kind`，具体名称、帮助文本和直方图桶由 `pkg/lightning/metric/metric.rs::new_common` 定义，本文件只确定 namespace、subsystem 和常量标签。

## 依赖与调用关系

上游方面，`pkg/metrics/lib.rs` 公开本模块；全仓 Rust 引用搜索未发现两个公开函数的直接调用者。Go 同路径实现的已验证调用者是 `pkg/executor/importer/chunk_process_testkit_test.go`，它说明了预期的“创建—放入导入 context—延迟注销”使用形态，但不能作为 Rust 生产链已经接线的证据。

下游方面：

- `crate::metricscommon` 重导出 `astersql-metrics-common`，本文件调用其 `GetMergedConstLabels`。
- `crate::promutil` 重导出 `astersql-util-promutil`，提供 `Factory` trait；直接依赖在 `pkg/metrics/Cargo.toml` 中声明为 `astersql-util-promutil`。
- `crate::metric` 重导出 `astersql-lightning-metric::metric`，提供 `new_common`、`Common::register_to` 与 `Common::unregister_from`；直接依赖在 Cargo manifest 中声明为 `astersql-lightning-metric`。
- `crate::bindinfo::compat_prometheus` 提供 `Labels` 和 `DefaultRegisterer` 适配器；后者把操作委托给 `prometheus::default_registry()`。`prometheus = "0.14"` 是该 crate 的直接外部依赖。

## 错误处理与边界

两个公开函数都不返回 `Result`。构造阶段的无效指标选项由默认 `Factory` 的 `expect` 转为 panic；当前 `new_common` 的固定名称、标签和桶是其有效性边界。注册阶段使用 `DefaultRegistry::MustRegister`，任何 collector 注册错误（典型情况是默认注册表已有同名且描述冲突/重复的 collector）都会以 `"metric registration failed"` panic，而不是回传错误。该适配器逐项注册，因此若后续 collector 才失败，先前项可能已经注册；本文件没有回滚逻辑。

注销阶段忽略 `Registry::Unregister` 的布尔结果，所以传错实例、重复注销或 collector 不在注册表时均不会报告错误。调用者必须保证注册与注销成对并使用同一 `Common`。本文件不校验标签键、标签基数或调用时序；高基数常量/动态标签造成的内存与抓取成本由调用链共同承担。

## 并发与资源生命周期

包级标签通过 `RwLock` 读取并克隆，锁中毒时恢复内部值，因此合并操作具有线程同步且不会长期持锁。`promutil::Registry` 要求 `Send + Sync`，默认实现委托 Prometheus 全局注册表；本文件没有另建线程、异步任务、通道或事务。

生命周期不是 RAII：创建函数在返回前即完成全局注册，`Common` 离开作用域不会自动注销，必须显式调用 `UnregisterImportMetrics`。相反，注销只移除注册表中的 collector，并不销毁调用者仍持有的 `Common`。并发或重复注册同名指标可能触发 panic；并发记录样本的线程安全性由 `prometheus` collector 提供，但调用方仍需协调注册/注销与写入的业务时序。

## 与 Go 版本的对应关系

`pkg/metrics/import.go` 具有同名常量和两个同名函数，流程逐步对应：合并常量标签、`metric.NewCommon`/`metric::new_common` 构造、向 `prometheus.DefaultRegisterer` 注册、返回句柄，以及配对注销。Rust 用 `Box<dyn Factory>` 表达 Go 接口值，用拥有所有权的 `metric::Common` 代替 Go 的 `*metric.Common`，注销入口则用 `&mut metric::Common` 代替指针。

可确认的语义差异主要来自适配层表达方式：Rust 默认注册器的 `MustRegister` 逐个调用 `prometheus::default_registry().register(...).expect(...)`，注销返回值被丢弃；Go 同样使用 MustRegister 风格的强制注册和无返回值的包装注销。`pkg/executor/importer/chunk_process_testkit_test.go` 验证了 Go 侧调用生命周期；Rust 没有同名门面测试或调用者，因此不能声称该函数已在 Rust importer 主链中运行。

## 扩展指南

- 若只新增或调整导入通用指标，应优先修改真实所有者 `pkg/lightning/metric/metric.rs::Common` 与 `new_common`，同步更新其独立测试 `pkg/lightning/metric/metric_test.rs` 或 `pkg/lightning/metric/migration_aster_unit_test.rs` 中的 collector 数量、标签和注册/注销断言；本门面通常无需复制指标定义。
- 若改变命名空间、subsystem 或标签合并规则，应修改本文件对应符号，并同步核对 `pkg/metrics/import.go`、`pkg/metrics/common/wrapper.rs` 及其独立测试 `pkg/metrics/common/wrapper_test.rs`，防止指标名称或全局标签优先级破坏监控兼容性。
- 若为 Rust importer 接线，调用点应持有返回的 `Common`，把它传入导入执行上下文，并确保所有退出路径显式注销；应在调用者同目录的独立 `*_test.rs` 文件中增加回归测试，不能把测试内嵌到本生产文件。
- 新增错误恢复或幂等需求时，应先决定是否继续保持 Go 的 MustRegister/panic 语义。改成可恢复注册、回滚部分注册或报告注销失败会改变公开 API 和兼容行为，需同时评估并发注册、重复任务、Prometheus 描述一致性及性能/基数风险。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 `pkg/metrics/import.rs`；`files --filter pkg/metrics` 显示该文件有 3 个符号；`query GetRegisteredImportMetrics --kind function` 返回 Go/Rust 两个同名定义。`explore/node` 未返回正文，`callers` 查询在当前索引上未及时完成，因此调用者结论使用全仓 `rg` 引用搜索补证，不宣称获得了图调用边。
- 目标与 crate 边界：`pkg/metrics/import.rs`、`pkg/metrics/lib.rs`、`pkg/metrics/Cargo.toml`。
- 下游实现：`pkg/lightning/metric/metric.rs`（`Common`、`new_common`、`register_to`、`unregister_from`）、`pkg/util/promutil/factory.rs`（`Factory`）、`pkg/util/promutil/registry.rs`（`Registry`）、`pkg/metrics/common/wrapper.rs`（标签快照与合并）、`pkg/metrics/bindinfo.rs`（默认注册器适配）。
- Go 对照与调用：`pkg/metrics/import.go`、`pkg/executor/importer/chunk_process_testkit_test.go`。
- 独立 Rust 测试：目标模块没有同名测试；底层 8 个 collector 的注册/注销行为由 `pkg/lightning/metric/metric_test.rs::test_metrics_register`、`test_metrics_unregister` 和 `pkg/lightning/metric/migration_aster_unit_test.rs::registration_and_unregistration_cover_all_go_metrics` 覆盖；标签覆盖规则由 `pkg/metrics/common/wrapper_test.rs::test_get_merged_const_labels` 覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工核对唯一新增产物、无源码/Cargo/总计划修改。
