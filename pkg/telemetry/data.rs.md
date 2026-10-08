# `pkg/telemetry/data.rs`

## 文件定位

`data.rs` 是 `astersql-telemetry` crate 的单次遥测载荷编排层。模块由 [`pkg/telemetry/lib.rs`](lib.rs) 以 `mod data` 装入并通过 `pub use data::*` 在 crate 根重导出；真正触发它的是 [`ReportUsageData`](telemetry.rs)（`pkg/telemetry/telemetry.rs:138`）。它不负责调度、开关判断、指标累加或网络发送，而是在上报已经获准执行后，把功能使用快照和滑动窗口快照组装成一个顶层对象，并提供上报后的计数基线推进钩子。

所属 crate 由 [`pkg/telemetry/Cargo.toml`](Cargo.toml) 定义，库入口是 `lib.rs`。清单中的跨 crate 依赖全部位于 `target.'cfg(any())'.dependencies`；`cfg(any())` 恒为假，因此当前 Rust 实现并不靠这些声明参与正常构建，`data.rs` 实际只使用 crate 内重导出的 `SessionContext`、`featureUsage`、`windowData` 及采集/后置函数，以及标准库时间 API。

## 核心职责

本文件承担三个紧邻上报边界的职责：

1. `telemetryData` 定义一份报告的顶层形状：生成时刻、可缺失的功能使用信息和零到多个窗口统计。
2. `generateTelemetryData` 读取当前状态并生成快照；功能采集失败被降级为 `None`，窗口采集仍继续。
3. `postReportTelemetryData` 在调用方认定报告进入上报流程后，把各累计计数的当前值推进为下一周期的差分基线；`PostReportTelemetryDataForTest` 只暴露表分区这一项，供独立测试精确控制基线。

它是“取快照”和“推进基线”的协调者，不拥有指标存储。功能计数的 `current`/`initial` 状态位于 [`data_feature_usage.rs`](data_feature_usage.rs)，窗口队列和原子计数位于 [`data_window.rs`](data_window.rs)。

## 主要符号

- `pub struct telemetryData`（`data.rs:15`）：顶层载荷。`ReportTimestamp: i64` 是 Unix 秒；`FeatureUsage: Option<featureUsage>` 用 `None` 表示采集失败；`WindowedStats: Vec<windowData>` 保存已经按小时合并的窗口。类型派生 `Clone`、`Debug`、`Default`，但命名和字段可见性沿用 Go 风格而非惯用 Rust 风格。
- `telemetryData::Marshal(&self) -> String`（`data.rs:25`）：按固定字段顺序手工拼接 JSON。`FeatureUsage` 为 `None` 时输出 JSON `null`；每个 `windowData` 分别调用其 `Marshal`，再用逗号连接成数组。
- `generateTelemetryData(&SessionContext) -> telemetryData`（`data.rs:44`）：记录当前 Unix 秒，调用 `getFeatureUsage(ctx).ok()` 和 `getWindowData()`，始终直接返回载荷而不是 `Result`。
- `postReportTelemetryData()`（`data.rs:57`）：依次推进事务、CTE、账号锁定、多 Schema 变更、交换分区、表分区、非事务语句、SAVEPOINT、惰性悲观事务唯一性检查、DDL、Index Merge、Store Batch Coprocessor、公平锁等计数基线。
- `PostReportTelemetryDataForTest()`（`data.rs:74`）：仅调用 `postReportTablePartitionUsage`。它是测试辅助 API，不等价于完整上报后置处理。

本文件没有常量、trait、条件编译项或异步函数，也没有内嵌测试模块。

## 执行流程

正常主链由 `pkg/telemetry/telemetry.rs:138-148` 明确给出：

1. `ReportUsageData` 先调用 `IsTelemetryEnabled`；关闭时立即返回，因而不会生成快照或推进基线。
2. 开启时调用 `generateTelemetryData(ctx)`。函数先从 `SystemTime::now()` 计算 Unix 秒；随后调用 `getFeatureUsage` 汇总会话变量、Schema 摘要和计数差分，再调用 `getWindowData` 读取窗口队列。
3. `getFeatureUsage` 返回错误时，`.ok()` 将错误信息丢弃并把 `FeatureUsage` 置为 `None`；这不会阻止窗口数据生成或后续处理。
4. `ReportUsageData` 接着调用 `postReportTelemetryData`，逐项把当前计数写入差分基线。
5. 最后调用 `telemetryData::Marshal`，并把结果追加到 `telemetry.rs` 的进程内 `reports(): Mutex<Vec<String>>`。

关键顺序不变量是“先生成载荷，再推进基线，再序列化/保存”。这样当前报告读取的是旧基线到当前值之间的增量，而下一次报告从新基线开始。窗口数据只读取并合并已有子窗口；`getWindowData` 本身不清空窗口队列。

## 数据与状态

