# `pkg/parser/format/format.rs`

## 文件定位

本文件是独立 crate `astersql-parser-format` 的核心实现。`pkg/parser/format/Cargo.toml` 指定库入口为 `lib.rs`，没有声明第三方依赖；`pkg/parser/format/lib.rs` 通过 `pub mod format` 加载本文件，并用 `pub use format::*` 把公开项再导出到 crate 根。该 crate 位于 parser 基础层，为 AST（抽象语法树）及其消费者提供“把结构重新写成 SQL 文本”的共享上下文，同时提供带 `%i`/`%u` 层级命令的文本格式化器。

它不是 SQL 解析器，也不遍历具体 AST。具体节点负责调用 `RestoreCtx` 的写入方法；本文件只保存恢复策略、表达式父子上下文和 CTE 名称作用域，并把最终字节交给调用方提供的 `std::io::Write`。已接线的 Rust 上游包括 `pkg/parser/types/field_type.rs`、`pkg/parser/test_driver/test_driver.rs`、`pkg/types/parser_driver/value_expr.rs`、`pkg/util/ranger/ranger.rs` 和 `dumpling/export/schema_projection_restore.rs`；这些调用点分别用于类型、值表达式、范围条件或 schema SQL 的恢复。

## 核心职责

- `Formatter`、`IndentFormatterState` 与 `FlatFormatterState` 维护跨多次 `Format` 调用的四态状态机，解释 `%i`（增加缩进）和 `%u`（减少缩进），并把其余常见 printf 形状交给 `render_printf_shape`。
- `OutputFormat` 对普通字符串执行固定 SQL 风格转义：NUL、单引号、换行和回车被替换，其余字符原样保留。
- `RestoreFlags` 定义 AST 恢复策略的位集合，包括字符串/名称引号、大小写、表达式括号、字符集前缀、TiDB 特殊注释、Placement/TTL 及计划缓存相关选项。
- `RestoreCtx` 将标志、输出 writer、默认数据库、表达式父子关系和 CTE 状态聚合为 AST 节点共享的恢复上下文，并提供关键字、字符串、名称、纯文本及特殊注释写入方法。
- `CTERestorer` 记录当前 `WITH` 作用域中的 CTE 名称，并以“记录旧长度、退出时截断”的方式恢复外层作用域。

## 主要符号

- `ST0`、`ST_BOL`、`ST_PERC`、`ST_BOL_PERC`：`IndentFormatterState::format_inner` 的四个状态，分别表示普通正文、行首、正文中刚遇到 `%`、行首刚遇到 `%`。
- `pub trait Formatter: Write`：在标准 `Write` 上增加 `Format(&str, &[&dyn Display]) -> io::Result<usize>`。Rust 使用 `Display` trait 对象切片代替 Go 的可变 `...any`。
- `pub struct IndentFormatterState<W>`：拥有底层 `writer`、每级缩进字节 `indent`、可为负的 `indent_level` 和状态机 `state`。字段不公开，调用方通过 `Write`/`Formatter` 使用。
- `render_printf_shape`：内部渲染器。支持 `%%`、顺序参数、`%[n]` 一基显式参数索引，以及常用的左对齐、零填充、宽度和字符串/十进制精度；所有实际值先走 `Display`，它不是完整的 Go `fmt.Fprintf` 实现。
- `IndentFormatter` / `FlatFormatter`：公开构造函数。前者在行首展开缩进；后者复用同一状态，在缩进层级非零时把格式串中的换行改为空格。
- `OutputFormat`：公开的字符替换辅助函数，不处理反斜线，也不自动增加字符串引号。
- `pub struct RestoreFlags(pub u64)`：公开位集，支持 `|` 和 `|=`；从 `RestoreStringSingleQuotes` 到 `RestoreSkipRedundantParentheses` 共使用 bit 0..20。`DefaultRestoreFlags` 是“单引号字符串 + 大写关键字 + 反引号名称”。
- `RestoreFlags::Has*`：公开查询方法。互斥组合本身不会被拒绝；实际写入方法按判断顺序赋予左侧标志优先级，例如大写优先于小写、单引号优先于双引号。
- `pub trait RestoreWriter: Write`：为任何 `Write` 自动提供 `write_string`，默认只调用一次 `write`。这使 `Vec<u8>` 等标准 writer 可直接作为恢复目标。
- `pub struct RestoreCtx<'a>` / `NewRestoreCtx`：上下文持有借用的 writer，因此不会拥有或关闭输出资源。构造时 `DefaultDB` 为空，父二元运算符/侧别为 `0`，`InUnaryOperation` 为 `false`，CTE 列表为空。
- `RestoreCtx::{WriteKeyWord, WriteString, WriteName, WritePlain, WritePlainf}`：分别执行策略化关键字、字面量、标识符、原样文本和类 printf 写入；都返回 `io::Result<()>`。
- `WriteWithSpecialComments` / `WriteKeyWordWithSpecialComments`：在启用 `RestoreTiDBSpecialComment` 时生成 `/*T![feature] ... */`；否则直接执行正文回调。
- `pub struct CTERestorer`：公开字段 `CTENames: Vec<String>` 保存名称；`IsCTETableName` 精确且区分大小写，`RecordCTEName` 追加，`RestoreCTEFunc` 返回稍后截断列表的单次闭包。

