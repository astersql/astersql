# `pkg/parser/digester.rs`

## 文件定位

本文件实现 `astersql-parser` crate 的 SQL 规范化与摘要能力：把字面量不同但结构相同的 SQL 归一成稳定文本，并对该文本计算 SHA-256。它不是完整 SQL parser，也不构造 AST；核心路径是文件内的轻量 `Scanner` 加 `SqlDigester` 状态机。模块入口位于 [`lib.rs`](./lib.rs)：`digester_impl` 通过 `include!("digester.rs")` 纳入实现并公开再导出，因此调用方既可从 crate 根使用 `astersql_parser::NormalizeDigest`，也可经 `digester_impl` 使用这些 API。

该能力处在“收到或还原 SQL 文本”与“按语句形状聚合、匹配或观测”之间。RustCodeGraph 的调用边显示，实际生产调用者包括 `pkg/sessionctx/stmtctx/stmtctx.rs`（缓存原始 SQL 的 normalized/digest）、`pkg/session/runtime/planning.rs` 与 `control.rs`（会话规划和观测）、`pkg/session/runtime/dispatch.rs` 及 `pkg/bindinfo/binding.rs`（binding 规范化）、`pkg/expression/builtin_info.rs`（SQL digest 内建功能）和 `pkg/util/topsql/collector/mock/mock.rs`（TopSQL 收集测试支撑）。

## 核心职责

- `Normalize`、`NormalizeKeepHint`、`NormalizeForBinding` 将输入 SQL 变成稳定、空格分隔的规范文本；`OFF`/空 redact 原样返回，`ON` 把一般字面量替换为 `?` 或列表 `...`，`MARKER` 用 `‹...›` 保留可读值（`Normalize`、`SqlDigester::normalize`、`reduce_lit`）。
- `NormalizeDigest` 与 `NormalizeDigestForBinding` 在同一路径上同时生成规范文本和 SHA-256；`DigestNormalized` 只哈希调用者承诺已规范化的字符串，`DigestHash` 是“先规范化再哈希”的兼容 API（同名公开函数及 `do_*` 方法）。
- 词法层识别字符串、数字、参数标记、用户/系统变量、引号标识符、注释和运算符，并使用 `keywords::Keywords` 与内置函数表决定标识符是否按关键字输出（`Scanner::scan`、`Scanner::token_identifier`）。
- 归并层删除普通规范化路径中的 optimizer/index hint，把 `straight_join` 统一为 `join`，折叠连续字面量列表、`VALUES` 行列表及 binding 的 `IN`/`ROW` 形态，同时保留 `ORDER BY`/`GROUP BY` 的整数序号（`reduce_optimizer_hint`、`reduce_lit` 及三个 binding/replayer helper）。
- 展开 MySQL/TiDB 特殊注释 `/*!...*/` 和受支持的 `/*T![auto_rand,clustered_index] ...*/`，普通注释仍交由 scanner 跳过（`expand_special_comments`、`Scanner::reset`）。

## 主要符号

- `Digest { bytes, text }`：拥有摘要字节并缓存小写十六进制文本。`new`/`NewDigest` 构造时编码一次；`bytes`/`Bytes` 返回只读切片，`string`/`String` 返回缓存字符串引用。类型实现 `Clone`、`Eq` 和 `PartialEq`，等价性同时比较两个字段。
- `Normalize(sql, redact)`：通用入口。只有空字符串或精确的 `"OFF"` 走原样快速路径，其余值均进入规范化；常规调用应使用约定的 `ON`/`MARKER`。
- `NormalizeForBinding(sql, for_plan_replayer_reload)`：开启 binding 规则；布尔值为 `false` 时把单元素 `IN (?)`、`IN(ROW(...))` 折叠为 `IN (...)`，为 `true` 时执行相反的 plan-replayer 兼容规则，把 `IN (...)` 改为 `IN (?)`。
- `NormalizeKeepHint(sql)`：使用 `ON` 字面量归并，但让 scanner 返回 `/*+ ... */` token，因而保留 optimizer hint；它并不承诺保留普通注释。
- `NormalizeDigest` / `NormalizeDigestForBinding` / `DigestNormalized` / `DigestHash`：四个摘要入口。前三者明确区分普通、binding 和“输入已经规范化”的契约；`DigestHash` 保留 Go 已废弃 API 的语义。
- `SqlDigester { buffer, lexer, hasher, tokens }`：一次调用所需的全部可变状态，分别承载输出、扫描位置、SHA-256 状态和待渲染 token。
- `Scanner { sql, offset, keep_hint }` 与 `ScanPos`：轻量扫描器及 token 起始字节位置。`scan` 返回 `(kind, pos, literal)`；`token_identifier` 在排除点号限定名后，将关键字或紧邻 `(` 的内置函数提升为 `KEYWORD`。
- `Token { kind, literal }` / `TokenDeque`：归并阶段的 token 及尾部模式匹配容器。`GENERIC_SYMBOL=-1` 与 `GENERIC_SYMBOL_LIST=-2` 是 scanner 不会产生的合成 kind，分别渲染为 `?`、`...`。
- `expand_special_comments`、`is_keyword`、`is_window_function`、`is_builtin_function`、`charset_get_info`：扫描前展开和词类判断辅助函数。当前字符集白名单为 `utf8`、`utf8mb4`、`binary`、`latin1`、`ascii`。

