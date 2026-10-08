# `pkg/telemetry/data_feature_usage.rs`

## 文件定位

本文件属于 `astersql-telemetry` crate 的“功能使用量”采集层。`pkg/telemetry/lib.rs` 将它声明为 `data_feature_usage` 模块并把公开项重新导出；上游 `pkg/telemetry/data.rs::generateTelemetryData` 调用 `getFeatureUsage` 生成 `telemetryData.FeatureUsage`，随后 `pkg/telemetry/telemetry.rs::ReportUsageData` 将整份报告序列化并缓存。报告成功路径再调用 `postReportTelemetryData`，把本文件维护的增量计数基准推进到当前值。

`pkg/telemetry/Cargo.toml` 声明 crate 名为 `astersql-telemetry`，并用 `package.metadata.porting.go-package = "pkg/telemetry"` 标明 Go 来源。清单中的 TiDB 子系统依赖均位于 `target.'cfg(any())'.dependencies`，该条件恒为假；因此当前 Rust 文件并不直接链接真实 `infoschema`、`metrics`、`sessionctx` 等 crate，而是使用本 crate 在 `telemetry.rs` 中定义的 `SessionContext` 内存模型。这是已接入 Rust 遥测报告流程的实现，但不是对 Go 运行时依赖的逐接口直连。

## 核心职责

1. 定义功能计数器、采集结果和最终 `featureUsage` 载荷结构，例如 `TxnCommitCounter`、`TablePartitionUsageCounter`、`TxnUsage` 与 `featureUsage`。
2. 维护进程级 `current`/`initial` 两份 `MetricsSnapshot`，通过“当前累计值减上次上报基准”得到本报告周期的增量。
3. 从 `SessionContext` 的全局变量、schema/table 摘要及配置字段采集布尔开关、对象数量和聚簇索引统计。
4. 聚合来自同 crate 的 TTL 统计（`getTTLUsageInfo`），并通过 `featureUsage::Marshal` 生成与 Go JSON tag 对齐的 JSON 文本。
5. 提供一组 `postReport*` 钩子，在报告完成后分别刷新基准，防止累计指标被下一周期重复上报。

文件不负责调度、传输或落盘：启用判断和报告生命周期位于 `telemetry.rs`，顶层报告装配与后置清理位于 `data.rs`。

## 主要符号

- 差分计数器：`TxnCommitCounter::Sub`、`CTEUsageCounter::Sub`、`AccountLockCounter::Sub`、`NonTransactionalStmtCounter::Sub`、`MultiSchemaChangeUsageCounter::Sub`、`ExchangePartitionUsageCounter::Sub`、`IndexMergeUsageCounter::Sub`、`FairLockingUsageCounter::Sub` 均逐字段执行 `current - old`。`DDLUsageCounter::Sub` 只差分数值字段并把 `MetadataLockUsed` 清零，随后由 `getDDLUsageInfo` 从当前变量补入。`StoreBatchCoprCounter::Sub` 同样把配置型 `BatchSize` 清零后再采集。
- `TablePartitionUsageCounter::Cal`：除 `TablePartitionMaxPartitionsCnt` 外均做普通差分；最大分区数字段采用 `(current - initial).max(initial)`，与普通单调计数语义不同。
- `MetricsSnapshot`、`current()`、`initial()`：汇总所有进程级累计计数，并以两个 `OnceLock<Mutex<_>>` 单例保存当前值与报告基准。`SetMetricsSnapshot`、`ResetMetricsForTest` 是公开的测试/注入辅助入口。
- 载荷类型：`placementPolicyUsage`、`resourceControlUsage`、`NewClusterIndexUsage`、`TxnUsage` 和 `featureUsage`。`featureUsage` 是本文件的总输出，字段名称与 Go 侧遥测 JSON 契约对应。
- `featureUsage::Marshal`：手工拼装完整 JSON；`txn_json` 和 `ttl_json` 分别处理嵌套事务、TTL 结构，`json_quote` 实现字符串转义并特别转义 `<`、`>`、`&`、U+2028、U+2029。
- 主入口 `getFeatureUsage(&SessionContext) -> Result<featureUsage, TelemetryError>`：组合所有采集器，再调用 `collectFeatureUsageFromInfoschema` 填充 schema/table 相关字段。
- 采集器：`getTxnUsageInfo`、`getClusterIndexUsageInfo`、`getCTEUsageInfo`、`getAccountLockUsageInfo`、`getMultiSchemaChangeUsageInfo`、`getExchangePartitionUsageInfo`、`getTablePartitionUsageInfo`、`getNonTransactionalUsage`、`getIndexMergeUsageInfo`、`getDDLUsageInfo`、`getStoreBatchUsage` 以及各配置布尔值读取函数。
- 基准推进：私有 `snapshot_field` 是通用实现；`postReportTxnUsage` 等公开函数更新单个字段。`postReportTablePartitionUsage` 独立实现最大分区数的特殊保留规则。

