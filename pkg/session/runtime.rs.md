# `pkg/session/runtime.rs` 逻辑说明

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准。

## 文件定位

提供语句级公共策略：只读模式检查、parser 与 SQL mode 适配、会话 KV 表约束、简单 WHERE/分区表达式求值、split bound、日期时间、NULL 和错误文本转换。

## 核心职责

1. 组装具体运行时：声明四十余个 `runtime/*` 子模块，并重导出 `ConcreteSession`、`RuntimeDomain`、`CanonicalSessionFactory`、`ConcreteRecordSet`、`PlannedKVResult`、typed adapter、统计故障注入 guard 等公共入口。
2. 维护按 Domain 隔离的共享目录：实例计划缓存、prepared statement 计数与上限、计划缓存 generation、global bindings、数据库/placement/resource group/权限/拓扑/region/index-usage 等运行时状态。稳定身份由 `runtime_domain_id` 生成，避免裸 `Arc` 地址复用唤醒旧状态。
3. 提供语句级公共策略：只读模式检查、parser 与 SQL mode 适配、会话 KV 表约束、简单 WHERE/分区表达式求值、split bound、日期时间、NULL 和错误文本转换。
4. 管理内存与资源边界：把 SQLKiller、日志、panic-on-exceed、rate limit 接入 `ActionOnExceed`，并用 Drop guard 释放编译配额、statement tracker 和 arbitrator root pool。
5. 管理进程内 DDL 作业可观测状态：分配单调 job ID/TSO、按 Domain/schema/table 冲突关系排队、记录 snapshot/checkpoint/history，并确保正常、错误和 panic 路径都结束 active job。
6. 为测试或外层工厂提供真实观察/注入面：`RegisterRuntimeTopology`、`SetShowClusterConfigForTest`、`RuntimeDdlHistoryJobForTest`、`RuntimeReplicaReadRequest`、`RuntimeSelectRequest`、`RuntimeStaleReadState`，避免用固定 SQL 响应冒充执行行为。

## 主要符号

