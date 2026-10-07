# `cmd/mirror/stubs.rs`

## 文件定位

`cmd/mirror/stubs.rs` 是 `astersql-cmd-mirror` crate 的外部系统边界层，由 [`cmd/mirror/lib.rs`](lib.rs) 以 `pub mod stubs` 装配。二进制入口 [`cmd/mirror/bin_main.rs`](bin_main.rs) 经库入口进入 [`cmd/mirror/mirror.rs`](mirror.rs)；后者的 `run_main_with`、`mirror_with`、`create_tmp_dir`、`list_all_modules`、`download_zips` 和 `dump_new_deps_bzl` 通过本文件提供的类型和 trait 访问命令行、Bazel runfiles、子进程及文件系统。

它虽名为 `stubs`，但并非未接线占位文件：`ProdBazel`、`ProdRunner`、`OsFs` 是生产实现，`ScriptedEnv`、`MemFs`、`Capture` 才是独立测试使用的可控替身。crate 的 [`cmd/mirror/Cargo.toml`](Cargo.toml) 将该包声明为 `kind = "binary"`，同时提供库和 `astersql-cmd-mirror` 二进制；依赖仅有 `serde` 与 `serde_json`，本文件自身只使用标准库。

## 核心职责

本文件将 mirror 主流程依赖的外部行为压缩为三组可注入接口，并补齐入口兼容所需的值类型：

- `Bazel` 抽象临时目录和 runfile 定位，隔离 Bazel/环境变量差异。
- `Runner` 抽象 `go` 子进程执行，使主流程只处理参数、工作目录、环境和结果。
- `Fs` 抽象目录创建、文件复制、递归删除和存在性检查。
- `Error`/`Result` 保留普通错误、进程退出、文件不存在三种上层需要区分的形状。
- `Flags`、`parse_flags_checked` 和 `args_from_env` 模拟 Go `flag` 的相关入口语义。
- `Capture`、`MemFs`、`ScriptedEnv` 记录输出和副作用，用于不接触真实磁盘、Bazel 或网络的契约测试。
- `ProdBazel`、`ProdRunner`、`OsFs` 将相同接口绑定到真实进程环境。

因此本文件的稳定契约不是业务数据转换，而是“主流程能观察哪些外部结果、如何区分失败、测试如何重放这些结果”。

## 主要符号

- `Error { msg, stderr, is_exit, is_not_exist }`：统一边界错误。`new` 创建普通错误；`not_exist` 标记可忽略的缺失；`exit` 保存子进程 stderr；`From<io::Error>` 将 `NotFound` 映射到 `is_not_exist`。`Display`、`std::error::Error` 和 Go 风格 `Error()` 暴露消息文本。`ExitError` 仅保存 `Stderr`，是 Go `*exec.ExitError` 的形状兼容类型；当前主流程实际使用的是 `Error::exit`。
- `Flags`、`parse_flags`、`parse_flags_checked`、私有 `parse_go_bool`：解析 `mirror`/`upload` 两个废弃布尔参数。checked 版本返回 `Result`，非 checked 版本按当前实现把错误转为 panic。
- `CommandSpec`：记录一次 `go` 调用的可观察字段：可执行文件、参数、工作目录和环境。
- `Capture`：以 `Arc<Mutex<Vec<u8>>>` 实现可克隆的 `Write`；`bytes`/`string` 读取快照，`clear` 清空。
- `Bazel`：公开 `NewTmpDir`、`Runfile`、`RunfilesPath`。方法保留 Go 风格名称，crate 根在 `lib.rs` 中允许 `non_snake_case`。
- `Runner::output`：执行命令并返回 stdout；失败通过统一 `Error` 返回。
- `Fs`：公开 `mkdir_all`、`copy_file`、`remove_all`、`stat`。
- `MemFs`：用 `Rc<RefCell<...>>` 保存文件、目录创建记录、删除记录和可注入删除错误；`put`、`get`、`removed_paths` 是测试观察入口。
- `ScriptedEnv`：同时实现 `Bazel` 和 `Runner`；保存 runfile 映射、固定 JSON 输出、调用记录，以及 list/download/建临时目录故障注入点。
- `OsFs`、`ProdBazel`、`ProdRunner`：生产实现，分别绑定 `std::fs`、runfiles/临时目录环境和 `std::process::Command`。
- `resolve_runfiles_path`：从 `RUNFILES_DIR`（优先）或 `TEST_SRCDIR` 与 `TEST_WORKSPACE` 组合出 workspace 根。
- `read_file`：读取完整文件为字节；源码注明它只服务于 `OsFs` 真实复制路径的测试用途，当前 mirror 主链没有调用它。

文件没有条件编译项；测试模块是在 `lib.rs` 中用 `#[cfg(test)]` 独立挂接，符合测试与生产源文件分离要求。

