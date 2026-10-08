# `pkg/ttl/ttlworker/persistent.rs`

## 文件定位

本文件位于 `astersql-ttl-ttlworker` crate，由 [`lib.rs`](lib.rs) 以 `pub mod persistent` 公开。它是 Rust TTL worker 的 SQL 持久化边界：把表级 TTL job 的领取、租约续期、超时接管和完成状态写入 `mysql.tidb_ttl_table_status`、`mysql.tidb_ttl_job_history` 与 `mysql.tidb_ttl_task`。系统表是跨节点协调的权威状态，内存 job 映射只承担 worker 侧缓存，见模块注释和 `PersistentJobStore`。

crate 清单 [`Cargo.toml`](Cargo.toml) 将该包映射到 Go 包 `pkg/ttl/ttlworker`。本文件直接使用同 crate 的 `session`、`job`、`job_manager` 模块，并通过唯一的常规外部依赖 `astersql-ttl-cache` 编解码扫描范围；清单中大量额外依赖仅在 `cfg(windows)` 下声明，不是本文件当前接口的直接依赖。

## 核心职责

- `PersistentJobStore::start_job` / `start_job_with_ranges`：在一个悲观事务中锁定物理表状态行，检查并发 job 和调度间隔，写入当前所有者，创建 history 行，并持久化一个或多个扫描任务。
- `PersistentJobStore::heartbeat`：只有 durable 状态中的 `table_id + job_id + owner_id` 全部匹配时才续租，避免旧 owner 延长已被接管的 job。
- `PersistentJobStore::takeover_timeout` / `takeover_timeout_for_job`：对心跳过期的状态行执行无等待加锁并转移 owner；定时器事件可额外限定预期 job ID。
- `PersistentJobStore::finish_job`：验证完成者仍持有 job 后，原子地把 current 状态转成 last 状态、删除扫描任务并完成历史摘要。

