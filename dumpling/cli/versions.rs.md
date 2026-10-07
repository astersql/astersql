# `dumpling/cli/versions.rs`

## 文件定位

[`versions.rs`](versions.rs) 是 Cargo 包 `astersql-dumpling-cli` 的版本元数据实现文件，对应 Go 包中的 [`versions.go`](versions.go)。[`Cargo.toml`](Cargo.toml) 将 crate 根设为 [`lib.rs`](lib.rs)；crate 根再用 `#[path = "versions.rs"] mod versions;` 挂载本文件，并通过 `pub use versions::*;` 把公开状态和函数提升到 `astersql_dumpling_cli::*`。因此它不是独立二进制入口，而是 Dumpling 命令行和导出初始化流程共享的版本信息服务。

生产调用有两条直接主链。命令行侧，[`dumpling/cmd/dumpling/main.rs`](../cmd/dumpling/main.rs) 的 `long_version_output` 调用 `cli::LongVersion()`，供 `-V` 版本提前退出路径打印；导出侧，[`dumpling/export/dump.rs`](../export/dump.rs) 的 `initLogger` 在自行创建应用 logger 后调用 `cli::LogLongVersion(&d.tctx.L())`，写入启动欢迎日志。RustCodeGraph 将本文件标记为被 `dumpling/cli/parity_test.rs` 与 `dumpling/export/dump.rs` 使用；跨 crate 的命令行调用由 Cargo 路径依赖和源码引用补充确认。

## 核心职责

1. 保存发布版本、构建时间、Git 提交、Git 分支和 Rust 编译器版本这五类进程级元数据（`ReleaseVersion`、`BuildTimestamp`、`GitHash`、`GitBranch`、`RustVersion`）。
2. 由 `LongVersion` 按稳定的五行布局生成面向命令行的文本，保持 Go 版本的字段顺序、对齐空格、末尾换行和构建时间 `Z` 后缀。
3. 由 `LogLongVersion` 把同一组元数据作为有序结构化字段写入一条 Info 级 `Welcome to dumpling` 日志。
4. 由 `rust_version` 在未覆盖时读取实际运行时 rustc 版本；由 `read_var` 统一普通字符串槽的读锁与中毒处理。
5. 由 `reset_version_vars` 恢复默认状态，隔离会直接改写公开全局槽的独立测试。

本文件不负责解析命令行参数、决定何时显示版本、采集 Git 信息、执行构建期注入、创建 logger 或运行数据导出；它只定义元数据存储以及文本/日志两种读取视图。当前源码也没有自动从环境变量或 Cargo 包版本填充值的逻辑，未注入的四个构建字段会保持 `"Unknown"`。

## 主要符号

- `pub static ReleaseVersion: RwLock<&'static str>`：当前程序发布版本，默认 `"Unknown"`。
- `pub static BuildTimestamp: RwLock<&'static str>`：不含尾部 `Z` 的 UTC 构建时间主体，默认 `"Unknown"`；展示函数负责无条件追加 `Z`。
- `pub static GitHash: RwLock<&'static str>`：构建对应的 Git 提交哈希，默认 `"Unknown"`。
- `pub static GitBranch: RwLock<&'static str>`：构建时活动分支，默认 `"Unknown"`。
- `pub static RustVersion: RwLock<Option<&'static str>>`：编译器版本覆盖槽，默认 `None`。`Some(value)` 始终优先，包括 `Some("")`。
- `fn rust_version() -> String`：私有解析函数；持有 `RustVersion` 读锁，复制覆盖值，或调用 `rustc_version_runtime::version().to_string()` 获取实际版本。
- `fn read_var(v: &RwLock<&'static str>) -> &'static str`：私有读锁封装，供两条公开输出路径统一读取四个普通元数据槽。
- `pub fn LongVersion() -> String`：构造固定五行版本文本，返回拥有型 `String`。
- `pub fn LogLongVersion(logger: &Logger)`：借用调用方 logger，创建五个 `Field::string` 字段并调用 `Logger::Info`；无返回值。
- `pub fn reset_version_vars()`：依次取得五个槽的写锁，将四个字符串复位为 `"Unknown"`，将编译器覆盖复位为 `None`。

