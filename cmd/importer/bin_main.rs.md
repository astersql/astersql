# `cmd/importer/bin_main.rs`

## 文件定位

`cmd/importer/bin_main.rs` 是 `astersql-cmd-importer` Cargo 包的二进制入口包装层。`cmd/importer/Cargo.toml` 的 `[[bin]]` 将二进制命名为 `astersql-cmd-importer`，并把本文件指定为入口；同一 manifest 的 `[lib] path = "lib.rs"` 则定义了它所调用的库 crate。Cargo 会把包名中的连字符转换为 Rust crate 标识符中的下划线，因此源码使用 `astersql_cmd_importer::main()`。

本文件不是 importer 业务流程的实现位置。库根 `cmd/importer/lib.rs::main` 继续转发到 `entry::main`（由 `#[path = "main.rs"]` 引入），实际的参数解析、DDL 解析、连接创建、可选统计加载、建表和并发导入编排位于 `cmd/importer/main.rs::run_with_args`。

文件当前只有模块文档和一个私有函数，没有模块级常量、类型、trait、`impl`、feature gate 或其他条件编译项。顶部的 `// Copyright 2026 AsterSQL.` 表明该 Rust 入口已经按仓库迁移约定处理。

## 核心职责

本文件只承担两个职责：

1. 为操作系统启动的 `astersql-cmd-importer` 可执行文件提供 Cargo 所要求的 `fn main()`。
2. 无条件把控制权交给库 crate 的共享入口 `astersql_cmd_importer::main()`，使二进制运行与库内测试/复用路径最终使用同一份 importer 编排逻辑。

这种薄包装避免在二进制层复制参数解析、退出码映射或资源回收逻辑。也因此，本文件本身不承诺具体导入行为；这些行为应以 `cmd/importer/lib.rs::main`、`cmd/importer/main.rs::main` 和 `run_with_args` 为准。

## 主要符号

- `fn main()`：文件唯一的函数，也是仅由运行时调用的二进制入口。它不是 `pub` API，没有参数和显式返回值。
- `astersql_cmd_importer::main()`：本函数唯一调用的库函数。它在 `cmd/importer/lib.rs` 中公开，函数体调用 `entry::main()`。
- 间接相关的 `cmd/importer/main.rs::main()`：从环境读取参数，调用 `run_with_args(&args, None)`，并把 `Err(code)` 交给 `stubs::os_exit(code)`。
- 间接相关的 `cmd/importer/main.rs::run_with_args(args, dbs_override)`：共享的可测试主流程；`dbs_override` 允许独立测试注入内存数据库句柄，但生产二进制始终传入 `None`。

RustCodeGraph 将本文件识别为 2 个节点，并将入口函数识别为 `cmd/importer/bin_main.rs::main`（第 8 行，签名 `fn main()`）。精确 `query` 能定位该符号；`callers`/`callees` 查询在本地索引上未能及时返回，因此调用边另由函数体、库根和 Cargo 声明直接核实。

## 执行流程

完整的入口链如下：

1. Cargo 按 `cmd/importer/Cargo.toml` 的 `[[bin]]` 构建并启动 `bin_main.rs`。
2. Rust 运行时调用本文件的 `fn main()`。
3. `fn main()` 同步调用 `astersql_cmd_importer::main()`，自身没有前置或后置分支。
4. `cmd/importer/lib.rs::main()` 同步调用 `entry::main()`。
5. `cmd/importer/main.rs::main()` 通过 `stubs::args_from_env()` 读取进程参数，再调用 `run_with_args(&args, None)`。
6. `run_with_args` 依次完成配置解析、表和索引 SQL 解析、数据库连接建立、可选统计加载、表和索引 DDL 执行、`doProcess` 并发导入以及 `closeDBs` 连接关闭。
7. 正常完成时控制逐层返回；参数错误由共享入口映射为退出码，其他 fatal 路径由共享桩终止流程。本包装层不改写这些结果。

