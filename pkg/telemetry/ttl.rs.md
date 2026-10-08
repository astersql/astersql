# `pkg/telemetry/ttl.rs`

## 文件定位

[`ttl.rs`](ttl.rs) 属于 `astersql-telemetry` crate 的 TTL（Time To Live，生存时间）功能用量采集层。模块由 [`lib.rs`](lib.rs) 以 `mod ttl` 装配并通过 `pub use ttl::*` 重导出；上游 [`getFeatureUsage`](data_feature_usage.rs) 调用 `getTTLUsageInfo`，把结果放入 `featureUsage::TTLUsage`，随后 `featureUsage::ttl_json` 将它编码为遥测 JSON。该文件只读取遥测视角的 [`SessionContext`](telemetry.rs) 摘要和进程级开关，不执行 TTL 删除任务，也不改变表元数据。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名是 `astersql-telemetry`，库入口是 `lib.rs`。清单中的依赖全部位于 `target.'cfg(any())'`，因此当前 `ttl.rs` 的可编译实现仅使用标准库以及 crate 内的 `SessionContext`；Go 版本依赖的 infoschema、sessionctx、SQL executor 等真实组件尚未接入这条 Rust 路径。

## 核心职责

该文件承担四项职责：保存进程级 TTL 作业开关 `TTL_JOB_ENABLED`；定义遥测输出所需的 `ttlHistItem` 和 `ttlUsageCounter`；把删除行数及延迟小时数归入固定直方图桶；由 `getTTLUsageInfo` 汇总 public 且配置了 TTL 的表数量、开关状态和历史样本。

两条 SQL 常量 `selectDeletedRowsOneDaySQL`、`selectDelaySQL` 保留了 Go 版查询 `mysql.tidb_ttl_job_history` 的过滤及分组文本，但当前 Rust 函数不会执行它们。实际输入来自 `SessionContext::TTLDeletedRows` 和 `SessionContext::TTLDelayHours`，因此它们目前主要用于移植语义留存和 [`TestTTLQueriesMatchGoFilters`](data_feature_usage_test.rs) 的文本一致性检查。

## 主要符号

- `selectDeletedRowsOneDaySQL: &str`：按 `parent_table_id` 聚合昨日已完成窗口内的删除行数；排除 `running`，并把扫描窗口限制在最近七天。
- `selectDelaySQL: &str`：对最近七天内成功且没有 `scan_task_err` 的历史，先按物理表和父表取最大 `ttl_expire`，再按父表计算距当天零点的分钟数。当前仅保存查询契约。
- `TTL_JOB_ENABLED: AtomicBool` 与 `SetTTLJobEnabled(bool)`：保存和更新进程级 TTL 作业开关。写使用 `Release`，采集读取使用 `Acquire`。
- `ttlHistItem { LessThan, LessThanMax, Count }`：一个直方图桶。`LessThan = Some(n)` 表示严格小于上界 `n`；末桶以 `LessThanMax = true` 表示无穷上界。字段名沿用 Go 迁移命名。
- `ttlUsageCounter`：完整 TTL 遥测快照，包含全局作业开关、TTL 表总数、表级启用数、直方图日期以及两组直方图。
- `int64Pointer(i64) -> Option<i64>`：Go `*int64` 辅助函数的形态映射，返回 `Some(value)`；当前生产初始化直接构造 `Some`，没有调用该函数。
- `ttlUsageCounter::UpdateTableHistWithDeleteRows(i64)`：按顺序找到首个匹配桶并将计数加一。
- `ttlUsageCounter::UpdateTableHistWithDelayTime(i32, i64)`：按顺序找到首个匹配桶并增加指定表数；用于单表历史，也用于一次性记录全部无历史表。
- `yesterday() -> String`：根据 Unix 秒数计算前一 UTC 日序，再用 civil-date 算法格式化为 `YYYY-MM-DD`。
- `getTTLUsageInfo(&SessionContext) -> ttlUsageCounter`：本文件主入口，创建固定桶、统计表、消费注入样本并返回快照。

## 执行流程

1. [`getFeatureUsage`](data_feature_usage.rs) 在构造 `featureUsage` 时调用 `getTTLUsageInfo(ctx)`。
2. `getTTLUsageInfo` 以 `Acquire` 读取 `TTL_JOB_ENABLED`，调用 `yesterday` 生成日期，并初始化两组各五个桶：删除行数上界为 `10_000 / 100_000 / 1_000_000 / 10_000_000 / +∞`，延迟小时上界为 `1 / 6 / 24 / 72 / +∞`。
3. 函数遍历 `ctx.Schemas[*].Tables`。只有 `Public == true` 且 `TTLEnabled.is_some()` 的表计入 `TTLTables` 和候选 ID 集合；其中 `TTLEnabled == Some(true)` 的表另计入 `TTLJobEnabledTables`。`Some(false)` 仍是配置了 TTL 的表，只是不计入启用数。
4. 若 `FailTTLDeletedQuery == false`，函数逐个读取 `TTLDeletedRows`，由 `UpdateTableHistWithDeleteRows` 将每个样本放入首个满足 `rows < LessThan` 的桶；等于边界的值进入下一个桶。
5. 若 `FailTTLDelayQuery == false`，函数仅接纳表 ID 仍在 TTL 候选集合中的 `TTLDelayHours` 项，为每个有历史的表按小时数加一，同时记录其 ID。候选表中没有有效历史的数量统一以 `i64::MAX` 投入 `+∞` 桶。
6. 返回的 `ttlUsageCounter` 被 `featureUsage::ttl_json` 按 Go JSON 字段名编码，并经 `generateTelemetryData` 进入最终遥测载荷。