本文件没有类型、trait、`impl` 或条件编译项。所有 `pub` 符号经 `lib.rs` 的通配再导出成为 crate 根 API；两个辅助函数保持模块私有。

## 执行流程

`LongVersion` 的执行顺序是：

1. 依次通过 `read_var` 读取 `ReleaseVersion`、`GitHash`、`GitBranch` 和 `BuildTimestamp`。
2. 调用 `rust_version`；若 `RustVersion` 是 `Some`，复制该静态字符串，否则查询当前运行时 rustc 版本。
3. 使用一次 `format!` 按“发布版本、提交哈希、分支、构建时间、Rust 版本”的顺序拼接文本。
4. 在构建时间后无条件附加 `Z`，并让最后一行也以换行符结束。
5. 命令行 `long_version_output` 在这个已有末尾换行的字符串之后再追加一个换行，以对齐 Go `println(cli.LongVersion())` 的双换行字节行为；这一层不属于本文件。

`LogLongVersion` 的执行顺序是：

1. 读取四个普通元数据槽，并用与 Go 对照一致的键名创建字符串字段。
2. 通过 `rust_version` 取得覆盖值或真实 rustc 版本，创建 `Rust Version` 字段。
3. 调用 `logger.Info("Welcome to dumpling", fields)`。日志等级过滤和 sink 行为由 `astersql-dumpling-log` 实现，本函数不自行判断等级。

导出器的 `initLogger` 只有在配置未提供 logger、且内部 logger 初始化成功后才走到 `LogLongVersion`；若复用外部 logger，则提前返回而不记录欢迎日志。这一分支与 Go `dumpling/export/dump.go::initLogger` 的调用位置一致。

## 数据与状态

五个公开静态槽都是进程级共享状态。四个普通字段存储 `&'static str`，适合构建期静态值或测试字面量，却不能直接接收短生命周期或运行时拥有的 `String`。`RustVersion` 用 `Option` 区分“没有覆盖，应查询真实工具链”与“显式覆盖”；空字符串仍是有效覆盖，而不是缺失值。

`LongVersion` 与 `LogLongVersion` 不缓存最终结果，每次调用都会重新获取各槽当前值。`reset_version_vars` 是显式清理函数，不会自动在作用域结束时执行。独立测试 [`parity_test.rs`](parity_test.rs) 会直接写这些公开锁，因此在各契约开始或结束时调用复位函数以降低跨用例污染。

各字段分别拥有锁，而不是放在一个聚合状态锁中。单次输出会按字段获取多个短读锁，所以它只保证每次单字段读取的数据竞争安全，不保证五个字段组成同一原子快照；若另一个线程逐项写入，理论上可能观察到新旧值混合。

## 依赖与调用关系

- 标准库依赖：`std::sync::RwLock` 提供并发读和独占写。
- 日志依赖：`astersql_dumpling_log::{Field, Logger}`；`LogLongVersion -> Field::string -> Logger::Info`。该路径由 `dumpling/cli/Cargo.toml` 中对 `../log` 的直接路径依赖建立。
- 工具链依赖：`rust_version -> rustc_version_runtime::version`；Cargo 依赖版本为 `0.3`。
- crate 门面：`dumpling/cli/lib.rs::versions -> pub use versions::*`，使调用方使用 `astersql_dumpling_cli::LongVersion` 等稳定路径，而不依赖私有模块名。
- 命令行上游：`dumpling/cmd/dumpling/main.rs::long_version_output -> cli::LongVersion`；`dumpling/cmd/dumpling/Cargo.toml` 通过 `../../cli` 引入本 crate。
- 导出上游：`dumpling/export/dump.rs::initLogger -> cli::LogLongVersion`；`dumpling/export/lib.rs` 将本 crate 别名为 `cli`，`dumpling/export/Cargo.toml` 通过 `../cli` 建立依赖。
- 直接测试：`dumpling/cli/parity_test.rs::go_rust_public_contract_matches` 聚合默认文本/日志、自定义与空值、日志等级过滤、状态复位四类契约；`dumpling/cmd/dumpling/parity_test.rs` 另行验证版本提前退出和双换行行为。

