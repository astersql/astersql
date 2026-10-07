# `cmd/benchdb/lib.rs`

## 文件定位

[`cmd/benchdb/lib.rs`](lib.rs) 是 Cargo 包 `astersql-cmd-benchdb` 的库根，而不是 benchmark 业务逻辑的实现文件。`cmd/benchdb/Cargo.toml` 的 `[lib] path = "lib.rs"` 把它声明为库目标，`[[bin]] path = "bin_main.rs"` 则声明同名二进制目标；二进制入口 `bin_main.rs::main` 调用公开的 `astersql_cmd_benchdb::main`，由本文件把控制权继续交给 `entry::main`。

该文件还是三个编译模块的装配点：用 `#[path = "stubs.rs"]` 暴露 `stubs`，用 `#[path = "main.rs"]` 暴露 `entry`，并仅在测试构建中用 `#[cfg(test)]` 纳入 `parity_test.rs`。因此它的角色是“crate 门面 + 模块接线 + 统一入口”，真实作业解析、初始化和 SQL benchmark 流程位于 [`cmd/benchdb/main.rs`](main.rs)。

## 核心职责

1. 建立稳定的公共模块路径：调用者可通过 `astersql_cmd_benchdb::stubs` 和 `astersql_cmd_benchdb::entry` 访问两个公开模块（符号 `pub mod stubs`、`pub mod entry`）。
2. 为 Cargo 二进制提供单一库入口：`pub fn main()` 只调用 `entry::main()`，使二进制包装层和库测试共享同一份主流程。
3. 为单独测试文件提供 crate 内部上下文：`parity_test` 在 `cfg(test)` 下编入，可以用 `crate::entry`、`crate::stubs` 检查 Rust/Go 对齐合同；生产构建不包含该模块。
4. 在 crate 根统一放宽迁移代码的 lint：`#![allow(...)]` 覆盖 dead code、Go 风格命名、未使用项和全部 Clippy lint。这是编译策略而非运行时行为，也意味着新增代码可能不会得到通常的 lint 提醒。

本文件不解析参数、不创建 store/session、不生成 SQL，也不直接管理资源；这些职责均在 `entry` 与 `stubs` 中。

## 主要符号

- `pub mod stubs`（`lib.rs:19-20`）：把 `stubs.rs` 映射为公开模块。该模块提供 `Flags`、`SessionFactory`、记录型 session、store/log/flag 等边界替身；`Cargo.toml` 的依赖表为空，说明当前 Rust crate 的这些边界由本地实现承接，而非链接完整 TiDB/TiKV Rust 依赖。
- `pub mod entry`（`lib.rs:22-25`）：把文件名为 `main.rs` 的实现映射到模块名 `entry`。路径属性很重要：代码中的规范访问名是 `crate::entry`，不是由文件名自然推导出的 `crate::main`。
- `mod parity_test`（`lib.rs:27-29`）：私有、仅测试模块，对应独立文件 `parity_test.rs`，满足测试逻辑不内嵌生产源文件的仓库约束。
- `pub fn main()`（`lib.rs:31-36`）：无参数、无返回值的公开转发函数，函数体唯一动作是 `entry::main()`。它不捕获 panic、不转换退出码，也不执行清理。
- crate 级 `#![allow(...)]`（`lib.rs:9-17`）：对整个 crate 生效，允许移植代码保留 Go 风格标识符与暂未使用的兼容面。

本文件没有常量、结构体、枚举、trait、`impl` 块或 feature 条件；唯一条件编译项是 `parity_test`。

## 执行流程

生产入口的完整薄包装链如下：

1. Cargo 根据 `cmd/benchdb/Cargo.toml` 构建 `[[bin]]`，进入 `cmd/benchdb/bin_main.rs::main`。
2. `bin_main.rs::main` 调用 `astersql_cmd_benchdb::main()`，即本文件的公开函数。
3. `lib.rs::main` 同步调用 `entry::main()`。
4. `cmd/benchdb/main.rs::main` 从环境读取参数，经 `stubs::parse_flags` 解析并打印默认值，然后调用 `run_with_flags(flags, SessionFactory::default())`。
5. `run_with_flags` 初始化日志与 TiKV 类型注册，创建 `BenchDB`，按 `|` 拆分 `run_jobs`，再分派 `create`、`truncate`、`insert`、两种 update 拼写、`select` 或 `query`。未知作业记录消息后立即停止流水线。

