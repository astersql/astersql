# [`pkg/ddl/ddl.rs`](ddl.rs)

## 文件定位

`pkg/ddl/ddl.rs` 是 `astersql-ddl` crate 公开的 DDL 基础模型模块，由 `pkg/ddl/lib.rs` 中的 `pub mod ddl` 暴露。它集中定义轻量的 Job 数据模型、管理命令状态机、进程内 DDL 容器，以及恢复 DROP/TRUNCATE 元数据时使用的筛选函数。crate 边界由 `pkg/ddl/Cargo.toml` 的 `[package] name = "astersql-ddl"` 和 `[lib] path = "lib.rs"` 确认；该 manifest 还用 `package.metadata.porting.go-package = "pkg/ddl"` 标明 Go 对照包。

这个文件不是当前 Rust DDL 完整执行框架的总入口。生产模块会直接复用这里的 `Job`、`JobState` 和 `ActionType`（例如 `job_submitter.rs`、`job_scheduler.rs`、`job_worker.rs`），但仓库搜索未发现非测试代码构造本文件的 `Ddl`，也未发现非测试代码使用本文件的 `JobWrapper`、`recover_snapshot_ts` 或 `drop_or_truncate_table_info_from_jobs`。持久化提交、owner 调度、逐步执行和 schema diff 分别另在 `job_submitter.rs`、`job_scheduler.rs`、`job_worker.rs`、`schema_version.rs`；因此 `Ddl` 应理解为已测试的进程内语义模型，而不是 Go `ddl` 对象的完整生产移植。

## 核心职责

1. 定义跨 DDL 子模块共享的作业身份与状态：`Job`、`JobState`、`ActionType`、`CURRENT_VERSION`。
2. 表达创建对象时的名字冲突和 ID 分配策略：`OnExist`、`CreateTableConfig`、`create_table_config`。
3. 提供单次结果通知包装：`JobWrapper::new`、`notify_result`、`wait_result`。
4. 记录“变更已完成但 schema 尚未同步”和“本 owner 可能已执行过一次”的提示集合：`UnsyncedJobTracker`。
5. 用 `Ddl` 模拟启动、启停接单、MDL 开关、提交、完成归档，以及取消/暂停/恢复的状态转换；`process_jobs_transactionally` 为管理命令增加失败回滚边界。
6. 为恢复表流程提供时间戳选择与候选过滤：`recover_snapshot_ts`、`drop_or_truncate_table_info_from_jobs`。

它不负责 SQL AST 转 Job、系统表读写、owner 竞选、worker 线程、schema 版本同步、reorg/backfill 或 delete-range；这些能力不能从本文件的接口存在推断为已经接线。

## 主要符号

- `CURRENT_VERSION: i64 = 1`：`Job::new` 和 `Ddl::submit_job` 写入的当前内存 Job 版本。
- `StartMode::{Normal, Bootstrap, Upgrade}`：记录实例启动语境；默认 `Normal`。Go 还有 `BR` 模式，Rust 此枚举当前没有对应项。
- `OnExist::{Error, Ignore, Replace}` 与 `CreateTableConfig { on_exist, id_allocated }`：描述对象同名时行为和 ID 是否由调用方预分配。`create_table_config(None, ...)` 回退到 `Error`。
- `JobState::{None, Running, Paused, Cancelling, Cancelled, Done, Synced}`：作业生命周期状态。这里的管理接口只直接产生 `Running`、`Paused`、`Cancelling`、`Synced`；`Done`/`Cancelled` 由其他执行层状态机使用。
- `AdminCommandOperator::{User, System}`：保存暂停来源，并限制用户恢复系统暂停的 Job。
- `ActionType::{Other, DropTable, TruncateTable}`：只覆盖恢复候选筛选所需的动作子集，不等同于 Go `model.ActionType` 全集。
- `Job`：保存 `id`、原始 `query`、状态/版本、`start_ts`/`real_start_ts`、动作类型、schema/table ID 和 `paused_by`。`Job::new` 初始化为 `None`、当前版本、零时间戳与 `ActionType::Other`。
- `JobWrapper`：拥有一个 `Job`、`id_allocated` 和标准库 MPSC 通道。发送端使用 `Option::take`，使 `notify_result` 至多发送一次。
- `UnsyncedJobTracker`：以 `BTreeSet<i64>` 保存 `unsynced` 与 `already_run_once`。其方法需要 `&mut self` 才能写入，本身没有内部锁。
- `Ddl`：公开保存实例 ID、`Options`、启动/接单/TiFlash/MDL 开关、启动模式、按 ID 排序的进行中 Job、历史向量和跟踪器。
- `JobCommand::{Cancel, Pause, Resume}`：传给 `process_jobs`/`process_jobs_transactionally` 的管理命令。
- `recover_snapshot_ts`：非零 `real_start_ts` 优先，否则使用 `start_ts`。
- `drop_or_truncate_table_info_from_jobs`：遍历 DROP/TRUNCATE 候选，先检查 GC 安全点，再调用访问闭包，并支持成功短路。

