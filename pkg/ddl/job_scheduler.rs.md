# `pkg/ddl/job_scheduler.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod job_scheduler` 对外暴露。它位于 Rust 正常 DDL 的 owner 侧调度层：生产入口 `pkg/session/runtime/normal_ddl_service.rs::NormalDDLService::start` 创建名为 `normal-ddl-scheduler` 的线程，取得 owner 身份后构造 `JobScheduler`，再周期性调用 `JobScheduler::schedule_persisted`。

当前文件同时保留两套表面：`enqueue` + `schedule` 驱动内存 `VecDeque<ScheduledJob>`，用于轻量迁移语义及冲突测试；`schedule_persisted` 每轮从 `mysql.tidb_ddl_job` 读取 Go wire 格式的持久任务，是已接入正常 DDL 服务的生产路径。二者共享 `RunningJobs`、General/Reorg 两类 `JobWorker`，但不可把内存队列路径理解为生产持久化或故障恢复机制。

从 DDL 生命周期看，这是 job-based 路径而非 metadata-only fast path：scheduler 本身不实现具体 schema 状态转换或回填，它选择可运行任务并委托 `pkg/ddl/job_worker.rs`；worker/executor 负责事务、状态推进、持久化和 schema-version barrier。是否需要 delete-only/write-only/reorg/public 等状态、回填检查点、回滚或 delete-range GC，由具体 job executor 决定。

## 核心职责

1. 只允许有效 owner 推进持久任务：`schedule_persisted` 在打开 SQL 查询前检查 `closed`、`JobLease::is_owner` 和 `JobLease::is_cancelled`；正常服务还用 owner epoch 在换主时丢弃旧 scheduler 与 executor。
2. 按涉及对象做互斥/共享冲突控制：从 job 的 `get_involving_schema_info` 生成 `InvolvingSchemaInfo`，交给 `RunningJobs::check_runnable`，不可运行的任务进入本轮 pending 集合。
3. 将普通任务和重组任务路由到不同 worker：内存路径依据 `ScheduledJob::job_type`，持久路径依据表中 `reorg` 列选择 `general_worker` 或 `reorg_worker`。
4. 每次只推进 job 的一个持久步骤，并在步骤后等待 schema 同步：`JobWorker::transit_persisted_job_step` 的版本结果随后传给 `DurableJobExecutor::wait_synced`。
5. 在成为 owner 前后维护 schema 新鲜度：`must_reload_schemas_with` 持续调用 `SchemaLoader::reload`，仅在失败后检查取消并按给定间隔重试。
6. 提供 Go `unSyncedJobTracker` 的集合语义和轻量内存调度接口；当前 Rust `UnSyncedJobTracker` 在仓库中仅由独立测试引用，生产持久调度的同步恢复由 worker/executor 接口承担。

## 主要符号

- `JOB_RECORD_CAPACITY: usize = 16`：两个 tracker 集合的初始小容量；清空一次运行提示集合时也回到此容量。
- `JOB_ONCE_CAPACITY: usize = 1000`：`once_ids` 的软上界。`set_already_run_once` 在插入前发现 `len() > 1000` 才整体替换，因此集合可先增长到 1001，再在下一次写入时清空；这是与 Go 容量控制相同的提示性缓存，不是精确 LRU。
- `UnSyncedJobTracker`：用两个 `RwLock<HashSet<i64>>` 分别记录尚未完成 schema 同步的 job ID，以及当前 owner 可能已执行过一次的 job ID。`add_un_synced`/`remove_un_synced` 幂等，`maybe_already_run_once` 只提供布尔提示。
- `JobType::{General, Reorg}`：内存调度路径的 worker 分类。
- `ScheduledJob`：内存排队单元，包含简化 `Job`、冲突对象 `involves`、worker 类型和 `SchemaAction`。
- `JobScheduler`：持有 owner/closed 状态、内存队列、`RunningJobs`、两类 worker 和可观察的 `reload_schema_count`。其公开方法包括构造、owner 生命周期、入队、两类调度、关闭、schema 重载和背压判断。
- `JobScheduler::schedule`：测试/轻量路径；每轮只扫描开始时的队列长度，避免本轮重新入队的 job 再次执行。
- `JobScheduler::must_reload_schemas_with<L, C>`：可注入 loader、重试间隔和取消谓词的 owner 接管重载循环。
- `JobScheduler::schedule_persisted`：生产核心入口；依赖 `DurableJobSession`、`JobLease`、`DurableJobExecutor` 三个动态接口和 `min_job_id` 下界。

## 执行流程