这些操作只管理 job 元数据，不扫描或删除用户数据；实际调度、扫描和删除主链位于 [`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 的 `run_ttl_tick_inner`。

## 主要符号

- `pub struct PersistentJobStore`：无字段的命名空间类型，本身不持有连接或缓存；所有状态都显式来自 `&mut dyn WorkerSession` 和系统表。
- `takeover_timeout(session, table_id, new_owner_id, now, timeout_seconds) -> Result<Option<String>, SessionError>`：通用接管入口，委托给 `takeover_timeout_for_job(..., None)`；`Some(job_id)` 表示接管成功，`None` 表示没有符合条件的过期 job。
- `takeover_timeout_for_job(..., expected_job_id) -> Result<Option<String>, SessionError>`：可按当前 timer event 的 job ID 限制接管，先锁行再更新 owner 与心跳。
- `heartbeat(...) -> Result<bool, SessionError>`：返回 `true` 表示所有权仍有效；返回 `false` 表示 job/owner 不再匹配；SQL 执行失败则返回错误。
- `finish_job(..., summary: &TtlSummary, summary_text: &str) -> Result<(), SessionError>`：完成当前 job。结构化计数来自 `TtlSummary`，状态表中的 JSON/文本摘要由调用方以 `summary_text` 提供。
- `start_job(...) -> Result<bool, SessionError>`：便捷入口，使用 `astersql_ttl_cache::table::newFullRange()` 创建单个全范围扫描任务。
- `start_job_with_ranges(..., ranges, scan_index_id) -> Result<bool, SessionError>`：生产主入口，接受 Region/索引拆分后的范围；`false` 是 TTL 禁用、已有当前 job 或调度间隔未到等正常“不领取”结果。
- `fn start_job_in_transaction(...)`：私有事务体，包含状态行创建与锁定、竞争检查、过期水位计算、history/task 写入。

## 执行流程

新 job 的流程如下（`start_job_with_ranges`、`start_job_in_transaction`）：

1. `table.ttl_enabled == false` 时直接返回 `Ok(false)`，不启动事务。
2. 执行 `BEGIN PESSIMISTIC`，用 `SELECT ... FOR UPDATE NOWAIT` 锁定 `table.physical_id` 对应的状态行；不存在时用 `INSERT_NEW_TABLE_INTO_STATUS_SQL` 创建后重新加锁，仍不存在则报错。
3. 状态行已有 `current_job_id` 时返回 `false`；若指定 `schedule_interval_seconds`，且 `last_job_start_time` 晚于 `now - interval`，也返回 `false`。
4. 以 `PhysicalTable::expire_time(now)` 计算过期水位，条件更新 current job、owner、心跳、开始时间和 `running` 状态；随后用 `SELECT ROW_COUNT()` 强制确认恰好更新一行。
5. 写入 `tidb_ttl_job_history` 的 running 记录；按输入顺序枚举 `ranges`，用 `EncodeDatums` 编码边界，并以从 0 开始的 `scan_id` 写入 `tidb_ttl_task`。可选 `scan_index_id` 随每个任务保存。
6. 事务体返回 `true` 才提交；正常未领取执行回滚；错误路径也回滚。提交或回滚生命周期失败时调用 `avoid_reuse`，防止污染的池化会话再次使用。

续租时，`heartbeat` 先做带 job/owner 条件的更新，再读取 `ROW_COUNT()`。MySQL 对“新旧时间相同”的更新可能报告 0 changed rows，因此它会再查询所有权：匹配行仍存在时仍返回 `true`，而不是误判租约丢失。

接管时，`takeover_timeout_for_job` 在悲观事务中选择 `current_job_owner_hb_time < FROM_UNIXTIME(now - timeout_seconds)` 的行并使用 `FOR UPDATE NOWAIT`；`expected_job_id` 存在时加入等值条件。选中后只改变 owner 与 owner heartbeat，保留 job ID、原开始时间、过期水位及已有 task/checkpoint，以便恢复执行。

完成时，`finish_job` 先按 `table_id + job_id + owner_id` 条件把 current 字段迁入 last 字段并清空 current 字段，要求 `ROW_COUNT() == 1`；随后删除该 job 的 task，更新 history 的完成时间、摘要、计数和 `finished` 状态，最后统一提交。

## 数据与状态

- `mysql.tidb_ttl_table_status`：每个物理表的互斥锁与当前/上一次 job 摘要。`physical_id` 用作 `table_id`，逻辑表 ID 写入 `parent_table_id`，因此分区表可以独立领取 job，又能追溯父表。
- `mysql.tidb_ttl_job_history`：记录 job 身份、表名、分区名、创建/完成时间、TTL 过期水位、处理计数与最终状态。创建时 `finish_time` 使用 `FROM_UNIXTIME(1)` 作为未完成占位。
- `mysql.tidb_ttl_task`：每个扫描范围一行；范围端点由 `EncodeDatums` 序列化为 bytes，`scan_id` 由输入切片顺序决定，索引扫描时保存 `scan_index_id`。
- 时间参数均以 `u64` Unix 秒进入 `FROM_UNIXTIME`；超时阈值、调度最早时间和过期水位使用 `saturating_sub`，避免小时间戳发生无符号下溢。
- `PersistentJobStore` 无进程内状态，因此重启或换节点后仍以系统表为准。接管只更新 owner，正是恢复已持久化 scan range/cursor 的基础。

关键不变量是：同一物理表最多一个 current job；只有匹配的 owner 才能续租和完成；status、history 与初始 task 要么一起提交，要么一起回滚；完成操作也必须整体提交。

## 依赖与调用关系

上游生产调用集中在 [`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs)：

- `run_ttl_tick_inner` 先调用 `takeover_timeout_for_job` 尝试恢复过期 job；没有可接管 job 时拆分扫描范围并调用 `start_job_with_ranges`。
- `JobHeartbeat::start` 在独立 SQL session/线程中周期调用 `heartbeat`；扫描批次回调还会同步核验一次所有权。
- 全部扫描任务成功后，`run_ttl_tick_inner` 聚合 `TtlSummary` 和 JSON 摘要并调用 `finish_job`。

直接下游包括：

