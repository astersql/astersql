# `build/linter/lll/analyzer.rs`

## 文件定位

本文件是 Go 长行检查器 [`analyzer.go`](./analyzer.go) 的 Rust 移植，位于构建期 linter 区域，而不是 SQL 请求、规划或存储运行时。它定义名为 `lll` 的 `analysis::Analyzer`，目标是读取 `analysis::Pass` 中涉及的 Go 源文件并在过长行的行首报告 `too long`（`Analyzer`、`runLll`，第 33、55、81 行）。

当前接线状态需要区分 Go 与 Rust：Go 文件由 [`BUILD.bazel`](./BUILD.bazel) 声明为公开 `go_library`，并由 [`build/BUILD.bazel`](../../BUILD.bazel) 在 `with_nogo` 选择分支加入构建 linter；Rust 文件所在目录没有 `Cargo.toml`，根 [`Cargo.toml`](../../../Cargo.toml) 也没有把它声明为生产模块。它目前仅通过根 crate 的 [`pkg/lib.rs`](../../../pkg/lib.rs) 中 `#[cfg(test)]` 的 `build_linter_lll_analyzer_test`，再由 [`analyzer_test.rs`](./analyzer_test.rs) 的 `include!("analyzer.rs")` 编译和验证。因此它是有可执行测试证据的移植实现，但尚不是 Rust 生产 linter 主链中的注册项。

## 核心职责

1. `Analyzer` 固化默认策略：最大行宽 `120`、tab 宽度 `1`，不声明前置 analyzer（`requires: &[]`），运行完成后返回 `Ok(None)`。
2. `runLll` 从 `analysis::Pass.Files` 收集真实文件名，过滤 `failpoint_binding__.go`，逐文件扫描，并把每个结果转换为 `token::Pos` 后交给 `Pass::Reportf`。
3. `getLLLIssuesForFile` 管理文件打开和缓冲读取；`scanLLLIssues` 实现可测试的逐行规则：展开 tab、忽略 `//go:` 指令和 import 行/块、按 Go 的 UTF-8 rune 计数语义判断长行。
4. `nextScannerLine` 复刻 Go `bufio.Scanner` 默认 64 KiB token 上限；`goRuneCount` 与 `findLineOffset` 分别保持无效 UTF-8 和原始字节偏移语义。
5. `init` 依次调用 `util::SkipAnalyzerByConfig` 与 `util::SkipAnalyzer`，保留 Go 包初始化时的跳过/包装接线顺序；不过当前 Rust 生产模块并未调用该函数。

## 主要符号

- `lllName: &str`：analyzer 注册名，值为 `"lll"`（第 33 行）。
- `MAX_SCAN_TOKEN_SIZE: usize`：扫描 token 上限 `64 * 1024`；为 `pub(super)`，供同一父模块中的独立测试访问（第 34 行）。
- `ScannerLine`：内部扫描状态，区分 `Eof`、已取得的 `Line(Vec<u8>)` 与 `TooLong`（第 36—40 行）。
- `goCommentDirectivePrefix`：需忽略的 Go 指令前缀 `//go:`（第 43 行）。
- `settings`：公开配置结构，含 `LineLength: i32` 与 `TabWidth: i32`；命名保持 Go 对照形状（第 46—51 行）。
- `Analyzer`：公开静态 analyzer；回调以默认 `settings` 调用 `runLll`，传播错误，并声明没有依赖 analyzer（第 55—70 行）。
- `result`：公开诊断候选，保存 `Filename`、一基行号 `Line` 和描述性 `Text`（第 73—78 行）。注意当前 `runLll` 最终固定报告 `"too long"`，不会把 `Text` 传给 `Reportf`。
- `runLll`：公开的 pass 级入口，负责文件枚举、错误传播、偏移换算与诊断提交（第 81—106 行）。
- `getLLLIssuesForFile`：公开的单文件入口，打开文件后委托 `scanLLLIssues`（第 109—118 行）。
- `scanLLLIssues`：`pub(super)` 的通用 `BufRead` 扫描核心，也是行为测试的主要入口（第 121—195 行）。
- `nextScannerLine`、`replaceTabs`、`goRuneCount`：私有字节处理辅助函数（第 198—264 行）。
- `findLineOffset`：`pub(super)` 的行首字节偏移计算函数（第 267—285 行）。
- `init`：公开注册调整入口，保持两次 `util` 调用顺序（第 288—291 行）。文件没有 trait、`impl` 或条件编译项。

