# `build/linter/forbidigo/analyzer.rs`

## 文件定位

本文件位于构建辅助目录 `build/linter/forbidigo/`，是同目录 Go analyzer（[`analyzer.go`](analyzer.go)）的机械迁移草稿。源码第 15—18 行明确声明它目前“不保证可编译”，不会真正接入 Go analysis 框架、打开仓库文件或执行业务动作。因此，它描述的是 forbidigo 检查器的 Rust API 形状和预期数据流，而不是当前 AsterSQL Rust 运行时的一部分。

当前生产接线仍属于 Go/Bazel：[`BUILD.bazel`](BUILD.bazel) 只把 `analyzer.go` 放进 `go_library(name = "forbidigo")`；上层 `build/BUILD.bazel` 将 `//build/linter/forbidigo` 加入 nogo analyzer 集合；`build/nogo_config.json` 再为名为 `forbidigo` 的 analyzer 配置排除路径。根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员没有 `build/linter/forbidigo`，该目录也没有自己的 `Cargo.toml`。Rust 侧只有 `pkg/lib.rs` 在 `#[cfg(test)]` 下通过路径挂载 [`analyzer_test.rs`](analyzer_test.rs)，而该测试用 `include_str!` 检查源码文本，并不编译本文件。

## 核心职责

按当前草稿表达的设计，本文件把一条针对 `sessionctx.Context.GetSessionVars` 的 forbidigo 规则包装成 `analysis::Analyzer`，并负责三件事：

1. `Analyzer` 注册 analyzer 名称、说明和 `run` 回调。
2. `run` 创建外部 forbidigo linter，向它提供当前 analysis pass 的 AST、文件集和类型信息，再收集 issue。
3. `reportIssues` 读取 issue 所在源码行，按 TiDB/AsterSQL 的允许成员名单消除误报，并把其余 issue 报告为 `restriction` 诊断。

`init` 还保留了 Go 包初始化时按仓库配置跳过 analyzer 的接线意图。以上职责均可在 `Analyzer`、`patterns`、`run`、`reportIssues` 和 `init` 五处代码中复核；“当前可运行”则没有 Cargo 或模块接线证据，不能据此推断。

## 主要符号

- `lc: Lazy<fsutils::LineCache>`：进程级惰性行缓存，由 `NewFileCache` 构造底层文件缓存。它只被 `reportIssues` 用于按 issue 的文件名和行号取得整行文本。
- `Analyzer: analysis::Analyzer`：公开静态 analyzer 描述符，`name` 为 `forbidigo`，`doc` 为 `forbid identifiers`，没有前置 analyzer（`requires: &[]`），回调指向 `run`。独立测试明确要求它是直接静态值，而不是多余的 `Lazy<analysis::Analyzer>`。
- `patterns: Lazy<Vec<String>>`：惰性创建的规则列表，目前只有一条 forbidigo DSL 规则，匹配 `sessionctx.Context.GetSessionVars`，并给出检查合理性及 `//nolint:forbidigo` 逃生口提示。
- `run(pass: &mut analysis::Pass) -> anyhow::Result<Option<Box<dyn Any>>>`：analyzer 主回调。成功返回 `Ok(None)`，对应 Go 的 `(nil, nil)`；配置和运行错误沿返回值传播。
- `reportIssues(pass, issues)`：内部诊断过滤与上报阶段；虽然当前声明为 `pub`，没有发现本文件外调用证据。
- `init()`：调用 `util::SkipAnalyzerByConfig(&Analyzer)`，保留 Go `init` 的配置跳过语义。Rust 本身不会自动调用普通的 `init` 函数；当前也没有模块接线或显式调用证据。

文件没有自定义 `struct`、`enum`、`trait`、`impl` 或条件编译项。RustCodeGraph 将本文件识别为 4 个主要符号，且文件级结果显示 `used by 0 files`；对 `run`、`reportIssues`、`init` 的 callers/callees 查询未返回跨文件调用边，这与未接线草稿的现状一致。

## 执行流程

如果未来有宿主按 `Analyzer.run` 调用本文件，流程如下：

1. `run` 用 `patterns` 和三个选项构造 forbidigo linter：允许 permit/nolint 指令、不过滤 Go doc example、启用类型分析。
2. 若规则配置失败，错误被包装为带有 `failed to configure linter` 上下文的 `anyhow::Error`，并保留原始错误源。
3. 以 `pass.Files.len()` 预分配 `nodes`，逐个把原始文件 AST 通过 `as_node()` 放入列表。这里刻意不克隆 AST，以维持 `TypesInfo` 以 AST 节点身份为键时的对应关系。
4. `RunConfig` 借用 `pass.Fset` 和 `pass.TypesInfo`，关闭调试日志；`RunWithConfig(config, nodes)` 执行规则。执行错误由 `?` 原样向上传播。
5. `reportIssues` 对每个 issue 用全局 `lc` 读取其源码行，并检查该行是否含有任一白名单字符串。命中即跳过当前 issue；未命中则调用 `pass.Report`，使用 issue 的位置与详情并设置类别 `restriction`。
6. 全部 issue 处理后，`run` 返回 `Ok(None)`，不产生 analysis result。