## 执行流程

`Ddl` 的进程内流程如下：

1. `Ddl::new(id, options)` 调用 `options::apply_options` 顺序消费 functional options，建立未启动但默认允许接单、启用 TiFlash 轮询和 MDL 的空实例。
2. `start(mode)` 若已经启动则幂等成功；否则要求 `options.store` 已配置，保存模式并置 `started = true`。它没有创建线程、竞选 owner 或初始化系统表。
3. `submit_job(job)` 依次检查实例已启动且 `enabled`、ID 未重复；随后强制刷新 `version`、转为 `Running`、加入未同步集合，再插入 `jobs`。任一前置条件失败时不修改状态。
4. `process_jobs(ids, command, operator)` 按输入顺序逐项调用私有 `process_job`，所以返回向量和 ID 一一对应，单项错误不会中断后续 ID。合法转换为：`Running|Paused --Cancel--> Cancelling`、`Running --Pause--> Paused`、`Paused --Resume--> Running`；用户恢复系统暂停的任务被拒绝。
5. `process_jobs_transactionally` 先克隆 `jobs` 快照，再执行同一批状态转换和调用外部 `commit`。提交失败只恢复 `jobs`，并同时返回逐 Job 结果与提交结果；跟踪器和历史在当前命令路径不变，因此无需一并回滚。
6. `finish_job(id)` 先从 `jobs` 移除；存在时转为 `Synced`、清除未同步标记并追加到 `history`。ID 不存在时返回错误且不改变历史。
7. `all_jobs()` 返回进行中 Job（`BTreeMap` 的 ID 升序）后接历史插入顺序；它不是持久化系统表的全量查询。

恢复候选流程与上述容器独立：`drop_or_truncate_table_info_from_jobs` 跳过 `Other`，对每个 DROP/TRUNCATE 选择恢复快照；若 `gc_safe_point > snapshot_ts` 立即报错，否则调用 `visit`，闭包返回 `true` 时立即返回 `Ok(true)`，遍历完毕则 `Ok(false)`。

## 数据与状态

- `Ddl.jobs: BTreeMap<i64, Job>` 以 ID 唯一索引运行态 Job，并为 `all_jobs` 提供确定性排序；`history: Vec<Job>` 只保留本实例完成顺序，不去重、不持久化。
- `submit_job` 的关键不变量是：同一 `Ddl` 中进行中 ID 唯一；接受后的 Job 总是使用 `CURRENT_VERSION`、处于 `Running` 且出现在 `tracker.unsynced`。
- `finish_job` 的关键不变量是：同一 Job 不再同时存在于 `jobs` 与 `history`，归档状态固定为 `Synced`，对应未同步标志被移除。
- `paused_by` 只在 Pause 成功时设置，在 Resume 成功时清空；Cancel 不清空它。调用方若需要取消后的来源语义，不能假设该字段已重置。
- `process_jobs_transactionally` 的快照是完整 `BTreeMap` 克隆，成本与进行中 Job 数量及其中字符串大小成正比；它表达提交失败不泄漏暂存状态的语义，不是实际 KV 事务。
- `UnsyncedJobTracker.already_run_once` 没有 Go 版本的容量上限/重置逻辑；同目录 `job_scheduler.rs` 另有一个带 `RwLock<HashSet<_>>` 且容量受控的 `UnSyncedJobTracker`，两者是不同类型，不能混用。
- `tiflash_poll_enabled` 在本文件只初始化和保存，没有切换方法或消费方；`metadata_lock_enabled` 也只由 `switch_metadata_lock` 更新，不执行系统表检查或持久化。

