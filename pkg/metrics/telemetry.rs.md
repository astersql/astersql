# `pkg/metrics/telemetry.rs`

## 文件定位

[`telemetry.rs`](telemetry.rs) 是 `astersql-metrics` crate 的遥测指标定义与快照算术模块，由 [`lib.rs`](lib.rs) 以 `pub mod telemetry` 公开。它处在“业务路径累计 Prometheus Counter”与“遥测周期读取累计值、计算增量”之间：文件本身构造并持有 CTE、分区、DDL、Index Merge、Store Batch Copr 等功能计数器，同时提供与 Go `pkg/metrics/telemetry.go` 同名的兼容入口和快照类型。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：包名为 `astersql-metrics`，本文件直接使用 `prometheus = "0.14"`，没有本文件专属 feature。包级 [`metrics.rs`](metrics.rs) 的 `InitMetrics` 调用 `telemetry::InitTelemetryMetrics`；[`../../session/metrics/metrics.rs`](../session/metrics/metrics.rs) 再从同一 `TelemetryMetrics` 克隆句柄，供 Session/执行路径累计用量。目标包没有 `doc.go`，因此模块入口 `lib.rs`、Cargo 清单与代码调用边是这里的边界依据。

## 核心职责

1. `TelemetryMetrics::new` 创建一组名称、namespace、subsystem 和标签值对齐 Go 的 Prometheus `Counter`/`CounterVec`。多数指标使用 `tidb_telemetry_*`，非事务 DML、语句节点、惰性悲观唯一检查和 Fair Locking 使用 `tidb_server_*`。
2. `TELEMETRY_METRICS` 与 `init_telemetry_metrics` 提供进程级惰性单例，使包级初始化、Session 指标别名和 DDL 写入路径共享同一批 collector 对象。
3. `Get*Counter` 系列把 Prometheus 累计值投影为纯数据快照；`snapshot!`、显式 `sub` 和 `TablePartitionUsageCounter::cal` 支持两次采样之间的增量计算。
4. 对 Go 中由其他 metrics 文件拥有的指标，getter 优先读取 Rust 的真实共享句柄：`crate::session::{NonTransactionalDMLCount, LazyPessimisticUniqueCheckSetCount, FairLockingUsageCount}` 和 `crate::executor::StmtNodeCounter`，只有这些静态量尚未初始化时才退回本模块自有计数器。

本文件不负责定时上报、JSON 序列化或默认 registry 的完整注册流程。Rust 中遥测汇总另见 `pkg/telemetry/data_feature_usage.rs`；当前代码搜索未发现它直接调用本文件的 `Get*Counter`，因此不能把两个模块描述成已经接通的生产采样链。

## 主要符号

- `counter` / `counter_vec`：内部构造辅助函数，统一设置 `namespace("tidb")` 和默认的 `subsystem("telemetry")`；`read_counter` 用 `Counter::get()` 取 `f64` 后以 `as i64` 转换。
- `TelemetryMetrics`：公开 collector 容器。`cte`、`account_lock`、`non_transactional_dml`、`stmt_node`、`fair_locking_usage` 是 `CounterVec`；`table_partition` 是严格按 Go 字段顺序排列的 15 元素数组；其余字段是标量 `Counter`。
- `TelemetryMetrics::new() -> Result<Self, prometheus::Error>`：按固定描述符创建所有 collector；任一描述符非法都会提前返回错误，不留下半构造的全局对象。
- `TELEMETRY_METRICS: OnceLock<TelemetryMetrics>`、`init_telemetry_metrics`、Go 风格别名 `InitTelemetryMetrics`：单例初始化入口。内部 `metrics()` 把初始化错误转成 panic，供无 `Result` 返回值的 getter 使用。
- 快照读取入口：`GetCTECounter`、`GetAccountLockCounter`、`GetMultiSchemaCounter`、`GetExchangePartitionCounter`、`GetTablePartitionCounter`、`ResetTablePartitionCounter`、`GetNonTransactionalStmtCounter`、`GetSavepointStmtCounter`、`GetLazyPessimisticUniqueCheckSetCounter`、`GetDDLUsageCounter`、`GetIndexMergeCounter`、`GetStoreBatchCoprCounter`、`GetFairLockingUsageCounter`。
- `snapshot!`：为简单的纯 `i64` 快照生成 `Clone + Debug + Default + PartialEq + Eq`、公开字段，以及逐字段 `sub`/`Sub`。它生成 `CTEUsageCounter`、`AccountLockCounter`、`MultiSchemaChangeUsageCounter`、`ExchangePartitionUsageCounter`、`IndexMergeUsageCounter`、`NonTransactionalStmtCounter` 和 `FairLockingUsageCounter`。
- 显式快照类型：`TablePartitionUsageCounter` 处理 15 个有顺序约束的字段及特殊 `cal`；`DDLUsageCounter` 的 `MetadataLockUsed` 不参与差分；`StoreBatchCoprCounter` 的 `BatchSize` 不参与差分。

