# `pkg/parser/misc.rs`

## 文件定位

`misc.rs` 是 `astersql-parser` crate 的词法支撑文件。它不是独立 Rust 模块，而是由 [`pkg/parser/lib.rs`](lib.rs) 的 `parser_impl::lexer_support` 通过 `include!("misc.rs")` 嵌入，因此可以直接使用同一作用域中的 `Scanner`、`Pos`、`token::*`、Hint token 常量以及 `lexer.rs` 定义的扫描函数。Cargo 边界见 [`pkg/parser/Cargo.toml`](Cargo.toml)：crate 名为 `astersql-parser`，库入口是 `lib.rs`，该清单没有为本文件设置条件 feature。

它处在“SQL 字节流 → 词法 token → 生成语法分析器”的前半段：[`pkg/parser/lexer.rs`](lexer.rs) 的 `Scanner::scan` 使用这里的 `ruleTable` 选择扫描函数或运算符 token，`Scanner::Lex` 再调用这里的 `Scanner::isTokenIdentifier` 把普通 identifier 提升为关键字、内建函数或窗口函数 token。优化器 Hint 的 lexer 则由 [`pkg/parser/hintparserimpl.rs`](hintparserimpl.rs) 查询 `hintTokenMap`。

## 核心职责

1. 用 `isLetter`、`isDigit`、`isIdentChar`、`isIdentExtend`、`isUserVarChar` 定义与 Go parser 对齐的字节级字符分类；这里处理的是 `u8`，非 ASCII 字节统一按标识符扩展字节接纳，而不是在本层验证完整 Unicode 字符。
2. 用 `trieNode` 和 `buildRuleTable` 建立单字节入口规则、复合运算符最长匹配路径以及特殊前缀扫描函数分派表。
3. 保存普通 SQL 关键字、需结合 `(` 判断的内建函数、窗口函数、关键字别名、可接受 Hint 的语句 token、Hint 关键字等静态映射。
4. 在 `Scanner::isTokenIdentifier` 中结合限定名上下文、`IGNORE_SPACE` SQL mode 和窗口函数开关，决定一个已扫描 identifier 是否应改写为专用 token。

本文件只做词法分类和映射，不构造 AST、不决定语法是否合法，也不直接生成用户可见错误；后续语法接受/拒绝由 parser 生成代码负责。

## 主要符号

- `isLetter(u8) -> bool`、`isDigit(u8) -> bool`：仅认可 ASCII 字母和数字。
- `isIdentExtend(u8) -> bool`：对 `0x80..=0xff` 返回 `true`；`isIdentChar` 在字母、数字、`_`、`$` 之外委托给它。
- `isInCorrectIdentifierName(&str) -> bool`：保留 Go 的反向命名；空串或最后一个字节为空格时返回 `true`，嵌入空格不在此处判错。
- `isUserVarChar(u8) -> bool`：比普通标识符额外允许 `.`，用于用户变量字符集合。
- `trieNode`：每个节点有 256 个可选子节点、一个 `i32` token 和一个可选扫描函数 `fn(&mut Scanner) -> (i32, Pos, String)`。字段公开是因为同一嵌入模块中的 `lexer.rs` 直接读取它们；构造函数 `trieNode::new` 仍为文件内部辅助。
- `initTokenByte`、`initTokenString`、`initTokenFunc`：分别注册单字节 token、多字节 token 路径、以及首字节到扫描函数的分派。`initTokenFunc` 对传入字符串的每个字节都从根节点挂函数，不把字符串建成一条路径。
- `buildRuleTable() -> trieNode` 与 `ruleTable: LazyLock<trieNode>`：前者构建完整规则树，后者首次使用时安全发布不可变全局实例。根 token 是 `invalid`；表中既有 `?`、`=` 等单字符 token，也有 `<=>`、`:=`、`\\N` 等复合 token，并把引号、数字、注释/运算符前缀等交给 `lexer.rs` 的专用函数。
- `lookup_token`、`isInTokenMap`：前者在线性切片表中做精确、区分大小写的名称查询，未命中返回 `0`；后者只检查 `tokenMap`。
- `tokenMap`：普通 SQL 关键字到 parser token 的静态表；输入在查询前通常已由 `isTokenIdentifier` 做 ASCII 大写化。
- `btFuncTokenMap`：只有后续是 `(`，或 `IGNORE_SPACE` 允许跳过空白后遇到 `(` 时，才采用的内建函数 token 表。
- `windowFuncTokenMap`：仅在 `Scanner::supportWindowFunc` 开启且普通关键字未命中时查询。
- `aliases`：记录 `SCHEMA → DATABASE`、`SCHEMAS → DATABASES`、`DEC → DECIMAL`、`SUBSTR → SUBSTRING` 的维护关系；一致性测试要求别名与规范名具有同一 token。
- `hintedTokens`：允许识别优化器 Hint 的语句起始 token 集合，由 `lexer.rs` 的 Hint 入口判断使用。
- `hintTokenMap`：MySQL/TiDB Hint 名称、别名及参数关键字到 Hint lexer token 的表，由 `hintparserimpl.rs` 使用。
- `Scanner::isTokenIdentifier(&mut self, &str, i32) -> i32`：本文件的关键运行时入口；返回 `0` 表示继续把文本当普通 identifier。

