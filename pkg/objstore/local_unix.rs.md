# `pkg/objstore/local_unix.rs`

源文件：[`local_unix.rs`](./local_unix.rs)；直接调用方：[`local.rs`](./local.rs)；Go 对照：[`local_unix.go`](./local_unix.go)。

## 文件定位

本文件是 `astersql-objstore` crate 的 Unix 平台目录创建适配层。crate 根模块 [`lib.rs`](./lib.rs) 以 `pub mod local_unix` 暴露它；[`local.rs`](./local.rs) 在 `#[cfg(not(windows))]` 下导入 `local_unix::mkdirAll`，Windows 构建则改用 [`local_windows.rs`](./local_windows.rs) 的同名实现。因此它不是完整存储后端，而是 `LocalStorage` 为保持 Go 版目录权限语义而复用的单函数平台边界。

[`Cargo.toml`](./Cargo.toml) 将该模块归入包 `astersql-objstore`，库入口为 `lib.rs`。本文件直接依赖 `anyhow` 作为错误边界，并用 `libc` 调用 Unix `umask`；两项都由该 manifest 的普通依赖声明提供，没有额外 feature。包目录中没有 `doc.go`。

## 核心职责

- `mkdirAll` 递归创建目标路径及缺失祖先目录。
- 创建前把进程 umask 临时设为 `0`，再通过 `DirBuilderExt::mode(0o777)` 请求目录权限，使新目录不会被调用时的 umask 进一步裁剪；这与 [`local_unix.go`](./local_unix.go) 的 `syscall.Umask(0)` 和 `os.MkdirAll(..., localDirPerm)` 对齐。
- 用局部 RAII 守卫保存并恢复原 umask，即使 `DirBuilder::create` 返回错误或函数在栈展开期间退出，也尽量恢复进程状态。
- 把标准库 `io::Error` 转为 `anyhow::Error`，让 `LocalStorage` 的三条上层路径统一传播或补充上下文。

本文件不负责对象名规范化、文件写入、原子 rename、硬链接、重试决策或根目录存在性检查；这些策略都在 [`local.rs`](./local.rs) 中。

## 主要符号

- `#[cfg(not(windows))] use std::os::unix::fs::DirBuilderExt`：只在非 Windows 目标编译 Unix mode 扩展。文件模块仍由 `lib.rs` 声明，但核心函数与 Unix 扩展都带有或依赖非 Windows 条件。
- `pub fn mkdirAll(base: &Path) -> anyhow::Result<()>`：唯一公开函数。输入只借用路径，不保存、规范化或修改路径值；成功表示递归创建完成或目录已经存在。
- `struct UmaskGuard(libc::mode_t)`：定义在 `mkdirAll` 函数内部的私有守卫，元组字段保存调用 `libc::umask(0)` 返回的旧掩码，无法被模块外构造或观察。
- `impl Drop for UmaskGuard`：析构时再次调用 `libc::umask(self.0)` 恢复旧值。`umask` 是无失败返回通道的进程级系统调用，所以 `Drop` 不返回错误。
- `std::fs::DirBuilder`：设置 `recursive(true)` 与 Unix `mode(0o777)` 后执行实际创建。代码先保存 `create` 的结果，再显式 `drop(guard)`，最后返回该结果。

文件没有模块级常量、trait、长期存活结构体、测试模块或后台任务。

## 执行流程

1. `mkdirAll(base)` 调用 `libc::umask(0)`；该调用同时把当前进程 umask 设为零并返回旧值，旧值立即封装进局部 `UmaskGuard`。
2. 函数新建 `DirBuilder`，开启递归模式，并用 Unix 扩展设置请求 mode 为 `0o777`。
3. `builder.create(base)` 创建所有缺失目录。已存在且为目录的路径按标准库递归创建语义成功；权限、只读文件系统、路径组件不是目录等失败由 `io::Error` 表示。
4. 创建结果先经 `map_err(Into::into)` 转成 `anyhow::Result<()>` 保存，确保无论成功还是失败都继续执行恢复步骤。
5. `drop(guard)` 立即恢复进入函数前的 umask，然后函数返回此前保存的创建结果。若在守卫存活期间发生可展开的 panic，Rust 栈展开也会调用 `Drop`；若进程 abort，则不能依赖析构执行。
6. 上层 [`local.rs`](./local.rs) 在三个场景进入本流程：`Storage::WriteFile` 首次写临时文件因父目录不存在而失败后创建父目录并重试；`Storage::CopyFrom` 在创建目标硬链接前准备目标父目录；`NewLocalStorage` 在根路径不存在时创建根目录。

## 数据与状态

函数自身不保留持久状态。唯一局部状态是旧 umask、`DirBuilder` 和一次创建结果；退出后只有文件系统中新建的目录以及恢复后的进程 umask 可被观察。

