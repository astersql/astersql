# `cmd/pluginpkg/lib.rs`

## 文件定位

[`cmd/pluginpkg/lib.rs`](lib.rs) 是 Cargo 包 `astersql-cmd-pluginpkg` 的库根，而不是插件打包算法的实现文件。`cmd/pluginpkg/Cargo.toml` 以 `[lib] path = "lib.rs"` 声明该库，并以 `[[bin]] path = "bin_main.rs"` 声明同名二进制；`cmd/pluginpkg/bin_main.rs::main` 调用 `astersql_cmd_pluginpkg::main()`，再由本文件转发到 `pluginpkg::main()`。因此它位于“物理二进制入口”和“可测试的命令实现”之间，负责让二者共享同一份 crate 模块树。

本文件用显式 `#[path]` 装配公开的 `stubs`、`pluginpkg` 两个生产模块，并在测试构建中装配 `parity_test.rs` 与 `pluginpkg_test.rs`。实际参数解析、清单读取、Go 源码生成、`go build` 调用及输出均位于 `cmd/pluginpkg/pluginpkg.rs`；文件系统、进程、时钟和测试替身边界位于 `cmd/pluginpkg/stubs.rs`。它不是生成代码，也不是把实现再导出到其他 canonical crate 的兼容门面，而是该本地命令 crate 的薄装配根。

## 核心职责

1. 通过 `pub mod stubs` 和 `pub mod pluginpkg` 建立 crate 的公开模块路径，使命令实现、二进制入口和独立测试使用同一套类型与函数。
2. 提供公共的无参数进程入口 `pub fn main()`，只做一次同步转发，不复制 `pluginpkg.rs` 的业务流程。
3. 以 `#[cfg(test)]` 把两份独立测试文件纳入 crate 内部测试构建；生产构建不包含这些模块。
4. 在 crate 根集中允许迁移代码所需的 `dead_code`、Go 风格命名、未使用项和 `clippy::all`。这些属性改变编译诊断范围，不代表运行时容错，也不证明所有被抑制项都应长期保留。

本文件不解析 CLI、不读写文件、不启动 `go` 子进程、不持有 manifest，也不决定失败后的退出或清理语义；这些职责必须继续留在下游实现和系统边界模块中。

## 主要符号

- `pub mod stubs`（`cmd/pluginpkg/lib.rs:18-19`）：把 `stubs.rs` 暴露为公共模块。主要边界包括 `Flags`、`Fs`、`Runner`、`Clock`，生产实现 `OsFs`、`ProdRunner`、`SystemClock`，以及独立测试使用的 `MemFs`、`ScriptedRunner`、`FixedClock`、`Capture`。
- `pub mod pluginpkg`（`cmd/pluginpkg/lib.rs:21-23`）：把 `pluginpkg.rs` 暴露为公共模块。该模块提供生产入口 `pluginpkg::main`、可注入核心流程 `run_with`，以及模板、manifest 和 `go build` 参数辅助函数。
- `mod parity_test`（`cmd/pluginpkg/lib.rs:25-27`）：仅在 `cfg(test)` 下加载 `parity_test.rs`，集中验证 Rust 与 Go 的用户可观察合同。
- `mod pluginpkg_test`（`cmd/pluginpkg/lib.rs:29-31`）：仅在 `cfg(test)` 下加载 `pluginpkg_test.rs`，补充模板作用域、部分输出、延迟清理顺序和动态类型断言的回归覆盖。
- `pub fn main()`（`cmd/pluginpkg/lib.rs:33-36`）：本文件唯一函数；无参数、无返回值，函数体唯一语句是 `pluginpkg::main()`。
- crate 级 `#![allow(...)]`（`cmd/pluginpkg/lib.rs:8-16`）：对上述生产模块及测试模块生效。本文件没有常量、类型、trait、`impl`、feature 分支或运行期全局变量。

## 执行流程

生产入口链如下：

