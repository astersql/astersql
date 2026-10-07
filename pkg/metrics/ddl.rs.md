# `pkg/metrics/ddl.rs`

## 文件定位

本文件是 `astersql-metrics` crate 的 DDL 指标定义与生命周期管理模块，由 [`pkg/metrics/lib.rs`](lib.rs) 以 `pub mod ddl` 暴露。它不执行 DDL，也不决定 DDL 状态机；它负责构造供 DDL、schema syncer、Owner、回填和 Lightning ingest 路径使用的 Prometheus collector、固定标签与辅助注册表。

包级入口 [`metrics::InitMetrics`](metrics.rs) 调用 `ddl::InitDDLMetrics` 创建 collector，随后 [`metrics::RegisterMetrics`](metrics.rs) 将其中 14 个向量注册到默认 Prometheus registry。`pkg/metrics/Cargo.toml` 将本目录声明为 `astersql-metrics`，并直接依赖 `prometheus`、`astersql-metrics-common`、`astersql-util-promutil` 与 `astersql-lightning-metric`；这些依赖分别提供 collector 类型/默认 registry、Go 兼容构造器、工厂和 Lightning 通用指标集合。

## 核心职责

1. `InitDDLMetrics` 按 Go 版本的名称、帮助文本、标签顺序和指数桶配置创建 DDL collector，并从 `DDLWorkerHistogram` 预绑定七个常用阶段的 `Observer`。
2. `generateReorgLabel`、`GetBackfillTotalByTableID` 和 `GetBackfillProgressByTableID` 生成带 schema、table、column/index 信息的动态 `type` 标签；`backfillMetricRegistry` 按 table ID 记住这些标签，供 `DDLClearBackfillMetrics` 删除高基数时序。
3. `RegisterLightningCommonMetricsForDDL` 与 `UnregisterLightningCommonMetricsForDDL` 按 DDL job ID 复用、注册和注销 Lightning `metric::Common`。
4. 暴露 DDL worker、syncer、Owner、回填和临时索引统计所需的标签常量、collector 槽位以及六个可替换的临时索引写入钩子。

本模块只定义和管理指标对象。RustCodeGraph 显示目标文件由 `pkg/metrics/metrics.rs` 和 `pkg/metrics/bindinfo_1_aster_unit_test.rs` 使用；仓库搜索未发现 Rust 生产代码调用回填辅助 API 或 Lightning job 指标 API，因此不能把 Go 侧已有的完整业务接线当作当前 Rust 事实。

## 主要符号

- `backfillMetricRegistry { byTblID: HashMap<i64, HashSet<String>> }`：table ID 到动态标签集合的索引。`register` 去重登记；`clear` 原子移除该 table ID 的集合并把标签所有权交给调用者。
- `registeredJobMetrics: LazyLock<Mutex<HashMap<i64, metric::Common>>>`：按 job ID 缓存 Lightning 通用指标。相同 job ID 的重复注册返回 clone，不重复构造。
- `backfillMetricsRegistry: LazyLock<Mutex<backfillMetricRegistry>>`：回填动态标签的全局注册表；互斥锁承担 Go 版本结构体内 `mu` 的职责。
- `InitDDLMetrics()`：集中构造 `JobsGauge`、`HandleJobHistogram`、`BatchAddIdxHistogram`、三个 syncer/Owner 直方图、`DDLWorkerHistogram`、`DDLCounter`、两个回填向量、job table/运行任务/扫描速率指标和 `RetryableErrorCount`。函数先取得 `crate::metrics::PACKAGE_INIT_LOCK`，再写入 `static mut Option<_>` 槽位。
- `DDLIncrSchemaVerOpHist`、`DDLLockSchemaVerOpHist`、`DDLRunJobOpHist`、`DDLHandleJobDoneOpHist`、`DDLTransitOneStepOpHist`、`DDLLockVerDurationHist`、`DDLCleanMDLInfoHist`：以 `[阶段, "*", "*"]` 从 worker 直方图预绑定的 observer，用于聚合公共阶段而不增加 DDL 类型维度。
- `DDLAddOneTempIndexWrite`、`DDLCommitTempIndexWrite`、`DDLRollbackTempIndexWrite`、`DDLResetTempIndexWrite`、`DDLClearTempIndexWrite`、`DDLSetTempIndexScanAndMerge`：默认无副作用的函数指针钩子，预留给运行时安装具体统计实现。
- `generateReorgLabel(label, schemaName, tableName, colOrIdxNames)`：返回 `label-schema-table` 或 `label-schema-table-column/index`；不会转义 `-`，也不会拆分调用方用 `+` 合并的多名称。
- `GetBackfillTotalByTableID` / `GetBackfillProgressByTableID`：生成并登记标签，然后分别从 `BackfillTotalCounter` / `BackfillProgressGauge` 取得单一标签实例。
- `DDLClearBackfillMetrics` / `DDLHasBackfillMetrics` / `GetBackfillLabelsForTest`：删除指定表的两个向量时序、查询全局是否非空、为测试返回指定表标签集合的副本。
- `RegisterLightningCommonMetricsForDDL` / `UnregisterLightningCommonMetricsForDDL` / `GetRegisteredJob`：管理带 `job_id` 常量标签的 Lightning `metric::Common` 及其缓存快照。

