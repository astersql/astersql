# `pkg/executor/analyze_utils.rs`

## 文件定位

该文件是 `astersql-executor` crate 的 ANALYZE 共用工具模块，由 [`pkg/executor/lib.rs`](lib.rs) 通过 `pub mod analyze_utils` 暴露。它位于 SQL 执行层的统计信息采集支撑面，提供错误分类与 panic 归一化、自适应 DistSQL 扫描并发、会话变量读取、通知通道、等待组包装和协作式取消信号；它本身不解析 SQL，也不负责生成或持久化统计信息。

[`pkg/executor/Cargo.toml`](Cargo.toml) 将 crate 根指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/executor"` 标明 Go 对照包。当前文件的实现只直接使用 Rust 标准库；crate 清单中的大量 executor 依赖属于整个 crate，不能据此推断本文件直接依赖这些子系统。

当前 Rust 生产接线需要区分两层事实：模块已经公开且所有工具都有实现，但仓库内非测试 Rust 调用搜索只确认 [`pkg/executor/analyze.rs`](analyze.rs) 直接使用 `getAnalyzePanicErr`、`errAnalyzeWorkerPanic` 和 `errAnalyzeOOM`；其他工具目前主要由 [`pkg/executor/analyze_utils_test.rs`](analyze_utils_test.rs) 验证。Go 版本的同名工具具有更广泛的生产调用面，不能直接当作 Rust 已接线证据。

## 核心职责

1. `AnalyzeErrorKind`、`AnalyzeError`、`errAnalyzeWorkerPanic`、`errAnalyzeOOM` 和 `isAnalyzeWorkerPanic` 建立 ANALYZE 专用错误分类，避免只靠文本判断 worker panic 与内存超限。
2. `getAnalyzePanicErr` 把 `catch_unwind` 的动态 panic 载荷转换为可返回的 `AnalyzeError`；固定全局内存超限文案会映射成带采样率建议的 OOM 错误，未知载荷回落为 worker panic。
3. `adaptiveAnlayzeDistSQLConcurrency` 在显式配置未生效时，根据 TiKV store 数量计算 DistSQL 扫描并发；无法获得 store 信息时告警并使用默认值 15。
4. `getIntFromSessionVars` 及两个专用包装函数读取并解析构建统计相关的会话/全局变量。
5. `NotifyChannel`、`WaitGroup`、`Completion` 以及两个 wrapper 实现 Go 通道与 `sync.WaitGroup` 风格的工作完成通知：所有预登记任务结束后，由第一个启动的任务关闭通知通道。
6. `normalizeCtxErrWithCause` 保留取消/超时背后的根因；`KillSignal` 提供原子、协作式的取消检查。

## 主要符号

