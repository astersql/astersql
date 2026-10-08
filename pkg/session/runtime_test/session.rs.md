# `pkg/session/runtime_test/session.rs`

## 文件定位

本文件是 `astersql-session` crate 的会话运行时集成测试分片，源码见 [`session.rs`](./session.rs)。它不属于生产编译路径：`pkg/session/lib.rs` 只在 `#[cfg(test)]` 下加载 `runtime_test.rs`，后者再以 `#[path = "runtime_test/session.rs"] mod session;` 引入本文件。文件通过 `use super::*` 复用父测试模块中的 `runtime()`、`concrete_session()`、`ConcreteSession`、`CanonicalSessionFactory`、日志类型以及 `Arc`、`HashMap`、`SystemTime` 等测试设施。

因此，这个文件存在的目的不是提供可调用 API，而是从 SQL 和协议可观察面验证 `pkg/session/runtime/session.rs` 等生产实现。当前共有 17 个私有 `#[test]` 函数、一个私有测试替身 `StaticSessionManager`，以及它对 `InfoSchemaCoordinator`、`Manager` 两个 trait 的实现；没有模块级常量、公开函数、条件编译项或可供其他模块复用的生产类型。

## 核心职责

测试覆盖可归为八组契约：

1. 会话所有权与隔离：克隆会话共享同一 `Rc<ConcreteSessionInner>`，工厂创建的不同会话共享 Domain/存储但隔离当前数据库等连接状态。
2. SQL 生命周期：解析、预编译、参数执行、提交、回滚、结果集读取与关闭形成闭环，并验证生产 `ConcreteRecordSet` 和协议状态不依赖测试 trait。
3. 模式与变量：TiFlash Cop 开关同步到规划变量；普通表、无关表、全局临时表的 schema 变更在提交时具有不同影响；只读陈述、全局默认值和 Connector/J 探测变量保持兼容语义。
4. 统计同步加载：真实 Domain 统计句柄、加载队列和 `SessionStatsSyncLoadAdapter` 同时覆盖成功发布及零超时错误路径。
5. 计划重放：`PLAN REPLAYER DUMP/LOAD` 生成、读取并重载 ZIP，保留 SQL、schema、统计信息及列注释中的分号。
6. hint、binding 与慢日志：只有显式 `WRITE_SLOW_LOG` 或 binding 注入该 hint 的语句被强制记录，多语句执行按每条原始 SQL 匹配并在未命中语句后复位状态。
7. 元数据与会话管理：`SHOW [FULL] PROCESSLIST` 的权限过滤、文本截断、字段类型，以及 `SHOW COLLATION`、`mysql.user` 的结果和元数据与 Go 行为对齐。
8. SEM 与内核差异：动态权限控制受限 SQL；Classic 与 NextGen 对 `IMPORT INTO` 中显式 external ID 的处理分支保持独立。

## 主要符号

- `cloned_concrete_session_retains_shared_state_until_last_owner_drops()`：设置用户变量后克隆并丢弃原句柄，证明克隆共享会话内部状态且最后一个所有者仍可执行 SQL。
- `setting_tiflash_cop_updates_planner_session_vars()`：通过 `WithSessionVars` 观察 `SET tidb_allow_tiflash_cop` 对 `IsTiFlashCopBanned()` 的双向更新。
- `plan_replayer_dump_sql_writes_a_downloadable_zip()`：安装测试外部存储，dump、解码、load、再次 dump，验证归档路径、SQL 和带分号注释的 schema。
- `transaction_schema_check_ignores_temporary_tables_but_rejects_changed_normal_tables()`：用两个 `ConcreteSession` 并发模拟事务与 DDL，区分临时表、无关普通表和已写普通表的 schema 校验。
- `stale_read_only_transaction_does_not_require_noop_functions()`：区分受 `tidb_enable_noop_functions` 门控的普通 `READ ONLY` 与 TiDB 的 `AS OF` stale read。
- `session_stats_waiter_uses_real_sync_load_queue_for_completion_and_timeout()` 与 `concrete_session_initializes_statement_stats_sync_timeout()`：验证加载队列完成/超时以及默认 `DefTiDBStatsLoadSyncWait` 初始化。
- `concrete_runtime_executes_parse_prepare_commit_rollback_and_record_set()`：覆盖 `ConcreteTestRuntime`、`PrepareStmt`、`ExecutePreparedStmt`、事务提交/回滚和结果集关闭。
- `canonical_factory_shares_domain_and_store_but_isolates_session_state()`：同时保留 `from_tikv_store` 的生产构造器类型约束，并使用 `from_storage_for_test` 验证工厂共享边界。
- `production_record_set_and_connection_state_do_not_depend_on_test_traits()`：直接检查 SQL 拆分、结果列、EOF、关闭语义与 `ConcreteProtocolState`。
- `concrete_set_global_default_uses_the_registered_sysvar_default()` 与 `concrete_session_answers_connector_j_connection_variables()`：验证系统变量默认值和连接器启动探测所需的完整结果行。
- `forced_slow_log_is_emitted_by_the_concrete_session_only_for_the_hinted_statement()` 与 `multi_statement_binding_matches_each_statement_text_and_logs_only_its_match()`：验证慢日志强制标记、binding 注入及 `TiDBFoundInBinding` 生命周期。
- `StaticSessionManager`：仅为 processlist 测试提供固定 `ProcessInfo` 映射；`InfoSchemaCoordinator` 的会话登记操作为空实现，`Manager` 只真实实现查询进程，其余控制面返回空值或无操作。
- `show_processlist_uses_session_manager_rows_and_full_flag()`、`show_collation_and_mysql_user_catalog_match_go_metadata()`：验证权限、截断、字段元数据、用户创建/删除的可见性。
- `sem_restricted_sql_uses_authenticated_dynamic_privileges_and_preserves_import_path()`：局部定义 RAII 清理器 `SemCleanup`，验证 SEM、动态权限和内核路径处理。

