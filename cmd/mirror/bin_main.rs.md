# `cmd/mirror/bin_main.rs`

## 文件定位

`cmd/mirror/bin_main.rs` 是 `astersql-cmd-mirror` 包的可执行文件入口，不是 mirror 业务实现。`cmd/mirror/Cargo.toml` 同时声明了以 `lib.rs` 为根的库目标和以本文件为根的 `astersql-cmd-mirror` 二进制目标；Cargo 将包名中的连字符转成下划线，因此本文件通过 `astersql_cmd_mirror` 引用同包的库 crate。根 `Cargo.toml` 将 `cmd/mirror` 纳入 workspace，包元数据则把它标记为 Go 包 `cmd/mirror` 的 binary 移植。

## 核心职责

本文件只承担进程入口适配：操作系统调用私有函数 `fn main()` 后，它立即调用 `astersql_cmd_mirror::main()`。这一层不解析参数、不创建运行时、不实例化外部依赖，也不处理错误；实际装配位于 `cmd/mirror/lib.rs::main`，命令流程位于 `cmd/mirror/mirror.rs::main` 和 `run_main_with`。保持这种单跳转发可以让可执行文件与可注入、可测试的库实现共用行为。

## 主要符号

- `fn main()`：本文件唯一的符号，为私有、无参、无显式返回值的 Rust 二进制入口。它没有条件编译属性、泛型、trait 或本地状态。
- `astersql_cmd_mirror::main()`：唯一的下游调用。它在 `cmd/mirror/lib.rs` 中是公开库入口，再单跳调用 `mirror::main()`。
- `cmd/mirror/mirror.rs::main()`：不在本文件定义，但是这条入口链的真实进程装配点；它读取环境参数，构造 `ProdBazel`/`ProdRunner`/`OsFs` 和标准输出流，再调用 `run_main_with`。

## 执行流程

1. 运行时进入 `cmd/mirror/bin_main.rs::main`。
2. 该函数无分支地调用 `astersql_cmd_mirror::main()`，即 `cmd/mirror/lib.rs::main`。
3. 库入口调用 `cmd/mirror/mirror.rs::main`；后者收集 argv，建立生产环境适配器和 stdout/stderr，并调用 `run_main_with`。
4. `run_main_with` 初始化与解析标志，对已废弃的 `--mirror`/`--upload` 向 stderr 发出提示，然后通过 `mirror_with` 执行临时目录、Go 模块列举/下载和 `DEPS.bzl` 输出流程。
5. 参数解析的普通 `Err` 由 `mirror.rs::main` 写入 stderr 并以状态码 2 退出；`mirror_with` 的业务错误由 `run_main_with` 按 Go 对齐语义 panic。本文件不截获其中任何结果。

## 数据与状态

本文件没有模块级常量、静态变量、结构体、枚举或可变状态，也不持有 argv、IO 句柄或临时目录。所有进程数据都在下游产生：`mirror.rs::main` 创建生产适配器和输出对象，`run_main_with` 管理标志状态，`mirror_with` 管理模块清单、下载信息和临时目录守卫。因此对本文件的状态不变式可简化为：入口层不复制也不改写下游数据。

## 依赖与调用关系

上游只有操作系统/Cargo 生成的启动边界；Rust 源码中没有应用内调用者。下游链为 `bin_main.rs::main` → `lib.rs::main` → `mirror.rs::main` → `mirror.rs::run_main_with` → `mirror_with`。RustCodeGraph 能显示 `lib.rs::main` 的源码及 `mirror.rs::main` 到 `run_main_with` 的 call edge，但对 `bin_main.rs` 中以 crate 名调用库入口的跨目标边返回空集；这一跳由本文件的直接源码和 `Cargo.toml` 的 lib/bin 声明交叉核验。

`bin_main.rs` 本身不直接依赖 `serde` 或 `serde_json`；这些是同包库实现使用的依赖。Go/Bazel 的 `cmd/mirror/BUILD.bazel` 只声明 `mirror.go` 的 `go_library` 和 `go_binary`，不是 Rust 二进制的构建定义。

## 错误处理与边界

`fn main()` 不返回 `Result`，也不安装 panic hook 或进程退出映射，所以库入口的 panic、显式 `process::exit(2)` 和正常返回会原样穿过此壳层。边界风险在于，若在这里新增参数处理、错误转换或另一套实例化逻辑，会绕过 `run_main_with` 的对齐测试，并可能使 Rust 二进制与 Go `main` 产生不同的 stderr、panic 文本或退出码。

