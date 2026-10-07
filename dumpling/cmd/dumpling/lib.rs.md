# `dumpling/cmd/dumpling/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-dumpling-cmd-dumpling` 的库 crate 根。`dumpling/cmd/dumpling/Cargo.toml` 以 `[lib] path = "lib.rs"` 明确这一边界，同时以 `[[bin]] path = "bin_main.rs"` 声明真实二进制入口。二进制壳层 `bin_main.rs::main()` 调用本库公开的 `astersql_dumpling_cmd_dumpling::main()`，本文件再把控制流转发到 `entry::main()`。

它也是 Go 包 `dumpling/cmd/dumpling` 在 Rust 迁移中的模块装配点：通过显式 `#[path]` 将 `stubs.rs`、`config_flags.rs` 和承载完整 CLI 流程的 `main.rs` 分别公开为 `stubs`、`config_flags`、`entry`。因此本文件负责 crate 结构和稳定入口，不负责实现参数解析或数据导出。

## 核心职责

本文件有三项聚焦职责：定义 crate 级兼容 lint 策略；声明并公开 CLI 所需的三个生产模块；提供统一的公开 `main()` 门面。测试构建时，它还用 `#[cfg(test)]` 挂接两个独立测试文件，使测试可通过 `crate::entry`、`crate::config_flags` 和 `crate::stubs` 检查内部契约。

该门面避免二进制 target 和测试各自复制启动逻辑。所有启动方先经过 `lib.rs::main()`，然后进入 `main.rs::main()`；后者才读取进程参数、计算退出码并在失败时终止进程。`lib.rs` 本身不创建配置、Dumper、logger 或 Prometheus collector。

## 主要符号

- crate 属性 `#![allow(...)]`（`lib.rs:9-17`）：允许迁移代码中的未使用项、Go 风格命名以及 Clippy 告警。它作用于整个库 crate，包括下述模块；这是一项迁移兼容策略，不代表这些模块无需后续质量检查。
- `pub mod stubs`（`lib.rs:19-21`）：将 `stubs.rs` 作为公开模块，提供本地 `pflag`/Prometheus 等兼容面；`entry` 和参数模块都直接使用它。
- `pub mod config_flags`（`lib.rs:23-25`）：将 `config_flags.rs` 作为公开模块，承载 `DefineFlags` 与 `ParseFromFlags` 等参数定义、解析和配置同步逻辑。
- `pub mod entry`（`lib.rs:27-28`）：把文件名 `main.rs` 映射为公开模块名 `entry`，避免与 crate 根公开函数 `main()` 混淆；完整 CLI 主流程在此模块中。
- `mod parity_test`、`mod config_flags_test`（`lib.rs:30-36`）：仅在测试配置下编译的私有测试模块，分别映射到两个独立 `*_test.rs` 文件。
- `pub fn main()`（`lib.rs:39-42`）：本文件唯一函数及公开进程入口门面，无参数、无返回值，函数体只调用 `entry::main()`。

本文件没有常量、struct、enum、trait、`impl` 或 feature 条件；唯一条件编译项是两个测试模块。

## 执行流程

1. Cargo 分别构建同包的库 target（`lib.rs`）和二进制 target（`bin_main.rs`）。
2. 操作系统启动二进制后进入 `bin_main.rs::main()`，其唯一动作是调用 `astersql_dumpling_cmd_dumpling::main()`。
3. 本文件的公开 `main()` 无条件调用一次 `entry::main()`，不增加前置初始化、错误转换或清理步骤。
4. `entry::main()` 收集 `std::env::args().skip(1)`，调用 `run()`，并仅在返回码非零时执行 `std::process::exit(code)`。
5. `entry::run_with_factory()` 完成 usage/flag 注册、解析和校验，设置运行时指标 gatherer，创建 Dumper，执行 `Dump()`，随后始终调用 `Close()`，最后记录成功或失败。

核心不变量是本层保持透明转发：二进制运行与库内可测试入口必须抵达同一套 `entry` 控制流。测试编译只额外加入测试模块，不改变生产入口的调用路径。

## 数据与状态

本文件没有自有运行时数据：不声明静态变量、不保存 argv、不持有配置或资源，也没有缓存、锁或可变全局状态。模块声明只在编译期确定命名空间和源码映射；`main()` 的栈帧只覆盖一次同步转发调用。

