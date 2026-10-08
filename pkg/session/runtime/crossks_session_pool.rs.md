# [`pkg/session/runtime/crossks_session_pool.rs`](crossks_session_pool.rs)

## 文件定位

本文件位于 `astersql-session` crate 的 `runtime` 模块中，由 `pkg/session/runtime.rs` 公开为 `crossks_session_pool`。它不是通用连接池，而是目标 keyspace 生产运行时中的系统 SQL 会话适配层：`CrossKSProductionRuntimeFactory::create_runtime` 在打开目标存储并构造目标 `Domain` 后创建一个 `CrossKSSessionPool`，再把同一个池交给跨 keyspace DDL 后端、DDL owner、jobsubmit 适配器和 DDL 系统表管理器（`pkg/session/runtime/crossks_runtime.rs`）。

该文件属于 `pkg/session/Cargo.toml` 声明的 `astersql-session` crate。直接依赖包括 `astersql-domain` 的 `Domain`、`astersql-domain-crossks` 的池生命周期接口、`astersql-ddl-jobsubmit` 与 `astersql-ddl-systable` 的会话接口，以及 `astersql-util-codec` 和本模块的 `ConcreteSession`、`kv`。Cargo 中没有为本文件单独设置 feature；跨 keyspace 总体可用性由上层运行时和内核模式决定。

## 核心职责

1. `CrossKSSessionPool` 预先创建固定数量的工作线程，每个线程只拥有一个 `ConcreteSession`，从而保证会话及其事务状态始终在线程内使用。
2. `CrossKSSessionLease` 把调用方的 SQL或元数据操作编码为 `Command`，通过通道同步转发到租约对应的工作线程，并在租约析构时归还工作线程索引。
3. `CrossKSJobSessionPool` 把租约适配成 `jobsubmit::SessionPool` / `jobsubmit::Session`，提供事务、BDR 角色、全局 ID 锁与分配等 DDL 提交语义。
4. `CrossKSSystemTablePool` 把同一租约适配成 `systable::SessionPool` / `systable::Session`，供 DDL 系统表查询使用。
5. `CrossKSFlashbackGuard` 与 `CrossKSMinJobId` 将系统表 manager 和最小 job ID 刷新器进一步适配给 jobsubmit。
6. `SessionPool::close` 负责一次性唤醒等待者、停止全部工作线程并等待退出。

## 主要符号

- `CROSS_KS_SESSION_POOL_SIZE`：直接采用 `astersql_domain_crossks::CROSS_KEYSPACE_SESSION_POOL_SIZE`，是工作线程和可同时租借会话的固定上限。
- `Command`：工作线程消息，分为普通 SQL `Query`、强类型元数据操作 `Metadata` 和终止信号 `Stop`；前两者都携带容量为 1 的同步回复通道。
- `MetadataCommand` / `MetadataValue`：封装不能仅靠本文件 SQL 文本安全表达、或需要直接访问事务/存储的操作及返回值，包括 BDR 角色与事务起始时间、全局 ID 锁、当前版本、快照时间和批量全局 ID。
- `global_id_key` / `bdr_role_key`：用 TiDB 元数据编码规则构造 `NextGlobalID` 与 `BDRRole` 键。编码由 `astersql_util_codec` 完成，键值访问通过 `kv` 接口完成。
- `Worker`：持有命令发送端和可被 `close` 取走并 `join` 的线程句柄。
- `CrossKSSessionPool::{new, try_new, acquire}`：分别提供失败即 panic 的便利构造、可返回线程创建错误的生产构造，以及阻塞式租借。
- `run_worker`：每个工作线程的事件循环；只在该线程上创建和调用一个 `ConcreteSession`。
- `run_metadata`：在工作线程上执行事务级元数据操作，并把底层错误统一转为字符串。
- `CrossKSSessionLease::{query, metadata}`：租约的请求/回复边界；`query` 是公开的 SQL 执行入口，`metadata` 是本文件内部的强类型入口。
- `job_error` / `system_table_error`：分别把字符串失败映射为 jobsubmit 的 `Storage` 错误和 systable 的 `Execute` 错误。
- `CrossKSJobSessionPool`、`CrossKSSystemTablePool`：两个借出/归还适配器；`put` 通过销毁 trait object 触发租约的 `Drop`。
- `CrossKSFlashbackGuard`：把 `systable::Manager::has_flashback_cluster_job` 转发为 jobsubmit 所需接口。
- `CrossKSMinJobId`：把 `MinJobIdRefresher::current_min_job_id` 暴露为 jobsubmit 的最小 ID 提供者。

