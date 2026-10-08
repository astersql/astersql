# `pkg/util/wait_group_wrapper.rs`

## 文件定位

本文件属于 `astersql-util` crate（`pkg/util/Cargo.toml`），并由 `pkg/util/lib.rs` 以 `pub mod wait_group_wrapper` 公开。它把 Go `pkg/util/wait_group_wrapper.go` 中围绕 `sync.WaitGroup`、goroutine 池和 `errgroup.Group` 的辅助能力移植为 Rust 线程同步组件：调用者提交一次性闭包，包装器负责计数、等待、panic 恢复、后台任务标签追踪或错误汇总。

它是通用并发基础设施，不负责 SQL、存储或调度业务本身。当前可确认的生产接线包括：`pkg/resourcemanager/rm.rs` 用 `WaitGroupWrapper` 管理资源调度循环的启动和停止；`br/pkg/metautil/statsfile.rs` 用它等待并发统计文件下载；`pkg/util/worker_pool.rs` 把池化 worker 任务提交给 `ErrorGroupWithRecover`。`pkg/statistics/handle/types/interfaces.rs` 还重新导出 `WaitGroupEnhancedWrapper`，供统计模块接口使用。

## 核心职责

- `WaitGroup` 提供可克隆的共享计数器和“等待计数归零”语义，是其余包装器的共同基础。
- `WaitGroupWrapper` 为每个闭包启动独立 OS 线程，并提供普通运行、记录 panic、panic 恢复回调三种入口。
- `WaitGroupEnhancedWrapper` 在计数之外维护唯一任务标签；可选的退出检查线程会在收到退出信号后持续报告尚未注销的后台任务。
- `WaitGroupPool` 把相同的计数/等待语义接到已有 `threadpool::ThreadPool`，避免每个任务都直接创建线程。
- `ErrorGroupWithRecover` 汇总返回 `anyhow::Result<()>` 的线程，将 panic 转成错误，并可在任一任务失败时取消关联的子 `CancellationToken`。
- `DoneGuard` 与 `EnhancedDoneGuard` 用 RAII 保证正常返回和线程展开路径都会减计数；增强守卫还负责注销标签。

## 主要符号

- `PanicPayload = Box<dyn Any + Send + 'static>`：`catch_unwind` 的 panic 载荷类型。`panic_message` 只识别 `&str` 和 `String`，其他载荷统一显示为 `panic with non-string payload`。
- `WaitState { count: Mutex<usize>, done: Condvar }`：共享计数和条件变量。它是私有状态，借助 `Arc` 被所有 `WaitGroup` 克隆共享。
- `WaitGroup::{Add, Done, Wait}`：`Add` 使用 `checked_add`；`Done` 要求计数大于零，并在减至零时 `notify_all`；`Wait` 用 `while` 循环抵抗条件变量的伪唤醒。
- `DoneGuard(WaitGroup)`：析构时调用 `Done`，是任务计数最终归还的关键不变量。
- `WaitGroupWrapper::{Run, RunWithLog, RunWithRecover, Wait}`：基础线程包装。`RunWithLog` 捕获并记录 panic；`RunWithRecover` 把可选载荷交给回调，并在确有 panic 时记录错误。
- `NewWaitGroupEnhancedWrapper`、`WaitGroupEnhancedWrapper::{Run, RunWithRecover, check, Wait}`：创建和操作增强包装器。`on_start`/`on_exit` 管理私有的 `HashSet<String>` 标签集合；`check_unexited_process` 实现退出后的两秒轮询。
- `NewWaitGroupPool`、`WaitGroupPool::{Run, Wait}`：把任务交给 `threadpool::ThreadPool::execute`，仍由内部 `WaitGroup` 跟踪本包装器提交的任务。
- `NewErrorGroupWithRecover`：创建不带取消上下文的空错误组。
- `NewErrorGroupWithRecoverWithCtx`：从父 `CancellationToken` 派生 child token，并同时返回持有该 child 的错误组和 child 本身。
- `ErrorGroupWithRecover::{Go, Wait}`：`Go` 启动线程、捕获 panic、发送结果并在错误时取消 token；`Wait` 收取当前已登记任务的全部结果、连接全部线程，并返回按完成时序观察到的第一个错误。

## 执行流程

基础包装器的流程是：`Run*` 先同步执行 `Add(1)`，克隆共享 `WaitGroup`，再创建线程；线程首先构造 `DoneGuard`，随后执行闭包或在 `catch_unwind(AssertUnwindSafe(...))` 中执行闭包。无论正常返回还是 panic 展开，守卫析构都会调用 `Done`。调用方的 `Wait` 在同一共享计数上阻塞，直到最后一个守卫把计数减为零。

