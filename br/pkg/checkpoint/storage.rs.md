# `br/pkg/checkpoint/storage.rs`

源文件：[`storage.rs`](./storage.rs)

## 文件定位

本文件属于 Cargo 库 `astersql-br-pkg-checkpoint` 的 SQL 表检查点后端。`br/pkg/checkpoint/Cargo.toml` 以 `lib.rs` 为 crate 根，`lib.rs` 通过 `pub mod storage` 纳入本文件并用 `pub use storage::*` 展平公开项。它位于 `CheckpointRunner` 与 TiDB 内部 SQL 会话之间：runner 把 data/checksum 字节交给 `tableCheckpointStorage`，恢复管理器则通过本文件的查询和 meta 辅助函数重新装载状态。

该文件不是通用 KV `Storage` 实现，也不是对象存储后端；对象存储路径、文件命名和锁协议在 `external_storage.rs`。表后端以临时数据库中的 `cpt_data`、`cpt_checksum`、`cpt_metadata`、`cpt_progress`、`cpt_ingest` 五类表保存检查点，其中前两类按 UUID 分组，后三类按单一 segment 序列保存。生产接线主要在 `manager.rs::TableMetaManager`，`MemSession`/`MemRestricted` 仅是 Rust 对等测试使用的内存 SQL 替身。

## 核心职责

1. 定义三类临时检查点数据库名前缀、五张表的名称以及创建、写入、读取所用 SQL 模板；`IsCheckpointDB` 用前缀识别 BR 临时库及带 restore ID 的后缀变体。
2. 以 `CheckpointIdMapBlockSize = 524288` 为上限切分字节流。data/checksum 的一次 flush 共享一个随机 UUID，并以从 0 连续增长的 `segment_id` 写入；meta/progress/ingest 没有 UUID，只用 `segment_id` 作为主键。
3. 由 `tableCheckpointStorage` 实现 `checkpointStorage` trait，把 runner 的 data/checksum flush 转成逐段 `REPLACE INTO`，并在关闭时接管和释放 `Session`。
4. 读取 data/checksum 时按 `uuid, segment_id` 排序合并；任何 UUID 组内出现 segment 空洞就丢弃整组，避免把不完整 payload 交给 `parseCheckpointData` 或 `parseCheckpointChecksum`。
5. 为任务元数据提供 JSON 序列化、分片写入、连续性校验和反序列化，并在删除指定表后仅于数据库确实为空时删除临时库。
6. 提供 `MemSession` 与 `MemRestricted`，让独立测试在不连接真实 TiDB 的情况下验证表路径；它们不是生产 SQL 引擎，也不完整模拟 DDL、唯一键或会话关闭语义。

## 主要符号