所有测试符号均为模块私有；唯一的 trait 实现也只服务本文件断言。

## 执行流程

Rust 测试框架在 `astersql-session` 的测试构建中发现每个 `#[test]` 并独立调用。大多数用例遵循同一链路：父模块的 `runtime()`/`concrete_session()` 或 `crate::runtime::CreateAnalyzeSession()` 创建 mock store、Domain 与 `ConcreteSession`；测试通过 `execute`/`Execute` 进入解析、规划、执行和事务路径；随后通过 `RecordSet::Next`/`next_row`、会话变量、协议状态、日志或存储副作用断言结果。

关键分支流程如下：

- schema 校验用例先创建两个共享 Domain 的会话，再在一个事务存活期间由另一个会话执行 DDL。临时表变更和未被事务写入的普通表变更允许提交；事务已写表发生变更时，提交必须返回 `[domain:8028]Information schema is changed`，且数据不可见。
- 统计加载用例把未完全加载的列统计写入 Domain cache，把 `StatsLoadItem` 放入 statement context，调用 `StatsLoadWaiter::SyncWaitStatsLoad`；一条路径由真实 worker 发布完整统计，另一条使用默认零超时并断言错误含 `timeout`。
- plan replayer 用例把全局测试外部存储指向唯一临时目录，执行 dump 后按 token 读取 `replayer/<token>`，解码并核对归档；随后删除表、从本地 ZIP load、检查重建表并再次 dump。
- processlist 用例构造 root/alice 两条固定进程，弱引用注入 `ConcreteSession`；无 PROCESS 权限时只返回登录用户并截断 Info 到 100 字符，有权限且使用 `FULL` 时返回两行和完整 SQL。
- SEM 用例注册缺失的受限变量，切换到 SEM v2，分别认证普通用户和拥有 `RESTRICTED_SQL_ADMIN` 的用户。资源组修改依权限拒绝或通过；显式 external ID 在 NextGen 始终拒绝，在 Classic 保留原路径直到后续“对象存储未配置”边界。

## 数据与状态

- `ConcreteSession` 克隆复用 `Rc<ConcreteSessionInner>`；用户变量、binding 标记和结果状态属于同一具体会话。`CanonicalSessionFactory::create_session()` 则创建不同 inner：Domain、实例计划缓存和底层存储共享，当前数据库等 session-local 状态隔离。
- 事务用例依赖 Domain schema 版本、事务访问表集合及临时表类别。核心不变量是只有与事务实际访问相关的普通表 schema 变化才使提交失败。
- 统计用例直接构造 `TableStats`/`ColumnStats`，以 `loaded_or_evicted` 表示完整载入状态；`NeededItems` 位于 `StmtCtx.StatsLoad` 的互斥保护队列中。
- 计划重放状态跨全局测试外部存储、临时目录、ZIP 字节与重建后的 Domain schema；归档至少包含 `schema/test.plan_replayer_sql.schema.txt`、`stats/test.plan_replayer_sql.json` 和 `sql/sql0.sql`。
- 慢日志用例使用内存 `Logger`，以 entry 数量和消息内容为可观察状态；binding 用例还检查 statement 生命周期结束后 `TiDBFoundInBinding == Off`。
- `StaticSessionManager.processes` 是按连接 ID 索引的 `HashMap<u64, Arc<ProcessInfo>>`。测试会话仅保存 manager 的 `Weak` 引用，因此局部 `Arc<dyn Manager>` 必须活到查询完成。
- SEM 用例会修改全局 SEM 模式和系统变量注册表；`SemCleanup` 的 `Drop` 保证切换函数返回的清理闭包在正常返回或 panic 展开时执行。