## 执行流程

`Analyzer.run` 的完整路径是：

1. 构造 `settings { LineLength: 120, TabWidth: 1 }` 并调用 `runLll`。
2. `runLll` 遍历 `pass.Files`，通过 `pass.Fset.PositionFor(f.Pos(), false)` 得到文件名；空文件名和后缀为 `failpoint_binding__.go` 的生成文件不会进入扫描列表。
3. 使用 `" ".repeat(TabWidth)` 生成 tab 替换字节串，然后按文件顺序调用 `getLLLIssuesForFile`。任何一个文件的打开或扫描错误都会立即终止后续文件处理。
4. 单文件入口用 `File::open` 和 `BufReader` 建立读取器；资源在函数/作用域结束时由 Rust 自动释放。
5. `scanLLLIssues` 反复调用 `nextScannerLine`。普通行先去除末尾 LF，再去除末尾 CR，之后展开所有 tab。
6. 以 `//go:` 开头的行直接跳过；以 `import` 开头的行也跳过，若它以 `(` 结尾则进入多行 import 状态，直到内容严格等于 `)` 的行才退出。判断都发生在 tab 展开之后，但不做 trim。
7. 其余行由 `goRuneCount` 计数，只有 `lineLen > maxLineLen` 才生成 `result`，等于阈值不报告。
8. 对每个结果，`runLll` 再经 `util::ReadFile` 取得完整字节和 `token::File`，用 `findLineOffset` 找到对应行首，并报告固定文本 `too long`。

超长 token 是特殊分支：当尚未找到换行而累计长度达到 `65536` 字节时，`nextScannerLine` 返回 `TooLong`。若配置阈值小于该上限，扫描器追加一个 `Line` 为“此前成功扫描行数”的结果并停止该文件；若阈值大于或等于该上限，则返回与 Go `bufio.Scanner` 对齐的 `token too long` 错误。

## 数据与状态

- 每次 `runLll` 调用的可变状态均为局部值：文件名 `Vec<String>`、tab 替换串和逐文件结果，没有跨调用缓存。
- `scanLLLIssues` 维护 `lineNumber` 与 `multiImportEnabled`。`lineNumber` 只在成功取得普通行后递增，所以首行即超长 token 时结果行号为 `0`；独立测试明确锁定了这一 Go 兼容行为。
- 行内容始终以 `Vec<u8>`/`&[u8]` 处理。这样既不会用有损 UTF-8 转换改变字节偏移，也允许 Go 源中存在无效 UTF-8；每个无法解码的字节按一个 rune 计数。
- `result.Text` 保留详细计数（例如 `line is 121 characters` 或 `line is more than 65536 characters`），供扫描层测试和潜在调用方使用；pass 级诊断只暴露固定消息。
- `Analyzer` 是只读静态值；配置结构本身公开且字段可写，但默认回调每次创建新的固定配置。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 与源码共同确认：`Analyzer.run -> runLll -> getLLLIssuesForFile -> scanLLLIssues`；`scanLLLIssues` 调用 `nextScannerLine`、`replaceTabs`、`goRuneCount`，`runLll` 还调用 `findLineOffset`。RustCodeGraph 还显示 `scanLLLIssues` 被 [`analyzer_test.rs`](./analyzer_test.rs) 的三个行为测试直接调用，`findLineOffset` 被其中一个偏移测试调用。

下游依赖分为三组：标准库的 `File`、`BufReader`、`BufRead` 负责同步文件 I/O；仓库移植接口 `analysis`、`token` 和 `util` 提供 analyzer/pass、token 文件基址、文件读取及 analyzer 跳过逻辑；测试文件为这些接口提供最小替身后 `include!` 本实现。目标目录无独立 Cargo manifest，根 crate 只在测试配置下包含测试包装模块，故不能从当前 Cargo 接线推导出 Rust analyzer 已参与生产 nogo。

