# `lightning/pkg/importinto/job_monitor.rs`

## 文件定位

本文件属于 Cargo crate `astersql-lightning-pkg-importinto`，crate 入口是同目录的 `lib.rs`，并由该入口以 `mod job_monitor` 加载、再公开重导出。它位于 IMPORT INTO 的“提交后等待”阶段：`job_orchestrator.rs` 的 `NewJobOrchestrator` 在调用方未注入自定义监控器时构造 `NewJobMonitor`，`DefaultJobOrchestrator::SubmitAndWait` 在所有作业提交成功后调用 `JobMonitor::WaitForJobs`。因此本文件不负责创建或取消作业；它负责观察同一组已提交作业、汇总进度、把终态写入 checkpoint，并把成功或首个错误交还编排器。编排器随后决定是否取消剩余作业。

`Cargo.toml` 将该 crate 标为映射 Go 包 `lightning/pkg/importinto` 的 library；监控逻辑直接使用的 `importsdk`、日志、context、errors 等类型由本 crate 的 `stubs.rs` 暴露，而不是在本文件单独声明外部 Cargo 依赖。目标文件没有条件编译项。

## 核心职责

1. 通过公开 trait `JobMonitor` 隔离作业编排器与具体轮询实现，便于测试或调用方注入替身。
2. 以输入作业的第一个 `GroupKey` 周期性调用 `importsdk::SDK::GetJobsByGroup`，只处理 `JobID` 存在于本次输入集合中的状态，忽略同组历史作业。
3. 对每次成功查询得到的状态重算 pending/running/finished/failed/cancelled 数量和导入行数；同时委托 `jobProgressEstimator::updateJobProgress` 维护每个作业的总字节与已完成字节，并向可选 `ProgressUpdater` 发布组级总量。
4. 每个受跟踪作业首次进入完成态时，调用 `recordCompletion` 写入表级 checkpoint，并通过 `logJobCompletion` 记录终态。
5. 实现快速失败：失败作业立即导致返回错误；取消、checkpoint 更新失败等由 `processJobStatuses` 产生的错误也不会等待其余作业。SDK 查询的暂时错误则仅告警并继续下一轮。

## 主要符号

- `pub trait JobMonitor: Send + Sync`：公开抽象边界，唯一方法 `WaitForJobs(&self, ctx, jobs) -> Result<()>`。`Send + Sync` 允许监控器以 `Arc<dyn JobMonitor>` 在编排器及其调用环境间共享。
- `pub struct DefaultJobMonitor`：默认实现，持有共享 SDK、checkpoint manager、轮询/日志间隔、logger，以及可缺省的进度更新器。字段均为私有，构造统一经过 `NewJobMonitor`。
- `struct groupStats`：单次成功轮询的内部快照，保存五类状态计数及 `totalImportedRows`；它不是跨轮次累加器，`WaitForJobs` 只保留最近一次快照用于日志和退出判断。
- `pub fn NewJobMonitor(...) -> Arc<dyn JobMonitor>`：注入依赖并隐藏具体实现类型，不校正零间隔；默认间隔由上游 `NewJobOrchestrator` 处理。
- `DefaultJobMonitor::WaitForJobs`：主循环。建立按 `JobID` 索引的输入、尺寸和完成集合，处理取消、定时日志、轮询、快速失败与全体完成。
- `logProgress`：把最近一次快照写为组级结构化日志，不改变状态。
- `processJobStatuses`：每轮状态处理核心；过滤非本批作业、更新进度、分类计数、首次终态落 checkpoint，并返回本轮统计与首个错误。
- `logJobCompletion`：按作业 ID 以及可用的库表名派生 logger，分别记录 finished、failed、cancelled。
- `recordCompletion`：要求 `ImportJob::TableMeta` 存在，构造 `TableCheckpoint`；成功映射为 `CheckpointStatus::Finished`，失败或取消都映射为 `CheckpointStatus::Failed`，最后调用 `CheckpointManager::Update`。

## 执行流程

`WaitForJobs` 的路径如下：

