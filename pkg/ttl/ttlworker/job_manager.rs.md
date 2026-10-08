# `pkg/ttl/ttlworker/job_manager.rs`

## 文件定位

本文件属于 `astersql-ttl-ttlworker` crate；该 crate 由 `pkg/ttl/ttlworker/Cargo.toml` 定义，入口 `lib.rs` 通过 `pub mod job_manager` 对外公开本模块。它提供 TTL（Time To Live）表级作业的内存协调模型，并实现 `timer.rs::TtlJobAdapter`，把定时器的“能否提交、提交、查询”协议接到作业状态上。

它不是当前 Rust 运行时中 Go `JobManager` 的完整替代。`pkg/session/runtime/ttl_runtime.rs` 的生产路径直接使用 `persistent.rs::PersistentJobStore` 完成系统表事务，只复用本模块的 `TtlSummary`；本文件的 `JobManager` 当前主要由 `job_manager_test.rs`、`job_manager_integration_test.rs` 和 `timer_test.rs` 驱动。系统表 SQL 的真实原子锁定和完成顺序应以 `persistent.rs` 为准，不能把本文件中的 `BTreeMap` 更新理解为跨节点持久化。

## 核心职责

- `JobManager` 维护物理表元数据、表级作业状态、本节点活跃作业/历史、扫描任务和定时器 request 到 job 的关联。
- `lock_new_job` 在 leader 门禁下保证同一物理表至多有一个内存活跃作业，计算过期水位并创建历史快照。
- `update_heartbeat` 与 `reschedule_timeout_jobs` 分别续约本地作业和接管心跳超时、但不在本地活跃集合中的作业。
- `finish_completed_jobs` 汇总已完成扫描任务，通过 `job.rs::TtlJob::finish` 原子地更新本地活跃集合、任务计数和历史摘要，再释放表状态。
- `gc` 仅允许 leader 按保留期清理历史，并删除已经不在表缓存中且没有运行作业的状态。
- `TtlJobAdapter` 实现负责定时器侧提交/查询，包含 TTL 启用、表身份和 index scan 版本一致性门禁。

## 主要符号

- `INSERT_NEW_TABLE_INTO_STATUS_SQL`：创建 `mysql.tidb_ttl_table_status` 行的 SQL 模板。本文件不执行它；`persistent.rs::start_job_in_transaction` 在锁行失败且需建行时使用。
- `TtlSummary { total_rows, success_rows, error_rows, scan_task_err }`：Rust 作业摘要。它被 `job.rs` 历史记录、`timer.rs` 定时器响应、`persistent.rs::finish_job` 和 session runtime 共享。
- `summarize_task_results(&[ManagedTask], Option<&str>)`：累加每个任务的 `TaskState` 三类行数，并可附加一个调用方提供的扫描错误字符串。它不统计任务总数/调度数/完成数，也不合并任务自身的多条错误。
- `TableStatus`：`tidb_ttl_table_status` 当前作业字段的内存投影，以 Unix 秒保存心跳、开始时间和过期水位。
- `JobManager`：核心协调结构。公开字段便于相邻模块和测试组装状态；`request_to_job`、`ttl_indexes`、`job_version_checker` 保持内部封装。
- `JobManager::new`：建立空缓存和空仓，构造具有至少一个执行槽位约束的 `TaskManager`；默认不是 leader，index scan 默认开启，server version 默认不可用。
- `set_ttl_index` / `refresh_tables`：分别增删某物理表的候选 TTL 索引、整体替换表元数据快照。
- `lock_new_job`、`update_heartbeat`、`reschedule_timeout_jobs`、`finish_completed_jobs`、`gc`：内存作业生命周期的五个主要操作。
- `TtlJobAdapter::{can_submit_job, submit_job, get_job, now}`：供 `timer.rs::TtlTimerHook` 调用的适配接口实现。
- `initial_managed_task`：把 `TtlScanTask` 包装成无 owner、零心跳、零计数的 `Waiting` 任务；独立的 task manager 测试也复用它。

## 执行流程