公开 API 保留 Go 命名（如 `GetCTECounter`、字段 `NonRecursiveCTEUsed`）并通过 `#[allow(non_snake_case)]` 兼容机械移植调用；内部辅助函数和 Rust 原生方法使用 snake_case。

## 执行流程

初始化主链如下：

1. `metrics.rs::InitMetrics` 在包级 `Once` 内依次初始化各子系统，并调用 `telemetry::InitTelemetryMetrics()`。
2. `init_telemetry_metrics` 先检查 `TELEMETRY_METRICS.get()`；未初始化时执行 `TelemetryMetrics::new()`，成功后尝试 `OnceLock::set`，最后从单例取回静态引用。并发竞争中只有一个值被保存，失败的 `set` 结果被有意忽略，随后统一读取胜出的对象。
3. `pkg/session/metrics/metrics.rs::InitMetricsVars` 调用同一入口，把 CTE、分区、账号锁、Index Merge 和 Store Batch 等 collector/带标签子 counter 克隆到 Session 侧静态句柄。生产代码通过这些句柄递增；例如 `pkg/ddl/index.rs::init_for_reorg_indexes` 在选中 merge 流程时直接取得 `add_index_ingest` 并 `inc()`。
4. 采样方调用 `Get*Counter`。本文件拥有的指标经 `metrics()` 读取；非事务 DML、Savepoint、Lazy Unique Check 和 Fair Locking 则先尝试 Session/Executor 的共享静态量，以免读取一个未被业务路径递增的备用 collector。
5. 遥测周期把当前快照与基准快照传给 `sub`/`Sub` 或 `cal`/`Cal`，得到区间增量；上报后的基准更新属于上层职责。

分区快照是特殊流程：`GetTablePartitionCounter` 读取全部 15 项；`cal` 先逐项执行 `current - previous`，再令索引 9（最大分区数）为 `max(差值, previous.max)`；`ResetTablePartitionCounter` 从实时计数开始，应用相同最大值规则，并把索引 10 到 13 清零，对应 Go reset 省略的 interval/compact 字段，索引 14 的 reorganize 值保留为当前读数。

## 数据与状态

`TelemetryMetrics` 保存单调递增的 Prometheus counter 句柄，不保存“本周期”概念。标签和值是兼容契约：CTE 使用 `nonRecurCTE`、`recurCTE`、`notCTE`；账号使用 `lockUser`、`unlockUser`、`createOrAlterUser`；非事务 DML 使用 `delete`、`update`、`insert`；Fair Locking 使用 `txn-used` 和 `txn-effective`（共享指标路径从 `crate::session` 常量取标签）。改变这些字符串会创建不同的时间序列。

`table_partition: [Counter; 15]` 的位置与 `TablePartitionUsageCounter::{from_values, values}` 是双向位置协议：0..=8 为分区类型，9 为最大分区数，10..=12 为 interval create/add/drop，13 为 compact，14 为 reorganize。新增或重排字段必须同步构造数组、两个转换方法、reset/cal 逻辑、Go 对照和测试。

快照全部使用有符号 `i64`，因此 counter 值经 `f64 as i64` 截断；普通差分没有饱和或归零保护，进程重启、基准错序或非常大的数值都可能产生负值或转换边界行为。`BatchSize` 当前固定为 0；`MetadataLockUsed` 构造时为 false，二者都明确不从 Prometheus 指标读取。

## 依赖与调用关系

上游调用与写入证据：

- `pkg/metrics/metrics.rs::InitMetrics -> telemetry::InitTelemetryMetrics`：包级初始化入口。
- `pkg/session/metrics/metrics.rs::InitMetricsVars -> init_telemetry_metrics`：把同一对象的 collector 克隆成 Session 侧写入句柄。
- `pkg/ddl/index.rs::init_for_reorg_indexes -> InitTelemetryMetrics -> add_index_ingest.inc()`：直接生产写入边。
- RustCodeGraph 对 `init_telemetry_metrics` 的结果还列出本文件内部 `metrics`/`InitTelemetryMetrics` 以及 Session metrics 迁移测试；对 `metrics` 的调用者列出全部 `Get*` 入口和包内相关初始化/测试。

