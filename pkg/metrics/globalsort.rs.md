# `pkg/metrics/globalsort.rs`

## 文件定位

源文件 [`pkg/metrics/globalsort.rs`](./globalsort.rs) 属于 `astersql-metrics` crate；crate 入口 `pkg/metrics/lib.rs` 以公开模块 `globalsort` 暴露它。该文件只负责定义并初始化全局排序（global sort）和归并排序（merge sort）的 Prometheus collector，不执行排序、云存储 I/O 或 worker 调度。包级中枢 `pkg/metrics/metrics.rs` 的 `InitMetrics` 调用 `InitGlobalSortMetrics`，随后 `RegisterMetrics` 将本文件的九个 collector 注册到默认 Prometheus registry。

`pkg/metrics/Cargo.toml` 表明该 crate 的库入口是 `lib.rs`，并直接依赖 `prometheus = "0.14"` 和本地 `astersql-metrics-common`；本文件实际经 `crate::bindinfo` 中的兼容层使用这两类能力。文件没有 feature gate、条件编译项、类型、trait 或局部辅助函数。

## 核心职责

本文件维护九个进程级指标句柄，原有八个由入口 `unsafe fn InitGlobalSortMetrics()` 一次构造一组与 Go 版本相同的指标元数据：

- 四个直方图向量分别描述云存储写入/读取的耗时和速率；它们都使用 `type` 标签区分阶段。
- 两个 gauge 描述工作的 ingest worker 数量和活跃上传 worker 数量。
- 两个 counter 累计归并排序读写字节数。

本文件的职责止于“定义和初始化”。初始化后的统一注册由 `pkg/metrics/metrics.rs::RegisterMetrics` 完成；Rust 业务目录中当前未发现对这些八个静态量的采样调用，因此不能据此声称 Rust 全局排序路径已经实际上报样本。

新增孤儿全局排序数据量 `GlobalSortOrphanDataSize`，用于没有活跃 DXF 生产者时的测量结果；它和原有八个初始化式 collector 的生命周期不同。

## 主要符号

- `GlobalSortWriteToCloudStorageDuration: Option<HistogramVec>`：指标名 `tidb_global_sort_write_to_cloud_storage_duration`；桶边界为 `0.001 * 2^i`（20 个桶），用于秒级耗时，标签为 `type`。
- `GlobalSortWriteToCloudStorageRate: Option<HistogramVec>`：指标名 `tidb_global_sort_write_to_cloud_storage_rate`；20 个桶从 `0.05` 起按 2 倍增长，标签为 `type`。
- `GlobalSortReadFromCloudStorageDuration: Option<HistogramVec>`：读取耗时，桶配置与写入耗时相同，标签为 `type`。
- `GlobalSortReadFromCloudStorageRate: Option<HistogramVec>`：读取速率，桶配置与写入速率相同，标签为 `type`。
- `GlobalSortIngestWorkerCnt: Option<GaugeVec>`：指标名 `tidb_global_sort_ingest_worker_cnt`，以 `type` 标签区分 worker 用途。
- `GlobalSortUploadWorkerCount: Option<Gauge>`：指标名 `tidb_global_sort_upload_worker_cnt`，无标签。
- `MergeSortWriteBytes: Option<Counter>` 与 `MergeSortReadBytes: Option<Counter>`：指标名分别为 `tidb_global_sort_merge_sort_write_bytes`、`tidb_global_sort_merge_sort_read_bytes`，累计值只能单调增加。
- `unsafe fn InitGlobalSortMetrics()`：依次为上述八个 `static mut Option<_>` 填入 collector。函数公开是为了保持 Go 风格包级初始化接口；`unsafe` 反映其会写可变静态量。

原有八项的名称、帮助文本、桶和标签写在 `InitGlobalSortMetrics` 中；孤儿指标在 LazyLock 闭包中定义。这里的 counter 名称未显式带 `_total`；Prometheus Rust client 暴露 metric family 时是否规范化后缀，应以采集结果为准，本文不作超出源码的推断。