## 执行流程

启动初始化与注册流程如下：

1. `metrics::InitMetrics` 受 `INIT_METRICS_ONCE` 保护，调用 `InitDDLMetrics`。
2. `InitDDLMetrics` 获取 `PACKAGE_INIT_LOCK`，逐项创建 `tidb_ddl_*` collector，并写入 `Option` 静态槽位。
3. 它从刚创建的 `DDLWorkerHistogram` 取得七个固定标签 observer；如果该槽位未成功写入会以 `expect` 终止。
4. `metrics::RegisterMetrics` 要求初始化已经完成，克隆并注册 14 个 DDL collector。预绑定 observer 共享相应 histogram，不单独注册；`BatchAddIdxHistogram` 的注册结果由 `metrics_test.rs::test_register_metrics` 冒烟验证。

回填指标流程如下：

1. 调用者选择 `LblAddIndex`、`LblModifyColumn`、`LblAddIdxRate` 等语义标签，并传入 table ID 与对象名。
2. getter 通过 `generateReorgLabel` 生成动态 `type` 标签，在互斥区内把标签加入 table ID 对应的 `HashSet`，再返回 Prometheus counter/gauge handle。
3. DDL 生命周期结束时调用 `DDLClearBackfillMetrics(tableID)`；该函数先从注册表移除并取得全部标签，释放锁后逐一从 gauge 与 counter 向量删除同名 series。
4. 未登记或已清理 table ID 会产生空列表，因此重复清理是无操作；Go 测试 `pkg/ddl/backfill_metrics_test.go::TestBackfillMetricsIdempotentCleanup` 明确验证了这一语义。

Lightning job 指标流程如下：

1. 注册函数锁定 job map；命中 job ID 时返回现有 `Common` 的 clone。
2. 未命中时创建带 `job_id=<十进制 ID>` 常量标签、namespace `tidb`、subsystem `ddl` 的 `metric::Common`，注册到默认 registry 后缓存。
3. 注销函数对 `None` 直接返回；有值时在同一锁内从默认 registry 注销并删除缓存。`GetRegisteredJob` 返回 map 快照，避免调用方直接修改受锁保护的状态。

## 数据与状态

collector 槽位采用 `pub static mut Option<T>`：`None` 表示 `InitDDLMetrics` 尚未执行，`Some` 保存已构造向量或 observer。crate 根以 `#![allow(static_mut_refs)]` 允许这种机械迁移形态；真正的全局一次性保证来自上层 `InitMetrics` 的 `Once`，本文件自身的 `PACKAGE_INIT_LOCK` 只串行化初始化写入，不能单独表达“只执行一次”。直接重复调用 `InitDDLMetrics` 会替换槽位，因此正常入口应是 `InitMetrics`。

指标名均位于 `tidb` namespace、`ddl` subsystem。标签顺序是 API 契约，例如 `DDLWorkerHistogram` 为 `[type, action, result]`，syncer 部署和 Owner handler 为 `[type, result]`，`UpdateSelfVersionHistogram` 只有 `[result]`。调用者必须提供完全相同数量和顺序，否则 Prometheus 兼容层会失败或 panic。

动态状态分为两组：回填注册表用 `HashSet` 消除相同 table/label 的重复登记；Lightning 注册表用 job ID 保证单 job 复用。两个容器以 64 为初始容量，仅是减少常见场景重分配的优化，不是数量上限。`GetBackfillLabelsForTest` 和 `GetRegisteredJob` 都 clone 返回，外部修改不会改变内部 map。

## 依赖与调用关系

上游关系：