## 执行流程

1. 公开 API 通过 `with_digester` 创建一个独立 `SqlDigester`，选择普通、binding、keep-hint 或直接哈希的 `do_*` 路径。`Normalize` 的 `OFF`/空值例外不会创建状态机。
2. `normalize` 调用 `Scanner::reset`；后者先经 `expand_special_comments` 生成扫描文本，再清零偏移，并按入口配置 `keep_hint`。
3. 主循环反复调用 `Scanner::scan`。scanner 跳过空白、行注释与普通块注释，按优先级识别 hint、引号文本、变量、参数标记、`\N`、数值、标识符、多字符运算符和单字符。非法 token、EOF 或末尾分号会结束循环。
4. token 文本除 hint 外先转为小写；非 keep-hint 路径先运行 `reduce_optimizer_hint`，删除 hint/index 指示并规范化 `straight_join`。
5. `reduce_lit` 处理 MARKER、`count(*)`、数值一元正负号、普通列表/字符集列表/多行列表和 `ORDER BY`/`GROUP BY` 序号。随后 binding 模式调用 `reduce_in_list_with_single_literal`、`reduce_in_row_list_with_single_literal`，plan-replayer reload 模式则调用 `replace_single_literal_with_in_list`。
6. 未被归并的 `IDENTIFIER` 再分类：有效 `_charset` 改为 `UNDERSCORE_CS`；关键字和紧邻左括号的内置函数改为 `KEYWORD`。这个顺序保证未加引号的 `null` 会先按字面量归并。
7. token 入队后，循环结束统一渲染：token 之间通常插入一个空格，用户变量保留 `@` 形态，字符集 introducer 输出 `(_charset)`，普通/引号标识符统一用反引号包裹，其余写入归并后的 literal。残缺块注释会在已有输出末尾补空格；最后清空 token 队列。
8. 摘要路径把规范缓冲或已规范化输入写入 `Sha256`，`finish_hash` 用 `finalize_reset` 取出 32 字节结果并重置哈希器，随后构造带 hex 缓存的 `Digest`。

## 数据与状态

输入 SQL 在 `Scanner::reset` 时复制到 `Scanner.sql`，且特殊注释展开可能产生另一份字符串。规范化输出累积在 `SqlDigester.buffer`；token 按扫描顺序保存在 `TokenDeque.items: Vec<Token>`。`back(n)` 只借用尾部切片，长度不足返回空切片；`pop_back(n)` 长度不足不修改队列，足够时用 `split_off` 返回并移除尾部。

核心不变量是：每次公开调用使用全新的 `SqlDigester`；每条正常 `do_*` 路径在返回前取走或清空 `buffer`、重置 SHA-256（若使用）并由 `normalize` 清空 token 队列。因此前一次 SQL 不应污染下一次调用。`Digest` 自己拥有字节和 hex 文本，没有悬垂借用；但 `DigestNormalized` 不验证输入是否真的经过 Normalize，输入契约由调用者负责。

规范文本保留 SQL 的结构性信息（关键字、标识符、运算符和括号），有意丢失或折叠字面量、注释、部分 index 指示和列表长度。它用于分组/匹配，不应被当作可逆 SQL 或完整语法验证结果。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义为 `astersql-parser`，库入口是 `lib.rs`。本文件直接使用外部依赖 `sha2 = "0.10"` 的 `Sha256`/`Digest` trait；关键字表来自同一 `digester_impl` 内的 [`keywords.rs`](./keywords.rs)。Cargo 还声明 parser 的 AST、charset、mysql、types 等子 crate，但本文件没有直接调用这些 crate；尤其字符集判断当前由本地 `charset_get_info` 白名单实现。