- 数据库与表常量：`LogRestoreCheckpointDatabaseName`、`SnapshotRestoreCheckpointDatabaseName`、`CustomSSTRestoreCheckpointDatabaseName` 是 `IsCheckpointDB` 的三个前缀；`checkpointDataTableName`、`checkpointChecksumTableName`、`checkpointMetaTableName`、`checkpointProgressTableName`、`checkpointIngestTableName` 分别标识数据、校验和、任务元数据、恢复阶段与 ingest 修复 SQL。
- SQL 模板：`createCheckpointTable`/`insertCheckpointSQLTemplate`/`selectCheckpointSQLTemplate` 面向带 `(uuid, segment_id)` 主键的 data/checksum 表；`createCheckpointMetaTable`/`insertCheckpointMetaSQLTemplate`/`selectCheckpointMetaSQLTemplate` 面向只以 `segment_id` 为主键的 meta 类表。`%n` 和 `%?` 由 `Session`/`RestrictedSQLExecutor` 边界解释，库表名不是通过字符串拼接进入 DDL 参数。
- `IsCheckpointDB(dbname: &str) -> bool`：只做大小写敏感的 `starts_with` 判断；它会接受固定前缀后任意后缀，也不会查询 InfoSchema 验证数据库是否真实存在。
- `chunkInsertCheckpointData(data, fn_)`：按 524288 字节切片并同步回调 `(segment_id, chunk)`；空输入不会调用回调，回调首个错误立即返回，后续分片不再处理。
- `chunkInsertCheckpointSQLs(dbName, tableName, data)`：生成每段 SQL 与参数列表；一次调用只生成一个 `Uuid::new_v4()`，因此同一 payload 的所有段属于同一组。返回值不携带错误，因为内部回调固定返回成功。
- `tableCheckpointStorage { se, checkpointDBName }`：`se` 是 `Mutex<Option<Box<dyn Session>>>`。`new` 接管 session；`flushCheckpointData` 和 `flushCheckpointChecksum` 顺序执行分片 SQL；`close` 取出并关闭 session；`initialLock`/`updateLock` 明确 `panic!("unimplement!")`。
- `mergeSelectCheckpoint`：data/checksum 的核心重组状态机，状态为 `lastUUID`、`lastUUIDInvalid`、`nextSegmentID` 和当前 `rowData`。它忽略空 UUID 行，只提交 segment 从 0 连续出现且最终非空的组。
- `selectCheckpointData<K,V,F>`：调用 `mergeSelectCheckpoint(cpt_data)`，逐个完整 payload 交给 `parseCheckpointData`，通过回调吐出键值并累加历史执行耗时。
- `selectCheckpointChecksum`：调用 `mergeSelectCheckpoint(cpt_checksum)`，再用 `parseCheckpointChecksum` 合入 `HashMap<i64, ChecksumItem>`；相同 table ID 的后续解析结果按底层解析函数语义覆盖。
- `initCheckpointTable`：幂等创建数据库，再为调用方给出的 data/checksum 类表名逐一执行 `createCheckpointTable`。
- `insertCheckpointMeta<T: Serialize>` 与 `selectCheckpointMeta<T: DeserializeOwned>`：前者 JSON 序列化后建表并分段 `REPLACE`；后者要求查询结果非空且枚举位置与 `segment_id` 完全一致，再拼接 JSON 并写回调用方提供的 `meta`。
- `dropCheckpointTables`：先逐表 `DROP TABLE IF EXISTS`，再通过 `Domain::InfoSchema().SchemaTableInfos` 检查剩余表；只有空库才执行 `DROP DATABASE`。
- `MemSession`/`MemRestricted`：共享 `Arc<Mutex<HashMap<String, Vec<SqlRow>>>>`。前者识别有限的 create/drop/replace 形式，后者按 `db.table` 返回行副本；二者的可见性和注释均表明其用途是 parity tests。

## 执行流程

表后端的写入主链如下：

1. `manager.rs::TableMetaManager::StartCheckpointRunner` 从 `runnerSe` 中取走一个 session，以 `tableCheckpointStorage::new` 包装，并把它交给 `newCheckpointRunner`；表后端的 lock tick 传入 `Duration::ZERO`。
2. runner flush data 或 checksum 时调用 `checkpointStorage::flushCheckpointData`/`flushCheckpointChecksum`。函数先由 `chunkInsertCheckpointSQLs` 产生同 UUID 的连续分片，再持有 session mutex，逐条执行 `REPLACE INTO <db>.<table>`。
3. 保存任务元数据时，`TableMetaManager::SaveCheckpointMetadata` 先调用 `initCheckpointTable` 创建库以及 data/checksum 表，然后调用 `insertCheckpointMeta` 创建 `cpt_metadata` 并写入 JSON 分片。日志恢复的 progress 和 ingest repair SQL 也复用 `insertCheckpointMeta`，分别写入 `cpt_progress` 与 `cpt_ingest`。
4. 读取 data/checksum 时，管理器取得 `RestrictedSQLExecutor` 并调用 `selectCheckpointData`/`selectCheckpointChecksum`。`mergeSelectCheckpoint` 依赖 SQL 的 `ORDER BY uuid, segment_id`，在 UUID 切换时提交上一完整组，遇到不连续 segment 后标记该 UUID 无效并跳过其余行。
5. 读取 meta 类表时，`selectCheckpointMeta` 取得所有行，要求非空并按返回次序验证 segment 为 `0..n-1`，拼接后用 `serde_json::from_slice` 反序列化。这里的 SQL 模板没有显式 `ORDER BY`，所以生产 SQL 执行边界必须维持 Go 实现依赖的 segment 顺序；若返回顺序不同会被判为“不完整”。
6. 清理时，快照与日志 `TableMetaManager::RemoveCheckpointData` 都要求删除五张检查点表。`dropCheckpointTables` 随后检查库中是否还有表；存在任何剩余表就保留数据库，否则删除数据库。