文件没有 trait、条件编译项或模块级常量；所有公开 API 通过 `lib.rs` 的通配重新导出暴露给 crate 使用者。

## 执行流程

一次生产报告的路径如下：

1. `ReportUsageData` 在遥测开启时调用 `generateTelemetryData`。
2. `generateTelemetryData` 调用 `getFeatureUsage`；如果返回错误，以 `.ok()` 降级为 `FeatureUsage: None`，整份报告仍可继续生成。
3. `getFeatureUsage` 先构造默认载荷：事务、CTE、账号锁定、非事务 DML、DDL、IndexMerge、分区、TTL、Store Batch Copr 等字段分别由专用函数采集。
4. 计数型采集器各自锁住 `current` 与 `initial`，复制相应字段后计算差分；配置型采集器读取 `SessionContext.GlobalVars` 或显式字段。
5. `collectFeatureUsageFromInfoschema` 遍历 `ctx.Schemas` 及每个 schema 的 `Tables`：用 OR 汇总临时表、缓存表和 `AutoIDCache == 1`，累计库/表/分区放置策略数量，并填入资源组数量及资源管控开关。
6. `telemetryData::Marshal` 调用 `featureUsage::Marshal`，把载荷嵌入顶层 JSON。
7. `ReportUsageData` 随后调用 `postReportTelemetryData`；它依次调用本文件的全部 `postReport*` 钩子，将已上报累计值变为下一周期基准，然后保存刚生成的 JSON。

关键顺序不变量是“先生成报告，后推进基准”。若先调用后置钩子，本周期增量会变为零；若生成后未调用后置钩子，下次会重复包含同一累计值。

## 数据与状态

- `current` 表示各指标的累计当前值，`initial` 表示上次成功上报后的基准。普通差分字段允许产生负值；代码没有钳制或检测计数器回退。
- `on` 只把大小写不敏感的 `on`、`1`、`true` 视为开启；变量缺失及其他文本均为关闭。
- `getTxnUsageInfo` 同时携带实时开关（异步提交、1PC、mutation checker、RC 检查、公平锁）和累计差分（提交路径、SAVEPOINT、惰性唯一检查、公平锁使用/生效次数）。断言级别缺失时返回空字符串。
- `collectFeatureUsageFromInfoschema` 每次作用于新建的默认 `featureUsage`；计数字段做加法，布尔字段做 OR。它不会检查 `TableInfo.Public`，而是相信 `SessionContext.Schemas` 已提供适用对象集合。
- `getClusterIndexUsageInfo` 以 `ClusteredTableTypes.len()` 为总表数，只把字符串严格等于 `"CLUSTERED"` 的项计为聚簇表。
- `getGlobalMemoryControl` 依次读取 `tidb_server_memory_limit` 和兼容键 `server_memory_limit`；只有能解析成 `u64` 且大于零才启用。
- `getStoreBatchUsage` 的计数来自快照差分，而 `BatchSize` 来自当前全局变量；变量缺失或解析失败时保持 `Sub` 产生的零值。
- `featureUsage::Marshal` 固定输出全部字段，没有 `omitempty` 分支；字段顺序由格式字符串固定，但消费者应依赖 JSON 名称而非顺序。

## 依赖与调用关系

