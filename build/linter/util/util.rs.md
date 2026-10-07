# `build/linter/util/util.rs`

## 文件定位

本文件是 `build/linter` 下多个 Go 静态分析器共用辅助层的 Rust 移植，对照实现是同目录的 [`util.go`](./util.go)。它处理 linter 指令、分析器包装、源文件与位置映射等基础能力，位于“注册 analyzer → 筛选输入文件 → 运行第三方检查器 → 把结果映射成 `analysis::Diagnostic`”链路中，不属于 TiDB/AsterSQL 的 SQL 请求运行时。

当前接线边界需要特别区分：根 [`Cargo.toml`](../../../Cargo.toml) 的 workspace 成员中没有 `build/linter`，同目录 [`BUILD.bazel`](./BUILD.bazel) 的 `go_library` 也只列出 `exclude.go` 和 `util.go`。因此 `util.rs` 目前不是正式 Cargo/Bazel 生产目标；其可执行语义由独立的 [`util_test.rs`](./util_test.rs) 用模拟的 Go AST/analysis API 和 `include!("util.rs")` 聚焦验证。仓库中的 Rust linter 移植文件会直接写出 `util::SkipAnalyzer`、`util::ReadFile` 等调用点，但其中部分文件仍是迁移草稿，不能据此宣称整套 Rust linter 已接入应用构建。

## 核心职责

- 解析附着在 Go AST 节点上的 `//lint:ignore`、`//lint:file-ignore` 和 `//nolint:<checks>` 指令，并通过静态 `Directives` analyzer 把结果提供给其他 analyzer（`parseDirective`、`ParseDirectives`、`doDirectives`、`Directives`）。
- 改写 analyzer 的 `Run` 回调：`SkipAnalyzer` 同时过滤整文件和拦截特定行、特定检查器的诊断；`SkipAnalyzerByConfig` 则按 `exclude.rs::shouldRun` 过滤传入文件。
- 为外部 linter 提供适配工具：Markdown 风格代码格式化、`analysis::Pass` 到 `loader::PackageInfo` 的转换、源文件读取并注册进 `token::FileSet`、Go 行/字符列到字节偏移的转换，以及 import 路径到包名的解析。
- 尽量保持 [`util.go`](./util.go) 的行为与失败方式。例如缺失或类型错误的 `Directives` 结果会 panic，非法 UTF-8 在偏移计算中按单字节前进，文件打开/读取错误保留操作和路径上下文。

## 主要符号

- `skipType::{skipNone, skipLinter, skipFile}`：内部指令分类。虽然枚举公开，但变体命名与 Go 常量保持一致；数值分别为 0、1、2。
- `Directive { Command, Linters, Directive, Node }`：保存解析结果、原始 `*mut ast::Comment` 和指令关联的 `ast::Node`。后续真正用于抑制判断的是 `Command`、`Linters` 与 `Node`；原始注释指针用于保留 Go 数据形状。
- `parseDirective(String)`：内部解析器。`//lint:ignore` 和 `//lint:file-ignore` 使用空格切分并返回余下参数；未知 `//lint:` 命令返回 `skipNone`；其他输入按 `//nolint:` 语义成为单项 linter 字符串。
- `ParseDirectives(Vec<*mut ast::File>, &token::FileSet)`：公开收集入口。它借助 `ast::NewCommentMap` 只读取与 AST 节点关联的注释，并忽略不以两种受支持前缀开头的注释。
- `doDirectives`、`directivesResultType`、`Directives`：组成名为 `directives` 的前置 analyzer。它没有依赖项、即使源代码已有错误仍运行，并声明结果类型为 `Vec<Directive>`。
- `SkipAnalyzer(&mut analysis::Analyzer)`：为 analyzer 增加 `Directives` 依赖并接管其原始 `Run`。这是指令级跳过规则的核心入口。
- `SkipAnalyzerByConfig(&mut analysis::Analyzer)`：用相邻 [`exclude.rs`](./exclude.rs) 的 `shouldRun(analyzer.Name, filename)` 筛选 `Pass.Files`，然后调用原始 `Run`。
- `FormatCode(&str) -> String`：若文本不含反引号则包上一对反引号；若已含反引号则原样返回，避免当前尚未实现的转义问题。
- `MakeFakeLoaderPackageInfo(&analysis::Pass)`：把当前包、文件和类型信息装配成外部工具期待的 `loader::PackageInfo`，并把 `Importable`、`TransitivelyErrorFree` 设为 `true`、错误列表置空。
- `ReadFileError`、`ReadFile`：分别是带 `Op`、`Path`、原始 `std::io::Error` 的错误类型，以及读取原始字节并在 `FileSet` 中登记行表的函数。
- `FindOffset(&[u8], i32, i32)`、`goRuneLen`：按一基行号和 rune 列扫描原始字节，返回零基字节偏移；找不到时返回 `-1`。内部 UTF-8 宽度判断让合法多字节字符占一个列，非法首字节或截断序列占一个字节。
- `GetPackageName(Vec<*mut ast::ImportSpec>, path, defaultName)`：按带双引号的 import path 精确匹配；显式别名优先，否则返回默认包名，未导入则返回空字符串。

