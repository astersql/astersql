# `lightning/cmd/tidb-lightning-ctl/lib.rs`

## 文件定位

本文件是 Cargo package `astersql-lightning-cmd-tidb-lightning-ctl` 的库 crate 根。`lightning/cmd/tidb-lightning-ctl/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定它，并把同一 package 标记为对照 Go package `lightning/cmd/tidb-lightning-ctl` 的 `binary` 移植单元。真正的可执行文件入口位于 `bin_main.rs`：其中的私有 `fn main()` 调用本库导出的 `astersql_lightning_cmd_tidb_lightning_ctl::main()`。

因此，这个文件是装配门面而不是控制命令的业务实现。它把本地兼容桩、FIPS 钩子、命令入口和独立测试组成一个 crate；命令解析、PD/TiKV 控制和 checkpoint 操作位于它通过 `#[path = "main.rs"] pub mod entry` 纳入的 `main.rs`。

## 核心职责

1. 用 crate 级 `#![allow(...)]` 暂时放宽机械迁移代码中的命名、未使用项和 Clippy 约束。该属性作用于整个 crate，包含这里声明的子模块；它反映当前 Go 对齐阶段的编译策略，不代表这些告警在业务上没有风险。
2. 以显式 `#[path]` 组装三个生产模块：公开的 `stubs`、公开的 `fips`，以及以 Rust 模块名 `entry` 暴露的 `main.rs`。
3. 通过 `pub use stubs::*` 把 `stubs.rs` 的公开符号提升到 crate 根，供 `entry` 中的 `use crate::stubs::*`、库使用者及 crate 内测试共享兼容层类型和函数。
4. 仅在测试构建中纳入 `parity_test.rs` 和 `main_test.rs`，避免测试逻辑进入普通库或二进制构建。
5. 提供唯一的函数级公开入口 `pub fn main()`，将调用原样转发给 `entry::main()`。

本文件不解析参数、不创建网络客户端、不选择控制动作，也不持有运行时状态；这些职责都由下游模块承担。

## 主要符号

- `pub mod stubs`（`lib.rs:17-18`）：把 `stubs.rs` 声明为公开模块。该文件自述为面向本地 arm64 环境的 flag、PD HTTP、TiKV、配置兼容桩；它不是本文件内联实现的真实网络客户端。
- `pub use stubs::*`（`lib.rs:20`）：通配重导出 `stubs` 的全部公开 API。这构成较宽的 crate 根兼容面；新增同名符号可能造成名称冲突或意外扩大公共 API。
- `pub mod fips`（`lib.rs:22-23`）：公开纳入 `fips.rs`。其当前公开入口 `enable_fips_only()` 是空函数；实际命令主流程仍会调用它，以保留与 Go FIPS 启动顺序对应的接线点。
- `pub mod entry`（`lib.rs:26-27`）：将物理文件 `main.rs` 命名为 `entry`，避免与 crate 根的 `main()` 混淆。业务入口 `entry::main()`、`run_main()`、`run()`、`compactCluster()` 和 `fetchMode()` 均定义在该模块。
- `mod parity_test` 与 `mod main_test`（`lib.rs:29-35`）：两个私有、仅测试构建可见的独立测试模块，分别覆盖 Go/Rust 外部契约与进程入口行为。
- `pub fn main()`（`lib.rs:39-41`）：无参数、无返回值的库级进程入口；函数体只有 `entry::main()`，不捕获 panic、不改写返回值，也不增加清理逻辑。

本文件没有常量、静态变量、类型、trait、`impl` 或宏定义；除测试模块的 `#[cfg(test)]` 外也没有 feature 条件分支。

## 执行流程

普通二进制运行的直接调用链为：

1. Cargo 根据 `Cargo.toml` 的 `[[bin]] path = "bin_main.rs"` 构建可执行目标。
2. `bin_main.rs::main()` 调用 `astersql_lightning_cmd_tidb_lightning_ctl::main()`。
3. 本文件的 `main()` 立即调用 `entry::main()`。
4. `entry::main()` 收集 `std::env::args().skip(1)`，再进入 `main_with_args()`；后者触发 FIPS 钩子、执行 `run_main()`，并在非零退出码时调用可替换的退出函数。
5. 更深层的配置装载、TLS/PD client 建立、动作优先级分派、checkpoint 控制与 client 关闭均发生在 `main.rs`，不属于本门面的内部逻辑。