## 数据与状态

data/checksum 表每行包含 32 字节 UUID、`u64` 语义的 `segment_id`、最多 524288 字节的 BLOB 及更新时间。一次 flush 的 UUID 是逻辑 payload 标识，不是任务 ID；多个 flush 可以并存于同一张表，读取时以 UUID 排序分组。`REPLACE` 配合 `(uuid, segment_id)` 主键使同组同段可覆盖，但每次 `chunkInsertCheckpointSQLs` 都生成新 UUID，所以重复 flush 通常形成新组而不是覆盖旧组。

meta/progress/ingest 表只有 `segment_id`、BLOB 和更新时间，主键是 segment ID。再次写入较短 JSON 时，`insertCheckpointMeta` 只覆盖新 payload 涉及的 segment，没有主动删除旧 payload 的高编号尾段；这是与 Go 相同的当前行为，扩展写入协议时必须考虑旧尾段导致 JSON 拼接异常的兼容风险。

`mergeSelectCheckpoint` 的完整性粒度是 UUID 组：空 UUID 行被忽略，首段不是 0、编号跳跃或乱序都会使该组失效；已完整的其他 UUID 组仍可返回。空 data payload不会生成 SQL，也不会成为读取结果。meta 的完整性更严格：空结果和任意 segment 不连续都直接返回错误，不会部分恢复。

`tableCheckpointStorage` 的唯一可变资源状态是 mutex 内的可选 session。`close` 将其从 `Some` 变为 `None`；之后 flush 返回 `Error("session closed")`。`MemSession.closed` 只记录 `Close` 已调用，当前 `ExecuteInternal` 不读取该标志，故不能用它证明生产 session 的关闭后行为。

## 依赖与调用关系

- crate 边界：`br/pkg/checkpoint/Cargo.toml` 声明本库为 `astersql-br-pkg-checkpoint`，本文件直接使用其依赖中的 `serde`、`serde_json` 和启用 `v4` feature 的 `uuid`；标准库依赖为 `HashMap`、`Arc`、`Mutex`、`Duration`。
- 下游内部依赖：`checkpoint.rs` 提供 `checkpointStorage` trait、`KeyType`、`ValueType`、`ChecksumItem`、`parseCheckpointData` 和 `parseCheckpointChecksum`；`stubs.rs` 提供 `Context`、`Session`、`RestrictedSQLExecutor`、`Domain`、`SqlRow`、`SqlValue`、`Error` 与 `Result`。
- 主要上游是 `manager.rs::TableMetaManager`。快照和日志两套 trait 实现都调用 `selectCheckpointData`、`selectCheckpointChecksum`、`selectCheckpointMeta`、`initCheckpointTable`、`insertCheckpointMeta` 和 `dropCheckpointTables`，并在启动 runner 时构造 `tableCheckpointStorage`。日志侧额外用 meta 辅助函数处理 progress 与 ingest repair SQL。
- `log_restore.rs::newTableCheckpointStorage` 是公开构造包装，直接转调 `tableCheckpointStorage::new`。`lib.rs` 又把本文件公开项提升到 crate 根。
- RustCodeGraph 的精确流证据包括 `selectCheckpointData -> mergeSelectCheckpoint` 和 `selectCheckpointChecksum -> mergeSelectCheckpoint`；文件级索引还显示 `storage.rs` 被 `checkpoint_test.rs`、`log_restore.rs`、`storage_test.rs` 使用。对泛型和 trait 动态调用，图结果不完整，因此管理器调用点以 `manager.rs` 源码交叉核对。

## 错误处理与边界

所有 SQL、JSON 和解析错误都通过 crate 的 `Result` 向上传播。`mergeSelectCheckpoint` 和 `selectCheckpointMeta` 会用数据库与表名注释受限 SQL 查询错误；建库、建表、写段和删表在首个错误处停止。`chunkInsertCheckpointData` 同样在回调首错处停止，因此可能留下已成功写入的前缀分片；读取侧用连续性规则阻止这类半写 payload 被当作完整检查点。

