# `pkg/parser/parsergen/grammar.rs`

## 文件定位

`grammar.rs` 是 `astersql-parsergen` crate 的文法输入层：它把维护在 `pkg/parser/grammar/main.astergram` 与 `pkg/parser/grammar/hint.astergram` 中的文本解析为强类型 `Grammar`，并在交给自动机和分析表构造之前完成静态合法性检查。crate 边界由 `pkg/parser/parsergen/Cargo.toml` 定义；`pkg/parser/parsergen/lib.rs` 以 `pub mod grammar` 声明模块并公开重导出其 API。

运行时 SQL 解析并不逐次调用本文件。生成流程中的 `generate_parser_output`（`pkg/parser/parsergen/generate.rs`）读取 `.astergram`，调用 `Grammar::parse`，再把结果传给 `GeneratedParser::build` 并渲染为 `pkg/parser/generated/*.rs`。因此本文件位于“受维护的文法清单 → LALR(1) 自动机/表 → 签入的 Rust 解析表”链路起点，而不是 SQL 请求热路径。

## 核心职责

- 定义生成器各后续阶段共享的文法模型：`SourceSpan`、`Token`、`Associativity`、`PrecedenceSymbol`、`Precedence`、`ProductionItem`、`RuleId`、`Production` 和 `Grammar`。
- 由 `Lexer::tokenize` 识别 `.astergram` 的声明、分区符、标识符、数字、引号字面量、产生式标点、`@action`、`ε` 与文件结束，并保留字节区间及从 1 开始的行列位置。
- 由 `Parser::parse` 解析 `%start`、`%token`、`%left`、`%right`、`%nonassoc`、`%precedence`、`%%` 和产生式备选分支，同时拒绝重复声明。
- 将尾部 `@action` 记录为 `Production::requires_action`，将规则中间动作记录为带符号位置的 `ProductionItem::Action`；动作代码本身不在文法文件中，本文件也不执行语义动作。
- 通过 `validate_references` 检查起始符、优先级符号、右部符号和 `%prec` 覆盖均能解析到已声明对象。
- 通过 `canonical_signature` 与 `rule_id` 产生与声明顺序、进程哈希随机化无关的规则身份，供生成表和独立 Rust 语义动作映射使用。

## 主要符号

- `SourceSpan { start, end, line, column }`：源码半开字节区间 `[start, end)` 及起点行列；`through` 把起点 span 延伸至另一个 span 的末端。偏移按 UTF-8 字节推进，行列按 `char` 推进。
- `Token { name, number, literal, span }`：固定编号的终结符；编号 `0` 明确保留给输入结束。可选 `literal` 支持在产生式及优先级声明中按引号字面量引用 token。
- `Associativity` 与 `Precedence`：分别表达左结合、右结合、非结合或仅优先级，以及按声明顺序从 `1` 递增的优先级层级。`PrecedenceSymbol` 区分名称和字面量，避免二者混为一个命名空间。
- `ProductionItem`：右部可以是命名符号、字面量或中间动作位置；`Action { position }` 的位置是动作前已经出现的文法符号数，不把动作自身计入。
- `Production`：保存左部、结构化右部、可选 `%prec`、是否需要动作、源 span、规范签名和稳定 `RuleId`。同一 `lhs : a | b;` 会展开为两个 `Production`。
- `RuleId(String)`：外部只能用 `as_str` 或 `Display` 读取；内部 `rule_id` 使用最长 48 字符的可读签名前缀和 64 位 FNV-1a 摘要构造稳定值。
- `Grammar`：公开聚合起始符、token、优先级和产生式；唯一公开入口 `Grammar::parse(&str) -> Result<Grammar, GrammarError>` 串联词法、语法和引用校验。
- `GrammarErrorKind`、`GrammarError`：稳定分类解析/校验失败，并携带消息和 `SourceSpan`；实现 `Display` 和标准 `Error`。
- `LexemeKind`、`Lexeme`、`Lexer`、`Parser`：文件私有的词法单元和两阶段解析状态，不暴露给生成器其他模块。
- `validate_references`、`canonical_signature`、`rule_id`：文件私有的后置引用检查、规则规范化及稳定标识生成函数。

