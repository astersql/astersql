# `pkg/session/runtime/dispatch.rs`

## 文件定位

源文件为 [`dispatch.rs`](./dispatch.rs)。它是 `astersql-session` crate 中 `runtime` 模块的 SQL 请求分发层。模块入口 `pkg/session/runtime.rs` 以私有 `mod dispatch;` 装入它；文件的大部分实现通过 `impl ConcreteSession` 扩展 `runtime/session.rs` 定义的具体会话，而不是定义新的会话类型。对外执行入口是 `ConcreteSession::execute`，测试/工具可通过 `TestSession` 实现或 `ExecuteWithSlowLogLogger`、`execute_with_failpoint_hook` 进入同一生命周期。

它位于“SQL 文本/AST”与具体执行子模块之间：先解析并建立逐语句上下文，再由 `execute_statement` 按 AST 类型路由到 `ddl.rs`、`dml.rs`、`query.rs`、`system_query.rs`、`statistics.rs`、`control.rs` 等实现。它本身也承载跨执行器的会话级契约，例如事务边界、MDL 清理、stale read 限制、绑定与计划缓存、日志、告警、内存跟踪和结果状态发布。

crate 归属由 `pkg/session/Cargo.toml` 的 `[package] name = "astersql-session"` 和 `[lib] path = "lib.rs"` 确认；`nextgen` feature 会把内核/部署模式 feature 传给配置 crate，`execute_statement` 还会在 NextGen 模式拒绝若干系统对象 DDL。该文件不是薄门面：当前源码约 4,638 行，包含真实分发、状态维护和资源回收逻辑。

## 核心职责

1. `execute_with_logger_and_hook` 将 SQL 解析为一个或多个 AST，逐条初始化 statement context、trace、RU、hint/binding、内存 tracker、告警与慢日志上下文，再调用 `execute_statement`。
2. `execute_statement` 执行统一前置校验并按 AST 动态类型分发。它处理会话/权限/事务控制、DDL、DML、查询、EXPLAIN、PREPARE/EXECUTE、Plan Replayer、绑定、管理语句等；具体业务操作尽量下沉到相邻 runtime 子模块。
3. `execute_prepare`、`execute_prepared` 与 `execute_simple_prepared_select_through_adapter` 管理命名预处理语句、参数绑定、schema/事务上下文感知的计划缓存，以及简单主键查询的 typed adapter 快路径。
4. `SessionPlanReplaySource` 把当前 `ConcreteSession` 适配为 Domain Plan Replayer 的数据源，并由 `execute_plan_replayer_dump/load` 负责归档写入和回放。
5. `record_replica_read_request`、`record_select_request`、`record_statement_metric`、`record_runtime_statement_plan`、`record_last_query_info` 和 `log_general_query` 发布执行观察数据。
6. `split_statement_sql`、`quote_argument`、`bind_parameters` 提供多语句文本切分和测试/内部 prepared 参数替换；前两者由 `runtime.rs` 在 crate 内重导出。
7. `Drop for ConcreteSessionInner` 在会话销毁时释放 prepared 配额、回滚残留事务、清除 MDL/行锁及事务观测表。

## 主要符号

