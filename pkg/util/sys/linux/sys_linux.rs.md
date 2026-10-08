# `pkg/util/sys/linux/sys_linux.rs`

## 文件定位

该文件是 `astersql-util-sys-linux` crate 在 Linux 目标上的系统调用实现，源码由
[`lib.rs`](./lib.rs) 通过 `#[cfg(target_os = "linux")] pub mod sys_linux` 选择并将公开 API
再导出。crate 的 [`Cargo.toml`](./Cargo.toml) 把库入口设为 `lib.rs`，仅在 Unix 目标引入
`libc = "0.2"`；因此这里直接面向 Linux libc ABI，而不是一个跨平台通用实现。

文件提供三项公开能力：读取内核版本信息、设置当前进程 CPU 亲和性、读取 Unix domain
socket 对端的有效 UID；另有一个 crate 内可见的 C 字符数组转换辅助函数。它对应 Go 包
`pkg/util/sys/linux` 的 Linux 实现，但当前 Rust 生产接线并不完整：工作区根清单以
`facade_util_sys_linux` 登记该 crate，`pkg/server/Cargo.toml` 也声明依赖；代码搜索却未发现
Rust `pkg/server` 对这些 API 的调用，`cmd/tidb-server/main.rs` 的绑核路径仍调用
`cmd/tidb-server/stubs.rs` 中的同名 `linux::SetAffinity` 桩。因而本文件目前是可独立测试的
真实 Linux 实现，不能据此断言所有 Go 生产调用链已经迁移。

## 核心职责

- `charsToString` 把 `libc::utsname` 中的定长 C 整型字符数组转换成 Rust `String`，并在首个
  NUL 处停止。
- `OSVersion` 调用 `libc::uname`，按 `Sysname Release.Machine` 形状生成操作系统版本串。
- `SetAffinity` 构造 `libc::cpu_set_t` 位图，并通过 `sched_setaffinity` 修改当前进程的 CPU
  亲和性。
- `GetSockUID` 对 `UnixStream` 的原始文件描述符读取 `SO_PEERCRED`，返回对端凭据中的 UID。

这些函数只封装同步、进程本地的系统调用，不维护缓存、全局配置或后台任务。公开函数均以
`std::io::Result` 暴露操作系统错误，使调用方能够决定失败是否阻塞启动或连接处理。

## 主要符号

- `pub(crate) fn charsToString<T>(ca: &[T]) -> String where T: Copy + Into<i64>`：接受不同
  libc 平台表示所需的整型切片；逐项转成 `i64`，在零值处截断，再以 `u8` 收集并用
  `String::from_utf8_lossy` 构造拥有所有权的字符串。它不是 crate 外 API。
- `pub fn OSVersion() -> io::Result<String>`：零初始化 `libc::utsname`，执行 `libc::uname`，
  成功后组合 `sysname`、`release` 和 `machine`。格式中系统名与 release 之间是空格，release
  与架构之间是点号。
- `pub fn SetAffinity(cpus: &[i32]) -> io::Result<()>`：清空 CPU 位图，只设置非负且小于
  `libc::CPU_SETSIZE` 的编号，然后以 `libc::getpid()` 为目标执行 `libc::sched_setaffinity`。
- `pub fn GetSockUID(socket: &UnixStream) -> io::Result<u32>`：通过 `AsRawFd` 借用 socket 的
  文件描述符，使用 `SOL_SOCKET/SO_PEERCRED` 填充 `libc::ucred`，校验内核返回长度后读取
  `credential.uid`。

文件没有模块级可变状态、结构体、trait 或 `impl`；除 crate 入口的 Linux 条件编译外，文件
内部没有 feature 或条件编译分支。`#![allow(non_snake_case)]` 保留与 Go API 一致的符号名。

## 执行流程

`OSVersion` 的执行顺序是：零初始化 `utsname`；调用 `uname`；非零返回值立即转换为
`last_os_error`；成功时分别用 `charsToString` 处理三个 C 数组；最后格式化结果。辅助转换在
第一个 NUL 处停止；若第一个元素就是 NUL，结果为空串；若字节不是有效 UTF-8，则替换无效
序列而不返回转换错误。

`SetAffinity` 先得到全零 `cpu_set_t`，随后遍历输入。负数和超出位图容量的编号被忽略，合法
编号交给 `CPU_SET`。完成后，无论输入是否最终设置了任一位，都把整个位图交给内核。空输入
或全部无效输入通常形成空掩码，并由内核以错误返回；函数本身不预先发明错误类型。调用目标
是 `getpid()`，即当前进程，而非传入线程 ID。

`GetSockUID` 零初始化 `ucred` 和长度，取得但不接管 `UnixStream` 的原始 fd，再调用
`getsockopt`。系统调用失败时直接返回 errno；成功后必须确认返回长度恰好等于
`size_of::<libc::ucred>()`，否则返回 `InvalidData`；最后才读取 UID。该顺序避免在 ABI 返回
尺寸异常时把部分凭据当作完整结果。

