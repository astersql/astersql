# `pkg/session/runtime/normal_ddl_service.rs`

## 文件定位

本文件属于 `astersql-session` crate（见 `pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以公开模块 `normal_ddl_service` 装配。它不是 SQL DDL 语义的具体实现，而是普通（serving、非 cross-keyspace）`Domain` 的 DDL 生命周期适配层：把 owner 选举、持久化任务调度、schema/MDL 同步、系统会话池和存储层级迁移轮询组合成 `astersql_domain::domain::DdlService`。真实装配入口是 `pkg/session/runtime/session_factory.rs::install_serving_ddl_runtime`；cross-keyspace 工厂不创建本服务，符合文件头注释所述边界。

## 核心职责

1. `NormalDdlService` 实现 `DdlService`：启动/停止普通 DDL、暴露 owner 身份、提交持久化任务与表模式变更，并查询存储层级迁移状态。
2. owner 任期内运行 `JobScheduler`，同时消费 General 与 AddIndex 两类持久化队列；新任期先重新加载 schema，再创建任期绑定的 executor。
3. `UpgradeState` 把集群 upgrading 状态发布成 owner operation；同步失败会记录到 `last_error` 并在下一轮强制重试，不继续调度任务。
4. `NormalSchemaRuntime` 拥有普通 Domain 的四个后台循环：schema reload、MDL 检查、job schema-version 发布和最小 job ID 刷新。
5. `NormalSchemaCoordinator` 同时检查普通用户事务和借用的内部 SQL 会话，形成 MDL 栅栏。
6. NextGen 下轮询 `mysql.tidb_storage_class_transition_history`，汇总 TiFlash store 观测、完成迁移并定期裁剪历史。

## 主要符号

- `parse_transition_time`：按 `%Y-%m-%d %H:%M:%S%.f` 解析历史表时间；解析错误转为 `String`。
- `transition_statuses`：读取 RUNNING 历史行，重建 `StorageClassTransitionOperation`，以 manager 缓存观测覆盖初始状态并重算持续时间。
- `poll_storage_class_transitions` / `prune_storage_class_transition_history`：分别执行 NextGen 进度聚合/完成更新，以及按稳定复合排序边界删除超额完成或被替代历史。
- `DomainSchemaLoader(Weak<Domain>)`：`SchemaLoader` 适配器；升级弱引用后调用 `Domain::reload`，避免服务反向强持有 Domain。
- `Lease`：把 `Manager` 的 owner epoch、owner 状态和取消上下文适配成 `JobLease`。
- `Lifecycle` / `Worker`：在互斥锁内记录 started、closed、启动模式和 scheduler 线程的 stop sender/join handle。
- `UpgradeState::sync`：消费/重建状态 watch，读取全局状态，并在 owner epoch 再确认后写入 `OpSyncUpgradingState` 或 `OpNone`。
- `NormalDdlService::{new,with_schema_runtime,with_upgrade_policy,last_error}`：构造服务、附加 schema 资源、替换升级感知 executor 工厂，以及暴露后台最近一次错误。
- `DdlService for NormalDdlService`：本文件的公开运行时契约，包含 `start`、`stop`、job 提交、状态查询和 owner 查询。
- `NormalSchemaCoordinator`：实现 `InfoSchemaCoordinator`，联合 internal coordinator 与 Domain session manager。
- `NormalSchemaRuntime::{start,close,active_loop_count,schema_barrier}`：管理四个线程并为每次 owner 任期创建 `NormalDdlSchemaBarrier`。

## 执行流程

装配时，`prepare_normal_schema_runtime`（`session_factory.rs`）先创建共享 validator、`SystemSessionPool`、schema syncer、版本协议和 min-job refresher，完成协议初始化与首次 reload；随后 `install_serving_ddl_runtime` 构造 `NormalDdlService`，调用 `with_schema_runtime` 与 `with_upgrade_policy`，写入 `Domain::set_ddl`，最后 `start(StartMode::Normal)`。

`start` 在 `Lifecycle` 锁内拒绝已关闭实例并对重复启动直接成功。它先启动 DXF worker；允许竞选时，Upgrade 模式先强制成为 owner，再发起 campaign，并创建 `normal-ddl-scheduler` 线程。线程每 300 ms（或 stop 消息）推进一轮：若当前是 owner，先同步 upgrading 状态；owner epoch 改变时销毁旧 scheduler/executor、重建双 worker scheduler，并以 200 ms 间隔 reload schema，期间持续检查取消、owner 身份和 epoch。栅栏通过后才懒创建 executor、从系统会话池调度持久化任务，并按 10 秒/60 秒节奏轮询迁移与裁剪历史。丢失 owner 时清空迁移缓存和任期资源，等待重新取得 owner。最后启动附加的 `NormalSchemaRuntime` 四循环并将生命周期标记为 started。

`stop` 先在锁内一次性设置 closed 并取走 worker，随后取消服务上下文、DXF worker、升级 watch、schema 等待以及 schema runtime 上下文；再通知并 join scheduler，关闭 owner，关闭 schema runtime，最后关闭会话池。`Drop` 再调用一次 `stop`，依赖其幂等语义。

## 数据与状态

`NormalDdlService` 的长期共享状态均由 `Arc` 持有；`lifecycle: Mutex<Lifecycle>` 串行化 start/stop，`last_error: Arc<Mutex<Option<String>>>` 在后台线程和调用方之间传递诊断。`campaign_enabled` 决定是否产生 scheduler；即使禁用竞选，schema runtime 与 DXF worker仍按生命周期启动。`upgrade_state` 和 `schema_runtime` 是可选能力，分别由 builder 方法安装。

owner 任期由 `owner_epoch` 标识。executor 只在有效任期内懒创建，任期变化或失主即清空；这避免旧 barrier/上下文跨任期复用。`NormalSchemaRuntime::lifecycle` 记录四个 join handle；`started` 使重复启动无副作用，`closed` 使关闭和析构可重复。

迁移状态的持久事实来自 `mysql.tidb_storage_class_transition_history`，内存 manager 只是观测缓存。进度使用饱和加法汇总各 store 的 ready/total；只有至少观测到一个非零副本集合且 ready 等于 total 才完成。历史裁剪按 `finish_time, table_id, start_ts, direction` 的完整逆序键选择边界，以便重复删除仍指向同一旧键区间。

## 依赖与调用关系

上游调用链为 `session_factory::install_serving_ddl_runtime -> NormalDdlService::new -> with_schema_runtime -> with_upgrade_policy -> Domain::set_ddl -> DdlService::start`。`Domain` 后续通过 `DdlService` trait 调用 start/stop、持久化提交与状态查询；相关测试也直接构造服务以验证 owner 交接。

调度下游为 `astersql_ddl::{JobScheduler, JobWorker, DurableJobExecutor}`、`normal_ddl_submit::submit_and_wait` 和 `SystemSessionPool`。owner/升级下游为 `astersql_owner::Manager` 与 `astersql_ddl_serverstate::Syncer`。schema 下游为 `astersql_infoschema_issyncer::Syncer`、`astersql_infoschema_isvalidator::Validator`、`astersql_ddl_schemaver::Syncer` 和 `astersql_ddl_systable::MinJobIdRefresher`。存储迁移下游为 `astersql_domain_infosync`、`astersql_store_helper`、`astersql_meta_model` 与 `storage_class_transition` 模块。

`pkg/session/Cargo.toml` 明确列出上述 workspace crate，并以 `nextgen` feature 联动 `astersql-config-deploymode/nextgen` 和 `astersql-config-kerneltype/nextgen`；实际轮询仍在运行时通过 `IsNextGen()` 判断。

## 错误处理与边界

公开方法统一返回 `Result<_, String>`。构造/启动关键步骤（DXF、ForceToBeOwner、CampaignOwner、线程创建、schema runtime 启动）失败会向调用者传播；线程创建失败额外取消 campaign，schema runtime 启动失败则调用整服务 `stop` 清理部分资源。后台调度和迁移轮询错误不会杀死线程：调度错误写入 `last_error` 并打印，迁移错误只打印后等待下一周期。一次成功调度会清空 `last_error`。

行解析严格检查列数以及整数、JSON、方向和时间格式；非法数据立即返回错误。Tombstone store 的采集错误被忽略，其他 store 错误传播。`submit_persistent_job` 与 `alter_table_mode` 在 closed 后拒绝调用。Upgrade 模式且 DDL 竞选禁用时明确返回 `DDL must be enabled when upgrading`。

值得注意的对齐边界：Rust 的迁移轮询比 Go `storageClassTransitionManager.poll` 更窄。当前 Rust 路径未在本文件中执行 Go 的 schema-version eligibility、孤儿表 supersede 与分区拓扑 reconcile；文档不能据此声称这些行为已由本服务支持。Rust 完成 UPDATE 也不检查 affected rows，而是随后直接移除缓存。它们是当前实现差异，不应在本纯文档任务中改写。

## 并发与资源生命周期

服务有一个 scheduler OS 线程，schema runtime 有四个具名 OS 线程。scheduler 的退出条件是 cancellation、stop channel 发送或 sender 断开；每轮 owner 操作前后都以 epoch 和取消状态防止旧 owner 继续工作。`UpgradeState::sync` 在读取全局状态后再次验证 owner/epoch，避免状态读取与发布之间发生交接。

关闭顺序刻意先取消可能阻塞的 schema 发布与等待，再 join scheduler；注释指出 scheduler 可能正处于共享 schema loader 内，因此 pool 必须保留到所有使用者退出后才关闭。`NormalSchemaRuntime::close` 先取消两个上下文，再 drain/join 全部循环，然后关闭版本协议、停止 validator、关闭 pool。`DomainSchemaLoader` 和 `NormalSchemaCoordinator.domain` 使用 `Weak<Domain>`，防止 Domain—DDL—pool 的强引用环。

锁粒度方面，`start` 在完成启动期间持有 lifecycle 锁；`stop` 只在取出 worker 时持锁，join 时已释放。`last_error` 单独加锁，不与生命周期锁嵌套。系统会话借出/归还/销毁回调由 `prepare_normal_schema_runtime` 注册到 internal coordinator，使内部事务的 MDL 状态与借用生命周期一致。

## 与 Go 版本的对应关系

`pkg/ddl/ddl.go::{newDDL,ddl.Start,ddl.Stop}` 是服务生命周期的主要 Go 对照：两边都组装 owner、schema/state syncer、系统会话、min-job refresher和任务执行器；Upgrade 都要求启用 DDL并强制成为 owner。Rust 将原先散布在 `ddl` 与 `Domain.Start` 的 schema 循环集中到 `NormalSchemaRuntime`，并用 RAII/幂等 `close` 表达资源所有权。

`pkg/ddl/job_scheduler.go::{schedule,checkAndUpdateClusterState}` 对应 scheduler 线程的 takeover reload、状态 watch、owner op 发布和持久化队列调度。Rust 额外显式记录 owner epoch，并在任期变化时重建 executor/scheduler。

`pkg/domain/domain.go::Domain.Start` 对应普通 Domain 在 DDL 启动后运行 `SyncLoop` 与 `MDLCheckLoop`；Rust 的四循环还包含 `SyncJobSchemaVerLoop` 和 min-job refresher。`pkg/ddl/storage_class_transition.go::{run,poll,pruneStorageClassTransitionHistory,completeStorageClassTransition}` 对应 10 秒轮询和 60 秒裁剪，但如上一节所述，Go 的拓扑与 schema 发布防护更完整。

Rust 独立测试位于 `pkg/session/runtime/normal_ddl_test.rs`，而非源文件内：`crossks_align_normal_ddl_service_consumes_mixed_queue_and_stops_owner` 验证混合队列、启动/停止幂等与关闭后拒绝重启；`crossks_align_normal_ddl_service_public_owner_handoff_consumes_same_queue` 验证交接；`normal_ddl_plan_schema_runtime_*` 验证四循环、构造失败清理和取消；`normal_ddl_plan_upgrade_owner_*` 验证升级状态同步和重试。迁移 SQL/状态另由 `normal_ddl_create_table_test.rs` 覆盖。

## 扩展指南

新增 DDL 生命周期能力时，优先判断归属：任期内任务放入 `DdlService::start` 的 scheduler 循环；普通 Domain 长寿命 schema 任务放入 `NormalSchemaRuntime::{start,close}`，并同时补齐取消、join、部分启动失败清理；executor 策略放入 `with_upgrade_policy` 或其工厂；公开提交能力应扩展 `DdlService` trait及 `install_serving_ddl_runtime` 接线。

修改 owner 流程必须保持三项不变量：读取外部状态后再次核对 epoch、交接时丢弃任期绑定 executor、reload 完成前不加载队列。修改关闭路径必须维持“先取消阻塞点、再 join、最后关闭 pool”。增加线程时要把 handle 纳入 `SchemaLifecycle::loops` 并在 `normal_ddl_plan_schema_runtime_service_owns_domain_loops` 一类独立测试中更新期望。

扩展迁移轮询时应以 Go `storage_class_transition.go` 为语义基线，特别同步 schema-version gate、孤儿记录 supersede、分区拓扑 reconcile、affected-row 竞争语义和请求超时；测试应放在同目录独立 `*_test.rs`，不可嵌入本源文件。兼容风险主要是 Go/Rust 状态机偏差；性能风险主要是 300 ms scheduler 轮询内增加阻塞 I/O、逐 operation×store 请求放大以及持 lifecycle 锁执行慢启动。

## 验证依据

- 源码：`pkg/session/runtime/normal_ddl_service.rs`（全文 857 行），重点核对 `NormalDdlService`、`UpgradeState::sync`、`DdlService` impl、`NormalSchemaCoordinator`、`NormalSchemaRuntime` 和三个迁移辅助函数。
- 装配与 crate：`pkg/session/runtime.rs`、`pkg/session/runtime/session_factory.rs::{prepare_normal_schema_runtime,install_serving_ddl_runtime}`、`pkg/session/Cargo.toml`。
- Rust 独立测试：`pkg/session/runtime/normal_ddl_test.rs` 的 service、owner handoff、schema runtime/barrier、upgrade owner 测试；`pkg/session/runtime/normal_ddl_create_table_test.rs` 的存储层级迁移记录与状态测试。
- Go 对照：`pkg/ddl/ddl.go`、`pkg/ddl/job_scheduler.go`、`pkg/ddl/storage_class_transition.go`、`pkg/domain/domain.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点；`query NormalDdlService --json`、`query NormalSchemaRuntime --json`、`query poll_storage_class_transitions --json` 和 `query install_serving_ddl_runtime --json` 精确定位了上述符号。针对本文件的 `explore/files/node --file` 无输出，按技能回退到源码读取；对 struct/function ID 的 callers/callees 查询在 30 秒内无结果，因此调用边由模块入口、构造点和直接引用的 `rg` 结果交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo；结构检查要求目标存在且固定二级标题恰为 11 个。
