# [`pkg/expression/fts_to_like.rs`](./fts_to_like.rs)

## 文件定位

本文件属于 `astersql-expression` crate（`pkg/expression/Cargo.toml` 的 `[package] name = "astersql-expression"`），由 `pkg/expression/lib.rs` 通过 `#[path = "fts_to_like.rs"] mod fts_to_like_kernel;` 作为私有内核装入。它承担两项相关但层次不同的工作：一是把 `MATCH ... AGAINST` 的受限语义降级成由 `ILIKE`、`IFNULL`、`AND`、`OR`、`NOT` 组成的轻量表达式树；二是提供 `Datum`、`FieldType`、`Column`、`ScalarFunction`、`Expression` 等轻量表达式模型，供同一移植批次的 `infer_pushdown.rs` 等内核复用。

当前 Rust 仓库中，精确搜索 `build_fts_to_ilike_expression*` 和 `validate_fts_search_string_for_like_fallback` 只找到本文件内部调用以及 `pkg/expression/fts_to_like_test.rs`、`pkg/expression/fts_to_like_36_aster_unit_test.rs` 两个测试模块；没有发现 Rust planner/cardinality 生产调用者。`pkg/expression/lib.rs` 只在测试辅助模块 `expression_files_36` 中再导出整个内核，公共的 `infer_pushdown` 模块仅选择性再导出轻量模型类型。因此，本文件已经实现并验证了 Go 语义内核，但不能据现有证据称其已经接入 Rust SQL 规划主链。

## 核心职责

- `validate_fts_search_string_for_like_fallback` 定义安全降级的严格词法边界：自然语言模式只接受由 ASCII 字母数字或非 ASCII UTF-8 字节组成的空白分隔词；布尔模式额外允许每个词开头的单个 `+` 或 `-`。短语、通配符、相关度修饰符、括号以及 `_`、`%`、词中连字符等均拒绝，避免把 MySQL FTS 分词语义错误近似为子串匹配。
- `build_fts_to_ilike_expression` 按 `FulltextSearchModifier` 分派自然语言或布尔模式，生成整数型谓词树；无列、查询扩展和不支持的搜索串会返回错误，空搜索串返回常量 `0`。
- `build_fts_to_ilike_expression_from_builtin` 校验并拆解轻量 `fts_mysql_match_against` 标量函数，供“从已有 builtin 替换”的场景使用。该入口刻意只接受单列、字符串常量搜索词，并保留搜索常量为 `NULL` 时的 SQL 三值逻辑。
- `Expression::eval_bool` 与私有 `eval` 为所生成的有限函数集合提供可执行语义，既用于证明树形组合的行为，也明确了这个轻量模型并非完整 SQL 表达式执行器。
- `escape_fts_like_pattern` 和 `ilike_predicate` 保证 `%`、`_`、反斜杠按字面量进入 `%term%` 模式，并用 `IFNULL(..., 0)` 把空列视作“不包含该词”。

## 主要符号

- `FulltextSearchModifier`：四种模式枚举；`is_boolean_mode`、`is_natural_language_mode`、`with_query_expansion` 分别完成分类。带查询扩展的两种变体能被分类，但构建入口会明确拒绝。
- `Datum`、`FieldKind`、`FieldType`：常量和值类型元数据。`FieldType::integer`、`varchar`、`enum_type`、`decimal` 是便捷构造器，`is_decimal_valid` 校验 `DECIMAL` 的精度和小数位边界。
- `Column`：用 `unique_id` 标识优化器列，用 `index` 从测试行中取值，并记录类型与 `encodable` 标记。
- `CastFamily`、`Signature`：表达标量函数签名；`Signature::name` 为普通、CAST 和不支持签名提供稳定名称。
- `ScalarFunction`：保存小写函数名、签名、参数、返回类型、可选 FTS 修饰符和元数据可序列化标记。`ScalarFunction::fts` 固定生成名为 `fts_mysql_match_against`、签名为 `FTSMysqlMatchAgainst`、返回类型为 `Real` 的节点，并把搜索表达式放在参数 0。
- `Expression`：轻量 AST，支持 `Constant`、`Column`、`ScalarFunction` 和显式 `Unsupported`。公开方法还包括类型推导、列 ID 收集、规范键、显示和布尔求值。
- `FtsError`：区分 `NotSupported`、`InvalidBuiltin` 和 `Evaluation`，让调用方能分辨能力边界、输入形态错误和执行错误。
- `FtsSearchTerm`：保存词体以及 required/excluded 两个互斥意图；`required`、`excluded`、`optional` 构造对应 `+word`、`-word` 和裸词。
- 公开转换入口：`parse_fts_boolean_search_string`、`parse_fts_search_term`、`escape_fts_like_pattern`、`validate_fts_search_string_for_like_fallback`、`build_fts_to_ilike_expression`、`build_fts_to_ilike_expression_from_builtin`。
- 私有组合器：`build_fts_boolean_mode`、`build_fts_natural_language_mode`、`ilike_predicate`、`compose`、`zero`、`ilike`。

