# `lightning/pkg/importinto/job_orchestrator.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-importinto` crate 中 IMPORT INTO 作业的编排层，源码由 [`lib.rs`](lib.rs) 作为 `job_orchestrator` 模块装配并重导出。它位于导入器与三个可替换边界之间：上游 [`Importer::buildOrchestrator`](importer.rs) 构造它、[`Importer::runOnce`](importer.rs) 调用 `SubmitAndWait`，下游则分别由 [`JobSubmitter`](job_submitter.rs) 提交单表作业、[`CheckpointManager`](checkpoint.rs) 持久化恢复状态、[`JobMonitor`](job_monitor.rs) 等待远端作业结束，并通过 `importsdk::SDK` 查询和取消远端作业。

最近的 [`Cargo.toml`](Cargo.toml) 声明包名为 `astersql-lightning-pkg-importinto`、库入口为 `lib.rs`，移植元数据把它对应到 Go 包 `lightning/pkg/importinto`。清单中的显式外部依赖只有 precheck、`serde`、`serde_json`、`url`、`uuid`；本文件使用的 `context`、`importsdk`、`log`、`zap`、`failpoint` 等接口来自同 crate 的 [`stubs.rs`](stubs.rs) 重导出，因此这里描述的是当前 Rust crate 内的可观察行为，而不是独立上游 SDK crate 的实现细节。

## 核心职责

1. `NewJobOrchestrator` 归一化提交并发数、轮询周期和日志周期，按需创建默认 `JobMonitor`，再把各组件组装为 `Arc<dyn JobOrchestrator>`。
2. `SubmitAndWait` 分两阶段执行：先由 `submitAllJobs` 为各表跳过、恢复或新建作业并记录 checkpoint，再由 `JobMonitor::WaitForJobs` 监控已收集作业。
3. 提交或监控发生非上下文取消错误时，以独立的 `cancelTimeout` 后台上下文调用 `Cancel`，尽力清理已经进入远端的作业；清理错误只记录日志，原始提交/监控错误仍是主返回值。
4. `Cancel` 按 group key 查询组内全部作业，仅取消未完成作业，对可重试错误做有界指数退避，并把远端最终状态或本次取消结果回写到活跃表的 checkpoint。
5. 为“请求已经发出但父上下文随后取消”的竞态提供宽限上下文：已开始的提交仍有机会取得 Job ID，checkpoint 记录又获得一个新的独立宽限期，从而让后续清理可以定位远端作业。

## 主要符号

- `DefaultSubmitConcurrency = 10`、`DefaultPollInterval = 5s`、`DefaultLogInterval = 60s`：构造器在配置值无效（并发数 `<= 0`）或为零（两个周期）时采用的公开默认值。
- `submitGraceTimeout = 60s`：已经开始的提交以及随后记录 checkpoint 的默认宽限期；测试可通过 `failpoint::submit_grace_timeout_override` 覆盖。
- `cancelJobMaxRetry = 5`、`cancelJobRetryBaseBackoff = 100ms`、`cancelJobRetryMaxBackoff = 1s`：取消远端作业的次数与指数退避边界。
- `JobOrchestrator: Send + Sync`：公开抽象，仅暴露 `SubmitAndWait(ctx, tables)` 和 `Cancel(ctx)`；返回统一的 `Result<()>`。
- `DefaultJobOrchestrator`：默认实现，持有 `submitter`、`cpMgr`、`monitor`、`sdk`、日志器和并发配置；`activeJobs: Mutex<Vec<ImportJob>>` 保存本轮已提交或从 checkpoint 恢复的作业，供取消和 checkpoint 对账使用。
- `OrchestratorConfig`：依赖注入配置。`Monitor` 为 `None` 时才调用 `NewJobMonitor`；测试可注入脚本化 monitor。`ProgressUpdater` 只在创建默认 monitor 时向下传递。
- `NewJobOrchestrator`：公开工厂，返回 trait object，屏蔽默认实现细节。
- `submitAllJobs`：并发提交核心；返回所有已发现的 `ImportJob` 和第一个错误，而不是在首错后丢弃已经成功或恢复的作业。
- `recordSubmission`：要求 `ImportJob.TableMeta` 存在，并写入 `Running` checkpoint（表名、Job ID、group key）。
- `cancelJobsInGroup` / `cancelJobWithRetry` / `updateCheckpointsAfterCancel`：分别负责枚举与逐项取消、单项重试、checkpoint 对账。
- `SemGuard`：线程结束或提前返回时通过 `Drop` 归还一个并发令牌。
- `newStartedSubmitContext`、`newSubmissionRecordContext`：分别创建“从父上下文取消时刻开始计时”的提交宽限上下文，以及“不继承父取消、从创建时刻开始计时”的记录宽限上下文。
- `shouldRetryCancelJobErr`、`sleepWithContext`：前者把包含 `task not found` 的跨 keyspace 短暂竞态和 `common::IsRetryableError` 判定为可重试；后者用最多 2ms 的短睡眠模拟可被上下文中断的定时等待。

