# `pkg/objstore/local_windows.rs`

## 文件定位

本文件是 `astersql-objstore` crate 的 Windows 平台目录创建适配层。crate 入口 `pkg/objstore/lib.rs` 公开声明 `local_windows` 模块；`pkg/objstore/local.rs` 仅在 `cfg(windows)` 下导入这里的 `mkdirAll`，在非 Windows 平台则改用 `local_unix.rs` 的同名实现。文件中的导入和函数均受 `#[cfg(windows)]` 约束，因此它不参与非 Windows 目标的有效代码生成。

`pkg/objstore/Cargo.toml` 将该 crate 定义为 `astersql-objstore`，库入口为 `lib.rs`，并通过 `anyhow = "1"` 提供本文件公开签名中的 `Result`。本文件不是完整的本地对象存储实现；对象路径映射、写入、复制和构造流程都位于 `pkg/objstore/local.rs`。

## 核心职责

唯一职责是由 `mkdirAll(base: &Path)` 在 Windows 上递归确保目标目录存在。实现直接调用 `std::fs::create_dir_all(base)`，让已经存在的目录保持成功语义，并把底层 `std::io::Error` 转换为 `anyhow::Error` 后交给调用者。

该平台分层隔离了 Unix 特有的 umask 行为：Windows 版本不设置权限位、不改变进程级 umask，也不包含重试或路径规范化。上述策略由调用方 `local.rs` 负责选择，不能把 Unix 版本的权限处理视为本文件行为。

## 主要符号

- `use std::path::Path`：Windows 下借用路径参数所需的标准库类型；不拥有或复制路径。
- `use anyhow::Result`：统一返回 `Result<()>`，使目录创建错误进入 objstore 现有的 `anyhow` 错误链。
- `pub fn mkdirAll(base: &Path) -> Result<()>`：文件唯一公开函数。`pub` 是为了供同 crate 的 `local` 模块调用；名称保留 Go 版本的 `mkdirAll` 形式，crate 根已允许 `non_snake_case`。
- 条件编译项 `#[cfg(windows)]`：分别保护两个导入和函数，保证 Unix 构建不会引用 Windows 实现。

本文件没有常量、结构体、枚举、trait、全局变量或 `impl` 块。

## 执行流程

1. `pkg/objstore/local.rs` 在 Windows 构建中通过 `use crate::local_windows::mkdirAll` 选择本函数。
2. 调用者传入需要存在的根目录或父目录引用；本函数不改写路径。
3. `std::fs::create_dir_all(base)` 递归创建缺失的路径组件；目录已经存在时按标准库语义成功返回。
4. 成功值 `()` 原样成为 `Ok(())`；失败的 `std::io::Error` 经 `map_err(Into::into)` 转为 `anyhow::Error`。
5. 调用者决定是否添加业务上下文或继续后续文件操作。

在生产路径中有三个直接使用场景（均见 `pkg/objstore/local.rs`）：`LocalStorage::WriteFile` 首次临时文件写入因父目录不存在而失败时创建父目录并重试；`LocalStorage::CopyFrom` 在创建硬链接前确保目标父目录存在；`NewLocalStorage` 在根目录不存在时创建根目录。

## 数据与状态

输入 `base` 是只读借用的 `&Path`，输出只表达成功或错误，不返回新路径。函数没有内部缓存、静态可变数据或持久化对象；唯一可观察副作用是文件系统上可能新增一个或多个目录。

函数不负责对象名到本地路径的转换，也不验证路径是否位于 `LocalStorage` 根目录内。生产调用点传入的路径已经由 `LocalStorage::object_path`、临时文件路径或构造参数形成。目录权限没有本文件可配置的状态；这与 Windows 下忽略 Unix mode 参数的 Go 对照行为相符。

## 依赖与调用关系

上游关系：`pkg/objstore/lib.rs` 声明模块；`pkg/objstore/local.rs` 在 Windows 条件下导入函数，并由 `LocalStorage::WriteFile`、`LocalStorage::CopyFrom`、`NewLocalStorage` 调用。`pkg/objstore/objectio/lib.rs` 还在 `cfg(all(test, windows))` 下通过 `#[path = "../local_windows.rs"]` 编入同一实现，以支撑 objectio 的本地存储测试边界。

下游关系只有 `std::fs::create_dir_all` 和 `Into::into` 错误转换。`Path` 来自标准库，`Result` 来自 `anyhow`。没有网络、异步运行时、对象存储服务或 Go FFI 依赖。

RustCodeGraph 将 `pkg/objstore/local_windows.rs` 识别为含两个符号的已索引文件，并给出文件级使用者 `pkg/objstore/local.rs`；精确 `query mkdirAll --kind function` 同时定位 Windows/Unix 的 Rust/Go 四个对应函数。限定名的 `callers`/`callees` 查询未返回函数级边，因此上述三个调用点以 `local.rs` 的条件导入和源码调用为直接证据，不声称图中存在未观察到的边。

## 错误处理与边界

