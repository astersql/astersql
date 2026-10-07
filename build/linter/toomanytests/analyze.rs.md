# `build/linter/toomanytests/analyze.rs`

## 文件定位

本文件是 Go 构建检查器 [`build/linter/toomanytests/analyze.go`](./analyze.go) 的 Rust 对照实现，位于构建辅助目录 `build/linter/toomanytests`，职责是限制一个 Go 包中的顶层测试函数数量。它定义了分析器描述、扫描入口和阈值规则，但当前不是一个独立 Cargo crate：仓库根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员中没有 `build/linter/toomanytests`，目录内也没有 `Cargo.toml`。

当前生产构建链仍是 Go/Bazel 链路。[`build/linter/toomanytests/BUILD.bazel`](./BUILD.bazel) 只把 `analyze.go` 声明为 `go_library`；上层 [`build/BUILD.bazel`](../../BUILD.bazel) 将 `//build/linter/toomanytests` 放进 `nogo` 分析器依赖。Rust 文件只被 [`pkg/lib.rs`](../../../pkg/lib.rs) 中的 `#[cfg(test)]` 模块间接使用：该模块加载独立测试 [`analyze_test.rs`](./analyze_test.rs)，测试再以 `include!("analyze.rs")` 将本文件放入桩实现环境。因此它目前是“可独立验证算法形状的迁移实现”，不是已经替代 Go `nogo` 分析器的生产入口。

## 核心职责

- `Analyzer` 描述名为 `toomanytests` 的分析器，把执行入口绑定到 `run`，且声明没有前置 analyzer 依赖（`requires: &[]`）。
- `run` 遍历 `analysis::Pass::Files`，只处理文件名以 `_test.go` 结尾的 Go AST 文件，并只统计顶层、无接收者、名字以 `Test` 开头且不等于 `TestMain` 的函数声明。
- `checkRule` 为两个历史上测试量较大的包提供例外阈值：`pkg/planner/core` 为 210，`pkg/util/topsql/reporter` 为 90，其余包为 50。
- 当计数严格大于阈值时，`run` 在最后一个测试文件的位置报告一次包级诊断；等于阈值不报告。
- `init` 表达与 Go 版本一致的两层跳过接线意图：先应用配置中的文件排除，再应用 lint 指令跳过。

这些职责均可由 `Analyzer`、`run`、`isTestFile`、`checkRule` 和 `init` 五个模块级符号直接复核；文件没有类型定义、trait、`impl` 或条件编译项。

## 主要符号

- `pub static Analyzer: analysis::Analyzer`：公开静态分析器描述。`name` 和 `doc` 用于识别及说明规则，`requires` 为空，`run` 字段直接保存本文件函数指针。当前声明本身不负责注册；Go/Bazel 的注册证据在 `build/BUILD.bazel`。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：公开扫描入口。成功时恒定返回 `Ok(None)`；规则违规通过 `pass.Reportf` 产生诊断，而不是通过返回值产生错误。
- `pub fn isTestFile(file: &token::File) -> bool`：公开的纯辅助函数，仅以 `token::File::Name()` 的字符串后缀 `_test.go` 判定；它不读取 AST 内容，也不验证路径是否存在。
- `pub fn checkRule(pkg: &str) -> i32`：公开的纯阈值函数，以精确字符串匹配两个例外目录，其余目录统一返回 50。
- `pub fn init()`：公开初始化函数，依次调用 `util::SkipAnalyzerByConfig(&Analyzer)` 与 `util::SkipAnalyzer(&Analyzer)`。这对应 Go 的包初始化意图；当前 Rust 生产模块没有被 Cargo 接线，不能据此断言该函数会在程序启动时自动运行。

RustCodeGraph 将上述符号识别为本文件的 5 个符号，并确认 `run` 的文件限定 ID 为 `build/linter/toomanytests/analyze.rs:37:function:run`。图索引未给出可用的跨模块静态调用边；实际入口和测试关系因此由 Cargo、Bazel、`pkg/lib.rs` 及独立测试交叉核验。

## 执行流程

1. 框架通过 `Analyzer.run` 进入 `run`，函数初始化累计数 `cnt = 0` 和默认位置 `pos`。
2. 对 `pass.Files` 中每个 AST 文件，以 `f.Pos()` 查询 `pass.Fset.File(...)`。若位置不属于该 `FileSet`，Rust 实现通过 `expect` 立即 panic；正常情况下得到对应 `token::File`。
3. `isTestFile` 过滤掉所有非 `_test.go` 文件。被过滤文件既不参与函数统计，也不会更新诊断位置。
4. 对测试文件的顶层 `Decls` 逐项调用 `as_func_decl()`。非函数声明被忽略；函数必须同时满足名称前缀为 `Test`、`Recv.is_none()`、名称不是 `TestMain`，才让 `cnt` 加一。因此方法、benchmark、示例函数和 `TestMain` 都不计数。
5. 每处理完一个测试文件，就将 `pos` 更新为该文件的起始位置。循环结束后，`pass.Fset.Position(pos).Filename` 给出文件名，`filepath::Dir` 从中推导包目录 `pkgName`。
6. `checkRule(&pkgName)` 选取阈值。只有 `cnt > threshold` 时才调用一次 `pass.Reportf`，消息为 `<包目录>: Too many test cases in one package: <数量>`，位置是最后一个测试文件的 `pos`。
7. 无论是否报告诊断，函数都返回 `Ok(None)`。独立测试 `run_counts_only_go_test_declarations_and_reports_strictly_above_limit` 验证默认阈值 50 本身允许、加入第 51 个合格测试后恰好报告一次。

