# `cmd/mirror/lib.rs`

## 文件定位

`cmd/mirror/lib.rs` 是 Cargo 包 `astersql-cmd-mirror` 的库根，而不是 mirror 业务算法的实现文件。`cmd/mirror/Cargo.toml` 同时声明 `[lib] path = "lib.rs"` 与二进制 `astersql-cmd-mirror`（入口为 `bin_main.rs`），并把该包标记为对应 Go 包 `cmd/mirror` 的二进制迁移单元。二进制入口 `cmd/mirror/bin_main.rs::main` 只调用 `astersql_cmd_mirror::main()`，因此本文件承担“让二进制和 crate 内测试共享同一套模块与进程入口”的装配边界。

本文件通过 `#[path = "stubs.rs"]` 和 `#[path = "mirror.rs"]` 暴露 `stubs`、`mirror` 两个公开模块；实际命令流程位于 `cmd/mirror/mirror.rs`。它不是兼容再导出到其他 canonical crate 的门面，也不是生成代码，而是一个很薄的本地 crate 根。

## 核心职责

本文件只有三类职责：

1. 在 crate 级集中允许迁移代码目前存在的命名、未使用项和 Clippy 告警，包括 `non_snake_case`、`non_camel_case_types`、`unused_imports` 与 `clippy::all`。这些属性作用于整个 crate，便于 Rust 代码保留 Go 风格名称和迁移期兼容结构。
2. 装配公开的 `stubs` 与 `mirror` 模块。`stubs.rs` 定义文件系统、Bazel、子进程和测试注入边界；`mirror.rs` 实现参数处理、依赖枚举、下载及 `deps.bzl` 输出流程。
3. 提供无参数、无返回值的公共进程入口 `pub fn main()`，单跳转发到 `mirror::main()`。本层不解析参数、不创建资源，也不自行处理错误。

## 主要符号

- `pub mod stubs`（`cmd/mirror/lib.rs:20-23`）：公开系统边界与测试替身。下游实现使用其中的 `Bazel`、`Runner`、`Fs`、`Error`、`Result` 以及生产实现；测试使用 `ScriptedEnv`、`MemFs`、`Capture` 等注入对象。
- `pub mod mirror`（`cmd/mirror/lib.rs:25-28`）：公开真实 mirror 流程和可注入辅助入口。关键下游符号包括 `mirror::main`、`run_main_with` 与 `mirror_with`。
- `mod parity_test`（`cmd/mirror/lib.rs:30-33`）：仅在 `cfg(test)` 下编译的独立测试文件 `parity_test.rs`，验证 Rust 与 Go 的用户可观察契约。
- `mod mirror_test`（`cmd/mirror/lib.rs:35-37`）：仅在 `cfg(test)` 下编译的独立测试文件 `mirror_test.rs`，补充输出错误和清理 panic 文本的回归覆盖。
- `pub fn main()`（`cmd/mirror/lib.rs:43-45`）：本文件唯一函数和对外进程入口，函数体只有 `mirror::main()`；没有参数、返回值、泛型或条件编译分支。

## 执行流程

生产调用链为：

1. Cargo 构建的二进制从 `cmd/mirror/bin_main.rs::main` 启动。
2. 二进制调用库 crate 的 `astersql_cmd_mirror::main`，即本文件的 `main`。
3. 本文件同步调用 `cmd/mirror/mirror.rs::main`，不拦截返回、不捕获 panic，也不创建额外线程。
4. `mirror.rs::main` 从进程环境读取参数，构造 `ProdBazel`、`ProdRunner`、`OsFs` 及标准输出/错误，然后调用 `run_main_with`。
5. `run_main_with` 解析已废弃的 `mirror`/`upload` 标志并输出兼容提示，随后经 `mirror_with` 创建临时目录、列举 Go 模块、下载 zip 信息并输出新的 `deps.bzl` 内容。
6. 普通下游错误在 `run_main_with` 中转成 panic；若错误代表子进程退出，panic 文本包含该子进程的 stderr。只有 `run_main_with` 自身返回 `Err` 的路径才由 `mirror.rs::main` 写入 stderr 并以状态码 2 退出。

