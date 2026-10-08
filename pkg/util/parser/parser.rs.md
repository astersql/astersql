# `pkg/util/parser/parser.rs`

## 文件定位

本文件属于 Cargo crate `astersql-util-parser`（入口为 [`pkg/util/parser/lib.rs`](lib.rs)，依赖声明见 [`pkg/util/parser/Cargo.toml`](Cargo.toml)）。它不是 SQL 语法解析器的主体：`pub use crate::parser_core::Parser` 把 `astersql-parser` 中的完整 `Parser` 类型暴露到本 crate，而本文件只提供该类型的复用池，以及一组消费输入前缀的轻量字节匹配函数。

在当前 Rust 生产代码中，直接使用对象池的已确认入口是 `pkg/util/generatedexpr/generated_expr.rs::parse_expression`：它从这里取得 `Parser`，解析包装成 `select <expr>` 的表达式，再将解析器归还。`lib.rs` 通过 `pub use parser::*` 将本文件的公开项提升到 crate 根，因此调用方使用 `astersql_util_parser::GetParser`、`DestroyParser` 等名称。

## 核心职责

1. `GetParser` / `DestroyParser` 复用完整 SQL `Parser`，并保证归还前调用 `Parser::Reset`。池是线程局部的，适配 Rust 版本 AST trait object 非 `Send` 的事实。
2. `Match` / `MatchOne` 实现公共的“从开头连续匹配，失败不消费输入”协议。
3. `AnyPunct`、`AnyChar`、`Char`、`Space`、`Space0`、`Digit` 在公共协议上提供单字节标点、任意字节、指定字节、空白和 ASCII 数字匹配。
4. `Number` 将至少一个 ASCII 数字组成的前缀转换为 `isize`，同时保留未消费的尾部和可区分的失败原因。

这些函数刻意返回拥有所有权的 `Vec<u8>`，而不是字符串切片；因此它们可以表达 Go `string` 的字节消费行为，包括消费一个 UTF-8 码点的中间字节。

## 主要符号

- `pub enum ParseError { PatternNotMatch, InvalidNumber(String) }`：前者表示输入未达到最小匹配次数，后者保存十进制前缀转为 `isize` 时的具体错误文本。该类型可克隆、调试输出和比较。
- `pub const ErrPatternNotMatch`：Go 包级错误变量的兼容名称，值为 `ParseError::PatternNotMatch`。
- `thread_local! static POOL: RefCell<Vec<Box<Parser>>>`：每个线程独立的后进先出解析器池。`RefCell` 提供线程内可变借用，`Vec` 保存堆分配的完整解析器。
- `GetParser() -> Box<Parser>`：弹出池顶对象；池空时调用 `crate::parser_core::New` 创建。
- `DestroyParser(Box<Parser>)`：先调用 `Parser::Reset`，再压回当前线程的池。
- `Match<B, F>(buf, pat, times)`：统计开头连续满足 `Fn(u8) -> bool` 的字节数；不足 `times` 时返回空匹配、完整原输入和 `ErrPatternNotMatch`。
- `MatchOne<B, F>(buf, pat)`：只检查首字节；空输入或首字节不匹配时原样返回输入。
- `is_go_byte_punctuation(u8)`：内部谓词，接受 ASCII 标点，以及 Go 将单字节转成 rune 后会认作标点的 `0xA1/0xA7/0xAB/0xB6/0xB7/0xBB/0xBF`。
- `AnyPunct` / `AnyChar` / `Char`：分别匹配一个标点字节、任意一个字节、一个指定字节，均委托给 `MatchOne`。
- `Space` / `Space0`：通过 `Match` 消费字节转为 `char` 后被 `is_whitespace` 接受的连续前缀；`Space0` 固定最小次数为零。
- `Digit`：通过 `Match` 消费连续 ASCII 数字。
- `Number`：先调用 `Digit(input, 1)`，再以十进制解析为 `isize`。

文件没有 trait、`impl` 块或条件编译项；所有公开函数都保留了 Go 风格名称，并通过局部 `allow(non_snake_case)` 或常量命名豁免实现兼容。

## 执行流程

对象池主流程如下：调用方执行 `GetParser`；函数在当前线程的 `POOL` 上短暂取得可变借用并 `pop`；有缓存则返回缓存对象，否则由 `parser_core::New` 创建。调用方完成解析后把所有权传给 `DestroyParser`，后者调用 `Reset` 清理解析状态和配置开关，再把对象压回同一线程的池。`pkg/util/generatedexpr/generated_expr.rs::parse_expression` 展示了完整生产路径：取对象、可选设置 SQL mode、调用 `ParseSQL`、归还对象，然后处理解析结果。