## 执行流程

生产入口的边界调用链如下：

1. `bin_main.rs::main` 调用库 `main`，再进入 `mirror.rs::main`。
2. `mirror.rs::main` 用 `args_from_env` 跳过 argv0，构造 `ProdBazel`、`ProdRunner`、`OsFs`，调用 `run_main_with`。
3. `run_main_with` 调用 `parse_flags_checked`。解析按参数顺序进行：遇到 `--`、单独的 `-` 或首个非 flag 参数立即停止；只接受 `mirror`/`upload`，无显式值时为 `true`，显式值由 `parse_go_bool` 按 Go 支持的大小写集合解释。
4. `mirror_with` 调用 `create_tmp_dir`；其中 `ProdBazel::NewTmpDir` 创建唯一目录，`ProdBazel::Runfile` 定位 `go.mod`/`go.sum`，`OsFs` 创建 parser 子目录并复制文件。
5. `list_all_modules` 和 `download_zips` 通过 `ProdBazel::Runfile("bin/go")` 找到 Go，随后由 `ProdRunner::output` 在临时目录运行命令。额外环境中的 `GOSUMDB=sum.golang.org` 覆盖继承环境中的同名键。
6. `dump_new_deps_bzl` 通过 `Bazel::RunfilesPath` 与 `Fs::stat` 判断补丁文件是否存在，并把生成结果写到 stdout。
7. `mirror.rs::TmpDirGuard` 在成功、普通错误或 panic 展开时调用 `Fs::remove_all`；删除失败自身会 panic，对应 Go `defer os.RemoveAll` 后 panic 的可见行为。

测试路径把第 4～6 步的生产实现替换为同一个 `ScriptedEnv` 和一个 `MemFs`：前者记录命令并返回预置 JSON，后者记录文件与清理副作用，`Capture` 收集 stdout/stderr。主流程代码不需要为测试另写分支。

## 数据与状态

`Error` 的四个字段共同形成判别状态：普通失败的两个标志均为 false；文件不存在仅 `is_not_exist` 为 true；子进程退出仅 `is_exit` 为 true，并将 stderr 保留为原始字节。调用方据此在 `dump_patch_args_for_repo` 忽略缺失补丁，或在 `run_main_with` 把退出 stderr 包装进 panic。

`MemFs` 的所有可变状态位于共享的 `Rc<RefCell<_>>` 中，克隆对象会看到同一份文件映射、目录/删除记录及 `fail_remove`。其 `remove_all` 用字符串前缀删除文件条目，目的是模拟测试所需的目录清理可见效果，并不声称实现完整路径语义。`stat` 只检查 `files` 映射，不检查 `dirs`。

`ScriptedEnv` 的 `tmp_counter` 和 `commands` 也由 `Rc<RefCell<_>>` 共享。`output` 总是先记录 `CommandSpec`，再以首参数区分 `go list` 与 `go mod ...`，分别返回预置结果或故障；其他参数形状返回 `unexpected go args`。默认临时目录前缀为 `/tmp/gomirror`、runfiles 根为 `/runfiles`。

`Capture` 用 `Arc<Mutex<Vec<u8>>>`，与上述 `Rc<RefCell<_>>` 不同，允许克隆的 writer 在多线程类型层面共享缓冲区。生产临时目录序号则是进程级 `AtomicU64`，搭配 PID 与原子序号构造候选名。

## 依赖与调用关系

上游直接依赖主要来自 [`cmd/mirror/mirror.rs`](mirror.rs)：

- `run_main_with` → `parse_flags_checked`；`main` → `args_from_env`。
- `create_tmp_dir` → `Bazel::{NewTmpDir, Runfile}` 与 `Fs::{mkdir_all, copy_file}`。
- `list_all_modules`、`download_zips` → `Bazel::Runfile`、`Runner::output`。
- `dump_patch_args_for_repo` → `Bazel::RunfilesPath`、`Fs::stat`，并依赖 `Error::is_not_exist` 分支。
- `mirror_with` 的清理守卫 → `Fs::remove_all`。
- `run_main_with` 依赖 `Error::{is_exit, stderr}` 决定 panic 文本。

下游实现依赖全部来自标准库：`std::env` 读取参数和 runfiles/临时目录变量；`std::fs`、`std::io` 完成真实文件操作；`std::process::Command` 执行 Go；`PathBuf` 拼接路径；`AtomicU64` 生成临时目录序号。`stubs.rs` 不依赖 `serde`，JSON 解析留在 `mirror.rs`。