测试构建走另一条装配路径：启用 `cfg(test)` 后，`parity_test.rs` 和 `main_test.rs` 成为 crate 内私有模块，因此可以使用 `crate::entry`、`crate::fips` 和 `crate::stubs` 验证同一套库代码，而无需把测试写入生产源文件。

## 数据与状态

本文件自身不定义或存储任何业务数据。`main()` 不接受参数，参数来源由 `entry::main()` 从进程环境读取；它也不返回 `Result` 或退出码，因此错误和退出状态完全由下游入口处理。

这里唯一具有全 crate 影响的“状态”是编译期结构：模块可见性、通配重导出和 lint 配置。`pub use stubs::*` 决定了 crate 根的名字集合；`#[cfg(test)]` 决定两个测试模块是否存在；`#![allow(...)]` 决定子模块中的相关诊断是否被抑制。这些都是编译期约束，不是可变运行时状态。

## 依赖与调用关系

上游直接调用者是 `lightning/cmd/tidb-lightning-ctl/bin_main.rs::main`，其唯一语句调用本 crate 的 `main()`。根 workspace 的 `Cargo.toml` 将 `lightning/cmd/tidb-lightning-ctl` 列为成员，package 自身的 `Cargo.toml` 同时声明库目标和二进制目标。

本文件的唯一函数调用边是 `lib.rs::main -> entry::main`。模块级依赖为：

- `lib.rs -> stubs.rs`：公开模块并通配重导出；
- `lib.rs -> fips.rs`：公开 FIPS 接线模块；
- `lib.rs -> main.rs`：以 `entry` 名称公开真正主流程；
- 测试构建时，`lib.rs -> parity_test.rs` 与 `lib.rs -> main_test.rs`。

`Cargo.toml` 的直接 package 依赖只有 `astersql-lightning-pkg-importer` 和 `astersql-lightning-pkg-server` 两个同 workspace path crate。它们由本 crate 的下游实现/桩层使用；`lib.rs` 本身没有 `use` 它们，也不应据此声称门面直接执行 importer 或 server 逻辑。

RustCodeGraph 能精确定位 `lib.rs:39` 的 `main` 节点及其源码，但本次索引对同名 `main` 的 callers/callees 查询返回了 `br/**` 等无关目录的结果。那些结果与直接源码引用冲突，未被采用；上述调用边由 `lib.rs`、`bin_main.rs` 和 Cargo 声明交叉核对。

## 错误处理与边界

本文件没有错误类型、`Result`、错误转换或日志输出。`main()` 对 `entry::main()` 的行为完全透明：下游发生 panic 时会继续展开，进程退出或错误格式化也不会被本层拦截。

边界上需特别注意：

- `entry::main()` 的签名同样为 `()`，因此库级入口不能向调用者返回结构化错误；若需要可测试的返回值，应使用下游 `entry::run(args) -> Result<()>` 或 `entry::run_main(args) -> i32`，而不是改变本门面现有的二进制契约。
- `pub use stubs::*` 会把兼容桩暴露为正式公共符号。它便于当前移植和测试，但调用者不应误把所有桩行为理解为真实 PD/TiKV 网络实现。
- `#[path]` 把模块名与文件名解耦；移动或改名任一被引用文件时，必须同步本文件，否则在编译期直接失败。
- crate 级 `allow` 范围很宽。新增生产逻辑时不能依赖被抑制告警来隐藏未使用、命名冲突或 Clippy 问题。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、上下文、客户端或文件句柄，也没有 `unsafe` 和自定义析构。它的同步调用在 `entry::main()` 返回后立即结束。

并发和资源生命周期只通过下游代码间接发生：`main.rs::run_loaded()` 创建 PD client，并在 `dispatch()` 返回后显式 `Close()`；`stubs.rs` 的 `ForAllStores` 以及 `parity_test.rs::for_all_stores_runs_in_parallel_and_cancels_child_context` 涉及并行 store 遍历和子 context 取消。它们可用于验证整个 crate 的行为，但不能归因成本文件自身实现。若在 `lib.rs::main()` 增加前置或后置逻辑，必须避免绕过下游退出路径或改变 client 清理时序。

## 与 Go 版本的对应关系