1. 输入为空时立即成功，不访问 SDK、checkpoint 或 progress updater。
2. 将输入复制到 `jobMap`，从存在的 `TableMeta` 初始化 `jobTotalSize`，取 `jobs[0].GroupKey` 作为整组查询键，并创建空的 `finishedJobs` 与进度估算器。
3. 记录开始等待日志。`last_poll` 和 `last_log` 从当前时刻开始，因此不会立刻轮询；主循环每次先检查 `ctx.Err()`，然后在日志间隔到达时输出最近统计。
4. 未到轮询间隔时睡眠 5 ms 后重试；到期后调用 `sdk.GetJobsByGroup`。查询失败只告警并继续，下一次查询仍须等待新的完整 `pollInterval`。
5. 查询成功后，`processJobStatuses` 遍历状态。未知 `JobID` 被忽略；受跟踪状态先更新尺寸估算和统计。已在 `finishedJobs` 中的作业仍参与统计与进度估算，但不重复写 checkpoint 或重复记录完成日志。
6. 新完成作业先加入 `finishedJobs`。失败或取消生成首个业务错误；随后仍尝试写 checkpoint。checkpoint 失败在尚无业务错误时成为首个错误，已有业务错误则保留原错误。
7. 一轮结束时按输入作业集合汇总尺寸，并依次调用 `UpdateTotalSize`、`UpdateFinishedSize`。
8. 回到 `WaitForJobs` 后，若本轮存在 failed 状态，立即返回已获得的首错；异常情况下若未形成具体错误，则生成组级兜底错误。否则，全体作业已完成时成功返回，或返回 checkpoint/取消等首错；未达终止条件则继续轮询。

## 数据与状态

- `jobMap: HashMap<i64, ImportJob>` 界定本次监控范围，也是过滤同组旧作业的依据。重复 `JobID` 会以后出现的输入覆盖先前值，而完成目标仍使用原始 `jobs.len()`；调用方应保证作业 ID 唯一。
- `jobTotalSize` 初始来自 `TableMeta.TotalSize`。`jobProgressEstimator` 可在其非正时从状态的 `SourceFileSize` 或 `TotalSize` 补全，并以已知最大值为准。
- `jobFinishedSize` 跨轮次保存每个作业的估算完成字节。估算器对运行态取历史值与当前估值的最大值，对成功终态提升到总量，对失败/取消保留历史值，并限制不超过总量，所以正常输入下组级进度不会倒退。
- `finishedJobs` 是“已观察到终态”的去重集合，不等同于 checkpoint 已成功持久化：代码在调用 `recordCompletion` 前插入 ID。checkpoint 更新失败会让本轮立即返回错误，因此当前调用不会重试该 checkpoint。
- `groupStats` 每轮从零构造，仅统计 SDK 本轮返回且属于 `jobMap` 的状态。若 SDK 暂时漏掉某个受跟踪作业，该作业不计入该轮分类和行数，但仍保留此前的尺寸状态与完成集合。
- 所有输入被假定属于同一个 group；实现只使用第一个作业的 `GroupKey`，不会逐个校验其余作业。

## 依赖与调用关系

上游主链是 `NewJobOrchestrator` → `NewJobMonitor`，以及 `DefaultJobOrchestrator::SubmitAndWait` → `JobMonitor::WaitForJobs`。RustCodeGraph 对 `SubmitAndWait` 的 callees 查询明确给出到 `job_monitor.rs:91` 的调用边；目标文件的文件节点还显示它被 `job_orchestrator.rs`、`job_monitor_test.rs`、`job_orchestrator_test.rs`、`lib.rs` 和 mock parity 测试引用。

主要下游关系为：

