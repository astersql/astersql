# `pkg/session/runtime/relational_scan.rs`

## 文件定位

[`relational_scan.rs`](./relational_scan.rs) 属于 `astersql-session` crate 的 `runtime` 私有子模块：`pkg/session/runtime.rs` 以 `mod relational_scan` 装配它，并在 crate 内重导出 `scan_mlog_record_commit_ts`，对外重导出 `transaction_has_table_prefix`。`pkg/session/Cargo.toml` 的 `[package.metadata.porting]` 将整个 crate 对应到 Go `pkg/session`；本文件本身没有同路径 Go 文件，而是把 Go 中分散在 session、executor、planner 与 tablecodec 的表读语义集中到 Rust 会话运行时。

它位于 SQL 查询主链的“物理计划/会话可见性到 KV 读取”边界：上游主要是 `runtime/query.rs`、`runtime/dml.rs`、`runtime/ddl.rs`、`runtime/explain_analyze.rs` 与 `runtime/canonical_table_reader.rs`，下游则是 `astersql-kv` 的 transaction/snapshot/client、`astersql-tablecodec` 的键值编解码、`tipb` coprocessor 协议，以及 planner 的 `PhysicalTableReader`。文件包含真实读路径而非门面或桩。

## 核心职责

1. 在事务、历史/陈旧快照、外部时间戳和最新提交快照之间选择正确的读取视图，并为 READ COMMITTED 锁定读叠加当前事务的本地写入（`scan_registered_table_with_limit`、`scan_registered_table_ranges`、`scan_latest_with_transaction_overlay*`）。
2. 将记录前缀、整数聚簇主键条件和本地二级索引条件转换为 KV 范围，以正序/逆序和分区物理表范围执行扫描（`relational_row_scan_ranges`、`relational_primary_key_scan_ranges`、`secondary_index_predicate_bounds`）。
3. 解码记录键和值，恢复 PK handle、DDL 新增列默认值、虚拟生成列、隐式 `_tidb_rowid` 及运行时类型标记（`decode_relational_row_value`）。
4. 为 COUNT、带过滤 TableReader、ANALYZE、ANN 构造并执行 TiKV/TiFlash 请求，合并执行详情，且在能力或请求路径不适用时显式回退本地快照扫描（`relational_*_request`、`execute_relational_count_request`、`scan_relational_ann_rows`）。
5. 优化 LIMIT/OFFSET：先以 key-only 游标定位窗口；大 OFFSET 的整数 handle 还可先估算 seek key，再用 TiKV 精确计数补偿键空洞（`scan_relational_row_ranges_window_key_only`、`integer_handle_offset_candidate`、`count_relational_key_range_with_coprocessor`）。
6. 从简单 SQL 谓词选择二级索引或 index merge 分支，并对多个索引分支并发扫描、去重或求交（`relational_secondary_index_access`、`relational_index_merge_access`、`scan_registered_table_at_with_index_merge`）。

## 主要符号

- `RelationalRowScanRange = (kv::Key, kv::Key, bool)`：半开区间的起止键与逆序标记。调用者必须维持 `start < end`，第三项决定 `Iter` 或 `IterReverse`。
- `RelationalSecondaryIndexAccess`：选中的 `IndexInfo` 与按物理表展开的索引键范围；仅接受 public、可见、非 primary/MV/global/partial 的本地索引。
- `RelationalIndexMergeAccess`：多个二级索引分支以及 union（OR）或 intersection（AND）模式。
- `scan_relational_rows*` / `scan_relational_row_ranges*`：记录前缀或范围扫描的基础层，负责迭代器关闭、OFFSET 消费和有限行解码。
- `decode_relational_row_value`：从 tablecodec datum map 生成 `RelationalRow`；这是处理 schema evolution、handle 恢复和虚拟生成列的集中入口。
- `relational_scan_column`、`planned_table_reader_{dag,select_dag}`、`relational_count_dag`：把元数据/物理计划编码为 tipb executor 链；前者保留字段类型、排序规则、flags、数组与 PK-handle 信息。
- `relational_coprocessor_request` / `relational_select_request`：建立 TiKV DAG 请求基础字段、分区 ranges、并发度、replica read、txn scope、stale-read 和连接标识。
- `count_relational_rows_with_coprocessor`：优先发送 Checksum 精确统计记录 KV，失败后回退 TableScan + partial COUNT DAG；分区表或不支持 DAG 的 client 返回 `None` 让上层改走本地路径。
- `ConcreteSession::scan_registered_table_at_with_window`：自动提交分页快路径，统一快照选择、key-only OFFSET 和整数 handle seek。
- `ConcreteSession::scan_registered_table_at_with_index_merge`：以 `std::thread::scope` 并发读取分支，按 `(physical_id, record_key)` 合并，intersection 要求候选命中全部分支。
- `ConcreteSession::latest_relational_version`：RC 事务通过 `TimestampFuture::Wait` 获取 statement timestamp，且只在成功等待后累计 `DurationWaitTS`。
- `transaction_has_table_prefix`：检查非 pipelined 事务 mem-buffer 是否含表前缀，用于决定 union-scan 类保护；pipelined transaction 当前直接返回 `false`。

