# `pkg/session/runtime/ttl_timer.rs`

## 文件定位

本文件属于 `astersql-session` crate 的 `runtime` 私有模块；模块由 `pkg/session/runtime.rs` 通过 `mod ttl_timer` 装配，文件内可见性均限制为当前父模块（`pub(super)`）或文件内部。它把两条链路连接起来：一条是把当前 InfoSchema 中的 TTL 表/分区同步成 `mysql.tidb_timers` 记录，另一条是实现通用 timer runtime 的 `Hook`，把被触发的 timer event 转换成一次 TTL job 并跟踪到完成。

生产接线位于 `pkg/session/runtime/ttl_runtime.rs`：`start_domain_ttl_job_manager_with_interval` 每轮从同一 InfoSchema 快照收集 `TtlSchedule`，调用 `sync_ttl_timers`，随后以 `TIMER_KEY_PREFIX` 构建 timer runtime，并把 `SqlTtlTimerHook::new` 注册到 `TIMER_HOOK_CLASS`（`tidb.ttl`）。`run_ttl_tick_inner` 在没有指定单个 event 时也调用同步函数。因此本文件不是独立调度器；定时扫描、hook 派发和 timer 表存储分别由 `astersql-timer-runtime`、`astersql-timer-api` 和 session 侧 table store 承担。

`pkg/session/Cargo.toml` 声明 crate 名为 `astersql-session`，直接依赖 `astersql-domain`、`astersql-timer-api`、`astersql-timer-runtime`、`astersql-timer-tablestore`、`astersql-ttl-ttlworker`、`astersql-sessionctx-vardef`、`serde_json` 与 `chrono`。本文件没有条件编译项，`nextgen` feature 也没有改变这里的代码路径。

## 核心职责

1. `sync_ttl_timers` 将一个 `TtlSchedule` 切片收敛到持久化 timer 表：为每个活跃物理表创建或更新 timer，保留已有手工触发/事件扩展状态，并对快照中已消失的 timer 先禁用、满 10 分钟后删除。
2. `SqlTtlTimerHook::OnPreSchedEvent` 在真正触发前执行守门：TTL 全局开关必须开启、目标物理表仍存在于当前 TTL schedule、当前 UTC 时间必须位于调度窗口内；否则固定延迟 60 秒再检查。
3. `SqlTtlTimerHook::OnSchedEvent` 负责幂等地承接事件：若对应 job 尚未完成且事件有效，就调用 `run_ttl_event`；之后轮询 job history，并用完成摘要和事件开始时间关闭 timer event。
4. `Stop` 对所有本文件创建的 watcher/job 线程发出停止信号、唤醒并 `join`，保证 Domain 关闭时不遗留本文件拥有的后台线程。

这里维护的是“timer 元数据与 TTL job 之间的适配层”，不负责枚举 TTL 表的具体规则（`collect_ttl_schedules`）、执行 TTL 扫描/删除的主体逻辑（`run_ttl_event`），也不实现 timer runtime 自身的状态机。

## 主要符号

- `SqlTtlTimerHook { domain, owner_id, client, stop, jobs }`：`Hook` 的 session 侧实现。`domain` 提供最新 InfoSchema、关闭状态与内部 SQL session；`owner_id` 传给 TTL job；`client` 用于读取及关闭 timer event；`stop` 是跨线程取消标志；`jobs` 保存所有仍由 hook 管理的 `JoinHandle`。
- `SqlTtlTimerHook::new(domain, owner_id, client)`：把传入的 `Box<dyn TimerClient>` 转成共享的 `Arc<dyn TimerClient>`，初始化未停止状态与空线程列表。
- `timer_ids(data)`：解析 timer 的 JSON `Data`，强制取得有符号 64 位 `table_id` 和 `physical_id`。缺失字段与非法 JSON 都转换为 `TimerError`。
- `pre_schedule_delay(enabled, table_exists, now, start, end)`：纯判定函数。任一守门条件不满足时返回 60 秒，否则返回零；调度窗口解析错误原样以 `String` 返回。
- `job_status(domain, table_id, physical_id, event_id)`：查询 `mysql.tidb_ttl_job_history`。无行返回 `None`；有行返回“是否终态”和摘要 JSON。终态只包括 `finished`、`timeout`、`cancelled`。
- `Hook::Start`：空实现；资源是在 `OnSchedEvent` 时按事件创建，而不是在 hook 启动时创建。
- `Hook::Stop`：以 Release 顺序写入停止标志，逐个 `unpark` 并 `join` 所有线程；线程侧以 Acquire 读取。
- `Hook::OnPreSchedEvent`：读取 timer、解析 ID、从当前 InfoSchema 重建 TTL schedules，调用 `pre_schedule_delay` 并返回 `PreSchedEventResult`。
- `Hook::OnSchedEvent`：清理已完成 handle、检查 event 输入、启动名为 `ttl-timer-job` 的线程，在该线程中提交/续接 TTL job并等待 event 可安全关闭。
- `sync_ttl_timers(session, schedules, now)`：通过 `TtlWorkerSqlSession::execute` 直接维护系统表的主同步函数；返回 `Result<(), String>`，任一关键 SQL/JSON 错误会终止本轮同步。