初始化流程与单次运行分开：`init` 只请求 `util::SkipAnalyzerByConfig` 根据配置禁用 `Analyzer`，不创建 linter，也不扫描文件。由于没有 Rust 调用者，这一流程当前仅是移植形状。

## 数据与状态

文件包含两份进程级惰性状态：`lc` 缓存文件内容/源码行，`patterns` 保存拥有所有权的规则字符串。`Analyzer` 是静态描述数据。单次调用状态则局限于栈上的 `nodes`、`RunConfig`、`issues`、每个 issue 的 `skip` 标志和函数内新建的 `whiteLists`。

白名单是纯字符串包含判断，当前值为 `SQLMode`、`CDCWriteSource`、`StmtCtx`、`TimeZone`、`Location`、`GetSplitRegionTimeout`、`SetInTxn`、`BuildParserConfig`、`DiskFull`、`DefaultCollationForUTF8MB4`、`RowEncoder`、`SetStatusFlag`。它不解析表达式结构，也不证明该成员确实属于 `GetSessionVars()` 调用链；其语义是对已由 forbidigo 发现的 issue 做行级二次过滤。

重要不变量是 AST 节点和 `TypesInfo` 必须来自同一 `analysis::Pass`。`analyzer_test.rs::run_uses_original_ast_and_shared_type_context` 通过禁止 clone 形式和要求共享借用，固化了这一点。另一个不变量是读取源码行失败不能让 issue 消失：`GetLine` 只有成功时才检查白名单，失败时 `skip` 保持 `false`。

## 依赖与调用关系

上游设计入口是 `Analyzer.run -> run`；配置阶段另有 `init -> util::SkipAnalyzerByConfig(&Analyzer)`。RustCodeGraph 没有找到实际 Rust 上游调用者。真实生产上游在 Go/Bazel：`build/BUILD.bazel` 聚合 `//build/linter/forbidigo`，nogo 再按 `build/nogo_config.json` 的 `forbidigo.exclude_files` 决定哪些文件不参与检查。

`run` 的主要下游关系为：

- `forbidigo::NewLinter`：解析 `patterns` 并应用三个 option；
- `ast::Node` / `f.as_node()`：把 pass 中的文件 AST 作为 linter 输入；
- `forbidigo::RunConfig` 与 `RunWithConfig`：传递文件集、类型信息并产生 issues；
- `reportIssues`：执行仓库特有的白名单策略；
- `analysis::Pass::Report`：提交最终诊断。

`reportIssues` 依赖 `fsutils::LineCache::GetLine`、`forbidigo::Issue::{Position, Pos, Details}` 和字符串 `contains`。Go 的真实依赖版本由 `go.mod` 固定为 `github.com/ashanbrown/forbidigo/v2 v2.3.0`，Bazel 依赖由本目录 `BUILD.bazel` 声明；根 Cargo manifest 没有为这些 Rust 命名空间声明依赖，因此不能把它们视为已存在的 Rust crate API。

## 错误处理与边界

`NewLinter` 失败时，`run` 添加固定上下文并保留错误链；`RunWithConfig` 失败时直接用 `?` 传播。任一错误都会阻止诊断过滤和 `Ok(None)` 返回。`reportIssues` 本身不返回错误：读取某行失败会采用保守策略继续报告该 issue，避免因缓存/文件读取问题形成漏报。

白名单判断的边界是“同一源码行包含子串”，因此可能把注释、字符串字面量或同名标识符造成的命中也当成允许用法；跨行调用也可能无法在 issue 所在行找到允许成员。该策略是 Go 原实现为弥补正则不支持 negative lookahead 而采用的局部折中，不应描述为语法级精确过滤。

`OptionIgnorePermitDirectives(true)` 与提示文字共同表明显式 `//nolint:forbidigo` 是设计允许的人工豁免途径。仓库里确有这类标记，但是否生效取决于真实运行的 Go analyzer；当前 Rust 草稿不会执行它们。空文件列表将产生空 `nodes` 并交给外部 linter；本文件没有单独特判。`Analyzer.requires` 为空，但启用了类型分析并直接借用 `pass.TypesInfo`，其完整性由宿主 analysis pass 保证。

## 并发与资源生命周期

本文件没有显式线程、异步任务、通道、锁或事务。`linter`、`nodes`、`config` 和 `issues` 均在一次 `run` 调用范围内创建并释放；`RunConfig` 对 pass 数据的借用不应逃逸出该调用。`pass` 以可变引用传入，最终诊断按当前循环顺序同步报告。

