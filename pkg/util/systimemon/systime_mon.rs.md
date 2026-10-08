# [`pkg/util/systimemon/systime_mon.rs`](./systime_mon.rs)

## 文件定位

该文件是 `astersql-util-systimemon` crate 的核心实现，负责检测进程所见的系统墙钟是否发生回退。crate 入口 `pkg/util/systimemon/lib.rs` 以私有模块加载本文件，并公开再导出其唯一 API `StartMonitor`；根 crate 又在 `pkg/lib.rs` 的 `pkg::util::systimemon` 门面中再导出该能力。`pkg/util/systimemon/Cargo.toml` 声明该 crate 仅依赖 `log`，没有 feature 开关。

需要区分“能力已导出”和“生产入口已接线”：RustCodeGraph 当前只找到三个对本文件 `StartMonitor` 的直接调用，全部位于独立 Rust 测试。`cmd/tidb-server/main.rs::setupMetrics` 虽有同名调用，但其 `systimemon` 来自 `crate::stubs`，实际落到 `cmd/tidb-server/stubs.rs::systimemon::StartMonitor`，不是本文件。Go 生产入口 `cmd/tidb-server/main.go::setupMetrics` 则调用 `pkg/util/systimemon/systime_mon.go::StartMonitor`。

## 核心职责

- 以 100 ms 固定节拍比较相邻两次墙钟采样；当前值严格早于上一值时，认定发生时间回退。
- 通过注入的 `now: FnMut() -> SystemTime` 获取时间，使检测逻辑不绑定 `SystemTime::now`，也可由测试构造确定性的回退时钟。
- 通过注入的 `systimeErrHandler: FnMut()` 把异常后的处置交给上层；函数本身只记录错误日志，不修改指标、不终止进程。
- 使用单调时钟 `Instant` 维护采样节拍，避免墙钟本身的回退影响调度；慢采样后推进到下一个尚未到达的节拍，而不是从慢采样结束时重新等待完整 100 ms。

这里检测的是相邻采样窗口内可观察到的净回退，而不是持续跟踪历史最大墙钟值。若回退和随后前进在两个采样点之间相互抵消，或回退幅度不足以让第二次采样早于第一次采样，均不会触发回调。

## 主要符号

- `pub fn StartMonitor<Now, SystimeErrHandler>(mut now: Now, mut systimeErrHandler: SystimeErrHandler)`：文件唯一的模块级函数和公开 API。`Now: FnMut() -> SystemTime` 与 `SystimeErrHandler: FnMut()` 允许两个闭包保有并修改捕获状态。
- `interval: Duration`：函数内固定为 100 ms，当前不是参数或常量，决定检测粒度和正常情况下的最大发现延迟。
- `next_tick: Instant`：下一个计划节拍的单调时钟时间点；初始化为函数启动时刻加一个间隔，此后只按固定间隔累加。
- `last: SystemTime`：每轮等待前取得的墙钟基准，只活到当前循环比较完成。
- `tick_observed_at: Instant`：睡眠返回后读取的单调时间，用于一次性跳过已经错过的所有节拍。

函数沿用 Go 导出名 `StartMonitor`，因此以 `#[allow(non_snake_case)]` 局部关闭 Rust 命名告警。文件没有类型、trait、模块级常量、静态状态或条件编译项。

## 执行流程

1. `StartMonitor` 先以 `log::info!` 记录监控启动。
2. 创建 100 ms 间隔，并用 `Instant::now() + interval` 计算首个计划节拍。
3. 每轮循环先调用一次 `now()`，把返回的 `SystemTime` 保存为 `last`。
4. 计算 `next_tick.saturating_duration_since(Instant::now())` 并调用 `thread::sleep`。若调用 `now()` 或调度延迟已经越过计划节拍，饱和差值为零，不会因负时长出错。
5. 睡眠返回后读取 `tick_observed_at`；只要 `next_tick <= tick_observed_at`，便反复增加 100 ms，最终使 `next_tick` 指向未来的第一个固定节拍。这保留了 Go `time.Ticker` 在消费者变慢时仍继续计时的语义。
6. 再调用一次 `now()`；仅当新值严格小于 `last` 时记录错误日志并同步调用 `systimeErrHandler()`。相等或向前移动均不触发。
7. 回调返回后进入下一轮；函数没有退出分支，正常情况下永不返回。

## 数据与状态

