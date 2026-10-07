# `build/linter/util/exclude.rs`

## 文件定位

`build/linter/util/exclude.rs` 是 nogo/linter 工具链的“按分析器配置过滤源文件”辅助实现，语义对应同目录 Go 文件 [`exclude.go`](exclude.go)。它读取 [`build/config.rs`](../../config.rs) 中的 `NogoConfig`/`AnalysisConfig`，回答某个 analyzer pass 是否应处理给定文件名。

当前 Rust crate 接线是迁移期形态：根 [`Cargo.toml`](../../../Cargo.toml) 定义 `astersql` package，`[lib]` 指向 `pkg/lib.rs`，并依赖 `regex = "1"`；[`pkg/lib.rs`](../../../pkg/lib.rs) 仅在 `cfg(test)` 下将 [`exclude_test.rs`](exclude_test.rs) 挂入 crate，测试再用 `include!("exclude.rs")` 编译本文件。仓库中未找到将本文件直接声明为非测试 Rust 模块的入口，因此不应将其描述为已接入 Rust 生产 linter 可执行链。

## 核心职责

- `shouldRun` 以 analyzer/pass 名查找全局 nogo 配置；没有专用配置时默认允许分析。
- `shouldRunConfig` 实现 `only_files` 优先、`exclude_files` 其次、其余默认允许的决策表。
- `regexMatch` 将配置键当作正则表达式编译，再对完整 `fileName` 字符串做“是否存在匹配”判定。

该文件不解析 JSON、不遍历 AST，也不执行 analyzer；配置加载归 `build/config.rs`，文件列表的实际过滤在 [`util.rs`](util.rs) 的 `SkipAnalyzerByConfig` 中完成。

## 主要符号

- `pub fn shouldRun(passName: &str, fileName: &str) -> bool`：文件的对外入口。它从 `build::NogoConfig` 取得 `passName` 对应的 `AnalysisConfig`，未命中时返回 `true`，命中时委托 `shouldRunConfig`。
- `pub(super) fn shouldRunConfig(config: &build::AnalysisConfig, fileName: &str) -> bool`：可在父模块内使用的纯决策函数，也是独立测试构造配置时的直接入口。
- `pub(super) fn regexMatch(pattern: &str, text: &str) -> Result<bool, regex::Error>`：薄封装 `regex::Regex::new(pattern)` 和 `Regex::is_match(text)`，保留“无效正则”与“合法但未匹配”的区别。

本文件没有自定义常量、struct、enum、trait、`impl` 或条件编译项；仅导入 `crate::build` 与 `regex::Regex`。命名保留 Go 式样，根 crate 用 `#![allow(non_snake_case, non_upper_case_globals)]` 接纳这类迁移命名。

## 执行流程

1. 上游传入 analyzer 名和由 Go `FileSet` 定位得到的文件名。在 Rust 对照实现中，这个上游是 `util.rs::SkipAnalyzerByConfig`。
2. `shouldRun` 查询 `build::NogoConfig.get(passName)`。不存在该 pass 时立即返回 `true`。
3. 若 `OnlyFiles` 为 `Some`，`shouldRunConfig` 遍历其键集：任意模式匹配就返回 `true`，全部不匹配返回 `false`。空 map 因此拒绝所有文件。
4. 仅当 `OnlyFiles` 为 `None` 时才检查 `ExcludeFiles`：任意模式匹配就返回 `false`，全部不匹配返回 `true`。空 map 因此允许所有文件。
5. 两个字段均为 `None` 时返回 `true`。若两者均为 `Some`，`OnlyFiles` 分支会提前返回，`ExcludeFiles` 不会参与决策。
6. 设计上，`SkipAnalyzerByConfig` 用该布尔结果重建 `pass.Files`，然后调用原 analyzer `Run`；当前 Rust 仓库中这条路径由 [`util_test.rs`](util_test.rs) 的 include 测试证明，而 Go 生产实现则在 `util.go` 中实际接线。

## 数据与状态

`AnalysisConfig` 在 `build/config.rs` 中定义，包含 `Option<HashMap<String, String>>` 形式的 `OnlyFiles` 和 `ExcludeFiles`。本文件只读 map 的键，不使用值；键就是正则模式。`None` 与 `Some(empty_map)` 的语义不同：前者表示该规则未配置，后者表示已配置但集合为空。

`NogoConfig` 是 `LazyLock<HashMap<String, AnalysisConfig>>`，数据来自 `build/nogo_config.json` 的 `include_bytes!`。首次 `get` 会触发一次性解析；之后只共享不可变读取。本文件不缓存编译后的 `Regex`：每次比对、每个模式都会调用 `Regex::new`。`HashMap` 的迭代顺序不是决策契约，但在“是否存在匹配”的合法配置下不影响最终布尔值。

## 依赖与调用关系

- 上游设计调用边：`util.rs::SkipAnalyzerByConfig` 包装 analyzer 的 `Run`，对 `pass.Files` 调用 `shouldRun(analyzerName, &pos.Filename)` 做保留过滤。[`util_test.rs`](util_test.rs) 将 `exclude.rs` 包含成子模块，并用 `config_wrapper_filters_files_before_calling_the_original_analyzer` 验证 `gofmt` 会保留 `some.go`、排除 `uca_generated.go`。
- 上游 Rust 直接使用：`exclude_test.rs` 直接调用 `shouldRun`/`shouldRunConfig`/`regexMatch`。RustCodeGraph `explore` 能识别 `shouldRunConfig <- shouldRun` 及多个测试对 `shouldRunConfig`/`regexMatch` 的调用，但精确 `callers`/`callees` 命令对这些 `include!` 边返回空集；因此调用链同时以源文件和测试夹具核验。
- 下游数据依赖：`build::NogoConfig` 和 `build::AnalysisConfig`，定义于 `build/config.rs`。
- 下游算法依赖：`regex::Regex::new` 与 `Regex::is_match`，由根 `Cargo.toml` 的 `regex = "1"` 提供。
- Go 生产链：`util.go::SkipAnalyzerByConfig -> exclude.go::shouldRun -> build.NogoConfig/regexp.MatchString`；多个 Go analyzer 在初始化时调用 `SkipAnalyzerByConfig`。

