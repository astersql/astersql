# `cmd/pluginpkg/stubs.rs`

## 文件定位

[`cmd/pluginpkg/stubs.rs`](./stubs.rs) 是 `astersql-cmd-pluginpkg` crate 的系统边界适配层，而不是插件打包业务本体。`cmd/pluginpkg/lib.rs` 以 `pub mod stubs` 暴露本模块，并同时装配承载打包流程的 `pluginpkg.rs`；`bin_main.rs` 再把二进制入口转发给库入口。crate 在 `cmd/pluginpkg/Cargo.toml` 中同时声明库和 `astersql-cmd-pluginpkg` 二进制，依赖仅有 `serde`、`serde_json` 和 `toml`，因此这里直接以标准库实现参数、文件、进程和时钟边界，避免引入 TiDB 重型 crate。

生产调用链是 `bin_main::main -> lib::main -> pluginpkg::main`。其中 `pluginpkg::main` 调用 `args_from_env` 和 `try_parse_flags`，随后构造 `OsFs`、`ProdRunner`、`SystemClock`，把它们交给 `pluginpkg::run_with`。测试则以 `MemFs`、`ScriptedRunner`、`FixedClock` 和 `Capture` 替换这些真实边界。

## 核心职责

本文件承担五组职责：

1. 以 `Flags`、`parse_flags`、`try_parse_flags` 和 `args_from_env` 提供 `pluginpkg` 所需的 Go `flag`/`os.Args` 子集。
2. 以 `Fs`、`Runner`、`Clock` 三个窄 trait 把文件系统、`go build` 子进程和墙上时钟从 `pluginpkg::run_with` 中解耦。
3. 以 `OsFs`、`ProdRunner`、`SystemClock` 提供生产实现；其中只有 `ProdRunner::run` 会启动真实子进程。
4. 以 `MemFs`、`ScriptedRunner`、`FixedClock`、`Capture` 提供确定性的测试替身，并显式记录文件权限、删除轨迹和命令调用。
5. 以 `Error`、`Result`、`fatal_exit`、`flag_parse_exit` 和 `log_printf` 统一错误文本、退出分类及日志写入行为，使 Rust 测试能够观察 Go 的 `os.Exit`/`flag.ExitOnError` 语义。

这里的抽象范围刻意很小：例如 `Runner` 只覆盖 `pluginpkg` 发起 `go build` 所需的程序、参数、工作目录和附加环境变量，并不是通用进程管理框架；`Fs` 也只覆盖打包流程实际用到的路径与单文件操作。

## 主要符号

- `Error { msg, is_exit }` 与 `Result<T>`：统一 OS/IO/子进程错误。`Error::new` 表示普通失败，`Error::exit` 表示子进程非零退出；`Error::Error` 保留 Go 风格消息访问器，`From<io::Error>` 把标准 IO 错误归为非退出错误。
- `fatal_exit(msg) -> !`：非测试构建直接以状态码 1 终止进程；测试构建以带 `pluginpkg-exit:` 前缀的 panic 代替，使 `catch_unwind` 可以断言致命路径。私有 `flag_parse_exit` 同理，但生产退出码为 2，测试前缀为 `pluginpkg-flag-exit:`。
- `Capture`：基于 `Arc<Mutex<Vec<u8>>>` 的可克隆 `Write`，提供 `bytes`、宽松 UTF-8 的 `string` 和 `clear`，用于捕获日志与标准输出。
- `Flags { pkg_dir, out_dir, pgo_file, next_gen }`：对应 Go 文件注册的四个命令行选项。
- `parse_flags` / `try_parse_flags`：解析 `-name=value`、`--name=value` 和分离值形式；布尔选项接受 Go `strconv.ParseBool` 在此流程使用的大小写拼写；遇到 `--`、单个 `-` 或首个位置参数停止；帮助、未知项、缺值、非法布尔值和三个连字符语法走错误分支。`strip_flag` 只处理内联赋值。
- `CommandSpec`：保存一次命令的 `program`、`args`、`dir` 和附加 `env`，供测试检查。
- `Fs`：要求实现 `abs`、`read_to_string`、`write`、`remove`；默认 `base` 和 `join` 使用 `std::path`。
- `Clock`：唯一方法 `now_string`，为 manifest 的 `buildTime` 提供文本。
- `Runner`：唯一方法 `run`，描述同步执行命令并等待结果的边界。
- `MemFs`：以内层 `Rc<RefCell<...>>` 保存文件、权限、删除记录和故障注入；`put/get/get_string/removed_paths/mode_of` 是测试观测接口。
- `OsFs`：真实文件系统实现；`abs` 配合 `normalize_abs` 做不要求路径存在、也不解析符号链接的词法规范化；`write` 以创建、截断和指定 Unix mode 的方式写全量数据。
- `FixedClock` / `SystemClock`：分别返回固定文本和由 `chrono_like_now` 生成的当前 UTC 文本。`civil_from_days` 把 Unix epoch 日数换算为公历日期，进程内 `OnceLock<Instant>` 提供 `m=+...` 单调时长部分。
- `ScriptedRunner`：先记录 `CommandSpec`，再按 `fail` 注入可选错误；不启动进程。
- `ProdRunner`：通过 `std::process::Command` 同步执行程序，设置工作目录、继承 stdout/stderr、应用形如 `KEY=VALUE` 的附加环境；非零状态转为 `Error::exit`，并把 Rust 的 `exit status: N` 规范为 Go 的 `exit status N`。
- `log_printf`：向任意 `Write` 写入格式化文本和换行，不自动增加时间戳。