## 执行流程

### 构造与主入口

`Importer::buildOrchestrator` 创建 submitter，将 checkpoint manager、SDK、表并发配置、日志器和进度更新器传给 `NewJobOrchestrator`。工厂补齐默认值；没有注入 monitor 时调用 `NewJobMonitor`。随后 `Importer::runOnce` 在创建表、获取 table meta 和完成 precheck 后调用 `SubmitAndWait`。

`SubmitAndWait` 先调用 `submitAllJobs`。若提交阶段返回错误，它仍把已经收集的 jobs 写入 `activeJobs`；错误不是上下文取消时，再用后台超时上下文执行 `Cancel`。无论取消是否成功，最终都返回添加了 `submit jobs` 上下文的原提交错误。若没有可执行作业则直接成功；否则保存 `activeJobs` 并调用 `monitor.WaitForJobs`。监控的非取消错误同样触发尽力取消，最终返回 monitor 的结果。

### 单表提交与恢复

`submitAllJobs` 对每张表依次做以下处理：

1. `DataFiles` 为空或 `TotalSize == 0` 时只记录日志并跳过。
2. 自旋获取由 `submitConcurrency.max(1)` 初始化的令牌，然后为该表启动一个 OS 线程；`SemGuard` 保证线程退出时归还令牌。
3. 在线程内，用 `WithoutCancel(parent) + cancelTimeout` 查询表 checkpoint。这个查询不因兄弟任务首错或父取消立即停止，因此已有运行作业仍可被纳入后续清理。
4. checkpoint 为 `Finished` 时直接跳过；为 `Running` 且 Job ID 有效时，构造 `ImportJob` 加入结果集；没有 checkpoint 或状态为失败/取消时继续提交新作业。
5. 新提交前再次检查父 `ctx.Err()`。提交一旦开始，就改用 `newStartedSubmitContext`：父取消不会立刻中断它，而是在父取消后再给最多 `submitGraceTimeout`。
6. `SubmitTable` 成功后先把 job 放入共享结果集，再用全新的 `newSubmissionRecordContext` 调用 `recordSubmission` 写 `Running` checkpoint。即使记录失败，Job ID 仍保留在返回集合中，后续取消仍能发现它。
7. 每个线程只竞争写入 `first_err` 的第一个错误；主线程等待全部 handle，再返回 jobs 与首错。线程 panic 的 `join` 结果当前被忽略，详见边界章节。

### 取消与对账

`Cancel` 优先从 `activeJobs[0].GroupKey` 取组键，否则退回 `submitter.GetGroupKey()`；空键直接跳过。`cancelJobsInGroup` 先用 SDK 获取整组状态，把每项放入 `statusByID`，跳过 `IsCompleted()` 的作业，并对其余作业调用 `cancelJobWithRetry`。它继续处理后续作业，只保留第一个取消错误，同时在 `cancelledJobs` 记录成功发出取消的 Job ID。

随后 `updateCheckpointsAfterCancel` 只遍历 `activeJobs` 中具有 table meta 且 Job ID 为正的项：远端 finished 映射为 `Finished`；failed 映射为 `Failed` 并保留 `ResultMessage`；cancelled 映射为 `Failed / "cancelled by user"`；状态尚未刷新但本轮取消成功的作业也映射为同一失败消息。其他项不更新。所有 checkpoint 更新都会尝试，返回首个更新错误。若取消和更新同时失败，`Cancel` 优先返回取消错误并把更新错误写日志。

## 数据与状态