## 执行流程

1. `Grammar::parse` 构造 `Parser::new`；后者先让 `Lexer::tokenize` 一次性扫描全部输入，并在尾部加入 `LexemeKind::End` 哨兵。词法器先跳过空白、`#` 行注释和 `//` 行注释，再按首字符分派 token。
2. `Parser::parse` 在 `%%` 之前只接受声明指令。解析过程中用四个 `HashMap` 分别追踪 token 名称、编号、字面量和优先级符号的首次位置，使重复错误指向后一次声明并在消息中报告首次行号。
3. `%start` 必须恰好一次；`%token NAME NUMBER ["literal"];` 要求名称、非零编号和可选字面量各自唯一；四类优先级声明至少有一个符号，且声明次序决定 `level`。
4. 遇到 `%%` 后，解析器要求已经存在 `%start`，然后循环读取 `lhs : alternative | alternative ;`。`parse_alternative` 将空分支或单独的 `ε` 表示成空 `rhs`，处理符号、字面量、中间/尾部动作和至多一个 `%prec` 覆盖。
5. 每个分支经 `canonical_signature` 规范化。空右部显式写成 `ε`，字面量重新转义，中间动作写成 `@<position>`，覆盖优先级附加 `%prec`；相同签名立即报 `DuplicateProduction`，否则据此生成 `RuleId`。
6. 至少生成一个产生式后，`validate_references` 建立 token 名称、token 字面量和产生式左部集合：起始符必须是非终结符，优先级声明只能引用 token，`%prec` 必须引用已声明优先级，右部只能引用 token、非终结符、已声明 token 字面量或动作。
7. 全部检查通过才返回 `Grammar`。随后 `generate_parser_output` 把它交给 `GeneratedParser::build`，下游 `automaton.rs`、`table.rs` 和 `render.rs` 消费这些结构及 `RuleId`。

## 数据与状态

解析期间的可变状态严格局限于当前调用。`Lexer` 借用输入字符串，维护 UTF-8 字节 `offset`、`line` 和 `column`；扫描产出的 `Vec<Lexeme>` 拥有标识符和字面量字符串。`Parser` 再以 `cursor` 顺序消费该向量，`advance` 在 `End` 上不继续前移，从而让报错路径安全查看 EOF。

唯一性索引使用 `HashMap`/`HashSet`，但不会把迭代顺序写入输出：声明和产生式按源文件次序保存在 `Vec` 中，`RuleId` 仅由规范签名字节计算。这一不变量由 `grammar_rule_ids_are_reorder_stable` 验证。`RuleId` 的可读前缀会把非 ASCII 字母数字折叠为下划线并截到 48 字符，真正区分规则的是后缀 FNV-1a 摘要；它提供确定性身份，不是安全哈希或碰撞证明。

空分支和显式 `ε` 最终都是空 `rhs`。尾部 `@action` 不进入 `rhs`，只设置 `requires_action`；非尾部 `@action` 同时设置该标志并进入 `rhs`，其位置参与签名和规则身份。`Production::span` 从左部起点覆盖到该备选最后一个被消费的元素；错误位置则尽量使用引发错误的局部 token span。

## 依赖与调用关系

本文件只依赖标准库 `HashMap`、`HashSet`、`fmt` 和 `Error`；`pkg/parser/parsergen/Cargo.toml` 的 `[dependencies]` 为空。`pkg/parser/parsergen/lib.rs` 将本模块的公开类型重导出到 `astersql_parsergen::*`。

