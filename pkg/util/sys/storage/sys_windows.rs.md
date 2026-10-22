# `pkg/util/sys/storage/sys_windows.rs`

## 文件定位

唯一职责是实现 `GetTargetDirectoryCapacity`：把调用者提供的路径转换成 Windows API 接受的、以 NUL 结尾的 UTF-16 路径，调用 `GetDiskFreeSpaceExW`，并返回该路径所在卷上“调用者可用”的空闲字节数。

## 核心职责

唯一职责是实现 `GetTargetDirectoryCapacity`：把调用者提供的路径转换成 Windows API 接受的、以 NUL 结尾的 UTF-16 路径，调用 `GetDiskFreeSpaceExW`，并返回该路径所在卷上“调用者可用”的空闲字节数。

实现只请求 `GetDiskFreeSpaceExW` 的第一个输出值 `lpFreeBytesAvailableToCaller`，总字节数和总空闲字节数两个输出指针均传空。这一点与同路径 Go 文件 `sys_windows.go` 的 `windows.GetDiskFreeSpaceEx(..., &freeBytes, nil, nil)` 一致；它回答的是配额判断所需的可用容量，不是卷总容量。

## 主要符号

- `GetTargetDirectoryCapacity<P: AsRef<std::path::Path>>(path: P) -> Result<u64, std::io::Error>`（`sys_windows.rs:43`）：本文件唯一公开函数。泛型参数允许 `&str`、`String`、`Path`、`PathBuf` 等路径形态，成功值以字节为单位。
- `free_bytes: u64`（`sys_windows.rs:47`）：交给 Windows API 写入的输出槽；只在系统调用成功后返回。
- `wide_path: Vec<u16>`（`sys_windows.rs:51`）：由 `OsStrExt::encode_wide` 生成并追加一个终止 NUL。缓冲区局部持有，覆盖整个 FFI 调用，保证传入指针在调用期间有效。
- `ok`（`sys_windows.rs:61`）：`GetDiskFreeSpaceExW` 的非零/零成功标志；零值进入 `last_os_error` 错误分支。

文件没有自定义类型、trait、常量或 `impl`。`#![allow(dead_code)]` 与 `#![allow(non_snake_case)]` 分别容纳当前接线状态及为对齐 Go API 而保留的函数命名。

## 执行流程

1. 调用者传入可借用为 `Path` 的目标路径。
2. `path.as_ref().as_os_str().encode_wide()` 按 Windows 原生宽字符表示编码路径，随后追加 `0`，收集为 `Vec<u16>`。
3. 在一个局部 `unsafe` 块中调用 `windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW`：输入 `wide_path.as_ptr()`，把 `&mut free_bytes` 作为调用者可用容量输出，另外两个输出传 `null_mut()`。
4. 若返回值为零，立即用 `std::io::Error::last_os_error()` 捕获线程当前的 Windows OS 错误并返回 `Err`。
5. 若返回值非零，返回 API 写入的 `free_bytes`。

Go 的上游意图可由 `cmd/tidb-server/main.go::checkTempStorageQuota` 复核：仅当 `TempStorageQuota >= 0` 时查询容量，容量小于 quota 则阻止继续启动。Rust 的 `cmd/tidb-server/main.rs` 保留相同判断结构，但目前调用桩模块；不能据此声称真实 Windows 查询已经接入服务主链。

## 数据与状态

函数是无持久状态的同步查询。所有数据都在单次调用的栈帧或局部堆缓冲区中：`wide_path` 拥有 UTF-16 数据，`free_bytes` 接收一个 `u64` 结果。函数不缓存容量，不修改全局配置，也不保存路径。

返回值语义来自 Windows API 的 `lpFreeBytesAvailableToCaller`，可能受用户磁盘配额影响；它不保证等于卷上所有用户可见的总空闲空间。数值直接由 API 写入，无额外换算或乘法，因此不存在本文件内的容量算术溢出路径。

## 依赖与调用关系