## 执行流程

参数侧，`pluginpkg::main` 先从 `args_from_env` 取得去掉 argv0 的参数，再调用 `try_parse_flags`。解析器逐项扫描：先识别停止标记和帮助，再依次处理三个字符串选项及 `next-gen`；无法识别的选项返回 Go 风格消息。库级 `parse_flags` 是便于测试/调用方使用的终止式包装，解析失败会进入 `flag_parse_exit`。

生产打包侧，`pluginpkg::main` 构造三个零大小真实实现并调用 `run_with`。`run_with` 使用 `Fs::abs/read_to_string/write/remove/base/join` 完成路径归一化、manifest 读取、临时 `.gen.go` 创建/写入及清理；使用 `Clock::now_string` 注入构建时间；使用 `Runner::run("go", ...)` 执行一次 `go build`。因此本文件位于所有系统副作用的最下游，业务顺序和 manifest/template 处理仍由 `pluginpkg.rs` 决定。

测试侧，`MemFs::put` 预置 `manifest.toml`，`FixedClock` 固定输出，`ScriptedRunner` 记录命令或注入失败，两个 `Capture` 分别承接 log/stdout。成功后测试读取命令和删除轨迹；失败后通过 panic 捕获退出，并读取仍存在的生成文件、权限和日志。

## 数据与状态

`Flags` 和 `CommandSpec` 是按值快照，避免调用期间借用进程参数。`Error` 也可克隆，便于故障注入在多个替身对象间传递。

`Capture` 的缓冲区使用 `Arc<Mutex<_>>`：克隆体共享同一输出，且写入和读取由互斥锁串行化。相对地，`MemFs` 与 `ScriptedRunner` 使用 `Rc<RefCell<_>>`，共享的是单线程测试状态：文件字节、mode、删除列表、按路径的 `abs` 错误、全局删除错误以及命令序列。它们不承诺跨线程共享，也不模拟目录、符号链接、真实权限检查或并发文件系统竞争。

`SystemClock` 的 `OnceLock<Instant>` 在首次调用时初始化，之后输出真实墙钟的 UTC 日期/纳秒以及自首次调用起的单调时长。它不保存业务状态；`FixedClock` 则始终克隆 `value`。`OsFs` 和 `ProdRunner` 自身是无状态零大小类型，状态存在于宿主文件系统和子进程中。

## 依赖与调用关系

上游直接关系由源码和 RustCodeGraph 查询共同确认：

- `cmd/pluginpkg/lib.rs` 公开 `stubs` 模块；`cmd/pluginpkg/pluginpkg.rs` 导入 `Clock`、`Flags`、`Fs`、`OsFs`、`ProdRunner`、`Runner`、`SystemClock`、`fatal_exit` 和 `log_printf`。
- `pluginpkg::main` 调用 `args_from_env`、`try_parse_flags`，并装配三个生产实现；`pluginpkg::run_with` 调用三个 trait 的方法完成全部外部副作用。
- `pluginpkg::usage` 和多个打包失败分支调用 `fatal_exit`；业务日志统一经 `log_printf` 写入。
- `cmd/pluginpkg/parity_test.rs` 直接使用 `Capture`、`Error`、`FixedClock`、`Flags`、`MemFs`、`ScriptedRunner`，并单独测试 `ProdRunner` 与 `SystemClock`；`cmd/pluginpkg/pluginpkg_test.rs` 使用这些替身验证模板失败和清理时序。

