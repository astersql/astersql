# `pkg/util/sys/storage/sys_posix.rs`

## 文件定位

本文件是 `astersql-util-sys-storage` crate 在 Linux 与 macOS 上的 POSIX 实现，源码入口为 [`sys_posix.rs`](sys_posix.rs)。crate 根文件 [`lib.rs`](lib.rs) 通过 `#[cfg(any(target_os = "linux", target_os = "macos"))]` 声明并再导出 `sys_posix`；其他平台分别选择 `sys_windows.rs` 或 `sys_other.rs`，因此同一次编译只暴露一套平台实现。

crate 的 [`Cargo.toml`](Cargo.toml) 将 `lib.rs` 指定为库入口，并只在 Linux/macOS 目标上引入 `libc = "0.2"`。工作区根 `Cargo.toml` 以 `facade_util_sys_storage` 指向该 crate，随后 `pkg/lib.rs` 在 `util::sys::storage` 门面中再导出它。它属于本地文件系统容量探测工具，不负责 SQL、事务、远程存储或容量预留。

## 核心职责

文件只实现一个公开函数 `GetTargetDirectoryCapacity`，职责是返回指定路径所在文件系统对非特权用户仍可使用的字节数。实现把 Rust 路径转换为 POSIX C 字符串，调用 `libc::statfs`，读取 `f_bavail` 与 `f_bsize`，再计算 `f_bavail * f_bsize`。

这里选择 `f_bavail` 而非 `f_bfree` 是行为契约的一部分：结果排除了通常为特权用户保留的块，更接近普通进程实际可以分配的容量。函数是即时快照查询；返回值可能在调用结束后因其他进程写入、配额或文件系统状态变化而失效，不能视为预留空间或后续写入成功保证。

## 主要符号

- crate 级 `#![cfg(any(target_os = "linux", target_os = "macos"))]`：把整个文件限制在 Linux/macOS。正常接线中 `lib.rs` 已使用相同条件选择模块；文件级条件是第二层平台约束。
- `pub fn GetTargetDirectoryCapacity<P: AsRef<std::path::Path>>(path: P) -> Result<u64, std::io::Error>`：唯一公开 API。泛型参数允许传入 `&str`、`Path`、`PathBuf` 等可借用为路径的类型；成功返回可用字节数，失败返回标准 I/O 错误。
- 局部变量 `c_path`：由路径的原始 Unix 字节构造的 `CString`，保证传给 C API 的指针以 NUL 结尾并在系统调用完成前保持有效。
- 局部变量 `stat: libc::statfs`：由内核写入的文件系统统计结构。该值先被清零，只有 `statfs` 返回成功后才读取字段。
- 局部变量 `bavail`、`bsize` 与 `c`：把平台字段转换为 `u64`，并通过 `wrapping_mul` 得出与 Go `uint64` 乘法一致的结果。

文件没有模块级可变状态、类型定义、trait、impl、常量或后台任务。`#![allow(dead_code)]` 与 `#![allow(non_snake_case)]` 分别容纳当前工作区可能没有生产调用点的 API，以及为兼容 Go 命名而保留的大写函数名。

## 执行流程

1. 调用者把 `path` 传给 `GetTargetDirectoryCapacity`；函数通过 `AsRef<Path>` 取得借用视图，不复制或规范化路径。
2. `OsStrExt::as_bytes` 读取 Unix 路径原始字节，`CString::new` 增加结尾 NUL。若原路径内部包含 NUL，则在进入系统调用前返回 `InvalidInput`。
3. 函数以零值创建 `libc::statfs`，然后在一个受限的 `unsafe` 块中调用 `libc::statfs(c_path.as_ptr(), &mut stat)`。`c_path` 和 `stat` 都活到调用返回之后，分别保证输入指针和输出指针有效。
4. 非零返回码表示系统调用失败；函数立即用 `std::io::Error::last_os_error()` 捕获线程本地的 OS 错误并返回，不读取统计字段。
5. 成功时读取 `stat.f_bavail` 与 `stat.f_bsize`，转成 `u64` 后使用 `wrapping_mul` 计算字节数，最后返回 `Ok(c)`。

该函数没有重试、缓存、路径创建或备用查询。相对路径由调用时进程的当前工作目录解释；符号链接、挂载点和权限等语义全部交给操作系统的 `statfs`。

## 数据与状态

输入状态仅是调用瞬间的路径字节和进程环境（当前目录、挂载命名空间、权限）。路径不会先转成 UTF-8，因此合法的非 UTF-8 Unix 路径可以到达 `statfs`；`migration_aster_unit_test.rs` 的 `posix_path_accepts_non_utf8_bytes_like_go_string` 验证了这一点。