mode `0o777` 是“请求的新目录权限”。因为函数临时把 umask 清零，新创建目录通常获得这三组读写执行位；现有目录不会被 chmod，也不会修正已有权限。实际权限仍可能受文件系统、挂载选项、ACL、安全模块或平台兼容层影响，所以不能把返回成功解释为逐位验证过 mode。

`base` 是 `&Path`，允许相对路径、绝对路径和多级路径。函数不把路径约束到 `LocalStorage.base`；安全的对象路径组合由调用者负责。三处生产调用均传入根目录或已经由 `LocalStorage::object_path` 派生出的父目录。

## 依赖与调用关系

上游关系：

- [`lib.rs`](./lib.rs) 声明公开模块 `local_unix`，并在独立 [`local_test.rs`](./local_test.rs) 中装配本地存储测试。
- [`local.rs`](./local.rs) 用 `#[cfg(not(windows))] use crate::local_unix::mkdirAll` 选择本实现。精确源码核对确认三处调用边：`Storage::WriteFile -> mkdirAll(parent)`、`Storage::CopyFrom -> mkdirAll(to.parent())`、`NewLocalStorage -> mkdirAll(base)`。
- [`storage.rs`](./storage.rs) 的本地后端工厂通过 `NewLocalStorage` 间接进入本函数；更高层消费者通常只面对 `Storage` trait，而不会直接调用平台辅助函数。

下游关系：

- `libc::umask` 读取并修改进程级 Unix 文件模式创建掩码。
- `std::os::unix::fs::DirBuilderExt::mode` 把 `0o777` 应用于 `DirBuilder` 创建的新目录。
- `std::fs::DirBuilder::create` 执行递归目录创建；`anyhow::Result` 只承担错误类型擦除，不添加本文件自己的上下文。

RustCodeGraph 已索引目标文件，并将 `mkdirAll` 同时定位到 Rust/Go 的 Unix 与 Windows 四个对应实现。函数级 `callers`/`callees` 查询对该限定名没有返回边，因此三处生产调用以图中的目标源码节点和 `local.rs` 精确调用点交叉核实，没有把模糊同名结果当作证据。

## 错误处理与边界

- `DirBuilder::create` 的任何 `io::Error` 都转为 `anyhow::Error` 原样上抛；本文件不重试、不记录日志，也不附加路径文本。
- `WriteFile` 调用点会在目录创建失败时保留最初写文件错误，并把 mkdir 错误放进上下文；`CopyFrom` 与 `NewLocalStorage` 则直接通过 `?` 传播本函数错误。
- 若目标路径已存在且是目录，递归创建应成功；若某个组件是普通文件、无访问权限、路径无效或文件系统不可写，则失败。函数不会删除部分创建的祖先目录，因此多级创建中途失败可能留下已成功创建的前缀目录。
- `to.parent().unwrap_or_else(|| Path::new("."))` 是 `CopyFrom` 调用方的空父路径回退，不属于本函数内部行为。
- `unsafe` 仅包围两次 `libc::umask`。代码用同一个 guard 约束旧值恢复，但没有同步其他线程，也没有校验恢复后掩码；这是对 Go 行为的直接移植边界。
- 当前独立测试没有直接断言目录 mode、失败后的 umask 恢复、panic 展开或并发 umask 干扰，因而这些结论来自源码与系统调用契约，而非本仓库运行测试结果。

## 并发与资源生命周期

`umask` 是进程级状态，不是线程局部状态。`mkdirAll` 从清零到 `drop(guard)` 之间存在一个临界窗口：同进程其他线程创建文件或目录时也会观察到零 umask；多个线程同时调用本函数时，后进入者可能把已经为零的值保存为“旧值”，并以不同顺序恢复，导致最终 umask 不一定等于第一个调用前的值。当前实现没有 mutex 或其他串行化设施，这一风险与 Go 对照实现相同，扩展时不能把 RAII 误认为并发隔离。

正常成功和普通错误返回都会显式释放守卫并恢复旧 umask；panic 栈展开会隐式释放。进程 abort、`mem::forget` 守卫（当前代码没有）或外部代码在临界窗口改写 umask，都可能破坏恢复假设。

函数没有文件描述符、线程、异步任务、通道、锁或事务。`DirBuilder` 与守卫都局限于单次同步调用；创建后的目录生命周期交给文件系统和上层存储使用者管理。

## 与 Go 版本的对应关系

直接对照文件是 [`local_unix.go`](./local_unix.go)。两版执行顺序一致：保存旧 umask、临时设为零、以 `0777` 递归创建目录、恢复旧 umask、返回创建错误。Go 的 mode 来自 [`local.go`](./local.go) 中的 `localDirPerm`，Rust 在本文件直接写出 `0o777`。

