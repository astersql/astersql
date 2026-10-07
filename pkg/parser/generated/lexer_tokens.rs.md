# `pkg/parser/generated/lexer_tokens.rs`

## 文件定位

`lexer_tokens.rs` 是 `astersql-parser` crate 的主 SQL 词法 token 编号表。它不是手写业务逻辑，而是 `astersql-parsergen` 根据 `pkg/parser/grammar/main.astergram` 生成并提交到仓库的静态产物；文件头的 `@generated` 明确要求不要直接编辑。`pkg/parser/lib.rs` 在私有的 `parser_impl::lexer_support` 模块中用 `include!("generated/lexer_tokens.rs")` 展开它，因此其中的私有 `token` 模块只在该词法支撑作用域及其同作用域源码中可见，不构成 crate 根的公开 API。

普通编译和数据库启动不会现场生成此文件。`pkg/parser/Cargo.toml` 以 `lib.rs` 为库入口，没有 build script 或 build dependency；生成器 `astersql-parsergen` 仅是 dev dependency。生成和防漂移入口位于 `pkg/parser/parsergen/generate.rs`，显式维护命令由 `pkg/parser/readme-rust.md` 记录。

## 核心职责

该文件为词法扫描器、Hint 词法适配器和主解析器提供同一套 `名称 -> i32 编号` 契约。当前文件仅包含一个 `#[allow(non_upper_case_globals)] mod token`，模块内有 943 个 `pub const`，没有函数、trait、类型、条件编译或可变静态数据。

常量分为两类：21 个单字节标点 token 直接使用 ASCII 数值，例如 `literal_21 = 33`（`!`）、`literal_28 = 40`（`(`）和 `literal_7e = 126`（`~`）；其余 922 个命名 token 连续占用 `57346..=58267`，从 `identifier` 到 `builtinMinCount`。这张表只定义身份，不负责扫描、关键字判定、语法移进/归约或 AST 构造；这些行为分别位于 `lexer.rs`、`keywords.rs`、`hintparserimpl.rs`、`yy_parser.rs`、`parser.rs` 和生成的 `main_tables.rs`。

## 主要符号

- `mod token`：私有命名空间，避免近千个生成常量污染 `lexer_support` 作用域；子项为 `pub` 是为了让同一父模块中的 lexer、parser 代码通过 `token::<name>` 使用。
- `literal_21` 至 `literal_7e`：语法中使用的单字符终结符。名称采用字符码的十六进制后缀，值采用字符本身的 ASCII 码；实际 lexer 对多数单字符直接返回 `byte as i32`，因此两边必须一致。
- `identifier`、`stringLit`、`intLit`、`decLit`、`floatLit`、`hexLit`、`bitLit`、`invalid`：扫描结果的基本类别。`lexer.rs::Lex` 会进一步把 `identifier` 解析为关键字，或按 SQL mode 改写 token。
- `singleAtIdentifier`、`doubleAtIdentifier`、`hintComment`：用户变量、系统变量和优化器 Hint 的词法边界；`lexer.rs::startWithAt` 与 Hint 解析路径直接消费这些编号。
- `add`、`selectKwd`、`tableKwd` 等关键字 token：由关键字表和复合关键字逻辑返回，并作为主解析表的终结符。
- `lowerThanSelectOpt`、`higherThanParenthese`、`neg` 等优先级标记：来自主语法的 precedence 声明，主要用于生成解析表和解决冲突，不代表输入中可直接扫描出的文本。
- `builtinMaxCount`、`builtinMinCount`：当前命名 token 区间的末端；它们与前面的内建函数、运算符和优先级 token 一样，由语法声明决定编号。

生成侧的关键符号是 `parsergen/generate.rs::generate_outputs` 和私有 `render_token_module`：前者从主语法构建 `GeneratedParser`，后者依次遍历 `GeneratedParser::tokens`，通过 `rust_identifier` 处理 Rust 关键字（例如生成 `r#as`、`r#in`、`r#match`、`r#use` 和 `r#async`），并以 `i32` 渲染这个模块。

## 执行流程

1. 维护者在 `pkg/parser/grammar/main.astergram` 中声明 token、编号、显示文本及 precedence；`GeneratedParser::build` 构造与该语法一致的 token 列表和解析表。
2. 显式执行 parsergen 的 generate 流程时，`generate_outputs` 同时生成 `main_tables.rs`、`hint_tables.rs` 和本文件。`render_token_module(&main.1, "token", "i32")` 取主解析器的 token 列表，输出 Rust 常量；`write_generated_outputs` 再写入 `pkg/parser/generated/`。
3. 编译 `astersql-parser` 时，`lib.rs` 先在 `parser_impl` 中包含主解析表，再在 `lexer_support` 中包含本文件、Hint 表和 lexer 实现。它不是运行时 I/O，也不会在启动阶段重新计算。
4. 扫描 SQL 时，`lexer.rs::scan` 产生字符 token或 `token::identifier` 等基本类别；`Scanner::Lex` 用关键字表、复合关键字预读和 SQL mode 把它们改写成最终 token，例如把 `AS OF` 合并为 `token::asof`、在 ANSI_QUOTES 下把双引号字符串改为 `token::identifier`、按模式把 `pipes`/`not` 改为对应 token。
5. 主 parser 使用同一编号在 `main_tables.rs` 的翻译表与 LR 表中查找动作；`yy_parser.rs` 的深度保护也会用 `token::intLit`、`token::floatLit` 和 `token::identifier` 判断操作数。Hint 路径在 `hintparserimpl.rs` 中把主 lexer token 映射为 Hint parser 的专用 token。