## 执行流程

格式化路径如下：

1. `IndentFormatter` 以 `indent_level = 0`、`state = ST_BOL` 创建状态；`FlatFormatter` 用空缩进串创建相同状态并包一层新类型。
2. `Format` 进入 `format_inner`，逐字节扫描格式串。普通换行把状态切到行首；行首遇到正文时，非 flat 模式先重复写入 `indent`；`%i`/`%u` 只调整层级而不进入输出缓冲。
3. flat 模式只在 `indent_level != 0` 时把换行替换为空格；零层级换行保留，因此能够形成“一条顶层结构一行”的输出。
4. 未被状态机消费的 `%` 动词保留在缓冲中，再由 `render_printf_shape` 依次选择参数、应用有限的宽度/精度规则并生成字符串。缺少参数时，当前格式指令原样保留；末尾悬空 `%` 也被写回。
5. `format_inner` 对底层 writer 只调用一次 `write`，直接返回实际写入字节数；无错误短写不会自动补写。格式器对象继续保存缩进层级与行首状态，供下一次 `Format` 使用。

AST 恢复路径通常从 `NewRestoreCtx(flags, writer)` 开始。节点根据内容调用 `WriteKeyWord`、`WriteString`、`WriteName` 或 `WritePlain`：方法先按 `Flags` 做大小写/引号/转义，再依次写入输出。需要 TiDB 特殊语法时，`WriteWithSpecialComments` 先检查标志；启用后写前缀、可选 feature id 和空格，执行一次正文闭包，成功后才补 ` */`。表达式恢复调用方可以暂存并修改 `ParentBinaryOp`、`ParentBinarySide`、`InUnaryOperation`，完成子节点后必须恢复旧值。

CTE 路径在进入嵌套作用域前调用 `RestoreCTEFunc` 取得清理闭包，再用 `RecordCTEName` 追加本层名称；作用域退出时把可变 `CTERestorer` 传给闭包，列表截断到进入前的长度，因此保留外层名称并删除本层新增名称。

## 数据与状态

`RestoreFlags` 是复制语义的 `u64` 位集合，不包含动态分配；`DefaultRestoreFlags` 只是一组常量位。互斥组没有构造期校验，优先级由 `WriteKeyWord`、`WriteString` 和 `WriteName` 中的 `if/else if` 顺序保证。部分标志（例如 Placement、TTL、字符集前缀和表达式括号）只在本文件提供查询器，真正决定是否省略相应 AST 片段的是上游节点。

`RestoreCtx` 是一次恢复过程中的可变状态。`In` 是 `&mut dyn RestoreWriter`，其生命周期受 `'a` 限制；`DefaultDB` 和 `CTERestorer.CTENames` 拥有字符串内存。三个表达式字段是调用方维护的动态上下文，不存在自动入栈/出栈保护，因此“不论成功或失败都恢复旧值”是上游应维持的不变量。

格式器状态跨调用保留。`indent_level` 使用 `i32`，`%u` 可以降到负数；源码明确只在 `max(0)` 后展开缩进，而 flat 模式把任何非零层级（包括负数）视为需要压平换行。Go 文档把负层级行为定义为未定义，扩展代码不应依赖这一 Rust 细节。`render_printf_shape` 为每次调用创建输出 `String`，`format_inner` 还创建中间 `Vec<u8>`；没有全局缓存。

## 依赖与调用关系