## 依赖与调用关系

直接下游依赖很少：标准库 `BTreeMap`/`BTreeSet` 负责有序状态，`std::sync::mpsc` 负责结果通知；crate 内仅依赖 `options::{OptionFn, Options, apply_options}`。`Options` 可携带 store、etcd、info cache、lease、schema loader 等配置，但本文件只有 `start` 读取 `store.is_some()`，其他字段由别的模块负责。

已确认的 Rust 生产侧关系：

- `job_submitter.rs` 导入 `ddl::Job`，将其放入 `JobSpec`、持久化映射和 pending 队列。
- `job_scheduler.rs` 导入 `Job`/`JobState`，调度 `ScheduledJob` 并按 `Synced|Cancelled` 判断完成。
- `job_worker.rs` 导入 `ActionType`/`Job`/`JobState`，由 `transit_one_job_step` 推进 `None -> Running -> Done -> Synced` 或 `Cancelling -> Cancelled`。
- `schema_version.rs`、`ddl_history.rs` 等也复用 `Job`，形成 schema diff 与历史模型的共享数据边界。

上游方面，RustCodeGraph 对文件给出 28 个使用文件，并明确列出 `job_scheduler.rs` 及多份测试；精确 `query` 能定位 `submit_job`、`wait_result` 和恢复函数。精确 `callers/callees` 查询在本次环境中长时间无输出，故又用限定到非测试 Rust 文件的 `rg` 复核：`Ddl`、本文件 `JobWrapper` 及两个恢复函数没有生产调用点。注意 `pkg/ddl/systable` 还导出另一个来自 meta model 的 `JobWrapper`，不是这里的 MPSC 包装器。

## 错误处理与边界

- 所有业务错误都用 `String`，没有结构化错误类型或错误链；调用方若要按类别处理，只能依赖当前文本，不宜在新代码中扩大这种耦合。
- `start` 仅检查 store 是否存在；重复启动成功，`stop` 仅清 `started`，既不禁用 `enabled`，也不清任务或等待资源退出。
- `JobWrapper::notify_result` 忽略接收端已断开的发送错误；这是通知已无人等待时的有意吞弃。`wait_result` 在发送端全部关闭且未收到值时返回固定错误 `DDL result channel closed`。
- `submit_job` 在停止或禁用时统一返回 `DDL is not running`；重复 ID 返回包含 ID 的错误。
- `process_job` 对不存在 ID、非法状态转换以及权限冲突返回错误。Cancel 只接受 `Running|Paused`，Pause 只接受 `Running`，Resume 只接受 `Paused`。
- `process_jobs_transactionally` 不重试提交失败；逐 Job 状态处理可能全为 `Ok`，同时整体提交结果为 `Err`，调用方必须检查返回元组的两个部分。
- GC 判断采用严格的 `gc_safe_point > snapshot_ts`；相等时允许访问。首个过期候选立即终止，后续候选不会被访问。
- `visit` 只能返回布尔值，无法传播自身错误；这比 Go 恢复函数的 `(bool, error)` 回调能力弱。

## 并发与资源生命周期

`JobWrapper` 的 `mpsc::channel` 是本文件唯一直接并发原语。接收端 `wait_result` 会无限阻塞到收到一次结果或所有发送端关闭；没有超时、取消或异步接口。`notify_result` 通过取走唯一发送端保证至多一次通知，而不是 Go `JobWrapper` 对合并作业的多通道广播。

