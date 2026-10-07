# `pkg/metrics/tikv_client_metrics.rs`

## 文件定位

本文件属于 `astersql-metrics` crate；`pkg/metrics/Cargo.toml` 将该 crate 的入口指定为 `lib.rs`，而 `pkg/metrics/lib.rs` 以公开模块 `tikv_client_metrics` 挂载它。它不是 Rust TiKV 客户端的埋点实现，而是 TiDB 指标层的一组 **client-go 描述符兼容 collector**：AsterSQL 使用的 Rust TiKV 客户端自带 Prometheus 0.13 私有注册表，不能直接向本 crate 使用的 Prometheus 0.14 注册表提供 TiDB 仪表盘所需的那 8 个 client-go collector，因此这里在 0.14 侧重建相同的名称、标签和桶定义。

模块位于包级指标初始化链上：`pkg/metrics/metrics.rs::RegisterMetrics` 调用本文件的 `RegisterMetrics`，把 collector 注册到默认 registry；同文件的 `ToggleSimplifiedMode` 调用 `unused_collectors`，在普通模式与简化模式之间注销或重新注册它们。指标值真正在哪里被观测需要区分实现：Go 版由 client-go 请求路径更新；本仓库 Rust 搜索只发现本模块定义、注册及测试引用，没有发现业务请求路径对这 8 个静态量调用 `observe`/`inc`，所以当前有注册兼容性，但不能据此声称 Rust TiKV 请求会填充这些序列。

## 核心职责

1. 用 `NAMESPACE = "tidb"` 和主要子系统 `SUBSYSTEM = "tikvclient"` 重建 8 个 TiKV client-go 指标描述符；其中两项 SLI 指标按 Go 兼容约定使用 `sli` 子系统。
2. 通过 `LazyLock` 延迟且只初始化一次 collector，避免模块加载时立即分配所有 Prometheus 对象。
3. 通过 `InitMetrics` 显式触发全部惰性静态量，使注册入口能取得完整、稳定的 collector 集合。
4. 通过 `unused_collectors` 返回共享句柄的克隆，供常规注册和 `ToggleSimplifiedMode` 的注销/重注册流程复用。
5. 通过 `RegisterMetrics` 将整组 collector 注册到调用方提供的 `Registry`，并保留首个注册错误。

这里不负责采集 TiKV RPC、RawKV 或 Region 重试数据，也不实现简化模式互斥状态；前者在 Go 对照中属于 client-go 的请求路径，后者由 `pkg/metrics/metrics.rs::ToggleSimplifiedMode` 的 `MODE` 锁管理。

## 主要符号

- `NAMESPACE` / `SUBSYSTEM`：私有字符串常量，分别固定为 `tidb` 和 `tikvclient`，参与大部分完整指标名的构造。
- `TiKVRawkvSizeHistogram: LazyLock<HistogramVec>`：`tidb_tikvclient_rawkv_kv_size_bytes`，标签为 `type`，指数桶从 1 字节开始、倍率 2、共 30 个边界；Go 快捷标签包括 `key`、`value`（以及现有 client-go 中名为 `raw_checksum` 的快捷量）。
- `TiKVRawkvCmdHistogram: LazyLock<HistogramVec>`：`tidb_tikvclient_rawkv_cmd_seconds`，标签为 `type`，指数桶参数为 `0.0005, 2, 29`，表示 RawKV 命令耗时。
- `TiKVReadThroughput: LazyLock<Histogram>`：`tidb_sli_tikv_read_throughput`，无可变标签，指数桶参数为 `1024, 2, 13`；它刻意不使用 `tikvclient` 子系统。
- `TiKVSmallReadDuration: LazyLock<Histogram>`：`tidb_sli_tikv_small_read_duration`，无可变标签，指数桶参数为 `0.0005, 2, 28`。
- `TiKVBatchWaitOverLoad: LazyLock<Counter>`：`tidb_tikvclient_batch_wait_overload`，无可变标签，统计 TiKV transport 批处理过载事件。
- `TiKVBatchClientRecycle: LazyLock<Histogram>`：`tidb_tikvclient_batch_client_reset`，无可变标签，指数桶参数为 `0.001, 2, 28`，记录连接回收及重连耗时。
- `TiKVRequestRetryTimesHistogram: LazyLock<Histogram>`：`tidb_tikvclient_request_retry_times`，无可变标签，显式桶边界为 `1, 2, 3, 4, 8, 16, 32, 64, 128, 256`。
- `TiKVStatusDuration: LazyLock<HistogramVec>`：`tidb_tikvclient_kv_status_api_duration`，标签为 `store`，指数桶参数为 `0.0005, 2, 20`。
- `InitMetrics()`：公开的强制初始化入口，逐一解引用 8 个 `LazyLock`；它没有返回值，构造失败只能在初始化闭包内部 panic。
- `unused_collectors() -> Vec<Box<dyn Collector>>`：先调用 `InitMetrics`，再按固定顺序克隆并装箱 8 个 collector。克隆 Prometheus 句柄不会创建独立时间序列，底层状态仍共享。
- `RegisterMetrics(registry: &Registry) -> prometheus::Result<()>`：依序注册 `unused_collectors` 返回的句柄；任一 `registry.register` 失败即由 `?` 提前返回。

