# `pkg/statistics/handle/usage/collector/collector.rs`

## 文件定位

本文件实现统计 usage 子系统可复用的“会话增量 → 节点全局状态”异步收集框架。它属于独立 crate `astersql-statistics-handle-usage-collector`；crate 入口 `pkg/statistics/handle/usage/collector/lib.rs` 公开 `collector` 模块并再导出本文件的公开项，crate 清单 `pkg/statistics/handle/usage/collector/Cargo.toml` 只引入 `crossbeam-channel = "0.5"`（测试另用 `rand`）。

当前生产侧直接使用者是 `pkg/statistics/handle/usage/indexusage/collector.rs`：其中 `Collector::NewCollector` 用 `NewGlobalCollector` 注册 `mergeDelta`，`Collector::{StartWorker, Close, SpawnSessionCollector}` 分别转发本文件的 worker、关闭和会话派生能力；`SessionIndexUsageCollector::{Report, Flush}` 分别调用 `SendDelta` 与 `SendDeltaSync`。因此本文件不理解索引统计的数据结构，只负责有界排队、调度、关闭和回调执行。

## 核心职责

1. `NewGlobalCollector` 建立普通数据、高优先级数据和关闭信号三组通道，并保存调用方提供的 `merge_fn`。
2. `globalCollector::SpawnSession` 为每个会话克隆发送端和关闭接收端，同时独立记录最后一次成功投递时间。
3. `globalCollector::StartWorker` 启动后台线程，优先处理已排队的高优先级增量，再等待任一数据通道或关闭信号。
4. `sessionCollector::SendDelta` 提供通常不阻塞、允许失败的快速路径；长期未成功上报时转入 `SendDeltaSync`，避免增量永久饥饿。
5. `globalCollector::Close` 只执行一次，广播关闭、等待所有 worker，并依靠 worker 尾部的 `flush` 合并已经入队的残留增量。

该框架只承诺处理“成功进入通道”的值。`SendDelta` 返回 `false` 的值没有被框架接管，调用方必须保留或重试；`indexusage::SessionIndexUsageCollector::Report` 正是仅在返回 `true` 后才替换本地增量容器。

## 主要符号

- `DEFAULT_TIMEOUT: Duration`：五分钟。会话在此期间没有成功投递时，下一次 `SendDelta` 改为同步高优先级发送。
- `DEFAULT_CHANNEL_SIZE: usize`：普通通道和高优先级通道各自的容量，值为 10。
- `GlobalCollector<T>`：公开 trait，定义 `SpawnSession`、`StartWorker`、`Close`。其实现只是转发到 `globalCollector<T>` 的固有方法。
- `SessionCollector<T>`：公开 trait，定义 `SendDelta` 和 `SendDeltaSync`。其实现转发到 `sessionCollector<T>` 的固有方法。
- `globalCollector<T>`：公开具体类型，内部保存 `merge_fn`、两组有界通道、关闭通道、worker 句柄、一次性关闭门闩、关闭原子标记和超时值；字段均为私有。
- `sessionCollector<T>`：公开会话句柄类型，持有两个发送端、关闭接收端、超时值及 `last_update`；字段均为私有，每个派生会话有自己的时间状态。
- `NewGlobalCollector<T: Send + 'static>`：公开构造器，接收 `Fn(T) + Send + Sync + 'static` 回调并返回具体的 `globalCollector<T>`。
- `globalCollector::{SpawnSession, StartWorker, Close}`：全局实例的生命周期和会话工厂 API。
- `sessionCollector::{SendDelta, SendDeltaSync}`：会话实例的快速投递和保证等待投递 API。
- `flush`：私有函数，以高优先级优先的顺序非阻塞排空两个接收端。

本文件没有条件编译项；测试挂载发生在相邻的 `lib.rs` 中，而不是生产实现中。

## 执行流程

典型路径如下：