- `pub(crate) use session::ValidateDiagnosticBootstrap`：本次冻结范围在该文件的唯一增量，把 `runtime/session.rs` 的校验函数向 crate 内重导出，供 `diagnostic.rs` 调用；没有新增对外公开 API。校验正数 KV bootstrap version、SQL bootstrapped 状态及 SQL/KV 版本相等，不要求版本达到当前 320，也不写入版本。
- 模块和重导出：`pub use session::{ConcreteSession, RuntimeDomain, ...}` 是主要公共门面；`planning::PlannedKVResult`、`typed_adapter_bridge::{OwnedKVSnapshotSource, SessionBoundAdapterOwner, TypedScanSpec}`、`statistics::*Guard` 也是外部可见契约。`dispatch`、`dml`、`query`、`transaction` 等私有导入供整个 runtime 模块树共享。
- `runtime_instance_plan_cache` / `RUNTIME_INSTANCE_PLAN_CACHES`：以 `runtime_domain_id` 为键保存 `Weak<InstancePlanCache>`；`runtime/session.rs::ConcreteSession::new` 为同一 Domain 的会话取得共享实例缓存，最后一个强引用释放后可自然重建。
- `runtime_read_only_mode_enabled`、`runtime_read_only_mode_error`：在最终写边界读取进程级 restricted/super read-only 变量，并保持 Go/TiDB 的 planner 错误类别。
- `RUNTIME_PREPARED_STMT_COUNTS`、`RUNTIME_MAX_PREPARED_STMT_COUNTS`：按 Domain 指针维护 prepared statement 配额；`runtime_prepared_stmt_reserve` 先增后校验，超限时回退计数，`runtime_prepared_stmt_release` 饱和减法防止下溢。
- `RUNTIME_PLAN_CACHE_GENERATIONS` 与 global binding 辅助函数：generation 递增使同一 Domain 的命名 prepared plan 失效；global binding 以 SQL digest 去重并按 Domain 共享，会话 binding 则仍留在会话自身 catalog。
- `RuntimeReplicaReadRequest`、`RuntimeSelectRequest`、`RuntimeSelectRequestDispatch`、`RuntimeStaleReadState`：记录最后一次 KV 请求、并行分支派发和 stale-read 时间戳/InfoSchema 版本，用作只读可观测快照；`RuntimeSelectRequest` 用独立 `Arc<kv::Request>` 表达各扫描分支不能共享可变请求。
- `runtime_domain_id`：维护 `Arc` 地址到 `(Weak<Domain>, u64)` 的映射，先清理已释放 Domain，再验证 `Arc::ptr_eq`；新实例从 `NEXT_RUNTIME_DOMAIN_ID` 取进程唯一 ID。
- `prepare_import_path_for_kernel`：仅在 nextgen 且 SEM 开启时约束 S3/OSS URI；校验或补写当前 keyspace `external-id`，并要求 role ARN 或成对 access/secret key。测试包装为 `ValidateImportPathForKernelForTest` 和 `PrepareImportPathForKernelForTest`。
- `build_mem_arbitrator_digest_id`、`approx_compile_plan_token_count`：前者按规范 SQL 与小写 DB 生成稳定内存画像 ID，空 SQL 返回 invalid；后者估算编译 token，并让无 FROM 的 SELECT 返回零。
- `RuntimeCompileMemoryQuota`、`RuntimeStatementMemoryTracker`、`RuntimeMemoryArbitrationGuard`：三个 RAII guard；Drop 分别返还 await-free 配额、解绑 tracker 并移除 root pool、或直接移除 root pool。
- `RegisterRuntimeTopology`：过滤空地址后按 Domain 指针登记真实 store ID/地址；调用者 `pkg/testkit/mockstore.rs` 在 bootstrap 后注入 mockstore 拓扑，查询和 cluster virtual table 复用它。
- `RuntimeDdlJob`、`RuntimeDdlJobs`、`RuntimeDdlJobGuard`：active/history/global-task-history 的内部状态；`begin_runtime_ddl_job` 按冲突范围等待早先 job，`finish_runtime_ddl_job` 归档为 synced/cancelled/rollback done，guard 的 Drop 为 owner 异常退出兜底。
- `parse` / `parse_with_sql_mode`：配置 parser SQL mode、显式拒绝 DECLARE CURSOR，并把 parser 失败统一包装为 `[parser:1064]`；后者被 `runtime/session.rs`、typed executor、scan adapter、dispatch 使用。
- `split_integer` / `split_datums`、`partition_expression_value`、`row_matches_simple_where`：分别处理常量算术 split bound、有限分区表达式和有限 WHERE 子集。它们是运行时兼容辅助，不是完整 expression 引擎。
- `parse_datetime_micros` / `parse_stale_datetime_micros`：严格检查日历日期和 0/6 位微秒；stale read 另把无时区 SQL wall clock 按系统时区转换。`ParseDateTimeMicrosForTest` 暴露独立测试入口。
- `SESSION_KV_TABLE`、`SESSION_KV_PREFIX`、`CONCRETE_NULL_VALUE`、`ensure_session_table`、`select_key`：限定内置 `aster_session_kv` 快路径及其 key/projection/WHERE 形状；内部 NULL 哨兵由协议适配层还原为 SQL NULL。

## 执行流程

会话主链从外层工厂或 testkit 取得 `Domain` 开始。`runtime/session.rs::ConcreteSession::new` 安装 planner 谓词简化入口、建立默认 `SessionVars` 和 session memory tracker，调用本文件的 `runtime_domain_id` 取得实例身份，再调用 `runtime_instance_plan_cache` 共享实例缓存。随后会话加载持久全局变量，并由 `dispatch.rs`/`transaction.rs` 等子模块处理语句生命周期。`pkg/testkit/mockstore.rs` 在 bootstrap 后调用 `RegisterRuntimeTopology`，因此 replica request 和 cluster table 使用真实工厂拓扑，而非 SQL 层猜测地址。

SQL 文本先经 `parse_with_sql_mode` 转 AST；普通入口 `parse` 使用默认 SQL mode，`ConcreteSession` 则从会话状态取得 mode。`runtime/session.rs` 用它统计/校验 starter SQL，`runtime/typed_dml_executor.rs` 和 `typed_analyze_executor.rs` 构建 typed 执行器，`runtime/typed_adapter_bridge.rs`、`scan_adapter_runtime.rs` 和 `dispatch.rs` 也复用同一解析规则。分派后进入 query/DML/DDL/control 子模块；本文件提供的表名、字面量、简单谓词、日期和错误辅助在这些路径中被调用。