- `GlobalSortOrphanDataSize: LazyLock<GaugeVec>`：完整指标名 `tidb_global_sort_orphan_data_size_bytes`，无可变标签；惰性构造，不由 `InitGlobalSortMetrics` 赋值。使用向量是为了让 `reset()` 删除样本，区分“尚未测量”和“测量为零”。

## 执行流程

1. 应用或测试调用 `pkg/metrics/metrics.rs::InitMetrics`。该函数由 `INIT_METRICS_ONCE.call_once` 包住整个包的初始化序列。
2. `InitMetrics` 在初始化 ingest/DXF 等子系统指标之后调用 `crate::globalsort::InitGlobalSortMetrics()`。
3. `InitGlobalSortMetrics` 先取得 `crate::metrics::PACKAGE_INIT_LOCK`；锁用于串行化该 crate 内会改写 Go 风格静态量的初始化过程。
4. 函数通过 `metricscommon::NewHistogramVec`、`NewGaugeVec`、`NewGauge` 和 `NewCounter` 顺序构造八个 collector，并写入对应的 `Some(...)`。
5. 随后的 `RegisterMetrics` 通过 `register_options!` 依次读取这八个 `Option`；`register_option` 克隆 collector 后调用 `prometheus::register`。
6. 业务代码应在完成初始化/注册后取得对应 collector，按阶段标签记录耗时、吞吐、worker 数或字节数。当前 Rust 仓库搜索只找到初始化、注册和冒烟测试引用，尚未找到这些业务采样调用。

初始化与注册是两个阶段：仅调用 `InitGlobalSortMetrics` 会创建句柄，但不会把它们加入默认 registry；仅调用 `RegisterMetrics` 而未先初始化则会触发 `expect` panic。

集中注册函数另用 `register_clone` 注册孤儿数据 gauge。`pkg/ingestor/globalsort/orphandata.rs::Monitor::trigger` 在扫描前后确认无活跃生产者，成功后才以空标签取样本并设置字节数；错误、取消或生产者出现保留旧值。`Monitor::reset` 删除系列，而非写入 0。指标定义不负责启动监视器或调度扫描。

## 数据与状态

原有八个句柄都是 `pub static mut Option<_>`，初值为 `None`，初始化后变为 `Some(collector)`。这保留了 Go 包级变量在 `init` 前为空的迁移形状，但也意味着 Rust 类型系统无法自动保证访问同步；调用者必须遵守先初始化、后访问的全局协议。

四个 `HistogramVec` 和 `GlobalSortIngestWorkerCnt` 的唯一可变维度是公共常量 `LblType`（值为 `"type"`）。新增标签值不要求改 collector 定义，但会增加 Prometheus time series 基数。上传 worker gauge 与两个字节 counter 没有标签，不产生标签基数扩张。

直方图只保存聚合后的桶、计数和总和，不保存单次事件；gauge 可以增减或直接设置；counter 只适合累计读写字节。文件本身没有缓存、事务状态或持久化数据。

现在共有九个静态 collector：原有八个 `static mut Option` 和一个安全惰性 `LazyLock<GaugeVec>`。孤儿 gauge 初始未创建样本；设置 0 后可见零值样本，reset 后样本消失。

## 依赖与调用关系

上游调用和装配关系如下：

- `pkg/metrics/lib.rs` 声明 `pub mod globalsort`，形成 crate 公共入口。
- `pkg/metrics/metrics.rs::InitMetrics` 是包级初始化上游，并在 `INIT_METRICS_ONCE` 内调用 `InitGlobalSortMetrics`。
- `pkg/metrics/metrics.rs::RegisterMetrics` 是注册上游，列举原有八个静态量，另以 `register_clone` 注册孤儿 gauge；其 `register_option` 要求初始化已经完成。
- `pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata` 直接调用初始化函数，验证这些 Go 风格元数据至少能被兼容构造器接受。
- `pkg/metrics/metrics_test.rs::test_register_metrics` 间接覆盖完整 `InitMetrics`/`RegisterMetrics` 路径，但断言的是 DDL 指标，未逐项验证 global-sort 指标的名称、桶、标签或采样值。

