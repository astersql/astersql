# `pkg/lightning/common/storage.rs`

## 文件定位

本文件是 `astersql-lightning-common` crate 的本地文件系统容量与同盘判断公共门面。`pkg/lightning/common/lib.rs` 以私有模块 `mod storage` 装入它，并通过 `pub use storage::*` 将 `StorageSize`、`GetStorageSize` 和 `SameDisk` 暴露到 crate 根。平台相关系统调用不在本文件实现，而由条件编译分派到 `storage_unix.rs` 或 `storage_windows.rs`。

当前 RustCodeGraph 索引显示，本文件由平台实现和测试模块引用，未发现生产 Rust 调用者；仓库中同类生产调用仍位于 Go 侧，例如 Lightning 预检查的 `lightning/pkg/importer/precheck_impl.go`、DDL ingest 的 `pkg/ddl/ingest/disk_root.go`、导入执行的 `pkg/executor/importer/table_import.go` 和 domain 初始化的 `pkg/domain/domain.go`。因此本文件是已经公开的 Rust 移植边界，但不能据此声称 Rust 生产主链已接线。

## 核心职责

- 用 `StorageSize` 统一表达一个文件系统的总容量和当前调用者可用容量，单位均为字节。
- 用 `GetStorageSize(path)` 提供与平台无关的容量查询入口，并把工作交给编译目标对应的平台模块。
- 用 `SameDisk(left, right)` 提供与平台无关的同设备判断入口。
- 为非 Unix、非 Windows 目标定义显式退化行为：容量查询返回 `CommonError`，同盘判断返回 `Ok(false)`。

它不负责选择临时目录、判断容量阈值、预留空间或执行文件迁移；这些策略属于调用者。本文件也不缓存结果，每次调用都是一次独立的平台查询。

## 主要符号

- `pub struct StorageSize { Capacity: u64, Available: u64 }`：可复制的容量快照，派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`PartialEq`。字段沿用 Go 导出字段命名；`Capacity` 是文件系统总字节数，`Available` 是当前可用字节数。
- `pub fn GetStorageSize(path: &str) -> Result<StorageSize, CommonError>`：公共查询入口。Unix 调用 `crate::storage_unix::GetStorageSize`，Windows 调用 `crate::storage_windows::GetStorageSize`，其他目标返回 `CommonError::new("storage", "unsupported platform")`。
- `pub fn SameDisk(left: &str, right: &str) -> Result<bool, CommonError>`：公共同盘入口。Unix/Windows 分别委托给对应模块，其他目标保守地返回 `Ok(false)`。

本文件没有模块级常量、trait、impl、可变全局状态或异步入口。两个函数和一个结构体均经 crate 根再导出，是公开 API；平台模块本身在 `lib.rs` 中保持私有。

## 执行流程

`GetStorageSize` 的流程如下：

1. 编译期由 `#[cfg]` 选择唯一的平台分支，不进行运行时平台判断。
2. Unix 目标把原始 `&str` 交给 `storage_unix::GetStorageSize`。该实现将路径转换成无内嵌 NUL 的 `CString`，调用 `libc::statvfs`，再以 `f_blocks * f_frsize` 和 `f_bavail * f_frsize` 生成容量快照。
3. Windows 目标把原始路径交给 `storage_windows::GetStorageSize`，后者编码为以 NUL 结尾的 UTF-16，并调用 `GetDiskFreeSpaceExW`；可测试辅助函数 `GetStorageSizeWith` 负责组装结果和统一错误上下文。
4. 不支持的平台立即返回 `kind = "storage"` 的错误，不伪造容量。

`SameDisk` 同样由编译期分支决定实现：Unix 读取两个路径的文件元数据并比较 `MetadataExt::dev()`；Windows 实现为与 Go 保持一致的待实现占位，始终返回 `Ok(false)`；其他平台也返回 `Ok(false)`。因此 `false` 既可能表示确实不同设备，也可能表示该平台没有实现判断，调用者不能把它解释为可诊断的精确结论。

## 数据与状态

