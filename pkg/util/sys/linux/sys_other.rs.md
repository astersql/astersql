# `pkg/util/sys/linux/sys_other.rs`

## 文件定位

本文件是 `astersql-util-sys-linux` crate 在“非 Linux 且非 Windows”目标上的平台实现，源码入口见 `pkg/util/sys/linux/lib.rs` 中受 `cfg(all(not(target_os = "linux"), not(target_os = "windows")))` 约束的 `sys_other` 模块及其公开再导出。crate 的清单是 `pkg/util/sys/linux/Cargo.toml`，根工作区通过 `facade_util_sys_linux` 引入它，`pkg/lib.rs` 再把它暴露为 `util::sys::linux`。因此，上层代码可以使用统一的 `linux::{OSVersion, SetAffinity, GetSockUID}` API，而不需要自行选择平台文件。

尽管模块条件写成“非 Linux/Windows”，本文件直接使用 `std::os::unix::net::UnixStream`，Cargo 依赖也按 `cfg(unix)` 声明；它实际面向的是 macOS、FreeBSD、DragonFly 等类 Unix 目标，不是任意非 Unix 目标。

## 核心职责

本文件提供三项跨平台兜底行为：

- `OSVersion` 生成与 Go `runtime.GOOS + "." + runtime.GOARCH` 一致的标识，并通过 `go_os`、`go_arch` 修正 Rust 与 Go 的命名差异。
- `SetAffinity` 在非 Linux 系统上接受 CPU 列表但不执行系统调用，保持 Go 版本的成功空操作语义。
- `GetSockUID` 在 Apple、FreeBSD、DragonFly 上读取 Unix domain socket 对端凭证中的 UID；其他类 Unix 目标明确返回 `io::ErrorKind::Unsupported`。

它不是系统抽象层的业务入口，也不维护全局状态；它通过与 Linux、Windows 兄弟实现保持同名签名，让调用方获得稳定的平台 API。

## 主要符号

- `fn go_os() -> &'static str`：内部命名适配器。把 Rust 的 `std::env::consts::OS == "macos"` 映射为 Go 的 `darwin`，其他值原样返回。
- `fn go_arch() -> &'static str`：内部架构适配器。完成 `x86 -> 386`、`x86_64 -> amd64`、`aarch64 -> arm64` 三组 Go 命名映射，未知架构原样返回。
- `pub fn OSVersion() -> io::Result<String>`：公开 API。格式化为 `<go_os>.<go_arch>`；当前实现没有可失败操作，但保留 `io::Result` 以与其他平台实现和调用方接口一致。
- `pub fn SetAffinity(_cpus: &[i32]) -> io::Result<()>`：公开 API。参数有意不读取并固定返回 `Ok(())`。
- `pub fn GetSockUID(socket: &UnixStream) -> io::Result<u32>`：仅在 Apple vendor、FreeBSD、DragonFly 上存在的实现，调用 `nix::sys::socket::getsockopt(..., sockopt::LocalPeerCred)` 并返回 `credential.uid()`。
- `pub fn GetSockUID(_socket: &UnixStream) -> io::Result<u32>`：与上一实现互斥，仅在其余目标上编译，固定返回 `Unsupported`。

文件没有模块级常量、自定义类型、trait 或 `impl`。`#![allow(non_snake_case)]` 是为了保留 Go 风格公共函数名。

## 执行流程

`OSVersion` 的执行链为 `OSVersion -> go_os` 与 `OSVersion -> go_arch`。两个辅助函数读取编译目标常量而不是运行时探测主机，完成命名映射后由 `format!` 以点号拼接；例如 Apple Silicon macOS 目标得到 `darwin.arm64`。

`SetAffinity` 收到任意切片后立即返回成功，不验证 CPU 编号、不改变进程或线程亲和性。生产侧 `cmd/tidb-server/main.rs::setCPUAffinity` 会解析 `affinity-cpus` 并调用统一的 `linux::SetAffinity`；在本文件覆盖的平台上，这条启动流程继续成功，但不会真正绑核。

