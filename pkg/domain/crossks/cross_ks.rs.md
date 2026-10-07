# `pkg/domain/crossks/cross_ks.rs`

[查看对应 Rust 源文件](./cross_ks.rs)

## 文件定位

本文件是 `astersql-domain-crossks` crate 的跨 Keyspace 运行时生命周期核心。crate 入口 `pkg/domain/crossks/lib.rs` 将本模块公开再导出；`pkg/domain/crossks/Cargo.toml` 则把它放在 `pkg/domain/crossks` 的独立 crate 中，并声明其直接依赖包括 KV、元数据、DDL job submit、InfoSchema 同步、session manager 和 domain server-info。

在完整应用中，生产装配器 `pkg/session/runtime/session_factory.rs::KeyspaceSessionFactory::install_on_domain` 调用 `new_manager_with_server_info_provider`，启动 `Manager::start_idle_gc`，再把 Manager 安装到 `pkg/domain/domain.rs::Domain`。跨 Keyspace 分布式任务由 `pkg/session/runtime/modify_column_dist_backfill.rs::TaskRuntimeBinding::acquire` 取得 `RuntimeHandle`；`Domain::close` 会先取出并关闭该 Manager，再清理自身 server-info。文件因此位于“服务 Domain → 目标 Keyspace runtime → Store/系统会话池/DDL 客户端”这条资源所有权链的中间层。

## 核心职责

1. `Manager` 按 Keyspace 懒创建并缓存 `SessionManager`，拒绝 classic kernel、当前 Keyspace、空目标和已关闭 Manager 上的跨 KS 请求。
2. `Manager::acquire` 以 `(keyspace, holder_id)` 追踪使用者，返回 RAII 风格的 `RuntimeHandle`；句柄显式 `release` 或 `Drop` 后，最后一个持有者的释放时间成为空闲回收基准。
3. `Manager::sweep_idle_runtimes`、`start_idle_gc` 和 `run_system_keyspace_gc_loop` 负责定时驱逐无持有者且超过 30 分钟的运行时；默认扫描周期为 60 秒。
4. `RegisteredRuntimeFactory` 在真正构造 runtime 前，针对目标 Keyspace 注册一个虚拟 server-info，并把同一个虚拟 server ID 传给 `RuntimeFactory::create_with_server_info`。成功后清理责任转交给 `SessionManager`，失败时回收注册和预备资源。
5. `SessionManager` 聚合目标 Keyspace 的 Store、InfoSchema cache、系统会话池、schema coordinator、DDL client、后台生命周期对象和 server-info syncer，并提供幂等、按顺序的关闭过程。
6. `RuntimeHandle::alter_table_mode` 与 `SessionManager::alter_table_mode` 只是到 `DdlClient` 的委托层；DDL 的解析、提交和等待逻辑位于同 crate 的 `ddl_submit.rs`，不在本文件重新实现。

## 主要符号

- 常量：`CROSS_KEYSPACE_SESSION_POOL_SIZE = 5` 与 Go 的 `crossKSSessPoolSize` 对齐；`CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT = 30 min`、`CROSS_KEYSPACE_RUNTIME_SWEEP_INTERVAL = 60 s` 控制回收；`SYSTEM_KEYSPACE = "SYSTEM"` 同时影响目标校验和 Store 关闭所有权。
- `ManagerError(String)`：本模块工厂、校验、线程启动和注册路径的轻量错误载体。
- 边界 trait：`Store` 暴露 `keyspace/close/close_on_runtime_shutdown`；`SessionPool` 暴露 `close`；`InfoCache` 暴露可选 schema；`Lifecycle` 表示必须随 runtime 逆序停止的组件；`ServerInfoSyncer` 抽象虚拟服务注册清理；`RuntimeFactory` 抽象目标 runtime 的 prepare/create/failure rollback。
- `ServerInfoRegistration`：持有尚未转交的 syncer。`into_runtime` 把它放入 `SessionManager`；若提前 Drop，则执行 `remove_server_info` 和 `revoke_session`，形成失败安全的所有权守卫。
- `RegisteredServerInfo`：包装真实 `astersql_domain_serverinfo::Syncer`，保存虚拟 server ID，并持有 server-info lease recovery 线程的 stop channel 与 join handle。
- `RegisteredRuntimeFactory`：装饰用户工厂，实现“prepare → 解析目标 etcd → 注册 server-info → 带 ID 创建 runtime → 启动 sync loop → 转交清理责任”的启动协议。
- `RuntimeEntry` / `ManagerState`：分别保存单个 Keyspace 的 `SessionManager`、holder 集合、最后释放时间，以及进程内 Keyspace 到 runtime 的映射。
- `Manager`：公开入口包括 `all_keyspaces`、`get`、`close_ks`、`get_or_create`、`acquire`、两种 GC 启动方式和 `close`；`has_active_holder` 等若干方法主要为行为测试提供可观察性。
- `RuntimeHandle`：保存到 Manager 的 `Weak`、目标 Keyspace、holder ID、强引用 `SessionManager` 和原子 released 标记；提供 Store、系统会话池与 Alter Table Mode 能力。
- `SessionManager`：单 Keyspace 资源聚合体；访问器返回 `Arc` 克隆，`set_server_info_syncer` 完成 server-info 清理责任转移，`close` 负责最终释放。