下游依赖：

- `prometheus::{Counter, CounterVec, Opts, Error}` 提供 collector、标签向量、描述符与构造错误。
- `std::sync::OnceLock` 提供进程级一次初始化和 `&'static TelemetryMetrics` 生命周期。
- `crate::session` 与 `crate::executor` 提供四类由其他模块真正拥有和注册的共享指标。`metrics.rs::RegisterMetrics` 明确注册 `StmtNodeCounter`、`NonTransactionalDMLCount`、`LazyPessimisticUniqueCheckSetCount` 和 `FairLockingUsageCount`。

代码搜索限制：除 `telemetry_test.rs` 外，当前 Rust 生产代码没有直接引用这些 `Get*Counter` 名称；`pkg/telemetry/data_feature_usage.rs` 维护自己的一套 `MetricsSnapshot` 和同名结构。因而本模块当前明确接线的是 collector 构造/共享写入，公开快照 getter 的生产消费端未在仓库中验证到。

## 错误处理与边界

- `TelemetryMetrics::new` 和两个初始化入口返回 `prometheus::Error`；`metrics.rs::InitMetrics` 用 `?` 传播并在包级 Once 状态中记录错误。
- 无 `Result` 的 getter 经 `metrics()` 初始化；如果描述符构造失败，会以 `expect("valid telemetry metric descriptors")` panic。当前名称与标签是静态常量，但新增动态或冲突描述符时仍需保留失败路径。
- `OnceLock::set` 的竞争失败不是错误：另一个线程已经发布有效对象，函数最终读取单例。若 `new()` 本身失败，则不会写入单例，后续调用仍可重试。
- `CounterVec::with_label_values` 要求标签个数与声明一致；本文件使用固定标签数量。扩展标签而不更新所有调用点会在运行时失败/panic。
- 读取共享 `static mut Option<_>` 位于 `unsafe` 块。若其他模块尚未初始化则走本地备用值，但这只能保证 getter 可调用，不能保证备用值与随后初始化的共享 collector 合并。
- Go `readCounter` 通过 `Metric.Write` 读取并在错误时返回 `-1`；Rust `read_counter` 直接调用 `get()`，没有可报告的读取错误。因此错误哨兵语义并非逐字等价。
- 普通 `sub` 允许负数；没有检测 counter reset、采样逆序或溢出。`TablePartitionUsageCounter::cal` 的最大值特殊规则也完全按 Go 算法保留，而非通用 counter 差分。

## 并发与资源生命周期

`TELEMETRY_METRICS` 是进程级永久对象：第一次成功初始化后不替换、不析构，返回引用具有 `'static` 生命周期。`OnceLock` 使并发初始化不会发布半构造状态；每个竞争者可能各自在栈上成功构造对象，但只有胜者进入单例，败者对象被丢弃。

Prometheus `Counter`/`CounterVec` 的 clone 是共享底层状态的句柄，这使 `pkg/session/metrics/metrics.rs` 的别名递增能被本文件快照读回。此文件自身不创建线程、任务、通道、锁或事务，也不执行网络/文件 I/O。并发安全主要委托给 `OnceLock` 与 Prometheus collector；相反，读取其他模块的 `static mut Option` 依赖全局初始化先后约定，类型系统无法替调用方验证该约定。

快照结构是拥有型整数数据，没有资源释放要求。它们可克隆，但除 `TelemetryMetrics` 外未实现跨线程同步容器；周期基准如何加锁、何时重置由上层遥测模块负责。

## 与 Go 版本的对应关系

Rust 主要镜像 [`telemetry.go`](telemetry.go)：指标名、namespace/subsystem、标签值、公开 getter、快照字段和逐字段差分都能一一对应。`snapshot!` 是 Rust 为消除重复而引入的实现手段，不改变 Go 的逐字段减法意图。

已验证的特殊等价规则包括：

- `TablePartitionUsageCounter::cal/Cal` 的 `TablePartitionMaxPartitionsCnt` 取“当前减基准”和“基准历史峰值”中的较大者。
- `ResetTablePartitionCounter` 不携带 Go 返回字面量中省略的 interval create/add/drop 与 compact 值，Rust 通过清零数组索引 10..14（不含 14）实现；reorganize 保留当前值。
- `DDLUsageCounter::sub` 不携带 `MetadataLockUsed`，固定为 false。
- `StoreBatchCoprCounter::sub` 不携带 `BatchSize`，固定为 0；getter 的 BatchSize 也固定为 0。
- Fair Locking 的 used/effective 两项都参与差分。

