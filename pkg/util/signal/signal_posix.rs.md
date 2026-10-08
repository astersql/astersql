# [`pkg/util/signal/signal_posix.rs`](signal_posix.rs)

## 文件定位

本文件是 `astersql-util-signal` crate 的 POSIX/Unix 实现，只在 `pkg/util/signal/lib.rs` 的 `#[cfg(unix)]` 分支中编译并由该入口重导出。crate 边界由 `pkg/util/signal/Cargo.toml` 定义：库入口是 `lib.rs`，Unix 目标专用依赖为 `libc` 与 `signal-hook`。它把操作系统信号转换为两个进程级能力：收到 `SIGUSR1` 时输出诊断栈，以及收到关闭信号时把原始信号号交给上层关机回调。

应用侧的直接接线位于 `cmd/tidb-server/stubs.rs` 的 `signal` 模块。非测试构建中，该门面调用本 crate 的 `SetupUSR1Handler` 和 `SetupSignalHandler`；`cmd/tidb-server/main.rs` 在主要后台组件启动前注册 USR1 处理器，并在 server、storage、domain 创建后注册关闭处理器。因此本文件位于“Unix 信号来源”与“服务端退出编排”之间，不负责实现资源清理本身。

## 核心职责

- `SetupUSR1Handler` 注册并持续监听 `SIGUSR1`，每次收到该信号都通过 `handle_usr1_signal` 生成诊断文本并写入标准错误。
- `SetupSignalHandler` 注册 `SIGHUP`、`SIGINT`、`SIGTERM`、`SIGQUIT`，只消费第一个关闭信号，然后通过 `dispatch_shutdown_signal` 调用一次上层回调。
- `handle_usr1_signal` 与 `dispatch_shutdown_signal` 将容易单测的判断、格式化和回调转发从后台监听线程中拆出；二者虽为公开符号，但以 `#[doc(hidden)]` 隐藏于生成文档，主要服务于内部实现与独立回归测试。
- 本文件不发送信号、不等待服务退出，也不决定退出码。发送当前进程信号由同 crate 的 `exit.rs` 负责；信号枚举映射、清理顺序和退出码计算位于 `cmd/tidb-server/stubs.rs`、`cmd/tidb-server/main.rs`。

## 主要符号

- `pub type Signal = libc::c_int`：信号的公共表示，直接保持 libc 整数编号，便于跨 crate 传递且不引入本地枚举转换。
- `fn get_goroutine_stacks() -> String`：调用 `Backtrace::force_capture()` 捕获当前调用线程的 Rust backtrace，并加上 `stack backtrace:` 前缀。名称沿用 Go 语义，但结果不是全进程 goroutine dump。
- `pub fn handle_usr1_signal(sig: Signal) -> Option<String>`：仅当 `sig == SIGUSR1` 时返回带开始/结束标记的 dump；其他信号返回 `None`。它调用 `get_goroutine_stacks`，是诊断格式和信号过滤的可测试边界。
- `pub fn SetupUSR1Handler()`：用 `Signals::new([SIGUSR1])` 完成注册，成功后创建常驻线程并遍历 `signals.forever()`；注册失败时记录错误并直接返回。
- `pub fn dispatch_shutdown_signal<F>(sig: Signal, shutdown_func: F)`，其中 `F: FnOnce(Signal)`：先打印收到的信号号，再把同一编号原样传给回调。
- `pub fn SetupSignalHandler<F>(shutdownFunc: F)`，其中 `F: FnOnce(Signal) + Send + 'static`：注册四种关闭信号并将监听器及回调移入新线程；只获取迭代器的第一项，因此类型约束和控制流共同保证回调至多执行一次。

公开 API 保留了 Go 风格的 `Setup...` 命名，并通过 `#[allow(non_snake_case)]` 局部豁免 Rust 命名规则；内部辅助函数仍使用 snake_case。

## 执行流程

诊断路径如下：

1. `cmd/tidb-server/main.rs` 在大部分后台模块启动前调用门面 `signal::SetupUSR1Handler()`。
2. `cmd/tidb-server/stubs.rs` 在非测试构建中转调 `astersql_util_signal::SetupUSR1Handler()`。
3. 本文件用 `signal_hook::iterator::Signals::new` 注册 `SIGUSR1`，把 `Signals` 所有权移动到新线程。
4. 线程永久遍历 `signals.forever()`。每个信号交给 `handle_usr1_signal`；匹配 `SIGUSR1` 时强制捕获监听线程 backtrace，格式化后用 `eprint!` 输出。

关机路径如下：

1. `cmd/tidb-server/main.rs` 创建 server、storage、domain 后调用门面 `signal::SetupSignalHandler`，闭包捕获这些资源及共享退出码。
2. `cmd/tidb-server/stubs.rs` 保存应用回调，并在非测试构建中向本 crate 注册一个整数信号适配闭包；该闭包把常见编号映射为门面枚举后调用 `deliver`。
3. 本文件注册 `SIGHUP`、`SIGINT`、`SIGTERM`、`SIGQUIT`，把 `Signals` 和 `FnOnce` 回调移动到新线程。
4. 线程执行 `signals.forever().next()`，只等待第一项；收到后由 `dispatch_shutdown_signal` 记录编号并调用回调。
5. 上层 `deliver` 执行服务端回调并唤醒退出等待者；`main.rs` 中的回调依次关闭 server、停止资源管理器、清理 storage/domain、停止 profiler 与 executor，并根据该信号设置退出码。上述资源清理不是本文件的职责。

