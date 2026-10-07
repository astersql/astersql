# `pkg/lightning/common/storage_unix.rs`

## 文件定位

本文件是 `astersql-lightning-common` crate 的 Unix/类 Unix 本地文件系统实现层，只在 `cfg(unix)` 下由 `pkg/lightning/common/lib.rs` 编译为私有模块 `storage_unix`。对 crate 外的公开边界是 `pkg/lightning/common/storage.rs` 中重导出的同名平台门面：`GetStorageSize` 和 `SameDisk`；本文件本身不定义命令入口、存储抽象或长期服务。

`pkg/lightning/common/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/lightning/common`，并为本实现声明唯一直接外部依赖 `libc = "0.2"`。因此它的作用是把 POSIX 文件系统信息转换成 Lightning 通用类型，而不是通用对象存储层。

## 核心职责

- `GetStorageSize` 通过 `libc::statvfs` 一次获取路径所在文件系统的总块数、非特权用户可用块数与基本块大小，输出字节数快照 `StorageSize`。
- `SameDisk` 通过两个路径的 `stat` 元数据设备号 `dev()` 判断是否位于同一挂载设备/文件系统。
- 两个函数都将平台错误折叠为 crate 通用的 `CommonError`，不重试、不缓存，也不修改磁盘状态。

Go 主链证据位于 `lightning/pkg/importer/precheck_impl.go`：`localDiskPlacementCheckItem.Check` 使用 `common.SameDisk` 警告源目录与 `sorted-kv-dir` 同盘可能降低性能，`localTempKVDirCheckItem.Check` 使用 `common.GetStorageSize` 将可用空间与估算导入量、磁盘配额比较。这解释了文件为何存在；全仓 Rust 精确引用搜索则表明，当前 Rust 侧尚未有测试以外的业务调用者。

## 主要符号

- `pub fn GetStorageSize(dir: &str) -> Result<StorageSize, CommonError>`：Unix 容量查询入口。`pub` 只是为了被同 crate 的 `storage` 门面调用；由于 `storage_unix` 模块私有，crate 外应调用 `crate::GetStorageSize`。
- `pub fn SameDisk(dir1: &str, dir2: &str) -> Result<bool, CommonError>`：Unix 设备同一性查询入口。成功时 `true` 仅表示两个已解析路径的 `st_dev` 相等，不表示路径相同、可写、空间足够或硬链接必然可用。
- `StorageSize` 定义于 `pkg/lightning/common/storage.rs`，字段 `Capacity` 和 `Available` 均为字节数 `u64`。
- `CommonError` 定义于 `pkg/lightning/common/errors.rs`；本文件产生的错误 `Kind` 固定为 `"storage"`，未设置 RFC ID 或结构化 OS 错误码。

本文件没有模块级常量、自定义类型、trait、`impl` 或额外的条件编译项；平台条件在 `lib.rs` 和 `storage.rs` 中完成。

## 执行流程

`GetStorageSize` 的执行顺序如下：

1. 用 `CString::new(dir)` 把 Rust UTF-8 字符串转成 NUL 结尾的 C 路径；如果输入内含 NUL 字节，直接返回 `CommonError`，不发起系统调用。
2. 为 `libc::statvfs` 的输出结构分配未初始化的 `MaybeUninit<libc::statvfs>`，然后在单个 `unsafe` 调用中传入 C 路径指针和可写输出指针。
3. `statvfs` 非零返回值表示失败；立即读取 `std::io::Error::last_os_error()` 并返回带路径上下文的错误。
4. 只有零返回值时才 `assume_init()`，然后以 `f_frsize` 作为基本块大小，计算 `f_blocks * f_frsize` 和 `f_bavail * f_frsize`。
5. 将两个结果分别写入 `StorageSize.Capacity` 与 `StorageSize.Available` 后返回。

`SameDisk` 顺序调用 `std::fs::metadata(dir1)` 和 `std::fs::metadata(dir2)`；任一路径失败即短路返回错误，两者成功后使用 Unix `MetadataExt::dev()` 比较设备 ID。`metadata` 会跟随符号链接，所以比较的是最终目标的设备号。

## 数据与状态

实现是无状态的：输入为借用的 `&str`，输出是按值返回的 `StorageSize` 或 `bool`，没有全局变量、缓存、文件描述符或持久化副作。每次调用都读取当时的文件系统元数据，返回值只是瞬时快照，调用后的容量可立即变化。