下游只依赖 Rust 标准库：`env`、`fs`、`path`、`io::Write`、`process::Command`、`SystemTime/Instant/OnceLock` 及同步/内部可变性容器。`cmd/pluginpkg/Cargo.toml` 的三个第三方依赖均由 `pluginpkg.rs` 的 manifest/template/JSON 逻辑使用，本文件没有直接依赖它们。

## 错误处理与边界

普通 IO 失败通过 `Error` 返回给 `run_with`，是否升级为退出由业务层决定。`ProdRunner` 启动失败经 `From<io::Error>` 返回，命令非零退出则带 `is_exit = true`；`ScriptedRunner` 即使被配置为失败，也会先记录调用，保证失败参数可检查。

退出行为有条件编译差异：生产中的 `fatal_exit`/`flag_parse_exit` 不展开栈，而测试中使用 panic 以避免终止测试进程。这是测试协议，不意味着生产调用者可以恢复。`Capture::bytes/clear/write` 对 poisoned mutex 使用 `unwrap`，锁中发生 panic 后后续访问也会 panic。

文件边界方面，`OsFs::abs` 只做词法 `.`/`..` 消解，不检查目标存在且不解析符号链接；`normalize_abs` 遇到超过根部的父目录只执行 `pop`。`OsFs::write` 依赖 Unix 的 `OpenOptionsExt`，因此本实现具有 Unix 平台约束；mode 仅在创建新文件时由操作系统应用，已有文件的权限不会被强制重设。`MemFs::read_to_string` 和 `get_string` 使用宽松 UTF-8 替换，而真实 `fs::read_to_string` 会拒绝非法 UTF-8，这是替身没有完全模拟的边界。

命令环境只处理包含首个 `=` 的字符串；不含 `=` 的附加项被忽略。`ProdRunner` 继承父环境和标准输出/错误，但不提供取消、超时、stdin 管理或异步等待。`log_printf` 忽略写错误；这是对 Go 日志调用在当前命令中的最低限度适配。

## 并发与资源生命周期

生产调用是同步的：`ProdRunner::run` 在 `cmd.status()` 返回前阻塞，且输出直接继承父进程句柄。本文件不创建线程、任务或通道，也不持有长期文件句柄；`OsFs::write` 在函数返回时关闭局部文件对象。

`Capture` 可跨线程共享，因为状态位于 `Arc<Mutex<_>>`；每次写入在单次加锁内追加完整传入切片。`MemFs` 与 `ScriptedRunner` 的 `Rc<RefCell<_>>` 明确限制在单线程，运行时借用冲突会 panic。扩展测试时不应把它们直接移动到并发 worker；如确有并发契约，应另行设计线程安全替身并增加独立测试。

临时 `.gen.go` 的生命周期由 `pluginpkg::run_with` 控制，而非 `Fs` 自动管理：成功输出 manifest 后调用 `remove`；模板或 `go build` 的致命退出会跳过清理并保留文件；成功路径删除失败只记录日志。`parity_test.rs::contract_resource_cleanup` 与 `pluginpkg_test.rs::deferred_remove_runs_after_manifest_output` 明确锁定了这一顺序。

## 与 Go 版本的对应关系

直接对照文件是 `cmd/pluginpkg/pluginpkg.go`。Rust 将 Go `main` 中硬编码的标准库调用拆成可注入边界，但保持以下语义：