## 数据与状态

本文件没有可变全局变量、缓存或显式共享状态。每次 setup 调用都会创建一个独立的 `Signals` 迭代器和独立线程：USR1 路径在线程内持有监听器；关闭路径还持有用户回调。信号集合以编译期常量 `SIGUSR1`、`SIGHUP`、`SIGINT`、`SIGTERM`、`SIGQUIT` 表达。

`handle_usr1_signal` 的输出是新分配的 `String`；每次有效 USR1 都会同步捕获并格式化一次 backtrace。`dispatch_shutdown_signal` 不改写信号值。关闭回调使用 `FnOnce`，因此不能被重复调用；setup 函数本身不返回线程句柄、注册句柄或成功状态。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 与源码核对为：

- `cmd/tidb-server/main.rs::run_main_inner` 调用 `cmd/tidb-server/stubs.rs::signal::SetupUSR1Handler` 和 `SetupSignalHandler`。
- `cmd/tidb-server/stubs.rs::signal::SetupUSR1Handler` 在 `#[cfg(not(test))]` 下调用本文件重导出的同名函数。
- `cmd/tidb-server/stubs.rs::signal::SetupSignalHandler` 在 `#[cfg(not(test))]` 下调用本文件重导出的同名函数，并把 libc 整数信号适配到服务端门面信号。
- `pkg/util/signal/migration_aster_unit_test.rs` 直接调用 `handle_usr1_signal` 与 `dispatch_shutdown_signal` 验证纯逻辑边界。

下游依赖为：

- `signal_hook::iterator::Signals`：负责注册信号并提供阻塞迭代器；本文件通过 `forever()` 在普通线程上下文处理信号，而不是直接在异步信号处理器中执行格式化或清理。
- `signal_hook::consts::signal`：提供五个 POSIX 信号常量。
- `libc::c_int`：定义公共 `Signal` 的 ABI 对齐整数类型。
- `std::backtrace::Backtrace`：生成监听线程诊断栈。
- `std::thread::spawn`、标准错误输出宏和传入闭包：分别承担生命周期隔离、可见输出和向应用层转交关闭事件。

## 错误处理与边界

两个 setup 函数都显式处理 `Signals::new` 的失败：向标准错误写固定消息后返回，不 panic，但也不把错误返回给调用者。因此注册失败后应用会继续运行，却缺少对应的诊断或优雅关闭处理能力；调用方当前无法程序化区分成功与失败。

`handle_usr1_signal` 对非 `SIGUSR1` 返回 `None`，不会捕获 backtrace。`dispatch_shutdown_signal` 不捕获回调 panic；如果回调 panic，关闭监听线程随之终止。关闭监听线程只读取一个信号，后续关闭信号不会再次通过该实例触发回调。若 `signals.forever()` 因流结束而返回 `None`，线程静默结束且不调用回调。

诊断栈存在明确能力边界：`Backtrace::force_capture` 捕获的是执行 `handle_usr1_signal` 的监听线程栈，不是 Go 原实现的所有 goroutine 栈，也不是所有 Rust 线程栈。符号完整度还取决于构建和运行环境。本文件使用 `eprint!/eprintln!`，没有接入结构化日志器；输出错误本身不可从 API 观察。

重复调用任一 setup 函数会建立额外监听器和线程，本文件没有幂等保护或注销机制。是否以及如何向多个 `signal-hook` 监听器分发同一信号由该依赖负责；调用方不应假设重复注册等价于覆盖旧注册。

## 并发与资源生命周期

两条路径都使用 detached `std::thread`：setup 返回后线程继续运行，调用方拿不到 `JoinHandle`，也不能在正常关闭时显式 join。USR1 线程的预期生命周期接近进程生命周期，持续阻塞等待并串行处理诊断请求。关闭线程在第一次信号完成回调后自然退出，并释放其 `Signals` 与闭包捕获值。

后台线程通过 move 获得监听器所有权，不与调用线程共享本地可变状态。关闭回调要求 `Send + 'static`，以保证跨线程移动和独立生命周期；`FnOnce` 与只调用 `.next()` 的实现共同形成“至多一次”不变量。至于回调内部的锁、资源关闭顺序及跨线程同步，由 `cmd/tidb-server/main.rs` 和门面层负责，本文件不提供互斥或超时。