`StorageSize` 是调用瞬间的值快照，不持有文件描述符、路径或系统资源。两个数值均为 `u64` 字节数；本文件不规定 `Available <= Capacity` 等额外校验，而依赖平台 API 的结果。

本文件自身无共享状态、缓存和配置。输入路径仅以借用字符串存在于调用期间。Unix 实现使用 `wrapping_mul` 计算块数与块大小的乘积，这意味着极端溢出会按 `u64` 回绕而不是报错；这是扩展容量语义时需要保留或有意识修改的既有行为。

## 依赖与调用关系

- crate 装配：`pkg/lightning/common/lib.rs` 声明 `storage`，按目标声明 `storage_unix` / `storage_windows`，并再导出本文件 API。
- 错误类型：`crate::CommonError` 定义于 `pkg/lightning/common/errors.rs`；本文件只构造 `kind = "storage"` 的通用错误。
- Unix 下游：`pkg/lightning/common/storage_unix.rs` 依赖 `libc::statvfs` 和标准库文件元数据；`libc = "0.2"` 由 `pkg/lightning/common/Cargo.toml` 声明。
- Windows 下游：`pkg/lightning/common/storage_windows.rs` 直接链接 `kernel32` 的 `GetDiskFreeSpaceExW`，不增加 Cargo 第三方依赖。
- Rust 上游：RustCodeGraph 对目标文件列出平台模块与测试引用，仓库文本搜索未发现测试之外的 Rust 调用点。
- Go 业务上游：`lightning/pkg/importer/precheck_impl.go` 同时使用 `SameDisk` 与 `GetStorageSize` 做导入资源预检查；`pkg/ddl/ingest/disk_root.go`、`pkg/executor/importer/table_import.go`、`pkg/domain/domain.go` 使用容量查询。它们是 Go 对照 API 的调用证据，不是本 Rust 函数的直接调用边。

`Cargo.toml` 将本目录定义为 `astersql-lightning-common`，库入口为 `lib.rs`，并在 `package.metadata.porting` 中标记 Go 包来源为 `pkg/lightning/common`。

## 错误处理与边界

- 本文件不捕获或改写支持平台返回的 `CommonError`，错误原样通过 `Result` 向上传播。
- Unix 容量查询会拒绝含内嵌 NUL 的路径；`statvfs` 失败时错误消息包含路径与最后一个 OS 错误。Unix 同盘判断任一路径元数据读取失败都会立即返回错误。
- Windows 容量查询失败时，由 `GetStorageSizeWith` 添加 `cannot get disk capacity at <path>` 上下文并保留 OS 错误文本。
- Windows 的 `SameDisk` 当前不访问路径，也不会因路径不存在报错，而是无条件 `Ok(false)`；这是明确保留的 Go `FIXME` 合约。
- 其他平台上，两个 API 的退化不对称：容量查询报“不支持平台”，同盘判断则成功返回 `false`。新增调用者应分别处理，不能统一假设“不支持”总会表现为错误。
- API 接受 `&str`，Unix 路径必须能表达为该字符串且不得含 NUL；这不是接受任意 `OsStr`/原生字节路径的接口。

## 并发与资源生命周期

所有公开数据均按值返回，函数没有锁、任务、通道、事务或全局可变状态，因此多个线程可并发调用，彼此不共享本文件内状态。实际结果可能因外部文件系统容量在调用之间变化而不同，快照没有一致性保证。

Unix 的 `CString`、未初始化的 `statvfs` 输出缓冲区和文件元数据都局限于单次调用；只有系统调用成功后才读取输出结构。Windows 的 UTF-16 路径缓冲区和两个 `u64` 输出槽在 FFI 调用期间保持存活，调用返回后即释放；没有需要调用者关闭的句柄。

## 与 Go 版本的对应关系