增强包装器的 `Run*` 在加计数前调用 `on_start(label)`，以原子加锁的 `HashSet::insert` 拒绝重复标签；线程内的 `EnhancedDoneGuard` 在析构时先 `on_exit` 删除标签，再 `Done`。若构造时 `exited_check == true`，`NewWaitGroupEnhancedWrapper` 还会把检查线程计入同一个 wait group；该线程先等待 `exit.recv()`，之后每两秒调用 `check`，直至标签集合为空才退出。因此此模式下 `Wait` 同时等待业务任务和退出检查线程，调用方必须发送或关闭退出通道，否则检查线程会一直阻塞。

池化包装器与基础流程相同，只是执行载体从 `thread::spawn` 换为 `ThreadPool::execute`。`ErrorGroupWithRecover::Go` 则为每个任务保存 `JoinHandle`，在线程内把闭包的普通 `Err` 原样保留，把 panic 载荷转成 `anyhow` 错误，再把结果写入无界 channel；有错误时先取消可选 token。`Wait` 先取走当前句柄列表，按 channel 到达顺序接收同样数量的结果，记住第一个错误，再逐一 `join` 所有句柄，确保其他任务也结束后才返回。

## 数据与状态

`WaitGroup` 的状态是 `Arc<WaitState>`，因此克隆包装器或传给守卫不会复制计数。计数只在 `Mutex<usize>` 保护下变化；计数为零是 `Wait` 唯一的完成条件。`WaitGroupWrapper` 可克隆，克隆值共享同一计数域，而不是创建新的任务组。

`WaitGroupEnhancedWrapper` 通常放在 `Arc` 中，因为 `Run*` 的接收者是 `&Arc<Self>`，线程和 `EnhancedDoneGuard` 都需要持有所有权。`source` 仅用于日志上下文，`register_process` 是当前在途任务标签的集合；标签必须在同一增强包装器内唯一。

`WaitGroupPool` 拥有一个 `ThreadPool` 和独立计数域。`ErrorGroupWithRecover` 用互斥量保存尚未等待的句柄，用一对无界 sender/receiver 传递完成结果，并可选持有 child cancellation token。`Wait` 通过 `mem::take` 消耗当时的句柄列表；完成后再次调用 `Wait` 只会处理之后新登记且尚未取走的句柄。

## 依赖与调用关系

直接标准库依赖包括 `Any`、`HashSet`、`Arc`、`Mutex`、`Condvar`、线程/`JoinHandle`、panic 捕获和 `Duration`。`pkg/util/Cargo.toml` 声明了本文件使用的外部依赖：`anyhow` 统一错误、`crossbeam-channel` 提供增强包装器的退出 receiver 与错误组结果 channel、`log` 输出诊断、`threadpool` 承载池化任务、`tokio-util` 提供 `CancellationToken`。

已核实的调用边如下：

- `pkg/resourcemanager/rm.rs::ResourceManager::Start` 通过 `ResourceManagerInner.wg.Run` 启动每 100 ms 调度循环，`Stop` 发退出信号后调用 `wg.Wait`，使资源管理器关闭等待后台循环退出。
- `br/pkg/metautil/statsfile.rs::downloadStats` 为每个统计文件调用 `WaitGroupWrapper::Run`，任务内部从另一 worker pool 借/还 worker；停止派发后用 `Wait` 等待已启动下载。
- `pkg/util/worker_pool.rs::{ApplyOnErrorGroup, ApplyWithIDInErrorGroup}` 调用 `ErrorGroupWithRecover::Go`，借助 `RecycleGuard` 在任务结束或错误展开时归还 worker。
- `pkg/statistics/handle/types/interfaces.rs` 公开重导出 `WaitGroupEnhancedWrapper`，并在统计接口的 `SubLoadWorker` 参数中使用该类型。
- crate 入口 `pkg/util/lib.rs` 挂载生产模块，并在 `cfg(test)` 下把 `pkg/util/wait_group_wrapper_test.rs` 作为独立测试模块挂载，符合测试逻辑不内嵌源文件的仓库约束。

RustCodeGraph 的文件级结果显示目标文件被 35 个已索引文件引用；方法级精确 callers/callees 查询未给出稳定结果，因此上述具体边均另外用索引源码片段或文本引用位置复核，没有把文件级数量等同于 35 个有效生产调用点。

