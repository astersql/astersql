# `pkg/util/signal/signal_wasm.rs`

## 文件定位

本文件属于 `astersql-util-signal` crate（见 `pkg/util/signal/Cargo.toml`），是 `wasm32` 目标的信号兼容桩。`pkg/util/signal/lib.rs` 只在 `#[cfg(target_arch = "wasm32")]` 下声明 `mod signal_wasm` 并 `pub use signal_wasm::*`；因此外部使用的是 `astersql_util_signal::SetupUSR1Handler`、`astersql_util_signal::SetupSignalHandler` 和 `astersql_util_signal::Signal`，而不是直接访问私有模块。

该文件不模拟操作系统信号，也不承担真正的关机动作。它存在的目的，是让依赖统一信号 API 的上层代码在通常没有 POSIX 进程信号的 WASM 环境中仍能通过条件编译。Unix 和 Windows 的真实平台实现分别位于 `signal_posix.rs`、`exit.rs` 与 `signal_windows.rs`。

## 核心职责

本文件只有两项运行时职责：为 WASM 定义一个与其他平台可对接的信号编号占位类型，以及提供两个保持公开 API 形状的空操作函数。

- `SetupUSR1Handler` 立即返回，不注册监听器，也不产生诊断栈。
- `SetupSignalHandler` 接收关机回调后立即将其丢弃，既不注册事件源，也不调用回调。

因此这是明确的“平台能力缺失兼容层”，不是尚待补齐的 POSIX 信号实现。`cmd/tidb-server` 的启动与关机编排可以引用统一 API，但若该调用链被构建到 `wasm32`，这两个入口本身不会使进程响应 USR1、INT、TERM、HUP 或 QUIT。

## 主要符号

- `pub type Signal = i32`：WASM 下信号编号的占位别名。它保持回调参数为简单整数，与相邻 Rust 平台实现的信号值表示兼容；本文件没有创建任何实际 `Signal` 值。
- `pub fn SetupUSR1Handler()`：公开的无参数、无返回值空操作。名称为配合 Go/既有 API 使用大驼峰，并通过 `#[allow(non_snake_case)]` 局部允许该命名。
- `pub fn SetupSignalHandler<F>(shutdown_func: F) where F: FnOnce(Signal) + Send + 'static`：公开的泛型空操作。边界要求回调至多调用一次、可跨线程发送且不借用非静态数据，从类型形状上对齐会异步持有回调的平台实现；函数体仅执行 `let _ = shutdown_func`，随后参数在函数返回时被释放。

文件没有常量、结构体、枚举、trait、`impl`、锁或文件内条件编译项；平台选择集中在 `lib.rs`。

## 执行流程

调用 `SetupUSR1Handler()` 时，控制流进入空函数体并立即返回。没有系统调用、事件订阅、日志、线程或返回状态。

调用 `SetupSignalHandler(shutdown_func)` 时：

1. 编译器先检查闭包满足 `FnOnce(Signal) + Send + 'static`。
2. 函数取得闭包所有权。
3. `let _ = shutdown_func` 明确消费该参数但不执行 `FnOnce::call_once`。
4. 闭包值在函数结束时被释放，然后函数返回 `()`。

这条流程的关键不变量是“回调调用次数始终为零”。上层不能依赖该回调去关闭服务、停止后台组件、设置退出码或解除等待；WASM 退出必须由其他生命周期机制驱动。

## 数据与状态

本文件没有全局状态、持久状态、注册表或缓存。`Signal` 只是 `i32` 的类型别名，不提供范围检查、枚举语义或平台信号常量。

唯一进入函数的数据是 `SetupSignalHandler` 的闭包值。它不会被保存，也不会接收信号参数；所有权在本次同步调用内结束。若闭包捕获了资源，这些捕获会随闭包被丢弃而释放。通常资源析构不等于回调执行，因此不能把析构副作用当作关机通知契约。

## 依赖与调用关系

- crate 装配：`pkg/util/signal/lib.rs` 在 `target_arch = "wasm32"` 时编译本模块并公开重导出全部符号；Unix 和 Windows 分支互相独立。
- Cargo 边界：`pkg/util/signal/Cargo.toml` 把库入口设为 `lib.rs`，只为 Unix 声明 `libc`/`signal-hook`，只为 Windows 声明 `ctrlc`。WASM 桩只使用 Rust 核心语言能力，没有专属外部依赖或 feature。
- 已核验的上游主链：`cmd/tidb-server/stubs.rs::signal::SetupUSR1Handler` 与 `SetupSignalHandler` 在非测试构建中转发到 `astersql_util_signal`；`cmd/tidb-server/main.rs` 分别在启动早期和服务创建后调用这两个 stubs 入口。前者原意是安装在线诊断，后者的回调负责关闭 server、停止资源管理/执行器并清理 storage/domain。
- 下游调用：两个 WASM 函数都不调用其他函数。RustCodeGraph 文件节点将 `signal_wasm.rs` 标为 `used by 0 files`；这反映索引没有建立到条件重导出后具体平台实现的文件级边，不能取代 `lib.rs` 和直接调用点所证明的静态接线。
- 其他依赖声明：workspace 根 `Cargo.toml` 暴露该 crate，`cmd/tidb-server/Cargo.toml` 和 `pkg/standby/Cargo.toml` 声明依赖。依赖声明不等于在 WASM 上能够构建完整 server，也不等于 standby 源码直接调用了本文件。