prepared statement 与计划缓存链按 Domain 隔离：prepare 时 `runtime_prepared_stmt_reserve` 预占名额；drop/关闭时 release；会话记录创建时的 plan-cache generation，实例级 invalidation 递增 generation；同一 Domain 的 `InstancePlanCache` 由 Weak map 复用。global binding 同样按稳定 Domain ID 共享，而 session binding 不跨连接。

DDL 链由 `runtime/ddl.rs`、`recovery.rs` 或 `dxf_session.rs` 调用 `begin_runtime_ddl_job`。函数先把 job 放入 active，再触发提交/投递 failpoint，然后在 Condvar 上等待同 Domain、同 schema 且表冲突的更早 job；独立表可并行。执行中更新 detail/checkpoint/snapshot/table info，完成后 guard 调用 `finish_runtime_ddl_job` 转 history 并 `notify_all`；panic/提前返回时 Drop 以错误状态结束 job，避免队列永久占用。

内存链由会话/语句 tracker 选择 action：bootstrap 可只记录一次并把后续动作交给 fallback，cancel action 向 `SQLKiller` 发送 memory-exceeded 信号，panic adapter 保留底层 `PanicOnExceed` 行为，rate-limit action 在不能继续限速时记录 consumed/quota 并完成。编译配额和 root pool 全部通过 guard 的 Drop 在正常返回、错误或 unwind 中释放。

## 数据与状态

状态分为会话内、Domain 级和进程级。`ConcreteSession` 自身位于 `runtime/session.rs`，以 `Rc<ConcreteSessionInner>`、`RefCell` 和原子字段保存单连接状态，因此主体不宣称可跨线程共享；本文件的 `Runtime*Request/State` 多为 Clone 快照，供测试和适配器读取。

Domain 级共享状态大多使用 `LazyLock<Mutex<HashMap<...>>>`：计划缓存使用稳定 u64 Domain ID 加 `Weak` 生命周期；prepared counter 当前以 `Arc` 指针为键；数据库选项等部分表以指针键，global bindings、权限、cluster config、resource group、region/index usage 则使用稳定 ID。新增表时必须明确选择键语义，不能把可复用的裸地址当永久身份。锁中毒通常以 `PoisonError::into_inner` 恢复；拓扑和 DDL 作业这类关键协调状态使用 `expect`，中毒被视为不可继续的不变量破坏。

DDL 状态由单个 `Mutex<RuntimeDdlJobs>` 和 `Condvar` 保护。`runtime_ddl_tso` 以当前毫秒左移 18 位，并通过 CAS 保证即使时钟不前进也严格递增。job 保存 `Weak<Domain>`、schema/table/kind、状态/detail、并发/批量参数、进度、起止时间以及回滚需要的旧表快照；history 是进程内可观测记录，不替代 Domain 存储中的 canonical recovery state。

常量表达性能和协议边界：OFFSET 批量为 65,536、index backfill batch 为 4,096、cop scan concurrency 为 15；本地临时表 ID 从 `i64::MAX / 2` 原子递增，避免共享存储中的物理 key 冲突；`CONCRETE_NULL_VALUE` 是内部协议哨兵；`SESSION_KV_PREFIX` 隔离内置表键空间。这些值被子模块通过父模块私有可见性复用，修改会影响扫描 RPC 数、事务大小或兼容输出。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 定义/引用查询与限定源码搜索共同确认：