现有 `cmd/mirror/parity_test.rs::contract_error_paths` 验证下游子进程退出时的 `subprocess exited with stderr` panic 文本，也验证已废弃标志提示；它调用的是 `run_main_with`，不是本二进制入口，因而不覆盖 Cargo/OS 启动边界本身。

## 并发与资源生命周期

本文件不创建线程、async task、通道、锁或事务，也没有自己的资源清理路径。资源语义位于下游：`mirror_with` 建立 `TmpDirGuard`，使临时目录在成功或业务失败路径上均被清理，清理失败按 Go `defer os.RemoveAll` 对齐语义 panic。`cmd/mirror/parity_test.rs::contract_resource_cleanup` 和 `cmd/mirror/mirror_test.rs::cleanup_panic_uses_go_error_text_instead_of_rust_debug_fields` 分别覆盖成功/失败后清理及清理 panic 文本。壳层必须继续单跳转发，才不会提前结束或绕过该守卫。

## 与 Go 版本的对应关系

Go 版本不分离二进制壳和可测库入口：`cmd/mirror/mirror.go::main` 直接执行 `flag.Parse`、废弃标志警告、`mirror()` 调用和 panic 映射。Rust 版本将同样的逻辑放到 `mirror.rs::main`/`run_main_with`，而本文件只保留 Rust 可执行文件所需的 `fn main()`。因此语义对应是整条 Rust 入口链对 Go `main`，不是本文件七行代码单独重现 Go 全部逻辑。

`cmd/mirror` 目录没有 Go `*_test.go` 文件；对齐证据来自独立 Rust 测试 `parity_test.rs` 和 `mirror_test.rs`。这些测试验证的是下游可注入入口与 Go 行为，尚无直接启动 `astersql-cmd-mirror` 二进制的集成测试。

## 扩展指南

- 修改命令行参数、标准流或退出语义时，应优先修改 `cmd/mirror/mirror.rs::run_main_with` 及其上层 `mirror.rs::main`，并在独立的 `parity_test.rs`/`mirror_test.rs` 增补测试；不应把业务逻辑或测试内嵌到 `bin_main.rs`。
- 只有在必须改变进程启动边界时才修改本文件，例如增加只能在二进制目标生效的编译期入口属性。此类改动应新增独立的二进制启动集成测试，因为当前测试只覆盖库内入口。
- 若重命名包、lib 目标或 bin 路径，需要同步 `cmd/mirror/Cargo.toml` 与本文件中的 crate 引用，并核对根 workspace 成员配置。
- 兼容性风险主要是 stderr/panic/退出码与 Go 工具不一致；性能风险在当前壳层近乎为零，但在入口层增加重复初始化会带来启动开销和行为分叉。

## 验证依据

- 目标源码：`cmd/mirror/bin_main.rs` 第 5–6 行，证明唯一符号与单跳 crate 调用。
- crate 边界：`cmd/mirror/Cargo.toml` 的 `[lib]`、`[[bin]]` 和 `[package.metadata.porting]`；根 `Cargo.toml` 的 workspace 成员 `cmd/mirror`。
- 直接实现：`cmd/mirror/lib.rs::main`、`cmd/mirror/mirror.rs::main`、`run_main_with`、`mirror_with`。
- Go 对照：`cmd/mirror/mirror.go::mirror` 和 `cmd/mirror/mirror.go::main`；Bazel 对照目标见 `cmd/mirror/BUILD.bazel`。
- 独立测试：`cmd/mirror/parity_test.rs::contract_error_paths`、`contract_resource_cleanup`，以及 `cmd/mirror/mirror_test.rs::cleanup_panic_uses_go_error_text_instead_of_rust_debug_fields`。目录内未找到 Go `*_test.go` 或直接启动 `bin_main.rs` 的 Rust 测试。
- RustCodeGraph 查询：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter cmd/mirror`、`explore "bin_main cmd/mirror"`、`node --file cmd/mirror/bin_main.rs`、`node --file cmd/mirror/lib.rs`、`node --file cmd/mirror/mirror.rs`、以及对 `lib.rs::main`、`mirror.rs::main`、`run_main_with` 的 callers/callees 查询用于核对符号与调用链。图对 `bin_main.rs::main` 的 callers/callees 均返回空集，跨目标第一跳因此以源码与 Cargo 声明为准。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验应确认本文档存在且恰有十一个规定二级标题。