本文件不改变上述任何一步的顺序或结果；它的契约是保持一次透明的同步转发。

## 数据与状态

`lib.rs` 不声明常量、结构体、枚举、trait、类型别名、静态变量或可变全局状态，也不持有命令参数、输出缓冲区或资源句柄。唯一与全 crate 有关的状态是编译配置：顶层 `#![allow(...)]` 会影响两个生产子模块以及测试模块的 lint 诊断。

运行期状态均属于下游：废弃参数状态由 `stubs::Flags` 及 `mirror.rs` 的设置逻辑承载；文件系统、Bazel 和子进程状态通过 `stubs.rs` 的接口传入；列举及下载结果由 `mirror.rs` 的 `ListedModule`、`DownloadedModule` 等类型承载。本文件不缓存、复制或转换这些值。

## 依赖与调用关系

上游直接入口是 `cmd/mirror/bin_main.rs::main`，其调用 `astersql_cmd_mirror::main()`；Cargo 的 `[[bin]]` 与 `[lib]` 声明确立了这条二进制到库的边界。RustCodeGraph 将本文件识别为包含两个符号的已索引文件，并确认 `cmd/mirror/lib.rs::main` 的函数体调用 `mirror::main`；精确调用链也可由三个入口函数源码直接复核。

下游直接依赖只有本 crate 的 `mirror` 模块。`mirror.rs::main` 再依赖 `stubs::args_from_env`、`ProdBazel`、`ProdRunner`、`OsFs` 和标准 IO。`cmd/mirror/Cargo.toml` 的外部 Rust 依赖只有 `serde`（启用 `derive`）与 `serde_json`，它们用于下游模块的 Go 命令 JSON 数据解析，并非由 `lib.rs` 直接引用。

测试关系由本文件显式装配：`parity_test.rs` 和 `mirror_test.rs` 使用 `crate::mirror`、`crate::stubs`，所以测试看到的模块结构与生产库入口一致。两份测试均保持在独立文件中，没有把测试逻辑内嵌到 `lib.rs`。

## 错误处理与边界

`lib.rs::main` 没有 `Result` 返回值，也没有 `match`、`?`、panic 捕获或退出码处理。因而 `mirror::main` 的 panic 和进程退出语义会原样穿过本层；在这里增加吞错、重试或错误改写都会改变当前进程契约。

实际错误边界见 `cmd/mirror/mirror.rs:461-510`：参数解析错误可从 `run_main_with` 返回；mirror 流程的普通错误被 `panic!("{err}")` 包装；子进程退出错误被包装成包含 stderr 的 panic；最外层保留一个打印错误并 `exit(2)` 的分支。`cmd/mirror/parity_test.rs` 的 `contract_error_paths` 验证缺失下载结果、坏 JSON、子进程 stderr 和废弃参数提示，`cmd/mirror/mirror_test.rs` 则验证输出写失败不会遮蔽业务错误，以及临时目录删除失败的 panic 文本。

crate 级宽松 lint 是迁移边界，不等于运行时容错：它只抑制编译诊断，不能证明被允许的未使用项或 Go 风格命名都应永久保留。收紧这些 lint 时应逐模块评估，不能只删本文件属性后假定行为不受影响。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道或事务；`main` 是同步单跳调用，生命周期完全包含于下游调用期间。它也不拥有文件、目录、子进程或输出 writer，因此本层没有显式清理逻辑。

下游的关键资源契约是 `mirror_with` 在创建临时目录后立即建立 `TmpDirGuard`，使成功和后续失败路径都尝试删除目录；这对应 Go `mirror()` 中 `defer os.RemoveAll(tmpdir)`。`parity_test.rs::contract_resource_cleanup` 覆盖成功、列举失败和删除失败三类路径，`mirror_test.rs::cleanup_panic_uses_go_error_text_instead_of_rust_debug_fields` 固定删除失败的可见 panic 文本。生产外部调用通过 `Bazel`、`Runner`、`Fs` trait 同步执行；本文件不引入额外并发或改变资源析构时机。