Go 侧的生产上游是 Bazel `//build:linter` 聚合目标：[`build/BUILD.bazel`](../../BUILD.bazel) 仅在 `//build:with_nogo` 时加入 `//build/linter/lll`。这条边属于 Go 实现，不是 Rust `Analyzer` 的生产调用边。

## 错误处理与边界

- 文件打不开时，`getLLLIssuesForFile` 返回 `can't open file <name>: <io error>`；读取失败时，`scanLLLIssues` 返回 `can't scan file <name>: <io error>`。
- `util::ReadFile` 失败被包装为 `can't get file <name> contents: <error>`。这些错误由 `?` 一路传播到 `Analyzer.run`，并中止剩余文件。
- `TabWidth` 转为 `usize` 使用 `expect("negative Repeat count")`，所以负值会 panic，而不是返回 `analysis::Error`；扩展可配置入口时必须先决定是否继续保持 Go `strings.Repeat` 对负数 panic 的契约。
- `maxLineLen` 使用有符号整数；负阈值会使所有未被忽略的普通行（包括空行）满足“长度大于阈值”。当前默认值不会触发此边界。
- import 识别是字节级前缀/后缀匹配：带前导空格的 ` import` 不被忽略；`import (` 之外只要以 `import` 开头也会跳过；多行块结束行必须严格为 `)`。这是 Go 原实现的直接语义，不应擅自改成语法解析。
- 64 KiB 判断使用“达到上限即 TooLong”；超长后不消费剩余 token，而且无论容忍还是报错都会停止当前文件，独立测试验证后续行不会继续处理。
- `findLineOffset` 对空内容、非正行号、超出范围及文件恰在目标行开始前结束时返回 `-1`。`runLll` 没有显式检查该哨兵；它只为扫描器实际发现的行调用此函数。`token::File` 由 `util::ReadFile` 返回裸指针，代码仅在第 99 行以 `unsafe` 解引用读取 `Base()`，安全性依赖该指针在调用期间有效。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或共享可变容器。一个 pass 内的文件和诊断按顺序处理，因此执行时间与读取文件总量、行数和每行字节数线性相关；结果向量还占用与发现的问题数成正比的内存。

`File` 被移入 `BufReader`，二者在 `getLLLIssuesForFile` 返回时自动关闭/释放，等价替代 Go 的 `defer f.Close()`。每行缓冲最多增长到 `MAX_SCAN_TOKEN_SIZE` 判定点，避免对无换行巨型行进行无界分配。`util::ReadFile` 会为每个问题重新读取对应文件；若同一文件有很多长行，这可能产生重复 I/O，是未来性能优化需要保持诊断位置语义时重点评估的地方。

静态 `Analyzer` 本身不保存运行状态；但 `init` 对 analyzer 的真实修改语义取决于生产版 `util` 实现。当前测试替身的两个 `SkipAnalyzer*` 都是无操作函数，只验证调用形状与顺序，不能作为并发安全或全局注册副作用已在 Rust 生产环境验证的证据。

## 与 Go 版本的对应关系

Rust 版本逐项保留了 [`analyzer.go`](./analyzer.go) 的关键契约：analyzer 名称/说明、默认 120/1 配置、failpoint 生成文件过滤、tab 展开、`//go:` 与 import 区域跳过、Unicode rune 计数、`bufio.Scanner` 64 KiB 分支、行首诊断以及两次 `SkipAnalyzer*` 调用顺序。

主要实现差异如下：

- Go 把扫描循环直接放在 `getLLLIssuesForFile`；Rust 抽出泛型 `scanLLLIssues<R: BufRead>`，以便用内存游标覆盖边界行为。
- Go `scanner.Text()` 给出字符串，但 `utf8.RuneCountInString` 对无效编码仍按每个错误字节一个 rune 处理；Rust 全程保留字节，并由 `goRuneCount` 显式复刻该行为。
- Go 用 `util.FindOffset(string(fileContent), line, 1)`；Rust 的 `findLineOffset` 只实现本调用需要的“列 1”字节偏移，避免无效 UTF-8 的字符串转换。
- Go 用 `defer f.Close()`；Rust 依靠所有权和析构关闭文件。
- Go 的 `Analyzer` 已由 Bazel/nogo 生产接线；Rust 版本当前通过独立测试编译，尚无等价生产注册入口。因此“行为已被测试”不等于“Rust linter 已投入生产”。

