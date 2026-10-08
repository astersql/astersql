# `pkg/util/table-filter/matchers.rs`

## 文件定位

本文件是 `astersql-util-table-filter` crate 的底层名称匹配层，源码见 [`matchers.rs`](matchers.rs)。crate 入口 [`lib.rs`](lib.rs) 将本模块公开并再导出其符号；[`Cargo.toml`](Cargo.toml) 表明该 crate 对应 Go 包 `pkg/util/table-filter`，直接依赖 `regex 1.11` 与 `regex-syntax 0.8`。

它不负责解析完整规则列表或决定规则优先级，而是提供“一个模式如何匹配一个名称”的统一接口，以及承载表规则、列规则所需的数据结构。上层 [`parser.rs`](parser.rs) 将字面量、通配符和 `/regex/` 构造成这里的 matcher；[`table_filter.rs`](table_filter.rs) 与 [`column_filter.rs`](column_filter.rs) 再组合 matcher 完成 schema、table、column 的过滤判断。

## 核心职责

- 用 `matcher` trait 统一精确字符串、恒真通配和正则三种匹配策略。
- 用 `tableRule`、`columnRule` 把 matcher 与 `positive` 接受/拒绝标志组合成上层规则的最小载体。
- 将特殊正则 `(?s)^.*$` 优化为 `trueMatcher`，使上层可通过 `matchAllStrings()` 识别真正的“匹配任意名称”。
- 为大小写不敏感包装生成规则侧变体：字符串按 Go `strings.ToLower` 的单 rune 简单映射处理，正则通过前置 `(?i)` 启用不区分大小写。
- 把 Rust 正则解析错误转换成尽量兼容 Go `regexp/syntax` 的 `FilterError` 文本，供解析器附加文件名和行号。

## 主要符号

- `FilterError(pub String)`：公开错误包装，实现 `Display` 与 `std::error::Error`。本层只保存稳定的用户可见文本，不保留结构化 cause。
- `tableRule { schema, table, positive }`：公开结构体，分别保存 schema matcher、table matcher 和接受/拒绝标志；字段均公开给同 crate 的解析、兼容和过滤实现组装。
- `columnRule { column, positive }`：列规则载体，语义与 `tableRule` 相同但只有一个名称 matcher。
- `matcher: Debug + Send + Sync`：公开 trait，定义 `matchString(&str)`、`matchAllStrings()` 与 `toLower()`。返回 `Box<dyn matcher>` 让不同实现可由规则统一持有；`Send + Sync` 允许包含它的过滤器跨线程共享。
- `stringMatcher(String)`：公开元组结构体，执行完全相等比较；`matchAllStrings()` 固定为 `false`。
- `trueMatcher`：公开零大小类型，任何输入均匹配且 `matchAllStrings()` 为 `true`；`Clone + Copy + Default` 便于无状态复用。
- `regexpMatcher { pattern: regex::Regex }`：私有字段保存已编译正则；匹配调用 `Regex::is_match`，但即使表达式事实上覆盖所有字符串，`matchAllStrings()` 仍为 `false`，只有规范化的特殊模式会被提升为 `trueMatcher`。
- `newRegexpMatcher(&str) -> Result<Box<dyn matcher>, FilterError>`：公开构造入口，负责通配特例优化、正则编译和错误兼容转换。
- `goToLower(&str) -> String`、`regexp_error(&str, regex::Error) -> FilterError`：私有兼容辅助函数，分别对齐 Go 大小写映射与常见正则诊断分类。

## 执行流程

1. [`parser.rs`](parser.rs) 的 `matcherParser::parsePattern` 判断输入是 `/regex/`、引号字面量还是通配模式。字面量直接构造 `stringMatcher`；通配模式被转换为以 `(?s)^` 开头、以 `$` 结尾的正则，再经 `matcherParser::regexp_matcher` 调用 `newRegexpMatcher`。
2. `newRegexpMatcher` 首先检查模式是否精确等于 `(?s)^.*$`。这是 `*` 生成的全匹配形式，函数直接返回 `trueMatcher`；否则调用 `regex::Regex::new` 生成 `regexpMatcher`。
3. 正则编译失败时，`regexp_error` 再用 `regex_syntax::ast::Parser` 获取结构化 `ErrorKind` 与 span，把未闭合字符类/分组、非法 look-around、非法范围、转义、重复次数等常见情况映射为 Go 风格消息；未专门覆盖的错误回退到 `regex` 原始诊断。
4. `Parse` 或 `ParseColumnFilterRules` 把 matcher 放入 `tableRule`/`columnRule`。规则列表的反转和“后写优先”由上层完成，不属于本文件。
5. 匹配时，`tableFilter::MatchTable` 对 schema 与 table 分别调用 `matchString`；`tableFilter::MatchSchema` 还调用 table matcher 的 `matchAllStrings`，以判断一条否定规则能否否决整个 schema；`ColumnFilterRules::match_rule` 对小写后的列名调用列 matcher。
6. `CaseInsensitive` 经 `Filter::toLower` 逐条调用 matcher 的 `toLower`。字符串 matcher 生成 Go 式小写副本，恒真 matcher 复制自身，正则 matcher 重新编译 `(?i)` 加原表达式。

