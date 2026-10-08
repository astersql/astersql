# `pkg/session/runtime/explain_read.rs`

## 文件定位

目标源码 [`explain_read.rs`](explain_read.rs) 属于 `astersql-session` crate 的具体会话运行时，由 `pkg/session/runtime.rs` 以私有模块 `mod explain_read` 装配。文件只为 `ConcreteSession` 增加一个 `pub(super)` 方法 `execute_native_explain_read`；没有模块级常量、独立类型、trait、条件编译项或对 crate 外公开的 API。

它位于 `EXPLAIN ANALYZE SELECT` 的“真实读取”边界：直接上游 `pkg/session/runtime/explain_analyze.rs` 创建 `ReadStats`，调用本方法取得行和实际 KV/Coprocessor 派发统计，再继续生成执行信息。该方法不是完整查询执行器，也不负责渲染 EXPLAIN 行；当事务、分区表或非 TiKV 存储使本快路径不适用时，它以 `Ok(None)` 通知调用者回退到 `scan_registered_table_at_with_index_window` 或 `scan_registered_table_with_limit`。

## 核心职责

- 从 SELECT 的首个 `TableSource` 解析显式 `AS OF` 时间戳，并与会话级 `snapshot_read_ts`、`session_stale_read_ts` 合成为本次读取版本。
- 对整数主键 handle 的等值条件和 `IN` 条件分别走 Snapshot `Get` 或 `BatchGet`，让 PointGet/BatchPointGet 的真实 RPC（包括底层重试）进入传入的 `ReadStats`。
- 对不能识别为整数主键点查的查询构造最小 `tipb::DagRequest`，向 TiKV 发送全表 `TableScan` Coprocessor 请求，并解码返回行。
- 把响应 subset 和最终 `ReadStats` 中的 read-pool task details 合并到 `SessionVars.StmtCtx.SyncExecDetails`，为 EXPLAIN 运行信息提供实际派发证据。
- 保持适用范围窄而明确：活动事务、分区表和存储名不是 `TiKV` 时不模拟原生请求，交还上游回退逻辑。

## 主要符号

- `ConcreteSession::execute_native_explain_read(&self, statement, table, timeout_ms, stats) -> SessionResult<Option<Vec<RelationalRow>>>`：文件中唯一符号。输入为已解析 SELECT、表元数据、TiKV 客户端读取超时和共享统计对象；`Ok(Some(rows))` 表示原生读取已执行，`Ok(None)` 表示调用者必须回退，`Err(SessionError)` 表示原生路径已失败，不能静默回退。
- `statement: &ast::SelectStmt`：这里只读取 `From`、`Where` 和 `TableSource.AsOf`。过滤、排序、聚合、LIMIT 等完整 SQL 语义仍由 `explain_analyze.rs` 的后续流程处理；Coprocessor DAG 本身只包含 TableScan。
- `table: &astersql_meta_model::TableInfo`：提供表 ID、列定义、整数主键 handle 与分区信息。`PKIsHandle` 是进入点查识别的必要条件。
- `stats: Arc<astersql_store::ReadStats>`：同一个实例同时传给 Snapshot 的 `CollectRuntimeStats` 或 KV Client `Send`，并在方法末尾读取 read-pool 汇总。
- 返回行类型 `RelationalRow` 来自父模块：点查通过 `decode_relational_row_value` 恢复 handle、默认值和生成列；Coprocessor 路径把每行表示为 `(0, HashMap<列名, Option<String>>)`，后续逻辑按列名消费。

## 执行流程