- 常量：`DEF_ANALYZE_DIST_SQL_SCAN_CONCURRENCY = 15` 是 store 状态不可用或 store 数不超过 5 时的默认扫描并发；`DEF_ROWS_FOR_SAMPLE_RATE = 110_000` 仅用于 OOM 建议文案；`GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED` 是识别全局 ANALYZE 内存超限的固定 panic 载荷；两个 `TIDB_BUILD_*_CONCURRENCY` 常量是系统变量名。
- 错误模型：`AnalyzeErrorKind::{AnalyzeWorkerPanic, AnalyzeOutOfMemory, Canceled, DeadlineExceeded, Other}` 与包含 `kind`、`message` 的 `AnalyzeError`。`AnalyzeError` 实现 `Display` 和 `std::error::Error`，并提供 `new`、`other`、`canceled`、`deadline_exceeded` 构造函数。
- 上下文边界：`AnalyzeContext { cause }` 保存取消原因；`StoreStatusError` 区分取得 PD HTTP client 与调用 `GetStores` 两类失败；`AnalyzeSessionContext` trait 把会话并发、系统变量、TiKV store 数量和告警输出抽象为四个必需操作，不会把查询失败伪装成成功。
- 并发选择：`adaptiveAnlayzeDistSQLConcurrency(&AnalyzeContext, &impl AnalyzeSessionContext) -> usize`；名称中的 `Anlayze` 拼写与 Go 版本一致，是兼容接口的一部分。
- 配置读取：`getIntFromSessionVars`、`getBuildStatsConcurrency`、`getBuildSamplingStatsConcurrency`。
- panic/上下文工具：`errAnalyzeWorkerPanic`、`errAnalyzeOOM`、`isAnalyzeWorkerPanic`、`getAnalyzePanicErr`、`normalizeCtxErrWithCause`。
- 同步原语：`NotifyChannel<T>` 是基于 `Arc<Mutex<VecDeque<T>>> + Condvar` 的可关闭 FIFO；`WaitGroup` 用计数器和条件变量实现 `Add`、`Done`、`Wait`。
- 任务包装：`analyzeResultsNotifyWaitGroupWrapper`/`NewAnalyzeResultsNotifyWaitGroupWrapper` 在独立线程运行任务并传递 `AnalyzeResults`；泛型 `notifyErrorWaitGroupWrapper<P>`/`newNotifyErrorWaitGroupWrapper` 通过 `WorkerPool` 提交任务并传递 `AnalyzeError`。`ThreadWorkerPool` 是直接调用 `thread::spawn` 的最小实现。
- 取消与结果：`KillSignal` 用 `AtomicBool` 保存取消状态；`AnalyzeResults` 当前只携带可选错误，是比 Go `statistics.AnalyzeResults` 更窄的本地载体。

## 执行流程

自适应并发流程从 `adaptiveAnlayzeDistSQLConcurrency` 开始。它先读取显式配置：正数立即转换为 `usize` 返回；转换异常时回落默认值。非正数配置触发 store 探测：`Ok(None)`、PD client 失败和 GetStores 失败分别输出对应告警并返回 15；成功得到数量后按 `0..=5 => 15`、`6..=10 => count`、`11..=20 => 2 * count`、`21..=50 => 3 * count`、其余 `4 * count` 计算，并用饱和乘法避免整数溢出。

panic 转换流程由执行端先用 `std::panic::catch_unwind` 捕获载荷，再调用 `getAnalyzePanicErr`。载荷若是 `String`、`&'static str` 或 `AnalyzeError` 且消息等于 `GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED`，返回 `AnalyzeOutOfMemory`；普通 `AnalyzeError` 保留原种类和消息；其他载荷统一返回 `AnalyzeWorkerPanic`。[`pkg/executor/analyze.rs`](analyze.rs) 的本地适配函数把该错误转为执行模块的字符串错误，并在 canonical 执行、worker join 和单任务 worker 等捕获点调用它。

结果通知 wrapper 的使用顺序是不变量：调用者必须先以 `Add(n)` 登记所有任务，再调用 `Run` 恰好启动这些任务。每次 `Run` 用原子计数确定是否为第一个启动的任务，构造 `Completion` 并把它移入任务闭包；闭包正常返回或 unwind 时，`Drop` 都先 `Done`。只有第一个任务的 `Completion` 会继续 `Wait`，等全部登记任务完成后 `close` 通道，因此消费者能以“关闭且队列为空”为终止条件。

取消归一化流程是：`normalizeCtxErrWithCause` 对 `None` 原样返回；仅当表面错误为 `Canceled` 或 `DeadlineExceeded` 且 `AnalyzeContext.cause` 存在时替换为 cause；其他错误不变。`KillSignal::kill` 原子置位，后续 `check` 返回 `Canceled("analyze killed")`。

## 数据与状态

`AnalyzeError`、`AnalyzeContext`、`AnalyzeResults` 都是拥有数据的轻量值类型，可克隆后跨边界传递。`AnalyzeContext` 不是完整请求上下文，只保存可选 cause；超时、取消状态和 deadline 本身由调用者转换成对应 `AnalyzeErrorKind`。

`NotifyChannel` 的共享状态由 `NotifyState { queue, closed }` 表示。队列与关闭位受同一把 `Mutex` 保护，保证发送、接收和关闭的顺序一致；clone 只克隆 `Arc`，所有句柄观察同一队列。关闭不会丢弃已排队值：`recv` 总是先弹出队首，只有队列空且 `closed` 为真时才返回 `None`。