RustCodeGraph 对 `getFeatureUsage` 的节点轨迹确认其直接调用 `getClusterIndexUsageInfo`、`getTxnUsageInfo`、`collectFeatureUsageFromInfoschema`、各差分采集器和 `pkg/telemetry/ttl.rs::getTTLUsageInfo`。精确源码搜索补充了生产上游：

- `pkg/telemetry/data.rs::generateTelemetryData -> getFeatureUsage`；
- `pkg/telemetry/telemetry.rs::ReportUsageData -> generateTelemetryData -> telemetryData::Marshal -> featureUsage::Marshal`；
- `pkg/telemetry/data.rs::postReportTelemetryData -> postReportTxnUsage` 及其余基准推进函数。

本文件的直接类型/函数依赖来自 crate 根重新导出的 `SessionContext`、`TelemetryError`、`getTTLUsageInfo`、`ttlUsageCounter`，以及标准库 `Mutex`/`OnceLock`。`lib.rs` 还把本文件的公开符号反向重新导出。

RustCodeGraph 的 `callers getFeatureUsage --file ...` 返回空数组，和节点轨迹/源码搜索存在索引限制；节点轨迹能看到测试门面 `pkg/telemetry/main_test.rs::GetFeatureUsage`，而生产调用边由 `data.rs` 源码确认。因此不能仅凭 callers 空结果断言本模块未接线。

## 错误处理与边界

- `getClusterIndexUsageInfo` 在 `SessionContext.FailClusterQuery` 为真时返回 `TelemetryError("cluster index query failed")`；`getFeatureUsage` 用 `?` 立即传播，这是本文件主采集路径唯一显式可恢复错误。
- 顶层 `generateTelemetryData` 把该错误转换为 `FeatureUsage: None`，所以功能采集失败不会阻断时间戳和窗口数据报告。
- 所有指标锁大多使用 `unwrap()`，部分使用带消息的 `expect("metrics lock poisoned")`；任一持锁线程 panic 导致 mutex poisoned 后，后续采集或重置可能继续 panic，而不是返回 `TelemetryError`。
- 数字变量解析失败会静默降级：内存限制视为未开启，Store Batch Size 保持零。未知布尔文本也视为关闭。
- 手工 JSON 序列化只对字符串字段调用 `json_quote`；当前数值与布尔字段无需字符串转义。新增字符串字段若直接插入格式串，会引入非法 JSON 或转义不一致风险。
- `getFeatureUsage` 在聚簇索引查询失败时停止，不会执行随后 `collectFeatureUsageFromInfoschema`；但此前读取快照没有副作用，基准只在独立的 post-report 阶段更新。

## 并发与资源生命周期

`current` 与 `initial` 均为进程生命周期单例，首次访问时由 `OnceLock` 惰性初始化；内存由进程持有，没有显式销毁。`Mutex` 保证单次结构读取/写入的数据竞争安全，测试文件额外用 `TEST_LOCK` 串行化会修改共享快照的用例。

采集函数通常先分别锁住并复制 `current`、`initial`，随后释放锁并计算；`snapshot_field` 先复制 `current`，再锁 `initial` 写入。锁不会跨外部调用或 JSON 序列化长期持有，且常规路径的获取顺序均为 current 后 initial。两份快照不是在同一个锁下读取，因此并发更新或并发上报时不保证跨快照原子一致，也没有防止两个上报线程同时推进基准的事务边界。

`postReportTablePartitionUsage` 同时读取 current 并持有 initial 写锁，按特殊公式更新最大分区数；修改该逻辑时必须维持锁顺序，避免与其他 current→initial 路径形成反向加锁。

## 与 Go 版本的对应关系

对应源文件为 `pkg/telemetry/data_feature_usage.go`，相关 Go 测试为 `data_feature_usage_test.go`。两侧保留了相同的 `featureUsage`/`TxnUsage` 字段集合、主要采集函数名称、累计值减初始值的周期差分模型，以及上报后推进初始基准的生命周期。Rust 测试名也基本逐项对应 Go 测试：事务、临时/缓存表、AutoID、账号锁、分区与放置策略、资源组、各开关、DDL、IndexMerge、TTL、Store Batch Copr 和公平锁。

重要实现差异如下：