- 装配边界：`pkg/parser/format/lib.rs -> format.rs`，并把本文件公开项全部再导出。`Cargo.toml` 没有 `[dependencies]`，实现只依赖 Rust 标准库的 `fmt::Display`、`io::Write` 和位运算 trait。
- 格式器内部下游：`IndentFormatterState::Format -> format_inner(false) -> render_printf_shape -> writer.write`；`FlatFormatterState::Format -> format_inner(true)`；`FlatFormatter -> IndentFormatter(writer, "")`。
- 恢复上下文内部下游：`WriteKeyWord`/`WriteString`/`WriteName` 查询 `RestoreFlags::Has*` 后调用 `RestoreWriter::write_string`；`WritePlainf -> render_printf_shape`；`WriteKeyWordWithSpecialComments -> WriteWithSpecialComments -> WriteKeyWord`。
- 直接 Rust 上游：`pkg/parser/types/field_type.rs` 使用 `OutputFormat` 处理枚举元素并用 `NewRestoreCtx` 恢复字段类型；`pkg/parser/test_driver/test_driver.rs`、`pkg/types/parser_driver/value_expr.rs` 构造上下文恢复表达式；`pkg/util/ranger/ranger.rs` 恢复范围表达式；`dumpling/export/schema_projection_restore.rs` 通过依赖别名 `schema-format` 恢复 schema。
- crate 传播：`pkg/parser/opcode`、`pkg/parser/auth`、`pkg/parser/types`、`pkg/parser/test_driver`、`pkg/types/parser_driver` 等 Cargo manifest 以路径依赖引用本 crate，其中若干入口再导出为各自的 `format` 模块。这解释了具体 AST/类型实现为何可以用统一的 `RestoreCtx` API。
- 独立测试由 `pkg/parser/format/lib.rs` 在 `cfg(test)` 下挂载 `format_test.rs` 和 `migration_aster_unit_test.rs`；测试没有内嵌在生产源文件中。

## 错误处理与边界

- 所有 writer 错误以 `io::Result` 向上传播。`WriteString` 和 `WriteName` 分三次写首引号、正文、尾引号；任一步失败都会立即返回，已写出的前缀不会回滚。
- `WriteWithSpecialComments` 的正文回调失败时原样返回错误，不补结尾 ` */`。因此部分输出是明确契约，测试覆盖了只留下 `/*T!` 前缀的失败路径。
- `RestoreWriter::write_string` 默认调用 `write` 而不是 `write_all`。成功但短写会被视为成功，`Write*` 方法又丢弃计数，所以自定义 writer 若可能短写，调用方必须自行提供保证完整写入的 `Write` 实现或接受部分 SQL。格式器路径则有意把实际短写计数返回给调用方。
- `render_printf_shape` 只模拟常见 printf 形式：格式动词本身不决定数值进制或调试表示，参数统一经 `Display`；精度仅对 `s` 截字符、对 `d` 补零；缺参和非法/不完整选项不会生成 Go 的 `%!…` 诊断。新增调用不得假设其覆盖完整 `fmt.Fprintf` 语义。
- 格式串按 UTF-8 字节扫描，但只有 ASCII `%`、换行和命令字符参与状态机；普通多字节内容作为原字节进入缓冲，最后用 `from_utf8_lossy` 拼接。合法 Rust `&str` 的普通字符不会损坏。
- `OutputFormat` 不转义反斜线、双引号或其他控制字符；它与 `RestoreCtx::WriteString` 的职责不同，不能互换。
- 标志冲突不会报错。单引号、大写关键字、大写名称、双引号名称分别优先于同组右侧选项；调用方若需要唯一规范输出，应使用经过约束的组合（通常从 `DefaultRestoreFlags` 开始）。
- `IsCTETableName` 区分大小写且只做线性查找；名称规范化应在调用前完成。`RestoreCTEFunc` 捕获的是创建时长度，传入错误的 restorer 也会按该长度截断，类型系统不绑定闭包与原实例。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道或事务。`IndentFormatterState`、`FlatFormatterState` 和 `RestoreCtx` 都需要可变借用才能写入；它们不是设计为并发共享的单例。是否能够在线程间移动最终取决于泛型 writer，但 API 本身不承诺并发写入顺序。

writer 的所有权在两条路径上不同：格式器泛型 `W` 按值持有 writer，格式器销毁时 writer 随之销毁；调用方也可传入 `&mut W` 以保留外部所有权。`RestoreCtx` 始终只借用 `&mut dyn RestoreWriter`，销毁上下文不会 flush、关闭或销毁底层资源，必要的 flush/持久化由调用方负责。

CTE 清理闭包拥有捕获的长度而不借用 restorer，便于作用域内继续修改列表；它是 `FnOnce`，只应执行一次。`RestoreCtx` 的表达式父状态同样没有 RAII guard，提前返回路径必须由具体 AST 恢复实现显式还原。

## 与 Go 版本的对应关系

直接对照是 `pkg/parser/format/format.go`。Rust 的 `Formatter`、`IndentFormatterState`、`FlatFormatterState`、`RestoreFlags`、`RestoreCtx` 和 `CTERestorer` 分别对应 Go 的 `Formatter`、`indentFormatter`、`flatFormatter`、同名位集、同名上下文和嵌入式 CTE 状态。四态状态机、bit 0..20 的顺序、默认标志、冲突优先级、特殊注释形状和 CTE 栈截断顺序均保持一致。

