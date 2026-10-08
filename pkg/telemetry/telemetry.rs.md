# `pkg/telemetry/telemetry.rs`

## 文件定位

`telemetry.rs` 是 `astersql-telemetry` crate 的上报入口与轻量运行时边界。它由 [`pkg/telemetry/lib.rs`](lib.rs) 通过 `mod telemetry` 装入，并经 `pub use telemetry::*` 把类型、常量和入口函数暴露到 crate 根。文件负责回答“是否允许上报”、承载当前 Rust 遥测采集所需的上下文摘要、触发载荷生成与上报后清理，以及保存可观测的内存报告；具体功能统计、窗口统计和 TTL 统计分别位于 `data_feature_usage.rs`、`data_window.rs` 和 `ttl.rs`。

所属 crate 由 [`pkg/telemetry/Cargo.toml`](Cargo.toml) 定义，库入口是 `lib.rs`。清单把 Go 包映射记录为 `pkg/telemetry`，但生产依赖全部放在恒假的 `target.'cfg(any())'.dependencies` 下；当前文件实际只使用标准库和同 crate 的重导出符号。`pkg/domain/Cargo.toml`、`pkg/session/Cargo.toml` 和 `pkg/distsql/Cargo.toml` 声明了对该 crate 的依赖，但 Rust 源码引用搜索没有发现这些上层 crate 调用本文件入口。因此它目前是可复用的已实现边界，不应描述成已经接入 Rust 服务器的周期调度链。

## 核心职责

本文件集中承担五项职责：

1. 用 `ReportInterval` 固定正常遥测周期为六小时。
2. 用 `TableInfo`、`SchemaInfo` 和 `SessionContext` 提供采集器所需的、可直接注入的集群与会话摘要。
3. 将进程级原子总开关与 `tidb_enable_telemetry` 会话映射组合成统一的启用判定。
4. 在启用时调用 `generateTelemetryData`，执行 `postReportTelemetryData`，并把序列化结果写入进程内报告列表。
5. 提供首次运行配置记录和轻量日志类别名。

它不负责定时循环、网络传输、真实 SQL/infoschema 查询或日志后端集成。Go 版本中的这些集成由真实 `sessionctx.Context`、`Domain.TelemetryLoop` 和 zap logger 完成；Rust 当前实现通过 `SessionContext` 字段和 `Reports()` 可观测缓冲保留核心决策与数据流。

## 主要符号

- `pub const ReportInterval: Duration`（`telemetry.rs:15`）：六小时的默认报告周期，语义对齐 Go 的 `6 * time.Hour`。
- `pub struct TelemetryError(pub String)`（`telemetry.rs:18`）：字符串错误包装，实现 `Display` 和 `std::error::Error`；当前由遥测开关读取和下游采集失败注入使用。
- `pub struct TableInfo`（`telemetry.rs:27`）：单表摘要，包含 ID、public/临时/缓存状态、表与分区放置策略、自增 ID 缓存，以及 TTL 启用状态和间隔。
- `pub struct SchemaInfo`（`telemetry.rs:49`）：Schema 级放置策略标记及其 `Tables` 列表。
- `pub struct SessionContext`（`telemetry.rs:57`）：Rust 采集边界。除全局变量映射外，还保存代价模型、分页、Schema、资源组、聚簇索引分类、TTL 样本、三类查询失败注入和日志备份开关。`Default` 将代价模型设为 1、分页设为开启，其余集合为空、计数为零、失败与日志备份开关为假。
- `global_enabled() -> &'static AtomicBool`（`telemetry.rs:105`）：私有惰性进程级开关存储，初值为 `true`。
- `SetGlobalTelemetryEnabled(bool)`（`telemetry.rs:110`）：以 `Release` 顺序写入进程总开关。
- `getTelemetryGlobalVariable(&SessionContext) -> Result<bool, TelemetryError>`（`telemetry.rs:114`）：读取固定键 `tidb_enable_telemetry`；仅大小写不敏感的 `on` 或精确的 `1` 为真，缺键报错。
- `IsTelemetryEnabled(&SessionContext) -> Result<bool, TelemetryError>`（`telemetry.rs:122`）：先以 `Acquire` 读取进程总开关；关闭时直接返回 `false`，开启时再读取会话变量。
- `reports() -> &'static Mutex<Vec<String>>` 与 `Reports() -> Vec<String>`（`telemetry.rs:129、134`）：私有惰性报告存储及其克隆快照 API。
- `ReportUsageData(&SessionContext) -> Result<(), TelemetryError>`（`telemetry.rs:138`）：单次上报主入口。
- `InitialRun(&SessionContext) -> Result<(), TelemetryError>`（`telemetry.rs:151`）：先记录配置摘要，再触发单次上报。
- `Logger() -> &'static str`（`telemetry.rs:163`）：返回固定类别名 `telemetry`，不是实际日志器对象。

