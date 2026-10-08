# `pkg/util/table-filter/column_filter.rs`

对应源文件：[`column_filter.rs`](./column_filter.rs)。

## 文件定位

该文件属于 `astersql-util-table-filter` crate；crate 根 `pkg/util/table-filter/lib.rs` 以 `pub mod column_filter` 声明模块，并通过 `pub use column_filter::*` 重导出这里的公开 API。`pkg/util/table-filter/Cargo.toml` 将 `lib.rs` 设为库入口，声明的运行时依赖只有 `regex` 与 `regex-syntax`；这两个依赖由下游匹配器实现使用，本文件本身只通过同 crate 的 `columnRule`、`columnRulesParser`、`matcherParser` 和 `FilterError` 间接使用它们。

在完整应用中，本文件提供“把列规则文本编译成可重复查询的列过滤器”这一层。已索引的直接生产接线位于 `dumpling/export/column_filter.rs`：`columnFilterFromToml` 调用 `ParseColumnFilterRules` 编译每一组 TOML `columns`，`columnFilterConfig::applyToColumns` 再针对通过表过滤器选中的规则组调用 `ColumnFilterRules::match_rule`，生成可写列及其原始下标。它不是 SQL 执行器或存储层组件，也不自行读取 Dumpling 配置。

## 核心职责

- 定义对象安全的 `ColumnFilter` trait，以 `MatchColumn(&self, column: &str) -> bool` 表达“该列是否被接纳”。
- 用 `ColumnFilterRules(Vec<columnRule>)` 持有已经解析、按优先级排好序的规则，并提供布尔匹配、保留显式排除结果的组合匹配，以及只读长度查询。
- 用 `ParseColumnFilterRules` 驱动 `columnRulesParser`，将命令行风格规则（包括由解析器处理的 `@file`）编译为具体规则集。
- 用 `ParseColumnFilter` 把具体规则集擦除成 `Box<dyn ColumnFilter>`，为只需要单一布尔判定的调用方提供稳定接口。
- 保证列名匹配不区分大小写，并通过反转解析结果实现“后写规则优先”。若没有任何规则匹配，默认拒绝该列。

本文件不负责模式语法、正则编译或文件导入细节；这些分别落在 `parser.rs` 的 `columnRulesParser`/`matcherParser` 和 `matchers.rs` 的各类 `matcher` 实现中。

## 主要符号

- `pub trait ColumnFilter: Debug`：公开过滤接口。唯一方法 `MatchColumn` 借用过滤器与列名，不改变状态。trait 只约束 `Debug`，接口类型本身没有声明 `Send` 或 `Sync`。
- `pub struct ColumnFilterRules(Vec<columnRule>)`：具体规则容器。元组字段私有，调用方只能通过构造函数和公开查询方法使用；`Default` 产生空规则集。
- `ColumnFilterRules::match_rule(&self, column: &str) -> Option<bool>`：先对输入列名逐字符小写化，再从规则向量开头查找第一条匹配规则；肯定规则返回 `Some(true)`，否定规则返回 `Some(false)`，无匹配返回 `None`。保留三态是组合多个规则组时区别“明确排除”和“该组未表态”的关键。
- `ColumnFilterRules::len` / `is_empty`：暴露规则条数与空状态，不泄露内部 `columnRule`。
- `ColumnFilterRules::MatchColumn`（固有方法）：转发到同类型的 `ColumnFilter` trait 实现，使持有具体类型的调用方无需导入 trait 也能调用同名行为。
- `pub fn ParseColumnFilter(Vec<String>) -> Result<Box<dyn ColumnFilter>, FilterError>`：调用具体解析函数，成功后装箱；错误原样通过 `?` 传播。
- `pub fn ParseColumnFilterRules(Vec<String>) -> Result<ColumnFilterRules, FilterError>`：创建带 `<cmdline>:1` 位置上下文的解析器，逐项解析，反转规则，再构造具体规则集。
- `impl ColumnFilter for ColumnFilterRules`：把 `match_rule` 的三态压缩为布尔值，使用 `unwrap_or(false)` 实现默认拒绝。

