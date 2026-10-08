# [`pkg/session/runtime/explain_analyze.rs`](./explain_analyze.rs)

## 文件定位

本文件属于 `astersql-session` crate 的具体会话运行时，由 `pkg/session/runtime.rs:55` 以私有模块 `explain_analyze` 装配。它不是 SQL 解析器或通用物理计划实现，而是 `ConcreteSession` 在收到 `EXPLAIN ANALYZE` 后的执行与结果整形层：`pkg/session/runtime/dispatch.rs:3148-3172` 识别 SELECT 或 DML 子语句，SELECT 调用 `explain_analyze_relational_select`，`FORMAT='ru'` 再调用 `explain_analyze_ru_rows`。

`pkg/session/Cargo.toml` 将本 crate 映射到 Go 包 `pkg/session`，并直接依赖 executor、planner、parser、KV、store、tablecodec、types 等内部 crate。本文件通过 `use super::*` 使用 `runtime.rs` 聚合的这些类型；其职责横跨会话调度、真实执行器适配、KV 读取统计和 MySQL 结果元数据，但 API 可见性均限制在 `runtime` 父模块内。

## 核心职责

1. `explain_analyze_relational_select` 执行被解释的 SELECT，并生成与执行器树对应的行，包括算子名、估算/实际行数、任务位置、访问对象、执行信息、算子信息、内存和磁盘九列（`explain_analyze.rs:417-905`）。
2. 对满足条件的简单 SELECT，`explain_analyze_simple_typed_select` 优先走真实 planner/executor 适配链，耗尽并结束 `RecordSet`，从真实扁平物理计划提取算子树和实际行数，同时把语句 RU 终态责任交回 session（`explain_analyze.rs:98-318`）。
3. 对未进入 typed 快路径的查询，按标量子查询、CTE、LATERAL、点查、批量点查、IndexMerge、覆盖索引、IndexLookUp、普通表扫等形态构造兼容的 ANALYZE 输出（`explain_analyze.rs:320-905`）。
4. `explain_analyze_ru_rows` 把已执行的普通 ANALYZE 行投影为 Go 的七列 RU 格式；当前只保留 `id/task/actRows`，RU 数值列仍为空，不能把它描述为已经完成 RU 归因（`explain_analyze.rs:33-96`）。
5. `append_read_pool_execution_info` 只在 `PoolTaskDetails` 非空时追加完整的 `read_pool:` 聚合字符串（`explain_analyze.rs:18-28`）。

## 主要符号

- `append_read_pool_execution_info(&mut String, Option<&kv::PoolTaskDetails>)`：模块内辅助函数。保持已有执行信息，并用逗号分隔追加非空 read-pool 统计；被 typed 计划行和回退计划行共同调用。
- `ConcreteSession::explain_analyze_ru_rows(ConcreteRecordSet) -> SessionResult<ConcreteRecordSet>`：父模块可见的格式转换入口。按列名定位 `id`、`task`、`actRows`，过滤占位 ID `_0`，为七列构造 `information_schema` 字符串字段元数据；缺列时返回 `SessionError`。
- `ConcreteSession::explain_analyze_simple_typed_select(&self, &SelectStmt, &str) -> SessionResult<Option<ConcreteRecordSet>>`：私有优化路径。`Some` 表示成功使用真实物理执行器，`None` 表示调用者应回退；执行过程中真正的错误以 `Err` 返回。
- `ConcreteSession::select_contains_scalar_subquery(&SelectStmt) -> bool`：递归检查投影、WHERE、HAVING 以及 CTE 查询块中的子查询（`explain_analyze.rs:320-345`）。
- `ConcreteSession::scalar_plan_to_explain_analyze(ConcreteRecordSet) -> ConcreteRecordSet`：把 plan-tree 文本拆成九列协议行；识别 root/cop/mpp task 标记、`table:` 访问对象，并把 `MaxOneRow` 的实际行数置为 1，其余置为 0（`explain_analyze.rs:347-411`）。
- `ConcreteSession::explain_analyze_relational_select(&self, &SelectStmt, &str) -> SessionResult<ConcreteRecordSet>`：本文件主入口，负责策略选择、真实读取、统计计算和最终算子行生成。
- 本文件没有模块级类型、trait、常量或 feature 条件；唯一条件编译点是 typed 路径中的 `#[cfg(test)] statement_ru_post_compile` 故障注入钩子（`explain_analyze.rs:175-176`）。

## 执行流程