- `SessionPlanReplaySource<'a>(&'a ConcreteSession)`：私有适配器，实现 `astersql_domain::plan_replayer_dump::PlanReplaySource`。它从会话解析表/视图依赖，执行 `SHOW CREATE`/`EXPLAIN`，导出 binding；`stats`、TiFlash 副本、配置等部分目前返回精简占位内容，不能理解为 Go Plan Replayer 的完整等价实现。
- `strip_optimizer_hint_comments`、`binding_sql_parts`：为 binding SQL 提取原始语句和带 hint 语句；前者只剥离闭合的 `/*+ ... */` 并规范化空白，未闭合 hint 会原样保留剩余文本。
- `dp_join_reorder_ignores_leading_hint`：当高级 join reorder 启用、表源数在阈值内且存在 `LEADING` hint 时识别不适用组合，外层据此产生 warning。
- `validate_alter_column_charset`、`format_placement_policy_options`、`sql_option_u64`：分别处理 ALTER 列字符集/排序规则一致性、placement 选项序列化及 SQL 选项中的无符号整数提取。
- `ConcreteSession::execute_statement(&dyn ast::Node, Option<&str>)`：核心 AST 分发器，返回 `Option<ConcreteRecordSet>`；`None` 表示无结果集的语句。
- `ConcreteSession::execute_with_logger_and_hook`：完整语句生命周期协调器。`execute_with_logger`、公开 `execute`、强制慢日志入口和请求局部 failpoint hook 最终汇入此处。
- `ConcreteSession::execute_prepare` / `execute_prepared`：SQL `PREPARE`/`EXECUTE` 路径；区别于文件末尾 `TestSession::PrepareStmt`/`ExecutePreparedStmt` 的数值 ID 测试接口。
- `ConcreteSession::record_select_request`：构造一个主请求和若干辅助请求，用 scoped threads、`Barrier` 与原子计数记录并行分支和 store 选择；结果写入 `SessionState::last_select_request` 供确定性检查。
- `split_statement_sql`：识别单/双引号、反引号、反斜杠、块注释、`#` 注释及符合 MySQL 空白规则的 `--` 注释，只在这些结构之外按分号切分。
- `quote_argument` / `bind_parameters`：字符串转义和 `?` 替换。后者不替换引号内问号，支持测试协议的 NULL/数值标记，并拒绝参数过少或过多。
- `Drop for ConcreteSessionInner`：会话资源最终清理防线。

## 执行流程

1. `ConcreteSession::execute(sql)` 调用 `execute_with_logger(sql, None)`，再进入 `execute_with_logger_and_hook`。入口先恢复未完成的 ADD INDEX 任务，并处理请求局部 `mockGetTSFail` 注入。
2. SQL 按当前 `sql_mode` 解析；restricted SQL 会暂时去除 `NO_BACKSLASH_ESCAPES`。AST 列表与 `split_statement_sql` 产生的原文本片段按索引配对，因此每条语句保留自己的日志/hint 文本。
3. 每条语句先清除 scalar-subquery registry，设置 EXPLAIN 上下文，重置 DistSQL-cache、warning 和统计同步等待状态；随后生成 trace ID，建立 statement RU scope 和挂到 session tracker 的内存 tracker。
4. `StartStatementHintsWithBindings` 同步全局 binding，选择普通语句、prepared 模板或 EXPLAIN 子句作为 hint 匹配对象，并用 guard 保证 `SET_VAR` 等临时变量在末尾恢复。
5. `begin_statement_memory_arbitration` 成功后调用 `execute_statement`。该分发器先设置本语句是否写入、`SQL_NO_CACHE` 状态，执行 SEM/NextGen/stale-read 前置限制，然后按 AST 类型路由。
6. 分发的主要顺序是：数据库/Plan Replayer/用户权限与 binding；隐式提交型 DDL；导入与管理语句；事务和会话控制；DML；EXPLAIN/集合操作/DO/SELECT；最后是 PREPARE、EXECUTE、DEALLOCATE。未识别 AST 返回“需要完整 planner/executor session ABI”。顺序很重要，例如权限/DDL 必须在普通查询分支之前处理。
7. SELECT 会依次尝试 information schema、deadlock/DDL/statistics 系统表、完整关系查询、精简关系查询、常量查询，最后才落到会话 KV 兼容读取。表查询前会确保隐式事务、校验 read TS、登记 MDL并检查 GROUPING/`ONLY_FULL_GROUP_BY`。
8. 执行后统一记录指标/计划/General Log，合并 statement warnings，完成事务观察并恢复 hint 状态；随后写慢日志和 last-query-info，结算内存 tracker。
9. 成功结果还会应用 SHOW `WHERE` 过滤、`sql_select_limit` 截断，并更新 `FOUND_ROWS`/`ROW_COUNT`/`LAST_INSERT_ID`。任一执行、hint 恢复或事务观察错误都会终止当前批次并向调用者传播。

Prepared 路径中，`execute_prepare` 只解析并验证单语句，然后保存 SQL、当前数据库和缓存元数据。`execute_prepared` 读取用户变量、校验 LIMIT 参数、绑定 marker，并依据全局 cache generation、catalog version、事务/脏表上下文、参数形状、partition pruning、NULL/负无符号比较、hint/binding 等因素决定复用或重建计划。满足严格条件的单表列投影/主键等值 SELECT 可进入 typed adapter；不满足或 encoder 不支持时安全回退普通执行。

