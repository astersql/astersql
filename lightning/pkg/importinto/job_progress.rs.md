# `lightning/pkg/importinto/job_progress.rs`

## 文件定位

本文说明的真实源文件是 [`job_progress.rs`](job_progress.rs)。它属于 Cargo crate `astersql-lightning-pkg-importinto`；crate 在 `lightning/pkg/importinto/Cargo.toml` 中声明为库，入口 `lightning/pkg/importinto/lib.rs` 以 `#[path = "job_progress.rs"] mod job_progress` 装配本模块并将其符号重导出。它位于 Lightning 的 `IMPORT INTO` 作业监控链中：`DefaultJobMonitor::WaitForJobs` 创建一个 `jobProgressEstimator`，`processJobStatuses` 每轮读取 SDK 状态后调用 `updateJobProgress`，把离散的 phase、step、percent 转换成每个作业的预计总字节数和已完成字节数（`lightning/pkg/importinto/job_monitor.rs`）。

本文件不是持久化进度、查询 SDK 或展示日志进度的组件；它只做纯计算、更新调用方提供的两个 `HashMap`，并在无法解析输入字符串时记录告警。实际状态轮询、终态统计和退出条件由 `job_monitor.rs` 负责。

## 核心职责

1. `updateJobTotalSize` 为作业选择一个可靠的总大小：优先保留缓存或 `ImportJob.TableMeta.TotalSize` 中的正值，二者都不可用时再依次尝试 `JobStatus.SourceFileSize` 和 `JobStatus.TotalSize`。
2. `isGlobalSortStatus` 通过 phase 或 step 识别全局排序路径；`updateJobProgress` 一旦识别成功就把估算器的 `isGlobalSort` 永久置为 `true`，避免后续较普通的状态使阶段模型回退。
3. `jobProgressPhases` 描述普通导入与全局排序的阶段/步骤表，`jobProgress` 将当前状态等权换算为 `[0, 1]` 的整体进度。
4. `estimateJobFinishedSize` 把比例换算为字节，保证运行中估值不倒退、完成时达到已知总量、失败或取消时保持先前值，并在总量已知时封顶。
5. `parseDockerHumanSize` 在 crate 内复刻 Go `docker/go-units.FromHumanSize` 对本路径需要的十进制单位行为；`estimate_progress_for_test` 则为 crate 内 parity 测试暴露稳定的进度计算入口。

## 主要符号

- `jobProgressEstimator { logger, isGlobalSort }`：crate 内可见的有状态估算器。`logger` 只用于解析失败告警；`isGlobalSort` 是跨轮询状态，初值为 `false`。
- `newJobProgressEstimator(logger)`：构造估算器，不读取外部状态，也不分配后台资源。
- `parseHumanSize(jobID, sizeText, warnMsg) -> (i64, bool)`：空串直接报告“不存在”；非空串调用 `parseDockerHumanSize`，失败时附带 `size`、`jobID` 和错误记录告警，并返回 `(0, false)`。
- `updateJobTotalSize(...) -> i64`：按缓存、表元数据、源文件大小、状态总大小的顺序求总量，只把正且发生变化的结果写回 `jobTotalSize`。
- `isGlobalSortStatus(status) -> bool`：识别 phase `global-sorting`、`resolving-conflicts`，或 step `encode`、`merge-sort`、`ingest`、`collect-conflicts`、`conflict-resolution`。
- `stepRatio(status) -> f64`：将百分数字符串除以 100 后夹在 `[0, 1]`；空串、`N/A` 或解析失败均为零，解析失败还会告警。
- `jobProgress(status) -> f64`：先定位 phase，再定位 step，按“阶段等权、阶段内步骤等权”的规则计算整体比例；未知 phase 返回零，未知 step 从当前阶段起点开始。
- `estimateJobFinishedSize(status, jobTotal, prevFinished) -> i64`：将状态和比例转换成单调、封顶的完成字节数。
- `updateJobProgress(...)`：组合入口，依次锁定全局排序模式、更新总量、读取旧完成量并写回新估值。
- `parseDockerHumanSize(sizeText) -> Result<i64>`：私有大小解析器，接受无后缀、`B`，以及 K/M/G/T/P 的裸前缀、`B`、`iB` 拼写，大小写不敏感且统一按 1000 进位；也接受 `1e3` 这类 `f64` 科学计数法。
- `jobProgressPhase`、`jobProgressPhases`、`findPhase`、`findStep`：内部阶段表及线性查找辅助符号。
- `estimate_progress_for_test(...)`：公开测试辅助函数，构造最小 `JobStatus` 后调用真实 `jobProgress`；生产监控不使用它。

