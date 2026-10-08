# `pkg/util/signal/exit.rs`

## 文件定位

本文件属于 `astersql-util-signal` crate（见 `pkg/util/signal/Cargo.toml`），提供 Unix/POSIX 平台向当前进程投递信号的实现。`pkg/util/signal/lib.rs` 仅在 `cfg(unix)` 下声明 `mod exit` 并公开重导出其符号，因此这里的 API 不会进入 Windows 或 WASM 构建；Windows 的同名兼容入口位于 `signal_windows.rs`。

该文件对应 Go 的 `pkg/util/signal/exit.go`。它是信号发送端，不负责注册信号处理器；Unix 的 SIGUSR1 诊断和关机信号监听由相邻的 `signal_posix.rs` 承担。

## 核心职责

文件只承担两层职责：

1. `send_to_current_process` 直接执行 `kill(getpid(), sig)`，把系统调用成功或失败保留为 `io::Result<()>`，便于调用者和独立测试观察失败。
2. `TiDBExit` 提供与 Go 同名的公开兼容 API，把发送动作包装成尽力而为操作；失败时写标准错误，但不 panic、不中止调用线程，也不把错误继续返回。

当前 Rust 全仓引用搜索只发现独立测试直接调用 `send_to_current_process`，没有发现生产 Rust 代码调用 `TiDBExit`。因此它目前是已公开、已具备底层回归覆盖但尚未在 Rust 生产主链接线的移植 API；不能仅根据 Go 调用链声称 Rust standby 已使用它。

## 主要符号

- `pub fn send_to_current_process(sig: libc::c_int) -> io::Result<()>`：标记为 `#[doc(hidden)]` 的公开底层入口。公开可见性使 crate 外部测试可以调用，隐藏属性只影响生成文档的常规展示，不改变可调用性。函数以 `libc::getpid()` 取得当前 PID，再调用 `libc::kill`；返回值为 `0` 时返回 `Ok(())`，否则用 `io::Error::last_os_error()` 捕获紧随系统调用产生的操作系统错误。
- `pub fn TiDBExit(sig: libc::c_int)`：与 Go API 保持名称一致，因此使用 `#[allow(non_snake_case)]`。它调用 `send_to_current_process`，只在 `Err` 分支通过 `eprintln!` 输出信号号和错误。

文件没有模块级常量、类型、trait、`impl` 或额外条件编译项；Unix 条件编译位于上层 `lib.rs`。

## 执行流程

调用 `TiDBExit(sig)` 时的流程如下：

1. `TiDBExit` 把原始 `libc::c_int` 信号号传给 `send_to_current_process`。
2. `send_to_current_process` 在一个很小的 `unsafe` 块中先调用 `libc::getpid()`，再以该 PID 和传入信号调用 `libc::kill`。
3. 若 `kill` 返回 `0`，底层函数返回 `Ok(())`，外层函数静默结束。信号后续如何处理由进程已经注册的处理器或操作系统默认动作决定，不由本文件控制。
4. 若 `kill` 返回非零值，底层函数立即读取最后一个 OS 错误并返回 `Err`；`TiDBExit` 将错误打印到标准错误后结束。

测试也可以绕过吞错包装直接调用 `send_to_current_process`。`migration_aster_unit_test.rs::invalid_signal_send_reports_error_without_terminating_process` 传入 `-1` 并断言返回 `Err`，验证失败路径可观察且不会因包装逻辑终止测试进程。

## 数据与状态

本文件没有持久状态、全局变量、缓存或可变静态数据。输入只有一个按值传递的 `libc::c_int`，它保留 Unix 信号号的原始整数表示；输出要么是底层函数的 `io::Result<()>`，要么是 `TiDBExit` 的单元值。

唯一读取的进程状态是调用时的当前 PID，唯一可能产生的外部副作用是向该 PID 投递信号以及失败时写标准错误。错误值在 `kill` 失败后立即由 `last_os_error()` 构造，避免后续系统调用覆盖对应错误状态。

## 依赖与调用关系

- 上层装配：`pkg/util/signal/lib.rs` 在 Unix 下编译并 `pub use exit::*`，所以外部路径是 `astersql_util_signal::TiDBExit` 和隐藏文档入口 `astersql_util_signal::send_to_current_process`。
- 外部依赖：`pkg/util/signal/Cargo.toml` 把 `libc = "0.2"` 放在 `target.'cfg(unix)'.dependencies` 中；本文件使用其 `c_int`、`getpid` 和 `kill`。
- 文件内调用边：RustCodeGraph 的精确节点追踪记录 `TiDBExit -> send_to_current_process`。
- 已验证的 Rust 上游：`pkg/util/signal/migration_aster_unit_test.rs` 直接调用 `send_to_current_process`。截至本次分析，全仓 `.rs` 搜索未找到 `TiDBExit` 的生产调用者；`cmd/tidb-server/stubs.rs` 使用同一 crate 的 `SetupUSR1Handler` 和 `SetupSignalHandler`，但没有使用本文件的 API。
- crate 依赖声明：workspace facade、`cmd/tidb-server/Cargo.toml` 和 `pkg/standby/Cargo.toml` 都声明或暴露该 crate。Cargo 依赖本身不等于实际调用；特别是当前 Rust standby 源码搜索没有出现 `astersql_util_signal`。
- Go 主链证据：`pkg/standby/standby.go` 将 `tidbExit` 初始化为 `signal.TiDBExit`，`pkg/standby/idle_watcher.go` 会按退出场景传入 `SIGTERM` 或 `SIGINT`。这是 Go 版本的接线依据，不是当前 Rust 调用边。

