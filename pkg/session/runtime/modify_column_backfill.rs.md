# `pkg/session/runtime/modify_column_backfill.rs`

源文件：[`modify_column_backfill.rs`](./modify_column_backfill.rs)

## 文件定位

该文件属于 `astersql-session` crate 的私有 `runtime::modify_column_backfill` 模块（模块声明见 `pkg/session/runtime.rs:90`），为会话运行时提供持久化 `MODIFY COLUMN`/索引重组所需的 KV 批处理原语。它不负责创建 DDL job、推进 schema state 或提交事务；这些边界由 DDL worker 和 `SystemSession` 调用方管理。文件内六个函数中，`batch`、`temporary_value`、`merge`、`ingest`、`ingest_with_options` 仅在父模块可见，`translate_fixed_timestamp` 仅供本文件使用。

从应用主链看，`SystemSession::backfill_modified_column` 调用 `batch`，`SystemSession::merge_modified_indexes` 在非分布式路径调用 `merge`，分布式 merge 子任务也复用 `merge`；普通 DML 写入路径调用 `temporary_value` 保存唯一临时索引的操作历史；索引回填的 ingest 路径调用 `ingest_with_options`。证据分别位于 `pkg/session/runtime/system_session.rs:1417-1433`、`:1640-1659`、`:2288-2310`，`pkg/session/runtime/modify_column_dist_backfill.rs:889-933` 和 `pkg/session/runtime/dml.rs:765-829`。

## 核心职责

1. `batch` 在调用方已经开启的事务内，对一个物理表记录键区间做有界快照扫描，把旧列值按捕获的 SQL mode 和时区转换为新列类型，再重编码整行并写回。
2. `temporary_value` 在 DML 覆盖唯一临时索引值前读取并拼接已有值，保留同一临时键上的操作历史，使后台 merge 能按顺序消解并发 DML。
3. `merge` 有界扫描临时索引键，过滤被覆盖的历史，把有效删除/插入操作回放到正式索引键，处理唯一索引冲突和孤儿 handle，随后删除临时键并返回检查点。
4. `ingest`/`ingest_with_options` 把已编码的 KV 对写入 Lightning 本地磁盘引擎，关闭 writer/engine 后经存储层 `Write`/`MultiIngest` 导入并清理引擎。
5. `translate_fixed_timestamp` 弥补表达式上下文只接受命名时区的 ABI 限制，在固定偏移时区与 UTC 日历字段之间显式转换 MySQL `TIMESTAMP`。

这些函数共同提供“单批、调用方控制事务或子任务生命周期”的底层执行能力，不包含重试、任务调度、checkpoint 持久化或 DDL schema 状态机。

## 主要符号

- `batch(session, table, old, new, physical, start, end, limit, mode, location) -> Result<(Vec<u8>, i64), String>`：返回下一批起始键和本批扫描键数。它构造 `EvalContext`，验证物理表键范围，扫描至 `limit`，逐键加锁、读取当前值、解码、转换、编码并 `Set`。
- `temporary_value(txn, key, value) -> Result<Vec<u8>, SharedError>`：仅处理“索引键且临时索引键且当前元素为 distinct”的值；其他值原样返回。处理时锁住键，读取旧值并执行 `old || new` 拼接，未找到旧值时返回新值。
- `merge(session, request) -> Result<BackfillTaskContext, String>`：使用 `IndexBackfillBatch` 的 schema/table/index/range/batch size，在活动事务中回放临时索引历史，并生成 `next_key`、`done`、`scan_count`、`added_count` 与事务 `StartTS`。
- `ingest(domain, job_id, pairs) -> Result<(), String>`：使用默认 `SSTImportOptions` 的便利入口；当前直接调用点使用的是可配置版本。
- `ingest_with_options(domain, job_id, pairs, options) -> Result<(), String>`：空输入直接成功；非空输入创建 `import_sst::Backend`、engine manager、名为 `ddl-{job_id}` 的 engine 和 local writer，依次执行 append、close、import、cleanup。
- `translate_fixed_timestamp(value, zone, from_utc) -> Result<(), String>`：只改写非零 `KindMysqlTime`；`from_utc=true` 用于读取旧 `TIMESTAMP`，反向用于新 `TIMESTAMP` 写入前的固定偏移转换。