## 数据与状态

主要可变数据位于 `ConcreteSession::state: RefCell<SessionState>`，本文件读写以下类别：当前数据库、事务与 stale-read 时间戳、prepared 映射及计划缓存标志、用户变量、binding 观察、warning、DML report、协议状态、最后查询/计划/副本读请求、临时表和事务写集合。`RefCell` 表明这些操作依赖会话线程所有权，不提供跨线程共享的内部可变性。

`execute_statement` 用局部 `StatementMDL` RAII guard 保存并恢复 `mdl_autocommit_write`；若语句进入时不是写路径且结束后没有事务，它清理 transaction MDL、表/库集合及 metadata error。事务开始、隐式提交、提交/回滚和 stale read 的真实状态转换委托给 `transaction.rs` 中的方法。

Prepared 状态同时有两套接口：SQL `PREPARE name` 使用 `prepared_by_name: HashMap<... NamedPreparedStatement>`；`TestSession` 二进制风格接口使用数值 ID 的 `prepared` 映射。全局配额通过 `runtime_prepared_stmt_reserve/release` 维护，替换/DEALLOCATE/会话 Drop 都需要同步释放或删除 typed plan。

每条语句拥有独立的 `Tracker`、RU scope、trace context、hint guard 和 warning 集合。执行成功后 tracker 被 detach，仅最后一个 tracker 保留用于观察；慢日志可立即写入，也可在 `defer_protocol_finish` 时排入 `pending_protocol_slow_logs`。

Plan Replayer load 会暂时执行 `SET FOREIGN_KEY_CHECKS = 0`，按归档路径排序创建数据库/对象，再恢复为 1 并加载 binding。当前实现如果中途返回错误，没有单独的 guard 保证该变量恢复，这是扩展或修复时应关注的状态边界。

## 依赖与调用关系

上游已验证入口：

- `runtime.rs` 声明私有 `dispatch`，并从中重导出 `quote_argument`、`split_statement_sql`；`runtime/session.rs::SplitSQLStatements` 调用后者。
- `ConcreteSession::execute` 是 runtime 测试和同 crate 业务代码的主要文本 SQL 入口；`TestSession::Execute` 将结果包装成 trait object。
- `runtime_test/session.rs` 调用 `ExecuteWithSlowLogLogger`；`tests/realtikvtest/sessiontest/session_fail_test.rs` 调用 `execute_with_failpoint_hook`。
- `ttl_timer_store.rs`、`ttl_worker_session.rs`、`starter_bootstrap_file.rs` 和 `runtime/session.rs` 复用 `quote_argument`。

下游依赖按职责分组：

- 解析/AST：`astersql-parser`、`astersql-parser-ast`、`astersql-parser-mysql`；
- 计划与执行：`astersql-planner-core*`、`astersql-executor*`、`planning.rs`、`typed_adapter_bridge.rs`；
- 存储/事务：`astersql-kv`、Domain storage、`transaction.rs`、`relational_scan.rs`；
- DDL/DML/查询：同目录 `ddl.rs`、`dml.rs`、`query.rs`、`source.rs`、`statistics.rs`、`system_query.rs`、`control.rs` 等；
- 会话治理：`astersql-sessionctx-*`、binding、privilege、SEM、resource group、SQLKiller、memory tracker、metrics/log/trace；
- Plan Replayer：`astersql-domain::{plan_replayer, plan_replayer_dump}`、`astersql-util-replayer`、`astersql-planner-extstore`。

RustCodeGraph 的文件节点报告 `dispatch.rs` 被 11 个文件使用，并展示了 `pkg/session/runtime/ddl.rs` 等直接关系；对本文件 `impl ConcreteSession` 方法执行的精确 callers/callees 查询没有返回方法边，因此以上方法级关系进一步由模块重导出和仓库调用点核对，而非把缺失图边当成“没有调用”。

## 错误处理与边界

本文件统一返回 `SessionResult<T>`，底层错误通常通过 `session_error(context, error)` 或 `SessionError::new` 增加语境。解析错误、权限拒绝、非法 stale-read 写入、缺失表、未知 prepared 名称、参数数量不匹配、计划缓存校验失败、存储错误和 hint 恢复错误均向上返回；多语句执行在首个错误处停止。

重要行为边界包括：