## 数据与状态

扫描状态全部局限于单次 `run` 调用：`cnt` 是包内合格测试函数总数，`pos` 是最近处理的测试文件位置，`pkgName` 是从该位置解析出的目录。函数不缓存 AST、不写全局集合，也不跨调用保存计数。

`Analyzer` 是静态元数据对象；规则阈值没有可变配置结构，而是固化在 `checkRule` 的精确路径分支中。`run` 借用 `&mut analysis::Pass`，唯一可观察副作用是通过 `Reportf` 向 pass 增加诊断。返回的 `Option<Box<dyn Any>>` 恒为 `None`，说明该分析器不向依赖它的其他分析器提供结果对象；`requires: &[]` 也说明它不消费其他 analyzer 的结果。

包名实质上是目录字符串而非 Go package declaration。相对路径是否能命中特例取决于 `FileSet` 中保存的文件名形式；`checkRule` 对绝对路径或带不同前缀的路径不会自动规范化到两个特例字符串。

## 依赖与调用关系

内部调用关系清晰且单向：`Analyzer.run -> run`；`run -> isTestFile`、`run -> checkRule`，并调用 `analysis::Pass`、`token::FileSet`、AST declaration、`strings` 和 `filepath` 兼容 API；`init -> util::SkipAnalyzerByConfig -> util::SkipAnalyzer`（按调用顺序描述，不表示两者彼此调用）。

Go 生产侧的外部链路是 `build/BUILD.bazel` 的 `nogo` 配置依赖 `//build/linter/toomanytests`，后者由目录 `BUILD.bazel` 编译 `analyze.go` 并依赖 `//build/linter/util` 与 `golang.org/x/tools/go/analysis`。Rust 侧没有对应 Cargo dependency 或生产注册调用者；根 crate 仅在测试配置下从 `pkg/lib.rs` 加载 `analyze_test.rs`，测试在本地定义 `analysis`、`ast`、`token`、`strings`、`filepath`、`util` 桩后包含目标文件。

这种接线意味着源码里的 `analysis`、`token` 等名称目前由包含它的测试模块提供，而非由本文件显式 `use` 或 Cargo 依赖解析。若未来要投入 Rust 生产使用，必须先确定真实兼容 crate、模块导入、静态 analyzer 的可变初始化方案和注册位置，不能只把文件加入某个 `mod` 声明。

## 错误处理与边界

- `pass.Fset.File(f.Pos())` 返回 `None` 时，`expect("AST file position must belong to the pass FileSet")` 会 panic，而不是返回 `analysis::Error`。这是 Rust 相对 Go 指针假设的显式不变量检查。
- 没有测试文件时，`pos` 保持默认值；代码仍会求 `Position(pos).Filename` 和目录。Go 注释说明零位置通常导出空文件名；由于计数为 0，不会报告诊断，但具体目录字符串取决于兼容 `filepath::Dir` 实现。
- 文件识别只看大小写敏感后缀 `_test.go`；例如 `_test.rs`、`_test.GO` 或普通 `.go` 文件不会进入统计。
- 名称检查只要求 `Test` 前缀，没有复刻 Go `testing` 包对 `TestXxx` 后续字符的额外约束；这是源 Go 实现本身的规则，而非 Rust 简化。
- 只检查顶层 `FuncDecl` 且要求无接收者；方法即使以 `Test` 开头也不计。`TestMain` 被显式排除，`Benchmark*`、`Example*` 也不计。
- 超限是严格大于：阈值 50/90/210 都允许恰好达到该数量。
- `Reportf` 不会让 `run` 返回错误；函数签名允许 `analysis::Error`，但当前函数体没有产生 `Err` 的分支。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件句柄、网络连接或事务。每次 `run` 的局部计数和位置随调用结束释放；AST 与 `FileSet` 均由调用方的 `Pass` 拥有，本函数只在调用期间借用。

