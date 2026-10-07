# `pkg/lightning/common/storage_windows.rs`

## 文件定位

本文件是 `astersql-lightning-common` crate 的 Windows 磁盘信息后端。crate 入口 `pkg/lightning/common/lib.rs` 仅在 `cfg(windows)` 或单元测试构建时声明 `storage_windows` 私有模块；业务侧使用的是 `pkg/lightning/common/storage.rs` 再导出的公共 `GetStorageSize` 与 `SameDisk`，该门面只在 Windows 条件分支进入本文件。因此，本文件不是 Lightning 的独立入口，而是平台分发层下的 Win32 适配器。

`pkg/lightning/common/Cargo.toml` 将 crate 名定义为 `astersql-lightning-common`，库入口为 `lib.rs`。本实现只使用标准库、crate 内的 `CommonError`/`StorageSize` 和 Windows 系统库 `kernel32`，没有引入 Cargo 中的额外 Windows 第三方依赖。

## 核心职责

- `GetStorageSize` 查询路径所在卷的总容量和“调用者可用”字节数，并返回公共数据结构 `StorageSize { Capacity, Available }`。
- `GetStorageSizeWith` 把结果初始化、输出字段接线和错误归一化从 Win32 FFI 中抽出，使 Windows 逻辑可以在非 Windows 测试主机上用闭包验证。
- `SameDisk` 保留 Go Windows 实现的未完成语义：无论输入路径为何都成功返回 `false`，当前并不真正判断卷是否相同。
- 非 Windows 编译下的同名 `GetStorageSize` 是测试/可移植编译用的明确失败分支，不会替代公共门面在 Unix 上选择的 `storage_unix` 实现。

## 主要符号

- `GetDiskFreeSpaceExW(...) -> i32`：仅在 `cfg(windows)` 下声明的 `extern "system"` FFI，链接 `kernel32`。四个参数依次为 UTF-16 路径、调用者可用字节、卷总字节、卷总空闲字节；本文件只接收前两个输出，并为最后一个输出传空指针。
- `GetStorageSizeWith<F>(dir, query) -> Result<StorageSize, CommonError>`：crate 内可见的可测试核心。`query` 是只调用一次的闭包，接收原始路径以及 `Available`、`Capacity` 两个可变引用。成功时返回闭包写入的快照；失败时转换为 `Kind == "storage"` 的 `CommonError`。
- `GetStorageSize(dir) -> Result<StorageSize, CommonError>`（Windows）：公开于私有模块内部，供 `storage.rs` 调用。它将 Rust `&str` 按 Windows 宽字符编码为以 NUL 结尾的 `Vec<u16>`，随后调用 FFI。
- `GetStorageSize(dir) -> Result<StorageSize, CommonError>`（非 Windows）：仅当 `not(windows)` 时存在，经 `GetStorageSizeWith` 返回 `io::ErrorKind::Unsupported`。由于 `lib.rs` 只在 `cfg(any(windows, test))` 时装入本模块，正常非 Windows 生产构建不会接线到它。
- `SameDisk(dir1, dir2) -> Result<bool, CommonError>`：忽略两个参数并返回 `Ok(false)`；返回类型与公共平台接口一致，但当前没有系统调用和失败路径。

## 执行流程

容量查询的生产路径为：调用方进入 `storage.rs::GetStorageSize`，Windows 条件分支调用本文件的 `GetStorageSize`，后者再调用 `GetStorageSizeWith`。辅助函数先用 `StorageSize::default()` 得到两个字段均为零的快照，然后执行一次查询闭包。

Windows 闭包将传入路径编码成 UTF-16，并显式追加单个 `0` 终止码。它把 `StorageSize.Available` 和 `StorageSize.Capacity` 的可变引用作为输出指针传给 `GetDiskFreeSpaceExW`，而不请求 `total_number_of_free_bytes`。Win32 返回非零时闭包成功，辅助函数返回已写入的 `StorageSize`；返回零时读取线程的最后一个操作系统错误，再由辅助函数增加路径上下文并转换为 `CommonError`。

