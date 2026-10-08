# `pkg/util/printer/printer.rs`

源码入口：[`printer.rs`](./printer.rs)；crate 装配入口：[`lib.rs`](./lib.rs)；依赖声明：[`Cargo.toml`](./Cargo.toml)。

## 文件定位

本文件是 `astersql-util-printer` crate 的业务实现文件，由同目录的 `lib.rs` 以 `#[path = "printer.rs"] mod printer; pub use printer::*;` 装配并导出。它包含两类彼此独立但与 Go `pkg/util/printer/printer.go` 对齐的能力：生成 TiDB/AsterSQL 构建与运行模式信息，以及把二维字符串数据渲染为简单 ASCII 表格。

当前 Rust 接线并不等同于 Go 主程序的完整接线。`pkg/expression/builtin_info.rs::tidb_version` 通过依赖别名 `printer_dependency` 调用 `GetTiDBInfo`，`pkg/server/extract_runtime.rs::ProductionExtractSource::dump_package` 也用它生成回放包中的 `meta.txt`。相反，`cmd/tidb-server/main.rs` 的 `-V` 和启动日志当前调用 `crate::stubs::printer`，不是此 crate；因此 `PrintTiDBInfo` 虽然已有实现和测试，却尚未成为 Rust server 主入口的实际启动横幅实现。仓库搜索未发现生产 Rust 调用者使用 `GetPrintResult`。

## 核心职责

- `PrintTiDBInfo`：读取发布版本、构建元信息、内核类型、部署模式和配置快照，通过 `tracing::info!` 输出欢迎日志及序列化后的全局配置。字段集合会随 Classic/NextGen 和企业扩展 hash 是否为空而变化。
- `GetTiDBInfo`：把同一组版本与配置信息组织为稳定的多行文本，供 SQL 内建函数和 Extract 诊断归档使用；企业扩展行按需插入，内核类型总是最后一行。
- `GetPrintResult`：先校验表头与所有数据行的列数，再按 UTF-8 字节长度计算列宽，生成带分隔线、表头和数据行的 ASCII 表。
- 私有辅助函数集中封装发布版本派生、全局版本快照、表格校验、宽度计算和逐行拼接，避免公开入口重复条件逻辑。

## 主要符号

- `static buildVersion: OnceLock<String>` 与 `build_version() -> &'static str`：首次访问时调用 `rustc_version_runtime::version()`，此后进程内复用同一个编译器版本字符串。名字沿用 Go，但值是 Rust 编译器版本。
- `struct VersionInfo`：私有拥有型快照，包含 `edition`、`git_hash`、`git_branch`、`build_ts` 和 `enterprise_extension_git_hash`。`version_info()` 分别读取 `crate::versioninfo` 的五个 `RwLock<&str>` 并复制成 `String`，避免后续格式化期间继续持锁。
- `getReleaseVersionsForDisplay() -> (String, String)`：Classic 返回原始 `mysql::TiDBReleaseVersion` 和空 component；NextGen 先规范化版本，再调用 `BuildTiDBXReleaseVersion`。转换成功时返回 TiDBX 展示版本与规范化 component 版本，失败时回退原版本且 component 为空。
- `pub fn PrintTiDBInfo()`：日志型公开入口。NextGen 增加 `component_version`、`deploy_mode`；企业扩展 hash 非空时增加 `enterprise_extension_commit_hash`。随后把 `config::GetGlobalConfig()` 序列化为 JSON 并记录 `loaded config`。
- `pub fn GetTiDBInfo() -> String`：文本型公开入口。输出 release、edition、Git 信息、构建时间、`RustVersion`、race 开关、删表检查开关、store、可选企业扩展 hash 和 kernel type；不会输出 component version 或 deploy mode。
- `checkValidity`：要求表头非空、数据集非空且每行长度恰好等于表头长度。
- `getMaxColLen`：以 `String::len` 统计 UTF-8 字节数，取表头与各单元格的逐列最大值。
- `getPrintDivLine`、`getPrintRow`、`getPrintCol`、`getPrintRows`：分别生成分隔线、单行、表头和全部数据行；`getPrintCol` 直接复用 `getPrintRow`。
- `pub fn GetPrintResult(cols: &[String], datas: &[Vec<String>]) -> (String, bool)`：表格公开入口；失败约定为 `(String::new(), false)`，成功返回完整文本和 `true`。

本文件没有 trait、枚举、`impl` 或条件编译项；条件编译只出现在相邻 `lib.rs` 对独立测试模块的装配中。

## 执行流程

`PrintTiDBInfo` 的流程是：调用 `getReleaseVersionsForDisplay` 确定展示版本；调用 `version_info` 建立构建信息快照；读取删表检查、内核名和企业扩展 hash；按 NextGen/Classic 与企业 hash 是否为空选择四个日志字段组合；最后读取全局配置，使用 `serde_json::to_string` 序列化，并输出第二条配置日志。NextGen 的部署模式只在 NextGen 分支读取。