- SEM 下 restricted SQL 要求 `RESTRICTED_SQL_ADMIN`；NextGen 下特定系统库/表 DDL被拒绝。
- stale transaction 和 pending snapshot 都禁止写入与 locking SELECT；EXPLAIN/EXPLAIN ANALYZE 对 DML 也有对应限制。
- 非临时表 DDL、资源组 DDL及用户/权限变更遵循相应隐式提交边界；本地临时表创建被排除在普通 DDL 隐式提交之外。
- `EXPLAIN FORMAT=RU` 必须和 ANALYZE 一起使用；非 ANALYZE EXPLAIN 的受支持子句有显式类型限制。
- `split_statement_sql` 只负责把原 SQL 配给已解析 AST，不代替 parser；`bind_parameters` 是测试/内部适配层字面量替换，不应作为通用 SQL 参数协议。
- OOM action 为 Cancel 且未启用 rate-limit 时，执行后的估算内存费用可能转换成 `[executor:8175]`；否则 tracker 使用日志或限速 action。
- `SessionPlanReplaySource::stats` 等返回的是当前 Rust runtime 的简化数据；`PLAN REPLAYER CAPTURE/REMOVE` 明确返回尚未接线错误。

## 并发与资源生命周期

`ConcreteSession` 的主体状态使用 `RefCell`/`Cell`，设计为归属一个会话执行线程；不能据此假设同一实例可并行调用。跨请求取消通过共享 `Arc<SQLKiller>` 与原子 connection ID 传播。

`record_select_request` 是文件内显式并发点：它用 `std::thread::scope` 为每个请求分支启动 scoped worker；`Barrier` 让分支同时进入 dispatch，`AtomicUsize` 统计实时与最大并发，`Mutex<Vec<_>>` 收集结果，scope 退出保证 worker 已 join，随后按 branch 排序以恢复确定性。request 使用 `Arc`，记录地址用于确认分支持有独立请求对象。

每条 SQL 的资源顺序由局部对象约束：RU scope、hint guard、内存 arbitration guard、statement tracker 和 `StatementMDL` 均在语句结束时结算/恢复。即使 `execute_statement` 返回错误，外层仍记录 General Log、warning、慢日志、last query info，并尝试恢复 hint；但执行错误优先于后续恢复/观察错误返回。

会话析构时 `ConcreteSessionInner::drop` 清理 MDL，按两套 prepared 容器计数释放全局配额，回滚仍存活事务，释放该 row-lock owner 的所有锁，并从 `RUNTIME_TXN_INFOS` 删除记录。析构中的事务回滚错误被忽略，因为此时已经无法向调用者报告。

## 与 Go 版本的对应关系

`pkg/session/Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/session"` 指明 Go 对照包。最接近的主链位于 `pkg/session/session.go`：

- Rust `execute`/`execute_with_logger_and_hook` 对应 Go `(*session).Execute` 的文本解析入口，以及 `ExecuteStmt`/`executeStmtImpl` 的逐语句生命周期；Rust 当前支持多语句列表，而 Go 注释标明的旧 `Execute` API只接受单语句。
- Rust `execute_statement` 将一部分 Go `executeStmtImpl` 中的前置/收尾职责与具体 AST 分发集中在同一文件；Go 主路径通过 compile 后的 `executor.ExecStmt` 统一执行，因此两者是契约对齐而非结构一一对应。
- Rust `execute_prepare` 明确采用“PREPARE 只解析，EXECUTE 再计划”的 SQL 路径；Go `PrepareStmt` 当前会创建 `PrepareExec`、预处理并支持 prepare dedup cache。Rust `execute_prepared` 自行维护 named plan-cache 与 typed adapter 快路径，不能视为 Go `PlanCacheStmt` 内部实现的逐行翻译。
- Rust `TestSession::PrepareStmt`/`ExecutePreparedStmt` 对应 Go 数值 ID API 的用途，但 Rust 测试接口通过文本替换后重新执行，和 Go `ExecutePreparedStmt` 构造 `ast.ExecuteStmt`、携带表达式参数的生产协议不同。
- Rust `log_general_query` 对齐 Go `logGeneralQuery` 的核心字段：连接、用户、schema version、事务时间戳、隔离/悲观模式、当前库和 SQL；Go 还具有参数脱敏/替换等更完整的生产日志语义。
- Rust 的注释和逻辑刻意复现 Go 的 DDL 隐式提交、statement context reset、Plan Replayer schema 顺序、prepared cache 命中跳过优化等行为；差异或尚未接线项在代码中以显式错误/占位值暴露。