## 执行流程

指令链从 `Directives.Run = doDirectives` 开始。`doDirectives` 把 `pass.Files` 和 `pass.Fset` 交给 `ParseDirectives`；后者逐文件建立 comment map，遍历节点、注释组和单条注释，仅把支持的前缀交给 `parseDirective`，最后把 `Vec<Directive>` 装箱返回。

调用 `SkipAnalyzer` 时，函数先把 `&Directives` 加入目标 analyzer 的 `Requires`，再取走原始 `Run` 并安装闭包。闭包运行时复制 `Pass`，从 `ResultOf` 以 `Directives` 的指针身份取回结果，先把所有 `skipFile` 所在文件加入 `HashSet`，再从副本的 `Files` 中剔除这些文件。随后它替换 `Report`：对于 `skipLinter`，仅当指令节点与诊断处于同一文件、同一行，且逗号分隔并去空白后的检查器名等于当前 analyzer 名称时吞掉诊断；对于 `skipFile`，目标文件在集合中时吞掉诊断；其他诊断转交原始 `Report`。最后用修改后的 pass 调用原始 analyzer。

`SkipAnalyzerByConfig` 是较窄的包装器：复制 pass，以每个 AST 文件的起始位置取得文件名，用 `shouldRun` 过滤 `Files`，然后调用原始 `Run`。若两种包装都应用，调用顺序决定闭包嵌套顺序；仓库中的 `misspell`、`gosec`、`revive` 等迁移文件通常先调用配置包装，再调用指令包装。

外部检查器的位置映射通常是 `ReadFile` 读取文件并将其加入 `FileSet`，再用 `FindOffset` 把外部工具的一基行/rune 列转成字节偏移，最后与 `token::File::Base()` 相加得到诊断位置。`gosec/analysis.rs` 还使用 `MakeFakeLoaderPackageInfo` 为 gosec 构造 loader 输入；`deferrecover/analyzer.rs` 和 `etcdconfig/analyzer.rs` 使用 `GetPackageName` 识别默认或别名导入。

## 数据与状态

文件本身没有可变全局业务状态；唯一静态值 `Directives` 是 analyzer 描述符。运行态数据集中在每次 `analysis::Pass` 的浅克隆和闭包捕获中：`SkipAnalyzer` 捕获 analyzer 名称与原始 `Run`，每次执行再创建 `dirs`、`ignoreFiles` 以及供报告闭包持有的克隆。

`Directive` 和多个 API 使用裸 AST 指针，以贴近 Go 指针式 AST。代码假设这些指针在 analyzer 执行期间有效，并只在受限位置 `unsafe` 解引用：读取文件注释、文件起始位置、import 字段，以及给新建 `token::File` 设置行表。`ReadFile` 返回的 `Vec<u8>` 保留任意原始字节，不强制 UTF-8；这也是 `FindOffset` 接受字节切片而非 `&str` 的原因。

`ReadFile` 对 `FileSet` 有明确副作用：`AddFile(filename, -1, len)` 分配位置区间，`SetLinesForContent` 登记换行偏移。因此其返回的 token 文件与传入 `FileSet` 必须一起使用。`MakeFakeLoaderPackageInfo` 则复制/克隆 pass 内的引用型数据，不重新解析源码，也不验证 `Importable` 或“传递无错误”标志。

## 依赖与调用关系

下游直接依赖包括模拟或迁移后的 `ast`、`token`、`analysis`、`loader`、`reflect`、`report` API，以及标准库的 `Any`、`HashSet`、`Read`、`Rc`。`SkipAnalyzerByConfig` 额外依赖相邻 [`exclude.rs`](./exclude.rs) 的 `shouldRun`；该函数从 `build::NogoConfig` 读取 analyzer 的 only/exclude 文件正则配置，正则非法时会 panic。