- 平台依赖：`Cargo.toml` 仅在 `cfg(target_os = "windows")` 下启用 `windows-sys = 0.61` 及 `Win32_Storage_FileSystem` feature；这提供 `GetDiskFreeSpaceExW` 绑定。
- 标准库依赖：`std::os::windows::ffi::OsStrExt` 保留 Windows 路径的宽字符语义；`Path`/`AsRef<Path>` 提供通用路径入参；`std::io::Error` 承载 OS 错误。
- 模块入口：`lib.rs` 只在 Windows 上编译并 `pub use sys_windows::*`。工作区根 `Cargo.toml` 把 crate 注册为 `facade_util_sys_storage`，`pkg/lib.rs` 再导出到 `pkg::util::sys::storage`。
- 已核实调用者：独立 Rust 测试 `sys_test.rs::test_get_target_directory_capacity` 与 `migration_aster_unit_test.rs` 通过 crate 根再导出调用该 API；其中 Windows 可运行现有目录和缺失路径用例。
- 应用接线边界：`cmd/tidb-server/main.rs::checkTempStorageQuota` 的名称和业务意图与 Go 调用点对应，但其 `storage_sys` 明确来自 `crate::stubs`，不是本 crate。代码搜索未发现其他 Rust 生产调用点。

RustCodeGraph 精确符号查询同时找到各平台同名函数；由于重名，`callers` 未给出可归属到 Windows 定义的调用边，故调用者结论由模块条件编译和仓库级精确文本搜索交叉核验，而不是把其他平台或桩函数的边误归给本文件。

## 错误处理与边界

`GetDiskFreeSpaceExW` 返回零时，函数立即返回 `std::io::Error::last_os_error()`；成功时不会检查或改写 OS 状态。缺失路径的预期由 `migration_aster_unit_test.rs::missing_path_returns_the_os_error` 固化为 `ErrorKind::NotFound`（在 Windows/Linux/macOS 真实实现上启用）。权限不足、无效路径、不可用卷等其他失败保留 Windows 原始错误映射，没有重试或降级值。

路径转换不会进行 UTF-8 强制转换，因而适合 Windows 原生路径字符。但当前实现只是无条件在编码结果末尾追加 NUL；若输入 `OsStr` 内含嵌入 NUL，本文件没有先行拒绝，Windows API 会看到第一个 NUL 之前的路径。这一点未被现有测试覆盖，扩展或加固时应先确认与 Go `windows.StringToUTF16Ptr` 的兼容预期。

本函数不判断传入对象是否为目录；它把路径直接交给 Windows API。容量为零仍属于成功结果，调用者必须自行决定零容量的业务含义。

## 并发与资源生命周期

函数没有共享可变状态、锁、通道、后台任务或异步边界；多个线程可独立调用，彼此只通过操作系统的文件系统状态产生间接关联。

`wide_path` 和 `free_bytes` 都活到 FFI 调用返回之后，避免悬垂指针。`unsafe` 范围仅包围一次系统调用：输入指针指向连续、以 NUL 结尾且在调用期间不移动的 `Vec<u16>`，输出指针指向有效的可写 `u64`，其余输出明确为空。返回后 `Vec` 自动释放，不持有 Windows handle，因此没有显式关闭或泄漏路径。

## 与 Go 版本的对应关系

- Go `path string` 对应 Rust 泛型 `P: AsRef<Path>`；Rust 接口更宽，但仍把路径转换为 Windows UTF-16。

## 扩展指南

- 若要返回总容量或总空闲容量，应扩展 `GetDiskFreeSpaceExW` 的后两个输出，但需先设计新的返回类型；不要悄悄改变现有 `u64` 的“调用者可用容量”语义。
- 若要把真实实现接入服务启动流程，应修改 `cmd/tidb-server` 的模块依赖和导入，使 `checkTempStorageQuota` 使用 `astersql-util-sys-storage`/门面导出，并同步删除或绕开 `stubs.rs::storage_sys` 的固定值路径。这属于跨文件接线工作，不是本文件当前已经具备的行为。
- 修改路径编码、错误映射或 FFI 参数时，应在独立测试文件中增加 Windows 条件测试，优先扩展 `migration_aster_unit_test.rs`；不要把测试嵌入本生产文件。建议覆盖 Unicode 路径、磁盘配额语义、嵌入 NUL 的兼容决策和明确的不存在路径。
- 保持 Windows 依赖位于 Cargo 的 target-specific dependency 下，并保留 `Win32_Storage_FileSystem` feature，避免让非 Windows 构建无条件承担平台绑定。
- FFI 调整必须继续维持指针有效期、终止 NUL、输出指针对齐/可写及 `last_os_error` 的即时捕获；这些是主要正确性与兼容风险。容量查询是同步系统调用，若将来放入高频路径，应由上层评估缓存和阻塞成本，而不是在本文件引入隐式全局缓存。