文件没有模块级常量、类型、trait、`impl` 或条件编译项。`#[allow(clippy::too_many_arguments)]` 只作用于 `batch`。

## 执行流程

### 列值回填

`batch` 先把 `TimeZoneLocation` 解析为命名 `chrono_tz::Tz` 或固定 `chrono::FixedOffset`；非法名称和非法 offset 立即失败。随后以 `WithSQLMode`、`WithLocation` 构造表达式上下文，固定偏移时额外用 `WithCurrentTime` 提供本地日历字段。取得 `ConcreteSession.state` 的可变借用后，要求存在活动事务，并读取会话的 row encoder 开关。

它用 `GenTableRecordPrefix(physical)` 建立范围不变量：`start` 必须位于该物理表前缀下，`end` 不得超过此前缀的 `PrefixNext`。快照迭代器最多收集 `limit` 个仍属于此前缀的键；下一检查点为尚未处理的当前键，否则为 `end`。关闭迭代器后，函数为表内全部列构建 `column ID -> FieldType` 解码映射。

每个候选键先以 `InternalTxnDDL` 上下文无限等待加锁，再从当前事务读取，以避免直接使用快照中的旧值覆盖并发更新。记录已不存在时跳过；存在时解码整行。旧列缺失则通过 `GetColOriginDefaultValue` 补原始默认值。新列带 `NOT NULL` 而值为 NULL 时返回 `[ddl:1138]Invalid use of NULL value`。固定偏移且旧类型为 `TIMESTAMP` 时先转成本地日历；新类型为 `TIMESTAMP` 时先按 `DATETIME` 做解析/舍入，再转 UTC，最后按真实新类型校验范围。普通路径直接调用 `CastValue`。转换值以新列 ID 写回 datum map，整行按原 session row encoder 配置重编码并写入事务。

### 临时索引写入与合并

普通 DML 组装 mutation 后，在显式事务和 autocommit 两条路径中都先调用 `temporary_value`，再 `transaction.Set`（`pkg/session/runtime/dml.rs:765-829`）。只有 distinct 临时索引才需要将旧历史与新元素拼接；锁与读发生在同一事务中。

`merge` 先从活动事务的 snapshot 收集最多 `batch_size` 个键，检查点是下一个未处理键或范围终点。它通过 `TransactionMutator::get_table` 取得当前表元数据。每个键必须是临时索引键；解码并掩掉 `IndexIDMask` 后，不在 `request.index_ids` 中的键跳过，目标 index 元数据不存在则失败。正式索引键由 `TempIndexKey2IndexKey` 得到，临时键与正式键一起加锁，再重新读取临时值。

解码后的历史经 `FilterOverwritten` 压缩，`TempIndexKeyTypeMerge` 元素因已经双写而跳过。删除元素会删除正式键；对 distinct 删除，如果正式键指向不同 handle，只有该 handle 的行已经不存在时才允许删除，从而避免误删仍有存活行的唯一索引。插入元素对 distinct 键先检查正式值：相同值视为已经正确写入并跳过，不同值返回 duplicate-key 错误，不存在才写入。处理完一个临时键后将其删除。调用方负责提交；分布式路径在成功后提交并验证 checkpoint 前进（`modify_column_dist_backfill.rs:889-933`）。

### SST 导入

`ingest_with_options` 对非空 pairs 创建 job 专属 backend 和 engine，把全部 pairs 一次交给 local writer，严格按 `writer.Close -> engine.Close -> engine.Import -> engine.Cleanup` 顺序完成资源状态转换。`Import` 使用 `96 MiB` region split size 与 `960000` split keys。任何一步失败立即返回字符串错误，后续步骤不会执行。

## 数据与状态