`GetTiDBInfo` 同样先派生展示版本并读取快照。它把非空企业扩展 hash 预先构造成带前导换行的可选片段，再一次性格式化主体；`Store` 后直接拼接该片段，最后追加 `Kernel Type`。这个顺序由 `printer_test.rs::test_get_tidb_info` 和 `migration_aster_unit_test.rs::tidb_info_matches_classic_nextgen_and_enterprise_branches` 固定。

`GetPrintResult` 先调用 `checkValidity`，任何空输入或列数不齐都立即返回失败。成功路径调用 `getMaxColLen`，只生成一次 divider，然后依次拼接“分隔线、表头、分隔线、全部数据行、分隔线”。每个单元格以 `"| "` 开始，内容后补 `max_width + 1 - cell.len()` 个空格，行尾为 `"|\n"`。

## 数据与状态

本文件不维护请求级可变状态。唯一文件级状态 `buildVersion` 是只写一次的 `OnceLock<String>`；初始化结果在进程生命周期内不变。

版本信息和配置来自相邻 `lib.rs` 暴露的全局态：`versioninfo` 使用多个 `RwLock<&'static str>`，`config` 使用 `LazyLock<RwLock<Config>>` 与 `RwLock<bool>`，`kerneltype` 使用 `AtomicBool`，`deploymode` 使用 `RwLock<&str>`，`mysql::TiDBReleaseVersion` 当前是 `static mut &str`。`version_info` 会获得五次独立读锁而非一个原子聚合快照，所以若测试辅助在并发中逐项改写，理论上可能观察到跨版本组合；现有会改全局态的测试用 `serial_test::serial` 串行化。

表格函数只借用输入切片并创建新的 `String`/`Vec<usize>`，不会修改调用方数据。列宽刻意按字节而非 Unicode 显示宽度计算，以匹配 Go `len(string)`；多字节字符的终端视觉对齐并非其保证。

## 依赖与调用关系

crate 边界由 `pkg/util/printer/Cargo.toml` 定义，库入口为 `lib.rs`。本文件直接使用标准库 `OnceLock`，以及 crate 内 `config`、`deploymode`、`israce`、`kerneltype`、`mysql`、`versioninfo`；外部依赖为 `rustc_version_runtime`、`serde_json` 和 `tracing`。版本换算的 `semver`、配置序列化的 `serde` 由 `lib.rs` 中的相邻模块使用。

已确认的 Rust 上游调用边如下：

- `pkg/expression/builtin_info.rs::tidb_version -> printer::GetTiDBInfo`，向 SQL `TIDB_VERSION()` 语义提供文本；其 Cargo 依赖名为 `printer-dependency`。
- `pkg/server/extract_runtime.rs::ProductionExtractSource::dump_package -> astersql_util_printer::GetTiDBInfo`，把文本写入 Extract 回放归档的 `meta.txt`。
- `pkg/util/printer/printer_test.rs` 与 `migration_aster_unit_test.rs` 直接调用三个公开入口。

RustCodeGraph 的文件节点同时报告 `printer.rs` 被 `pkg/server/extract_runtime.rs` 使用。仓库级 `rg` 还确认 `cmd/tidb-server/main.rs` 中同名调用解析到 `stubs::printer`，所以不能列为本文件调用边。生产 Rust 中未找到 `GetPrintResult` 的调用点；Go 侧 `GetTiDBInfo` 另被 server、executor、expression 和 plan replayer 等路径广泛消费，这些是对齐参考而不是当前 Rust 调用事实。

## 错误处理与边界

`getReleaseVersionsForDisplay` 对 NextGen 版本转换错误采用降级策略：保留原始 release version，并把 component version 置空，不向上传播错误。这与 Go 为“正常启动流外调用”保留 fallback 的意图一致。

`PrintTiDBInfo` 对配置 JSON 序列化使用 `expect("global TiDB configuration must be serializable")`；当前 `Config` 只有可序列化的 `String` 字段，失败被视为不可恢复的不变量破坏。`version_info` 及相邻全局态读取均使用 `RwLock::read().unwrap()`，锁中毒会 panic。读取 `mysql::TiDBReleaseVersion` 需要 `unsafe`，调用者依赖外部约束避免与写入并发形成数据竞争。

`GetPrintResult` 不返回详细错误：空表头、空数据集和任一行列数不等都统一返回空串与 `false`。通过先校验每行长度，它保证后续 `getMaxColLen` 与 `getPrintRow` 的索引合法；私有辅助函数若被未来代码绕过校验直接误用，仍可能因索引越界或减法下溢而 panic。

单元格中的换行、竖线等字符不会转义，超大输入也没有显式大小限制。输出宽度按字节数，因此适合复现 Go 文本，不等价于面向终端的 Unicode 列宽算法。