## 执行流程

创建阶段由 `CrossKSProductionRuntimeFactory::create_runtime` 调用 `CrossKSSessionPool::try_new`。构造函数循环创建 `CROSS_KS_SESSION_POOL_SIZE` 个命名线程；每个线程进入 `run_worker`，在本线程构造一个 `ConcreteSession`。全部成功后，池把所有工作线程索引按逆序放入 `available`，因此第一次 `pop` 得到索引 0。若中途创建线程失败，已经启动的线程会收到 `Stop`，随后全部被 `join`，错误再返回给上层；上层继续关闭目标 `Domain` 和 store。

借用阶段中，`acquire` 先锁住 `available`。池已关闭时立即失败；有索引时返回绑定该索引的 `CrossKSSessionLease`；无索引时通过 `Condvar::wait` 释放互斥锁并等待归还或关闭通知，醒来后重新检查关闭状态和可用列表。

SQL 阶段中，`CrossKSSessionLease::query` 创建一次性同步回复通道，将 `Command::Query` 发送给对应 worker，并阻塞接收结果。`run_worker` 调用 `ConcreteSession::execute`，依次遍历所有 result set，并用 `next_row` 收集所有文本行；任一执行或取行错误终止本次命令并返回错误，但 worker 循环仍可接收后续命令。

元数据阶段通过 `metadata` 和 `run_metadata` 完成。`ReadBdrRoleAndStartTs` 在没有活动事务时会临时执行 `BEGIN PESSIMISTIC`，递归完成读取后再 `ROLLBACK`；已有事务时则复用当前事务。`CurrentVersion` 是唯一不要求活动事务的其他分支，直接调用目标 `Domain` 的 storage handle。其余命令先取得活动事务，否则报错：读取事务 `StartTS`，设置 `SnapshotTS`，锁住编码后的全局 ID 键，或用 `kv::IncInt64` 原子递增并返回连续 ID 区间。

归还阶段由 `CrossKSSessionLease::drop` 自动完成：若池尚未关闭，则把索引压回 `available` 并唤醒一个等待者。两个 trait 池适配器的 `put` 都只需 `drop(session)`。关闭阶段由 `SessionPool::close` 用原子交换保证只执行一次；它先唤醒所有等待者，再发送 `Stop`，最后逐一取出线程句柄并 `join`。

## 数据与状态

池级状态由 `workers`、`available`、`ready` 和 `closed` 组成。`workers` 创建完成后不再改变；`available` 中每个索引至多出现一次，索引缺席表示对应会话正被租约独占；`ready` 只协调索引归还和关闭；`closed` 是关闭状态的跨线程可见标志。`AtomicBool` 使用 Acquire/AcqRel 顺序，使关闭发布在获取和请求前检查中可见。

会话状态保存在 worker 内部的单个 `ConcreteSession` 中，而不在租约中。因此同一租约的多次命令始终到达同一 worker，`BEGIN` 后的事务状态可跨多个 `query` / `metadata` 调用延续；租约归还后，下一个借用者也可能取得同一持久会话。正确性依赖调用方在归还前结束事务。`jobsubmit::Session::rollback` 是尽力而为，忽略回滚错误；测试主要验证正常提交路径，没有证明异常归还时可自动清理脏事务。

元数据值刻意限制为 `RoleAndStartTs`、`Timestamp`、`Ids` 和 `Unit`，调用 trait 方法时按命令与返回变体的一一对应关系解包，错配被视为内部不变量破坏并 `unreachable!()`。系统表结果转换会尝试把每个文本字段解析为 `i64`，成功则产生 `systable::Value::Int`，否则按原始字节产生 `Value::Bytes`；本层不生成 `Null`。

## 依赖与调用关系

上游生产装配链为 `CrossKSProductionRuntimeFactory::create[_with_server_info]` → `create_runtime` → `CrossKSSessionPool::try_new`（`pkg/session/runtime/crossks_runtime.rs`）。随后存在四条直接使用链：

- `CrossKSProductionDdlBackend::session_variables` 直接 `acquire` 并查询 `@@sql_mode`。
- `CrossKSJobSubmitter::new` 用 `CrossKSJobSessionPool` 构造 `jobsubmit::SubmitOptions`，提交过程由此完成事务、BDR 检查和全局 ID 分配（`pkg/session/runtime/crossks_job_submit.rs`）。
- `CrossKSDdlOwner::{foreign_jobs_present, process_one, process_job}` 获取租约读取、锁定和更新目标 keyspace 的 DDL 队列（`pkg/session/runtime/crossks_owner.rs`）。
- `CrossKSSystemTablePool` 交给 `systable::new_manager`；其 manager 支撑 flashback guard 和 `MinJobIdRefresher`，后者又由生产运行时的刷新线程维护。