- 会话状态：`batch`/`merge` 通过 `RefCell::borrow_mut` 独占访问 `ConcreteSession.state`，使用其中活动 `kv::Transaction`；`batch` 还读取 `row_encoder_enabled`。
- 行数据：`batch` 以 `HashMap<column_id, Datum>` 表示整行。Rust `HashMap` 的迭代顺序不稳定，但 `EncodeRow` 同时接收对应的 ID/value 向量，二者由同一次 `unzip` 产生，配对关系保持一致。
- 扫描检查点：`batch.next` 和 `merge.next_key` 都指向本批未处理的第一键；若无剩余键则等于调用方给出的 `end`。`merge.done` 由 `next >= end_key` 得出。
- 计数：`batch` 返回已扫描键数，记录在扫描后消失仍计入；`merge` 的 `scan_count` 与 `added_count` 当前都等于收集的临时键数，即使某键因 index ID 不匹配、已消失或历史元素被过滤。这是当前实现事实，不应解释成精确的实际 KV 写入数。
- 临时索引值：编码值可能包含多个 `TempIndexValueElem`。`temporary_value` 保留旧字节历史；`merge` 依赖 `Current`、`FilterOverwritten`、`Distinct`、`Delete`、`KeyVer`、`Handle` 和 `Value` 决定最终动作。
- 元数据：`batch` 使用传入的 `TableInfo`/旧新 `ColumnInfo`；`merge` 在当前事务中按 schema/table ID 重新读取表，避免使用过期 index 列定义。

## 依赖与调用关系

上游调用关系（RustCodeGraph 的文件节点显示目标文件被 7 个文件使用，精确引用再由 `rg` 核对）：

- `system_session.rs::backfill_modified_column -> batch`。
- `system_session.rs::merge_modified_indexes -> merge`；分布式模式转入 `modify_column_dist_backfill::run`，其 merge 阶段仍调用本文件 `merge`。
- `dml.rs` 的显式事务与 autocommit mutation 应用路径 `-> temporary_value`。
- `system_session.rs::backfill_index_batch_with_ingest_options -> ingest_with_options`。
- 测试模块直接调用 `merge` 和 `ingest_with_options`。

主要下游依赖为：`astersql-kv` 的 transaction/snapshot/iterator/lock API，`astersql-tablecodec` 的行与临时索引编解码，`astersql-expression-exprstatic` 与 `astersql-table::column` 的求值/默认值/类型转换，`astersql-meta`/`astersql-meta-model` 的表元数据，以及 Lightning backend/encode/kv 与 `session::runtime::import_sst::Backend` 的物理导入链。`pkg/session/Cargo.toml` 将该模块归入 `astersql-session`，声明 `nextgen` feature，但本文件没有 feature gate；其中直接声明了 Lightning crates、`chrono` 和 `chrono-tz`，其他 AsterSQL crate 也由该 manifest 提供。

RustCodeGraph 对该文件识别出 10 个符号，并能精确定位 `temporary_value`、`ingest_with_options`、`translate_fixed_timestamp`；通用名称 `batch`/`merge` 的全局查询噪声较大，且一次精确 `callers` 查询超时，因此上述调用边以索引的 `used by` 集合加精确源码引用交叉验证。

## 错误处理与边界

- 所有入口都用 `Result` 返回错误，不在本文件内重试。多数下游错误经 `to_string()` 抹平具体 Rust 错误类型；`temporary_value` 保留 `SharedError`。
- `batch` 的硬边界包括：有效时区、活动事务、物理表范围、行解码/默认值/转换/编码成功以及 `NOT NULL` 检查。`limit == 0` 时不处理键并把当前首键作为下一检查点；调用方必须避免在循环中以零 limit 造成不前进。
- `batch` 总会从事务当前值重新读记录；快照扫描后删除的记录安全跳过。它没有在本地提交，任一中途错误由调用方决定回滚。
- `merge` 拒绝范围中的非临时索引键、缺失表或 index、无法解码的 index ID/value/handle。distinct 插入值不同时报 `[kv:1062]Duplicate entry for key '<index name>'`。
- distinct 删除的防误删分支很关键：当正式键指向另一 handle 且对应行仍存在时跳过删除；仅“行不存在”才继续删除正式键。`normal_ddl_masking_policy_test.rs::modify_column_temporary_unique_delete_removes_orphaned_handle` 覆盖孤儿 handle 应被清理的情形。
- `ingest_with_options` 的空输入不会创建 backend。非空路径没有显式的失败清理 guard：writer close、engine close 或 import 失败时，本函数不会继续调用 `Cleanup`；是否有底层析构清理由 backend 类型决定，本文件没有提供保证。
- `translate_fixed_timestamp` 对非时间 datum 和零时间是 no-op；反向转换使用 `.single()`，无法形成唯一固定偏移本地时间时失败（固定 offset 本身通常无 DST 歧义）。