data/checksum 与 meta 对不完整数据采用不同策略：`mergeSelectCheckpoint` 静默跳过坏 UUID 组并继续返回其他完整组，空 UUID 也直接忽略；`selectCheckpointMeta` 则把空表或 segment 空洞作为任务级错误。Rust 当前没有复刻 Go 对空 UUID 的 warning 日志，也没有在调用 `ExecRestrictedSQL` 前显式包装 Go 的 `kv.InternalTxnBR` 上下文；这些是当前边界差异，不能在文档中表述为已对齐能力。

`initialLock` 与 `updateLock` 不是可恢复错误，而是直接 panic。生产 `TableMetaManager` 以零 lock tick 启动 runner，避免表后端进入锁更新路径；任何新调用方若把表后端用于需要锁的 runner，必须先实现或显式规避这两个方法。`Mutex::lock().unwrap()` 在锁中毒时也会 panic。

`dropCheckpointTables` 不是事务操作：前面的表可能已经删除，而后续表删除、InfoSchema 查询或数据库删除失败。它通过检查剩余表避免误删含用户表的库，但 Rust 版本不像 Go 版本那样记录“保留非空库”的 warning。`MemSession` 对未知 SQL 静默成功、DDL 不改变表 map、data `REPLACE` 不模拟主键覆盖，测试结论不得外推到真实 TiDB 的所有 SQL 行为。

## 并发与资源生命周期

`tableCheckpointStorage.se` 的 mutex 串行化同一存储实例的 flush 与 close，保证一个 session 不被并发可变借用；flush 在执行全部分片 SQL 期间持锁，因此并发 flush 不会在单个实例内交错，但慢 SQL 会延长锁持有时间。`close` 用 `Option::take` 确保 session 至多关闭一次，后续 `close` 为空操作，后续 flush 返回关闭错误。

分片写入本身没有数据库事务包裹，进程中断或中途 SQL 错误可留下前缀段。data/checksum 通过每次 flush 的独立 UUID 和读取时的连续性校验隔离不完整组；meta 使用固定 segment 主键和严格连续性校验，但仍需关注前述旧尾段问题。随机 UUID 降低并发 runner 写同一 data/checksum 表时的主键碰撞概率，文件本身不负责清理旧 UUID 组。

`MemSession` 和 `MemRestricted` 通过共享 `Arc<Mutex<HashMap<...>>>` 支持测试中的写后读。查询会克隆行列表后释放锁；`Close` 不清空表，以便测试保留 executor 观察结果。这套生命周期是测试便利设计，不代表真实 `Session` 在关闭后仍允许读取。

## 与 Go 版本的对应关系

本文件逐项对应 `br/pkg/checkpoint/storage.go`：数据库/表常量、524288 字节块大小、两类表结构、分片函数、`tableCheckpointStorage`、UUID 合并状态机、data/checksum 解析、meta JSON 往返以及“库为空才删库”的顺序均保持一致。Rust 的 `Uuid::new_v4` 对应 Go `uuid.New`，`serde_json` 对应 `encoding/json`，`Mutex<Option<Box<dyn Session>>>` 则把 Go 可空 session 的所有权与关闭状态显式化。

已确认的语义差异包括：Go 的 `initialLock`/`updateLock` 调用 `log.Fatal` 后形式上返回 `nil`，Rust 直接 panic；Go 在空 UUID 和保留非空数据库时写 warning，Rust 不记录日志；Go 为受限 SQL 设置 `kv.InternalTxnBR`，Rust `Context`/`RestrictedSQLExecutor` stub 接口没有等价调用；Go `close` 没有把 `se` 置空，而 Rust `close` 取走 session，使后续 flush 得到 `session closed`。这些差异都应由调用方约束或未来移植任务评估，不能为了文档任务改动实现。

Rust 独有的 `MemSession`/`MemRestricted` 不存在于 `storage.go`，属于独立测试支撑。`storage_test.rs::ddl_identifiers_use_go_percent_n_binding` 验证 DDL 标识符仍以 `%n` 参数传递；`parity_test.rs::go_rust_public_contract_matches` 覆盖数据库前缀、块边界、连续 segment 合并和空洞组丢弃。Go 同目录没有 `storage_test.go`，相关原始 Go 行为主要由 `checkpoint_test.go` 通过管理器/runner 链路间接覆盖。

## 扩展指南

