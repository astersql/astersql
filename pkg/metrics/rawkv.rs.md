# `pkg/metrics/rawkv.rs`

## 文件定位

为包级 [`metrics::InitMetrics`](metrics.rs) 与 [`metrics::RegisterMetrics`](metrics.rs) 提供初始化和注册对象。

## 核心职责

- 声明 `RAW_KV_BATCH_PUT_DURATION_SECONDS`，按 column family（`cf`）记录一次 RawKV BatchPut 的耗时分布，单位为秒。
- 声明 `RAW_KV_BATCH_PUT_BATCH_SIZE`，按 `cf` 记录一次 BatchPut 包含的键值条目数分布。
- 在 `init_raw_kv_metrics` 中按 Go `InitRawKVMetrics` 的名称、帮助文本、标签和指数桶配置构造两个 `HistogramVec`。
- 为包级 [`metrics::InitMetrics`](metrics.rs) 与 [`metrics::RegisterMetrics`](metrics.rs) 提供初始化和注册对象。

本文件只创建 collector，不选择实际 `cf` 值，也不调用 `observe`。是否产生样本取决于业务调用方取得对应标签子指标并写入观测值。

## 主要符号

- `pub static mut RAW_KV_BATCH_PUT_DURATION_SECONDS: Option<prometheus::HistogramVec>`：初始化前为 `None`，初始化后保存耗时向量。metric family 全名由 namespace `tidb`、subsystem `rawkv`、name `rawkv_batch_put_duration_seconds` 组成，即 `tidb_rawkv_rawkv_batch_put_duration_seconds`；唯一变量标签为 `cf`。17 个桶由 `ExponentialBuckets(0.001, 2.0, 17)` 生成，从 1ms 起逐次翻倍，最后一个显式边界为 65.536 秒。
- `pub static mut RAW_KV_BATCH_PUT_BATCH_SIZE: Option<prometheus::HistogramVec>`：初始化前为 `None`，初始化后保存批大小向量。全名为 `tidb_rawkv_rawkv_batch_put_batch_size`，唯一变量标签同为 `cf`。9 个桶为 1、2、4、8、16、32、64、128、256。
- `pub unsafe fn init_raw_kv_metrics()`：唯一函数，构造上述两个向量并写入全局 `Option`。函数为 `unsafe`，因为它直接写 `static mut`；它不返回 `Result`，也不负责注册。
- `metricscommon::NewHistogramVec`：来自 [`bindinfo.rs`](bindinfo.rs) 的 Go 兼容构造器。它把本文件的 Go 风格 `HistogramOpts` 和标签数组转换为 `prometheus` crate 的配置，再委托 crate 的 metrics-common 工厂创建向量。

文件没有类型、trait、`impl`、条件编译项或内部帮助函数；三个公开符号构成全部 API。

## 执行流程

正常包级流程如下：

1. 上层调用 [`metrics::InitMetrics`](metrics.rs)。该函数由 `INIT_METRICS_ONCE` 保护，在固定的子系统初始化顺序中调用 `crate::rawkv::init_raw_kv_metrics()`。
2. `init_raw_kv_metrics` 先构造耗时 `HistogramVec`：设置 `tidb/rawkv` 前缀、metric 名称和帮助文本，生成 17 个指数桶，并声明 `cf` 标签。
3. 函数再以相同方式构造批大小 `HistogramVec`，但使用从 1 到 256 的 9 个指数桶。
4. 两个新 handle 分别写入对应的全局 `Option`。初始化函数本身不产生任何时间序列；带标签的 child 通常在第一次 `with_label_values(&[cf])` 后出现。
5. 上层调用 [`metrics::RegisterMetrics`](metrics.rs) 时，`register_options!` 读取两个 `Option`，要求它们已为 `Some`，克隆 collector 并注册到默认 Prometheus registry。
6. 理想消费流程是 BatchPut 完成一次尝试后，以同一个 `cf` 分别观测批内条目数和耗时。Go 客户端在满批 `Put` 与残余批 `PutRest` 中这样做；当前 Rust BR 客户端也在同样位置收集同样数据，但写入的是其局部 `metrics` 模块，而非本文件的向量。

