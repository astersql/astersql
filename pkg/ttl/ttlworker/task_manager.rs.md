# `pkg/ttl/ttlworker/task_manager.rs`

## 文件定位

本文件属于 `astersql-ttl-ttlworker` crate（`pkg/ttl/ttlworker/Cargo.toml`），是 TTL worker 的单节点、纯内存扫描子任务调度模型。模块由 `pkg/ttl/ttlworker/lib.rs` 以 `pub mod task_manager` 导出；其上层 `JobManager` 在 `pkg/ttl/ttlworker/job_manager.rs` 中持有公开字段 `task_manager: TaskManager`，并在 `JobManager::new` 中创建实例。

当前 Rust 实现不直接访问 `mysql.tidb_ttl_task`、不创建 worker，也不自行启动后台循环。它接收已经构造好的 `ManagedTask`，在等待、运行和完成三个集合之间迁移，并把需要启动的 `TtlScanTask` 返回给调用者。因此它是 Go `taskManager` 的可测试内存语义子集，而不是 Go 生产管理器（`pkg/ttl/ttlworker/task_manager.go`）的完整替代。仓库内对 `push_waiting`、`reschedule`、`heartbeat_or_resign`、`report_finished` 和 `remove_invalid_jobs` 的 Rust 调用目前位于独立测试文件；生产 Rust 代码只构造/持有该对象。

## 核心职责

- 用 `max_running_tasks` 限制本节点同时处于 `Running` 的任务数，并通过 `VecDeque` 保持等待任务的 FIFO 顺序（`TaskManager::new`、`TaskManager::reschedule`）。
- 以 `(job_id, scan_id)` 作为运行任务的身份键，防止同一身份在 `running` 集合中重复调度（`TaskManager::reschedule`）。
- 在调度时写入本节点 `owner_id` 和调用方提供的心跳时间，在续约或 TTL 失效时更新/释放归属（`TaskManager::reschedule`、`TaskManager::heartbeat_or_resign`）。
- 接收扫描结果：普通终止进入完成集合，worker 停止则清除 owner 并回到等待队列，保证缩容不会丢任务（`TaskManager::report_finished`）。
- 清理已取消或过期作业的等待/运行任务，并向上层暴露运行数和完成结果只读视图（`remove_invalid_jobs`、`running_count`、`finished`）。

`COUNT_RUNNING_TASKS_SQL` 保留了 Go `countRunningTasks` 的同值 SQL 常量，但本文件没有执行它；Rust 的并发上限仅依据本对象的 `running.len()`，不是跨节点的数据库全局限制。

## 主要符号

- `COUNT_RUNNING_TASKS_SQL: &str`：查询系统表中全局 `running` 数的 SQL 文本。目前没有本 crate 内 Rust 调用点，是与 Go 逻辑对齐的预留接口。
- `TaskStatus::{Waiting, Running, Finished, Error}`：内存生命周期枚举。`Error` 是兼容态，本文件没有把任何结果写成 `Error`；扫描错误也会进入 `Finished`。
- `TaskState { total_rows, success_rows, error_rows }`：任务累计统计。`report_finished` 当前只从 `ScanResult::scanned_rows` 更新 `total_rows`，成功数和错误数须由其它接线补充。
- `ManagedTask`：将 `TtlScanTask`、状态、owner、心跳和统计合并为管理器内记录。`job_manager::initial_managed_task` 是当前标准构造入口，初始化为 `Waiting`、无 owner、零心跳和默认统计。
- `TaskManager`：保存节点 ID、并发上限、FIFO `waiting`、按复合键索引的 `running` 以及追加式 `finished`。
- `TaskManager::new`：把零并发配置钳制为 1，建立空集合。
- `push_waiting`：只接纳状态恰为 `Waiting` 的任务；其它状态静默忽略。
- `reschedule`：填充空闲运行槽，完成状态、owner 和心跳迁移，并返回本次启动所需的扫描任务副本。
- `heartbeat_or_resign`：由闭包判断每个运行任务的 TTL 是否仍启用；启用则续约，否则恢复为无 owner 的等待态。
- `report_finished`：按结果身份取走运行任务；`WorkerStop` 回队，其它原因进入 `finished`。未找到身份时返回 `false`。
- `remove_invalid_jobs`、`running_count`、`finished`：分别清除无效作业、查询运行数、借用完成切片。