## 数据与状态

本文件的数据全部是编译期 `i32` 常量，不持有输入文本、当前位置、错误列表、解析栈或全局可变状态，也没有初始化顺序问题。标点 token 与 lexer 直接返回的字节值共享数值域；命名 token 使用高值区间，`0` 则由扫描/解析约定作为 EOF，未在这里重复声明。

编号是跨生成产物的不变量，而不是可独立重排的枚举序号。当前 `lexer_tokens.rs` 与 `generated/main_tables.rs` 各含 943 个 token 常量；前者用 `i32` 服务 lexer，后者用 `isize` 服务解析表。名称和值必须同步，否则 lexer 返回的 token 会被解析表解释为另一个终结符。生成顺序由 `GeneratedParser::tokens` 决定，文件自身没有查重或范围检查逻辑；语法解析和 parsergen 构建阶段承担输入验证，提交产物则由防漂移测试校验。

## 依赖与调用关系

生成依赖链为 `grammar/main.astergram -> parsergen::Grammar/GeneratedParser -> generate_outputs -> render_token_module -> generated/lexer_tokens.rs`。`pkg/parser/Cargo.toml` 表明 parsergen 是开发期依赖，运行库不依赖外部生成器。

消费链为 `lib.rs::parser_impl::lexer_support -> include!(lexer_tokens.rs) -> lexer.rs / hintparserimpl.rs / misc.rs`，并通过 `Scanner` 间接向 `yy_parser.rs` 和主解析表提供 token。典型直接引用包括 `lexer.rs::Lex` 对 `token::identifier`、`token::invalid`、`token::asof`、`token::pipesAsOr` 的比较或返回，`lexer.rs::startWithAt` 对变量 token 的分类，`hintparserimpl.rs` 对 `identifier`、`stringLit`、`intLit` 的 Hint 映射，以及 `yy_parser.rs` 对字面量/标识符 token 的 AST 深度判断。

RustCodeGraph 状态检查显示索引包含 11,467 个文件，目标文件被识别为 944 个图符号（文件节点加 943 个常量）；精确查询能定位 `lexer_tokens.rs::literal_21` 和 `lexer_tokens.rs::identifier`。常量不是函数调用，图的 callers/callees 不足以表达 `include!` 展开后的引用关系，因此上述消费边以 `lib.rs` 的静态包含和 `rg` 找到的直接 `token::...` 引用为准。

## 错误处理与边界

该文件没有可执行分支和 `Result`，自身不会产生或传播错误。词法错误通过 `token::invalid` 进入上层：例如 `lexer.rs::convert2Connection` 在严格模式且字符集转换失败时可返回 `invalid`，`Scanner::Lex` 在括号深度更新失败时返回 `token::invalid`；错误对象和 warning 状态由 `Scanner` 管理，不在常量表中管理。

必须特别区分“可扫描 token”和“语法内部 token”。优先级标记通常只参与语法冲突处理，不能据此推断 lexer 会从输入直接产生它们。单字符 token 则可能由 lexer 直接返回 ASCII 值，而不显式写成 `token::literal_*`。Rust 原始标识符前缀 `r#` 只是让生成名称合法，不改变语法 token 名或编号。

手工改动本文件是边界违规：即使代码能编译，也可能破坏 lexer、翻译表和 LR 表间的一致性，并会被 `check_generated_outputs` 判定为 stale。正确错误恢复路径是修改语法或生成器，重新生成全部相关产物，再审查差异。

## 并发与资源生命周期

常量表是只读编译期数据，所有线程共享代码段，没有锁、原子变量、通道、任务、堆分配或析构过程；它不会扩大每个连接/语句的运行时状态。真正的可变生命周期属于每个 `Scanner` 和 parser 实例，例如 reader 位置、关键字历史、错误列表与解析栈。

生成阶段由显式命令顺序读取语法并整体写出三个文件。`check_generated_outputs` 只读磁盘并比较字节：缺失记为 `Missing`，内容不同记为 `Stale`，不会在检查过程中创建或重写文件。仓库未在此层提供并发写入协调，因此维护者不应同时运行多个生成进程修改同一 `generated/` 目录。

## 与 Go 版本的对应关系

