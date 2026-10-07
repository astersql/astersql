# `br/pkg/version/build/info.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-version-build` 的实际实现文件；包入口 `br/pkg/version/build/lib.rs` 通过 `#[path = "info.rs"] pub mod info` 挂载它并用 `pub use info::*` 重导出全部公开项。`br/pkg/version/build/Cargo.toml` 将该包声明为 library，并以 `package.metadata.porting.go-package = "br/pkg/version/build"` 标明其 Go 对照包。

它位于 BR 的版本/构建元数据层，负责把解析器中的发布版本、`versioninfo` 中的构建注入值、Rust 编译器版本、竞态检测标志和内核类型汇总为稳定的文本接口。当前 Rust 生产代码中，`br/pkg/version/version.rs` 使用发布版本和 Git 分支做兼容性判断，`br/pkg/gluetikv/glue.rs::Glue::GetVersion` 使用 `Info` 形成 `BR\n...` 版本文本。

需要注意迁移期边界：`br/cmd/br/*.rs` 中形如 `build::Info()`、`build::LogInfo(build::BR)` 的调用目前解析到 `br/cmd/br/stubs.rs::build`，而 `br/cmd/br/Cargo.toml` 没有依赖本 crate；因此不能把这些 CLI 位点当作本文件已经接线的调用者。RustCodeGraph 对目标文件给出的文件级 “used by” 结果包含同名符号造成的宽泛关联，本文以上述 Cargo 依赖和精确导入为准。

## 核心职责

- `getReleaseVersion` / `ReleaseVersion`：读取 `astersql_parser_mysql::TiDBReleaseVersion`，过滤未注入的 `"None"` 和包含 `this-is-a-placeholder` 的占位值，并回落到 `ReleaseVersionForTest`；首次结果随后被缓存。
- `BuildTS`、`GitHash`、`GitBranch`：从 `astersql_util_versioninfo` 的三个 `RwLock<&str>` 全局值读取字符串，并分别在首次读取后固定快照。
- `rustVersion`：通过 `rustc_version_runtime::version()` 构造带 `rustc ` 前缀的编译器版本字符串。
- `LogInfo`：把所有版本字段、`RaceEnabled` 和 `IsNextGen()` 拼成一行欢迎信息，并写到标准错误；测试构建还捕获该行。
- `Info`：生成供命令版本输出或 Glue 展示使用的七行人类可读文本。
- `BR`、`Lightning` 与 `AppName`：提供两个约定的应用展示名和日志入口的参数类型。

本文件只读取和格式化元数据，不负责构建时注入、命令行解析、日志后端配置或集群版本兼容规则。

## 主要符号

- `pub const ReleaseVersionForTest: &str = "nightly-dirty"`：占位发布版本的统一回落值。`br/pkg/version/version.rs::CheckVersionForBRPiTR` 和 `CheckVersionForBR` 以它为哨兵，命中时跳过真实版本兼容检查。
- `fn getReleaseVersion() -> String`：内部解析入口。它通过 `unsafe` 读取 `pkg/parser/mysql/const.rs::TiDBReleaseVersion`；有效值原样复制，无效/占位值回落。
- `pub fn ReleaseVersion() -> String`：公开发布版本接口。函数内 `OnceLock<String>` 只运行一次 `getReleaseVersion`，每次调用向调用者克隆缓存字符串。
- `pub fn BuildTS() -> String`、`GitHash() -> String`、`GitBranch() -> String`：三个结构相同的公开快照接口，分别读取 `TiDBBuildTS`、`TiDBGitHash`、`TiDBGitBranch` 的读锁并缓存。
- `fn rustVersion() -> String`：私有编译器版本格式化函数，不缓存；`LogInfo` 和 `Info` 每次各自调用时重新查询运行库。
- `pub type AppName = &'static str`：日志应用名仅接受静态字符串，比 Go 的具名 `string` 类型更窄。
- `pub const BR` / `pub const Lightning`：值分别为 `"Backup & Restore (BR)"` 和 `"TiDB-Lightning"`。
- `pub fn LogInfo(name: AppName)`：公开单行日志输出入口。字段顺序固定为发布版本、Git hash、Git branch、Rust 版本、UTC 构建时间、race 标志、next-gen 标志。
- `pub fn Info() -> String`：公开多行文本入口。最后一行 `Kernel Type` 使用 `IsNextGen()` 选择 `Next-Gen` 或 `Classic`，且末尾没有换行。
- `VERSION_METADATA_TEST_LOCK`、`LAST_LOG`、`take_logged_info`：仅在 `cfg(test)` 下存在。前者串行化会改写共享版本元数据的测试；后两者以线程本地缓冲捕获并排空 `LogInfo` 输出，不属于生产 API。