## 并发与资源生命周期

`batch` 和 `merge` 都遵循“先用 snapshot 有界枚举、再对每个键加锁并读事务当前值”的模式。snapshot iterator 在进入逐键写循环前显式 `Close`，避免扫描资源跨锁/写阶段存活。锁随外层事务结束释放；本文件不提交或回滚。`LockKeys` 在 `batch` 使用 `WaitTimeoutMs = -1`，可能无限等待冲突锁；`merge` 和 `temporary_value` 使用默认 lock context。

`temporary_value` 的读改写由同一事务的键锁串行化；该保证只覆盖它实际处理的 distinct 临时索引键。DML 外层还管理行锁、事务回滚和 autocommit 提交，本函数不拥有这些资源。

分布式 merge 每批显式 `BEGIN`，调用 `merge` 后 `COMMIT`，错误时尝试 `ROLLBACK`；提交后才持久化 subtask checkpoint。因而本文件返回的 `next_key` 只是候选进度，不是持久化完成证明。

SST 路径的线性资源顺序是 backend -> manager -> opened engine -> local writer -> closed engine -> import -> cleanup。它没有在此函数中启动线程；并行与速率控制由 Lightning backend 和传入 `SSTImportOptions` 实现。`import_sst_test.rs::read_index_task_meta_updates_live_physical_import_limiter` 在线程中验证已有物理导入会观察动态限速更新，`read_index_cancel_stops_existing_physical_write_limiter` 验证取消语义。

## 与 Go 版本的对应关系

`batch` 对应 Go `pkg/ddl/column.go` 的 `updateColumnWorker.fetchRowColVals`、`getRowRecord` 和 `BackfillData`：两者都在事务内按范围/批量扫描、解码旧行、转换新列、重编码并写回。Rust 当前实现是较集中的版本，并非逐行等价复刻：Go 还显式处理 changing column 已由 DML 写入时跳过、cast warning 聚合、generated expression、row checksum、txn source、priority/resource group 和错误重格式化；这些行为在本文件中未出现，不能从 Go 实现推断 Rust 已支持。反之，Rust 对固定偏移 `TIMESTAMP` 的显式日历转换由 `translate_fixed_timestamp` 完成，并由本仓库 Rust 测试覆盖。

`temporary_value` 对应 Go `pkg/table/tables/index.go` 在唯一临时索引写入/删除时读取原值并调用 `TempIndexValueElem.Encode(originValue)` 追加历史的语义。Rust 把这一步放在 session mutation 落 KV 之前统一处理。

`merge` 对应 Go `pkg/ddl/index_merge_tmp.go` 的 `mergeIndexWorker.BackfillData`、`fetchTempIndexVals`、`batchCheckTemporaryUniqueKey`：共同点是有界扫描临时键、`FilterOverwritten`、跳过已双写版本、回放正式索引并删除临时键。Go worker 还设置 txn priority/resource group、对 retryable error 缩小 batch 并重试、记录 metrics/failpoint，并把 `TempIndexKeyTypeDelete` 也作为无需回放的版本过滤；Rust 本函数自身没有这些 worker 层机制，且只显式跳过 `TempIndexKeyTypeMerge`。扩展时必须按任务链整体比较，不能仅凭函数名认定完全对齐。

`ingest_with_options` 对应 Go `pkg/ddl/ingest/engine.go` 的 local writer/`AppendRows` 阶段及 `pkg/ddl/ingest/backend.go::unsafeImportAndReset` 的 closed engine import/cleanup 阶段。Rust 将单批所需步骤压缩为一次调用，固定 split 参数；Go backend 上下文还管理多 engine、TS、reset、分布式锁、指标与更丰富的错误转换。

相关 Go 测试证据包括 `pkg/ddl/modify_column_test.go`（NULL、类型转换和回填行为）、`pkg/ddl/tests/indexmerge/merge_test.go`（临时索引并发/merge 行为）与 `pkg/ddl/ingest/integration_test.go`（ingest modify-column 流程）。它们说明上游意图，但不是 Rust 实现已经通过同等覆盖的证据。