`Available` 故意使用 `f_bavail` 而不是 `f_bfree`，表示非特权用户可使用的块，不包含为管理员保留的空间。容量乘法显式使用 `wrapping_mul`：在极端大数超过 `u64` 时按模 $2^{64}$ 回绕，不报错也不饱和。这是当前实现的边界语义，扩展时不应默认更改。

## 依赖与调用关系

- 上游 Rust 边：`pkg/lightning/common/storage.rs::GetStorageSize` 与 `SameDisk` 在 `cfg(unix)` 分支中直接调用本文件同名函数；`lib.rs` 再对外 `pub use storage::*`。RustCodeGraph 的文件节点显示直接使用文件为 `storage_unix_test.rs`，精确全仓引用搜索另外确认门面两条转发边。
- 当前 Rust 业务接线：除 `storage.rs` 门面和 `storage_test.rs` / `storage_unix_test.rs` 外，未找到 Rust 调用者。`pkg/executor/importer/import_test.rs` 中的同名 failpoint 字符串是 Go 路径兼容证据，不是对本函数的静态调用。
- 下游：`GetStorageSize` 调用标准库 `CString::new`、`CommonError::new`、`libc::statvfs` 和 `std::io::Error::last_os_error`；`SameDisk` 调用 `std::fs::metadata`、`CommonError::new` 和 Unix `MetadataExt::dev`。
- crate 边界：`Cargo.toml` 未定义控制此文件的 Cargo feature；平台选择完全依赖 Rust `cfg(unix)`。
- Go 应用链：`lightning/pkg/importer/precheck_impl.go` 是与这两项能力直接对应的业务使用点；其他 Go 调用者还包括 `pkg/domain/domain.go`、`pkg/ddl/ingest/disk_root.go` 和 `pkg/executor/importer/table_import.go` 的容量检查。这些证明 Go API 的应用位置，不代表 Rust 版已在相同链路接线。

## 错误处理与边界

- `GetStorageSize` 的内含 NUL 输入、路径不存在、无权访问或 `statvfs` 其他失败均返回 `Kind = "storage"` 的 `CommonError`。错误消息保留 `cannot get disk capacity at {dir}` 路径上下文，但 OS 错误只被格式化为文本，没有填入 `CommonError.Code` 或 `Causes`。
- `SameDisk` 的两次 `metadata` 是顺序、短路的：第一个路径失败时不读取第二个；第二个失败时丢弃已读取的第一个元数据。错误同样仅保留文本且 `Kind = "storage"`，没有像 `GetStorageSize` 那样在消息中显式附加路径前缀。
- `SameDisk` 要求两个路径在检查时都存在，且只比较设备 ID。网络文件系统、联合/覆盖挂载、容器挂载等环境中的“物理磁盘”语义由操作系统的 `st_dev` 定义，本函数不再做推断。
- 没有内建重试或错误降级；调用者必须决定失败是终止导入、警告还是重试。

## 并发与资源生命周期

两个 API 无共享可变状态，不使用锁、通道、异步任务或事务，因此可由多个线程并发调用；各调用仍是同步、阻塞的文件系统查询。

`GetStorageSize` 中 `CString`、`MaybeUninit` 和初始化后的 `statvfs` 结构都局限于单次调用，返回前自动释放；`statvfs` 不打开也不留存文件描述符。两处 `unsafe` 的安全不变式是：C 路径在调用期间有效且以 NUL 结尾；只在 `statvfs` 返回零、承诺写完输出结构后才执行 `assume_init()`。

`SameDisk` 只保留两个值类型元数据快照。它的两次查询不是原子操作：文件系统可在两次 `metadata` 之间被重新挂载或路径被替换，本文件不提供 TOCTOU 保护。

## 与 Go 版本的对应关系

Go 对照实现是 `pkg/lightning/common/storage_unix.go`，两边共享的核心语义为：容量等于总块数乘块大小，可用空间等于 `Bavail`/`f_bavail` 乘块大小，同盘判定等于两个 `Stat` 结果的设备号相等，系统调用失败立即向上传播。

已核实的差异有：

