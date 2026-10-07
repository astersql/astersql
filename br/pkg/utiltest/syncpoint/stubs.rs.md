# [`br/pkg/utiltest/syncpoint/stubs.rs`](./stubs.rs)

## 文件定位

本文件属于 `astersql-br-pkg-utiltest-syncpoint` library crate；crate 根由 `br/pkg/utiltest/syncpoint/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 装入本模块，并重新导出 `Context`、`CancelHandle`、`StopWatch` 与 `after_func`。它不是 BR 备份恢复业务实现，而是测试同步点编排器 `syncpoint.rs` 的本地取消语义适配层。

Cargo 元数据把该 crate 对应到 Go 包 `br/pkg/utiltest/syncpoint`，并明确选择轻量本地 Context 桩，以避免在 darwin arm64 上为测试编排引入 kv/domain/kvproto/grpcio 等重型依赖。模块自身只使用 Rust 标准库；crate 的 `astersql-testkit-testfailpoint` 与 `fail` 依赖由相邻的 `syncpoint.rs` 和测试使用。

## 核心职责

文件提供 Go `context.Context`、取消函数和 `context.AfterFunc` 在当前 syncpoint 场景所需的最小子集：创建永不自行取消的背景上下文，创建手动取消或定时取消的上下文，共享并查询首个取消原因，阻塞等待取消，以及注册一个在取消后至多执行一次、可提前停止的回调。

其能力边界由源码模块注释和真实调用共同限定：它只服务 `Script::BeginSeq` 的取消监听，不实现父子 context 树、deadline 查询、value 传播或完整 Go context API。`syncpoint.rs:123-179` 使用 `after_func` 把上下文取消转换为序列错误并唤醒错序等待者；序列正常完成或 `EndSeq` 时停止监听。

## 主要符号

- `Context { inner: Arc<ContextInner> }`：可克隆的公开上下文句柄。所有 clone 共享同一个内部状态。`Default` 等价于 `Context::background()`。
- `ContextInner`：私有共享状态，包含顺序一致原子取消标志 `cancelled`、由互斥锁保护的首个错误字符串 `err`，以及供等待者睡眠/唤醒的 `(Mutex<()>, Condvar)`。
- `Context::background() -> Context`：创建初始未取消、无错误且不会自动变化的上下文。
- `Context::with_cancel() -> (Context, CancelHandle)`：创建共享同一 `ContextInner` 的上下文和取消句柄。
- `Context::with_timeout(Duration) -> (Context, CancelHandle)`：在 `with_cancel` 基础上启动后台线程；睡眠到期后以 `context deadline exceeded` 取消。
- `Context::is_done() -> bool`：非阻塞读取取消标志。
- `Context::err_message() -> Option<String>`：克隆并返回首个取消文案；尚未取消时返回 `None`。
- `Context::wait_cancelled()`：在条件变量上循环等待，直至取消标志为真；循环可抵御虚假唤醒。
- `CancelHandle { inner: Arc<ContextInner> }`：可克隆、可跨线程使用的取消端。`cancel()` 使用 `context canceled`，`cancel_with` 接受自定义字符串。
- `StopWatch { stop: Arc<AtomicBool> }`：`after_func` 的停止句柄；`stop()` 首次抢到执行槽时返回 `true`，回调已抢到或此前已停止时返回 `false`。
- `after_func(Context, FnOnce) -> StopWatch`：启动一个监听线程，在上下文取消与停止请求之间原子竞争，并保证回调至多执行一次。

本文件没有模块级常量、trait、enum 或条件编译项；`ContextInner` 是唯一私有类型，其余上述类型和函数均为公开 API，并经 `lib.rs` 再导出。

## 执行流程

手动取消路径如下：调用方通过 `with_cancel` 得到共享同一 `Arc<ContextInner>` 的 `Context` 和 `CancelHandle`；`cancel` 转发到 `cancel_with`；`cancel_with` 先在 `err` 锁内仅写入首个原因，再以 `SeqCst` 将 `cancelled` 置真，最后 `notify_all` 唤醒全部等待者。先写错误、后发布取消标志是不变量，避免观察者看到“已取消”却暂时读不到原因。

定时取消路径在上述流程外增加一个独立线程：`with_timeout` 克隆取消句柄，线程完整睡眠指定 `Duration` 后调用 `cancel_with("context deadline exceeded")`。调用方仍可通过返回的句柄提前取消；由于首错保留，之后到期的线程不会覆盖手动取消原因。

`after_func` 为每次注册创建共享原子槽 `stop` 和一个后台线程。线程持有 context 等待互斥锁，循环依次检查停止标志和取消标志；未发生任一事件时以 20 毫秒超时等待条件变量。若先观察到停止便退出；若先观察到取消，则释放等待锁并用 `swap(true)` 竞争唯一执行权，胜者调用一次 `FnOnce`，败者直接退出。返回的 `StopWatch` 操作相同的原子槽。

在实际主链中，`Script::BeginSeq` 克隆传入 context 并注册 `after_func`；取消回调取得脚本状态锁，若序列仍在等待，则记录当前步骤及 `ctx.err_message()`，再广播脚本自己的条件变量。`Script::EndSeq` 和 `advance` 完成最后一步时调用 `StopWatch::stop`，防止迟到取消污染成功序列。

## 数据与状态

一个 context 的全部可变状态都位于 `Arc<ContextInner>` 中，因此 `Context` clone 和所有 `CancelHandle` clone 观察同一取消事实。取消状态单向变化：`cancelled` 只从 `false` 变成 `true`；`err` 只从 `None` 变为第一个 `Some(String)`，后续取消不会替换它。代码没有恢复或复用同一 Context 的入口。

取消标志和 `after_func` 的停止槽均采用 `Ordering::SeqCst`，为跨线程观察提供单一全序。错误字符串由独立 `Mutex` 保护；等待条件变量配有自己的空值互斥锁。`cancel_with` 的发布顺序使已取消观察与错误读取保持预期关系，但 `is_done()` 与 `err_message()` 仍是两个独立 API，不构成一次原子快照。

`StopWatch` 不持有线程 join handle；其共享原子值同时表示“已停止”或“回调执行权已被领取”。因此该值适合一次性仲裁，而不能区分停止原因。

## 依赖与调用关系

向上，`lib.rs` 声明并再导出本模块；RustCodeGraph 将 `stubs.rs` 标为被 `syncpoint.rs` 和 `syncpoint_test.rs` 使用。关键业务内调用边是 `Script::BeginSeq -> stubs::after_func`，回调再读取 `Context::err_message`；`StateInner` 保存 `Option<StopWatch>`，`BeginSeq` 替换旧监听、`EndSeq` 收尾、`advance` 完成最后一步时都调用 `StopWatch::stop`。

`Context::with_timeout` 的直接测试调用者包括 `syncpoint_test.rs` 的三个测试，以及 `parity_test.rs` 的正常、边界和复用场景；`parity_test.rs` 的取消场景还直接使用 `with_cancel` 与 `cancel_with`。`Context::background`、`is_done` 和 `wait_cancelled` 当前在该 crate 的同步点主链中没有外部使用证据，但它们构成同一最小 Context API。

向下，本文件仅依赖 `std::sync::{Arc, Mutex, Condvar}`、`AtomicBool`、`std::thread` 和 `Duration`。它不直接调用 Cargo 中声明的 failpoint 依赖，也不接触数据库、网络、磁盘或异步运行时。

## 错误处理与边界

取消不是 `Result` 错误，而是共享状态：未取消时 `err_message` 为 `None`，默认取消文案为 `context canceled`，超时文案为 `context deadline exceeded`，自定义取消文案原样保存。多次取消是幂等的状态发布，但每次仍会置位原子并广播；只有首次调用能决定错误文案。

所有标准库锁操作使用 `unwrap`，`after_func` 的超时等待使用 `expect("context wait poisoned")`；若持锁线程 panic 导致锁中毒，当前策略是继续 panic，而不是恢复或返回结构化错误。后台线程中的此类 panic 不会通过本 API 回传给调用者。

`with_timeout` 不校验零时长，零时长意味着新线程可立即竞争取消。它也不支持撤销计时线程：即使提前手动取消或所有外部句柄被丢弃，该线程仍会睡到期限并再次发布取消，但不会覆盖首错。

`after_func` 的“停止成功”只表示调用者赢得一次性原子槽，不表示监听线程已经退出；监听线程在没有 context 通知时最多要等下一次 20 毫秒轮询超时才观察到停止。回调一旦取得执行权，`stop()` 不等待回调结束，也不能撤销正在运行的回调。这与 Go `context.AfterFunc` 返回函数“不等待回调完成”的关键边界一致。

## 并发与资源生命周期

`Context` 和 `CancelHandle` 通过 `Arc` 共享所有权，可被移动或克隆到不同线程。取消广播会唤醒所有 `wait_cancelled` 和 `after_func` 等待者；每个等待者都在循环中复查原子条件，正确处理虚假唤醒和多个竞争事件。

每次 `with_timeout` 创建一个分离线程，每次 `after_func` 也创建一个分离线程，二者都不保存 `JoinHandle`。线程捕获的 `Arc` 会延长内部状态生命周期，直到超时到达或监听线程观察到停止/取消并退出。因此丢弃所有公开句柄并不保证相关线程立即消失。

`after_func` 中取消和停止通过同一个 `AtomicBool::swap` 仲裁：停止先抢到则回调不运行；取消线程先抢到则恰好运行一次；并发或重复 `stop()` 不会产生第二次成功。回调执行前显式释放 context 的等待锁，因此回调可以读取 context 或触发其他同步动作而不被该锁自死锁。

该实现不使用 Tokio、通道或任务取消；资源成本是每个 timeout/watch 各一个操作系统线程。它适合数量有限的测试编排，不应被误当作高并发生产 context 实现。

## 与 Go 版本的对应关系

Go 同路径 `syncpoint.go` 不定义 context，本文件对应的是该文件 `BeginSeq` 中使用的标准库 `context.Context` 与 `context.AfterFunc`。Go 在 `BeginSeq` 注册取消回调，把 `ctx.Err()` 包装进“等待第几步”的错误，并将 `context.AfterFunc` 返回的停止函数保存在 `state.stopWatch`；Rust `syncpoint.rs` 使用本文件的 `Context::err_message`、`after_func` 和 `StopWatch` 复现这一链路。

语义对齐点包括：背景 context 默认不取消；取消原因只保留首次结果；默认取消和 deadline 文案贴近 Go；取消唤醒全部等待者；AfterFunc 回调至多一次；停止函数以布尔值报告是否在回调启动前成功；停止不等待已启动回调结束。`parity_test.rs:145-176` 验证取消会解除错序线程阻塞并由 `EndSeq` 暴露错误，Go `syncpoint.go:118-131` 是对应实现依据。

差异必须保留在认知中：Rust Context 没有父子传播、真实 deadline、value 或接口多态；`with_timeout` 通过不可撤销的睡眠线程模拟 deadline；`after_func` 用线程加 20 毫秒轮询支持停止，而非 Go runtime 的 context 内部机制；错误仅保存字符串，不能进行 Go 的错误身份比较或 `%w` 解包。这里的 Rust `Step` 又固定为零参 `Fn()`，所以该桩只需支持当前同步点取消流程，不能推广为完整 Go context 替代品。

## 扩展指南

若要新增取消原因或查询能力，应优先修改 `ContextInner`、`cancel_with` 及对应只读方法，并保持“先写首错、后发布取消、最后广播”的顺序。任何需要父子传播或真实 deadline 的扩展都已超出当前最小桩边界，应先确认调用方确有需求，并避免悄然改变现有首次取消原因和 clone 共享语义。

若要改变回调生命周期，应集中修改 `after_func` 与 `StopWatch::stop` 的同一原子仲裁协议，同时检查 `syncpoint.rs` 中三处 stop 调用。不得令回调持有 context 等待锁执行，也不得让停止和取消都能执行回调。若增加线程回收保证，需明确是否引入 join、通知 stop 等待者或替代 20 毫秒轮询，并评估测试延迟、死锁和线程数量风险。

测试逻辑必须继续放在独立文件，不应嵌入 `stubs.rs`。应扩展同目录 `parity_test.rs` 覆盖手动取消、首错保留、stop/cancel 竞态和资源清理；面向 `Script` 的顺序行为继续扩展 `syncpoint_test.rs`。若 Go 公开契约变化，还应同步检查 `syncpoint.go` 与 `syncpoint_test.go`。当前测试没有独立逐项覆盖 `wait_cancelled`、重复取消、零时 timeout 和 `StopWatch::stop` 返回值，修改这些行为时应补上定向回归用例。

兼容性风险主要是错误文案、停止布尔语义和取消回调时序；性能风险主要是每个 watcher/timeout 创建操作系统线程以及 20 毫秒轮询。扩展前应避免把该测试专用 API 接入生产请求主链。

## 验证依据

- `br/pkg/utiltest/syncpoint/stubs.rs`：读取完整 174 行，核对 16 个索引符号及共享状态、取消顺序、等待和一次性回调仲裁实现。
- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标目录的 `stubs.rs`、`lib.rs`、`syncpoint.rs`、`parity_test.rs`、`syncpoint_test.rs` 及 Go 对照均已覆盖。
- RustCodeGraph `files --filter br/pkg/utiltest/syncpoint`：确认目录内源文件与独立测试集合；目标文件图关系显示直接由 `syncpoint.rs` 和 `syncpoint_test.rs` 使用。
- RustCodeGraph 对 `stubs.rs`、`lib.rs`、`syncpoint.rs` 的按文件 `node` 查询：确认 crate 装配、再导出，以及 `BeginSeq -> after_func`、取消回调读取 `err_message`、三处 `StopWatch::stop` 生命周期边。
- RustCodeGraph `explore`：确认 `with_timeout` 的三个 `syncpoint_test.rs` 调用者以及 `after_func` 的 `BeginSeq` 调用者；精确 `query after_func --kind function` 定位到 `stubs.rs:147`。
- `br/pkg/utiltest/syncpoint/Cargo.toml`：确认 library crate 边界、Go 包映射、轻量依赖选择及没有 context 外部依赖。
- `br/pkg/utiltest/syncpoint/syncpoint.go` 与 `syncpoint_test.go`：确认 Go `context.AfterFunc`、停止函数保存/调用、取消广播及公开顺序测试意图。
- `br/pkg/utiltest/syncpoint/parity_test.rs`：确认正常、边界、取消和清理契约；其中取消场景直接使用 `with_cancel`/`cancel_with`。
- `br/pkg/utiltest/syncpoint/syncpoint_test.rs`：确认 `with_timeout` 被独立测试用于顺序、回调重叠和无活跃序列场景。任务为纯文档分析，按计划未运行 Cargo。