本文件没有 trait、条件编译项、异步函数或内嵌测试模块；测试独立放在 `telemetry_test.rs`。

## 执行流程

启用判定按短路顺序执行：`IsTelemetryEnabled` 先读取 `global_enabled`。若进程开关为假，它不访问 `SessionContext.GlobalVars`，直接返回 `Ok(false)`；否则 `getTelemetryGlobalVariable` 查找 `tidb_enable_telemetry`，缺失时返回错误，存在时按 `on`/`1` 规则解析。

`ReportUsageData` 的主流程是：

1. 调用 `IsTelemetryEnabled(ctx)`。错误立即通过 `?` 返回；关闭则无副作用地返回 `Ok(())`。
2. 调用 `crate::generateTelemetryData(ctx)` 生成时间戳、功能使用和窗口统计快照。
3. 调用 `crate::postReportTelemetryData()` 推进各累计指标的差分基线。
4. 调用载荷的 `Marshal()`，锁定进程报告列表并追加 JSON 文本，最后返回成功。

关键顺序不变量是“先采集、再推进基线、最后序列化并保存”。因此本次载荷看到的是推进前的指标差分；但若锁中毒导致保存阶段 panic，基线已经推进，当前实现不会回滚。

`InitialRun` 先计算 `IsTelemetryEnabled(ctx)`，把包含六小时间隔和布尔启用值的配置文本写入同一报告列表，然后调用 `ReportUsageData`。开启时会再次读取启用状态并再追加一条 JSON；关闭时只留下配置行；会话变量缺失时，第一次判定已返回错误，因此不会写配置行。该函数不创建定时器，周期调度仅存在于 Go 的 `Domain.TelemetryLoop`。

## 数据与状态

`TableInfo`、`SchemaInfo` 和 `SessionContext` 都是拥有所有权的数据快照，并派生 `Clone`/`Debug`；前两者还派生 `Default`。采集函数从 `SessionContext` 读取数据，不在本文件内修改上下文。下游 `getFeatureUsage` 会遍历 Schema/表摘要，并读取 `FailClusterQuery`；TTL 采集读取表的 public/TTL 字段、删除行和延迟样本，并根据 `FailTTLDeletedQuery`、`FailTTLDelayQuery` 跳过对应直方图更新。

进程级可变状态有两处：

- `global_enabled` 是 `AtomicBool`，首次访问即取得静态实例，默认开启。
- `reports` 是 `OnceLock<Mutex<Vec<String>>>`，首次访问时分配空向量，此后持续到进程结束。`Reports()` 返回完整克隆而非借用或排空，因此读取不会消费报告，列表也没有公开清空 API。

报告列表可混合两种文本：`InitialRun` 产生的人类可读配置行，以及 `ReportUsageData` 产生的 JSON 载荷。调用方不能仅凭容器类型假设所有元素都是 JSON。列表无容量界限，长期或高频调用会持续占用内存；这与 Go 版本写日志后由日志设施管理生命周期不同。

## 依赖与调用关系

本文件的直接下游调用边为：

- `IsTelemetryEnabled -> getTelemetryGlobalVariable`
- `ReportUsageData -> IsTelemetryEnabled`
- `ReportUsageData -> generateTelemetryData`（`data.rs`）
- `ReportUsageData -> postReportTelemetryData`（`data.rs`）
- `ReportUsageData -> telemetryData::Marshal`（`data.rs`）
- `InitialRun -> IsTelemetryEnabled`
- `InitialRun -> ReportUsageData`

`generateTelemetryData` 继续调用 `getFeatureUsage` 和 `getWindowData`；`postReportTelemetryData` 推进十三组功能计数基线。`SessionContext` 被 `data_feature_usage.rs`、`ttl.rs`、相应独立测试及 `main_test.rs` 的测试包装使用，因此它虽然定义在上报入口文件中，实际也是整个 crate 的共享采集模型。

