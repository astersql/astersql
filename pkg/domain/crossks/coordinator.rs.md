# `pkg/domain/crossks/coordinator.rs`

## 文件定位

本文件属于 `astersql-domain-crossks` crate（见 `pkg/domain/crossks/Cargo.toml`），实现跨 Keyspace 与普通 Domain 共用的“内部 SQL Session—InfoSchema MDL 屏障”适配层。模块由 `pkg/domain/crossks/lib.rs` 公开导出；生产侧在 `pkg/session/runtime/session_factory.rs` 创建 `SchemaCoordinator`，把系统会话池借出的真实 Session 包装成 `RegisteredMDLSession`，再由 `pkg/infoschema/issyncer/syncer.rs::Syncer::check_mdl_with_context` 通过 `InfoSchemaCoordinator` 调用它。

它不负责执行 DDL、加载 InfoSchema 或持有 Session worker，也不主动释放事务锁。它只维护当前借出的内部 Session 视图，并从“本轮可推进的 DDL Job”集合中剔除仍受旧事务 MDL 约束的 Job。

## 核心职责

1. `SchemaCoordinator` 以 Session ID 为键登记、查询和注销 `Arc<dyn InternalSession>`，使 InfoSchema 后台检查能够观察当前在用的内部 Session。
2. `check_old_running_transaction` 在一个一致的登记表快照上依次调用 Session 的 `remove_lock_ddl_jobs`，原地收缩 Job 集合；留下的 Job 才能继续发布 schema 版本。
3. `RegisteredMDLSession` 把协调器本地的 `JobMdl` 转换为 `astersql_session_sessmgr::mdldef::JobMDL`，复用真实事务的 `TransactionMDL::check_jobs`，再把过滤结果映射回调用方集合。
4. `InfoSchemaCoordinator for SchemaCoordinator` 适配 `astersql-infoschema-issyncer` 的 `JobMDL` 类型和 Go 风格方法名，使本 crate 能直接接入 InfoSchema Syncer。
5. `kill_non_flashback_cluster_connections` 在 crossKS 场景保持空操作；普通 Domain 会由 `pkg/session/runtime/normal_ddl_service.rs::NormalSchemaCoordinator` 另外转发给外部连接管理器。

## 主要符号

- `JobMdl { version, table_ids }`：协调器内部的 Job 描述。`version` 是 Session 对相关表至少应达到的 schema 版本，`table_ids` 是该 Job 涉及的表集合。
- `InternalSession: Send + Sync`：协调器的最小 Session 抽象。`id()` 提供登记键；`remove_lock_ddl_jobs(&mut jobs, print_log)` 允许实现按自身事务状态删除被阻塞 Job。trait 不承诺释放锁。
- `SchemaCoordinator`：核心状态容器。`sessions` 是按 ID 索引的动态 trait object；`print_mdl_log_time` 控制十秒一次的日志许可。
- `new_schema_coordinator()`：创建空登记表，并把日志计时起点设为当前时刻；返回值不是 `Arc`，由调用者决定共享所有权。
- `store_internal_session`：写锁下按 `session.id()` 插入；同 ID 再次登记会替换旧值。
- `delete_internal_session`、`contains_internal_session`、`internal_session_count`：分别按 ID 删除、查询及统计登记项；删除不存在的 ID 是静默无操作。
- `check_old_running_transaction`：持有 Session 读锁完成整轮回调，计算是否允许打印日志，并让每个 Session 连续过滤同一份 Job map。
- `kill_non_flashback_cluster_connections`：crossKS 的显式空实现。
- `RegisteredMDLSession { id, mdl }`：真实 SQL Session 的共享 MDL 状态借用视图；只持有 Session ID 和 `Arc<TransactionMDL>`，不拥有 worker。
- `impl InfoSchemaCoordinator for SchemaCoordinator`：在 `astersql_infoschema_issyncer::JobMDL` 与本地 `JobMdl` 之间做一次复制转换，并保留仍存在于本地结果中的 Job ID。

## 执行流程

