# `cmd/importer/lib.rs` 逻辑说明

## 文件定位

`cmd/importer/lib.rs` 是 Cargo 包 `astersql-cmd-importer` 的库 crate 根，也是 Rust importer 的装配门面。`cmd/importer/Cargo.toml` 通过 `[lib] path = "lib.rs"` 选择本文件，并通过 `[[bin]] path = "bin_main.rs"` 构建同名可执行文件；`cmd/importer/bin_main.rs::main` 再调用 `astersql_cmd_importer::main`。因此真实启动链是 `bin_main.rs::main` → `lib.rs::main` → `entry::main`，而业务编排位于由 `#[path = "main.rs"] pub mod entry` 引入的 `cmd/importer/main.rs`，不在本文件中。

该包的 `package.metadata.porting` 将 Go 来源标为 `cmd/importer`、迁移形态标为 `binary`。本文件自身共 87 行，只声明模块、条件测试模块、crate 级 lint 宽免和一个公开转发函数；它不是数据生成、SQL 解析或并发导入的实现位置。

## 核心职责

本文件承担三个边界职责：

1. 用显式 `#[path = "..."]` 声明组装 importer 的生产模块：`stubs`、`config`、`data`、`rand`、`stats`、`parser`、`db`、`job` 和别名为 `entry` 的 `main.rs`。
2. 在 `cfg(test)` 下把七个独立测试文件接入同一 crate，使测试能够通过 `crate::...` 访问生产模块，同时遵守“测试逻辑不内嵌生产源文件”的布局。
3. 以公开 `main()` 为库与二进制之间的稳定单跳入口，把控制权交给 `entry::main()`。

crate 根没有解析参数、创建连接、维护表元数据或启动 worker。任何关于导入行为的修改都应先定位到相应子模块；仅当模块暴露关系、测试接线或进程入口边界变化时才应修改本文件。

## 主要符号

- crate 属性 `#![allow(...)]`：集中容忍迁移代码中的 Go 风格命名、暂未使用符号和 Clippy 告警。它作用于整个 crate，会降低编译器告警对迁移代码的噪声，但也可能掩盖新增死代码，因此扩展时不能把它当作无需审查的理由。
- `pub mod stubs`：引入 `stubs.rs`，提供当前 importer 所依赖的本地兼容类型和运行时适配；Cargo 注释明确说明该包刻意避免重型 TiDB crate。
- `pub mod config`、`data`、`rand`、`stats`、`parser`、`db`、`job`：分别公开配置解析、数据生成、随机辅助、统计加载、DDL 解析、数据库操作和任务调度模块。它们对应同目录的 `.rs` 文件，也与 Go 包内的同名 `.go` 文件形成迁移对照。
- `pub mod entry`：以 `#[path = "main.rs"]` 将真实进程编排模块暴露为 `entry`，避免与 crate 根的 `main()` 混淆。`cmd/importer/main.rs` 公开 `entry::main()` 和可注入数据库句柄的 `entry::run_with_args(...)`。
- `mod parity_test`、`db_test`、`config_test`、`data_test`、`parser_test`、`rand_test`、`stats_test`：只在测试构建中存在的私有模块，对应七个独立 `*_test.rs` 文件。
- `pub fn main()`：本文件唯一函数和公开进程入口，无参数、无返回值，函数体仅调用 `entry::main()`。

本文件没有模块级常量、类型、trait、`impl`、状态字段或其他条件编译项。

## 执行流程

生产执行从 `cmd/importer/bin_main.rs::main` 开始，该函数调用库名路径 `astersql_cmd_importer::main()`。本文件的 `main()` 随即调用 `entry::main()`；`entry` 是 `cmd/importer/main.rs` 的模块别名。

后续流程由 `cmd/importer/main.rs` 负责：`entry::main` 从环境取得参数并调用 `run_with_args`；后者依次解析配置、解析建表和索引 SQL、创建或接收数据库句柄、可选加载统计信息、执行 DDL、调用 `job::doProcess` 导入数据，最后调用 `db::closeDBs`。帮助参数与非法参数分别映射到退出码 0 和 2，其他解析、连接、统计或 SQL 错误沿该模块的 fatal 边界终止。上述细节是下游 `entry` 的行为，不应误归为 crate 根的内部实现。

测试构建不会改变生产入口函数，而是在编译 crate 时额外装入七个测试模块。`parity_test.rs` 直接调用 `crate::entry::run_with_args`，绕过环境参数来源并注入内存数据库，从而验证完整编排；各专项测试则直接验证相应公开子模块。

## 数据与状态

`lib.rs` 自身不持有可变全局状态、配置实例、数据库连接、通道、线程句柄或表结构。它传递的唯一运行时控制流是无参数的 `main()` 调用。

真正的数据边界在下游模块中：`entry::run_with_args` 构造配置和表元数据，将表包装为 `Arc` 后交给 `job::doProcess`，并接收 `Vec<DB>` 作为连接集合。由于这些对象既不在本文件创建也不在本文件销毁，crate 根不能保证它们的内容不变量；它只保证生产入口始终经过同一 `entry::main`。