Go 对照来源是 `pkg/parser/parser.y`、其生成产物 `pkg/parser/parser.go` 和 `pkg/parser/lexer.go`。两种实现共享大量 token 语义名称和词法分支：例如 Go lexer 与 Rust lexer 都先得到 `identifier`，再进行关键字识别；两者也对应处理 quoted identifier、用户变量、数字字面量和 SQL mode。`lexer_5_aster_unit_test.rs` 的测试名称与断言明确以 Go 行为为对照，覆盖字符 token、变量、字面量、注释位置和 ANSI_QUOTES 等情形。

不能假定 Go 与 Rust 的数值表逐项相同。当前 Rust `add = 57363`，而 `parser.go` 中为 `57364`；Rust `builtinMaxCount = 58266`，Go 中为 `58159`。这反映两套当前语法/生成序列的差异，不能通过手工改号“对齐”。Rust 运行时的正确契约是 `main.astergram`、`lexer_tokens.rs`、`main_tables.rs` 和 Rust lexer 彼此一致；Go 文件用于核对行为意图和命名语义，而不是 Rust token ABI 的数值来源。

## 扩展指南

新增或调整 SQL token 时，应先修改 `pkg/parser/grammar/main.astergram` 中的 token/precedence 和相关产生式，必要时同步 `keywords.rs`、`lexer.rs` 或复合关键字逻辑；不要直接编辑本文件。若生成名称会碰到 Rust 关键字或包含标点，应检查 `parsergen/generate.rs::rust_identifier` 以及 `parsergen/render.rs` 的标识符规范化逻辑，避免名称碰撞或产生非法 Rust。

完成语法修改后，应使用 `astersql-parsergen generate` 重新生成三个静态产物，并用 parsergen 的 check 流程确认无漂移。至少同步独立测试：生成稳定性/漂移行为在 `pkg/parser/parsergen/generate_aster_unit_test.rs`，特殊 token 名渲染在 `pkg/parser/parsergen/render_test.rs`，静态消费契约在 `pkg/parser/parser_generated_sources_aster_unit_test.rs`，实际词法边界优先扩展 `pkg/parser/lexer_5_aster_unit_test.rs`；主语法行为还应放入相应 parser 独立测试，而不是把测试嵌入生成文件。

兼容风险主要是编号错配导致解析表误判，行为风险是新增关键字把既有合法标识符变成保留词，性能风险则来自 lexer 新增预读或分支，而不是常量读取本身。评审生成差异时应确认三份产物同步、token 值无意外整体漂移、Go 对照行为仍被测试覆盖，并保留文件的 AsterSQL 版权头与 generated 标记。

## 验证依据

- 源码全貌：`pkg/parser/generated/lexer_tokens.rs` 共 949 行；除版权/生成标记和 `#[allow(non_upper_case_globals)] mod token` 外，仅有 943 个 `pub const`。首末命名常量及数值范围由文件直接核对。
- 生成依据：`pkg/parser/grammar/main.astergram` 的 `%token`/precedence 声明；`pkg/parser/parsergen/generate.rs::{generate_outputs, render_token_module, rust_identifier, write_generated_outputs, check_generated_outputs}`；`pkg/parser/parsergen/render.rs::GeneratedParser`。
- crate 与入口：`pkg/parser/Cargo.toml`、`pkg/parser/lib.rs`、`pkg/parser/generate.rs`；目标包没有 `doc.go`，因此包边界以这些 Rust 入口和 manifest 为准。
- 调用证据：`pkg/parser/lexer.rs::{Scanner::Lex, getNextToken, getNextTwoTokens, startWithAt, scanIdentifier}`、`pkg/parser/hintparserimpl.rs`、`pkg/parser/yy_parser.rs`，以及 `lib.rs` 的静态 `include!`。
- 测试证据：`pkg/parser/parser_generated_sources_aster_unit_test.rs` 验证提交产物被静态包含；`pkg/parser/parser_no_legacy_dependency_aster_unit_test.rs` 验证无 build script/旧动态生成依赖；`pkg/parser/parsergen/generate_aster_unit_test.rs` 验证确定生成、缺失/陈旧检测及已提交产物一致；`pkg/parser/parsergen/render_test.rs` 验证标点名称转合法 Rust 标识符；`pkg/parser/lexer_5_aster_unit_test.rs` 与 `pkg/parser/misc_2_aster_unit_test.rs` 验证实际 token 行为。
- Go 对照：`pkg/parser/parser.y`、`pkg/parser/parser.go`、`pkg/parser/lexer.go`。数值差异已抽样核对，故文档未宣称跨语言编号 ABI 一致。
- RustCodeGraph：`status` 确认索引可用；`files --filter pkg/parser/generated` 确认目标在图中且有 944 个符号；`query literal_21 --kind constant`、`query identifier --kind constant` 定位目标常量。图对生成常量的 callers/callees 解析未提供可靠引用边，相关关系改由静态包含与直接源码引用验证并在本文明确限制。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的标题计数命令验证恰有 11 个固定二级章节，并人工复核未把生成表描述成运行时生成或把 Go 数值当作 Rust 真值。
