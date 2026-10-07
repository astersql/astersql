# `build/linter/misspell/analyzer.rs` 逻辑说明

## 文件定位

`build/linter/misspell/analyzer.rs` 是 Go 文件 `build/linter/misspell/analyzer.go` 的 Rust 语义迁移稿，描述一个名为 `misspell` 的 `analysis::Analyzer`。它属于开发/构建辅助目录 `build/linter`，目标是检查 Go 源文件中的英文拼写，而不是 SQL 请求、规划或执行链的一部分（证据：`Name`、`Analyzer`、文件头迁移说明）。

当前 Rust 文件尚未接入生产 crate：`build/linter/misspell/` 下没有 `Cargo.toml` 或 Rust 模块入口，根 `Cargo.toml` 的 workspace 成员和依赖也没有为它声明 crate 或 `misspell` 依赖。仓库中唯一 Rust 接线是根 crate `pkg/lib.rs` 在 `#[cfg(test)]` 下以路径模块加载 `build/linter/misspell/analyzer_test.rs`，后者再用 `include!("analyzer.rs")` 编译目标文件。因此本文区分“代码表达的预期行为”和“当前实际接线”，不把测试内的桩模块当成生产实现。

Go 版本则已接入真实构建链：`build/linter/misspell/BUILD.bazel` 定义 `go_library(name = "misspell")`，`build/BUILD.bazel` 将 `//build/linter/misspell` 放入 `nogo` 分析器依赖，`build/nogo_config.json` 为名称 `misspell` 配置排除路径。

## 核心职责

- 用 `Name` 和静态 `Analyzer` 描述分析器元数据，把执行回调绑定到 `run`，且不声明前置分析器（`requires: &[]`）。
- `init` 表达两层注册期包装意图：`util::SkipAnalyzerByConfig` 处理配置排除文件，`util::SkipAnalyzer` 处理通用的文件/行级忽略指令。
- `run` 从主词典构造并编译 `misspell::Replacer`，收集 `analysis::Pass.Files` 对应的文件名，再逐文件调用 `runOnFile`。
- `runOnFile` 读取原始文件字节，使用纯文本 `Replace`（而非只检查注释的 `ReplaceGo`），把每条拼写差异转换成 `token::Pos` 并通过 `Pass::Reportf` 上报。
- `sanitizeForMisspell`、`findOffset` 和 `goRuneWidth` 共同桥接 Rust UTF-8 字符串要求、misspell 的字节列偏移和 Go 按 rune 计列的定位语义。

## 主要符号

- `pub const Name: &str = "misspell"`：诊断器稳定名称，同时用于诊断前缀和配置键。
- `pub static Analyzer: analysis::Analyzer`：公开分析器描述，名称为 `misspell`，说明为 `Checks the spelling error in code`，回调为 `run`。当前字段形状是迁移稿使用的小写 Rust 风格字段；实际仓库 `build/linter/util/util.rs` 中的 `analysis::Analyzer` 接口使用另一套字段/可变包装方式，且目标文件没有生产模块接线，不能仅凭该静态值断言可直接用于真实 Rust linter。
- `pub fn init()`：表达与 Go 包初始化相同的两个跳过包装调用顺序。目标签名传入 `&Analyzer`，而实际 `util.rs` 的两个函数签名均接收 `&mut analysis::Analyzer`；独立测试以只读参数桩替代了真实函数，所以现有测试不能证明这段生产接线可编译。
- `pub struct Misspell { Locale, IgnoreWords }`：配置模型。`IgnoreWords` 对应 Go 的 `mapstructure:"ignore-words"` 字段；`Locale` 当前只保存而未参与替换规则选择。
- `pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error>`：分析器主入口；成功时返回 `Ok(None)`，首个文件错误会短路返回。
- `pub fn runOnFile(fileName, r, pass)`：单文件 I/O、替换检查和诊断上报入口。
- `pub(super) fn sanitizeForMisspell(&[u8]) -> String`：把每个非法 UTF-8 字节一对一改成 NUL；保持总字节长度和合法 UTF-8 片段不变，使 `Replace(&str)` 可处理任意文件字节。
- `pub(super) fn findOffset(&[u8], line, column) -> i32`：把一基行号和按 Go rune 计数的一基列号转换成零基字节偏移；找不到时返回 `-1`。
- `fn goRuneWidth(&[u8]) -> usize`：返回首个合法 Unicode 标量的 UTF-8 宽度；若开头是非法字节则返回 `1`，模拟 Go 遍历无效 UTF-8 时逐字节推进。