`telemetryData` 自身是一次性拥有的数据快照，没有静态状态和借用字段。`Default` 会得到时间戳 `0`、缺失的功能数据和空窗口数组；生产路径不会使用该默认值，而是显式填充三个字段。

状态来源分为两类：

- 功能使用：`getFeatureUsage`（`data_feature_usage.rs:582`）组合 `SessionContext` 中的配置/Schema 摘要与 `MetricsSnapshot` 的 `current - initial` 差分。各 `postReport*` 函数把对应的 `current` 字段复制到 `initial`；表分区最大分区数有单独的 Go 兼容合并规则（`data_feature_usage.rs:703-713`）。
- 窗口使用：`getWindowData`（`data_window.rs:264`）在窗口互斥锁下按每 60 个子窗口一组进行合并。即使不足 60 个也会形成一个输出窗口；空队列返回空数组。

序列化时顶层字段名固定为 `reportTimestamp`、`featureUsage`、`windowedStats`。顶层实现依赖两个子对象的 `Marshal` 返回合法 JSON 片段，而不是再次转义它们。窗口内置函数映射会在 `windowData::Marshal` 中按键排序，因此该部分输出确定；顶层数组保留窗口读取顺序。

## 依赖与调用关系

上游直接调用关系：

- `telemetry.rs::ReportUsageData -> generateTelemetryData`
- `telemetry.rs::ReportUsageData -> postReportTelemetryData`
- `telemetry.rs::ReportUsageData -> telemetryData::Marshal`
- `data_feature_usage_test.rs::TestTablePartition -> PostReportTelemetryDataForTest`

下游直接依赖：

- `generateTelemetryData -> getFeatureUsage`（`data_feature_usage.rs`）
- `generateTelemetryData -> getWindowData`（`data_window.rs`）
- `telemetryData::Marshal -> featureUsage::Marshal` 和 `windowData::Marshal`
- `postReportTelemetryData ->` 十三个 `postReport*`/`PostSavepointCount` 基线推进函数，均由 `data_feature_usage.rs` 提供
- `PostReportTelemetryDataForTest -> postReportTablePartitionUsage`

RustCodeGraph 的文件关系报告显示 `data.rs` 被 `telemetry.rs`、`data_feature_usage_test.rs` 和 `pkg/session/runtime/normal_ddl_test.rs` 使用；精确的源码引用搜索确认前两者存在上述符号调用，而 `normal_ddl_test.rs` 只出现无关类型的 `Marshal`，没有调用本文件符号。因此应用主链证据以 `telemetry.rs` 的明确调用为准，不把文件级近似关系误记为调用边。

## 错误处理与边界

- `generateTelemetryData` 有意吞掉 `getFeatureUsage` 的 `TelemetryError`，以 `FeatureUsage = None` 降级；顶层 JSON 对应输出 `"featureUsage":null`。当前接口不记录具体错误，也不允许调用方区分失败原因。
- 系统时间早于 Unix epoch 时，`duration_since(UNIX_EPOCH).unwrap_or_default()` 把时间戳降级为 `0`。正常系统上是 Unix 秒；极端远未来的 `u64 -> i64` 转换使用 `as`，没有溢出检查。
- `Marshal` 返回 `String` 而非 `Result`，因此没有 Go `encoding/json` 的序列化失败通道；正确性依赖子 `Marshal` 生成合法 JSON。新增字符串字段时必须在所属子序列化器中正确引用和转义，不能直接拼入。
- `getWindowData`、指标快照函数和报告缓存在互斥锁中毒时会 `expect`/`unwrap` 并 panic，而不是返回 `TelemetryError`。
- `ReportUsageData` 在基线推进后才序列化并锁定报告缓存。如果后两步 panic，基线已经前移而报告没有保存；当前实现没有事务式回滚。扩展错误路径时必须明确是否仍接受这一顺序语义。
- `postReportTelemetryData` 仅推进功能计数基线，不清除窗口队列；不要把它误当作所有遥测状态的统一 reset。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务或外部连接。`telemetryData` 在调用栈中创建，序列化后即可释放；其中的 `FeatureUsage` 和 `WindowedStats` 都是拥有所有权的快照。

并发安全由下游提供：功能指标的 `current()` 与 `initial()` 分别是 `OnceLock<Mutex<MetricsSnapshot>>`，窗口存储也是互斥保护，报告列表为 `Mutex<Vec<String>>`。但是一次完整报告并没有覆盖“采集全部字段 + 推进全部基线”的总锁：每个读取或字段推进会分别加锁，多个上报线程可能交错，因而本文件不提供跨字段原子快照或严格的恰好一次消费保证。`postReportTelemetryData` 的固定调用顺序只是单次调用内的程序顺序。