1. 上层调用 `NewGlobalCollector(merge_fn)`。构造器创建容量各为 10 的普通/高优先级通道，以及容量为 0 的关闭通道；关闭 sender 被包在 `Mutex<Option<_>>` 中，以便通过 `take` 丢弃。
2. 上层调用 `StartWorker`。方法先锁住 worker 列表并检查 `closed`；未关闭时克隆回调和三个 receiver，启动线程后把 `JoinHandle` 保存到列表。该 API 没有限制只能调用一次，因此关闭前每调用一次都会增加一个消费线程。
3. worker 的外层 `select_biased!` 先检查高优先级队列和关闭信号；若二者当时均未就绪，进入普通 `select!`，阻塞等待普通数据、高优先级数据或关闭。收到数据即在 worker 线程中直接调用 `merge_fn(data)`。
4. 会话快速上报时，`SendDelta` 先检查 `last_update.elapsed() > timeout`。未超时时对普通通道执行 `try_send`：成功则更新时间并返回 `true`，通道满等错误返回 `false` 且不更新时间；已超时则把原值移交 `SendDeltaSync`。
5. `SendDeltaSync` 同时等待高优先级发送和关闭信号。发送成功才更新时间并返回 `true`；发送端失败或关闭先就绪则返回 `false`。
6. `Close` 在 `Once` 中先以 Release 顺序写入 `closed = true`，再取走并丢弃唯一的关闭 sender。所有关闭 receiver 因断开而就绪，worker 和可能阻塞的同步发送者被唤醒。随后逐个 `join` 已登记 worker。
7. worker 离开主循环后调用 `flush`。每轮先尝试高优先级接收；没有高优先级值时再尝试普通接收；两个队列均空或断开才返回。因而 `Close` 返回前，已入队且能被这些 receiver 看见的数据会交给 `merge_fn`。

## 数据与状态

`T` 是所有权转移的增量值。发送成功后值由通道和 worker 所有，最终按值传给 `merge_fn`；发送失败时 API 只返回布尔值。特别是 `try_send` 的错误中携带的原值在当前实现里被丢弃，所以调用方不能从 `SendDelta(false)` 取回值，应像 index usage 调用方那样发送可共享句柄并在成功前保留自己的状态。

两个数据通道彼此独立，各有 10 个槽位。高优先级只表示 worker 在“已有高优先级值可取”时优先选择；进入内层无偏 `select!` 后，如果普通和高优先级同时就绪，选择次序不构成严格全序保证。多个 worker 启动时也会并发执行 `merge_fn`，因此回调必须满足其 `Send + Sync` 约束并自行同步共享状态。

`last_update` 是单个 `sessionCollector` 的局部状态，初值为派生时刻，只在任一发送路径成功时刷新。失败的普通发送会让它继续老化，最终触发同步路径。`closed` 只阻止新的 `StartWorker`；真正唤醒各线程的是 `close_sender.take()` 导致的通道断开。`close_once` 保证重复 `Close` 不会重复关闭或 join。

## 依赖与调用关系

上游生产调用边（由 RustCodeGraph 文件关系和调用位置核对）：

- `indexusage::NewCollector` → `collector::NewGlobalCollector`，回调为 `mergeDelta`。
- `indexusage::Collector::SpawnSessionCollector` → `globalCollector::SpawnSession`。
- `indexusage::Collector::{StartWorker, Close}` → 同名全局收集器方法。
- `indexusage::SessionIndexUsageCollector::Report` → `sessionCollector::SendDelta`；仅成功后清空/替换待上报增量。
- `indexusage::SessionIndexUsageCollector::Flush` → `sessionCollector::SendDeltaSync`。

本文件内部调用边为：`SendDelta` 在超时分支调用 `SendDeltaSync`；worker 线程退出前调用 `flush`；两个 trait impl 调用对应的固有方法。下游库依赖集中在 `crossbeam_channel::{bounded, select, select_biased, Sender, Receiver, TryRecvError}`；标准库提供 `Arc`、`Mutex`、`Once`、`AtomicBool`、线程句柄和单调时间 `Instant`。

Cargo 层面，`pkg/statistics/handle/usage/indexusage/Cargo.toml` 以路径 `../collector` 和依赖别名 `usage-collector` 引用本 crate；根 `Cargo.toml` 同时登记 workspace facade `facade_statistics_handle_usage_collector`。父目录 `pkg/statistics/handle/usage/Cargo.toml` 的业务依赖位于永不启用的 `cfg(any())` 表中，不是本 crate 的运行时调用证据。

## 错误处理与边界

本 API 不返回 `Result`，通道拥塞、断开和关闭都压缩为布尔成功/失败：