- Go `GetStorageSize` 包含 `failpoint.Inject("GetStorageSize", ...)`，可为导入预检查注入容量；Rust 实现没有对应 failpoint。Go 测试 `lightning/pkg/importer/check_info_test.go::TestLocalResource` 依赖该注入点，Rust 中出现的同路径字符串不能改变本文件的返回值。
- Go 使用 `unix.Statfs` 并先取 `Bsize`，如果结构包含 `Frsize` 再通过反射替换，以兼容不暴露 `Frsize` 的平台；Rust 直接使用 POSIX `statvfs.f_frsize`，没有回退到 `f_bsize`。
- Go 用 `errors.Annotatef` 保留可解包的原始错误；Rust 将 OS 错误格式化进 `CommonError.Message`，没有结构化 cause。
- Rust 输入是 `&str`，因而 API 不能表示非 UTF-8 Unix 路径，且会在内含 NUL 时于 FFI 前报错；Go `string` 也不能把内含 NUL 的值作为有效 C 路径，但错误产生点由 Go 的系统调用封装决定。

现有 Go 测试 `pkg/lightning/common/storage_test.go::TestGetStorageSize` 和 Rust 门面测试 `pkg/lightning/common/storage_test.rs::test_get_storage_size` 都只要求临时目录的 `Capacity` / `Available` 为正数。Rust 独立测试 `storage_unix_test.rs::get_storage_size_does_not_depend_on_df_or_path` 还在清空 `PATH` 的子进程中运行查询，证明实现不依赖外部 `df` 命令。现有 Rust 独立测试尚未覆盖 `SameDisk`、失败路径、NUL 输入或乘法溢出。

## 扩展指南

- 修改 Unix 容量计算时，主要接入点是 `GetStorageSize`。必须保留 `f_bavail` 的非特权用户语义，并明确评估 `f_frsize == 0`、大数溢出、不同 Unix 实现上字段类型与 Go `Frsize` 回退策略。
- 修改同盘规则时，主要接入点是 `SameDisk`。如果需要区分符号链接本身与目标，应有意识地选择 `symlink_metadata` 或现有 `metadata`；如果需要降低 TOCTOU 风险，应设计基于已打开文件描述符的方案，不应把单纯的再试当作原子性。
- 若要对齐 Go 的导入预检查，除本文件外还需在相应 Rust 业务链接入 `storage` 公开门面；不要误把 Go 调用者或 failpoint 字符串当成 Rust 接线。
- 测试逻辑必须继续放在独立的 `pkg/lightning/common/storage_unix_test.rs`，不得内嵌进生产文件。优先增加：同目录/不同已知设备的 `SameDisk` 用例（无可移植的第二设备时应可跳过）、不存在路径的错误用例、内含 NUL 的 `GetStorageSize` 用例，以及可注入数值计算的纯函数测试。
- 若改变公开结果或错误约定，同步复核 `storage.rs`、Windows 实现及对应独立测试，并与 `storage_unix.go` 的应用语义比较。兼容风险主要是可用空间口径和错误文本/类型改变；性能风险主要是把当前单次系统调用改成多次查询或外部命令。

## 验证依据

- RustCodeGraph 索引状态：项目已索引，目标文件被识别为 67 行、6 个符号；`node --file pkg/lightning/common/storage_unix.rs` 返回完整源码并报告 `storage_unix_test.rs` 的使用关系。
- RustCodeGraph 符号查询：`query GetStorageSize --kind function` 定位 `storage_unix.rs::GetStorageSize` （签名 `(dir: &str) -> Result<StorageSize, CommonError>`）；`query SameDisk --kind function` 定位 `storage_unix.rs::SameDisk` （签名 `(dir1: &str, dir2: &str) -> Result<bool, CommonError>`）。精确 `callers` / `callees` 命令在本次环境中长时间无输出而终止，因此没有将其当作已成功证据；调用边由已索引节点源码、模块门面和全仓精确引用搜索交叉核对。
- Rust 代码与 crate 边界：`pkg/lightning/common/storage_unix.rs`、`storage.rs`、`lib.rs`、`errors.rs`、`Cargo.toml`。目录下不存在 `doc.go`，因此无额外包约定可读。
- 独立测试：`pkg/lightning/common/storage_unix_test.rs`、`pkg/lightning/common/storage_test.rs`；对照 Go 测试为 `pkg/lightning/common/storage_test.go` 和 `lightning/pkg/importer/check_info_test.go::TestLocalResource`。
- Go 对照与业务位置：`pkg/lightning/common/storage_unix.go`、`lightning/pkg/importer/precheck_impl.go`；全仓精确引用搜索用于区分 Go 主链与 Rust 当前接线。
- 本任务是纯文档分析，依照总计划与任务要求不运行 Cargo。交付前仅运行任务指定的 11 章结构校验，并人工复核每个“已支持”陈述均有上述源码、配置、调用或测试证据。
