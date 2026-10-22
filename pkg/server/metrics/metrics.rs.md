# `pkg/server/metrics/metrics.rs`

## 文件定位

定义 MySQL 命令字节常量和命令到 Prometheus `type` 标签的稳定映射，核心入口为 `cmd_to_string` / `CmdToString`。

## 核心职责

1. 定义 MySQL 命令字节常量和命令到 Prometheus `type` 标签的稳定映射，核心入口为 `cmd_to_string` / `CmdToString`。
2. 创建四组本地 Prometheus 向量：`QUERY_TOTAL_COUNTER`、`DISCONNECTION_COUNTER`、`CONN_IDLE_DURATION_HISTOGRAM`、`PACKET_IO_COUNTER`。
3. 由向量预绑定常用标签，形成调用方可直接 `inc`、`inc_by` 或 `observe` 的句柄；查询计数器以命令字节为稀疏下标。
4. 用 `init_metrics_vars`、`init` 和 Go 风格别名 `InitMetricsVars` 显式强制初始化所有惰性句柄，补偿 Rust 没有 Go 包级 `init()` 钩子的差异。
5. 保留 Go 导出名兼容层，例如 `QueryTotalCountOk`、`DisconnectNormal`、`InPacketBytes`，使移植代码能沿用原有概念和命名。

## 主要符号

- `DEFAULT_RESOURCE_GROUP_NAME: &str = "default"`：查询指标默认使用的资源组标签值。
- `COM_SLEEP` 至 `COM_STMT_FETCH`：本文件使用的 MySQL 命令字节常量。`COM_CREATE_DB` 仅用于表达稀疏槽位，不在 `QUERY_COMMANDS` 中预绑定。
- `QUERY_TOTAL_COUNTER: LazyLock<CounterVec>`：公开的查询总数向量，指标全名由 namespace/subsystem/name 组成，即 `tidb_server_query_total`；标签顺序为 `type`、`result`、`resource_group`。
- `DISCONNECTION_COUNTER`：私有断连向量，指标为 `tidb_server_disconnection_total`，标签为 `result`。
- `CONN_IDLE_DURATION_HISTOGRAM`：私有连接空闲时长向量，指标为 `tidb_server_conn_idle_duration_seconds`，标签为 `in_txn`；桶由 `exponential_buckets(0.0005, 2.0, 29)` 生成。
- `PACKET_IO_COUNTER`：私有包 IO 字节向量，指标为 `tidb_server_packet_io_bytes`，标签为 `type`。
- `QUERY_COMMANDS: &[(u8, &str)]`：13 个需预绑定命令与标签名的唯一表驱动来源，也被命令字符串转换复用。
- `query_counters(result: &str) -> Vec<Option<Counter>>`：内部构造器，生成长度为 `COM_STMT_FETCH + 1` 的稀疏计数器数组。
- `QUERY_TOTAL_COUNT_OK` / `QUERY_TOTAL_COUNT_ERR`：分别以 `OK` / `Error` 绑定 `result` 的查询计数器数组；Go 风格再导出名为 `QueryTotalCountOk` / `QueryTotalCountErr`。
- `DISCONNECT_NORMAL`、`DISCONNECT_BY_CLIENT_WITH_ERROR`、`DISCONNECT_ERROR_UNDETERMINED`：分别绑定 `ok`、`error`、`undetermined`。
- `CONN_IDLE_DURATION_HISTOGRAM_NOT_IN_TXN` / `..._IN_TXN`：分别绑定 `in_txn="0"` 与 `"1"`。
- `IN_PACKET_BYTES` / `OUT_PACKET_BYTES`：分别绑定 `type="In"` 与 `"Out"`。
- `cmd_to_string(cmd: u8) -> String`：已知命令返回 Go 兼容标签，未知命令返回无符号十进制字符串；`CmdToString` 是兼容包装。
- `init_metrics_vars()`：强制初始化九组公开句柄；`init()` 与 `InitMetricsVars()` 都只转调它。

## 执行流程

初始化主流程如下：