- [`session.rs`](session.rs) 的 `WorkerSession::execute`、`avoid_reuse`、`Datum`、`SessionError`、`PhysicalTable::expire_time`；
- [`job.rs`](job.rs) 的 `REMOVE_TASK_FOR_JOB_SQL`；
- [`job_manager.rs`](job_manager.rs) 的 `INSERT_NEW_TABLE_INTO_STATUS_SQL` 与 `TtlSummary`；
- `astersql_ttl_cache::table::{ScanRange, newFullRange}` 和 `astersql_ttl_cache::task::EncodeDatums`。

RustCodeGraph 能定位目标文件、上述类型和下游 `execute` 调用，但当前索引没有为关联函数形式的 `PersistentJobStore::...` 生成有效 callers 结果；上游调用边因此由精确源码搜索核验，而不是据此推断。

## 错误处理与边界

- 所有 SQL/编码错误通过 `SessionError` 向上传播；范围编码的字符串错误被映射成 `SessionError::Execute`。
- `Ok(false)`/`Ok(None)` 是可预期的协调结果，不是错误：分别表示未取得新 job 或没有可接管 job。
- `FOR UPDATE NOWAIT` 不等待其他节点释放锁；锁冲突由会话层作为错误返回，让上层本轮放弃而非重复完成昂贵的范围计算。
- status 行插入后仍无法锁到、领取 UPDATE 未恰好影响一行、完成时所有权已变化，均视为状态一致性错误。
- `heartbeat` 特意区分 “affected rows 为 0 但所有权仍匹配” 与真正丢失所有权，以兼容相同秒内重复心跳。
- `finish_job` 的 owner 条件比 Go `finishJobTemplate` 的 `table_id + job_id` 更严格，可阻止被接管前的旧 owner 完成 job；对应回归在 `ttl_worker_session_test.rs` 中明确验证。
- rollback 本身失败时保留原始业务错误，但标记会话不可复用；正常未领取分支若 rollback 失败，则把 rollback 错误返回。commit 失败同样标记不可复用。
- 本文件不重试 SQL，也不自行处理锁冲突；重试/下一轮调度由上层 runtime 生命周期决定。

## 并发与资源生命周期

跨节点互斥完全由系统表事务提供：领取/接管先锁状态行，领取还用 `current_job_id IS NULL` 的条件更新和 `ROW_COUNT()` 做第二道竞争校验。`NOWAIT` 使竞争者立即失败，避免多个 manager 同时为一张表持有 job。

心跳线程使用与扫描、删除会话分离的 SQL session，防止长扫描或限速等待饿死 lease；一旦 `heartbeat` 返回 `false` 或错误，运行时设置 `lost` 并停止该 job。`PersistentJobStore` 自身不创建线程、不持有锁，也不管理连接池；借用的 `WorkerSession` 生命周期完全由调用方控制。

事务生命周期由每个公开复合操作显式管理。task/history/status 不应拆到不同事务，否则宕机可能留下“已领取但无任务”或“已完成但任务仍可执行”的不一致状态。扩展任何事务步骤时，要保持所有提前返回都经过统一 commit/rollback 分派，并继续在事务终结失败时调用 `avoid_reuse`。

## 与 Go 版本的对应关系

[`job_manager.go`](job_manager.go) 的 `lockNewJob` 对应 Rust 的 `start_job_with_ranges`/`start_job_in_transaction`：二者都在悲观事务中锁/创建状态行、校验是否可调度、计算过期水位、写 history，并为拆分范围逐一写 task。Rust 将已由运行时生成的 ranges 作为参数传入；Go 在事务回调内选择索引或主键范围。

Go 的 `lockHBTimeoutJob` 对应 `takeover_timeout_for_job`：均锁定状态行并保留既有 job 身份与水位后更换 owner。Rust 的 timer-event 入口额外用 `expected_job_id` 防止陈旧事件接管另一 job，专用测试 [`persistent_test.rs`](persistent_test.rs) 验证不匹配事件不会更新 owner。

Go 的 `updateHeartBeatForJob` 与 Rust `heartbeat` 都用 owner 条件保护续租。Rust 还显式查询所有权以处理 MySQL “相同时间值导致 changed rows 为 0” 的情况。Go 方法还负责 job 总超时后的汇总和完成；Rust 此文件不承接该上层策略。

