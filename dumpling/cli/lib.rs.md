# `dumpling/cli/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-dumpling-cli` 的库入口；[`Cargo.toml`](Cargo.toml) 通过 `[lib] path = "lib.rs"` 明确把它设为 crate 根，并以 `package.metadata.porting.go-package = "dumpling/cli"` 记录其 Go 对照包。它自身不实现版本格式化或日志写入，而是用 `#[path = "versions.rs"] mod versions;` 挂载 [`versions.rs`](versions.rs)，再通过 `pub use versions::*;` 提供稳定的 crate 根 API。

生产调用主要有两条：[`dumpling/cmd/dumpling/main.rs`](../cmd/dumpling/main.rs) 通过 `astersql_dumpling_cli` 调用 `LongVersion` 生成命令行版本输出；[`dumpling/export/lib.rs`](../export/lib.rs) 将该 crate 引入为 `cli`，其 `dump.rs::initLogger` 在初始化内部 logger 后调用 `cli::LogLongVersion`。因此本文件位于 CLI/导出编排层与版本元数据实现之间，是接口门面而不是业务执行器。

## 核心职责

1. 用显式 `#[path = "versions.rs"]` 固定版本实现文件，避免目录调整改变隐式模块解析结果（`lib.rs::versions`）。
2. 用通配再导出把 `versions.rs` 的公开项提升到 crate 根，包括 `ReleaseVersion`、`BuildTimestamp`、`GitHash`、`GitBranch`、`RustVersion`、`LongVersion`、`LogLongVersion` 与 `reset_version_vars`（`lib.rs::pub use versions::*`；`versions.rs`）。
3. 仅在 `cfg(test)` 下挂载独立测试文件 [`parity_test.rs`](parity_test.rs)，保证测试逻辑不进入正式产物，也不与生产源文件混放。
4. 在 crate 级允许迁移期遗留的 Go 风格命名和未使用项。这里的 `#![allow(...)]` 只放宽诊断，不改变符号可见性或运行行为。

本文件不负责解析参数、注入构建信息、配置日志器或执行导出；这些职责分别位于 `dumpling/cmd/dumpling/main.rs`、构建流程/公开版本变量、`dumpling/log` 与 `dumpling/export`。

## 主要符号

- `mod versions`：私有子模块，源码由 `versions.rs` 提供。模块本身不公开，但其中全部 `pub` 项经下一条再导出。
- `pub use versions::*`：crate 的核心兼容门面。调用方使用 `astersql_dumpling_cli::LongVersion` 等路径，无需依赖内部文件布局。
- `mod parity_test`：只在测试配置下存在的私有测试模块，来源为 `parity_test.rs`。
- crate 级 `allow` 属性：允许 `dead_code`、Go 风格的非 snake/camel/upper-case 命名、未使用导入和变量，适应移植中的公开 API 命名。

经门面暴露、但实际定义在 `versions.rs` 的关键 API 是：

- 五个全局版本槽：四个 `RwLock<&'static str>` 和一个 `RwLock<Option<&'static str>>`。
- `LongVersion() -> String`：按固定五行布局生成版本文本。
- `LogLongVersion(&Logger)`：以 Info 级别写一条 `Welcome to dumpling` 结构化日志。
- `reset_version_vars()`：恢复默认元数据，主要用于隔离会修改全局槽的测试。

## 执行流程

命令行版本展示路径如下：

1. `dumpling/cmd/dumpling/main.rs` 将本 crate 引入为 `cli`。
2. `long_version_output` 调用门面导出的 `cli::LongVersion()`，并额外追加换行，以模拟 Go `println(cli.LongVersion())` 的字节结果。
3. `versions.rs::LongVersion` 依次读取发布版本、提交哈希、分支和构建时间；编译器版本优先使用 `RustVersion` 覆盖值，否则读取当前运行时 rustc 版本。
4. 返回固定标签、字段顺序和对齐空格的字符串；构建时间主体之后无条件追加 `Z`。

导出启动日志路径如下：

1. `dumpling/export/lib.rs` 把依赖包绑定为 `cli`，`NewDumper` 的初始化步骤列表首先执行 `dump.rs::initLogger`。
2. 若配置已提供 logger，`initLogger` 复用它并提前返回，不写欢迎日志；若未提供，则创建应用 logger。
3. 内部 logger 初始化成功后，`initLogger` 调用门面导出的 `cli::LogLongVersion(&d.tctx.L())`。
4. `versions.rs::LogLongVersion` 读取同一组全局版本槽，并向 logger 发出一条 Info 级欢迎日志及五个有序字段。

