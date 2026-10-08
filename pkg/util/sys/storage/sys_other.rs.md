# `pkg/util/sys/storage/sys_other.rs`

## 文件定位

本文件是 `astersql-util-sys-storage` crate 的非 Linux、非 Windows、非 macOS 平台兜底实现。它只在 `cfg(all(not(target_os = "linux"), not(target_os = "windows"), not(target_os = "macos")))` 成立时参与编译；同一条件也出现在 [`lib.rs`](./lib.rs) 中，后者声明 `sys_other` 模块并把其公开项再导出为 crate API。

该文件不是磁盘容量探测的通用实现：Linux/macOS 和 Windows 分别由 `sys_posix.rs`、`sys_windows.rs` 提供真实系统调用实现。当前服务器主链也尚未直接接入这个 crate：`cmd/tidb-server/main.rs` 中的 `storage_sys` 名称来自 `cmd/tidb-server/stubs.rs` 的本地桩模块，而不是 `astersql-util-sys-storage` 依赖。

## 核心职责

唯一职责是为 Go 的 `//go:build !linux && !windows && !darwin` 分支保留兼容 API：当目标平台没有专用容量查询实现时，`GetTargetDirectoryCapacity` 不访问文件系统、不验证路径，而以“容量实际上无限大”的哨兵方式返回 `i64::MAX` 对应的无符号值和成功结果。

这样可让上层的容量与配额比较继续工作，同时避免未知平台因缺少 `statfs` 或 Windows API 实现而无法编译。代价是返回值不是目标目录的真实剩余容量，调用者不能把它用于监控或精确容量规划。

## 主要符号

- `GetTargetDirectoryCapacity<P: AsRef<std::path::Path>>(path: P) -> Result<u64, std::io::Error>`：文件中唯一的生产函数，也是唯一公开符号。泛型参数接受字符串、`Path`、`PathBuf` 等路径形态，与其他平台 Rust 实现保持统一调用形状。
- 函数受文件级目标条件约束，仅在 Linux/Windows/macOS 之外存在。`#[allow(non_snake_case)]` 保留 Go 导出函数的命名，便于逐项迁移和对照。
- 返回常量不是独立模块常量，而是函数体中的 `i64::MAX as u64`，数值为 `9_223_372_036_854_775_807`；这与 Go `math.MaxInt64` 转为 `uint64` 的结果一致。

## 执行流程

1. 调用者通过 `astersql_util_sys_storage::GetTargetDirectoryCapacity(...)`（或 crate 内的再导出）传入任意满足 `AsRef<Path>` 的值。
2. 函数把已取得所有权的 `path` 绑定给 `_` 并立即丢弃；不会调用 `as_ref()`，也不会检查路径是否存在、是否为目录或能否编码。
3. 函数构造 `Ok(i64::MAX as u64)` 并返回。当前实现没有条件分支、系统调用或错误分支。

在当前 `cmd/tidb-server` 启动流程里，`checkTempStorageQuota` 会调用同名 API并比较容量与 `TempStorageQuota`，但其实际解析目标是 `cmd/tidb-server/stubs.rs::storage_sys::GetTargetDirectoryCapacity`。因此不能把该调用边当作本文件已经接入服务器的证据；本文件目前可确认的直接消费者是 crate 自身的跨平台测试和任何显式依赖该 crate 的外部调用者。

## 数据与状态

本文件不定义结构体、枚举、trait、静态变量或可变状态。输入路径仅为保持跨平台接口一致而存在，不参与结果计算；输出是固定的 `u64` 哨兵值。

关键不变量是：只要该函数在兜底目标上成功编译，每次调用都返回同一个 `Ok(9_223_372_036_854_775_807)`，结果与路径内容和文件系统状态无关。`Result` 的错误类型仍保留为 `std::io::Error`，以便平台实现可互换，但当前函数不会主动构造错误。

## 依赖与调用关系

- 上游装配：[`lib.rs`](./lib.rs) 在相同 `cfg` 条件下声明并 `pub use` 本模块，使调用者无需感知平台文件名。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 声明包名 `astersql-util-sys-storage`、库入口 `lib.rs`。兜底分支只使用标准库；`libc` 仅属于 Linux/macOS 目标依赖，`windows-sys` 仅属于 Windows 目标依赖。
- 已确认的测试调用者：[`sys_test.rs`](./sys_test.rs) 通过 crate 根再导出调用函数并断言容量至少为 1；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的已有目录用例在所有目标运行，因此在兜底目标会覆盖固定成功值。其“缺失路径返回错误”用例明确只在 Linux/macOS/Windows 编译，不适用于本实现。
- 应用相邻链路：`cmd/tidb-server/main.rs::checkTempStorageQuota` 的业务意图是“获取临时目录容量后校验 quota”，对应 Go `cmd/tidb-server/main.go`；但 Rust 文件当前导入 `stubs::storage_sys`，故本 crate 到该入口的真实接线尚未建立。
- 下游依赖：函数体只有标准库类型、整数常量、丢弃输入和构造 `Ok`，没有可继续追踪的业务被调用函数。

