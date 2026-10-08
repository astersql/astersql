# `pkg/session/runtime/ttl_runtime.rs`

## 文件定位

`ttl_runtime.rs` 位于 `astersql-session` crate 的 `runtime` 模块中，由 `pkg/session/runtime.rs` 以 `pub mod ttl_runtime` 暴露。它是 Domain 完成 SQL bootstrap 之后的 TTL 作业运行时编排层：把 infoschema 中的 TTL 表定义、持久化 timer、etcd 通知、owner 选举、SQL 系统表状态，以及 `astersql-ttl-ttlworker` 提供的扫描/删除原语串成可停止、可接管的一次作业执行链。实际启动点是 `pkg/session/runtime/session.rs` 对 `start_domain_ttl_job_manager_with_transport` 的调用；无 etcd 的公开入口是 `start_domain_ttl_job_manager`。

该文件不是 TTL SQL 构造或元数据模型的唯一实现位置。表发现与分片规划来自同目录 `ttl_metadata.rs`，timer hook 位于 `ttl_timer.rs`，SQL session 适配位于 `ttl_worker_session.rs`；扫描、删除、重试和持久作业操作分别由 `astersql-ttl-ttlworker::{scan,del,persistent}` 提供。本文件负责这些组件的生命周期和正确调用顺序。

## 核心职责

- `start_domain_ttl_job_manager*` 在 Domain 上安装后台循环，按外部工作负载角色决定是否参加 TTL owner 选举，定期同步 TTL timer，并响应 etcd 的命令/扫描通知。
- `trigger_ttl_command` 把 `trigger_ttl_job` 命令映射为 timer 手动事件，等待 `ManualProcessed`，再以 `mysql.tidb_ttl_job_history` 确认作业完成并返回逐物理表结果。
- `run_ttl_tick` 执行一次常规调度；`run_ttl_event` 执行 timer 指定的单表事件；两者汇入 `run_ttl_tick_inner`。
- `run_ttl_tick_inner` 领取新作业或接管心跳超时作业，恢复持久化扫描范围、游标和统计，逐范围扫描并删除过期行，最后持久化汇总。
- `JobHeartbeat` 用独立 SQL session 维持作业所有权；`checkpoint_cursor` 在悲观事务和 `NOWAIT` 锁保护下保存游标及累计统计。
- `TtlWatchRuntime`、`EtcdTtlWatchTransport`、`TtlElection` 和若干 RAII 容器保证 watcher、命令线程、timer runtime、session pool 与 owner lease 随 Domain 循环一起退出。

## 主要符号

- `TtlWatchKind::{Command, Scan}` 与 `TtlWatchEvent`：区分命令前缀监听和扫描唤醒；命令事件携带 `request_id`、库名和表名。
- `TtlWatchTransport`：运行时可替换的监听边界。`watch` 创建一次订阅，`take_command` 原子领取请求，`response_command` 写响应；默认的 `ttl_owner` 与 `timer_notifier` 可为空，便于无 etcd 环境和测试替身。
- `TtlWatchRuntime`：为两类 watcher 各建一个线程，通过 `mpsc` 汇聚事件并 `unpark` manager 线程。`stop`/`Drop` 设置原子停止标记并 join 线程。
- `EtcdTtlWatchTransport`：生产 etcd 实现。命令监听 `/tidb/ttl/cmd/req/` 前缀，扫描监听 `/tidb/ttl/notification/scan`；领取命令用“键存在则删除”的事务，响应写入带 180 秒 lease 的 `/tidb/ttl/cmd/resp/<request_id>`。
- `TtlElection`：持有 Tokio runtime 与 `astersql_owner::Manager`。`start` 安装 `TtlOwnerListener` 并发起 campaign，`Drop` 调用 `Close` 释放竞选资源。
- `DomainTtlTimerRuntime`：组合 `TimerGroupRuntime`、`TimerStore` 和 `AdvancedSessionPool`；析构顺序为停止 runtime、关闭 store、关闭 pool。
- `JobHeartbeat`：独立线程周期调用 `PersistentJobStore::heartbeat`；返回 `false` 或报错即把 `lost` 置位并退出。
- `TtlTickResult { tables, claimed, resumed, finished }`：一次 tick 的可观察计数；`claimed` 是新领取，`resumed` 是超时接管，`finished` 是完成落库。
- `PersistedScanRange`、`PersistedTaskState`：分别承载系统表中的扫描边界/索引 ID，以及 JSON 状态中的游标和累计计数。
- `ConfiguredDeleteRateLimiter`：从 `TTLDeleteRateLimit` 动态读取速率，按批行数计算等待时间，并以 100 ms 小步检查取消。
- `NEXT_JOB_ID`：进程内原子序列，仅用于常规 tick 生成包含 owner、物理表、时间和序号的新 job ID；timer 事件直接使用 event ID。

