# `pkg/session/txninfo/txn_info.rs`

## 文件定位

本文件属于 `astersql-session-txninfo` crate，crate 入口 `pkg/session/txninfo/lib.rs` 以 `pub mod txn_info` 暴露它；`pkg/session/txninfo/Cargo.toml` 通过 `[package.metadata.porting].go-package = "pkg/session/txninfo"` 明确它是 Go 包 `pkg/session/txninfo` 的迁移实现。它定义单笔活跃事务的诊断快照、向 `Datum` 的列值转换，以及事务状态 Prometheus 指标的标签选择接口。

Rust 当前已有三类直接接线：`pkg/session/sessmgr/lib.rs::txninfo` 重新导出本文件类型，`pkg/server/server.rs::ShowTxnList` 构造 `TxnInfo` 快照，`pkg/util/metricsutil/common.rs::initMetricsVars` 调用 `InitMetricsVars`。相邻的 `pkg/session/txninfo/summary.rs::TrxHistoryRecorder::OnTrxEnd` 还以 `TxnInfo` 的 `StartTS` 和 `AllSQLDigests` 生成事务历史摘要。代码搜索未发现 Rust 信息模式读取器直接调用 `TxnInfo::ToDatum`；因此，本文件保留了 Go `TIDB_TRX` 数据源的列转换契约，但不能据此断言 Rust 端已经完整接通该表的读取链路。

## 核心职责

1. 用 `TxnRunningState`、`TxnIdle` 至 `TxnRollingBack` 以及 `TxnRunningStateStrs` 固定事务状态的数值、展示文本和 Prometheus 标签语义。
2. 用 `TxnInfo`、`BlockStartTime`、`ProcessInfo` 保存事务 TSO、SQL digest、当前状态、锁等待、MemDB 条目数和会话身份等诊断数据。
3. 用 `TxnInfo::ToDatum` 和 `columnValueGetterMap` 将列名转换为 TiDB `Datum`，供与 `TIDB_TRX` 相同的表结构消费。
4. 用 `TxnDurationHistogram`、`TxnStatusEnteringCounter` 按状态选择全局指标句柄，并由 `InitMetricsVars` 触发指标向量初始化。

本文件只保存和投影快照，不负责推进事务状态、不负责同步字段，也不负责生成 `CURRENT_SQL_DIGEST_TEXT` 或 `MEM_BUFFER_BYTES`；这两个列名常量存在，但没有注册 getter，所以 `ToDatum` 对它们返回 SQL `NULL`。

## 主要符号

- `TxnRunningState = i32`：与 Go `int32` 别名对齐。合法状态索引是 `0..TxnStateCounter`；`TxnStateCounter == 5` 是数量标记，不是可展示状态。
- `TxnIdle`、`TxnRunning`、`TxnLockAcquiring`、`TxnCommitting`、`TxnRollingBack`：依次表示空闲、执行 SQL、等待悲观锁、提交和回滚。
- `TxnRunningStateStrs`：将上述状态映射为 `Idle`、`Running`、`LockWaiting`、`Committing`、`RollingBack`；`stateDatum` 使用相同索引，并将枚举数值转换为一基值。
- `metricStateLabel`：将状态映射为 `idle`、`executing_sql`、`acquiring_lock`、`committing`、`rolling_back`。它直接用 `state as usize` 索引固定数组。
- `InitMetricsVars`：访问 `metrics::TxnStatusEnteringCounterVec()` 与 `metrics::TxnDurationHistogramVec()`，触发两个全局向量的懒初始化；真正的注册和存储由 `astersql-metrics` 提供。
- `TxnDurationHistogram(state, hasLock)`：以状态标签和 `hasLock` 的 `"true"`/`"false"` 标签取得 `Histogram`。
- `TxnStatusEnteringCounter(state)`：以状态标签取得 `Counter`。
- `TxnInfo`：事务快照主体。`StartTS` 是 TSO；`CurrentSQLDigest` 是当前语句摘要；`AllSQLDigests` 是事务内摘要序列；`State`、`LastStateChangeTime`、`BlockStartTime`、`EntriesCount` 表示运行状态；`ProcessInfo` 补充连接和表信息。
- `BlockStartTime`：用 `Valid` 区分真实时间和无效占位时间；默认值为 `Valid = false`、`Time = UNIX_EPOCH`。
- `ProcessInfo`：包含 `ConnectionID`、`Username`、`CurrentDB` 和用 `HashMap<i64, ()>` 表示的 `RelatedTableIDs` 集合。
- `COLUMN_VALUE_GETTER_MAP` / `columnValueGetterMap`：用 `OnceLock<HashMap<...>>` 懒构建只读列投影表。
- `TxnInfo::ToDatum(column)`：命中 getter 时执行转换；未知列或未注册列返回 `Datum::default()`，即 SQL `NULL`。