输出数据来自 `libc::statfs`：`f_bavail` 是非特权用户可用块数，`f_bsize` 是用于容量计算的块大小。两者被转换成 `u64`，乘积单位为字节。`wrapping_mul` 明确保留 Go 版本无符号整数溢出时的模运算语义，而不是在 Rust 调试构建中 panic。

函数不保存跨调用状态，不修改路径，也不分配持久资源。唯一的临时所有权对象是 `CString` 和栈上的 `statfs` 结构，它们在函数返回时自动释放。

## 依赖与调用关系

上游装配链为 `pkg/util/sys/storage/lib.rs` → `sys_posix` 模块及其公开再导出；工作区门面链为根 `Cargo.toml` 中的 `facade_util_sys_storage` → `pkg/lib.rs::util::sys::storage`。在 Linux/macOS 上，调用门面导出的 `GetTargetDirectoryCapacity` 最终落到本文件；Windows 和其他系统不会编译本实现。

直接下游依赖包括标准库的 `Path`/`AsRef`、`OsStrExt`、`CString`、`std::io::Error` 和 `std::mem::zeroed`，以及目标专属依赖 `libc::statfs`。真正的外部副作用只有一次 POSIX `statfs` 系统调用。

RustCodeGraph 将本文件索引为一个函数节点 `pkg/util/sys/storage/sys_posix.rs::GetTargetDirectoryCapacity`。精确节点源码确认了 `CString`、`libc::statfs` 和容量乘法流程；索引的精确 callers 查询在本地超时，callees 查询又产生明显的同名误匹配，因此不能据此声称生产调用关系。补充的全仓 `rg` 只找到 `sys_test.rs` 与 `migration_aster_unit_test.rs` 对该 Rust API 的直接调用，未找到当前 Rust 生产代码的直接调用点；这表示 API 已经接入门面并有测试覆盖，但当前仓库搜索未证明它位于活跃生产主链。

## 错误处理与边界

- 路径包含内部 NUL：`CString::new` 失败，映射为 `std::io::ErrorKind::InvalidInput`。这是 Rust FFI 必需的显式边界；Go 版本由 `syscall.Statfs` 的字符串转指针层处理对应非法输入。
- 路径不存在、不可遍历或系统调用因其他 OS 原因失败：`libc::statfs` 返回非零，函数立即返回 `last_os_error()`。`migration_aster_unit_test.rs::missing_path_returns_the_os_error` 在真实查询平台上要求缺失路径映射为 `NotFound`。
- 非 UTF-8 路径：不会因编码转换失败而提前返回；原始字节直接交给内核。独立测试要求此类缺失路径的错误不能是 `InvalidInput`。
- 算术溢出：函数采用 `wrapping_mul`，保持 Go `uint64` 乘法语义。结果不做饱和或范围报错。
- 容量为零：这是合法的成功结果，函数不会把它转换成错误。现有冒烟测试选择一个预期有空间的目录并断言结果大于零，不代表 API 对所有有效文件系统都保证正数。

本文件不处理磁盘配额差异、稀疏文件、预分配、并发写入导致的竞态，也不验证返回容量是否足以完成后续业务操作。需要“检查并保证可写”的上层逻辑不能只依赖此快照。

## 并发与资源生命周期

实现无全局变量、锁、通道、线程、异步任务或缓存，每次调用只使用自己的栈状态和 `CString`，因此函数本身可由多个线程并发调用。它没有要求调用者串行化，但操作系统返回的是各次调用时刻的独立快照，并发调用之间不保证数值一致。

两个 `unsafe` 点承担清晰且局部的资源契约：`zeroed` 用于创建供内核完整写入的 C 结构，`libc::statfs` 接收在调用期间有效的 NUL 结尾路径指针和可写结构指针。系统调用返回后不保留这些指针，也没有文件描述符需要关闭。错误分支在读取输出字段之前退出，避免使用失败调用后的内容。

测试公共初始化位于独立的 `main_test.rs`，使用 `std::sync::Once` 调用测试环境设置；这不属于生产函数的并发状态。Go `main_test.go` 还执行 goroutine 泄漏检查，而 Rust 测试设置明确不创建对应的后台线程检查器。

## 与 Go 版本的对应关系

直接对照文件是 [`sys_posix.go`](sys_posix.go)，其 build tag 为 `linux || darwin`。Rust 用 Linux/macOS cfg 表达相同范围，其中 Go 的 `darwin` 对应 Rust 的 `target_os = "macos"`。

