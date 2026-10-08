# `pkg/util/timeutil/time.rs`

## 文件定位

本文件属于 workspace crate `astersql-util-timeutil`，crate 入口 `pkg/util/timeutil/lib.rs` 以公开模块 `time` 暴露它。它对应 Go 包 `pkg/util/timeutil` 中的 `time.go`，提供一个可被取消的异步等待原语，而不是普通的阻塞睡眠或日期/时区计算。时区解析、系统时区和日内时间窗位于同 crate 的 `time_zone.rs`，不属于本文件职责。

`pkg/util/timeutil/Cargo.toml` 声明库入口为 `lib.rs`，本文件直接依赖 `tokio` 的计时器和选择器以及 `tokio-util` 的取消令牌。workspace 中多个 crate 依赖整个 `astersql-util-timeutil`，但仓库搜索显示当前 Rust 生产代码使用的是 `time_zone` API；`time::Sleep` 目前只被本 crate 的独立测试直接调用，尚无已验证的 Rust 生产调用点。

## 核心职责

- 通过 `pub use tokio_util::sync::CancellationToken` 为调用方提供统一的取消令牌类型。
- 用 `SleepError::Cancelled` 表示等待尚未自然结束就收到取消信号。
- 由异步函数 `Sleep` 同时等待计时器和取消通知，并返回先完成分支的结果。
- 保持 Go `Sleep(context.Context, time.Duration) error` 的核心语义：计时器先到期返回成功，context 先结束则提前返回错误，并清理未胜出的计时等待。

本文件不创建 Tokio runtime、不派生后台任务、不管理全局时钟，也不实现截止时间；取消发生的时机由调用者持有的 `CancellationToken` 决定。

## 主要符号

- `pub use tokio_util::sync::CancellationToken`：公开重导出 Tokio Util 的克隆式取消令牌。克隆共享同一取消状态，任一克隆调用 `cancel()` 都可唤醒 `cancelled()` 等待者。
- `pub enum SleepError { Cancelled }`：无负载、可复制且可比较的单一错误枚举。它只区分“被取消”和正常完成，不携带取消来源、deadline 或底层 I/O 错误。
- `impl fmt::Display for SleepError`：把唯一错误格式化为 `context canceled`，与 Go `context.Canceled` 的文本保持一致。
- `impl std::error::Error for SleepError`：允许该错误进入标准 Rust 错误传播链；本实现没有额外 `source`。
- `pub async fn Sleep(context: &CancellationToken, duration: Duration) -> Result<(), SleepError>`：公开异步入口。参数只借用令牌，等待期间无需取得所有权；`Duration` 是非负值，因此不存在 Go 风格负时长的输入表示。

文件没有模块级常量、可变静态状态、trait、自定义结构体或条件编译分支。

## 执行流程

1. 调用 `Sleep` 只构造 future；调用方 `.await` 后才开始执行选择逻辑。
2. `tokio::select!` 同时轮询 `tokio::time::sleep(duration)` 和 `context.cancelled()`。
3. 若计时 future 先就绪，函数返回 `Ok(())`。
4. 若取消 future 先就绪，函数返回 `Err(SleepError::Cancelled)`。
5. `select!` 结束时未胜出 future 被丢弃；取消分支获胜时，未完成的 Tokio sleep 不再保留或继续运行。

`tokio::select!` 未使用 `biased;`。如果两个分支在同一次轮询中均已就绪（例如令牌已经取消，同时零时长计时器也就绪），代码没有声明“取消必定优先”或“计时必定优先”的不变量，调用者不应依赖竞态时的固定结果。

## 数据与状态

`Sleep` 的本地状态只有一个 Tokio 计时 future 和一个由传入令牌产生的取消等待 future。`duration` 按值传入；`context` 只读借用，函数不会取消、重置或替换令牌。

取消状态存储在 `tokio_util::sync::CancellationToken` 的共享内部状态中，而不在本文件内。令牌取消是持续状态：已经取消的令牌生成的 `cancelled()` future 会立即就绪。`SleepError` 不保存时间戳、耗时或令牌引用，因此函数返回后没有由本模块维护的残留业务状态。

## 依赖与调用关系

下游依赖如下：

- `std::time::Duration` 定义等待长度。
- `std::fmt` 和 `std::error::Error` 提供错误展示及标准错误接口。
- `tokio::time::sleep` 提供运行时驱动的异步计时器；调用环境必须有启用 time driver 的 Tokio runtime。
- `tokio::select!` 负责竞争两个 future。
- `tokio_util::sync::CancellationToken` 提供可克隆、可广播的取消信号。

上游证据如下：

- `pkg/util/timeutil/lib.rs` 通过 `pub mod time` 公开本文件，并通过 `#[path = "time_test.rs"]` 保持测试逻辑在独立文件中。
- `pkg/util/timeutil/time_test.rs::test_sleep` 直接导入并调用 `Sleep`，验证取消分支。
- `pkg/util/timeutil/migration_aster_unit_test.rs::{sleep_returns_context_error_before_the_timer_like_go, sleep_completes_normally_when_the_timer_wins}` 分别验证取消和自然完成分支。
- RustCodeGraph 还把 `pkg/executor/test/jointest/join_test.rs::test_join_leak` 列为调用者，但该文件第 2194 行只是把 Go 源码文本 `time.Sleep(time.Millisecond)` 记录为字符串，未导入或调用本函数，不能视为真实 Rust 调用边。

仓库中的 `pkg/session`、`pkg/executor`、`pkg/timer` 等 Cargo manifest 虽声明了对 `astersql-util-timeutil` 的依赖，已检索到的 Rust 生产引用当前都指向 `time_zone` API；这只能证明 crate 边界已接线，不能证明 `Sleep` 已进入应用主链。