## 执行流程

生产创建流程从 `new_manager_with_server_info_provider` 开始：它用 `RegisteredRuntimeFactory` 包装真实工厂后调用 `new_manager`。第一次 `get_or_create` 或 `acquire` 进入 `get_or_create_locked`；若映射中没有目标 Keyspace，包装工厂先 `prepare`，再通过 provider 获取目标 Keyspace 自己命名空间下的 etcd client，构造 UUID 虚拟 server-info 并调用 `NewSessionAndStoreServerInfo`。注册成功后取得 ID，调用内层 `create_with_server_info`，用 runtime Store 启动 `ServerInfoSyncLoop`，最后由 `ServerInfoRegistration::into_runtime` 把清理责任交给 `SessionManager`。

持有流程由 `Manager::acquire` 执行：先拒绝空 holder，再调用 `validate_target_keyspace`，随后在 `state` 锁下创建或取得 `RuntimeEntry`。同一 Keyspace 中重复的 holder ID 返回错误；成功则插入 holder，并返回含相同 `SessionManager` 强引用的句柄。`RuntimeHandle::release` 通过 `AtomicBool::swap` 保证只执行一次，升级到 Manager 的弱引用后删除 holder；最后一个 holder 被删掉时记录 `Instant::now()`。句柄 Drop 复用相同释放路径。

回收流程中，`sweep_idle_runtimes` 在 Manager 锁内只筛选并移除“holder 为空、已有 release 时间、elapsed 达到阈值”的条目，然后在锁外逐个调用 `SessionManager::close`。`start_idle_gc` 创建名为 `crossks-idle-gc` 的线程，以 channel 的 `recv_timeout` 作为定时器和停止信号；`run_system_keyspace_gc_loop` 则兼容由外部 `Cancellation` 驱动的 SYSTEM KS 循环。

关闭流程是分层且幂等的。`Manager::close` 原子置 closed、通知并 join 自有 GC 线程、排空映射，最后在锁外关闭每个 runtime。`SessionManager::close` 先关闭 session pool，再逆序关闭 `lifecycles`，随后移除 server-info 并撤销 lease，最后仅在 `Store::close_on_runtime_shutdown` 为真时关闭 Store。默认实现保留共享 SYSTEM Store；独立打开的 SYSTEM client 可覆盖该方法表达自己的所有权。

## 数据与状态

`Manager.state: Mutex<ManagerState>` 是 runtime 映射和 holder 元数据的唯一同步点；每个 `RuntimeEntry.active_holders` 是 `HashSet<String>`，所以同一 holder 对同一 Keyspace 最多占一个槽。`last_release_at` 仅在 holder 集合变为空时写入；重新 acquire 并不清空该字段，但 GC 同时要求 holder 为空，因此活跃 runtime 不会因旧时间戳被驱逐。被驱逐后再次 acquire 会重新调用工厂并生成新 runtime。

`Manager.closed`、`RuntimeHandle.released` 与 `SessionManager.closed` 都是原子布尔值，分别保证 Manager 不接受新目标、句柄释放幂等和单 runtime 关闭幂等。`RuntimeHandle` 对 Manager 只持 `Weak`，避免“Manager → SessionManager、Handle → Manager”形成强引用环；它仍强持 `SessionManager`，因此即使 Manager 已移除映射，正在使用句柄的调用方仍不会访问悬空对象，但 Manager 的显式关闭会令底层资源进入 closed 状态。