生产路径从 `pkg/session/runtime/normal_ddl_service.rs` 开始：服务线程竞选 owner；检测到新的 owner epoch 时关闭旧 scheduler，创建两类 `JobWorker`，调用 `must_reload_schemas_with`。确认 owner/epoch 仍有效后取得数据库 session，并执行 `schedule_persisted(..., 0)`；线程以 300 ms 的接收超时继续下一轮，失去 owner 时清空关联状态、关闭并重建 scheduler。

`schedule_persisted` 的一轮流程如下：

1. 若已关闭、不是 owner 或租约已取消，立即返回 `Ok(0)`，且不访问 SQL session。
2. 执行 `select reorg, job_meta from mysql.tidb_ddl_job where job_id >= {min_job_id} order by job_id`，保证按 job ID 读取持久队列。
3. 每行开始前再次检查 owner/cancellation；所有行必须恰有两列，否则返回 `invalid durable DDL queue row`。
4. 将 `reorg` 解析为整数，将 `job_meta` 通过 `astersql_meta::decode_go_history_job` 解码为 `astersql_meta_model` job，再将 shared/exclusive 涉及对象映射到本 crate 的 `InvolvingMode`。
5. 先调用 `executor.runnable` 处理暂停、取消、升级等管理策略，再调用 `RunningJobs::check_runnable` 做 schema/table/policy/resource-group 冲突判断；任一不允许运行时只记 pending 并继续扫描后续 job。
6. 登记 running，按 `reorg` 选择 worker，调用 `transit_persisted_job_step` 推进一步；成功取得版本后串接 `executor.wait_synced`。
7. `Synced`、`Cancelled`、`RollbackDone` 是最终状态。未完成或步骤/同步失败时保留其 pending 冲突依赖；成功处理一个 job 后计数加一。
8. 无论闭包成功还是报错，返回前都执行 `running.reset_all_pending()`，使 pending 只在本轮用于公平性/冲突屏障，而不会永久阻塞后续轮次。

内存 `schedule` 也先检查 owner/closed，随后对本轮初始队列逐项检查冲突、选择 worker 并调用 `transit_one_job_step`。只有 `JobState::Synced | Cancelled` 视为结束；其他状态回到队尾。注意它在 `result?` 前已更新 running/pending 和队列，所以 worker 返回错误时状态仍保留供后续处理。

## 数据与状态

`JobScheduler::owner` 和 `closed` 是局部生命周期门闩；生产路径的真实 owner 权威来自传入的 `JobLease`，而 `schedule_persisted` 不读取 `self.owner`。`on_become_owner`/`on_retire_owner` 只影响内存 `schedule`，其中接管会增加 `reload_schema_count`，卸任会清空 owner 标记并重置 pending。

`queue` 只保存内存 `ScheduledJob`。持久路径没有把 SQL 行复制进该队列，而是每轮重读 `mysql.tidb_ddl_job`，所以 owner 重启后能从数据库恢复；实际 job 元数据、状态、schema version、reorg checkpoint 和历史迁移由 `JobWorker`/`DurableJobExecutor` 维护，不存放在 scheduler 自身。

`running: RunningJobs` 是冲突控制状态。任务在推进前加入 running；完成时移除，未完成或失败时转为 pending。每轮结束清空 pending 标记，下一轮再按持久状态重新判定。`worker_pool_exhausted` 仅以“内存队列非空且至少有 running ID”作为轻量背压信号，不等同于 Go worker pool 的可用槽位计数，也未用于 `schedule_persisted`。

`reload_schema_count` 是计数型可观察状态，不表示加载到哪个 schema version。两个 tracker 集合在锁内更新；锁 poisoning 使用 `expect`，因此不是可恢复业务错误。

## 依赖与调用关系

上游生产调用边为：`NormalDDLService::start` → owner 调度线程 → `JobScheduler::must_reload_schemas_with` → `JobScheduler::schedule_persisted`。RustCodeGraph 和仓库搜索还显示，大量 `pkg/session/runtime/normal_ddl*_test.rs` 与 `durable_scheduler_test.rs` 直接调用持久入口，覆盖重启、换主、暂停/取消、事务冲突和 schema barrier 等组合场景。

主要下游关系为：

- `crate::ddl_running_jobs::RunningJobs`：对象冲突、running/pending 集合与公平性。
- `crate::job_worker::{JobWorker, JobContext}`：内存 job 的单步状态推进。
- `crate::job_worker::{DurableJobSession, JobLease, DurableJobExecutor}`：持久 SQL 访问、owner/cancel/epoch 边界、job 策略及同步等待。
- `astersql_meta::decode_go_history_job`：解码数据库中兼容 Go 的 job wire 数据；`pkg/ddl/Cargo.toml` 将 `astersql-meta` 声明为同 workspace 路径依赖。
- `astersql_meta_model::group_3::{JobState, InvolvingSchemaInfoMode}`：持久 job 的最终状态和冲突模式；manifest 以 `astersql-meta-model` 路径依赖提供。
- 标准库 `VecDeque`、`HashSet`、`RwLock`、`thread::sleep`：内存排队、集合跟踪和重载退避。