## 数据与状态

本模块状态只有两个进程级 `static mut Option<HistogramVec>`。`None` 对应 Go 包级指针在 `InitRawKVMetrics` 之前的 nil 状态；`Some` 持有可克隆的 collector handle。collector 内部累计每个 `cf` 标签组合的样本数、样本和以及各桶累计计数。

两个向量的标签集合固定为单个 `cf`。本文件不枚举或验证 label value；Go/Rust RawKV 客户端证据展示了 `default` 和 `write` 等值。新增高基数、动态生成的 `cf` 值会增加时间序列数量，调用方必须维持 column family 的有限集合约束。

直接重复调用 `init_raw_kv_metrics` 会用新 handle 替换全局 `Option`，不会清空已经注册在 registry 中的旧 collector。正常入口的 `InitMetrics` 用 `Once` 阻止这种替换；但公开函数自身没有锁或幂等保护。与部分 sibling metrics 初始化函数不同，本函数也没有取得 `PACKAGE_INIT_LOCK`。

## 依赖与调用关系

- 模块装配边：[`lib.rs`](lib.rs) 的 `pub mod rawkv` 使两个静态量和初始化函数可从 `astersql_metrics::rawkv` 访问。
- 初始化边：RustCodeGraph 对 [`metrics.rs`](metrics.rs) 的源码关系显示 `metrics::InitMetrics -> rawkv::init_raw_kv_metrics`；Go 对应边是 `metrics.InitMetrics -> InitRawKVMetrics`。
- 注册边：[`metrics.rs`](metrics.rs) 的 `RegisterMetrics` 在 `register_options!` 清单中读取 `RAW_KV_BATCH_PUT_DURATION_SECONDS` 和 `RAW_KV_BATCH_PUT_BATCH_SIZE`。Go [`metrics.go`](metrics.go) 同样对两者调用 `prometheus.MustRegister`。
- 构造边：`init_raw_kv_metrics -> compat_metricscommon::NewHistogramVec -> metricscommon::NewHistogramVec`。[`bindinfo.rs`](bindinfo.rs) 的兼容层负责将 namespace、subsystem、name、help、桶和标签转换到外部 `prometheus` crate。
- Cargo 边：[`Cargo.toml`](Cargo.toml) 将本模块归入 `astersql-metrics`，直接依赖 `prometheus = "0.14"`，并通过路径依赖 `astersql-metrics-common` 使用统一工厂；没有控制 rawkv 模块的 feature。
- 测试边：[`metrics_2_aster_unit_test.rs`](metrics_2_aster_unit_test.rs) 经 `InitMetrics` 取得耗时向量，写入 `cf="default"` 的 0.001 秒样本，并断言它包含 17 个桶。
- 业务边界：Go [`rawkv_client.go`](../../br/pkg/restore/internal/rawkv/rawkv_client.go) 的 `Put`/`PutRest` 直接观测这两个包级指标。Rust 同路径文件的 `Put`/`PutRest` 调用的是本地 `metrics::RawKVBatchPut*Observe`；其局部模块用 `Mutex<Vec<Observation>>` 保存测试观测，不依赖 `astersql-metrics`，不能视为本模块的生产消费者。

## 错误处理与边界

`init_raw_kv_metrics` 没有可恢复错误通道。兼容构造器最终通过 metrics-common 工厂创建 `prometheus::HistogramVec`；非法名称、帮助文本、标签或桶配置属于编程错误，相关构造路径会 panic，而不是由本函数返回 `Result`。当前固定元数据与 Go 对照一致，现有 Rust 初始化测试证明正常配置可创建。