虚拟 server-info 的状态分两阶段所有权：启动阶段由 `ServerInfoRegistration.syncer: Option<Arc<_>>` 守护；转交后由 `SessionManager.server_info_syncer: Mutex<Option<Arc<_>>>` 持有。`RegisteredServerInfo.worker` 保存停止 sender 和 join handle，`stop` 可重复调用而只消费一次 worker。

## 依赖与调用关系

上游生产调用关系如下：

- `pkg/session/runtime/session_factory.rs::KeyspaceSessionFactory::install_on_domain` → `new_manager_with_server_info_provider` → `Manager::start_idle_gc` → `Domain::install_cross_ks_manager`，这是包含自动 GC 的主要生产装配路径。
- `pkg/session/runtime/crossks_runtime.rs::CrossKSRuntimeFactory::install_on_domain` 也构造带目标 etcd provider 的 Manager 并安装到 Domain；该路径本身未在所读片段中调用 `start_idle_gc`，其生命周期由更外层装配负责。
- `pkg/session/runtime/modify_column_dist_backfill.rs::TaskRuntimeBinding::acquire` → `Domain::cross_ks_manager` → `Manager::acquire`，holder 名为 `DXF/executor/<task-id>`；绑定释放时丢弃句柄，触发 `RuntimeHandle::drop/release`。
- 同文件的 dist-task 初始化仍调用 `Manager::get_or_create(SYSTEM_KEYSPACE)`，对应 Go 源码对 raw `GetOrCreate` 与 GC 有冲突的备注。该调用没有 holder 保护，是扩展时必须谨慎处理的兼容边界。
- `pkg/domain/domain.rs::Domain::close` → `Manager::close`；替换已安装 Manager 时 `Domain::install_cross_ks_manager` 也会关闭旧实例。

本文件的直接下游包括：`astersql-domain-serverinfo`（虚拟服务注册、etcd lease 和同步循环）、同 crate 的 `DdlClient`/`SchemaCoordinator`、工厂提供的 Store/InfoCache/SessionPool/Lifecycle，以及标准库的 `Mutex`、原子、mpsc 和线程。Cargo manifest 还声明了 KV、meta/model、DDL jobsubmit、InfoSchema issyncer 和 session sessmgr；其中一部分由本 crate 其他模块或公开类型接线，并非本文件每个符号都直接调用。

RustCodeGraph 将 `cross_ks.rs` 标为被 10 个文件使用，并识别到 `pkg/domain/canonical_domain.rs`、`pkg/domain/infosync/info.rs`、`pkg/domain/serverinfo/syncer.rs` 及相关测试。由于 `acquire/close` 是仓库高频同名符号，宽泛图查询会混入大量无关节点；本文的具体生产边因此以精确文件节点和上述入口源码为准，而不把同名 blast radius 当作本模块调用证据。

## 错误处理与边界

`validate_target_keyspace` 明确拒绝空 Keyspace、classic kernel、当前 Keyspace 和已关闭 Manager；`acquire` 还拒绝空 holder 以及同一 Keyspace 上重复的 holder。工厂错误原样以 `ManagerError` 传播，失败的 runtime 不会插入映射。

server-info 注册路径包含多层回滚：目标 etcd provider 失败时调用 `registration_failed`；`NewSessionAndStoreServerInfo` 失败时先移除信息、撤销 session，再通知工厂回滚；注册后而 runtime 创建失败时，`ServerInfoRegistration` 的 Drop 自动清理注册。不过在 runtime 已创建、`RegisteredServerInfo::start` 失败的分支中，代码调用 `manager.close()` 后直接返回，局部 registration 守卫随后 Drop，仍会完成 server-info 清理。

关闭 API 有意忽略部分清理错误：`SessionManager::close` 丢弃 `Lifecycle::close` 与 `Store::close` 的错误，worker stop/join 也不向调用方返回失败；调用者因此只能依靠组件自身记录或外部健康检查观察这些故障。所有 `Mutex::lock` 使用 `expect/unwrap`，锁中毒被视为不可恢复的进程级编程错误。`close_ks` 不检查活跃 holder，会立即移除并关闭 runtime；它是管理/测试接口，不应当替代 holder 安全的空闲 GC。