1. 调用方先通过 `refresh_tables` 装入 `PhysicalTable`，并设置 `is_leader`；如果有可用 TTL 索引，再调用 `set_ttl_index`。
2. `timer.rs::TtlTimerHook::on_event` 先调用 `can_submit_job`。只有当前节点是 leader、physical ID 对应正确的逻辑 table ID、TTL 已启用且状态中没有 current job 时才继续。
3. `submit_job` 生成 `{physical_id}-{now}-{request_id}` 形式的 job ID。若 index scan 开启且表有候选索引，它调用 `JobVersionChecker::check`：一致时记录索引，允许安全回退时走主键扫描，明确版本不一致时返回错误并让定时器以后重试。
4. `lock_new_job` 再次执行 leader、表存在、当前 job 为空及可选调度间隔检查；用 `PhysicalTable::expire_time(now)` 计算水位，更新 `TableStatus`，写入 `JobStore::active_jobs`，并由 `TtlJob::create_history` 建立未完成历史。
5. 提交成功后，`submit_job` 保存 job 的 scan 选择及 `request_id -> (physical_id, job_id)` 映射，返回未完成的 `TtlJobTrace`。扫描任务应由相邻调度逻辑生成，并以 `initial_managed_task` 进入 `TaskManager`。
6. 运行期间 `update_heartbeat` 仅更新“owner 是本节点且 physical ID 确实在本地 active_jobs 中”的状态；`reschedule_timeout_jobs` 只接管不在本地 active_jobs、仍有 current job 且心跳严格超时的状态。
7. `finish_completed_jobs` 对每个本地活跃 job 收集 `TaskManager::finished()` 中同 job ID 的任务。已登记任务数非零且完成数不足时跳过；否则汇总并调用 `TtlJob::finish`。成功后清空状态的 current job/owner，历史保留完成时间和摘要。
8. `TtlTimerHook::poll` 经 `get_job` 查询：physical/table 身份不匹配时报错；对应物理表仍活跃则返回运行中，否则从历史取摘要并返回完成。
9. leader 可调用 `gc`：历史仅保留 `now - create_time <= retention_seconds` 的记录；已从表快照消失的状态，只有 current job 也为空时才删除。

## 数据与状态

`tables` 和 `statuses` 都按 physical ID 索引，前者描述当前元数据，后者描述当前作业所有权。分区表因此可以逐分区独立调度，而 `TableStatus::parent_table_id` 和 `PhysicalTable::table_id` 保留逻辑表身份；`job_manager_integration_test.rs::submission_enforces_leader_enabled_and_table_identity` 验证了 table/physical ID 不可混用。

`JobStore`（见 `job.rs`）将状态拆成三个集合：`active_jobs` 以 physical ID 保证单表唯一活跃 job，`tasks_by_job` 记录预期扫描任务数，`history` 以 job ID 保存创建/完成快照。`finish_completed_jobs` 的不变量是：只有 `TtlJob::finish` 再次确认该 job 仍是对应 physical ID 的 active owner 后，才删除 active job 和任务计数并写历史。

`request_to_job` 支持定时器按 request ID 轮询；`ttl_indexes` 是表级候选索引，`job_scan_indexes` 是创建作业时冻结的选择，避免后续元数据变化改写既有 job 的扫描方式。当前 `gc` 不清理这三个映射，长生命周期实例若不断产生请求可能增长；扩展持久化/回收逻辑时需要显式处理其生命周期。

时间值均为调用方注入的 Unix 秒。差值使用 `saturating_sub`，因此时钟倒退不会产生无符号下溢；调度间隔使用 `<=`，只有严格超过 `expire_after_seconds` 才能再次锁定；超时接管也使用 `>`，等于阈值时仍不接管。GC 在恰好等于保留期时保留历史，集成测试 `leader_gc_honors_retention_boundary_and_running_status` 固化了这个边界。

## 依赖与调用关系

上游方面，`timer.rs::TtlTimerHook` 通过 `TtlJobAdapter` 调用 `can_submit_job`、`submit_job`、`get_job` 和 `now`；`lib.rs` 公开模块；`job_manager_test.rs`、`job_manager_integration_test.rs` 和 `timer_test.rs` 是当前 `JobManager` 的直接行为证据。RustCodeGraph 的文件关系还显示 `job.rs` 使用 `TtlSummary`，`persistent.rs` 使用摘要及插入状态 SQL，`pkg/session/runtime/ttl_runtime.rs` 使用摘要类型。

下游方面，本文件调用 `PhysicalTable::expire_time` 计算水位，调用 `JobVersionChecker::check` 决定 index scan 门禁，调用 `TaskManager::{new, finished}` 读取任务状态，并调用 `TtlJob::{create_history, finish}` 更新 `JobStore`。`initial_managed_task` 依赖 `scan::TtlScanTask` 和 `task_manager::{ManagedTask, TaskState, TaskStatus}`。