- `SendDelta` 的 `try_send` 任何错误均为 `false`，不区分“满”和“断开”。这是允许背压拒绝的路径。
- `SendDeltaSync` 在高优先级发送完成前阻塞；关闭信号或发送失败时返回 `false`。名称中的“Sync”表示同步等待入队，不表示等待 `merge_fn` 已执行完毕。
- `Close` 不返回错误；如果任一 worker 或其中执行的 `merge_fn` panic，`join().expect("collector worker panicked")` 会让关闭调用者 panic。
- `Mutex::lock().unwrap()` 在锁中毒时 panic，包括 worker 列表和关闭 sender 两处。框架没有恢复策略。
- 构造后若从未 `StartWorker`，普通通道最多接受 10 个值；高优先级通道也最多接受 10 个，之后 `SendDeltaSync` 会阻塞到 worker 消费或 `Close` 断开关闭通道。
- `Close` 只排空已经进入通道的数据，不会恢复先前被 `SendDelta(false)` 拒绝的值，也不会自动刷新仍保留在上层会话对象里的聚合状态。

关闭与启动的交错由 `closed` 和 worker 列表锁共同约束：关闭标记写入后新启动直接返回；若一次启动已经通过检查并持有列表锁，关闭会在释放该锁后取得并 join 新登记的线程。关闭后派生会话当前仍被允许，但其同步发送会观察到已断开的关闭通道并失败，普通发送可能在未满的仍存活数据通道上暂时成功；调用方应把 `Close` 视为禁止继续派生或上报的生命周期终点。

## 并发与资源生命周期

`globalCollector` 可被共享引用并由内部同步原语协调：worker 列表和关闭 sender 用 `Mutex`，关闭状态用 Acquire/Release 原子读写，一次性关闭用 `Once`。`SpawnSession` 克隆 channel handle，因此会话可移到其他线程；`T: Send + 'static` 约束出现在构造和全局 worker API 上。`merge_fn` 置于 `Arc<dyn Fn(T) + Send + Sync>`，允许每次启动的 worker 共享调用。

资源所有权从构造开始，到 `Close` 完成所有已登记线程的 join 为止。丢弃关闭 sender 是广播机制：容量为 0 的通道不传递具体消息，断开状态被所有克隆的 receiver 观察到。数据 sender/receiver 本身随全局对象、会话和 worker 的克隆生命周期释放；本文件没有为 `globalCollector` 实现 `Drop`，因此调用方若不显式 `Close`，worker 可能继续阻塞，而持有的 `JoinHandle` 仅在结构析构时被丢弃，并不会自动 join。