文件中没有 trait、`impl` 块、宏或条件编译项；条件编译发生在外部的 `pkg/lib.rs` 测试模块声明处。

## 执行流程

1. 预期注册阶段调用 `init`，先应用 `SkipAnalyzerByConfig`，再应用 `SkipAnalyzer`。Go 生产链据此让配置和源码指令过滤分析范围；Rust 当前没有生产注册入口。
2. 分析框架调用 `Analyzer.run` 指向的 `run`。函数复制 `misspell::DictMain` 到新的 `Replacer.Replacements`，随后创建 `Locale` 和 `IgnoreWords` 都为空的默认 `Misspell`。
3. 只有 `IgnoreWords` 非空才调用 `RemoveRule`；以当前硬编码默认值，此分支不会执行。随后无条件 `Compile` 替换规则。`Locale` 当前完全未读，因此注释中的 regional variations 尚未落实成行为。
4. `run` 先从 `pass.Files` 的 AST 文件位置解析出文件名并放入独立 `Vec<String>`，再逐一可变借用 `pass` 调用 `runOnFile`。这种两阶段处理避免遍历 `pass.Files` 时同时把整个 `pass` 可变借给下游。
5. `runOnFile` 通过 `util::ReadFile(&mut pass.Fset, fileName)` 得到原始字节和 token 文件裸指针。读取失败被加上文件名上下文后返回，`run` 用 `?` 停止处理后续文件。
6. 文件字节先经 `sanitizeForMisspell` 变成有效 UTF-8，再交给 `Replacer::Replace`。选择 `Replace` 意味着注释、标识符附近文本和字符串字面量都按普通文本扫描；返回的修正文案 `_updated` 被丢弃，只消费 `diffs`。
7. 每条差异用 `findOffset` 从行/列换算成原始文件字节偏移，加到 `token::File.Base()` 上形成 `token::Pos`，再报告 ``[misspell] `原词` is a misspelling of `修正词` ``。
8. 全部文件完成后返回 `Ok(None)`，分析器不产生供其他分析器消费的结果对象。

## 数据与状态

`Analyzer` 和 `Name` 是进程级静态元数据；`Misspell`、`Replacer`、文件名列表和清洗后的字符串均为单次 `run` 调用的局部数据。`Replacer` 每次运行都从 `DictMain` 新建并编译，没有跨 pass 缓存或共享可变词典。

`settings` 当前不是从配置文件反序列化，而是直接构造空 `String`/空 `Vec`。因此 `IgnoreWords` 数据模型和 `RemoveRule` 调用点虽已保留，当前运行路径不会忽略任何用户配置词；`Locale` 也不改变词典。这一点与结构测试只检查源码形状相符，不能描述为“配置已可用”。

`files` 预分配 `pass.Files.len()` 容量并拥有文件名，代价约与输入文件数线性相关。每个文件还会分配原始字节缓冲、等长清洗副本和由 `Replace` 返回的更新字符串/差异集合；目标文件没有复用这些缓冲。

## 依赖与调用关系

预期上游是 `analysis` 框架：`Analyzer.run -> run`。文件内确认的直接边为 `run -> runOnFile`、`runOnFile -> util::ReadFile`、`runOnFile -> sanitizeForMisspell`、`runOnFile -> misspell::Replacer::Replace`、`runOnFile -> findOffset`、`runOnFile -> analysis::Pass::Reportf`、`findOffset -> goRuneWidth`。`init` 还指向 `util::SkipAnalyzerByConfig` 和 `util::SkipAnalyzer`。

