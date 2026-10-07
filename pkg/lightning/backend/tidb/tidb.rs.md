# `pkg/lightning/backend/tidb/tidb.rs`

## 文件定位

本文件实现 Lightning 的 **TiDB 逻辑导入后端**：它把解析后的 `Datum` 行编码为 `INSERT`、`INSERT IGNORE` 或 `REPLACE` SQL，再通过抽象的 `SqlExecutor` 发送给目标 TiDB，而不是像本地导入后端那样生成并导入 TiKV KV/SST。crate 入口 `pkg/lightning/backend/tidb/lib.rs` 私有声明 `mod tidb` 后用 `pub use tidb::*` 再导出本文件 API；`pkg/lightning/backend/tidb/Cargo.toml` 将该 crate 命名为 `astersql-lightning-backend-tidb`，并只直接依赖通用 `backend`、`encode`、`verification` 与 `uuid` crate。

本文件同时占据三条边界：`NewEncodingBuilder`/`tidbEncoder` 实现输入行到 SQL 行的编码，`NewTargetInfoGetter`/`targetInfoGetter` 查询目标库表元数据，`NewTiDBBackend`/`tidbBackend`/`Writer` 实现通用 `Backend` 与 `EngineWriter` 写入接口。根工作区通过 `Cargo.toml` 中的 `facade_lightning_backend_tidb` 依赖和 `pkg/lib.rs` 的 facade 再导出接纳该 crate；当前 RustCodeGraph 与仓库搜索未发现生产 Rust 调用者直接构造上述三个入口，直接行为证据主要来自同目录独立测试 `tidb_test.rs`，因此不能把 Go 侧完整应用接线表述为已经在 Rust 主链启用。

## 核心职责

1. **行编码**：`tidbEncoder::Encode` 缓存输入列到表列的排列关系，校验行宽，将各类 `Datum` 写成 SQL 字面量；启用 `LogicalImportPrepStmt` 时同时产生 `?` 占位片段和 `SqlValue` 参数。
2. **SQL 安全拼装**：`appendSQLBytes` 根据 `SQL_MODE_NO_BACKSLASH_ESCAPES` 处理引号、反斜杠及控制字符；`buildStmt` 对列名中的反引号加倍，并按重复键策略选择 SQL 动词。调用者仍须传入已经安全限定的 `tableName`，本函数不会替它引用表名。
3. **批量写入与降级**：`WriteRows` 先按 `maxChunkSize`/`maxChunkRows` 切块并批量写；可重试错误最多尝试三次，满足错误预算或错误记录条件的不可重试批错误降级到逐行写，以定位并跳过允许范围内的坏行。
4. **错误与冲突记账**：`recordDuplicateCount` 根据请求行数与受影响行数差值消耗冲突阈值；`recordRowError` 区分类型错误和重复键，记录来源路径、偏移与行文本，并可由 `persistErrorRecord` 写入错误表。
5. **目标元数据探测**：`FetchRemoteDBModels` 执行 `SHOW DATABASES`；`FetchRemoteTableModels` 分批查询 `information_schema.columns`，再用 `FetchTableAutoIDInfos` 补充 `AUTO_INCREMENT`/`AUTO_RANDOM` 信息。
6. **通用后端适配**：`Backend` 的 engine 生命周期操作在逻辑导入中是无状态空操作；`LocalWriter` 返回共享后端状态的 `Writer`，由其 `AppendRows` 直接执行 SQL。

## 主要符号

