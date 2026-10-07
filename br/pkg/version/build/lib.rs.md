# `br/pkg/version/build/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-version-build` 的 crate 根。`br/pkg/version/build/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，并在 `package.metadata.porting.go-package` 中把对应 Go 包标为 `br/pkg/version/build`。本文件不是版本信息算法的实现文件，而是装配层：生产构建挂载 `info.rs`，测试构建再挂载两个独立测试文件，并将 `info` 的公开项重新导出到 crate 根。

该 crate 已被根工作区 `Cargo.toml` 纳入成员。当前可确认的直接 Rust 消费者包括 `br/pkg/version/version.rs` 和 `br/pkg/gluetikv/glue.rs`；前者通过 Cargo 依赖读取构建版本以执行兼容性判断，后者使用 `Info()` 组成 `Glue::GetVersion()` 的返回值。需要注意，`br/cmd/br/stubs.rs` 与 `lightning/pkg/server/stubs.rs` 仍各自定义同名 `build` 模块，因此这些命令层代码中的 `build::LogInfo` 目前不能据此认定为调用本 crate。

## 核心职责

本文件只有三项职责，依据分别是 `pub mod info`、两个 `#[cfg(test)] mod ...` 声明和 `pub use info::*`：

1. 用 `#[path = "info.rs"]` 指定版本/构建信息的 canonical Rust 实现文件。
2. 仅在测试构建中，把 `parity_test.rs` 与 `info_test.rs` 作为独立模块编译，遵守测试逻辑不与生产源文件混放的仓库约束。
3. 通过通配重新导出，让调用者使用 `astersql_br_pkg_version_build::Info`、`LogInfo`、`ReleaseVersion` 等 crate 根 API，而不必写 `::info::...`。

文件顶部的 crate 级 `#![allow(...)]` 放宽迁移代码的命名与暂未使用项警告，以容纳与 Go 对齐的 `Info`、`LogInfo`、`BuildTS` 等非 snake_case 名称；它不改变运行时行为。

## 主要符号

- `pub mod info`：公开子模块，实际源码固定为 `br/pkg/version/build/info.rs`。模块内包含本 crate 的所有生产逻辑。
- `mod parity_test`：仅在 `cfg(test)` 下存在，加载 `parity_test.rs`；它验证 Rust 公开契约与 Go 包的标签、常量、哨兵回退和日志字段保持一致。
- `mod info_test`：仅在 `cfg(test)` 下存在，加载 `info_test.rs`；它验证七行 `Info()` 格式、两个应用名的 `LogInfo()` 可调用性及版本元数据只快照一次。
- `pub use info::*`：将 `info.rs` 的公开符号提升到 crate 根。当前主要导出包括 `ReleaseVersionForTest`、`ReleaseVersion()`、`BuildTS()`、`GitHash()`、`GitBranch()`、`AppName`、`BR`、`Lightning`、`LogInfo()` 和 `Info()`；`take_logged_info()` 只在测试配置下公开。

`lib.rs` 自身不声明结构体、枚举、trait、函数、运行时常量或 `impl`。上述 API 的签名与行为均以 `info.rs` 为准，不能把本文件当成第二份实现。

## 执行流程

生产编译时，Rust 从 Cargo 指定的 `lib.rs` 进入，应用 crate 级 lint 放宽，随后加载 `info.rs` 并把其中的公开项重新导出。调用方访问 crate 根 API 后，执行才进入 `info.rs`：例如 `br/pkg/gluetikv/glue.rs` 把 `Info` 别名为 `BuildInfo`，在 `Glue::GetVersion()` 中返回 `format!("BR\n{}", BuildInfo())`；`br/pkg/version/version.rs` 的 `effective_release_version()` 和 `git_branch()` 分别调用 `build::ReleaseVersion()`、`build::GitBranch()`。

测试编译比生产编译多一步：`cfg(test)` 使 `parity_test.rs` 与 `info_test.rs` 成为 crate 内部模块。它们既能通过 crate 根测试 `pub use` 后的公开接口，也能通过 `crate::info::VERSION_METADATA_TEST_LOCK` 访问 `pub(crate)` 测试辅助状态。两个测试文件不会进入普通依赖方的产物。

本文件没有初始化函数或显式控制流。版本值的惰性初始化、格式化和输出流程都发生在 `info.rs`：`OnceLock` 缓存版本元数据，`Info()` 生成七行文本，`LogInfo()` 生成欢迎行并写到标准错误。

## 数据与状态