`get_or_create` 不登记 holder。Go 源码 `RunSystemKSGCLoop` 的注释也明确指出 raw `GetOrCreate` 与 GC 存在冲突，计划在调用方迁移到 `Acquire` 后再收紧 API；Rust 当前仍保留同样的兼容面。因此需要跨越异步任务或 GC 周期持有 runtime 的新代码，应使用 `acquire` 而不是裸 `get_or_create`。

## 并发与资源生命周期

Manager 对映射采用单个 `Mutex`，故同一 Keyspace 的并发首次 acquire 只会创建一次 runtime，重复 holder 的竞争也只允许一个成功。测试 `test_acquire_runtime_handle_concurrently_tracks_holder_ids` 用 15 个唯一 holder 和 8 次相同 holder 并发验证：唯一 holder 全部成功，重复 holder 仅一个成功，工厂只创建一次。

耗时清理不会持有 Manager state 锁：`sweep_idle_runtimes` 和 `Manager::close` 都先移除/排空，再关闭；`test_evict_runtime_closes_idle_entry_outside_manager_lock` 让 pool 的 close 回调重新进入 `all_keyspaces`，以不会死锁证明此不变量。`close_ks` 也在释放锁后关闭目标 runtime。

GC worker 只持 Manager 的 `Weak`，不会单独延长 Manager 生命周期；Manager close 通过 channel 唤醒 `recv_timeout` 并 join，且避免在线程自身调用 close 时自 join。`RegisteredServerInfo` 的 lease recovery worker 则由 `stop` 在移除信息或撤销 session 前结束。`SessionManager` 的关闭顺序经测试固定为“session pool → 逆序 lifecycle/后台循环 → server-info remove/revoke → owned Store”，避免后台循环在会话池或 lease 已撤销后继续工作。

共享 SYSTEM Store 默认不随 runtime 关闭，这是 Go 语义；其他 Store 默认由 runtime 所有并关闭。`RuntimeHandle::Drop` 提供遗忘显式 release 时的兜底，但若句柄被永久泄漏，holder 仍不会消失，空闲回收也不会发生。

## 与 Go 版本的对应关系

Rust 主体逐项对应 `pkg/domain/crossks/cross_ks.go`：`Manager/runtimes`、`runtimeEntry`、`GetOrCreate/Acquire/release/sweepIdleRuntimes/Close`、`runtimeHandle` 和 `SessionManager` 的责任保持一致，三个容量/超时常量数值也一致。Rust 用 trait 注入 Store、pool、InfoCache、lifecycle 和 factory，将 Go `createSessionManager` 内的大量具体构造移到 `pkg/session/runtime/session_factory.rs` 等装配层；这不是删减生命周期，而是把具体实现放在 crate 边界之外。

锁策略有实现差异：Go 先用读锁快速 `get`，未命中后再取写锁；Rust `get_or_create` 直接取得单个 `Mutex`。二者都保证同一 Keyspace 单实例，但 Rust 在热点读取上的并发度较低。Go 的 `runtimeHandle` 保存 `runtimeEntry` 指针并用 `sync.Once`，Rust 保存 `Arc<SessionManager>`、Manager 的 `Weak` 和 `AtomicBool`，提供等价的幂等释放及更明确的 RAII Drop。

Go 的完整 `createSessionManager` 同时启动 InfoSchema sync、MDL check、min-job-ID refresh 等循环；Rust 本文件只定义通用 `Lifecycle` 与 server-info worker，具体 runtime 工厂在 `pkg/session/runtime/session_factory.rs` 中创建这些组件并把 `RuntimeLifetime` 等生命周期对象交给 `SessionManager`。因此不能仅阅读本文件就断言某一生产工厂启用了哪些后台循环。

Go `validateTargetKS` 未显式拒绝空 Keyspace或 closed Manager，而 Rust 增加了这两个防线。Rust 还提供 `start_idle_gc` 的 Manager 自有线程，并保留基于 `Cancellation` 的 `run_system_keyspace_gc_loop`；Go 只有 context/ticker 形式的 `RunSystemKSGCLoop`。Go close 会等待其 `WaitGroup` 后关闭 schema syncer、etcd 和 Store；Rust通过逆序 `Lifecycle::close`、`RegisteredServerInfo::stop` 与 Store 所有权策略表达对应顺序。