## 数据与状态

`TTL_JOB_ENABLED` 是进程级共享状态，默认 `false`。其余状态都在一次 `getTTLUsageInfo` 调用内局部构造：`HashSet<i64>` 对 TTL 表 ID 去重，另一集合记录有延迟历史的 ID；计数器按值返回，不在多次采集间累积。

直方图依赖两个不变量：桶必须按上界升序排列，且最后存在 `LessThanMax` 桶。更新方法遇到首个匹配项立即返回；如果调用者构造的桶既无匹配上界也无末桶，样本会静默丢失。当前主入口固定构造合法桶。重复的 `TTLDelayHours` 表 ID 会对直方图重复计数，但无历史表计算仍只按 ID 去重；`TTLDeletedRows` 则没有表 ID，函数无法过滤已删除或非 TTL 表的注入样本。

`TTLHistDate` 的计算基于 `SystemTime` 距 Unix epoch 的整日数，即 UTC 日界；Go 版本用 `time.Now().Add(-24h)` 的本地时区日期。接近时区或夏令时边界时二者可能不同。系统时间早于 epoch 或不足一天时，`duration_since` 的错误会被转成零时长，但随后的无符号 `- 1` 存在下溢风险；正常现代系统时间不触发该边界。

## 依赖与调用关系

上游主链为 `ReportUsageData`（[`telemetry.rs`](telemetry.rs)）→ `generateTelemetryData`（[`data.rs`](data.rs)）→ `getFeatureUsage`（[`data_feature_usage.rs`](data_feature_usage.rs)）→ `getTTLUsageInfo`。RustCodeGraph 将 `getTTLUsageInfo` 的直接下游识别为 `yesterday`、`UpdateTableHistWithDeleteRows` 和 `UpdateTableHistWithDelayTime`；精确源码引用还确认 `getFeatureUsage` 是生产调用者。

数据依赖来自 [`telemetry.rs`](telemetry.rs) 的 `SessionContext`、`SchemaInfo` 和 `TableInfo`。输出消费者是 `featureUsage::ttl_json`，它把 `Option<i64>` 映射成可省略的 `less_than`，仅在末桶输出 `less_than_max: true`。`lib.rs` 的公开重导出还使 `SetTTLJobEnabled`、计数类型和 SQL 常量可在 crate 外可见，尽管 Rust 命名保留了 Go 风格并通过 crate 级 lint allowance 接受。

当前实现没有调用 `Cargo.toml` 所列的 infoschema、meta-model、sessionctx 或 sqlexec 依赖；这些依赖声明处于永不成立的 `cfg(any())`，不能作为真实运行接线的证据。

## 错误处理与边界

`getTTLUsageInfo` 返回值而非 `Result`。它用 `FailTTLDeletedQuery` 和 `FailTTLDelayQuery` 模拟 Go 侧受限 SQL 查询失败：删除行查询失败时保留全零删除直方图；延迟查询失败时保留全零延迟直方图，并且不会把无历史表记入末桶。函数仍返回表计数和全局开关，不把部分采集失败升级成整份遥测失败。这对应 Go 版记录警告后继续返回部分结果的总体策略，但 Rust 当前不会记录日志。

表过滤要求同时满足 public 和已配置 TTL；非 public、`TTLEnabled == None` 的表完全忽略，`Some(false)` 只影响启用表计数。延迟输入中未知、已删除或已不再配置 TTL 的表 ID 被忽略。负数样本没有显式拒绝，会进入第一个桶；这依赖上游保证删除行数和延迟语义有效。两项 `FailTTL*` 分支目前没有独立 Rust 测试直接覆盖。

## 并发与资源生命周期

唯一跨调用共享的资源是 `AtomicBool`。`SetTTLJobEnabled` 的 `Release` 与采集端 `Acquire` 建立可见性顺序，读写无需锁；它不提供跨多个字段的事务快照，因为此文件也没有其他共享字段。测试通过 `data_feature_usage_test.rs::TEST_LOCK` 串行化会修改全局开关和指标快照的用例，并由 `fresh_ctx` 把开关恢复为 `false`。