## 执行流程

1. 调用方把规则字符串交给 `ParseColumnFilter` 或 `ParseColumnFilterRules`。前者只是后者的动态分发包装。
2. `ParseColumnFilterRules` 按参数数量预分配 `parser.rules`，并将诊断来源初始化为 `<cmdline>` 第 1 行。
3. 参数按输入顺序逐个传给 `columnRulesParser::parse(&arg, true)`。相邻 `parser.rs` 证明该解析器会忽略空白行和注释，识别前导 `!`，允许顶层 `@file`，解析正则、引号或通配模式，并在入库前调用匹配器的 `toLower()`。
4. 任一参数解析失败时，`?` 立即返回 `FilterError`，不会交付部分规则集。全部成功后调用 `parser.rules.reverse()`。
5. 匹配时，`match_rule` 将查询列名小写化并从反转后的向量起点扫描；因此原输入中最后出现、且能匹配该列的规则最先决定结果。
6. 单组接口 `MatchColumn` 把无匹配视为 `false`。Dumpling 的组合路径则保留 `Option<bool>`：`applyToColumns` 将适用的表规则组倒序后执行 `find_map`，第一个 `Some`（包括 `Some(false)`）即停止组合，无组命中才回落到 `false`。

例如规则 `*`, `!secret*`, `secret_public` 解析后逆序扫描；`secret_public` 被最后一条肯定规则接纳，其他 `secret...` 被中间否定规则拒绝，剩余列由 `*` 接纳。这一优先级来自反转加“首个匹配”，不是合并所有匹配结果。

## 数据与状态

`ColumnFilterRules` 拥有一个 `Vec<columnRule>`。每个 `columnRule`（定义于 `matchers.rs`）拥有 `Box<dyn matcher>` 和 `positive: bool`；`matcher` trait 要求 `Debug + Send + Sync`，具体实现包括精确字符串、恒真和正则匹配器。规则解析完成后，本文件只读取这些对象，没有缓存、计数器或可变全局状态。

规则向量的不变量是：由 `ParseColumnFilterRules` 生成时已按原始书写顺序反转，索引越小优先级越高；模式在 `columnRulesParser::parse` 入库前已小写化。元组字段私有可防止 crate 外调用方绕过这些约定直接重排或注入规则，但 `ColumnFilterRules::default()` 合法地产生空集合，此时任何列都不匹配。

查询列名使用 `chars()` 遍历，每个字符调用 `to_lowercase()` 并只取产生序列的第一个字符。现有 Rust 测试专门验证土耳其大写点号 I（`İ`）能匹配规则 `i`；不过“只取第一个小写字符”是实现细节，扩展 Unicode 兼容性时不能假定它等价于任意完整 Unicode case folding。

## 依赖与调用关系

上游关系：

- `pkg/util/table-filter/lib.rs` 声明并重导出本模块，同时把独立的 `column_filter_test.rs` 作为测试模块接入。
- `dumpling/export/column_filter.rs::columnFilterFromToml` 调用 `ParseColumnFilterRules`，将配置中的列规则编译进 `columnFilterRule`。
- `dumpling/export/column_filter.rs::columnFilterConfig::applyToColumns` 调用 `match_rule`；其结果决定保留哪些列及下标。该方法又由 Dumpling 导出流程的 `buildColumnProjection` 调用，RustCodeGraph 也显示对应配置测试覆盖该路径。
- `ParseColumnFilter` 主要作为 Go 对齐的通用 API 和独立单元测试入口；RustCodeGraph 对当前索引未显示它在其他 Rust 生产文件中的直接调用。

下游关系：

- `ParseColumnFilter` 调用 `ParseColumnFilterRules`。
- `ParseColumnFilterRules` 构造并调用 `parser.rs::columnRulesParser::parse`；后者可进一步调用 `matcherParser::parsePattern` 或执行一次 `@file` 导入。
- `match_rule` 调用每条 `columnRule.column` 的 `matcher::matchString`；规则中的匹配器定义于 `matchers.rs`。
- 错误类型 `FilterError` 由 crate 的相邻模块定义并贯穿解析链；本文件不包装或改写它。

