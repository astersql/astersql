# `br/pkg/utils/wait.rs`

## 文件定位

`wait.rs` 属于 `astersql-br-pkg-utils` library crate；crate 边界由 `br/pkg/utils/Cargo.toml` 的 `[lib] path = "lib.rs"` 确定。`br/pkg/utils/lib.rs` 以 `#[path = "wait.rs"] pub mod wait` 挂载本文件，并通过 `pub use wait::WaitUntil` 把唯一公开函数提升到 crate 根，因此外部预期入口是 `astersql_br_pkg_utils::WaitUntil`。

这个文件移植自同目录 `br/pkg/utils/wait.go`，负责提供与具体备份、恢复业务无关的同步条件轮询原语。当前仓库搜索只发现 Rust crate 入口和测试引用，没有发现生产 Rust 调用点；也就是说，API 已接线并可被下游使用，但尚不能据现有调用关系断言它已进入 Rust BR 主流程。Go 版的直接生产调用位于 `br/pkg/task/stream.go::waitUntilSchemaReload`，用于等待 schema lease 恢复。

## 核心职责

`WaitUntil` 在调用线程上反复执行一个可变条件闭包，直到出现三个终态之一：条件成立并返回 `Ok(())`，`Context` 被取消并返回 BR 的 `Canceled` 错误，或总等待时间到达 `max_timeout` 并返回超时错误。依据是 `br/pkg/utils/wait.rs::WaitUntil` 的快路径、循环退出分支和 `br/pkg/utils/wait_test.rs::slow_condition_does_not_cause_burst_checks_for_dropped_ticks`。

除基本轮询外，该实现还模拟 Go `time.Ticker` 的两个重要语义：节拍以初始时间为基准按固定速率推进；条件检查耗时跨过多个节拍时，最多保留一个未消费 tick，其余 tick 丢弃，不把积压检查连续突发执行。它不是异步 runtime 定时器，也不创建后台线程或任务。

## 主要符号

- `pub fn WaitUntil(ctx: &Context, mut condition: impl FnMut() -> bool, check_interval: Duration, max_timeout: Duration) -> Result<(), SharedError>`：文件唯一函数和公开 API。`FnMut` 允许闭包在多次探测间维护计数器等局部状态；闭包返回 `true` 表示条件达成。返回错误统一封装为 `astersql_errors::SharedError`。
- `started: Instant` 与 `deadline: Instant`：记录单调时钟起点和绝对截止点。总超时从第一次快路径检查完成后开始计时，而不是从函数入口开始计时，这与 Go 版先执行快路径、再创建 `context.WithTimeout` 相同。
- `next_tick: Instant`：下一个固定速率节拍的目标时间。普通消费后以 `next_tick += check_interval` 推进，避免每轮都从条件检查结束时重新计时。
- `buffered_tick: bool`：模拟容量为一的 ticker 缓冲。当条件执行跨过 `next_tick` 时置位，使下一轮立即进行一次补偿检查；消费后清零。
- `wake_at` 与 `wait`：选择下一 tick 和总 deadline 中更早者，并用 `saturating_duration_since` 计算非负等待时长，交给 `Context::wait_cancelled_timeout` 同时等待时间流逝或取消。

本文件没有模块级常量、类型、trait、`impl`、宏或条件编译项。

## 执行流程

1. `WaitUntil` 首先同步调用一次 `condition()`。若立即为真，直接返回成功；此路径不会检查零间隔，也不会创建任何等待状态。
2. 快路径失败后断言 `check_interval` 非零。零间隔会以 `non-positive interval for NewTicker` panic，对齐 Go `time.NewTicker` 对非正 duration 的拒绝；Rust `Duration` 本身无法表达负值。
3. 以 `Instant::now()` 记录 `started`，计算 `deadline = started + max_timeout` 和首个 `next_tick = started + check_interval`。
4. 每轮先检查 `ctx.is_cancelled()`，再检查当前时间是否达到 deadline。deadline 分支会再次读取取消状态，之后才构造 `TimedOut` 错误。
5. 选择 buffered tick（立即唤醒）或 `next_tick`，再与 deadline 取较早者。`ctx.wait_cancelled_timeout(wait)` 若观察到取消，立即返回 `Canceled`；否则继续判断时间。
6. 唤醒后若已到 deadline，返回超时；否则只在已有 buffered tick 或已到 `next_tick` 时调用条件。
7. 条件为真即成功；条件为假时，如果执行结束已跨过下个节拍，就保留一个 buffered tick，并用纳秒余数计算回到原固定速率网格的下个节拍。这样先执行一次即时补偿检查，随后恢复原 cadence，而不会重放所有错过的 tick。

## 数据与状态

函数自身只持有栈上局部状态，不写全局变量。`condition` 的可变捕获由调用者拥有，在一次调用内按顺序、单线程访问；函数不要求捕获值实现 `Send` 或 `Sync`。

