# `pkg/executor/infoschema_reader.rs`

## 文件定位

[`infoschema_reader.rs`](./infoschema_reader.rs) 位于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 与 `pkg/executor/lib.rs` 中的 `pub mod infoschema_reader` 共同把它暴露为公开模块。文件没有 `cfg`/feature 条件，crate 的 `nextgen` feature 也没有在本文件中直接分支。

它承担 Information Schema（内存系统表）读取的 Rust 契约和通用控制流：定义行/单元格模型、谓词和快照状态、数据源接口、按表名分发的通用检索器，以及 COLUMNS、DDL jobs、TABLE_STORAGE_STATS、事务、锁等待、死锁和 TiFlash 系统表等专用分页检索器。

当前接线状态需要谨慎理解：RustCodeGraph 能解析本文件及其内部调用边，`lib.rs` 也会编译该模块；但对仓库生产 Rust 源做精确符号检索，没有找到这些检索器在本文件之外的构造或调用。已确认的直接使用者主要是 `infoschema_reader_test.rs`、`infoschema_reader_internal_test.rs`、`infoschema_reader_keyspace_test.rs` 和 `infoschema_cluster_table_test.rs`。因此，本文件目前是可测试的移植抽象/行为契约；它是否已接入完整 SQL 执行主链未得到代码证据，不能按 Go 版本的已上线接线来理解。

## 核心职责

1. 用 `Datum`、`Row`、`ColumnInfo`、`TableInfo` 表达虚拟表的完整行和输出投影，用 `InfoSchemaError`/`InfoResult` 统一错误边界。
2. 用 `DataRequest` 描述所有后端读取意图，用 `InfoSchemaDataSource` 隔离会话、快照、权限、统计、DDL、PD/TiFlash 与死锁数据的具体实现。文件自身主要决定“何时、以什么参数读取”，数据生产由 trait 实现者负责。
3. `memtableRetriever::retrieve` 选择事务快照或最新快照，经 `dispatch_table` 按表名加载数据，按 1024 行分批输出，并通过 `adjustColumns` 完成列裁剪。
4. 对大表和外部资源使用独立状态机：`hugeMemTableRetriever`、`tableStorageStatsRetriever`、`tidbTrxTableRetriever`、`dataLockWaitsTableRetriever`、`deadlocksTableRetriever`、`TiFlashSystemTableRetriever` 各自维护游标和终止状态。
5. 提供与系统表语义相关的纯函数：数值精度、字符八位组长度、ANALYZE 剩余时间、权限缺省、标签规则解析、table ID 解码和列投影。

## 主要符号

- `BATCH_SIZE = 1024`：通用批大小，影响普通内存表返回、内存追踪刷账、COLUMNS、存储统计、事务/锁/死锁和 TiFlash 分页。
- `Datum` / `Row`：当前 Rust 移植使用的简化值模型和行别名。`Datum::estimated_size` 只估算载荷字节，不包含容器、枚举或分配器开销。
- `PredicateExtractor`：携带 `skip_request`、schema/table/column 集合、一般谓词和 TiFlash 实例/库/表过滤文本。
- `InfoSchemaDataSource`：本文件的关键反转接口。`load_rows(DataRequest, snapshot, extractor)` 是绝大多数表的统一下游；其余方法处理快照、权限、统计缓存、DDL reader 生命周期和专用计数/解析。
- `DataRequest`：后端协议枚举。简单变体表示一次整表读取；带字段的变体携带表 ID、cluster/history 标记、分页游标、权限掩码或 TiFlash 查询范围。
- `memtableRetriever`：通用检索器。`retrieve` 是主入口，`dispatch_table` 覆盖 SCHEMATA、TABLES、PARTITIONS、CLUSTER_INFO、USER_ATTRIBUTES、CHECK_CONSTRAINTS、索引使用、计划缓存等表；未知表名当前返回成功空集。
- `hugeMemTableRetriever`：面向 COLUMNS 的分页检索器，使用事务 InfoSchema，并通过库/表游标及 `batch` 推进。
- `DDLJobsReaderExec`：以 `Open`/`Next`/`Close` 管理数据源令牌；`Next` 未打开时返回错误。
- `tableStorageStatsRetriever`：要求 `TABLE_SCHEMA` 谓词，初始化候选表后按表游标加载存储统计。
- `tidbTrxTableRetriever`、`dataLockWaitsTableRetriever`、`deadlocksTableRetriever`：分别分页读取事务、锁等待和死锁等待链；后两者与当前 Rust 事务检索器都先检查 `PROCESS`。
- `TiFlashSystemTableRetriever`：解析实例列表，按实例和行偏移读取；一实例不足一批时切换到下一实例。
- `adjustColumns`：当输出列少于完整表列时按 `ColumnInfo.offset` 投影；越界返回 `InfoSchemaError`。
- `checkRule`、`decodeTableIDFromRule`、`tableOrPartitionNotExist`：验证 label rule 结构、从十六进制 `start_key` 委托解码 table ID、再核对库表/分区身份。