已确认的实现差异：

- Go 接收 `string`，Rust 接收 `&Path`，避免为系统路径强制 UTF-8 转换。
- Go 手工在 `os.MkdirAll` 后恢复 umask；Rust 还用 `Drop` 守卫覆盖错误返回和 panic 栈展开，并显式在返回结果前 drop。Go 若 `MkdirAll` 正常返回，无论成功失败也会恢复，但同样没有并发同步。
- Go 用 `errors.Trace(err)` 保留 PingCAP 错误栈语义；Rust只把 `io::Error` 转为 `anyhow::Error`，具体调用点再决定是否补上下文。
- Windows 两版都不触碰 umask：Rust 的 [`local_windows.rs`](./local_windows.rs) 直接调用 `create_dir_all`，平台选择在 `local.rs` 的条件导入处完成。

测试意图也对应：[`local_test.go`](./local_test.go) 与 [`local_test.rs`](./local_test.rs) 都覆盖写入嵌套路径 `123/456/789.txt`，证明缺失父目录可经 `WriteFile` 间接创建；Rust 的 [`helper_2_aster_unit_test.rs`](./helper_2_aster_unit_test.rs) 还通过两个不存在的根目录和 `copied/1.txt` 覆盖 `NewLocalStorage` 与 `CopyFrom` 的目录准备及硬链接结果。两边都没有本文件级的 umask/权限专测。

## 扩展指南

若只改变某个业务场景的重试、错误上下文、对象路径或硬链接策略，应修改 [`local.rs`](./local.rs) 对应的 `WriteFile`、`CopyFrom` 或 `NewLocalStorage`，不要把业务分支放入平台辅助函数。修改 `mkdirAll` 时必须同时维持三处调用者共同依赖的契约：递归创建、已存在目录成功、错误传播，以及 Unix 权限与 Go 版兼容。

涉及权限时，应在独立 Rust 测试文件中增加 Unix 条件测试，至少覆盖：非零初始 umask 下新目录 mode、成功和失败后旧 umask 恢复、已存在目录权限不变。由于 umask 是进程级状态，这类测试必须串行化并用 RAII 在测试退出时恢复环境，避免污染并行测试；测试不要写入本生产源文件。

若要消除并发窗口，需要设计进程内共享锁，并确保所有会修改 umask 的本仓代码遵守同一锁；只在本函数内加局部锁无法约束外部库或其他语言线程。此类变化还要核对 [`local_unix.go`](./local_unix.go) 的兼容取舍，并评估锁竞争、panic poisoning 处理和跨 FFI 调用。

若新增平台分支，应同步检查 [`lib.rs`](./lib.rs) 的模块装配、[`local.rs`](./local.rs) 的 `cfg` 选择和 Cargo 目标兼容性。性能风险主要是每次缺失目录都进行两次进程级系统调用；正确性风险则集中在全局 umask 竞态、部分目录已创建后失败，以及放宽新目录权限带来的安全边界。

## 验证依据

- RustCodeGraph：`status` 显示项目索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/objstore` 确认目标文件被索引且含 4 个符号；`node --file pkg/objstore/local_unix.rs` 核对完整 45 行源码；`query mkdirAll --kind function --json` 定位 Rust/Go 的 Unix、Windows 四个实现。限定名 `callers`/`callees` 未返回函数级结果，此限制已在调用关系章节说明。
- 源码与装配：[`local_unix.rs`](./local_unix.rs)、[`local.rs`](./local.rs)、[`lib.rs`](./lib.rs)、[`storage.rs`](./storage.rs)，确认唯一函数、条件编译、三处生产调用和本地后端工厂入口。
- crate 边界：[`Cargo.toml`](./Cargo.toml)，确认包名、库入口、`anyhow`/`libc` 普通依赖以及没有影响本实现的 feature；包目录内未发现 `doc.go`。
- Go 对照：[`local_unix.go`](./local_unix.go)、[`local.go`](./local.go)、[`local_windows.go`](./local_windows.go)，确认 `umask(0)`、`0777`、恢复顺序、三处对应调用及平台差异。
- 独立测试：[`local_test.rs`](./local_test.rs)、[`local_test.go`](./local_test.go)、[`helper_2_aster_unit_test.rs`](./helper_2_aster_unit_test.rs)，确认嵌套写入、根目录创建和 `CopyFrom` 目标父目录创建的间接覆盖；未发现直接验证 umask 或最终目录 mode 的测试。
- 按任务约束未运行 Cargo。交付仅执行固定 11 章节结构验证，并人工复核文档没有把平台辅助函数描述为完整存储后端，也没有把未覆盖的权限与并发性质写成测试已证明。