## 执行流程

1. bootstrap 完成后，`runtime/session.rs` 构造可选 `EtcdTtlWatchTransport`，调用 `start_domain_ttl_job_manager_with_transport`。函数先根据 `Domain::should_start_ttl_job_manager` 决定是否启动，再生成 owner ID；TTL task worker 角色可通过 transport 建立 `TtlElection`。
2. Domain 后台循环第一次运行时启动 `TtlWatchRuntime`。每轮排空通知、读取当前 Unix 秒，并在失去 owner 时销毁 timer runtime、回收已结束命令线程后提前返回。
3. owner 轮次调用 `collect_ttl_schedules` 和 `sync_ttl_timers`。timer runtime 尚未存在时，创建容量为 8 的 TTL timer session pool、表存储以及仅匹配 TTL key 前缀/TTL hook class 的 `TimerGroupRuntime`，注册 `SqlTtlTimerHook` 后启动。
4. 命令通知先经 `take_command` 竞争性删除请求键，成功者在独立 `ttl-command-response` 线程中运行 `trigger_ttl_command`。后者检查全局开关和调度窗口，按大小写不敏感的库表名选择所有物理表 timer，逐个 `ManualTriggerEvent`，最长等待 300 秒；只有历史表出现 job ID 才视为完成。扫描通知只负责立即唤醒本轮 timer/元数据同步。
5. `SqlTtlTimerHook`（`ttl_timer.rs`）收到 timer 事件后调用 `run_ttl_event`；显式周期入口调用 `run_ttl_tick`。`run_ttl_tick_inner` 先收集 schedules；事件模式只保留匹配 `table_id + physical_id` 的表，常规模式先同步 timers。全局 TTL 开关关闭或当前时间不在窗口时，不领取作业。
6. 对每个 schedule，先用 `takeover_timeout_for_job(..., 240, event_id)` 尝试接管超过 240 秒未更新心跳的作业。接管时从 `scan_id=0` 的任务恢复原始 `expire_time`；否则计算新的过期水位，调用 `split_ttl_scan_ranges`，再以 `start_job_with_ranges` 原子领取并持久化所有范围及可选扫描索引。
7. 读取作业创建时间并交给 `Domain::track_ttl_job`，再加载全部持久扫描范围。每个范围根据持久索引 ID 重新解析索引列，恢复游标和累计统计，构造批大小为 128 的 `TtlScanTask`。
8. 每个范围分别建立 heartbeat、scan、delete、checkpoint SQL session。scan/delete session 先应用 TTL 专用 session 设置；scan session 计算带全局时区的过期谓词，delete session 复用该谓词。
9. `execute_with_checkpoint` 扫描每批候选行。删除前同步刷新所有权心跳，构造 `DeleteTask`，经限速器执行删除并记录失败项；存在待重试删除时，本批不允许写 checkpoint。成功批次由 `checkpoint_cursor` 原子校验当前 owner/任务仍存在并写入 JSON 游标与统计。
10. 扫描结束后按 retry buffer 的退避间隔重试；取消、失去 owner 或离开调度窗口时清空重试。只有 heartbeat 未丢失且终止原因是 `Finished` 才恢复 session 设置并累计统计，否则返回错误并保留持久作业供之后接管。
11. 所有范围完成后构造 `TtlSummary` 和包含任务数的 JSON 摘要，调用 `PersistentJobStore::finish_job`，再 `Domain::complete_ttl_job`。整轮结束时回收 Domain 中已完成作业并返回 `TtlTickResult`。

## 数据与状态