## 执行流程

普通 SELECT 从 `runtime/query.rs` 进入：先判断 ANN、COUNT、二级索引、index merge、主键范围和 LIMIT/OFFSET 等可用快路；最终调用 `scan_registered_table_at*`。这些入口先按显式 `read_ts`、事务 stale timestamp、snapshot timestamp、session stale timestamp、external timestamp 的优先级选择版本；没有固定版本时，自动提交取 `CurrentVersion`，普通显式事务直接以 transaction 作为 retriever。

基础记录扫描由 `relational_row_scan_ranges` 生成每个分区或逻辑表的 record-prefix 范围。`scan_relational_row_ranges` 为每个范围创建迭代器，`consume_relational_row_iterator` 先推进 OFFSET，再解码最多 `count` 行，并在任何结果路径上关闭迭代器。解码时先 `DecodeRowToDatumMap`，对 `PKIsHandle` 从 record handle 补主键，再填 origin default、计算非 stored generated column，最后按需添加 `_tidb_rowid`。

二级索引路径由 `relational_secondary_index_access` 匹配 WHERE/ORDER BY 与索引前缀；`secondary_index_predicate_bounds` 只处理可安全转成范围的 `AND`、`BETWEEN` 和比较表达式。扫描阶段先从索引 KV 解 handle，随后 `BatchGet` record keys 并复用记录解码。Index merge 将 OR/显式 hinted AND 拆成至少两个唯一索引分支，并发取候选后做 union 去重或 intersection 过滤；若 LIMIT 已嵌入，残余谓词必须在裁剪前重新计算。

COUNT 路径先尝试从 SQL 经过 `NewPlanBuilder -> DoOptimize -> PhysicalTableReader` 得到 Go 对齐的下推 executor 链。无过滤 COUNT 可先走 Checksum；Checksum 失败会发送 TableScan + HashAgg DAG。返回的 region partial datum 用 checked conversion/addition汇总，响应必须关闭。能力不足、分区表、显式事务或规划结果不匹配时，上层回退逐键计数或带事务可见性的 root 侧扫描。

ANN 路径只允许 `store.Name() == "TiKV"`，但请求目标为 TiFlash：构造 `AnnQueryInfo`、列式向量索引与 TableScan DAG，流式解码所有列并合并 read-pool details。非 TiKV storage 返回 `Ok(None)`，由调用者决定常规路径。

## 数据与状态

核心输入是 `TableInfo`/`IndexInfo`、`kv::Retriever`/`Snapshot`/`Transaction` 和 session `state`。记录范围使用 tablecodec 的 record/index prefix，分区扫描通过克隆 `TableInfo` 并将 `ID` 替换为 physical partition ID 实现；不会修改共享表元数据。

`RelationalRow` 的首项只在整数 handle 时保存 handle，否则为 `0`；第二项是按小写列名索引的运行时值 map，并可能含类型 marker 或 `_tidb_rowid`。记录解码依赖当前 `TableInfo.Columns`，因此缺失列按 `OriginDefaultValue` 语义补齐，虚拟生成列在读时求值。

会话读取时间戳状态的优先级在多个 `scan_registered_table_at*`/`count_registered_table_at` 入口中保持一致。`state.transaction` 决定是否需要读本地 mem-buffer；RC 特例不是固定事务快照，而是“最新 committed snapshot + transaction snapshot/visible 差集”的 overlay。coprocessor 请求还携带 resource group、runaway checker、paging size、replica read、txn scope 和 `ConnID`，返回统计合并到 `StmtCtx.SyncExecDetails`。

Key-only 扫描临时对 snapshot 设置 `kv::KeyOnly=true`，定位结束后恢复为 `false`；调用方不可假设该选项会永久保持。二级索引 `BatchGet` 以 `KeyMapName` 查回记录值，索引指向缺失记录被视为一致性错误而不是跳过。

## 依赖与调用关系