## 与 Go 版本的对应关系

Go 对照文件 `cmd/mirror/mirror.go` 使用 `package main`，其 `main`（第 319-334 行）直接解析 flag、输出两个废弃参数提示，并调用 `mirror()`；`mirror()`（第 297-317 行）创建临时目录、注册延迟删除、列举模块、下载模块并输出依赖定义。Rust 将同一文件中的职责拆成三层：`bin_main.rs` 是物理二进制入口，`lib.rs::main` 是共享库入口，`mirror.rs::main`/`run_main_with`/`mirror_with` 承载 Go 主流程和可测试注入点。

因此，Rust `lib.rs` 没有逐行对应的 Go 独立文件；它是为 Cargo 的“库加薄二进制”组织方式增加的局部接线。可观察语义仍以 Go `mirror.go` 为基准：旧 flag 被接受并提示、主流程顺序不变、子进程退出错误保留 stderr、临时目录在成功和失败时清理。`parity_test.rs::go_rust_public_contract_matches` 汇总正常输出、边界、错误和清理四组对齐场景；`mirror_test.rs` 补充 Go `fmt.Print*` 忽略写错误及清理错误文本的细节。

## 扩展指南

- 若新增纯业务步骤，应优先修改 `mirror.rs` 的 `run_main_with` 或 `mirror_with`，并通过现有 `Bazel`、`Runner`、`Fs` 边界注入；不要把业务状态塞入 `lib.rs::main`。
- 若新增操作系统、网络、Bazel 或进程能力，应先扩展 `stubs.rs` 中最窄的 trait 契约及其生产/测试实现，再在 `mirror.rs` 接线。需评估外部命令次数、下载量、临时文件量和输出稳定性等性能风险。
- 若改变公开模块名、可见性、crate 入口或二进制接线，才需要修改 `lib.rs`/`bin_main.rs`；同时检查 `Cargo.toml` 的 `[lib]`、`[[bin]]` 和包元数据，避免破坏调用路径或测试访问面。
- 行为变更应同步更新独立测试 `cmd/mirror/parity_test.rs` 或 `cmd/mirror/mirror_test.rs`，不要在生产源文件内嵌测试。用户可观察的命名、排序、提示、panic 文本及清理语义优先放入 parity 测试。
- 收紧 `#![allow(...)]` 属于全 crate 影响的兼容性修改，应先确认 `mirror.rs`、`stubs.rs` 和两份测试不再依赖相应迁移豁免。公开入口签名变化还会影响二进制调用者，属于编译兼容风险。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter cmd/mirror` 列出 `lib.rs`、`bin_main.rs`、`mirror.rs`、`stubs.rs`、`parity_test.rs`、`mirror_test.rs` 和 Go 对照 `mirror.go`。
- RustCodeGraph 源码/符号查询：`node --file cmd/mirror/lib.rs` 确认文件共 45 行、公开模块和唯一函数；`node cmd/mirror/lib.rs::main` 确认 `pub fn main()` 仅调用 `mirror::main()`；文件查询还核对了 `bin_main.rs::main` 与 `mirror.rs::main`、`run_main_with`、`mirror_with`。
- crate 边界：`cmd/mirror/Cargo.toml` 声明库路径、二进制路径、Go 包映射、二进制迁移类型以及 `serde`/`serde_json` 依赖；根 `Cargo.toml` 将 `cmd/mirror` 纳入 workspace。
- Go 对照：`cmd/mirror/mirror.go:297-334` 给出 `mirror()` 与 Go `main()` 的流程、错误和临时目录清理语义。
- 独立测试：`cmd/mirror/parity_test.rs` 验证 flag、JSON 串流解析、依赖输出、错误包装与资源清理；`cmd/mirror/mirror_test.rs` 验证输出错误忽略规则和清理 panic 文本。两者由 `lib.rs` 的 `cfg(test)` 模块声明接入。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求本文恰好具有任务规定的 11 个二级标题。
