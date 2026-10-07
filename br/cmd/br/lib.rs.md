# `br/cmd/br/lib.rs`

源码：[`lib.rs`](lib.rs)

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-cmd-br` 的库 crate 根，而不是备份或恢复算法的实现文件。`br/cmd/br/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定它，同时用 `[[bin]] path = "bin_main.rs"` 声明同名可执行目标。二进制壳 `bin_main.rs::main` 调用 `astersql_br_cmd_br::main()`，因此本文件承担“可执行目标、库模式和 crate 内测试共享同一模块树与入口”的适配职责。

该文件是门面和装配层：它公开命令模块及迁移期桩模块，并将 crate 根的 `main()` 薄转发到以 `#[path = "main.rs"]` 引入的 `entry` 模块。真正的进程启动、命令树构造和执行逻辑位于 `br/cmd/br/main.rs::main`。

## 核心职责

1. 用 crate 级 `#![allow(...)]` 放宽迁移代码的命名、未使用项和 Clippy 检查，使仍保留 Go 命名习惯及阶段性桩的命令模块能够在同一个 crate 中编译。
2. 声明并公开十个生产模块：`stubs`、`cmd`、`abort`、`backup`、`debug`、`fips`、`entry`、`operator`、`restore`、`stream`。其中 `entry` 的物理文件是 `main.rs`，其余模块名与物理文件名一致。
3. 仅在测试构建中引入六个独立测试模块：`parity_test`、`backup_test`、`cmd_test`、`debug_test`、`stream_test`、`main_test`，保持生产源码和测试源码分离。
4. 导出稳定的 `pub fn main()`，让 `bin_main.rs` 和 crate 内入口测试不必依赖内部模块别名。

本文件不解析参数、不注册子命令、不持有配置，也不实现备份、恢复或流式日志业务；这些行为由公开子模块负责。

## 主要符号

- `pub mod stubs`（`#[path = "stubs.rs"]`）：命令层迁移期依赖的本地瘦桩集合。它是公开模块，调用方可以使用其中的 CLI、操作系统及外部组件适配类型。
- `pub mod cmd`：公共 CLI 初始化、通用 flag、默认上下文、日志和 status server 相关逻辑。
- `pub mod abort`、`backup`、`debug`、`operator`、`restore`、`stream`：分别承载对应一级命令族；本文件只使模块进入 crate 并公开其符号。
- `pub mod fips`：FIPS 能力查询适配层。
- `pub mod entry`（`#[path = "main.rs"]`）：将物理文件 `main.rs` 映射为 `entry`，避免其 `main` 与 crate 根导出的 `main` 在路径表达上混淆。
- `mod parity_test`、`backup_test`、`cmd_test`、`debug_test`、`stream_test`、`main_test`：全部受 `#[cfg(test)]` 约束，不进入普通库或二进制构建。
- `pub fn main()`：本文件唯一函数，无参数、无返回值，函数体只有 `entry::main()`；它不截获错误，也不改变入口行为。

本文件没有常量、类型、trait、`impl`、宏定义或自身状态。

## 执行流程

生产进程的入口链为：

1. Cargo 构建 `[[bin]]` 指向的 `br/cmd/br/bin_main.rs`。
2. `bin_main.rs::main` 调用公开库函数 `astersql_br_cmd_br::main()`。
3. 本文件的 `main()` 无条件调用 `entry::main()`。
4. `br/cmd/br/main.rs::main` 创建后台上下文和退出信号监听器，定义根命令与公共 flag，关闭 BR 进程内的 TiDB DDL 行为，按 `debug`、`backup`、`restore`、`stream`、`operator`、`abort` 的顺序注册一级命令，传入 `argv[1..]` 并执行命令。
5. 根命令执行失败时，`entry::main` 记录错误并经 OS 适配层退出；正常返回时由其 `CancelOnDrop` 释放取消回调。本文件对此不增加任何分支或清理动作。

测试构建另有一条入口：`main_test.rs::test_run_main` 在线程中调用 `crate::main()`，验证该共享入口能够返回。因为测试模块声明在 crate 根，它能直接访问 `crate::main` 和其他 crate 内模块。

## 数据与状态

本文件自身没有字段、容器、缓存或全局可变状态。它建立的是编译期模块图和符号可见性：十个生产模块为 `pub`，六个测试模块为私有且仅在 `cfg(test)` 下存在。

运行时数据完全透传给 `entry::main` 所触发的流程。默认上下文、根命令、进程参数、全局配置以及退出状态均在 `main.rs` 或其下游模块中创建和管理。`pub fn main()` 的 `()` 返回类型意味着错误不会作为 `Result` 跨过这个门面；当前入口契约是在下游记录并处理失败。

## 依赖与调用关系

上游调用者有两类直接证据：

- `br/cmd/br/bin_main.rs::main -> astersql_br_cmd_br::main`：生产二进制调用库入口。
- `br/cmd/br/main_test.rs::test_run_main -> crate::main`：Rust 独立测试验证共享入口可返回。

本文件唯一运行时下游边是 `lib.rs::main -> main.rs` 中的 `entry::main`。其余关系均为编译期模块包含边：各 `pub mod` 将同目录文件纳入 crate，`entry` 通过显式 `#[path]` 重命名物理文件。

`Cargo.toml` 表明该 crate 直接依赖三个 BR 任务/跟踪 crate、streamhelper 配置 crate，以及 `hex`、`serde`、`serde_json`、`sha2`；测试额外依赖 `astersql-util-memory`。这些依赖由子模块使用，`lib.rs` 本身没有 `use` 声明，也不直接调用它们。Cargo 的 `package.metadata.porting` 将 Go 包标为 `br/cmd/br`、种类标为 `binary`，说明库目标主要是 Rust 的复用边界，最终产品角色仍是 BR 命令行程序。