取消状态来自 `br/pkg/utils/stubs.rs::context::Context`。该类型内部以 `Arc<State>` 共享状态，以 `AtomicBool` 表示取消，并用 `Mutex`/`Condvar` 支持可中断等待。`WaitUntil` 只借用 `&Context`，不会取消或派生 context，也不会取得其所有权。`SharedError` 是成功以外的统一返回载体；超时原因是 `std::io::ErrorKind::TimedOut`，取消原因是 `astersql_br_pkg_errors::Canceled`。

时间计算使用单调 `Instant`，不受系统墙钟调整影响。`buffered_tick` 只有布尔容量，因此状态量与错过的 tick 数无关，内存占用恒定。

## 依赖与调用关系

上游接线为 `br/pkg/utils/lib.rs -> wait.rs::WaitUntil`：模块公开且函数在 crate 根再导出。RustCodeGraph 将目标文件标为被 `br/pkg/utils/lib.rs`、`br/pkg/utils/parity_test.rs`、`br/pkg/utils/wait_test.rs` 使用；局部 Rust 搜索未发现生产调用者。Go 对照链为 `br/pkg/task/stream.go::waitUntilSchemaReload -> br/pkg/utils/wait.go::WaitUntil`，其条件读取 `client.GetDomain().IsLeaseExpired()`，错误由调用方补充 `failed to wait until schema reload` 上下文。

直接下游依赖如下：

- `std::time::{Duration, Instant}`：表达轮询间隔、总时限和单调时刻。
- `crate::stubs::context::Context::{is_cancelled, wait_cancelled_timeout}`：读取取消状态，并通过 condvar 进行可被取消唤醒的阻塞等待。
- `astersql_br_pkg_errors::Canceled`：取消时的领域错误值。它对应 Cargo 中的本地依赖 `astersql-br-pkg-errors`。
- `astersql_errors::SharedError`：统一封装取消和标准库 I/O 超时错误；对应 Cargo 中的本地依赖 `astersql-errors`。
- 调用者提供的 `FnMut() -> bool`：真正的业务状态读取完全在闭包中完成，本文件不依赖 schema、PD、TiKV 或网络客户端。

RustCodeGraph 的精确 `callers`/`callees` 查询在当前索引上超时且未返回边，因此上述直接边同时由函数源码、crate 入口和限定范围的引用搜索核验，没有把同名 `WaitUntilFinish` 等符号混入结论。

## 错误处理与边界

- 条件首次即为真：返回 `Ok(())`，即使 context 已取消、间隔为零或总超时为零也不会进入后续检查。这与 Go 源码的快路径顺序一致。
- context 取消：循环前检查、阻塞等待和 deadline 前的复查均可生成 `SharedError(Canceled)`；`Context::cancel` 会通过 condvar 唤醒正在等待的线程，所以通常不必等到下一个 tick。
- 总超时：返回包装 `std::io::ErrorKind::TimedOut` 的 `SharedError`，消息包含 `waitUntil timed out after waiting for {max_timeout:?}`。`br/pkg/utils/parity_test.rs::wait_until_uses_ticker_interval_and_honors_timeout_deadline` 验证长检查间隔不会把总超时拖到下一 tick。
- 零 `check_interval`：仅在快路径失败后 panic。不要把外部可控的零间隔未经校验传入；新增 API 若需要可恢复错误，应显式改变契约并同步 Go 语义与测试，而不是悄悄吞掉该错误。
- 零 `max_timeout`：快路径失败后首次循环即满足 deadline 并返回超时。
- 条件闭包 panic：本函数不捕获 panic，栈展开行为交给调用方；条件闭包也没有 `Result` 返回通道，因此业务读取失败必须由闭包外部状态或更高层 API 表达。
- 条件执行时间计入总耗时，但无法在闭包运行期间抢占；若闭包长时间阻塞，取消和 deadline 只能在闭包返回后被观察。
- deadline 附近的取消存在并发竞态：代码在部分超时路径会复查取消，但唤醒后的 `now >= deadline` 分支直接返回超时。文档不应把所有同时发生情形描述成严格的全序优先级。

## 并发与资源生命周期

`WaitUntil` 是同步阻塞函数：所有条件检查都发生在调用线程且不会重叠。它不 spawn 线程、不建立 channel、不持有异步 task，也不需要显式清理 ticker 资源；所有计时状态随函数返回释放。

等待的可取消性来自 `Context::wait_cancelled_timeout` 的 condvar。调用方可在另一线程调用同一状态的 `Context::cancel`；取消位使用顺序一致性原子读写，`notify_all` 唤醒等待者。`Context` 的父子传播属于 `stubs.rs` 的职责，不由本文件维护。