## 执行流程

生产主链为 `DefaultJobMonitor::WaitForJobs` → `processJobStatuses` → `jobProgressEstimator::updateJobProgress`（`lightning/pkg/importinto/job_monitor.rs`）。`WaitForJobs` 为整个 group 初始化 `jobTotalSize`、`jobFinishedSize` 和一个估算器；每次轮询得到的每个已知作业状态都经过以下步骤：

1. `updateJobProgress` 先检查状态是否暴露全局排序 phase/step。命中后设置 `isGlobalSort = true`；该值不再被清零。
2. `updateJobTotalSize` 从 `jobTotalSize[jobID]` 开始，与正的 `TableMeta.TotalSize` 取较大值。仅当结果仍非正时才解析 `SourceFileSize`，仍非正才解析 `TotalSize`。
3. 从 `jobFinishedSize[jobID]` 取得旧值，缺失视为零，再调用 `estimateJobFinishedSize`。
4. 若状态为 finished 且总量为正，完成量直接等于总量；若 failed/cancelled，保持旧值；其他状态在总量为正时用 `jobProgress` 求估值并与旧值取较大值。
5. 已知总量时最终值不超过总量，结果写回 `jobFinishedSize[jobID]`。随后 `job_monitor.rs` 独立统计作业状态、记录完成作业并决定继续轮询还是返回。

进度比例的具体公式是：先取当前步骤比例 `ratio`，算 `phaseProgress = (stepIdx + ratio) / phase.steps.len()`，再算 `progress = (phaseIdx + phaseProgress) / phases.len()`；两层结果都做 `[0, 1]` 夹取。普通模式有 `importing/import`、`validating/post-process` 两阶段，因此 import 进行 50% 时整体为 25%。全局排序有四阶段，首阶段含两个步骤，因此 encode 进行 50% 时整体为 6.25%。

## 数据与状态

`jobProgressEstimator` 唯一会变化的内部字段是 `isGlobalSort`。该字段属于估算器而不是某个 `jobID`；当前调用方在一个 `WaitForJobs` group 内共享该实例，所以一旦任一轮状态证明该 group 使用全局排序，后续计算都使用四阶段模型。这与 Go 同名实现及“检测后不回退”的测试一致。

两个映射由调用方拥有，本文件仅通过可变借用原地更新：

- `jobTotalSize: HashMap<i64, i64>` 缓存每个作业已确认的正总量。正缓存不会被状态字符串中的更小值覆盖；正的表元数据可把现有值提高。
- `jobFinishedSize: HashMap<i64, i64>` 保存每个作业的累计估值。运行态通过 `max(prevFinished, estimate)` 保证不倒退；失败/取消保持旧值；已知总量时统一封顶。

`jobProgressPhase` 中的 phase 和 steps 都是静态字符串切片，`jobProgressPhases` 每次调用只构造很小的 `Vec`。计算使用 `f64`，最终估算通过 `(jobTotal as f64 * progress) as i64` 截为整数；该值随后受旧值下界和总量上界约束。`ProcessedSize` 未被 Rust 或 Go 的此实现读取，进度来源是阶段模型而非直接采用 SDK 的已处理字节数。