1. `pkg/session/runtime/session_factory.rs` 为一个普通或 Keyspace runtime 创建 `Arc<SchemaCoordinator>`，并把它捕获进 `SystemSessionCallbacks`。
2. 系统 Session 从池中借出时，`borrowed` 回调取得该 Session 的共享 `TransactionMDL`，构造 `RegisteredMDLSession` 后调用 `store_internal_session`；归还或销毁时，回调以 Session ID 调用 `delete_internal_session`。
3. `pkg/infoschema/issyncer/syncer.rs::Syncer::check_mdl_with_context` 读取待检查 Job；若能取得协调器，则调用 trait 方法 `CheckOldRunningTxn`。
4. trait 适配实现把每个外部 Job 的 `Ver`、`TableIDs` 复制为本地 `JobMdl`，然后进入 `check_old_running_transaction`。
5. `check_old_running_transaction` 先取得 `sessions` 读锁，再在 `print_mdl_log_time` 互斥锁下判断距上次许可是否严格超过十秒；达到间隔时更新时间并把 `print_log=true` 传给本轮所有 Session。
6. 每个登记 Session 依次过滤同一个 map。对 `RegisteredMDLSession`，实现把 Job 再包装为 `Arc<session_sessmgr::JobMDL>`，调用 `TransactionMDL::check_jobs`，最后按过滤后的键集合收缩本地 map。
7. trait 适配层再按本地 map 的键收缩 InfoSchema Syncer 原始 map。`check_mdl_with_context` 只对留下的 Job 调用 schema version syncer，因此被旧事务命中的 Job 本轮不会推进。

## 数据与状态

- `sessions: RwLock<HashMap<u64, Arc<dyn InternalSession>>>` 是进程内登记表。键的唯一性由 `InternalSession::id` 提供；协调器不检查 ID 冲突，冲突语义是替换。
- `print_mdl_log_time: Mutex<Instant>` 只记录最近一次“允许打印”的时间，不记录每个 Session 的日志时间。新建后十秒内首次检查不会获得打印许可，判断条件为 `elapsed() > 10s` 而不是大于等于。
- Job map 是调用方拥有的可变工作集。过滤是单调删除：协调器及 `TransactionMDL::check_jobs` 都不会新增 Job，也不会修改保留 Job 的版本或表集合。
- `TransactionMDL` 的表版本状态位于 `pkg/session/sessmgr/mdl.rs`：若 Session 是 restricted SQL，则 `check_jobs` 直接返回、不阻塞任何 Job；否则，只要某个相关表已登记且持有版本低于 Job 版本，就删除该 Job。版本为零覆盖“已经开始访问表、尚未装载最新元数据”的窗口。
- `RegisteredMDLSession` 持有 `Arc<TransactionMDL>`，所以检查不需要借用线程局部 SQL Session；登记项删除后，只要别处仍持有 `Arc`，MDL 状态本身仍可存活。

## 依赖与调用关系

- crate 边界：`pkg/domain/crossks/Cargo.toml` 声明直接依赖 `astersql-infoschema-issyncer` 与 `astersql-session-sessmgr`；本文件实际使用前者的 `InfoSchemaCoordinator`/`JobMDL`，以及后者的 `TransactionMDL`/`mdldef::JobMDL`。
- 上游创建与生命周期：`pkg/session/runtime/session_factory.rs::{KeyspaceSessionFactory::prepare, build_normal_schema_runtime}` 创建协调器并通过系统 Session 池回调登记/注销；`pkg/session/runtime/crossks_runtime.rs` 也把新协调器交给 crossKS manager。
- 上游检查：`pkg/infoschema/issyncer/syncer.rs::Syncer::check_mdl_with_context` 调用 `CheckOldRunningTxn` 后，仅发布未被删除的 Job；`postReload` 在 flashback cluster 动作上调用 `KillNonFlashbackClusterConn`。
- 普通 Domain 组合：`pkg/session/runtime/normal_ddl_service.rs::NormalSchemaCoordinator::CheckOldRunningTxn` 先调用本文件的内部 Session 协调器，再检查普通用户连接的 Session manager，两侧都可继续收缩 Job 集合。
- 下游核心：`RegisteredMDLSession::remove_lock_ddl_jobs` 调用 `pkg/session/sessmgr/mdl.rs::TransactionMDL::check_jobs`。RustCodeGraph 也解析到 `check_old_running_transaction -> InternalSession::remove_lock_ddl_jobs` 调用边。
- 模块公开面：`pkg/domain/crossks/lib.rs` 通过 `pub mod coordinator` 与 `pub use coordinator::*` 暴露本文件 API，并把 `coordinator_test.rs` 作为独立测试模块挂载。