核心步骤逐项保持一致：Go 声明 `syscall.Statfs_t`，Rust 声明 `libc::statfs`；两者调用平台 `statfs`；失败时都返回零值结果语义之外的原始系统错误；成功时都计算 `Bavail * Bsize` 并返回 `uint64`/`u64`。Rust 的 `wrapping_mul` 是对 Go 无符号乘法溢出规则的显式复刻。

语言边界造成的可见差异是：Rust API 用 `Result<u64, io::Error>` 而不是 `(uint64, error)`，并接受任意 `AsRef<Path>`；FFI 前必须构造 `CString`，所以内部 NUL 被明确映射为 `InvalidInput`。Rust 不要求 UTF-8，因而仍保持 Go `string` 可携带任意非 NUL 字节的关键路径能力。

Go `sys_test.go::TestGetTargetDirectoryCapacity` 与 Rust `sys_test.rs::test_get_target_directory_capacity` 都查询 `"."` 并断言结果至少为 1。Rust 的 `migration_aster_unit_test.rs` 另覆盖临时目录、缺失路径和 Unix 非 UTF-8 路径，是当前实现边界的更细验证证据。

## 扩展指南

若修改容量定义（例如改用 `f_bfree`、考虑配额或新增饱和运算），应首先修改或评审 `GetTargetDirectoryCapacity`，并同步核对 `sys_posix.go`，因为这些变化会改变 Go 对齐语义。平台共用 API 的签名变化还必须同步 `sys_windows.rs`、`sys_other.rs`、`lib.rs` 的再导出以及根门面使用方。

新增行为测试应放在独立测试文件中，不要嵌入 `sys_posix.rs`：通用契约优先扩展 `migration_aster_unit_test.rs`，与 Go 现有冒烟用例对应的断言扩展 `sys_test.rs`；平台专属边界可以新建同目录的独立 `*_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 模块挂载。应至少覆盖内部 NUL、系统调用错误种类、零容量可接受性和乘法策略；涉及真实磁盘数值的断言要避免依赖易变的精确剩余空间。

扩展 `unsafe` 代码时应继续把原始指针生命周期限制在单次系统调用内，并在任何输出字段读取前检查返回码。性能上当前成本主要是一次小型分配和一次系统调用；若上层引入缓存，必须明确过期策略与挂载点变化风险，不能在本底层函数中隐式缓存快照。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/sys/storage` 确认平台实现、模块入口与独立测试均在索引中。
- RustCodeGraph `node --file pkg/util/sys/storage/sys_posix.rs` 及精确 `node pkg/util/sys/storage/sys_posix.rs::GetTargetDirectoryCapacity`：确认文件共 70 行、仅一个函数节点及完整控制流。
- RustCodeGraph `query GetTargetDirectoryCapacity --kind function`：确认 POSIX Rust/Go、Windows、其他平台与测试中的同名符号。精确 `callers` 查询超时无结果；`callees` 输出发生同名扩散，故未将其误匹配边作为结论。
- [`sys_posix.rs`](sys_posix.rs)：平台 cfg、函数签名、路径字节转换、`statfs` 调用、错误传播及 `wrapping_mul` 的首要证据。
- [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、仓库根 `Cargo.toml` 与 `pkg/lib.rs`：crate 边界、目标专属依赖、平台选择及工作区公开门面的证据。
- [`sys_posix.go`](sys_posix.go)：Go build tag、`syscall.Statfs`、`Bavail * Bsize` 与错误返回的直接对照。
- [`sys_test.rs`](sys_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`main_test.rs`](main_test.rs)：当前目录冒烟、临时目录、缺失路径、非 UTF-8 路径及测试初始化证据。
- [`sys_test.go`](sys_test.go)、[`main_test.go`](main_test.go)：Go 冒烟测试和 Go 测试生命周期的对照证据。
- 全仓 `rg` 对 `GetTargetDirectoryCapacity`、crate 名和门面名的检索：确认 Rust 直接调用目前仅见于上述独立测试，并确认工作区依赖及再导出位置。该结论限于当前检出中的静态文本引用，不等同于证明外部消费者不存在。

本任务是纯文档分析，按计划不运行 Cargo。结构验收以文件存在且固定十一个二级标题恰好各出现一次为准，并辅以人工复核：文档说明了文件存在原因、实际执行步骤、平台/FFI 边界、安全扩展入口与当前调用证据的限制。