## 错误处理与边界

- `WaitGroup::Add` 在 `usize` 溢出时 panic；`Done` 在零计数上调用时以 `negative WaitGroup counter` panic。所有互斥量/条件变量中毒都通过 `expect` 转成 panic，而不是返回错误。
- `WaitGroupWrapper::Run` 不捕获用户闭包 panic，但 `DoneGuard` 仍会减计数；创建出的 `JoinHandle` 被丢弃，所以调用者不能从 `Wait` 得知该 panic。需要可观察 panic 时应使用 `RunWithLog`、`RunWithRecover` 或错误组。
- 基础版 `RunWithRecover` 无论是否 panic，只要提供了回调都会调用它：正常完成传 `None`，panic 传 `Some(PanicPayload)`。增强版只在发生 panic 时调用恢复回调，且参数不是 `Option`。
- `RunWithLog` 和基础 `RunWithRecover` 只记录提取后的消息/固定文案，不返回错误；增强版 `RunWithRecover` 捕获后也不会让 `Wait` 失败。不能把这些 API 当作结果传播通道。
- 启用退出检查时，`exit` 必须是 `Some`，否则构造函数 panic。通道关闭和收到一个值都会令 `recv` 返回并开始检查；检查没有超时上限，只要标签未清空就每两秒继续。
- `WaitGroupPool::Run` 在提交前已加计数；如果底层线程池拒绝/丢弃任务而闭包从未运行，计数无法由 `DoneGuard` 归还。当前接口没有提交错误返回值，扩展时必须评估该风险。
- `ErrorGroupWithRecover` 把字符串 panic 转成可读 `anyhow`，非字符串 panic 退化为占位文本。任一普通错误或 panic 都会取消 child token，但 `Wait` 仍等待全部已登记任务。返回的是最先从 channel 到达的错误，不是提交顺序中的第一个错误。
- 错误组线程若在 group/receiver 已被提前丢弃后发送结果，`send(...).expect(...)` 会再次 panic。正常用法必须让 group 存活到 `Wait` 完成。
- 与 Go `errgroup` 一样，不应把“并发调用 `Go` 与 `Wait`”作为受支持模式；Rust `Wait` 只取走调用瞬间已登记的句柄。

## 并发与资源生命周期

计数增加发生在线程提交之前，因此正常路径不会出现任务已经完成而尚未计数的竞态。守卫在线程闭包最外层创建，确保普通返回、用户 panic 和错误返回都能归还计数。`Condvar::notify_all` 允许多个 `Wait` 调用者在归零时一起唤醒，`while count != 0` 则保证伪唤醒不会提前返回。

增强标签与计数是两套状态：标签先注册、再加计数；线程退出时标签先删除、再减计数。这样当 `Wait` 观察到归零时，该任务标签已清理。退出检查线程本身没有业务标签，但会占一个计数；它必须在 exit 信号到达且所有业务标签消失后才释放该计数。

基础和增强 `Run*` 都创建分离线程，不保留句柄；资源回收依赖线程自行退出。池化版本的任务生命周期受传入线程池管理。错误组明确保存句柄，`Wait` 即使已经看到首错也会继续收齐结果并 join 所有线程；取消 token 只是协作式信号，不会强制终止闭包，任务必须主动检查 token 才能提前结束。

## 与 Go 版本的对应关系

Rust 的公开命名和职责直接对应 `pkg/util/wait_group_wrapper.go`：`WaitGroupWrapper`、`WaitGroupEnhancedWrapper`、`WaitGroupPool`、`ErrorGroupWithRecover` 及各 `New*`/`Run*`/`Wait` 入口均保留 Go 风格名称。Rust `WaitGroup` 用 `Mutex<usize> + Condvar` 替代 `sync.WaitGroup`；`thread::spawn` 替代 goroutine；`crossbeam_channel::Receiver<()>` 替代退出 channel；`threadpool::ThreadPool` 替代 `gp.Pool`；`CancellationToken` 替代 `context.Context`；内部结果 channel 和句柄列表实现 `errgroup.Group` 的等待语义。

已核对的行为一致点包括：提交前加计数、退出时减计数；基础 `RunWithRecover` 在正常路径也调用回调并传空恢复值；增强恢复回调只在 panic 时调用；重复标签属于编程错误；退出后每两秒检查未注销任务；错误组把 panic 转成错误且按完成先后返回首错。`pkg/util/wait_group_wrapper_test.rs` 的并发计数、恢复回调、标签检查、日志恢复和首个完成错误测试对应 `pkg/util/wait_group_wrapper_test.go` 的原始意图。