- [`pkg/metrics/lib.rs`](lib.rs) 声明并公开 `ddl` 模块，同时提供 `LblType`、`LblResult`、`TiDB` 等包级符号。
- [`pkg/metrics/metrics.rs`](metrics.rs) 的 `InitMetrics` 调用 `InitDDLMetrics`，`RegisterMetrics` 注册本文件的 collector；这是当前 Rust 主接线。
- [`pkg/metrics/bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs) 的 `all_metric_initializers_accept_go_metadata` 直接冒烟调用初始化函数。
- [`pkg/metrics/metrics_test.rs`](metrics_test.rs) 的 `test_register_metrics` 写入 `BatchAddIdxHistogram`、注册全包指标并检查 `tidb_ddl_batch_add_idx_duration_seconds` 可被 gather。

下游关系：

- `metricscommon::NewGaugeVec`、`NewHistogramVec`、`NewCounterVec` 构造兼容 Go 元数据的向量。
- `prometheus::ExponentialBuckets` 生成直方图桶；`WithLabelValues` 取得具体 observer/counter/gauge；`DeleteLabelValues` 删除动态时序；`DefaultRegisterer` 用于 Lightning 指标注册。
- `promutil::NewDefaultFactory` 与 `metric::new_common` 构造 Lightning 通用指标集合。

RustCodeGraph 对 `InitDDLMetrics` 的被调用函数解析出了 `ExponentialBuckets`，文件关系解析出 `metrics.rs` 与测试文件。精确仓库搜索没有找到 `GetBackfill*`、`DDLClearBackfillMetrics`、`RegisterLightningCommonMetricsForDDL` 等函数在其他 Rust 文件中的调用；其业务侧调用链目前只能由 Go 对照文件证明，不能视为 Rust 已接线。

## 错误处理与边界

本模块没有 `Result` 返回。初始化顺序、标签数量和锁健康被视为进程级不变量，违反时通过 `expect`/`unwrap` panic：包括 `PACKAGE_INIT_LOCK` 或两个全局 mutex 中毒、初始化前读取 collector、以及注册表操作期间的锁失败。Prometheus 的注册/注销结果由兼容 API 封装；本文件不向调用者传播错误。

`generateReorgLabel` 只做拼接，不规范化或转义 schema/table/column/index 名称；名称中已有 `-` 时仍保持原样，因此标签用于观测和清理，不应反向解析为可靠结构。空 `colOrIdxNames` 不追加末尾分隔符，非空时追加一段；多列/多索引的 `+` 拼接由调用者负责。

`DDLClearBackfillMetrics` 同时尝试删除 gauge 与 counter 中的每个登记标签，因为两类 getter 共享同一集合；即使某标签只在一种向量出现，删除另一种仍是安全的布尔结果忽略路径。未注册 table ID 和重复清理均为空循环。`UnregisterLightningCommonMetricsForDDL` 接受 `Option<&Common>` 来保持 Go `nil` 无操作语义，但它不会验证传入对象是否就是该 job ID 缓存的对象。

## 并发与资源生命周期

`LazyLock` 保证两个 map 容器按需创建；`Mutex` 串行化 map 访问。回填 getter 在锁内只登记字符串，随后才访问 collector。清理路径先在锁内把整组标签从 map 移走，再在锁外调用 Prometheus 删除，避免外部 collector 操作扩大临界区，也使同一 table ID 的旧生命周期在清理开始时即与注册表分离。

需要注意清理与新登记的竞态：清理取走旧集合后，另一线程可以为同一 table ID 建立新集合；清理仍会删除旧标签对应的全局 series。若新旧生命周期复用了完全相同的标签，新登记取得的 series 可能被并发清理删除，因此业务层应在同一 table ID 的旧回填完全结束后再开始同标签的新生命周期。

Lightning 注册、构造、默认 registry 操作和缓存修改都发生在 `registeredJobMetrics` 锁内，保证相同 job ID 不会并发重复创建。代价是 registry 调用位于临界区；扩展时不能在这些调用内部形成反向获取本锁的路径。注销完成后调用者持有的 `Common` clone 仍可存在，但不再由默认 registry 导出，且 cache 中对应 job ID 已移除。

collector 本身由 Prometheus 类型提供并发安全更新；`static mut` 的初始化与读取安全则依赖启动顺序约束，而不是 Rust 类型系统。六个临时索引钩子也是可变静态函数指针，读取和替换必须由更高层保证没有数据竞争；当前默认实现不分配资源也无副作用。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/metrics/ddl.go`](ddl.go)。Rust 保留了 Go 的 collector 名称、帮助文本、标签顺序、桶参数、阶段 observer、动态标签格式、按 table ID 清理和按 job ID 缓存语义。Go 的 `map[int64]map[string]struct{}` 对应 Rust `HashMap<i64, HashSet<String>>`；Go 指针 collector 对应 Rust `Option<collector>`；Go `*metric.Common` 的 `nil` 注销参数对应 Rust `Option<&metric::Common>`。