## 依赖与调用关系

- 上游生产调用者：`lightning/pkg/importinto/job_monitor.rs` 的 `WaitForJobs` 构造估算器，`processJobStatuses` 调用 `updateJobProgress`。RustCodeGraph 对目标文件报告测试文件使用关系；由于方法调用边未产出结果，生产边通过精确符号搜索核实。
- 上游测试调用者：`job_progress_test.rs` 直接访问 crate 内符号；`parity_test.rs` 通过 `estimate_progress_for_test` 验证跨实现契约。
- `crate::job_submitter::ImportJob` 提供 `JobID` 和可选 `TableMeta`；`crate::stubs::*` 提供当前 crate 的 `importsdk::JobStatus`、`log::Logger`、`zap` 字段、`mathutil::Clamp`、`Error/Result` 等边界替身。
- 标准库 `HashMap` 保存按 `JobID` 分区的总量和完成量。
- `Cargo.toml` 只声明 precheck、serde、serde_json、url、uuid；本文件看似使用的 importsdk/log/mathutil/zap 并非独立 Cargo 依赖，而是由本 crate 的 `stubs.rs` 提供。这是当前 Rust 移植边界，不能把它描述为已接入完整 TiDB SDK 或真实日志后端。
- 下游计算均为本地同步函数；文件不访问网络、磁盘、SQL 或 SDK。

## 错误处理与边界

- 空大小字符串是“无候选值”，不会记告警；非法非空字符串由 `parseHumanSize` 记录告警后降级为不可用，不中断监控。
- `parseDockerHumanSize` 对缺少数字、负数、未知前缀、过长或格式错误的后缀返回 `Error`。其调用包装会吞掉错误并继续尝试下一总量来源。
- `stepRatio` 对空串或 `N/A` 静默返回零；其他非法百分比记录告警并返回零。可解析但超界的百分比会被夹到 0 或 1。
- 空 phase 或未知 phase 使整体进度为零。已知 phase 下的未知 step 被视为该 phase 的起点，而不是错误。
- `jobTotal <= 0` 时运行态无法新增完成字节，finished 状态也不能凭空生成总量；旧值原样保留。总量为正时任何结果都不超过总量。
- failed/cancelled 不清零已完成量，也不继续推算；这使终态错误不会造成进度倒退。
- 阶段表中每个 phase 至少有一个 step，所以当前实现的除数非零；扩展阶段表时必须维持此不变量。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件句柄或事务。`jobProgressEstimator`、两个 `HashMap` 都在 `WaitForJobs` 的同步轮询栈中创建和销毁；`processJobStatuses` 以 `&mut` 借用串行更新它们，Rust 类型系统阻止同一时刻的未同步可变访问。

`log::Logger` 是传入估算器的克隆值；在当前 `stubs.rs` 中其消息缓冲区内部使用 `Arc<Mutex<_>>`，但本文件不管理锁生命周期。解析告警调用结束后不保留错误或输入借用。`jobProgressPhases` 返回的临时 `Vec` 在单次 `jobProgress` 返回时释放，内部字符串均为静态数据。若未来让多个轮询线程共享估算器，必须重新设计 `isGlobalSort` 和两张映射的同步边界，不能仅依赖当前的可变借用模式。

## 与 Go 版本的对应关系

`lightning/pkg/importinto/job_progress.go` 是直接语义基准。Rust 的构造器、总量来源优先级、全局排序识别集合、两套阶段表、百分比夹取、未知 phase/step 行为、完成量单调性以及 finished/failed/cancelled 分支均与 Go 对应实现一致。`job_progress_test.rs` 复刻 Go 的普通导入和全局排序主场景：普通模式 50% import 得到 0.25 和 25 MB，全局排序 50% encode 得到 0.0625，并验证检测全局排序后不回退；finished 状态缺少大小字段时仍用缓存总量完成到 100 MB。

实现形式有三点值得区分：