## 扩展指南

新增 AST 语句时，应先判断它属于前置安全校验、隐式事务边界还是普通执行分支，再在 `execute_statement` 的正确顺序中接入，并把具体实现放到对应独立模块。不要把大型 DDL/DML 算法继续堆入本文件；本文件应保留跨语句协调与路由职责。

修改查询路径时需同步检查：`record_replica_read_request`、MDL、stale read/read TS、`ONLY_FULL_GROUP_BY`/GROUPING 校验、SELECT INTO、结果限制及事务观察。新增写语句还需加入 `writes`、stale-read 禁止列表、内存费用和隐式事务/提交判断，避免只有执行分支而遗漏会话契约。

扩展 prepared cache 时必须同时考虑全局 generation、catalog version、当前数据库、事务/脏表上下文、参数 shape、partition 模式、binding hint 与 typed plan ID 的失效；释放路径至少覆盖替换、DEALLOCATE 和 Drop。参数语义变化应修改独立测试，不要在 `dispatch.rs` 内嵌测试。

修改 SQL 文本拆分或引用规则时，在 `pkg/session/runtime/dispatch_test.rs` 扩展独立单元测试，至少覆盖引号、转义、块/行注释和 MySQL `--` 空白规则。日志、慢日志和 statement 生命周期场景主要在 `pkg/session/runtime_test/session.rs`；failpoint 的真实 TiKV 入口在 `tests/realtikvtest/sessiontest/session_fail_test.rs`。本文件的其他行为还由大量 session compatibility/runtime 测试间接覆盖。

Plan Replayer 扩展应优先补齐 `SessionPlanReplaySource` 的真实 stats/TiFlash/config/metadata 数据，并为 load 期间 `FOREIGN_KEY_CHECKS` 使用可恢复 guard；同时核对 Go 对应归档布局与错误契约。涉及并行 SELECT 观察时，应保留 scoped join、确定性排序与 request 所有权不变量。

## 验证依据

- RustCodeGraph：`status` 确认本地索引包含 7,032 个 Rust 文件；`files --filter pkg/session/runtime` 确认 `dispatch.rs` 和独立 `dispatch_test.rs` 均已索引；`node --file pkg/session/runtime/dispatch.rs` 分段读取了完整文件的关键区间，并确认文件级“used by 11 files”。
- RustCodeGraph 符号查询：`query split_statement_sql --kind function` 定位到 `dispatch.rs:4442`；对 `impl ConcreteSession` 私有方法执行 query/callers/callees 未返回可用方法边，因此没有据此推断不存在调用。
- 源码与模块：`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/session.rs`；后两者确认模块装配、重导出和 `SplitSQLStatements` 调用。
- crate/feature：`pkg/session/Cargo.toml`，确认 crate 名、库入口、Go package 映射、`nextgen` feature，以及 parser/domain/KV/planner/executor/sessionctx/memory/log/trace 等依赖边界。
- 独立 Rust 测试：`pkg/session/runtime/dispatch_test.rs` 验证 MySQL `--` 注释切分规则及 EXPLAIN ANALYZE RU 的 schema、行数、错误与 plan-cache context；`pkg/session/lib.rs` 通过 `#[path = "runtime/dispatch_test.rs"] mod runtime_dispatch_test;` 独立挂载，符合测试不与源文件同置的约束。
- 直接调用点：`pkg/session/runtime_test/session.rs`、`tests/realtikvtest/sessiontest/session_fail_test.rs`、`pkg/session/runtime/ttl_timer_store.rs`、`pkg/session/runtime/ttl_worker_session.rs`、`pkg/session/starter_bootstrap_file.rs`。
- Go 对照：`pkg/session/session.go` 中 `Execute`、`ExecuteStmt`、`executeStmtImpl`、`PrepareStmt`、`ExecutePreparedStmt`、`logGeneralQuery`。
- 本任务是只读分析加 Markdown 文档，不修改运行时代码，按任务约束未运行 Cargo。交付前运行任务指定的 11 章节结构命令，并额外检查变更范围、源文件链接/符号引用和文档差异。