`WaitGroup.pending` 必须与实际任务数一致。`Add` 的正增量采用 checked add，负增量采用 checked sub；溢出、未登记就 `Done` 或 mutex poison 都会 panic，而不是静默破坏同步语义。两个 wrapper 的 `cnt: AtomicU64` 只识别首个 `Run`，不会替代 `WaitGroup` 的完成计数。

`KillSignal.killed` 与 wrapper 的 `cnt` 都使用 `Ordering::SeqCst`，优先提供简单、全序的跨线程可见性。它们没有复位接口：取消信号一旦置位，在该实例生命周期内保持取消；wrapper 也设计为一次任务批次，而不是关闭后复用。

## 依赖与调用关系

- 模块装配：[`pkg/executor/lib.rs`](lib.rs) → `pub mod analyze_utils`；同一文件在 `#[cfg(test)]` 下挂载独立测试 `analyze_utils_test.rs`。
- Rust 生产调用：[`pkg/executor/analyze.rs`](analyze.rs) → `analyze_utils::getAnalyzePanicErr`，并通过 `errAnalyzeWorkerPanic`/`errAnalyzeOOM` 做错误识别。该链覆盖 `catch_unwind`、worker join 和 worker 主体 panic 的转换。
- 测试调用：[`pkg/executor/analyze_utils_test.rs`](analyze_utils_test.rs) 直接覆盖 panic 分类、store 阈值/回退、变量解析、cause 替换以及通道延迟关闭；[`pkg/executor/test/analyzetest/panictest/lib.rs`](test/analyzetest/panictest/lib.rs) 以 `#[path]` 挂载本文件，供 SQL 级 panic failpoint 测试复用同一生产实现。
- 下游抽象：`adaptiveAnlayzeDistSQLConcurrency` 只依赖 `AnalyzeSessionContext` 的四个方法；变量读取也通过该 trait。通知和等待实现只依赖 `std::{sync, thread, collections}`，不直接依赖 executor crate 的外部库。
- Go 主链：[`pkg/executor/analyze.go`](analyze.go)、[`pkg/executor/analyze_col_sampling.go`](analyze_col_sampling.go)、[`pkg/executor/builder.go`](builder.go)、[`pkg/executor/table_reader.go`](table_reader.go) 等调用 Go 同名函数。它们是移植意图和语义对照证据，不证明对应 Rust 调用已存在。

RustCodeGraph 对精确 callee 的结果确认：`adaptiveAnlayzeDistSQLConcurrency` 调用 trait 方法 `analyze_dist_sql_scan_concurrency`、`tikv_store_count`、`warn`；Rust `getAnalyzePanicErr` 调用 `errAnalyzeOOM` 与 `errAnalyzeWorkerPanic`。图的 callers 查询未返回有效文本，因此上游调用面另以仓库内精确符号搜索核对，并在上述列表中限定为已观察到的调用。

## 错误处理与边界

显式并发只有 `> 0` 才覆盖自适应策略；0 或负数均进入 store 数量推导。store 类型不适用、PD client 获取失败、GetStores 失败都不是 ANALYZE 的硬错误，而是告警加默认值。系统变量读取错误用 `?` 原样传播；文本不是十进制 `i64` 时转换为带变量名、原值和解析原因的 `Other` 错误。

OOM 识别是严格字符串协议：只有 panic 载荷消息精确等于 `GLOBAL_PANIC_ANALYZE_MEMORY_EXCEED` 才分类为 `AnalyzeOutOfMemory`。任意其他字符串，即使包含类似文本，也会归为 worker panic；已包装成 `AnalyzeError` 的非 OOM 错误则保留。扩展载荷类型时必须保持未知类型的安全回退。