文件顶部还以 `#[path]` 声明 `function_traits`、`grouping_sets`、`helper`、`infer_pushdown` 四个公开子模块；crate 根同时另行装入这些文件作为内核模块。它们属于 task 700 的聚合布局，不参与 FTS 转换算法本身，扩展本文件时不应误把这些声明当作转换调用链。

## 执行流程

1. 直接入口 `build_fts_to_ilike_expression(columns, search_text, modifier)` 先拒绝空列集合和 `WITH QUERY EXPANSION`，再调用 `validate_fts_search_string_for_like_fallback`。这里先校验、后解析是不变量；布尔解析器自身允许空词体，但生产构建路径不会让裸 `+`/`-` 通过。
2. 空字符串精确值直接变成 `zero()`。仅含空白的字符串会通过校验，随后在模式构建器按无词处理并同样得到 `0`。
3. 布尔模式由 `parse_fts_boolean_search_string` 按空白拆词，再分为 required、excluded、optional：
   - 每个 required 词在所有列上构造 `OR(IFNULL(col ILIKE pattern, 0), ...)`，各 required 条件最终参与外层 `AND`；
   - 每个 excluded 词构造同样的列间 `OR`，再包一层 `NOT`；
   - 有 required 时忽略 optional，因为子串回退不实现 FTS 排名；没有 required 时，把所有 optional×列谓词合成一个大 `OR`，并在有 excluded 时与排除条件 `AND`；
   - 只有 excluded 的查询按 MySQL 布尔模式返回恒假 `0`。
4. 自然语言模式把所有空白分隔词与所有列组合成 ILIKE 谓词，先按列 `OR`、再跨列 `OR`；代数上等价于“任一列包含任一词”。
5. `ilike_predicate` 先经 `escape_fts_like_pattern` 转义，再生成 `%escaped-term%`、反斜杠转义码 `92` 和 `ilike(column, pattern, escape)`，最后包成 `ifnull(ilike, 0)`。
6. builtin 入口 `build_fts_to_ilike_expression_from_builtin` 依次检查函数名、`FTSMysqlMatchAgainst` 签名、至少两个参数、至多一个列参数、参数 0 为字符串常量，以及存在 FTS modifier；成功后把列切片和搜索串交回直接入口。`NULL` 搜索常量在分派前原样返回 `Constant(Null)`。
7. 测试求值由 `Expression::eval_bool` 进入私有 `eval`。`and`/`or` 实现 SQL 三值逻辑与短路；`not` 保留 `NULL`；`ilike` 只实现本模块生成的“两端 `%` + 反斜杠转义”模式，并用 Unicode 小写后的 `contains` 做近似大小写不敏感子串匹配。

## 数据与状态

所有转换函数都是纯函数：输入是借用的列切片、搜索字符串和复制型 modifier，输出是新建的 `Expression` 或 `FtsError`。表达式节点通过拥有的 `String`、`Vec<Expression>` 和克隆的列节点组成树，不共享可变全局状态。

关键数据约定包括：`ScalarFunction::fts` 的参数 0 永远是 AGAINST 搜索表达式，后续参数是 MATCH 列；`Column.index` 是求值时的行下标而 `unique_id` 用于标识与规范键；谓词返回 `FieldType::integer()`，真/假分别表示为 `Datum::Int(1/0)`；`modifier` 只对 FTS builtin 有意义；`metadata_serializable` 和 `encodable` 在本文件内只保存、不参与转换判定。

`compose` 对单元素直接返回原表达式，避免无意义的单参数 AND/OR；对零元素则仍能构造空参数组合节点，但现有构建流程通过空词、空列和 `predicates.is_empty()` 分支阻止这种形态进入正常 FTS 结果。`canonical_key` 只编码函数名及参数、不编码返回类型、签名或 modifier，因此它适合作为当前轻量模型的规范描述，不能未经核验提升为完整 SQL 语义身份键。