- 调整分片大小时必须同步 `CheckpointIdMapBlockSize`、两类 `BLOB(524288)` 建表模板、Go `storage.go` 与兼容性测试；已有表和跨版本恢复是首要兼容风险，不能只改循环常量。
- 新增 data/checksum 类 payload 时，应复用 UUID 分组协议并在 `manager.rs` 接入；若新增 meta 类 payload，应复用连续 segment 协议，同时明确重写较短数据时如何清理旧尾段。新增测试放在独立的 `storage_test.rs` 或现有 `parity_test.rs`，不要嵌入生产文件。
- 修改 `mergeSelectCheckpoint` 时保持 SQL 排序、UUID 切换提交、首段必须为 0、空洞整组丢弃及坏组不污染好组等不变量；至少补充空 UUID、乱序、首段非 0、多个完整组和查询错误用例。
- 修改 meta 读取时应显式评估 `SELECT` 是否需要 `ORDER BY segment_id`，并覆盖空表、乱序、空洞、无效 JSON、单段和多段。任何排序修复都要与 Go 行为及真实 TiDB 查询契约一起核对。
- 新接线到 `tableCheckpointStorage` 前确认 runner 的 lock tick 为零；否则 `initialLock`/`updateLock` 会 panic。若要支持锁，应在表后端定义清晰协议并更新 `checkpointStorage` 的独立测试，而不是吞掉调用。
- 扩展 `MemSession` 时保持它是明确受限的测试替身；需要验证真实 SQL 事务、唯一键或 InfoSchema 行为时，应使用适合的集成测试表面，不能不断把生产数据库语义复制进该内存实现。
- 修改 SQL 标识符绑定或清理策略时同步 `storage_test.rs::ddl_identifiers_use_go_percent_n_binding`；修改公共 Go/Rust 契约时同步 `parity_test.rs::go_rust_public_contract_matches` 和相应 Go 测试。保留源文件的 PingCAP Apache License 与 `// Copyright 2026 AsterSQL.`。

## 验证依据

- 源码与模块：`br/pkg/checkpoint/storage.rs`（607 行，全部常量、函数、trait 实现和测试替身）；`br/pkg/checkpoint/lib.rs`（模块声明、独立测试挂载、根级再导出）；`br/pkg/checkpoint/Cargo.toml`（crate 名、library target、`serde`/`serde_json`/`uuid` 依赖）。同目录不存在 `doc.go`。
- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/checkpoint` 定位 26 个 Go/Rust 文件；`node --file br/pkg/checkpoint/storage.rs --offset 1 --limit 320` 与 `--offset 321 --limit 320` 覆盖完整 607 行；`query` 定位 `tableCheckpointStorage`、`mergeSelectCheckpoint`、`selectCheckpointData`、`insertCheckpointMeta`、`dropCheckpointTables`、`IsCheckpointDB`；精确 `explore` 给出 `selectCheckpointData -> mergeSelectCheckpoint`、`selectCheckpointChecksum -> mergeSelectCheckpoint` 调用边及测试调用点。
- 上游接线：`br/pkg/checkpoint/manager.rs` 中 `TableMetaManager` 的快照/日志 trait 实现负责加载、保存、删除和启动 runner；`br/pkg/checkpoint/log_restore.rs::newTableCheckpointStorage` 是公开构造包装。
- Go 对照：`br/pkg/checkpoint/storage.go`，逐项核对常量、SQL、分片、合并、错误、清理及日志行为；`br/pkg/checkpoint/checkpoint_test.go` 是同包 Go 回归入口。
- Rust 独立测试：`br/pkg/checkpoint/storage_test.rs::ddl_identifiers_use_go_percent_n_binding` 核对 `%n` 标识符参数；`br/pkg/checkpoint/parity_test.rs::go_rust_public_contract_matches` 核对三个数据库前缀、524288 字节分段、连续 segment 合并与空洞 UUID 组丢弃；`checkpoint_test.rs` 覆盖管理器和 runner 的表存储主链。
- 本任务只新增说明文档，按计划不运行 Cargo。交付结构验证要求本文恰好包含固定的十一个二级章节；人工复核重点为测试替身不冒充生产实现、锁方法的 panic 边界、meta 查询排序依赖与 Rust/Go 已知差异均有明确证据。