静态 `Analyzer` 具有程序级生命周期，但当前文件没有内部同步。尤其需要注意：仓库真实 [`build/linter/util/util.rs`](../util/util.rs) 中跳过函数接收 `&mut analysis::Analyzer`，而本文件 `init` 传入 `&Analyzer`；独立测试提供的桩函数则接收不可变引用。这再次表明现有验证覆盖的是迁移算法与源码契约，并不能证明本文件已能直接接入真实 Rust linter 运行时。未来若使 analyzer 配置可变，必须选择初始化期独占、`Lazy`/锁或构造函数等明确模型，并避免运行过程中并发修改共享 analyzer。

## 与 Go 版本的对应关系

Rust `Analyzer` 对应 Go 的包级 `var Analyzer = &analysis.Analyzer{...}`，字段值、无依赖声明和扫描语义保持一致；Rust 将 Go 字面量中的匿名 `Run` 闭包提取为具名 `run` 函数。Rust 的 `Ok(None)` 对应 Go 的 `return nil, nil`，`format!` 生成的诊断文本对应 Go `Reportf` 格式串。

`isTestFile` 和 `checkRule` 是逐语义对应：后缀判定、两个例外路径、90 上限旁的 TopRU 原因以及默认 50 都被保留。扫描也保持 Go 的三项条件和最后测试文件定位策略。主要显式差异是 Rust 对缺失 FileSet 映射使用 `expect`；Go 版本直接使用返回的 `*token.File`，其后续解引用同样依赖位置有效，但失败形态不是显式 Result。

接线状态不对等：Go 文件由 Bazel `go_library` 编译，并注册进 `nogo`；Rust 文件没有 Cargo crate 或生产模块入口。独立 Rust 测试的 `analyzer_uses_rust_api_shape_and_separate_run_entry`、`scan_matches_go_top_level_test_function_contract`、`package_threshold_diagnostic_and_skip_wiring_match_go` 主要通过源码契约检查迁移形状，另有桩环境中的行为测试；它们不能替代真实 `go/analysis` 兼容层的集成验证。

## 扩展指南

- 调整默认或包级阈值时修改 `checkRule`，并同步 Go 原实现（若仍保持双实现）以及 `analyze_test.rs::helper_boundaries_match_go`、`package_threshold_diagnostic_and_skip_wiring_match_go`。新增特例前应确认传入 `pkgName` 的路径形态，避免绝对/相对路径导致规则不命中。
- 改变“什么算测试”时修改 `run` 的声明条件或 `isTestFile`，并扩充独立测试覆盖 `TestMain`、方法、benchmark、非函数声明、阈值边界。Rust 单元测试应继续保留在独立 `analyze_test.rs`，不要嵌入生产源文件。
- 改变诊断位置或消息时修改 `run` 中 `pos` 更新和 `Reportf`，同步断言完整消息及位置；需要特别测试无测试文件、多测试文件和只有非测试文件的包。
- 若要真正接入 Rust linter，首先新增或选择所属 crate 与兼容 API，再在真实模块树中导出和注册 `Analyzer`，解决 `init` 对静态值可变配置的接口冲突，并增加不依赖测试桩的集成测试。不要把当前 `pkg/lib.rs` 的测试模块视作生产注册点。
- 性能上当前复杂度为所有输入文件顶层声明总数的线性扫描，额外空间为常数。扩展时应避免对每个声明重复解析路径或构建全包集合；兼容风险主要是阈值路径规范化、Go 测试命名语义和 skip wrapper 的执行顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter build/linter/toomanytests` 找到 Go/Rust 源和独立测试；文件节点完整读取 `analyze.rs` 97 行并识别 5 个符号；`query 'analyze.rs::run'` 定位到 `build/linter/toomanytests/analyze.rs:37:function:run`。`explore` 仅解析出 `run -> isTestFile/checkRule` 的局部关系，同名符号噪声较多，文件限定 callers/callees 未产生可用外部边，因此未把不存在的图边当成事实。
- 源与测试：读取了 [`analyze.rs`](./analyze.rs)、[`analyze_test.rs`](./analyze_test.rs) 全部内容；测试覆盖辅助函数边界、50/51 严格超限、顶层函数筛选、分析器 API 形状、阈值/诊断/skip 接线以及版权与占位文本约束。
- Go 对照：读取了 [`analyze.go`](./analyze.go)，逐项核对 analyzer 字段、循环、函数判定、阈值、诊断和 `init` 顺序。
- 构建与模块：读取了目录 [`BUILD.bazel`](./BUILD.bazel)、上层 [`build/BUILD.bazel`](../../BUILD.bazel)、根 [`Cargo.toml`](../../../Cargo.toml)、[`pkg/lib.rs`](../../../pkg/lib.rs) 的测试模块接线，以及 [`build/linter/util/util.rs`](../util/util.rs) 的真实 skip 函数签名；这些文件共同证明 Go 生产接线与 Rust 测试接线的边界。
- 本任务为纯文档分析，按计划不运行 Cargo。结构验证应确认目标文件存在且恰含“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”11 个固定二级标题。