RustCodeGraph 对 `LongVersion` 的精确 callee 结果确认其引用四个元数据静态项并调用 `rust_version`；对 `LogLongVersion` 的结果确认其引用相同字段、调用 `read_var`/`rust_version` 并下沉到 Info 日志。图的常见符号消歧会混入同名 Go 定义和其他 `Info`/`string`，因此本文只采用能由目标源码、限定路径和 `rg` 引用共同复核的边。

## 错误处理与边界

本文件不返回 `Result`，也不吞掉可恢复错误。全部锁获取都使用 `expect`：读取路径的消息是 `"version var poisoned"`，复位写路径的消息是 `"poisoned"`。如果线程在持锁期间 panic 导致锁中毒，后续文本生成、日志写入或复位都会 panic；当前实现没有恢复中毒锁或返回错误给调用方的机制。

格式边界由测试明确锁定：默认值逐字显示为 `Unknown`；空字符串仍保留标签、对齐空格和换行；空构建时间输出 `Build timestamp: Z`；`Some("")` 的 Rust 版本不会退回真实 rustc 版本。`LongVersion` 固定带末尾换行，调用方若再使用 `println` 语义会形成空白行。

`LogLongVersion` 本身不检查日志等级，也不报告日志是否真正写出。`parity_test.rs::contract_error_logger_level_filter` 证明 Error 门限会丢弃 Info 欢迎日志，nop logger 也不会留下捕获项且调用不 panic。logger 初始化失败则发生在上游 `dump.rs::initLogger`，此函数只有在 logger 已可用时才被调用。

## 并发与资源生命周期

多个线程可以并发调用 `LongVersion` 和 `LogLongVersion`；`RwLock` 允许并发读取。写入公开槽或调用 `reset_version_vars` 时会对相应单槽取得独占锁。锁 guard 都只在单次表达式或辅助函数内短暂存在，输出字符串和日志字段拥有复制后的内容，不会把 guard 传给下游 logger。

本文件不创建线程、异步任务、通道、事务、文件或网络连接；它也不拥有 logger，`LogLongVersion` 仅借用 `&Logger`。`rustc_version_runtime::version()` 的结果在本函数内转为 `String`。资源生命周期的主要风险来自全局可变状态：并行测试即使都调用 `reset_version_vars`，仍可能在“复位—写入—断言”区间互相覆盖；新增写状态测试应采用串行策略或专门的测试互斥保护。

由于五个写锁依次获取，复位过程也不是跨字段原子的。当前使用场景集中在启动和测试，不是高频请求路径；锁和格式化开销有限，但不应把版本日志接入循环或每请求路径。

## 与 Go 版本的对应关系

Go 对照 [`versions.go`](versions.go) 定义 `ReleaseVersion`、`BuildTimestamp`、`GitHash`、`GitBranch`、`GoVersion`，以及同名的 `LongVersion`、`LogLongVersion`。Rust 保留前四个名称和默认值，并保持两种输出的字段顺序、文本标签布局、构建时间 `Z` 后缀、欢迎语及日志字段顺序。

明确差异如下：

- Go 用无锁包级 `string`，Rust 用 `RwLock<&'static str>` 提供线程安全共享读写。
- Go 的编译器槽是默认 `"Unknown"` 的 `GoVersion`；Rust 改为 `RustVersion: Option<&'static str>`，未覆盖时通过 `rustc_version_runtime` 展示实际 rustc 版本。
- 文本和日志键分别从 `Go version`/`Go Version` 改为 `Rust version`/`Rust Version`。
- Rust 额外提供 `read_var`、`rust_version` 和 `reset_version_vars`；Go 文件没有这些辅助函数。
- Go 变量能接收任意运行时字符串；Rust 的公开槽要求值具有 `'static` 生命周期。