所有运行状态都位于一次 `StartMonitor` 调用的栈和两个闭包内部，没有全局可变数据。调度状态使用 `Instant`，检测数据使用 `SystemTime`：前者保证节拍不受墙钟校准影响，后者保留需要被检测的系统时间变化。

`last` 每轮重置，因而不保存“启动以来最大时间”。两个 `FnMut` 参数的捕获状态由调用者拥有；本函数按“每轮两次 `now`、检测到回退时零次或一次 handler”的顺序串行访问它们。`SystemTime` 值被按严格顺序比较，日志中的 `last={last:?}` 仅输出回退前样本，不输出回退后的样本。

## 依赖与调用关系

- crate 装配：`pkg/util/systimemon/lib.rs` 用 `#[path = "systime_mon.rs"] mod systime_mon;` 加载实现，并用 `pub use systime_mon::*` 导出 `StartMonitor`。
- Cargo 边界：`pkg/util/systimemon/Cargo.toml` 的生产依赖只有 `log = "0.4"`；`testsetup` 仅为开发依赖。根 `Cargo.toml` 以 `facade_util_systimemon` 指向此 crate，`pkg/lib.rs` 再将其暴露为 `pkg::util::systimemon`。
- 标准库下游：`std::thread::sleep` 负责阻塞当前线程；`Duration` 表示间隔；`Instant` 驱动固定节拍；`SystemTime` 是待检测墙钟。
- 日志下游：`log::info!` 记录启动，`log::error!` 记录检测到的回退。
- RustCodeGraph 对 `systime_mon.rs::StartMonitor` 的调用边为 `migration_aster_unit_test.rs::detects_a_backward_system_time_jump`、`systime_mon_test.rs::test_systime_monitor` 和 `systime_mon_test.rs::slow_sample_does_not_restart_the_tick_interval`；图中没有生产调用边，也没有可解析的函数级 callee 边。
- 应用主链现状：`cmd/tidb-server/main.rs::setupMetrics` 在独立线程调用的是 `cmd/tidb-server/stubs.rs` 中的同名桩，该桩只记录事件且立即返回。若未来把入口改接本 crate，预期回调 `metrics::TimeJumpBackCounter.Inc()` 才会由这里的真实循环触发；当前不能据此宣称 Rust server 已启用真实时间回退监控。

## 错误处理与边界

本 API 没有 `Result` 返回值，也没有内部恢复或重试分支。`thread::sleep` 和 `SystemTime` 比较在此处不产生可传播错误；检测到回退被当作可报告事件，记录日志后调用 handler，随后继续监控。

`now` 或 `systimeErrHandler` 若 panic，panic 会沿当前执行线程展开并终止该次监控；函数不捕获 panic。handler 若永久阻塞，后续采样停止；handler 若执行很慢，固定节拍追赶逻辑会跳过已经过去的节拍。函数没有取消令牌、关闭通道或最大迭代次数，不能由 API 自身优雅停止。

边界语义包括：严格小于才算回退；首轮也必须等待一个节拍后才能判断；检测精度受 100 ms 采样和线程调度影响；只比较相邻样本；日志只含旧样本。调用者还必须保证监控运行在可长期阻塞的线程上，否则直接调用会永久占用当前线程。

## 并发与资源生命周期

`StartMonitor` 自身不创建线程，它在调用线程中无限循环并同步执行两个闭包。生产或测试调用者若不希望被阻塞，必须显式 `thread::spawn`；本文件的泛型约束因此没有要求 `Send` 或 `'static`，这些约束只会在调用者把闭包移动进线程时由 `thread::spawn` 施加。

函数不持有锁、通道、文件描述符或 ticker 对象。唯一持续资源是执行它的线程及闭包捕获的数据。`next_tick` 的生命周期覆盖整个循环，保证节拍锚定在初始单调时间线上。由于没有停止机制，正常结束时没有清理阶段；Rust 测试创建的后台线程也不会被 join，Go 的 `main_test.go` 因此把 Go 版 `StartMonitor` 列入 goroutine 泄漏检查白名单。

多个调用实例彼此没有共享内部状态，可以并行运行；是否安全共享外部状态完全取决于闭包的捕获方式。handler 与 `now` 在单个实例内不会并发执行，但上层的其他线程仍可能并发读写它们引用的共享对象。

## 与 Go 版本的对应关系