实现差异需要保留关注：Go 使用包级可变 collector 变量，Rust 聚合为 `TelemetryMetrics + OnceLock`；Go `InitTelemetryMetrics` 无返回值，Rust 返回 `Result<&'static TelemetryMetrics, Error>`；Go `readCounter` 可能返回 `-1`，Rust 直接 `get()`；Rust 为共享所有权指标增加“优先读 Session/Executor，未初始化再 fallback”的分支。Rust 快照结构没有 Go 的 serde/JSON tag 等价物，因此本文件只提供内存模型，不证明 JSON 输出兼容。

## 扩展指南

新增遥测指标时，最小安全改动链是：

1. 在 `TelemetryMetrics` 增加字段，并在 `TelemetryMetrics::new` 使用与 Go 完全相同的 namespace、subsystem、name、help 和标签顺序构造。
2. 若业务写入通过 Session/Executor 等模块进行，应像现有共享指标一样复用同一 collector，避免创建两个同名但状态分离的对象；同时检查 `metrics.rs::RegisterMetrics` 是否注册真正被写入的 owner collector。
3. 增加或扩展快照类型、getter 和差分逻辑。若扩展 `table_partition`，必须同步数组长度与位置映射；不要只在结构体末尾加字段。
4. 同步 Go `pkg/metrics/telemetry.go` 的真实语义。对尚未由 Go 支持的新功能，应清楚记录为 Rust 独有逻辑，而不是假称移植完成。
5. 测试必须放在独立文件：共享 owner 路径/fallback 用 [`telemetry_test.rs`](telemetry_test.rs)，快照算术、标签和构造行为用 [`metrics_2_aster_unit_test.rs`](metrics_2_aster_unit_test.rs)；不要把 `#[cfg(test)]` 测试内嵌到本生产文件。

兼容风险主要是 Prometheus 全名或标签变化导致仪表盘/采集断裂、重复注册导致初始化失败、共享 owner 与备用 collector 分叉、分区数组错位、以及 Go/Rust 差分结果不一致。性能上 getter 会创建/查找 label child 并逐项读取，新增高基数标签会放大内存和采集成本；遥测标签应继续保持固定低基数。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点、1,848,419 条边；通过 `node --file pkg/metrics/telemetry.rs` 阅读 1--682 行，通过 `query GetCTECounter` 同时定位 Rust/Go 定义，通过 `explore "pkg/metrics/telemetry.rs symbols callers callees telemetry metrics"` 核对 `init_telemetry_metrics -> metrics`、`InitTelemetryMetrics -> init_telemetry_metrics` 以及 getter/测试调用关系。
- 目标源码：[`telemetry.rs`](telemetry.rs)；crate 与模块入口：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`metrics.rs`](metrics.rs)。`pkg/metrics/doc.go` 在当前仓库不存在。
- 上游直接证据：[`../../session/metrics/metrics.rs`](../session/metrics/metrics.rs) 的遥测句柄绑定；[`../ddl/index.rs`](../ddl/index.rs) 的 add-index ingest 递增路径；RustCodeGraph 与 `rg` 对 `InitTelemetryMetrics`、`TELEMETRY_METRICS` 和各字段引用的交叉核对。
- Go 对照：[`telemetry.go`](telemetry.go) 的 `InitTelemetryMetrics`、`readCounter`、全部快照/getter 及特殊 `Cal`/`Reset`/`Sub` 规则。
- 独立测试：[`telemetry_test.rs`](telemetry_test.rs) 验证四类 getter 读取共享 Session/Executor 指标；[`metrics_2_aster_unit_test.rs`](metrics_2_aster_unit_test.rs) 验证普通差分、分区最大值规则、被省略字段归零、Fair Locking 双字段差分和 Go 标签值。
- 消费边界复核：`rg` 对全部 `Get*Counter`/`ResetTablePartitionCounter` 的 Rust 引用仅命中本文件与 `telemetry_test.rs`；`pkg/telemetry/data_feature_usage.rs` 只作为相邻架构边界阅读，未据此宣称已存在调用边。
- 本任务是纯文档分析，按任务要求未运行 Cargo。交付结构检查要求文档存在且恰有十一个固定二级标题；同时人工复核了源文件角色、运行流程、状态/并发、Go 差异、扩展点和独立测试位置。