## 执行流程

通用内存表的单次查询流程如下：

1. `memtableRetriever::retrieve` 对 `CLUSTER_INFO` 先调用 `hasPriv(..., "PROCESS")`；权限明确为 false 时立即失败。
2. 若 `retrieved` 已置位，直接返回空行，保证耗尽后幂等。
3. 首次调用读取 `SessionState`。事务中优先采用非零 `snapshot_ts`，否则用 `transaction_start_ts` 调用 `snapshot_info_schema`；非事务调用 `latest_info_schema`。
4. `dispatch_table` 将不区分大小写的表名映射到一个 `setData*` 方法。多数方法再通过 `replace_rows` 或 `append_rows` 构造 `DataRequest` 并调用 `source.load_rows`；快照和谓词随请求传递。
5. 每个加载行经 `recordMemoryConsume` 累积估算内存；达到 1024 条或初始化结束时由 `flush_memory` 一次提交给 `MemoryTracker`。
6. 返回阶段从 `rows[row_idx..end]` 克隆至多 1024 行，推进 `row_idx`，耗尽时设置 `retrieved`，最后交给 `adjustColumns` 投影。

特殊流程：

- `setDataForUserAttributes` 在加载后读取当前用户/主机和可选的具体权限管理器，通过 `NewUserAttrFilter` 过滤可见账号；形状不是三列或前两列非文本的行会被跳过，空属性文本转为 `Null`。
- `statsReadRequirements` 只在输出包含 `TABLE_ROWS` 时要求行数；包含 `AVG_ROW_LENGTH`、`DATA_LENGTH` 或 `INDEX_LENGTH` 时同时要求列长度。`updateStatsCacheIfNeed` 按“所有分区 ID 后跟表 ID”的顺序提交。
- `hugeMemTableRetriever::retrieve` 首次取得 `transaction_info_schema`，每轮调用 `Columns { schema_cursor, table_cursor, batch }`；少于一批即耗尽，否则按返回行数推进 `table_index`。Rust 当前没有在本文件中推进 `database_index` 的逻辑，其实际解释完全依赖数据源契约。
- `DDLJobsReaderExec` 的令牌只能由 `Open` 获取；`Next` 把 `maximum_chunk_size` 传给数据源并累计 `cursor`；`Close` 用 `take()` 确保同一实例最多关闭一次令牌。
- `deadlocksTableRetriever` 跳过空等待链，按 `(current_index, current_wait_chain_index)` 推进；数据源必须让 `Deadlocks` 请求返回的行数与等待链推进数一致。
- `TiFlashSystemTableRetriever::retrieve` 遇到 `skip_request` 直接为空；初始化筛选实例后循环读取当前实例，只有取到行或所有实例耗尽才返回。少于 1024 行被视为当前实例结束。

## 数据与状态

