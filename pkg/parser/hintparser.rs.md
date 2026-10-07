# `pkg/parser/hintparser.rs`

## 文件定位

本文件是 `astersql-parser` crate 内 optimizer hint 的 LALR 解析核心：接收 `hintScanner` 已经切分出的 token，通过生成表执行移进/归约，并构造 `parser_ast::TableOptimizerHint`。它不负责从完整 SQL 中识别 `/*+ ... */`，也不直接提供公开入口；词法扫描、解析器实例和公开 `ParseHint` 位于 [`hintparserimpl.rs`](./hintparserimpl.rs)。

模块装配见 [`lib.rs`](./lib.rs)：`parser_impl::lexer_support` 依次 `include!` 生成 token/表、本文件、`hintparserimpl.rs`、通用 lexer 等，因此本文件可直接使用同一模块作用域中的 `ast`、`Error`、错误常量、`hintParser`、`yyhintSetOffset` 和生成常量。SQL 主解析链在 [`parser_actions/query.rs`](./parser_actions/query.rs) 的 `TableOptimizerHintsAlt01` 调用 `Parser::parseHint`；后者位于 [`yy_parser.rs`](./yy_parser.rs)，复用 `hintParser` 并最终进入本文件的 `yyhintParse`。crate 根还再导出 `ParseHint`，允许独立解析一段完整的 hint 注释。

## 核心职责

- `yyhintSymType` 定义 token 与非终结符共用的语义栈槽，承载标识符、整数、单个/多个 hint、表、分区名和 LEADING 嵌套结构。
- `yyhint_tables` 为 [`generated/hint_tables.rs`](./generated/hint_tables.rs) 提供有边界检查的访问层：token 翻译、符号名、规则元数据和稀疏动作表查询。
- `yyhintlex1` 把 lexer 返回的非正 token 归一为 EOF；`yyhintParse` 维护状态栈、语义值栈和 lookahead，完成 shift、reduce、goto、accept 与失败返回。
- `reduce_hint` 实现生成文法中所有带动作的 legacy 规则，生成或组合 `TableOptimizerHint`、`HintTable`、`HintData`、索引列表、分区列表及 LEADING 树。
- 对明确列为伪 hint/暂不支持的规则追加 warning 而不产出 AST；对 `MEMORY_QUOTA` 乘法溢出降级为 warning；对越界负整数停止解析。

## 主要符号

- `yyhintSymType`：私有解析语义槽，仅以 `pub(super)` 暴露给同一 `parser_impl` 的相邻装配层。`yys` 是状态，`offset` 是位置；`ident`/`number` 是词法值；`hint`/`hints`、`table`、`modelIdents`、`leadingList`、`leadingElement` 是归约结果。它实现 `Default + Clone`，因为 shift 和 reduce 都会复制栈值。
- `LeadingElement::{Table,List}`：Rust 对 Go `interface{}` 的类型安全替代；`From<LeadingElement> for ast::LeadingItem` 把元素写入 `LeadingList::Items`。
- `yyhintXError { state, xsym }`：扩展错误表键。当前 `yyhint_tables::error_message` 恒为 `None`，候选查找最终总是回退为 `"syntax error"`，且该字符串随后没有传给 `Errorf`。
- `yyhintLexer`：本文件与 scanner 的协议，包括 `Lex`、`Errorf`、error/warn 累积和诊断读取。`yyhintLexerEx` 增加 `Reduced` 回调；默认 `as_extended` 返回 `None`，仓库当前 `hintScanner` 实现没有覆盖它。
- `yyhintSymName` / `yyhintlex1`：分别完成 token 的可读名称映射和 EOF 归一。未知 token 名回退为十进制编号。
- `yyhintParse`：解析主循环。返回 `0` 表示接受，`1` 表示语法、goto 或语义动作失败，`-1` 预留给扩展 lexer 请求提前终止。
- `reduce_hint`：按 `GENERATED_HINT_LEGACY_RULES` 映射后的 Go 规则号分派语义动作。无显式动作的规则保留 yacc 默认值。
- `new_data_hint`：统一构造带 `HintName`、`QBName`、`HintData` 的 boxed AST；其他字段取默认值。
- `reduce_table_and_index`：合并规则 48、49、52、53、56、57、59、60 的表/索引列表动作，保持追加顺序。
- `yyhintDebug`：当前固定返回 `0`，所以调试打印分支默认不可达。