1. `pkg/util/metricsutil/common.rs::initMetrics` 调用 `initParentMetricsCollectors`，再按固定顺序调用各子 crate 初始化器，其中包含 `server_metrics::InitMetricsVars()`。
2. `InitMetricsVars()` 转调 `init_metrics_vars()`。
3. `init_metrics_vars()` 对查询成功/失败数组、三个断连句柄、两个空闲直方图和两个包 IO 句柄逐一执行 `LazyLock::force`。
4. 第一次初始化查询数组时，`query_counters("OK")` 和 `query_counters("Error")` 分别分配 29 个 `Option<Counter>` 槽位，再遍历 `QUERY_COMMANDS`，通过 `QUERY_TOTAL_COUNTER.with_label_values(...)` 把 13 个已知命令绑定到对应字节下标；其余下标保持 `None`。
5. 其余句柄在各自首次强制求值时，从对应 `CounterVec` / `HistogramVec` 绑定固定标签值。后续调用 `InitMetricsVars()` 只读取已初始化的 `LazyLock`，不会重新创建句柄。

命令标签转换流程独立于初始化：`cmd_to_string` 在线性表 `QUERY_COMMANDS` 中查找命令字节，命中则复制标签名，未命中则执行 `u8::to_string`。Go 风格入口 `CmdToString` 不增加分支。

## 数据与状态

本文件的长期状态全部位于进程级 `static LazyLock` 中。四个基础向量拥有真实的 Prometheus 样本集合；公开的 `Counter` / `Histogram` 是它们按标签取得的克隆句柄，更新会落到对应标签序列。

查询数组刻意采用 `Vec<Option<Counter>>`，以命令字节直接索引并保留 Go 复合字面量产生的空洞语义。数组长度为 `COM_STMT_FETCH`（28）加一；例如 `COM_CREATE_DB`（5）没有出现在 `QUERY_COMMANDS`，因此成功和失败数组的该槽位均为 `None`。调用方必须同时检查下标范围和 `Option`，不能假定所有协议命令均有预绑定句柄。

标签值区分大小写并属于兼容契约：查询结果使用 `OK` / `Error`，断连使用小写 `ok` / `error` / `undetermined`，包方向使用 `In` / `Out`，事务状态使用字符串 `0` / `1`。修改这些值会形成新的 Prometheus 时间序列并破坏现有查询或看板。

## 依赖与调用关系

- 上游装配：`pkg/server/metrics/lib.rs` 私有装载实现并公开再导出；测试构建时同一根模块还装载 `migration_aster_unit_test.rs`。
- 上游生产初始化：`pkg/util/metricsutil/common.rs::initMetrics -> astersql_server_metrics::InitMetricsVars -> init_metrics_vars`。RustCodeGraph 也将 `pkg/util/metricsutil/common.rs` 标为本文件使用方。
- crate 边界：`pkg/server/metrics/Cargo.toml` 仅声明 `prometheus` 依赖；`pkg/server/Cargo.toml` 以路径依赖 `metrics` 引入该 crate。
- 内部调用边：`init -> init_metrics_vars`、`InitMetricsVars -> init_metrics_vars`、`CmdToString -> cmd_to_string`；两个查询数组的惰性初始化闭包调用 `query_counters`。
- 下游库调用：向量构造使用 `prometheus::{Opts, HistogramOpts, CounterVec, HistogramVec}`；标签预绑定使用 `with_label_values`；惰性生命周期由 `std::sync::LazyLock` 管理。
- Go 消费证据：`pkg/server/conn.go` 更新查询、断连和空闲指标，`pkg/server/internal/packetio.go` 更新包字节指标。当前 Rust 全仓搜索未找到对应公开句柄在协议运行时的直接更新，因此这些 Go 调用点是迁移对照和未来接线依据，不是 Rust 当前已执行调用边。

## 错误处理与边界

指标构造失败被视为编程期不变量破坏，而非可恢复运行时错误：`CounterVec::new`、`HistogramVec::new` 使用带说明的 `expect`，指数桶生成使用 `unwrap`。这里的参数和标签均为编译进二进制的常量；若将来改为动态配置，应重新评估 panic 策略。

