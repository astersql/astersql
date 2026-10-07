# `pkg/ingestor/ingestmetric/metric.rs`

## 文件定位

本文件实现 next-generation ingest 路径的 Prometheus 耗时指标，所属 Rust crate 是 `astersql-ingestor-ingestmetric`。crate 入口 [`lib.rs`](./lib.rs) 声明 `metric` 模块并重导出其全部公开项，因此调用方以 `astersql_ingestor_ingestmetric::InitIngestMetrics`、`WriteAPIDuration` 等路径使用这里的 API。

[`Cargo.toml`](./Cargo.toml) 表明该 crate 只直接依赖 `astersql-metrics-common` 和 `prometheus 0.14`；端到端消费者是 `pkg/metrics/metrics.rs`（全局初始化和注册）及 `pkg/ingestor/ingestcli/client.rs`（记录 write/ingest 请求耗时）。它只负责定义、初始化和注册指标，不执行 SST 写入、HTTP 请求或 TiKV ingest。

## 核心职责

1. 用 `InitIngestMetrics` 创建名为 `tidb_ingestor_write_ingest_api_duration` 的 `HistogramVec`，并以 `api` 标签区分 `write` 与 `ingest`。
2. 从同一个向量取得两个已绑定标签的 `Histogram` 观察器，供请求热路径直接 `observe`，避免调用点重复选择标签。
3. 用三个进程级、延迟构造且可替换的共享槽位暴露向量和观察器；`Register` 可将向量注册到指定 `prometheus::Registry`。

该文件不是门面、生成代码或桩：指标描述、桶边界、标签集合和初始化顺序都在此处形成真实运行时状态。

## 主要符号

- `lblAPI: &str = "api"`：私有标签名。它决定导出序列中的维度名，修改会改变监控查询契约。
- `LabelWriteAPI = "write"`、`LabelIngestAPI = "ingest"`：公开标签值，与 Go 版本和客户端打点语义一致。
- `WriteIngestAPIDuration: LazyLock<RwLock<Option<HistogramVec>>>`：完整 collector。初始值为 `None`，初始化后供注册、测试按标签读取以及重新取得子观察器。
- `WriteAPIDuration`、`IngestAPIDuration: LazyLock<RwLock<Option<Histogram>>>`：分别绑定 `write`、`ingest` 标签的观察器。两者是对同一个 `HistogramVec` 子序列的句柄，不是另建两个指标族。
- `InitIngestMetrics()`：建立 20 个指数桶（首个上界 `0.001` 秒，倍率 `2`，最后一个显式上界 `524.288` 秒），构造 namespace=`tidb`、subsystem=`ingestor`、name=`write_ingest_api_duration` 的向量，再依次写入三个全局槽位。
- `Register(&prometheus::Registry)`：克隆已初始化的向量句柄并注册；初始化缺失、锁中毒或 registry 拒绝注册时会 panic。

本文件没有 trait、struct、enum、普通 `impl` 或条件编译项；唯一的同步结构来自标准库 `LazyLock`/`RwLock`。

## 执行流程

正常应用链路如下：

1. `pkg/metrics/metrics.rs::InitMetrics` 在其 `INIT_METRICS_ONCE.call_once` 闭包中调用 `InitIngestMetrics`。
2. `InitIngestMetrics` 通过 `prometheus::exponential_buckets(0.001, 2.0, 20)` 生成桶；当前参数为常量，失败被视为程序员错误并由 `expect` 终止。
3. `metricscommon::NewHistogramVec` 创建带唯一变量标签 `api` 的向量；随后 `with_label_values` 分别物化 `write` 和 `ingest` 子观察器。
4. 函数依次把向量、write 观察器、ingest 观察器写入静态槽位。全局 `InitMetrics` 的一次性保护保证生产初始化不会反复替换它们。
5. `pkg/metrics/metrics.rs::register_external_metrics` 当前直接读出并克隆 `WriteIngestAPIDuration`，把它注册到 registry。虽然本文件也提供 `Register`，当前 Rust 生产接线未调用它；`migration_aster_unit_test.rs` 会直接验证该 API。
6. write 请求由 `pkg/ingestor/ingestcli/client.rs::WriteWorker::new` 的后台线程在 `send_write_request` 返回后调用 `observe_write_duration`。因此成功和错误返回都计入从 worker 开始到请求结束的耗时。
7. ingest 请求在 `ClientImpl::ingest` 发 HTTP POST 前创建 `IngestDurationGuard`；RAII guard 在作用域退出时打点，因此成功、非 200、传输错误以及后续解码错误都会记录从 guard 创建后的耗时。guard 创建前的 URL、region epoch 和 SST metadata 校验不计入该指标。