## 执行流程

`ReleaseVersion()` 的流程是：访问函数局部的 `OnceLock`；若尚未初始化，则执行 `getReleaseVersion()`；后者读取解析器全局发布版本，先要求值不等于 `None`，再要求不包含拼接得到的占位片段；满足条件时复制原值，否则复制 `nightly-dirty`；最终从缓存克隆结果返回。拼接占位片段不改变匹配语义，只避免源码中直接出现完整占位字面量。

`BuildTS()`、`GitHash()` 和 `GitBranch()` 各自拥有独立 `OnceLock<String>`。第一次调用取得对应 `RwLock` 的读锁、复制当前 `&str` 并写入缓存；此后即使源全局量被改写，也只返回最初快照。`br/pkg/version/build/info_test.rs::version_metadata_is_snapshotted_once` 明确验证这一不变量。

`LogInfo(name)` 依次调用四个元数据读取函数和 `rustVersion()`，同时读取编译期选择的 `RaceEnabled` 与 `IsNextGen()`，用一次 `format!` 形成完整行。测试构建先把该行克隆进线程本地 `LAST_LOG`，之后无条件执行 `eprintln!`。

`Info()` 从空 `String` 开始，按固定次序追加七个标签和值：`Release Version`、`Git Commit Hash`、`Git Branch`、`Rust Version`、`UTC Build Time`、`Race Enabled`、`Kernel Type`。前六项带换行，最后一项不带。`br/pkg/version/build/info_test.rs::test_info` 和 `parity_test.rs::go_rust_public_contract_matches` 锁定了行数、顺序和关键值。

## 数据与状态

生产状态由四个彼此独立、函数局部的 `OnceLock<String>` 组成，分别保存发布版本、构建时间、Git hash 和 Git branch。缓存没有显式重置接口，其生命周期等同进程；返回 `String` 克隆使调用者无法改写缓存本体，但每次读取会分配/复制字符串。

上游原始状态来自两类全局量：`pkg/parser/mysql/const.rs::TiDBReleaseVersion` 是 `static mut &str`，本文件只做不安全读取；`pkg/util/versioninfo/versioninfo.rs` 中三个值是 `RwLock<&str>`，本文件只持短暂读锁。缓存初始化后，上游再变化不会反映到本文件输出。

`RaceEnabled` 由 `astersql-util-israce` 的 `race` Cargo feature 在编译期选择；`IsNextGen()` 由 `astersql-config-kerneltype` 的 `nextgen` feature 选择实现。因此它们不是本文件维护的运行期可变状态。

测试状态包括进程级 `Mutex<()>` 和每线程 `RefCell<Vec<String>>`。互斥锁只保护会临时改写共享 `versioninfo` 值的测试；日志捕获采用线程局部存储，避免并行测试互相消费记录。

## 依赖与调用关系

下游依赖由 `br/pkg/version/build/Cargo.toml` 明确声明：

- `astersql-parser-mysql` 提供可构建注入的 `TiDBReleaseVersion`。
- `astersql-util-versioninfo` 提供 `TiDBBuildTS`、`TiDBGitHash`、`TiDBGitBranch`。
- `rustc_version_runtime` 提供实际 Rust 编译器版本。
- `astersql-util-israce` 提供 `RaceEnabled`。
- `astersql-config-kerneltype` 提供 `IsNextGen()`。
- Rust 标准库提供 `OnceLock`；测试配置额外使用 `Mutex`、`RefCell` 和线程本地存储。

已核实的生产上游调用关系为：