`lib.rs` 不拥有运行时数据。它只决定名称空间和条件编译边界。实际状态在 `info.rs`：

- `ReleaseVersion()`、`BuildTS()`、`GitHash()`、`GitBranch()` 各自使用函数内 `OnceLock<String>`，首次读取后返回克隆值，模拟 Go 包级变量初始化一次的语义。
- `ReleaseVersionForTest` 是占位版本回退值 `nightly-dirty`；`BR` 与 `Lightning` 是两个静态应用展示名。
- 测试配置下，`LAST_LOG` 是线程本地日志捕获缓冲，`VERSION_METADATA_TEST_LOCK` 串行保护会修改共享版本元数据的测试。

由于 `pub use info::*` 是通配导出，在 `info.rs` 新增任何 `pub` 项都会自动扩大 crate 根 API；这是本装配方式最重要的 API 面约束。

## 依赖与调用关系

向下依赖由 `Cargo.toml` 和 `info.rs` 共同确定。`lib.rs` 直接装配 `info.rs`，后者依赖：

- `astersql-parser-mysql::const::TiDBReleaseVersion`：发布版本来源及占位哨兵判断。
- `astersql-util-versioninfo::{TiDBBuildTS, TiDBGitBranch, TiDBGitHash}`：构建时间和 Git 元数据。
- `astersql-util-israce::RaceEnabled`：是否启用 race 的展示值。
- `astersql-config-kerneltype::IsNextGen`：输出 Classic 或 Next-Gen 内核类型。
- `rustc_version_runtime`：生成 `rustc <version>` 文本。

已核实的向上关系如下：

- `br/pkg/version/Cargo.toml` 依赖本 crate；`br/pkg/version/version.rs` 以 `use astersql_br_pkg_version_build as build` 引入它，并把 `ReleaseVersion()`、`GitBranch()` 用于 BR 与集群的版本兼容性判断。
- `br/pkg/gluetikv/Cargo.toml` 依赖本 crate；`br/pkg/gluetikv/glue.rs` 直接导入 `Info as BuildInfo`，用于 `Glue::GetVersion()`。
- RustCodeGraph 将 `info.rs` 的已索引使用文件列为 `info_test.rs`、`parity_test.rs` 和 `dumpling/cli/versions.rs`；但后者只是复用 `ReleaseVersion()`，不能由此推导整个 `lib.rs` 调用链。对 crate 依赖与精确路径的判断以 Cargo 声明和上述显式 `use` 为准。

## 错误处理与边界

本文件不返回 `Result`，也没有自行处理错误。边界主要来自装配与导出：

- `#[path]` 指向的文件缺失或无法编译时，crate 在编译阶段失败；不存在运行时降级。
- `cfg(test)` 保证测试辅助 API 和测试模块不进入生产构建。生产代码不得依赖 `take_logged_info()` 或 `VERSION_METADATA_TEST_LOCK`。
- 通配导出可能无意中把未来 `info.rs` 的新公开符号变成稳定 crate 根接口；新增公开项前应审查兼容性。
- `info.rs` 内部对 `RwLock` 使用 `unwrap()`，锁中毒会 panic；占位或 `None` 发布版本则不会报错，而是回退到 `nightly-dirty`。
- `Info()` 的七个标签及顺序、最后一行无尾换行是已被测试锁定的文本契约。Rust 将 Go 的 `Go Version` 有意对应为 `Rust Version`；不能机械要求字面完全相同。

## 并发与资源生命周期

`lib.rs` 不创建线程、任务、通道、锁、文件、网络连接或事务，也不负责资源释放。它所暴露实现的并发状态全部位于 `info.rs`：生产元数据通过 `OnceLock` 只初始化一次，初始化完成后只读并在每次调用时克隆 `String`；因此后续修改底层 `TiDBBuildTS`、`TiDBGitHash`、`TiDBGitBranch` 不会改变已缓存结果。

测试日志缓冲是 `thread_local! RefCell<Vec<String>>`，每个测试线程独立；共享版本元数据的变更由 `VERSION_METADATA_TEST_LOCK: Mutex<()>` 协调。测试必须恢复写入过的全局值，`info_test.rs::version_metadata_is_snapshotted_once` 已展示这一生命周期。`LogInfo()` 的生产副作用只有同步 `eprintln!`，本 crate 不持有日志后端守卫。

## 与 Go 版本的对应关系