匹配器主流程如下：`Match` 把输入视为字节切片，从索引零开始执行谓词，直到第一个不匹配字节；若匹配数低于 `times`，不提交这次消费，否则在计数位置拆成匹配前缀和剩余后缀。`MatchOne` 是只消费首字节的变体。`Space`、`Digit` 选择不同谓词并复用 `Match`；三个单字节函数复用 `MatchOne`。

`Number` 先要求一个或更多数字。若 `Digit` 失败，它返回数值零、完整原输入和模式错误；若数字前缀存在，则该前缀必为 ASCII，所以 UTF-8 转换使用内部不变量保证的 `expect`。转换成功时返回数值与尾部；转换溢出时返回 `isize::MAX`、已经由 `Digit` 分出的尾部和 `InvalidNumber`。

## 数据与状态

持久状态只有线程局部 `POOL`。每个元素是一个 `Box<Parser>`，池本身没有容量上限或主动回收策略；对象一直存活到再次取用或线程退出。`GetParser` 把对象所有权移出池，`DestroyParser` 要求调用方交回所有权，避免同一对象同时在池内和调用方手中。

匹配函数自身无共享状态。输入约束是 `AsRef<[u8]>`，允许字符串、字节切片、数组和 `Vec<u8>`；输出复制为新的 `Vec<u8>`。`times` 是 `isize`：零表示允许空匹配，负数也会自然满足 `count >= times`，因此返回当前可匹配前缀而不报错；迁移测试明确覆盖了负值。`Number` 的数值范围与目标平台的 `isize` 一致。

## 依赖与调用关系

crate 直接依赖 `astersql-parser`，并在 `lib.rs::parser_core` 中完整再导出该依赖。本文件向下调用 `parser_core::New` 和 `Parser::Reset`；真实实现位于 `pkg/parser/yy_parser.rs`。`New` 建立带 yacc 符号缓存的解析器，`Reset` 清空缓存值并恢复默认 SQL mode、窗口函数及严格类型检查等开关，因此池复用不会有意保留上一次调用设置的解析选项。

内部调用边为：`AnyPunct`、`AnyChar`、`Char` → `MatchOne`；`Space`、`Digit` → `Match`；`Space0` → `Space`；`Number` → `Digit`。RustCodeGraph 还确认 `pkg/util/generatedexpr/generated_expr.rs::parse_expression` 是池 API 的生产调用者；其 Cargo manifest 通过 `parserutil-dependency = { package = "astersql-util-parser", path = "../parser" }` 建立依赖。

RustCodeGraph 对 `parser.rs` 报告多个“used by”文件，但其中包含 crate/测试层面的引用；本次针对公开匹配函数的仓库搜索只确认独立测试使用它们，不能据此宣称它们已经接入更多生产主链。

## 错误处理与边界

- `Match` 的关键不变量是失败不消费：即使开头已有部分字节满足谓词，只要少于 `times`，匹配部分仍为空且 `rest` 是完整原输入。
- `MatchOne` 对空输入和首字节不匹配采用相同的 `PatternNotMatch`，并保留原输入。
- `Space0` 以零为下限，按实现不会得到错误；`debug_assert!` 仅在调试构建检查这一内部不变量。
- `AnyChar` 是“任意 Go 字符串字节”而不是 Unicode 标量匹配器。输入 `"é"` 时只消费 UTF-8 的第一个字节，剩余字节不一定是合法 UTF-8 文本。
- `AnyPunct` 同样按单字节转 rune 的 Go 行为匹配；它不是完整 Unicode 标点解析器。额外七个高位字节是为对齐 `unicode.IsPunct(rune(b))`，不能扩展为“所有 UTF-8 标点”而不改变兼容语义。
- `Digit` 只接受 ASCII `0..=9`。尽管 Go 源码写作 `unicode.IsDigit(rune(c))`，参数 `c` 仍是单个 byte，因此 Rust 的 ASCII 判断覆盖当前单字节输入语义。
- `Number` 对无数字前缀返回 `(0, 原输入, PatternNotMatch)`；对超出 `isize` 范围的前缀返回 `(isize::MAX, 已消费数字后的尾部, InvalidNumber)`。后者保留了 Go `strconv.Atoi` 已识别前缀后返回转换错误的结构，但错误值和溢出数值的类型表示不是 Go `error`/`int` 的逐类型复制。

## 并发与资源生命周期

`POOL` 使用 Rust `thread_local!`，不会在线程间共享解析器，也不需要互斥锁。这样避免为含非 `Send` AST trait object 的 `Parser` 错误实现跨线程传递。`RefCell` 的动态借用只包围单次 `pop` 或 `push`，在返回给调用方前已经结束；正常 API 使用不会跨解析过程持有池借用。

