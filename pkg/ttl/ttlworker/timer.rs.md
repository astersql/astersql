# `pkg/ttl/ttlworker/timer.rs`

## 文件定位

本文件是 `astersql-ttl-ttlworker` crate 的 TTL 定时触发适配层，由 [`lib.rs`](lib.rs) 通过 `pub mod timer` 暴露。它把“某张逻辑表/物理表的定时事件”转换为 `TtlJobAdapter` 上的可提交性检查、作业提交与状态查询，并用 `TimerResponse` 把结果表达为重试、运行中或关闭事件。当前 Rust 生产代码中，`JobManager` 实现了这里的适配接口，但仓库搜索未发现生产路径实例化 `TtlTimerHook` 或 `TtlTimerRuntime`；二者目前主要由 [`timer_test.rs`](timer_test.rs) 直接验证。因此它是可用的领域适配基线，而不是 Go 版 timer runtime 的完整接线。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-ttl-ttlworker`，入口为 `lib.rs`。本文件只直接导入同 crate 的 `job_manager::TtlSummary`，没有直接使用 Cargo 中的外部依赖。

## 核心职责

- `TtlJobAdapter` 隔离定时器逻辑与实际作业管理器，使钩子只依赖“是否可提交、提交、查询、取当前时间”四项能力。
- `TtlTimerHook::on_event` 在每日调度窗口内执行提交门禁，并以定时事件的 `event_id` 作为作业请求 ID、以 `created_at` 作为提交时间水位。
- `TtlTimerHook::poll` 查询同一请求；未结束时继续返回 `Submitted`，结束后生成 `TtlTimerSummary` 并返回 `Closed`。
- `within_window` 实现普通窗口和跨午夜窗口的闭区间判断。
- `TtlTimerRuntime` 仅保存幂等的启停布尔状态；它没有创建线程、注册 timer hook 或持有 timer store。

这些职责由 `timer.rs:44-165` 的 trait、钩子和运行时实现直接给出；作业层的实际约束由 [`job_manager.rs`](job_manager.rs) 中 `impl TtlJobAdapter for JobManager` 提供。

## 主要符号

- `TtlTimerSummary { last_job_request_id, last_job_summary }`：事件关闭时保留最近请求 ID 和可选 `TtlSummary`。作业已结束但无汇总时，`last_job_summary` 可以是 `None`。
- `TtlJobTrace { request_id, finished, summary }`：适配器返回的作业快照。`finished == false` 表示仍需轮询；`summary` 不由本文件强制与 `finished` 保持组合约束。
- `TtlJobAdapter`：公开 trait。`submit_job` 需要可变借用，其他三个方法只需共享借用；所有失败统一为 `String`。
- `TimerEvent`：一次触发的值对象。`table_id` 标识逻辑表，`physical_id` 标识实际表或分区；`event_id` 才是当前协议使用的请求 ID。`request_id` 是兼容旧调用方的保留字段，在本文件执行路径中不读取。
- `TimerResponse`：`Retry`、`Submitted(TtlJobTrace)`、`Closed(TtlTimerSummary)` 三态结果。
- `TtlTimerHook<A>`：泛型钩子，公开持有适配器以及一天内的起止分钟。分钟值的文档约定为 `0..1440`，构造时没有运行时校验。
- `TtlTimerHook::on_event`：提交入口，返回 `Result<TimerResponse, String>`。
- `TtlTimerHook::poll`：状态查询入口，返回相同响应类型。
- `within_window`：私有窗口判定函数，边界包含起止分钟。
- `TtlTimerRuntime`：只有公开的 `running: bool`；`Default` 为暂停，`resume`/`pause` 分别写入 `true`/`false`。

## 执行流程

`on_event` 的流程如下：

1. 调用 `adapter.now()` 取得 Unix 秒，并通过 `((now / 60) % 1440) as u16` 折算为 UTC/纪元意义上的当天分钟；本接口不携带时区。
2. 调用 `within_window(minute, start, end)`。当 `start <= end` 时接受闭区间 `[start, end]`；当 `start > end` 时接受 `minute >= start || minute <= end`，即跨午夜区间。
3. 窗口不匹配时短路返回 `Ok(Retry)`，不会调用 `can_submit_job`；窗口匹配但 `can_submit_job(table_id, physical_id)` 为假时也返回 `Ok(Retry)`。
4. 门禁通过后调用 `submit_job(table_id, physical_id, event.event_id, event.created_at)`，成功值映射成 `Submitted`，错误原样传播。

`poll` 的流程如下：

1. 使用 `table_id`、`physical_id` 和 `event_id` 调用 `get_job`，查询错误通过 `?` 原样传播。
2. 若 `trace.finished` 为假，返回携带原快照的 `Submitted`。
3. 若已结束，返回 `Closed`；摘要中的请求 ID取自事件的 `event_id`，作业汇总取自 `trace.summary`。这里不会校验 `trace.request_id` 是否与事件一致。

运行时状态流程独立于上述作业流程：`TtlTimerRuntime::default()` 的 `running` 为假，任意次数 `resume` 后为真，任意次数 `pause` 后为假。

## 数据与状态

本文件没有全局可变状态。`TtlTimerHook` 的持久状态只有适配器和两个分钟边界；一次事件的身份与水位均由调用方传入。提交时使用 `event_id` 而非 `TimerEvent::request_id`，这是 [`timer_test.rs`](timer_test.rs) 中 `submission_uses_go_event_id_and_event_start_watermark` 明确锁定的协议。关闭摘要同样记录 `event_id`，由 `closed_summary_records_go_event_id` 覆盖。

`JobManager` 的适配实现为这些抽象赋予实际状态语义：`can_submit_job` 要求当前节点为 leader、物理表存在且逻辑表 ID 匹配、TTL 已启用、该物理表没有活跃作业；`submit_job` 建立 `request_id -> (physical_id, job_id)` 映射并锁定新作业；`get_job` 从活跃作业或历史记录构造进行中/已完成快照。相应证据位于 `job_manager.rs` 的 `impl TtlJobAdapter for JobManager`。

时间值均为无类型时区信息的 `u64` Unix 秒。`now / 60` 舍弃秒级部分，`created_at` 不在本文件中解释或修改，直接传入适配器。

## 依赖与调用关系

- 模块装配：`lib.rs -> pub mod timer`，使外部 crate 可经 `astersql_ttl_ttlworker::timer` 访问公开符号。
- 下游类型：`timer.rs -> crate::job_manager::TtlSummary`，用于 trace 和关闭摘要。
- 适配实现：`job_manager.rs -> TtlJobAdapter, TtlJobTrace`，`JobManager` 是仓库内找到的生产实现。
- 测试调用：`timer_test.rs` 直接构造 `TtlTimerHook`/`TtlTimerRuntime`；`job_manager_integration_test.rs` 导入 trait 以验证 `JobManager` 门禁和提交行为。
- 生产接线现状：对公开类型名的仓库搜索未发现测试之外的 `TtlTimerHook`、`TtlTimerRuntime`、`TimerEvent` 或 `TimerResponse` 使用点。不能据此声称本文件已经接入正在运行的 timer 框架。

RustCodeGraph 对 `timer.rs` 的索引给出 21 个符号；精确查询能定位 `on_event`、`poll`、`within_window`、`TtlTimerHook` 和 `TtlTimerRuntime`，但 `callers`/`callees` 未返回这些方法的可用静态边，因此以上调用关系使用模块声明、trait 实现和精确仓库引用补证。

## 错误处理与边界

- `on_event` 的窗口拒绝和容量/冲突拒绝不是错误，而是 `Ok(Retry)`；只有 `submit_job` 的失败进入 `Err(String)`。
- `poll` 的 `get_job` 失败立即返回 `Err(String)`，本文件不重试、不记录日志、不分类错误。
- 普通窗口与跨午夜窗口均包含两端。`start == end` 时只允许该一个分钟值，并不表示全天。
- `schedule_start_minute` 与 `schedule_end_minute` 是公开字段且无构造器校验；若调用方传入大于等于 1440 的值，实际分钟永远无法等于该值，可能形成意外窗口。
- `u64` 时间避免负值，但极大的秒值在除法和取模路径中仍安全；提交水位完全由适配器解释。
- `poll` 假设调用方只在已提交事件上使用；适配器找不到请求时，`JobManager::get_job` 返回 `"TTL request not found"`。
- 该 Rust 版本没有 Go 版的全局 TTL enable 门禁、timer data JSON 解码、十分钟未提交关闭、timer 删除/事件 ID 变化检测、关闭失败重试或日志记录。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、ticker 或取消令牌。`on_event` 需要 `&mut self`，因此同一个 hook 的提交操作必须由调用方串行化或置于外部同步原语后；`poll` 只需 `&self`，但实际能否并发取决于适配器实现。trait 没有 `Send`/`Sync` 上界。

`TtlTimerRuntime` 的 `running` 是普通 `bool`，只提供值级幂等，不提供线程安全，也没有资源启动/停止副作用。与之相比，Go `ttlTimerHook` 持有 `context.CancelFunc`、`sync.WaitGroup` 和 ticker 驱动的等待 goroutine，`Stop` 会取消并等待；Go `ttlTimerRuntime` 会构造、启动和停止 `TimerGroupRuntime`。因此 Rust 的 `pause`/`resume` 目前不能被解释为完整资源生命周期管理。

## 与 Go 版本的对应关系

直接对照文件是 [`timer.go`](timer.go)，相关 Go 测试是 [`timer_test.go`](timer_test.go)。类型层面，`TtlTimerSummary`、`TtlJobTrace`、`TtlJobAdapter` 分别对应 Go 的 `ttlTimerSummary`、`TTLJobTrace`、`TTLJobAdapter`；Rust 保留了表/物理表、请求 ID、完成标志和摘要这些核心数据。

已经对齐的关键语义包括：

- 作业请求 ID 使用 timer event ID，而不是旧请求字段。
- 提交水位使用 event start/`created_at`。
- 提交前检查调度窗口和 `CanSubmitJob`；闭区间及跨午夜行为由 Rust 测试固定。
- 作业完成时把 event ID 和可选作业摘要写入 timer 摘要。

尚未等价移植的部分也很重要：Go 将预调度与正式调度分成 `OnPreSchedEvent`/`OnSchedEvent`，通过 timer client 关闭事件并写入 watermark/JSON summary；后台 goroutine 周期轮询并处理 timer 删除、事件变化、取消及瞬时错误；runtime 注册 hook factory 并真实启动 timer group。Rust 将其压缩为同步 `on_event` 与由外部驱动的单次 `poll`，`TimerResponse::Closed` 也只是返回值，不会自行持久化关闭事件。因此扩展时应以 Go 行为作为兼容目标，不能把当前轻量实现当成完整替代品。

## 扩展指南

- 若增加提交门禁，优先修改 `TtlTimerHook::on_event`，并在独立的 `timer_test.rs` 增加窗口外、适配器拒绝、适配器错误及短路顺序用例；不要把测试嵌入生产文件。
- 若改变事件身份或水位协议，必须同时检查 `TimerEvent`、`on_event`、`poll`、`TtlTimerSummary`，以及 `JobManager` 的 `request_to_job` 映射；重点防止分区表的 `table_id`/`physical_id` 混用。
- 若引入完整 timer 框架接线，需要新增明确的 store/client/hook 生命周期，而不是仅扩展 `running` 布尔值；同步覆盖启动、重复启停、取消、轮询错误、事件被删除或替换、关闭事件失败重试。Go `ttlTimerHook::{OnPreSchedEvent, OnSchedEvent, waitJobFinished}` 与 `ttlTimerRuntime::{Resume, Pause}` 是直接行为基准。
- 若保留分钟配置，应增加合法范围校验并定义 `start == end` 的产品语义；任何语义变化都需要普通窗口、端点、跨午夜及相等端点测试。
- 若增强错误类型，需同时修改 trait、`JobManager` 实现及测试适配器；当前公开 API 使用 `String`，变更会影响 crate 使用者。
- 性能风险主要在未来轮询频率和并发任务数；当前同步方法自身没有循环。兼容风险主要来自时区、窗口边界、event ID/watermark 和关闭摘要格式。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ttl/ttlworker` 确认目标及相邻文件被索引；`node --file pkg/ttl/ttlworker/timer.rs --offset 1 --limit 500` 读取完整 165 行；`query TtlTimerHook`、`query TtlTimerRuntime`、`query can_submit_job`、`query on_event --kind function`、`query within_window --kind function` 核对主要符号。对精确方法执行的 `callers`/`callees` 查询无可用输出，已通过引用搜索补证而未臆造调用边。
- 源码与装配：[`timer.rs`](timer.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 直接生产实现：[`job_manager.rs`](job_manager.rs) 中 `impl TtlJobAdapter for JobManager`。
- Rust 独立测试：[`timer_test.rs`](timer_test.rs) 覆盖幂等启停、闭区间边界、跨午夜、event ID、水位和关闭摘要；[`job_manager_integration_test.rs`](job_manager_integration_test.rs) 覆盖 leader/TTL/表身份门禁和作业唯一性。
- Go 对照：[`timer.go`](timer.go) 与 [`timer_test.go`](timer_test.go)，用于确认完整 hook/runtime 生命周期及未移植分支。
- 人工复核结论：本文件存在于 timer 事件与 TTL 作业管理之间；当前运行方式是外部调用同步提交并显式轮询；安全扩展必须同步 trait 实现和独立测试，并特别保护分区身份、调度窗口、event ID 与 watermark 协议。