若在初始化前调用 `RegisterMetrics`，[`metrics.rs`](metrics.rs) 的 `register_option` 会以 `InitMetrics must run before RegisterMetrics` panic。注册时的重复 metric family 或 registry 错误则由 `RegisterMetrics() -> Result<(), prometheus::Error>` 传播。若绕过包级 `Once` 重跑本初始化函数，已注册的旧 handle 与新全局 handle 可能脱节。

本模块不判断 BatchPut 成功或失败，也不控制观测时机。Go 与 Rust 客户端都在底层 `BatchPut` 返回后、检查错误前记录两项数据，所以失败尝试也会进入观测；这是调用方语义，不是本模块的分支。负耗时、负批大小或未知 `cf` 也不会在此被拒绝，正确单位和值域由调用者保证。

当前最重要的功能边界是接线缺口：Rust BR 的局部观测桩可被独立测试验证，但不会更新这里注册的 Prometheus collector。因此不能仅凭这两个 collector 已注册就断言 Rust RawKV BatchPut 已向服务 registry 发布真实样本。

## 并发与资源生命周期

两个 `HistogramVec` 的内部观测操作由 `prometheus` crate 提供并发安全性；获得稳定 handle 后，不同线程可以对不同或相同 `cf` 子指标并发 `observe`。collector 注册后由默认 registry 持有 clone，其计量状态按 handle 的共享语义存活至进程结束，不需要显式关闭。

危险点在全局容器而非 collector：`static mut Option` 的读写需要 `unsafe`，本文件没有锁。正常生命周期依赖 [`metrics::InitMetrics`](metrics.rs) 的 `Once` 先完成唯一一次写入，再由注册和消费者读取。并发直接调用 `init_raw_kv_metrics`，或在初始化写入时读取静态量，均不受本文件同步保护；扩展时不应把公开初始化函数当作可并发重入 API。

本文件不创建线程、异步任务、channel、事务、网络连接或文件资源。业务侧用于测量耗时的 `Instant`、键值缓冲和 BatchPut 错误生命周期均位于 RawKV 客户端实现，不归本模块所有。

## 与 Go 版本的对应关系

差异是 Rust 的全包初始化增加 `Once` 和错误保存状态，但本文件自身仍以可替换的 `static mut Option` 表达迁移形状。

## 扩展指南

- 修改 metric 名称、namespace、subsystem 或 `cf` 标签前，必须核对 Go [`rawkv.go`](rawkv.go)、dashboard/告警兼容性和注册清单；这些字段变化会产生新的时间序列并中断现有查询。
- 修改桶边界时，应分别考虑耗时分布的精度/尾延迟范围与批大小上限。桶数量增加会放大每个 `cf` 的时间序列开销；不能只为了测试方便缩减 Go 已有的 17/9 桶语义。
- 新增 RawKV 指标时，需要同时增加静态槽位、初始化构造、[`metrics.rs`](metrics.rs) 注册项及独立 Rust 测试；若任务要求 Go 对齐，还要逐项核对 Go 增量。
- 要补齐真实 Rust 消费链，应在 [`br/pkg/restore/internal/rawkv/rawkv_client.rs`](../../br/pkg/restore/internal/rawkv/rawkv_client.rs) 的满批 `Put` 和 `PutRest` 路径接入 `astersql-metrics`，同时决定是否保留或替换局部观测桩。接线必须保持现有“底层调用结束后、错误判断前观测”的 Go 时序，并避免引入依赖环或让 BR 测试依赖全局 registry。
- 测试必须放在独立文件中。可扩展 [`metrics_2_aster_unit_test.rs`](metrics_2_aster_unit_test.rs) 验证两项全名、标签和完整桶；业务接线则扩展 [`br/pkg/restore/internal/rawkv/parity_test.rs`](../../br/pkg/restore/internal/rawkv/parity_test.rs) 或同目录独立测试，证明实际 collector 收到成功与失败批次观测。
- 若重构全局状态，优先使用能安全发布且不会替换已注册 handle 的一次初始化结构；同时保留测试环境需要的初始化顺序，避免在并发测试中创建两个同名 collector。