仓库中没有同目录 `analyzer_test.go`；本任务可用的 Go 语义权威是生产 `analyzer.go`。Rust 回归集中在独立的 `analyzer_test.rs`，符合测试与源文件分离要求。

## 扩展指南

- 修改默认阈值或 tab 宽度：改 `Analyzer.run` 构造的 `settings`，并同步 `analyzer_and_run_wiring_use_rust_api_shape`；若引入用户配置，还需明确负数、零和超大值的错误契约。
- 修改忽略规则：在 `scanLLLIssues` 的 directive/import 分支接入，并扩展 `scanner_skips_directives_and_import_blocks`。若从字节匹配升级为语法感知，必须评估与 Go analyzer 输出的兼容性，不能只让 Rust 测试通过。
- 修改长 token 或编码逻辑：优先修改 `nextScannerLine`/`goRuneCount`，同步 `scanner_matches_go_max_token_size_branch_and_stops` 与 `scanner_counts_runes_tabs_and_invalid_utf8_like_go`；注意内存上限和无效 UTF-8 计数都是显式兼容要求。
- 修改诊断定位：在 `findLineOffset` 与 `runLll` 接线处实施，并同步 `byte_line_offsets_match_go_for_column_one`、`files_errors_and_diagnostics_preserve_go_contract`；需特别检查空文件、末尾换行和裸指针生命周期。
- 优化重复读取：可考虑让一次文件读取同时服务扫描和位置映射，但必须保持 `analysis::Pass.Fset`/`token::File.Base()` 的位置体系以及错误文本。
- 真正接入 Rust 生产主链：需要新增明确的 crate/module 注册与真实 `analysis`、`token`、`util` 依赖，而不是复用测试替身；这属于当前文件说明之外的后续接线工作。接入后应增加独立集成测试，证明 `init` 注册效果和 `Pass::Reportf` 的实际位置。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter build/linter/lll` 列出 `analyzer.go`、`analyzer.rs`、`analyzer_test.rs`；文件节点读取确认本文件 291 行、12 个符号。
- RustCodeGraph 调用证据：`getLLLIssuesForFile` 的 Rust 调用者为 `runLll`；`scanLLLIssues` 的调用者为 `getLLLIssuesForFile` 及三个 Rust 扫描测试；`nextScannerLine`、`replaceTabs`、`goRuneCount` 由 `scanLLLIssues` 调用；`findLineOffset` 由 `runLll` 调用。自然语言全仓查询会混入同名符号，因此结论只采用带 `build/linter/lll` 路径的结果。
- 源码：[`analyzer.rs`](./analyzer.rs) 全部 291 行，核对了常量、类型、公开性、控制流、错误包装、字节处理、`unsafe` 边界和 `init`。
- Go/Bazel 对照：[`analyzer.go`](./analyzer.go)、[`BUILD.bazel`](./BUILD.bazel)、[`build/BUILD.bazel`](../../BUILD.bazel)。
- Cargo/模块边界：根 [`Cargo.toml`](../../../Cargo.toml) 的 `[lib] path = "pkg/lib.rs"`，以及 [`pkg/lib.rs`](../../../pkg/lib.rs) 中仅在 `#[cfg(test)]` 下引用 [`analyzer_test.rs`](./analyzer_test.rs) 的模块声明；目标目录不存在自己的 `Cargo.toml`。
- 独立测试：[`analyzer_test.rs`](./analyzer_test.rs) 验证 Unicode/tab/无效 UTF-8/尾 CR、directive/import 忽略、64 KiB 分支与停止语义、字节行偏移、Analyzer 接线、错误/诊断契约及有界分配实现。按任务要求本次为纯文档分析，未运行 Cargo。
