# `pkg/util/format/format.rs`

## 文件定位

本文件是 `astersql-util-format` crate 的核心实现，对应 Go 包 `pkg/util/format`。crate 入口 `pkg/util/format/lib.rs` 通过 `pub mod format` 和 `pub use format::*` 导出本文件 API；根 crate 又在 `pkg/lib.rs` 的 `util::format` 门面中再导出 `facade_util_format::*`。因此调用方既可以直接依赖 `astersql-util-format`，也可以经根门面访问。

文件提供三类通用文本处理能力：带 `%i`/`%u` 控制命令的缩进格式化、按嵌套层级压平换行、以及 SQL 展示文本需要的字符转义。它不负责 SQL 解析或执行，只把格式串、显示参数与任意 `std::io::Write` 连接起来。`pkg/util/format/Cargo.toml` 没有声明第三方依赖或 feature；`[package.metadata.porting]` 明确把它映射到 Go 包 `pkg/util/format`。

## 核心职责

1. `IndentFormatter` 构造一个有状态的 `Formatter`，把 `%i` 和 `%u` 解释为增加/减少缩进层级，并在每个非空行第一次输出普通字符前写入对应数量的缩进串（`indentFormatter::format`）。
2. `FlatFormatter` 复用同一状态机，但不写缩进；当当前层级非零时把格式串中的换行改成空格，从而把嵌套结构压到顶层单行（`flatFormatter::Format`）。
3. `write_go_style` 实现当前迁移所需的 Go `fmt.Fprintf` 子集，在控制命令处理完成后解析 `%d`、`%s`、`%v`、`%x`、`%X`、`%f` 和 `%%`，再对标志、宽度和精度做有限处理。
4. `OutputFormat` 逐 Unicode 标量扫描输入，将 NUL、单引号、换行、回车和反斜杠替换为固定转义文本，其他字符保持不变。
5. `Formatter: Write` 保留 Go `Formatter` 同时也是 `io.Writer` 的接口语义；直接调用 `write`/`flush` 会转发给底层 writer，而不会进入格式串状态机。

## 主要符号

- `FormatArg<'a> = &'a dyn fmt::Display`：公开的格式参数类型。它只保存 `Display` 能力，不保留 Go `any` 的完整运行期类型信息。
- `Formatter: Write`：公开 trait；`Format(&mut self, format, args) -> io::Result<usize>` 返回底层 writer 实际接受的字节数。
- `IndentFormatter<W: Write + 'static>`：公开构造函数，返回 `Box<dyn Formatter>`；初始 `indentLevel = 0`、`state = stBOL`。
- `FlatFormatter<W: Write + 'static>`：公开构造函数，用空缩进串创建 `indentFormatter`，再包装为 `flatFormatter`。
- `OutputFormat(&str) -> String`：公开无状态转义函数。
- `indentFormatter<W>`：内部状态体，保存 `writer`、缩进字节串、当前层级和扫描状态。
- `st0`、`stBOL`、`stPERC`、`stBOLPERC`：内部状态常量，分别表示普通正文、行首、正文中刚读到 `%`、行首刚读到 `%`。
- `indentFormatter::format`：缩进和扁平模式共享的逐字节控制命令状态机；`flat` 参数选择换行策略。
- `write_go_style`、`parse_format_spec`、`format_argument`：格式说明解析与参数渲染链。
- `apply_integer_precision`、`format_integer_hex`、`apply_width`、`numeric_prefix_len`：整数精度、十六进制、符号/前缀定位和宽度填充的内部辅助函数。
- `replace`：`OutputFormat` 的固定字符映射函数。

## 执行流程

`IndentFormatter` 和 `FlatFormatter` 的主流程相同。构造阶段由 `new_indent_formatter` 记录底层 writer 与缩进字节串，并把扫描位置初始化为行首。每次 `Format` 调用随后执行：