锁的布局略有不同：Go 的回填 registry 自带 `mu`，Rust 把整个 registry 包在 `Mutex` 中；Go Lightning map 使用独立包级 `mu`，Rust 把 map 直接包进 `Mutex`。Go `clear` 先删 map 项再在锁外复制标签；Rust `remove` 直接取得集合所有权，减少一次复制。两者都在 Prometheus 删除动作前释放回填锁。

Go 侧 [`pkg/ddl/backfill_metrics_test.go`](../ddl/backfill_metrics_test.go) 覆盖了普通表、分区表、重组分区使用逻辑表 ID、series 实际删除和幂等清理；`pkg/ddl/ingest/testutil/testutil.go::CheckIngestLeakageForTest` 用 `GetRegisteredJob` 检查 job 指标泄漏。当前没有同等的独立 Rust DDL 回填指标测试，且 Rust 生产侧未检索到这些辅助 API 的调用，因此它们的代码级移植不等于完整运行时行为已迁移。

## 扩展指南

新增 DDL collector 时，应在 `InitDDLMetrics` 创建它，并按是否需要直接导出决定是否加入 `metrics.rs::RegisterMetrics`；同步核对 `ddl.go` 的名称、标签顺序、帮助文本和桶，避免产生不兼容的新时序。新增固定 worker 阶段优先复用 `DDLWorkerHistogram` 并预绑定 observer，只有确有查询维度需求时才增加标签，防止基数膨胀。

新增回填类型应选择明确的 `Lbl*` 常量，并继续通过两个 table-ID getter 登记，禁止绕过注册表直接创建动态 series，否则任务结束后无法完整清理。选择 table ID 时需同步 DDL 业务语义：Go 测试表明普通分区回填通常按物理分区清理，而 partition reorg rate 使用逻辑表 ID。若把这些 API 接入 Rust DDL，应在独立测试文件（例如新增 `pkg/metrics/ddl_test.rs` 并由 `lib.rs` 的 `#[cfg(test)]` 声明，或放入相应 DDL crate 的独立 `*_test.rs`）覆盖标签生成、去重、实际 series 删除、分区 ID 选择、重复清理和并发生命周期；不要把测试嵌入本源文件。

新增 Lightning job 指标调用点必须成对安排注册与注销，所有提前返回和错误路径都要执行注销；可用 `GetRegisteredJob` 只读快照做泄漏断言。更改全局初始化模型时，优先把 `static mut Option<_>` 收敛到 `OnceLock`/不可变访问器，并同时调整 `InitMetrics`、`RegisterMetrics` 与现有测试，避免仅在本文件局部增加第二套初始化状态。

兼容风险主要是指标名、标签名/顺序或桶变化导致仪表盘与告警失配；正确性风险主要是初始化前访问、回填标签未清理、table ID 选择错误和 job 指标未注销；性能风险主要来自高基数动态标签、全局 mutex 竞争以及在 Lightning 锁内执行 registry 操作。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/metrics` 确认 `ddl.rs`、Go 对照和相关测试在索引中；`node --file pkg/metrics/ddl.rs --offset 1/240` 读取完整 483 行并报告该文件由 `pkg/metrics/metrics.rs`、`pkg/metrics/bindinfo_1_aster_unit_test.rs` 使用；`query InitDDLMetrics` 同时定位 Go/Rust 定义；`callees InitDDLMetrics` 识别 `ExponentialBuckets` 依赖。
- Rust 源与配置：[`pkg/metrics/ddl.rs`](ddl.rs)、[`pkg/metrics/lib.rs`](lib.rs)、[`pkg/metrics/metrics.rs`](metrics.rs)、[`pkg/metrics/Cargo.toml`](Cargo.toml)。
- Rust 测试：[`pkg/metrics/metrics_test.rs`](metrics_test.rs) 的 `test_register_metrics`；[`pkg/metrics/bindinfo_1_aster_unit_test.rs`](bindinfo_1_aster_unit_test.rs) 的 `all_metric_initializers_accept_go_metadata`。目录中没有 `ddl_test.rs`，仓库搜索也未发现回填/Lightning 辅助 API 的 Rust 调用者。
- Go 对照与边界测试：[`pkg/metrics/ddl.go`](ddl.go)、[`pkg/ddl/backfill_metrics_test.go`](../ddl/backfill_metrics_test.go)、`pkg/ddl/ingest/testutil/testutil.go`。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务规定的 11 章节结构检查，并检查 git diff 仅包含本说明文档与任务文件删除。