## 并发与资源生命周期

`OnceLock` 保证编译器版本在并发首次访问时只初始化一次，并安全返回进程期静态借用。版本与配置读取锁都只在复制或克隆字段期间短暂持有；日志格式化和 JSON 输出不会持续持锁。函数不创建线程、异步任务、通道、事务、文件句柄或网络资源。

`PrintTiDBInfo` 每次调用都会重新读取当前配置与版本全局态并发出两条日志；只有编译器版本被缓存。`GetTiDBInfo` 每次也重新取快照。因此测试修改全局态后能观察到新值，但修改这些全局态必须串行协调；相关 Rust 测试用 `#[serial]` 体现这一生命周期约束。

## 与 Go 版本的对应关系

Rust 的 `getReleaseVersionsForDisplay`、`PrintTiDBInfo`、`GetTiDBInfo`、`checkValidity`、列宽/行生成辅助函数和 `GetPrintResult` 与 `pkg/util/printer/printer.go` 一一对应。Classic/NextGen 版本转换、NextGen component/deploy-mode 条件字段、企业扩展条件字段、表格验证和拼接顺序均保留。

有三点需明确区分：第一，Go 的 `buildVersion` 来自 `runtime.Version()` 并标为 `GoVersion`，Rust 来自 `rustc_version_runtime`；日志仍使用 `go_version` 字段名以靠近 Go，而文本改为 `RustVersion`。第二，Go 通过 `logutil.BgLogger()` 和 zap 输出标题式字段名，Rust通过 `tracing` 输出 snake_case 字段。第三，Go 主程序真实调用本包的 `PrintTiDBInfo`/`GetTiDBInfo`，Rust `cmd/tidb-server` 目前仍走桩；现阶段真正接入本 crate 的生产路径只有已搜索到的 expression 与 server Extract 路径。

`printer_test.go` 与 `printer_test.rs` 共同固定基础表格输出、空输入、Classic/NextGen 版本和日志条件字段；Rust 的 `migration_aster_unit_test.rs` 进一步覆盖多字节字节宽度、企业扩展、配置 JSON 和更完整的分支组合。

## 扩展指南

新增版本字段时，应先判断它属于日志、诊断文本还是两者，并同步修改 `PrintTiDBInfo`、`GetTiDBInfo` 及 `VersionInfo`/`version_info`。字段若来自全局配置或版本态，应先复制快照再格式化，避免扩大锁持有范围；条件字段必须同时覆盖 Classic、NextGen、Community、Enterprise 组合。对应测试应放在独立的 `printer_test.rs` 或 `migration_aster_unit_test.rs`，不要嵌入生产文件，并同步检查 Go `printer.go`/`printer_test.go` 的行为契约。

若把真实 printer 接入 Rust server 启动流程，应修改 `cmd/tidb-server` 的依赖和导入接线，以本 crate 替换 `stubs::printer`，并验证 `-V` 与启动 `printInfo` 两条路径；这属于跨文件迁移任务，不是本文件当前已支持的事实。

扩展表格格式时，入口应继续通过 `checkValidity` 建立索引安全不变量。若改用 Unicode 显示宽度、转义换行/竖线或流式写入，会改变与 Go 的字节级兼容性和性能特征，必须同时更新 Go 对照约定及 `print_result_matches_go_validation_and_byte_width_behavior`。大表格当前会构造 divider、各行和最终结果多个临时 `String`；性能优化应以保持精确输出为前提，并增加独立的大输入或特殊字符测试。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`node --file pkg/util/printer/printer.rs` 读取了完整 265 行源码并报告该文件被 `pkg/server/extract_runtime.rs` 使用；`query` 分别定位 `PrintTiDBInfo`、`GetTiDBInfo`、`GetPrintResult` 的 Rust/Go 定义。精确 `callers`/`callees` 查询没有返回可用边，因此按技能规则用局部仓库搜索补证。
- 实现与边界：`pkg/util/printer/printer.rs`、`pkg/util/printer/lib.rs`、`pkg/util/printer/Cargo.toml`。
- Go 对照：`pkg/util/printer/printer.go`、`pkg/util/printer/printer_test.go`、`pkg/util/printer/main_test.go`。
- Rust 测试：`pkg/util/printer/printer_test.rs`、`pkg/util/printer/migration_aster_unit_test.rs`、`pkg/util/printer/main_test.rs`。
- Rust 实际调用与接线：`pkg/expression/builtin_info.rs::tidb_version`、`pkg/expression/Cargo.toml`、`pkg/server/extract_runtime.rs::ProductionExtractSource::dump_package`、`pkg/server/Cargo.toml`、`cmd/tidb-server/main.rs`、`cmd/tidb-server/stubs.rs::printer`。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，任务计划明确排除 Cargo；验收采用十一章结构检查、链接/路径核对和上述源码事实复核。