可观察状态均属于下游模块。`config_flags` 把命令行值写入 `astersql_dumpling_export::Config`；`stubs` 提供 flag 状态和可替换的默认 gatherer 槽；`entry::run_with_factory()` 持有 `Config` 与实现 `DumpSession` 的 Dumper。`entry::reset_cli_globals()` 通过 `stubs::clear_default_gatherer()` 清理测试共享状态，而 `lib.rs` 不直接读写该状态。

## 依赖与调用关系

- 上游运行时调用边：`dumpling/cmd/dumpling/bin_main.rs::main → astersql_dumpling_cmd_dumpling::main`；包名中的连字符按 Rust crate 规则转为下划线。
- 本文件直接下游：`lib.rs::main → entry::main`（`lib.rs:41`）。这是本文件唯一运行时函数调用。
- 模块内后续主链：`entry::main → entry::run → entry::run_with_factory`；`run_with_factory` 使用 `config_flags::{DefineFlags, ParseFromFlags}`、`stubs::{FlagSet, register_runtime_collectors, set_default_gatherer}` 和 `astersql_dumpling_export::NewDumper`。
- 模块依赖边：`config_flags.rs` 依赖 `crate::stubs::{FlagHelp, FlagSet}`；`main.rs` 同时依赖 `crate::config_flags` 与 `crate::stubs`。本文件的公开模块声明使这些 crate 内路径成立。
- Cargo 边界：`Cargo.toml` 将包标记为 `kind = "binary"` 的 Go 包移植，并直接依赖 `astersql-dumpling-cli`、`astersql-dumpling-export`、`astersql-dumpling-log`。这些依赖由子模块使用，`lib.rs` 本身没有 `use` 导入。
- 测试调用边：`parity_test.rs` 从 `crate::entry` 导入 `run`、`run_with_factory`、`DumpSession` 等，从公开参数和 stub 模块导入辅助 API；`config_flags_test.rs` 直接使用 `crate::config_flags` 与 `crate::stubs`。

RustCodeGraph 的文件查询确认 `lib.rs` 只有模块声明和 `main()` 两类符号，并能按文件展示目标及相邻入口源码。索引对常见限定名 `lib.rs::main` 存在跨目录歧义，精确 ID 查询也发生错误映射，因此不能把图中缺失或错误的 caller/callee 当作真实无调用；上述两条入口边由调用点源码与 Cargo target 声明交叉验证。

## 错误处理与边界

`lib.rs::main()` 没有 `Result` 返回值、错误分支、panic 捕获或退出码映射。下游正常返回时它同步返回；下游 panic 时默认 panic 继续越过本层；下游调用 `std::process::exit` 时控制流不会回到本层。

具体错误语义位于 `entry`：命令行语法错误返回 2；配置转换、残余位置参数、Dumper 创建或 Dump 失败返回 1；帮助、版本和成功路径返回 0。`entry::main()` 对非零结果调用 `process::exit`。`parity_test.rs` 的 `command_line_parse_errors_use_go_pflag_exit_code`、`contract_error_paths` 与 `contract_resource_cleanup_close_and_gatherer` 分别覆盖这些边界和失败后的清理行为。

crate 级宽松 lint 是编译边界而非运行时错误处理。新增代码不能依赖这些 `allow` 来吞掉真实错误；需要返回或记录的失败仍应在职责所属的子模块中显式表达。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件、网络连接或数据库会话，也没有直接资源清理。模块装配发生在编译期，公开 `main()` 只进行同步调用。

真实生命周期沿调用链向下：`entry::run_with_factory()` 注册 collector、安装默认 gatherer、创建 Dumper、调用 `Dump()`，并在 Dump 成功或失败后都调用 `Close()`；当前实现忽略 Close 错误以保持 Go 行为。`parity_test.rs::contract_resource_cleanup_close_and_gatherer` 使用 `AtomicBool`/`AtomicUsize` 验证失败路径仍调用 Close，并验证测试辅助函数能清理共享 gatherer。保持本门面透明可确保这些下游清理路径不被绕过。

## 与 Go 版本的对应关系