## 扩展指南

- 修改列转换规则时优先改 `batch`，并同步检查 `translate_fixed_timestamp`、`SystemSession::backfill_modified_column` 传入的 SQL mode/timezone，以及 `normal_ddl_masking_policy_test.rs` 中三个 `modify_timestamp_timezone` 用例。新增 Rust 测试应放在独立 `*_test.rs`，不要内嵌到生产文件。
- 增加 Go `updateColumnWorker` 语义时，应逐项说明 changing-column skip、warning、checksum、generated column、txn options 的证据和接线位置，避免把 worker 调度/重试递归搬进本文件。兼容风险主要是 SQL mode、时区、NULL、截断/越界错误文本和 checkpoint 不前进。
- 修改临时索引格式或 merge 规则时必须同时审查 `temporary_value` 与 `merge`，并对照 `tablecodec::TempIndexValueExt`、`dml.rs` 两条事务路径及 `modify_column_dist_backfill.rs`。至少扩展独立测试覆盖：多元素覆盖顺序、merge/delete key version、唯一值相同/冲突、handle 存活/孤儿、键在扫描后消失、batch 边界和不匹配 index ID。
- 修改 SST 导入流程时在 `ingest_with_options` 保持 writer/engine 状态机顺序，明确失败时清理策略，并同步 `import_sst_test.rs` 的限速、取消、duplicate/value error 测试。性能风险集中在一次性 `Vec<KvPair>`、固定 split 参数和磁盘引擎清理。
- 调整扫描范围或检查点时，保证返回键严格前进，并维持物理表/临时索引范围验证。调用方当前会把不前进视为分布式任务错误；零 batch size 应由上游归一化为至少 1（分布式调用点已用 `max(1)`）。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/session/runtime/modify_column_backfill.rs` 确认目标文件含 10 个符号；`node --file ... --offset 1 --limit 500` 读取了完整 431 行并给出 7 个使用文件；`query --json` 精确确认 `temporary_value`、`ingest_with_options`、`translate_fixed_timestamp` 的签名与位置。一次 `callers modify_column_backfill.rs::temporary_value` 查询超过 60 秒无输出后终止，调用关系改由索引使用集合和精确引用验证。
- Rust 源与接线：`pkg/session/runtime/modify_column_backfill.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/system_session.rs`、`pkg/session/runtime/dml.rs`、`pkg/session/runtime/modify_column_dist_backfill.rs`。
- crate 边界：`pkg/session/Cargo.toml`；crate 名为 `astersql-session`，本文件所需 Lightning、KV、table/tablecodec、expression、meta/model、chrono 依赖均在该 manifest 的依赖集合中，文件本身没有条件 feature。
- 独立 Rust 测试：`pkg/session/runtime/normal_ddl_masking_policy_test.rs::modify_column_temporary_unique_delete_removes_orphaned_handle`、`masking_policy_sql_modify_timestamp_uses_captured_timezone`、`masking_policy_sql_modify_datetime_to_timestamp_converts_local_calendar`、`masking_policy_sql_modify_timestamp_uses_named_timezone`；`pkg/session/runtime/import_sst_test.rs::read_index_task_meta_updates_live_physical_import_limiter` 与 `read_index_cancel_stops_existing_physical_write_limiter`。测试模块由 `pkg/session/runtime.rs:1807-1812` 独立接入。
- Go 对照：`pkg/ddl/column.go` 的 `updateColumnWorker`，`pkg/table/tables/index.go` 的临时索引历史编码，`pkg/ddl/index_merge_tmp.go` 的 `mergeIndexWorker`，`pkg/ddl/ingest/engine.go` 与 `pkg/ddl/ingest/backend.go` 的写入/导入生命周期；相关测试路径为 `pkg/ddl/modify_column_test.go`、`pkg/ddl/tests/indexmerge/merge_test.go`、`pkg/ddl/ingest/integration_test.go`。
- 本任务只增加说明文档，未运行 Cargo 或代码测试。交付验证以固定 11 章节结构检查、链接/路径检查和人工事实复核为准。