所有标准库建目录失败都会返回 `Err(anyhow::Error)`，本函数不吞错、不记录日志、不重试，也不添加路径文本。典型失败类别由平台和标准库决定，例如权限不足、路径组件被同名文件占用、无效路径或底层 I/O 故障。

业务上下文由上游补充：`LocalStorage::WriteFile` 在首次写临时文件失败后调用本函数；若建目录也失败，它保留首次写入错误为主错误，并在上下文中附带 mkdir 错误。`CopyFrom` 和 `NewLocalStorage` 则直接用 `?` 传播本函数错误。函数不保证随后写文件、硬链接或 rename 一定成功，也不执行失败回滚；并发进程在目录创建后删除目录等竞态属于后续操作需要处理的边界。

## 并发与资源生命周期

函数同步执行，没有 `async`、线程、任务、锁、通道、事务或显式文件句柄。路径借用只在调用期间有效；标准库调用返回后，本函数不保留资源。

多个线程或进程并发确保同一目录存在时，`create_dir_all` 的“目录已存在可成功”语义使常见竞争可收敛，但这不是跨多个文件操作的原子事务。尤其 `WriteFile` 的“检查父目录—创建—重试写入”和 `CopyFrom` 的“建目录—硬链接”仍可能受外部删除或替换路径影响。本文件不提供同步保证。

## 与 Go 版本的对应关系

直接对照是 `pkg/objstore/local_windows.go`：Go 的 `mkdirAll(base string) error` 调用 `os.MkdirAll(base, localDirPerm)`；Rust 的 `mkdirAll(base: &Path) -> anyhow::Result<()>` 调用 `std::fs::create_dir_all(base)`。两者都在 Windows 平台选择、递归创建目录、目录已存在时成功并向上返回错误。

签名差异来自语言惯例：Go 使用字符串和 `error`，Rust 使用借用的 `Path` 与 `anyhow::Result`。Go 传入 `localDirPerm`（在 `local.go` 为 `0o777`），但 Windows 不采用 Unix 权限模型；Rust 因此没有 mode 参数。与之相对，`local_unix.go` 会临时清零 umask，`local_unix.rs` 使用 `DirBuilderExt::mode(0o777)` 和 RAII guard 恢复 umask；这些 Unix 专属动作不应加入 Windows 文件。

相关行为测试是独立文件 `pkg/objstore/local_test.rs` 及 Go 对照 `pkg/objstore/local_test.go`：其中写入 `123/456/789.txt` 的场景验证不存在父目录可以被创建并成功写入。当前未发现直接调用 `local_windows::mkdirAll` 的专属 Rust 测试；现有覆盖通过 `LocalStorage` 间接发生，且只有在 Windows 测试目标上才会走本实现。

## 扩展指南

若要改变 Windows 目录创建行为，最小修改点是 `mkdirAll`，同时必须保持 `local.rs` 三个调用场景的共同契约：递归创建、已存在目录成功、错误可传播。若需求只涉及某一场景（例如写入重试的错误上下文、硬链接策略或根目录校验），应修改 `local.rs` 对应调用者，而不是把业务分支塞进平台适配函数。

测试应继续放在独立文件中，不要内嵌到本源文件。通用本地存储行为优先扩展 `pkg/objstore/local_test.rs`，并同步核对 `pkg/objstore/local_test.go` 的意图；Windows 专属语义可新建同目录独立测试文件并从 `lib.rs` 条件挂载。新增权限、规范化或重试逻辑前，应评估与 Go Windows 行为的兼容性、路径竞态、网络共享路径及错误上下文变化；不要照搬 Unix umask 逻辑。

## 验证依据

- 源文件：`pkg/objstore/local_windows.rs`，确认唯一函数、条件编译、标准库调用和错误转换。
- crate 与装配：`pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`、`pkg/objstore/objectio/lib.rs`，确认 crate 名称、`anyhow` 依赖、正式模块和 Windows 测试装配。
- 调用方：`pkg/objstore/local.rs`，确认条件导入及 `WriteFile`、`CopyFrom`、`NewLocalStorage` 三处生产调用。
- 平台对照：`pkg/objstore/local_windows.go`、`pkg/objstore/local_unix.go`、`pkg/objstore/local_unix.rs`、`pkg/objstore/local.go`，确认 Go Windows 语义、Unix 差异及 `localDirPerm = 0o777`。
- 测试：`pkg/objstore/local_test.rs`、`pkg/objstore/local_test.go`，确认嵌套父目录缺失时的写入行为；未发现本函数的 Windows 专属直接测试。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标文件已索引；`files --filter pkg/objstore/local_windows.rs` 显示目标文件含两个符号；`query mkdirAll --kind function --json` 定位四个 Rust/Go 平台对应实现；`node --file ...` 显示目标源码及文件级使用者 `pkg/objstore/local.rs`。限定名 `callers`/`callees` 未返回函数级结果，此限制已在依赖章节明确说明。
- 按任务约束未运行 Cargo；交付只执行 Markdown 结构验证，并人工复核文档没有把条件平台门面描述为完整存储实现。