文件没有自定义类型、trait、`impl` 或条件编译项；全部静态量和三个函数都是公开 API，但常规上游只经 `pkg/metrics/metrics.rs` 使用它们。

## 执行流程

常规启动注册链如下：

1. 包级 `pkg/metrics/metrics.rs::RegisterMetrics` 完成其余 TiDB collector 的注册。
2. 它以 `prometheus::default_registry()` 调用本模块 `RegisterMetrics`。
3. `RegisterMetrics` 调用 `unused_collectors`；后者先执行 `InitMetrics`。
4. 第一次解引用各 `LazyLock` 时，闭包借助 `astersql_metrics_common::NewHistogramVec`、`NewHistogram` 或 `NewCounter` 构造指标；后续调用直接取得已经初始化的共享对象。
5. `unused_collectors` 克隆 8 个句柄并按声明对应的固定顺序返回；`RegisterMetrics` 逐个注册。全部成功后返回 `Ok(())`，否则停止于第一个错误。

简化模式切换链为：

1. `pkg/metrics/metrics.rs::ToggleSimplifiedMode` 先持有包级 `MODE: Mutex<bool>`，相同目标模式直接返回。
2. 它处理其他高开销 TiDB 指标后遍历 `tikv_client_metrics::unused_collectors()`。
3. `simplified == true` 时尝试从默认 registry 注销句柄并忽略单项注销失败；`false` 时重新注册，注册错误会中断并向调用者返回。
4. `pkg/metrics/metrics_internal_test.rs::test_tikv_simplified_collectors_match_go_descriptors` 用独立 `Registry` 验证同一组句柄可以完成注册、逐项注销和再次注册。

## 数据与状态

模块级持久状态只有 8 个 `LazyLock`。每个锁在进程生命周期内最多执行一次初始化闭包，保存 Prometheus collector 句柄；本文件没有可变全局集合，也没有维护“是否已注册”标志。某个 collector 是否在某个 registry 中完全由对应 `Registry` 管理。

三个 `HistogramVec` 使用动态标签：RawKV 大小和命令耗时使用 `type`，状态 API 耗时使用 `store`；其余 collector 没有动态标签。直方图桶、namespace、subsystem、name 与 help 都是描述符身份和仪表盘兼容契约的一部分，不能只把名称视为兼容标准。`unused_collectors` 的返回顺序也被 Rust 测试用于按序核对完整名称，因此调整顺序会改变现有测试契约。

句柄克隆共享同一底层指标状态。这一性质使一个句柄可注册、另一个等价克隆可用于注销，也保证普通模式恢复后不是从一份新的本地 collector 状态开始。不过，registry 会拒绝重复注册同描述符 collector；本模块本身不做幂等检测。

## 依赖与调用关系

上游直接关系：

- `pkg/metrics/lib.rs`：声明 `pub mod tikv_client_metrics`，形成 crate 公共模块边界。
- `pkg/metrics/metrics.rs::RegisterMetrics`：调用 `tikv_client_metrics::RegisterMetrics(default_registry)`，是正常应用注册链的直接上游。
- `pkg/metrics/metrics.rs::ToggleSimplifiedMode`：调用 `unused_collectors`，根据模式对默认 registry 进行注销或注册。
- `pkg/metrics/metrics_internal_test.rs::test_tikv_simplified_collectors_match_go_descriptors`：直接验证描述符名称及注册生命周期。

