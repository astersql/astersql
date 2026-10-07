# `build/linter/gosec/analysis.rs`

## 文件定位

本文件是 Go 包 `build/linter/gosec` 中 `analysis.go` 的 Rust 迁移实现，目标是把 gosec 的安全检查包装成 Go analysis 风格的 analyzer：构造 `loader::Program`、执行选定的 gosec 规则、过滤 issue，并将 issue 的行号映射成 `analysis::Pass` 诊断位置。对外形状由 `Name`、`Analyzer`、`init` 和 `run` 组成。

当前接线必须区分 Go 生产路径与 Rust 验证路径。Go 文件由 `build/linter/gosec/BUILD.bazel` 声明为 `go_library`，并作为 `build/BUILD.bazel` 中 `nogo` 目标的依赖；Rust 文件没有独立 `Cargo.toml`，根 `Cargo.toml` 也没有为它声明生产模块。根 crate 的 `pkg/lib.rs` 仅在 `#[cfg(test)]` 下挂载 `analysis_test.rs`，而测试通过 `include_str!("analysis.rs")` 检查源码文本，并未编译或执行本文件。因此，本文件保留了迁移后的完整意图，但当前不能视为已替代 Go gosec analyzer 的生产实现。

## 核心职责

- `Name` 与 `Analyzer` 固定 analyzer 名称、说明和 `run` 回调，保持 Go 注册接口的形状。
- `init` 依次调用 `util::SkipAnalyzerByConfig` 与 `util::SkipAnalyzer`，保留按配置和统一跳过机制登记 analyzer 的初始化语义。
- `run` 只启用 `G104`、`G103`、`G101`、`G201` 四条 gosec 规则，并丢弃 gosec 自身日志输出。
- `run` 从当前 `analysis::Pass` 构造共享所有权的 `loader::Program`，执行 gosec，再把每个合格 issue 转为 `[gosec] <规则>: <说明>` 形式的诊断。
- `filterIssues` 按 severity、confidence 双阈值筛选；`parseIssueLine` 兼容单行号和 `from-to` 区间；`findLineOffset` 在原始字节中计算目标行的行首偏移。

## 主要符号