模块入口 `pkg/ddl/lib.rs` 公开 scheduler，并在独立模块中挂接 `job_scheduler_test.rs` 与 `job_scheduler_testkit_test.rs`；测试逻辑没有内嵌在生产源文件。

## 错误处理与边界

`schedule_persisted` 使用 `Result<usize, String>`，SQL 查询错误、行形状错误、`reorg` 解析错误、job 解码错误、策略判断错误、worker 步骤错误和同步等待错误都会向上返回。正常服务捕获字符串错误，记录到 `last_error` 并在下一轮继续；该文件不吞掉失败，也不自行把错误 job 改成 Cancelled。

最重要的边界是步骤可能已经提交、但 schema 同步失败。代码先执行 step，再 `wait_synced`，并用 `!finished || step.is_err()` 将冲突依赖保留为 pending；注释明确失败步骤可能已提交，调用者不得把返回错误解释为“没有状态变化”。下一轮会重新读取持久 job，由 worker/executor 的恢复逻辑判定如何继续。

owner/cancellation 在查询前和逐行处理前检查，但长时间运行的具体步骤还必须由 `JobLease` 和 worker 内部检查保护。若租约在行间丢失，循环停止并返回已成功推进的数量；若在具体步骤中丢失，错误/恢复语义由 worker 返回。

内存路径用 `JobState::Synced | Cancelled` 作为终态，而持久路径还接受 `RollbackDone`；扩展状态时必须明确两条路径是否都需要调整。`must_reload_schemas_with` 在第一次 `reload` 前不检查取消，且只在失败后检查，因此即使已经取消也会至少尝试一次加载，这一点由测试固定。

## 并发与资源生命周期

`JobScheduler` 本身的方法需要 `&mut self`，没有设计为被多个线程并发调用；正常服务把它封闭在单个 `normal-ddl-scheduler` 线程内。生产循环串行扫描 SQL 行，每个选中的 job 在本调用中完成“一步 + 同步等待”后才处理下一行；文件内没有生成每 job 线程，也没有实现 Go 的异步 worker pool 并发度。

`UnSyncedJobTracker` 可被共享引用并用两个独立 `RwLock` 保护集合。读操作允许并发，写操作独占；任何 panic 导致的 poisoned lock 会令后续访问 panic。它不持有外部资源，清空 once 集合会释放旧集合并重新分配小容量集合。

`close` 幂等设置 `closed`，随后依次关闭 General 与 Reorg worker。正常服务在换主、失主和线程退出时调用它；服务还会丢弃 executor/session，从而避免旧 owner epoch 的可变状态被继续使用。SQL session 的借用生命周期由调用方 pool 控制，本文件只通过 trait object 使用，不拥有连接池。

