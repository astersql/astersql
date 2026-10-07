# `br/pkg/version/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-version` 的 crate 根，而不是版本算法的实现文件。`br/pkg/version/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，并用 `package.metadata.porting.go-package = "br/pkg/version"` 声明对应的 Go 包。该文件的职责是把 [`version.rs`](version.rs) 装配为公开模块、在测试构建中挂载独立测试文件，并形成该 crate 的统一导入面。

在完整 BR 链路中，调用者依赖 crate 名而不是直接引用 `version.rs`。例如 `br/pkg/conn/conn.rs` 从 `astersql_br_pkg_version` 导入 `CheckClusterVersion`、`CheckVersionForBR`、`CheckVersionForBRPiTR` 和 `CheckVersionForDDL`，在连接管理器初始化及 DDL 能力判断时执行集群兼容性检查。因此，本文件位于“BR 建连/恢复准备 → 版本门面 → 具体兼容规则”的中间边界。

## 核心职责

本文件只承担四项装配职责：

1. 通过 `#[path = "version.rs"] pub mod version;` 声明实际实现模块。
2. 通过 `pub use version::*;` 将实现模块的公开项提升到 crate 根，使调用者可以写 `astersql_br_pkg_version::CheckClusterVersion`，无需增加 `::version` 层级。
3. 在 `cfg(test)` 下分别挂载 `parity_test.rs` 与 `version_test.rs`，保持生产逻辑和测试逻辑分文件。
4. 在 crate 根集中允许迁移期常见的 Go 风格命名和暂未接线项，包括 `non_snake_case`、`non_camel_case_types`、`non_upper_case_globals`、`dead_code` 及未使用项。

它不解析版本、不访问 PD 或 SQL，也不直接维护兼容状态；这些行为都来自被导出的 `version` 模块。crate 级 `allow` 会作用于整个 crate，因而也是当前 Go/Rust 迁移兼容策略的一部分，不能误当作仅影响这个短文件。

## 主要符号

- `version`：公开模块，源码由 `version.rs` 提供。该模块定义版本清洗、TiKV/TiFlash/BR/TiDB 兼容检查、服务端版本识别及最小 PD/SQL 抽象。
- `parity_test`：仅测试构建可见的私有模块，路径为 `parity_test.rs`；其 `go_rust_public_contract_matches` 从 crate 根导入公开项，验证根级再导出契约。
- `version_test`：仅测试构建可见的私有模块，路径为 `version_test.rs`；包含与 `version_test.go` 对齐的详细用例矩阵。
- `pub use version::*`：通配公开再导出，是本文件最关键的 API 行为。`version.rs` 当前公开的代表项包括 `Store`、`StoreLabel`、`PdClient`、`QueryExecutor`、`VerChecker`、`CheckClusterVersion`、多种 `CheckVersionFor*` 检查器、`FetchVersion`、`ParseServerInfo`、`ServerInfo` 和 `ServerType`。

文件没有自定义函数、结构体、常量、trait、`impl` 或 feature 条件；条件编译仅用于两个测试模块。

## 执行流程

生产构建的装配顺序如下：

1. 编译器以 `lib.rs` 作为 `astersql-br-pkg-version` 的 crate 根。
2. `#[path = "version.rs"]` 将相邻实现文件加载为公开的 `version` 模块。
3. `pub use version::*` 把该模块的所有公开项重新导出到 crate 根。
4. 上游调用者从 crate 根选择 API。例如 `br/pkg/conn/conn.rs` 的初始化路径把本地 PD 适配器交给 `CheckClusterVersion`，再按普通 BR、PiTR 或 DDL 场景注入不同 checker。
5. `CheckClusterVersion` 的实际实现拉取非墓碑 Store；TiFlash 走专用最低版本分支，TiKV 版本经 `removeVAndHash` 与 semver 解析后交给 checker。此逻辑属于 `version.rs`，本文件只使其可达。

测试构建会在上述步骤之外加载 `parity_test.rs` 和 `version_test.rs`。两个文件都作为 crate 内部子模块编译，因此既可通过 `crate::{...}` 验证根级导出，也可访问实现中限定为 `pub(crate)` 且仅测试启用的辅助入口。

## 数据与状态