1. Cargo 根据 `cmd/pluginpkg/Cargo.toml` 构建二进制 `astersql-cmd-pluginpkg`，进入 `cmd/pluginpkg/bin_main.rs::main`。
2. 二进制调用库符号 `astersql_cmd_pluginpkg::main()`，即本文件的 `main`。
3. `lib.rs::main` 同步调用 `cmd/pluginpkg/pluginpkg.rs::main`，不插入错误处理、清理或线程切换。
4. `pluginpkg.rs::main` 重置 flag 状态，从环境取得参数，用 `stubs::try_parse_flags` 解析，并装配 `OsFs`、`ProdRunner`、`SystemClock`、stderr 与 stdout。
5. `pluginpkg.rs::run_with` 规范化路径，读取和解析 `manifest.toml`，注入构建时间，校验插件名与目录名及字符串版本，创建并渲染 `<插件名>.gen.go`。
6. 主流程组合 `go build` 参数（可选 `-pgo`、固定 `codes` tag、可选 `nextgen` tag、`-buildmode=plugin` 和输出 `.so`），以插件目录为工作目录并附加 `GO111MODULE=on` 执行子进程。
7. 成功时输出打包路径及缩进后的 manifest JSON，随后尝试删除临时生成文件；失败路径按 Go `os.Exit` 对齐语义立即终止并保留已生成的临时文件。

测试构建时，编译器还会从本文件加载两份测试模块。测试主要直接调用 `run_with` 和辅助函数以注入内存文件系统、脚本执行器与固定时钟，并不依赖真实 `go build`。

## 数据与状态

`lib.rs` 自身不保存运行期数据：模块声明只建立编译期名称和可见性，薄入口不接收、复制或返回任何值。改变 `stubs` 或 `pluginpkg` 的 `pub` 可见性会改变 crate 的 Rust API 面，但不会由本文件产生额外状态。

下游状态需要理解但不归本文件所有：`stubs::Flags` 保存 `pkg_dir`、`out_dir`、`pgo_file` 和 `next_gen`；`pluginpkg.rs` 用四个 `thread_local!` 单元模拟 Go 包级 flag 变量，由 `init_flags` 重置、`run_with` 写入并取回。这避免 `unsafe` 进程级可变静态和跨线程测试污染，但也意味着每个线程有独立 flag 副本。manifest 以 `serde_json::Map<String, Value>` 承载，临时源码和输出路径只在下游流程内存在。

本文件唯一跨模块影响是 crate 级 lint 配置：它覆盖两个生产模块和两个测试模块。收紧或扩大该列表属于整个 crate 的编译策略变更，不能按单个函数局部修改来评估。

## 依赖与调用关系

直接上游源码边是 `cmd/pluginpkg/bin_main.rs::main → astersql_cmd_pluginpkg::main`；直接下游源码边是 `cmd/pluginpkg/lib.rs::main → pluginpkg::main`。RustCodeGraph 能精确定位这三个同名 `main` 节点，并在对 `pluginpkg.rs::main` 的流程查询中给出 `main → run_with`；当前索引的精确 `callers/callees` 命令没有为 crate 根这两条薄包装边返回记录，所以入口接线以函数源码和 Cargo 目标声明为直接证据，不能把图缺边解释为未接线。

`cmd/pluginpkg/Cargo.toml` 将该包标记为对应 Go 包 `cmd/pluginpkg` 的 `binary` 迁移单元，并声明直接外部依赖 `serde`（含 `derive`）、`serde_json`、`toml`。这些库由 `pluginpkg.rs` 的 manifest 转换、模板数据及 JSON 输出使用；`lib.rs` 本身不直接调用它们。根 `Cargo.toml` 将 `cmd/pluginpkg` 列为 workspace 成员。

测试依赖由本文件显式接线：`parity_test.rs` 和 `pluginpkg_test.rs` 通过 `crate::pluginpkg`、`crate::stubs` 访问生产模块。仓库搜索没有发现 Go `*_test.go` 对应文件，也没有发现 crate 外 Rust 代码直接调用 `astersql_cmd_pluginpkg`；当前生产调用面是本包的 `bin_main.rs`。