Go 对照是 `dumpling/cmd/dumpling/main.go::main`。Go 文件在单个函数中完成 usage 设置、flag 注册与解析、版本输出、配置校验、Prometheus collector 注册、Dumper 创建、Dump/Close 和日志/退出处理。Rust 没有把这些行为放在 crate 根，而是拆为 `bin_main.rs` 的进程壳、`lib.rs` 的公共门面和模块装配、`main.rs` 的可测试主流程，并把 flag 与本地替身进一步拆到独立模块。

因此本文件对应的主要是 Go `package main` 的包边界和 `func main()` 的可达入口身份；完整行为必须沿 `entry::main()` 查看。`Cargo.toml` 的 `package.metadata.porting.go-package = "dumpling/cmd/dumpling"` 明确记录该对应关系。`parity_test.rs::go_rust_public_contract_matches` 验证帮助/版本、默认参数、失败退出码与资源关闭契约；`config_flags_test.rs` 独立覆盖压缩参数、列过滤和 session 参数规范化等解析细节。

实现形态上，Go 直接使用包级 `pflag.CommandLine` 和多个 `os.Exit(1)`；Rust 让 `run()` 返回整数，再由 `entry::main()` 统一退出，从而使主流程可注入 Dumper 工厂并可直接测试。这是可测试性拆分，不是删减 Go 行为。

## 扩展指南

- 新增 crate 级生产模块时，在本文件声明清晰的模块名，并检查其是否真的需要 `pub`；模块路径或文件名变化还要同步 Cargo/Bazel 等构建元数据的适用部分。
- 修改 CLI 行为应落在 `main.rs::run()`/`run_with_factory()`；新增或调整 flag 应修改 `config_flags.rs`；pflag、collector 或 gatherer 兼容行为应修改 `stubs.rs`。不要把这些逻辑堆入 `lib.rs::main()`。
- 保持 `lib.rs::main()` 单次转发到 `entry::main()`。若入口门面增加初始化，必须评估二进制与测试路径是否一致、失败退出是否仍正确、Dumper Close 是否仍可达，以及重复初始化全局 gatherer 的风险。
- Rust 测试必须保持在独立文件：入口/Go 契约同步扩展 `parity_test.rs`，flag 转换同步扩展 `config_flags_test.rs`；若新增模块，应使用对应的独立 `*_test.rs` 并在 `#[cfg(test)]` 下挂接，不把测试写进生产源文件。
- 兼容风险集中在公开模块路径、库入口名称、退出码与 Go/Rust 行为漂移；性能风险当前极低，因为 crate 根只做静态装配和一次函数调用。新增启动工作时应评估启动耗时和资源释放。

## 验证依据

- 目标源码：`dumpling/cmd/dumpling/lib.rs:1-42`，确认 crate lint、三个公开生产模块、两个条件测试模块以及唯一公开函数 `main()`。
- Cargo 边界：`dumpling/cmd/dumpling/Cargo.toml`，确认包名、`[lib]`、`[[bin]]`、Go 包元数据和三个直接依赖；根 `Cargo.toml:82` 确认该包属于 workspace。
- 入口调用点：`dumpling/cmd/dumpling/bin_main.rs:4-5` 与 `lib.rs:39-41`，确认“二进制 → 库门面 → entry”的两级转发；`main.rs:52-167` 确认下游参数、退出码、Dumper 和关闭流程。
- Go 对照：`dumpling/cmd/dumpling/main.go:29-83`，确认原始 CLI 顺序、错误出口、指标安装以及 Dump 后 Close 语义。
- 独立 Rust 测试：`dumpling/cmd/dumpling/parity_test.rs` 覆盖帮助/版本、pflag 边界、退出码、Dumper 工厂错误、Dump 失败与 Close/gatherer 生命周期；`config_flags_test.rs` 覆盖独立参数转换契约。同目录不存在 Go `*_test.go`。
- RustCodeGraph：`status` 报告索引包含 7032 个 Rust 文件；`files --filter dumpling/cmd/dumpling` 列出本 crate 的 8 个 Go/Rust 文件；`node --file` 分别核对 `lib.rs`、`bin_main.rs`、`main.rs`、`main.go`、`parity_test.rs` 和 `config_flags_test.rs`。精确 caller/callee 查询的同名消歧限制已在“依赖与调用关系”中记录，并以源码调用点及 Cargo 声明补证。
- 本任务仅新增说明文档，按计划不运行 Cargo。交付前以固定标题结构命令验证恰有 11 个章节，并人工复核所有运行时行为均明确归属到真实下游符号。