## 错误处理与边界

两个函数都不返回 `Result`，不会主动报错、记录日志或 panic。由于没有尝试注册信号源，也就没有“平台不支持”或“注册失败”的运行时错误可观察；能力缺失通过空操作契约表达。

主要边界是调用者可能误以为统一 API 表示统一行为：在 WASM 上，`SetupUSR1Handler` 不会输出栈，`SetupSignalHandler` 不会触发关机回调。泛型边界仍要求 `Send + 'static`，即使当前实现不跨线程保存闭包；放宽它会使平台间 API 更难保持一致，收紧它则可能破坏现有调用点编译。

`let _ = shutdown_func` 不调用闭包，但释放闭包及其捕获值时仍会执行这些值各自的 `Drop`。因此“回调不执行”不应被扩大解释为“任何用户定义析构代码都绝不运行”。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、监听器或锁，也没有信号处理器的注册与注销生命周期。两个入口都是同步且有界的：调用完成即返回。

`SetupSignalHandler` 的 `Send + 'static` 约束保留了其他平台可能把回调移入后台线程或全局处理器的接口能力，但 WASM 实现没有发生这种转移。闭包在调用期间独占地按值传入，并在返回前被释放；不存在后续并发调用、一次性触发竞争或关机完成通知。资源回收只涉及闭包捕获值的正常析构。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/signal/signal_wasm.go`。Go 通过文件名 `_wasm.go` 选择 WASM 实现，Rust 通过 `lib.rs` 的 `cfg(target_arch = "wasm32")` 选择本文件。两侧的 `SetupUSR1Handler` 都是空操作，`SetupSignalHandler` 都接收回调但从不调用它，核心运行语义一致。

类型形状存在语言差异：Go 回调参数是 `os.Signal` 接口，Rust 使用 `i32` 别名 `Signal`；Go 函数参数无需声明线程安全和生命周期，Rust 明确要求 `FnOnce + Send + 'static`。Rust 的约束更接近相邻平台异步持有一次性关机回调的实现形状，但在当前 WASM 函数体内不会使用这些能力。

同目录 Go/Rust 独立测试没有覆盖 WASM 桩：`migration_aster_unit_test.rs` 整个测试模块受 `#[cfg(unix)]` 限制，只验证 POSIX 诊断、关机分发和发送失败；仓库搜索也未发现针对 `signal_wasm.go` 的 Go 测试。因此目前的 WASM 一致性证据来自两个实现本身及模块选择规则，而不是运行时回归测试。

## 扩展指南

- 若 WASM 运行时将来提供宿主关闭、浏览器生命周期或 WASI 信号能力，应优先新增明确的适配层，再决定是否改变这两个兼容入口；不要把 POSIX 信号号和浏览器事件直接等同。
- 改动 `SetupSignalHandler` 时必须同时检查 `pkg/util/signal/lib.rs` 的重导出、`cmd/tidb-server/stubs.rs` 的整数到上层 `Signal` 枚举映射，以及 Unix/Windows/Go 同名 API，保持调用次数和所有权语义清晰。
- 回归测试应放在同目录独立测试文件中，不要内嵌到 `signal_wasm.rs`。WASM 专属行为应以 `wasm32` 可执行的独立测试验证“回调未被调用”和“捕获资源按预期释放”；现有 Unix-only `migration_aster_unit_test.rs` 不能提供该覆盖。
- 若仍保持 no-op，文档和调用者应明确上层需要替代退出路径。若改为保存回调，则必须定义注册失败、重复注册、触发次数、并发安全和注销时机，不能仅为测试方便调用一次回调。
- 性能风险接近于零；主要风险是兼容性与正确性，包括误把空操作当作已安装处理器、跨平台泛型签名漂移，以及完整 server 在 WASM 上的其他依赖并未由本文件保证可用。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 Rust/Go 文件；`files --filter pkg/util/signal` 定位本 crate 的平台实现；`node --file pkg/util/signal/signal_wasm.rs` 核对 37 行源码、三个公开符号及 `used by 0 files` 结果；`query SetupUSR1Handler`、`query SetupSignalHandler` 定位各平台同名实现。精确 callers/callees 查询出现名称消歧噪声，未将其泛化结果作为调用关系证据。
- Rust 源与装配：`pkg/util/signal/signal_wasm.rs`、`pkg/util/signal/lib.rs`、`pkg/util/signal/migration_aster_unit_test.rs`、`cmd/tidb-server/stubs.rs`、`cmd/tidb-server/main.rs`。
- Cargo 边界：`pkg/util/signal/Cargo.toml`、workspace 根 `Cargo.toml`、`cmd/tidb-server/Cargo.toml`、`pkg/standby/Cargo.toml`。
- Go 对照：`pkg/util/signal/signal_wasm.go`；相邻 `signal_posix.go` 与 `signal_windows.go` 用于确认跨平台 API 的行为差异。
- 引用与测试核对：全仓搜索 `SetupUSR1Handler`、`SetupSignalHandler`、`astersql_util_signal`、`signal_wasm` 和 `wasm32`，确认 Rust 主链入口、条件重导出以及缺少 WASM 专属测试的现状。
- 本任务只新增说明文档，按计划未运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有十一个固定二级标题，并人工复核没有把统一 API、Cargo 依赖或 Go 行为误写成 WASM 已具有真实信号能力。
