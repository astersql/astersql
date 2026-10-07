# `pkg/parser/parse-demo1.rs` 逻辑说明

## 文件定位

`pkg/parser/parse-demo1.rs` 是 `astersql-parser` Cargo 包内名为 `parse-demo1` 的独立二进制目标，而不是 `pkg/parser/lib.rs` 暴露的解析器库模块。目标由 `pkg/parser/Cargo.toml` 的 `[[bin]]` 条目直接指向本文件；程序入口是私有函数 `main`。它用一个自包含的小型词法器、递归下降语句解析器和 Pratt 表达式解析器演示 SQL 解析原理。

本文件不在服务器 SQL 请求主链中，也不提供公开 API。源码外对 `parse-demo1` 的直接引用仅见于 Cargo 目标声明；RustCodeGraph 显示所有有意义的调用边都位于本文件内部。真正可复用的 Rust parser crate 入口在 `pkg/parser/lib.rs`，其 `parser_impl` 组合 `lexer.rs`、`yy_parser.rs`、生成表和 `parser.rs`；不要把本示例的能力等同于生产解析器能力。

## 核心职责

文件承担四项相互封闭的演示职责：

1. `lex` 把 ASCII 范围内的有限 SQL 子集切分为带字节偏移的 `Token`，并追加 `End` 哨兵。
2. `Parser::parse_statement` 只分派 `SELECT` 表达式列表与无列定义的 `CREATE TABLE ident`。
3. `Parser::parse_expression` 以 Pratt 绑定力解析一元正负号、括号和四则二元运算；`parse_prefix_expression` 同时接收标识符、整数、字符串及三种类型字面量。
4. `main` 解析两条硬编码 SQL，并把 AST 或错误打印到标准输出/标准错误。

它刻意不承担完整 SQL 语法、AST 兼容、SQL mode、字符集、注释、参数标记、错误恢复或多语句解析等生产职责。文件顶部注释和全部私有符号共同限定了这种“算法示例”角色。

## 主要符号

- `TokenKind`：词法类别。固定包含六个关键字、标识符/整数/字符串、四则运算符、括号、逗号、分号及 `End`；没有公开可扩展接口。
- `Token { kind, offset }`：携带词法类别和 SQL 原文中的字节偏移。`offset` 是所有诊断定位的基础。
- `Statement`：顶层 AST，仅有 `Select(Vec<Expr>)` 与 `CreateTable { name }`。
- `LiteralKind`、`Expr`、`UnaryOp`、`BinaryOp`：表达式 AST 及运算符模型。递归节点通过 `Box<Expr>` 拥有子树。
- `ParseError { offset, message }`：统一词法/语法错误；`ParseError::new` 构造错误，`fmt::Display` 输出 `<message> at byte <offset>`。
- `lex(sql)`：词法入口，返回完整 token 向量或首个词法错误。
- `Parser { tokens, cursor }`：拥有 token 向量及当前游标；`new`、`current`、`current_kind`、`at`、`bump`、`expect`、`error` 构成游标原语。
- `Parser::parse`：单语句语法入口，接收最多一个尾部分号，并强制随后为 `End`。
- `parse_statement`、`parse_select`、`parse_create_table`：语句分派和两种语句的具体解析。
- `parse_expression`、`parse_prefix_expression`、`parse_typed_literal`：Pratt 主循环、前缀项解析及类型字面量解析。
- `infix_binding_power`：把 `+ -` 映射为绑定力 `(1, 2)`，把 `* /` 映射为 `(3, 4)`；左右绑定力不同使同级运算左结合。
- `parse(sql)`：组合 `lex` 与 `Parser::parse` 的文件级入口；`main` 是其唯一文件内顶层调用者。

所有类型和函数均未使用 `pub`，文件也没有 trait、常量、宏或条件编译项。

## 执行流程

`main` 依次处理 `SELECT DATE '2026-07-13', price + 2 * 3;` 和 `CREATE TABLE users;`。对每条输入，`parse` 先调用 `lex`；词法成功后以 `Parser::new` 接管 token 所有权，再调用 `Parser::parse`。成功结果用 pretty `Debug` 格式打印，失败则经 `ParseError::fmt` 打印。

`lex` 逐字节推进 `offset`：跳过 ASCII 空白；单字符标点直接生成 token；单引号字符串扫描到下一个单引号；数字连续吞入 ASCII 数字并解析为 `i64`；标识符只接受 ASCII 字母/数字/下划线，并以 ASCII 大写副本识别关键字。末尾总是生成偏移为 `sql.len()` 的 `End`，从而让解析器在合法输入末尾也能安全读取当前 token。

`Parser::parse` 先由 `parse_statement` 查看首 token。`SELECT` 路径至少解析一个表达式，再按逗号循环追加表达式；因此空选择列表和尾随逗号都会在 `parse_expression` 中失败。`CREATE` 路径严格消费 `CREATE`、`TABLE` 和一个普通 `Identifier`。语句完成后只允许可选的一个分号，随后必须是 `End`，任何额外 token 都被拒绝。