下游直接关系：

- `prometheus` 0.14：提供 `Collector`、`Counter`、`Histogram`、`HistogramVec`、`Registry`、`Opts`、`HistogramOpts` 及 `exponential_buckets`。该版本由 `pkg/metrics/Cargo.toml` 明确声明。
- `astersql-metrics-common`：路径依赖 `pkg/metrics/common`，本文件使用其 Go 风格工厂函数 `NewCounter`、`NewHistogram` 和 `NewHistogramVec` 构造 collector。
- `std::sync::LazyLock`：承担线程安全的一次性延迟初始化。

RustCodeGraph 将文件识别为 4 个节点（文件节点，以及 `InitMetrics`、`unused_collectors`、`RegisterMetrics` 三个函数节点），并给出文件被 `metrics.rs` 与相关测试等引用；对本文件内部的有效调用边为 `RegisterMetrics -> unused_collectors -> InitMetrics`。图工具对同名 Go/Rust 函数存在歧义，因此应用级上游又以精确路径搜索核验。

## 错误处理与边界

- `prometheus::exponential_buckets(...).unwrap()` 出现在 7 个指数桶定义中。参数都是静态正数且 count 非零，按当前库约束应成功；若未来改为非法参数，第一次初始化相应 `LazyLock` 时会 panic，而不是返回可恢复错误。
- `InitMetrics` 无错误返回，指标工厂如果内部拒绝描述符也会在初始化阶段暴露为 panic；调用方不能逐项降级。
- `RegisterMetrics` 使用 `?` 返回首个注册错误，例如同一 registry 中已有同名/同描述符 collector。此前已经成功注册的项不会回滚，所以失败可能留下部分注册状态。
- `unused_collectors` 只产生句柄集合，不检查 registry，也不保证调用方的批量操作原子性。
- `ToggleSimplifiedMode` 的注销路径故意忽略单项失败，而恢复注册路径返回错误；模式布尔值在注册循环前已更新，因此中途失败时模式标志与实际 registry 内容可能暂时不完全一致。这是上游切换逻辑的边界，不由本文件修复。
- 当前 Rust 仓库没有找到业务路径更新这些兼容 collector 的证据；扩展者必须将“可抓取的描述符已注册”与“请求执行已产生样本”分开验证。

## 并发与资源生命周期

`LazyLock` 保证多个线程首次访问同一静态量时只执行一次初始化，并在进程余下生命周期持有 collector；本文件没有显式线程、异步任务、通道或事务。Prometheus 句柄的克隆适合跨调用共享，实际计数和直方图状态由库内部同步机制保护。

注册生命周期跨越两个模块：本文件提供稳定句柄，`Registry` 保存注册关系，`metrics.rs::MODE` 的互斥锁串行化简化模式切换。`unused_collectors` 每次分配新的 `Vec` 和 8 个 `Box<dyn Collector>`，但不会重建指标主体。注销或 registry 销毁只移除/释放注册持有关系；静态 `LazyLock` 仍存活到进程退出，之后重新注册仍指向相同底层累计状态。

## 与 Go 版本的对应关系

仓库 Go 入口 `pkg/metrics/metrics.go::ToggleSimplifiedMode` 直接引用 `github.com/tikv/client-go/v2/metrics` 的同名 8 个 collector，并把它们和其他“不被 Grafana 使用”的指标一起切换。`go.mod` 固定 client-go 为 `v2.0.8-0.20260928031501-8edb23f6c7ee`；该版本模块缓存中的 `metrics/metrics.go` 证明这 8 项的 namespace、subsystem、name、help、标签和桶与本文件逐项一致。

Go 与 Rust 的关键差异是所有权和数据来源：