- `SqlValue`：`SqlExecutor` 的绑定值/结果值枚举，覆盖空值、有符号/无符号整数、浮点、字节和字符串。
- `SqlError` 与 `SqlExecutor`：数据库适配边界。错误显式携带 `retryable`、`duplicate` 标记；执行器提供 `execute` 与 `query`。本文件没有真实连接池实现，生产接入者必须提供线程安全的实现。
- `tidbRow` / `tidbRows`：编码后的单行及行集合。单行保留字面量片段、预编译片段、参数、源文件路径与偏移；`ClassifyAndAppend` 只追加数据行并累计 data checksum，不产生 index rows。
- `encodingBuilder` / `NewEncodingBuilder`：通用 `EncodingBuilder` 工厂；`NewEncoder` 从 `EncodingConfig` 捕获 SQL mode、列定义、路径与预编译开关。
- `tidbEncoder`：核心编码器。`columnIndices` 和 `columnCount` 在首次 `Encode` 时初始化，之后假设该编码器处理相同的列排列。
- `EncodeRowForRecord`：错误记录用诊断编码；正常时复用 `tidbEncoder`，编码失败时退回 `formatDatumsForRecord`，保证仍能生成可记录字节串。
- `targetInfoGetter` / `NewTargetInfoGetter`：实现 `TargetInfoGetter` 的远端元数据读取器；`CheckRequirements` 当前无条件成功。
- `DuplicateResolution` 与 `TiDBBackendConfig`：配置重复键策略、批大小、预编译开关、兼容旧调用者的 `errorBudget`、独立类型/冲突阈值、记录上限、错误 schema 与 task ID。
- `tidbBackend` / `NewTiDBBackend`：共享 SQL executor、配置、语句缓存、错误集合及剩余预算的后端主体。`Ignore + maxRecordRows > 0` 会把内部 SQL 策略改为 `Error`，以便批失败后逐行定位冲突，但保留外部 `onDuplicate` 用于决定记录/报错语义。
- `WriteRows`、`WriteBatchRowsToDB`、`WriteRowsToDB`、`execStmts`：写入主链；分别负责切块和重试、构造多值语句、构造逐行任务、执行与错误分类。
- `Writer`：`EngineWriter` 适配器。`IsSynced` 恒为 `true`，`Close` 返回 `None`，表明逻辑写入没有待刷新的本地 chunk 状态。
- `TableAutoIDInfo` / `FetchTableAutoIDInfos`：兼容四列和五列 `SHOW TABLE ... NEXT_ROW_ID` 结果；四列格式默认 `AUTO_INCREMENT`。

## 执行流程

典型数据路径如下：

1. 上层通过 `NewEncodingBuilder` 创建 `tidbEncoder`，并从 `EncodingConfig` 注入表列、SQL mode、来源路径和是否使用预编译语句。
2. 首次调用 `Encode` 时，编码器把 `columnPermutation` 转成“输入列下标 → 表列下标”的 `columnIndices`，计算最低必需输入列数；输入过短或超过排列上限分别返回不同的 column-count 错误。
3. `Encode` 跳过映射为负数的输入列，逐个调用 `appendSQL` 构造 `(v1,v2,...)`。若启用预编译路径，还同步构造 `(?,?,...)` 并经 `datumToSqlValue` 收集参数。产出的 `tidbRow` 携带 `path`、`offset`，供错误定位。
4. `Row::ClassifyAndAppend` 将行放入 `tidbRows` 并按字面量长度和一行记录更新 `KVChecksum`。`Backend::LocalWriter` 以配置中的 `TiDB.TableName` 创建 `Writer`，`Writer::AppendRows` 要求动态类型确实是 `tidbRows`，否则报错。
5. `WriteRows` 用 `splitIntoChunks` 按字节和行数切分。每块先走 `WriteBatchRowsToDB`，将 `buildStmt` 前缀与多行字面量或占位片段拼成一个任务。
6. 批任务由 `execStmts(batch = true)` 执行一次；若它返回可重试错误，外层 `WriteRows` 最多重做整个批次三次。不可重试错误仅在仍有类型错误预算、兼容预算、记录额度，或 Error 策略配置了错误 schema 时，降级到 `WriteRowsToDB`。
7. 逐行路径为每行建立一个 `stmtTask`，`execStmts(batch = false)` 对每行最多尝试三次。成功时用 `abs_diff(rows.len(), affected)` 消耗冲突预算；最终失败时由 `recordRowError` 分类、计数、保存内存记录，并按配置写入 `type_error_v2` 或 `conflict_records_v2`。
8. 重复键在外部策略为 `Error` 时，即使已形成记录仍返回原错误；其他策略允许在阈值内继续下一行。任何预算降为负数都会结束导入并返回阈值错误。

元数据路径独立于写入路径：`FetchRemoteTableModels` 每 32 个表名构造一次 `information_schema.columns` 查询，按小写表名聚合列；随后逐表执行 `SHOW TABLE <qualified> NEXT_ROW_ID`。某张表的 auto-ID 查询失败时只移除该表并继续，列查询或结果形状错误则终止整个调用。

## 数据与状态