## 执行流程

指标路径从 `pkg/util/metricsutil/common.rs::initMetricsVars` 进入 `InitMetricsVars`，先确保两个指标向量可用。事务状态变化的调用者随后调用 `TxnStatusEnteringCounter(state)` 或 `TxnDurationHistogram(state, hasLock)`：两者先经 `metricStateLabel` 取得稳定标签，再从全局指标向量获得绑定句柄，调用者负责 `inc` 或 `observe`。

事务列表路径由 `pkg/server/server.rs::ShowTxnList` 将 server 内部事务记录转换为本文件的 `TxnInfo`。该实现当前填充 `StartTS`、`CurrentSQLDigest` 和从连接进程信息得到的 `ConnectionID`、`Username`、`CurrentDB`，其余字段沿用 `Default`；因此消费者必须按快照中实际存在的数据解释结果。

列读取路径以 `TxnInfo::ToDatum(column)` 开始。`columnValueGetterMap` 首次调用时初始化映射，之后复用同一静态映射：

- `ID` 直接返回 `StartTS`；`START_TIME` 将 `StartTS >> 18` 解释为 Unix 毫秒，再转成本地时区的 MySQL `TIMESTAMP`。
- `CURRENT_SQL_DIGEST` 在空字符串时返回 `NULL`；`STATE` 返回名称与一基数值组成的 MySQL enum。
- `WAITING_START_TIME` 和 `WAITING_TIME` 先检查 `BlockStartTime.Valid`；后者计算当前系统时间与阻塞开始时间的秒差。
- `MEM_BUFFER_KEYS` 返回 `EntriesCount`。
- `SESSION_ID`、`USER`、`DB` 从可选 `ProcessInfo` 读取；缺失时分别返回 `0`、空字符串、空字符串。
- `ALL_SQL_DIGESTS` 用 `serde_json` 序列化数组；`RELATED_TABLE_IDS` 遍历集合键并用逗号连接。
- 未知列以及未注册的 `CURRENT_SQL_DIGEST_TEXT`、`MEM_BUFFER_BYTES` 返回 `NULL`。

事务结束摘要是另一条下游路径：`pkg/session/txninfo/summary.rs::TrxHistoryRecorder::OnTrxEnd` 从 `StartTS >> 18` 恢复开始时间，检查持续时间阈值，再克隆 `AllSQLDigests` 写入摘要 LRU；它不经过 `ToDatum`。

## 数据与状态

`TxnInfo::default()` 建立“尚无事务信息”的零值：`StartTS = 0`、digest 为空、`State = TxnIdle`、两个时间为 Unix epoch、`EntriesCount = 0`、`ProcessInfo = None`。这个默认值既用于缺省初始化，也被 `ShowTxnList` 的结构更新语法用于补齐未采集字段。

TSO 物理时间采用固定右移 18 位的约定；`startTime` 用 `checked_add` 处理时间构造溢出，失败时退回 Unix epoch。`mysqlTimestamp` 转换到 `chrono::Local`，加 500 纳秒后截取微秒，以近似微秒四舍五入，并生成 `TypeTimestamp`、`MaxFsp` 的 MySQL 时间值。

`RelatedTableIDs` 是无序 `HashMap`，所以逗号字符串只保证包含所有键，不保证输出顺序。`AllSQLDigests` 是有序 `Vec<String>`，JSON 输出保留语句摘要顺序；空向量输出 `[]`。`LastStateChangeTime` 在本文件内没有消费者，它是上游进行时长观测所需的快照字段。

## 依赖与调用关系

crate 依赖由 `pkg/session/txninfo/Cargo.toml` 限定：`types` 提供 `Datum`、MySQL enum 和时间构造；`parser-mysql` 提供 `TypeTimestamp`；`metrics` 与 `prometheus` 提供指标向量和句柄；`chrono` 提供本地时间拆分；`serde_json` 提供 digest 数组序列化。`chrono-tz` 在本文件中未直接使用。

已核验的主要关系如下：