- `br/pkg/version/version.rs::effective_release_version -> build::ReleaseVersion`；`CheckVersionForBRPiTR`、`CheckVersionForBR` 再使用该结果解析 BR 版本或识别测试哨兵。
- `br/pkg/version/version.rs::git_branch -> build::GitBranch`，作为版本检查路径保留的分支信息入口。
- `br/pkg/gluetikv/glue.rs::Glue::GetVersion -> Info`，形成 `BR\n{七行版本文本}`；该依赖也出现在 `br/pkg/gluetikv/Cargo.toml`。

包内调用关系为 `ReleaseVersion -> getReleaseVersion`，`LogInfo -> ReleaseVersion/GitHash/GitBranch/rustVersion/BuildTS/IsNextGen`，`Info` 对同一组信息做多行格式化。`lib.rs` 的重导出使外部调用者可以直接从 crate 根访问这些符号。

## 错误处理与边界

本文件的公开生产函数都不返回 `Result`。发布版本缺失或仍为占位值属于预期构建边界，使用 `nightly-dirty` 回落而非报错；任意其他字符串（包括可能无法被语义化版本解析的字符串）都会原样返回，合法性由 `br/pkg/version/version.rs` 等消费者决定。

三个 `versioninfo` 读操作使用 `read().unwrap()`：若相应 `RwLock` 已中毒，首次快照会 panic。成功初始化后不再接触该锁，因此后续锁中毒不会影响已有缓存。`TiDBReleaseVersion` 的读取位于 `unsafe` 块；本文件没有同步其潜在写入，安全使用依赖构建注入/初始化阶段完成后不并发修改这一外部约束。

`Info` 和 `LogInfo` 不转义字段中的换行或分隔符；注入值若包含换行，会改变展示结构。正常构建元数据应当是单行字符串。`Info` 的最后一行无尾换行是被测试固定的格式契约。

`eprintln!` 写失败时采用标准宏行为而非可恢复错误通道；`LogInfo` 也不提供日志级别、结构化字段或后端注入。测试捕获只在 `cfg(test)` 下生效，并不能阻止真实标准错误输出。

## 并发与资源生命周期

`OnceLock` 保证每一项元数据在并发首次访问时只初始化一次，并让所有线程观察同一快照。四项缓存独立初始化，因此一次 `Info()` 不是对所有上游字段的原子联合快照：若其他线程恰在首次读取期间改写不同的 `versioninfo` 锁，理论上可能组合出不同时间点的字段。

读取 `versioninfo` 时读锁只覆盖 `&str` 到拥有所有权的 `String` 的复制过程，之后立即释放。这里没有后台任务、异步运行时、网络连接、文件句柄、通道或显式析构逻辑；缓存留存至进程结束。

并发调用 `LogInfo` 不共享可变生产缓冲，每次先构造完整 `String` 再输出。标准错误上的跨线程排序不由本文件保证。测试中的 `LAST_LOG` 每线程隔离，`take_logged_info` 以 `mem::take` 原子地（相对于同一线程的可变借用）取走当前向量；`VERSION_METADATA_TEST_LOCK` 需要相关测试主动持有，生产函数不会获取它。

## 与 Go 版本的对应关系

直接对照文件为 `br/pkg/version/build/info.go`，对应测试为 `info_test.go`。

- Go 的 `ReleaseVersion`、`BuildTS`、`GitHash`、`GitBranch` 是包初始化时求值的变量；Rust 用各函数内的 `OnceLock` 在首次访问时惰性求值。首次读取后保持快照的语义一致，但初始化时点不同。
- Go `getReleaseVersion` 对 `None` 和完整占位片段回落；Rust保持同一判断和 `nightly-dirty` 回落值。
- Go 使用 `runtime.Version()` 和 `Go Version` 标签；Rust使用 `rustc_version_runtime`、`rustc ...` 日志值和 `Rust Version` 标签，这是语言迁移所需差异。
- Go `AppName` 是具名 `string`，Rust 是 `&'static str` 别名；两个应用名字面量一致。
- Go `LogInfo` 临时把 PingCAP 日志级别设为 Info、写结构化字段，并用 `defer` 恢复旧级别；Rust当前只向标准错误写扁平单行，不读取或恢复日志级别。这是明确的实现差异，不应描述成结构化日志等价。
- `Info` 的字段顺序、除编译器标签外的标签文字、race/内核选择和最后一行无尾换行保持一致。Go 测试只检查七个前缀及两种应用名调用不崩溃；Rust `info_test.rs` 保留这些契约，并额外验证元数据只快照一次；`parity_test.rs` 进一步验证哨兵、具体布尔/内核值和测试日志捕获。
- Go 包被 BR 命令广泛调用；当前 Rust `br/cmd/br` 仍通过本地桩模块提供 `build`，真实 crate 尚未接入该命令包。当前已接线范围以 Cargo manifest 为准。