## 执行流程

规则树在第一次访问 `ruleTable` 时由 `buildRuleTable` 构造：先把根节点默认 token 设为 `invalid`，再注册固定单字符，随后沿 trie 注册复合运算符，最后为各种首字节挂接 `startWithAt`、`startWithSlash`、`scanIdentifier`、`startString` 等扫描函数。`Scanner::scan` 跳过空白后，从根节点按输入字节向下走；一旦节点带函数就立即委托，纯 token 路径则持续前进，最终返回走到的最深节点 token 和对应文本。这使 `<`、`<=`、`<=>` 等共享前缀按最长已注册路径识别。

当 `Scanner::Lex`、`getNextToken` 或 `getNextTwoTokens` 得到 `identifier` 后，会调用 `isTokenIdentifier`：

1. 若当前 reader 下一字节是 `.`，直接返回 `0`；否则从 `token_offset - 1` 向前跳过 ASCII 空格，若前一个有效字节是 `.` 也返回 `0`。因此 `select.x` 与 `db.select` 两侧都保持限定标识符。
2. 清空并复用 `Scanner::buf`，只将 ASCII 小写字节转成大写，形成查询键；其他字节原样保留。
3. 若紧邻 `(`，查询 `btFuncTokenMap`。启用 `ModeIgnoreSpace` 时，会先调用 `skipWhitespace` 推进 reader，再判断 `(`；命中内建函数 token 时立即返回。
4. 查询 `tokenMap`。只有普通表未命中且 `supportWindowFunc` 为真，才查询 `windowFuncTokenMap`；仍未命中返回 `0`。

Hint 路径不走 `isTokenIdentifier` 的三级判断：`hintparserimpl.rs` 对 identifier 做 Unicode `to_uppercase()` 后直接线性查 `hintTokenMap`，命中则返回 Hint 专用 token，否则保留 `hintIdentifier`。

## 数据与状态

`tokenMap`、`btFuncTokenMap`、`windowFuncTokenMap`、`aliases`、`hintedTokens`、`hintTokenMap` 都是只读静态切片；本文件没有运行时可变全局映射。`ruleTable` 是 `LazyLock` 包装的只读 trie，构建完成后扫描器只读访问。

单次 identifier 判定会修改所属 `Scanner` 的两个局部状态：复用 `buf` 保存规范化名称；在 `IGNORE_SPACE` 分支中，`skipWhitespace` 会推进 reader。这个推进与 Go 行为一致，也是调用者随后看到 `(` 的基础，不应把该方法误当作完全无副作用的查询。限定名回看使用 `token_offset` 索引 `reader.s` 的字节，依赖 offset 与原 SQL 字节位置一致。

