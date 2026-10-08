# `pkg/session/runtime/control.rs` 逻辑说明

## 文件定位

`control.rs` 属于 `astersql-session` crate（`pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以私有模块 `mod control;` 编入 `runtime`。它不定义新的会话类型，而是为 `pkg/session/runtime/session.rs` 中的公开类型 `ConcreteSession` 补充控制面实现。`runtime.rs` 的模块说明把这一运行时定位为基于规范 parser、`Domain` 与 KV 接口的可执行会话实现；完整 planner/executor 的 `sessionapi::Session` 仍是另一套 ABI。因此，本文件的作用不是通用协议门面，也不是完整 TiDB Go 会话的逐行翻译，而是把事务、协议预处理、会话变量、权限、资源组、统计收集和内存仲裁等控制行为接到 `ConcreteSession` 的执行主链上。

该文件约 6,700 行，模块级实体只有两个私有辅助函数 `default_resource_group_background`、`persist_default_resource_group_background`，一个 `pub(super)` 读取函数 `load_default_resource_group_background`，以及一个大型 `impl ConcreteSession`。文件没有自定义 `struct`、`enum`、`trait`、宏或条件编译项；它使用的会话状态、锁状态和进程级注册表来自相邻模块。`pkg/session` 下未发现 `doc.go`，所以 crate 边界以 `pkg/session/Cargo.toml`、`pkg/session/runtime.rs` 和 `pkg/session/runtime/session.rs` 为准。

## 核心职责

本文件把 `ConcreteSession` 的控制职责分成以下几组：

- 事务生命周期：开始显式/隐式事务，提交或回滚，提交前只读与 schema 校验，冲突检测/受限 SQL 重试，保存点恢复，锁释放及提交后统计、TTL 和临时表收尾。关键符号为 `begin_transaction`、`ensure_implicit_transaction`、`finish_transaction_with_retry`、`create_savepoint`、`rollback_to_savepoint`。
- 悲观锁与可观测性：在真实 TiKV 上调用 `Transaction::LockKeys`，在非 TiKV 运行时维护进程内兼容锁、等待队列、wait-for 图和死锁历史；同步 `RUNTIME_TXN_INFOS` 的 SQL digest、状态及 mem-buffer 计数。关键符号为 `acquire_row_lock_mode`、`release_all_row_locks`、`begin_txn_statement_observation`、`finish_txn_statement_observation`。
- 协议 prepared statement：解析并注册单条 SQL，解析字段元数据和参数标记，安全绑定 typed 参数，维护计划缓存代际和关闭时的全局配额。关键符号为 `describe_result_fields`、`prepare_protocol_statement`、`execute_protocol_statement`、`close_protocol_statement`。
- DML 与统计辅助：为特殊的 session KV 表执行 INSERT/UPDATE/DELETE，同时把一般关系表操作转交相邻实现；收集谓词列、同步/异步直方图加载需求，处理 `ANALYZE` 列选择和 DML EXPLAIN 结果。关键符号为 `apply_session_kv_mutations`、`execute_insert`、`execute_update`、`execute_delete`、`collect_predicate_columns_point`、`analyze_columns_info`。
- 时间戳与变量控制：解析 `SET` 表达式、读写用户/系统变量，维护事务隔离、stale read、全局开关、计划缓存失效和相关 Domain 状态。关键符号为 `select_variable`、`evaluate_set_expression`、`evaluate_stale_read_ts`、`execute_set`、`SetSessionSystemVar`。
- 管理语句：维护资源组运行时映射及默认资源组持久化；执行当前具体运行时支持的用户、角色和授权变更，并拒绝必须进入完整 planner/executor ABI 的选项。关键符号为 `execute_create_resource_group`、`execute_alter_resource_group`、`execute_alter_user`、`execute_create_user`、`execute_grant_role`、`execute_grant`、`execute_revoke`。
- 内存控制：为长数据、编译期和执行期建立 tracker/仲裁 guard，清理内存敏感计划缓存并保存测试可观测值。关键符号为 `ChargeBoundLongData`、`begin_compile_memory_arbitration`、`init_statement_memory_tracker`、`begin_statement_memory_arbitration`。

## 主要符号

模块级资源组函数：

- `default_resource_group_background(options)`：把 AST 中的后台任务名称和利用率限制规范化为 `ResourceGroupBackgroundSettings`；没有有效选项时返回 `None`。
- `persist_default_resource_group_background(domain, background)`：通过 `kv::RunInNewTxn` 更新元数据键 `ResourceGroups/ResourceGroup:1`；不存在时构造默认资源组，存在时兼容带前导版本字节和直接 JSON 两种编码。
- `load_default_resource_group_background(domain)`：使用当前版本 snapshot 读取默认资源组并格式化为 `TASK_TYPES=...`、`UTILIZATION_LIMIT=...` 字符串，供 `runtime/system_query.rs` 的展示路径调用。

`ConcreteSession` 的事务/锁方法包括：`SnapCacheSizeForTest`、`TransactionIsPessimistic`、`TransactionIsolation`、`HeldRowLockCount`、`RuntimeLockWaitCount`、`RuntimeDeadlockHistoryCount`、`ClearRuntimeDeadlockHistory`、`SetPessimisticLockTTLForTest`、`ExpirePessimisticLocksForTest`、`end_expired_pessimistic_transaction`、`retriever_visible`、`transaction_visible`、`snapshots_differ_on_keys`、`transaction_mem_usage`、`acquire_row_lock_mode`、`acquire_row_lock`、`acquire_point_row_lock`、`acquire_row_locks`、`acquire_shared_row_locks`、`release_all_row_locks`、`transaction_schema_changed`、`finish_transaction`、`commit_ddl_transaction`、`finish_transaction_with_retry`、`begin_transaction`、`ensure_implicit_transaction`、`create_savepoint`、`release_savepoint`、`rollback_to_savepoint`。其中带 `ForTest` 的公开方法是观察或注入测试状态，不应作为生产 SQL 控制入口。

协议和会话入口包括：`domain`、`runtime_topology`、`SQLKiller`、`BeginProtocolResponse`、`FinishProtocolResponse`、`LastWriteSQLRespDurationForTest`、`describe_result_fields`、`prepare_protocol_statement`、`execute_protocol_statement`、`close_protocol_statement`、`SetInRestrictedSQL`、`ChargeBoundLongData`、`BoundLongDataMemorySnapshot`、`WithSessionVars`、`SetSessionSystemVar` 和 `AddSessionBinding`。其中 `execute_protocol_statement` 被 `pkg/server/runtime.rs` 与 `pkg/testkit/mockstore.rs` 直接调用，是本文件明确跨模块可见的协议执行入口。

DML/统计方法包括：`read_session_kv`、`apply_session_kv_mutations`、`execute_insert`、`execute_update`、`mysql_stats_histograms_sets_version_zero`、`execute_delete`、`explain_dml_record_set`、`collect_predicate_columns_point`、`collect_statement_predicate_stats`、`collect_expression_subquery_stats`、`collect_query_statement_predicate_stats`、`referenced_column_ids`、`prune_index_access_paths`、`collect_predicate_column_names`、`collect_select_predicate_column_names`、`collect_query_predicate_column_names` 和 `analyze_columns_info`。相邻 `runtime/query.rs`、`runtime/explain_select.rs` 调用谓词统计入口；`runtime/typed_dml_executor.rs`、`runtime/import_compression.rs` 调用 INSERT 入口。

变量、时间戳和管理方法包括：`select_variable`、`evaluate_set_expression`、`current_store_ts`、`validate_table_read_ts_after_last_commit`、`evaluate_stale_read_ts`、`execute_mem_arbitrator_set`、三个资源组执行方法、`persist_account_tls`、`execute_alter_user`、`execute_create_user`、`execute_grant_role`、`execute_set_role`、`execute_set_default_role`、`execute_grant`、`execute_privilege_mutation`、`execute_drop_user`、`execute_revoke`、`resource_group_priority` 和 `execute_set`。`runtime/dispatch.rs` 根据 AST 类型调用这些 `pub(super)` 方法。

内存相关方法包括 `compiler_quota_at_executor_build_for_test`、`begin_compile_memory_arbitration`、`clear_memory_sensitive_plan_cache`、`init_statement_memory_tracker`、`begin_statement_memory_arbitration`，以及多个 tracker 测试观察器。它们连接 `astersql-util-memory` 的 tracker/仲裁器、`SQLKiller` 和 prepared plan cache。

## 执行流程

典型 SQL 控制流程如下：

1. `runtime/dispatch.rs` 在 statement 边界调用 `begin_txn_statement_observation`，刷新 digest、staleness 标记和 `RUNTIME_TXN_INFOS`；读请求还可通过 `refresh_session_stale_read_ts` 在每次语句开始重新计算负 staleness interval。
2. dispatch 按 AST 分派。`BEGIN` 进入 `begin_transaction`，DML 在 `autocommit=0` 下先进入 `ensure_implicit_transaction`，`SET` 进入 `execute_set`，资源组/账户/授权语句进入各自控制方法；普通关系 DML 再转交 `relational_*` 实现。
3. `begin_transaction` 处理只读与 AS OF 约束，必要时结束旧事务，创建带 StartTS/async commit/1PC/pessimistic/txn scope 选项的 KV transaction，固定起始 `InfoSchema`，并重置保存点、写集、冲突上下文、TTL 计数和 MDL 状态。
4. 悲观 DML/锁定读通过 `acquire_row_lock_mode` 获取锁。真实 TiKV 使用 `LockKeys`；本地存储使用 `RUNTIME_ROW_LOCKS` 的 holder、FIFO waiter 和 wait-for 图，实现共享/独占兼容、NOWAIT、超时、kill 中断和死锁牺牲者回滚。
5. DML 把变更写入当前事务；无活动事务的 session KV 路径自行 begin/commit。`collect_predicate_columns_point` 在规划点记录列使用，并依据同步加载等待值决定同步需求或异步队列。
6. `finish_transaction_with_retry` 先分离事务和会话事务状态，再执行只读复查、schema 校验、延迟唯一/FK 校验、快照/epoch 冲突检测和 KV commit/rollback。受限内部 SQL在非 TiKV 冲突时可重放本次写集；DDL 专用 `commit_ddl_transaction` 明确关闭这种重放。
7. 无论成功失败都释放行锁和 MDL。成功提交后清理 `OnCommitDeleteRows` 临时表、推进进程内提交 epoch/`last_commit_ts`、刷新统计 delta 并上报 TTL 行计数。`finish_txn_statement_observation` 把事务恢复为 Idle 或从运行时信息表移除。

二进制协议路径单独从 `pkg/server/runtime.rs` 进入：`prepare_protocol_statement` 限制为单条语句、解析结果字段与参数数、占用全局 prepared 配额；`execute_protocol_statement` 按 token 位置绑定经过验证的 SQL literal，调用统一 `execute`，并维护计划缓存命中标记；`close_protocol_statement` 删除注册并归还配额。`BeginProtocolResponse`/`FinishProtocolResponse` 把慢日志最终写入延迟到网络响应完成之后，以补入真实 `WriteSQLRespTotal`。

## 数据与状态

本文件自身不拥有结构体定义，主要修改 `ConcreteSession` 的 `Rc`/`RefCell` 会话状态与相邻模块的全局注册表：

- `ConcreteSession` 与 `ConcreteSessionInner` 定义在 `runtime/session.rs`。`self.state` 保存活动 transaction、事务模式/隔离级别、stale timestamp、写键和关联表、保存点、prepared statement、用户变量、warnings、内存仲裁配置、计划缓存状态等；`self.session_vars` 保存规范 `SessionVars` 和 statement context。
- `RuntimeRowLockKey`、`RuntimeRowLockMode`、`RuntimeRowLockState`、`RuntimeTxnInfo`、`RUNTIME_ROW_LOCKS`、`RUNTIME_TXN_INFOS` 定义在 `runtime/transaction.rs`。会话用 `row_lock_owner` 关联 holder/waiter/事务信息；锁键同时参与提交冲突检测。
- `RUNTIME_DEADLOCK_HISTORY` 定义在 `runtime/system_query.rs`，为 `information_schema` 兼容路径保留有限死锁边；`RUNTIME_TOPOLOGIES` 位于 `runtime.rs`；资源组、提交 epoch、prepared 计数等其他运行时 map/atomic 也由 `runtime.rs` 或相邻模块装配后通过 `super::*` 引入。
- 保存点不是底层 KV 原生 savepoint：`create_savepoint` 扫描 transaction 可见键值形成 `BTreeMap` 快照，并保存当时持有锁、延迟约束错误和 TTL 计数；`rollback_to_savepoint` 通过 Set/Delete 恢复差异并释放保存点之后新增的进程内锁。
- prepared state 同时保存 SQL 文本、是否已规划、具名 prepared 缓存和缓存 generation；改变影响语义/计划的系统变量时，`execute_set` 会清空或标脏相应缓存。
- 资源组背景设置中的任务名会 trim、转小写、过滤空项；普通运行时资源组按 `runtime_domain_id` 隔离，避免多个 Domain 共享错误状态。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 文件节点与 `rg` 交叉核对：索引显示 `control.rs` 被 38 个 Rust 文件使用。主链中，`runtime/dispatch.rs` 调用事务开始/结束、SET、DML、内存仲裁等方法；`runtime/query.rs` 与 `runtime/explain_select.rs` 调用谓词列收集；`runtime/dml.rs`、`runtime/scan_adapter_runtime.rs` 调用事务结束；`pkg/server/runtime.rs` 调用 `execute_protocol_statement`；`pkg/testkit/mockstore.rs` 同样通过 prepared 协议接口驱动测试。`runtime/system_query.rs` 调用 `load_default_resource_group_background` 展示默认资源组后台配置。

下游依赖按职责分层：

- 存储与事务：`astersql-kv` 的 `Storage`、`Transaction`、`Snapshot`、`Retriever`、`LockCtx`，以及 `astersql-meta` 的 `SnapshotReader` 和事务元数据键。
- catalog 与全局服务：`astersql-domain::Domain` 提供 `info_schema`、storage、statistics handle、全局变量和权限/后台工作负载接线；`astersql-infoschema` 与 schema validator 用于提交时版本检查。
- SQL 结构：`astersql-parser-ast` 提供所有语句/表达式节点；相邻 `dml_runtime`、`relational_*`、`planning` 和 `query` 模块承担计划或数据执行细节。
- 权限与管理：`astersql-privilege-privileges` 负责静态/动态权限验证和内存权限缓存，`astersql-executor::grant` 规范化账户 TLS 选项，受限 SQL mutation 更新 `mysql.*` 表。
- 资源/统计/内存：`astersql-resourcegroup`、`astersql-statistics-handle*`、`astersql-sessionctx-*`、`astersql-util-memory`、`astersql-util-sqlkiller` 分别承接资源组、统计加载、变量/StmtCtx、tracker/仲裁与取消信号。

`pkg/session/Cargo.toml` 声明 crate 名 `astersql-session`，`lib.rs` 为 crate 根，feature 仅有向 deploy/kernel crate 透传的 `nextgen`；`control.rs` 没有 feature gate。Cargo 的 `[package.metadata.porting] go-package = "pkg/session"` 表明其迁移边界对应 Go `pkg/session`，而不是单一 Go 文件。

## 错误处理与边界

所有可失败控制方法统一返回 `SessionResult<T>`，外部错误通过 `session_error("操作上下文", error)` 包装，语义错误使用 `SessionError::new` 保留 MySQL/TiDB 兼容文案和错误码。关键边界包括：

- 悲观锁区分 NOWAIT 3572、等待超时 1205、最大执行时间 3024、死锁 1213 和 kill 中断；检测到本地死锁时会先回滚牺牲事务，避免残留锁继续阻塞幸存者。
- 提交重新检查 restricted/super read-only，防止语句规划后管理员打开只读开关；schema 变更仅对实际写入或 locking read 涉及的表报 8028。真实 TiKV 的 MVCC/prewrite 为冲突权威，本地存储才补充 snapshot/epoch 检查。
- stale read 不能与冲突的 pending 模式组合，普通显式未来时间被拒绝；`tidb_bounded_staleness` 把 safe TS 限制在上下界。显式 stale/snapshot read 有意绕过 last-commit linearizability 检查。
- 保存点不存在时报 1305；特定悲观模式下关闭 in-place constraint check 时拒绝保存点。无活动 transaction 时创建保存点是空操作，而回滚到保存点要求活动 transaction。
- prepared SQL 必须恰好一条；未知 statement id、全局 prepared 配额耗尽、参数绑定失败会在执行前返回错误。参数只在 parser 识别的 marker 位置替换，字符串和注释中的问号不参与绑定。
- session KV UPDATE/DELETE 仅接受 `WHERE k = literal`；重复键、非 UTF-8 值和未知列显式报错。普通表 DML 不在这里简化执行，而是转交相邻关系实现。
- 用户/权限方法先检查 CREATE USER、UPDATE、SYSTEM_USER、RESTRICTED_USER_ADMIN 等权限，并对当前具体 ABI 不支持的认证、资源或双密码选项返回“需要完整 planner/executor session ABI”，不伪装成功。
- `execute_set` 先依据 sysvar metadata 验证 scope/只读性，再计算右值；各变量分支负责规范化、warning、Domain/atomic 更新和计划缓存失效。新增变量若只写 `SessionVars` 而漏掉本地镜像状态，可能造成读写语义分裂。

## 并发与资源生命周期

`ConcreteSession` 的大部分可变状态使用 `RefCell`，意味着它按单会话线程内串行访问设计；跨会话状态才使用 `Mutex`、`Condvar` 和 atomic。持锁等待每 10ms 最多唤醒一次，以便检查 deadline 和 `SQLKiller`。所有 poisoned mutex 均通过 `PoisonError::into_inner` 继续清理，避免锁表因一次 panic 永久不可用。

行锁生命周期从悲观事务/锁定语句获取开始，记录在会话 `held_row_locks` 和全局 holder map；commit、rollback、死锁牺牲、TTL 过期都会走 `release_all_row_locks` 并通知所有 waiter。真实 TiKV 分支仍把已获取键记入会话集合，以便事务边界统一清理，但等待与死锁事实由 TiKV lock manager 决定。

`finish_transaction_with_retry` 先把 transaction 从 `state` 中 `take` 出来，避免提交过程中会话仍表现为活动事务；局部 `ReleaseMDL` 的 `Drop` 保证函数任意退出路径清理 MDL。最小 commit TS failpoint 前会提前释放进程内行锁，注释明确用于 panic unwind 不泄漏 ownership。prepared statement 在注册时 reserve、close 时 release；若后续新增错误路径，必须保证已 reserve 的计数能被归还。

statement memory tracker 作为 session root tracker 的子节点存在，由初始化/仲裁 guard 在语句结束时 detach、释放配额并保留峰值供测试读取；`ChargeBoundLongData` 在 Consume 前检查限额，避免无响应的 `COM_STMT_SEND_LONG_DATA` 触发 OOM action。协议慢日志可暂存到 `pending_protocol_slow_logs`，直到 `FinishProtocolResponse` 得到网络写耗时后才最终写出。

## 与 Go 版本的对应关系

Cargo porting 元数据把整个 crate 对应到 `pkg/session`。本文件的事务结束与 `pkg/session/session.go` 的 `CommitTxn`/`RollbackTxn` 对齐：两者都在事务边界清理上下文、维护 last commit TS、更新统计并上报 TTL 计数；Rust 同时为本地可执行运行时补充进程内锁、epoch 冲突和临时表清理。Rust 的 `commit_ddl_transaction` 特意不重放 mutation，因为 Go DDL worker 会重新加载 Job 并检查 owner。

协议方法对应 `pkg/session/session.go` 的 `PrepareStmt`、`ExecutePreparedStmt`、`DropPreparedStmt`。Go 路径使用完整 preprocess/plan builder/executor 和 `PlanCacheStmt`；Rust 路径解析规范 AST、推导字段、进行 token-aware literal 绑定后回到 `ConcreteSession::execute`。因此 Rust 行为覆盖当前具体 runtime 所需的协议语义，但不能据此宣称实现了 Go prepared pipeline 的全部 optimizer/executor 能力。

谓词统计方法对应 Go planner 的 `CollectPredicateColumnsPoint`、索引裁剪和 statistics sync-load 约定；Rust 注释明确处理 plan-cache hit 跳过收集、静态分区展开、同步/异步 histogram 请求和单列全扫描 issue 路径。账户、角色、授权和资源组语义则分散对应 `pkg/executor/simple.go`、`pkg/executor/ddl.go`、`pkg/session` 资源组测试以及 privilege 实现，而非一个同路径 `control.go`。

Rust 文件中的 Go 对齐注释还明确覆盖 `ResetContextOfStmt` 的 DML strict flags、`SessionVars.InRestrictedSQL`、`MemTracker.GetChildrenForTest`、`DataSource.IsSingleScan`/`pruneIndexesForDataSource`、`getMustAnalyzedColumns` 和原生 sysvar getter。差异应视为明确边界：Rust 使用 `ConcreteSession` 专用状态和若干进程内兼容设施；遇到需要完整 ABI 的账户选项会拒绝，而不是做降级实现。

## 扩展指南

新增事务或锁行为时，优先修改 `begin_transaction`、`finish_transaction_with_retry`、`acquire_row_lock_mode` 及 `runtime/transaction.rs` 的状态定义，并同时覆盖真实 TiKV 与非 TiKV 分支。必须逐项检查成功、显式回滚、提交错误、deadlock、kill、timeout、panic/failpoint 和 schema changed 路径是否释放锁/MDL、清空保存点和更新 `RUNTIME_TXN_INFOS`。对应回归首先扩展 `pkg/session/runtime_pessimistic_test.rs`、`pkg/session/runtime/transaction_test.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs`；计划缓存相关事务语义扩展 `pkg/session/plan_cache_runtime_test.rs`。

新增 sysvar 时，先确认 `astersql-sessionctx-vardef`/`variable` 的名称、类型、scope、默认值和 validation，再决定 `execute_set` 是否还需同步 `state`、Domain、atomic、统计配置或 prepared cache。读取路径也要同步 `select_variable`。变量回归应放在现有独立测试，如 `pkg/session/test/variable/variable_test.rs`、`pkg/session/test/vars/vars_test.rs` 或 `pkg/session/runtime/ttl_sysvar_test.rs`，不要把测试嵌入生产文件。

新增协议 prepared 行为时，保持“只接受单 statement、marker 感知绑定、reserve/release 对称、缓存 generation 失效”四个不变量，并同步 `pkg/server/runtime.rs`、`pkg/testkit/mockstore.rs` 的调用契约。新增统计收集行为应从 `collect_statement_predicate_stats`/`collect_predicate_columns_point` 接入，避免在已裁剪索引上请求 histogram，同时覆盖 CTE、集合运算、表达式子查询和静态分区。

管理语句扩展不能只修改内存 cache：账户/权限修改需在 transaction 中持久化 `mysql.*` 后再更新 privilege handle，失败时回滚；资源组默认项需区分元数据持久化和当前 Domain 的 runtime map。兼容风险主要是错误码/文案、scope 与 warning；正确性风险主要是事务清理、权限绕过和 cache/持久化不一致；性能风险集中在保存点全量扫描、谓词列/索引遍历、提交前 snapshot 比较以及锁等待轮询。

## 验证依据

本说明读取并核对了以下路径：生产源 `pkg/session/runtime/control.rs`；crate/模块边界 `pkg/session/Cargo.toml`、`pkg/session/runtime.rs`、`pkg/session/runtime/session.rs`；共享事务状态 `pkg/session/runtime/transaction.rs`；死锁/资源组展示 `pkg/session/runtime/system_query.rs`；直接入口 `pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/query.rs`、`pkg/session/runtime/explain_select.rs`、`pkg/server/runtime.rs`、`pkg/testkit/mockstore.rs`；Go 对照 `pkg/session/session.go`、`pkg/session/txn.go`、`pkg/executor/simple.go`、`pkg/planner/core/stats.go`。

独立 Rust 测试证据包括 `pkg/session/runtime_pessimistic_test.rs`（事务模式、锁、死锁、TTL）、`pkg/session/runtime/scan_adapter_runtime_test.rs`（锁与扫描交互）、`pkg/session/runtime/transaction_test.rs`（事务边界）、`pkg/session/plan_cache_runtime_test.rs`（锁和编译配额/缓存）、`pkg/session/dml_runtime_test.rs`（DML）、`pkg/session/mysql_privilege_persistence_test.rs`（账户权限持久化）、`pkg/session/test/variable/variable_test.rs` 与 `pkg/session/test/vars/vars_test.rs`（SET/sysvar）。Go 测试参考包括 `pkg/session/test/common/prepare_dedup_cache_test.go`、`pkg/session/test/schematest/schema_test.go`、`pkg/session/test/resourcegrouptest/resource_group_test.go`。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime/control.rs` 识别该文件 544 个符号；文件节点报告其被 38 个文件使用，并成功展示源码。对 `ConcreteSession` 查得定义位于 `runtime/session.rs:587`。索引未把本文件 `impl ConcreteSession` 内多数方法按方法名暴露给 `query/callers/callees`，所以具体调用边用模块范围 `rg` 补证，并在上文只陈述找到的直接调用关系。最终还需以任务规定的命令确认文档存在且恰有 11 个固定二级章节；本任务为纯文档分析，按计划不运行 Cargo 或代码测试。