`cmd_to_string` 对任意 `u8` 都有结果，未知命令不会报错，而是回退到十进制文本；测试覆盖 `COM_CREATE_DB` 的 `"5"` 和 `u8::MAX` 的 `"255"`。`query_counters` 的写入安全依赖 `QUERY_COMMANDS` 中最大命令不超过 `COM_STMT_FETCH`；新增更大命令而不扩大向量会在初始化时越界 panic。

本文件不负责指标注册、HTTP 暴露、连接错误分类或调用方的边界检查。尤其 `QueryTotalCountOk/Err` 含 `None` 空洞，使用处必须避免直接解包。基础向量是本 crate 本地构造的 collector；文档证据只证明其可收集和可更新，不推断它们已经注册到某个全局 registry。

## 并发与资源生命周期

`LazyLock` 保证每个静态值在并发首次访问时只初始化一次；`init_metrics_vars` 可重复调用，同一进程生命周期内不会重建向量或丢失已经累计的样本。`prometheus::Counter`、`CounterVec` 和 `Histogram` 句柄可被多个调用方共享更新，本文件本身不创建线程、任务、锁守卫、通道或显式清理流程。

所有指标随进程静态生命周期存在，没有注销或重置 API。测试通过不同标签和固定增量读取样本，因此若未来增加并行测试或共享同一进程内反复运行，需要注意全局计数累积和测试间相互影响；新增测试仍应放在独立测试文件，而不是嵌入 `metrics.rs`。

## 与 Go 版本的对应关系

实现差异主要有三点：

1. Go 使用 `pkg/metrics` 中已经定义的 `QueryTotalCounter`、`DisconnectionCounter`、`ConnIdleDurationHistogram`、`PacketIOCounter`；Rust 本文件自行构造同名描述的本地向量。`pkg/util/metricsutil/common.rs` 的注释也明确说明 Rust 树把父 collector 分散在 owner crates 中。
2. Go 的包级 `init()` 自动调用 `InitMetricsVars()`；Rust 用显式 `init()` 函数和生产初始化链调用，函数名相同不代表语言运行时会自动执行它。
3. Go 查询数组元素类型为 `prometheus.Counter`，空洞表现为 `nil`；Rust 用 `Option<Counter>` 显式表达空洞。Rust 的 `LazyLock` 还使初始化幂等，而 Go 的 `InitMetricsVars` 每次会重新给包变量赋句柄。

## 扩展指南

新增或修改命令指标时，应同步检查 `COM_*` 常量、`QUERY_COMMANDS`、`query_counters` 的向量上界、`cmd_to_string` 的兼容标签以及 Go 对照 `metrics.go`。若新命令字节大于 28，必须先扩大稀疏向量，不能只向 `QUERY_COMMANDS` 追加记录。相应测试应扩展 `pkg/server/metrics/migration_aster_unit_test.rs`，至少覆盖名称转换、成功/失败标签和预期空洞；不要把测试写入生产源文件。

新增指标族时，应在本文件定义基础向量和预绑定句柄，把句柄加入 `init_metrics_vars` 的强制初始化列表，并通过 `lib.rs` 的既有再导出边界公开。还应核对 `pkg/util/metricsutil/common.rs` 的父 collector 初始化/注册责任，避免创建与中央 collector 描述相同却重复注册的实例。

把指标真正接入 Rust server 时，应优先对照 `pkg/server/conn.go` 和 `pkg/server/internal/packetio.go` 的更新时机，在 Rust 的连接关闭分类、命令完成、空闲采样和包读写边界分别接线；需要特别保持“错误/成功仅计一次”、命令数组越界/空洞回退、字节数口径以及事务状态采样时点。性能上应继续复用预绑定句柄，避免每次请求动态创建标签字符串或执行 vector 查找。

兼容性复核重点是指标全名、标签键、标签值大小写和桶边界；任何变化都可能导致监控时间序列断裂。若要改用共享 `pkg/metrics` Rust collector，需要先验证 registry 所有权和初始化顺序，不能仅替换类型来源。