`NotifyChannel::send` 在关闭后返回 `Err(value)`，把所有权还给调用者；不会重开通道。锁 poison 直接通过 `expect` panic。`WaitGroup` 要求所有任务在 `Run` 前统一登记；如果 `Add` 少于 `Run` 数，`Completion::drop` 会触发负计数 panic；如果多于实际完成数，首个任务会永久等待，通道也不会关闭。

`WorkerPool::spawn` 不返回调度失败信息，因此 wrapper 无法补偿“任务没有真正执行”的实现。自定义 pool 必须保证接受的闭包最终执行或以仍会析构闭包的方式失败，否则完成计数与关闭生命周期可能悬挂。

## 并发与资源生命周期

`NotifyChannel` 支持多个 clone 发送者；实现没有限制接收句柄数量，但单个 FIFO 队列意味着多个接收者会竞争消费，而不是广播。`send` 入队后 `notify_one`，`close` 置位后 `notify_all`，防止空队列上的等待者在关闭时滞留。

`Completion` 使用 RAII 保证任务闭包正常返回或 panic 展开时都会调用 `Done`。第一个 `Run` 的任务承担关闭责任，但它不会在自己完成时立即关闭：先减少自己的 pending，再等待其他任务归零。这个设计避免晚完成任务向已关闭通道发送。若构建启用了 abort-on-panic，Rust 不执行栈展开，RAII 保证自然也不成立；当前源码没有对此作额外恢复。

`analyzeResultsNotifyWaitGroupWrapper::Run` 返回 `JoinHandle<()>`，调用者可显式 join；pool 版本的 `Run` 不返回 handle，生命周期由 pool 与通知通道协调。`ThreadWorkerPool` 丢弃 `thread::spawn` 返回的 handle，因此只能经共享状态观察结束。