条件编译状态也很简单：非测试构建只含生产模块；测试构建额外含七个私有测试模块。它们不是公开 API，外部 crate 无法通过库接口访问。

## 依赖与调用关系

上游直接调用者是 `cmd/importer/bin_main.rs::main`，源码调用 `astersql_cmd_importer::main()`；Cargo 中包名的连字符在 Rust 路径里转换为下划线。仓库搜索没有发现其他 Rust 文件调用这个库入口。RustCodeGraph 将 `cmd/importer/lib.rs` 识别为含两个节点的文件（文件节点与 `main` 函数节点），但没有为薄转发体解析出 `entry::main` 调用边；该缺失与源码中明确的调用语句冲突，因此应视为索引限制，而不是“没有下游调用”。

下游直接依赖是 `entry::main()`。通过模块声明形成的编译依赖还包括同目录九个生产文件。它们的职责边界为：

- `config.rs`：配置默认值、TOML 和命令行解析；
- `parser.rs`：建表与索引 SQL 解析及表元数据装配；
- `stats.rs`：统计文件和直方图适配；
- `data.rs`、`rand.rs`：行数据及随机值生成；
- `db.rs`：连接、DDL/DML 和批量数据相关操作；
- `job.rs`：任务切分与 worker 调度；
- `stubs.rs`：迁移期本地兼容层；
- `main.rs`：上述组件的进程级编排。

`cmd/importer/Cargo.toml` 只有 `toml = "0.8"` 与 `serde_json = "1"` 两个外部依赖。crate 根不直接引用它们；它们由配置和统计等子模块消费。Cargo 注释明确说明 arm64 Darwin 场景不引入 kv/domain/kvproto/grpcio 等重型 TiDB 依赖，本地 `stubs` 是当前边界的一部分。

## 错误处理与边界

`lib.rs::main` 不返回 `Result`、不捕获 panic，也不转换错误；所有错误语义均委托给 `entry::main`。因此在 crate 根添加吞错、重试或退出码映射会改变现有边界，且会使二进制入口与测试可调用入口产生不必要分叉。

直接证据来自 `cmd/importer/main.rs`：参数帮助返回代码 0，其他参数错误返回代码 2 并记录错误；`entry::main` 用 `stubs::os_exit` 应用退出码；DDL 解析、连接创建、统计加载及 SQL 执行失败进入 `stubs::fatal`。`parity_test.rs::process_exit_codes_match_go_main` 验证 0/2 退出码，`contract_error_paths` 验证坏 SQL、表名不匹配、未知 flag、未支持类型和 SQL 执行 fatal 路径。

一个重要下游前置条件是 `run_with_args` 在执行 DDL 时索引 `dbs[0]`：生产路径的 worker 数量和测试注入连接集合必须保证至少一个数据库句柄。`createDBs(..., 0)` 虽可返回空集合，但该边界测试没有把空集合传入完整主流程。该风险属于 `entry::run_with_args`，不是 crate 根可自行修补的行为。

## 并发与资源生命周期

crate 根不创建线程、锁、通道或 `Arc`，也没有异步运行时。它对资源生命周期的唯一影响是同步、无分支地把进程控制权转交给 `entry::main`，所以入口返回之前不会在本层进行清理。

并发和资源行为位于下游：`entry::run_with_args` 在表元数据与可选统计信息装配完成、DDL 成功后，才以 `Arc<table>` 调用 `job::doProcess`；导入结束后显式 `closeDBs`。`parity_test.rs::contract_normal_path` 验证注入的两个数据库最终关闭且 DDL 与 INSERT 均执行，`contract_resource_cleanup` 验证 `doProcess` 自身不关闭句柄、批次完成后由 `closeDBs` 统一关闭，`worker_database_failure_does_not_deadlock` 验证 worker 启动失败不会让等待方死锁。

如果未来在 `lib.rs::main` 外再包一层后台任务或运行时，必须明确谁等待 importer 完成、谁拥有连接清理责任，以及 fatal/进程退出是否仍能执行清理；当前同步单跳模型没有这些额外生命周期问题。

## 与 Go 版本的对应关系

Go 的 `cmd/importer` 是单一 `package main`，`main.go::main` 直接完成配置解析、DDL 解析、连接创建、统计装配、DDL 执行和 `doProcess` 调用。Rust 版本将同一包拆成 Cargo 库和薄二进制：`bin_main.rs` 替代最外层可执行壳，`lib.rs` 提供共享入口及模块树，`main.rs` 保存 Go `main.go` 的真实流程。这个拆分使二进制与 Rust 对齐测试可以共享相同生产实现，同时不改变 Go 流程的顺序。

模块对应关系为 `config.rs` ↔ `config.go`、`data.rs` ↔ `data.go`、`rand.rs` ↔ `rand.go`、`stats.rs` ↔ `stats.go`、`parser.rs` ↔ `parser.go`、`db.rs` ↔ `db.go`、`job.rs` ↔ `job.go`，而 `entry`/`main.rs` ↔ `main.go`。`stubs.rs` 是 Rust 为降低依赖重量而存在的迁移兼容层，没有同名 Go 生产文件。