## 数据与状态

所有数据均为调用栈上的临时值：`utsname`、`cpu_set_t`、`ucred` 和 `socklen_t` 在函数返回后
销毁。唯一返回的持久数据是拥有所有权的 `String`、`u32` UID 或错误值。

`charsToString` 会为中间 `Vec<u8>` 和最终 `String` 分配内存；`OSVersion` 还会创建格式化结果。
`SetAffinity` 修改内核保存的当前进程调度属性，这是本文件唯一具有进程级持久副作用的操作。
`GetSockUID` 只查询 socket 状态，不修改 socket，也不保存 fd；`AsRawFd` 不转移所有权，因此
调用结束不会关闭用户传入的流。

关键不变量是：传给 `CPU_SET` 的索引已经过非负和 `CPU_SETSIZE` 上界检查；读取 `ucred.uid`
前已经确认 `getsockopt` 成功且返回结构尺寸符合本地 ABI；`utsname` 字段只在 `uname` 成功后
用于格式化。

## 依赖与调用关系

下游依赖集中在标准库和 `libc`：`std::mem` 负责 C 结构零初始化和尺寸计算，`std::io` 承载
错误，`std::os::fd::AsRawFd` 与 `std::os::unix::net::UnixStream` 提供 Unix socket 边界；libc
符号包括 `uname`、CPU 位图宏、`getpid`、`sched_setaffinity`、`getsockopt` 及相应常量和结构。

RustCodeGraph 将本文件识别为 5 个符号（文件节点及四个函数），并显示它被
`migration_aster_unit_test.rs` 与 Go 对照文件引用。精确名称查询还识别了三个 API 的跨平台
Rust/Go 同名实现。原始 Rust 引用搜索给出的实际调用边为：`OSVersion` 被 `sys_test.rs` 和
`migration_aster_unit_test.rs` 调用；`SetAffinity`、`GetSockUID` 和 `charsToString` 被
`migration_aster_unit_test.rs` 调用。生产 Rust 搜索没有找到指向本文件 API 的直接调用。

Go 生产链提供迁移意图证据：`cmd/tidb-server/main.go::setCPUAffinity` 调用 Go
`linux.SetAffinity`；`pkg/server/server.go::init` 调用 `linux.OSVersion`；Unix socket 接入路径
调用 `linux.GetSockUID`。这些是 Go 调用边，不应当冒充当前 Rust 调用边。尤其 Rust
`cmd/tidb-server/main.rs::setCPUAffinity` 目前解析配置后调用的是 `crate::stubs::linux`，后者只
记录事件并返回成功。

## 错误处理与边界

三个公开函数都不 panic 于正常系统调用失败：`uname`、`sched_setaffinity`、`getsockopt`
非零返回值被转换为 `io::Error::last_os_error()`。`GetSockUID` 另外把成功但结构长度异常的
情况归为 `io::ErrorKind::InvalidData`，错误消息为
`SO_PEERCRED returned an unexpected credential size`。

`charsToString` 有意使用 lossy UTF-8：内核字段中的无效字节会表现为替换字符，而不会令
`OSVersion` 失败。整数到 `u8` 的转换使用截断语义；这里依赖 `utsname` 字段本质为 C 字节
数组这一调用约束，不适合作为任意大整数的通用转换器。

`SetAffinity` 静默忽略负数和 `CPU_SETSIZE` 之外的编号，以对齐 Go `unix.CPUSet.Set` 的位图
边界语义。其后果是“输入非空”不保证“掩码非空”；最终有效性仍由内核判断。权限不足、CPU
不可用、空掩码或平台 ABI 问题均通过 errno 返回。

`GetSockUID` 只适用于 Linux Unix domain socket 的 `SO_PEERCRED`。传入的 Rust 类型已经排除
普通 TCP socket，但 socket 的实时状态和内核支持仍可能令调用失败。函数返回凭据里的 UID，
不执行授权判断；安全策略必须由上层使用该 UID 的代码承担。

## 并发与资源生命周期

文件没有锁、原子变量、通道、异步任务或共享 Rust 状态；函数可以由多个线程并发调用。
`OSVersion` 和 `GetSockUID` 是同步查询。`GetSockUID` 只在调用期间借用 `&UnixStream`，并借用
其 fd；它不复制所有权、不关闭 fd，也不延长 socket 生命周期。安全调用要求调用期间该引用
保持有效，这由 Rust 借用规则保证。

`SetAffinity` 的效果属于进程调度状态，可能与其他同时修改亲和性的代码产生最后写入者胜出
的竞争；本文件不做序列化。使用 `getpid()` 与 Go 对照一致，但调用者若需要线程级 affinity、
原子地协调线程池规模或恢复旧掩码，应在更高层设计相应生命周期，不能假设此函数提供这些
保证。