下游依赖是 `crate::bindinfo::{compat_metricscommon, compat_prometheus}`：前者提供 Go 风格 `New*` 工厂，后者暴露兼容的 `HistogramVec`、`GaugeVec`、`Gauge`、`Counter` 和 opts 类型。导入的 `CounterCompat`、`GaugeCompat`、`MetricCompat`、`ObserverCompat` trait 用于让机械移植的 Prometheus API 形状可用；本文件没有网络、磁盘或云存储依赖。

Go 业务调用点位于 `pkg/ingestor/globalsort`、`pkg/ingestor/simplesst`、`pkg/ingestor/ingestctrl` 和 `pkg/dxf/framework/taskexecutor`。对应 Rust 业务文件尚未引用这些全局 collector，这是当前迁移边界，而不是指标定义文件的隐式行为。

孤儿指标已有 Rust 业务引用：`pkg/ingestor/globalsort/orphandata.rs` 的 trigger/reset；独立 `orphandata_test.rs` 覆盖保留旧值及 reset 移除样本。上述旧八项的采样接线限制不能扩大为“本文件所有指标均无业务引用”。

## 错误处理与边界

`InitGlobalSortMetrics` 没有返回值，也没有可恢复错误分支。唯一显式失败点是取得 `PACKAGE_INIT_LOCK` 时的 `expect("metrics init lock poisoned")`：若持锁线程 panic 导致 mutex poisoned，初始化会继续以 panic 终止。兼容工厂在本接口中直接返回 collector，因此本文件不传播构造错误。

注册错误不在本文件处理：`pkg/metrics/metrics.rs::RegisterMetrics` 返回 `Result<(), prometheus::Error>`，例如默认 registry 中重复注册同名 collector 时会向上传播错误。若某个句柄仍为 `None`，`register_option` 会以 `InitMetrics must run before RegisterMetrics` panic。

本文件不校验观测值。调用者需要避免用零耗时作除数计算速率、避免给 counter 表达可回退状态，并控制 `type` 标签集合。上述输入约束来自指标种类和 Go 使用方式；原有八项的 Rust 业务采样接线尚未由上述引用证实，本文件不提供额外保护。

## 并发与资源生命周期

collector 是进程级长生命周期对象：初始化后由静态量持有，注册时向 registry 提交 clone，预期持续到进程结束。本文件没有显式销毁、注销、后台任务或通道；业务方可 reset 孤儿 GaugeVec 的样本。

并发安全分两层：包级 `InitMetrics` 用 `INIT_METRICS_ONCE` 防止正常生产路径重复初始化，本函数自身再用 `PACKAGE_INIT_LOCK` 串行化兼容静态量的写入。然而 `pub static mut` 的读取和直接重复调用仍依赖调用方的 `unsafe` 纪律；本函数的锁不能保护在锁外同时读取这些变量的代码。扩展时应复用 `InitMetrics` 路径，不应从并发业务线程任意重跑 `InitGlobalSortMetrics`。