- Go 版 collector 由 client-go 创建、注册并在真实客户端路径中更新。例如 `rawkv/rawkv.go` 观测 RawKV 命令耗时，`internal/client/conn_batch.go` 增加过载计数，`internal/client/client.go` 观测连接回收，`internal/locate/region_request.go` 观测 Region 请求重试，`internal/locate/store_cache.go` 观测状态 API 耗时，`metrics::ObserveReadSLI` 根据键数、大小和耗时选择小读取耗时或读取吞吐量。
- Rust 文件只在 `astersql-metrics` 的 Prometheus 0.14 侧复制描述符并负责注册生命周期；它没有上述请求路径接线。模块注释明确说明 Rust TiKV 客户端的 Prometheus 0.13 私有 registry 无法直接提供这些 0.14 collector。
- Go `ToggleSimplifiedMode` 自身不返回错误：恢复注册遇错时记录日志并停止；Rust `ToggleSimplifiedMode` 与本文件 `RegisterMetrics` 使用 `Result` 将注册错误向上返回。两边注销都不把失败作为切换失败。
- Go 定义可接受额外 `constLabels`；本文件工厂调用没有设置常量标签。当前文档只确认默认描述符兼容，不推断带 keyspace 等常量标签配置时仍完全等价。

## 扩展指南

若 client-go 的简化模式集合新增或修改指标，应同时完成以下工作：

1. 以 `go.mod` 当前固定版本的 `client-go/v2/metrics/metrics.go` 为准，逐项同步类型、namespace、subsystem、name、help、动态/常量标签和桶；不要只复制最终完整名称。
2. 在本文件新增或修改相应 `LazyLock`，并同步 `InitMetrics`、`unused_collectors` 的初始化和返回顺序。若指标应始终注册而不参与简化模式，不应无条件加入 `unused_collectors`。
3. 确认 `pkg/metrics/metrics.rs::RegisterMetrics` 与 `ToggleSimplifiedMode` 的接线语义仍正确；新增不同注册表或分组需求时，优先让句柄集合表达所有权，避免另建不共享状态的重复 collector。
4. 在独立测试文件 `pkg/metrics/metrics_internal_test.rs` 扩展 `test_tikv_simplified_collectors_match_go_descriptors`，至少核对完整名称、标签、桶（若有变化）以及注册—注销—重注册。遵循仓库约束，不把测试写进本生产源文件。
5. 如果目标是让 Rust TiKV 请求真正产生样本，还必须在 Rust 客户端或适配边界建立观测接线，并单独验证样本值；仅新增这里的描述符不能完成数据采集。跨 Prometheus 0.13/0.14 桥接还需评估依赖版本、重复注册、状态共享和抓取性能。

兼容风险主要是仪表盘查询因名称/标签变化而失效、直方图桶变化导致聚合不可比，以及同名 collector 重复注册。性能风险主要来自高基数 `store`/`type` 标签和新增直方图桶；正确性风险则是注册了空兼容序列却误认为底层客户端已经埋点。

## 验证依据

- 源文件：`pkg/metrics/tikv_client_metrics.rs`，核对了 2 个常量、8 个 `LazyLock` 静态 collector、3 个公开函数，以及 `RegisterMetrics -> unused_collectors -> InitMetrics` 的内部流程。
- crate 与模块边界：`pkg/metrics/Cargo.toml`（`astersql-metrics`、`prometheus = "0.14"`、`astersql-metrics-common` 路径依赖）和 `pkg/metrics/lib.rs`（公开模块声明）。该包没有 `doc.go`，因此以 crate 入口注释作为最近的包级契约。
- Rust 上游与测试：`pkg/metrics/metrics.rs::{RegisterMetrics, ToggleSimplifiedMode}`；`pkg/metrics/metrics_internal_test.rs::test_tikv_simplified_collectors_match_go_descriptors`。仓库搜索未发现同名独立 `tikv_client_metrics*_test.rs`，相关回归测试位于上述独立测试模块。
- Go 对照：`pkg/metrics/metrics.go::ToggleSimplifiedMode`；`go.mod` 中 client-go 固定版本；模块缓存 `github.com/tikv/client-go/v2@v2.0.8-0.20260928031501-8edb23f6c7ee/metrics/metrics.go`、`metrics/shortcuts.go`、`rawkv/rawkv.go`、`internal/client/{conn_batch.go,client.go}`、`internal/locate/{region_request.go,store_cache.go}`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件且目标文件已收录；`files --filter pkg/metrics/tikv_client_metrics.rs` 显示该文件有 4 个符号；`node --file ... --offset 1 --limit 400` 返回完整 173 行源码及引用摘要；`query tikv_client_metrics --json` 返回文件节点和三个函数节点；`callers`/`callees` 输出确认内部调用边，并暴露同名符号歧义，故调用者另用路径限定的 `rg` 复核。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰好包含这 11 个固定二级标题。