## 执行流程

1. 上层通常以 `job_manager::initial_managed_task` 包装 `TtlScanTask`，再调用 `push_waiting`。非 `Waiting` 输入不会入队。
2. 调用方传入时间 `now` 执行 `reschedule`。方法循环弹出队首，直到运行数达到 `max_running_tasks` 或等待队列为空。它再次过滤非等待态，并跳过已经存在于 `running` 的 `(job_id, scan_id)`。
3. 合法任务被改为 `Running`，owner 设置为管理器的 `owner_id`，`owner_heartbeat` 设置为 `now`；随后扫描任务被克隆到返回值，完整 `ManagedTask` 写入 `running`。
4. 运行期间，调用方可执行 `heartbeat_or_resign(now, ttl_enabled)`。闭包返回真时仅刷新心跳；返回假时任务恢复为 `Waiting`、清空 owner/心跳，从 `running` 移除后追加到等待队尾，并返回其身份键。
5. 扫描结束后，调用方传入 `ScanResult` 给 `report_finished`。方法先按身份从 `running` 删除记录并更新 `total_rows`。若原因为 `WorkerStop`，任务重置归属并回到等待队尾；其余所有原因（含 `Error`、取消和表变更）统一标为 `Finished` 并追加到 `finished`。
6. 作业集合变化时，`remove_invalid_jobs` 同时过滤等待和运行集合。之后到达的被清除任务结果因身份不存在而得到 `false`。

上述流程没有隐式线程或定时器；每一步都必须由持有 `&mut TaskManager` 的上层显式驱动。

## 数据与状态

`waiting: VecDeque<ManagedTask>` 保证正常输入的先进先出，但存在两个重要细节：重复身份可同时存在于等待队列；当第一份进入 `running` 后，后续重复项会在 `reschedule` 中被弹出并丢弃。辞任或 `WorkerStop` 的任务追加在队尾，不保留原排队位置。

`running: BTreeMap<(String, i64), ManagedTask>` 以作业 ID 和扫描 ID 联合唯一定位任务。选择 `BTreeMap` 使遍历/辞任键的输出按键有序，但本文件不声明该顺序为跨版本 API。`finished: Vec<ManagedTask>` 按报告顺序累积，既不去重也不自动清理。

状态迁移为 `Waiting -> Running -> Finished`，另有 `Running -> Waiting`（TTL 关闭或 `WorkerStop`）。`TaskStatus::Error` 没有本文件内的进入边。`owner_id` 只在运行态设为当前节点；回到等待态时置 `None`，心跳归零。构造器确保 `max_running_tasks >= 1`，因此配置 0 不表示停调度。

时间值是无单位的 `u64`，本文件既不读取系统时钟也不判断超时；注释约定其含义由调用方决定。因此调用者必须在所有调用中使用一致时间域。

## 依赖与调用关系

直接代码依赖很小：标准库 `VecDeque`、`BTreeMap`/`BTreeSet`，以及同 crate 的 `scan::{TtlScanTask, ScanResult, TaskTerminateReason}`。`Cargo.toml` 的常规依赖只有相邻 `astersql-ttl-cache`；大量完整 TTL 依赖被放在 `cfg(windows)` 条件表中，但本文件没有直接引用它们。

已确认的上游关系是：

- `lib.rs` 公开模块；`job_manager::JobManager` 聚合 `TaskManager`，`JobManager::new` 调用 `TaskManager::new`。
- `job_manager::initial_managed_task` 构造 `ManagedTask`，被 `task_manager_test.rs`、`task_manager_integration_test.rs` 和 `job_manager_integration_test.rs` 用作入队输入。
- 调度、心跳、完成和清理 API 的现有 Rust 调用来自上述测试；未发现生产 Rust 调用。这意味着本文件当前尚未连接到 Rust worker 执行循环或持久层。
- 下游 `TtlScanTask` 描述扫描范围/表/批大小，`ScanResult` 带终止原因和扫描行数；本文件只调度和归档这些值，不执行 `TtlScanTask::scan_sql`。
- `job_manager::summarize_task_results` 可读取完成任务中的 `TaskState` 形成作业汇总，但 `TaskManager::finished()` 与该函数之间当前没有生产调用边。