- 上游初始化：`pkg/util/metricsutil/common.rs::initMetricsVars -> txn_info::InitMetricsVars`。
- 上游快照生产：`pkg/server/server.rs::ShowTxnList -> TxnInfo`；类型经 `pkg/session/sessmgr/lib.rs::txninfo` 导出，并出现在 `pkg/session/sessmgr/processinfo.rs::Manager::ShowTxnList` 返回值中。
- 下游摘要消费：`pkg/session/txninfo/summary.rs::TrxHistoryRecorder::OnTrxEnd -> TxnInfo::{StartTS, AllSQLDigests}`。
- 列转换：当前仓库的 Rust 直接调用来自 `pkg/session/txninfo/migration_aster_unit_test.rs`；代码搜索未找到生产 Rust 调用 `TxnInfo::ToDatum`。
- 公共门面：`pkg/session/sessionapi/lib.rs::txninfo` 目前只重新导出 `TxnInfo`，而 `sessmgr` 重新导出整个 `txn_info` 模块公共项。

## 错误处理与边界

本文件没有返回 `Result` 的公共 API，而是采用兼容 Go 展示层的缺省值策略：未知列、空当前 digest、无效等待时间和 JSON 序列化失败都转为 `NULL`；缺失 `ProcessInfo` 的数值/字符串列转为 `0` 或空字符串。`startTime` 的时间加法溢出回退 Unix epoch；`waitingTime` 遇到阻塞开始时间位于未来时返回负秒数，与 `SystemTime` 的双向差值语义一致。

状态输入有严格但未在类型层表达的边界。`metricStateLabel` 和 `stateDatum` 都按 `state as usize` 直接索引长度为 5 的数组；负数、`TxnStateCounter` 或更大的值会 panic。调用者必须维持 `TxnIdle <= state < TxnStateCounter`。同理，指标向量必须接受本文件给出的标签组合；不匹配由 Prometheus API 的行为决定。

`relatedTableIDs` 不排序，不能把字符串顺序作为协议或测试不变量。`CURRENT_SQL_DIGEST_TEXT` 与 `MEM_BUFFER_BYTES` 虽有公开常量，但没有 getter；调用 `ToDatum` 得到 `NULL` 是当前明确行为，而非字段已被采集。Rust 的 JSON 失败分支静默返回 `NULL`，不像 Go 版本会写警告日志。

## 并发与资源生命周期

`COLUMN_VALUE_GETTER_MAP` 使用 `OnceLock`，初始化最多发生一次，初始化后的 `HashMap` 只读并可跨线程共享；getter 本身不修改 `TxnInfo`。Prometheus 句柄由全局指标向量拥有，本文件返回可克隆句柄，不管理注册表销毁。

`TxnInfo` 的字段均为普通值，没有原子、锁或内部可变性。本文件不为并发读写提供同步保证：应把它作为已构造完成的快照共享，或由外层会话/事务所有权保证读取期间不被修改。Go 源码明确区分不可变字段和由事务线程修改的字段；Rust 移植没有把这层约束编码成锁或原子类型。`ProcessInfo` 由 `Option` 独占保存，`RelatedTableIDs` 也没有内部同步。