`tidbEncoder` 的 `mode`、`columns`、`path` 与 `preparedStatement` 在构造后固定；`columnIndices`/`columnCount` 是首次编码建立的可变缓存。因此一个 encoder 不应跨不同 `columnPermutation` 复用。`tidbRow::Size` 只计算 `insertStmt` 字节长度，即使实际使用预编译语句，该值仍作为 chunk 大小估计和 checksum 输入；这是与现有 Go `Size` 一致的近似，而不是网络报文精确大小。

`tidbBackend` 通过 `Arc` 持有 `SqlExecutor`，通过多个 `Arc<Mutex<...>>` 共享语句缓存、错误列表、错误记录与三个剩余额度。`LocalWriter` 会重新构造一个 `tidbBackend` 值，但克隆这些 `Arc`，所以 writer 中发生的错误和预算消耗可从原 backend 观察；`TestLocalWriterSharesBackendErrorState` 明确覆盖此不变量。

`statementCache` 的键是完整 SQL 文本，容量固定为 100。当前实现只缓存同一字符串值作为存在性标记；容量满时删除 `HashMap` 遍历遇到的任意键，并非真正 LRU，也不拥有或关闭真实 prepared-statement 句柄。`ErrorRecord` 保存类别、表、路径、偏移、消息和字面量行；持久化冲突记录还受 `remainingConflictRecords` 限制，而内存 `errorRecords` 仍记录经过分类并未提前返回的记录。

## 依赖与调用关系

- 向上接口来自 `pkg/lightning/backend/backend.rs`：`Backend`、`EngineWriter`、`TargetInfoGetter`、配置/元数据类型和 `BackendError`。`Writer::AppendRows` 是通用 writer 到 `tidbBackend::WriteRows` 的直接桥接。
- 编码抽象来自 `pkg/lightning/backend/encode/encode.rs`：`EncodingBuilder`、`Encoder`、`Row`、`Rows`、`Datum`、`Column` 和 `EncodingConfig`。`tidbRow`/`tidbRows` 依靠 `Any` 做运行时下转型。
- checksum 来自 `pkg/lightning/verification`：`ClassifyAndAppend` 使用 `MakeKVChecksum(Size, 1, 0)`，明确没有 index KV 数量。
- `uuid` 只用于满足通用 engine 生命周期接口；本逻辑后端的 `OpenEngine`、`CloseEngine`、`ImportEngine`、`CleanupEngine`、`FlushEngine` 和 `FlushAllEngines` 均不使用 UUID 且返回成功。
- 下游数据库动作全部收口到 `SqlExecutor::execute/query`：编码/后端本身不打开或关闭连接。`Backend::Close` 与 `Encoder::Close` 也是空操作。
- RustCodeGraph 对 `NewTiDBBackend`、`EncodeRowForRecord`、`FetchTableAutoIDInfos` 的精确 callers/callees 查询没有给出跨文件生产调用边；仓库搜索只确认 crate 经根 facade 注册，且同目录 `tidb_test.rs` 直接调用这些入口。应用中的 Rust Lightning importer 当前存在自己的接口/桩，不能据此推断本后端已经接入完整导入流程。
- Go 对照 `pkg/lightning/backend/tidb/tidb.go` 的生产调用来自 Go 的 Lightning backend/importer 主链；这能证明设计来源，但不是 Rust 运行时接线证据。

## 错误处理与边界

- 编码阶段返回 `EncodeError`：包括列排列越界、输入列不足/过多、BIT 转 `u64` 溢出，以及 strict SQL mode 下 ASCII/UTF-8 列收到非法字节。非 strict mode 不做字符集合法性校验。
- `EncodeRowForRecord` 特意吞掉编码错误并退回原始 datum 格式，只用于尽力记录坏行；业务写入仍由 `Encode` 正常传播错误。
- `FetchRemoteDBModels` 和列查询对结果列数/类型做严格模式匹配，畸形结果返回 `BackendError`。`FetchTableAutoIDInfos` 接受非负 `Int`、`UInt` 或可解析字符串形式的 next ID，拒绝负数和其他类型。
- `FetchRemoteTableModels` 的 `FetchTableAutoIDInfos` 失败是逐表软失败；该表从结果中移除。这与列查询失败的整体硬失败不同，扩展时不要抹平两者。
- 批执行的重试分成两层：batch 模式的 `execStmts` 只尝试一次，把错误交给 `WriteRows` 重做完整 batch；逐行模式在 `execStmts` 内最多重试三次。只有带 `retryable` 标志的错误继续重试。
- `recordDuplicateCount` 使用受影响行数差的绝对值；异常地受影响行数大于请求行数也会计为冲突。阈值允许精确达到零，低于零才失败。
- `recordRowError` 先将文本错误追加到 `errors`，再检查预算；因此预算超限时内存错误文本可能已增加，而结构化 `errorRecords` 和持久化写入尚未发生。
- `persistErrorRecord` 自身失败会使导入失败。`errorSchema` 会进行反引号转义，但 `tableName` 在主写入 SQL 中直接使用，契约要求调用方提供可信、已限定的表标识。
- `Mutex::lock().unwrap()` 在锁中毒时会 panic；`Row`/`Rows` 下转型中的 `expect`/`unwrap` 也把类型配对视为内部不变量，而非可恢复的外部输入错误。