原子计数和取消位没有锁生命周期问题，但 `WaitGroup`/`NotifyChannel` 的条件变量等待均可能无限持续，调用者必须满足计数与任务启动不变量。文件中没有异步 runtime、事务、网络句柄或磁盘资源；TiKV 状态访问完全位于 `AnalyzeSessionContext` 实现侧。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/executor/analyze_utils.go`](analyze_utils.go)。两版保留了 Go 风格符号名与核心阈值：显式配置优先、store 数量分段乘数、PD/store 查询失败回退、两个系统变量入口、panic/OOM 区分、取消 cause 替换，以及“首个 Run 等待全部任务后关闭 channel”的 wrapper 语义。

Rust 为 Go 隐式接口补上显式类型边界：`AnalyzeSessionContext` 代替具体 `sessionctx.Context`/storage type assertion，`StoreStatusError` 保留失败阶段，`AnalyzeErrorKind` 代替 Go 哨兵错误比较，`WorkerPool` 代替 `gp.Pool + util.WaitGroupPool`，`NotifyChannel`/`WaitGroup` 则在标准库上重建 channel/WaitGroup 行为。

存在以下重要差异：

- Go 的 `getAnalyzePanicErr` 接受任意 `error` 并保留它；Rust 只能原样保留本模块的 `AnalyzeError`，其他动态载荷回退为 worker panic。
- Go wrapper 的 notify 类型分别是 `chan *statistics.AnalyzeResults` 和 `chan error`；Rust 的 `AnalyzeResults` 目前只含错误，且 `NotifyChannel` 是无容量上限的内存队列。
- Go 的错误 wrapper 构造函数接收 `*gp.Pool`，Rust 构造函数接收任意 `Arc<P: WorkerPool>`；不过当前实现与 Go 代码一样实际依赖“所有任务先 Add”的调用纪律。
- Go `context.Context` 同时承载取消、deadline 和 cause；Rust 将 cause 简化到 `AnalyzeContext`，取消检测另由 `KillSignal` 提供，二者没有自动关联。
- Go 生产代码已经在 builder、column sampling、table reader 等路径调用这些工具；Rust 仓库当前仅确认 panic 工具接入 `analyze.rs`，其余能力仍不能标记为完整生产接线。

[`pkg/executor/analyze_utils_test.go`](analyze_utils_test.go) 目前直接回归 panic 错误格式问题；Rust 独立单测在此基础上补充阈值、失败回退、变量解析、cause 和关闭时序。SQL 级 panic 行为由 `pkg/executor/test/analyzetest/panictest/panic_test.rs` 与 Go 同目录用例对应。

## 扩展指南

- 增加新的 panic 分类时，修改 `AnalyzeErrorKind`、`getAnalyzePanicErr` 和必要的构造函数，并同步 [`pkg/executor/analyze_utils_test.rs`](analyze_utils_test.rs)；如果用户可见的 ANALYZE 行为改变，还要同步 `test/analyzetest/panictest` 的独立 SQL/failpoint 测试。不要只改文本匹配而遗漏 `analyze.rs` 的适配与分类判断。
- 调整 DistSQL 并发公式时，以 `adaptiveAnlayzeDistSQLConcurrency` 为唯一计算入口，保持显式配置优先、非 TiKV/PD 失败回退和溢出策略；扩充 5/10/20/50 边界两侧的表驱动测试，并核对 Go 同路径函数是否需要同步。
- 增加会话变量时，优先复用 `getIntFromSessionVars` 并新增语义明确的薄包装；测试既要覆盖 getter 返回错误，也要覆盖非整数和数值范围。不要从 `Cargo.toml` 的 crate 依赖推导可直接访问的会话实现，应通过 `AnalyzeSessionContext` 扩展边界。
- 扩展通知载荷或 worker pool 时，保持“先 Add、后 Run、首任务最终关闭”的协议；补充任务发送数据、任务 panic、关闭后发送、等待者唤醒、错误 pool 行为等独立测试。若要支持可复用批次或 bounded channel，应新设计状态机，不能只重置 `cnt`。
- 若将当前仅测试覆盖的工具接入 Rust 生产路径，应在具体调用模块增加局部接线与测试，并显式记录与 Go 调用点的对应关系；不要把 Go 的调用存在性当作 Rust 接线完成。
- Rust 测试继续放在独立文件 `pkg/executor/analyze_utils_test.rs` 或独立测试 crate 中，不把 `#[cfg(test)]` 测试嵌回生产源文件。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/analyze_utils.rs` 确认目标文件已索引，`node --file` 分段读取了全部 526 行与 57 个符号。
- RustCodeGraph 符号查询：查询了 `adaptiveAnlayzeDistSQLConcurrency`、`NewAnalyzeResultsNotifyWaitGroupWrapper`、`newNotifyErrorWaitGroupWrapper`、`getAnalyzePanicErr`、`normalizeCtxErrWithCause`；callee 查询确认了自适应并发到 trait 方法、panic 转换到 OOM/worker-panic 构造函数的边。callers 查询无有效输出，因此没有把图中笼统的 “used by” 文件计数解释为真实调用者。
- 生产源码：完整读取 [`pkg/executor/analyze_utils.rs`](analyze_utils.rs)；读取 [`pkg/executor/lib.rs`](lib.rs) 的模块与测试装配；读取 [`pkg/executor/analyze.rs`](analyze.rs) 的错误适配和多个 `catch_unwind`/join 使用点。
- crate/Go 对照：读取 [`pkg/executor/Cargo.toml`](Cargo.toml) 的 package、lib、feature、porting metadata 与依赖边界；完整读取 [`pkg/executor/analyze_utils.go`](analyze_utils.go)，并以精确符号搜索核对 Go 的 `analyze.go`、`analyze_col_sampling.go`、`builder.go`、`table_reader.go` 等调用点。
- 测试证据：完整读取 [`pkg/executor/analyze_utils_test.rs`](analyze_utils_test.rs) 与 [`pkg/executor/analyze_utils_test.go`](analyze_utils_test.go)；读取 `pkg/executor/test/analyzetest/panictest/lib.rs` 和 `panic_test.rs`，确认生产文件复用方式及 SQL 级 panic 转错误场景。
- 人工边界复核：文档明确区分 Rust 已实现能力、Rust 已接线调用与 Go 对照调用；未声称运行任何 Cargo 测试。本任务是纯文档分析，按计划只执行结构验证。