## 错误处理与边界

`lib.rs::main` 不返回 `Result`、不捕获 panic、也不改写错误；`entry::main` 的行为原样成为公开入口行为。当前直接下游在根命令执行失败时记录 `br failed` 并调用 `os::Exit(1)`，所以扩展者不能假定库入口会把 CLI 错误返回给调用者。

crate 级 `allow` 范围覆盖整个模块树，会隐藏未使用项、Go 风格命名和全部 Clippy lint。它服务于迁移兼容，但也降低静态检查发现问题的能力；新增原生 Rust 代码时不应把该豁免理解为推荐风格。

模块声明要求对应路径在编译时存在。测试模块仅在测试配置下解析，普通生产构建不会包含这些测试代码。`lib.rs` 没有 feature 条件；Cargo 清单也没有为此 crate 声明 feature，因此模块集合除 `cfg(test)` 外固定。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源，也没有 `unsafe` 代码。薄转发调用是同步的：`entry::main()` 返回后，crate 根 `main()` 立即返回。

进程级资源生命周期属于直接下游：`main.rs::main` 启动退出信号监听器，并用 `CancelOnDrop` 在正常返回时调用取消函数；执行失败走 `os::Exit(1)` 前会遗忘守卫，以贴合 Go 中 `os.Exit` 不运行 `defer` 的语义。`main_test.rs::test_run_main` 另起线程并通过同步通道等待 `crate::main()` 返回，这是测试入口生命周期的证据，而不是 `lib.rs` 自身的并发行为。

## 与 Go 版本的对应关系

Go 的 `br/cmd/br` 使用同一个 `package main` 横跨 `main.go`、`cmd.go`、`backup.go` 等文件，不需要显式 crate 根。Rust 的 `lib.rs` 因模块系统和“库 + 薄二进制”布局而新增，故没有逐行对应的 Go 文件；它把 Go 包级文件集合显式映射为 Rust 模块集合。

行为对应集中在转发目标：Rust `entry::main` 对齐 Go `main.go::main` 的后台上下文、退出监听、根 Cobra 命令属性、公共 flag、默认上下文、禁用 DDL、六个一级子命令、stdout、`os.Args[1:]` 与失败退出流程。Rust crate 根只提供额外的一层稳定调用边，不应被误解为重复实现 Go 主流程。

测试方面，Go `main_test.go::TestRunMain` 清理测试参数后在 goroutine 中运行 `main()` 并等待；Rust `main_test.rs::test_run_main` 验证相同的参数过滤意图，并在线程中调用 `crate::main()`、通过通道等待返回。Rust 测试还覆盖内存上限公式和全局内存仲裁器清理，但这些属于 `cmd`/测试装配契约，不是 `lib.rs` 内部算法。

## 扩展指南

- 新增一级命令时，业务模块及命令构造器应放在独立 `.rs` 文件中；需要跨 crate 使用时在这里新增 `pub mod`，并在 `main.rs::main` 的 `rootCmd.AddCommand` 中完成注册。只添加模块声明不会让命令出现在 CLI 中。
- 若只是扩展现有命令，应修改对应的 `backup.rs`、`restore.rs` 等实现，而不是向这个门面添加业务逻辑。
- 若改变公开启动契约，需同步核对 `bin_main.rs`、`main.rs`、`main_test.rs` 以及 Go `main.go`/`main_test.go`。应优先保持 `lib.rs::main` 为无状态薄转发，避免库调用与二进制调用产生两套初始化顺序。
- 新测试应继续置于独立测试文件，并通过 `#[cfg(test)]` 与 `#[path = "..._test.rs"]` 接入；不要把测试逻辑内嵌到生产源文件。入口行为回归最接近 `main_test.rs`，模块间 Go/Rust 合约回归最接近 `parity_test.rs`。
- 若要让错误可由库调用者处理，不能只改变本文件返回类型；必须一起设计 `entry::main`、二进制退出码及 Go 对齐边界，否则会破坏现有 `os::Exit(1)` 契约。
- crate 级 lint 豁免影响所有子模块。缩小或移除豁免前应逐模块评估迁移代码，而不要在本门面一次性收紧后造成与本功能无关的大范围变更。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/cmd/br` 确认目标及相邻入口、测试均已索引。
- RustCodeGraph `node --file br/cmd/br/lib.rs --offset 1 --limit 240`：确认文件共 88 行、十个公开生产模块、六个条件测试模块、唯一函数及 `entry::main()` 调用。
- RustCodeGraph `node` 读取 `br/cmd/br/bin_main.rs`、`br/cmd/br/main.rs`、`br/cmd/br/main_test.rs`：确认生产调用链、根命令装配、错误/取消生命周期以及 `crate::main()` 的独立测试。
- `br/cmd/br/Cargo.toml`：确认 package 名、`[lib]`/`[[bin]]` 边界、porting 元数据、直接依赖与测试依赖。
- `br/cmd/br/main.go` 与 `br/cmd/br/main_test.go`：核对 Go 主入口顺序、失败退出行为和入口测试意图。
- `rg` 对 `astersql_br_cmd_br::main`、`crate::main()`、`entry::main()` 的引用检查：确认目标 crate 的直接生产调用者、测试调用者和唯一下游转发边。
- 本任务只增加说明文档，没有运行 Cargo；最终使用任务指定命令确认文档恰有十一个固定二级章节，并人工检查源码链接及引用路径存在。