`flush` 串行运行于每个正在退出的 worker。如果启动了多个 worker，它们可同时排空并并发调用回调，处理顺序不可依赖。即使只有一个 worker，高优先级优先也只保证选择策略，不保证跨两个通道的全局 FIFO；每个单独 crossbeam 通道内部保留自己的发送顺序。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/statistics/handle/usage/collector/collector.go`。常量（五分钟、容量 10）、双数据通道、嵌套 select 的高优先级倾向、会话超时降级、关闭后 flush、可多次启动 worker 和一次性关闭的总体结构一致。Go 的 goroutine/`sync.WaitGroup` 对应 Rust 的线程/`Vec<JoinHandle<()>>`，`sync.Once` 对应 `Once`，关闭 `closeCh` 对应丢弃 Rust 的 `close_sender`。

需要注意以下当前实现差异：

- Go `NewGlobalCollector` 返回 `GlobalCollector[T]` 接口，Rust 返回公开具体类型 `globalCollector<T>`，同时另行实现 trait。
- Rust `SpawnSession` 明确克隆 `close_receiver`；当前 Go `SpawnSession` 初始化式没有把 `g.closeCh` 赋给 `sessionCollector.closeCh`，因此 Go 会话的关闭 case 实际为 nil channel，而 Rust 的阻塞同步发送可被 `Close` 唤醒。Rust 独立测试把这一行为作为迁移期保证。
- Rust 用 `closed` 阻止 `Close` 之后启动新 worker；Go 没有对应字段，关闭后调用 `StartWorker` 会启动一个观察到已关闭 `closeCh`、随后 flush 并退出的 goroutine。
- Rust 的 `SendDelta` 在 `try_send` 失败时消费并丢弃传入值；Go 传值语义也不把失败值返回，但调用方变量本身仍可继续使用。当前 index usage 发送的是共享增量句柄，并由外层在成功前保留状态，符合两侧测试意图。

Go 测试 `collector_test.go` 与 Rust `collector_test.rs` 都覆盖：单会话普通发送的“接受数等于最终合并数”、多会话并发普通发送的同一不变量，以及并发同步发送无丢失。Rust 的 `migration_aster_unit_test.rs` 额外覆盖高优先级队列填满且无 worker 时，`Close` 解除阻塞并使发送返回 `false`。

## 扩展指南

- 若要让容量或超时可配置，最小接入点是 `NewGlobalCollector` 和 `globalCollector::{timeout, data/high_priority channel}` 的构造；同步修改 `SpawnSession` 的超时复制逻辑，并在独立的 `collector_test.rs` 或 `migration_aster_unit_test.rs` 增加零/小容量、超时边界测试。不要把测试嵌进本文件。
- 若要增强错误可观测性，应重新设计 `SessionCollector` 返回类型，并明确区分队列已满、全局已关闭和 receiver 断开；这会影响 `indexusage::SessionIndexUsageCollector::{Report, Flush}` 的状态保留逻辑及 Go API 兼容性。
- 若要保证 `Flush` 真正等待合并完成，仅“成功发送”不够，需要增量携带确认机制或新增屏障消息；必须同时处理多 worker、关闭竞态和回调 panic，不能把现有 `SendDeltaSync` 误当作完成确认。
- 若修改优先级或顺序语义，应集中审查 `StartWorker` 的两层 select 与 `flush` 的取数顺序。严格顺序需要单队列带优先级或序号协议，当前双通道无法提供跨通道 FIFO。
- 若添加自动清理，可考虑 `Drop`，但 `Drop` 中阻塞 join 可能死锁或造成不可控延迟；应先规定 worker 数量、回调重入和最后一个会话 sender 的所有权规则。
- 任何生命周期改动都应同步 Go 对照或明确记录迁移差异，并至少保留四类独立测试：普通路径接受计数、并发普通路径、并发同步路径、关闭解除阻塞；涉及 index usage 状态时还需同步 `pkg/statistics/handle/usage/indexusage/collector_test.rs`。

兼容风险主要是公开命名/返回类型和布尔语义；正确性风险集中在关闭与发送竞态、回调并发和未显式关闭；性能风险集中在有界容量导致的拒绝、同步发送阻塞、每次 `StartWorker` 创建 OS 线程，以及 `merge_fn` 在 worker 中直接执行造成的消费停顿。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/usage/collector` 确认实现、crate 入口及三份测试文件均已索引。
- RustCodeGraph `node --file pkg/statistics/handle/usage/collector/collector.rs --offset 1 --limit 420`：读取本文件完整 259 行，并确认它被 `collector_test.rs`、`migration_aster_unit_test.rs` 和 `indexusage/collector.rs` 等文件使用。
- RustCodeGraph 对 `NewGlobalCollector`、`StartWorker`、`SpawnSession`、`SendDelta`、`SendDeltaSync`、`flush` 执行过 `query`、`callers`、`callees`/`explore` 查询；常见短名产生跨仓库噪声，因此调用边又以已索引的 `indexusage/collector.rs` 第 150–329 行及精确调用位置复核。
- 源与配置证据：`collector.rs`；`collector/Cargo.toml`；`collector/lib.rs`；`indexusage/collector.rs`；`indexusage/Cargo.toml`；根 `Cargo.toml` 的 workspace facade 条目。
- 对照与测试证据：`collector/collector.go`；`collector/collector_test.go`；`collector/collector_test.rs`；`collector/migration_aster_unit_test.rs`；调用方测试位于 `indexusage/collector_test.rs` 和 `indexusage/migration_aster_unit_test.rs`。
- 人工事实复核：文档中的队列容量、超时、优先选择、失败布尔值、关闭广播、flush、panic 边界、多 worker 行为及 Go/Rust 差异均能回指上述符号和文件；没有把未运行的测试结果表述为已验证。本任务按要求不运行 Cargo。