## 错误处理与边界

`lib.rs::main` 没有 `Result`、`match`、`?`、panic 捕获或退出码转换。下游正常返回时它立即返回；下游调用 `std::process::exit(1)`、发生 panic 或阻塞时，本层保持原行为。若在此加入吞错、重试、日志或清理，会改变当前透明入口合同。

实际边界由 `pluginpkg.rs` 与 `stubs.rs` 定义：flag 错误和缺少必填目录进入 `usage`；路径、manifest 读取/解析、临时文件创建/写入、模板执行和 `go build` 失败记录相应诊断后走 `fatal_exit`。`fatal_exit` 在生产构建中退出进程，在测试构建中 panic 以便 `catch_unwind` 断言。manifest 的 `name`、`version` 保留 Go 动态类型断言语义，非字符串会 panic，而不是被宽松转换。

模板执行器明确只支持当前 `CODE_TEMPLATE` 所需的字段、`if` 和 `range` 子集，不能把它描述为完整的 Go `text/template` 实现。JSON 输出失败只记日志，不把已成功的插件构建改成失败；成功后的临时文件删除失败同样只提示人工清理。独立测试覆盖缺参数、坏 flag、坏 TOML、名称不一致、错误类型字段、模板失败、构建失败及删除失败。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或资源守卫；两层 `main` 调用都是同步嵌套的。它不拥有文件句柄、子进程、输出 writer 或临时文件，因此没有本层清理动作。

下游唯一显式并发相关状态是每线程 flag 存储；生产入口在当前线程初始化、解析和运行，不跨线程传递。`Capture` 使用 `Arc<Mutex<Vec<u8>>>` 保存测试输出，但实际打包主流程仍同步；`MemFs` 与 `ScriptedRunner` 使用 `Rc<RefCell<...>>`，是单线程测试替身，不能据此宣称生产边界线程安全。

关键资源生命周期与 Go 保持一致：先创建权限 `0700` 的 `.gen.go`，再渲染并执行 `go build`；只有正常完成输出后才删除临时文件。模板或构建失败对应 Go `os.Exit(1)` 跳过 `defer`，因此保留部分或完整临时源码；正常路径的删除失败只记录日志。`pluginpkg_test.rs::deferred_remove_runs_after_manifest_output` 验证删除发生在 manifest 输出之后，`parity_test.rs::contract_resource_cleanup` 验证成功/失败清理差异。

## 与 Go 版本的对应关系

Go 对照 `cmd/pluginpkg/pluginpkg.go` 把全局 flags、`init`、`usage`、模板常量和完整 `main` 都放在 `package main` 的单个文件中；它没有与 Rust `lib.rs`、`bin_main.rs` 一一对应的库门面。Rust 为 Cargo 的“可复用库 + 薄二进制”结构增加这两层接线，Go `main` 的实质对应物是 `pluginpkg.rs::main + run_with`，而不是本文件的转发函数。

下游 Rust 流程保留 Go 的关键顺序与可见行为：四个 flag 的默认值和解析形式、路径绝对化、`manifest.toml` 解码、`buildTime` 注入、目录名与插件名一致性、字符串 `version` 断言、生成文件名与 `0700` 权限、`codes[,nextgen]` tags、可选 PGO、`GO111MODULE=on`、成功提示、Go 风格 JSON 缩进以及 `defer`/`os.Exit` 清理差异。Rust 特有的可注入 `Fs`/`Runner`/`Clock` 和测试期 panic 只是为了独立验证这些合同。