## 依赖与调用关系

模块装配链为 `pkg/session/lib.rs`（`#[cfg(test)] mod runtime_test`）→ `pkg/session/runtime_test.rs`（路径模块 `session`）→ 本文件。`pkg/session/Cargo.toml` 定义 crate 名 `astersql-session`，生产依赖覆盖 Domain、KV、planner、statistics、sessionctx、bindinfo、SEM、extstore 等边界；本文件使用的 `rustls::ServerConfig` 来自 `[dev-dependencies] rustls = "0.23"`，`nextgen` feature 传播到 `astersql-config-deploymode/nextgen` 与 `astersql-config-kerneltype/nextgen`。

主要下游调用面是：

- `crate::runtime::{CreateAnalyzeSession, ConcreteSession, CanonicalSessionFactory}`：建立生产会话边界。
- `ConcreteSession::{execute, Execute, PrepareStmt, ExecutePreparedStmt, WithSessionVars, AddSessionBinding, ExecuteWithSlowLogLogger, SetSessionManager, AuthenticateUserForTest}`：驱动 SQL、变量、binding、日志、认证和 processlist 行为。
- `SessionStatsSyncLoadAdapter` 与 `astersql_planner_core_base::StatsLoadWaiter`：连接会话 statement context 和 Domain 统计加载队列。
- `astersql_planner_extstore` 与 `astersql_domain::plan_replayer_dump`：保存及解析 replay ZIP。
- `astersql_session_sessmgr::{InfoSchemaCoordinator, Manager, ProcessInfo}`：提供 `SHOW PROCESSLIST` 的管理器接口。
- `astersql_util_sem_compat`、`astersql_config_kerneltype` 及 runtime 的 import-path 测试入口：表达 SEM/内核组合的授权和路径规则。

RustCodeGraph 能定位本文件及上述生产类型，但对这些 `#[test]` 函数返回的 callers/callees 边为空；这表示当前索引没有解析出测试体中的动态/方法调用边，而不是这些函数没有下游依赖。调用关系因此以索引源码、模块装配和实际方法调用共同核验。

## 错误处理与边界

测试大量使用 `expect`/`assert`，任何意外 `Err`、缺行、字段不符或资源操作失败都会立即使测试失败。专门验证的错误边界包括：普通 `START TRANSACTION READ ONLY` 的 noop 功能门控、相关普通表 schema 变化导致的 8028 错误、零超时统计加载、关闭后的结果集拒绝继续读取、无动态权限时 SEM 拒绝受限 SQL，以及 NextGen 拒绝显式 external ID。

测试不会把所有失败都收敛成某个错误类型，而是按兼容契约检查稳定的错误前缀或子串。扩展这些断言时应优先沿用生产错误码或 Go 测试中的稳定消息，避免绑定完整调试文本。

边界限制也需明确：`StaticSessionManager` 不是完整 manager，实现中的 kill、TLS 更新和内部会话管理均为空；plan replayer 使用文件型测试存储而非远程对象存储；Classic import 用例止于规划/配置错误，不实际导入；本文件本身不覆盖网络协议字节流或真实 TiKV。

## 并发与资源生命周期

并发主要通过共享对象和后台 worker 表达，而非在测试体中显式启动线程。两个会话共享 `Arc<Domain>` 来交错执行事务与 DDL；统计适配器持有 Domain 级加载器，生产侧 `DomainStatsLoadWorkers::drop` 会置退出标志并 join worker，本文件通过完成/超时结果观察该生命周期。

资源释放契约包括：克隆会话在最后一个 `Rc` 所有者释放前保持可用；结果集 `Close` 后 `Next` 必须失败；工厂创建的一个会话销毁不影响另一个会话；processlist manager 的强 `Arc` 覆盖所有弱引用查询；SEM 由 `Drop` 清理器恢复。plan replayer 用例在成功路径显式清空全局外部存储、关闭 storage 并删除临时目录；如果在清理前 panic，这三步不是 RAII 保护，可能留下全局测试状态或临时目录，这是维护该用例时最需要注意的隔离风险。

## 与 Go 版本的对应关系

本文件是多个 Go 回归面的集中 Rust 对照，并非逐行复刻单一 `session_test.go`：