下游 SQL 路径是 `CrossKSSessionLease::query` → `run_worker` → `ConcreteSession::execute` / result set `next_row`。下游元数据路径是 `metadata` → `run_metadata` → `ConcreteSession` 当前事务、`kv::Transaction` 方法、`kv::IncInt64`，或 `Domain::storage_handle().with_storage(CurrentVersion)`。生命周期路径是跨 keyspace `SessionManager` 持有 `Arc<dyn astersql_domain_crossks::SessionPool>`，关闭运行时时调用本池 `close`（接口定义见 `pkg/domain/crossks/cross_ks.rs`）。

## 错误处理与边界

- `new` 对 worker 启动失败执行 `expect`，适用于测试和明确要求构造成功的调用；生产装配使用 `try_new`，能清理已启动线程并返回包含上下文的字符串错误。
- 池关闭后，`acquire`、`query` 和 `metadata` 都显式返回关闭错误。worker 通道发送或回复失败统一报告 worker 已停止。
- 普通 SQL 执行和逐行读取错误保留其显示文本，但跨过本层后不再保留结构化错误类型。jobsubmit 适配器统一标记为 `ErrorKind::Storage`；systable 适配器统一标记为 `Error::Execute` 或 `Error::Pool`。
- 除 `ReadBdrRoleAndStartTs` 可自行建立只读临时事务、`CurrentVersion` 不需要事务外，元数据命令必须位于活动事务中。`GenerateGlobalIds` 还会检查 `usize` 到 `i64` 的转换。
- `ReadBdrRoleAndStartTs` 把缺失的 BDR 元数据键解释为 `"none"`；存在但不是 UTF-8 的值会报错。临时事务的主体成功但回滚失败时，最终仍返回回滚错误。
- `set_snapshot_ts` 和 `rollback` 受上游 trait 签名限制，不向调用方返回错误；当前实现分别忽略 metadata 错误和 SQL 回滚错误。这是扩展时不能误认为“必定成功”的边界。
- 锁中毒使用 `expect`，返回变体错配使用 `unreachable!()`；这些属于内部不变量失败而非可恢复业务错误。

## 并发与资源生命周期

每个 `ConcreteSession` 只由自己的 worker 线程访问；调用线程只持有索引和通道，不跨线程移动会话本体。租约对索引的独占使一个 worker 同时只服务一个逻辑借用者，而租约内的命令按 `mpsc::Sender` 到单个 receiver 的顺序串行执行。固定大小池在耗尽时施加阻塞背压，没有超时、取消或动态扩容机制。

关闭是幂等的：第一个调用者把 `closed` 从 false 切换为 true，后续调用直接返回。关闭先 `notify_all`，保证阻塞在 `acquire` 的线程醒来观察关闭；再发送 `Stop` 并 `join`，保证 worker 已退出。已经占用的租约不会在析构时重新加入已关闭池，且后续操作会因 `closed` 检查失败。若关闭与一个已经通过关闭检查的请求并发，消息与 `Stop` 的具体入队次序决定该请求是否完成；发送/回复断开会被转成 worker-stopped 错误。

持久会话的生命周期等同于 worker 生命周期，而非单个租约。生产运行时在初始化后把池共享给多个组件；初始化失败时显式 `pool.close()`，正常关闭则由跨 keyspace `SessionManager` 的池生命周期入口触发。`CrossKSFlashbackGuard` 和 `CrossKSMinJobId` 只持有各自下游对象，不直接拥有或关闭池。

## 与 Go 版本的对应关系

Go 的对应装配位于 `pkg/domain/crossks/cross_ks.go::Manager.createSessionManager`。Go 使用容量为 `crossKSSessPoolSize`（5）的 `util.NewSessionPool`，通过目标 keyspace session factory 创建资源，并将同一个池适配为 DDL session pool 和 system-table manager；Rust 的 `CROSS_KEYSPACE_SESSION_POOL_SIZE`、`CrossKSSessionPool`、`CrossKSJobSessionPool` 和 `CrossKSSystemTablePool`保持了这一固定容量和多消费者共享关系。

两者的资源模型并非逐行同构。Go 池通过 factory 获取资源，并用创建、归还、销毁回调维护 coordinator 中的内部会话登记，还在归还时断言事务无效；Rust 本文件预先启动固定 worker，每个 worker 持有持久 `ConcreteSession`，以租约索引和消息通道维持线程亲和。本文件没有 Go 那组 coordinator 回调，也没有归还时的显式事务有效性断言；这些差异应视为当前实现事实，而不能从 Go 行为推断 Rust 已隐式具备。