1. `indentFormatter::format` 按字节扫描格式串。状态会跨多次 `Format` 调用保留，因此一次调用末尾建立的缩进层级和行首位置会影响下一次调用。
2. 在 `st0`/`stBOL` 中遇到 `%` 时延迟到相应百分号状态；下一字节为 `i` 或 `u` 时只修改 `indentLevel`，控制命令本身不进入输出缓冲。
3. 非控制命令的 `%` 与后续字节原样进入中间缓冲，交给 `write_go_style` 解析。格式串若以孤立 `%` 结束，状态机把 `%` 补回缓冲。
4. 非 flat 模式在行首遇到第一个普通字节时重复写入 `indent`；空行不会产生缩进。flat 模式不写缩进，并在 `indentLevel != 0` 时把换行替换为空格。
5. `write_go_style` 先把整个中间格式串渲染到 `Vec<u8>`。`%%` 变为字面量 `%`；受支持的格式说明依次消费一个参数；解析失败或参数不足的片段按字面量保留。
6. 渲染完成后只调用一次底层 `Write::write`。底层错误直接返回；无错误短写则原样返回实际写入长度，不自动补写剩余字节。

`OutputFormat` 不走上述流程。它按 `char` 遍历字符串，通过 `replace` 写入转义串或原字符，最后返回新 `String`。

## 数据与状态

`indentFormatter` 的关键不变量是 `state` 与 `indentLevel` 属于 formatter 实例并跨调用持久存在。`stBOL` 表示下一普通字节需要触发行首行为；换行后回到该状态。`stBOLPERC` 单独存在，是为了让行首的 `%i`/`%u` 不会提前写出缩进。`flatFormatter` 包装同一个状态体，所以层级跟踪规则完全共用，只有换行和缩进输出策略不同。

`indentLevel` 使用 `i32` 以贴近 Go `int` 的有符号层级语义。源码明确把降到负层级后的行为定义为未指定：Rust 的 `0..indentLevel` 在负值时不迭代，而 flat 模式仍把任何非零层级视为嵌套。调用方必须保证 `%i`/`%u` 配对，不能依赖负层级输出。

格式参数按顺序消费，额外参数不会输出。整数和浮点参数先经 `Display::to_string`，再尝试解析为 `i128` 或 `f64`；解析失败时保留显示文本。宽度按 Unicode 字符数计算，但整数精度和符号/前缀定位使用字节下标，适用于这里生成的 ASCII 数值文本。每次格式化会分配中间 `Vec<u8>`，参数渲染还可能分配 `String`。

## 依赖与调用关系

下游依赖仅为标准库 `std::fmt` 与 `std::io::{self, Write}`。内部调用链为：

`IndentFormatter`/`FlatFormatter` → `new_indent_formatter` → 对应 `Formatter::Format` → `indentFormatter::format` → `write_go_style` → `parse_format_spec`、`format_argument` → 数值/宽度辅助函数 → 底层 `Write::write`。

`OutputFormat` 独立调用 `replace`，不访问 formatter 状态或 writer。

上游接线方面，`pkg/util/format/lib.rs` 导出本文件，工作区根 `Cargo.toml` 用 `facade_util_format` 指向该 crate，`pkg/lib.rs` 再公开为 `crate::util::format`；`pkg/lib_test.rs::independent_util_modules_are_wired` 通过根路径构造 `IndentFormatter` 验证该门面。`pkg/executor/Cargo.toml` 声明了对该 crate 的路径依赖，但本次搜索未在 `pkg/executor/**/*.rs` 找到 API 使用。RustCodeGraph 对 `IndentFormatter`、`FlatFormatter`、`OutputFormat` 的 callers 查询也没有返回已解析调用边，因此当前证据不能声称它们已进入 Rust 生产执行主链。

Go 侧仍有明确生产调用：例如 `pkg/executor/show.go`、`pkg/ddl/partition.go`、`pkg/lightning/common/util.go` 和 `pkg/parser/types/field_type.go` 使用 `format.OutputFormat` 生成可展示的 SQL 文本。这些是迁移语义的参照，不等同于本 Rust crate 的调用者。