`GetSockUID` 在编译期分流：受支持的平台把 `UnixStream` 借用传给 `nix::getsockopt`，读取 `LocalPeerCred` 后取 UID；其他目标不检查 socket，直接构造不支持错误。两个函数不会在同一目标上同时存在。

## 数据与状态

本文件不保存可变状态，也不缓存系统信息。`go_os` 和 `go_arch` 返回编译期目标字符串的静态借用；`OSVersion` 每次分配一个新的 `String`。`SetAffinity` 只借用调用方的 CPU 切片且不读取内容。`GetSockUID` 只借用现有 `UnixStream`，不取得所有权、不关闭 fd，也不改变 socket 的读写状态。

关键不变量是公共 API 在三类平台实现间保持同名：版本查询返回 `io::Result<String>`，亲和性设置返回 `io::Result<()>`，类 Unix socket 凭证查询返回 `io::Result<u32>`。UID 的含义是 socket 对端凭证报告的有效用户 ID，不是本地进程 UID 的硬编码值。

## 依赖与调用关系

模块装配由 `pkg/util/sys/linux/lib.rs` 完成：非 Linux/Windows 时声明并 `pub use sys_other::*`。`pkg/util/sys/linux/Cargo.toml` 把 crate 名定义为 `astersql-util-sys-linux`；`nix 0.31` 的 `socket` feature 仅在 `all(unix, not(target_os = "linux"))` 下启用，`libc` 在 Unix 下可用（本文件本身不直接调用 `libc`，相关测试用它核对有效 UID）。

RustCodeGraph 对目标文件的精确节点显示 `OSVersion` 调用 `go_os` 和 `go_arch`。全仓精确引用搜索显示：

- `cmd/tidb-server/main.rs::setCPUAffinity` 通过 `linux::SetAffinity` 使用统一 API；该模块来自根 facade 的 `pkg::util::sys::linux` 再导出。
- `pkg/util/sys/linux/sys_test.rs::test_get_os_version` 在本平台选择 `sys_other::OSVersion`。
- `pkg/util/sys/linux/migration_aster_unit_test.rs::other_unix_tests` 直接覆盖本文件三项 API。
- 当前 Rust 生产代码中未找到 `OSVersion` 或 `GetSockUID` 的其他直接调用；这只表示当前仓库的静态引用现状，不代表 API 不可供外部 crate 使用。

## 错误处理与边界

`OSVersion` 当前始终返回 `Ok`，因为它只读取编译目标常量并格式化；返回 `Result` 是接口兼容选择。未知 OS/架构不会报错，而是保留 Rust 给出的字符串，因此新增 Rust 目标是否与 Go 命名一致需要单独核对。

`SetAffinity` 对空切片、负 CPU 编号、重复编号和超范围编号都返回成功，因为参数完全不参与执行。这与 Linux 实现会调用内核并可能失败的行为不同，是明确的平台降级语义。

受支持平台的 `GetSockUID` 把 `nix` 的 `Errno` 转为带原始 OS 错误码的 `io::Error`；无效 fd、非 socket fd 或系统调用失败均沿此路径传播。其余目标固定返回 `io::ErrorKind::Unsupported` 和消息 `peer credentials are unsupported on this target`。当前条件只将 Apple vendor、FreeBSD、DragonFly 列为支持者；新增 BSD 或类 Unix 目标不会自动尝试凭证查询。

模块入口的 cfg 比 `std::os::unix` 与 Cargo Unix 依赖条件更宽。若未来支持一种既非 Linux/Windows、又非 Unix 的 Rust 目标，应同步收紧模块条件或提供独立实现，否则本文件可能无法编译。

## 并发与资源生命周期

所有函数都是同步的，不创建线程、任务、通道、锁或后台 worker。命名转换只读编译期常量，可被并发调用；`SetAffinity` 没有副作用；`GetSockUID` 仅在一次同步 `getsockopt` 调用期间借用 socket。