同盘查询的路径为 `storage.rs::SameDisk` 的 Windows 分支直接调用本文件的 `SameDisk`，随后立即得到 `Ok(false)`；两条路径字符串都不会被访问或校验。

## 数据与状态

`StorageSize` 定义在 `pkg/lightning/common/storage.rs`，两个 `u64` 字段均以字节为单位。指针接线顺序是关键不变量：第二个 Win32 参数写 `Available`，第三个参数写 `Capacity`。`GetStorageSizeWith` 在查询前将二者初始化为零，因此闭包成功但没有写字段时会返回零值；它不额外校验 `Available <= Capacity`、非零或路径存在性，这些语义由实际 Win32 调用保证或由测试闭包负责。

本文件没有全局可变状态、缓存或持久化数据。Windows FFI 声明是静态链接符号描述，不像 Go 对照实现那样在包初始化时保存 DLL/过程句柄。

## 依赖与调用关系

上游模块关系由 `lib.rs` 和 `storage.rs` 明确给出：`lib.rs` 条件装配 `storage_windows`，`storage.rs::GetStorageSize` 与 `storage.rs::SameDisk` 在 `cfg(windows)` 分支调用对应实现。RustCodeGraph 的流程结果确认两种条件编译版本的 `GetStorageSize` 都调用 `GetStorageSizeWith`；独立测试 `storage_windows_test.rs` 也直接调用辅助函数和 `SameDisk`。

下游依赖包括 `StorageSize::default`、`CommonError::new`、`std::io`、Windows 的 `OsStrExt::encode_wide` 和 `kernel32!GetDiskFreeSpaceExW`。`CommonError::new` 定义于 `pkg/lightning/common/errors.rs`，此处固定使用错误种类 `storage`。仓库当前没有发现公共 Rust `GetStorageSize`/`SameDisk` 在 common crate 外的调用；现阶段可验证的运行接线止于公共平台门面，不能据此声称 Rust Lightning 主链已经消费该能力。

## 错误处理与边界

Win32 返回零是容量查询的唯一系统失败判据。`io::Error::last_os_error()` 保留操作系统错误描述，随后形成 `cannot get disk capacity at {dir}: {error}`，所以错误同时包含原始路径和 OS 原因；`CommonError` 的 `Kind` 为 `storage`，其他元数据使用构造器默认值。

路径允许空格并按 UTF-16 传递，末尾 NUL 由实现追加。接口接收 Rust `&str`，所以它只能表示有效 UTF-8 输入；与可持有任意 Windows 宽字符串的 `OsStr` 接口相比，这是明确的输入边界。代码未拒绝路径内部的 NUL；若存在，Win32 会把第一个 NUL 当作终止位置。容量数值不进行范围或一致性校验。

非 Windows 实现总是产生带相同路径上下文的 Unsupported 错误。`SameDisk` 当前无错误边界：不存在的路径、相同路径和不同驱动器都会得到 `Ok(false)`，调用者不得把 `false` 理解为经过真实卷标识比较后的结论。

## 并发与资源生命周期

每次容量查询只创建一个局部 `StorageSize` 和一个局部宽字符缓冲区。缓冲区覆盖整个 FFI 调用周期，输出指针在调用期间指向仍存活且可写的两个 `u64` 字段；FFI 返回后不保留任何指针。DLL 符号调用没有由本文件管理的句柄、锁、任务、通道或后台线程。