Rust 上游方面，`lib.rs` 重导出了所有公开符号；精确 Rust 引用搜索仅发现 crate 内部和测试引用，没有发现服务器启动或后台 worker 调用 `InitialRun`/`ReportUsageData`。Cargo 依赖声明不能证明运行时调用。Go 上游则由 `pkg/domain/domain.go:1699-1727` 的 `Domain.TelemetryLoop` 明确接线：启动时调用 `telemetry.InitialRun`，随后六小时 ticker 调用 `ReportUsageData`，子窗口 ticker 调用 `RotateSubWindow`，退出由 `do.exit` 控制。

## 错误处理与边界

- 会话映射缺少 `tidb_enable_telemetry` 时返回 `TelemetryError("tidb_enable_telemetry not found")`；不会采用默认值。进程总开关关闭时利用短路避免该错误。
- 解析规则精确：`ON` 的大小写变体和 `1` 为真；`true`、`yes`、空串、带空白的 `ON` 等均为假。`telemetry_test.rs` 对这些值和缺键错误有直接断言。
- `ReportUsageData` 仅传播启用判定错误。`generateTelemetryData` 会把功能采集错误降级为 `FeatureUsage = None`，手写 `Marshal` 不返回错误，因此与 Go 的 JSON marshal 错误通道不同。
- `Mutex::lock()` 使用 `expect("report lock poisoned")`。若持锁线程 panic 导致中毒，`Reports`、`ReportUsageData` 或 `InitialRun` 会 panic，而不是产生 `TelemetryError`。
- `InitialRun` 的“记录配置”和“上报数据”不是事务。配置行追加成功后，第二次启用判定仍可能受并发开关修改影响；如果后续发生错误或 panic，配置行不会撤销。
- 当前无网络发送、重试、超时、报告大小限制或持久化保证。`Ok(())` 只表示本地决策和内存追加完成，不能解释为外部遥测服务已接收。

## 并发与资源生命周期

全局开关使用 `Release` 写与 `Acquire` 读，使线程能安全观察开关值；它只同步该布尔量，本文件没有把 `SessionContext` 或整个报告周期纳入同一原子事务。调用方传入共享不可变引用，因此上下文在一次函数调用内不会由这些 API 修改。

报告列表由单个 `Mutex` 串行保护，`push` 与 `Reports` 的克隆各自是原子的临界区。锁在序列化完成后才获取，避免在生成整份载荷期间占用报告锁；但多个线程同时上报时，采集、推进指标基线和追加报告可以交错，不能保证跨指标一致快照或恰好一次消费。下游指标另有自己的互斥锁，组合操作并无覆盖全链的总锁。