- `activeJobs` 是编排器跨方法共享的内存状态。提交成功、部分提交失败或从运行中 checkpoint 恢复的作业都会进入它；`Cancel` 用它把 SDK 的 Job ID 状态重新关联到表名。它不负责持久化，持久状态由 `CheckpointManager` 管理。
- `jobs`、`first_err` 和并发令牌均用 `Arc<Mutex<...>>` 在线程间共享。作业收集顺序取决于线程完成顺序，没有稳定排序保证；逻辑只依赖集合内容。
- checkpoint 的关键状态转换为：新作业提交成功后写 `Running`；取消对账时写 `Finished` 或 `Failed`。已为 `Finished` 的输入 checkpoint 不再提交；`Running + JobID > 0` 被恢复；其他状态触发重新提交。
- `statusByID` 保留查询时的组状态快照，`cancelledJobs` 表示本轮 `CancelJob` 已成功返回的 ID。二者共同区分“远端已完成”“远端明确失败/取消”和“取消请求已接受但状态还未刷新”。
- `GroupKey` 是批次边界：提交路径从 submitter/job 获取，取消路径以它查询整组远端作业，checkpoint 更新也保存同一 group key。

## 依赖与调用关系

上游真实调用链为 `Importer::buildOrchestrator -> NewJobOrchestrator`、`Importer::runOnce -> JobOrchestrator::SubmitAndWait`；`Importer::Run` 在外层上下文取消时也会用独立超时上下文调用 `JobOrchestrator::Cancel`。RustCodeGraph 同时确认 `NewJobOrchestrator -> NewJobMonitor`，以及 `SubmitAndWait -> submitAllJobs / JobMonitor::WaitForJobs / Cancel`。

下游边界如下：

- [`job_submitter.rs`](job_submitter.rs)：`SubmitTable` 创建单表远端作业，`GetGroupKey` 支持恢复作业和无 active job 时的组级取消。
- [`checkpoint.rs`](checkpoint.rs)：`Get` 决定跳过、恢复或重提；`Update` 记录 `Running` 以及取消后的最终状态。
- [`job_monitor.rs`](job_monitor.rs)：`WaitForJobs` 承担轮询与完成判定；构造器的 poll/log interval 和 progress updater 都传给这里。
- [`stubs.rs`](stubs.rs)：提供当前 crate 使用的 `context`、`importsdk::SDK`、错误、日志、failpoint 等兼容接口。
- [`importer.rs`](importer.rs)：提供生命周期入口，并在运行取消时触发清理。

RustCodeGraph 对本文件内部的关键边给出：`Cancel -> getGroupKey / cancelJobsInGroup / updateCheckpointsAfterCancel`，`cancelJobsInGroup -> cancelJobWithRetry`，`cancelJobWithRetry -> shouldRetryCancelJobErr / sleepWithContext`，`updateCheckpointsAfterCancel` 和 `recordSubmission` 均构造 `TableCheckpoint`。图对 trait object、闭包内动态调用和同名符号的解析不完整，因此 `CheckpointManager::Get/Update`、`JobSubmitter::SubmitTable/GetGroupKey` 与 SDK 调用以源码直接证据核验。

## 错误处理与边界

- 提交阶段返回的是首个线程错误，但仍等待所有已启动表线程完成并保留已收集 jobs；这保证兄弟失败时 checkpoint 恢复路径仍能参与清理。
- checkpoint 查询、提交和记录错误分别添加 `get checkpoint for db.table`、`submit table db.table`、`record submission for db.table` 上下文；顶层再为提交阶段添加 `submit jobs`。
- 上下文取消被视为调用者控制流：`SubmitAndWait` 不额外自动取消；普通提交/监控错误才进入后台清理。外层 `Importer::Run` 负责常规取消场景，并对 failover cancellation 特判为不取消远端作业。
- 取消最多尝试 5 次，退避为 100ms、200ms、400ms、800ms（上限 1s）；不可重试错误或最后一次错误立即返回。退避期间每至多 2ms 检查一次上下文。
- 查询 group 状态失败时无法确认远端结果，立即返回该错误；单个 job 取消失败不阻止其他 job；单个 checkpoint 更新失败也不阻止其他更新。
- `recordSubmission` 在缺少 `TableMeta` 时显式报 `missing table meta`。取消对账则跳过缺 meta、非正 Job ID 或既无可映射状态也未成功取消的 active job。
- 互斥锁使用 `lock().unwrap()`，锁中毒会 panic；`submitAllJobs` 又忽略线程 `join` 的 panic 结果，因此线程 panic 可能表现为未被记录的缺失作业而不是 `Result` 错误。这是当前实现事实，扩展时不应把它误述为已被错误通道覆盖。
- 当前 Rust `SubmitAndWait` 没有 Go 实现中 `FailAfterSubmission` failpoint 的对应分支。它支持的宽限期 override 是另一项 `setSubmitGraceTimeout` 语义；两者不可混同。