crate 边界由 `Cargo.toml` 确认：常规依赖只有相邻 `astersql-ttl-cache`；大量 TiDB 子 crate 依赖位于 `cfg(windows)` 目标段。目标文件自身只通过同 crate 模块类型协作，没有直接引入外部 crate。生产 SQL 路径的更完整依赖集中在 `persistent.rs` 和 session runtime，不能从本文件的依赖规模推断完整 TTL 系统复杂度。

## 错误处理与边界

纯内存生命周期 API 多用 `Option` 或空向量表达“不满足条件”：非 leader、未知表、已有 current job、调度间隔不足都会令 `lock_new_job` 返回 `None`；无可接管 job 返回空 `Vec`。它们不会区分具体拒绝原因，也没有日志或重试策略。

定时器适配 API 使用 `Result<_, String>` 暴露可诊断边界：提交门禁失败、版本明确不一致、二次锁定失败、request 未知、table/physical ID 不匹配分别返回固定字符串。`get_job` 在 active job 消失后即报告 finished；若相应历史不存在或尚无摘要，`summary` 为 `None`，而不是错误。

`finish_completed_jobs` 只聚合 `TaskManager::finished()` 返回的任务。`tasks_by_job` 为零时，空任务集合也被视为完成，这与 Go `checkFinishedJob` 对空任务列表的 `allFinished=true` 相似；但 Rust 的摘要不会像 Go `summarizeTaskResultWithError` 那样收集每个任务的 `ScanTaskErr`、任务状态计数和 JSON `SummaryText`。调用方若需要取消/超时原因，必须显式把错误传给 `summarize_task_results` 或走更完整的持久化/runtime 流程。

本文件不执行数据库事务，因此不能解决并发节点同时加锁、所有权在完成前变化、SQL 部分成功等错误。对应保护位于 `persistent.rs`：`start_job_with_ranges` 使用 pessimistic transaction 和 `FOR UPDATE NOWAIT`，`finish_job` 校验 owner、删除任务、完成历史，并在提交/回滚失败时禁止复用 session。

## 并发与资源生命周期

`JobManager` 没有内部锁、线程、channel 或异步任务；所有变更方法要求 `&mut self`，预期由外部单线程事件循环或调用方同步串行化。它的 `BTreeMap`/`BTreeSet` 只提供确定性迭代与本地所有权，不提供跨进程互斥。Go 版 `JobManager` 的 `baseWorker` mutex、ticker/select 循环、etcd watcher、owner campaign、worker resize 和退出清理在本文件中均不存在。

表级生命周期为“刷新元数据 -> 锁定并建历史 -> 任务运行/心跳 -> 汇总完成 -> leader GC”。超时接管目前只改写 `TableStatus.owner_id/owner_heartbeat` 并返回 physical ID，不会自动重建 `TtlJob`、扫描任务或本地 active_jobs；调用方必须继续完成恢复接线。`heartbeat_and_timeout_takeover_do_not_interfere` 验证本地续约不会覆盖远端状态，且接管保留原 job ID。

资源回收方面，`TtlJob::finish` 删除 active job 和任务计数但保留历史；`gc` 再按 retention 清理历史和冗余状态。`request_to_job`、`job_scan_indexes` 与 `ttl_indexes` 当前不随 job 完成或 GC 回收，这是扩展长期运行服务时应审视的内存生命周期边界。

## 与 Go 版本的对应关系

Rust `JobManager` 对应 `pkg/ttl/ttlworker/job_manager.go::JobManager` 的核心状态机语义：leader 才能建立/GC job，同一 physical table 只能有一个 current job，锁定时计算过期水位并建立历史，本地 owner 发送心跳，失联 job 可被接管，所有扫描任务完成后汇总并收尾。Rust 独立测试明确映射了 Go 的 `TestParallelLockNewJob`、`TestFinishJob`、`TestSubmitJob`、`TestRescheduleJobs`、`TestGCTableStatus` 和 `TestGCTTLHistory`。

两者并非等量实现。Go `jobLoopWithSession` 在单 goroutine 中维护多组 ticker、timer 同步、命令/通知 watcher、task worker resize、任务检查和 metrics；Rust 本文件没有 loop。Go `lockNewJob` 在 pessimistic transaction 中 `FOR UPDATE NOWAIT` 锁状态行、计算 scan ranges、写 history 和每个 task，然后通知其他扫描管理器；Rust `lock_new_job` 只写内存，事务对应功能在 `persistent.rs::start_job_with_ranges`。Go `checkFinishedJob` 还会回收 external workload；Rust 没有该依赖。