## 执行流程

1. 完整 SQL 路径由 [`parser_actions/query.rs`](./parser_actions/query.rs) 取出 hint 文本、当前 SQL mode 和位置，调用 [`yy_parser.rs`](./yy_parser.rs) 的 `Parser::parseHint`；独立调用方也可直接使用 [`hintparserimpl.rs`](./hintparserimpl.rs) 的 `ParseHint`。
2. `hintParser::parse` 跳过输入开头的 `/*+`，重置 scanner、SQL mode、位置和 bang-comment 状态，然后临时移出 lexer 并调用 `yyhintParse`。
3. `yyhintParse` 将状态栈初始化为状态 0、值栈初始化为默认槽；lookahead 为空时调用 `yyhintlex1`，再由 `yyhint_tables::xlat` 将外部 token 转为动作表列。未知 token 使用符号表长度作为越界列，动作查询自然得到 0。
4. 动作为 `GENERATED_HINT_ACCEPT` 时保存值栈并返回成功；正数动作表示 shift，实际状态为 `action - 1`，当前 `yylval` 被复制入值栈并清空 lookahead。
5. 动作为负数时，索引 `GENERATED_HINT_REDUCTIONS` 得到左侧符号和 RHS 长度，再映射到 legacy rule。归约值默认复制 RHS 第一个槽，随后 `reduce_hint` 覆盖需要的字段。
6. 归约后同时截短状态栈和值栈，用基状态与左侧符号查 goto；goto 非正会追加 `invalid parser goto` 错误并失败。正常路径记录 offset、可选调用 `Reduced`，再推入新状态和值。
7. `reduce_hint` 先组合顶层 hint 列表；随后按类别生成无参、带 query block、数值、布尔、名称、`SET_VAR`、`TIME_RANGE`、表/索引、存储类型和 LEADING 等 AST。规则 1 最终把完整列表写入 `parser.result`。
8. `hintParser::parse` 收回 lexer；若 scanner 有 error，只返回 errors，否则返回 warnings，并移出 `parser.result`。

## 数据与状态

解析期间有两条等长的动态栈：`states: Vec<i32>` 保存自动机状态，`values: Vec<yyhintSymType>` 保存语义值。初始哨兵槽保证归约后总能取得基状态；shift、truncate 和最终 push 必须保持二者同步。`parser.yylval` 是当前 lookahead 的词法值，`lookahead == -1` 表示需要重新取词，`shifted_state` 只为错误候选键保留最近移入状态。

AST 通过 `Box` 和拥有所有权的 `String`/`Vec` 保存。大多数归约先 clone 前序值再追加，避免栈截断后出现借用失效；代价是长表列表、索引列表或 LEADING 树会产生复制。LEADING 同时保存两种视图：`HintData::Leading` 保留嵌套与顺序，`Tables` 通过 `ast::FlattenLeadingList` 保存扁平表序列，供后续初始化逻辑使用。

生成数据是 [`generated/hint_tables.rs`](./generated/hint_tables.rs) 中的只读静态切片：`GENERATED_HINT_XLAT`、`GENERATED_HINT_SYMBOL_NAMES`、`GENERATED_HINT_REDUCTIONS`、`GENERATED_HINT_LEGACY_RULES`、`GENERATED_HINT_PARSE_TABLE`。本文件不修改这些表。`hintParser.cache` 会接收退出时的值栈，但当前 Rust 主循环每次仍新建两个 `Vec`；它没有像 Go 版那样以 cache 作为初始可复用栈。

## 依赖与调用关系

[`Cargo.toml`](./Cargo.toml) 将本模块归入 `astersql-parser`，库入口为 `lib.rs`，并以路径依赖使用 `astersql-parser-ast`、mysql、terror、types、charset 等子 crate。本文件直接依赖 AST 类型和同一 include 作用域中的 parser/lexer 支撑；它没有自行引入第三方 crate。