RustCodeGraph 对 `NormalizeDigest` 给出的边是 `NormalizeDigest -> with_digester -> do_normalize_digest -> normalize`；`normalize` 的下游包括 `Scanner::{reset,set_keep_hint,scan,token_identifier}`、`reduce_optimizer_hint`、`reduce_lit`、三个 binding/replayer reducer、`TokenDeque` 操作和渲染逻辑。`NormalizeForBinding`、`DigestNormalized` 以相同门面进入各自 `do_*` 方法。

生产侧的直接文本搜索补充了调用图：`stmtctx.rs` 保存语句摘要，session 的 planning/control/dispatch 生成计划或 binding 所需形状，`bindinfo/binding.rs` 用 binding 专用 normalized/digest 建键，`builtin_info.rs` 暴露 SQL digest，TopSQL mock 使用 digest 聚合 SQL。调用方若已有 normalized SQL（如 binding session handle、scan adapter）才应使用 `DigestNormalized`，否则应使用组合入口，避免把原始字面量直接哈希。

## 错误处理与边界

公开 API 不返回 `Result`。scanner 将未闭合字符串/引号标识符/块注释、非法十六进制或位串等标记为 `INVALID`，`normalize` 看到后停止并返回此前已经形成的规范前缀；因此本模块负责“稳定且不死循环地降级”，不负责报告 SQL 语法错误。独立测试覆盖残缺 `ignore index(`、`select /*+ `、未闭合反引号和特殊注释 EOF，证明这是刻意的边界行为。

`--` 只有后面是空白或已到末尾时才视为注释；`#` 总是行注释。普通块注释被跳过，只有 `keep_hint` 时 `/*+...*/` 被作为 token 保留。`/*!...*/` 去掉开头版本数字后展开；`/*T![...]...*/` 只在 feature 全部属于 `auto_rand`/`clustered_index` 时展开，未知 feature 的内容被丢弃。

数字扫描区分整数、小数、科学计数、`0x` 和 `0b`；与标识符相连的数字会按 identifier 处理。字面量列表只有满足相邻 token 模式时才折叠，混入标识符会保留各项形状。`ORDER BY`/`GROUP BY` 的整数序号有意不改成 `?`，`count(*)` 等括号内星号则归并为 `?`。qualified name 点号两侧不升级为关键字；窗口函数表也有意保留为标识符。

`Normalize` 只对空字符串和精确 `OFF` 执行旁路；未识别的 redact 文本会落入普通扫描，并非显式报错。调用方应只传约定值。摘要是结构指纹而不是安全认证或碰撞证明，且 binding 的 `...` 会进一步主动抹平列表长度差异。

## 并发与资源生命周期

本文件没有锁、异步任务、通道、事务或外部 I/O。`with_digester` 在栈上为每次调用创建独立状态，闭包结束即释放其 `String`、`Vec<Token>` 和 SHA-256 状态；不同线程之间不共享 `Scanner` 或可变缓冲，因此公开函数可自然并发调用。

这与 Go 版本的资源策略不同：Go 使用全局 `sync.Pool` 复用 `sqlDigester`，Rust 明确选择每次新建，避免跨调用保留超大 SQL 缓冲和共享非线程安全 scanner。代价是每次调用可能重新分配；若未来引入池化或线程局部复用，必须保持 `buffer`、`tokens`、scanner offset/keep_hint 和 hasher 在所有提前退出路径上都被完整重置，并补充跨调用状态泄漏及大输入容量策略测试。

## 与 Go 版本的对应关系

直接对照文件是 [`digester.go`](./digester.go)，独立 Go 测试是 [`digester_test.go`](./digester_test.go)。公开 API、`Digest`、`sqlDigester`/`SqlDigester`、token deque、主循环以及 `reduceLit`、`reduceOptimizerHint`、binding/plan-replayer reducer 基本按名称和顺序一一对应；Rust 测试中的固定摘要、等价/非等价 SQL 分组也复现了 Go 测试意图。