- 快照状态：`memtableRetriever.info_schema` 在首次读取时固定；`hugeMemTableRetriever` 使用事务 InfoSchema；其他专用检索器多传 `None`，由数据源自行决定一致性语义。
- 分页状态：普通表使用 `row_idx/retrieved`；COLUMNS 使用 `database_index/table_index/batch`；存储统计使用 `current_table`；事务/锁等待使用 `cursor/total_rows`；死锁使用双游标；TiFlash 使用 `instance_index/row_index`。
- `retrieved` 是所有状态机的终止哨兵。空批可能代表耗尽，但普通 `memtableRetriever` 在全部行初始化完成后才分页，可能在内存中持有全量结果。
- `DataRequest` 是调用方与数据源之间的稳定边界。新增变体时必须同步 `InfoSchemaDataSource::load_rows` 的实现与测试假源，否则编译期或测试中的穷尽匹配会暴露缺口。
- `TableInfo.columns` 表示完整行布局，检索器的 `columns`/`output_columns` 表示实际输出；`ColumnInfo.offset` 必须指向完整行。仅比较列数并不能验证列顺序正确。
- `InfoSchemaDataSource` 和 `MemoryTracker` 都要求 `Send + Sync` 并通常放在 `Arc` 中；检索器自身的可变游标仍要求 `&mut self` 串行推进。

## 依赖与调用关系

上游关系：

- `pkg/executor/lib.rs` 公开声明模块，并在 `cfg(test)` 下装配四个直接相关 Rust 测试模块。
- RustCodeGraph 对 `memtableRetriever::retrieve` 的下游边确认了 `session_state -> snapshot_info_schema/latest_info_schema -> dispatch_table -> flush_memory -> adjustColumns`；对专用 `retrieve` 确认了各自的初始化、`load_rows`、权限和投影调用。
- 仓库搜索未发现生产 Rust 文件构造这些 reader；这限制了对“真实 SQL planner/builder 如何选择 reader”的验证。Go 版的上游接口位于 `pkg/executor/memtable_reader.go`，不能自动视为 Rust 接线证据。

下游关系：

- 本文件直接使用标准库集合、时间、`Arc`，以及 `astersql-privilege-privileges` 的 `UserPrivileges`、`RoleIdentity` 和 `NewUserAttrFilter`。`pkg/executor/Cargo.toml` 将该 crate 声明为 `astersql-privilege-privileges = { path = "../privilege/privileges" }`。
- 其余数据库能力均通过 `InfoSchemaDataSource` 委托；例如 `Tables`/`Partitions` 的统计需求、DDL token、TiFlash 实例、死锁记录和 label key 解码都不在本文件直接访问真实服务。
- `adjustColumns` 被普通、大表、存储统计、事务、锁等待、死锁和 TiFlash reader 共用，是所有完整行到查询列投影的收口点。

## 错误处理与边界

- 数据源错误通常通过 `?` 原样包装为 `InfoSchemaError` 向上返回；本文件没有重试、告警降级或错误分类。
- `getAutoIncrementID` 是明确例外：数据源错误与缺失值都降级为 `0`，调用方无法区分“尚未加载”“不存在”和读取失败。
- `hasPriv` 把 `privilege_verification` 返回的 `None` 视为允许，用来模拟 Go 内部会话无 privilege manager 时放行；数据源返回错误仍会失败。
- 普通分发遇到未知表名返回 `Ok(())`，最终得到空集；新增表名拼写错误不会自动报错。
- `adjustColumns` 会报告列偏移越界；Go 对照函数直接索引并可能 panic，这是 Rust 版更显式的边界。
- `tableStorageStatsRetriever::initialize` 没有 schema 谓词即报错，避免无约束遍历全部 schema。
- `checkRule` 拒绝段数不足、空类型、空 labels/数据和错误 `schema` 前缀；keyspace 模式要求 `keyspace/<id>/schema/<db>/<table>` 的最小形状。
- `decodeTableIDFromRule` 拒绝缺失/非法十六进制 `start_key` 和解码为 0 的 table ID。真实 table key 编解码语义委托给数据源。
- `calRemainInfoForAnalyzeStatus` 在总数为 0 时返回 `(0, 100.0)`；已处理行为 0 时用 1 防止除零；只有耗时严格为零才替换为一秒，非零亚秒会参与估算；负剩余值被夹为 0。