## 数据与状态

指标的稳定身份是 `tidb_ingestor_write_ingest_api_duration`，help 为 `write and ingest API duration`，变量标签只有 `api`。显式桶覆盖 1ms 到 524.288s；Prometheus histogram 还按库语义包含 `+Inf` 累计桶。观察值单位由调用者传入的 `elapsed().as_secs_f64()` 确定为秒。

三个 `LazyLock` 只延迟创建锁和初始 `None`，不会自行创建指标。真正的状态转换是 `None -> Some(...)`，由 `InitIngestMetrics` 完成。`HistogramVec::with_label_values` 返回的两个句柄与向量共享对应标签序列的数据；`migration_aster_unit_test.rs` 分别通过观察器写入，再从向量按相同标签读取计数，验证了这一关系。

`InitIngestMetrics` 本身没有 once guard，直接重复调用会用一套新 collector 替换三个槽位。应用级 `pkg/metrics/metrics.rs::InitMetrics` 提供 once 语义，但独立调用者必须自行保证初始化时机；已经克隆出去或已经注册的旧 collector 不会因槽位替换而自动变成新 collector。

## 依赖与调用关系

上游入口和消费者：

- `pkg/metrics/metrics.rs::InitMetrics -> InitIngestMetrics`：应用级指标初始化入口；RustCodeGraph 也把 `metric.rs` 标为由该文件使用。
- `pkg/metrics/metrics.rs::register_external_metrics -> WriteIngestAPIDuration`：当前生产注册边，先要求全局初始化已完成，再向 registry 注册 clone。
- `pkg/ingestor/ingestcli/client.rs::observe_write_duration -> WriteAPIDuration`：write worker 完成后观察耗时。
- `pkg/ingestor/ingestcli/client.rs::IngestDurationGuard::drop -> IngestAPIDuration`：ingest 调用离开计时作用域时观察耗时。
- [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 调用 `InitIngestMetrics` 和 `Register`，验证标签、样本、描述符与桶。

下游依赖：

- `prometheus::exponential_buckets`、`HistogramOpts`、`HistogramVec`、`Histogram`、`Registry` 提供桶、collector、观察器和注册表。
- `metricscommon::NewHistogramVec` 是仓库统一的 histogram-vector 构造入口。
- `std::sync::LazyLock` 提供进程级延迟初始化，`RwLock` 协调初始化/读取与测试中的替换。

RustCodeGraph 对本文件识别出 3 个函数级/文件级符号并报告使用方 `pkg/metrics/metrics.rs`、`pkg/ingestor/ingestcli/client_test.rs`；对这些静态变量没有建立可查询节点，因此静态变量的精确引用边由上述源码检索核实。

## 错误处理与边界

- 指数桶构造使用 `expect("valid ingest duration buckets")`。参数是固定合法常量，若库契约变化导致失败，初始化会 panic，而不是静默缺少指标。
- 三次写锁和 `Register` 的读锁都用 `unwrap`；曾持锁 panic 会造成锁中毒，随后初始化或注册会 panic。
- `Register` 要求先调用 `InitIngestMetrics`，否则 `Option::expect` panic；重复向同一 registry 注册同描述符 collector 时，`Registry::register` 错误也被 `expect` 转为 panic。
- 初始化写入三个槽位不是单次原子提交。绕过应用级初始化约束、在初始化并发读取时，调用者理论上可短暂看到部分槽位已更新；正常生产链通过 `InitMetrics` 先初始化、后注册和服务请求来规避这一窗口。
- ingest 客户端读取观察器时采用 `if let Ok(...) && let Some(...)`，锁中毒或未初始化会跳过打点而不影响业务请求。这个容错行为位于消费者，不在本文件中。
- 本文件不限制或过滤观测值；耗时来源、是否覆盖错误路径由客户端计时边界决定。

## 并发与资源生命周期

`Histogram`/`HistogramVec` 的并发计数由 Prometheus crate 管理；本文件的 `RwLock` 主要保护可替换的 `Option` 句柄，而非每次 histogram 更新。客户端先取得读锁、借用观察器并调用 `observe`，初始化则取得写锁替换句柄。

静态槽位生命周期与进程相同，没有显式释放或注销流程。注册时 clone collector 句柄并把 clone 的所有权交给 registry；局部观察器和 registry collector 共享底层指标状态。write 的计时资源随后台 worker 生命周期结束，ingest 的 guard 依靠 `Drop` 覆盖提前返回。该模块自身不创建线程、异步任务、通道、事务或外部连接。

若未来允许运行时重初始化，必须同时处理三个槽位的一致性、已注册旧 collector 的生命周期及并发观察；当前实现和测试只支持“启动期初始化，运行期只读/observe”的使用模型。

## 与 Go 版本的对应关系

直接对照文件是 [`metric.go`](./metric.go)。两版保持以下语义一致：指标 namespace/subsystem/name/help、`api` 标签、`write`/`ingest` 标签值、`0.001 * 2^n` 的 20 个显式桶，以及初始化后取得两个已绑定标签的观察器。Go 注释给出的范围 `1ms ~ 524s` 与 Rust 测试验证的末桶 `524.288` 秒一致。

实现差异主要来自语言和接线方式：

- Go 使用包级可变变量和 `prometheus.Observer` 接口；Rust 使用 `LazyLock<RwLock<Option<_>>>`，观察器保存为具体 `Histogram`。
- Go `Register` 接受通用 `prometheus.Registerer` 并调用 `MustRegister`；Rust 接受具体 `Registry`，同样把注册错误升级为 panic。
- Go 的 `pkg/metrics/metrics.go::RegisterMetrics` 调用 `ingestmetric.Register`；Rust 的 `pkg/metrics/metrics.rs::register_external_metrics` 当前直接克隆和注册 `WriteIngestAPIDuration`，行为等价但没有复用本文件的 `Register`。
- Go 包初始化流程调用一次 `InitIngestMetrics`；Rust 将一次性语义放在上层 `InitMetrics`，便于独立 crate 测试重新初始化。
- Go 客户端以 `defer` 覆盖 write/ingest 计时退出路径；Rust write 在线程函数结束处显式观察，ingest 用 `Drop` guard 对齐 defer 语义。

## 扩展指南

- 新增 API 类别时，应在本文件增加稳定标签常量和对应观察器，在 `InitIngestMetrics` 中从同一向量绑定标签，并在 `pkg/ingestor/ingestcli/client.rs` 的准确生命周期边界打点；同时同步 `metric.go`，避免 Rust/Go 指标序列分叉。
- 修改指标名、namespace、subsystem、help、标签名或标签值属于监控兼容性变更，会影响 dashboard、告警和历史序列；应优先保持现有 descriptor，确需变更时提供查询迁移方案。
- 修改桶应评估观测范围和每个标签序列的内存/抓取开销，并同步 Rust 独立测试末桶断言及 Go 实现。增加标签会按基数乘法扩张时间序列，禁止把 region、SST ID 等高基数字段直接加入。
- 若调整注册方式，应统一 `pkg/metrics/metrics.rs::register_external_metrics` 与本文件 `Register` 的职责，防止同一 registry 重复注册；若保留两个入口，应继续明确它们不可对同一 registry 重复调用。
- 测试逻辑必须继续放在独立文件。初始化/注册/桶/标签测试应更新 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；实际计时边界应更新 `pkg/ingestor/ingestcli/client_test.rs`，全局装配应更新 `pkg/metrics/metrics_internal_test.rs`。不要把 `#[cfg(test)]` 测试嵌入本文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/ingestor/ingestmetric` 找到 `metric.rs`、`lib.rs` 和独立迁移测试；`query InitIngestMetrics --kind function --json` 定位 Rust/Go 对照定义；`node --file .../metric.rs` 返回完整 78 行源码并报告 `pkg/metrics/metrics.rs`、`pkg/ingestor/ingestcli/client_test.rs` 使用该文件。精确 `callers/callees` 查询未返回可用边，故未以其推断调用关系。
- 源码与边界：[`metric.rs`](./metric.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、`pkg/metrics/metrics.rs`、`pkg/ingestor/ingestcli/client.rs`。
- Rust 测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 验证初始化、两个标签样本、注册后的全名/help 和首末桶；`pkg/ingestor/ingestcli/client_test.rs` 验证 write/ingest 实际耗时增长；`pkg/metrics/metrics_internal_test.rs` 验证全局初始化与外部注册。
- Go 对照：[`metric.go`](./metric.go)、`pkg/metrics/metrics.go`、`pkg/ingestor/ingestcli/client.go`、`pkg/ingestor/ingestcli/client_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构校验要求本文恰有十一个规定的二级标题；最终交付前另行执行并记录退出码。