## 错误处理与边界

`ParseColumnFilterRules` 是本文件唯一可能失败的实质入口。它在第一条非法规则处短路并返回 `FilterError`；`ParseColumnFilter` 不改变错误内容。解析器以 `<cmdline>:1` 作为直接参数的诊断位置，导入文件时临时切换为实际文件名和行号，因此非法正则、残余字符、不完整引号、无法打开/读取文件等错误都携带来源位置。

边界行为如下：

- 空参数、只有空白或只有注释会成功产生空规则集，但匹配结果恒为 `false`。
- 顶层规则可使用 `@file`；`parser.rs` 在处理导入文件内容时把 `can_import` 设为 `false`，所以递归导入报错。
- `!` 表示显式否定；由于规则逆序，较晚规则可覆盖较早的肯定或否定规则。
- 无规则命中时，trait 的布尔接口默认拒绝；组合接口返回 `None`，让上层继续检查其他规则组。
- 本文件不捕获 panic。正常输入解析走 `Result`；小写转换中的 `next().unwrap_or(ch)` 对空转换序列提供原字符回退。
- `ParseColumnFilterRules` 若中途失败，局部解析器及已构造规则随栈展开释放，不会把半成品返回给调用方。

## 并发与资源生命周期

解析阶段同步执行。规则字符串由函数取得所有权；解析器及其中间 `Vec` 是局部值，成功时规则向量移动进 `ColumnFilterRules`，失败时自动析构。`@file` 的打开和逐行读取发生在 `parser.rs::import_file`，文件句柄由局部 `File`/`BufReader` 的 RAII 生命周期关闭；本文件不持有文件、任务、通道、锁或事务。

匹配阶段只通过 `&self` 读取已编译规则，因此单个具体 `ColumnFilterRules` 没有内部可变性。底层 `matcher` 要求 `Send + Sync`，但公开的 `ColumnFilter` trait 及返回类型 `Box<dyn ColumnFilter>` 没有同样的并发界限；需要跨线程传递动态过滤器时，应先明确 API 是否要升级为 `dyn ColumnFilter + Send + Sync`，不能仅凭底层实现推断当前 trait object 具备该能力。