- 配置状态来自 `EnableTTLJob`、`TTLJobScheduleWindowStartTime`、`TTLJobScheduleWindowEndTime` 和 `TTLDeleteRateLimit`。`within_ttl_window` 使用带数值时区的 `HH:MM %z` 解析，并委托 `WithinDayTimePeriod` 处理跨午夜窗口。
- `mysql.tidb_timers` 是 schedule 与 timer event 的持久边界；manager 同步 timer，而 `SqlTtlTimerHook` 负责事件提交、执行结果和关闭事件，普通 `run_ttl_tick` 不越权关闭 timer event。
- `mysql.tidb_ttl_table_status` 保存当前 job ID、owner、心跳、创建时间及最后完成摘要；所有权变更是停止当前扫描的核心信号。
- `mysql.tidb_ttl_task` 保存每个 `scan_id` 的二进制范围、可选索引 ID、`expire_time` 和 JSON `state`。状态字段记录文本游标及 `total_rows/success_rows/error_rows`；空串、`null` 和 `<nil>` 兼容为空状态。
- `mysql.tidb_ttl_job_history` 是手动命令确认完成的依据。timer 标记已处理但 history 尚未出现时，命令线程继续轮询。
- 内存状态包括 watcher 的停止原子量、事件通道、命令线程列表、timer runtime 可选值、heartbeat 的 condvar/lost 标志，以及 Domain 的在途作业跟踪。持久作业是恢复真相源，进程内计数仅用于当前 tick 结果和 ID 去重。

## 依赖与调用关系

- 上游启动：`pkg/session/runtime/session.rs::start` 在 bootstrap、server-info/owner 初始化后调用 `start_domain_ttl_job_manager_with_transport`；公开 `start_domain_ttl_job_manager` 供无 transport 场景和测试使用。
- 上游事件：`pkg/session/runtime/ttl_timer.rs::SqlTtlTimerHook::OnEvent` 调用 `run_ttl_event`，并在成功/失败后用 timer client 关闭事件。`normal_ddl_create_table_test.rs` 与 `ttl_runtime_test.rs` 也直接调用 `run_ttl_tick`/`run_ttl_event` 验证系统行为。
- 下游元数据：`collect_ttl_schedules` 与 `split_ttl_scan_ranges` 来自 `ttl_metadata.rs`，Domain 提供 infoschema、统计表模型、后台循环和作业跟踪。
- 下游 timer：`astersql-timer-api`、`astersql-timer-runtime`、`astersql-timer-tablestore` 负责 timer client/store/runtime；`ttl_timer_store.rs` 提供 SQL session pool，`ttl_timer.rs` 提供 hook。
- 下游执行：`astersql-ttl-ttlworker` 提供 `PersistentJobStore`、`TtlScanTask`、`DeleteTask`、`DeleteRetryBuffer`、`TtlStatistics` 与 session 准备/恢复函数；`ttl_worker_session.rs::TtlWorkerSqlSession` 把 `ConcreteSession` 适配给这些接口。
- 外部协调：`astersql-owner` 管理 TTL owner lease；`etcd-client` 提供 watch、事务领取、lease 响应；Tokio runtime 只包围这些异步 API，主扫描删除链仍运行在线程和同步 SQL session 上。
- `pkg/session/Cargo.toml` 将本文件归入 `astersql-session`，并直接声明上述 Domain、owner、timer、TTL worker/cache、session sys pool、Tokio、etcd、serde/serde_json 等依赖；TTL 路径不受唯一的 `nextgen` feature 条件编译。

## 错误处理与边界