## 依赖与调用关系

下游依赖很小：标准库 `std::fmt` 用于 `Display`，`thiserror::Error`（由 `pkg/expression/Cargo.toml` 声明为版本 2）派生错误显示；其余转换、AST 和求值逻辑均在本文件内完成。`infer_pushdown.rs` 从 `crate::fts_to_like_kernel` 引用 `CastFamily`、`Expression`、`FieldKind`、`ScalarFunction`、`Signature`，说明轻量模型还作为下推判定的数据边界使用。

RustCodeGraph 对目标文件的索引读取成功，但对三个关键入口执行 `callers`/`callees` 没有返回静态边。随后全仓精确搜索确认 Rust 侧的直接使用者为：

- `pkg/expression/fts_to_like_test.rs`：专项边界和 builtin 形态测试；
- `pkg/expression/fts_to_like_36_aster_unit_test.rs`：聚合测试中的表达式树求值、NULL、多列和查询扩展验证；
- 本文件内部：builtin 入口调用直接构建入口，直接入口调用 validator 和两个模式构建器。

Go 侧生产链则已经接线：`pkg/planner/core/expression_rewriter.go` 调用 validator，`pkg/planner/core/fulltext_to_like.go` 调用直接构建入口，`pkg/planner/cardinality/selectivity.go` 调用 builtin 替换入口。Rust 当前没有与这三条 Go 调用边相对应的生产调用证据，因此安全扩展时应先判断是在完善内核，还是另行承担 planner/cardinality 接线任务。

## 错误处理与边界

`NotSupported` 表示可识别但不能安全近似的能力边界，包括无列、查询扩展、非法词法、builtin 多列替换、非字符串或非常量搜索词；`InvalidBuiltin` 表示函数名、签名、参数数目或 modifier 缺失等结构不变量被破坏；`Evaluation` 表示轻量求值器遇到字符串谓词、错误 ILIKE 参数、显式 Unsupported 节点或未知函数。

词法校验按 UTF-8 字节处理：每个非 ASCII 字节都视作词字符，所以中文测试可通过，但这不是完整 Unicode 字母分类或 MySQL 分词器实现。`escape_fts_like_pattern` 能处理 `\`、`%`、`_`，然而严格 validator 正常会提前拒绝包含这些符号的搜索词；该公开函数仍有独立测试，以保证未来其他入口使用时不会产生通配符注入。

回退本质是近似：`%term%` 是子串匹配，不能复现词边界、相关度、短语、前缀通配、查询扩展或完整 MySQL FTS 分词。代码通过限制输入来缩小差异，而非声称等价覆盖所有 MATCH 语法。简化 `ilike` 也只为本文件生成的模式服务，不应被复用为通用 SQL ILIKE 实现。

builtin 替换只支持单列，这是选择率替换场景的约束，不是直接构建器的限制。NULL 搜索串返回 NULL 而非 0，确保未来在 `NOT` 等组合下不把未知错误变成真；NULL 列则在单词谓词内部经 `IFNULL(..., 0)` 变成不命中，避免 excluded 词上的 `NOT(NULL)` 把本应保留的行过滤掉。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄、网络连接或事务。所有状态都局限于当前调用栈和返回的拥有型表达式树；借用的 `columns`、`search_text` 与测试 `row` 只在调用期间读取。错误通过 `Result` 同步返回，没有重试、清理回调或延迟资源释放。

主要资源成本来自树构建和字符串分配：布尔/自然语言转换最多生成与“列数 × 词数”同阶的 ILIKE/IFNULL 节点；每个单词模式会分配转义串和 `%...%` 字符串；轻量 `eval` 对每个 ILIKE 调用执行双方 `to_lowercase()`。因此增加更宽松语法或更多组合层次时，需要关注表达式节点膨胀和重复大小写转换，而不是并发安全问题。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/fts_to_like.go`，测试对照为 `pkg/expression/fts_to_like_test.go`。Rust 的以下流程与 Go 保持一致：严格词法集合；布尔词的 required/excluded/optional 分类；只有排除词时恒假；有 required 时忽略 optional；自然语言的任意词/任意列 OR；ILIKE 模式转义和 `IFNULL(..., 0)`；拒绝查询扩展；builtin 替换的单列限制；NULL 搜索常量直通；非字符串/非常量拒绝。