函数不共享可变状态，因而本文件本身没有串行化要求。对系统卷状态的读取是瞬时快照：并发磁盘写入可能使返回值在调用结束后立即过时，这属于容量探测的固有时序边界，而不是内存数据竞争。最后一个 OS 错误必须在失败的 FFI 调用后立即读取；当前闭包在其间没有插入其他系统调用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/common/storage_windows.go`。两版都调用 `GetDiskFreeSpaceExW`，把调用者可用字节写入 `Available`、卷总字节写入 `Capacity`，忽略第四个总空闲字节输出，并在返回值为零时用 `cannot get disk capacity at <path>` 增加上下文。两版的 `SameDisk` 也都保留 FIXME 语义并返回 `false`、无错误。

实现方式存在三点可见差异。第一，Go 用 `MustLoadDLL`/`MustFindProc` 动态取得过程，Rust 通过 `#[link(name = "kernel32")] extern "system"` 声明链接符号。第二，Go 包含名为 `GetStorageSize` 的 failpoint，可直接注入容量；Rust 没有对应 failpoint，而以 `GetStorageSizeWith` 闭包作为单元测试接缝，二者不是运行时等价能力。第三，Rust 的公开输入是 UTF-8 `&str` 后再宽字符编码，Go 使用 `StringToUTF16Ptr`；文档不能把 Rust 接口描述成支持任意非 UTF-8 Windows 路径。

Go 的 `pkg/lightning/common/storage_test.go::TestGetStorageSize` 只验证临时目录能返回正容量，且受 `//go:build windows` 的实现选择影响。Rust 的 `storage_windows_test.rs` 更聚焦于字段接线、路径保留、错误注释和占位同盘契约；它没有在真实 Windows 上直接断言 FFI 成功。

## 扩展指南

若要改变 Windows 容量查询，应优先保持 `GetStorageSizeWith` 的输出顺序和统一错误格式，并在独立的 `pkg/lightning/common/storage_windows_test.rs` 中补充成功、失败及边界用例；不要把测试写回生产源文件。若增加实际的 Windows `SameDisk`，修改点是本文件的 `SameDisk`，同时必须更新当前断言恒为 `false` 的测试，并与 `storage_windows.go` 的迁移语义作出明确对齐或记录有意差异。可考虑比较卷序列号或规范化卷根路径，但在获得系统 API 与兼容性证据前不能把某一方案写成既定行为。

涉及 FFI 时应继续保证宽字符串在调用期间存活、NUL 终止、输出指针非空且可写，并在失败后立即读取 last-error。若需要支持非 UTF-8 Windows 路径，单改编码代码不够：公共 `storage.rs` 接口及所有调用者的 `&str` 边界也必须一并评估。若引入新的外部 crate，还需同步 `Cargo.toml`；当前实现不需要 Cargo feature。

兼容性风险主要是错误文本、`CommonError.Kind`、字段含义和 `SameDisk` 的保守返回值；性能风险主要来自每次调用分配宽字符 `Vec<u16>`，但当前没有测量证据表明需要缓存。任何缓存都必须先处理路径多样性、卷状态变化和并发生命周期问题。

## 验证依据

- 源码与接线：`pkg/lightning/common/storage_windows.rs`、`pkg/lightning/common/storage.rs`、`pkg/lightning/common/lib.rs`、`pkg/lightning/common/errors.rs`。
- crate 边界：`pkg/lightning/common/Cargo.toml`（package `astersql-lightning-common`，库入口 `lib.rs`）。
- Rust 独立测试：`pkg/lightning/common/storage_windows_test.rs`；补充公共冒烟面为 `pkg/lightning/common/storage_test.rs`。
- Go 对照与测试：`pkg/lightning/common/storage_windows.go`、`pkg/lightning/common/storage.go`、`pkg/lightning/common/storage_test.go`。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`explore "pkg/lightning/common/storage_windows.rs GetStorageSizeWith GetStorageSize SameDisk"` 给出 `GetStorageSize -> GetStorageSizeWith` 两条条件编译调用边、公共 `storage.rs` 源码和测试调用者；`query` 分别定位到目标文件的两个 `GetStorageSize` 条件实现、`GetStorageSizeWith` 与 `SameDisk`。单独的 `callers GetStorageSizeWith` 查询在本机未能及时返回，已终止，调用关系改由上述 `explore` 结果和源码/测试交叉确认。
- 人工复核结论：本文件存在于 Windows 平台分发边界；容量路径执行一次同步 Win32 查询并统一包装错误；同盘路径仍是保守占位；安全扩展必须维护 FFI 生命周期、字段顺序、Go 对照语义与独立测试。