上游直接证据包括：`runtime/query.rs` 调用 ANN、COUNT、二级索引、index merge、窗口和普通扫描；`runtime/dml.rs` 与 `runtime/ddl.rs` 调用全表/范围扫描做写入校验、约束和 DDL 数据处理；`runtime/explain_analyze.rs` 复用索引选择与有限扫描；`runtime/canonical_table_reader.rs` 用 `relational_select_request` 和 `planned_table_reader_select_dag` 构建规范 TableReader；`runtime.rs` 装配并重导出少量边界函数。

下游依赖可分为四组：`astersql-kv` 提供 MVCC 版本、迭代器、snapshot/transaction、coprocessor client 与 request；`astersql-tablecodec` 提供 record/index key、handle 和 datum 编解码；`astersql-planner-core*`/`astersql-expression` 生成并编码物理执行器；`tipb`/`protobuf` 表达 DAG、Checksum、ANN 和返回 chunk。`pkg/session/Cargo.toml` 还确认该 crate 直接依赖这些 workspace crate，以及固定 revision 的 `tipb` protobuf 实现。

RustCodeGraph 将本文件标记为被 17 个文件使用；对 `scan_registered_table_at_with_window` 的 callee 图明确连接到 `count_relational_key_range_with_coprocessor`、`integer_handle_offset_candidate`、两个 key-only 扫描函数和 `Snapshot::SetOption`。部分方法的 callers 查询在当前索引未返回边，但 `rg` 的直接引用补充了上述上游证据。

## 错误处理与边界

所有存储、codec、planner 和 protobuf 错误经 `session_error(context, error)` 增加阶段上下文，语义性失败直接构造 `SessionError`。典型硬错误包括：ANN 索引缺向量列、TiFlash/TiKV 返回协议错误、COUNT 溢出或 datum 类型非法、索引 handle/record 缺失、DAG executor 首尾不符合 TableScan-to-Aggregation 约束，以及显式事务误入只允许 autocommit 的 OFFSET/index 路径。

可优化性不足通常不是错误：不支持 DAG、分区 COUNT、无法安全解析谓词、索引不满足约束、planner 未生成目标 TableReader 等返回 `None`，要求调用者保留正确但较慢的通用路径。二级索引常量类型不兼容也放弃 access path，把强制转换与错误语义交给根谓词求值。

边界由 checked/saturating 运算保护：COUNT 使用 `checked_add`，region partial 使用 `usize::try_from`；OFFSET 窗口用 `saturating_add`；整数 handle candidate 的 offset/加法溢出会放弃优化。空范围、`count == 0`、`start >= end` 都短路为空结果。无符号整数 handle 因 tablecodec 物理顺序在 `i64::MAX` 处分段，范围顺序由 SQL ASC/DESC 重新组织。

响应与迭代器都显式关闭。对 response，读取错误优先于 close 错误；读取成功但 close 失败仍返回错误。`scan_mlog_record_commit_ts` 对 `CommitTs == 0` fail closed，避免把无法证明早于 cutoff 的记录交给清理逻辑。

## 并发与资源生命周期

普通 KV 扫描同步执行，每个 `kv::Iterator` 都在成功或闭包返回错误后调用 `Close`。临时 snapshot 选项 `KeyOnly` 在定位闭包结束后恢复；coprocessor/TiFlash `Response` 在流消费结束后关闭，并把 `ReadPoolTaskDetails` 合并到 statement context。

Index merge 是本文件唯一显式多线程区域：`std::thread::scope` 让 worker 借用范围不逃逸；每个分支克隆 domain/table/access 并取得同一 `Version` 的独立 snapshot，因此多个分支读同一 MVCC 视图而不共享可变 snapshot。任一 worker panic 转为 `SessionError("index-merge worker panicked")`，任一分支错误使整体失败；所有 scoped worker 在离开作用域前完成。

`ConcreteSession.state` 使用运行时 borrow；代码在进入 storage closure、递归调用或耗时扫描前显式 `drop(state)`，避免跨边界持有 borrow。RC TSO 等待结束后才锁 `DurationWaitTS` 累计时间，失败的 oracle 请求既不形成版本也不计等待。事务 overlay 会把完整结果暂存于 `BTreeMap`，其内存随表/范围结果增长；普通 count 与 key-only OFFSET 则避免物化全部行值。

## 与 Go 版本的对应关系

crate 元数据只声明整体对应 Go `pkg/session`，不存在 `pkg/session/runtime/relational_scan.go` 一对一文件。可复核的语义对应点是：