1. 方法先检查 `self.state.borrow().transaction` 和 `table.GetPartitionInfo()`；任一存在就立即返回 `Ok(None)`，避免绕过事务覆盖层或错误地把分区表当单一物理表扫描。
2. 它沿 `SelectStmt.From -> TableRefs.Left -> ResultSetNode::TableSource` 取得首个表源。若有 `AS OF`，调用 `evaluate_stale_read_ts` 求值并传播错误；否则依次采用会话的 `snapshot_read_ts` 或 `session_stale_read_ts`。三者都没有时，稍后向存储请求当前全局版本。
3. 通过 `self.domain.storage().with_storage` 进入存储边界。`store.Name() != "TiKV"` 时返回 `Ok(None)`；TiKV 路径用显式/会话 read TS 构造 `kv::NewVersion`，或调用 `CurrentVersion(kv::GlobalTxnScope)` 获取 TSO。
4. 若 `table.PKIsHandle`，先用 `primary_point_get_value` 识别 `pk = literal`（也接受字面量在左侧），再识别非 `NOT` 的 `pk IN (literal...)`。所有值都必须能解析为 `i128`；任一条件不满足就不走点查，而是继续走 Coprocessor TableScan。
5. 点查分支取得 Snapshot，设置 `kv::TiKVClientReadTimeout` 与 `kv::CollectRuntimeStats`，并用 `relational_handle_key(table.ID, handle)` 编码 record key。一个 key 调用 `Get`，not-found 转为空结果；零个或多个 key 调用 `BatchGet`，之后按输入 key 顺序从返回 map 取存在的值。
6. 点查返回值逐条先由 `DecodeRecordKey` 恢复 handle，再交给 `decode_relational_row_value`。后者负责 row datum、PK handle、缺失列默认值、虚拟生成列和 `_tidb_rowid` 等会话运行时行语义。本分支完成后返回 `Ok(Some(rows))`。
7. 扫描分支把每个 `ColumnInfo` 经 `relational_scan_column` 转为 tipb 列，组装单个 `TypeTableScan` executor、完整列 `output_offsets` 和覆盖该表 record prefix 的 `kv::Request`。请求写入 DAG bytes、超时、`IsStaleness`，并按会话 `replica_read == "leader"` 选择 Leader，否则选择 Mixed；`ClientSendOption.EnableCollectExecutionInfo` 固定为 `true`。
8. `store.GetClient().Send` 必须返回 response。循环调用 `response.Next`：每个 subset 先合并 `ReadPoolTaskDetails`，再解析 `tipb::SelectResponse`、检查服务端 error，并按 `table.Columns` 的固定顺序反复 `DecodeOne` 组成行。
9. 无论解码循环成功或失败，代码都会调用一次 `response.Close()`；读取/解码错误优先于 close 错误，只有读取成功且 close 成功才返回 `Ok(Some(rows))`。
10. 离开存储闭包后，方法从共享 `ReadStats` 取得最终 read-pool task details，经 `astersql_store_driver::read_pool_task_details` 转换后再次合入语句上下文，最后返回原生读取结果。

## 数据与状态

- 本文件不持有跨调用的自有字段；可变状态都属于 `ConcreteSession.state`、语句上下文、底层 Snapshot/Response 或调用者传入的 `Arc<ReadStats>`。
- 读取版本优先级是“显式 `AS OF` > `snapshot_read_ts` > `session_stale_read_ts` > 当前全局版本”。只要前三者之一存在，Coprocessor 请求的 `IsStaleness` 就设为 `true`。
- 点查只识别 `PKIsHandle` 的整数 handle。`IN ()` 可形成零 key 的 `BatchGet`；结果 map 中缺失的 key 被过滤，输入重复 key 是否保留重复行取决于 `BatchGet` 返回 map 与按 key 重取的组合，扩展时不能假设它等同于任意 SQL IN 去重实现。
- Snapshot `Get` 的 not-found 是正常空结果；`BatchGet` 返回 map 后仍按原 key 列表排列输出，因此不是按 map 的迭代顺序输出。
- Coprocessor 路径没有把 WHERE、ORDER BY、GROUP BY、HAVING、DISTINCT 或 LIMIT 下推到 DAG；它读取原始表行，后续过滤/计数和 EXPLAIN 节点构造发生在 `explain_analyze.rs`。这是当前实现范围，不应解释为通用 planner 已下推完整计划。
- Coprocessor 行的 tuple handle 固定写为 `0`，而点查解码会保留真实整数 handle。当前调用链主要按列名执行后续选择；新增依赖 handle 的行为必须处理这项差异。