Go 对照文件是 `br/pkg/version/build/info.go`，Rust 具体实现是 `info.rs`，而本 `lib.rs` 对应的是 Go 包天然拥有、Rust 需要显式声明的包边界。

- Go 的包级变量 `ReleaseVersion`、`BuildTS`、`GitHash`、`GitBranch` 在包初始化时快照；Rust 用同名函数加 `OnceLock` 模拟一次性快照。
- Go `getReleaseVersion()` 在 `TiDBReleaseVersion == "None"` 或含 `this-is-a-placeholder` 时返回 `nightly-dirty`；Rust保持相同分支，并用 `concat!` 写出占位片段。
- Go `AppName` 是 `string` 新类型；Rust 是 `&'static str` 类型别名。二者都为 `BR` 和 `Lightning` 提供静态展示名，但 Rust 别名不提供额外类型隔离。
- Go `LogInfo()` 临时把 PingCAP 日志级别切到 Info、结构化记录字段并恢复原级别；Rust 当前格式化单行后写 `stderr`，测试构建额外捕获该行。字段语义和顺序已对齐，但日志后端与级别恢复机制并不等价。
- Go `Info()` 第四行是 `Go Version`，Rust 对应为 `Rust Version`；其余标签顺序及无尾换行约定一致。

`info_test.go` 验证七个前缀和两个应用名的 `LogInfo()` 调用；独立 Rust 测试在此基础上增加元数据快照、哨兵回退、内核类型、日志捕获和常量检查。

## 扩展指南

若新增构建字段，优先修改 `info.rs` 的来源读取、缓存和 `Info()`/`LogInfo()` 格式；同时更新 `info_test.rs` 与 `parity_test.rs`，并核对 `info.go` 的真实契约。若字段只供内部使用，应避免声明为 `pub`，否则 `pub use info::*` 会把它自动暴露在 crate 根。

若要收窄或重命名公开 API，应先搜索 `astersql_br_pkg_version_build` 的显式依赖，至少检查 `br/pkg/version/version.rs`、`br/pkg/gluetikv/glue.rs` 及对应 Cargo 清单。不要仅修改 `lib.rs` 的导出而保留调用方假设。

新增测试应继续放在独立 `*_test.rs` 文件中，并在 `lib.rs` 用 `#[cfg(test)]` 与 `#[path]` 挂载；不要把测试函数内嵌到 `info.rs` 或 `lib.rs`。新增模块时应明确它是公开实现还是测试辅助，避免无意扩大生产 API。若未来把 BR/Lightning 命令从本地 `stubs.rs::build` 接到本 crate，还需单独验证日志后端、日志级别恢复及字段结构，不能只凭同名函数替换。

兼容性风险主要是人类可读版本文本和 crate 根符号的变化；性能风险很低，现有缓存避免重复读取共享元数据。并发修改时要维持一次初始化语义以及测试对共享状态的串行保护。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/version/build` 列出 `lib.rs`、`info.rs`、两个 Rust 测试及 Go 对照文件。
- RustCodeGraph `node --file br/pkg/version/build/lib.rs`：确认本文件共 28 行，只有 `info`、两个测试模块和通配重新导出；索引报告该文件自身被 `tools/tazel/parity_test.rs` 作为结构检查对象使用。
- RustCodeGraph `node` 已阅读：`br/pkg/version/build/info.rs`、`info_test.rs`、`parity_test.rs`、`info.go`、`info_test.go`、`br/pkg/version/version.rs`、`br/pkg/version/version_test.rs`、`br/pkg/gluetikv/glue.rs` 与 `lightning/pkg/server/lightning.rs` 的相关片段。
- Cargo 证据：`br/pkg/version/build/Cargo.toml` 定义 crate 根、Go 包映射与五项实现依赖；`br/pkg/version/Cargo.toml`、`br/pkg/gluetikv/Cargo.toml` 声明对本 crate 的路径依赖；根 `Cargo.toml` 纳入该成员。
- 文本搜索证据：精确搜索 `astersql_br_pkg_version_build` 与 `build::...`，确认 canonical crate 的直接引用，并确认 `br/cmd/br/stubs.rs`、`lightning/pkg/server/stubs.rs` 存在本地同名模块这一接线限制。
- 测试证据：`info_test.rs` 覆盖输出结构、可调用性和一次快照；`parity_test.rs` 覆盖 Go/Rust 公开契约、占位回退、日志副作用与常量；`info_test.go` 提供 Go 基准行为。本任务按计划为纯文档分析，未运行 Cargo 或代码测试。