主调用链为 `Parser grammar action -> Parser::parseHint -> hintParser::parse -> yyhintParse -> yyhintlex1 / yyhint_tables / reduce_hint`。`reduce_hint` 的下游包括 `ast::NewCIStr`、`ast::FlattenLeadingList`、`TableOptimizerHint`/`HintTable`/各类 `HintData` 构造，以及 lexer 的 `AppendWarn`/`AppendError` 和 `parser.lastErrorAsWarn`。公开旁路是 `ParseHint -> newHintParser -> hintParser::parse`。

RustCodeGraph 精确查询识别出 `pkg/parser/hintparser.rs::yyhintParse`、`reduce_hint`、`yyhintLexer`，并同时显示 Go 同名节点；图的 callers/callees 命令未返回可用边，因此调用关系又以源码搜索核实。直接 Rust 调用点是 [`hintparserimpl.rs`](./hintparserimpl.rs) 的 `yyhintParse(&mut lexer, self)`；SQL 主链的桥接点则是 [`parser_actions/query.rs`](./parser_actions/query.rs) 与 [`yy_parser.rs`](./yy_parser.rs)。

## 错误处理与边界

词法层产生的非法整数、非法 token 和括号深度错误由 [`hintparserimpl.rs`](./hintparserimpl.rs) 先累积；本文件在动作表返回 0 时再次通过 `Errorf("", &[])` 追加语法错误，保存当前值栈并立即返回 `1`。虽然代码计算了四个 `(state, symbol)` 扩展错误候选，但 `error_message` 当前固定为 `None`，且回退消息没有进入 `Errorf`，所以实际诊断依赖 scanner 的位置包装而非生成表文案。

这与 Go 生成 parser 存在重要差异：[`hintparser.go`](./hintparser.go) 使用 `Errflag`、error token 139、弹栈和丢弃 lookahead 尝试恢复；Rust 当前首次无动作即终止，不会继续解析后续 hint。扩展 lexer 的提前终止形状被保留，但现有 `hintScanner` 不暴露该实现。

规则 20 在 `MEMORY_QUOTA` 的数值乘单位前用 `i64::MAX / unit` 防溢出；溢出时先追加错误，再由 `lastErrorAsWarn` 降级并丢弃该 hint。规则 72 允许绝对值恰为 `2^63` 生成 `i64::MIN`，更大则追加范围错误并终止。多个动作使用 `unwrap` 读取文法保证存在的值；因此生成表、legacy rule 映射与语义动作一旦漂移，可能 panic，而不是返回结构化错误。`input[3..]` 的最小长度/UTF-8 边界契约属于调用它的 `hintParser::parse`，本文件假定已经获得合法 token 流。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务或 I/O。生成表是不可变 `static`，可跨线程安全共享；所有可变解析状态都属于一次 `hintParser::parse` 调用。公开 `ParseHint` 每次新建 parser，因此调用间不共享状态；完整 SQL 的 `Parser::parseHint` 会在同一个 `Parser` 内复用 `hintParser`，但 `parse` 会清空结果并重置 lexer。

`yyhintParse` 持有对 lexer 和 parser 的独占可变借用。`hintParser::parse` 用 `std::mem::take` 暂时移出 lexer，是为同时满足对 lexer 与 parser 的可变访问；无论 `yyhintParse` 返回成功还是普通失败，随后都会把 lexer 放回。这里没有 panic 清理保护：若语义动作中的 `unwrap` panic，赋回语句不会执行。AST 与栈在返回或覆盖时按 Rust 所有权自动释放。

## 与 Go 版本的对应关系

直接对照是 [`hintparser.go`](./hintparser.go)，文法源是 [`hintparser.y`](./hintparser.y)，词法与公开入口对照是 [`hintparserimpl.go`](./hintparserimpl.go)。`yyhintSymType` 字段、lexer 接口、token 翻译、shift/reduce/goto 结构和规则 1–76 的有动作分支与 Go 保持同一语义顺序；Rust 用 `HintData` enum 代替 Go `interface{}`，用 `LeadingElement`/`LeadingItem` 明确 LEADING 元素类型，用 `Box` 表示指针拥有关系。

语义对齐包括：不支持 hint 只报警告；query block/table/index/partition 按输入顺序保留；READ_FROM_STORAGE 为每个存储类型生成 hint；LEADING 同时保留嵌套与扁平视图；`SET_VAR`、布尔、时间范围和内存配额分别进入对应 `HintData` 变体；负整数 `-2^63` 是合法边界。