- `pkg/session/lib.rs -> pub mod runtime` 建立 crate 公共入口；`pkg/server/runtime.rs` 和多组 executor/session 测试使用其公开类型。
- `pkg/testkit/mockstore.rs -> RegisterRuntimeTopology` 在 `BootstrapSession` 后登记 store 拓扑。
- `runtime/session.rs::ConcreteSession::new -> runtime_domain_id -> runtime_instance_plan_cache` 建立会话的 Domain 身份和共享计划缓存；同文件的 starter/普通执行路径调用 `parse_with_sql_mode`。
- `runtime/ddl.rs`、`runtime/recovery.rs`、`runtime/dxf_session.rs -> begin_runtime_ddl_job/RuntimeDdlJobGuard`；`admin.rs`、`control.rs`、`dispatch.rs`、`query.rs`、`system_query.rs` 等广泛调用 `runtime_domain_id` 访问共享目录。
- `runtime/dispatch.rs -> prepare_import_path_for_kernel` 在 IMPORT INTO 分派时应用 nextgen/SEM URI 约束。
- `runtime/typed_dml_executor.rs`、`typed_analyze_executor.rs`、`typed_adapter_bridge.rs`、`scan_adapter_runtime.rs -> parse_with_sql_mode` 保持各执行入口的 parser 规则一致。

主要下游 crate 是 `astersql-domain`（Domain/InfoSchema/SQLKiller）、`astersql-kv`（Request/Key/Storage）、parser/AST/MySQL mode、planner core/cache/rule、executor、statistics、sessionctx variables、memory arbitrator、trace event、meta model、DDL 和 bindinfo。外部依赖 `chrono`/`chrono-tz` 用于时间转换。`Cargo.toml` 的 `nextgen` feature 传递给 deploymode/kerneltype；本文件在运行时以 `is_nextgen` 与 SEM 开关控制导入 URI行为，而不是通过本文件级 `cfg(feature)` 删除代码。

## 错误处理与边界

公开和内部可恢复失败统一为 `SessionError`/`SessionResult`。`session_error` 增加操作上下文，`session_kv_error` 保留 shared source；parser 错误固定为 MySQL 1064 风格，read-only 使用 planner 的 `ErrSQLInReadOnlyMode`，duplicate primary key 使用 `[kv:1062]` 风格。调用者再负责语句回滚、warning/statement context 和协议编码。

需要保留的显式边界包括：DECLARE CURSOR 当前直接拒绝；`aster_session_kv` 只接受单物理表、一个 `v`/通配投影和 `WHERE k = literal`；`split_integer` 只求值整数常量的一元/二元算术并检查溢出/除零；`row_matches_simple_where` 只覆盖括号、NULL、IN、BETWEEN、LIKE、AND/OR 和简单比较，无法识别的形态并不等同完整 SQL 语义；分区表达式只覆盖列、已有 DML expression evaluator 和少量日期/ABS 形状。

IMPORT URI 在普通模式或非 S3/OSS 时原样返回；仅 nextgen+SEM 强制凭据和 external ID。百分号解码拒绝截断或非法 hex，显式 external ID 必须等于当前 keyspace。日期 parser 只接受 `YYYY-MM-DD HH:MM:SS` 或六位微秒；测试证明不存在的闰日/月末被拒绝。`increment_ttl_insert_rows_metric` 含受控 `unsafe`：为读取遗留 `static mut Option` 使用 raw pointer，并故意 forget 临时 handle，因为所有权留在进程静态变量；在未初始化指标的轻量 runtime 中不做操作。

## 并发与资源生命周期

所有跨会话目录都由 `Mutex` 或原子量保护。`runtime_domain_id` 在锁内清除 strong count 为零的 Weak 条目并用 `Arc::ptr_eq` 防地址复用；instance plan cache 自身只存 Weak，避免全局表延长 Domain/cache 生命周期。prepared count 的 reserve/release 必须成对，release 使用 `saturating_sub`；修改 prepare/close 路径时需验证错误和连接关闭不会泄漏配额。

DDL 排队以 `Mutex + Condvar` 实现：提交顺序由 job ID 决定，同 schema 的 schema-level job 与所有表冲突，同表作业串行，不同表允许并行。等待期间 Condvar 原子释放锁；归档后先 drop guard 再 `notify_all`。`RuntimeDdlJobGuard` 的 Drop 覆盖 unwind，但 canonical 恢复数据仍在 Domain storage，进程内 active/history 不能替代持久恢复。