`parity_test.rs::go_rust_public_contract_matches` 汇总正常打包、边界参数和模板、错误路径、资源清理四类合同；其余测试固定 flag 的 Go 解析细节、动态类型断言、时间格式、缺失模板字段和 JSON HTML 转义。`pluginpkg_test.rs` 补充 `range` 作用域、模板失败留下部分输出、删除时机以及非字符串名称的 panic。当前同目录没有 Go 测试文件，因此 Go 语义证据来自 `pluginpkg.go` 本身和 Rust 对照测试，而不是虚构的 Go 测试覆盖。

## 扩展指南

- 新增打包步骤、manifest 字段或模板行为时，应修改 `cmd/pluginpkg/pluginpkg.rs` 的最窄相关函数（通常为 `run_with`、模板辅助函数或 `build_go_flags`），不要把业务逻辑放进 `lib.rs::main`。
- 新增文件系统、进程、时钟或参数解析能力时，先扩展 `cmd/pluginpkg/stubs.rs` 中最窄的 trait/边界，并同步生产实现和测试替身；评估真实子进程环境继承、文件权限及失败清理兼容性。
- 只有改变 crate 模块布局、公开可见性、测试模块接线或库入口签名时才应修改本文件；文件移动还必须同步 `#[path]` 和 `cmd/pluginpkg/Cargo.toml` 的 `[lib]`/`[[bin]]` 路径。
- 行为变更应同步独立测试 `cmd/pluginpkg/parity_test.rs` 或 `cmd/pluginpkg/pluginpkg_test.rs`，不得把 Rust 测试逻辑内嵌到生产源文件。涉及 Go 对齐时，应以 `cmd/pluginpkg/pluginpkg.go` 的真实控制流为基准，并分别覆盖成功、错误和资源生命周期。
- 收紧 crate 级 allow 列表前要检查 `pluginpkg.rs`、`stubs.rs` 及两份测试；公开模块或 `main` 签名变化属于编译兼容风险。模板/flag/日志变化属于脚本兼容风险，额外文件和子进程步骤还可能增加启动延迟、磁盘 IO 或重复编译成本。

## 验证依据

- RustCodeGraph `status`：索引可用，覆盖 11,467 个文件，其中 Rust 7,032 个；`files --filter cmd/pluginpkg` 列出目标、二进制入口、实现、系统边界、两份 Rust 测试和 Go 对照文件。
- RustCodeGraph `node --file cmd/pluginpkg/lib.rs --offset 1 --limit 240`：确认目标文件共 36 行、两个公开模块、两个测试模块和唯一函数；`node cmd/pluginpkg/lib.rs::main` 确认函数只调用 `pluginpkg::main()`。
- RustCodeGraph 符号/流程查询：精确定位 `cmd/pluginpkg/bin_main.rs:7`、`lib.rs:34`、`pluginpkg.rs:816` 三个 `main`；`explore` 给出 `pluginpkg.rs::main → run_with`，并列出 `run_with` 的生产调用与同目录测试调用。精确 `callers/callees` 未返回 crate 根薄包装边，本文据实记录该索引限制。
- 已读构建和入口证据：`cmd/pluginpkg/Cargo.toml`、根 `Cargo.toml`、`cmd/pluginpkg/bin_main.rs`；前者确认库/二进制目标、Go 包映射及三个外部依赖，后者确认 workspace 成员和二进制到库入口。
- 已读实现与 Go 对照：`cmd/pluginpkg/pluginpkg.rs`、`cmd/pluginpkg/stubs.rs`、`cmd/pluginpkg/pluginpkg.go`，核对参数、模板、manifest、子进程、错误、线程局部状态及清理顺序。
- 已读独立测试：`cmd/pluginpkg/parity_test.rs`、`cmd/pluginpkg/pluginpkg_test.rs`；仓库搜索确认同目录没有 Go `*_test.go`。测试覆盖正常、边界、错误、清理、类型断言与格式兼容合同。
- 本任务是纯文档分析，按计划不运行 Cargo；交付只运行任务规定的 11 标题结构检查，并人工复核“为何存在、如何运行、如何安全扩展”均有上述源码、图查询或配置证据。