- `pub const Name: &str = "gosec"`：analyzer 的稳定名称，同时用于诊断前缀。
- `pub static Analyzer: analysis::Analyzer`：静态 analyzer 描述符，`doc` 为 `Inspects source code for security problems`，没有前置 analyzer（`requires: &[]`），回调为本文件的 `run`。
- `pub fn init()`：注册期跳过配置入口，无返回值且不保存本地状态。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error>`：主分析入口；通过修改 `pass` 上报诊断，所有正常出口均返回 `Ok(None)`。
- `pub fn filterIssues(issues: Vec<gosec::Issue>, severity: gosec::Score, confidence: gosec::Score) -> Vec<gosec::Issue>`：消费 issue 列表，只保留两个评分均达到阈值的项。
- `pub fn parseIssueLine(value: &str) -> Option<i32>`：解析十进制单行号，或验证 `from-to` 两端都是整数后返回起始行。
- `pub fn findLineOffset(fileContent: &[u8], line: i32) -> i32`：返回一基行号对应的零基字节偏移；无法定位时返回 `-1`。

文件没有自定义 `struct`、`enum`、`trait`、`impl` 或条件编译项，也没有可变模块级状态。公开函数和字段保留了 Go 风格命名，以便逐项对照迁移来源。

## 执行流程

1. `run` 创建默认 gosec 配置，通过 `rules::Generate` 只选择 `G104`、`G103`、`G101`、`G201`，并以 `io::Discard` 为输出创建 logger。
2. `gosec::NewAnalyzer` 构造执行器，`LoadRules` 安装上述规则的 builders。
3. `util::MakeFakeLoaderPackageInfo(pass)` 从当前 pass 创建一个 `PackageInfo`。它被放入 `Arc`；`Created` 持有该 `Arc`，`AllPackages` 以 `pkg.Pkg.clone()` 为键、`Arc::clone` 为值，保证两个集合共享同一个 package-info 对象而不使用裸指针。
4. `loader::Program` 借用 `pass.Fset`，令 `Imported` 为 `None`，并装入 `Created` 与 `AllPackages`；随后 `ProcessProgram` 执行检查，`Report` 取回 issue。
5. 若初始 issue 为空，立即返回 `Ok(None)`。否则用 `Low/Low` 调用 `filterIssues`；由于这是最低阈值，当前意图是保留所有达到 gosec 最低评分的 issue。
6. 对每个保留项，`util::ReadFile` 读取 issue 指向的文件并把它登记进 `pass.Fset`。读取失败按 Go 版本语义直接 panic。
7. `parseIssueLine` 将单行号或区间起始行转为 `i32`；格式无效时跳过该 issue，不影响其他诊断。
8. 从 `token::File` 取得 base，以 `findLineOffset` 在原始字节中找行首，二者相加形成 `token::Pos`；`pass.Reportf` 上报 `[gosec] RuleID: What`。遍历结束返回 `Ok(None)`。

## 数据与状态

持久的模块数据只有只读 `Name` 与静态 `Analyzer`。每次 `run` 的配置、规则集合、logger、gosec analyzer、package 表和 issue 列表都是调用内局部值，不跨 pass 缓存。

`Created` 与 `AllPackages` 共享 `Arc<loader::PackageInfo>`，其关键不变量是两处看到相同的 package-info 身份；这对应 Go 版本两个集合保存同一指针，但规避 Rust 裸指针所有权问题。`loader::Program.Fset` 只在此次执行期间借用 `pass.Fset`。

issue 定位以字节而非 Unicode 字符计数。`findLineOffset` 只识别 `b'\n'`，因此不会因非法 UTF-8 或多字节字符发生 lossy 转换造成偏移扩张。它约定行号从 1 开始：非空文件的第 1 行偏移为 0；空文件、非正行号、不存在的行以及文件末尾换行之后没有内容的“下一行”都返回 `-1`。

## 依赖与调用关系

RustCodeGraph 的精确调用边显示，Rust `run` 直接调用本文件的 `filterIssues`、`parseIssueLine`、`findLineOffset`；`filterIssues` 的唯一 Rust 调用者是 `run`，测试则以文本断言覆盖这些符号的关键语句。`Analyzer.run` 还通过函数字段保存 `run`，但这是值绑定而非图中普通调用边。

主要下游接口包括 `rules::Generate/Builders`、`gosec::NewConfig/NewAnalyzer/ProcessProgram/Report`、`util::MakeFakeLoaderPackageInfo/ReadFile`、`loader::Program`、`token::File::Base` 与 `analysis::Pass::Reportf`。源码中的 `analysis`、`gosec`、`rules`、`loader`、`types`、`token`、`util`、`log`、`io`、`HashMap` 均依赖未来真实 Rust 模块提供；当前生产 Cargo 模块树没有接入这些依赖。

Go 侧由 `build/linter/gosec/BUILD.bazel` 明确依赖 `build/linter/util`、golangci gosec/rules、golangci-lint result、Go analysis 与 loader，并由 `build/BUILD.bazel` 的 `nogo` 依赖进入实际 lint 构建。Rust 侧唯一仓库入口是 `pkg/lib.rs` 的测试模块到 `analysis_test.rs`；后者只 `include_str!` 源文件，所以证明的是迁移形状，而非链接或运行行为。

## 错误处理与边界

- `gosec::Report` 的第二个返回值被 `_` 丢弃，与 Go 版本忽略该值一致；本文件不会把 gosec 报告阶段的信息转换为 `analysis::Error`。
- `util::ReadFile` 失败会 `panic!("{}", err)`，不是可恢复错误；这是有意保留 Go 实现的 `panic(err)` 行为。
- issue 行号既不是整数也不是完整的 `from-to` 两整数区间时，`parseIssueLine` 返回 `None`，该 issue 被静默跳过。`"3-"`、`"-4"`、多段区间或含非数字端点均属此分支；函数不检查起始行是否正数或 `from <= to`。
- `findLineOffset` 的 `-1` 哨兵没有在 `run` 中单独拦截。按源码表达式，它仍会参与 `file_base + offset`；安全扩展时必须先确定 `token::Pos` 和上游 issue 行号对异常值的契约，不能假定不存在越界行。
- `token::File` 由 `ReadFile` 返回裸指针，`run` 通过 `unsafe { (*tf).Base() }` 做一次解引用。其安全前提是 `ReadFile` 成功后返回非空、有效且在读取 `Base` 时仍存活的指针；当前文本测试只能约束写法，不能证明此前提。
- `filterIssues` 使用包含边界的 `>=`；低于任一阈值的 issue 被排除。当前调用固定为 `Low/Low`，但辅助函数仍支持更高阈值。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络连接。logger 写向 `io::Discard`，没有需要关闭的外部输出资源；文件内容由 `ReadFile` 作为本轮 issue 的局部缓冲持有。

`Arc` 只用于表达 `loader::Program` 内部两个集合的共享所有权，并不表示存在并发执行；源码没有跨线程克隆或发送这些值。`prog`、gosec analyzer 和所有 `Arc` 在 `run` 返回前释放，`Fset` 借用也随调用结束。

`Analyzer` 是静态只读描述符，但 `init` 调用的 util 注册操作可能影响框架级配置；在没有真实 Rust 生产接线前，不能从本文件推断它们是否线程安全或可重复调用。裸 `token::File` 指针的生命周期由 `ReadFile`/`Fset` 契约控制，未来接线必须优先消除或封装这一 `unsafe` 边界。

## 与 Go 版本的对应关系

`build/linter/gosec/analysis.go` 是直接语义基准。两版的 analyzer 名称、说明、无前置依赖、初始化跳过顺序、四条启用规则、丢弃日志、fake loader program 字段、提前返回、`Low/Low` 过滤、读文件失败 panic、区间取起始行和诊断文本一致。

所有权是主要语言差异：Go 的 `Created []*loader.PackageInfo` 和 `AllPackages map[*types.Package]*loader.PackageInfo` 共享指针；Rust 用 `Arc<loader::PackageInfo>` 和克隆的 `types::Package` 键避免保存可变裸指针。Go issue 是指针切片，Rust `filterIssues` 消费并返回值向量，筛选语义不变。

定位实现也有显式迁移差异。Go 使用 `strconv.Atoi`，失败后用 `fmt.Sscanf("%d-%d")`，Rust 使用 `parse::<i32>` 与 `split_once('-')`；对仓库测试覆盖的普通单值/区间保持相同意图，但对空白、额外尾随内容等输入的接受范围可能不同。Go 最终调用 `util.FindOffset(string(fileContent), line, 1)`，Rust 内联为原始字节扫描，避免 `String::from_utf8_lossy` 改变字节位置。当前 Rust 测试没有执行这些函数，因此这些对应关系来自源码审查，不能冒充运行时等价证明。

## 扩展指南

- 调整安全规则时修改 `run` 中 `rules::Generate` 的 allowlist，并同步 `analysis_test.rs::analyzer_rules_and_skip_wiring_match_go`；若目标仍是逐提交对齐，还必须同步核对 `analysis.go`，避免 Rust 独自漂移。
- 改变评分策略时修改 `severity/confidence` 与 `filterIssues`，在独立的 `analysis_test.rs` 增加可执行的阈值边界测试；应覆盖等于阈值、只低一个维度和保持输入顺序。
- 扩展行号格式时集中修改 `parseIssueLine`，并补单行、区间、空串、负数、反向区间、多连字符、溢出整数测试。修改定位逻辑时对 `findLineOffset` 补空文件、首行、中间行、末行、末尾换行、CRLF、非法 UTF-8 和越界行测试。
- 变更 package program 组装时保持 `Created` 与 `AllPackages` 共享同一 `PackageInfo` 的不变量，并同步 `loader_program_uses_shared_owned_package_info_without_invalid_raw_pointers`；不要退回 `Vec<*mut ...>`。
- 修改诊断位置或文本时同步 `issue_lines_file_offsets_and_diagnostics_match_go`，并增加真实 `Pass` 的行为测试；特别要先处理 `findLineOffset == -1` 和 `token::File` 裸指针的契约。
- 若要投入 Rust 生产使用，需要新建明确的模块/依赖接线，并把当前仅扫描文本的测试升级为编译与行为测试。兼容风险是 Go/Rust 行号解析、评分排序和 loader package 身份漂移；性能风险主要是每个 issue 重新读取文件并从文件头线性扫描，issue 多时可能重复产生 O(issue 数 × 文件大小) 工作。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter build/linter/gosec` 找到 `analysis.go`、`analysis.rs`、`analysis_test.rs`；`node --file build/linter/gosec/analysis.rs --offset 1 --limit 260` 返回完整 161 行源码。`query` 确认 `filterIssues`、`parseIssueLine`、`findLineOffset` 的定义，`callees run` 确认 Rust `run -> filterIssues/parseIssueLine/findLineOffset` 三条本地调用边；宽泛 `explore` 也将 `analysis_test.rs` 标为该 Rust 文件的索引使用者。
- Rust 源码：`build/linter/gosec/analysis.rs`，核对全部 7 个模块级符号、四规则 allowlist、`Arc` 共享、issue 过滤、行号解析、字节偏移、panic 与 `unsafe` 边界。
- Go 对照：`build/linter/gosec/analysis.go`，核对 analyzer 元数据、规则选择、loader program、评分过滤、区间解析和诊断格式。
- Rust 独立测试：`build/linter/gosec/analysis_test.rs`，核对 4 个源码形状测试。测试使用 `include_str!`，没有调用 `run`、`filterIssues`、`parseIssueLine` 或 `findLineOffset`，因此只提供静态迁移约束。
- 接线与 crate 边界：根 `Cargo.toml` 的 `[package]`/`[lib] path = "pkg/lib.rs"`、`pkg/lib.rs` 的 `#[cfg(test)]` 路径模块、`build/linter/gosec/BUILD.bazel` 和 `build/BUILD.bazel`，分别证明 Rust 测试入口、无 Rust 生产模块接线及 Go Bazel `nogo` 生产接线。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务指定 shell 命令验证目标文件存在且恰有十一个固定二级标题，并人工复核没有把文本测试描述成运行时验证。