RustCodeGraph 对精确符号给出的关键调用证据包括：`parse_flags_checked` 被 `parse_flags`、`mirror.rs::run_main_with` 和 `parity_test.rs::go_flag_parser_stops_and_rejects_invalid_input` 调用；`resolve_runfiles_path` 被 `ProdBazel::{Runfile, RunfilesPath}` 与 `runfiles_path_includes_the_main_workspace` 调用；`Error::not_exist` 被 `MemFs::{copy_file, stat}` 和 `ScriptedEnv::Runfile` 使用；`Error::exit` 被 `ProdRunner::output` 使用。

## 错误处理与边界

- flag 未定义或布尔值非法时，`parse_flags_checked` 返回带 Go 风格文本的普通 `Error`；遇到位置参数后不会再解析后续 flag。
- `From<io::Error>` 只特判 `ErrorKind::NotFound`。`OsFs::remove_all` 进一步把删除不存在目录视为成功，保持幂等；其他 I/O 错误保留文本并向上传播。
- `ProdBazel::Runfile` 先检查当前工作目录相对路径，再尝试 runfiles 环境；runfiles 环境不完整或候选不存在最终都返回 `is_not_exist`。`RunfilesPath` 则要求根目录和 workspace 都存在，缺项为普通错误。
- `ProdBazel::NewTmpDir` 最多尝试 10,000 个 PID+序号候选；名称冲突继续，其他创建错误立即返回，耗尽时报带基目录的普通错误。
- `ProdRunner::output` 的 spawn/I/O 失败经 `From<io::Error>` 转换；非零退出使用 `Error::exit(out.stderr)`，不保留退出码或 stdout。该信息边界必须与 `run_main_with` 的 stderr panic 契约一起考虑。
- `Capture::bytes`/`write` 直接 `unwrap` mutex；锁中毒会 panic。`String::from_utf8_lossy` 使非 UTF-8 输出可观察但可能以替换字符呈现。
- `MemFs` 的 `RefCell` 借用冲突会 panic，且其路径判断是测试模型，不应当替代真实文件系统安全检查。
- `ScriptedEnv::output` 只识别首参数 `list` 或 `mod`，用于固定当前 mirror 命令协议；扩展新子命令时必须同步该分派和测试。

## 并发与资源生命周期

生产实现没有后台任务、通道、异步运行时或显式线程。`ProdRunner::output` 同步等待子进程结束并一次性收集 stdout/stderr；大输出会整体驻留内存。

`ProdBazel::NewTmpDir` 的 `AtomicU64` 使用 `Ordering::Relaxed`：这里只要求同一进程内取得不同序号，不承载跨线程同步其他状态；真正的排他性由 `fs::create_dir` 保证，遇到已存在即重试。目录的最终生命周期由 `mirror.rs::TmpDirGuard` 管理，而不是由 `ProdBazel` 自行回收。

`Capture` 可跨线程共享底层缓存（`Arc<Mutex<_>>`），每次写入和读取快照都持锁；它不保证跨多次 `write` 调用的更高层消息原子性。相反，`MemFs` 和 `ScriptedEnv` 使用 `Rc<RefCell<_>>`，只适合单线程测试，不能发送到线程间；当前独立测试均在单测试调用链内使用它们。

真实文件复制依赖 RAII：`OsFs::copy_file` 中输入、输出 `File` 离开作用域时关闭。子进程句柄由 `Command::output` 管理，函数返回时已完成等待和输出收集。

## 与 Go 版本的对应关系

Go 对照是 [`cmd/mirror/mirror.go`](mirror.go)。Go 文件直接调用 `bazel.NewTmpDir`/`Runfile`/`RunfilesPath`、`os`、`exec.Command(...).Output()` 和全局 stdout；Rust 把这些调用提炼为三个 trait，以便同一业务流程接受生产实现或测试替身。

具体对应关系如下：

- Go `flag.BoolVar`/`flag.Parse` ↔ `Flags`/`parse_flags_checked`。Rust仅实现本程序注册的两个 bool flag 及当前测试所需的停止与报错语义，不是通用 Go flag 包。
- Go `*exec.ExitError.Stderr` ↔ `Error { is_exit, stderr }`；`ExitError` 结构保留相同外形，但当前链路不直接构造它。
- Go `os.IsNotExist` ↔ `Error::is_not_exist`；缺失补丁是正常分支，其他 stat 错误仍返回。
- Go `os.MkdirAll`/`io.Copy`/`os.RemoveAll`/`os.Stat` ↔ `Fs` 与 `OsFs`。
- Go rules_go Bazel helper ↔ `ProdBazel`。Rust 生产实现以环境变量和当前目录解析 runfiles，并自行创建临时目录；这是行为适配而非复用 Go 库。
- Go `exec.Command` 继承 `os.Environ` 再追加 `GOSUMDB` ↔ `mirror.rs::command_env_with_gosumdb` 构造环境字符串，`ProdRunner` 按顺序调用 `cmd.env`，从而让附加值覆盖同名继承值。
- Go 测试目录中没有 `*_test.go`；当前直接契约证据来自独立 Rust 测试 [`cmd/mirror/parity_test.rs`](parity_test.rs) 与 [`cmd/mirror/mirror_test.rs`](mirror_test.rs)，以及 Go 生产实现本身。