映射值和 trie token 都以 `i32` 表示，`0` 被用作“无映射/普通 identifier”哨兵；因此新增有效 token 时不能把 `0` 当作可识别值。`trieNode` 固定保留 256 个槽位，体现本层按字节而非 Unicode scalar 分派。

## 依赖与调用关系

上游调用边：

- `lexer.rs::Scanner::scan` 读取 `ruleTable`、`trieNode` 和 `isIdentExtend`，完成底层 token 扫描。
- `lexer.rs::Scanner::Lex`、`getNextToken`、`getNextTwoTokens` 调用 `Scanner::isTokenIdentifier`，将 identifier 转为语法 token。
- `lexer.rs` 的 Hint 启动逻辑读取 `hintedTokens`；`hintparserimpl.rs` 读取 `hintTokenMap`。
- `parser_impl` 中的生成 parser 通过 lexer 获得这些 token；RustCodeGraph 的文件节点同时确认 `misc.rs` 被 `lexer.rs`、`hintparserimpl.rs` 等索引文件使用。图查询对 `Scanner::isTokenIdentifier` 的 Rust impl 方法未建立独立方法节点，因此上述三条精确调用边由索引中的 `lexer.rs` 源码节点及直接引用检索共同核验。

下游依赖：本文件使用同一 `lexer_support` include 作用域里的 `Scanner`、`Pos`、`token` 与扫描函数；这些名字分别来自 `lexer.rs`、`generated/lexer_tokens.rs` 及周边 include。标准库依赖只有 `std::sync::LazyLock`。Cargo 清单的 parser 子 crate 依赖用于整个 parser crate，本文件没有直接调用第三方 crate API。

## 错误处理与边界

字符分类函数不返回 `Result`：输入是单字节，非 ASCII 字节只按“扩展标识符字节”处理。`isInCorrectIdentifierName` 也只覆盖空名称和尾随 ASCII 空格，不是完整 identifier 校验器。

查询未命中统一返回 `0`，交由 lexer 保留 `identifier`；这不是错误。规则树根 token 为 `invalid`，没有匹配规则时 `Scanner::scan` 可把 `invalid` 交给上层，真正的解析错误由 lexer/parser 的既有错误路径产生。复合运算符需要保持前缀节点可用；更改注册顺序或把函数错误挂到深层路径可能改变最长匹配和委托时机。

`isTokenIdentifier` 的限定名回看只跳过字节值为空格的字符，与 `IGNORE_SPACE` 分支调用的通用 whitespace 跳过范围并不相同；扩展时应保留 Go 的这一精确语义。表查询本身区分大小写，直接调用 `isInTokenMap("select")` 会返回 `false`，只有 scanner 路径负责规范化。

## 并发与资源生命周期

`ruleTable` 由 `LazyLock` 保证跨线程只初始化一次；发布后没有可变操作。其 `Box<trieNode>` 子节点随进程级静态表存活，不存在手动释放、锁持有、后台任务、通道、事务或 I/O 生命周期。