每次采集都会重新分配两个五元素 `Vec` 和两个 `HashSet`，生命周期止于返回的计数器及函数局部变量。函数不启动任务、不持有锁、不打开连接，也不执行 SQL；复杂度主要是扫描所有 schema/table 以及两组注入样本，期望时间为 `O(表数 + 删除样本数 + 延迟样本数)`，集合空间为 `O(TTL 表数)`。

## 与 Go 版本的对应关系

Rust 的类型、桶边界、严格小于比较、public/TTL 表计数、无历史表投入最大桶以及两条 SQL 文本均直接对应 [`ttl.go`](ttl.go)。[`data_feature_usage_test.rs::TestTTLTelemetry`](data_feature_usage_test.rs) 复现了 Go [`data_feature_usage_test.go::TestTTLTelemetry`](data_feature_usage_test.go) 的主要断言：一个、两个及三个 TTL 表的计数，删除行和延迟落桶，以及关闭 TTL 的表仍计入 TTL 表总数。`TestTTLQueriesMatchGoFilters` 额外锁定查询字符串中的窗口、状态、JSON 错误过滤和分组条件。

关键差异是 Go `getTTLUsageInfo` 从真实 infoschema 枚举表，并用 `sqlexec.RestrictedSQLExecutor` 执行两条查询；对每个表还根据 TTL interval 动态执行 SQL，把分钟延迟换算成相对过期区间的小时数。Rust 只消费 `SessionContext` 中预先算好的 `TTLDeletedRows` 和 `(table_id, hours)`，`TTLIntervalHours` 也未使用。因此 Rust 目前是可测试的遥测数据模型与聚合实现，不等价于 Go 的生产数据库采集接线。另一个差异是 Go 的日期基于本地时间减 24 小时，而 Rust 使用 UTC 日序减一。

## 扩展指南

若接入真实 Rust TTL 遥测，最可能修改的是 `getTTLUsageInfo`：应把 infoschema 枚举、受限 SQL 执行、每表 interval 换算及警告记录接到现有计数器上，同时保持两条查询的过滤契约和“失败返回部分统计”语义。此工作还需调整 `Cargo.toml` 中目前受 `cfg(any())` 屏蔽的依赖，而不能仅让常量看似被使用。需要同步扩展独立文件 [`data_feature_usage_test.rs`](data_feature_usage_test.rs)，不要把测试内嵌进 `ttl.rs`；至少覆盖查询失败、未知表、边界值恰等于桶上界、重复表历史、非 public 表、日期时区和 interval 换算。

若只新增直方图桶，必须保持升序、严格小于语义和最终 `LessThanMax` 桶，并同步 `featureUsage::ttl_json` 的兼容性断言及 Go 对照测试。改变 JSON 字段、桶边界或日期含义会破坏遥测后端的序列兼容性；引入真实 infoschema/SQL 后还要评估全表扫描、逐表查询造成的性能开销以及采集期间 schema 变化的一致性。全局开关若扩展成多字段配置，应避免用多个独立原子产生混合快照。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/telemetry/ttl.rs`；`files --filter pkg/telemetry` 确认模块及独立测试；`explore 'pkg/telemetry/ttl.rs TTLUsagePayload collect_ttl_usage'` 给出本文件 9 个符号及调用范围；`callees getTTLUsageInfo --file pkg/telemetry/ttl.rs` 确认三个直接下游方法。部分 `callers` 查询未返回边，故上游以精确源码引用交叉验证。
- 生产源码：[`ttl.rs`](ttl.rs)（全部符号与聚合流程）、[`data_feature_usage.rs`](data_feature_usage.rs)（`getFeatureUsage` 调用和 JSON 消费）、[`telemetry.rs`](telemetry.rs)（输入模型及报告入口）、[`data.rs`](data.rs)（载荷接线）、[`lib.rs`](lib.rs)（模块装配和重导出）。`pkg/telemetry` 不存在 `doc.go`。
- crate 配置：[`Cargo.toml`](Cargo.toml)（包名、库入口、移植元数据及 `cfg(any())` 依赖边界）。
- Go 对照：[`ttl.go`](ttl.go)（真实 infoschema/SQL 算法、错误降级和桶定义）、[`data_feature_usage_test.go::TestTTLTelemetry`](data_feature_usage_test.go)（SQL 历史过滤、表计数和桶期望）。
- Rust 测试：[`data_feature_usage_test.rs::TestTTLTelemetry`](data_feature_usage_test.rs)（当前注入模型的聚合行为）和 `TestTTLQueriesMatchGoFilters`（SQL 文本契约）。没有同名 `ttl_test.rs`，也未发现对两个 `FailTTL*` 分支的直接测试。
- 本任务为纯文档分析，按计划不运行 Cargo 或代码测试；交付验证仅检查固定十一节结构、路径链接和工作区变更范围。