测试构建走另一条装配路径：编译器因 `cfg(test)` 加载 `parity_test.rs`；该文件直接引用 `crate::entry::{...}` 与 `crate::stubs::{...}`，绕过进程参数来验证默认值、作业顺序、SQL 副作用、错误和关闭行为。除帮助参数测试显式启动测试子进程外，测试不会经过 `lib.rs::main`。

## 数据与状态

本文件自身没有可变静态量、堆对象或持久状态。模块声明只在编译期建立名称和可见性，`lib.rs::main` 也不保存返回值。

运行状态从下一层开始产生：`entry::main` 创建 `Flags`，`run_with_flags` 创建 `BenchDB`；`BenchDB` 保存 `Storage`、`RecordingSession` 和 flags 快照。由于本文件把 `stubs` 公开，外部 Rust 调用者理论上能访问这些桩类型；改变模块可见性属于公共 API 变更。测试模块则是私有且条件编译的，不进入生产产物。

## 依赖与调用关系

上游调用者和构建接线：

- `cmd/benchdb/bin_main.rs::main → astersql_cmd_benchdb::main`：由二进制薄包装源码直接确认。
- 根 `Cargo.toml` workspace 成员包含 `cmd/benchdb`；局部 `Cargo.toml` 同时声明库和二进制目标，并在 `package.metadata.porting` 中记录 Go 包 `cmd/benchdb`、类型 `binary`。
- `cmd/benchdb/parity_test.rs` 由本文件的 `mod parity_test` 纳入测试构建，并直接依赖 `entry` 与 `stubs`。

下游关系：

- `lib.rs::main → entry::main`。
- `entry::main → stubs::{args_from_env, parse_flags, print_defaults}`，随后进入 `entry::run_with_flags`。
- `entry::run_with_flags` 再连接日志、store 注册、session factory 和各 benchmark 作业；这些是为了说明入口最终到达何处，并不表示它们由 `lib.rs` 自己实现。

RustCodeGraph 将 `lib.rs` 识别为 36 行、2 个符号的文件，并能精确定位 `cmd/benchdb/lib.rs::main` 及 `cmd/benchdb/main.rs::main`。其同名 `main` 的 callers/callees 查询在当前索引中发生消歧限制：对 `lib.rs::main` 报告“无 callees”，也未返回 caller；因此薄包装边以精确源码与 Cargo 目标声明为准，而不将图中的缺边解释成入口未接线。

## 错误处理与边界

`lib.rs::main` 没有 `Result` 返回值和错误分支。`entry::main()` 正常返回时它随即返回；下层发生 panic、进程退出或其他不返回行为时，本层原样传播，不捕获、不包装，也不会追加诊断信息。

边界风险主要来自装配：

- `#[path]` 依赖三个相对文件仍位于同目录；移动或改名时必须同步属性。
- `parity_test` 只能在测试构建中引用；不能把生产代码建立在该模块上。
- crate 级 `clippy::all` 与未使用项豁免可能掩盖新问题，扩展时应依靠针对性审查和测试，而不能假设 lint 会提示所有风险。
- 当前局部 `Cargo.toml` 没有外部依赖，注释明确说明使用本地 stubs 覆盖 SQL session、store、日志与 flags 边界。因此这里呈现的是移植/对齐用可执行装配，不能仅凭 `TiKVDriver` 等名称宣称已连接真实 TiKV 客户端。

具体输入错误和 SQL 错误在下层转成 fatal/panic；`parity_test.rs::contract_error_paths` 覆盖非法范围、非法整数、未知 flag 和执行失败。它们不是本文件新增的错误策略，本文件只保持传播路径不变。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或显式资源句柄。两次入口调用都是同步调用，生命周期完全嵌套：二进制入口等待库入口，库入口等待 `entry::main`。

真实资源由下层创建和消费：`new_bench_db` 组装 store/session，作业函数执行事务与查询；`BenchDB::must_exec` 在成功路径读空并关闭结果集。`parity_test.rs::contract_resource_cleanup` 验证正常查询恰好关闭一次结果集、关闭失败进入 fatal/panic。由于 `lib.rs::main` 没有 `catch_unwind` 或清理守卫，它不会改变这些下层生命周期语义，也不能补偿 fatal 路径跳过的清理。

## 与 Go 版本的对应关系

Go 同路径只有 `cmd/benchdb/main.go` 的 `package main`，没有与 `lib.rs` 一一对应的库门面。Rust 为适配 Cargo 的“可复用库 + 极薄二进制”结构，额外引入了 `lib.rs` 和 `bin_main.rs`：两层转发最终仍只进入一份 `main.rs` 主流程。