`must_reload_schemas_with` 使用阻塞 `thread::sleep`，因此只能放在专用调度线程；零间隔可用于独立测试。Go 版通过 context 与定时器退出，Rust 版用注入的 `is_cancelled` 闭包表达同一取消边界。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/ddl/job_scheduler.go`。两边共同语义包括：仅 owner 调度、接管后强制重载 schema、按 `mysql.tidb_ddl_job` 的 `reorg/job_meta` 读取任务、根据 involving schema 信息做冲突控制、普通与 reorg worker 分流，以及单步推进后等待 schema version 同步。

对应关系如下：

- Rust `schedule_persisted` 对应 Go `schedule` 循环中的 `loadAndDeliverJobs` 核心读取与筛选部分；Rust 正常服务外层线程承担 Go `scheduleLoop` 的重试/owner 生命周期。
- Rust `must_reload_schemas_with` 对应 Go `mustReloadSchemas`；二者均持续重试至成功或取消。Go 成功后还调用 `markStorageClassTransitionReady`，Rust 由正常服务在 scheduler 之外轮询 storage-class transition，当前函数自身不发布该 ready 信号。
- Rust `RunningJobs` 调用对应 Go `runningJobs.checkRunnable/addPending/addRunning/finishOrPendJob/resetAllPending`。
- Rust `UnSyncedJobTracker` 的接口和容量行为对应 Go `unSyncedJobTracker`，但当前 Rust 生产持久调度没有直接持有或调用它；Go 在 `transitOneJobStepAndWaitSync` 中用于换主后的未同步版本恢复。

重要差异是 Go `deliveryJob` 从有容量的 worker pool 取 worker，为每个 job 启动异步 goroutine，并持续推进到终态/暂停/换主；Rust 文件只有固定的两个 `JobWorker` 值，生产路径串行推进每个任务一步。Go 还会排除已 running ID、检查 pool `available()`、支持 next-gen background pool、etcd/通知通道、指标/trace、MDL 清理与 storage-class ready；这些能力不在本文件中，部分由 Rust worker、executor 或正常服务承担，部分没有在此调度层复刻。扩展时应以当前 Rust 接线为事实，不能直接假设 Go 的并发与观测能力已经存在。

## 扩展指南

新增调度策略时，优先判断它属于哪个层次：涉及 SQL 队列筛选、owner/cancel 门禁或冲突登记，修改 `schedule_persisted`；涉及具体 job 状态机、事务、reorg checkpoint、schema diff/版本同步或 rollback/delete-range，修改对应 `DurableJobExecutor`/`JobWorker` 实现，而不是把业务动作塞进 scheduler。

新增 job 终态或暂停状态时，应同步检查 `schedule` 的 `finished`、`schedule_persisted` 的 `finished`、`executor.runnable` 和 `RunningJobs::finish_or_pend_job` 的依赖保留规则。新增冲突维度时，应从 meta job 的 `get_involving_schema_info` 开始，保证映射完整，并在 `pkg/ddl/ddl_running_jobs_test.rs` 增加 shared/exclusive、公平性及多对象覆盖。

改变 owner 接管行为时，要保持顺序不变量：先同步 cluster upgrade state，再重载 schema，确认 owner epoch 未变，最后打开持久队列并执行 job。必须覆盖“查询前失主”“步骤提交后同步失败”“重启重新读取同一 job”“系统暂停与用户暂停并发”等情况。

测试应保持独立文件：基础重载/tracker 行为放在 `pkg/ddl/job_scheduler_test.rs`；内存调度冲突和 Go 归一化序列放在 `pkg/ddl/job_scheduler_testkit_test.rs`；持久调度及故障恢复优先扩展 `pkg/session/runtime/durable_scheduler_test.rs` 或相应 `normal_ddl*_test.rs`。性能改动需特别评估当前串行“一步 + wait”是否成为瓶颈；若引入并发，必须同时解决 session/worker 所有权、running/pending 原子性、worker 容量、公平性和换主取消，不能只并行循环。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标 `pkg/ddl/job_scheduler.rs` 被索引为 330 行、43 个符号。
- 目标源码：`pkg/ddl/job_scheduler.rs`，核对了常量、`UnSyncedJobTracker`、`JobType`、`ScheduledJob`、`JobScheduler` 的全部实现及 `schedule_persisted` 完整控制流。
- crate 与模块边界：`pkg/ddl/Cargo.toml`（`astersql-ddl`、`astersql-meta`、`astersql-meta-model` 路径依赖）和 `pkg/ddl/lib.rs`（公开模块及两个独立测试模块）。
- 生产上游：`pkg/session/runtime/normal_ddl_service.rs:410-559`，确认专用线程、owner epoch、schema reload、session 获取、`schedule_persisted` 调用、300 ms 周期及换主/关闭生命周期。
- Go 对照：`pkg/ddl/job_scheduler.go:120-359` 与 `:397-722`，核对 `jobScheduler` 字段、`scheduleLoop`、`loadAndDeliverJobs`、`mustReloadSchemas`、异步 `deliveryJob`、同步恢复及 worker-pool 背压。
- 独立 Rust 测试：`pkg/ddl/job_scheduler_test.rs` 验证 schema reload 成功/重试/取消和 tracker 增删；`pkg/ddl/job_scheduler_testkit_test.rs` 验证 owner 门禁、冲突组不交错、状态推进和无 involving 信息的 view job；持久路径的直接调用证据来自 `pkg/session/runtime/durable_scheduler_test.rs` 与 `pkg/session/runtime/normal_ddl*_test.rs`。
- RustCodeGraph 调用证据：`on_become_owner` 调用 `must_reload_schemas`；Go `scheduleLoop` 调用 `schedule`、`loadAndDeliverJobs` 调用 `deliveryJob`；`schedule_persisted` 的生产调用者包含 `normal_ddl_service.rs`，并由大量 normal DDL/durable scheduler 测试覆盖。对图中未精确解析的 Rust trait 动态调用，使用目标源码和直接调用点核验，没有据此推断静态边。
- 本任务为纯文档分析，按任务约束不运行 Cargo；最终只执行固定十一章节的结构验证，并人工检查所有“已接入/未接入”结论均有上述源码、调用边、Cargo、Go 或测试证据。