1. `dispatch.rs` 已确认语句是 `EXPLAIN ANALYZE`，记录 replica-read 请求后调用主入口；若格式为 RU，再对所得行做七列投影。
2. 主入口首先尝试 typed 快路径。它拒绝非简单形状、活动事务、任何 stale-read 状态、系统库、本地临时表以及非简单主键谓词；从原 SQL 去掉 `explain analyze` 前缀并重新解析，准备并绑定 planned KV SELECT（`explain_analyze.rs:103-169`）。
3. typed 路径构建 `ExecStmt`，安装失败终态 guard，读取 `TypedFlatPlan`，用 `ChildrenEndIdx` 栈计算深度；随后 `Exec()` 并循环 `Next()` 直到 EOF，统计返回行。读取失败先记录 RU 失败；无论正常读取与否都调用 `Finish()`，最后移除临时 `prepared_planned` 项（`explain_analyze.rs:171-231, 313-317`）。
4. typed 路径从 adapter effects 取扫描行数，从 `StmtCtx` 取 read-pool 详情，为每个真实物理算子生成九列行；扫描算子使用 `last_scanned_rows`，其他算子使用返回行数。`ExecStmt`、已 Finish 的底层 record set 和 RU failure guard 被保存到 `PendingStatementRU`，由 session 终态统一完成（`explain_analyze.rs:233-311`）。
5. 快路径不可用时，标量子查询交给 `explain_scalar_subquery_plan` 后整形；CTE 与 LATERAL 通过 `execute_insert_select_query` 取得真实结果，但目前只输出聚合根行，分别标识 `CTE_0` 和 `Apply`（`explain_analyze.rs:425-480`）。
6. 普通单表回退路径解析表元数据、LIMIT/OFFSET、二级索引访问和超时 hint；先调用 `execute_native_explain_read`。真实 TiKV 可用时，`pkg/session/runtime/explain_read.rs:8-105` 在 snapshot 或 Coprocessor 边界执行 PointGet/BatchGet/表扫并采集 `ReadStats`；否则使用注册表的内存扫描，并仅把 failpoint 当作格式模拟（`explain_analyze.rs:481-600`）。
7. 运行时统计由实际选中行、扫描行、耗时、原生请求 attempts 或模拟 region/topology 数推导。之后依次识别主键等值、主键 IN、IndexMerge、索引访问或普通 TableReader，并插入可选 Limit 根节点（`explain_analyze.rs:601-905`）。

## 数据与状态

- 输入 AST 为 `ast::SelectStmt`，输出为拥有列名、可选字段元数据和 `VecDeque<Vec<String>>` 行缓冲的 `ConcreteRecordSet`（定义见 `pkg/session/runtime/session.rs:1651-1690`）。普通九列结果不附加 catalog 字段；RU 结果显式构造字符串型 `ConcreteResultField`。
- 会话 `state: RefCell<SessionState>` 提供事务/stale-read 状态、临时表、read timeout、replica-read 请求等；代码在进入执行器前显式 `drop(state)`，避免跨后续会话调用持有动态借用（`explain_analyze.rs:106-114`）。
- `statement_ru_pending: RefCell<Option<PendingStatementRU>>` 保存 typed 执行器的终态所有权（字段见 `pkg/session/runtime/session.rs:587-598`），而不是在本函数中过早发布成功结果。
- `RUNTIME_REGION_COUNTS` 是进程级 `LazyLock<Mutex<HashMap<(domain_id, database, table, index), usize>>>`；本文件短暂加锁并立即克隆快照，后续闭包不持锁（定义见 `pkg/session/runtime.rs:675-684`，读取见 `explain_analyze.rs:609-625`）。
- `Arc<ReadStats>` 跨 store/client 请求聚合真实尝试与 read-pool 数据；`Instant` 只测本次执行/扫描耗时。闭包 `row`、`with_limit`、`scan_execution` 只格式化局部数据，不保存跨请求状态。

## 依赖与调用关系

上游调用边经 RustCodeGraph 与源码共同确认：

`ConcreteSession::execute/dispatch` → `explain_analyze_relational_select` →（可选）`explain_analyze_ru_rows`。

关键下游边包括：

- typed 链：`simple_typed_select_shape` / `simple_typed_primary_key_predicate` → `PreparePlannedKVSelect` → `SessionBoundAdapterOwner::{BindPreparedPlannedKVSelect, BuildPreparedExecStmt}` → `ExecStmt::{TypedFlatPlan, Exec}` → `RecordSet::{Next, Finish}`。
- 子查询链：`select_contains_scalar_subquery` → `explain_scalar_subquery_plan` → `scalar_plan_to_explain_analyze`。
- 通用查询链：CTE/LATERAL → `execute_insert_select_query`；LATERAL 还读取 `parallel_apply_concurrency`。
- 读取链：`domain.stats_table` → `execute_native_explain_read`；不能原生读取时调用 `scan_registered_table_at_with_index_window` 或 `scan_registered_table_with_limit`。原生函数继续调用 KV snapshot 的 `Get/BatchGet` 或 client `Send/response.Next/Close`。
- 统计链：`StmtCtx.GetExecDetails().ReadPoolTaskDetails` 与 `ReadStats::snapshot` → 本文件输出中的 `read_pool`、RPC 次数及时间。

