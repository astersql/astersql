# `pkg/util/signal/signal_windows.rs`

## 文件定位

本文件是 `astersql-util-signal` crate 的 Windows 平台实现。crate 根文件 `pkg/util/signal/lib.rs` 仅在 `cfg(windows)` 下声明 `signal_windows` 模块并公开重导出其内容，因此这里的 API 在 Windows 构建中承担 `SetupUSR1Handler`、`SetupSignalHandler` 和 `TiDBExit` 的平台适配；Unix 与 WASM 分别由同目录的其他实现提供。

该 crate 在 `pkg/util/signal/Cargo.toml` 中把库入口指定为 `lib.rs`，并只在 `cfg(windows)` 依赖 `ctrlc = "3"`。文件不实现业务协议，而是把 Windows 控制台中断转换为服务器关机回调能够消费的整数信号值。

## 核心职责

- 以 `Signal = i32` 保持与上层整数信号接口兼容。
- 通过 `SetupSignalHandler` 注册进程级控制台中断处理器，并保证传入的 `FnOnce` 关机回调最多执行一次。
- 用 `WINDOWS_INTERRUPT = 2` 把 `ctrlc` 的无参数通知映射为上层识别的 SIGINT 语义；`cmd/tidb-server/stubs.rs` 会进一步把整数 `2` 转换为服务器侧的 `Signal::SIGINT`。
- 明确 Windows 能力边界：`SetupUSR1Handler` 和 `TiDBExit` 当前均为空操作。

## 主要符号

- `pub type Signal = i32`：Windows 平台公开的信号参数类型。它不是操作系统句柄或强类型枚举，只是上层平台统一接口使用的整数。
- `const WINDOWS_INTERRUPT: Signal = 2`：文件内部常量，表示传给关机回调的 SIGINT 兼容值。
- `pub fn SetupUSR1Handler()`：公开的兼容入口。Windows 没有本实现所依赖的 POSIX SIGUSR1 诊断路径，所以函数立即返回。
- `pub fn SetupSignalHandler<F>(shutdown_func: F) where F: FnOnce(Signal) + Send + 'static`：核心公开入口。`'static` 与 `Send` 约束允许 `ctrlc` 库持有并从其处理线程调用闭包；`FnOnce` 表明关机逻辑只能被消费一次。
- `pub fn TiDBExit(sig: Signal)`：公开的兼容入口。当前仅用 `let _ = sig` 消除未使用参数，不会向当前进程发送信号，也不会主动退出。

这些公开函数保留 Go 风格命名，分别用 `#[allow(non_snake_case)]` 避免 Rust 命名警告。

## 执行流程

`SetupSignalHandler` 的流程如下：

1. 将 `shutdown_func` 包装为 `Some(shutdown_func)`，再置于 `Mutex` 和 `Arc` 中，使注册闭包可以安全地拥有共享状态。
2. 调用 `ctrlc::set_handler` 注册一个进程级处理闭包。
3. 每次控制台中断到来时，处理闭包获取互斥锁并对 `Option` 执行 `take()`。
4. 首次触发取得回调，向标准错误输出 `got signal to exit: signal=2`，然后调用 `callback(WINDOWS_INTERRUPT)`。
5. 后续触发时 `Option` 已为 `None`，因此不会再次执行日志或关机回调。
6. 如果注册失败，函数只向标准错误输出错误并返回；返回类型不会向调用者暴露失败。

在服务器主链中，`cmd/tidb-server/main.rs` 先调用 `signal::SetupUSR1Handler()`，完成存储、Domain 和 Server 初始化后再调用 `signal::SetupSignalHandler(...)`。该闭包按顺序关闭 Server、停止资源管理器、清理存储和 Domain、停止 profiler 与 executor，并计算退出码。中间适配层 `cmd/tidb-server/stubs.rs::signal::SetupSignalHandler` 在非测试构建中调用本 crate，并将整数信号映射为服务器内部枚举后交给 `deliver`。

## 数据与状态