主要实现差异如下：Go 复用项目完整 `Scanner`、`charset.GetCharsetInfo` 和 `sync.Pool`，Rust 在本文件内实现轻量 scanner、有限字符集白名单并逐调用创建实例；Go 的 `Digest.Bytes()` 暴露可变 slice 语义，Rust 返回不可变 `&[u8]`；Go 返回 `*Digest`，Rust 按值返回；Go 的 hasher 显式 `Reset`，Rust 用 `finalize_reset`。Rust 还显式处理特殊注释展开和残缺块注释输出空格，以测试锁定当前对齐结果。

“对应”不意味着可脱离测试假设二者永远完全相同。尤其 scanner 词类、关键字/内置函数列表、字符集支持、SQL mode、特殊注释 feature 和非法输入截断策略是最易漂移的区域。新增 Go 行为时应同时核对 Rust scanner 和归并顺序，而不是只添加一个最终字符串特例。

## 扩展指南

- 新增公开规范化模式时，优先在现有 `normalize` 参数和 `do_*` 路径中接线，保持 `Normalize*` 门面薄；同时说明该模式是否保留 hint、使用 binding 归并、接受何种 redact 值，以及摘要是否基于完全相同的输出。
- 新 token 或字面量首先扩展 `Scanner::scan` 的最长匹配与 kind，再决定它是否属于 `is_lit`、是否需在 `token_identifier` 分类，最后补渲染规则。应特别测试 UTF-8、转义、EOF 和与相邻运算符/标识符粘连的情况。
- 新增关键字、窗口函数或内置函数时同步检查 [`keywords.rs`](./keywords.rs)、`is_window_function`、`is_builtin_function`，并覆盖限定名、函数名后是否紧邻 `(` 等上下文，防止标识符被错误加/去反引号。
- 修改列表折叠必须维护“只看队尾、命中后精确 pop”的不变量，并分别覆盖普通 Normalize、binding 与 plan-replayer reload；`IN (?)`、`IN (...)`、`IN(ROW(...))`、多行 `VALUES` 和混合字面量/标识符不能互相误伤。
- 修改摘要编码或 hash 算法会破坏持久化/跨组件键兼容。必须保留 Go 固定 SHA-256 向量并审计所有 `String()` 消费者；不要只验证 `NormalizeDigest == Normalize + DigestNormalized`，因为两条路径同时漂移仍可能互相一致。
- 测试逻辑应继续放在独立 [`digester_test.rs`](./digester_test.rs) 和 [`digester_1_aster_unit_test.rs`](./digester_1_aster_unit_test.rs)，不要嵌回生产文件。前者保持 Go 测试结构，后者适合补 Rust/Go 差异、完整用例表和回归边界。

## 验证依据

- 源码全量读取：[`digester.rs`](./digester.rs)，包括 15 类 scanner token、`Digest` 和 8 个公开构造/规范化/摘要入口、`SqlDigester` 主循环、所有 reducer、`Token`/`TokenDeque` 及合成 kind。
- 模块与依赖：[`lib.rs`](./lib.rs) 的 `digester_impl` include/re-export 和独立测试模块声明；[`Cargo.toml`](./Cargo.toml) 的 `astersql-parser` crate、`lib.rs` 入口、`sha2` 依赖与 `pkg/parser` Go 包移植元数据。本包未发现 `doc.go`，故以这些最近入口作为包级契约证据。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`node pkg/parser/digester.rs::NormalizeDigest`、`NormalizeForBinding`、`DigestNormalized`、`Normalize`、`normalize`、`reduce_lit` 核对了公开入口、内部 callees 和已解析 callers。另用 Rust 源码调用点搜索补足图未解析到的生产调用者。
- Go 对照：[`digester.go`](./digester.go) 的公开 API、pool、状态机和 reducer；[`digester_test.go`](./digester_test.go) 的 Normalize、MARKER、keep-hint、固定 digest、等价/非等价分组及 Digest 构造用例。
- Rust 独立测试：[`digester_test.rs`](./digester_test.rs) 的 Go 结构移植，以及 [`digester_1_aster_unit_test.rs`](./digester_1_aster_unit_test.rs) 的固定 SHA-256、全量 Go normalize 表、数值边界、特殊注释、关键字/窗口函数和状态泄漏回归。按任务约束未运行 Cargo；本任务仅以源码、调用图、对照测试和文档结构检查作验证。