- `Flags` 与 Go 的 `pkgDir/outDir/pgoFile/nextGen` 一一对应；解析器复刻当前命令使用到的 `flag` 行为，而非完整实现 Go `flag` 包。
- `Fs::abs/base/join/read_to_string/write/remove` 分别对应 `filepath.Abs/Base/Join`、TOML 文件读取、`os.OpenFile` 写入和 `os.Remove`。真实写入保留 `O_CREATE|O_TRUNC` 与 `0700` 的核心效果。
- `Clock::now_string` 对应 `time.Now().String()`；`FixedClock` 只服务确定性测试，`SystemClock` 生成近似的 UTC 文本和 `m=+` 部分，不承诺复制 Go `time.Time.String` 的所有时区/单调时钟细节。
- `Runner::run` 对应 `exec.CommandContext(...).Run()` 的当前实际用法：工作目录、继承 stdout/stderr、继承父环境并覆盖 `GO111MODULE=on`。Rust 接口没有携带 `context.Context`，所以没有 Go context 的取消能力。
- `fatal_exit` 对应 `os.Exit(1)`；失败时不执行清理的可观察结果由上层流程和测试保留。测试构建中的 panic 是为了可验证性，不是 Go 生产行为本身。
- `Capture`、`MemFs`、`ScriptedRunner`、`FixedClock` 在 Go 源文件中没有同名生产结构，它们是 Rust 为依赖注入和契约测试新增的设施。

同目录未发现 Go `_test.go`。Go 行为的直接来源是 `pluginpkg.go`，Rust 回归证据位于独立文件 `cmd/pluginpkg/parity_test.rs` 和 `cmd/pluginpkg/pluginpkg_test.rs`。

## 扩展指南

增加命令行选项时，应同步修改 `Flags`、`try_parse_flags`、`pluginpkg.rs` 的 usage/状态装配和 Go `init/main` 对照逻辑，并在 `parity_test.rs` 增加分离值、内联值、错误值、首个位置参数停止等契约用例。不要把完整参数框架无依据地扩进本文件。

增加新的外部副作用时，优先把最小能力加入 `Fs`、`Runner` 或新的窄 trait，并同时实现生产版与测试替身；需要记录的参数应扩展 `CommandSpec` 或对应状态快照。任何 `Fs` 方法变化都必须复核临时文件的创建、部分写入、权限和清理顺序；任何 `Runner` 变化都必须复核继承环境、stdout/stderr 和非零退出文本。

若要求并发执行构建，不能直接复用 `MemFs`/`ScriptedRunner` 的 `Rc<RefCell<_>>`；需要先定义线程安全、顺序可观察的替身和独立测试。若要求跨平台支持，应优先处理 `std::os::unix::fs::OpenOptionsExt` 与 mode 语义，并明确 Windows 的替代契约。

测试必须继续放在独立的 `parity_test.rs` 或 `pluginpkg_test.rs`，不要嵌入 `stubs.rs`。重点同步覆盖：CLI 解析错误、`ProdRunner` 非零状态、时钟格式、非法 UTF-8/路径边界（若改变）、写入与删除失败、子进程环境，以及成功/失败路径的资源生命周期。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter cmd/pluginpkg` 确认 crate 内 `bin_main.rs`、`lib.rs`、`pluginpkg.rs`、`stubs.rs` 和两个独立测试文件。
- RustCodeGraph `node --file cmd/pluginpkg/stubs.rs`：读取 1–654 行，核对全部结构体、trait、函数、impl、`#[cfg(test)]`/`#[cfg(not(test))]` 分支及真实/替身实现。
- RustCodeGraph 对 `cmd/pluginpkg/pluginpkg.rs` 的文件节点与定向 explore：确认 `main` 装配 `OsFs/ProdRunner/SystemClock`，`run_with` 消费 `Fs/Runner/Clock`，并确认 `fatal_exit`、`log_printf` 的上游调用位置。
- RustCodeGraph 文件节点 `cmd/pluginpkg/lib.rs`、`bin_main.rs`：确认模块公开方式与二进制入口链。
- RustCodeGraph 文件节点 `cmd/pluginpkg/parity_test.rs`、`pluginpkg_test.rs`：确认参数形式、命令参数/环境、真实非零退出格式、当前日期、文件 mode、部分生成物、成功删除、失败保留和删除失败仅记日志等断言。
- `cmd/pluginpkg/Cargo.toml`：确认 crate 名、库/二进制路径、移植元数据和轻量依赖边界。
- `cmd/pluginpkg/pluginpkg.go`：确认 Go `flag` 注册、路径绝对化、文件模式、`time.Now().String()`、`exec.CommandContext`、环境、输出继承、退出与 `defer os.Remove` 语义；`rg` 确认同目录没有 Go 独立测试引用。
- 本说明只记录静态源码、索引调用关系与现有独立测试所证明的事实；按任务约束未运行 Cargo，也未声称替身覆盖完整 Go 标准库语义。