`lib.rs` 在上述流程中没有分支和计算；它的运行时作用是让两个调用方都解析到 `versions.rs` 的同一组公开定义。

## 数据与状态

本文件自身不保存运行时数据。它公开的共享状态全部定义在 `versions.rs`：`ReleaseVersion`、`BuildTimestamp`、`GitHash`、`GitBranch` 默认均为 `"Unknown"`，`RustVersion` 默认为 `None`。四个字符串槽只接受 `&'static str`，说明当前注入接口适合编译期常量或测试字面量，不接受任意短生命周期字符串。

`LongVersion` 和 `LogLongVersion` 每次调用都从锁中读取当前值，没有额外缓存。`reset_version_vars` 逐项取得写锁，恢复四个 `"Unknown"` 和 `RustVersion = None`。这些都是进程级共享状态：测试或构建接线若修改它们，会影响随后所有调用，必须在用例边界显式复位。

## 依赖与调用关系

- crate 边界：`dumpling/cli/Cargo.toml` 声明库名 `astersql-dumpling-cli`，直接依赖 `astersql-dumpling-log` 和 `rustc_version_runtime`。
- 下游实现：本门面只指向 `versions.rs`；其中 `LogLongVersion` 使用 `astersql_dumpling_log::{Logger, Field}`，`rust_version` 使用 `rustc_version_runtime::version()`。
- 上游命令：`dumpling/cmd/dumpling/main.rs::long_version_output -> cli::LongVersion -> versions.rs::LongVersion`。对应测试 `dumpling/cmd/dumpling/parity_test.rs::version_output_matches_go_builtin_println_bytes` 验证双换行契约。
- 上游导出：`dumpling/export/dump.rs::initLogger -> cli::LogLongVersion -> Logger::Info`；`dumpling/export/Cargo.toml` 以路径依赖 `../cli` 接入本 crate。
- 测试调用：`parity_test.rs::go_rust_public_contract_matches` 聚合默认格式、覆盖值/空值、日志等级过滤和状态清理四类契约，并通过 `use crate::{...}` 验证这些符号确实从 crate 根可见。

RustCodeGraph 将 `versions.rs` 标为被 `parity_test.rs` 与 `dumpling/export/dump.rs` 使用，并给出 `LogLongVersion <- dump.rs::initLogger`、`LongVersion <- cli parity tests` 等调用边。图对 `pub use` 门面的跨 crate 解析并不完整，因此 Cargo 路径依赖和源码中的 `use astersql_dumpling_cli as cli` 是补充证据。

## 错误处理与边界

`lib.rs` 没有 `Result`、错误分支或恢复逻辑。真实边界来自被再导出的实现：

- 所有读写锁都用 `expect`/`unwrap` 处理 poisoning；若某线程持有版本锁时 panic，后续读取、日志记录或复位会再次 panic，而不是返回可恢复错误。
- 空字符串是有效覆盖值：标签和换行仍保留，空构建时间仍产生 `Build timestamp: Z`。`parity_test.rs::contract_boundary_custom_values` 锁定了此行为。
- `RustVersion = None` 时动态读取实际 rustc 版本；`Some("")` 则保留空值，不回退。
- `LogLongVersion` 不返回错误。Error 级过滤器会抑制 Info 欢迎日志，nop logger 也不会产生捕获项；相关契约由 `contract_error_logger_level_filter` 验证。
- `dumpling/export/dump.rs::initLogger` 只在自行创建 logger 的分支记录版本；传入外部 logger 时提前返回。Go `dump.go::initLogger` 也只在内部初始化分支调用 `cli.LogLongVersion`。

## 并发与资源生命周期

版本槽使用标准库 `RwLock`，允许多个 `LongVersion`/`LogLongVersion` 调用并发读取，并让测试或构建接线独占写入。一次格式化会分别获取多个短读锁，因此它不提供跨全部字段的原子快照：若有并发写者逐项更新，单次输出理论上可能混合新旧字段。当前代码没有一次性注入全部版本元数据的事务接口。