## 数据与状态

matcher 创建后按只读值使用：`stringMatcher` 持有一个 `String`，`regexpMatcher` 持有已编译的 `regex::Regex`，`trueMatcher` 没有字段。规则通过 `Box<dyn matcher>` 独占 matcher 对象；`toLower` 返回新对象，不原地改变旧规则。

`positive` 仅记录命中后的接受或拒绝含义，本文件不解释规则顺序。上层 `tableFilter`/`ColumnFilterRules` 按顺序寻找首条命中，因此解析阶段反转规则列表后形成“后写规则优先”的整体语义。

`matchAllStrings` 不是对正则做一般性等价分析，而是一个保守标志：仅 `trueMatcher` 返回 `true`。这个不变量对 `MatchSchema` 很重要，因为具体表级否定规则不应误判为整个 schema 被拒绝。

## 依赖与调用关系

上游构造者包括：

- `matcherParser::parsePattern` / `parseWildcardPattern`（[`parser.rs`](parser.rs)）：构造 `stringMatcher`，或经 `newRegexpMatcher` 构造正则/恒真 matcher。
- `matcherFromLegacyPattern`（[`compat.rs`](compat.rs)）：把旧 MySQL replication 的 `~regex`、glob 或字面量转换为同一 matcher 抽象，并直接使用 `trueMatcher` 构建默认规则。
- `columnRulesParser::parse`（[`parser.rs`](parser.rs)）：在列规则入库前调用 `toLower`。

下游消费者包括：

- `tableFilter::MatchTable`：同时要求 schema 与 table 的 `matchString` 命中。
- `tableFilter::MatchSchema`：使用 `matchString` 和 `matchAllStrings` 区分 schema 级与具体 table 级规则。
- `tableFilter::toLower`：为两个 matcher 生成大小写不敏感变体。
- `ColumnFilterRules::match_rule`：对列名调用 `matchString`。

crate 外部通过 `lib.rs` 再导出的 `Filter`、`Parse`、列过滤等高层 API 使用这些实现；仓库 Cargo 清单显示 `dumpling/export`、`br/pkg/utils`、`pkg/util/filter`、`pkg/importsdk`、`pkg/executor` 等 crate 依赖本 crate，但它们通常不直接操作私有的匹配实现。

## 错误处理与边界

- `newRegexpMatcher` 是本文件唯一返回 `Result` 的入口。成功后运行期匹配不再产生错误；编译错误包装为 `FilterError`。
- `regexp_error` 优先依赖 `regex-syntax` AST 的 span，而不是从渲染后的错误字符串猜偏移。映射覆盖 `ClassUnclosed`、`GroupUnclosed`、`GroupUnopened`、`UnsupportedLookAround`、`ClassRangeInvalid`、转义错误和重复错误；其他类别使用 `error parsing regexp: {error}` 回退。
- Rust `regex` 不支持 look-around/backreference。兼容层把 `(?=...)`/`(?!...)` 报为“不支持的 Perl 语法”，把 `(?<=...)`/`(?<!...)` 按 Go 诊断归类为非法命名捕获；这只是错误消息兼容，不表示这些语法可执行。
- `regexpMatcher::toLower` 使用 `expect`，其不变量是给已经成功编译的表达式前置合法的 `(?i)` 不会使表达式失效；若未来改变拼接方式，需要把该不变量纳入回归测试。
- `goToLower` 对每个 Unicode scalar 仅取 `to_lowercase()` 的第一个结果，以模拟 Go 的简单大小写映射，故不会采用 Rust 完整映射可能产生的多字符展开。
- 空规则、错误分隔符、非法通配转义和文件导入错误由 [`parser.rs`](parser.rs) 处理，不由本文件处理。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或 I/O 资源。`matcher` 强制 `Send + Sync`，其三个实现都只保存可安全共享的不可变数据；匹配方法只借用 `&self`，不会改变状态。

已编译正则随 `regexpMatcher` 生命周期存在，避免每次匹配重新编译；只有构造和 `toLower` 时编译。matcher 由 `Box` 随所属规则释放，未使用全局缓存。上层 `loweredFilter` 使用 `Arc<dyn Filter>` 共享整个过滤器，但该资源管理发生在 [`table_filter.rs`](table_filter.rs)，本文件只通过 trait 边界满足其线程安全要求。

## 与 Go 版本的对应关系

直接对照文件是 [`matchers.go`](matchers.go)。Rust 保留了 Go 的 `tableRule`、`columnRule`、`matcher`、`stringMatcher`、`trueMatcher`、`regexpMatcher` 和 `newRegexpMatcher` 分工：精确比较、`*` 特例、正则匹配、`matchAllStrings` 与 `toLower` 的总体语义一致。