时间值取自 `SystemTime::now()` 或由 TSO 推导，不持有计时器、任务或通道。`waitingTime` 每次读取都会重新计算，因此同一快照的展示值会随墙钟变化；系统时钟回拨时可能为负数。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/txninfo/txn_info.go`。状态数值、两套状态文本、`TxnInfo`/`ProcessInfo` 字段、`TIDB_TRX` 列常量和绝大多数 `ToDatum` 缺省语义保持一致。Go `oracle.ExtractPhysical(StartTS)` 对应 Rust 的 `StartTS >> 18`；Go `time.Since` 对应 Rust 对 `duration_since` 正反两个分支的秒数计算；Go 的 `nil` digest slice 会先变为空数组，Rust 的 `Vec` 天然以空向量表示并序列化为 `[]`。

已确认的差异与迁移限制：

- Go `init()` 自动调用 `InitMetricsVars` 并预绑定二维句柄数组，还校验数组长度等于 `TxnStateCounter`；Rust 由全局 `pkg/util/metricsutil/common.rs::initMetricsVars` 显式触发向量初始化，访问器按需取标签句柄，没有等价的长度断言。
- Go `StateStr` 通过 `types.ParseEnumValue` 校验枚举，错误时以“不应发生”为由 panic；Rust 直接索引 `TxnRunningStateStrs` 并构造 `Enum`，非法状态同样会因越界 panic。
- Go JSON 序列化失败会记录包含 `txnStartTS` 的警告；Rust 只返回 `NULL`。
- Go `RELATED_TABLE_IDS` 与 Rust 都遍历无序 map，因此两边均不承诺 ID 顺序。
- Go 信息模式测试 `pkg/infoschema/test/clustertablestest/tables_test.go::TestTiDBTrx` 验证了完整 SQL 表链路，包括 digest 文本和来自 session 内存跟踪器的字节数；Rust 本文件既不生成 digest 文本也不存储字节数，且未检索到生产 Rust `ToDatum` 调用，不能把该 Go 端到端覆盖等同于 Rust 接线完成。

## 扩展指南

新增事务状态时，必须同步修改状态常量、`TxnStateCounter`、`TxnRunningStateStrs` 和 `metricStateLabel`，并扩展 `pkg/session/txninfo/migration_aster_unit_test.rs::metric_accessors_select_the_go_state_and_lock_labels`；还应核对 Go 对照文件与指标 label 的兼容性。状态值是数组索引和外部展示 enum，插入或重排现有值会造成兼容风险，通常只应追加。

新增或接通 `TIDB_TRX` 列时，应同时定义/复用列常量、编写单一 getter、注册到 `columnValueGetterMap`，并在独立测试 `txn_info_columns_match_go_datum_shapes_and_defaults` 中覆盖有效值、缺失值和异常边界。若实现 `MEM_BUFFER_BYTES` 或 `CURRENT_SQL_DIGEST_TEXT`，还需定位数据真正拥有者；不要在本文件凭空推导，否则会与 Go 中分别来自 session 内存跟踪器和 statements summary 的语义偏离。

若上游开始并发更新 `TxnInfo`，应在拥有者层生成不可变快照，或明确引入同步模型；不要仅在 `ToDatum` getter 内局部加锁。若需要稳定输出 `RELATED_TABLE_IDS`，排序会增加 `O(n log n)` 成本且改变现有无序协议，必须同步 Go 兼容决策和测试。

所有测试逻辑继续放在独立的 `pkg/session/txninfo/migration_aster_unit_test.rs`，不要内嵌进生产文件。改动字段或公共导出时，还要核对 `pkg/server/server.rs::ShowTxnList`、`pkg/session/sessmgr/processinfo.rs::Manager::ShowTxnList`、`pkg/session/sessionapi/lib.rs::txninfo` 和 `pkg/session/txninfo/summary.rs::TrxHistoryRecorder::OnTrxEnd`。

## 验证依据

- RustCodeGraph `status`：项目索引有效，包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/session/txninfo` 列出本 crate 的 Rust/Go 实现与独立测试。
- RustCodeGraph `node --file pkg/session/txninfo/txn_info.rs --offset 1 --limit 400`：读取目标文件全部 348 行，核对状态常量、3 个结构、指标访问器、列 getter 和 `ToDatum`。
- RustCodeGraph `query TxnInfo`、`query TxnDurationHistogram`、`query columnValueGetterMap`：核对 Rust/Go 同名定义和相邻会话入口；`callers/callees` 对 `TxnInfo::ToDatum` 未返回生产调用边，随后用精确 Rust 文本搜索确认直接调用仅在独立测试。
- RustCodeGraph `node`：读取 `pkg/session/txninfo/txn_info.go`、`pkg/session/txninfo/migration_aster_unit_test.rs`、`pkg/session/txninfo/lib.rs`、`pkg/session/txninfo/summary.rs`、`pkg/util/metricsutil/common.rs`、`pkg/server/server.rs` 和 `pkg/session/sessmgr/lib.rs` 的相关定义与调用位置。
- 配置与门面：读取 `pkg/session/txninfo/Cargo.toml`；检查 `pkg/session/sessionapi/lib.rs`、`pkg/session/sessionapi/session.rs`、`pkg/session/sessmgr/processinfo.rs` 的 re-export 和接口签名。该目录没有 `doc.go`。
- Go 行为测试：读取 `pkg/infoschema/test/clustertablestest/tables_test.go::TestTiDBTrx`，核对 `TIDB_TRX` 的列形状、锁等待、空值、digest JSON 和 digest 解码语义。
- Rust 独立测试：`pkg/session/txninfo/migration_aster_unit_test.rs::txn_info_columns_match_go_datum_shapes_and_defaults` 覆盖所有已注册 getter、两个未注册列和未知列；`metric_accessors_select_the_go_state_and_lock_labels` 覆盖计数器/直方图标签选择。依任务约束，本次纯文档分析未运行 Cargo。