## 并发与资源生命周期

- `DefaultJobOrchestrator` 通过 `Arc<dyn ...>` 共享依赖，并由 trait 的 `Send + Sync` 约束允许跨线程持有；`activeJobs`、提交结果和首错均由 `Mutex` 保护。
- 并发限制是一个受互斥锁保护的计数器。调用线程在获取令牌前以 1ms 间隔等待，取得后才 spawn；`SemGuard::drop` 自动归还令牌。它限制同时运行的提交线程数，但不是异步 semaphore，也没有公平性保证。
- 每个 handle 都在返回前 `join`，所以 `submitAllJobs` 正常返回时提交工作线程不再存活；`SemGuard` 通过 RAII 归还令牌。需要注意，`checkpointCtx`、`submitCtx` 和 `recordCtx` 返回的 cancel function 只绑定为下划线变量，并未像 Go 的 `defer cancel()` 那样显式调用；闭包被丢弃不等于执行取消，因此相关 context/AfterFunc 是否继续到自身超时取决于 [`stubs.rs`](stubs.rs) 的实现。这是资源生命周期上的当前差异，不能假定已有主动清理。
- checkpoint 读取上下文与父取消解耦，但受 `cancelTimeout` 限制。已经开始的提交上下文在父取消后才启动宽限倒计时；若调用先完成，其返回的 cancel function 会停止 AfterFunc 并取消 grace context。
- checkpoint 记录上下文在提交完成后重新创建，直接获得完整的独立宽限期；这避免长时间提交耗尽记录阶段的时间预算。
- 自动清理使用 `context::Background() + cancelTimeout`，不复用已经失败/取消的请求上下文，确保取消请求仍有执行窗口。
- `activeJobs` 不在监控成功后清空；同一 orchestrator 的后续 `Cancel` 仍可看到最近一次非空提交。当前上游按 importer 生命周期复用该实例，修改复用方式时需评估陈旧状态风险。

## 与 Go 版本的对应关系

主要结构与 [`job_orchestrator.go`](job_orchestrator.go) 一一对应：相同的公开接口、配置字段、默认值、两阶段主流程、checkpoint 恢复规则、group 取消、五次指数退避、`task not found` 特判、取消后 checkpoint 映射，以及两种提交宽限上下文。Rust 用 `Arc<dyn Trait>` 对应 Go interface，用 `Arc<Mutex<_>>` 对应 Go 的共享切片保护，用线程加 `SemGuard` 对应 `errgroup.SetLimit`，并用短周期检查实现 Go `select { ctx.Done(), timer.C }` 的可中断退避。

需要注意的实现差异：

- Go `errgroup.WithContext` 在首错时取消 `egCtx`，尚未真正提交的 goroutine 会在 `egCtx.Err()` 处退出；Rust 不建立首错派生上下文，而是让所有已调度线程继续到父 `ctx.Err()` 检查，因此兄弟错误本身不会阻止后续新提交。源码注释强调仍调度每张表以保留 checkpoint-resume 清理语义，但“无 checkpoint 的兄弟表是否应继续提交”存在调度层面的差异，修改时应以 Go 测试意图为准重新核对。
- Go `activeJobs` 是普通切片并依赖调用时序；Rust 用 `Mutex<Vec<_>>` 支持 trait 的 `Sync`。可观察的 group key 优先级和 checkpoint 对账语义保持一致。
- Go `recordSubmission` 是接收者方法；Rust 是接受 `Arc<dyn CheckpointManager>` 的模块私有函数，行为仍是写 `Running` checkpoint。
- Go 的 `newStartedSubmitContext` 用 timer 和 `select`，完成时能立即观察 grace context 已结束；Rust `AfterFunc` 回调直接 sleep 到宽限期，cancel function 通过 `stop()` 尝试终止回调。文档只确认对外测试覆盖的“父取消后短期仍存活”与“最终取消”行为，不推断底层线程实现完全等价。
- Go 在 checkpoint 查询、提交和 checkpoint 记录作用域结束时均 `defer cancel()`；Rust 当前只保存 `_cancelCheckpoint`、`_cancel`、`_cancelRecord` 而不调用，因此不能认为具有同样的提前释放时机。
- Go `SubmitAndWait` 含 `FailAfterSubmission` failpoint；当前 Rust 没有。两端的 `setSubmitGraceTimeout` 均有对应测试。