文件没有可变全局变量。每次调用 `SetupSignalHandler` 都创建一份 `Arc<Mutex<Option<F>>>`：

- `Arc` 负责让注册闭包拥有回调状态，其生命周期由 `ctrlc` 注册的处理器延长。
- `Mutex` 串行化潜在的重复通知。
- `Option::take` 是“一次性执行”不变量的实际实现；成功取出后状态永久变为 `None`。

`WINDOWS_INTERRUPT` 是编译期常量。`SetupUSR1Handler` 与 `TiDBExit` 不创建线程、通道、句柄或持久状态。

## 依赖与调用关系

上游接线：

- `pkg/util/signal/lib.rs` 在 `cfg(windows)` 下重导出本文件公开符号。
- `cmd/tidb-server/stubs.rs::signal::SetupUSR1Handler` 和 `SetupSignalHandler` 是服务器迁移层的直接入口；非测试构建才转调 `astersql_util_signal`。
- `cmd/tidb-server/main.rs::run_main_inner` 在启动早期调用 USR1 兼容入口，并在服务资源建立后注册关机闭包。

下游依赖：

- 标准库 `Arc`、`Mutex`、`Option` 管理一次性回调的共享所有权和同步。
- Windows 条件依赖 `ctrlc::set_handler` 提供控制台 Ctrl+C/Ctrl+Break 注册能力；该依赖由 `pkg/util/signal/Cargo.toml` 的 target-specific 段声明。
- `eprintln!` 是唯一日志出口；本文件不依赖仓库日志 crate。

RustCodeGraph 能定位 `signal_windows.rs::SetupSignalHandler`、`SetupUSR1Handler`、`TiDBExit` 及 crate 根的条件重导出，但本次精确 callers/callees 查询未产出边；因此上述上游关系以 `cmd/tidb-server/stubs.rs` 和 `cmd/tidb-server/main.rs` 的真实调用点为准，未把图查询无结果解释成“没有调用者”。

## 错误处理与边界

- `ctrlc::set_handler` 的注册错误仅打印到标准错误，`SetupSignalHandler` 仍正常返回。上层无法据返回值判断关机处理器是否安装成功，这是当前 API 的明确限制。
- `callback.lock().expect("signal callback lock poisoned")` 在互斥锁中毒时会 panic。锁守卫是 `if let` 判别表达式中的临时值；当前写法不能把“回调执行时一定已经释放锁”作为接口保证。若回调间接重入同一处理状态，会有阻塞风险。
- `FnOnce` 加 `Option::take` 让重复信号成为无操作，避免重复清理服务器资源。
- 传给回调的值固定为 `2`；本文件不保留 Ctrl+C 与 Ctrl+Break 的差异，也不处理 SIGHUP、SIGTERM、SIGQUIT 的 Windows 数字语义。
- `TiDBExit` 不返回错误，也不执行退出动作。调用者不能依靠它在 Windows 上唤醒已注册处理器。
- `ctrlc` 通常要求进程只安装一个全局处理器；重复注册可能走错误日志分支，文件自身不协调多个注册者。

## 并发与资源生命周期

控制台通知由 `ctrlc` 管理的全局处理机制异步分发。本文件不显式创建或 join 线程；处理器及其捕获的 `Arc` 按 `ctrlc` 的全局注册生命周期存活，函数返回不会注销处理器。

同一回调状态的并发访问由 `Mutex` 保护。首次通知取得并移走回调后，其他通知在能够获得锁后只会看到 `None`。由于锁守卫来自 `if let` 的判别表达式，当前源码没有显式在调用用户回调前 `drop` 守卫；扩展时应把 `take()` 单独放入局部作用域，才能明确缩短临界区。回调运行在哪个线程、允许执行哪些阻塞操作，还受 `ctrlc` 库的处理线程模型约束。