表达式解析先由 `parse_prefix_expression` 建立左树，再循环查询当前 token 的 `infix_binding_power`。只有左绑定力不小于本层 `minimum_power` 时才消费运算符，并以右绑定力递归解析右树。乘除的绑定力高于加减，故示例中的 `price + 2 * 3` 形成 `price + (2 * 3)`；同级运算使用 `(left, right) = (1, 2)` 或 `(3, 4)`，故形成左结合。前缀 `+/-` 以最小绑定力 `5` 解析操作数，优先于四种二元运算。括号把内部解析下限重置为 `0`，并强制匹配右括号。

## 数据与状态

唯一的可变运行状态是 `lex` 的局部 `offset/tokens` 和 `Parser::cursor`。`Parser` 独占 `Vec<Token>`；`bump` 返回消费前 token 的借用，并仅在当前项不是 `End` 时递增游标。这个规则与词法器始终追加 `End` 的不变量配合，使错误路径反复读取 EOF 时不会越过 token 向量。

AST 和 token 中的文本都使用拥有所有权的 `String`。标识符保留原始大小写，关键字识别本身不区分 ASCII 大小写；字符串值去掉外围单引号。`Expr::Unary` 与 `Expr::Binary` 使用堆分配的 `Box` 表示递归树。文件没有全局可变状态、缓存或跨调用共享对象，每次 `parse` 都创建并消费一套完整 token 和 parser 状态。

偏移量按 UTF-8 字节计数而非字符计数。更严格地说，当前词法器只对 ASCII 标识符提供正常路径；遇到非 ASCII 字节时会在该字节偏移返回意外字符错误。

## 依赖与调用关系

外部代码依赖仅为标准库 `std::fmt`，用于实现 `ParseError` 的显示格式；本文件没有使用 `pkg/parser/Cargo.toml` 中 parser 子 crate、`regex`、`sha2` 等包级依赖。

RustCodeGraph 核对出的主要调用链如下：

- `main` 调用文件级 `parse`。
- 文件级 `parse` 调用 `lex`、`Parser::new` 和 `Parser::parse`。
- `Parser::parse` 调用 `parse_statement`、`at`、`bump`、`expect`。
- `parse_statement` 分别调用 `parse_select` 或 `parse_create_table`。
- `parse_select` 调用 `parse_expression(0)`；`parse_expression` 调用 `parse_prefix_expression`、`infix_binding_power`，并递归调用自身。
- `parse_prefix_expression` 在一元和括号分支回调 `parse_expression`，在类型关键字分支调用 `parse_typed_literal`。

RustCodeGraph 对同名 `Parser::parse` 与文件级 `parse` 会返回两个定义，调用报告中也可能把同名边同时列出；结合源码位置可消歧为上述链路。仓库搜索未发现其他源码或测试直接调用本文件私有符号。

## 错误处理与边界

全部可预期失败统一返回 `Result<_, ParseError>`，并用 `?` 从内层传播到 `parse`/`main`。词法阶段报告三类明确错误：未闭合字符串定位到起始单引号，超过 `i64` 的十进制整数定位到数字开头，未知字节定位到该字节。语法阶段包括错误的语句起始、缺失指定 token、缺失表达式、类型字面量后不是字符串，以及 `CREATE TABLE` 后不是普通标识符。

边界和限制包括：

- 空输入、仅分号、`SELECT` 后无表达式、尾随逗号都会失败。
- 只接受单条语句和最多一个尾部分号；第二条语句或第二个分号触发 `expected End`。
- 字符串没有转义或双单引号处理，遇到第一个后续单引号即结束。
- 负整数在词法层面是 `Minus + Number`，在 AST 中表示一元表达式；数字本体必须落入 `i64` 正数解析范围。
- `DATE/TIME/TIMESTAMP` 后只验证 token 类别为字符串，不校验日期时间内容。
- `CREATE TABLE` 不接收列定义、限定名、引用标识符或其他 DDL 子句。
- 运算符右侧缺项、括号未闭合和输入尾部都能借助 `End` 产生错误而非推进越界。

`main` 只打印错误并继续下一条样例，不设置非零退出码；因此它适合观察演示结果，不适合作为验证失败的命令行协议。

## 并发与资源生命周期

文件没有线程、异步任务、锁、通道、事务、文件或网络资源。两条示例 SQL 在 `main` 的普通 `for` 循环中串行处理。每次迭代中，输入是静态字符串切片；`lex` 分配 token 及字符串，`Parser` 随后取得 token 所有权，返回的 `Statement` 再拥有 AST 字符串和子节点。迭代结束后结果及全部临时分配按 Rust 所有权规则释放。

递归只发生在表达式解析。极深括号、连续一元运算符或极长的右侧嵌套可增加调用栈深度；当前演示没有深度或输入长度限制。扩展为面向不可信输入的生产路径前，应单独评估栈耗尽和分配上限，而不是假定当前实现已有防护。