`Ddl`、`UnsyncedJobTracker` 及其集合没有 `Mutex`/`RwLock`，修改 API 需要 `&mut self`，预期由调用方串行拥有；本文件没有后台任务、owner 生命周期、session pool 或 shutdown join。相比之下，Go `ddl` 用 mutex、wait group、context cancellation、owner manager、submit loop 和多个 manager 管理资源；Rust 的 `job_scheduler.rs` 也另行使用线程与锁。本文件的 `start`/`stop` 因而只是状态门闩，不能被解释为完整资源生命周期。

快照回滚会克隆所有进行中 Job；大规模队列下可能产生内存和复制开销。恢复候选遍历为线性扫描，访问闭包可能短路，且本函数不持有锁或事务；若列表来自可变持久化状态，调用方负责提供一致快照。

## 与 Go 版本的对应关系

- Rust `StartMode` 对应 `pkg/ddl/ddl.go` 的 `StartMode`，但缺少 Go `BR`；Rust `start` 也未移植 Go 的 owner 强制切换、submit loop、session/system-table manager、delete-range manager、syncer 初始化等工作。
- `OnExist`/`CreateTableConfig` 对应 Go `OnExist`、`CreateTableConfig` 和 functional options。Go 注释说明 `OnExistReplace` 当前只支持 VIEW、预分配 ID 主要供 BR 使用；Rust 类型没有在自身逻辑中强制这两个约束。
- Rust `Job` 是 Go `model.Job` 的聚焦子集；动作枚举、错误、参数、binlog/history info、多 schema 信息等大量字段不在此处。
- Rust `JobWrapper` 对照 Go 同名类型，但 Go 保存 `model.JobArgs` 和一组结果通道，可向合并 Job 的多个等待者广播 job ID/merged/error；Rust 只向单个接收端发送 `Result<(), String>`。
- Rust `UnsyncedJobTracker` 对照 Go `unSyncedJobTracker`，但 Go 用 `RWMutex` 保护 map，并在 once map 超过容量时重置；Rust 本类型既不线程安全也不设容量。`job_scheduler.rs` 的另一跟踪器更接近这部分 Go 行为。
- Rust `switch_metadata_lock` 只是更新布尔值并报告是否变化；Go `SwitchMDL` 会检查运行中 Job、写全局变量和持久化元数据，且可能返回 session/KV 错误。
- Rust `process_jobs`/`process_jobs_transactionally` 对照 Go `CancelJobs`、`PauseJobs`、`ResumeJobs` 与底层 `processJobs` 的逐 Job 结果和事务提交语义。Rust 测试覆盖提交失败回滚，但没有 SQL 系统表查询、并发冲突重试和真正事务。
- Rust `all_jobs` 对应 Go “运行中后历史”的遍历意图，但它只读取内存集合；Go `GetAllDDLJobs`/`IterAllDDLJobs` 查询 job 表并继续遍历持久化历史。
- `recover_snapshot_ts` 精确对齐 Go `GetRecoverSnapshotTS` 的 `RealStartTS` 优先规则。候选过滤保留 DROP/TRUNCATE、GC 安全点和短路意图，但 Go 还从快照 InfoSchema 取表、区分可忽略错误，并允许回调返回错误。

## 扩展指南