调用语义仍对齐：Go `dumpling/cmd/dumpling/main.go` 用 `println(cli.LongVersion())`，Rust 命令行包装函数补上额外换行；Go `dumpling/export/dump.go::initLogger` 与 Rust `dump.rs::initLogger` 都只在内部创建 logger 的分支记录版本信息。同目录未发现 Go `*_test.go`，对应契约由独立 Rust parity 测试固定。

## 扩展指南

- 新增版本字段时，应同步修改静态状态、`LongVersion`、`LogLongVersion` 和 `reset_version_vars`，并扩展 `dumpling/cli/parity_test.rs` 对默认值、自定义值、空值、日志字段和清理的断言；若改变命令行换行，还要同步 `dumpling/cmd/dumpling/parity_test.rs::version_output_matches_go_builtin_println_bytes`。
- 若字段必须保持一致快照，应把元数据聚合到单一结构和单一锁中，而不是继续增加独立锁；需要评估现有公开静态变量的兼容迁移方案。
- 若要支持运行时拥有字符串，不能只把赋值处改为 `String`；还需重新设计静态存储类型、读锁 guard 到日志字段的所有权转换，以及现有调用方/测试 API。
- 修改文本标签、空格、顺序、时间后缀或末尾换行属于用户可见兼容变更，应同时核对 Go 对照、CLI parity 测试和可能解析版本输出的外部脚本。
- 新测试继续放在独立的 `parity_test.rs` 或同目录独立 `*_test.rs` 中，不要把测试逻辑嵌入生产 `versions.rs`。修改锁行为时应增加锁中毒或并行访问的独立回归测试。
- 性能风险主要来自误把格式化/日志调用放到高频路径；当前启动级调用无需优化。正确性风险主要是多字段非原子更新、锁中毒 panic、全局测试污染，以及 Go/Rust 展示契约漂移。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；目标文件已索引。
- RustCodeGraph `node --file dumpling/cli/versions.rs`：确认文件共 92 行、五个公开静态槽、两个私有辅助函数和三个公开函数；无类型、trait、`impl` 或条件编译项。
- RustCodeGraph `query LongVersion`、`query LogLongVersion`、`query reset_version_vars`：确认 Rust/Go 同名定义及 Rust 符号位置；文件限定 callee 查询确认 `LongVersion`/`LogLongVersion` 到元数据槽和辅助函数的关系。对内部节点 ID 的调用图查询出现模糊消歧，因此未把其无关结果作为证据。
- RustCodeGraph 已读路径：`dumpling/cli/lib.rs`、`dumpling/cli/versions.go`、`dumpling/cli/parity_test.rs`、`dumpling/cmd/dumpling/main.rs`、`dumpling/cmd/dumpling/parity_test.rs`、`dumpling/export/dump.rs`。
- 配置与源码交叉核对：`dumpling/cli/Cargo.toml`、`dumpling/cmd/dumpling/Cargo.toml`、`dumpling/export/Cargo.toml`、`dumpling/export/lib.rs`，以及 `rg` 对全部 Dumpling Rust/Go 引用的检索。
- Go 直接证据：`dumpling/cli/versions.go`、`dumpling/cmd/dumpling/main.go:48`、`dumpling/export/dump.go:1646`。同目录没有 Go 测试文件。
- Rust 独立测试证据：`dumpling/cli/parity_test.rs` 覆盖默认格式、自定义/空值、Info 等级过滤、nop sink 和全局复位；`dumpling/cmd/dumpling/parity_test.rs` 覆盖 `-V` 提前退出及双换行字节契约。
- 本任务是纯文档分析，按计划不运行 Cargo；完成检查仅包含固定章节结构、链接/事实人工复核和差异范围自审。