资源生命周期是进程级的：`OnceLock` 状态在首次访问时初始化并持续到进程结束；后置函数修改的是长期基线，不释放存储。测试使用 `TEST_LOCK` 和 `ResetMetricsForTest` 串行化共享状态（`data_feature_usage_test.rs:30-48`），说明涉及这些全局快照的新测试也应避免并行相互污染。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/telemetry/data.go`](data.go)：

- `telemetryData` 的三个字段及 JSON 名称一一对应。Go 的 `*featureUsage` 对应 Rust 的 `Option<featureUsage>`；Go 的 `[]*windowData` 对应 Rust 的 `Vec<windowData>`。
- 两边都使用当前 Unix 秒，都在功能采集失败时保留空值而不中止整份报告，也都继续采集窗口数据。
- Go 在采集前用 `kv.WithInternalSourceType(..., InternalTxnTelemetry)` 创建内部事务上下文，并把它传给 `getFeatureUsage`；当前 Rust `SessionContext` 是可注入数据的轻量结构，`getFeatureUsage` 不接收独立事务上下文。这是实际移植边界，不应把 Rust 文档描述成已拥有 Go 的内部事务标记。
- `postReportTelemetryData` 的十三项调用及顺序与 Go `data.go:44-58` 一致；测试辅助入口也都只重置表分区计数。
- Go 由 `encoding/json` 根据 tag 序列化，可能返回错误，并把结果写日志；Rust 使用各类型的手写 `Marshal`，返回不可失败的 `String`，再存入内存 `reports` 列表。两者的字段输出目标一致，但错误通道和最终输出介质不同。

Go 测试 `data_feature_usage_test.go:231-289` 在真实 SQL 操作间调用 `PostReportTelemetryDataForTest`，验证表分区基线切换后的增量。Rust 对应测试 `data_feature_usage_test.rs:280-349` 使用注入的 `MetricsSnapshot` 模拟相同边界，并显式串行化共享指标；它验证的是移植后的计数语义，不是 SQL 到指标累加的端到端链路。

## 扩展指南

- 新增顶层载荷字段时，同时修改 `telemetryData`、`generateTelemetryData` 和 `Marshal`，并对照 Go JSON tag 确认字段名、空值形态与顺序要求；应在独立测试文件（优先 `pkg/telemetry/telemetry_test.rs`，或新增独立的 `data_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 区域挂载）添加顶层 JSON 的精确回归测试，不能把测试内嵌进 `data.rs`。
- 新增累计型功能指标时，采集函数和对应 `postReport*` 基线推进必须成对接入；否则会在后续报告中重复累计。还需在 `data_feature_usage_test.rs` 中验证“首次采集—后置推进—再次累加”的差分行为，并对照 Go 同名测试。
- 改变上报顺序前，应同时审查 `telemetry.rs::ReportUsageData`。特别要决定序列化或保存失败时是否推进基线，以及并发报告是否需要一把覆盖整个周期的锁；这些属于兼容性和丢计数风险，而非单纯重排。
- 新增窗口字段时，修改点主要在 `data_window.rs` 的快照、合并和 `Marshal`，并同步 `data_window_test.rs`；`data.rs` 只在顶层结构形状发生变化时需要调整。
- 性能上，当前顶层序列化会为每个子对象分配字符串、收集 `Vec<String>` 后再连接。窗口数量或载荷显著增长时，可考虑单缓冲写入，但必须保持 JSON 转义、确定性键顺序和 Go 兼容输出，并用独立测试锁定行为。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph 状态：索引包含 11,467 个文件，`pkg/telemetry/data.rs` 被识别为 76 行、6 个符号；`node --file` 已读取完整目标文件。
- RustCodeGraph 精确查询：`query generateTelemetryData`、`query postReportTelemetryData`、`query PostReportTelemetryDataForTest`、`query telemetryData` 均同时定位到 Go/Rust 对照符号；`callers/callees` 对这些 Rust 限定名未返回边，故调用关系进一步以已索引文件源码和精确 `rg` 引用核验，没有推测缺失边。
- 已读生产路径：`pkg/telemetry/lib.rs`、`Cargo.toml`、`telemetry.rs`、`data_feature_usage.rs:560-733`、`data_window.rs:230-278`；包内没有 `doc.go`。
- 已读 Go 对照：`pkg/telemetry/data.go` 和 `pkg/telemetry/telemetry.go`。
- 已读独立测试：`pkg/telemetry/data_feature_usage_test.rs:280-349`、`data_feature_usage_test.go:230-289`、`telemetry_test.rs`；引用搜索未发现直接覆盖 `generateTelemetryData`、`postReportTelemetryData` 或顶层 `telemetryData::Marshal` 的 Rust 测试。`data_window_test.rs` 与 `data_feature_usage_test.rs` 分别覆盖子对象序列化/行为，但不能替代顶层组合测试。
- 人工复核结论：该文件之所以存在，是为了固定上报顶层 schema、协调两个采集域并在报告周期边界推进计数基线；安全扩展必须同时维护载荷构造、JSON 形状、后置基线和独立测试。

任务规定的结构检查应确认本文档存在且恰好包含上述 11 个固定二级标题。该任务只分析文档，按计划不运行 Cargo 或代码测试。