该文件没有显式取消、注销或资源回收 API。若未来需要可重装处理器或进程内测试隔离，必须先解决 `ctrlc` 全局处理器的所有权，而不能只重置局部 `Option`。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/util/signal/signal_windows.go`。两者都保留三个同名公开入口，`SetupUSR1Handler` 都是空操作，关机处理也都只消费一次通知；但实现和行为存在重要差异：

- Go 使用容量为 1 的 `chan os.Signal`、`signal.Notify` 和 goroutine，登记 SIGHUP、SIGINT、SIGTERM、SIGQUIT，并把实际收到的 `os.Signal` 交给回调。
- Rust 使用 `ctrlc::set_handler`，没有信号通道，把所有受支持的控制台中断统一映射为整数 `2`，通过 `Mutex<Option<FnOnce>>` 保证只调用一次。
- Go 通过仓库日志器记录实际信号；Rust 直接写标准错误。
- Go `TiDBExit` 会查找当前进程并尽力调用 `p.Signal(sig)`；Rust `TiDBExit` 完全忽略参数。因此 Rust 版本只是 Windows 平台 API 兼容门面，不能宣称已经移植 Go 侧的自发信号能力。

同目录 `migration_aster_unit_test.rs` 由 `lib.rs` 在测试配置下挂载，但其测试模块受 `cfg(unix)` 限制，只验证 Unix 的诊断、信号透传和非法信号错误。当前没有针对本 Windows 文件的一份独立 Rust 回归测试。

## 扩展指南

- 若增加 Windows 信号种类或需要区分 Ctrl+C/Ctrl+Break，应优先修改 `SetupSignalHandler` 的事件到 `Signal` 映射，并同步检查 `cmd/tidb-server/stubs.rs` 中整数到服务器枚举的映射。不能只新增常量而不更新上层转换。
- 若要让注册失败可观测，应评估把 `SetupSignalHandler` 改为返回 `Result` 对 `lib.rs` 的跨平台公开 API、服务器启动错误路径以及 Go 对齐的兼容影响。
- 若实现 `TiDBExit`，需要用 Windows 支持的进程/控制台机制验证它确实能触发同一关机链；不能简单照搬 POSIX `kill`。实现还应说明目标进程、权限和 Ctrl 事件传播范围。
- 测试逻辑应放在独立文件中，建议新增 Windows 条件测试文件并由 `lib.rs` 挂载，覆盖首次触发、重复触发、注册失败/重复注册边界和上层整数映射；不要把测试内嵌回 `signal_windows.rs`。
- `ctrlc` 处理器具有进程级全局状态，测试应使用可隔离的内部分发函数或串行进程测试，避免多个用例竞争注册器。
- 任何改动都应继续保留 PingCAP Apache License 注释，并在行为真正可用后保留文件顶部的 AsterSQL 处理标记。

## 验证依据

- 源实现：`pkg/util/signal/signal_windows.rs`，符号 `Signal`、`WINDOWS_INTERRUPT`、`SetupUSR1Handler`、`SetupSignalHandler`、`TiDBExit`。
- crate 边界：`pkg/util/signal/lib.rs` 的 `cfg(windows)` 模块声明与重导出；`pkg/util/signal/Cargo.toml` 的 `[lib]` 和 Windows `ctrlc` 条件依赖。
- 应用接线：`cmd/tidb-server/stubs.rs::signal::{SetupUSR1Handler, SetupSignalHandler, deliver}`；`cmd/tidb-server/main.rs::run_main_inner` 中的注册位置和关机闭包。
- Go 对照：`pkg/util/signal/signal_windows.go`。
- 测试边界：`pkg/util/signal/migration_aster_unit_test.rs` 仅含 `cfg(unix)` 测试；仓库检索未找到 Windows 专用 Rust 测试。
- RustCodeGraph：`status` 显示索引包含 `pkg/util/signal/signal_windows.rs`；`files --filter pkg/util/signal` 列出 Rust/Go 平台实现；`node --file` 核对目标文件及 `lib.rs`；`query` 核对三个公开函数的跨平台同名实现；精确 callers/callees 查询未返回可用调用边，故调用证据改由上述真实入口文件补足。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定章节结构命令和人工事实复核验收。