DDL 元数据语义对应 Go `pkg/ddl/jobsubmit/submit.go`：`NextGlobalID` 是 meta key，必须直接悲观锁定而不能用表行 `SELECT ... FOR UPDATE`；Rust 的 `LockGlobalId` 同样设置 `SnapshotTS` 后调用 `LockKeys`，`GenerateGlobalIds` 通过 meta key 增量分配连续 ID。Go `lockGlobalIDKey` 会针对写冲突循环获取新版本并退避，而本文件只执行一次 `LockKeys`；若重试由上层承担，需以 jobsubmit 调用链验证，不能把 Go 的内部无限重试直接归于本函数。

Rust 独立回归 `pkg/session/runtime/crossks_session_pool_test.rs` 的测试名带 `go_merge_43`，验证了迁移目标中的会话持久性与关闭、缺失 BDR 角色为 `none`、正的事务开始时间、版本/锁/快照/连续全局 ID，以及空 DDL job 表上的 flashback 与最小 job ID 行为。Go 的跨 keyspace 综合测试还覆盖“只允许系统表”等更广运行时约束，那些约束不由本文件单独实现。

## 扩展指南

- 新增需要保持事务连续性的操作时，应扩展 `MetadataCommand`、`MetadataValue`、`run_metadata` 和 `CrossKSSessionLease` 的封装，确保底层会话仍只在 worker 线程上访问；同时在独立的 `pkg/session/runtime/crossks_session_pool_test.rs` 增加正常、无事务和关闭后的回归用例。
- 修改池容量、等待或关闭策略时，重点检查索引唯一性、关闭唤醒、请求与 `Stop` 竞态以及 worker 启动半失败清理。若引入超时/取消，不能让已超时调用的回复阻塞 worker，也不能提前归还仍在执行命令的索引。
- 修改 jobsubmit 适配时应同步核对 `pkg/ddl/jobsubmit/types.rs::Session` 与 Go `pkg/ddl/jobsubmit/submit.go`，尤其是悲观事务、写冲突重试、快照时间和全局 ID 连续性；不要仅以 SQL 执行成功代替元数据锁语义。
- 修改系统表行转换时应核对 `pkg/ddl/systable/manager.rs::Row` 的 `bytes` / `int64` 解码约定。当前“能解析为 i64 就转换为 Int”的启发式可能影响数字样式的字节列，扩展类型时需增加精确列类型证据和测试。
- 若要强化脏事务清理，应先对齐 Go 池归还断言和 `jobsubmit` 的所有错误路径，再决定在 `Drop`、`put` 或 worker 中处理；析构不能返回错误，因此需要可观测的失败策略。
- 若新增外部依赖或 feature，需同步 `pkg/session/Cargo.toml`；普通逻辑修改还应遵守仓库要求，把 Rust 测试留在独立测试文件中。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件、307,296 个节点、1,848,419 条边；使用 `node --file` 完整读取 `pkg/session/runtime/crossks_session_pool.rs`，并读取直接装配/调用文件 `crossks_runtime.rs`、`crossks_owner.rs`、`crossks_job_submit.rs`，以及接口文件 `pkg/domain/crossks/cross_ks.rs`、`pkg/ddl/jobsubmit/types.rs`、`pkg/ddl/systable/manager.rs`。
- 主要调用证据：`CrossKSProductionRuntimeFactory::create_runtime` 创建池并装配 system-table manager、job submitter、owner 和跨 keyspace `SessionManager`；`CrossKSDdlOwner` 与 `CrossKSProductionDdlBackend` 直接借用租约；两个适配池分别满足 jobsubmit 和 systable trait。
- crate 与模块证据：`pkg/session/Cargo.toml`、`pkg/session/lib.rs`、`pkg/session/runtime.rs`；目标包下未发现 `doc.go`，因此没有额外的包级 Go 契约文件可读。
- Go 对照：`pkg/domain/crossks/cross_ks.go::createSessionManager` / `SessionManager.close`，以及 `pkg/ddl/jobsubmit/submit.go::lockGlobalIDKey`。
- 测试证据：`pkg/session/runtime/crossks_session_pool_test.rs` 的三个独立测试覆盖池复用与关闭、jobsubmit 元数据/全局 ID 路径、系统表/flashback/min-job-ID 路径。本任务按计划为纯文档分析，未运行 Cargo，也未把测试嵌入生产源文件。