Go 同路径代码使用单一 `package main`，`main.go::main()` 直接执行 `run()`、格式化错误并调用可替换的 `exit(1)`；Go 没有与 Rust `lib.rs` 一一对应的库门面文件。Rust 为兼顾 Cargo 的库/二进制复用，将对应关系拆成三层：`bin_main.rs` 提供可执行壳，`lib.rs::main()` 提供稳定库入口，`main.rs::entry::main()` 承担 Go `main()` 的进程行为。

Rust 的 `stubs` 通配重导出和 `fips` 显式模块也是迁移期装配结构。Go 通过 package 作用域直接共享符号，并可通过 import 副作用接入 FIPS；Rust 则通过模块路径和显式调用保留接线点。`Cargo.toml` 的 `package.metadata.porting.go-package` 明确把整个 crate 映射到该 Go package，而不是宣称 `lib.rs` 单独复刻某个 Go 文件。

对应测试为：

- `main_test.go::TestRunMain`：Go 侧验证入口可运行、checkpoint 表不存在时不输出栈、普通错误仍保留栈；
- `main_test.rs::test_run_main`：Rust 侧用独立子进程、可替换 exit 和参数过滤验证入口边界；
- `parity_test.rs`：验证 flag 语义、错误身份、动作契约、PD client 清理以及 store 遍历等 Go/Rust 对齐行为。

这些测试由 `lib.rs` 在测试配置中纳入，但具体断言针对 `entry`、`fips` 和 `stubs` 的组合行为。

## 扩展指南

- 新增控制子命令或改变参数、错误码、动作优先级时，应修改 `main.rs` 的装载/分派符号，并同步 `parity_test.rs`；不要把业务分支堆入 `lib.rs::main()`。
- 新增真实公共模块时，在本文件明确声明其可见性；除非确有兼容需求，优先使用具名重导出，避免继续扩大 `pub use ...::*` 带来的冲突面。
- 替换或收窄 `stubs` 时，要先盘点 crate 根重导出 API 以及 `main.rs`、`main_test.rs`、`parity_test.rs` 的使用者，并保持 `Cargo.toml` 依赖可在目标平台复现。
- 改变 FIPS 初始化方式时，应保留 `entry::main()` 中先于主逻辑执行的语义，并同步 `fips.rs`/`fips.go` 及 parity 测试。
- 改变库入口签名或可见性会同时影响 `bin_main.rs` 和潜在库调用者；至少应新增或更新独立测试文件，不能把单元测试内嵌到 `lib.rs`。本仓库约定 Rust 测试与生产源文件分离，现有两个 `#[cfg(test)] #[path = ...]` 模块正是接入点。
- 若只是为下游提供可测试错误边界，优先复用 `entry::run`/`run_main`；保持 `lib.rs::main()` 为薄转发层可以降低二进制启动契约的兼容风险和性能影响。

## 验证依据

本说明核对了以下直接证据：

- `lightning/cmd/tidb-lightning-ctl/lib.rs`：完整 41 行源码；确认 crate 属性、三个生产模块、两个测试模块、通配重导出和唯一函数体。
- `lightning/cmd/tidb-lightning-ctl/Cargo.toml`：确认 package 名、`[lib]`、`[[bin]]`、Go package 元数据、binary 移植类别及两个 path 依赖。
- `lightning/cmd/tidb-lightning-ctl/bin_main.rs`：确认二进制到库入口的直接调用边。
- `lightning/cmd/tidb-lightning-ctl/main.rs` 与 `fips.rs`：确认 `entry::main()` 后续流程、可测试入口、资源关闭位置及当前 FIPS 钩子事实。
- `lightning/cmd/tidb-lightning-ctl/main.go`、`fips.go`、`main_test.go`：确认 Go package 的入口组织、错误/退出语义和原始测试意图。
- `lightning/cmd/tidb-lightning-ctl/main_test.rs`、`parity_test.rs`：确认 Rust 独立测试模块及其覆盖的入口、对齐、并发和资源清理契约。
- 根 `Cargo.toml`：确认该 package 是 workspace 成员。目标目录不存在 `doc.go`，因此没有额外的 package 合同文件可读。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning-ctl` 找到目标及相邻入口/测试；`node main --file .../lib.rs` 精确返回 `lib.rs:39-41`。同名 `main` 的 callers/callees 结果出现跨目录误配，故没有将其作为调用关系证据。

本任务是纯文档分析，未运行 Cargo。结构验收以目标文档存在且恰好具有本计划规定的十一个二级标题为准。