- `WaitForJobs` → `importsdk::SDK::GetJobsByGroup`：取得组状态。
- `WaitForJobs` → `processJobStatuses` → `jobProgressEstimator::updateJobProgress`：维护单作业尺寸与单调进度。
- `processJobStatuses` → `ProgressUpdater::{UpdateTotalSize, UpdateFinishedSize}`：发布组级字节进度。
- `processJobStatuses` → `recordCompletion` → `CheckpointManager::Update`：持久化表名、作业 ID、组键、终态和错误消息。
- `recordCompletion` → `common::UniqueTable`：生成 checkpoint 使用的唯一表名。
- 各阶段 → `log::Logger`/`zap`：记录开始、周期进度、查询错误、完成结果和退出原因。

`Cargo.toml` 没有为本文件定义 feature；crate 仅直接列出 precheck、serde、serde_json、url、uuid，监控器所见的 Go 风格运行时接口来自 crate 内 `stubs.rs`。这意味着理解运行边界时应以这些 trait/类型的当前 Rust 定义为准，不能把 Cargo 列表误读成真实 TiDB SDK 的直接依赖关系。

## 错误处理与边界

- context 已取消时，循环顶部立即返回 `ctx.Err()`；独立 Rust 测试还验证了首次轮询前取消会胜过已经准备好的 SDK 响应。
- `GetJobsByGroup` 错误被视为可恢复，只记录 warning 并无限重试，直到后续成功或 context 取消；没有错误次数上限。
- failed 状态生成 `job {id} failed: {ResultMessage}`，cancelled 生成 `job {id} was cancelled`。若统计发现 failed 却没有具体错误，则构造包含 group key 和失败数的兜底错误。
- `recordCompletion` 在缺少 `TableMeta` 时返回 `missing table meta`；checkpoint manager 的错误被包装为 `update checkpoint`。缺失元数据在作业运行期间不会报错，只在首次终态持久化时暴露。
- 非成功完成态统一写 `CheckpointStatus::Failed`；取消没有独立 checkpoint 状态，消息取 `status.ResultMessage`，而返回给调用方的取消错误不使用该消息。
- SDK 返回未知作业会被安全忽略。反之，若某个输入作业永远不出现在状态响应且 context 不取消，完成集合永远达不到输入长度，监控会持续运行。
- `pollInterval == 0` 或 `logInterval == 0` 时本文件不拒绝输入；前者会持续查询，后者会每轮记录进度。常规构造路径由 orchestrator 填充默认值，直接调用 `NewJobMonitor` 的调用方需自行保证合理间隔。

## 并发与资源生命周期

监控器内部没有 mutex、channel 或异步任务；一次 `WaitForJobs` 在当前线程同步执行，并以 `std::thread::sleep(5 ms)` 做间隔等待。`sdk`、`cpMgr` 和 `progressUpdater` 用 `Arc` 共享，trait 的 `Send + Sync` 只保证它们可跨线程持有，并不使同一个 `WaitForJobs` 调用并行处理状态。

Rust 实现没有创建需显式释放的 ticker：两个 `Instant` 控制轮询和日志时机，函数返回即释放局部 maps、set 和估算器。睡眠以 5 ms 粒度检查 context，因此取消响应可能最多受到该睡眠及正在执行的同步 SDK/checkpoint 调用影响；context 不能抢占这些调用。日志计时与轮询计时互相独立，慢下游调用会使实际周期延长而不会补发错过的 tick。

虽然 `DefaultJobMonitor` 可被多个线程通过 `Arc` 同时调用，但每次调用的可变跟踪状态都是栈上局部数据；共享依赖是否允许并发调用由其各自 trait 实现负责。本文件不持有后台线程，也没有 drop 时的额外清理动作。

## 与 Go 版本的对应关系

Rust 文件逐段对应同目录 `job_monitor.go`：trait/interface、默认实现字段、`groupStats`、构造器、轮询主循环、状态处理、完成日志和 checkpoint 写入顺序基本一致。`job_monitor_test.rs` 也复现了 Go 测试的空输入、成功、失败、快速失败、忽略旧作业、查询错误后恢复和跨状态进度不回退场景。

需要明确的实现差异有：

