# `br/pkg/checkpoint/ticker.rs`

## 文件定位

`ticker.rs` 属于 Cargo crate `astersql-br-pkg-checkpoint`（`br/pkg/checkpoint/Cargo.toml`），由 crate 入口 `br/pkg/checkpoint/lib.rs` 以 `pub mod ticker` 装配并通过 `pub use ticker::*` 展平导出。它不负责保存 checkpoint 数据，而是为 `checkpoint.rs` 的两个后台循环提供统一的周期事件接口：主循环用它驱动 meta、checksum 和锁续期，刷盘循环用它驱动失败批次重试。

本文件直接对齐 Go 的 `br/pkg/checkpoint/ticker.go`。其存在的主要原因是：Go 的 `select` 可以安全接收 nil channel，从而用 `dispatcherTicker(0)` 禁用可选的锁 ticker；Rust 的 `crossbeam_channel::Select` 不能注册一个不存在的接收端，因此以 `TimeTicker::Ch() -> Option<&Receiver<Instant>>` 显式表达“有周期事件”或“该分支未启用”。

## 核心职责

- `TimeTicker` 抽象真实 ticker 与禁用状态，使调用方只需按 `Ch()` 是否为 `Some` 决定是否向 `Select` 注册分支，并统一用 `Stop()` 请求收尾。
- `dispatcherTicker` 根据 `Duration` 选择实现：正周期启动后台线程；零周期返回不产生事件的 `manualTicker`。
- 正周期实现使用容量为 1 的 crossbeam channel 和 `try_send`，保留至多一个待消费 tick；消费者落后时丢弃后续 tick，模拟 Go `time.Ticker` 的合并/丢 tick 语义并限制内存占用。
- 停止标志在发送前后均被检查，避免已经收到停止请求后再投递新 tick。

该文件不决定各周期的具体值，也不处理 tick 到达后的业务错误。默认 flush/checksum/lock/retry 周期定义在 `br/pkg/checkpoint/checkpoint.rs`，事件对应的刷盘、校验和锁续期逻辑也由该文件处理。

## 主要符号

- `pub trait TimeTicker: Send`：可在线程间转移的 ticker 接口。`Ch(&self)` 返回可选的 `Instant` 接收端；`Stop(&mut self)` 发出幂等停止请求。接口采用 `&mut self`，使停止动作在调用点具有独占访问语义。
- `pub struct timeTicker`：正周期实现。`rx` 是消费者侧接收端；`stop: ArcStop` 持有共享原子停止位；`_handle: Option<JoinHandle<()>>` 保存后台线程句柄，避免构造后立即丢弃句柄，但当前实现不调用 `join`。
- `struct ArcStop(Arc<AtomicBool>)`：私有的新类型包装，只用于在 `timeTicker` 与后台线程间共享停止状态。
- `impl TimeTicker for timeTicker`：`Ch` 恒返回 `Some(&rx)`；`Stop` 以 `Ordering::SeqCst` 将停止位置为 `true`。
- `pub struct manualTicker {}`：零周期占位实现，不创建线程、不持有 channel。
- `impl TimeTicker for manualTicker`：`Ch` 恒返回 `None`，`Stop` 是空操作。
- `pub fn dispatcherTicker(d: Duration) -> Box<dyn TimeTicker>`：本文件唯一构造入口。返回 boxed trait object，使 `checkpoint.rs` 能以相同类型持有两种实现。

虽然 `timeTicker` 和 `manualTicker` 类型本身是公开的，字段均不可从 crate 外构造；正常接入点应是 `dispatcherTicker`。命名保留 Go 风格，crate 根也通过 lint allow 接受非 Rust 惯用的大小写。

## 执行流程

正周期路径（`d > Duration::ZERO`）如下：

1. `dispatcherTicker` 创建容量为 1 的 `(Sender<Instant>, Receiver<Instant>)`。
2. 创建初值为 `false` 的 `Arc<AtomicBool>`，一份保存在 `timeTicker`，一份移入后台线程。
3. 后台线程在停止位仍为 `false` 时执行 `thread::sleep(d)`。
4. 睡眠返回后再次检查停止位；若已停止则直接退出，避免发送尾部 tick。
5. 未停止时以 `tx.try_send(Instant::now())` 非阻塞投递。槽位已满或接收端已断开时，错误被忽略；循环进入下一周期。
6. 调用方通过 `Ch()` 取得接收端并注册到 `crossbeam_channel::Select`。`checkpoint.rs::CheckpointRunner::startCheckpointMainLoop` 分别创建 flush、checksum、lock 三个 ticker；`startCheckpointFlushLoop` 另建 retry ticker。
7. 循环退出时，调用方对 ticker 调用 `Stop()`。停止并不唤醒正在 sleep 的线程；线程最迟在当前周期睡眠完成、复查停止位后退出。