Go `main.go` 用 `defer closeDBs(dbs)` 注册连接回收，Rust `run_with_args` 在正常主流程末尾显式调用 `closeDBs(&dbs)`；在 fatal 语义下两者都以进程终止为边界，但测试以 panic 模拟 fatal。Go 索引统计时直接取首个索引列，Rust 入口还检查空索引列和列偏移范围；这些差异位于 `main.rs`，此处只记录为入口委托后可观察的迁移现状，不能据此声称 crate 根实现了额外保护。

## 扩展指南

- 新增独立 importer 能力时，优先在对应业务模块实现，并在 `main.rs::run_with_args` 接线；只有新增源模块或需要对外暴露 API 时才在本文件增加 `#[path] pub mod ...`。
- 新增模块应沿用同目录独立测试文件，在 `cfg(test)` 下接入；不要把测试函数写进 `lib.rs` 或生产模块。若只是扩展现有模块，优先扩展现有 `config_test.rs`、`data_test.rs`、`db_test.rs`、`parser_test.rs`、`rand_test.rs`、`stats_test.rs` 或跨模块的 `parity_test.rs`。
- 修改启动流程时必须保持 `bin_main.rs::main` → `lib.rs::main` → `entry::main` 单一入口，或同步说明为何需要新入口；避免在二进制壳和库根复制参数解析与退出码逻辑。
- 修改模块可见性前评估外部 API 兼容性。当前九个生产模块均为 `pub`，测试模块均为私有且仅测试编译；将公开模块改私有可能破坏外部使用者，将测试模块公开则会扩大无必要 API。
- 新增依赖应继续遵守 `Cargo.toml` 的轻量边界，特别关注 arm64 Darwin 可构建性、二进制体积和是否把重型 TiDB 子系统带入 importer。
- 调整 lint 宽免时要先检查整个 crate 的迁移命名和未使用符号；收紧可能产生大范围告警，继续放宽则可能隐藏真实缺陷。
- 入口或错误边界变更至少同步 `parity_test.rs` 的退出码、完整流水线、fatal 和资源回收断言；具体业务算法变更还需同步相应专项 Rust 测试及同路径 Go 测试意图。

主要风险是：入口分叉导致二进制与测试行为不一致；模块可见性变化造成兼容性破坏；连接/worker 接线变化造成清理遗漏或死锁；统计与数据生成接线变化影响性能和数据分布。crate 根本身没有热点算法，单跳转发的性能成本可忽略，真正的性能风险在 `job`、`db`、`data` 和 `stats`。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 7,032 个 Rust 文件；`files --filter cmd/importer` 列出目标、入口、九个生产模块和七个独立测试模块。
- RustCodeGraph 源码与符号：`node --file cmd/importer/lib.rs` 显示完整 87 行、九个公开生产模块、七个 `cfg(test)` 私有模块和唯一函数 `main`；`query main --kind function` 定位 `cmd/importer/lib.rs:85`、`bin_main.rs:8` 和 `main.rs:37` 的三个入口层次。
- RustCodeGraph 调用查询：`callees lib.rs::main` 对 `cmd/importer/lib.rs:85` 报告无边，而源码明确调用 `entry::main()`；文档据此将该结果标为索引限制，并以 `bin_main.rs`、`lib.rs`、`main.rs` 三处源码交叉验证调用链。
- Cargo 边界：读取 `cmd/importer/Cargo.toml`，确认库/二进制 target、Go 来源元数据、binary 迁移类型以及 `toml`、`serde_json` 两项依赖。
- Go 对照：读取 `cmd/importer/main.go`，核对配置解析、0/2 退出码、表/索引解析、数据库创建与关闭、统计装配、DDL 执行和 `doProcess` 顺序；同目录各同名 Go/Rust 文件提供模块级对应关系。
- Rust 测试：读取 `cmd/importer/parity_test.rs`，确认 `run_with_args` 的完整流水线、错误、并发失败和资源回收契约；读取 `cmd/importer/db_test.rs`，确认专项测试以独立文件接入。仓库搜索同时确认其余五个专项测试文件由本 crate 根声明，且外部 Rust 代码只在 `bin_main.rs` 调用库入口。
- Go 测试：`cmd/importer/db_test.go` 是同目录现有 Go 专项回归，Rust `db_test.rs` 明确按其表驱动样例验证 decimal 文本转换；同目录未发现 Go 的 `main` 专项测试，完整入口契约由 Rust `parity_test.rs` 覆盖。
- 结构校验要求：本文仅包含计划规定的十一个固定二级章节；交付前用任务文件给定的 `test`/`rg -c` 命令验证章节数量和文件存在性。

人工复核结论：本文件之所以存在，是为 Cargo 库/二进制拆分提供统一模块树与可复用进程入口；运行时仅作同步单跳转发；安全扩展应把行为放入对应子模块和 `entry::run_with_args`，并通过独立专项测试或 `parity_test.rs` 验证，而不是把业务逻辑堆入 crate 根。