各处 `unsafe` 都局限于 libc FFI、零初始化 C 结构、CPU 位图宏和原始 fd 系统调用。C 结构在
调用前被清零，输出长度可写，指针只在同步系统调用期间存活；新增 FFI 时应维持同样的局部化
和返回值检查。

## 与 Go 版本的对应关系

[`sys_linux.go`](./sys_linux.go) 是直接语义基线。两端均在 NUL 处截断 `utsname` 字段，输出
`Sysname Release.Machine`；均构造 CPU 位图并对当前进程调用调度接口；均通过
`SOL_SOCKET/SO_PEERCRED` 获取对端 UID。

存在几处语言和边界实现差异：Go `charsToString` 对整数字节直接构造字符串，Rust 使用
`from_utf8_lossy`，所以无效 UTF-8 的具体表现不同；Go 通过 `net.UnixConn.SyscallConn` 的
`Control` 闭包协调 fd 访问，Rust 依靠 `&UnixStream` 和 `AsRawFd` 的借用生命周期；Rust 额外
校验 `getsockopt` 返回的结构尺寸。Go 的 `SetAffinity` 由 `x/sys/unix.CPUSet` 隐式处理越界，
Rust 在调用 `CPU_SET` 前显式检查上下界。

测试对齐方面，Go [`sys_test.go`](./sys_test.go) 只验证 `OSVersion` 成功且非空；Rust
[`sys_test.rs`](./sys_test.rs) 复刻这项冒烟测试，
[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 进一步验证 NUL 截断、版本串
形状、空 affinity 掩码的内核错误，以及 socket pair 的对端 UID 等于当前有效 UID。
[`main_test.rs`](./main_test.rs) 处理 crate 测试环境初始化，不直接验证本文件行为。

## 扩展指南

若扩展版本信息格式，应先修改 `OSVersion` 并同步 Go 兼容性判断及
`sys_test.rs`、`migration_aster_unit_test.rs`；格式可能被诊断信息或客户端属性消费，不能只以
“非空”作为兼容性标准。若新增 `utsname` 字段，要继续通过 `charsToString` 或等价的有界 NUL
解析，禁止把定长 C 缓冲区当作无界字符串。

若扩展 CPU 亲和性，应在 `SetAffinity` 附近明确进程/线程语义、无效编号策略和空集合行为，
并在独立测试文件增加成功路径及权限/拓扑可控的边界测试；不要把测试嵌入生产源文件。真正接入
Rust 服务启动链时，还需把 `cmd/tidb-server/stubs.rs::linux::SetAffinity` 替换为该 crate 的
调用，并验证线程池并行度处理与 Go `GOMAXPROCS` 行为差异，而不是仅删除桩。

若扩展 socket 凭据，应在 `GetSockUID` 中保持 fd 所有权不转移、ABI 长度检查和 errno 传播，
并使用 Unix socket pair 在 `migration_aster_unit_test.rs` 验证。任何授权规则应放在服务器连接
处理层；本工具层只负责可信地读取内核凭据。跨平台行为应分别修改 `sys_other.rs` 或
`sys_windows.rs`，不要用运行时判断绕过 `lib.rs` 已有的目标平台选择。

性能风险主要是系统调用本身和 `OSVersion` 的小额分配；这些函数不在数据行处理热路径。
兼容性风险主要来自 Linux ABI、公开符号名、版本串形状与 affinity 作用域。修改公开签名时还
应检查工作区根依赖别名和 `pkg/server/Cargo.toml` 的 crate 边界。

## 验证依据

- RustCodeGraph：`status` 显示当前索引含本目录；`files --filter pkg/util/sys/linux` 列出目标、
  平台实现与测试；`node --file pkg/util/sys/linux/sys_linux.rs` 展示完整 109 行及引用文件；对
  `OSVersion`、`SetAffinity`、`GetSockUID`、`charsToString` 的 `query --kind function` 核对符号。
- 实现与装配：[`sys_linux.rs`](./sys_linux.rs)、[`lib.rs`](./lib.rs)、
  [`Cargo.toml`](./Cargo.toml)、工作区根 `Cargo.toml`、`pkg/server/Cargo.toml`。
- Go 对照与生产调用：[`sys_linux.go`](./sys_linux.go)、`cmd/tidb-server/main.go`、
  `pkg/server/server.go`。
- Rust 当前接线：`cmd/tidb-server/main.rs::setCPUAffinity` 与
  `cmd/tidb-server/stubs.rs::linux::SetAffinity`；全仓 Rust 符号搜索未发现本文件三个公开 API
  的其他生产调用。
- 测试证据：[`sys_test.go`](./sys_test.go)、[`sys_test.rs`](./sys_test.rs)、
  [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`main_test.go`](./main_test.go)、
  [`main_test.rs`](./main_test.rs)。本任务依约不运行 Cargo；测试文件仅作为行为证据读取。
- 结构验收使用任务指定命令，要求目标存在且固定二级标题恰好为 11 个；同时人工复核相对链接、
  Rust/Go 调用边区分、错误边界、资源生命周期和扩展入口。