语义对应关系是：Go `main.go::main` 约等于 Rust `entry::main + run_with_flags`，而不是 `lib.rs::main` 本身。Rust 的 `entry` 保留 Go 的参数默认值、初始化顺序、`runJobs` 拆分、作业分派和未知作业停止策略；`parity_test.rs::go_rust_public_contract_matches` 聚合正常路径、解析/批次边界、致命错误和资源清理检查。Rust 特有差异包括：依赖通过 `stubs.rs` 本地模拟，fatal 主要以 panic 表达以便测试，并将 Go 的包级 flags 收拢进 `Flags`/`BenchDB`。

默认作业串含 `gc`，但 Rust/Go 当前 switch 都没有 `gc` 分支；对齐测试明确验证它作为未知作业会停止后续流水线。文档因此只描述当前行为，不推断 GC 作业已经实现。

## 扩展指南

- 新增 benchmark 作业时，业务修改应落在 `cmd/benchdb/main.rs::run_with_flags` 及对应 `BenchDB` 方法，并同步 Go `cmd/benchdb/main.go` 的实际语义；不要把分派或 SQL 逻辑塞入 `lib.rs::main`。
- 新增外部系统边界时，应先扩展 `stubs.rs` 的最小接口，再在 `main.rs` 接线；若引入真正外部依赖，需同步审查 `cmd/benchdb/Cargo.toml`，并明确当前 arm64/轻依赖设计是否仍成立。
- 若需要新增公共入口，可在库根薄转发或从 `entry` 选择性再导出，但应评估 `pub mod entry`、`pub mod stubs` 已形成的公共 API 兼容性。
- 修改模块文件名或目录布局时，同步更新三个 `#[path]`、Cargo 的 `[lib]/[[bin]]` 路径和源码链接。
- 测试应继续写在独立的 `cmd/benchdb/parity_test.rs`（或同目录新的独立 `*_test.rs`），并由 `cfg(test)` 模块接入；至少覆盖二进制到库入口的可达性，以及新增作业的 Go/Rust 可观察合同。不要把测试逻辑内嵌进 `lib.rs`。
- 如收紧 crate 级 allow 列表，应先检查全部移植符号，避免一次 lint 策略变化造成无关的大范围重命名；反之新增 allow 时要说明为何必须作用于整个 crate。

兼容性风险集中在公共模块路径与入口签名；正确性风险集中在转发到错误模块或测试模块意外进入生产构建；本文件本身没有性能热点，额外逻辑只会给进程启动增加固定开销，但若把业务逻辑移入门面会破坏当前单一实现和测试复用结构。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；本次查询时索引可用。
- RustCodeGraph `files --filter cmd/benchdb`：确认索引包含 `bin_main.rs`、`lib.rs`、`main.rs`、`parity_test.rs`、`stubs.rs` 与 Go `main.go`。
- RustCodeGraph `node --file cmd/benchdb/lib.rs --offset 1 --limit 240`：读取目标文件全部 36 行，确认 crate 属性、三个模块声明和唯一函数。
- RustCodeGraph `node cmd/benchdb/lib.rs::main`：确认公开签名及唯一语句 `entry::main()`。
- RustCodeGraph 对 `cmd/benchdb/lib.rs::main` 和 `cmd/benchdb/main.rs::main` 的 callers/callees 查询：确认 `main.rs::main → run_with_flags` 可见，同时记录 crate 根薄包装边在当前图中未解析及同名消歧限制。
- 已读直接源码：`cmd/benchdb/bin_main.rs`、`cmd/benchdb/main.rs`、`cmd/benchdb/parity_test.rs`、`cmd/benchdb/main.go`。
- 已读构建证据：`cmd/benchdb/Cargo.toml`；另用 `rg` 确认根 `Cargo.toml` 的 workspace 成员包含 `cmd/benchdb`，并核对同目录 Rust 引用。
- 独立测试证据：`parity_test.rs` 的 `go_rust_public_contract_matches`、`negative_count_matches_go_integer_range_semantics`、`lone_dash_stops_flag_parsing_like_go`、`help_flag_exits_successfully_without_running_jobs`，以及其正常、边界、错误、资源清理子合同。
- 本任务只生成文档，按计划不运行 Cargo。交付前以任务规定的 11 标题命令做结构验证，并人工复核本文件定位、调用链、当前桩边界和扩展落点均有上述源码或配置依据。