## 执行流程

Timer 同步流程如下（`sync_ttl_timers`）：

1. 为输入 schedules 建立 `live` key 集合；key 和 tags 分别由 `timer_key`、`timer_tags` 生成，timer data 固定包含 `table_id`/`physical_id`，新扩展对象包含 `tags`、空 `manual` 和空 `event`。
2. 按 namespace `default` 与 key 查询现有记录。若存在，仅在 interval 表达式、启用状态或 tags 有差异时更新；更新会刷新 data、表达式、扩展 tags、启用 timer，并递增 `VERSION`。修改扩展时先解析旧 JSON，只覆盖 `tags`，从而保留 `manual`、`event` 及其他未知字段。
3. 若不存在，则从 `mysql.tidb_ttl_table_status` 读取该物理表的 `last_job_start_time` 作为 watermark，并插入 `INTERVAL`/UTC、hook class 为 `tidb.ttl`、初始状态 `IDLE` 的 timer。没有有效 watermark 时使用 `1970-01-01 00:00:01`；源码明确说明这是为了模拟 Go 零时间会立即调度的行为，因为 Rust 通用 timer 把 `NULL` 解释为“没有下一事件”。
4. 再列出所有 `default` namespace、`tidb.ttl` hook 的 timer。仍在 `live` 中的不动；不在快照中的记录，若按可解析的 UTC `CREATE_TIME` 已超过 600 秒则删除，否则仅在当前启用时禁用并递增版本。无法解析创建时间的记录不会被判为过期，因此最多会停留在禁用状态。

事件流程如下：

1. Timer worker 通过 `Hook` trait 先调用 `OnPreSchedEvent`。本实现要求 event 中存在 timer，解析 timer data，并依据最新 InfoSchema、全局 `EnableTTLJob` 及调度窗口计算零或 60 秒 delay。
2. worker 接着调用 `OnSchedEvent`。该函数先回收此前已经结束的线程，然后要求 timer 和 `EventStart` 存在，读取 event ID，并用 `job_status` 判断同 ID job 是否已经终结。
3. 新线程再次以最新 InfoSchema 校验物理表。若 timer 已禁用、表已失效，或事件从开始到当前已超过 600 秒且尚无已完成 job，则直接关闭 event，并保持先前 watermark，避免跳过应补跑的时间点。
4. 有效且未完成的事件调用 `run_ttl_event(domain, owner_id, now, table_id, physical_id, event_id, canceled)`；取消闭包同时观察 hook stop 与 Domain closed。执行错误会记日志，但线程仍进入完成跟踪，以便已写入的 job 状态能够收敛 timer event。
5. watcher 每 10 秒读取 timer。timer 读取失败、event ID 已变化、收到 stop 或 Domain 关闭都会退出。若 job history 进入终态，则以 `EventStart` 更新 watermark，并写入 `{last_job_request_id, last_job_summary}` 后关闭事件；关闭失败会继续下一轮重试。
6. 若等待超过 600 秒且仍完全找不到 job history，则以旧 watermark 尝试关闭并退出。这个分支与“job 已存在但长期未完成”不同：后者没有本文件内的 10 分钟强制关闭条件。

## 数据与状态