内存 guard 的 Drop 是关键不变量：编译配额只能返还一次（`reserved` 清零）；statement tracker 先 detach arbitrator 再按 SessionID 移除 root；通用 guard 无条件移除 UID。`RuntimeSelectRequest` 用每分支独立 `Arc<Request>` 和 address 记录检查并发 worker 不共享可变请求。相比之下，`ConcreteSession` 使用 `Rc/RefCell`，属于连接所有者线程的对象；若新增后台任务，应通过已有 owner/adapter 或拥有型快照跨边界，不能直接共享其内部可变状态。

## 与 Go 版本的对应关系

Rust 把 Go `pkg/session/session.go`、事务、planner/executor glue、testkit 与 DDL owner 的部分行为聚合为 `runtime.rs` 模块树，因此只能按行为链对照，不能声称逐函数一一翻译。

本文件只提供 parser、共享状态和辅助边界，不代表 Go 完整 session API。

Go `CommitTxn`/`RollbackTxn` 和 `Close` 的事务/资源清理在 Rust 主要由 `runtime/transaction.rs`、`session.rs` 及 Drop guard 承担，而非本文件单独实现。

DDL job 的提交顺序、history 和 failpoint 名称对齐 Go DDL owner/testutil 的可观察行为，但 Rust 这里的 `RuntimeDdlJobs` 是进程内协调/测试视图；持久 job 与恢复仍归 Domain 存储。

## 扩展指南

新增 SQL 行为应先选择正确子模块：分派进入 `dispatch.rs`，事务进入 `transaction.rs`，关系 DML 进入 `dml.rs`，读取进入 `query.rs`/`relational_scan.rs`，DDL 进入 `ddl.rs`，会话字段与生命周期进入 `session.rs`。只有多个子模块共同需要的常量、Domain 级目录、guard 或窄辅助函数才应加入 `runtime.rs`，避免继续扩大模块根的职责。

新增 Domain 级全局表时，必须回答三件事：键是稳定 `runtime_domain_id` 还是仅活跃期 Arc 指针；Domain 释放后如何清理；锁中毒、并发更新和跨测试隔离如何处理。若值拥有 Domain/cache，应优先用 Weak，防止全局目录制造生命周期泄漏。修改 prepared counter、plan-cache generation 或 binding 作用域时，应同步 `pkg/session/runtime_test/session.rs`、规划测试及 Go prepared/plan-cache 回归。

扩展 parser/简单表达式辅助时，不要把有限 evaluator 悄悄当完整 SQL 语义。新增 AST 形状必须覆盖 NULL、类型转换、collation、溢出、非法日期、SQL mode 和错误类别；需要完整 planner/expression 的行为应下沉到相应 crate/子模块。IMPORT URI 变更须覆盖编码键名、重复参数、非法 percent、凭据组合、SEM 和 nextgen 双开关。

DDL 协调变更应保持“同 schema 冲突串行、独立表可并行、任何退出都会归档并唤醒等待者”的不变量，并在独立 `pkg/session/runtime_test/ddl.rs` 或 `runtime/ddl_test.rs` 增加测试；不要把测试嵌进生产文件。内存动作变更应验证 fallback 只被正确调用、kill signal、日志一次性和所有 Drop 路径；并发 request 变更应继续验证每个 scan branch 拥有独立请求对象。

性能风险主要来自全局 Mutex 热点、每次语句 parser/字符串转换、Domain map 清理、超大 OFFSET/DDL batch 参数和 DDL Condvar 冲突范围。兼容风险主要来自错误文本/错误码、SQL mode、prepared 配额作用域、NULL 哨兵、系统时区和 Go 的完整 expression 语义。改动前应在相应独立 Rust 测试中增加回归，并用 Go 同行为测试确认意图。

## 连接方言身份

普通连接执行通过 `parse_with_dialect` 创建 `Parser::with_dialect`，SQL mode 仍独立设置。无连接上下文的既有 `parse`/`parse_with_sql_mode` 保持 MySQL 约定；预处理身份接线由后续批次完成。测试构建用线程局部观测实际 parser 输入，不添加产品 SQL 调试函数。


## 2026-10-24 预处理语句身份承接

新增内部 parse_in_dialect，在保留原默认 SQLMode 的前提下让预处理模板、绑定后重解析和参数相关规划检查显式使用连接方言；原 parse 仍默认 MySQL。