- 新增 Job 状态或管理命令时，优先修改 `JobState`/`JobCommand` 与 `Ddl::process_job`，同时检查 `job_worker.rs::transit_one_job_step`、`job_scheduler.rs` 的完成判定和所有匹配分支。独立测试放在 `pkg/ddl/ddl_test.rs`、`pkg/ddl/cancel_test.rs` 或 `pkg/ddl/tests/adminpause/*_test.rs`，不要嵌回生产文件。
- 扩展 `Job` 字段或动作类型时，同步检查 `job_submitter.rs`、`job_scheduler.rs`、`job_worker.rs`、`ddl_history.rs`、`schema_version.rs` 的克隆/持久化/状态处理，并对照 Go `model.Job` 的序列化兼容性；不要仅提高 `CURRENT_VERSION` 而没有迁移策略。
- 若要把 `Ddl` 接入生产主链，必须先明确系统表持久化、owner failover、schema version 同步、MDL、reorg checkpoint、取消回滚与资源关闭行为。直接给当前内存容器增加调用点会绕过 `docs/agents/ddl/README.md` 所述的 job-based/owner-driven 不变量。
- 若增强事务语义，避免继续全量克隆队列；可将变更暂存为逐 Job delta，但必须保持“逐项结果可观察、整体提交失败不泄漏状态”的现有测试契约。
- 若增强恢复函数，需要决定是否对齐 Go 的 table snapshot 获取和回调错误传播，并在 `ddl_test.rs` 添加 GC 相等边界、首个过期候选、跳过非候选、回调错误/短路等用例。
- 若使跟踪器并发共享，应评估复用或合并 `job_scheduler.rs::UnSyncedJobTracker`，避免长期保留两个语义相近但容量和同步策略不同的类型。
- 性能风险主要在批量 `process_jobs` 的逐项查找、事务快照全量克隆与 `all_jobs` 的全量复制；兼容风险主要在 Job 状态/版本、管理权限和恢复时间戳规则。

## 验证依据

本说明基于以下直接证据，未运行 Cargo（任务明确为纯文档分析）：

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件，目标文件可读取。
- RustCodeGraph `node --file pkg/ddl/ddl.rs --offset 1 --limit 220` 与 `--offset 221 --limit 245`：核对目标文件全部 466 行、符号、字段、分支和文件使用概览。
- RustCodeGraph `query 'Ddl::submit_job'`、`query process_jobs_transactionally`、`query drop_or_truncate_table_info_from_jobs`、`query 'JobWrapper::wait_result'`：核对主要符号；其中通用名称查询会混入 Go/其他模块同名项，`process_jobs_transactionally` 未被索引查询命中。
- RustCodeGraph `node` 读取 `pkg/ddl/job_submitter.rs`、`pkg/ddl/job_scheduler.rs`、`pkg/ddl/job_worker.rs`：确认 `Job`/`JobState` 的生产使用及提交、调度、执行职责分布。精确 `callers/callees` 查询在本环境超过 30 秒仍无输出，已终止并用限定范围的源码搜索补证。
- 读取 `pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、`pkg/ddl/options.rs`：确认 crate、模块导出、移植元数据和直接配置依赖。
- 读取 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`：确认 DDL 的 schema 同步不变量及 job-based、owner-driven 总体约束；结论均再由 Rust 源码核对。
- 读取 Go 对照 `pkg/ddl/ddl.go`：核对 StartMode、配置、JobWrapper、tracker、NewDDL/Start/Stop、SwitchMDL、管理命令、全量遍历和恢复快照逻辑。
- 读取 Rust 测试 `pkg/ddl/ddl_test.rs`、`pkg/ddl/cancel_test.rs`、`pkg/ddl/tests/adminpause/pause_negative_test.rs`、`pkg/ddl/tests/adminpause/pause_resume_test.rs`，并搜索 `pkg/ddl/db_change_test.rs`：确认恢复时间戳/GC/短路、批处理顺序、状态转换、系统暂停权限、提交失败回滚、创建配置和完成归档契约。
- 用限定非测试文件的 `rg` 搜索主要类型与函数：确认 `Job`/`JobState` 被生产模块复用，同时当前 `Ddl`、本文件 `JobWrapper` 和恢复帮助函数没有非测试调用点。

人工复核结论：该文件存在的直接价值是提供共享 Job 模型与一组可测试的 Go 对齐语义；当前执行方式是串行内存状态转换和独立纯函数扫描；安全扩展必须同步检查真实 submitter/scheduler/worker，并保持独立测试与 Go 兼容边界。