## 并发与资源生命周期

`SqlExecutor: Send + Sync` 与 `Arc` 允许 backend/writer 跨线程共享；所有可变预算和收集器都由独立 `Mutex` 保护。单个预算的减法是原子的，但错误文本、结构化记录和数据库持久化分别持锁/解锁，并不是一个跨集合或跨数据库事务；并发观察者可能短暂看到不同步状态，持久化失败也不会回滚之前的内存更新。

`WriteRows` 自身按 chunk、task 和 retry 串行执行，不创建线程或异步任务。`FetchRemoteTableModels` 虽暴露 `fetchRemoteTableModelsConcurrency = 8`，当前 Rust 实现实际只按批次串行循环；该常量没有参与调度。与之相对，Go 版本用带并发上限的 error group 并行查询各批次。

资源所有权保持“借用数据库”的语义：`Backend::Close` 不关闭 executor，`Writer::Close` 不刷新也不释放数据库资源，engine 生命周期方法均为空操作。当前语句缓存只保存 SQL 字符串，因此淘汰和 close 不涉及数据库句柄；若未来改为真实 prepared statement，必须定义并测试并发双检、淘汰关闭和 backend close 行为。

## 与 Go 版本的对应关系

Rust 文件以 `pkg/lightning/backend/tidb/tidb.go` 为直接移植基线，已保持的核心语义包括：三种 SQL 动词、`Ignore + MaxRecordRows` 内部改用 Error SQL、最多三次重试、批失败后逐行隔离、按大小/行数切块、列排列缓存、SQL 字节转义、Go 风格浮点文本、诊断编码回退、四/五列 `NEXT_ROW_ID` 兼容、auto-random 占位元数据、engine 空操作及 writer 的同步/关闭语义。`tidb_test.rs` 还覆盖了 Go Datum 分支、极端浮点、严格字符集校验、两种列数错误文本、重复键/类型错误阈值、预编译批量参数和共享错误状态。

仍需明确记录的差异与未迁移能力：

- Go `NewTiDBBackend` 接收完整 Lightning config、真实 `*sql.DB` 与 `ErrorManager`；Rust 使用更窄的 `TiDBBackendConfig` 和抽象 `SqlExecutor`，错误管理为本地简化模型。
- Go 元数据查询使用带 context 的事务与 SQL retry，批次并发上限为 8，并检查 `rows.Close/Err`；Rust 串行直接调用 executor，`Context` 参数未参与取消、deadline 或事务。
- Go prepared-statement cache 保存真实 `*sql.Stmt`、近似 LRU、并在淘汰时关闭句柄，且用读写锁做并发双检；Rust 缓存 SQL 字符串并任意淘汰，主要验证占位符/绑定值形状，不能宣称已提供同等的 prepare 性能或资源管理。
- Go 通过 MySQL 错误号识别 duplicate，通过 `common.IsRetryableError`/context 识别错误；Rust 完全信任 `SqlError` 的两个布尔标志。
- Go 有日志、redaction、failpoint 和 context-cancel 分支；Rust 当前均无对应机制。Go `FetchRemoteTableModels` 的并发常量在 Rust 中虽存在但未使用。
- Rust strict-mode 字符集拒绝逻辑是当前 Rust 的显式行为，而所读 Go `appendSQL` 中相应 cast 代码仍被注释；不能把它描述成逐行完全等价实现。
- Go 支持 `_tidb_rowid` 的额外列定义；Rust `getColumnByIndex` 只从普通列向量读取，超出范围得到 `None`，因此没有携带等价的 extra-handle 列元数据。

## 扩展指南