- 对外主入口统一返回 `Result<_, String>`，在跨 crate/session 边界处附加“同步 timer、领取/接管作业、读取范围、恢复设置、完成作业”等上下文；扫描/删除闭包内部保留 `SessionError` 以适配 worker trait。
- 非法调度窗口、超出时间范围、缺少 schedule、timer 手动请求被替换/取消、300 秒超时、系统表缺行、日期/JSON/范围 datum 解码失败、索引消失或列 offset 非法都会显式失败，不用默认值伪造成功。
- `checkpoint_cursor` 只接受文本 cursor datum，并在 `BEGIN PESSIMISTIC` 中用两个 `FOR UPDATE NOWAIT` 分别确认 table-status 所有权和 task 存在；任一步失败都会 `ROLLBACK`。因此并发 owner 不会覆盖新 owner 的 checkpoint。
- 新作业领取返回 `false` 表示别的 manager 已持有，当前表直接跳过；锁竞争可能将 MySQL `NOWAIT` 错误向上传播。测试允许并发竞争者出现错误，但要求总计只有一个 claimant。
- timer 同步错误只记录日志，manager 循环继续；单个命令领取/响应错误也不终止主循环。相反，扫描主链的状态损坏、所有权丢失、取消或不完整终止会让本次调用失败，并故意不调用 `finish_job`，保留作业供恢复。
- `trigger_ttl_command` 可返回部分成功的 `table_result`；仅当所有物理表都没有 `job_id` 时才把首个错误提升为整个命令失败。

## 并发与资源生命周期

- Domain 拥有 manager 停止标记和 join 行为；重复启动由 `Domain::start_ttl_job_manager`/`should_start_ttl_job_manager` 边界拒绝。闭包只持有 Domain 的 `Weak`，避免后台循环反向延长 Domain 生命周期。
- 每类 etcd watch 有外层重连线程，实际 etcd stream 又运行在独立 current-thread Tokio runtime 中。断连后外层循环等待 100 ms 再订阅，停止标记使用 Acquire/Release；事件到达时 `unpark` manager 以缩短响应延迟。
- `EtcdTtlWatchTransport` 记录内部 stream 线程并清理已结束线程；其 `Drop` join 剩余线程。`TtlWatchRuntime`、`TtlCommandWorkers`、`TtlElection`、`DomainTtlTimerRuntime` 和 `JobHeartbeat` 都以 `Drop` 提供兜底收尾。
- 命令执行独立于 manager 主循环，避免最长 300 秒等待阻塞 timer 同步；每轮 `reap` join 已结束线程。停止时会设置 watcher 标志，命令线程观察同一标志并退出轮询。
- heartbeat 使用独立 `ConcreteSession`，避免扫描 SQL 或删除限速阻塞 lease 刷新。`Condvar` 允许 stop 立即唤醒，不必等待完整 heartbeat interval。
- 扫描、删除和 checkpoint 使用三个独立 SQL session；coordinator session 负责领取、同步 heartbeat 和完成作业。此隔离是避免长扫描、删除事务和 checkpoint 锁相互占用 session 状态的关键约束。
- 删除重试必须在 checkpoint 前清空：否则游标前移会使失败删除在接管后永久跳过。离开调度窗口或取消时清空内存 retry，但持久 cursor 停留在上一成功批次，后续可安全重扫。

## 与 Go 版本的对应关系

- Go 启动链为 `pkg/session/session.go` 调用 `Domain.StartTTLJobManager`，后者在 `pkg/domain/domain.go` 根据角色创建并启动 `ttlworker.JobManager`；Rust 将 Domain 生命周期接线放到 `runtime/session.rs`，核心循环集中在本文件。
- Go 的主要对照实现是 `pkg/ttl/ttlworker/job_manager.go`：`NewJobManager`/`jobLoopWithSession` 对应 Rust 的启动函数与 Domain loop；`onTimerTick`、`triggerTTLJob`、`rescheduleJobs`、`lockHBTimeoutJob`、`lockNewJob`、`updateHeartBeatForJob` 和 `checkFinishedJob` 分别对应 timer 同步/命令触发、领取或接管、心跳和完成路径。
- 两版都以 owner/leader、系统表和 timer 为协调基础，都要求全局开关及调度窗口允许后才创建作业，并保留超时作业接管语义。Rust 的 `run_ttl_tick_inner` 把 Go manager、task manager 和 job 的关键调用顺序压缩为同步的一次执行流程，但复用拆分后的 Rust TTL worker crate，而不是逐类型镜像 Go 对象图。
- Rust 特有的显式 RAII 容器负责线程/runtime/pool 析构；Go 版主要通过 context、WaitGroup、`Stop`/`WaitStopped` 管理。Rust watcher transport trait 也把真实 etcd 与测试 transport 分离。
- `ttl_runtime_test.rs` 测试名带 `go_merge_43`，验证的是移植语义：watch 重连、timer hook、双 manager 单次领取与接管、真实 etcd、跨午夜窗口、心跳失主、持久范围、Domain 停止、过期行删除、取消保留作业、原水位恢复、游标恢复和多范围恢复。Go 侧的主要回归面是 `pkg/ttl/ttlworker/job_manager_test.go` 与 `job_manager_integration_test.go`。

