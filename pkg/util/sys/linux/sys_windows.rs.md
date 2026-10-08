# `pkg/util/sys/linux/sys_windows.rs`

## 文件定位

本文件是 `astersql-util-sys-linux` crate 的 Windows 平台实现。crate 入口 [`lib.rs`](lib.rs) 只在 `cfg(target_os = "windows")` 下声明 `sys_windows`，并用 `pub use sys_windows::*` 将本文件的公共函数再导出到 crate 根；Linux 和其他平台分别选择 `sys_linux.rs` 与 `sys_other.rs`，因此三个实现不会在同一目标中同时成为公共 API。

[`Cargo.toml`](Cargo.toml) 指定 crate 名为 `astersql-util-sys-linux`、库入口为 `lib.rs`。它只为 Unix 目标声明 `libc`/`nix` 依赖，所以 Windows 实现仅使用 Rust 标准库。工作区根 `Cargo.toml` 以 `facade_util_sys_linux` 收录该 crate，`pkg/server/Cargo.toml` 也声明了路径依赖；当前 Rust 源码搜索未发现生产代码实际调用这里的 API，已确认的直接使用者是条件编译测试。

## 核心职责

本文件保持 Go 同路径 Windows 实现的三项跨平台契约：

- `OSVersion` 返回 `windows.<GOARCH>` 形式的平台标识；
- `SetAffinity` 在 Windows 上接受配置但不执行绑核，且报告成功；
- `GetSockUID` 明确拒绝 Unix domain socket 对端 UID 查询。

它是平台兼容层，不负责探测详细 Windows 版本、修改调度策略或模拟 Unix 凭据。模块级 `#![allow(non_snake_case)]` 用于保留 Go 导出函数的命名，降低迁移调用接口的差异。

## 主要符号

- `fn go_arch() -> &'static str`：内部架构名适配器。读取编译目标常量 `std::env::consts::ARCH`，将 Rust 的 `x86`、`x86_64`、`aarch64` 分别映射为 Go 的 `386`、`amd64`、`arm64`；其他架构名原样返回。返回值引用静态常量，不分配内存。
- `pub fn OSVersion() -> io::Result<String>`：拼接 `windows.` 与 `go_arch()` 的结果。签名保留可失败的跨平台接口，但当前实现只构造字符串，始终返回 `Ok`。
- `pub fn SetAffinity(_cpus: &[i32]) -> io::Result<()>`：忽略全部 CPU 编号并返回 `Ok(())`。参数名以下划线开头，明确表示兼容接口中的有意未使用参数。
- `pub fn GetSockUID<T>(_socket: T) -> io::Result<u32>`：接受任意类型的占位 socket 参数，始终构造 `io::ErrorKind::Unsupported`，错误文本为 `UNIX domain socket is not supported on Windows`。泛型参数比 Go 的 `net.UnixConn` 约束更宽，但不会读取或保存传入值。

文件没有模块级可变变量、自定义类型、trait、常量或 `impl` 块。

## 执行流程

`OSVersion` 的执行链是 `OSVersion -> go_arch`：先读取编译期目标架构名，执行三项 Go 命名映射或走原样回退，再由 `format!` 生成拥有所有权的结果字符串，包在 `Ok` 中返回。RustCodeGraph 的文件限定节点也记录了这条调用边。

`SetAffinity` 不遍历、不验证 `cpus`，也不调用 Windows API；无论切片为空、包含重复项还是负数，都会直接成功。这与 Go Windows 版本的空操作一致，不应将成功解释为系统已经应用亲和性。

`GetSockUID` 不检查参数，直接返回固定的 `Unsupported` 错误。由于返回前无需系统调用，调用结果与参数类型和值无关；作为值传入的参数会按普通 Rust 所有权规则在函数返回时被丢弃。

## 数据与状态

该实现无持久状态、缓存和全局可变数据。`go_arch` 的输入是当前编译目标的静态架构字符串，而不是运行时环境变量；交叉编译产物因此报告目标架构而非构建主机架构。

唯一分配发生在 `OSVersion` 的 `format!` 所创建的 `String`。`SetAffinity` 不修改输入切片；`GetSockUID` 不读取 socket，也不会产生 UID。三项公开函数均为同步函数。

## 依赖与调用关系

下游依赖只有 `std::env::consts::ARCH`、`std::format!` 形成的字符串格式化能力，以及 `std::io::{Result, Error, ErrorKind}`。[`Cargo.toml`](Cargo.toml) 中的 `libc` 和 `nix` 都受 Unix target 条件限制，不参与 Windows 实现。

上游装配由 [`lib.rs`](lib.rs) 的 Windows `cfg` 完成。直接测试调用位于 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `windows_tests::windows_fallbacks_match_go`，以及 [`sys_test.rs`](sys_test.rs) 的 `test_get_os_version`；两者也只在 Windows 目标选择本模块。RustCodeGraph 已索引本文件及 5 个节点（文件节点加 4 个函数），精确 `OSVersion` 节点给出对 `go_arch` 的调用边；文件节点显示没有跨文件使用边，因此生产调用现状还通过仓库 `rg` 交叉核验。

Go 应用主链提供迁移意图证据：`cmd/tidb-server/main.go::setCPUAffinity` 调用 Go `SetAffinity`；`pkg/server/server.go::init` 调用 Go `OSVersion`，Unix socket 接入路径调用 Go `GetSockUID`。这些是 Go 版本调用关系，不应误记为当前 Rust 生产代码已经接线。