1. Go 使用 `units.FromHumanSize`，Rust 在本文件内以 `parseDockerHumanSize` 复刻其本路径语义，并额外由 `test_job_progress_estimator_scientific_notation_size` 固定 `1e3` → 1000 的兼容行为。
2. Go 方法接收 `*ImportJob` 并显式检查 nil；Rust 接收 `&ImportJob`，类型上排除了空 job，但仍保留 `TableMeta: Option<_>` 的缺失分支。
3. Rust 增加 `estimate_progress_for_test`，供 `parity_test.rs` 验证普通/全局排序和空 phase、`N/A` 百分比边界；Go 生产文件没有对应导出辅助函数。

当前 Rust crate 的 importsdk、日志及工具函数来自本地 `stubs.rs`，因此这里验证的是移植后的可观察计算契约，并不证明真实外部 SDK 类型或日志后端已完整接线。

## 扩展指南

- 新增或重命名服务端 phase/step 时，先更新 `isGlobalSortStatus` 的识别集合以及 `jobProgressPhases` 的顺序表；两者必须同步，否则可能先选错模式或把合法状态降为阶段起点。阶段顺序和步骤数量会直接改变历史进度比例，属于兼容性风险。
- 调整阶段权重时应修改 `jobProgress` 的等权公式，而不是只改字符串表，并在独立的 `job_progress_test.rs` 增加普通模式、全局排序、边界百分比、未知 phase/step 和单调性用例；不要把测试嵌入生产文件。
- 增加新的总量来源时应在 `updateJobTotalSize` 中明确优先级，维持“已知正值不被不可靠值降低”和“只缓存正值”的约束，同时与 Go `job_progress.go` 同步。
- 扩充大小单位解析时，应以 docker/go-units 的真实行为为基准修改 `parseDockerHumanSize`，覆盖大小写、空格、科学计数法、负数和非法后缀。不要改用 `stubs.rs::units::FromHumanSize` 而未验证其语义差异。
- 若作业 group 可能混用普通导入与全局排序，应先确认 `isGlobalSort` 的 group 级状态是否仍成立；若需按作业区分，应把模式状态按 `jobID` 分区，并同步修改 `job_monitor.rs` 生命周期与测试。
- 性能上当前阶段/步骤均为常量级小集合，线性查找和临时 `Vec` 成本很低；只有在阶段表显著增长或轮询频率大幅提高时才值得改为静态切片或索引映射，且要保持计算顺序不变。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter lightning/pkg/importinto` 确认目标、Go 镜像及测试均已索引；`node --file lightning/pkg/importinto/job_progress.rs` 读取完整 367 行并报告目标被 `job_progress_test.rs`、`parity_test.rs` 使用；对方法 callers/callees 的查询没有产出可用边，因此以精确符号搜索补证生产调用。
- 生产源码：`lightning/pkg/importinto/job_progress.rs`（估算器、解析器、阶段模型、测试辅助入口）、`lightning/pkg/importinto/job_monitor.rs`（构造、状态轮询与 `updateJobProgress` 调用）、`lightning/pkg/importinto/lib.rs`（模块装配、重导出及独立测试声明）、`lightning/pkg/importinto/stubs.rs`（当前 SDK、日志、错误与 clamp 边界）。
- crate 配置：`lightning/pkg/importinto/Cargo.toml`，确认 crate 名、库入口、Go 包映射和实际依赖集合。
- Go 对照：`lightning/pkg/importinto/job_progress.go`、`lightning/pkg/importinto/job_monitor.go`、`lightning/pkg/importinto/job_progress_test.go`。
- Rust 测试：`lightning/pkg/importinto/job_progress_test.rs` 验证普通导入、全局排序、完成态和科学计数法大小；`lightning/pkg/importinto/parity_test.rs` 验证核心比例以及空 phase、`N/A` 百分比边界。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查文档固定章节、范围和上述源码事实。