Rust 实现直接对应 `pkg/util/systimemon/systime_mon.go::StartMonitor`：两者都以 100 ms 周期采样、比较等待前后的墙钟、严格回退时记错并同步执行回调，而且都设计为无限运行。Rust 的泛型 `FnMut` 对应 Go 的两个函数参数，`SystemTime` 的直接比较对应 Go 的 `UnixNano()` 整数比较。

调度实现不是逐句翻译。Go 使用 `time.NewTicker` 并等待 `<-tick.C`；Rust 没有持有 ticker，而是用 `Instant` 和 `next_tick` 模拟固定节拍。`while next_tick <= tick_observed_at` 是保持语义的关键：当第一次 `now()` 很慢时，已经到达的 tick 可立即被消费，而不是额外等待 100 ms。`systime_mon_test.rs::slow_sample_does_not_restart_the_tick_interval` 专门锁定这一差异点。

日志实现也有边界差异：Go 通过 `logutil.BgLogger()` 和结构化 `zap.Int64("last", last)` 记录纳秒整数；Rust 通过 `log` facade 用 `Debug` 格式记录 `SystemTime`。Go 的 `defer tick.Stop()` 在无限循环正常路径上同样不会执行，但为异常返回保留资源清理；Rust 没有独立 ticker 资源。Go 测试 `TestSystimeMonitor` 与 Rust 的 `test_systime_monitor` 都构造约 2 秒回退并要求 1 秒内触发。

## 扩展指南

- 修改采样频率或调度策略时，应优先把当前函数内的 `interval` 提升为明确配置，同时保留 `Instant` 驱动和“慢消费者不重启间隔”的不变量；同步扩展 `pkg/util/systimemon/systime_mon_test.rs`，不要把测试内嵌回源文件。
- 若增加停止能力，可在 `StartMonitor` 的边界引入取消信号或另建可停止 API；需要定义 sleep 期间的唤醒、handler 正在运行时的取消以及线程 join 责任，并与 Go 版本的生命周期约定共同评估。
- 若改为跟踪历史最大时间、设置回退阈值或抑制重复告警，必须明确这会改变当前“相邻样本且严格小于”的行为，并在独立测试覆盖相等、小幅回退、连续回退、恢复前进和慢 handler。
- 若把真实实现接入 Rust server，修改点不在本文件，而在 `cmd/tidb-server` 的依赖和 `main.rs` 导入：需要用真实 `astersql-util-systimemon`/根 facade 替换 `crate::stubs::systimemon`，并验证后台线程确实长期存活、指标回调真实触发。不能只保留当前桩的事件记录测试作为行为证据。
- 修改日志字段时需考虑 Go 的结构化 `last` 字段兼容性和可观测性；修改闭包约束时需避免无必要地把 `Send + 'static` 强加给同步调用场景。

## 验证依据

- 源码与装配：`pkg/util/systimemon/systime_mon.rs`（完整 52 行）、`pkg/util/systimemon/lib.rs`、`pkg/util/systimemon/Cargo.toml`、根 `Cargo.toml` 与 `pkg/lib.rs`。
- RustCodeGraph：索引状态为 11,467 个文件；`files --filter pkg/util/systimemon` 确认目标、Go 对照和测试均已索引；`node systime_mon.rs::StartMonitor` 核对函数签名与实现；`callers systime_mon.rs::StartMonitor` 得到三个测试调用者；`callees` 返回空列表。自然语言 `explore` 未返回内容，因此没有把它当作事实证据。
- Rust 独立测试：`pkg/util/systimemon/systime_mon_test.rs` 验证 2 秒回退触发回调及慢采样后的固定 ticker 语义；`pkg/util/systimemon/migration_aster_unit_test.rs` 从公开 crate 名调用 API，再次验证回退检测；`pkg/util/systimemon/main_test.rs` 只负责公共测试 setup，不验证监控算法。
- Go 对照：`pkg/util/systimemon/systime_mon.go`、`pkg/util/systimemon/systime_mon_test.go`、`pkg/util/systimemon/main_test.go`；应用入口证据为 `cmd/tidb-server/main.go::setupMetrics`。
- Rust 应用接线：`cmd/tidb-server/main.rs::setupMetrics`、其顶部 `use crate::stubs::{..., systimemon, ...}` 以及 `cmd/tidb-server/stubs.rs::systimemon::StartMonitor`，共同证明当前 server 调用的是桩而非本文件。
- 本任务是纯文档分析，按计划不运行 Cargo。最终只执行固定十一章节的结构检查，并人工复核所有“当前已接线”结论均有上述源码或调用图依据。