直接生产调用边由源码搜索确认：`pkg/parser/parsergen/generate.rs::generate_parser_output` 调用 `Grammar::parse`，随后调用 `GeneratedParser::build` 和 `render_compatibility_tables`。同 crate 的 `automaton.rs` 读取 `Grammar`、`ProductionItem`、`RuleId` 构造 LR 项目/状态，`table.rs` 用优先级、结合性和规则身份解决冲突并编码表，`render.rs` 把规则身份及表渲染成 Rust 源码。

`pkg/parser/Cargo.toml` 把 `astersql-parsergen` 作为开发依赖；`pkg/parser/lib.rs` 在测试配置下还以路径直接挂载本文件，使 parser crate 的清单/语义动作测试可以解析主文法。源码搜索到的直接测试调用者包括 `parser_manifest_aster_unit_test.rs`、`parsergen_baseline_aster_unit_test.rs` 以及 `parser_actions/*_aster_unit_test.rs`；它们用稳定 `RuleId` 对照生成表和语义动作覆盖面。

## 错误处理与边界

所有预期输入失败都返回 `GrammarError`，不使用 panic。词法错误覆盖非法字符、未知 `@` 标记、数字溢出、非法转义、未闭合或跨行引号；支持的转义仅为反斜杠、单双引号、`n`、`r`、`t` 和 `0`。标识符首字符仅允许 ASCII 字母或下划线，后续另允许 ASCII 数字、连字符和点。

结构错误包括未知指令、意外 token、缺失/重复 `%start`、缺失 `%%`、无产生式、token 名称/编号/字面量重复、优先级符号重复和产生式签名重复。编号 `0` 由 `ReservedTokenNumber` 拒绝，因为解析器运行时将其用作 EOF。`ε` 必须独占右部，且不能位于中间动作之前；空备选无需写 `ε` 也合法。`%prec` 每个分支最多一次，并须引用已经进入优先级声明的符号。

引用检查是全文件后置执行，所以允许产生式引用后面才定义的非终结符。另一方面，诊断遇到首个错误即返回，不聚合多个问题。`SourceSpan` 的 `line`/`column` 是字符计数，而 `start`/`end` 是字节偏移；调用方展示切片时必须按字节边界使用 offset。稳定 `RuleId` 依赖规范签名格式与 FNV-1a 算法，修改任一者会改变生成表和语义动作映射，属于兼容性敏感变更。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄或事务。每次 `Grammar::parse` 都拥有独立的 lexer/parser、集合和输出模型，除只读借用调用方的 `&str` 外没有全局或共享可变状态，因此不同线程可并行解析不同输入。

输入借用只持续到 `Lexer::tokenize` 完成；返回的 `Grammar`、错误消息和 span 均不借用输入内容。内存峰值包含原始输入之外的一份完整 lexeme 向量、字符串副本、唯一性索引和最终模型；这是一次性生成/检查路径的设计取舍。函数提前返回错误时，这些拥有型资源由 Rust 自动释放，无额外清理协议。

## 与 Go 版本的对应关系

仓库不存在 `pkg/parser/parsergen/grammar.go` 或与本文件逐函数对应的 Go 实现。Go 历史链路以 `pkg/parser/parser.y`、`pkg/parser/hintparser.y` 表达 yacc 文法，由 `pkg/parser/goyacc/main.go` 驱动生成 `parser.go`/`hintparser.go`；这些 `.y` 文件还内嵌 Go 类型声明和语义动作。相比之下，Rust 链路把可生成的文法元数据抽到 `pkg/parser/grammar/*.astergram`，并用本文件解析，语义动作按稳定 `RuleId` 放在独立 Rust 模块中。

两边保留的核心 yacc 语义包括起始符、终结符及固定编号、字面量、优先级/结合性、产生式备选、空产生式和 `%prec`。差异在于 `.astergram` 用分号结束声明和产生式，用 `@action` 只标记动作存在/位置，不携带 Go 代码；本文件还增加了明确的重复/引用校验、源码 span 和稳定规则 ID。`pkg/parser/generate.go` 只保留 Go 的 `go:generate ./genkeyword`，不能视为此 Rust grammar parser 的实现。