对应测试为 `pkg/domain/crossks/cross_ks_test.go`、`cross_ks_internal_test.go` 和 `export_test.go`；Rust 独立测试位于同目录的 `cross_ks_test.rs`、`cross_ks_internal_test.rs`、`export_test.rs`，没有把测试嵌入生产文件。

## 扩展指南

- 新增 runtime 级资源时，优先在生产 `RuntimeFactory` 中构造，并通过 `Lifecycle` 交给 `SessionManager`；保证 `close` 可重复、可在线程未启动完整时调用，并补充关闭顺序测试。不要把具体 session/etcd 实现重新塞回通用 Manager。
- 新增长期使用方时，调用 `Manager::acquire` 并使用稳定、全局可区分的 holder ID；让 `RuntimeHandle` 与实际任务同寿命。只有能证明不会跨越 GC 驱逐窗口的短暂兼容逻辑才考虑 `get_or_create`。
- 修改 holder、超时或驱逐逻辑时，同步扩展 `cross_ks_internal_test.rs`，至少覆盖重复 holder、并发首次创建、活跃 holder 不驱逐、锁外 close、驱逐后重建和 cancellation 退出。
- 修改 server-info 启动/转交时，同步扩展 `cross_ks_test.rs` 的成功、注册失败、工厂失败、晚转交和“先停循环后撤 lease”用例，并核对目标 Keyspace 使用自己的 etcd namespace 与同一虚拟 server ID。
- 修改 `SessionManager::close` 时维持幂等和当前资源顺序；特别评估 SYSTEM Store 是否共享、后台组件是否仍依赖 pool/etcd、错误是否需要从目前的 best-effort 策略升级为可观察结果。
- 修改公开类型或依赖时同步检查 `lib.rs` 再导出、`Cargo.toml`、`pkg/session/runtime/session_factory.rs`、`crossks_runtime.rs` 和 `modify_column_dist_backfill.rs`。锁粒度变化需评估首次建 runtime 的串行成本、关闭回调重入和高并发 holder 性能。
- 测试继续放在同目录的独立 `*_test.rs` 文件；本仓库约定不把 Rust 测试模块内嵌到生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/domain/crossks` 列出本 crate 的 Rust/Go 实现和独立测试；`node --file pkg/domain/crossks/cross_ks.rs` 完整读取 758 行并报告该文件被 10 个文件使用。
- 目标源码：`pkg/domain/crossks/cross_ks.rs`，重点核对 `RegisteredRuntimeFactory::create`、`Manager::{get_or_create,acquire,release,sweep_idle_runtimes,start_idle_gc,close}`、`RuntimeHandle::{release,drop}`、`SessionManager::{set_server_info_syncer,close}`。
- crate 与装配：`pkg/domain/crossks/Cargo.toml`、`pkg/domain/crossks/lib.rs`、`pkg/session/runtime/session_factory.rs::KeyspaceSessionFactory::install_on_domain`、`pkg/session/runtime/crossks_runtime.rs::CrossKSRuntimeFactory::install_on_domain`。
- 上游使用与关闭：`pkg/domain/domain.rs::{install_cross_ks_manager,cross_ks_manager,close}`、`pkg/session/runtime/modify_column_dist_backfill.rs::TaskRuntimeBinding::acquire` 及其 SYSTEM `get_or_create` 兼容路径。
- Go 对照：`pkg/domain/crossks/cross_ks.go` 的 `Manager`、`Acquire`、`createSessionManager`、`RunSystemKSGCLoop`、`sweepIdleRuntimes`、`runtimeHandle` 和 `SessionManager.close`；另核对 `cross_ks_test.go`、`cross_ks_internal_test.go`、`export_test.go` 的测试面清单。
- Rust 测试：`pkg/domain/crossks/cross_ks_internal_test.rs` 验证参数拒绝、并发 holder、空闲驱逐、锁外关闭、重建、Manager close 与 GC cancellation；`cross_ks_test.rs` 验证虚拟 server-info 注册/回滚/清理顺序、classic/current KS 拒绝、runtime 复用与 DDL 委托；`export_test.rs` 验证 `get/close_ks`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务规定的 11 章节结构命令，并人工复核本文没有把测试桩、Go 预期或外层工厂行为误写成本文件自身实现。