## 错误处理与边界

`send_to_current_process` 不校验信号号，而是把合法性、权限和平台错误交给 `kill` 判定，并原样转成当前 OS 错误。调用者需要区分两种契约：需要确认发送结果时使用该函数并处理 `Result`；只需尽力触发退出时使用 `TiDBExit`，但不能从其返回值判断是否发送成功。

`TiDBExit` 的错误边界与 Go `exit.go::TiDBExit` 一致：发送失败只记录，不 panic，也不重试。Rust 使用 `eprintln!`，Go 使用结构化 `log.Error`，因此日志后端、字段结构和可观测性并不完全相同。传入一个具有终止默认动作的有效信号可能导致整个进程退出，不能在同进程单元测试中随意用成功路径调用；现有测试选择非法信号只覆盖安全的失败分支。

## 并发与资源生命周期

两个函数都是同步调用：没有创建线程、任务、通道、锁或堆上长期资源。系统调用返回只表示投递请求成功或失败，不表示目标进程的信号处理流程已经完成。

信号目标是在每次调用时取得的当前进程 PID，因此没有 PID 句柄需要保存或释放。成功投递后的处理时机具有信号机制本身的异步性，并可能与其他线程并发发生；本文件不维护一次性保护、顺序保证或关机完成通知。需要等待关机完成的上层代码必须使用信号处理模块或自己的同步机制，不能把 `TiDBExit` 返回当作退出完成屏障。

## 与 Go 版本的对应关系

Unix Rust 实现逐步对应 `pkg/util/signal/exit.go::TiDBExit`：两者都取得当前 PID、向自身发送调用者指定的信号，并在失败时仅记录错误。Rust 将系统调用拆成可返回 `io::Result` 的 `send_to_current_process` 和吞错兼容层 `TiDBExit`；Go 版本没有对应的公开可测试底层函数，而是直接调用 `syscall.Kill(syscall.Getpid(), sig)`。

类型上，Go 接收 `syscall.Signal`，Rust 接收其 ABI 整数表示 `libc::c_int`。日志上，Go 记录结构化错误与 `sig.String()`，Rust 输出整数信号号和 `Display` 格式错误。平台边界也不同：Go 的 `exit.go` 由 `//go:build !windows` 覆盖所有非 Windows 目标；Rust 本文件由 `cfg(unix)` 限定。Windows 两侧另有 `signal_windows.go/.rs`，其中当前 Rust `TiDBExit` 是明确的空操作，不应与本文件的 POSIX 行为混用。

Go 的 `pkg/standby/standby_nextgen_test.go` 通过替换 `tidbExit` 验证 graceful 路径选择 `SIGTERM`、拒绝路径不发送信号等上层策略；它没有执行真实 `kill`。Rust 独立测试只验证非法信号的错误返回，尚未覆盖 `TiDBExit` 的标准错误输出或成功投递。

## 扩展指南

- 修改 Unix 发送语义时，优先保持 `send_to_current_process` 为可观察错误的底层边界，并让 `TiDBExit` 保持 Go 兼容的尽力而为契约；若要改成返回错误、重试或结构化日志，应先核对所有未来调用者的退出语义和 Go 兼容影响。
- 新增信号类型封装或输入校验时，应明确是否仍允许 POSIX 的全部整数语义，并同时审查 `exit.go`、`lib.rs` 的重导出和 Windows/WASM 平台 API 形状。
- 回归测试应继续放在独立的 `pkg/util/signal/migration_aster_unit_test.rs` 或新增同目录独立测试文件，不要嵌入 `exit.rs`。安全测试优先使用必然失败的信号号或隔离子进程；不要在测试进程中直接发送具有默认终止动作的信号。
- 若把 `TiDBExit` 接入 Rust standby，应同步补充独立上层测试，像 Go 测试一样以可替换发送器验证 `SIGTERM`/`SIGINT` 选择，避免用真实进程退出替代业务断言。
- 性能风险很低，因为每次调用只有取得 PID、一次 `kill` 和可能的错误格式化；主要风险是兼容性与正确性，包括错误被吞掉、平台条件编译差异、错误信号导致非预期进程动作，以及把“已投递”误判为“已完成关机”。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引可用；`files --filter pkg/util/signal` 确认相关 Rust/Go 文件；`query send_to_current_process` 与 `query TiDBExit` 定位符号；精确 `node` 确认源码和 `TiDBExit -> send_to_current_process` 调用边。泛化 `callers/callees` 查询未返回完整边，因此未据此推断不存在内部调用。
- Rust 源与装配：`pkg/util/signal/exit.rs`、`pkg/util/signal/lib.rs`、`pkg/util/signal/migration_aster_unit_test.rs`、`pkg/util/signal/signal_windows.rs`。
- Cargo 边界：`pkg/util/signal/Cargo.toml`、workspace 根 `Cargo.toml`、`cmd/tidb-server/Cargo.toml`、`pkg/standby/Cargo.toml`。
- Go 对照与调用/测试：`pkg/util/signal/exit.go`、`pkg/util/signal/signal_windows.go`、`pkg/standby/standby.go`、`pkg/standby/idle_watcher.go`、`pkg/standby/standby_nextgen_test.go`。
- 全仓引用核对：对 `.rs` 文件搜索 `TiDBExit`、`send_to_current_process` 和 `astersql_util_signal`，确认当前 Rust 测试与生产接线范围；对 `.go` 文件搜索 `TiDBExit`，确认 Go standby 调用路径。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证目标文件存在且固定二级标题恰好为 11 个，并人工复核没有把 Go 主链或 Cargo 依赖误写成 Rust 已接线行为。