各 `Scanner` 独占 reader 和缓冲区，`isTokenIdentifier` 需要 `&mut self`，不会在调用间共享规范化缓冲。静态 token 切片只读，因此并发解析时无需额外同步。性能边界是：trie 扫描按已消费前缀长度进行，而各 token 表通过 `lookup_token` 线性搜索；这与 Go 的哈希 map 实现结构不同，扩充大表时应关注热路径成本，但不能在缺少基准和语义验证时擅自改成另一数据结构。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/parser/misc.go`](misc.go)。Rust 保留了 Go 的字符规则、运算符/扫描函数注册内容、关键字与 Hint 表内容，以及 `isTokenIdentifier` 的限定名、ASCII 大写化、`IGNORE_SPACE` 和窗口函数开关流程。

实现形态有四项明确差异：

- Go 用两个 `init` 修改包级 `ruleTable`，Rust 用 `buildRuleTable` 构造局部值并通过 `LazyLock` 一次发布，避免可变全局初始化。
- Go 的 trie 子节点是 `[256]*trieNode`，Rust 是 `Vec<Option<Box<trieNode>>>`；二者都以输入字节索引 256 个槽位。
- Go 的关键字表是 map/set，Rust 为有序静态切片并由 `lookup_token`/`contains` 查询；Rust 的 `aliases` 是显式二元组表，`hintedTokens` 是 token 切片。语义目标相同，但 Rust 查询复杂度为线性。
- Go 用预计算 `[256]bool` 实现 `isUserVarChar`，Rust 直接复用谓词计算；允许字符集合一致。

这些差异属于移植实现选择，不表示 Rust 可以缩减 Go 表项。表项或分支变化应先与同路径 Go 文件和 grammar 事实对照。

## 扩展指南

新增普通关键字时，修改 `tokenMap` 之前先确认生成 token 与 [`pkg/parser/grammar/main.astergram`](grammar/main.astergram) 的分类，并同步关键字生成物/一致性测试；别名还要更新 `aliases`，且别名与规范名必须映射到同一 token。新增窗口函数放入 `windowFuncTokenMap`，并验证开关关闭时仍作为 identifier。新增内建函数专用 token 放入 `btFuncTokenMap`，重点覆盖紧邻 `(`、普通空格以及 `ModeIgnoreSpace` 三种输入。

新增运算符或特殊前缀时，修改 `buildRuleTable` 中恰当的单字节、字符串或函数注册段；确认共享前缀的最短和最长形式都正确，且扫描函数应挂在每个首字节还是多级路径。新增 Hint 时同步 `hintTokenMap`；若要允许新语句承载 Hint，还需评估 `hintedTokens` 和 `lexer.rs` 的 Hint 启动逻辑。

测试逻辑必须继续放在独立文件。最直接的回归位置是 [`pkg/parser/misc_2_aster_unit_test.rs`](misc_2_aster_unit_test.rs)；表与 grammar 关系使用 [`pkg/parser/consistent_test.rs`](consistent_test.rs)，保留字例外使用 [`pkg/parser/reserved_words_test.rs`](reserved_words_test.rs)，公开 Parser/摘要路径的词法行为使用 [`pkg/parser/lexer_test.rs`](lexer_test.rs)。兼容风险主要是 token 编号或分类变化导致语法含义改变；性能风险主要是线性表继续增长；限定名和 SQL mode 分支必须与 Go 保持一致。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/parser/misc.rs` 找到本文件及 23 个符号；精确查询找到 Rust `buildRuleTable`、`isInTokenMap`、`ruleTable`、`hintTokenMap`，文件节点列出 `lexer.rs` 与 `hintparserimpl.rs` 等使用者；`node --file pkg/parser/misc.rs --offset 1180 --limit 80` 核验 `Scanner::isTokenIdentifier` 实现。泛化 `explore` 结果存在同名符号噪声，未把无关模块结果当作本文件调用证据。
- Rust 源与装配：[`pkg/parser/misc.rs`](misc.rs)、[`pkg/parser/lib.rs`](lib.rs)、[`pkg/parser/lexer.rs`](lexer.rs)、[`pkg/parser/hintparserimpl.rs`](hintparserimpl.rs)、[`pkg/parser/Cargo.toml`](Cargo.toml)。目标目录没有 `doc.go`，包入口和 include 边界以 `lib.rs` 为准。
- Go 对照：[`pkg/parser/misc.go`](misc.go)；调用路径还与同目录 `lexer.go` 的对应实现一致。
- 独立测试：[`pkg/parser/misc_2_aster_unit_test.rs`](misc_2_aster_unit_test.rs) 覆盖字符边界、反向命名、trie 最长匹配、四类映射、限定名、`IGNORE_SPACE` 与窗口函数开关；[`pkg/parser/consistent_test.rs`](consistent_test.rs)、[`pkg/parser/reserved_words_test.rs`](reserved_words_test.rs)、[`pkg/parser/lexer_test.rs`](lexer_test.rs) 分别约束 grammar/别名一致性、窗口关键字例外和公开解析路径。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证恰好存在十一个固定二级章节，并人工复核文档只描述已由上述源码、图查询和测试支持的当前行为。