算法资源特征为：解析占用与有效规则数线性相关的存储；一次匹配会先分配小写后的 `String`，再按优先级线性扫描，遇到首条匹配立即停止。规则很多或列数很大时，新增逻辑应避免破坏早停，也应评估重复小写分配和正则匹配成本。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/table-filter/column_filter.go`。两侧共有 `ColumnFilter`、`ColumnFilterRules`、`ParseColumnFilter`、`ParseColumnFilterRules` 和 `MatchColumn` 概念，并保持以下核心语义：解析所有参数、将规则反转、把列名转为小写、首条命中决定正负、无命中返回 `false`。`pkg/util/table-filter/column_filter_test.go` 与 Rust 的独立 `column_filter_test.rs` 使用相同的通配、正则、否定、引号、转义、中文、大小写、导入和错误样例验证这些行为。

表示层存在有意差异：Go 的 `ColumnFilterRules` 是 `[]columnRule` 类型别名，Rust 用私有字段的新类型封装；Go 接口值直接承载具体切片，Rust `ParseColumnFilter` 返回 `Box<dyn ColumnFilter>`；Rust 额外公开 `ParseColumnFilterRules` 的具体返回类型、`match_rule`、`len` 和 `is_empty`，以支持 Dumpling 的多规则组组合并验证具体容器。

Rust 的 `match_rule -> Option<bool>` 没有 Go 同文件中的直接同名方法，而是为 Rust Dumpling 接线保留“未匹配”与“明确排除”的区别。其单组 `MatchColumn` 仍与 Go 完全相同地把未匹配变为 `false`。大小写实现也不是逐字翻译：Go 使用 `strings.ToLower`，Rust 使用逐字符 `to_lowercase().next()`；现有迁移测试覆盖 `BAR`/`bar` 和 `İ`/`i`，更广的 Unicode 差异仍应以新增对照测试验证。

## 扩展指南

- 新增列规则语法时，主要修改点通常是 `parser.rs::columnRulesParser::parse` 或 `matcherParser::parsePattern`，匹配表示则在 `matchers.rs`；本文件只应调整规则优先级、组合返回值或公开 API。不要在这里另建一套语法解析。
- 改变默认允许/拒绝策略时，必须同时审查 `ColumnFilter for ColumnFilterRules::MatchColumn` 的 `unwrap_or(false)` 和 Dumpling `applyToColumns` 的组合回落，否则单组与多组行为会分叉。
- 改变优先级时，必须把 `parser.rules.reverse()` 与 `match_rule` 的 `find` 一起考虑，并同步 Go 版本；去掉任意一侧都会改变“后写优先”。
- 扩展大小写或 Unicode 语义时，应同时处理“模式入库小写化”（`columnRulesParser::parse`/`matcher::toLower`）和“查询列小写化”（`match_rule`），并加入 Rust/Go 成对样例，尤其覆盖一对多映射和非 ASCII 标识符。
- 增加公开查询能力时优先保持 `ColumnFilterRules` 内部向量私有，避免调用方构造未小写或顺序错误的规则。若要跨线程共享动态 trait object，应显式设计并验证 `Send + Sync` API，而不是只修改调用处类型。
- 测试逻辑必须继续放在独立的 `pkg/util/table-filter/column_filter_test.rs`，并与 `column_filter_test.go` 的意图尽量一致；Dumpling 组合行为则在 `dumpling/export/config_test.rs` 或相邻独立测试中覆盖。该任务不建议把测试内嵌进生产源文件。
- 兼容风险主要是规则优先级、错误文本/位置、Unicode 小写和显式否定的三态组合；性能风险主要是每列分配小写字符串及规则线性扫描。任何优化都应保留首命中短路语义。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为包含 13 个符号的 Rust 文件。
- RustCodeGraph `node --file pkg/util/table-filter/column_filter.rs`：核对了全部 90 行、公开符号、反转规则、逐字符小写和默认拒绝实现；索引还指出该文件被 `dumpling/export/column_filter.rs` 使用。
- RustCodeGraph `node/query/explore`：核对 `ParseColumnFilter -> ParseColumnFilterRules`、`applyToColumns -> match_rule` 调用边，以及 `columnFilterFromToml` 编译列规则、`buildColumnProjection` 消费应用结果的上游链。对名称歧义的查询使用了文件限定符或源码位置复核。
- `pkg/util/table-filter/lib.rs`：核对模块声明、公开重导出和独立测试模块接线。
- `pkg/util/table-filter/Cargo.toml`：核对 crate 名、`lib.rs` 入口、`regex`/`regex-syntax` 依赖与 Go 包迁移元数据。
- `pkg/util/table-filter/parser.rs` 与 `matchers.rs`：核对空白/注释、否定、`@file`、禁止递归导入、诊断位置、模式小写化，以及 `columnRule`/`matcher` 的真实定义。
- `pkg/util/table-filter/column_filter.go`：核对 Go 公共 API、规则反转、大小写不敏感、首条匹配与默认拒绝语义。
- `pkg/util/table-filter/column_filter_test.rs` 与 `column_filter_test.go`：核对空规则、通配/正则/否定、后写优先、大小写、Unicode、引号/转义、解析错误、文件导入与递归导入边界；Rust 侧另验证具体规则容器的 `len`、`is_empty` 和公开匹配能力。
- `dumpling/export/column_filter.rs`：核对本 crate 在应用中的直接生产入口、规则组组合方式和空选择错误边界。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务文件指定的命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工检查上述结论均有符号或路径依据。