实现载体存在明确差异：Go 使用真实的 `expression.Expression`、`BuildContext`、`NewFunction`、TiDB 类型和具体 `builtinFtsMysqlMatchAgainstSig`；Rust 使用本文件定义的轻量 AST、字符串函数名与 `Signature::Generic`。Go 构造过程中 `NewFunction` 可能传播真实函数构建错误，Rust 的 `ilike_predicate` 和 `compose` 目前不会失败，因此两个私有模式构建器虽返回 `Result`，实际错误主要来自入口验证。

Go 生产版本已经由 planner rewrite 和 cardinality selectivity 调用；Rust 版本目前仅有测试与 `infer_pushdown` 的模型复用证据。Rust 额外提供 `Expression::eval`/`eval_bool` 来直接验证生成树语义，而 Go 依靠真实表达式运行时。Rust 还显式检查 `Signature::Generic("FTSMysqlMatchAgainst")`；这对应 Go 对具体 builtin signature 类型的断言，防止仅函数名相同的伪造节点通过。

## 扩展指南

- 扩展支持的搜索语法时，首先修改 `validate_fts_search_string_for_like_fallback`，再同步解析与构建；不能只放宽 validator，因为 `%term%` 近似可能无法保留新语法的词边界、优先级或排名语义。
- 修改布尔组合规则应集中在 `build_fts_boolean_mode`，并同步 `pkg/expression/fts_to_like_test.rs` 的解析/builtin 边界和 `pkg/expression/fts_to_like_36_aster_unit_test.rs` 的行求值断言。测试必须继续放在独立测试文件，不能内嵌到生产 `.rs`。
- 修改模式或 NULL 行为应集中在 `escape_fts_like_pattern`、`ilike_predicate` 以及必要时轻量 `ilike`/`eval`；应覆盖反斜杠、大小写、多字节文本、NULL 列、excluded 词和 SQL 三值逻辑。
- 若要把该内核接入 Rust 生产 planner/cardinality，不能仅把私有模块改成公开；需要用真实 Rust 表达式/上下文核对 Go 的三条调用链，并明确轻量 `Expression` 与生产 AST 的转换边界。此工作超出本文件文档任务的已验证范围。
- 新增 modifier 时须同时审查三个分类方法、顶层分派、builtin modifier 携带方式以及 `infer_pushdown.rs` 的 TiFlash 规则，避免某个模式被错误下推或静默落入不可达分支。
- 性能上应控制 O(列数×词数) 节点数；若引入去重或缓存，要注意 `canonical_key` 当前不包含类型、签名和 modifier，不能直接假设它足以表达完整等价性。
- 与 Go 继续对齐时，同步检查 `pkg/expression/fts_to_like.go` 和 `pkg/expression/fts_to_like_test.go`，尤其关注真实 `NewFunction` 错误、类型/校对规则以及 planner 调用约束，避免用轻量求值通过代替生产可用性证据。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 11,467 个文件；`node --file pkg/expression/fts_to_like.rs --offset ...` 分段读取了目标文件全部 811 行；对 `build_fts_to_ilike_expression`、`build_fts_to_ilike_expression_from_builtin`、`validate_fts_search_string_for_like_fallback` 执行了 `query`，均定位到本文件；对应 `callers`/`callees` 无静态结果，随后以精确仓库搜索补证。
- 源码与 crate 边界：`pkg/expression/fts_to_like.rs`、`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`；目标包没有 `doc.go` 或 `doc.rs`。
- Rust 独立测试：`pkg/expression/fts_to_like_test.rs` 覆盖合法/非法词法矩阵、布尔解析、LIKE 转义、builtin 结构守卫和 TiFlash modifier；`pkg/expression/fts_to_like_36_aster_unit_test.rs` 覆盖两种模式的实际树求值、NULL、多列、查询扩展及下推规则。
- Go 对照：`pkg/expression/fts_to_like.go`、`pkg/expression/fts_to_like_test.go`；生产调用边由 `pkg/planner/core/expression_rewriter.go`、`pkg/planner/core/fulltext_to_like.go`、`pkg/planner/cardinality/selectivity.go` 的精确符号引用确认。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付验证使用任务文件指定的 11 章节结构命令，并人工复核文档明确回答文件用途、执行路径、能力边界、当前接线状态和安全扩展位置。