RustCodeGraph 能索引本文件全貌（20 个符号）及模块文件集合，但针对这些 `impl` 方法的精确 `query/callers/callees` 没有返回方法节点或调用边；上述调用关系因此由索引源码视图加精确仓库引用搜索交叉确认。

## 错误处理与边界

本 API 不返回 `Result`，边界事件使用忽略、布尔值或列表表达：`push_waiting` 静默拒绝错误状态；重复运行身份在调度时静默跳过；`report_finished` 对未知/已清除身份返回 `false`；辞任返回受影响身份列表。调用方若需要日志、重试或持久化一致性，必须在外层实现。

`report_finished` 不读取 `ScanResult::error`，且除 `WorkerStop` 外不区分 `Finished`、`Canceled`、`ErrorRateExceeded`、`TableChanged` 和 `Error`，这些结果都进入 `TaskStatus::Finished`。这是为了匹配 Go `reportTaskFinished` 的持久状态：诊断错误属于完成记录，不应制造 job manager 无法收敛的额外持久状态。

`remove_invalid_jobs` 直接丢弃运行任务，不取消底层 worker，也不移动到 `finished`。这只有在外层同时负责取消执行与处理迟到结果时才安全。完成列表也不会随 `valid_jobs` 过滤。

内存模型不实现 Go 版的事务锁、过期 owner 抢占、数据库 affected-row 校验、SQL/JSON 序列化错误、worker 调度失败回滚、处理行等待超时、指标或日志，因此不能用它证明这些生产边界已经移植。

## 并发与资源生命周期

`TaskManager` 的修改方法都要求 `&mut self`，类型内部没有锁、原子量、通道、异步任务或取消句柄。并发共享必须由上层串行化；`task_manager_integration_test.rs::parallel_lock_allows_exactly_one_copy_of_a_task_identity` 使用 `Arc<Mutex<TaskManager>>` 证明的是外部互斥下的行为，而非管理器自身无锁并发安全。

任务资源生命周期仅表现为集合所有权迁移：等待队列拥有未启动记录，运行映射拥有已认领记录，完成向量长期保留终态记录。返回的 `TtlScanTask` 是克隆值，管理器无法感知实际 worker 是否启动成功，也没有回收/cancel API。辞任和 `WorkerStop` 会保留记录并重新排队；无效作业清理则立即丢弃记录。

Go 实现的真实资源生命周期更复杂：`resizeWorkers` 启停 worker 并等待停止，`runningScanTask` 持有 cancel 函数和执行结果，`updateHeartBeat` 写系统表，`checkFinishedTask` 等待删除侧消费并最终释放 context。扩展 Rust 生产接线时必须补齐这些生命周期，而不能只依赖本文件的状态迁移。

## 与 Go 版本的对应关系

Rust `COUNT_RUNNING_TASKS_SQL` 对应 Go `countRunningTasks`；`reschedule` 概括 Go `rescheduleTasks`/`lockScanTask` 的容量检查、任务认领和分发；`heartbeat_or_resign` 概括 `updateHeartBeat`、`taskHeartbeatOrResignOwner` 和 `tryResignTaskOwner`；`report_finished` 概括 `checkFinishedTask`/`reportTaskFinished`；`remove_invalid_jobs` 对应 `checkInvalidTask` 的本地剔除意图。

两者并非一一等价：

- Go 从系统表查询候选，在悲观事务及 `FOR UPDATE` 下竞争 owner，并用全局 running 数限制所有节点；Rust 只管理预先入队的本地值，并按本地映射容量限制。
- Go 校验 info schema、调度真实扫描 worker、维护删除通道和指标；Rust 没有这些依赖或副作用。
- Go 心跳/辞任把累计统计与 previous owner 持久化，并用 affected rows 防止旧 owner 覆盖新 owner；Rust 只改内存字段。
- Go 缩容后会等待尚未处理的行、记录扫描错误并取消 context；Rust 遇到 `WorkerStop` 立即回队，只保留 `scanned_rows` 到 `total_rows`。
- Go 的无效任务判断是数据库记录消失或 owner 已变化；Rust 接收上层计算出的有效 job ID 集合。