## 并发与资源生命周期

- `Arc<dyn InfoSchemaDataSource>` 允许数据源跨 reader 共享，trait 的 `Send + Sync` 要求实现者自行保证内部并发安全。测试假源使用 `Mutex` 记录请求即体现这一约束。
- reader 游标通过 `&mut self` 修改，本文件没有内部锁，也没有声明同一 reader 可被并发调用。安全用法是一个执行实例串行调用 `retrieve`/`Next`。
- `MemoryTracker` 以批量记账降低调用频率，但只追踪已加载 `Datum` 的估算载荷；普通 reader 仍一次物化完整 `rows`，不等于常量内存。
- DDL jobs 是唯一显式的 open/next/close 资源协议。成功 `Open` 后应保证调用 `Close`；本类型没有 `Drop` 自动回收，错误路径需要上层负责关闭。
- 快照只在 reader 初始化时选取，后续批次复用；这保证单个普通 reader 的元数据视图稳定，但数据源所返回的动态系统状态是否稳定取决于实现。
- TiFlash、事务、锁等待和死锁的分页终止依赖首次取得的实例数/总行数/记录快照；数据源若在分页间改变集合，可能造成漏读或提前/延后结束。

## 与 Go 版本的对应关系

对应文件是 `pkg/executor/infoschema_reader.go`，主要类型和函数名基本保持 Go 风格，便于逐项对照，但 Rust 版不是逐语句等价实现：

- Go `memtableRetriever` 直接依赖 `sessionctx`、domain、InfoSchema、统计、PD 和 privilege manager；Rust 把这些操作压缩为 `InfoSchemaDataSource` 和 `DataRequest`。因此 Rust reader 的控制流可测，但真实数据拼装大多位于尚未在本文件定义的数据源实现中。
- Go 普通 reader 同样在事务中按 `SnapshotTS`/`StartTS` 取快照、非事务用最新 InfoSchema，分发后按 1024 行返回并投影；Rust保留了这一骨架。
- Go `hugeMemTableRetriever` 自己列举 schema/table、检查列权限、处理 view schema 缓存并在跨库时推进双游标；Rust 仅发送游标请求，没有 Go 的 view 锁、系统会话和逐列拼装逻辑。
- Go `DDLJobsReaderExec` 获取系统 session、开启事务、初始化 running/history job iterator，并在 `Close` 释放 session；Rust 只保留数据源 token 协议，不应解释为已经移植全部资源语义。
- Go `tidbTrxTableRetriever` 无 `PROCESS` 时仍允许查看当前用户自己的事务；Rust 当前要求 `PROCESS` 才继续。这是可观察的语义差异，而不是等价移植。
- Go 的锁等待/死锁 reader 除分页外还处理 resolving locks、SQL digest 文本、实例地址和 key 解码；Rust把行形成整体交给 `load_rows`。
- Go TiFlash reader直接构造 system 表 SQL、发送 TiKV RPC、解析 JSON 并映射列；Rust仅管理实例/offset/limit，实际请求与解析由数据源承担。
- Go `checkRule` 依据运行时 `kerneltype.IsNextGen()` 判断 keyspace 格式；Rust 使用 `LabelRule.keyspace_mode` 显式输入。Go 解码 start key 时包含 codec/tablecodec 处理，Rust则委托 `decode_table_id_from_start_key`。
- `getNumericPrecision` 和 `calcCharOctLength` 的核心映射与 Go 对齐；`calRemainInfoForAnalyzeStatus` 也保留总数为零、处理数为零和零耗时的约束，Rust 测试额外固定了非零亚秒不应被替换为一秒。