- Go 用两个 `time.Ticker` 和 `select` 同时等待 context、日志 tick、轮询 tick；Rust 用 `Instant` 加 5 ms sleep 模拟，并专门保证首次查询发生在完整轮询间隔之后。可观察结果相近，但事件同时到达时的选择机制和计时精度不同。
- Go 注入 `SlowDownPolling` failpoint；Rust 源码明确将它实现为 no-op。因此依赖该 failpoint 的调试/测试行为尚未移植，不能声称完全支持。
- Go 的 `logJobCompletion` 和 `recordCompletion` 直接解引用 `job.TableMeta`；Rust 日志在元数据缺失时仍可工作，而 `recordCompletion` 返回明确错误，避免 panic。
- Rust 用 `Arc<dyn ...>`、owned `ImportJob` clone、`HashMap`/`HashSet` 表达 Go interface、指针和 map 的相同共享/去重语义。
- Rust 独立测试增加了“首次轮询前取消”的显式断言，用来固定其 ticker 模拟与 Go 的预期一致；其余场景与 Go 测试意图对齐。

## 扩展指南

- 新增状态类别或改变终态规则时，集中修改 `processJobStatuses` 的分类、`IsCompleted` 后的错误构造和 `recordCompletion` 的 checkpoint 映射，并同步 `job_monitor_test.rs` 与 Go 对照测试；不要只改日志分支。
- 新增进度算法时，优先修改 `job_progress.rs` 的 `jobProgressEstimator`，保留 `processJobStatuses` 作为聚合边界，并验证总量补全、上界和不回退不变量。相关独立测试是 `job_progress_test.rs` 与本文件的多作业进度场景。
- 若引入重试策略，应区分 SDK 查询错误、checkpoint 持久化错误和业务失败。目前只有 SDK 查询错误重试；改变 checkpoint 重试时必须同时调整 `finishedJobs` 的插入时机，避免“已去重但未持久化”。
- 若支持混合 group 输入，应在进入循环前校验或按 group 分组，不能继续复用 `jobs[0].GroupKey`。还应为重复 JobID 和响应缺项确定清晰契约，防止完成条件不可达。
- 若将同步轮询改为 async/ticker，须保留“首次 tick 前可取消”、查询错误后继续、周期日志、无重复 checkpoint 更新和编排器快速取消链，并评估高频轮询、日志量与取消延迟。
- 测试逻辑必须继续放在独立的 `job_monitor_test.rs`，不得嵌入生产源文件。Rust 行为变更还应核对 `job_orchestrator_test.rs` 中 monitor 错误触发取消的契约，以及同路径 Go 文件/测试的语义。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用（11,467 files、307,296 nodes）；`node --file lightning/pkg/importinto/job_monitor.rs` 读取了 401 行目标源码并列出五个使用文件；`query` 同时定位 Rust/Go 的 `NewJobMonitor`、`WaitForJobs`，以及 Go 的私有辅助方法；`callees SubmitAndWait` 给出 Rust `job_orchestrator.rs:210` 到 `job_monitor.rs:91` 的调用边。针对 `NewJobMonitor`/`WaitForJobs` 的直接 callers/callees 查询未单独输出边，因此未把缺失的图边当作事实。
- 生产源码：`lightning/pkg/importinto/job_monitor.rs`（全部符号和分支）、`job_orchestrator.rs`（默认构造、等待与失败后取消）、`job_progress.rs`（总量补全、阶段估算、单调性与上界）、`lib.rs`（模块装配和公开重导出）。目标包没有 `doc.go`。
- crate 边界：`lightning/pkg/importinto/Cargo.toml` 的 package、lib、porting metadata 和 dependencies。
- Go 对照：`lightning/pkg/importinto/job_monitor.go`，核对了接口、字段、轮询顺序、错误策略、状态映射和 checkpoint 更新。
- 独立测试：`lightning/pkg/importinto/job_monitor_test.rs` 与 `job_monitor_test.go`，覆盖空作业、首次轮询前取消（Rust）、成功、失败、快速失败、旧作业过滤、SDK 暂时错误及进度不回退。本任务是纯文档分析，按计划未运行 Cargo 或代码测试；交付验证仅检查文档结构与人工事实对应。