## 错误处理与边界

正常计时完成返回 `Ok(())`；取消返回唯一的 `SleepError::Cancelled`。错误可用 `==` 精确比较，也可格式化为 `context canceled`。函数没有 panic、重试或日志分支，但若在没有 Tokio time driver 的执行环境中轮询 `tokio::time::sleep`，底层 Tokio 的运行时前置条件不成立；调用方应从正确配置的 runtime 中调用。

零时长会让计时分支尽快就绪。已取消令牌会让取消分支尽快就绪。两者同时就绪时的选择次序未被 API 保证。Rust `Duration` 无法表示负值，因此无需复刻 Go `time.NewTimer` 对负 duration 立即触发的输入形式。极大时长的可接受范围由 Tokio 计时器和运行时平台决定，本文件没有额外上限检查。

与 Go 版本相比，Rust 错误只表达取消，不能区分 `context.Canceled` 与 `context.DeadlineExceeded`。调用者若需要 deadline 语义，必须用外部任务或令牌管理逻辑触发取消，并自行保留原因。

## 并发与资源生命周期

函数本身没有锁、通道、线程、显式任务或全局共享变量。并发协作完全通过 `CancellationToken`：调用者可以把克隆令牌交给另一任务，由该任务调用 `cancel()` 唤醒当前等待。

计时器和取消等待都绑定到单次 `Sleep` future 的生命周期。任一分支胜出后，另一个 future 在离开 `select!` 时被丢弃；这对应 Go 实现 `defer t.Stop()` 的资源清理意图，但不是启动计时后台任务后再显式 join。若外部直接丢弃尚未完成的 `Sleep` future，两个内部等待也随之被丢弃，函数不会留下本文件创建的任务。

`time_test.rs` 使用 `tokio::spawn` 模拟异步取消并显式等待该任务结束；该任务属于测试调用方，不是 `Sleep` 内部资源。`Sleep` 借用令牌也确保被借用的 token 引用在等待完成前有效。

## 与 Go 版本的对应关系

Go `pkg/util/timeutil/time.go::Sleep` 调用 `time.NewTimer(d)`，再用 `select` 竞争 `t.C` 与 `ctx.Done()`，并以 `defer t.Stop()` 清理计时器。Rust `Sleep` 用 `tokio::time::sleep`、`context.cancelled()` 和 `tokio::select!` 形成同样的两分支结构，靠丢弃未胜出 future 完成清理。

两版的自然完成结果一致：Go 返回 `nil`，Rust 返回 `Ok(())`。取消路径的主要差异是 Go 原样返回 `ctx.Err()`，因此可保留 canceled/deadline exceeded；Rust 总是返回 `SleepError::Cancelled`，只用显示文本 `context canceled` 对齐常见取消错误。Go 接受任意 `context.Context`，Rust 固定接受 `CancellationToken`，没有 context value、deadline 查询或父 context 的完整接口。

Go `time_test.go::TestSleep` 只检查短 context timeout 能让十秒睡眠提前结束，且忽略返回错误。Rust `time_test.rs::test_sleep` 进一步断言 `SleepError::Cancelled`；`migration_aster_unit_test.rs` 又覆盖取消分支和计时器正常完成分支。当前独立 Rust 测试未固定同时就绪竞态、预先取消令牌、零时长或错误显示文本。

## 扩展指南

若只需新增可取消等待的生产调用，应复用 `Sleep`，并在调用侧明确谁持有令牌、谁触发取消、是否需要保留取消原因；同时在调用方自己的独立测试文件中验证资源收尾。不要把测试内嵌回 `time.rs`。

若要区分 deadline 与主动取消，最可能修改 `SleepError` 和 `Sleep` 的参数契约，并同步 `time_test.rs`、`migration_aster_unit_test.rs` 以及 Go 对照说明。该变更具有兼容性风险：新增错误变体会影响穷尽匹配，替换 `CancellationToken` 会影响现有导入；也不能仅凭错误字符串模拟完整 `context.Context`。

若要规定“取消与计时同时就绪”的优先级，需要有意识地调整 `tokio::select!` 策略并添加确定性测试。偏置选择可能改变高并发公平性，循环中频繁调用时还需评估取消检查与计时器分配的性能。若新增后台任务、锁或通道，应在同目录独立测试中证明取消后无泄漏，并更新本节的生命周期说明。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，可用于本次符号与文件导航。
- RustCodeGraph `node --file pkg/util/timeutil/time.rs`：核对全部 52 行源码以及 `Sleep`、`SleepError`、`CancellationToken` 的定义。
- RustCodeGraph `node time.rs::Sleep`：确认目标符号源码，并给出 `time_test.rs::test_sleep` 以及字符串型误判 `join_test.rs::test_join_leak`；随后用精确文本检索核验真实引用。
- 已读取 `pkg/util/timeutil/Cargo.toml` 与 `pkg/util/timeutil/lib.rs`，确认 crate 名、库入口、Tokio/Tokio Util 依赖、公开模块和独立测试装配。
- 已读取 Go 对照 `pkg/util/timeutil/time.go`、`pkg/util/timeutil/time_test.go`，以及 Rust 测试 `pkg/util/timeutil/time_test.rs`、`pkg/util/timeutil/migration_aster_unit_test.rs` 的相关用例。
- 已检索 workspace 的 `astersql-util-timeutil` / `astersql_util_timeutil` 引用，区分 crate 依赖者、`time_zone` 使用者和 `Sleep` 的直接调用者。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务规定的命令验证本文恰好包含十一个固定二级章节。
