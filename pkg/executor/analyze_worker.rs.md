# `pkg/executor/analyze_worker.rs`

## 文件定位

本文件属于 `astersql-executor` crate。crate 根在 [`pkg/executor/Cargo.toml`](Cargo.toml) 中以 `lib.rs` 声明，[`pkg/executor/lib.rs`](lib.rs) 通过 `pub mod analyze_worker;` 将本模块公开，并仅在测试构建中装配独立测试模块 `analyze_worker_test`。

它描述的是 ANALYZE 管线末端的“统计结果保存 worker”：接收已经计算完成的分区或表统计结果，委托运行时把结果持久化，结束对应分析作业，并释放结果载荷。它直接对照 [`pkg/executor/analyze_worker.go`](analyze_worker.go) 的同名 worker。

需要特别区分“已实现模块”和“已接入生产主链”：仓库内 Rust 引用搜索只发现 `lib.rs` 的模块声明与 [`pkg/executor/analyze_worker_test.rs`](analyze_worker_test.rs) 的测试调用，未发现 Rust 生产代码调用 `newAnalyzeSaveStatsWorker` 或 `analyzeSaveStatsWorker::run`。当前 Rust ANALYZE 生产实现仍由 [`pkg/executor/analyze.rs`](analyze.rs) 的 `AnalyzeExec::handleResultsErrorWithConcurrency` 自行创建保存线程并调用 `analyzeRuntime::save_analyze_result`。因此本文件当前是可测试的移植边界，还不是 Rust ANALYZE 主链的实际保存入口。

## 核心职责

- 用 `analyzeSaveStatsWorker` 持有单消费者结果通道、错误发送端和抽象运行时，并由 `run` 顺序消费结果。
- 每个结果先检查 kill 信号，再执行持久化；成功或失败都通过运行时结束对应 `analyzeJob`，并把结果交回资源池。
- kill 错误会令 worker 进入 drain 模式：后续结果只做作业失败收尾和资源回收，不再检查 kill 或写存储，从而持续排空通道，避免上游生产者因发送阻塞而死锁。
- 普通保存错误不会进入 drain 模式；后续分区仍获得保存机会，允许出现“部分统计已落盘”的结果，与 Go 实现注释和行为一致。
- 整个消费循环由 `catch_unwind` 包裹；panic 被转换为领域错误并通过错误通道上报，而不是跨越 worker 边界继续展开。
- 借助 `report_once` 保证每个 worker 最多上报一个错误；后续错误仍会完成日志、作业收尾和资源释放，但不会再次占用错误通道。

## 主要符号

- `AnalyzeSaveError(pub String)`：本模块统一错误值；实现 `Display` 与 `std::error::Error`。它是轻量字符串包装，不保留结构化错误类型或来源链。
- `AnalyzeSaveResult<T = ()>`：运行时操作的 `Result<T, AnalyzeSaveError>` 别名。
- `analyzeContext { requestID }`：保存调用的最小请求上下文。当前 worker 只透传给 `save_analyze_result_to_storage` 和保存失败日志，不自行读取字段。
- `analyzeJob { id, databaseName, tableName, partitionName }`：作业收尾所需的库、表、分区标识。本文件只把 `Option<&analyzeJob>` 交给运行时。
- `analyzeResults { job, tableID, count, payload }`：通道中的拥有型结果载荷。`run` 会以 `&mut analyzeResults` 调用保存接口，最后把所有权移交 `destroy_and_put_to_pool`。
- `statsMetaHistorySource::Analyze`：写存储时固定传递的历史统计来源标记；本文件目前仅有此枚举值。
- `analyzeSaveStatsRuntime: Send + Sync`：把 kill 检查、存储、作业收尾、资源回收和日志隔离为可替换边界。`Send + Sync` 允许实现安全地被 `Arc<dyn ...>` 共享，但本文件本身不创建线程。
- `analyzeSaveStatsWorker`：保存 worker，字段为 `mpsc::Receiver<analyzeResults>`、`mpsc::Sender<AnalyzeSaveError>` 与 `Arc<dyn analyzeSaveStatsRuntime>`。标准库 `Receiver` 是单消费者，因此 `run` 以 `&self` 暴露并不意味着可以安全地从多个线程并发调用；类型本身也不会因 `Receiver` 自动成为 `Sync`。
- `newAnalyzeSaveStatsWorker(...)`：仅组装三个依赖，不启动线程、不读取通道。
- `panic_message(...)`：识别 panic 载荷中的 `&str` 或 `String`，其他载荷降级为 `"unknown panic payload"`。
- `analyzeSaveStatsWorker::report_once(...)`：先把调用方的 `reported` 标记置为真，再尝试克隆并发送错误；接收端关闭时调用 `log_error_channel_closed`，不会重试。
- `analyzeSaveStatsWorker::run(...)`：核心消费循环和 panic 边界。`analyzeSnapshot` 原样传到存储接口，历史来源固定为 `Analyze`。