Go 生产调用关系由 Bazel 完成：`build/BUILD.bazel` 的 `nogo` 目标依赖 `//build/linter/misspell`，该库再依赖 `//build/linter/util`、`@com_github_golangci_misspell//:misspell` 和 `@org_golang_x_tools//go/analysis`。配置过滤来自 `build/nogo_config.json` 的 `misspell.exclude_files`。

Rust 侧根 `Cargo.toml` 没有对应 crate、feature 或第三方依赖声明；`pkg/lib.rs -> analyzer_test.rs -> include!(analyzer.rs)` 是唯一确认的 Rust 上游，而且只在测试构建中成立。测试自建 `analysis`、`token`、`misspell`、`util` 桩，因此没有验证真实外部 crate ABI 或实际 linter 注册。RustCodeGraph 的文件视图确认 `analyzer.rs` 被测试文件使用；对关键符号执行的 `callers/callees` 查询在本地索引上超时，本文未把该超时解释为“无调用边”，而是以源码和模块接线搜索补证。

## 错误处理与边界

- `ReadFile` 错误被映射为 `can't get file <name> contents: <err>`，并由 `?` 原样向 `run` 传播；此前文件可能已经上报诊断，因此处理不是事务性的，也没有回滚。
- `sanitizeForMisspell` 保证对任意字节输入生成有效 UTF-8，并一对一替换非法字节，避免 `String::from_utf8_lossy` 用多字节替换字符破坏列偏移。末尾 `expect` 依赖前述循环的不变量；若实现保持逐个替换所有非法字节，该断言应成立。
- `findOffset` 对行列越界、列为零、空文件或 EOF 位置均可能返回 `-1`。`runOnFile` 未检查该哨兵值，直接把它与文件 base 相加；当前正确性依赖 misspell 只返回可定位到输入内容内部的差异。
- 行处理只特殊识别字节 `\n`；CRLF 中的 `\r` 会作为普通 rune 计列，这与 Go `util.FindOffset` 的逐 rune 遍历方式一致。
- `goRuneWidth` 只由 `findOffset` 在非空切片上调用，故合法 UTF-8 分支的 `expect("caller excludes empty input")` 依赖调用方循环条件。
- `runOnFile` 通过 `unsafe { (*tf).Base() }` 解引用 `ReadFile` 返回的裸指针。安全性依赖 `ReadFile` 返回非空、仍有效且指向同一 `FileSet` 所拥有的 token 文件；目标函数没有运行时检查这一契约。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或长期句柄。`run` 和 `runOnFile` 都是同步串行流程：同一个 `Replacer` 以共享引用供各文件顺序使用，同一个可变 `Pass` 顺序接收诊断。

`Replacer`、配置、文件名列表和所有文件缓冲在 `run` 返回时释放。token 文件裸指针只在 `runOnFile` 内读取一次 base；它的有效期隐含绑定到 `pass.Fset`，但 Rust 类型系统无法从 `*mut token::File` 验证该生命周期。未来若并行化文件扫描，必须确认 `Pass.Reportf`、`FileSet`、`Replacer` 和裸指针是否可跨线程安全使用，并保持诊断顺序是否需要稳定；当前实现不提供这些保证。

## 与 Go 版本的对应关系

两版一致的主干语义包括：相同的 `Name`/说明文案、空依赖分析器集合、同顺序的两种 skip 包装、从 `DictMain` 建 replacer、可选删除忽略词、先编译再遍历 `pass.Files`、使用 `Replace` 覆盖字符串内容、读取失败立即返回，以及相同诊断格式（对照 `build/linter/misspell/analyzer.go` 的 `Analyzer`、`run`、`runOnFile`）。

Rust 为保持任意文件字节和定位语义增加了 Go 文件中没有的三个帮助函数：Go 可直接把任意字节转换为 `string` 并按 rune 遍历；Rust 的 `Replace` 需要 `&str`，所以先把非法字节等长替换，再用 `goRuneWidth` 模拟 Go 对合法 rune/非法单字节的推进。`analyzer_test.rs` 用合法多字节字符和 `0xff`/`0xfe` 覆盖了这层差异。