## 错误处理与边界

`OSVersion` 和 `SetAffinity` 保留 `io::Result` 以与其他平台实现统一，但 Windows 分支当前没有错误路径。`go_arch` 对未知 Rust 架构使用原名回退；这保证函数仍能返回结果，但不能保证新架构名称天然等于 Go `GOARCH`，新增目标时需要显式核对。

`GetSockUID` 的稳定边界是错误种类 `io::ErrorKind::Unsupported` 和固定错误文本。Go 版本只构造普通 `errors.New`，没有可分类的错误种类；Rust 的分类更强，但文本保持一致。泛型按值参数不会验证“确实是 socket”，这是为了表达 Windows 上无论输入为何都不支持，而不是提供通用 socket 抽象。

`SetAffinity` 对非法 CPU 编号也成功，是与 Go Windows 空操作保持一致的兼容行为。如果上层需要验证配置，应在调用本函数前完成，不能依赖此平台实现。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或异步运行时，也没有共享可变状态；多个线程并发调用不会在模块内部互相影响。`OSVersion` 每次独立创建并返回字符串。

`SetAffinity` 不持有输入引用超过调用期。`GetSockUID<T>` 获取参数所有权后不访问它，返回错误时参数随函数栈退出而析构；若未来将参数改为真实资源类型或引用，必须重新评估关闭/析构语义。文件不打开句柄，也没有需要显式清理的 OS 资源。

## 与 Go 版本的对应关系

直接对照文件是 [`sys_windows.go`](sys_windows.go)。Go `OSVersion` 使用 `runtime.GOOS + "." + runtime.GOARCH`；Rust 将已知 Rust 架构名转换为 Go 命名并固定 OS 名为 `windows`，在常见 x86、x86-64 和 ARM64 Windows 目标上保持相同输出形状。未知架构走原名回退，属于需要随目标扩展复核的差异点。

两种语言的 `SetAffinity` 都忽略 CPU 列表并返回成功。Go `GetSockUID(net.UnixConn)` 返回零 UID 加固定错误；Rust 用 `Result<u32>`，错误分支不携带占位 UID，并额外赋予 `Unsupported` 分类。Rust 的 socket 参数是无约束泛型，接口约束弱于 Go，但当前行为仍然是无条件失败。

Go 测试 [`sys_test.go`](sys_test.go) 只要求 `OSVersion` 无错误且非空。Rust 的 [`sys_test.rs`](sys_test.rs)保留该冒烟断言，[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步验证 Windows 前缀、亲和性空操作以及 `GetSockUID` 的固定错误文本。

## 扩展指南

- 支持新的 Windows 架构时，优先修改 `go_arch` 映射，并与 Go `runtime.GOARCH` 名称逐项核对；在独立测试文件中增加精确输出断言，不要把测试嵌入本生产文件。
- 若要实现真实 Windows CPU 亲和性，应修改 `SetAffinity`，明确空列表、越界编号、进程/线程作用域和部分失败语义，同时同步 Go 行为或清楚记录兼容差异；还需在 `Cargo.toml` 以 Windows target 条件声明依赖。
- 若 Windows 将来具备可替代的对端身份机制，不应悄然改变 `GetSockUID` 的 Unix 语义；应先定义调用方所需的身份与安全边界，再收紧泛型参数并补充资源生命周期测试。
- 任何公共签名变更都需同时检查 `lib.rs` 再导出、`pkg/server/Cargo.toml` 的消费边界，以及 `migration_aster_unit_test.rs`、`sys_test.rs`。当前没有 Rust 生产调用者，新增接线时还应补充调用方测试，不能以本模块单测代替应用路径验证。
- 维持平台隔离：Windows 专属依赖和代码必须受 `cfg(target_os = "windows")` 或 Cargo target 条件保护，避免破坏 Linux/其他 Unix 构建。

## 验证依据

- Rust 源码与装配：[`sys_windows.rs`](sys_windows.rs) 的 `go_arch`、`OSVersion`、`SetAffinity`、`GetSockUID`；[`lib.rs`](lib.rs) 的 Windows 条件模块与再导出。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的库入口、平台依赖和迁移元数据；工作区根 `Cargo.toml` 的 `facade_util_sys_linux`；`pkg/server/Cargo.toml` 的路径依赖。
- Rust 测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `windows_fallbacks_match_go`；[`sys_test.rs`](sys_test.rs) 的 `test_get_os_version`。本任务按纯文档约束未运行 Cargo，因此没有 Windows 目标的动态测试证据。
- Go 对照与调用意图：[`sys_windows.go`](sys_windows.go)、[`sys_test.go`](sys_test.go)、`cmd/tidb-server/main.go::setCPUAffinity`、`pkg/server/server.go::init` 及其 Unix socket 凭据路径。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/sys/linux` 列出本 crate 的 12 个 Go/Rust 文件；文件限定 `node` 确认本文件 51 行和 4 个函数，`node OSVersion --file ...` 确认 `OSVersion -> go_arch`。对同名符号的自然语言探索会混入其他平台实现，因此调用关系最终以文件限定节点和精确文本引用复核。
- 结构验证要求：本文应恰好包含任务规定的十一个二级标题；最终交付前使用任务中的 `test`/`rg -c` 命令检查。