## 执行流程

1. 调用者先建立结果通道和错误通道，再用 `newAnalyzeSaveStatsWorker` 注入通道接收端与运行时；构造过程没有后台副作用。
2. `run` 初始化 `error_reported = false`，并在 `catch_unwind(AssertUnwindSafe(...))` 内启动阻塞接收循环。结果通道断开且队列排空时，`recv()` 返回错误，循环正常结束。
3. 每次收到 `results`，若已经保存了 `drain_error`，则以同一个 kill 错误调用 `finish_analyze_job`，随后 `destroy_and_put_to_pool`，跳过 kill 检查与持久化。
4. 非 drain 状态下先调用 `handle_kill_signal`。若返回错误，保存为 `drain_error`，当前作业按失败结束并释放结果，错误只上报一次，然后继续排空余下结果。
5. 未被 kill 时调用 `save_analyze_result_to_storage(context, &mut results, analyzeSnapshot, statsMetaHistorySource::Analyze)`。
6. 保存成功时以 `None` 结束作业；保存失败时先 `log_save_warning`，再以该错误结束作业并尝试一次性上报。保存失败不会设置 `drain_error`，下一结果会重新检查 kill 并再次尝试保存。
7. 正常保存分支无论成功或失败，最后都把结果所有权交给 `destroy_and_put_to_pool`。
8. 若上述闭包任一点 panic，循环立即终止；外层提取 panic 文本、调用 `log_worker_panic`、用 `analyze_panic_error` 转换错误，并经 `report_once` 尝试上报。panic 发生时正在处理的 `results` 不会走显式的 `finish_analyze_job` 或 `destroy_and_put_to_pool`；其 Rust 值会在展开被捕获时正常 drop，但是否完成业务级池回收取决于载荷自身实现，不能等同于显式回池。

## 数据与状态

worker 的持久状态只有三个依赖字段；每次 `run` 的状态是局部的 `error_reported` 和 `drain_error`。`error_reported` 约束错误通道的消息基数为零或一，且即使发送失败也保持为真，防止关闭通道导致重复日志。`drain_error` 只由 kill 检查失败设置，生命周期持续到结果通道耗尽。

结果的所有权流是 `Receiver<analyzeResults>` → `run` 局部变量 → `destroy_and_put_to_pool(results)`。保存接口得到可变借用，允许运行时消费或重写 payload 内部内容，但必须在返回时归还借用，随后 worker 统一完成回池。作业仅以共享借用传给收尾接口，且允许结果没有作业（`job == None`）。

`count`、`payload`、库表名称及 `requestID` 在本文件中均不参与决策；它们是运行时持久化、日志或作业实现可使用的数据。这里没有全局状态、锁、原子量、事务对象或统计聚合状态。

## 依赖与调用关系

本文件的直接代码依赖全部来自 Rust 标准库：`Any`/`fmt`、`panic::{catch_unwind, AssertUnwindSafe}`、`Arc` 和 `mpsc`。虽然所属 crate 在 `Cargo.toml` 中依赖 statistics、storage、sqlkiller 等多个本地 crate，本文件没有直接引用这些具体类型，而是用 `analyzeSaveStatsRuntime` 隔离它们。

已验证的上游关系如下：

- `pkg/executor/lib.rs` 公开本模块。
- `pkg/executor/analyze_worker_test.rs::run_worker` 是当前 Rust 仓库内唯一直接构造并运行此 worker 的调用点。
- 未找到 Rust 生产调用点。`pkg/executor/analyze.rs::AnalyzeExec::handleResultsErrorWithConcurrency` 是当前生产语义最接近的保存路径，但它没有复用本文件，并且其具体流程存在差异：它共享带锁的接收端来启动多个保存线程，主线程负责 kill 检查、全局统计合并与历史统计记录。

本文件的下游调用全部经 `analyzeSaveStatsRuntime` 动态分派：`handle_kill_signal`、`save_analyze_result_to_storage`、`finish_analyze_job`、`destroy_and_put_to_pool` 以及四个日志/错误转换钩子。唯一非运行时下游是标准库通道的 `recv`/`send` 与 panic 捕获。

Go 生产调用链是 `pkg/executor/analyze.go::AnalyzeExec.handleResultsErrorWithConcurrency` 构造 `newAnalyzeSaveStatsWorker`，再在保存 goroutine 中调用 `worker.run`；该关系由 `analyze.go` 中构造调用及 `analyze_worker.go` 的同名定义直接验证。