不可把测试替身特性误写成 Go 生产语义：例如 `MemFs::remove_all` 的前缀删除和 `ScriptedEnv` 的命令首参数分派只是可观察测试模型。

## 扩展指南

- 新增外部系统动作时，先判断它属于 Bazel、进程还是文件系统边界；优先扩展相应 trait，并同时实现生产端与脚本/内存端。若是独立职责，新增窄 trait 比把无关状态塞入现有接口更安全。
- 扩展 CLI 时修改 `Flags`、`parse_flags_checked`/`parse_go_bool` 和 `mirror.rs::run_main_with`，并在 `parity_test.rs` 增加停止规则、合法值、非法值与提示文本测试。不要把测试内嵌回 `stubs.rs`。
- 扩展 `go` 命令形状时同步 `ScriptedEnv::output` 和 `CommandSpec` 断言，确保参数、目录和环境覆盖顺序仍可验证；注意真实执行会把输出整体载入内存。
- 改动 runfiles 查找时同步 `resolve_runfiles_path`、`ProdBazel::{Runfile, RunfilesPath}` 及 `runfiles_path_includes_the_main_workspace`；兼容风险集中在当前目录优先级、`RUNFILES_DIR` 对 `TEST_SRCDIR` 的优先级和 workspace 拼接。
- 改动错误模型时保持 `is_not_exist` 与 `is_exit` 的可区分性，并同步 `contract_error_paths`、补丁缺失场景和 `mirror_test.rs::cleanup_panic_uses_go_error_text_instead_of_rust_debug_fields`。否则可能把正常缺失升级为失败，或丢失子进程 stderr。
- 改动资源实现时同步 `bazel_new_tmp_dir_creates_unique_directories`、`remove_all_is_idempotent_like_go` 和 `contract_resource_cleanup`。若将测试替身用于并发场景，必须先把 `Rc<RefCell<_>>` 换成线程安全状态并明确锁粒度与死锁风险。
- 若新增 Rust 生产源文件或测试，继续保持生产逻辑和独立测试文件分离；本文件已有 AsterSQL 处理标记，不应删除文件顶部版权行。

性能风险主要是 `Command::output`、`Capture` 与 `read_file` 都整块缓存数据；兼容风险主要是 Go flag 文本、runfiles 顺序、环境覆盖和错误分类；正确性风险主要是测试替身与真实文件系统语义发生漂移。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：当前索引包含 7,032 个 Rust 文件；`files --filter cmd/mirror` 确认 `stubs.rs`、入口、主流程及两个独立测试均已索引。
- RustCodeGraph `node --file cmd/mirror/stubs.rs`：逐段核对本文件 662 行、公开/私有符号、trait 实现和无条件编译项。
- RustCodeGraph `explore` 与 callers/callees 查询：核对 `parse_flags_checked`、`resolve_runfiles_path`、`NewTmpDir`、`Runner::output`、`Error::{not_exist, exit}` 的上游与内部调用关系；通用名称会混入其他 crate 同名符号，因此结论只采用 `cmd/mirror` 路径限定结果。
- [`cmd/mirror/Cargo.toml`](Cargo.toml)、[`cmd/mirror/lib.rs`](lib.rs)、[`cmd/mirror/bin_main.rs`](bin_main.rs)、[`cmd/mirror/mirror.rs`](mirror.rs)：核对 crate 边界、装配入口、生产主链及 trait 消费位置。
- [`cmd/mirror/mirror.go`](mirror.go)：核对 flag、Bazel helper、文件操作、`go` 命令、`GOSUMDB`、ExitError 和 defer 清理的移植语义。
- [`cmd/mirror/parity_test.rs`](parity_test.rs)：核对唯一临时目录、幂等删除、runfiles workspace、Go flag 解析、命令记录、补丁文件、stderr panic 和成功/失败清理。
- [`cmd/mirror/mirror_test.rs`](mirror_test.rs)：核对输出写失败的 Go `fmt.Print*` 兼容行为及删除失败 panic 文本。
- `rg --files cmd/mirror`：确认目录内没有 Go 独立测试文件，也没有同名 `stubs` 测试文件；相关 Rust 测试按 `lib.rs` 的 `#[cfg(test)]` 独立挂接。

本任务是纯文档分析，按计划不运行 Cargo。交付结构校验要求本文恰有上述 11 个固定二级标题；事实复核以源码、图索引、Cargo、Go 对照和独立测试交叉完成。