## 依赖与调用关系

直接上游调用边是 `pkg/session/runtime/explain_analyze.rs` 中的 EXPLAIN SELECT 分支：创建 `native_stats`，调用 `execute_native_explain_read`，以 `Option::is_some()` 记录 native 标志，并在 `None` 时选择已注册表扫描回退。`pkg/session/runtime.rs` 只负责私有模块装配，不再导出该方法。

关键下游依赖如下：

- `ConcreteSession::evaluate_stale_read_ts`（`pkg/session/runtime/control.rs`）：解析显式 stale-read 表达式、用户变量、bounded staleness 及未来时间错误。
- `primary_point_get_value`（`pkg/session/runtime.rs`）：只识别主键与字面量之间的 `=`/`==`。
- `relational_handle_key`、`datum_to_runtime_value`（`pkg/session/runtime/row_codec.rs`）：编码整数 record key，并把 tablecodec datum 转成运行时字符串值。
- `relational_scan_column`、`relational_coprocessor_request`、`decode_relational_row_value`（`pkg/session/runtime/relational_scan.rs`）：建立 tipb 列/表 record range 请求与解码完整关系行。
- `astersql_domain::Domain -> storage() -> kv::{Storage, Snapshot, Client, Response}`：获取版本、执行 Get/BatchGet、发送 DAG 请求及同步拉取 subset。
- `tipb`、`protobuf`、`astersql_tablecodec`：编码 TableScan DAG，解析 `SelectResponse`，解码 record key 与行 datum。

`pkg/session/Cargo.toml` 把该文件归入 `astersql-session`（`lib.rs` 为 crate root），直接声明 workspace path 依赖 `astersql-domain`、`astersql-kv`、`astersql-meta-model`、`astersql-store`、`astersql-store-driver`、`astersql-tablecodec`、`astersql-types` 等；`protobuf` 固定为 `2.8.0`，`tipb` 固定到 Git revision `07f0ea6b6bffa9d8ac100d81ee51dbbfe4dda3bf`。本模块不受 `nextgen` feature 或自己的 feature gate 控制。

## 错误处理与边界

- 事务、分区和非 TiKV 是“不适用”而非错误，返回 `Ok(None)`；一旦选择原生路径，TSO、Snapshot、RPC、编码或解码失败都返回 `Err`，不会回退并掩盖真实派发错误。
- `AS OF` 求值错误由 `evaluate_stale_read_ts` 原样传播；当前版本获取错误加上 `EXPLAIN read TSO` 上下文。
- PointGet 把 `kv::IsErrNotFound` 转为空行，其余错误标记为 `EXPLAIN PointGet`；BatchGet 错误标记为 `EXPLAIN BatchPointGet`。record key 与行值解码错误分别由 `EXPLAIN decode handle` 和下游 `decode relational row` 等上下文报告。
- DAG 序列化、Coprocessor `Next`、response protobuf、datum 和 `Close` 分别附带 `encode EXPLAIN DAG`、`EXPLAIN Coprocessor`、`decode EXPLAIN Coprocessor`、`decode EXPLAIN datum`、`close EXPLAIN Coprocessor` 上下文。`SelectResponse.error` 的服务端消息直接成为 `SessionError`。
- 如果 `Client::Send` 返回 `None`，错误为 `EXPLAIN Coprocessor returned no response`。若读取/解码与 close 同时失败，`match` 的顺序保留前者；只有 result 成功而 close 失败时才返回 close 错误。
- Coprocessor datum 循环依赖两个不变量：`table.Columns` 非空，且 `rows_data` 每行恰有相同列数。零列且数据非空会使 `encoded` 无法前进；列数/编码不匹配会在 `DecodeOne` 处报错。扩展 DAG 输出时必须同步维护列与 `output_offsets`。
- 仅首个 `From.TableRefs.Left` 的直接 `TableSource` 可提供显式 `AS OF`。连接、派生表或其他形状不会在本方法中提取 source；它们是否会到达本入口由上游 EXPLAIN 选择逻辑约束。