RustCodeGraph 的精确 flow 确认 `explain_analyze_relational_select` 调用 `explain_analyze_simple_typed_select`，并把主入口调用者定位到 `pkg/session/runtime/dispatch.rs`；索引对跨模块常见名会产生噪声，因此具体边又以对应源码位置复核。

## 错误处理与边界

- typed 路径把“不适用”与“执行失败”分开：形状、环境、prepare/bind 不适用返回 `Ok(None)`；构建、执行、读取、关闭、物理计划丢失则返回带阶段上下文的 `SessionError`。准备项通过闭包后的无条件 `remove` 清理。
- `read` 与 `Finish` 都会执行；若读取先失败，调用 `RecordStatementRUFinalOutcome(false)`，随后优先传播读取错误，关闭错误仅在读取成功时传播（`explain_analyze.rs:213-232`）。
- 回退路径仅接受一个直接 `TableSource`，否则报 `EXPLAIN ANALYZE SELECT requires one table`；表不存在报 `unknown EXPLAIN table`。复杂 join 的支持应来自前面的 typed 或专用计划路径，不能假定此单表分支可覆盖。
- LIMIT 用 `saturating_add` 和 `saturating_sub` 防止 offset/count 溢出或下溢。预切分位数上限为 20，防止位移规模失控（`explain_analyze.rs:525, 606-607, 693-699`）。
- 原生读取不用于活动事务或分区表；这些场景回退，避免绕过事务覆盖层或错误解释分区语义（`explain_read.rs:15-17`）。索引窗口同样要求无事务（`explain_analyze.rs:526-535`）。
- RU 转换要求输入含 `id/task/actRows`，否则明确报错；它过滤 `_0`，且当前 RU 四个归因/详情列为空。这是明确的迁移限制。
- CTE/LATERAL 回退只生成六列聚合行；`scalar_plan_to_explain_analyze` 依赖 plan-tree 文本标记解析。这两处都不是完整的真实算子级统计，应在扩展时优先消除，而不是继续堆叠字符串启发式。

## 并发与资源生命周期

- `ConcreteSession` 内部使用 `Rc`/`RefCell`，typed `SessionBoundAdapterOwner` 以 `Arc` 包装以满足 adapter 所有权，但本文件没有创建线程或异步任务；会话对象仍应留在所属执行 worker。
- typed `RecordSet` 被完全拉取到 EOF，并调用 `Finish()` 收集执行统计。随后 `PendingStatementRU` 保留 `ExecStmt`、底层 record set 和 `StatementRUFailureGuard`；`pkg/session/runtime/typed_adapter_bridge.rs:1140-1230` 的 `SessionStatementRUScope` 在成功时记录 final outcome 并 `Close()`，失败时 `CloseWithError()`，嵌套语句退出时恢复原 pending 值。
- 原生 Coprocessor 响应在读取循环后总会调用 `response.Close()`；读取与关闭任一失败都会返回错误。每个响应子集的 read-pool 详情先合并进 `StmtCtx`，函数返回前又从共享 `ReadStats` 合并一次边界统计（`explain_read.rs:77-109`）。
- 全局 region map 通过 `Mutex` 保护；中毒时本文件使用 `expect` 并 panic，而 `runtime_domain_id` 的身份表使用 poison recovery。扩展共享状态时应维持短临界区和 domain 隔离键。
- `pkg/session/runtime/scan_adapter_runtime_test.rs:3100-3235` 验证成功、错误和 panic 后 RU terminal 责任被消费，scope depth 归零，pending/delayed 均清空，避免泄漏或重复终结。

## 与 Go 版本的对应关系

Rust 文件没有同路径的一对一 Go 文件；语义主要对应 `pkg/executor/explain.go` 的 `ExplainExec`，入口仍从 Rust session dispatch 汇合。Go `ExplainExec::executeAnalyzeExec`（`explain.go:98-168`）打开并耗尽 analyze executor、确保 Close、收集 RU runtime stats，再由 `generateExplainInfo` 调用 `RenderResult`（`explain.go:213-222`）。Rust typed 快路径保持“真实执行、拉到 EOF、关闭/终结后渲染”的生命周期意图，并通过 `PendingStatementRU` 延迟最终 session 结算。