与 Go `sync.Pool` 不同，线程局部 `Vec` 不受运行时 GC 清空，也不能被其他线程取走。调用方若忘记 `DestroyParser`，对象会正常随 `Box` 离开作用域而释放，但失去复用收益；若在改变 SQL mode 等状态后归还，`DestroyParser` 的 `Reset` 负责恢复默认状态。当前生产调用点即使 `ParseSQL` 返回错误，也会先归还解析器，再传播解析错误；新增调用点也应保持这种所有退出路径都归还的结构。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/parser/parser.go`，Rust 公共 API 按名称和分支结构逐项对应：Go `sync.Pool` 对应线程局部池；`GetParser` / `DestroyParser` 对应取用、复位和归还；`Match` / `MatchOne` 以及七个包装函数保持匹配前缀与失败不消费协议。

主要表示差异是：Go 输入输出使用 `string`，Rust 使用泛型字节输入和拥有所有权的 `Vec<u8>`；Go 返回 `error`，Rust 返回 `Option<ParseError>`；Go `int` 对应 Rust `isize`。Go 的 `sync.Pool.New` 对应 Rust 池空时调用 `parser_core::New`。由于完整 Rust `Parser` 含非 `Send` 对象，Rust 有意把全局并发池改成线程局部池，而不是不安全地强制跨线程共享。

测试对照为 `pkg/util/parser/parser_test.go` 与 `pkg/util/parser/parser_test.rs`：两者覆盖空白、数字、数值、指定字符和任意字符的同一组成功/失败表。Rust 的 `migration_aster_unit_test.rs` 额外固定了负 `times`、数值溢出、非 ASCII 单字节标点、空输入、UTF-8 拆字节以及池取还行为。这些额外断言是迁移边界证据，不表示 Go API 新增了不同能力。

## 扩展指南

- 新增一种前缀类别时，优先组合 `Match` 或 `MatchOne`，继续保持失败时不消费输入；在独立的 `parser_test.rs` 或迁移测试文件中同时覆盖成功、部分匹配不足、空输入和剩余字节。不要把测试嵌入本生产文件。
- 修改字节分类前，先对照 `parser.go` 的 `byte -> rune` 行为，尤其检查 `0x80..=0xFF` 和多字节 UTF-8；将 API 改成字符级解析会破坏现有 `AnyChar` 契约。
- 修改 `Number` 时应同时验证无前缀、平台范围边界和溢出后 `rest`；兼容风险集中在返回的哨兵数值、错误分类和是否消费数字前缀。
- 新增池调用点时，应确保解析器在成功和失败路径都传给 `DestroyParser`。若将来 `Parser` 变为可跨线程移动，仍需单独评估是否值得改回共享池；不能仅删除线程局部约束。
- 若扩展完整 SQL 解析行为，应修改 `astersql-parser` 中的 `pkg/parser/yy_parser.rs` 及其独立测试，而不是在本门面复制解析逻辑。
- 性能上，匹配函数每次都会复制前缀/尾部；若要引入借用切片或零复制变体，应保留现有兼容 API，并用调用点证明生命周期设计不会改变 Go 式返回语义。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/parser` 确认本 crate 的源码和测试集合；`node --file pkg/util/parser/parser.rs` 读取完整 152 行及符号；`query`/`node` 核对 `GetParser`、`Number`；`node --file pkg/util/generatedexpr/generated_expr.rs --offset 210 --limit 55` 核对生产池调用；`node --file pkg/parser/yy_parser.rs --offset 250 --limit 115` 核对 `Parser`、`New`、`Reset`。
- crate 与模块边界：`pkg/util/parser/Cargo.toml`、`pkg/util/parser/lib.rs`；生产调用依赖：`pkg/util/generatedexpr/Cargo.toml`、`pkg/util/generatedexpr/generated_expr.rs::parse_expression`。
- Go 对照：`pkg/util/parser/parser.go`；Go 测试：`pkg/util/parser/parser_test.go`。
- Rust 独立测试：`pkg/util/parser/parser_test.rs`、`pkg/util/parser/migration_aster_unit_test.rs`。覆盖的关键边界包括不足最小次数时原输入不变、负次数、溢出、标点高位字节、空输入、UTF-8 单字节消费和解析器池取还。
- 人工复核：本文件没有条件编译项或隐藏 `impl`；文档只把仓库搜索确认的 `generatedexpr::parse_expression` 称为生产调用点，并将完整 SQL 解析实现明确定位到 `pkg/parser/yy_parser.rs`，未把测试引用推断为生产接线。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构以任务文件指定的 11 个固定二级标题命令验证。