因此对齐时应比较“文法与生成结果的行为契约”，不能要求 Rust 数据结构机械复刻 goyacc 内部 AST。当前真实输入 `main.astergram` 和 `hint.astergram` 的 token 编号及规则，应与旧 yacc 产物和生成的 Rust 表保持兼容；对应证据由 parsergen baseline、manifest 及语义动作清单测试承担。

## 扩展指南

- 增加新声明或语法记号时，先扩展 `LexemeKind`/`Lexer::tokenize`，再在 `Parser::parse` 或 `parse_alternative` 接线，并为所有失败分支选择具体 `GrammarErrorKind` 和准确 span；不要用通用成功路径绕过引用校验。
- 改变标识符、字符串或注释规则时，同步检查 `take_quoted`、`skip_layout`、`is_identifier_start`/`is_identifier_continue`，并考虑 UTF-8 字节 offset 与字符列号的差异。
- 增加会影响规则语义的右部元素时，必须同步修改 `canonical_signature`；否则不同规则可能共享身份。修改签名展示、前缀截断或 FNV 算法会整体改变 `RuleId`，需同步生成产物、语义动作映射和兼容性基线，不能当作纯重构。
- 扩充引用规则时从 `validate_references` 接入，并保持前向非终结符引用可用。新增优先级能力还要检查 `table.rs` 的冲突处理，而不仅是本文件能否解析。
- 测试逻辑继续放在独立文件：基础解析/诊断加入 `pkg/parser/parsergen/grammar_aster_unit_test.rs`，关键回归可加入 `grammar_test.rs`；跨模块行为分别在 automaton/table/render/generate 的独立测试及 `pkg/parser/*_aster_unit_test.rs` 验证。不要把测试内嵌回 `grammar.rs`。
- 安全扩展后应至少覆盖成功模型、错误类别和 span、规范签名/`RuleId` 稳定性，以及真实 `main.astergram`/`hint.astergram` 到生成表的基线；本分析任务本身按要求不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件且 `pkg/parser/parsergen/grammar.rs` 已索引为 89 个符号；`files --filter pkg/parser/parsergen` 确认 crate 内 14 个 Rust 文件；`node --file ...` 分段核对全部 1,031 行；`node Grammar` 确认结构定义及 `Parser::parse` 构造边。对常见名 `parse` 的全局查询存在大量歧义，因此直接调用边再以限定目录源码搜索核实。
- 源码：`pkg/parser/parsergen/grammar.rs`（公开模型、词法/语法流程、校验、签名与 ID）；`lib.rs`（模块和重导出）；`generate.rs::generate_parser_output`（生产入口）；`automaton.rs`、`table.rs`、`render.rs`（下游消费者）。
- 配置与真实输入：`pkg/parser/parsergen/Cargo.toml`（独立 crate、无外部依赖）；`pkg/parser/Cargo.toml`（开发依赖）；`pkg/parser/grammar/main.astergram` 与 `hint.astergram`（实际格式和生成输入）。
- Rust 测试：`pkg/parser/parsergen/grammar_aster_unit_test.rs` 验证完整最小文法、重复 token、重排稳定 ID、空分支/转义、重复编号/产生式及未知符号位置；`grammar_test.rs` 固定 EOF 编号 `0` 保留规则。`parsergen_baseline_aster_unit_test.rs`、`parser_manifest_aster_unit_test.rs` 与 `parser_actions/*_aster_unit_test.rs` 提供跨模块 RuleId/清单证据。
- Go 对照：`pkg/parser/parser.y`、`hintparser.y`、`goyacc/main.go`、生成的 `parser.go`/`hintparser.go` 以及 `generate.go`，用于确认旧 yacc 链路与 Rust 专用描述层的职责差异；未发现同路径或逐函数 Go 实现。
- 本文档只记录静态分析证据；按任务约束未运行 Cargo。交付验证使用任务指定的 11 章结构命令，并人工复核本文能够回答文件存在原因、执行方式和安全扩展位置。