## 并发与资源生命周期

方法是同步的 `&self` 调用，没有创建线程、异步任务或通道。`ConcreteSession.state` 通过运行时已有的 `RefCell` 借用访问，`ReadStats` 通过 `Arc` 与 Snapshot/KV Client 共享；本文件本身不增加锁，也不声明同一个 `ConcreteSession` 可并发调用。

PointGet/BatchGet 的 Snapshot 是闭包内局部对象，选项随 Snapshot 生命周期存在；本方法没有显式清除 `CollectRuntimeStats`，对象在分支返回后释放。Coprocessor response 则具有明确的 `Send -> 多次 Next -> Close` 生命周期：正常 EOF、服务端错误、protobuf/datum 解码错误都会进入显式 `Close`，但 `Send` 返回 `None` 时没有 response 可关闭。`Close` 不依赖 `Drop` 才执行。

每个 subset 的 read-pool details 在消费时立即合并，方法末尾还会合并 `ReadStats` 的聚合视图。这里的目标是让已完成派发证据在返回前进入 `StmtCtx.SyncExecDetails`；修改合并时机或次数必须与 store-driver 的统计聚合语义一起验证，避免丢失重试或重复累计。

## 与 Go 版本的对应关系

`pkg/session` 下没有与 `runtime/explain_read.rs` 同路径、同名单文件的 Go 实现；Rust 文件是为具体会话运行时拆出的适配层。最接近的 Go 语义分布在执行器和 DistSQL 边界，而不是一个一一对应函数中。

- Go `pkg/executor/point_get.go::PointGetExecutor` 在有 runtime stats 时给 Snapshot 设置 `kv.CollectRuntimeStats`，并在 `Close` 把 scan/read-pool/time details 合入语句上下文；`pkg/executor/batch_point_get.go::BatchPointGetExec::Close` 做同类合并。Rust 点查分支复用同一类 Snapshot 选项，但直接返回 `RelationalRow`，未实现 Go 执行器完整的 Open/Next/Close、索引使用报告、锁与事务语义。
- Go `pkg/distsql/request_builder.go::RequestBuilder.Build` 从 `DistSQLContext` 写入 `TiKVClientReadTimeout`，`pkg/distsql/distsql.go::Select` 建立 `ClientSendOption`，`pkg/distsql/select_result.go` 在拉取 subset 时合并 read-pool details。Rust Coprocessor 分支在一个同步函数中完成这三段最小接线。
- Go 的查询计划/DAG 由 planner/executor 生成并可包含过滤、聚合、索引、分区、TiFlash 等算子；本文件只生成单 TableScan。活动事务与分区表被显式交给 Rust 上游回退，不可据此文件宣称 Go 全部 EXPLAIN ANALYZE 读取能力已移植。
- Go `pkg/executor/explain_test.go::TestCheckActRowsWithUnistore` 验证 EXPLAIN ANALYZE 的实际行数；Rust `pkg/session/runtime_pessimistic_test.rs::explain_analyze_select_reports_scan_rows_process_keys_and_rpc_info` 对应验证 Limit、索引查找、Point_Get、process keys 与 RPC 信息，但它覆盖的是完整会话路径而非直接调用本方法。

## 扩展指南