Go 的 RU 路径通过 `registerExplainRUOperatorStats` 和 `calculateStatementRUWithOperators` 填充真实 `selfRU/cumRU/cumRU%/detail`（`pkg/executor/explain.go:170-211`）；Rust `explain_analyze_ru_rows` 目前只对齐列顺序、行顺序、字段元数据和过滤 `_0`，数值列为空，是尚未完全移植的部分。

Go 的普通 EXPLAIN ANALYZE 输出由真实 executor 的 runtime stats 与 planner `RenderResult` 产生。Rust 对简单 typed SELECT 已使用真实物理计划和实际读取；但通用回退分支仍会依据 AST、表元数据、hint、region 数和 failpoint 构造部分统计，CTE/LATERAL 只汇总根行。因此兼容性结论必须按分支说明，不能笼统声称与 Go 完全一致。

Go 的 `TestExplainAnalyzeInvokeNextAndClose`（`pkg/executor/explain_unit_test.go:112-210`）确认 Next 错误或 panic 时 Close 仍执行，并覆盖 RU snapshot 时序；Rust 的对应资源终态证据来自 `scan_adapter_runtime_test.rs` 的 terminal cases，而输出形态证据来自下列 Rust 独立测试。

## 扩展指南

- 新增算子或访问路径时，优先让它进入 typed 物理计划；若必须扩展回退分支，在 `explain_analyze_relational_select` 的互斥分支中同时定义算子树、`actRows`、task、access object、RPC/process-key 统计和 LIMIT 缩进，避免仅凭 SQL 字符串新增不完整识别。
- 扩展 native 读取应修改 `pkg/session/runtime/explain_read.rs::execute_native_explain_read`，保证事务/分区/stale-read 边界、response Close 和 read-pool 合并不变；不要把网络重试事实从 failpoint 模拟值反推出来。
- 实现真实 RU 归因时，应替换 `explain_analyze_ru_rows` 的四个空列，并与 Go `registerExplainRUOperatorStats`、forest/tree/operator 坐标语义对齐；需同步 `pkg/session/runtime/explain_query_test.rs`、`dispatch_test.rs` 和 `scan_adapter_runtime_test.rs`，测试必须仍放在独立 `*_test.rs` 文件中。
- 调整字符串 plan-tree 解析时同步测试 task marker、树缩进、`table:` 提取、`MaxOneRow` 以及包含空格/逗号的 operator info。若能获得结构化计划，应逐步替代 `scalar_plan_to_explain_analyze` 的文本启发式。
- 修改资源终态时必须覆盖成功、Next/Finish 错误、panic、嵌套 session scope 和文件传输延迟终态，确保一次且仅一次记录 outcome/关闭底层 record set。
- 兼容性风险集中在 Go 输出列与格式、真实与模拟统计混用、事务可见性和 RU 终态时序；性能风险集中在回退整表物化、重复 parse/prepare 以及 region 数放大的模拟统计。任何变更都应先用已有小范围测试固定分支行为。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点、1,848,419 条边；`files --filter pkg/session/runtime/explain_analyze.rs` 确认目标文件已索引且有 35 个符号；`node --file ... --offset 1/401` 读取完整 906 行；精确 `query` 将主入口解析为 `explain_analyze.rs::explain_analyze_relational_select`；`explore` 确认 dispatch 调用主入口以及主入口调用 typed helper 的 flow。精确 `callers/callees` 命令在本地长时间无输出后中止，故调用边同时由索引 explore 和源码位置交叉验证。
- 生产源码：`pkg/session/runtime/explain_analyze.rs`、`runtime.rs`、`dispatch.rs:3125-3180`、`explain_read.rs`、`typed_adapter_bridge.rs:1130-1235`、`session.rs:570-610,1640-1695`；crate 边界与依赖来自 `pkg/session/Cargo.toml`。
- Rust 独立测试：`pkg/session/runtime/explain_query_test.rs:96-168` 覆盖 read-pool 追加及 RU 行/空输入；`dispatch_test.rs:37-151` 覆盖 RU 七列字段元数据、实际行数、必须搭配 ANALYZE 与 plan-cache 上下文；`runtime_pessimistic_test.rs:720-835` 覆盖 LIMIT、IndexLookUp、Point_Get、process keys、region/RPC 与超时重试；`scan_adapter_runtime_test.rs:3100-3235` 覆盖 RU 终态成功、错误、panic 和资源清理。
- Go 对照：`pkg/executor/explain.go:34-238`、`pkg/executor/explain_unit_test.go:112-210`；它们提供执行器耗尽/关闭、结果渲染和真实 RU 归因的权威语义参照。
- 本任务是纯文档分析，按计划未运行 Cargo。已执行任务指定的 11 章节结构命令，并人工检查所有“已支持”结论均限定到真实分支与上述证据。