本文件不负责创建、复制或关闭 Unix socket。调用方必须保证 `UnixStream` 在调用期间有效，并继续负责其生命周期。测试 `UnixStream::pair()` 中的两个端点由测试作用域自动释放。`pkg/util/sys/linux/main_test.rs` 的 `Once` 和测试初始化器属于 crate 测试环境，不是本文件运行时行为。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/sys/linux/sys_other.go`，其构建条件同样是 `!linux && !windows`。

- Go `OSVersion` 使用 `runtime.GOOS + "." + runtime.GOARCH`；Rust 通过 `go_os`、`go_arch` 显式补齐常见命名差异，语义一致。
- Go `SetAffinity` 忽略 `[]int` 并返回 `nil`；Rust 忽略 `&[i32]` 并返回 `Ok(())`，语义一致。
- Go `GetSockUID` 先通过 `net.UnixConn.SyscallConn` 取得受控 fd，再调用 `unix.GetsockoptXucred(SOL_LOCAL, LOCAL_PEERCRED)`；Rust 接收 `std::os::unix::net::UnixStream` 并通过 `nix::sockopt::LocalPeerCred` 调用 `getsockopt`。两者都返回凭证 UID并传播系统错误，但 Rust 额外以 cfg 明确限制受支持平台，并在其他 Unix 目标返回 `Unsupported`。

Go 的 `pkg/util/sys/linux/sys_test.go::TestGetOSVersion` 只断言版本查询无错误且非空。Rust 的 `pkg/util/sys/linux/sys_test.rs` 保留该冒烟意图，`migration_aster_unit_test.rs::other_unix_tests` 进一步验证 Go 命名、亲和性空操作，以及在支持平台上的 peer UID。

## 扩展指南

新增 OS 或架构命名时，应优先修改 `go_os` 或 `go_arch`，并在独立测试文件 `pkg/util/sys/linux/migration_aster_unit_test.rs` 增加目标条件与精确期望；不要把测试嵌入本生产文件。需要真正支持非 Linux CPU 亲和性时，应修改 `SetAffinity`，同时审查 `cmd/tidb-server/main.rs::setCPUAffinity` 对“成功即已生效”的假设，并补充成功、非法 CPU、系统调用失败测试；这会改变现有兼容语义和启动行为，不能仅替换空操作而不验证。

扩展 peer credential 支持时，应先核对目标 OS 的 socket option、返回凭证类型和 UID 含义，再同步修改两个互斥 cfg、`Cargo.toml` 的目标依赖条件和 `other_unix_tests`。不要把所有 Unix 平台直接并入 `LocalPeerCred` 分支，因为 API/常量并非跨 Unix 统一。系统调用仍应保留原始 OS 错误码，socket 所有权应继续留给调用方。

性能风险很低：版本函数只有一次小字符串分配，凭证函数只有一次系统调用。主要兼容风险来自目标三元组条件、Go/Rust 平台命名漂移和把空操作误解为亲和性已实际生效。

## 验证依据

本说明基于以下直接证据：

- 源码与装配：`pkg/util/sys/linux/sys_other.rs`、`pkg/util/sys/linux/lib.rs`、根 `pkg/lib.rs`。
- crate 与依赖：`pkg/util/sys/linux/Cargo.toml`、根 `Cargo.toml`、`pkg/server/Cargo.toml`。
- Go 对照：`pkg/util/sys/linux/sys_other.go`、`pkg/util/sys/linux/sys_test.go`、`pkg/util/sys/linux/main_test.go`。
- Rust 独立测试：`pkg/util/sys/linux/sys_test.rs`、`pkg/util/sys/linux/migration_aster_unit_test.rs`、`pkg/util/sys/linux/main_test.rs`。
- 上游调用：`cmd/tidb-server/main.rs::setCPUAffinity` 及其启动阶段调用点。
- RustCodeGraph：索引状态为 11,467 个文件；目标目录列出 12 个已索引源码文件；`query` 找到本文件的 `go_os`、`go_arch`、`OSVersion`、`SetAffinity` 和两个 cfg 互斥的 `GetSockUID`；精确 `node pkg/util/sys/linux/sys_other.rs::OSVersion` 确认其到 `go_os`、`go_arch` 的调用边。精确 `callers` 查询在 30 秒观察窗口内未返回，因此上游关系另以全仓精确引用搜索和模块再导出核对，未把超时当作“无调用者”证据。
- 测试说明只来自现有测试源码；按任务约束未运行 Cargo。结构完整性由任务规定的 11 标题命令单独验证。