## 错误处理与边界

`regexMatch` 本身不 panic，而是返回 `regex::Error`。`shouldRunConfig` 则将任意编译错误转成 `panic!("regex is wrong: {}", f)`，与 Go 版在 `regexp.MatchString` 返错后 panic 的契约对齐。panic 信息包含模式，但不包含 `regex::Error` 的详情。

重要边界包括：未知 pass 默认允许；`OnlyFiles = Some(empty)` 拒绝所有文件；`ExcludeFiles = Some(empty)` 允许所有文件；两者同时存在时只执行 `OnlyFiles`；正则是子串搜索，只有配置显式使用 `^`/`$` 才限定整串；文件路径不会在本文件内做分隔符或大小写归一化。

Rust `regex` crate 与 Go RE2 家族的常用子集高度接近，但并非所有语法都当然等价。当前保证范围是 `exclude_test.rs::every_checked_in_nogo_pattern_is_supported` 覆盖的已入库 nogo 模式，以及该测试列出的锚点、分组、字符类、量词等示例；新模式仍需做双端兼容验证。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、文件句柄或网络资源。`shouldRunConfig`/`regexMatch` 只在调用栈上工作，编译的 `Regex` 在单次 `regexMatch` 结束时释放。

共享状态只有下游 `NogoConfig: LazyLock<_>`：它由标准库提供线程安全的一次初始化，初始化后本文件只持有短期不可变引用。若多个 analyzer 或文件并发调用，本文件自身没有可变共享状态；主要成本是重复编译正则，而不是同步竞争。

## 与 Go 版本的对应关系

Rust 的 `shouldRun` 与 [`exclude.go`](exclude.go) 保留相同的决策顺序、早返回与 panic 策略。Rust 额外拆出 `shouldRunConfig`，便于不依赖全局 map 构造边界测试；Go 版将相同逻辑直接写在 `shouldRun` 内。Rust 还拆出返回 `Result` 的 `regexMatch`，而 Go 直接调用 `regexp.MatchString`。

`exclude_test.go::TestShouldRun` 提供三个原始回归用例，Rust `exclude_test.rs::test_should_run_matches_go_cases` 逐一对齐。Rust 测试另外固定了 Go 测试未显式覆盖的规则：`only_files` 优先级、空 map 语义、无效正则 panic、已入库配置在 Rust regex 引擎中可编译。

接线状态存在明确差异：Go `util.go` 与各 analyzer 已组成实际 linter 调用链；Rust `exclude.rs` 目前只由 Rust 测试通过 `include!` 编译和验证。所以这是可执行、有对齐测试的移植实现，但不是已完成非测试 Rust 主链接线的证据。

## 扩展指南

- 新增过滤策略时，首先在 `shouldRunConfig` 明确它与 `OnlyFiles`/`ExcludeFiles` 的优先级，并同步 `build/config.rs::AnalysisConfig` 与 Go `build.AnalysisConfig`；不要只改 Rust 分支。
- 新增或修改 nogo 正则时，同步扩展独立 [`exclude_test.rs`](exclude_test.rs)，验证 Rust 引擎与 Go 预期语义；对 Go 行为变更也应扩展 [`exclude_test.go`](exclude_test.go)。不要把 Rust 单元测试嵌入本生产文件。
- 若要优化每文件重复编译正则的开销，缓存必须保留无效配置的 panic 时机与信息，并考虑 `NogoConfig` 一次初始化的边界；需用代表性大配置测量性能，不能仅凭直觉加入全局可变缓存。
- 若完成 Rust 生产接线，应在 canonical linter crate/模块树中显式声明该模块，并用端到端 analyzer 测试证明配置确实裁剪 `Pass.Files`；现有 `include!` 测试只证明实现可编译与局部契约。
- 兼容风险主要在正则语法差异、路径形式和优先级改动；正确性风险是误过滤导致 analyzer 漏报；性能风险是在文件数与模式数乘积上重复编译正则。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件，其中 7032 个 Rust 文件；`files --filter build/linter/util` 识别到本文件、Go 对照、Rust/Go 测试及 `util.rs`/`util.go`。
- RustCodeGraph 符号查询：`shouldRun` 位于第 21 行，`shouldRunConfig` 位于第 28 行，`regexMatch` 位于第 56 行；`explore` 报告 `shouldRunConfig` 由 `shouldRun` 和对应 Rust 测试使用，`regexMatch` 由 `shouldRunConfig` 与多个正则契约测试使用。精确 callers/callees 对 include 路径返回空，未将其当作“无调用者”的证据。
- 已读源码：`build/linter/util/exclude.rs`、`build/linter/util/util.rs`、`build/config.rs`、`pkg/lib.rs`、根 `Cargo.toml`。
- 已读 Go 对照：`build/linter/util/exclude.go`、`build/linter/util/util.go`。
- 已读独立测试：`build/linter/util/exclude_test.rs`、`build/linter/util/util_test.rs`、`build/linter/util/exclude_test.go`。关键覆盖点为 Go 基准用例、优先级、空 map、无效正则、入库模式兼容性和 analyzer 文件列表裁剪。
- 按任务约束未运行 Cargo；本任务只新增文档，结构验证命令及 Ready 文档检查记录于本任务最终交付。