已确认的实现差异是：Rust 动作表把状态编码为 `action - 1` 并用 `i32::MIN` 表示 accept；Go 使用 `yyhintTabOfs`；Rust 不实现 Go 的语法错误恢复；扩展错误消息表为空实现；Rust 当前没有真正复用 `hintParser.cache` 的容量；Rust 的不支持 hint 规则在少数字段为空时会反向搜索最近的大写标识符作为兼容兜底，Go 直接使用固定栈偏移。修改时应以文法、Go 生成物与 Rust 生成表三方核验，不能仅凭同名函数认定完全等价。

## 扩展指南

- 新增或修改 hint 语法应先改文法/生成链并同步 [`generated/hint_tables.rs`](./generated/hint_tables.rs)，再核对 `GENERATED_HINT_LEGACY_RULES` 与 `reduce_hint` 的 rule 编号；切勿只手改动作 match，否则表与语义会错位。
- 新 `HintData` 形态优先通过 `new_data_hint` 或清晰的专用分支构造；若涉及表、索引、分区或 LEADING，应同时验证输入顺序、query block 归属、嵌套结构和扁平兼容视图。
- 修改错误路径时必须决定是否补齐 Go 的恢复语义、扩展错误消息和多 hint 后续解析。该区域会改变诊断数量、错误优先级及是否保留前面已成功的 AST，需要 Rust/Go 对照用例锁定。
- 调整栈或 cache 复用必须保持状态栈/值栈同长、归约基状态有效、默认 `$$ = $1` 语义以及所有提前返回路径保存/清理状态；性能优化不能以跳过语义 clone 后产生别名修改为代价。
- 测试应继续放在独立 [`hintparser_test.rs`](./hintparser_test.rs) 或相邻独立 parser 测试文件，不要嵌入生产文件。至少覆盖新增成功 AST、非法 token、数值上下界、SQL mode/位置传播、完整 SQL 接线和 Go 对照。

## 验证依据

- 源码全量读取：[`hintparser.rs`](./hintparser.rs) 的全部类型、trait、函数和规则分支；[`generated/hint_tables.rs`](./generated/hint_tables.rs) 的 token、accept、翻译表、符号表、归约/legacy rule 和动作表声明。
- 模块与调用链：[`lib.rs`](./lib.rs) 的 include/re-export 和独立测试装配；[`Cargo.toml`](./Cargo.toml) 的 crate 边界、路径依赖与 `go-package = "pkg/parser"`；[`hintparserimpl.rs`](./hintparserimpl.rs)、[`yy_parser.rs`](./yy_parser.rs) 和 [`parser_actions/query.rs`](./parser_actions/query.rs) 的公开入口与完整 SQL 桥接。本包未发现 `doc.go`，因此采用这些最近入口作为包级契约证据。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`query yyhintParse`、`query reduce_hint`、`query yyhintLexer`、`query hintParser` 确认 Rust/Go 主要符号和实现位置。目标路径的 `files --filter` 以及精确 callers/callees 没有返回可用文件/边，故按技能规则用源码调用点搜索补齐证据。
- Go 对照：[`hintparser.go`](./hintparser.go) 的生成栈机、错误恢复和规则 1–76；[`hintparserimpl.go`](./hintparserimpl.go) 的 parser 生命周期、诊断选择和公开入口；[`hintparser.y`](./hintparser.y) 的语法与语义来源。
- 独立测试：[`hintparser_test.rs`](./hintparser_test.rs) 覆盖空输入、非法 token、不支持 hint、MEMORY_QUOTA、QB_NAME、SET_VAR、表/索引/分区、存储类型、TIME_RANGE 和 LEADING，并深查各类 `HintData`；[`yy_parser_test.rs`](./yy_parser_test.rs) 覆盖 SQL mode/位置与完整 SQL AST 接线；[`go_merge_29_test.rs`](./go_merge_29_test.rs) 覆盖 10,000 层 hint 括号限制。Go 对照测试为 [`hintparser_test.go`](./hintparser_test.go)。按任务要求未运行 Cargo。