- Timer 身份由 `timer_key(table_id, physical_id)` 唯一表达；同一逻辑表的不同分区拥有不同 `physical_id`，但 data 同时保存父表 ID 与物理 ID。
- `TIMER_EXT` 的 `tags` 是同步判定的一部分；`manual` 和 `event` 属于其他 timer 流程的状态，本文件更新时必须保留。`pkg/session/runtime/ttl_timer_test.rs` 明确验证 interval 更新后两者不丢失。
- `WATERMARK` 表示上次已消费的调度位置。正常 job 完成时推进到当前 `EventStart`；取消无效/过旧事件以及“超时仍无 job”时保留原 watermark，使后续仍有机会重新调度。
- `SUMMARY_DATA` 只在确认 job history 终态后设置，字段名与 Go 的 `ttlTimerSummary` JSON 一致。空或 `"<nil>"` 的 `summary_text` 映射为 JSON `null`；非空但非法 JSON 是错误。
- `live` 只反映传入的单次 schedule 快照。生产调用在同步前从同一 `domain.info_schema()` 收集 schedules，避免一次同步混用多个 schema 版本；本函数自身不缓存 InfoSchema 版本。
- hook 的 `jobs` 只保存线程句柄，不保存按 timer 去重的 map。并发约束主要依赖 timer runtime 的 event 状态以及 job ID/event ID 的幂等关联；进入新的回调时会回收已经完成的 handle。

## 依赖与调用关系

上游调用与接线：

- `pkg/session/runtime/ttl_runtime.rs::start_domain_ttl_job_manager_with_interval` → `collect_ttl_schedules` → `sync_ttl_timers`；首次成功同步后创建 table-backed timer runtime，并注册 `SqlTtlTimerHook::new`。
- `pkg/session/runtime/ttl_runtime.rs::run_ttl_tick_inner` 在 `event.is_none()` 时调用 `sync_ttl_timers`，随后按 schedule 执行本地 TTL tick。
- `pkg/timer/runtime/worker.rs::triggerEventWithCounters` 通过 `pkg/timer/api/hook.rs::Hook` 调用 `OnPreSchedEvent` 和 `OnSchedEvent`；这是 trait 动态派发，不是对具体 struct 的静态调用。

主要下游：

- `ttl_metadata::collect_ttl_schedules` 与 `TtlSchedule`：从 InfoSchema 得到当前 TTL 物理表及 interval。
- `ttl_runtime::{within_ttl_window, run_ttl_event}`：分别提供窗口判定与实际 TTL event 执行。
- `ttl_worker_session::TtlWorkerSqlSession` / `WorkerSession::execute`：执行对 `mysql.tidb_timers`、`mysql.tidb_ttl_table_status`、`mysql.tidb_ttl_job_history` 的内部 SQL。
- `astersql_ttl_ttlworker::timer_sync::{timer_key, timer_tags, TIMER_HOOK_CLASS}`：确保 session 侧 timer key、标签和 hook class 与 TTL worker crate 约定一致。
- `TimerClient::{GetTimerByID, CloseTimerEvent}`：确认 event 仍为当前事件并原子地结束它；`WithSetWatermark` 与 `WithSetSummaryData` 是关闭时携带的更新选项。

RustCodeGraph 对 `sync_ttl_timers` 给出的直接 caller 包括 `run_ttl_tick_inner` 及 `ttl_runtime` 测试；对 `OnPreSchedEvent`/`OnSchedEvent` 给出的下游分别包括 `timer_ids`、`pre_schedule_delay`、`job_status`，并确认 `Hook` trait 方法由 timer worker 调用。

## 错误处理与边界

- 输入 timer 缺失、`EventStart` 缺失、timer data 非法或缺少两个 ID 时，hook 回调同步返回 `TimerError`，不会启动线程。
- `OnPreSchedEvent` 收集 schedule 或解析窗口失败时返回错误，而不是退化成 60 秒 delay。与之不同，条件合法但 TTL 被关闭、表不存在或窗口不匹配时正常返回 delay。
- `OnSchedEvent` 启线程之前查询 job history 失败会返回错误；线程创建失败也转换成带上下文的 `TimerError`。
- 线程内 `collect_ttl_schedules` 失败按“表无效”处理并关闭 event；`run_ttl_event` 失败只记录日志。轮询阶段 `GetTimerByID` 失败直接退出，不重试；`job_status` 错误被忽略并继续轮询，正常完成时的 `CloseTimerEvent` 失败也会重试。
- “等待超过 600 秒且查无 job”分支无论关闭调用成功与否都会退出。因此关闭失败可把 event 留给 timer runtime 的后续恢复路径；本文件不在该线程内无限重试此兜底关闭。
- 同步函数对错误采取 fail-fast：读取、解析旧扩展、更新、创建、列举或退役任一步失败都会中断本轮。它没有显式事务包裹全部记录，所以失败前已完成的逐条修改不会由本函数回滚，下一轮同步依靠幂等比较继续收敛。
- 退役时间只接受 `%Y-%m-%d %H:%M:%S` 并按 UTC 解释。缺列、非文本或不可解析时间不会触发删除；已启用记录仍会先被禁用。