- 数据模型对应 `pkg/lightning/common/storage.go` 的 `StorageSize`，字段名称、类型和字节单位一致。
- Unix 语义对应 `pkg/lightning/common/storage_unix.go`：都查询文件系统总量/可用量并按设备号判断同盘。Rust 使用 `statvfs.f_frsize`；Go 使用 `unix.Statfs`，并在平台结构存在 `Frsize` 时采用它，否则回退到 `Bsize`。Rust 当前没有 Go 的反射式回退，也没有 `GetStorageSize` failpoint。
- Windows 语义对应 `pkg/lightning/common/storage_windows.go`：都调用 `GetDiskFreeSpaceExW`，并且 `SameDisk` 都保留为无条件 `false` 的占位。Rust 同样没有 Go 的 failpoint。
- Go 公共结构体与平台函数分散在 `storage.go`、`storage_unix.go`、`storage_windows.go`；Rust 增加本文件作为稳定的跨平台分发门面，具体平台实现仍分文件维护。
- `pkg/lightning/common/storage_test.go` 与 Rust 的 `storage_test.rs` 都只对临时目录做容量大于零的冒烟检查。Rust 另有平台独立测试验证 Unix 不依赖外部 `df`/`PATH`，以及 Windows 路径传递、错误注释和 `SameDisk` 占位合约。

## 扩展指南

- 若新增公共容量字段或改变单位，先修改 `StorageSize`，同步两个平台实现，并更新 `storage_test.rs`、`storage_unix_test.rs`、`storage_windows_test.rs`；还需核对 Go 的 `storage.go` 和业务调用者是否依赖现有字段语义。
- 若实现 Windows `SameDisk`，修改真实实现文件 `storage_windows.rs` 而非在本门面加入平台逻辑，并将 `storage_windows_test.rs` 的占位断言替换为覆盖同卷、跨卷、路径错误的独立测试；同时评估是否应与 Go 的 `FIXME` 同步迁移。
- 若支持新平台，需要在 `lib.rs` 增加平台模块，在两个门面函数中添加一致的 `#[cfg]` 分支，并为容量错误与同盘“不确定”语义设计明确测试。不要让多个 `cfg` 分支在同一目标同时产生尾表达式。
- 若改为接受原生路径，应综合修改公共签名与平台签名，避免只在一个平台支持非 UTF-8 路径。
- 若补接 Rust 生产调用链，应在调用者层实现容量阈值、退化策略和用户可见错误；不要把业务策略下沉到这个平台门面。
- 测试逻辑应继续放在同目录独立测试文件中，不嵌入 `storage.rs`。兼容性风险集中在公开字段/错误文字和 Windows `false` 合约；性能风险主要来自调用频率，因为每次调用都会触发文件系统查询且无缓存。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件已索引；`files --filter pkg/lightning/common/storage.rs` 报告该文件含 4 个符号。
- RustCodeGraph `node --file pkg/lightning/common/storage.rs --offset 1 --limit 400`：核对 `StorageSize`、两个公共函数及全部条件编译分支。
- RustCodeGraph `query GetStorageSize` / `query SameDisk`：核对 Go/Rust 平台实现和测试中的同名符号；`node --file` 分别核对 `storage_unix.rs`、`storage_windows.rs` 与 `lib.rs` 的具体实现和再导出关系。
- RustCodeGraph `query CommonError --kind struct` 与 `node CommonError`：核对错误载体位于 `pkg/lightning/common/errors.rs`，包含 `Kind`、`Message` 等字段。
- 读取的 crate/模块证据：`pkg/lightning/common/Cargo.toml`、`pkg/lightning/common/lib.rs`。
- 读取的 Go 对照：`pkg/lightning/common/storage.go`、`pkg/lightning/common/storage_unix.go`、`pkg/lightning/common/storage_windows.go`、`pkg/lightning/common/storage_test.go`。
- 读取的 Rust 测试：`pkg/lightning/common/storage_test.rs`、`pkg/lightning/common/storage_unix_test.rs`、`pkg/lightning/common/storage_windows_test.rs`。
- 上游搜索：对仓库 Rust/Go 文件搜索 `GetStorageSize` 与 `SameDisk`，确认 Rust 仅见实现/测试调用，Go 生产调用点包括 Lightning 预检查、DDL ingest、executor importer 与 domain。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证文档存在且恰有 11 个固定二级章节。