[`job.go`](job.go) 的 `ttlJob.finish` 对应 Rust `finish_job` 的三步顺序：迁移 status、删除 task、完成 history，且都放在悲观事务中。Rust 额外校验 owner，摘要文本则由运行时构造后同时写入 status/history；Go 的 `TTLSummary` 自带 `SummaryText`。

因此这是行为对齐后的 Rust 持久化实现，但不是 Go `JobManager` 的逐字段复制：缓存刷新、通知 scan manager、本地 runningJobs 管理、超时策略和 failpoint 仍属于各自上层，不应误写成本文件已实现。

## 扩展指南

- 新增 job 状态字段：同步修改 `start_job_in_transaction` 的初始化 SQL、`finish_job` 的迁移/清理 SQL，以及系统表 schema/真实 SQL adapter；至少扩展 [`ttl_worker_session_test.rs`](../../session/runtime/ttl_worker_session_test.rs) 验证跨会话可见性和完成结果。
- 新增扫描范围元数据：在 `start_job_with_ranges` 参数和 task INSERT 中接线，保持 `scan_id` 稳定；扩展 [`ttl_runtime_test.rs`](../../session/runtime/ttl_runtime_test.rs) 的 `go_merge_43_ttl_persists_region_scan_ranges` 并覆盖恢复读取。
- 修改所有权或接管条件：同时检查 `takeover_timeout_for_job`、`heartbeat`、`finish_job` 三处条件，避免出现能接管却不能续租/完成或旧 owner 仍能完成的组合；同步独立的 [`persistent_test.rs`](persistent_test.rs) 和会话级竞争测试。
- 修改事务步骤：新增失败点测试，确认错误时 status/history/task 全部回滚，并确认 commit/rollback 失败会 `avoid_reuse`。不要把 Rust 单元测试内嵌回本源文件；本 crate 已通过 `lib.rs` 的 `#[cfg(test)] mod persistent_test` 保持生产代码与测试分离。
- 修改调度间隔语义：保留 Go `couldLockJobForCreate` 的时间边界意图，特别核对严格比较符和 `saturating_sub`；扩展同一会话级测试中的“过早拒绝/间隔后允许”用例。
- 性能上，ranges 数量决定同一事务内 INSERT 次数和锁持有时间；批量化或提前计算必须在不破坏原子性、scan ID 顺序和 Go 行为的前提下评估。

## 验证依据

- 目标实现：[`persistent.rs`](persistent.rs)，完整检查了 `PersistentJobStore` 的 6 个公开关联函数、1 个私有事务函数、全部 SQL 分支和事务终结路径。
- crate/模块边界：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`session.rs`](session.rs)、[`job.rs`](job.rs)、[`job_manager.rs`](job_manager.rs)。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ttl/ttlworker` 确认目标与独立测试均被索引；`node --file ...persistent.rs`、`node PersistentJobStore`、`node WorkerSession`、`node TtlSummary`、`callees heartbeat`、`callees finish_job` 和 `callees start_job_in_transaction` 用于核对符号及直接下游。
- 生产调用边：精确搜索确认 [`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 调用 `takeover_timeout_for_job`、`start_job_with_ranges`、`heartbeat` 和 `finish_job`。
- Rust 测试：[`persistent_test.rs`](persistent_test.rs) 覆盖 timer event 精确接管；[`ttl_worker_session_test.rs`](../../session/runtime/ttl_worker_session_test.rs) 覆盖跨会话持久化、并发领取、心跳、接管、旧 owner 完成失败、最终清理与调度间隔；[`ttl_runtime_test.rs`](../../session/runtime/ttl_runtime_test.rs) 覆盖周期心跳丢失所有权、Region ranges 持久化和持久游标/多范围恢复。
- Go 对照：[`job_manager.go`](job_manager.go) 的 `lockNewJob`、`lockHBTimeoutJob`、`updateHeartBeatForJob`，以及 [`job.go`](job.go) 的 `ttlJob.finish`。
- 本任务为只新增说明文档，按计划未运行 Cargo。最终结构检查要求本文恰有 11 个规定的二级标题。