## 错误处理与边界

- 所有锁获取都使用 `expect(...)`；任一 `Mutex`/`RwLock` 被 panic 污染后，后续访问会再次 panic，而不是返回可恢复错误。
- 登记、注销、查询和检查均不返回 `Result`。`InternalSession::remove_lock_ddl_jobs` 也没有错误通道；实现若失败只能选择不删除 Job 或 panic。
- trait 适配只按 Job ID 把过滤结果映射回原 map，因此 Session 实现对本地 `JobMdl` 字段的修改不会传播回 InfoSchema 的原对象；当前约定只允许删除，这与调用链相符。
- `RegisteredMDLSession` 在两次类型转换中克隆 `table_ids`，检查成本随 Job 数及相关表数增长；本文件没有批量上限或提前终止逻辑。
- `print_log` 对通用 `InternalSession` 可见，但 `RegisteredMDLSession` 当前将其命名为 `_print_log` 并忽略；因此十秒限流状态在真实 Rust MDL 适配路径上不产生日志，只保留与 Go 接口相容的能力。
- 空 Session 表或空 Job map 都是合法输入；前者不做回调，后者仍会完成锁与日志间隔检查。`kill_non_flashback_cluster_connections` 对 crossKS 永远不产生副作用。

## 并发与资源生命周期

- 增删 Session 取得 `sessions` 写锁，查询和计数取得读锁。`check_old_running_transaction` 在所有 Session 回调结束前一直持有读锁，保证检查期间登记集合不会变化。
- `pkg/domain/crossks/coordinator_test.rs::deleting_a_session_waits_for_an_in_progress_mdl_check` 用 barrier 和两个线程验证：回调被阻塞时并发删除不能完成；回调退出后删除才获得写锁。这与 Go 的 `RLock` 覆盖整个遍历一致。
- 锁顺序固定为 `sessions` 读锁后 `print_mdl_log_time` 互斥锁；登记/注销只取得 `sessions` 写锁。新增 `InternalSession` 实现不得在回调中同步调用同一协调器的增删方法，否则会等待当前仍由自己所在调用栈持有的读锁，形成死锁风险。
- Session 回调按 `HashMap::values()` 的非确定顺序串行执行。每个回调看到前序回调过滤后的 Job 集合，最终结果等价于所有 Session 允许集合的交集；实现不应依赖遍历顺序。
- 资源所有权由 `Arc` 管理。系统会话池的 borrowed/returned/destroyed 回调界定登记生命周期；`normal_ddl_test.rs::normal_ddl_plan_user_mdl_real_internal_pool_preserves_go_restricted_bypass` 验证归还、销毁与关闭池都会清理登记项，且重复关闭不会留下 Session。

## 与 Go 版本的对应关系