## 扩展指南

新增一个构建字段时，应先确定其权威来源和注入生命周期，再在本文件增加读取/缓存函数，并同步接入 `LogInfo` 与/或 `Info`。若要求保持 Go 对齐，应同时核对 `info.go` 的字段名、顺序、尾换行和默认值；相应断言放在独立的 `br/pkg/version/build/info_test.rs` 或 `parity_test.rs`，不要把测试写进 `info.rs`。

修改发布版本回落规则时，重点审查 `getReleaseVersion`、`ReleaseVersionForTest`，以及 `br/pkg/version/version.rs` 中对哨兵的短路逻辑。改变缓存策略时必须保留或有意更新“首次读取后快照不变”的测试，并评估并发初始化、锁中毒和每次克隆成本。

修改输出格式时，应把 `Info` 的七行文本视为外部展示契约，把 `LogInfo` 的单行字段顺序视为日志消费者契约；同步更新两个 Rust 独立测试和 Go 对照测试意图。若要把 BR Rust CLI 接到真实实现，正确的接线点包括 `br/cmd/br/Cargo.toml` 和当前 `br/cmd/br/stubs.rs::build` 使用处，但这属于跨 crate 的迁移任务，不应仅在本文件内假装完成。

若把 `AppName` 放宽为动态字符串，应评估公开 API 兼容性和分配生命周期。若改为结构化日志，应保留 Go 中临时确保 Info 级别并恢复旧级别的行为，或明确记录差异。性能上当前主要成本是返回缓存字符串的克隆和格式化分配；只有在高频调用证据出现时才值得改为借用或共享字符串。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件、7,032 个 Rust 文件，目标分析时索引可用。
- RustCodeGraph `files --filter br/pkg/version/build`：确认生产实现、crate 入口、两份独立 Rust 测试及 Go 对照文件均在该目录。
- RustCodeGraph `node --file br/pkg/version/build/info.rs`：读取目标文件 1–136 行并确认 22 个符号；同时读取了 `lib.rs`、`info_test.rs`、`parity_test.rs`、`info.go`、`info_test.go`。
- RustCodeGraph 精确查询 `TiDBReleaseVersion`、`ReleaseVersion`、`Info`、`GetRawInfo`，并读取 `pkg/parser/mysql/const.rs`、`pkg/util/versioninfo/versioninfo.rs`、`pkg/config/kerneltype/{lib.rs,type.rs}`、`pkg/util/israce/lib.rs`，核对元数据来源与 feature 语义。
- 调用证据：读取 `br/pkg/version/version.rs` 的 `effective_release_version`、`git_branch`、`CheckVersionForBRPiTR`、`CheckVersionForBR`，以及 `br/pkg/gluetikv/glue.rs::Glue::GetVersion`；用相邻 Cargo manifest 核实这两个 crate 对 `astersql-br-pkg-version-build` 的依赖。
- 接线边界证据：读取 `br/cmd/br/stubs.rs::build` 与 `br/cmd/br/Cargo.toml`，确认命令代码中的同名调用当前指向本地桩且 manifest 未依赖目标 crate。
- RustCodeGraph 的 `callers` / `callees` 命令在本地大索引上连续 30 秒未返回；因此调用边最终由其文件级使用关系、精确符号查询、源码导入和 Cargo 依赖交叉验证，没有据此推断未接线关系。
- 结构检查应确认本文存在且恰好包含任务规定的十一个二级标题；本任务是纯文档分析，按计划不运行 Cargo。