- `pkg/session/test/temporarytabletest/temporary_table_test.go::TestSchemaCheckerTempTable` 是临时表 schema 变更不阻塞提交的直接依据；Rust 用例同时补充了无关普通表与相关普通表对照。
- `pkg/statistics/handle/syncload/stats_syncload_test.go` 中的同步加载测试提供完成、超时和结果通道竞争语义；Rust 用例从会话适配器接入真实 Domain queue。
- `pkg/server/handler/optimizor/plan_replayer_test.go::TestPlanReplayerLoadWithSemicolonInColumnComment` 对应 replay ZIP 往返和列注释分号保真。
- `pkg/planner/core/hint_test.go` 的 `TestWriteSlowLogHint` 对应普通 SQL 不强制记录、带 hint SQL 强制写慢日志；Rust 还检查多语句 binding 的逐语句匹配与标志复位。
- `pkg/executor/test/showtest/show_test.go::TestCollation` 核对 `SHOW COLLATION` 字段类型；Go 的 session/server processlist 测试提供用户过滤、FULL 信息和元数据行为依据。
- `pkg/util/sem/compat/sem_integration_test.go::TestRestrictedSQL` 直接对应普通用户/`RESTRICTED_SQL_ADMIN`、`ALTER RESOURCE GROUP` 以及 Classic/NextGen external ID 分支。
- Go 中 prepared statement、事务、系统变量、Connector/J 初始化、`mysql.user` 和工厂/Domain 行为分散在 session、executor 与 server 测试中；本文件以具体 Rust runtime 的端到端断言统一覆盖，不应据此声称所有 Go 测试分支均已迁移。

重要差异是 Rust 测试直接调用 `ConcreteSession` 及测试入口，Go 多数通过 `testkit` 或 server/DB 接口；两者要求保持用户可观察结果一致，但生命周期与测试装配不必同形。

## 扩展指南

新增会话行为时，优先按可观察边界扩展最接近的既有测试，而不是在本文件增加测试内生产实现：

- 会话共享/隔离修改应同步检查 `ConcreteSession::clone`、`ConcreteSessionInner` 字段和 `CanonicalSessionFactory::create_session`，并扩展前两个所有权测试。
- SQL/事务/结果集修改应扩展 `concrete_runtime_executes_*` 或 `production_record_set_*`，同时在独立 Rust 文件（本目录约定为 `runtime_test/*.rs` 或邻近 `*_test.rs`）保留测试逻辑，不把测试嵌入生产 `runtime/session.rs`。
- schema 校验、stale read、系统变量、processlist、用户目录或 SEM 行为必须先找到对应 Go 测试，维持分支、错误码/稳定消息、字段类型与权限过滤，不能为让 Rust 测试通过而删减 Go 语义。
- 统计加载扩展必须覆盖成功、超时/错误和 worker 清理；涉及全局配置或后台线程时使用可恢复的 guard，避免跨测试污染。
- plan replayer 或 SEM 测试新增全局状态时，应把 extstore 清理也改为 RAII 或测试 cleanup guard；路径和归档断言只检查协议必要内容，避免对 ZIP 内部顺序做脆弱假设。
- 新增 processlist 字段时需同步 `StaticSessionManager`、结果列元数据和有/无 PROCESS 权限两条路径；trait 增加必需方法时，此测试替身也必须同步。

兼容性风险集中在 SQL 错误文本、MySQL 元数据和 Connector/J 探测变量；性能风险集中在统计 worker、plan replay I/O 以及测试内重复 bootstrap。纯断言扩展不应改变生产性能，但过宽的端到端场景会拉长 crate 测试时间。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/session/runtime_test` 定位七个测试分片；`node --file pkg/session/runtime_test/session.rs --offset 1/501` 阅读本文件 1–1048 行；`query` 确认 `StaticSessionManager`、`ConcreteSession`、`CanonicalSessionFactory`、`SessionStatsSyncLoadAdapter` 以及代表性测试函数的位置。对代表性测试执行 `callers`/`callees` 返回空数组，故没有把缺失的静态边写成事实。
- Rust 源与装配：`pkg/session/lib.rs` 的 `#[cfg(test)]` 引入、`pkg/session/runtime_test.rs` 的路径模块与父级 helper、`pkg/session/runtime/session.rs` 的 `ConcreteSession`/`ConcreteSessionInner`/`CanonicalSessionFactory`、`pkg/session/runtime/planning.rs` 的统计适配器和 worker 生命周期。
- crate 边界：`pkg/session/Cargo.toml` 的 `[lib] path = "lib.rs"`、`nextgen` feature、会话/Domain/planner/statistics/SEM/extstore 依赖及 `rustls` dev-dependency。
- Go 对照与相关测试：`pkg/session/test/temporarytabletest/temporary_table_test.go`、`pkg/statistics/handle/syncload/stats_syncload_test.go`、`pkg/server/handler/optimizor/plan_replayer_test.go`、`pkg/planner/core/hint_test.go`、`pkg/executor/test/showtest/show_test.go`、`pkg/util/sem/compat/sem_integration_test.go`。
- 人工复核：文档明确回答了该文件为何存在、测试如何进入生产会话链、共享/隔离和资源清理不变量、Go 对照位置，以及安全扩展时应修改的测试面。任务为纯文档分析，按计划未运行 Cargo。