- `pkg/domain/crossks/coordinator.go::schemaCoordinator` 同样保存日志时间、`RWMutex` 和 Session 集合；Rust 用 `u64` Session ID 映射替代 Go 以 `sessionctx.Context` 对象为键的集合，以便跨线程安全共享真实 MDL 状态。
- Go 的 `StoreInternalSession`/`DeleteInternalSession`/`ContainsInternalSession` 接受 `any` 后断言为 `sessionctx.Context`；Rust 把类型约束前移为 `Arc<dyn InternalSession>` 和 ID，避免运行时类型断言。
- Go `CheckOldRunningTxn` 在持有 `RLock` 时遍历 Session，并调用 `variable.RemoveLockDDLJobs`；Rust 保留锁覆盖范围与十秒日志节流，并通过 `InternalSession` trait/`TransactionMDL::check_jobs` 表达相同过滤职责。
- Go 直接修改 `map[int64]*mdldef.JobMDL`；Rust 因 InfoSchema crate 与 session-sessmgr crate 的 Job 类型不同，使用两层临时 map 并按键回写删除结果。行为目标仍是删除受阻 Job，而非释放事务锁。
- 两端 `KillNonFlashbackClusterConn` 在 crossKS 协调器上都是空实现。普通连接的 kill 行为由外层普通 Domain 协调器承担，不应添加到本文件的 crossKS 空实现中。
- `pkg/domain/crossks/cross_ks_test.go` 覆盖内部 Session 数量、包含关系及最终清理；Rust 对应覆盖分布在 `cross_ks_test.rs`、`coordinator_test.rs` 和 `pkg/session/runtime/normal_ddl_test.rs`。Rust 额外显式测试了真实 `TransactionMDL`、restricted SQL 绕过和系统 Session 池生命周期。

## 扩展指南

- 新增 Session 类型时实现 `InternalSession`，保证 `id` 在协调器生命周期内唯一，并让 `remove_lock_ddl_jobs` 只删除被自身状态阻塞的 Job。测试应放在独立的 `pkg/domain/crossks/coordinator_test.rs` 或相应 runtime 测试文件，不能内嵌进生产源文件。
- 改变 MDL 判定规则应优先修改 `pkg/session/sessmgr/mdl.rs::TransactionMDL::check_jobs` 并同步其独立测试；若只改变协调器聚合、锁语义或日志许可，则修改本文件并扩展 `coordinator_test.rs`。
- 改变 InfoSchema Job 类型或字段时，必须同步检查 `impl InfoSchemaCoordinator for SchemaCoordinator` 的双向适配、`pkg/infoschema/issyncer/syncer.rs` 调用约定及 `NormalSchemaCoordinator` 的组合过滤。
- 新增并发行为时必须保持“检查期间不能注销正在回调的 Session”这一不变量，或同时更新 Go 对齐依据和阻塞回归测试。不要在 Session 回调内回入同一协调器的写操作。
- 若要让 Rust 真实 MDL 路径输出限流日志，需要设计 `TransactionMDL` 或上层观察接口；不能仅使用当前被忽略的 `_print_log` 参数宣称已经支持。还需评估全局十秒窗口是否应继续由协调器统一维护。
- 性能优化可减少 Job 类型转换和 `table_ids` 克隆，但必须保持多 Session 过滤的交集语义、restricted SQL 绕过以及原始 InfoSchema map 只删除不改写的边界。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件；`files --filter pkg/domain/crossks` 确认目标、Go 对照与独立测试均已索引；`node --file pkg/domain/crossks/coordinator.rs` 读取全部 155 行及 21 个符号；`query` 定位 `SchemaCoordinator`、`new_schema_coordinator`、`check_old_running_transaction`、`RegisteredMDLSession`；`callees check_old_running_transaction` 确认其调用 `remove_lock_ddl_jobs`。
- 生产源码：`pkg/domain/crossks/coordinator.rs`；模块/Cargo 边界：`pkg/domain/crossks/lib.rs`、`pkg/domain/crossks/Cargo.toml`；实际 MDL 判定：`pkg/session/sessmgr/mdl.rs`。
- 上游调用：`pkg/session/runtime/session_factory.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/infoschema/issyncer/syncer.rs`；RustCodeGraph 未返回的部分上游边使用 `rg` 按符号名补齐。
- Go 对照：`pkg/domain/crossks/coordinator.go`；Go 生命周期测试：`pkg/domain/crossks/cross_ks_test.go`。
- Rust 测试：`pkg/domain/crossks/coordinator_test.rs` 验证读锁覆盖回调；`pkg/domain/crossks/cross_ks_test.rs` 验证登记 API；`pkg/session/runtime/normal_ddl_test.rs` 验证用户/内部事务联合过滤、restricted SQL 绕过与池生命周期；`pkg/server/runtime_test.rs` 验证真实 MDL 过滤适配。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰有十一个固定二级章节，并人工复核文档只描述上述源码和测试能够支持的现状。
