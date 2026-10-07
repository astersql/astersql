# `pkg/ddl/normal_policy.rs`

## 文件定位

`normal_policy.rs` 属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml` 与 `pkg/ddl/lib.rs` 的 `pub mod normal_policy`），实现普通 DDL owner 在持久化 SQL 作业队列上的“升级期准入策略”。它不是一个 DDL action handler，也不负责 schema state 迁移或回填；它在 `JobScheduler::schedule_persisted` 从 `mysql.tidb_ddl_job` 读出作业后、`JobWorker::transit_persisted_job_step` 真正推进作业前，决定当前作业是否可运行，并按需把作业持久化为系统暂停或恢复状态。

上游装配有两处：`pkg/session/runtime/normal_ddl_service.rs::NormalDdlService::with_upgrade_policy` 为实际服务安装带轮次缓存的策略；`pkg/session/runtime/session_factory.rs::install_serving_ddl_runtime` 先安装可独立工作的初始策略，随后由服务包装升级状态。该文件属于普通、owner 驱动、可恢复的 DDL job 主链，而不是 metadata-only 快路径。

## 核心职责

- `NormalDdlJobPolicy::runnable` 根据集群是否处于升级状态、作业自身状态、涉及的数据库以及暂停原因决定准入。
- 升级开始时，把可暂停的普通用户 DDL 标为 `JobState::Pausing`，并把 `admin_operator` 标为 `AdminCommandOperator::System`；已经暂停的作业不运行，正在暂停的作业继续交给 worker 收敛到 `Paused`。
- 系统库相关作业在升级期仍可运行，判断依据是 `Job::get_involving_schema_info` 中任一数据库满足 `astersql_meta_metadef::IsSystemRelatedDB`。
- 不能暂停的作业在升级期继续运行；其他暂停失败则记录错误并暂不准入。
- 升级结束后，只自动恢复由系统暂停且不是 TiKV 磁盘满保护导致的作业。磁盘满暂停必须保留，避免升级恢复逻辑越过资源保护边界。
- 向 `NormalDdlExecutor` 提供动态 DDL 错误次数上限和可选 MDL owner ID，分别来自 `GetDDLErrorCountLimit` 与 `IsMDLEnabled`。

## 主要符号

- `pub struct NormalDdlJobPolicy`：策略状态容器。
  - `state: Arc<dyn astersql_ddl_serverstate::Syncer>`：集群全局状态读取器；无轮次快照时由 `runnable` 调用 `is_upgrading_state`。
  - `context: SyncContext`：与 syncer 一起由服务装配并供外部刷新全局状态；本文件自身不直接读取该字段。
  - `owner_id: String`：MDL 启用时由 `mdl_owner` 返回，写入该 owner 对作业的 MDL 归属。
  - `round_upgrading: Option<Arc<AtomicBool>>`：一次队列 dispatch round 固定使用的升级状态快照；服务路径提供它，测试和初始装配路径可为 `None`。
- `impl DdlJobPolicy for NormalDdlJobPolicy`：公开行为边界。
  - `runnable(&mut self, session, job) -> Result<bool, String>`：准入及系统暂停/恢复入口。
  - `error_limit(&self) -> i64`：每次读取全局 DDL error-count limit，不缓存配置。
  - `mdl_owner(&self) -> Option<String>`：仅在 MDL 开启时复制返回 `owner_id`。
- `system_command(session, id, resume)`：私有持久化管理命令；最多三次执行“begin → 重读 job_meta → 修改/编码 → update → commit”的完整事务。
- `pause_system_job(job)`：私有纯状态转换。拒绝已经 pausing/paused 或不可暂停的作业；成功时写入 `Pausing + System`。
- `resume_system_job(job)`：私有纯状态转换。只接受 resumable 且确由系统暂停的作业；成功时写入 `Queueing`，并清除 pause reason、error 和 resume reason。

文件没有模块级常量、enum、条件编译项或内嵌测试。

## 执行流程

1. `JobScheduler::schedule_persisted` 按 job ID 读取持久化队列，解码 `Job`，然后调用 `DurableJobExecutor::runnable`；`NormalDdlExecutor::runnable` 原样委派给本策略。
2. `runnable` 优先确定本轮升级状态：有 `round_upgrading` 时以 `Ordering::Acquire` 读取快照，否则查询 `state.is_upgrading_state()`。
3. 升级期分支：
   - `Paused` 直接返回 `Ok(false)`；
   - `Pausing` 或涉及系统数据库返回 `Ok(true)`；
   - 其他作业调用 `system_command(..., resume = false)`。暂停成功后返回 `Ok(false)`；错误码前缀为 `[ddl:8260]`（不可暂停）时返回 `Ok(true)`；其余错误打印到 stderr 后返回 `Ok(false)`。
4. 非升级期分支：
   - 系统暂停且原因是 `JOB_PAUSE_REASON_KV_DISK_FULL` 时返回 `Ok(false)`；
   - 其他系统暂停作业调用 `system_command(..., resume = true)`，成功持久化恢复状态后故意返回错误 `system paused job:... need to be resumed`，使本次调度轮次停止在恢复边界，不在同一轮继续执行刚恢复的作业；
   - 其余作业以 `!job.is_paused()` 决定准入，因而保留最终用户暂停。
5. 准入成功后，scheduler 才检查 schema 冲突并调用 `JobWorker::transit_persisted_job_step`；策略本身不执行 action、不发布 schema version，也不等待 follower 同步。

## 数据与状态

持久状态是 `mysql.tidb_ddl_job.job_meta` 中 Go wire-compatible `Job` 的编码字节。`system_command` 使用 `astersql_meta::decode_go_history_job` 解码，再由 `encode_go_ddl_job(..., false)` 编码为十六进制 SQL literal 更新原行。重要状态转换为：

- 升级暂停：可暂停状态 → `Pausing`，`admin_operator = System`。
- 升级恢复：系统暂停且 resumable → `Queueing`，清空 `pause_reason`、`error`、`resume_reason`。
- 保留状态：最终用户暂停、磁盘满系统暂停、已经暂停的升级期作业均不被覆盖。

`round_upgrading` 是内存中的共享只读轮次快照，不是持久状态。它避免同一轮扫描期间因后端状态刷新而对不同作业采用不同准入判断。`context`、`state` 和 `owner_id` 的生命周期由 `NormalDdlService`/serving Domain 持有的 executor factory 闭包间接延长。

## 依赖与调用关系

上游调用链：

`install_serving_ddl_runtime` / `NormalDdlService::with_upgrade_policy` → 构造 `NormalDdlExecutor<Barrier, NormalDdlJobPolicy>` → `JobScheduler::schedule_persisted` → `NormalDdlExecutor::runnable` → `NormalDdlJobPolicy::runnable`。

下游直接依赖：

- `DurableJobSession::{begin, query, commit, rollback}`：对 durable SQL queue 执行管理事务。
- `astersql-meta`：Go-compatible DDL job 解码与编码。
- `astersql-meta-model::group_3::{Job, JobState, AdminCommandOperator, JOB_PAUSE_REASON_KV_DISK_FULL}`：状态机与原因标记。
- `astersql-ddl-serverstate::Syncer`：升级状态来源。
- `astersql-meta-metadef::IsSystemRelatedDB`：系统数据库豁免。
- `astersql-sessionctx-vardef`：动态错误上限与 MDL 开关。

`pkg/ddl/Cargo.toml` 将上述组件声明为本地 workspace 依赖；本模块没有自己的 feature gate。crate 根在 `pkg/ddl/lib.rs` 公开导出该模块，因此 session runtime 可通过 `astersql_ddl::normal_policy` 使用它。

## 错误处理与边界

- `system_command` 最多重试三次 commit failure；每次都重新开始事务并重读 job row，因此不会拿第一次读到的旧对象盲目覆盖并发管理命令。`pkg/session/runtime/normal_ddl_test.rs::crossks_align_normal_ddl_policy_commit_conflict_preserves_concurrent_user_pause` 验证提交冲突后能保留并发用户暂停。
- begin、query 或 decode/encode 的基础设施错误以 `Err(String)` 传播；闭包内错误会 rollback。commit 失败也 rollback，三次均失败时返回最后一次错误。
- job row 不存在时生成 `[ddl:8224]`；已暂停生成 `[ddl:8262]`；不可暂停生成 `[ddl:8260]`；不可恢复生成 `[ddl:8261]`。升级准入只把 `[ddl:8260]` 识别为“允许继续运行”，其余暂停命令错误会阻止本轮运行。
- 管理命令自身的业务错误仍会提交当前只读事务后返回，保持与 Go `processJobs` 的事务边界一致。
- SQL 由内部 i64 job ID 和本地编码字节拼接，不接受用户字符串；仍依赖 `DurableJobSession` 对事务隔离、冲突检测和 rollback 的正确实现。
- `commit_error.expect` 只有在固定三轮 commit 均失败后执行，此时 `commit_error` 必然已被赋值；它表达内部循环不变量。
- 日志目前使用 `eprintln!`，只报告暂停失败；恢复错误直接传播。若需要结构化观测，应保持错误码与 Go 兼容，不能通过吞错改变准入语义。

## 并发与资源生命周期

策略可跨 executor factory 实例共享 `Syncer` 与 `AtomicBool`。轮次缓存使用 writer 的 `Release`（服务/测试侧）与这里的 `Acquire` 配对读取；一旦一轮开始，所有 job 应观察同一升级状态。策略方法接收 `&mut self` 和 `&mut dyn DurableJobSession`，单次调用内串行执行事务，本文件不创建线程、任务或 channel。

durable job 的并发安全来自事务内重读和 commit 冲突重试。真正推进 action 时，`JobWorker::transit_persisted_job_step` 还会比较期望的原始 job bytes、检查 owner lease，并在同一 action 事务内更新 job；本策略的管理事务在它之前完成，两者不嵌套。owner、schema barrier、session pool 和取消信号的生命周期由 `NormalDdlService` 管理，本文件只借用 session，并通过 `Arc` 持有集群状态对象。

## 与 Go 版本的对应关系

主要对照是 `pkg/ddl/job_scheduler.go::processJobDuringUpgrade`：升级期的 Paused/Pausing/系统库分支、调用 `PauseJobsBySystem`、不可暂停错误继续运行，以及正常期跳过磁盘满暂停、调用 `ResumeJobsBySystem` 后返回“need to be resumed”，均在 `runnable` 中保留。

持久化事务对应 `pkg/ddl/ddl.go::processJobs`：两者都最多尝试三次，事务内重读 job、应用命令、更新 `mysql.tidb_ddl_job` 并在 commit 冲突后重试。Rust 这里只处理单个 job ID，而 Go helper 支持一批 ID 并返回逐项错误。

状态转换对应 `pkg/ddl/util/util.go::PauseRunningJob` 和 `pkg/ddl/ddl.go::resumePausedJob`/`resumePausedJobForUpgradeFinish`。Rust 恢复逻辑只允许 `AdminCommandOperator::System`，并保留磁盘满暂停；它清除 resume reason，符合“升级完成自动恢复”路径，而不是 Go 允许最终用户显式恢复磁盘满作业并设置 `JobResumeReasonKVDiskFull` 的另一条路径。

实现差异是 Rust 通过 `round_upgrading` 固定一次队列 dispatch 的升级状态，而 Go 展示的 `processJobDuringUpgrade` 每次调用 `IsUpgradingState()`；Rust 无快照的初始/测试路径仍会直接查询 syncer。文档没有假定两种读取方式在所有时序下完全等价。

## 扩展指南

- 新增升级准入例外时，修改 `NormalDdlJobPolicy::runnable`，并明确它应位于 Paused/Pausing 判断之前还是之后；涉及对象范围的例外应继续从 `Job::get_involving_schema_info` 取得，不应绕过 durable job 元数据。
- 新增系统暂停原因时，先决定升级结束能否自动恢复；资源保护类原因应仿照 `JOB_PAUSE_REASON_KV_DISK_FULL` 保留，并同步 Go 的 `resumePausedJobForUpgradeFinish` 语义。
- 修改暂停/恢复状态字段时，应同步 `pause_system_job`/`resume_system_job`、Go helper、wire 编解码兼容性以及独立测试；不能只修改 scheduler 返回值。
- 修改事务或重试逻辑时，必须保留“每次重试重新读 job row”的不变量，避免覆盖并发用户 pause/cancel；相应测试应继续放在独立文件 `pkg/session/runtime/normal_ddl_test.rs`，不要把测试嵌入本生产文件。
- 修改轮次缓存时，应同时检查 `NormalDdlService::with_upgrade_policy` 中快照的刷新时机和 Acquire/Release 顺序，避免一次扫描内状态撕裂。
- 修改 `error_limit` 或 `mdl_owner` 时，应验证动态配置读取以及 `mysql.tidb_mdl_info` 的 owner 记录。兼容风险集中在 Go error code、job wire fields 和管理者身份；性能风险主要是每个被暂停/恢复作业独立 SQL 事务及失败时最多三次重读/提交。

## 验证依据

- RustCodeGraph 索引状态：11467 files、307296 nodes、1848419 edges；目标 `pkg/ddl/normal_policy.rs` 已索引为 164 行、17 个符号。
- RustCodeGraph 查询：`node --file pkg/ddl/normal_policy.rs`；`node`/`callers`/`callees` 针对 `NormalDdlJobPolicy`、`system_command`、`pause_system_job`、`resume_system_job`；构造边指向 `NormalDdlService::with_upgrade_policy` 与 `install_serving_ddl_runtime`。图未返回文件内私有 caller 边，已由同一索引中的目标源码确认 `runnable → system_command → pause_system_job/resume_system_job`。
- 主链源码：`pkg/ddl/table_mode.rs::DdlJobPolicy`、`NormalDdlExecutor`；`pkg/ddl/job_scheduler.rs::schedule_persisted`；`pkg/ddl/job_worker.rs::DurableJobExecutor`、`transit_persisted_job_step`；`pkg/session/runtime/normal_ddl_service.rs::with_upgrade_policy`；`pkg/session/runtime/session_factory.rs::install_serving_ddl_runtime`。
- crate/模块依据：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、`pkg/ddl/doc.go`。
- Go 对照：`pkg/ddl/job_scheduler.go::processJobDuringUpgrade`；`pkg/ddl/ddl.go::processJobs`、`resumePausedJob`、`resumePausedJobForUpgradeFinish`、`PauseJobsBySystem`、`ResumeJobsBySystem`；`pkg/ddl/util/util.go::PauseRunningJob`。
- 独立 Rust 测试：`pkg/session/runtime/normal_ddl_test.rs::crossks_align_normal_ddl_policy_upgrade_pauses_then_resumes_durable_job`、`crossks_align_normal_ddl_policy_retains_user_and_disk_full_pauses`、`crossks_align_normal_ddl_policy_commit_conflict_preserves_concurrent_user_pause`、`normal_ddl_plan_upgrade_owner_cached_state_and_unpausable_jobs`、`normal_ddl_plan_upgrade_owner_dynamic_limits_and_mdl`。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在且恰有十一个固定二级标题。