- Go `getClusterIndexUsageInfo` 执行 SQL 查询，`collectFeatureUsageFromInfoschema` 访问真实 domain infoschema；Rust 从 `SessionContext.ClusteredTableTypes`、`Schemas`、`PlacementPolicies`、`ResourceGroups` 读取预聚合内存数据。
- Go 计数来自 `metrics` 包的全局计数器与其他真实子系统；Rust 统一使用可注入的 `MetricsSnapshot`。因此 Rust 生产链已接通遥测装配，但指标生产者是否持续写入该快照必须由其他文件另行验证，不能从本文件推断。
- Go 使用 `encoding/json` 的 struct tags；Rust 以 `Marshal` 手工维护等价键名。Rust 独立测试 `TestFeatureUsageMarshalPreservesGoFields` 专门校验关键 tag、字段大小写及 Go 风格 HTML/U+2028/U+2029 转义。
- Go 从系统变量访问器、全局内存限制、BR 配置等来源读取状态；Rust 的 `getGlobalKillUsageInfo`、`getLogBackupUsageInfo`、`getGlobalMemoryControl` 等从 `SessionContext` 字段或 map 读取。
- Rust 的失败注入仅覆盖聚簇索引查询；Go 的真实 SQL/全局变量访问还可能产生其他错误，其中若干在 Go 代码中被忽略或降级。

## 扩展指南

新增一个遥测功能字段时，应沿完整生命周期接线，而不是只改结构体：

1. 确认该字段是实时配置、对象扫描结果还是累计计数。累计计数应加入对应 counter/`MetricsSnapshot`，实现差分语义，并在 `postReportTelemetryData` 中增加基准推进钩子。
2. 在 `featureUsage` 增加字段，并在 `getFeatureUsage` 或 `collectFeatureUsageFromInfoschema` 填充；若需要真实外部子系统，需先明确当前 `cfg(any())` 依赖边界，不能假定 Cargo 已启用该依赖。
3. 同步 `featureUsage::Marshal` 的键名、嵌套形状和字符串转义；以 Go struct tag/`encoding/json` 输出为兼容基准。
4. 在独立文件 `pkg/telemetry/data_feature_usage_test.rs` 增加或扩展测试，不要把测试内嵌进生产 `.rs`。至少覆盖默认值、开启/使用值、上报后差分重置、错误/解析失败，以及 JSON 字段契约；若 Go 版本也有对应行为，应同步核对 `data_feature_usage_test.go`。
5. 并发相关修改必须考虑 current/initial 的一致性和 current→initial 锁顺序；高频采集字段还应评估复制整个 `MetricsSnapshot` 与多次加锁的开销。

高风险位置包括 `TablePartitionMaxPartitionsCnt` 的非普通差分、DDL/Store Batch 中“差分字段 + 实时配置字段”的混合、手工 JSON 格式串，以及报告生成与 post-report 基准推进的时序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/telemetry` 确认本模块及 Rust/Go 测试均已索引。
- RustCodeGraph：`node --file pkg/telemetry/data_feature_usage.rs --offset 1 --limit 500` 与 `--offset 500 --limit 400` 阅读完整 793 行源码；`node getFeatureUsage` 验证 Rust 主入口的直接下游和测试门面调用轨迹；`callees getFeatureUsage --file ... --json` 验证各 Rust 采集器及 `ttl.rs::getTTLUsageInfo` 边。
- 已读 Rust 路径：`pkg/telemetry/data_feature_usage.rs`、`Cargo.toml`、`lib.rs`、`data.rs`、`telemetry.rs`、`data_feature_usage_test.rs`；包内不存在 `doc.go`。
- 已读 Go 对照：`pkg/telemetry/data_feature_usage.go` 与 `pkg/telemetry/data_feature_usage_test.go` 的符号/测试清单及对应实现。
- Rust 独立测试覆盖 25 个测试入口，包括 `TestFeatureUsageMarshalPreservesGoFields`、差分/基准推进、infoschema 摘要、配置边界、TTL 与 Store Batch Copr；本任务按计划为纯文档分析，未运行 Cargo。
- 结构验收以任务指定命令检查目标文档存在且恰有十一个固定二级标题；结论均限定在上述源码、调用边、Cargo 声明和测试证据范围内。