## 错误处理与边界

当前实现总是成功，因此不存在权限不足、路径不存在、非目录、文件系统不可用或数值乘法溢出等错误路径。即使传入不存在路径或不可表示为 UTF-8 的 `Path`，函数也不会观察其内容。

边界风险来自语义而非运行错误：固定最大值会使通常的“quota 是否超过容量”比较通过，不能提供真实磁盘保护。返回值选用 `i64::MAX` 而非 `u64::MAX` 是为了精确保持 Go `math.MaxInt64` 行为；改动该常量会改变跨语言兼容契约。签名保留 `std::io::Error` 是跨平台统一接口的一部分，不应仅因本分支不会报错而改成裸 `u64`。

## 并发与资源生命周期

函数没有锁、原子变量、线程、异步任务、通道、事务、文件描述符或系统句柄。每次调用只拥有传入的 `path`，随后在函数内丢弃；如果调用者传入拥有资源的自定义 `P`，其正常 Rust 析构仍会在该值被丢弃时发生。

由于没有共享状态，函数天然可被多个线程并发调用，结果也与调用顺序无关。测试公共初始化 `main_test.rs::setup_for_common_test` 使用 `Once`，但那属于测试基础设施，不是本文件的资源生命周期。

## 与 Go 版本的对应关系

直接来源是 [`sys_other.go`](./sys_other.go)：Go 构建约束 `!linux && !windows && !darwin` 对应 Rust 对 Linux、Windows、macOS 的三重排除，其中 Go 的 Darwin 在 Rust 目标条件中按 macOS 表达。

Go 签名 `func GetTargetDirectoryCapacity(path string) (uint64, error)` 与 Rust 签名在语义上对应：二者都接受路径、返回无符号容量和错误通道；Rust 用 `AsRef<Path>` 扩大了可接受的路径类型，并用 `Result<u64, io::Error>` 表达双返回值。两边都忽略路径并返回 `math.MaxInt64`/`i64::MAX as u64`，且错误为空/`Ok`。

Go 的 [`sys_test.go`](./sys_test.go) 只断言当前目录查询无错误且结果至少为 1；Rust 的 `sys_test.rs` 保留同一冒烟意图。Rust 新增的 `migration_aster_unit_test.rs` 进一步覆盖其他平台的真实错误和非 UTF-8 路径，但这些条件测试没有把真实查询语义错误地施加到本兜底分支。

## 扩展指南

- 若要为某个目前落入兜底分支的操作系统增加真实容量查询，应新增独立平台实现文件并收窄本文件和 `lib.rs` 的互斥 `cfg`，同时在 `Cargo.toml` 中仅为该目标增加必要依赖；不要在本函数内堆叠平台分支。
- 行为变化应同步独立测试文件，优先扩展 `migration_aster_unit_test.rs` 或 `sys_test.rs`，不要把测试嵌入生产源文件。若新平台开始返回真实 OS 错误，还应调整测试的目标条件，覆盖存在路径、缺失路径和平台特有路径边界。
- 若要把 crate 接入服务器，需先替换 `cmd/tidb-server/stubs.rs` 中的同名桩并在相应 Cargo 清单中建立依赖；应单独验证 `checkTempStorageQuota` 的启动错误传播，不能仅凭同名函数推断接线完成。
- 保持 Go 兼容时需重点审查：构建目标集合、`i64::MAX` 哨兵值、公开函数名、路径接受能力和 `Result` 错误形状。真实查询可能引入系统调用成本和权限/挂载点兼容风险，需按平台测试，而非改变本兜底的常数时间行为。

## 验证依据

- 源码与装配：`pkg/util/sys/storage/sys_other.rs`、`pkg/util/sys/storage/lib.rs`、`pkg/util/sys/storage/Cargo.toml`。
- Go 对照：`pkg/util/sys/storage/sys_other.go`、`pkg/util/sys/storage/sys_test.go`，以及业务调用语义所在的 `cmd/tidb-server/main.go::checkTempStorageQuota`。
- Rust 测试与应用证据：`pkg/util/sys/storage/sys_test.rs`、`pkg/util/sys/storage/migration_aster_unit_test.rs`、`pkg/util/sys/storage/main_test.rs`、`cmd/tidb-server/main.rs::checkTempStorageQuota`、`cmd/tidb-server/stubs.rs::storage_sys`。
- RustCodeGraph：`status` 显示索引包含本目录 12 个文件；`query GetTargetDirectoryCapacity --kind function` 定位本函数及各平台/Go 对照；`node pkg/util/sys/storage/sys_other.rs::GetTargetDirectoryCapacity` 核实第 37—44 行函数体。`explore` 给出同名容量 API 到 `checkTempStorageQuota` 的候选关系；结合 Rust 导入和桩定义复核后，确认该候选不能证明本 crate 已接线。精确 `callers` 查询约 90 秒未返回并已中止，因此调用关系以模块再导出、文本引用和实际导入解析交叉验证；函数体没有业务 callee。
- 未运行 Cargo：本任务只新增文档，按任务约束以事实核对和固定章节结构检查代替构建测试。