## 错误处理与边界

- 所有 writer I/O 失败均通过 `io::Result` 原样上抛；源码没有重试、包装或吞错。`format_test.rs::test_format_propagates_writer_error` 验证 `BrokenPipe` 的 kind 和消息保持不变。
- 一次渲染只做一次 `write`。`test_format_preserves_go_short_write_result` 证明短写且无错误时返回短长度；调用方若要求完整写入，必须自行选择不会短写的 writer 或在更高层处理。
- 未支持的格式动词不会消费参数，而是从 `%` 开始按字面量继续输出；参数不足时保留完整格式片段；额外参数被忽略。这是一种宽松的迁移策略，不等同于 Go `fmt` 对所有错误格式和参数错配生成诊断文本的完整行为。
- 支持的动词限于 `%d/%s/%v/%x/%X/%f`，支持 `-`、`0`、`#`、`+`、空格标志以及十进制宽度/精度；不支持动态宽度、参数索引、其他整数/字符串/指针动词等完整 Go `fmt` 语法。
- `FormatArg` 只有 `Display`，因此 `%v` 及数值动词无法完全复现 Go 根据动态类型选择格式的行为；十六进制只接受能解析为 `i128` 的显示文本。
- 格式串状态机按 UTF-8 字节扫描，但普通非 `%` 字节会原样保留；`OutputFormat` 则按 Unicode 字符扫描并保留未映射字符。
- `%u` 造成负缩进的行为由公开注释声明为未定义，扩展时不应默默把它当作受支持输入。
- 状态机的兜底分支会在内部状态值非法时 panic；正常构造和方法路径只会产生四个已知状态。

## 并发与资源生命周期

formatter 拥有底层 writer，并通过 `&mut self` 执行 `Format`、`write` 和 `flush`；类型本身没有锁、线程、任务、通道或后台资源。构造函数返回 trait object，其生命周期与被装箱 writer 一致，但 `W: 'static` 限制意味着不能直接装入借用生命周期较短的 writer。

并发安全性完全取决于所有权和底层 writer：正常 Rust 借用规则禁止多个线程同时以可变引用调用同一 formatter；若调用方自行用锁共享，则锁与调用顺序由调用方负责。测试中的 `SharedWriter(Arc<Mutex<Vec<u8>>>)` 只是为了在 formatter 持有 writer 时读取测试输出，不代表生产实现内部提供并发协调。

formatter 被丢弃时没有显式 flush 或关闭逻辑；需要 flush 的 writer 应由调用方显式调用 `Write::flush`。每次 `Format` 的临时缓冲在调用结束时释放，持久保存的只有 writer、缩进串、层级和扫描状态。

## 与 Go 版本的对应关系

整体结构直接复刻 `pkg/util/format/format.go`：四状态常量、`Formatter` 接口、`indentFormatter` 字段、`%i/%u` 状态机、由相同状态派生的 flat 行为，以及 `OutputFormat` 的五项替换表均有一一对应实现。`format_test.rs::test_format_matches_go_output` 和 `migration_aster_unit_test.rs::migration_matches_go_indent_flatten_and_escape_behavior` 使用 Go 示例的期望输出验证基本对齐；跨调用状态由 `test_format_keeps_state_across_calls` 覆盖。

主要实现差异在普通 printf 格式化边界。Go 版把控制命令展开后的字符串直接交给标准库 `fmt.Fprintf`，支持完整的 Go 格式系统和 `any` 参数；Rust 版用 `write_go_style` 自行实现已迁移调用所需的子集，并把参数抽象为 `Display`。Rust 独有测试进一步覆盖整数精度、带进制前缀零填充、浮点精度和字符串左对齐，但这些证据只证明已列明的子集，不能推出与 Go `fmt` 全面等价。