## 错误处理与边界

- kill 错误是控制流错误：当前及后续结果都标记失败并回收，后续结果不再落盘；只把首次错误发送给调用者。
- 保存错误是单结果错误：会记录 warning、失败结束该作业、尝试上报一次，但继续处理后续结果。调用者不能仅凭错误通道消息数推断失败分区数。
- 错误通道关闭不会让 worker panic或停止 drain；`report_once` 改走 `log_error_channel_closed`。由于发送端是标准库无界 `mpsc::Sender`，正常发送不会因容量不足阻塞。
- 结果通道关闭是正常完成条件，不产生错误，也不会主动关闭错误通道；错误发送端随 worker 生命周期释放后，接收者才能观察到所有发送端断开。
- panic 被捕获并转换，但捕获范围只覆盖消费闭包；panic 后不继续接收剩余结果。如果此 worker 是唯一消费者，上游仍可能阻塞，因此运行时方法不应依赖 panic 作为普通错误路径。
- `AssertUnwindSafe` 是显式承诺：即使运行时包含内部可变状态，也允许跨 panic 边界捕获。实现者必须确保自身在 panic 后的状态可安全记录错误和销毁。
- `panic_message` 对非字符串 panic 载荷仅提供通用文本，调试信息可能丢失；具体堆栈记录取决于 `log_worker_panic` 实现。

## 并发与资源生命周期

本文件不创建线程，调用方决定在哪个线程运行 `run`。`run` 使用阻塞式 `recv` 并独占 `Receiver`，适合“一实例一消费者”；需要多个保存 worker 时，不能直接克隆标准库接收端，应由上层拆分通道或像当前 `analyze.rs` 那样用同步机制共享接收端。

结果通道承担背压与终止信号：只有全部发送端被释放且已排空时 worker 才返回。kill 后继续 drain 是本文件最关键的并发不变量，因为上游可能仍持有有界发送端并等待发送完成。普通保存失败也继续消费，避免失败分区使生产者永久阻塞。

错误通道注释说明其设计为无界：每个 worker 最多一条错误，报告错误不能反过来阻塞结果排空。`Arc<dyn analyzeSaveStatsRuntime>` 允许运行时跨线程共享，trait 的 `Send + Sync` 是这一边界的必要约束。

每个正常取出的结果恰好显式回池一次；drain、保存成功和保存失败三条正常路径都满足该不变量。panic 路径是例外：Rust 会 drop 当前拥有值，但不会显式调用运行时的回池钩子。作业同样在正常路径恰好收尾一次，而 panic 路径把收尾责任留给上层错误处理。

## 与 Go 版本的对应关系

Rust `analyzeSaveStatsWorker` 对应 Go `analyzeSaveStatsWorker`，`newAnalyzeSaveStatsWorker` 对应同名构造函数，`run` 对应 Go 方法 `(*analyzeSaveStatsWorker).run`。两者都遵守：逐项检查 kill、kill 后 drain、普通保存失败仍继续、每个正常结果结束作业并回池、每个 worker 至多上报一次错误，以及 recover/catch 后上报 panic 错误。

关键映射如下：

- Go `<-chan *statistics.AnalyzeResults` 对应 Rust `mpsc::Receiver<analyzeResults>`；Go 指针载荷的销毁方法映射为 Rust 拥有型载荷交给 `destroy_and_put_to_pool`。
- Go `*sqlkiller.SQLKiller.HandleSignal`、`*handle.Handle.SaveAnalyzeResultToStorage`、`finishJobWithLog` 和日志函数被统一抽象为 Rust `analyzeSaveStatsRuntime`，因此 Rust 文件本身不直接依赖 statistics/handle/sqlkiller crate。
- Go `util.StatsMetaHistorySourceAnalyze` 对应 Rust `statsMetaHistorySource::Analyze`；Go `analyzeSnapshot` 原样对应 Rust `analyzeSnapshot`。
- Go `defer recover` 对应 Rust `catch_unwind`；Go `getAnalyzePanicErr` 的职责由 `panic_message` 加运行时 `analyze_panic_error` 分担。
- Go 的 `errCh chan<- error` 可能由上层容量决定；Rust 使用无界标准库发送端，并通过 `report_once` 固化最多一次上报。
- Go 在 panic 时直接向 `errCh` 发送；Rust 若错误接收端已关闭会记录 `log_error_channel_closed`，不会二次 panic。