`lib.rs` 自身不持有运行时数据。公开 API 所涉及的数据和状态均定义在 `version.rs`：

- `Store`/`StoreLabel` 是版本检查所需的 PD Store 最小投影；`PdClient` 和 `QueryExecutor` 分别抽象 Store 枚举与单行 SQL 查询。
- `VerChecker` 是线程安全的 `Arc<dyn Fn... + Send + Sync>`，允许调用者把不同兼容策略传给集群遍历器。
- `checkpoint_support_error` 与 `pitr_support_batch_kv_files` 通过 `OnceLock<Mutex<...>>` 保存进程内检查结果；`CheckVersionForBR`、`CheckVersionForBRPiTR` 更新它们，查询函数读取它们。
- `RELEASE_OVERRIDE` 是线程局部测试覆盖值，避免 Rust 并行测试互相修改发布版本。
- `CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION` 固定为 `5`，表示当前备份支持的 TableInfo 版本上限。

这些状态因 `pub use version::*` 而通过 crate 根暴露相关读写函数，但存储本身没有定义在本文件中。

## 依赖与调用关系

crate 直接依赖由 `br/pkg/version/Cargo.toml` 声明：`astersql-br-pkg-errors` 提供版本不匹配错误，`astersql-br-pkg-version-build` 提供 BR 构建版本，`astersql-errors` 提供共享错误与注解，`regex` 负责版本串识别，`semver` 负责语义版本比较。`lib.rs` 没有直接 `use` 这些依赖；它们由 `version.rs` 消费。

已验证的调用关系包括：

- 上游：`br/pkg/conn/conn.rs` 从 crate 根导入 `CheckClusterVersion` 及三种 checker，并在约第 892、895、928 行分别执行普通、PiTR 和 DDL 检查。
- 门面：`lib.rs::version` 指向 `version.rs`，`pub use version::*` 把实现 API 提升至 crate 根。
- 下游：`CheckClusterVersion` 调用注入的 `PdClient::GetAllStores(true)`、TiFlash 检查、版本净化/解析和 checker；`FetchVersion` 调用注入的 `QueryExecutor::QueryRow`。
- 测试：`parity_test.rs` 使用 `crate::{...}` 导入根级 API；`version_test.rs` 覆盖更完整的 Go 对照矩阵。

RustCodeGraph 的文件节点显示 `version.rs` 被 24 个文件使用，而 `lib.rs` 的直接文件边很少；这是再导出门面的正常特征，不能据此断言 crate 无业务调用者。实际 crate 依赖需结合 Cargo 包名导入与源码引用判断。

## 错误处理与边界

本文件没有返回值或错误分支，但它定义了错误行为的公开边界：调用者通过根级再导出接收 `version.rs` 产生的 `SharedError`。主要边界包括版本串无法解析、TiKV/TiFlash/BR major 或历史断点不兼容、TiDB 类型/区间不符合要求，以及 SQL 版本查询失败；版本不匹配通常以 `ErrVersionMismatch` 注解。

装配层的关键边界是可见性与条件编译：

- 删除或收窄 `pub mod version` 会破坏显式使用 `astersql_br_pkg_version::version::...` 的路径。
- 删除或改写 `pub use version::*` 会破坏当前根级导入者，即便实现仍存在。
- `parity_test`、`version_test` 只在 `cfg(test)` 下存在，不进入生产 artifact。
- crate 级 lint 允许项服务于 Go 风格 API 迁移；贸然移除可能令现有大写符号产生大量告警或在更严格构建设置下失败。

`lib.rs` 不捕获或转换错误，也没有恢复策略；所有传播语义应到 `version.rs` 的具体符号处核对。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、锁、文件句柄或网络连接，也不拥有需要 `Drop` 的资源。模块装配在编译期完成，生产运行时没有额外生命周期成本。

经门面导出的实现包含并发约束：`PdClient`、`QueryExecutor` 和 `VerChecker` 要求 `Send + Sync`；checkpoint 与 PiTR 标志由全局 `Mutex` 保护；测试发布版本覆盖使用 `thread_local!` 隔离并行用例。`CheckClusterVersion` 当前同步遍历 Store，资源所有权仍由调用者持有，因为接口接收借用的 client/checker。扩展门面时应避免在 crate 根另建一套全局状态，否则会割裂 `version.rs` 已有的锁与测试隔离策略。