## 并发与资源生命周期

`SqlTtlTimerHook` 自身由 timer runtime 持有；每次 `OnSchedEvent` 最多在本次回调中创建一个 OS thread，并把 handle 纳入 `jobs`。线程共享 `Arc<Domain>`、`Arc<dyn TimerClient>` 和 `Arc<AtomicBool>`，字符串/ID/时间值按事件复制或移动，因而不借用回调参数。

停止协议是：`Stop` 以 `Ordering::Release` 设置标志，`unpark` 所有线程，使正在 10 秒 `park_timeout` 的线程立即检查，以 `Ordering::Acquire` 观察停止，再由 `join` 等待退出。Domain 自身的 `is_closed()` 是第二条取消信号；传给 `run_ttl_event` 的取消闭包和轮询循环都检查这两者。`Stop` 忽略线程 panic 的 join 错误，但仍遍历剩余句柄。

正常运行期间，已完成线程只在下一次 `OnSchedEvent` 或最终 `Stop` 时 join。`Start` 不分配资源。同步函数自身单线程执行 SQL，没有锁；其跨节点一致性依赖 timer 表版本、timer runtime/store 以及 TTL owner 接线，而不是本文件内的 mutex。生产接线仅在 TTL owner 节点保留 runtime，失去 owner 时 `ttl_runtime.rs` 会丢弃 runtime，触发其停止生命周期。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/ttl/ttlworker/timer.go` 和 `pkg/ttl/ttlworker/timer_sync.go`。

- Rust `SqlTtlTimerHook` 对应 Go `ttlTimerHook`；`OnPreSchedEvent` 都检查全局开关、时间窗口和表/物理表是否可提交，拒绝时延迟一分钟。Rust 直接从 Domain InfoSchema 收集 schedule，Go 通过 `TTLJobAdapter.CanSubmitJob`；Rust 的非法 timer data 返回错误，Go 版本记录错误后以一分钟 delay 返回。
- Rust `OnSchedEvent` 对应 Go 同名方法及 `waitJobFinished`：先复用已有 job 或提交新 job，再等待终态、更新 watermark 与 summary。Rust 把提交和等待放在同一个命名线程，Go 在回调线程提交后另起 goroutine 等待；两者的 Stop 都会取消并等待自有后台任务。
- Rust 以 `mysql.tidb_ttl_job_history` 直接实现 `GetJob` 语义，并通过 `run_ttl_event` 实现提交；Go 将两者抽象为 `TTLJobAdapter`。Rust 额外用 event ID 查询终态，使回调重入时可跳过重复执行。
- Rust `sync_ttl_timers` 对应 Go `TTLTimersSyncer.SyncTimers`/`syncOneTimer`：key、tags、data、interval、启停和 10 分钟延迟删除语义一致。Go 使用 `TimerClient` 和内存 cache，Rust 直接操作 `mysql.tidb_timers`；Rust 每轮全量读取相关 timer，不保留 `key2Timers` cache。
- Go 创建 timer 时遵循 `TTLInfo.Enable`，而 Rust 输入 `TtlSchedule` 已代表当前可调度对象，创建和更新时固定启用；消失对象由随后禁用/延迟删除路径处理。
- watermark 兼容点在 Rust 源码中特别标注：Go 的零 `time.Time` 会使新 timer 立即调度；Rust 通用 timer 对 `NULL` 不调度，所以 Rust 写入 1970 年非空值。

Go 测试 `pkg/ttl/ttlworker/timer_test.go` 覆盖 pre-schedule 守门、提交错误、复用已有 job、无效/禁用/过旧事件保留 watermark、timer 被删或 event ID 改变时退出、完成关闭失败重试以及 stop 等待；`pkg/ttl/ttlworker/integrationtest/timer_sync_test.go` 覆盖同步器的创建、更新、cache 与延迟删除。它们是移植语义的重要对照，但不是 Rust 测试的替代品。

## 扩展指南

- 增加 timer data 字段时，应同时修改 `sync_ttl_timers` 的序列化和 `timer_ids`（或引入明确的结构体反序列化），并同步 Go `TTLTimerData`/兼容策略。新字段必须考虑旧记录缺字段时的行为，并在独立的 `pkg/session/runtime/ttl_timer_test.rs` 增加非法、缺失和向后兼容用例。
- 改变触发守门条件时，优先扩展纯函数 `pre_schedule_delay`，再由 `OnPreSchedEvent` 提供所需事实；同步更新 Rust 窗口/开关/表存在性测试和 Go `TestTTLTimerHookPrepare` 对照。不要把可恢复条件误写成错误，否则 timer worker 的重试语义会改变。
- 修改 job 终态或摘要结构时，应联动 `job_status`、`OnSchedEvent` 的 summary JSON、TTL job history 写入方及 Go `ttlTimerSummary`。需验证 `finished`、`timeout`、`cancelled`、非终态、无行、空摘要与非法摘要。
- 修改同步字段时，必须保留不由本函数拥有的 `TIMER_EXT` 子树，并明确是否需要递增 `VERSION`。应扩展现有 Rust 同步测试，而不是把测试嵌入生产文件；还应验证创建、无变化、更新、禁用、十分钟后删除和不可解析创建时间。
- 调整轮询/超时时间时，应区分三个概念：触发前一分钟 delay、事件未提交的 600 秒保护、完成状态的 10 秒轮询。缩短时间会增加系统表和 timer store 压力；改变旧 watermark/新 watermark 选择会影响漏跑或重复跑风险。
- 引入更多并发时要保持 `Stop` 能唤醒并 join 所有本文件创建的任务，并评估同一 timer/event 的重复线程。若改为 async task，需同步 timer runtime 的阻塞/异步边界，不能只替换 `std::thread`。
- 本文件属于 session crate 私有实现；如果要暴露跨 crate API，应优先在 `astersql-ttl-ttlworker` 或 timer API 中定义稳定边界，而不是扩大 `pub(super)` 可见性。

## 验证依据

- 目标源码：`pkg/session/runtime/ttl_timer.rs`，核对了全部 387 行；主要符号为 `SqlTtlTimerHook`、`timer_ids`、`pre_schedule_delay`、`job_status`、`Hook::{Start, Stop, OnPreSchedEvent, OnSchedEvent}`、`sync_ttl_timers`，文件内无条件编译项。
- RustCodeGraph：`status` 显示目标位于已建立索引的 7,032 个 Rust 文件中；`files --filter pkg/session/runtime/ttl_timer.rs` 确认单一文件；`node --file ...` 读取全文件；`node sync_ttl_timers` 确认 caller 包括 `run_ttl_tick_inner` 和相关 runtime 测试；`node OnPreSchedEvent`/`node OnSchedEvent` 确认 trait worker 调用及本实现下游；`node job_status`、`node timer_ids`、`node pre_schedule_delay` 核对内部调用边。独立 `callers` 命令未返回结果，因此没有把该次空输出当作证据。
- crate 与模块：`pkg/session/Cargo.toml`、`pkg/session/runtime.rs`、`pkg/session/runtime/ttl_runtime.rs`；核对了 crate 归属、直接依赖、模块声明、同步调用、hook factory 注册与 owner 生命周期。
- Rust 独立测试：`pkg/session/runtime/ttl_timer_test.rs` 覆盖跨午夜窗口、全局关闭、表不存在、timer 创建/interval 更新、扩展字段保留、禁用和重新启用；`pkg/session/runtime/ttl_runtime_test.rs` 还包含 hook 提交/关闭事件、两个 manager 与接管、watermark/summary 等集成式场景。
- Go 对照：`pkg/ttl/ttlworker/timer.go`、`pkg/ttl/ttlworker/timer_sync.go`、`pkg/ttl/ttlworker/timer_test.go`、`pkg/ttl/ttlworker/integrationtest/timer_sync_test.go`；核对 hook 生命周期、守门、job 等待、timer 同步及延迟删除语义。
- 本任务为纯文档分析，按计划未运行 Cargo 或代码测试。交付验证只执行任务指定的 11 章节结构检查，并人工检查本说明能回答文件存在原因、运行流程和安全扩展位置。