## 扩展指南

- 新增 manager 事件或 transport 时，优先扩展 `TtlWatchKind`/`TtlWatchEvent`/`TtlWatchTransport`，并保持“订阅断开后重建、领取命令必须原子、manager 可被 unpark、stop 后线程可 join”的契约；同步更新独立的 `ttl_runtime_test.rs`，不要把测试嵌入生产文件。
- 改变调度条件应集中在 `scheduling_enabled`/`within_ttl_window`，同时覆盖普通窗口、跨午夜、非法格式、执行中离开窗口和全局开关关闭。时区语义须与 Go `JobManager` 及系统变量保持一致。
- 改变领取/恢复协议时，需成对审查 `PersistentJobStore::{start_job_with_ranges,takeover_timeout_for_job,heartbeat,finish_job}`、`persisted_expire_time`、`persisted_scan_ranges`、`persisted_task_state` 和 `checkpoint_cursor`；兼容已有系统表行与旧 JSON 状态，不能用新计算的过期水位覆盖被接管作业的原水位。
- 新增 cursor datum 类型时，应同时扩展 task range 解码与 checkpoint 编码，并添加恢复测试；当前 checkpoint 仅支持文本 datum，而扫描范围解码支持 Null、Int、UInt、Bytes、String，这一不对称不能静默放宽。
- 调整扫描/删除批处理时，必须维持“删除及重试成功后才能 checkpoint”的不变量，评估 `batch_size=128`、`TTLDeleteRateLimit`、心跳间隔 10 秒和超时接管 240 秒之间的性能与所有权风险。
- 修改 timer 生命周期时，同时检查 `ttl_timer.rs`：只有 hook 应关闭并汇总 timer event。普通 tick、手动命令等待和 timer runtime 停止的职责不可混合。
- 增加线程或异步任务时，应放入现有 RAII 所有者或新增可 join 的所有者；禁止创建无法由 Domain close 停止的 detached worker。涉及真实 etcd 的行为应保留可替换 transport 测试，并在环境允许时补真实 etcd 场景。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`pkg/session/runtime/ttl_runtime.rs` 已索引为 1,434 行、112 个符号。`query/node` 确认 `start_domain_ttl_job_manager` 调用 transport 版本，`run_ttl_tick` 与 `run_ttl_event` 均汇入 `run_ttl_tick_inner`；图还确认 `run_ttl_tick` 的直接测试调用者。
- 生产源码：`pkg/session/runtime/ttl_runtime.rs`（本文全部符号与流程）、`pkg/session/runtime/session.rs`（bootstrap 后启动与 etcd transport 构造）、`pkg/session/runtime/ttl_timer.rs`（timer hook 调用 `run_ttl_event`）、`pkg/session/runtime.rs`（模块声明）。
- crate 边界：`pkg/session/Cargo.toml` 的 package 名为 `astersql-session`，porting 元数据指向 `pkg/session`；直接依赖包含 Domain、owner、timer API/runtime/tablestore、TTL worker/cache、session syssession、Tokio 与 etcd client。
- Rust 独立测试：`pkg/session/runtime/ttl_runtime_test.rs`；相邻集成式覆盖还在 `pkg/session/runtime/normal_ddl_create_table_test.rs`。测试分别覆盖 watcher、timer、owner 竞争、心跳、范围持久化、停止、删除、取消和接管恢复。
- Go 对照：`pkg/session/session.go::BootstrapSession`、`pkg/domain/domain.go::{StartTTLJobManager,shouldStartTTLJobManager,TTLJobManager}`、`pkg/ttl/ttlworker/job_manager.go`，相关测试为 `pkg/ttl/ttlworker/job_manager_test.go` 和 `job_manager_integration_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有十一个固定二级标题，并人工复核未修改 Rust、Go、Cargo 或只读 `plan.md`。