- 增加 `Datum` 类型或改变 SQL 文本时，应同时修改 `appendSQL`、`datumToSqlValue` 和 `formatDatumsForRecord`，并在独立的 `pkg/lightning/backend/tidb/tidb_test.rs` 添加字面量、预编译值和错误记录三条路径的测试；不要把测试内嵌进生产源文件。
- 修改列映射时优先检查 `tidbEncoder::Encode` 的首次缓存不变量、忽略列（负映射）、不足/超宽两类错误，以及 `_tidb_rowid` 与 Go `getColumnByIndex` 的差距。若允许同一 encoder 改变 permutation，必须显式重建缓存。
- 增加重复键策略或错误预算字段时，应联动 `NewTiDBBackend` 的内部 `insertDuplicate` 选择、`WriteRows` 的降级条件、`recordDuplicateCount`、`recordRowError` 与 `persistErrorRecord`；测试至少覆盖 batch 成功、retryable、不可重试降级、预算边界和持久化失败。
- 接入真实 SQL driver 时，以 `SqlExecutor` 为边界保留可测试性，并补齐 context/事务、driver 错误分类、连接生命周期和 prepared statement 关闭。不要仅把真实句柄塞入现有字符串缓存；需要有所有权清晰的 cache value 和并发淘汰协议。
- 并行化 `FetchRemoteTableModels` 时才应使用 `fetchRemoteTableModelsConcurrency`；共享结果必须避免锁粒度过大，并保持“列查询硬失败、auto-ID 单表软失败”和结果小写 key 的现有契约。
- 修改 chunk 算法时同步核对单行大于 `maxChunkSize`、`maxChunkRows == 0`、空行集及边界恰好相等的行为。当前 Rust 用 `splitRows.max(1)` 避免零行上限导致空 chunk，这一点与 Go 对配置有效性的隐含假设不同。
- 若把 crate 接入 Rust Lightning 生产主链，应新增调用层集成测试，证明 facade 导出之外确有 `NewEncodingBuilder`、`NewTargetInfoGetter` 和 `NewTiDBBackend` 的真实装配；目前单元测试不能替代这项接线证据。

## 验证依据

- 源码全貌：RustCodeGraph `node --file pkg/lightning/backend/tidb/tidb.rs` 分段读取全部 1,281 行，核对了常量、数据类型、trait 实现、编码、元数据、写入、错误处理和 auto-ID 解析。
- crate 边界：读取 `pkg/lightning/backend/tidb/Cargo.toml` 与 RustCodeGraph 中的 `pkg/lightning/backend/tidb/lib.rs`；另用仓库搜索核对根 `Cargo.toml` facade 依赖和 `pkg/lib.rs` 再导出。
- 图查询：执行 RustCodeGraph `files --filter pkg/lightning/backend/tidb`、`explore`，以及对 `NewTiDBBackend`、`WriteRows`、`EncodeRowForRecord`、`FetchTableAutoIDInfos` 的 `query`；用精确 Rust symbol ID 执行 `callers`/`callees`，未得到跨文件生产 Rust 调用边，因此本文明确标注当前接线限制。
- Go 对照：通过 RustCodeGraph 分段读取 `pkg/lightning/backend/tidb/tidb.go` 全部相关区域，核对 `NewTiDBBackend`、encoder、目标元数据、批/逐行写、错误管理、writer 与 `FetchTableAutoIDInfos`。
- Rust 独立测试：通过 RustCodeGraph 读取 `pkg/lightning/backend/tidb/tidb_test.rs` 全部 989 行；关键证据包括 `TestWriteRowsIgnoreWithRecordingUsesErrorInsert`、`TestFetchRemoteTableModelsPreservesGoMetadata`、`TestWriteRowsErrorDowngradingAll`、`TestDuplicateThresholdCountsAffectedRowDifference`、`TestTypeErrorThresholdAndRecordClassification`、`TestStrictModeChecksColumnCharset`、`TestEncodeColumnCountErrorsMatchGo`、`TestLocalWriterSharesBackendErrorState`、`TestLogicalImportBatchPrepStmt` 和 `TestWriterCloseReturnsNilFlushStatus`。
- Go 独立测试：RustCodeGraph 索引确认同目录 `tidb_test.go` 及其 `TestEncodeRowForRecord` 等符号存在；Go 实现和 Rust 独立测试已提供本任务所需的对应语义证据。本任务按计划为纯文档分析，未运行 Cargo 或代码测试。