独立 Rust 测试 [`job_orchestrator_test.rs`](job_orchestrator_test.rs) 与 Go 测试 [`job_orchestrator_test.go`](job_orchestrator_test.go) 对齐覆盖：空输入、成功提交、已完成跳过、运行中恢复、失败后重提、提交/监控失败清理、部分提交仍记录与取消、恢复作业在兄弟失败后仍取消、提交和记录各自的宽限期、finished/running 取消对账、无 active job 的组级取消、`task not found` 重试，以及上下文可中断退避。

## 扩展指南

- 增加新的提交决策或 checkpoint 状态时，优先修改 `submitAllJobs` 的 checkpoint 分支，并同步 [`checkpoint.rs`](checkpoint.rs) 的状态定义与 [`job_orchestrator_test.rs`](job_orchestrator_test.rs) 中跳过/恢复/重提场景。不要把 Rust 测试嵌入生产源文件。
- 改变错误清理策略时，同时检查 `SubmitAndWait`、`Cancel` 和 [`Importer::Run`](importer.rs)：三者分别处理内部普通错误、显式组取消和上游上下文取消/故障转移。需要保持“主错误优先、清理错误记录”和 failover 不取消的兼容契约。
- 扩展取消状态映射时，在 `updateCheckpointsAfterCancel` 增加明确分支，并为远端状态、`cancelledJobs` 尚未刷新状态、更新失败三类路径补独立测试；避免把未知状态误写成成功。
- 修改并发模型时必须保留三项不变量：所有已经获得的 Job ID 都进入结果集；checkpoint 恢复作业不能因兄弟失败而遗失；函数返回前所有启动的工作单元已收敛。还应处理当前被忽略的线程 panic，并评估作业顺序无保证这一事实。
- 修改宽限期时同时维护 `newStartedSubmitContext` 和 `newSubmissionRecordContext`，但不要合并二者的计时起点；对应 Rust 测试为 `test_job_orchestrator_submit_grace_starts_after_context_cancel` 与 `test_job_orchestrator_record_submission_gets_fresh_grace_timeout`。
- 若要补齐 Go 的 `FailAfterSubmission`，应作为明确的移植任务加入 `SubmitAndWait` 在保存 `activeJobs` 之后、调用 monitor 之前的位置，并移植 Go 测试，而不能在本文档任务中宣称现有 Rust 已支持。
- 新增真实外部 Rust 依赖时应遵守仓库规则：在独立上游仓库移植并发布 tag，本仓库只引用统一 tag；本文件当前依赖均通过本 crate 接口注入，不应把临时本地副本复制进来。

兼容性风险主要在上下文取消时机、首错后的兄弟提交、group key 选择和 checkpoint 状态映射；性能风险主要在每表 OS 线程、1ms 获取令牌轮询、2ms 取消退避轮询以及 `activeJobs`/结果向量的互斥锁竞争。任何调整都应优先保持 Go 的可观察语义，而不是仅让 Rust 测试通过。

## 验证依据

- 生产源码：[`job_orchestrator.rs`](job_orchestrator.rs) 的 `NewJobOrchestrator`、`SubmitAndWait`、`Cancel`、`submitAllJobs`、`cancelJobWithRetry`、`updateCheckpointsAfterCancel`、`recordSubmission` 和两个宽限上下文构造函数。
- 上游与模块边界：[`importer.rs`](importer.rs) 的 `buildOrchestrator`、`Run`、`runOnce`；[`lib.rs`](lib.rs) 的模块装配；[`Cargo.toml`](Cargo.toml) 的 crate/Go package 元数据与依赖声明。
- Go 对照：[`job_orchestrator.go`](job_orchestrator.go) 的同名接口、结构、工厂、提交、取消、重试和宽限期函数。
- 测试证据：[`job_orchestrator_test.rs`](job_orchestrator_test.rs) 的 9 个独立测试，以及 [`job_orchestrator_test.go`](job_orchestrator_test.go) 的 8 个同族测试；Rust 额外显式验证取消退避可被上下文快速中断。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；查询确认 `NewJobOrchestrator -> NewJobMonitor`、`SubmitAndWait -> submitAllJobs / WaitForJobs / Cancel`、`Cancel -> getGroupKey / cancelJobsInGroup / updateCheckpointsAfterCancel`、`cancelJobWithRetry -> shouldRetryCancelJobErr / sleepWithContext`，以及 `Importer` 中的直接上游调用。图对闭包和动态 trait 调用不完整的部分已用上述源码补证。
- 本任务是纯文档分析，按计划不运行 Cargo；验收以固定章节结构检查和人工事实复核为准。