## 与 Go 版本的对应关系

仓库中不存在同路径 `pkg/parser/parse-demo1.go`，Git 历史显示该 Rust 文件在提交 `fac562eba94644781b9ac3725fb9c303d0885f75` 中作为新文件加入，没有可逐行移植的 Go 原件。因此它是 Rust 自有教学二进制，而非 Go 文件的一对一复刻。

概念上，它与 Go parser 都经历“词法 token → 语法分析 → AST/错误”，但实现和契约不同。Go 生产入口位于 `pkg/parser/yy_parser.go` 的 `Parser.ParseSQL`、`Parser.Parse` 和 `Parser.ParseOneStmt`；它调用由 `pkg/parser/parser.y` 生成到 `pkg/parser/parser.go` 的 `yyParse`，并使用正式 `pkg/parser/ast` 类型。Rust 生产库则由 `pkg/parser/lib.rs` 组装相应 lexer、生成表与 parser 实现。相比之下，本文件自建私有 AST，仅支持两个语句外形和少量表达式，不参与 Go/Rust parser 的兼容契约。

相关 Go 测试 `pkg/parser/main_test.go` 负责整个 Go 测试进程的泄漏检查，并不测试本 demo；Rust 的 `pkg/parser/main_test.rs` 验证生产 parser 入口能解析 `select 1`，同样不包含本文件。仓库搜索未发现 `parse-demo1` 的独立测试。因此生产 parser 的 Go/Rust 测试不能被当成本 demo 已覆盖的证据。

## 扩展指南

若只是扩充演示语法，应保持词法、AST、解析和测试四层同步：新增 token 时修改 `TokenKind` 与 `lex`；新增 AST 形态时修改 `Statement`/`Expr`/运算符枚举；新增语句时从 `parse_statement` 接线并增加专用解析函数；新增二元运算符时同时在词法器与 `infix_binding_power` 定义 token、AST 运算符及左右绑定力。新增前缀项应接入 `parse_prefix_expression`。任何关键字新增都会改变同名文本能否作为 `CREATE TABLE` 的普通标识符，需显式评估兼容性。

由于现状没有直接测试，安全扩展时应新建独立测试文件，而不要把 `#[cfg(test)]` 测试嵌入 `parse-demo1.rs`。可把可测试实现迁入独立模块并由二进制调用，测试至少覆盖：大小写关键字、空白和偏移；字符串未闭合与整数溢出；乘除优先级、同级左结合、一元优先级、括号；空 `SELECT`、多表达式和尾随逗号；类型字面量缺字符串；`CREATE TABLE` 缺标识符；可选分号与尾随输入。

若目标是扩展生产 SQL 能力，不应继续放大本示例，而应修改 `pkg/parser/lib.rs` 组装的正式 lexer/grammar/parser 路径，并同步对应的独立 Rust 测试和 Go 语义证据。性能风险主要来自全量 token/字符串分配和递归深度；兼容风险主要来自关键字集合、绑定力、错误偏移及新旧 AST 形状变化。

## 验证依据

- 源码全量阅读：`pkg/parser/parse-demo1.rs`，包括 9 个核心类型、`ParseError` 的实现、`lex`、`Parser` 的 15 个方法、`infix_binding_power`、文件级 `parse` 与 `main`；文件无条件编译项和公开符号。
- crate 边界：`pkg/parser/Cargo.toml` 明确声明 `[[bin]] name = "parse-demo1"`、`path = "parse-demo1.rs"`，同时 `lib.rs` 是独立库入口；根 `Cargo.toml` 把 `pkg/parser` 列为 workspace member。
- RustCodeGraph：`status` 显示索引包含 `pkg/parser/parse-demo1.rs`（该文件共 31 个索引符号）；`node --file` 核对 1–427 行；`node` 查询 `parse_expression`、`parse`、`main`、`lex` 与 `infix_binding_power`，确认本文件内部 callers/callees 和递归边。
- Go 对照：确认不存在 `pkg/parser/parse-demo1.go`；读取 `pkg/parser/yy_parser.go` 的生产入口定位、生成的 `pkg/parser/parser.go` 及 `pkg/parser/parser.y` 的 `yyParse` 关系，并读取 `pkg/parser/lib.rs` 区分 Rust 正式 parser 与 demo。
- 测试证据：仓库范围搜索未发现 `parse-demo1` 或其样例 SQL被独立测试引用；读取 `pkg/parser/main_test.rs` 与 `pkg/parser/main_test.go` 后确认二者属于正式 parser/包级测试，而非 demo 测试。
- 历史证据：`git log --follow -- pkg/parser/parse-demo1.rs` 只找到初始提交 `fac562eba94644781b9ac3725fb9c303d0885f75`，其补丁为完整新建文件。
- 本任务是纯文档分析，按任务约束未运行 Cargo，也未执行二进制；行为结论来自源码、调用图、Cargo/Go/测试和 Git 历史静态证据。交付结构以任务文件指定的 11 个固定二级标题命令验证。