本 crate 不创建线程、异步任务、通道、文件或网络资源，也没有显式关闭流程。`Logger` 由调用方拥有，`LogLongVersion` 只借用它。`reset_version_vars` 是共享状态的生命周期清理点，但不是自动 guard；并行测试若同时改写这些全局槽，仍可能互相干扰，新增测试应串行化修改或引入独立的测试级保护机制，而不能假设函数级复位足以提供并行隔离。

## 与 Go 版本的对应关系

Go 权威对照是 [`versions.go`](versions.go)，Bazel 的 `dumpling/cli/BUILD.bazel` 也只把该文件编入 Go `cli` 库。Rust 保持了 Go 的四个构建元数据名称、`LongVersion` 的字段顺序/空格/末尾换行、时间戳后的 `Z`，以及 `LogLongVersion` 的欢迎语和结构化字段顺序。

有意差异包括：

- Go 用无锁包级字符串，Rust 用 `RwLock` 包装进程级变量，以支持安全共享修改。
- Go 暴露 `GoVersion` 并打印 `Go version`/`Go Version`；Rust 暴露可选的 `RustVersion`，默认读取真实工具链版本，并打印 `Rust version`/`Rust Version`。
- Rust 额外公开 `reset_version_vars` 以清理测试覆盖；Go 文件没有对应函数。
- Go 的变量可直接赋任意运行时字符串；Rust 槽当前要求 `&'static str`。

命令行侧，Go `main.go` 直接执行 `println(cli.LongVersion())`，Rust 用 `long_version_output` 的额外换行保持同样输出。导出侧两版 `initLogger` 都只在内部创建 logger 时写版本欢迎日志；具体 logger 安装细节不同，但不属于本门面的职责。

## 扩展指南

- 新增版本字段时，应先修改 `versions.rs` 的状态、`LongVersion`、`LogLongVersion` 和 `reset_version_vars`，再同步 `versions.go` 或明确记录平台差异；`lib.rs` 的通配再导出通常无需改动。
- 同步扩展独立测试 `dumpling/cli/parity_test.rs`：至少覆盖默认值、自定义值、空值、日志字段和复位。若改变命令行文本的末尾换行，还要更新 `dumpling/cmd/dumpling/parity_test.rs::version_output_matches_go_builtin_println_bytes`。
- 若增加新的实现文件，应在本 crate 根显式挂载；只有需要成为公共 API 的项才再导出。不要把测试函数放入 `lib.rs` 或 `versions.rs`，继续使用独立 `*_test.rs` 文件。
- 修改全局注入方案时要评估兼容性：公开符号名受 Go 风格调用契约约束；把 `&'static str` 改成拥有型字符串会影响锁 guard 和日志字段构造；多字段更新若需一致快照，应把状态聚合在单一锁内。
- 性能风险很低，当前路径只在版本展示或 logger 初始化时读取少量锁；不要在高频路径引入版本日志。正确性风险主要是字段布局漂移、logger 分支语义改变、锁中毒以及并行测试污染。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标区域包含 `lib.rs`、`versions.rs`、`parity_test.rs` 和 `versions.go`。
- RustCodeGraph `node --file dumpling/cli/lib.rs`：确认文件仅含 `versions` 挂载、公开再导出和测试模块挂载。
- RustCodeGraph `node/query/explore`：确认 `LongVersion`、`LogLongVersion`、`rust_version`、`read_var`、`reset_version_vars` 的定义，以及 `dumpling/export/dump.rs::initLogger -> LogLongVersion`、parity 测试到公开 API 的调用边。
- 已读生产与配置路径：`dumpling/cli/lib.rs`、`dumpling/cli/versions.rs`、`dumpling/cli/Cargo.toml`、`dumpling/cli/BUILD.bazel`、`dumpling/export/lib.rs`、`dumpling/export/dump.rs`、`dumpling/export/Cargo.toml`、`dumpling/cmd/dumpling/main.rs`。
- 已读 Go 对照：`dumpling/cli/versions.go`、`dumpling/export/dump.go`、`dumpling/cmd/dumpling/main.go`。
- 已读独立 Rust 测试：`dumpling/cli/parity_test.rs`、`dumpling/cmd/dumpling/parity_test.rs`；同目录不存在 Go `*_test.go`，所以 crate 内的 Rust parity 测试是该门面的直接回归锚点。
- 本任务只新增说明文档，按计划不运行 Cargo；结构检查用于确认文档存在且恰有十一个规定章节。