USR1 处理是单线程串行的，捕获 backtrace 和写标准错误期间不会处理下一项；若信号到达速度超过消费速度，排队/合并行为取决于操作系统与 `signal-hook`，本文件没有背压、计数或丢失检测。关闭回调也在专用监听线程上同步执行，耗时清理会占用该线程，但不会阻塞注册调用者。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/signal/signal_posix.go`。两版保留了相同的两个公开入口与信号集合：USR1 路径持续处理，关闭路径收到第一项后调用一次回调。Go 的 channel/goroutine 对应 Rust 的 `Signals` 阻塞迭代器/OS 线程；Go 的 `os.Signal` 接口值对应 Rust 的 `libc::c_int`。

诊断语义并非等价移植。Go `getGoroutineStacks` 从 1 MiB 缓冲区开始调用 `runtime.Stack(buf, true)`，按需翻倍到最多 64 MiB，从而尝试包含所有 goroutine；Rust `get_goroutine_stacks` 只用 `Backtrace::force_capture` 捕获当前信号监听线程。Rust 保留了 Go 输出的开始和结束标记，但信号显示为整数，内容前缀为 `stack backtrace:`。

日志设施也不同：Go 的 USR1 路径使用标准 `log.Printf`，关闭路径使用 `logutil.BgLogger().Info` 和 zap 字段；Rust 两条路径都直接写标准错误，未提供结构化字段。Go `signal.Notify` 与 Rust `Signals::new` 都完成注册，但 Rust 额外显式面对注册失败并静默降级；Go 函数签名不返回注册错误。Rust 用 `FnOnce + Send + 'static` 在类型层表达关闭回调的单次、跨线程所有权，Go 由 goroutine 只接收一次的控制流保证单次调用。

独立 Rust 回归位于 `pkg/util/signal/migration_aster_unit_test.rs`：它验证非 USR1 不产生 dump、USR1 文本含三类约定标记，以及关机分发保留 `SIGQUIT` 编号。当前同目录未发现对应 Go 单元测试；因此关于 Go 行为的依据来自生产实现本身，而不是 Go 测试断言。

## 扩展指南

- 新增或删除诊断信号时，应同时修改 `SetupUSR1Handler` 的注册集合与 `handle_usr1_signal` 的过滤逻辑，并在独立测试文件 `pkg/util/signal/migration_aster_unit_test.rs` 增加匹配和非匹配用例；不要把测试内嵌回生产文件。
- 调整关闭信号集合时，应修改 `SetupSignalHandler`，并同步检查 `cmd/tidb-server/stubs.rs` 的整数到门面枚举映射、`cmd/tidb-server/main.rs::exitCodeForSignal` 及 Go 对照。只注册而不适配的新编号可能在门面层退化为 `Signal::Other`。
- 若要让注册失败可观测，应谨慎演进两个 setup 函数的返回类型及所有平台实现（`signal_windows.rs`、`signal_wasm.rs`）和门面调用者；当前 API 是无返回值，直接改签名会影响跨平台公共契约。
- 若要支持注销、可控停机或可测试的线程生命周期，应引入明确的句柄/停止协议并评估重复注册语义，不能仅在 detached 线程外增加布尔标志。要保持关闭回调至多一次，并为并发信号、停止与回调竞态编写独立测试。
- 若目标是与 Go 的全 goroutine dump 更接近，必须先明确“全 Rust 线程采样”的实现与平台能力；简单扩大字符串或重复调用 `Backtrace` 不能得到其他线程栈。该变化还涉及诊断暂停时间、内存分配和日志量风险。
- 若改用结构化日志，应确认信号处理器注册发生在 `setupLog()` 之前这一启动顺序：早期 USR1 仍需可靠输出，不能无条件依赖尚未初始化的日志子系统。

兼容性风险主要是信号编号和跨平台 API 一致性；正确性风险集中在重复注册、回调只执行一次及注册失败后的降级；性能风险主要来自每次 USR1 强制捕获 backtrace、分配格式化字符串并同步写标准错误。扩展后应保持源文件与独立测试文件分离，并同步核对 Go 行为，避免无意将近似实现描述为完全等价。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标区域列出 `signal_posix.rs`、`lib.rs`、`migration_aster_unit_test.rs` 及 Go/Windows/WASM 对照文件。
- RustCodeGraph 源码与调用边：`handle_usr1_signal -> get_goroutine_stacks`；`SetupUSR1Handler -> handle_usr1_signal`；`SetupSignalHandler -> dispatch_shutdown_signal`；测试 `diagnostic_dump_is_only_created_for_usr1 -> handle_usr1_signal`；测试 `shutdown_dispatch_preserves_the_received_signal -> dispatch_shutdown_signal`。
- 已读生产源码：`pkg/util/signal/signal_posix.rs`、`pkg/util/signal/lib.rs`、`cmd/tidb-server/stubs.rs` 的 signal 门面、`cmd/tidb-server/main.rs` 的注册与退出闭包。
- 已读配置与对照：`pkg/util/signal/Cargo.toml`、`pkg/util/signal/signal_posix.go`。
- 已读独立测试：`pkg/util/signal/migration_aster_unit_test.rs`。其中第三个测试覆盖 `exit.rs::send_to_current_process` 的非法信号错误，不属于本文件自身逻辑；本文只将前两个测试作为本文件的直接行为证据。
- 使用 `rg` 检索 Rust/Go 调用与测试引用，确认公开入口的生产接线和同目录测试范围；包目录中不存在 `doc.go`，包级装配契约由 `lib.rs` 提供。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定章节结构命令和人工事实复核验证文档。