关键不变量是本文件必须保持“单跳转发”：若在这里另行读取参数、初始化全局状态或捕获错误，二进制路径就可能与 `run_with_args` 的测试路径发生语义漂移。

## 数据与状态

本文件不声明、拥有或修改任何业务数据，也没有静态变量、缓存或配置对象。唯一可观察动作是一次普通函数调用。

命令行参数、配置、表元数据、数据库句柄和统计直方图都在下游创建和传递：`entry::main` 获取 `Vec<String>` 参数；`run_with_args` 创建配置和表描述；生产路径通过 `createDBs` 获取连接，并在流程结束时由 `closeDBs` 关闭。上述对象均不经过本文件的局部状态。

## 依赖与调用关系

上游是进程启动机制与 Cargo 二进制目标，而不是仓库内普通 Rust 调用者。`cmd/importer/Cargo.toml` 提供直接装配证据：包名为 `astersql-cmd-importer`，`[lib]` 指向 `lib.rs`，`[[bin]]` 指向 `bin_main.rs`。

直接下游只有 `astersql_cmd_importer::main()`；继续向下的已核实调用链为：

`bin_main.rs::main → lib.rs::main → main.rs::main → main.rs::run_with_args`。

再往下，`run_with_args` 调用 `config::NewConfig`/`Config::Parse`、`parser::{newTable, parseTableSQL, parseIndexSQL}`、`db::{createDBs, execSQL, closeDBs}`、`stats::loadStats` 和 `job::doProcess`。这些是理解完整应用位置所需的直接链路证据，不表示本包装层直接依赖所有这些符号。

manifest 中显式第三方依赖只有 `toml` 和 `serde_json`，由库内模块使用；本文件没有 `use` 声明，也不直接调用第三方 crate。Go Bazel 目标 `cmd/importer/BUILD.bazel` 只描述 Go `importer`，不是本 Rust 二进制的构建声明。

## 错误处理与边界

本文件没有 `Result`、`Option`、分支、日志或错误转换。`astersql_cmd_importer::main()` 返回 `()`，因此包装层既不捕获错误，也不自行选择退出码。

用户可观察的退出行为由 `cmd/importer/main.rs` 决定：帮助请求映射为退出码 0，其他参数解析错误记录日志并映射为退出码 2；DDL 解析、连接、统计加载或 SQL 执行失败走 `stubs::fatal`。`cmd/importer/parity_test.rs::process_exit_codes_match_go_main` 覆盖 0/2 退出码契约，fatal 路径通过 `panic::catch_unwind` 验证，但这些测试针对共享逻辑，不会直接执行本文件的 `fn main()`。

包装层边界也意味着它无法注入测试数据库：生产调用固定沿 `entry::main → run_with_args(..., None)` 前进。若未来需要改变进程退出策略，应优先修改并测试共享入口，不能只在 `bin_main.rs` 增加特殊处理。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或数据库连接；调用是同步的，生命周期等同于整个进程入口调用栈。

并发和资源所有权位于下游：`run_with_args` 在 DDL 成功后调用 `job::doProcess`，由 worker 执行导入；随后调用 `closeDBs` 关闭全部数据库句柄。`cmd/importer/parity_test.rs` 的完整流水线用两个内存 `DB` 验证 DDL/插入发生且连接最终关闭，`contract_resource_cleanup` 还验证 `doProcess` 自身不会关闭连接，而 `closeDBs` 才负责收口。这些约束解释了为何包装层不应提前创建或持有资源。

需要注意，Rust 主流程当前显式在成功路径末尾调用 `closeDBs`，而 Go 主流程在创建连接后使用 `defer closeDBs(dbs)`。这一区别属于共享主流程的资源清理语义，不由本文件处理。

## 与 Go 版本的对应关系