Rust 测试有意识地覆盖目前 API 可表达的交集：唯一身份、容量/FIFO、辞任后再调度、普通完成与 `WorkerStop` 分流、无效作业剔除。Go 集成测试还覆盖跨 manager 抢锁、心跳过期接管、worker 缩容等待、全局上限、SQL 错误隔离和 owner 竞争，这些是当前 Rust 文件未实现的差距。

## 扩展指南

- 若接入真实执行循环，应从 `JobManager` 的驱动处显式调用 `push_waiting`/`reschedule`/结果回报，并为 worker 启动失败设计回滚，避免任务已在 `running` 而实际未执行。对应测试应放在独立的 `task_manager_test.rs` 或 `task_manager_integration_test.rs`，不要内嵌到源文件。
- 若实现持久化和多节点竞争，应围绕 Go `lockScanTask` 的事务、owner 超时和 affected-row 条件建立独立存储接口；仅使用 `BTreeMap` 去重不足以保证跨节点唯一性。`COUNT_RUNNING_TASKS_SQL` 只有被事务性接线后才有意义。
- 若完善统计，应决定如何从扫描/删除阶段分别累计 `total_rows`、`success_rows`、`error_rows`，并保持 Go `dumpNewTaskState` 对接管前状态的累加语义。要增加错误详情时，优先扩展独立结果/状态类型，不要把扫描错误改成持久 `Error` 状态。
- 若允许停止所有调度，不应依赖 `TaskManager::new(..., 0)`，因为构造器会钳制为 1；需要新增显式暂停/禁用状态并测试恢复行为。
- 若改变辞任、重复任务或无效作业策略，必须同时验证 FIFO、公平性、迟到结果、底层任务取消和内存增长。完成列表当前无界，长期运行接线应定义消费或清理协议。
- 若增加内部并发，需明确锁顺序、回调是否可重入以及 `ttl_enabled` 执行期间能否阻塞；当前外部 `Mutex` 模式最简单，闭包在 `BTreeMap::retain` 中持有管理器可变借用。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/ttl/ttlworker` 列出该模块 58 个 Go/Rust 文件；`node --file pkg/ttl/ttlworker/task_manager.rs --offset 1 --limit 500` 返回目标文件全部 189 行和 20 个符号；`query TaskManager` 定位本文件类型。对 `reschedule` 等方法的 `query/callers/callees` 未返回可用调用边，此限制已通过精确引用搜索补证。
- 目标实现：`pkg/ttl/ttlworker/task_manager.rs`；crate/模块边界：`pkg/ttl/ttlworker/Cargo.toml`、`pkg/ttl/ttlworker/lib.rs`；相邻生产入口：`pkg/ttl/ttlworker/job_manager.rs`、`pkg/ttl/ttlworker/scan.rs`。
- Rust 独立测试：`pkg/ttl/ttlworker/task_manager_test.rs` 验证空调度、辞任回队和扫描错误仍为完成态；`pkg/ttl/ttlworker/task_manager_integration_test.rs` 验证外部互斥下去重、容量/FIFO、选择性辞任、`WorkerStop` 回队及无效作业；`pkg/ttl/ttlworker/job_manager_integration_test.rs` 验证 `JobManager` 聚合路径。
- Go 对照：`pkg/ttl/ttlworker/task_manager.go`；Go 测试：`pkg/ttl/ttlworker/task_manager_test.go`、`pkg/ttl/ttlworker/task_manager_integration_test.go`。后者覆盖跨节点锁、心跳接管、缩容、全局 running 限制和 owner 变化等完整生产语义。
- 人工复核结论：本文件存在于 TTL 作业之下，用有限容量的本地状态机管理扫描子任务；安全扩展必须在保持状态/身份不变量的同时补齐目前缺失的持久化、worker 取消和跨节点竞争语义。任务为纯文档分析，按计划未运行 Cargo。