慢条件的节拍生命周期由 `next_tick` 和 `buffered_tick` 管理。`br/pkg/utils/wait_test.rs::slow_condition_does_not_cause_burst_checks_for_dropped_ticks` 用一次 250ms 的条件执行跨过 100ms tick，验证只保留一个补偿 tick，之后的检查不会形成积压突发。这一行为可限制慢检查后的瞬时负载，但不能限制单次条件自身的资源消耗。

## 与 Go 版本的对应关系

共同语义以 `br/pkg/utils/wait.go::WaitUntil` 为基准：两者都先做一次不受取消/超时影响的快速条件检查；快路径失败后才建立总时限；随后在取消/超时与 ticker 事件间等待；条件为真返回成功；父 context 取消应与普通 timeout 区分。

实现手段不同：Go 通过 `context.WithTimeout`、`time.NewTicker` 和 `select` 管理两个 channel，并以 `defer cancel()`、`defer ticker.Stop()` 清理资源；Rust 通过 `Instant`、局部节拍状态和 `Context` condvar 完成同样的同步效果，不创建 timeout child context 或 ticker 对象。Rust 显式实现 `time.Ticker` 的单槽/丢 tick 语义，相关回归在独立文件 `br/pkg/utils/wait_test.rs`。

错误形态也不同：Go 在父 context 失败时直接返回 `ctx.Err()`，超时使用 `errors.Errorf`；Rust 将取消映射为 `astersql_br_pkg_errors::Canceled`，将总超时表示为包装后的 `std::io::ErrorKind::TimedOut`。Rust 参数使用 `Duration`，只能表达零或正值，因此只需检测零 interval；Go `time.Duration` 可为负，`time.NewTicker` 会对所有非正值 panic。

迁移状态上，Rust 函数和测试已经存在并从 crate 根导出，但尚无生产 Rust 调用点；Go 生产调用仍是当前可验证的应用主链证据。

## 扩展指南

- 修改轮询、超时或取消顺序时，首要修改点是 `wait.rs::WaitUntil`。必须同步检查 `br/pkg/utils/wait.go::WaitUntil` 的语义；若有意产生差异，应在文档和独立测试中明确说明原因。
- 新增边界回归应放在独立的 `br/pkg/utils/wait_test.rs`，不要把测试嵌入生产文件。现有测试适合继续覆盖慢条件、固定 cadence 和 tick 丢弃；取消优先级、零 interval panic、零 timeout、首次条件为真时忽略取消等仍可增加针对性测试。
- crate 级公开契约或再导出变化应同时检查 `br/pkg/utils/lib.rs` 和 `br/pkg/utils/parity_test.rs`。移除或改名 `WaitUntil` 会破坏 crate 根 API。
- 若把条件升级为可失败闭包或异步 future，需要重新设计返回类型与取消机制，不能在现有 `FnMut() -> bool` 内隐式丢弃错误；同时评估是否还要精确保留 Go `time.Ticker` 的单槽行为。
- 不要把条件闭包改成并发执行而不定义重入和顺序契约。当前调用者可以安全依赖 `FnMut` 串行修改捕获状态。
- 性能风险主要来自过小 interval 导致高频唤醒、慢条件占用调用线程，以及取消检查/条件读取的外部成本；兼容风险集中在快路径顺序、deadline 边界、错误类型和固定速率 cadence。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utils` 确认 `wait.rs`、`wait_test.rs`、`lib.rs` 和 Go 对照文件；`node --file br/pkg/utils/wait.rs --offset 1 --limit 240` 读取完整 113 行实现；`query WaitUntil --kind function --json` 区分目标函数与同名/近名符号。精确 `callers`/`callees` 查询超时且无结果，故调用边以局部引用搜索补证。
- 生产源码：`br/pkg/utils/wait.rs`；直接取消依赖：`br/pkg/utils/stubs.rs::context::Context`；crate 入口与再导出：`br/pkg/utils/lib.rs`；crate 元数据和依赖：`br/pkg/utils/Cargo.toml`。
- Go 对照与实际使用：`br/pkg/utils/wait.go::WaitUntil`、`br/pkg/task/stream.go::waitUntilSchemaReload`。
- 独立 Rust 测试：`br/pkg/utils/wait_test.rs::slow_condition_does_not_cause_burst_checks_for_dropped_ticks`；crate 契约测试：`br/pkg/utils/parity_test.rs::wait_until_uses_ticker_interval_and_honors_timeout_deadline` 以及同文件快路径检查。限定搜索未发现同目录 Go 测试直接覆盖 `WaitUntil`。
- 人工复核结论：该文件存在是为了把 Go BR 的可取消、带总时限的条件轮询语义提供给 Rust utils crate；运行方式、节拍状态、错误边界、当前未接入生产 Rust 调用链的事实，以及安全扩展时需同步的测试位置均已在上述章节明确。