主要实现差异如下：

- Go 使用 interface 值，Rust 使用 `Box<dyn matcher>`，并额外要求 `Debug + Send + Sync`。
- Go `regexp.Compile` 直接返回 Go 风格错误；Rust 用 `regex` 执行匹配，并由 `regex-syntax` 的 `ErrorKind`/span 重建兼容诊断。
- Go `strings.ToLower` 是简单 rune 映射。Rust 标准 `char::to_lowercase` 可能产生多个字符，因此 `goToLower` 明确只取首字符；[`matchers_test.rs`](matchers_test.rs) 用 `U+0130` 验证结果是 `i` 而不是 `i` 加组合点。
- 两端都将唯一规范模式 `(?s)^.*$` 优化为恒真 matcher；不会把任意语义等价的正则自动判为 `matchAllStrings`。
- Go 用 `regexp.MustCompile` 构造大小写不敏感正则；Rust 用 `Regex::new(...).expect(...)` 表达同一“原正则有效且加 flag 后仍有效”的不变量。

Go 的 [`table_filter_test.go`](table_filter_test.go)、[`column_filter_test.go`](column_filter_test.go) 和 [`compat_test.go`](compat_test.go) 提供集成语义基线；Rust 的独立 [`matchers_test.rs`](matchers_test.rs) 聚焦 Rust/Go 差异最大的 Unicode 小写与正则错误文本，表级组合行为另由 [`table_filter_test.rs`](table_filter_test.rs) 覆盖。

## 扩展指南

- 新增 matcher 实现时，实现三个 trait 方法并保持 `Debug + Send + Sync`；尤其要明确 `matchAllStrings` 是否能在所有输入上可靠成立。不要仅凭部分正则形态返回 `true`，否则 `MatchSchema` 会错误扩大否定规则范围。
- 新增模式语法应优先修改 [`parser.rs`](parser.rs) 的 `parsePattern`/`parseWildcardPattern`，只在需要新的运行期匹配策略时扩展本文件；同步新增独立测试文件中的用例，不要把测试嵌入生产源码。
- 调整大小写逻辑时，同时验证 `stringMatcher::toLower`、`regexpMatcher::toLower`、`table_filter.rs` 的输入侧 `go_lowercase` 与 `column_filter.rs` 的输入侧转换，避免规则侧和输入侧语义分叉。至少保留 `U+0130` 这类多字符映射回归。
- 扩展 `regexp_error` 映射时，应依据 `regex_syntax::ast::ErrorKind` 和 span 编写表驱动用例，并与 Go `regexp.Compile` 的实际消息核对。风险在于 `regex`/`regex-syntax` 版本升级改变错误分类或 span；未知类别应继续安全回退而不是伪造兼容消息。
- 若改变 `newRegexpMatcher` 的 `*` 优化，需同步验证 `tableFilter::MatchSchema` 的 schema 级否定行为，以及 Go/Rust 表过滤测试中的 `foo.*`、`!foo.*`、具体表否定等用例。
- 性能敏感点是正则编译与匹配。保持“构造时编译、匹配时复用”，避免在 `matchString` 内编译或分配；新增全局缓存则会引入当前不存在的并发和生命周期复杂度。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/table-filter` 确认目标及相邻实现/测试已索引。
- RustCodeGraph `node --file pkg/util/table-filter/matchers.rs`：核对了本文件 162 行源码及全部类型、trait、函数和实现；`query` 确认 `matcher::{matchString, matchAllStrings, toLower}` 与 `newRegexpMatcher` 的精确符号。
- RustCodeGraph `explore`：得到关键调用边：`tableFilter::{MatchSchema, MatchTable}` 和 `ColumnFilterRules::match_rule` 调用 `matchString`，`MatchSchema` 调用 `matchAllStrings`，`toLower` 由过滤器/解析路径使用；精确 `callers` 查询在本地索引上 30 秒内未返回，已停止，未据此添加额外结论。
- 已阅读生产证据：[`matchers.rs`](matchers.rs)、[`parser.rs`](parser.rs)、[`table_filter.rs`](table_filter.rs)、[`column_filter.rs`](column_filter.rs)、[`compat.rs`](compat.rs)、[`lib.rs`](lib.rs) 与 [`Cargo.toml`](Cargo.toml)。
- 已阅读对照与测试证据：[`matchers.go`](matchers.go)、[`matchers_test.rs`](matchers_test.rs)、[`table_filter_test.rs`](table_filter_test.rs)，并核对 Go 的 [`table_filter_test.go`](table_filter_test.go)、[`column_filter_test.go`](column_filter_test.go)、[`compat_test.go`](compat_test.go) 中匹配、大小写、通配与错误行为。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用固定 11 章节结构命令，并人工检查关键结论均能回溯到上述符号、调用边或对照文件。