已核对的 Rust 调用点包括：[`misspell/analyzer.rs`](../misspell/analyzer.rs) 调用两种 wrapper 和 `ReadFile`；[`gosec/analysis.rs`](../gosec/analysis.rs) 调用两种 wrapper、`MakeFakeLoaderPackageInfo` 与 `ReadFile`；[`revive/analyzer.rs`](../revive/analyzer.rs) 调用 wrapper、`ReadFile`、`FindOffset`；[`deferrecover/analyzer.rs`](../deferrecover/analyzer.rs) 调用 `GetPackageName` 和配置 wrapper。`rg` 还显示 `intrange`、`forcetypeassert`、`unconvert`、`allrevive`、`mirror`、`rowserrcheck`、`toomanytests` 等文件存在直接 wrapper 调用。

RustCodeGraph 的文件节点把 `util.rs` 标为被 9 个跨语言文件引用，但精确 `callers` 查询没有返回稳定结果，因此上述调用关系以精确符号搜索和调用点源码复核为准。仓库正式 Bazel 依赖边仍以 [`BUILD.bazel`](./BUILD.bazel) 的 Go 目标为准：`//build`、Staticcheck report、Go analysis 和 loader；不能把 Rust 文本调用点等同于已发布 crate 依赖。

## 错误处理与边界

- `parseDirective` 假定 `//lint:` 后经 `split(' ')` 至少产生一个字段；空参数仍产生空字符串字段，与 Go `strings.Split` 对齐。`SkipAnalyzer` 随后直接访问 `dir.Linters[0]`，所以手工构造的 `skipLinter` 若没有元素会越界 panic；正常 `parseDirective` 路径会提供至少一项。
- `SkipAnalyzer` 对缺少 `Directives` 结果、结果类型错误、原 analyzer 没有 `Run` 都使用 `expect` panic。这是对 Go map/type assertion 或 nil 回调失败方式的显式保留，而不是可恢复错误。
- 指令抑制按“同文件、同行”匹配，不比较列范围或 AST 包含关系；`Linters[0]` 才会按逗号拆分，`//lint:ignore` 后的更多空格字段不会被合并。
- `SkipAnalyzerByConfig` 的过滤错误由 `shouldRun` 决定；配置不存在时全部运行，非法正则 panic。若全部文件被过滤，原 analyzer 仍会收到空 `Files` 并执行。
- `ReadFile` 分别把打开和读取失败标成 `Op = "open"` 或 `"read"`，保留路径、错误种类和 `source()`；只有完整读取成功后才修改 `FileSet`。
- `FindOffset` 只在扫描某个字节前检查目标坐标，所以 EOF 位置不被识别；非正数坐标、越界行列或文件末尾之后统一返回 `-1`。换行只识别 `\n`，合法 UTF-8 序列按 1 个 rune 列计数，非法或截断序列按 1 字节/1 列计数。
- `FormatCode` 遇到任意反引号就完全不加格式标记；源码 TODO 表明转义/移除尚未实现。`GetPackageName` 只处理精确字符串字面量，不解析 raw string、点导入或语义 import 图。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道或事务。`Rc` 明确指向单线程共享：报告回调和 `analysis::Pass` 的聚焦模型并不具备跨线程发送语义。若未来把 analyzer 并行化，不能直接把这些闭包或裸 AST 指针跨线程移动，必须先确认真实 analysis API 的 `Send`/`Sync` 契约与 AST 所有权。

文件资源由 `std::fs::File` 的 RAII 管理：`ReadFile` 完成 `read_to_end` 或返回错误后文件句柄自动关闭；读取内容和 `FileSet` 登记由调用者持有。AST 裸指针生命周期不由本文件管理，它们必须由上游 arena/Box 等所有者覆盖整个解析、过滤和报告阶段。本文件不释放这些指针，也不缓存文件内容。

wrapper 会把原 `Run` 移入新闭包，并在每次运行时通过 `as_mut().call` 调用；因此它依赖 analyzer 注册阶段一次性、受控的可变修改。重复包装会继续叠加依赖和闭包层，源码没有去重或恢复机制。

## 与 Go 版本的对应关系

[`util.go`](./util.go) 是逐项对照基线：Rust 的 `skipType`/`Directive`、解析和 comment-map 遍历、`Directives` analyzer、两个 wrapper、格式化、loader 适配、文件读取、偏移计算和包名解析均保持同样的职责与主要分支。