- 扩展点查条件时，修改 `execute_native_explain_read` 的 `handles` 推导，并同步核对 `primary_point_get_value`。支持 common handle、unsigned 边界、复合主键、二级唯一索引或参数表达式时，必须对照 Go PointGet/BatchPointGet 的 key 编码、重复值、NULL、类型转换和顺序语义，不能只把字符串 `parse::<i128>()` 放宽。
- 扩展 Coprocessor 下推时，应优先复用 `relational_scan.rs` 的请求/DAG 构造能力，并同时维护 executor 顺序、`output_offsets`、返回列数和 datum 解码。WHERE、LIMIT、聚合或索引下推会改变 `explain_analyze.rs` 当前在本地计算 actRows/process keys 的假设。
- 支持分区或事务时，不应简单删除入口 guard。需要分别传入物理分区 ID/范围并合并结果，以及读取事务 mem-buffer/锁语义；否则 EXPLAIN 可能漏掉未提交写入或扫描错误物理 key 空间。
- 修改 stale read 或 replica read 时，同步检查 `control.rs::evaluate_stale_read_ts`、请求 `IsStaleness`、会话 `replica_read` 与 TiKV 的 retry/fallback 约定。当前除字符串 `leader` 外一律选择 Mixed，新增枚举值需要兼容性设计。
- 修改统计与生命周期时，同步核对 `explain_analyze.rs` 的 `native_stats` 消费、store-driver `read_pool_task_details` 转换及 Go `PointGetExecutor`/`BatchPointGetExec`/`selectResult` 的合并时机。重点风险是重试 RPC 数丢失、read-pool 重复累计和错误路径未关闭 response。
- 回归测试应继续放在独立文件。优先扩展 `pkg/session/runtime_pessimistic_test.rs`，覆盖单点、`IN` 批点、not-found、超时重试、全表 Coprocessor、事务/分区/非 TiKV 回退、显式与会话 stale TS、response/close/decode 错误；与 EXPLAIN 输出拼装有关的断言可放在 `pkg/session/runtime/explain_query_test.rs` 或现有 EXPLAIN 独立测试中。
- 性能关注点包括每行 `HashMap` 分配、Coprocessor 全表物化、每列 `DecodeOne`、点查 field map 克隆及 `BatchGet` 后逐 key map 查询。任何减少物化的优化都必须保留“统计来自真实已派发请求”的职责。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/explain_read.rs` 确认目标文件已索引并识别 19 个符号；`node --file ... --offset 1 --limit 260` 读取全部 124 行；`node execute_native_explain_read` 确认唯一函数定义。精确 `callers` 未返回可用结果，宽泛 `explore` 存在 `read`/`explain` 同名碰撞，因此直接调用点用 `rg -n 'execute_native_explain_read' pkg/session` 核对为 `pkg/session/runtime/explain_analyze.rs`。
- RustCodeGraph 下游符号：`evaluate_stale_read_ts`（`pkg/session/runtime/control.rs`）、`primary_point_get_value`（`pkg/session/runtime.rs`）、`relational_handle_key`/`datum_to_runtime_value`（`pkg/session/runtime/row_codec.rs`）、`relational_scan_column`/`relational_coprocessor_request`/`decode_relational_row_value`（`pkg/session/runtime/relational_scan.rs`）。
- Rust 源与 crate 边界：`pkg/session/runtime/explain_read.rs`、调用者 `pkg/session/runtime/explain_analyze.rs`、模块入口 `pkg/session/runtime.rs`、`pkg/session/Cargo.toml`。`pkg/session` 未发现 `doc.go`，因此没有额外的目标包 Go 契约文件可读。
- Rust 独立测试：`pkg/session/runtime_pessimistic_test.rs::explain_analyze_select_reports_scan_rows_process_keys_and_rpc_info` 验证 Limit/TableFullScan、IndexRangeScan、Point_Get 和 RPC/process-key 信息；同文件 `topology_and_runtime_faults_drive_cluster_rows_rpc_counts_and_commit_retries` 验证 Coprocessor region/RPC 统计及 `tikv_client_read_timeout` 延迟注入导致的 PointGet 重试计数。未发现以 `execute_native_explain_read` 命名的直接单元测试。
- Go 对照：`pkg/executor/point_get.go`、`pkg/executor/batch_point_get.go`、`pkg/distsql/distsql.go`、`pkg/distsql/request_builder.go`、`pkg/distsql/select_result.go`、`pkg/executor/explain_test.go::TestCheckActRowsWithUnistore`。这些文件提供分散的执行统计、请求超时、响应合并和 EXPLAIN 行数语义；没有同路径 Go 文件。
- 本任务只新增说明文档，按计划不运行 Cargo。结构验证命令及最终退出码在任务交付记录中给出；人工复核重点为适用 guard、两条读取分支、错误/Close 顺序、统计合并、Go 非一一对应关系和安全扩展边界。