本文件不启动线程、异步任务、通道、事务、ticker 或外部连接。`OnceLock` 内的开关与向量在进程生命周期内保留；报告字符串由向量拥有。由于没有清理接口，每次成功启用的上报都会永久追加一项，`InitialRun` 还会额外追加配置项。并行测试若修改全局开关或报告列表可能相互影响；新增测试应使用独立测试文件并通过串行化/状态恢复避免污染。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/telemetry/telemetry.go`](telemetry.go)：

- `ReportInterval`、`getTelemetryGlobalVariable`、`IsTelemetryEnabled`、`ReportUsageData`、`InitialRun` 和 `Logger` 的职责与大体顺序对应。
- Go 读取 `config.GetGlobalConfig().EnableTelemetry` 与真实 `GlobalVarsAccessor.GetGlobalSysVar`；Rust 用原子布尔和 `HashMap<String, String>` 模拟这两个边界。Go 的 `variable.TiDBOptOn` 与 Rust 当前测试一致地接受 `on` 大小写变体和 `1`。
- Go 的上下文是 `sessionctx.Context`，功能采集会连接真实系统状态；Rust 的 `SessionContext` 是显式字段集合和失败注入模型，不具备真实会话、infoschema 或 SQL 执行能力。
- 两边都在禁用时跳过载荷生成与后置处理，也都在生成数据后调用 `postReportTelemetryData`。Go 用 `encoding/json.Marshal`，失败可返回 error；Rust 用不可失败的手写 `Marshal`。
- Go 把 JSON 交给带 `category=telemetry` 的 zap logger；Rust 把 JSON 放入无限增长的内存向量，`Logger()` 仅返回类别字符串。两者最终输出介质和故障模型并不等价。
- Go 的 `Domain.TelemetryLoop` 负责首次运行、六小时 ticker、窗口轮转、退出与 goroutine 恢复；当前 Rust 文件没有对应调度器，且 Rust 引用搜索未发现运行时接线。

Rust 独立测试 [`pkg/telemetry/telemetry_test.rs`](telemetry_test.rs) 只覆盖全局变量解析和缺键错误，没有覆盖进程开关短路、上报禁用路径、配置/JSON 追加顺序、全局状态恢复或并发调用。Go 同目录没有 `telemetry_test.go`；Go 的实际调度依据来自 `domain.go`，而不是本包内单测。

## 扩展指南

- 修改启用语义时，应同时调整 `getTelemetryGlobalVariable`、`IsTelemetryEnabled` 和 `telemetry_test.rs`，并与 Go `variable.TiDBOptOn` 及全局配置短路顺序核对。新增用例要保存并恢复 `SetGlobalTelemetryEnabled` 的全局状态，避免污染其他测试。
- 增加采集字段时，先判断它属于 `SessionContext` 输入摘要、`TableInfo`/`SchemaInfo` 元数据，还是下游指标状态；同步修改 `data_feature_usage.rs` 或 `ttl.rs` 及同目录独立测试。不要把测试写进生产 `.rs` 文件。
- 改动报告生命周期时，应一起审查 `ReportUsageData` 与 `data.rs::postReportTelemetryData` 的顺序。把保存提前或引入可失败发送会改变“何时推进基线”，存在重复统计或丢统计兼容风险。
- 若接入 Rust 运行时调度，应在拥有服务器/domain 生命周期的模块创建 ticker 和退出机制，并调用本文件入口；不要让本文件自己拥有后台线程。还需独立测试首次运行、周期触发、关闭清理和错误重试，并确认 Cargo 依赖不再只是声明。
- 若内存列表继续用于生产，至少需要界限、清理/消费语义和并发测试；若改接日志或网络 sink，应让 `Logger`/上报返回值反映真实后端错误。迁移时需保留 Go 的 category、JSON 字段和禁用短路行为。
- 性能风险集中在 `Reports()` 的全量克隆、无界字符串累积，以及并发报告的重复载荷生成。优化时不能牺牲调用者可观察顺序或在持有报告锁时执行昂贵采集。

## 验证依据

本说明依据以下可复核材料：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/telemetry` 确认目标和 Go/Rust 对照文件均已索引。
- RustCodeGraph `node --file pkg/telemetry/telemetry.rs --offset 1 --limit 400`：读取完整 165 行目标文件，并识别 21 个符号；`query` 分别定位了 `ReportUsageData`、`InitialRun`、`IsTelemetryEnabled`、`getTelemetryGlobalVariable` 及其 Go 对照，另定位了 `generateTelemetryData`、`postReportTelemetryData` 的 Go/Rust 实现。
- RustCodeGraph 的文件级关系报告称目标被 6 个文件使用；精确 `callers/callees` 查询两轮均在 30 秒内无输出而超时，因此没有把该近似关系直接当作调用边，转而用精确源码引用搜索核验上下游。
- 已读 Rust 生产文件：`pkg/telemetry/telemetry.rs`、`lib.rs`、`Cargo.toml`、`data.rs:1-76`、`data_feature_usage.rs:500-610`、`ttl.rs:120-160`；包内不存在 `doc.go`。
- 已读独立 Rust 测试：`pkg/telemetry/telemetry_test.rs` 和 `main_test.rs`。前者直接证明变量解析边界；后者只是其他采集函数的测试包装，没有覆盖本文件上报入口。
- 已读 Go 对照与入口：`pkg/telemetry/telemetry.go`、`main_test.go`、`pkg/domain/domain.go:1699-1727`。Go 调度链为 `Domain.TelemetryLoop -> InitialRun/ReportUsageData`，Rust 侧未发现等价调用。
- 已检查 Rust/Cargo 引用：`pkg/domain`、`pkg/session`、`pkg/distsql` 声明依赖该 crate，但 Rust 源码只出现目标文件内部和测试侧入口引用，故文档明确标记当前运行时未接线。

人工复核结论：该文件存在的目的，是为遥测提供统一的启用门、采集输入模型和单次报告生命周期；安全扩展必须同时维护开关短路、采集/基线推进顺序、共享状态并发语义、Go 兼容输出和独立测试。该任务是纯文档分析，按计划不运行 Cargo 或代码测试。