有意的语言适配包括：Go 的 `any` 结果在 Rust 中是 `Option<Box<dyn Any>>`；`reflect.TypeOf([]Directive{})` 由 `TypeId` 函数提供；Go map 集合由 `HashSet<String>` 实现；Go 函数值由可变 `analysis::Run` 枚举/闭包承载；Go 的 `error` 在 `ReadFile` 中被具体化为 `ReadFileError`。Rust 还显式保存 `std::io::Error` 的 source 链，比简单字符串更利于上游分类。

`FindOffset` 的 Rust 实现不能直接遍历 `&str`，因为 Go string 可包含非法 UTF-8；它用 `&[u8]` 和 `goRuneLen` 模拟 Go `range` 的“合法 rune 按宽度、非法编码按单字节”行为。独立测试覆盖中文字符与 `0xff`。另一方面，当前 Rust API 的多个裸指针/值传递签名是移植层形状，不代表已形成稳定公共 crate API。

同目录没有 `util_test.go`；[`BUILD.bazel`](./BUILD.bazel) 的 Go 测试只包含 `exclude_test.go`，因此 `util.go` 本身的对照证据来自源码逐分支核对。Rust 的行为回归集中在独立 [`util_test.rs`](./util_test.rs)，未把测试内嵌进生产源文件。

## 扩展指南

新增指令命令时，应同步修改 `skipType`、`parseDirective` 和 `SkipAnalyzer` 的匹配分支，并在 [`util_test.rs`](./util_test.rs) 增加解析、文件筛选及诊断转发/抑制用例；还要同步核对 [`util.go`](./util.go)，避免 Rust 单方面扩展协议。若新增需要多个参数的命令，不应默认复用当前只读取 `Linters[0]` 的逻辑。

调整文件过滤时，配置语义应优先落在 [`exclude.rs`](./exclude.rs) 及其独立 [`exclude_test.rs`](./exclude_test.rs)，`SkipAnalyzerByConfig` 只负责 pass 适配。改变 wrapper 顺序、允许重复包装或把 analyzer 变成并发运行，必须检查原 `Run` 所有权、`Requires` 去重、`Rc` 和裸指针生命周期。

扩展外部检查器适配时，优先复用 `ReadFile`/`FindOffset`，并保留原始字节。任何把参数改回 `&str`、使用 lossy UTF-8、或把 rune 列直接当字节列的改动，都可能让非 ASCII/非法 UTF-8 后的诊断错位。新的边界测试至少应覆盖 EOF、CRLF、空文件、非法 UTF-8、别名/默认 import，以及 `ReadFile` 的 open/read 两类错误。

若要让本文件成为真正可复用的 Rust 生产模块，需要另行完成 crate/module 声明、真实 Go AST/analysis 兼容层依赖与构建目标接线；这属于比本文档任务更大的迁移范围。接线后应把当前 `include!` 聚焦测试迁入同目录但独立测试文件对应的正式 Cargo test target，并验证所有直接 caller 的签名一致性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter build/linter/util` 确认目标、Go 对照与独立测试；`node --file build/linter/util/util.rs` 读取 1–312 行；`query` 确认 `parseDirective`、`ParseDirectives`、`SkipAnalyzer`、`ReadFile`、`FindOffset` 的 Go/Rust 定义。精确 `callers` 无稳定输出后，使用 `rg` 回退核对直接调用点。
- 生产与对照源码：[`util.rs`](./util.rs)、[`util.go`](./util.go)、[`exclude.rs`](./exclude.rs)、根 [`Cargo.toml`](../../../Cargo.toml)、[`BUILD.bazel`](./BUILD.bazel)。Cargo workspace 与 Bazel 文件共同证明当前 Rust 文件尚未进入正式构建目标。
- 直接调用证据：[`misspell/analyzer.rs`](../misspell/analyzer.rs)、[`gosec/analysis.rs`](../gosec/analysis.rs)、[`revive/analyzer.rs`](../revive/analyzer.rs)、[`deferrecover/analyzer.rs`](../deferrecover/analyzer.rs)，以及对 `build/linter/**/*.rs` 的精确符号搜索。
- 独立测试：[`util_test.rs`](./util_test.rs) 覆盖指令解析/comment map、文件及同行诊断过滤、缺失结果 panic、配置过滤、格式化、UTF-8/非法字节偏移、import 名、读取和 FileSet 副作用、loader 适配、错误链与许可证约束。未发现 `util_test.go`；Go Bazel 测试仅列 `exclude_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo，也未执行 Go/Bazel 测试。交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工复核文档明确回答文件存在原因、运行链、当前接线限制和安全扩展点。