## 与 Go 版本的对应关系

Go 对照包以 `br/pkg/version/version.go` 直接承载类型、变量和函数；Go 不需要 Rust 式 `lib.rs` 模块门面。因此，`lib.rs` 没有逐行对应的 Go 文件，它对齐的是 Go 包级可见性：`pub use version::*` 让 Rust 调用者获得近似 `version.CheckClusterVersion` 等包公开符号的体验。

`version.rs` 保留了 Go 的主要语义：清理 `v`/git hash/`dirty` 后缀、区分 TiFlash、按 checker 遍历 Store、检查 BR/TiKV/PiTR/DDL/Keyspace 版本、解析 TiDB/MySQL/MariaDB/Cloud 版本、优先查询 `tidb_version()` 并回退 `version()`。Rust 为降低依赖使用本地 `Store`、`PdClient`、`QueryExecutor` 抽象，并用 `Mutex` 与线程局部值适配并行测试；这些是语言和依赖边界差异，不是 `lib.rs` 中的简化实现。

测试对应关系为：`version_test.rs` 对照 `version_test.go` 的集群版本、比较、解析、规范化、服务类型检测和 SQL 查询用例；`parity_test.rs::go_rust_public_contract_matches` 额外锁定 crate 根导出、关键阈值、错误分支与状态副作用。当前同目录已有独立 Rust 测试，测试逻辑没有内嵌进 `lib.rs`。

## 扩展指南

新增版本能力时，优先在 `version.rs` 添加实现和公开符号；若继续使用通配再导出，新的 `pub` 项会自动出现在 crate 根。新增内部辅助项应保持私有或 `pub(crate)`，避免无意扩大稳定 API。若未来改为显式导出列表，必须同步检查所有 `astersql_br_pkg_version::{...}` 调用者。

安全扩展应同步完成以下工作：

- 在 `version_test.rs` 增加与 Go `version_test.go` 对应的正常、边界和错误用例；面向包公开契约的变化同时更新 `parity_test.rs`。
- 若 Go 源先发生变化，核对 `version.go` 的阈值、错误文案、状态副作用与调用顺序，不要只移植函数签名。
- 新增生产模块时在 `lib.rs` 显式声明路径；新增测试必须继续放在独立 `*_test.rs` 文件，并仅用 `cfg(test)` 挂载。
- 调整 `allow` 范围前先清点 Go 风格公开名及下游构建策略；调整再导出前先搜索 crate 根和 `::version` 两类路径。
- 关注共享状态的并发语义和 `CheckClusterVersion` 的逐 Store 早退行为；门面改动通常无性能成本，但实现扩展可能增加 PD 枚举后的每节点工作量。

兼容风险主要来自公开路径变化和错误/阈值语义漂移；性能风险主要位于实际检查器，不在本门面。相关 Rust 测试应保持在 `br/pkg/version/version_test.rs` 与 `br/pkg/version/parity_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/version` 确认目标、实现、Go 对照和独立测试；`node --file br/pkg/version/lib.rs` 确认 28 行门面源码；`node --file br/pkg/version/version.rs` 确认实现符号与依赖；`node --file br/pkg/version/parity_test.rs` 确认根级公开契约测试。通用名称的 `callers` 查询未返回可消歧结果，因此调用者再由精确源码引用补证。
- 源码：`br/pkg/version/lib.rs` 的模块声明、两个 `cfg(test)` 测试挂载和通配再导出；`br/pkg/version/version.rs` 的公开符号、流程、错误和共享状态。
- crate 边界：`br/pkg/version/Cargo.toml` 的包名、`[lib]` 路径、Go 包映射和五项直接依赖。
- 上游调用：`br/pkg/conn/conn.rs` 的 crate 根导入及 `CheckClusterVersion` 普通/PiTR/DDL 调用位点。
- Go 对照：`br/pkg/version/version.go`；Go 测试：`br/pkg/version/version_test.go`。
- Rust 测试：`br/pkg/version/version_test.rs` 与 `br/pkg/version/parity_test.rs`，分别覆盖详细矩阵和公开契约。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证检查本文存在且恰好包含规定的十一个二级标题。