Go 摘要 `TTLSummary` 额外包含 total/scheduled/finished scan task 和序列化文本，并合并任务错误；Rust `TtlSummary` 只保留三类行数和一个错误字符串。Go `rescheduleJobs` 还处理全局 TTL 开关、调度窗口、表删除、job timeout 与取消摘要；Rust `reschedule_timeout_jobs` 只做心跳超时所有权改写。Go `DoGC` 清理持久化历史、task 和 status；Rust `gc` 只清理本地 history/status。上述缺口必须被记录为迁移边界，不能据此宣称 Rust 已具备 Go 管理器的完整生产能力。

## 扩展指南

- 增加提交门禁时，优先修改 `TtlJobAdapter::can_submit_job`/`submit_job`，并同步 `job_manager_integration_test.rs`；若门禁涉及系统表或集群状态，还必须在 `persistent.rs`/session runtime 生产路径落实，不能只改内存适配器。
- 修改锁定语义时，要同时维护 `TableStatus`、`JobStore::active_jobs/history` 和 `job_scan_indexes` 的一致性，并保留 `TtlJob::finish` 的 active-owner 二次校验。跨节点唯一性必须由 `PersistentJobStore` 事务提供。
- 扩展超时接管时，应明确接管后如何重建本地 `TtlJob` 和任务、如何避免旧 owner 继续完成，并新增独立测试覆盖 owner 竞争；不要把逻辑塞入源文件内的 `#[cfg(test)]`，测试应继续放在同目录 `*_test.rs`。
- 扩展摘要时，需要同步 `TtlSummary`、`summarize_task_results`、`job.rs` 历史、`persistent.rs::finish_job`、timer 返回值及 session runtime 的摘要构造，并核对 Go JSON 字段兼容性。
- 增加 GC 时，应为 request/job/index 映射定义明确保留期；删除映射前要保证 `TtlTimerHook::poll` 对已完成请求仍能得到稳定结果。
- 任何 index scan 改动都应保留版本检查的三态语义：允许、兼容回退、明确阻塞。对应测试 `index_scan_submission_obeys_version_gate_and_persists_choice` 还验证了不一致结果的一分钟缓存以及选定索引按 job 固化。
- 性能上，`finish_completed_jobs` 当前对每个 active job 扫描整个 finished 列表，复杂度近似 `O(active_jobs * finished_tasks)`；规模扩展时可按 job ID 建索引，但必须保持汇总和完成判定不变。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ttl/ttlworker/job_manager.rs` 确认目标文件有 26 个符号。
- RustCodeGraph 源码/关系查询：读取 `job_manager.rs` 全部 369 行；文件关系显示其被 `pkg/session/runtime/ttl_runtime.rs`、`pkg/session/runtime/ttl_worker_session_test.rs`、`pkg/ttl/ttlworker/job_manager_integration_test.rs`、`pkg/ttl/ttlworker/persistent.rs` 引用。另核对 `timer.rs::TtlJobAdapter/TtlTimerHook`、`job.rs::JobStore/TtlJob::finish`、`task_manager.rs`、`persistent.rs::{start_job_with_ranges, finish_job}` 和 session runtime。
- crate/模块证据：`pkg/ttl/ttlworker/Cargo.toml`、`pkg/ttl/ttlworker/lib.rs`。
- Rust 测试证据：`pkg/ttl/ttlworker/job_manager_test.rs`；`pkg/ttl/ttlworker/job_manager_integration_test.rs` 的唯一锁定、完成汇总、提交门禁、index scan 版本门禁、心跳/接管和 leader GC 用例；`task_manager_test.rs`、`task_manager_integration_test.rs` 对 `initial_managed_task` 的复用。
- Go 对照证据：`pkg/ttl/ttlworker/job_manager.go` 的 `JobManager`、`jobLoopWithSession`、`handleSubmitJobRequest`、`checkFinishedJob`、`rescheduleJobs`、`lockNewJob`、`updateHeartBeatForJob`、`summarizeTaskResultWithError`、`DoGC`；以及 `job_manager_test.go`、`job_manager_integration_test.go` 中对应回归用例。
- 人工复核结论：本文件存在是为了集中表达可测试的 TTL job 内存状态机和 timer adapter；安全扩展必须同时区分本地模型与 `persistent.rs`/session runtime 的生产事务路径。本文没有把 Go 独有的事件循环、外部 workload、metrics 或持久化 GC 描述成 Rust 已支持能力。