直接 Go 对照文件是 `cmd/importer/main.go`，其 `func main()` 同时承担进程入口和完整业务编排：解析 `os.Args[1:]`，处理帮助/非法参数退出码，解析 DDL，创建连接，加载统计，执行 DDL，调用 `doProcess`，并用 `defer closeDBs` 回收连接。

Rust 为可测试性把同一语义拆成三层：本文件提供进程入口，`lib.rs::main` 提供稳定的库门面，`main.rs::{main, run_with_args}` 承载 Go 对齐逻辑。因而不能只比较 Go `main.go` 与本文件的行数来判断迁移缺失；应沿转发链比较 `cmd/importer/main.rs`。

已核实的语义对齐包括帮助退出 0、非法参数退出 2、先解析表再解析索引、先执行 DDL 再启动导入，以及最后关闭连接。Rust 还提供 `dbs_override` 作为测试接缝；它不是 Go 用户接口，也不会被生产包装层启用。Rust 对空索引列增加了跳过保护、对列偏移增加了边界检查，这些差异存在于共享主流程而非本文件。

## 扩展指南

- 新增或修改 importer 业务行为时，应修改 `cmd/importer/main.rs::run_with_args` 或其下游模块，并在独立的 `cmd/importer/*_test.rs` 中补充测试；不要把业务逻辑塞入 `bin_main.rs`。
- 修改参数来源、进程退出映射或最外层初始化时，应优先保持 `bin_main.rs → lib.rs::main → entry::main` 单一路径，并同步扩展 `cmd/importer/parity_test.rs`。若确实需要包装层专属行为，应新增独立测试目标/集成测试，而不是在生产源文件内嵌 `#[cfg(test)]` 测试。
- 更改二进制名或入口文件时，需要同步检查 `cmd/importer/Cargo.toml` 的 `[[bin]]`；本文件改名而不更新 manifest 会使目标无法装配。
- 调整资源清理或并发初始化时，应在共享主流程中验证正常路径和 fatal 路径，重点防止连接泄漏、worker 在 DDL 前启动或测试注入路径与生产路径分叉。
- 兼容风险主要是 CLI 退出码和启动链漂移；性能风险在当前薄包装中可忽略。只有加入昂贵初始化、额外参数复制或阻塞操作后，包装层才会进入启动性能关键路径。

## 验证依据

- 源码：`cmd/importer/bin_main.rs`，确认只有私有 `fn main()` 和一次 `astersql_cmd_importer::main()` 调用，无条件编译项或本地状态。
- Cargo：`cmd/importer/Cargo.toml`，确认包/库边界、二进制名、`bin_main.rs` 入口和显式依赖。
- Rust 入口：`cmd/importer/lib.rs` 与 `cmd/importer/main.rs`，确认 `lib::main → entry::main → run_with_args` 以及实际导入顺序、错误映射和资源回收位置。
- Go 对照：`cmd/importer/main.go`，确认原版 `main` 的参数、DDL、统计、导入和连接生命周期语义。
- 独立测试：`cmd/importer/parity_test.rs`，确认共享入口的退出码、完整流水线、fatal 传播和连接回收；同目录没有专门执行 `bin_main.rs` 的 Rust 测试，Go 侧现有 `db_test.go` 也不测试进程包装层。
- 构建辅助证据：`cmd/importer/BUILD.bazel` 仅定义 Go binary/library/test；`cmd/importer/README.md` 记录 importer 的用户定位与 CLI 用法。
- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter cmd/importer` 覆盖本目录；`query main --kind function --limit 10000 --json` 定位 `cmd/importer/bin_main.rs::main`、库根 `main`、共享 `main` 与 `run_with_args`。对目标执行 `node/callers/callees` 时节点解析或响应超时，因此调用边使用已索引符号结果配合上述源文件逐层核验，没有据此臆测额外调用者。
- 结构验收使用任务指定命令，要求本文恰好包含以上 11 个固定二级标题；本任务为纯文档分析，按计划不运行 Cargo。