指标值本身由 Prometheus collector 实现并发更新语义；本文件不持有业务锁。Go 中 ingest worker 使用成对 `Inc`/`Dec`，上传 worker使用 `Set`，字节数使用 `Add`，这些生命周期约定尚未在 Rust 业务调用中得到直接验证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metrics/globalsort.go`。Rust 与 Go 均保留原有八个包级变量，并在同名 `InitGlobalSortMetrics` 中使用相同 namespace（`tidb`）、subsystem（`global_sort`）、name、help、桶参数和 `type` 标签。主要表示差异是 Go 使用指针/接口零值，Rust 使用 `Option`；Rust 还增加 `unsafe`、初始化互斥锁以及兼容 API 别名。

Go 的真实采样位置给出了各标签语义和生命周期证据：

- `pkg/ingestor/simplesst/writer.go` 使用 `sort`、`sort_and_write`、`write` 标签记录写入侧耗时和 MiB/s 速率。
- `pkg/ingestor/globalsort/reader.go` 与 `engine.go` 使用 `read_one_file`、`read_and_sort`、`read`、`sort` 标签记录读取侧耗时和速率。
- `pkg/ingestor/ingestctrl/job_worker.go` 对 `execute job` 标签执行 worker gauge 的 `Inc`、`Dec` 和兜底 `Set(0)`。
- `pkg/dxf/framework/taskexecutor/manager.go` 用活跃上传 worker 数覆盖设置上传 gauge。
- `pkg/ingestor/simplesst/onefile_writer.go` 与 `iter.go` 分别累计 merge-sort 写入和读取字节。

Rust 搜索未发现上述八个符号在业务文件中的引用，只有 `pkg/ingestor/simplesst/onefile_writer.rs` 的注释提到 Go 的 `MergeSortWriteBytes` 行为。因此指标定义和注册已经迁移，但业务采样接线仍未由当前代码证实。

Go `globalsort.go` 同样定义无可变标签的孤儿数据 GaugeVec 并在初始化中构造；Rust 采用 `LazyLock`。Go 监视器在 `pkg/ingestor/globalsort/orphandata/monitor.go`，Rust 对应 `orphandata.rs`；两端 reset 都表达未测量，而非零字节。

## 扩展指南

新增 global-sort 指标时，至少需要同步四处：在本文件增加独立静态句柄；在 `InitGlobalSortMetrics` 中使用稳定的 namespace/subsystem/name/help/桶/标签构造它；在 `pkg/metrics/metrics.rs::RegisterMetrics` 的列表中注册；在独立测试文件中验证 metric family、类型、标签、桶和一次真实更新。若对应 Go 指标已存在，应逐字段保持一致并列出 Go 采样点；若是 Rust 独有指标，则应明确其来源而不是伪装成机械移植。

给现有指标补齐 Rust 业务接线时，最可能修改的是 `pkg/ingestor/globalsort/*.rs`、`pkg/ingestor/simplesst/*.rs`、`pkg/ingestor/ingestctrl/*.rs` 或 DXF task executor 对应实现。测试应放在这些模块现有的独立 `*_test.rs` 文件或 `pkg/metrics` 的独立测试文件中，不能嵌入生产源文件。应重点验证错误/提前返回时 gauge 能否恢复、速率除数是否为零、counter 是否重复累计，以及 label 值是否与 Go 一致。

兼容性风险主要来自重命名指标或标签、修改桶边界以及遗漏注册；这些变化会破坏 dashboard、告警或 time series 连续性。性能风险主要来自无界扩张 `type` 标签值和在热点路径重复计算/记录；并发风险则来自直接访问 `static mut`。若未来不再要求保持 Go 迁移形状，优先考虑一次性线程安全容器和安全访问函数，但这属于跨文件设计变更，不应只改本文件。

新增惰性指标不必强制接入 `InitGlobalSortMetrics` 的可变静态赋值，但仍须明确注册和业务更新点；孤儿 gauge 必须保持 reset 与零值的区别。

## 验证依据

- 冻结依据：`ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`；用 `git diff ad0a6f4..2c944fb -- pkg/metrics/globalsort.rs` 定位增量、`git show 2c944fb:pkg/metrics/globalsort.rs` 核对源码。本文不包含目标提交之后的源码变化。
- RustCodeGraph `status` 显示当前索引含 Rust，但索引未固定到目标提交；冻结源码、测试和接线使用 `git show` / `git grep ... 2c944fb` 核对，不以当前图调用关系证明历史版本。
- 增量交叉证据：`pkg/metrics/lib.rs`、`Cargo.toml`、`metrics.rs` 和 Go `globalsort.go` / `metrics.go`；业务更新见 `pkg/ingestor/globalsort/orphandata.rs`，独立 `orphandata_test.rs` 的 `orphan_monitor_keeps_old_value_when_a_producer_appears_after_scan` 与 `orphan_monitor_first_gate_skips_storage_and_reset_removes_series` 固定旧值和 reset 语义。
- 本次只作静态文档同步；未运行 Cargo、单元测试或真实服务。Ready 文档验证执行 11 章节计数、限定文件 `git diff --check` 与秘密文本扫描；仓库 `.agents/skills/tidb-verify-profile` 缺失，按任务验证计划提供等价文档证据。