语言适配包括：Go 的 `...any` 在 Rust 中收窄为 `&[&dyn Display]`；Go 的 `io.Writer + io.StringWriter` 由带默认 `write_string` 的 `RestoreWriter` blanket impl 代替；Go 的指针上下文变成带 writer 生命周期的值类型；Go 的无参 CTE 恢复闭包在 Rust 中显式接收 `&mut CTERestorer`，以免长期可变借用阻止作用域内追加名称。Rust 的写入 API返回 `io::Result`，而 Go 的若干 `Write*` 方法忽略 writer 返回值；这使 Rust 可以传播多数写入错误，但仍保留部分输出。

最重要的实现差异是 Go 最终调用标准库 `fmt.Fprintf`，Rust 调用本文件的 `render_printf_shape`。现有测试固定了 `%d`、`%%`、`%03d`、常用宽度/精度和显式索引等已实现形状，但不能据此宣称与 Go 格式化系统完全等价。另一个小差异是 Go 在 CTE 旧长度为零时把切片恢复为 `nil`，Rust `Vec::truncate(0)` 得到空向量；对 `IsCTETableName` 和后续追加的可见行为等价，但内存表示并不相同。

`pkg/parser/format/format_test.go` 的 `TestFormat`、`TestRestoreCtx`、`TestRestoreSpecialComment` 在 Rust 的 `format_test.rs` 中有直接语义对照；`migration_aster_unit_test.rs` 另外覆盖跨调用状态、UTF-8/零填充、`OutputFormat`、writer 中途失败和 CTE 作用域。

## 扩展指南

- 增加或修改缩进命令时，集中调整 `format_inner` 的四个状态分支，并同时覆盖“行首/非行首、单次/跨次调用、indent/flat、零/正/负层级”。测试应继续放在独立 `format_test.rs` 或迁移测试文件。
- 扩展 printf 支持时修改 `render_printf_shape`，先列明要对齐的 Go 动词、flag、索引、宽度和精度语义；至少补齐缺参、Unicode 宽度、负数零填充和短写边界，避免让 AST 恢复依赖未实现的 Go 格式特性。
- 新增恢复策略时，在不改变现有 bit 编号的前提下追加 `RestoreFlags` 常量和 `Has*` 查询器，再到实际 AST 节点接线。同步检查 Go `format.go` 的位序、默认组合及互斥优先级，否则持久化/跨语言约定可能漂移。
- 修改引号或转义时分别定位 `WriteString`、`WriteName` 与 `OutputFormat`，不要把三者合并为一个规则；扩展测试应覆盖冲突标志和 writer 失败后的部分输出。
- 修改特殊注释时保持“未启用时只执行正文”“feature id 可空”“正文错误不补尾部”的控制流，并同步 `format_test.rs::test_restore_special_comment` 和 `migration_aster_unit_test.rs::special_comments_propagate_errors_and_keep_go_partial_output`。
- 扩展 CTE 作用域时优先围绕 `RecordCTEName`/`RestoreCTEFunc` 建立成对操作，明确名称是否已规范化；若改用哈希集合以优化查找，仍需保存嵌套作用域的插入顺序或等价回滚信息。
- 若希望所有恢复写入严格完整，应先设计 `RestoreWriter::write_string` 的 `write_all` 兼容迁移并增加短写回归测试；这是行为变化，不能仅作为文档或内部重构悄然修改。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/parser/format` 确认目标 Rust/Go 源、两个 Rust 测试、Go 测试和 crate 入口均在索引中。
- 完整源码核验：`rustcodegraph node --file pkg/parser/format/format.rs --offset 1 --limit 420` 与 `--offset 421 --limit 380` 覆盖全部 736 行，确认四态状态机、渲染器、21 个标志位、上下文写入和 CTE 生命周期。
- 装配与调用证据：读取 `pkg/parser/format/Cargo.toml` 和 RustCodeGraph 中的 `pkg/parser/format/lib.rs`；代码搜索核对了 `pkg/parser/types/field_type.rs`、`pkg/parser/test_driver/test_driver.rs`、`pkg/types/parser_driver/value_expr.rs`、`pkg/util/ranger/ranger.rs`、`dumpling/export/schema_projection_restore.rs` 的直接调用，以及相关 Cargo 路径依赖/再导出。
- Go 对照：RustCodeGraph 完整读取 `pkg/parser/format/format.go`（531 行）和 `format_test.go`（109 行），核对状态转换、公开 API、位序、优先级、特殊注释错误路径与 CTE 回滚行为。
- Rust 测试：完整读取 `pkg/parser/format/format_test.rs`（248 行）和 `migration_aster_unit_test.rs`（219 行）；现有用例覆盖缩进/扁平输出、无错误短写、15 组恢复标志、特殊注释成功及错误身份、跨调用状态、UTF-8/宽度、固定字符转义和 CTE 截断。
- 最近目录没有 `doc.go`，因此没有额外的包级 Go 契约可读。本任务是纯文档分析，按计划未运行 Cargo；交付验证使用任务指定的 11 章节结构检查，并人工复核重要结论均指向上述符号、调用点或对照测试。