Go 的 `flatFormatter` 是基于 `indentFormatter` 的新定义类型并通过指针转换复用实现；Rust 用一元组 `flatFormatter<W>(indentFormatter<W>)` 表达相同状态共享。Go 的 `TestMain` 安装全局测试环境和 goroutine 泄漏检查；Rust `main_test.rs` 说明该无依赖 crate 没有对等全局 setup，并改为验证直接 `Write` 转发。

## 扩展指南

- 新增 printf 动词或标志时，从 `parse_format_spec` 扩展语法，在 `format_argument` 实现语义，并同步检查 `apply_integer_precision`、`numeric_prefix_len` 与 `apply_width` 的标志优先级。应在独立的 `pkg/util/format/format_test.rs` 或 `migration_aster_unit_test.rs` 增加 Go 对照用例，不要把测试嵌入本源文件。
- 改动 `%i/%u`、行首或 flat 换行规则时，应先画出四个状态的输入转移，尤其覆盖行首控制命令、空行、调用末尾孤立 `%` 和跨调用保留状态；同步对照 `pkg/util/format/format.go` 与 `format_test.go`。
- 扩展 `OutputFormat` 的转义集合时，同时修改 `replace` 并核对所有 SQL 展示调用点是否需要完全相同的兼容文本；这类变化可能影响 `SHOW` 输出、DDL 展示和 dump 文本，不能只以“更安全”为由单方面改变。
- 若要接受借用 writer，需要重新设计当前构造函数的 `W: 'static` 与返回的 `Box<dyn Formatter>` 生命周期；这是公开 API 兼容性修改。
- 若要支持完整 Go `fmt`，应先明确动态类型模型和错误格式输出契约。继续向 `Display` 字符串上叠加推断会有类型歧义，并可能引入兼容性与性能风险。
- 性能敏感改动应关注每次调用的完整中间缓冲、每参数 `to_string` 分配和宽度填充；同时必须保留当前“一次底层 write”和短写返回语义，除非有明确的兼容性决策。
- Rust 源码修改后应按仓库规则同步修改独立测试并运行 `cargo fmt --all`；本次任务仅新增分析文档，没有修改源码或运行 Cargo。

## 验证依据

- Rust 实现：`pkg/util/format/format.rs`，重点符号为 `Formatter`、`IndentFormatter`、`indentFormatter::format`、`write_go_style`、`parse_format_spec`、`format_argument`、`FlatFormatter`、`OutputFormat`。
- crate 与门面：`pkg/util/format/Cargo.toml`、`pkg/util/format/lib.rs`、工作区根 `Cargo.toml` 的 `facade_util_format`、`pkg/lib.rs` 的 `util::format`、`pkg/lib_test.rs::independent_util_modules_are_wired`。
- Rust 独立测试：`pkg/util/format/format_test.rs`、`pkg/util/format/main_test.rs`、`pkg/util/format/migration_aster_unit_test.rs`。
- Go 对照：`pkg/util/format/format.go`、`pkg/util/format/format_test.go`、`pkg/util/format/main_test.go`；Go 生产调用样本为 `pkg/executor/show.go`、`pkg/ddl/partition.go`、`pkg/lightning/common/util.go`、`pkg/parser/types/field_type.go`。
- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/format` 确认实现与测试集合；`node --file pkg/util/format/format.rs` 和三个 Rust 测试文件用于读取符号及源码；对三个公开构造/函数执行 `query` 与 `callers`，未得到本 Rust 实现的已解析上游调用边；`callees write_go_style` 确认其调用 `parse_format_spec`、`format_argument` 和 writer，`callees format_argument` 确认其调用整数精度、十六进制及宽度辅助函数。
- 人工边界复核：确认文档只把测试覆盖的格式子集描述为已支持，明确负缩进和完整 Go `fmt` 等价性均不在已验证范围；未运行 Cargo，符合本纯文档任务约束。