尚未完成或存在接口差异的部分是：Rust 配置仍硬编码为空，`Locale` 两版都没有实际读取；Rust 没有 Cargo 依赖和生产注册；静态 `Analyzer`/`init` 的签名与仓库真实 `util.rs` 可变分析器接口不一致。Go 版本由 Bazel 构建和运行，Rust 测试则通过局部桩验证源码形状与字节帮助函数，二者的可运行证据不能互换。

## 扩展指南

- 接入真实 Rust linter 前，应先在明确的 crate/module 中声明 `analysis`、`token`、misspell 实现和 `util` 依赖，并统一 `Analyzer` 字段模型以及 skip 包装所需的可变初始化机制；同步修改 `pkg/lib.rs` 的接线测试，不能只扩充测试桩。
- 若要支持配置，最可能修改 `Misspell` 和 `run`：把 `Locale`/`IgnoreWords` 从真实配置注入，明确 locale 如何选择或追加词典，并保留 `RemoveRule` 发生在 `Compile` 之前的不变量。测试应增加非空忽略词、不同 locale、重复/未知词以及配置反序列化键 `ignore-words` 的独立用例。
- 若改变扫描范围，在 `runOnFile` 的 `Replace`/`ReplaceGo` 选择处接入，并同步验证字符串、注释、标识符、生成文件和 `build/nogo_config.json` 排除规则，避免无意减少 Go 版本现有覆盖。
- 若修改定位算法，应同步更新 `sanitizeForMisspell`、`findOffset`、`goRuneWidth` 与 `build/linter/misspell/analyzer_test.rs`；至少覆盖 ASCII、中文等多字节字符、CRLF、空文件、EOF、非法 UTF-8 连续字节和越界行列。还应决定 `-1` 是否改为显式错误，避免构造无效 `token::Pos`。
- 若考虑并行化或缓存 replacer，需先证明外部 replacer 可共享、`Pass.Reportf` 可并发、诊断顺序可接受，并评估每文件复制和 `_updated` 无用分配的性能；在没有这些证据前保持串行行为与 Go 版一致。
- 回归测试应继续独立放在同目录 `analyzer_test.rs`，不要嵌入生产文件；生产接线建立后还需要能使用真实依赖的集成/编译测试，因为当前桩测试无法发现真实接口漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter build/linter/misspell` 找到 Go 源、Rust 源和 Rust 测试；`node --file build/linter/misspell/analyzer.rs` 读取完整 169 行并显示测试文件使用；`query` 确认 `runOnFile`、`sanitizeForMisspell`、`findOffset`、`goRuneWidth`。`callers/callees` 对目标符号两次在 30 秒内未返回，因此直接调用边又以源文件和 `rg` 接线搜索核验。
- 生产与对照源码：`build/linter/misspell/analyzer.rs`、`build/linter/misspell/analyzer.go`、`build/linter/util/util.rs`。
- 构建/配置证据：根 `Cargo.toml`、`build/linter/misspell/BUILD.bazel`、`build/BUILD.bazel`、`build/nogo_config.json`。根 Cargo 只接入测试模块所需的主 crate，本目录无最近的 `Cargo.toml`，Rust 生产依赖未声明。
- 测试与模块接线：`build/linter/misspell/analyzer_test.rs`、`pkg/lib.rs`。现有测试覆盖等长 UTF-8 清洗、Go 风格偏移、关键源码形状和禁止 lossy 转换；没有 Go 专属 `*_test.go`，也没有用真实分析框架执行整个 analyzer 的 Rust 集成测试。
- 人工复核结论：该文件存在是为了保存 misspell analyzer 的 Rust 移植语义；实际流程是“构造/编译 replacer—逐文件读取—纯文本扫描—换算 token 位置—报告诊断”；安全扩展必须同时处理真实 Cargo/模块接线、配置注入、定位边界和独立测试，而不能把当前桩测试当作生产可用证明。