需要注意的差异是：Go 日志包含恢复对象和栈，Rust 当前只输出消息或固定文案；Go `GetRecoverError` 的具体错误包装未被完整复制，Rust 使用 `panic_message + anyhow`；Rust 的增强标签插入在单次持锁内完成，避免了 Go `onStart` 先检查再二次加锁插入的窗口；Rust 基础 `Run` 的线程 panic 不会传播到 `Wait`；Rust 错误组显式 join 线程，而 Go `errgroup.Wait` 等待 goroutine 完成但没有句柄概念。这些差异在修改可观测日志、错误文本或兼容行为前必须先确认是否属于有意移植选择。

## 扩展指南

- 扩展计数语义时优先修改 `WaitGroup::{Add, Done, Wait}`，保持“提交前加计数、所有退出路径恰好 Done 一次、归零才唤醒”的不变量；同步扩展独立测试 `pkg/util/wait_group_wrapper_test.rs`，不要把测试写回生产文件。
- 新增基础执行模式时复用 `DoneGuard`，新增增强执行模式时复用 `EnhancedDoneGuard`；不要手写多个提前返回分支的 `Done`/`on_exit`，否则容易出现泄漏或双减。
- 修改增强退出检查时关注 `NewWaitGroupEnhancedWrapper`、`check_unexited_process`、`check`、`on_start`、`on_exit` 的整体协议，并增加“未收到 exit 时 Wait 阻塞、收到 exit 后等待标签清空、重复标签失败”的确定性测试。现有 `TestWaitGroupWrapperCheck` 依赖一秒 sleep，可考虑用 channel/屏障替代脆弱时序。
- 为池化提交增加失败语义时，应让 `Run` 在提交失败时回滚计数或返回错误；需要同步验证饱和、关闭/拒绝提交和任务 panic 路径，并检查 `threadpool` 的真实保证。
- 扩展错误组时应保持“首个完成错误 + 等待全部任务”的行为，分别测试普通 `Err`、字符串/非字符串 panic、多错误完成顺序、取消 token 和重复 `Wait`。若要增强 Go 兼容性，最可能修改 `panic_message`、`Go` 的日志/错误构造和 `NewErrorGroupWithRecoverWithCtx`。
- 生产接线变更应回归直接消费者：资源管理器启停（`pkg/resourcemanager/rm.rs`）、BR 下载关闭顺序（`br/pkg/metautil/statsfile.rs`）、worker 回收与错误传播（`pkg/util/worker_pool.rs`）。并发基础设施的兼容风险主要是死锁、计数泄漏、错误选择顺序变化和日志/错误文本变化；性能风险主要是无界创建 OS 线程、两秒轮询以及无界结果 channel。

## 验证依据

- RustCodeGraph：`status` 确认本地索引包含 11,467 个文件；`query WaitGroupWrapper` 定位 Rust/Go 定义；`node wait_group_wrapper.rs::WaitGroupWrapper` 和 `node --file pkg/util/wait_group_wrapper.rs` 读取目标文件全部 445 行；文件级关系报告 35 个使用文件。精确方法级 callers/callees 未返回稳定边，已明确降级为索引文件片段与 `rg` 交叉验证。
- 生产源码：`pkg/util/wait_group_wrapper.rs`（全部类型、函数、实现和无条件编译项）；该文件没有 feature 或 `cfg` 分支。
- crate 边界：`pkg/util/Cargo.toml`（crate 名、lib 入口、依赖和测试 target）与 `pkg/util/lib.rs`（公开模块及独立测试挂载）。目标包下未找到 `pkg/util/doc.go`，因此没有可额外读取的包契约文件。
- Go 对照：`pkg/util/wait_group_wrapper.go` 与 `pkg/util/wait_group_wrapper_test.go`。
- Rust 测试：`pkg/util/wait_group_wrapper_test.rs`；补充证据 `pkg/util/security_2_aster_unit_test.rs` 覆盖基础/增强包装器和错误组生命周期。
- 直接调用证据：`pkg/resourcemanager/rm.rs::{Start, Stop}`、`br/pkg/metautil/statsfile.rs::downloadStats`、`pkg/util/worker_pool.rs::{ApplyOnErrorGroup, ApplyWithIDInErrorGroup}`、`pkg/statistics/handle/types/interfaces.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文能回答文件存在原因、运行路径、并发不变量、边界和安全扩展位置。