零周期路径直接返回 `manualTicker`。其 `Ch()` 为 `None`，因此 `checkpoint.rs` 用 `Option::map` 跳过相应 `Select` 分支。`parity_test.rs` 直接验证了 `dispatcherTicker(Duration::ZERO)` 无 channel 且可以安全停止。

## 数据与状态

每个正周期 ticker 的独立状态仅有三部分：一个单槽 channel、一个原子布尔停止位和一个线程句柄。tick 的载荷是创建事件时的单调时钟 `Instant`；当前 checkpoint 调用方只把它当作脉冲，接收后不读取时间值参与持久化。

容量 1 是重要不变量：未消费的事件最多一个。`try_send` 不等待消费者，因而慢消费者不会反压阻塞 ticker 线程，也不会形成无界事件积压。它同时意味着 tick 不是“每次必达”的任务队列；调用方只能依赖“周期性唤醒”，不能用接收数量计算经过周期数。

`stop` 只从 `false` 单向变为 `true`，没有重启路径。`Stop()` 可重复调用。`manualTicker` 完全无可变状态。`Duration` 在 Rust 中无负值，因此分派边界只有零与正数，不存在 Go `time.Duration` 的负周期输入。

## 依赖与调用关系

直接标准库依赖是 `std::thread::{spawn, JoinHandle}`、`std::time::{Duration, Instant}` 和 `std::sync::atomic::{AtomicBool, Ordering}`；唯一第三方依赖是 `crossbeam-channel = "0.5"`，由 `br/pkg/checkpoint/Cargo.toml` 声明。

已核对的上游接线包括：

- `br/pkg/checkpoint/checkpoint.rs::CheckpointRunner::startCheckpointMainLoop` 调用 `dispatcherTicker` 创建 flush、checksum、lock ticker，并把存在的接收端注册到 `Select`；对应事件分别调用 `flushMeta_internal`、`flushChecksum_internal` 和 `setLock_internal`。
- `br/pkg/checkpoint/checkpoint.rs::startCheckpointFlushLoop` 创建 retry ticker；tick 到达后调用 `Flusher::flushOneIncomplete` 重试一个失败批次。
- `br/pkg/checkpoint/ticker_test.rs::test_dispatcher_ticker_drops_ticks_when_receiver_is_slow` 构造正周期 ticker，读取首个事件后故意放慢消费者，验证排队事件不超过一个。
- `br/pkg/checkpoint/parity_test.rs` 构造零周期 ticker，验证禁用分支的 Go/Rust 契约。

下游没有异步 runtime、存储或网络依赖。线程只生成 `Instant` 并向本地 channel 尝试发送；具体 checkpoint 状态、错误通道和存储实现均在 `checkpoint.rs` 及其依赖中。

## 错误处理与边界

本 API 不返回 `Result`。后台发送使用 `let _ = tx.try_send(...)`，明确吞掉两类非致命情况：channel 已满表示消费者落后，应丢弃当前 tick；接收端已断开表示事件无人消费，也不会把错误上报给 checkpoint 主循环。

`d == Duration::ZERO` 不会传入 `thread::sleep` 或创建忙循环，而是产生无 channel 的 manual 实现。这与 Go `dispatcherTicker(0)` 返回 nil channel 的意图一致，也规避 Go `time.NewTicker(0)` 会 panic 的前置条件。Rust `Duration` 不表示负数，所以无法一比一接收 Go 的负 duration；若未来从有符号配置解析周期，必须在进入本函数前定义负值策略。

需要特别注意：`Stop()` 只设置标志，不等待后台线程退出，也不会中断当前 `sleep`。若在一个很长周期上停止，线程会保留到睡眠结束。当前实现也没有 `Drop` 自动置停止位；如果正周期 ticker 未调用 `Stop()` 就被丢弃，后台线程的发送会持续失败并继续循环，形成线程泄漏。因此所有新调用路径都必须像 `checkpoint.rs` 的循环收尾一样显式停止。

## 并发与资源生命周期

每个正周期 ticker 创建一个 OS 线程。停止位通过 `Arc<AtomicBool>` 共享，并采用最强的 `SeqCst` 顺序保证跨线程可见性；除此之外没有锁和共享可变数据。channel 的 `try_send` 非阻塞，生产线程不会因为调用方持锁或处理缓慢而卡住。

资源生命周期为“构造 ticker → 调用方持有 trait object 和接收端 → 后台线程周期投递 → 调用方 `Stop` → 后台线程在 sleep 返回后退出”。`JoinHandle` 存在于结构体中但没有被 `take`/`join`；结构体销毁时句柄被丢弃，Rust 会将线程分离，而不是等待线程结束。因此 `Stop` 是请求退出而非同步退出屏障。若扩展为需要严格停稳的资源管理，必须同时设计可唤醒等待与 join，不能只在现有 `Stop` 后直接 join，否则最长会阻塞一个完整周期。