## 扩展指南

- 新增普通 Information Schema 表：增加/复用 `DataRequest` 变体，在 `dispatch_table` 增加大小写无关的表名分支，实现对应 `setData*` 方法，并同步数据源实现。至少在 `infoschema_reader_internal_test.rs` 用记录请求的假源验证分发参数、错误传播和行形状。
- 新增分页表：先明确分页单位和稳定快照，复用 `BATCH_SIZE` 时要定义“少于一批即耗尽”是否安全；同时测试空集、恰好一批、多批、动态数据变化和重复 `retrieve`。
- 修改列布局：同步 `TableInfo.columns`、数据源完整行和 `ColumnInfo.offset`；加入投影越界/重排测试，不能只用“输出列数等于完整列数”作为布局正确证据。
- 修改权限：同时核对 Go 版对应函数及“无权限管理器”的内部会话行为。尤其不要把 `tidbTrxTableRetriever` 当前的全局 `PROCESS` 门槛当成与 Go 已对齐。
- 增加真实生产接线时，应在 executor builder/memtable reader 层增加明确构造点，并为 `InfoSchemaDataSource` 提供真实实现；在此之前不要让文档或 API 注释声称真实 PD/TiKV/DDL 调用已经由 Rust 完成。
- 测试必须保持与源码分文件：纯函数放 `infoschema_reader_test.rs`，内部分发/内存/权限放 `infoschema_reader_internal_test.rs`，keyspace label 放 `infoschema_reader_keyspace_test.rs`，集群/存储统计放 `infoschema_cluster_table_test.rs`。如扩展 DDL、事务、锁、死锁或 TiFlash 状态机，应新增相邻独立测试文件或扩展最贴近的现有文件，而不是把测试内嵌进生产文件。
- 性能风险集中在全量物化、行克隆、重复字符串和错误的终止条件；兼容风险集中在表名分发、Go/Rust 权限差异、DataRequest 参数和列 offset。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标 Rust 文件；`node --file pkg/executor/infoschema_reader.rs` 分段读取了 1–1545 行；`query memtableRetriever/DDLJobsReaderExec/TiFlashSystemTableRetriever/adjustColumns` 定位 Rust/Go 对照；`callees` 核对了通用和专用 `retrieve` 到快照、分发、数据源、权限、游标与投影的边。
- 源码与装配：`pkg/executor/infoschema_reader.rs`、`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`。
- Go 对照：`pkg/executor/infoschema_reader.go` 中的 `memtableRetriever`、`hugeMemTableRetriever`、`DDLJobsReaderExec`、`tableStorageStatsRetriever`、事务/锁/死锁 reader、`TiFlashSystemTableRetriever` 与 label rule 助手；`pkg/executor/memtable_reader.go` 仅作为 Go 上游接口位置证据。
- Rust 测试：`pkg/executor/infoschema_reader_test.rs`（精度、字符长度、ANALYZE 亚秒估算），`infoschema_reader_internal_test.rs`（统计请求、CHECK_CONSTRAINTS、KEYWORDS、USER_ATTRIBUTES 可见性/错误/内存），`infoschema_reader_keyspace_test.rs`（keyspace rule 与 table ID 解码），`infoschema_cluster_table_test.rs`（CLUSTER_INFO、Region 状态、TABLE_STORAGE_STATS 谓词和分页）。
- Go 测试：`pkg/executor/infoschema_reader_internal_test.go`、`infoschema_reader_keyspace_test.go`、`infoschema_reader_test.go`、`infoschema_cluster_table_test.go`，用于核对移植命名、表形状和边界意图。
- 生产接线限制：对 `pkg/executor/**/*.rs` 精确搜索主要 reader 类型和助手，除目标文件与上述测试外未发现构造/调用；RustCodeGraph 的文件级“used by”包含共享符号造成的宽泛引用，未据此宣称存在执行主链接线。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行固定 11 章节的结构验证。