`lc` 和 `patterns` 是静态惰性对象，首次访问时初始化，之后跨调用复用；其中 `patterns` 初始化后只读。`lc` 的缓存复用有减少重复文件读取的意图，但它的线程安全、淘汰策略和文件变更一致性取决于尚未在 Cargo 中落实的 `fsutils::LineCache` 对应实现，本文件及源码字符串测试没有给出可验证保证，故这些性质均属未验证。当前 Rust 文件未编译接线，也没有可据以证明多线程调用安全的运行测试。

## 与 Go 版本的对应关系

同目录 `analyzer.go` 是当前权威、可接线的实现。Rust 草稿逐项保留了 Go 结构：`lc`、`Analyzer`、`patterns`、`run`、`reportIssues`、`init`；规则文本、三个 linter option、十二项白名单、诊断位置/消息/类别也保持一致。

主要表示差异如下：Go 的 `Analyzer` 是 `*analysis.Analyzer`，Rust 写成直接静态 `analysis::Analyzer`；Go 的切片和可变长度参数在 Rust 中分别表现为 `Vec`/slice 与 `nodes` 参数；Go `(any, error)` 的成功空结果表现为 `anyhow::Result<Option<Box<dyn Any>>>` 的 `Ok(None)`；Go 用 `%w` 包装错误，Rust 用 `anyhow::Error::new(err).context(...)` 保留错误源；Go 的包初始化会自动运行 `init`，Rust 普通函数不会自动执行。

还有关键迁移差距：Go 依赖真实的 `golang.org/x/tools/go/analysis`、forbidigo 和 golangci-lint fsutils，且由 Bazel 构建；Rust 文件使用相似命名空间表达 API，但没有 Cargo crate 边界、依赖声明或生产模块入口。独立 Rust 测试只检查源文本包含/不包含若干片段，能够证明迁移意图和关键形状保持，却不能证明类型正确、可编译或运行结果与 Go 等价。

## 扩展指南

- 新增或调整禁止规则时修改 `patterns`，同时核对 forbidigo DSL、错误消息和 `OptionAnalyzeTypes` 所需上下文；在 `analyzer_test.rs` 增加对应的源码结构断言，并在 Go 仍为生产实现期间同步评估 `analyzer.go`，避免两份规则漂移。
- 调整允许用法时修改 `reportIssues` 的 `whiteLists`。应优先考虑误报与漏报：行级子串可能过宽，若要改成 AST/类型级过滤，接入点仍应位于 forbidigo 产生 issue 之后、`pass.Report` 之前，并补独立测试覆盖同名字符串、注释、跨行链式调用和读取行失败。
- 改变诊断协议时修改 `analysis::Diagnostic` 构造处，并保持 `Pos`、`Message` 和 `Category` 与上游工具/配置的消费约定一致；`build/nogo_config.json` 以 analyzer 名 `forbidigo` 配置排除项，重命名 `Analyzer.name` 必须同步该配置及 Bazel 聚合关系。
- 若要把草稿变成真正 Rust 实现，不能只解除注释或增加模块声明；需要先建立独立 Cargo crate/API 依赖，确认 `analysis`、`forbidigo`、`fsutils`、`ast`、`util` 的真实 Rust 实现和线程安全约束，再提供调用 `init`/注册 `Analyzer` 的宿主入口。生产代码测试应放在独立测试文件，不能内嵌进 `analyzer.rs`。
- 性能上应关注每个 issue 的源码行读取和 `O(issue 数 × 白名单数)` 子串扫描；目前白名单很小，且行缓存意图降低 I/O。扩大规则或白名单前应测量真实 issue 规模，不应基于当前草稿宣称性能保证。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter build/linter/forbidigo` 找到 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`；`node --file build/linter/forbidigo/analyzer.rs --offset 1 --limit 260` 返回全部 131 行、4 个主要符号并标记 `used by 0 files`；对 `run`、`reportIssues`、`init` 执行 callers/callees 查询未得到跨文件边。
- 源码与对照：完整阅读 [`analyzer.rs`](analyzer.rs) 和 [`analyzer.go`](analyzer.go)，逐项核对 analyzer 描述、规则、选项、AST/类型上下文、白名单、错误传播和配置跳过逻辑。
- crate/构建边界：检查根 [`Cargo.toml`](../../../Cargo.toml)、本目录 [`BUILD.bazel`](BUILD.bazel)、`build/BUILD.bazel`、`build/nogo_config.json`、`go.mod` 和 `pkg/lib.rs`；证据表明 Go analyzer 有生产接线，Rust 文件没有独立 Cargo crate，只有测试模块间接读取其文本。
- 测试：完整阅读 [`analyzer_test.rs`](analyzer_test.rs)。四个测试分别固定 analyzer 静态形状、linter 配置与错误源、原 AST/共享类型上下文、白名单及 restriction 诊断；它们不编译或执行 `analyzer.rs`。
- 按任务约束未运行 Cargo 或 Go 测试；本任务仅新增说明文档。交付前另运行任务指定的 11 章节结构检查，并人工复核本文没有把未接线草稿描述成已支持的生产 Rust 功能。