Go 端有真实生产接线和端到端回归：`pkg/executor/analyze_test.go::TestAnalyzeSaveResultErrorDoesNotHang` 验证保存失败不会挂住，`TestAnalyzeKillDuringSaveDoesNotHang` 用 failpoint 验证保存阶段收到 kill 后仍排空且作业记录为中断。Rust 本文件不实现该 failpoint，且当前没有生产接线；这些 Go 测试只能作为移植意图证据，不能当作 Rust 端到端验证结果。

## 扩展指南

- 修改 kill/drain 或错误聚合策略时，优先改 `analyzeSaveStatsWorker::run` 和 `report_once`，并在独立的 `pkg/executor/analyze_worker_test.rs` 增加事件顺序断言；不要把测试写入生产源文件。
- 新增持久化所需上下文时，可扩展 `analyzeContext`、`analyzeResults` 或 `analyzeSaveStatsRuntime::save_analyze_result_to_storage`。必须同步测试运行时实现，并对照 Go `statistics.AnalyzeResults` 与 `SaveAnalyzeResultToStorage`，避免只为 Rust 测试缩减语义。
- 新增历史来源时扩展 `statsMetaHistorySource`，同时检查运行时适配层及 Go `StatsMetaHistorySource` 的兼容映射；本 worker 当前应继续明确传递 `Analyze`，不要依赖默认值。
- 若把本模块接入 Rust 生产主链，应先协调 `analyze.rs::handleResultsErrorWithConcurrency` 已承担的多 worker 调度、kill 检查、全局统计合并、表 ID 收集和 `recordHistoricalStats`。直接替换现有保存循环会丢失这些职责；最小安全接线应明确谁拥有结果通道、谁聚合错误、谁记录历史统计。
- 若要支持多消费者，不能简单共享本结构；应重新设计接收端所有权或在上层受控加锁，并验证有界通道下的无死锁行为与吞吐影响。
- 改动 panic 策略时应补充非字符串 panic、错误通道关闭、panic 后当前作业/载荷清理等测试。目前独立 Rust 测试只覆盖字符串 panic 转换，未证明 panic 后继续排空。
- 保持许可证头不被删除；Rust 生产逻辑真正修改后按仓库规则保留或添加 `// Copyright 2026 AsterSQL.`，并先运行 `cargo fmt --all`。本说明任务不改 Rust 代码，也不运行 Cargo。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件可由文件节点读取；`query analyze_worker --json` 枚举出目标文件的错误类型、上下文/作业/结果类型、运行时 trait 及其方法。
- RustCodeGraph `node --file pkg/executor/analyze_worker.rs --offset 1 --limit 260`：读取目标文件全部 190 行，核对符号、分支、所有权转移、panic 与通道行为。
- RustCodeGraph `node --file pkg/executor/analyze_worker_test.rs --offset 1 --limit 500`：读取全部 219 行。四个测试分别证明成功事件顺序、保存失败只上报一次且后续仍保存、kill 后以同一错误 drain，以及 panic 转换为 worker 错误。
- RustCodeGraph 对精确 `callers` 的查询在本地索引上超时；因此调用关系另用精确仓库搜索复核。搜索只发现 `pkg/executor/lib.rs`、`pkg/executor/analyze_worker_test.rs` 对本 Rust 模块的声明/使用，没有发现 Rust 生产调用点。
- `pkg/executor/Cargo.toml` 与 `pkg/executor/lib.rs`：核对 crate 名称、`lib.rs` 根、`nextgen` feature，以及生产模块/独立测试模块的装配。目标文件没有条件编译项，也不直接使用 Cargo 中的外部或工作区依赖。
- `pkg/executor/analyze.rs`：核对当前 Rust 生产保存路径 `AnalyzeExec::handleResultsErrorWithConcurrency`，以及上游 `analyzeWorker`/`trySendAnalyzeResult` 的结果生产流程，确认它与本模块尚未接线且职责不完全相同。
- `pkg/executor/analyze_worker.go` 与 `pkg/executor/analyze.go`：核对 Go 同名类型、构造、`run` 的真实生产调用，以及 drain、一次上报、保存失败继续与 panic 恢复语义。
- `pkg/executor/analyze_test.go`：核对 `TestAnalyzeSaveResultErrorDoesNotHang` 和 `TestAnalyzeKillDuringSaveDoesNotHang` 的端到端意图。Rust 边界行为由独立的 `pkg/executor/analyze_worker_test.rs` 验证。
- `pkg/executor` 不存在 `doc.go`；本任务无法从该文件取得额外包契约，模块定位以 Cargo、`lib.rs`、生产调用与测试为准。
- 本次为纯文档分析，按任务约束未运行 Cargo 或代码测试；交付验证仅包含规定的章节结构检查与人工事实复核。