- Rust `planned_table_reader_dag` 使用 planner `FlattenListPushDownPlan` 并逐 operator `to_pb`；Go 对应实现在 `pkg/planner/core/operator/physicalop/physical_utils.go::FlattenListPushDownPlan` 和 `pkg/executor/internal/builder/builder_utils.go::ConstructDAGReq`，由 `pkg/executor/builder.go` 的 TableReader/IndexReader 构建链调用。
- Rust 的 snapshot/transaction 分流与 `scan_latest_with_transaction_overlay*` 对应 Go executor 的 union-scan/read-your-writes 契约，但 Rust 在本文件内以 record-key map 显式合并；它不是 Go `UnionScanExec` 的逐行复刻。
- Rust `latest_relational_version` 注释明确对齐 Go RC `getStmtTS`；Go `pkg/session/txn.go` 也只在成功取得时间戳后更新 `DurationWaitTS`。Rust 相关失败/延迟行为由 `runtime/scan_adapter_runtime_test.rs` 的 RC timestamp tests 覆盖。
- Rust 二级索引范围与 index merge 是当前 session runtime 的局部实现，遵循 Go ranger/index-merge 的结果语义，但只承认代码中列出的简单表达式、索引种类与显式 hinted intersection；不能据此声称覆盖 Go 成本模型和完整 ranger。
- Rust Checksum/partial COUNT、key-only OFFSET、ANN TiFlash 请求是为保持结果和资源语义实现的运行时快路；失败或不适用时仍保留 root/snapshot 回退，因此快路能力不等同于 SQL 支持范围。

## 扩展指南

新增读取语义时应先确定接入层：新的 KV 范围规则放在 `*_predicate_bounds`/`relational_*_scan_ranges`；新的记录字段恢复规则集中改 `decode_relational_row_value`；新的 coprocessor executor 改 DAG builder 与 response decoder；新的会话可见性规则必须同时更新普通、范围、窗口、索引和 COUNT 入口，避免不同快路读取不同版本。

扩展二级索引时，必须同步考虑分区 physical ID、prefix index、global/MV/partial/invisible index、类型强制转换、升降序、残余谓词以及 dangling index 一致性错误。扩展 index merge 时还要保证所有分支使用同一 version，明确 union/intersection 去重键，并限制并发与结果物化成本。

新增 OFFSET 优化不得以稠密 handle 为前提；当前 candidate 只是不会越过目标的下界，仍需精确范围计数补偿删除空洞。任何 snapshot option 修改都要恢复，任何 iterator/response 新分支都要关闭。新增 COUNT datum 类型或 executor 形态时保留溢出检查和 planner 首尾约束。

测试必须放在独立文件而非本源文件。优先扩展 `pkg/session/runtime_test/storage.rs`（计数、OFFSET、二级索引、index merge 与快路结果）、`pkg/session/runtime_test/planning.rs`（planner/DAG 形态），以及 `pkg/session/runtime/scan_adapter_runtime_test.rs`（client response 关闭、exec details、RC TSO、codec 边界）。若改变 Go 对齐语义，还应对照对应 Go executor/planner 测试，而不是只验证 Rust 能编译。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 文件；`files --filter pkg/session/runtime/relational_scan.rs` 确认目标被索引；`node --file ...` 分段读取了 2,883 行与 139 个符号；`callees scan_registered_table_at_with_window` 核实 OFFSET/coprocessor/key-only 调用链。callers 对部分 impl method 无输出，已用直接引用搜索补证。
- 源码与装配：`pkg/session/runtime/relational_scan.rs`、`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`、`pkg/session/runtime/query.rs`、`dml.rs`、`ddl.rs`、`explain_analyze.rs`、`canonical_table_reader.rs`。
- Rust 独立测试：`pkg/session/runtime_test/storage.rs`（基础 COUNT、key-only 大 OFFSET、TiKV partial COUNT、LIMIT/OFFSET、二级索引），`pkg/session/runtime_test/planning.rs`（规范 planner 生成 filtered COUNT DAG），`pkg/session/runtime/scan_adapter_runtime_test.rs`（响应关闭和统计合并、导入行 PK handle 恢复、RC timestamp 行为）。本任务按要求未运行 Cargo，测试文件仅作为现有行为证据读取。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_utils.go`、`pkg/executor/internal/builder/builder_utils.go`、`pkg/executor/builder.go`、`pkg/executor/select.go`、`pkg/session/txn.go`；这些是语义对应文件，不是本 Rust 文件的一对一来源。
- 人工复核重点：读取视图优先级、事务 overlay、范围半开区间、unsigned handle 分段、response/iterator 关闭、优化失败回退、index merge 同版本并发，以及新增测试必须保持独立文件。