`TimeTicker` 只要求 `Send`，未承诺 trait object 可被多个线程通过共享引用并发使用。当前设计是创建和停止都由所属循环线程控制，只有内部原子位与生产线程共享。

## 与 Go 版本的对应关系

Rust `TimeTicker::{Ch, Stop}` 对应 Go `TimeTicker` 接口；`timeTicker`、`manualTicker` 和 `dispatcherTicker` 也逐名对应 `br/pkg/checkpoint/ticker.go`。正周期的外部行为目标是 Go `time.Ticker`：周期产生时间事件、慢接收者不会积累无界事件、停止后不再产生新事件。零周期的 Rust `None` 对应 Go nil channel，使 `select` 分支永久不可选。

实现方式存在以下差异：

- Go 正周期实现直接包装运行时 `time.Ticker`；Rust 显式创建线程、单槽 crossbeam channel 和原子停止位。
- Go `time.Ticker.Stop` 由运行时取消计时；Rust `Stop` 不能唤醒 `thread::sleep`，退出可能延迟一个周期。
- Rust 通过 `try_send` 明确丢弃槽位已满的 tick；这是对 Go ticker 慢消费者语义的有意模拟，`ticker_test.rs` 对此有直接回归覆盖。
- Go 主循环只对可选 lock ticker 使用 `dispatcherTicker`，flush/checksum/retry 直接使用 `time.NewTicker`；Rust 为适配统一的 `crossbeam_channel::Select`，四类周期都复用本文件实现。业务触发点仍与 Go 对齐。
- Go duration 可为负，但 `dispatcherTicker` 将非正数都视为 manual；Rust `Duration` 不可为负，只有零会进入 manual 分支。

## 扩展指南

新增 ticker 使用点时，应从 `dispatcherTicker` 构造，通过 `Ch()` 的 `Option` 决定是否注册选择分支，并保证所有退出路径调用 `Stop()`。不要直接构造公开但字段私有的实现类型，也不要把 tick 当作可靠计数消息。

若修改 channel 容量、由 `try_send` 改为阻塞发送，或改成无界 channel，必须先评估慢 checkpoint 存储下的线程阻塞与内存增长，并同步扩展 `br/pkg/checkpoint/ticker_test.rs` 的丢 tick 回归。若改变零周期语义，需要同步检查 `checkpoint.rs` 中可选 lock ticker 的 `Option::map` 注册方式和 `parity_test.rs`。

若要修复停止延迟或未显式停止时的线程泄漏，建议引入可唤醒的停止通道/条件变量并在 `Stop` 或 `Drop` 中完成确定性收尾；同时避免工作线程 join 自身，并为“长周期能够快速停止”“重复 Stop 幂等”“drop 后无线程继续运行”增加独立测试。Rust 单元测试应继续放在同目录的 `ticker_test.rs`，不要内嵌回生产文件。

修改 Go/Rust 公共契约时，应同时复核 `br/pkg/checkpoint/ticker.go`、`checkpoint.go`、`checkpoint.rs` 和两个 Rust 测试文件；性能风险集中在线程数量、唤醒频率、丢 tick 策略及停止等待时间。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/checkpoint` 确认目标源、Go 对照、crate 入口和独立测试均已索引。
- RustCodeGraph `node --file br/pkg/checkpoint/ticker.rs`：核对了全部 96 行、所有 trait/结构体/实现/函数及无条件编译结构；该文件没有 `cfg` 分支。
- RustCodeGraph `explore 'br/pkg/checkpoint/ticker.rs symbols callers callees checkpoint ticker'`：确认 `dispatcherTicker` 的 Rust 测试调用边，以及 `Ch`/`Stop` 在目标与测试中的调用关系。精确 `callers/callees` 因 Go/Rust 同名符号查询超时，未将其作为唯一依据。
- RustCodeGraph 对 `br/pkg/checkpoint/checkpoint.rs` 的文件节点：确认主循环第 572–574 行的三类 ticker、`Select` 注册、退出时第 691–693 行的停止，以及刷盘循环第 886、918 行的 retry ticker 接线。
- `br/pkg/checkpoint/Cargo.toml` 与 `lib.rs`：确认 crate 名、library 边界、`crossbeam-channel` 依赖、模块装配和测试文件挂载；目录中不存在 `doc.go`。
- `br/pkg/checkpoint/ticker.go` 与 `checkpoint.go`：核对 Go 接口、nil-channel manual ticker、`time.NewTicker` 行为及主/刷盘循环的实际业务接线。
- `br/pkg/checkpoint/ticker_test.rs`：核对单槽丢 tick 的回归断言；`br/pkg/checkpoint/parity_test.rs`：核对零周期 manual ticker 和默认周期的对齐断言。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构检查要求目标文档恰好包含规定的 11 个二级章节。
