# `pkg/util/table-filter/table_filter.rs`

## 文件定位

本文件是 `astersql-util-table-filter` crate 的表级过滤核心。crate 入口 `pkg/util/table-filter/lib.rs` 公开重导出本模块，因此调用方可直接使用 `Filter`、`Parse`、`CaseInsensitive` 和 `All`。`pkg/util/table-filter/Cargo.toml` 将 crate 的 Go 对照包标为 `pkg/util/table-filter`，直接依赖只有 `regex` 与 `regex-syntax`；正则的构造实际由相邻 `matchers.rs` 承担，本文件只组合解析器和匹配器。

已确认的生产接线包括：`dumpling/export/column_filter.rs` 的列过滤参数解析先调用 `Parse` 构造表过滤器，并在配置要求大小写不敏感时调用 `CaseInsensitive`；`pkg/util/table-filter/compat.rs::ParseMySQLReplicationRules` 在未提供旧式复制规则时调用 `All`。其他 crate 也可通过公开 trait `Filter` 持有过滤器，例如 `br/pkg/utils/filter.rs` 引入该 trait；不能仅凭同名符号把 `br` 中的本地桩实现视为本文件调用者。

## 核心职责

本文件把“规则文本”转换为统一的动态分发过滤接口，并定义三种实现形态：

- `tableFilter`：按优先级有序保存 `tableRule`，执行 schema/table 规则匹配。
- `loweredFilter`：把规则侧和输入侧都转换为与 Go 兼容的小写形式，再委托内层过滤器。
- `allFilter`：无条件接受所有 schema 和 table，作为无过滤配置的恒真实现。

核心语义是“后写规则优先”。`Parse` 在逐条解析输入后反转 `parser.rules`，而 `MatchTable`、`MatchSchema` 都只采用第一个有决定权的规则。因此反转后的第一次命中就是原始输入中的最后一条适用规则。若没有规则决定结果，两种匹配均返回 `false`。

## 主要符号

- `pub trait Filter: Debug + Send + Sync`：统一边界。`MatchTable(schema, table)` 判定具体表，`MatchSchema(schema)` 判定一个库是否仍可能包含可处理对象，私有于 crate API 语义的 `toLower` 生成规则侧小写变体。`Send + Sync` 允许 trait object 跨线程共享，但接口本身不引入并发执行。
- `pub struct tableFilter(pub Vec<tableRule>)`：规则列表的具体实现。字段公开，但 `tableRule` 来自 `matchers.rs`，每条规则含 schema matcher、table matcher 和 `positive` 决策。
- `pub fn Parse(Vec<String>) -> Result<Box<dyn Filter>, FilterError>`：公开解析入口。初始化 `tableRulesParser`，以 `"<cmdline>"` 和第 1 行作为直接参数的错误位置，逐项调用 `tableRulesParser::parse(arg, true)`，反转结果后装箱为 `tableFilter`。
- `pub fn CaseInsensitive(Box<dyn Filter>) -> Box<dyn Filter>`：先且仅先调用一次传入实现的 `toLower`，再把结果转成 `Arc<dyn Filter>` 放入 `loweredFilter`。
- `tableFilter::{MatchTable, MatchSchema, toLower}`：分别完成具体表决策、schema 粗粒度决策，以及不改变规则顺序和正负性的 matcher 深拷贝转换。
- `fn go_lowercase(&str) -> String`：逐 Unicode 标量调用 `char::to_lowercase()`，只取第一个映射字符，刻意模拟 Go `strings.ToLower` 在本模块所需的简单映射行为，避免 Rust 完整大小写展开及上下文终结 sigma 差异。
- `struct loweredFilter { wrapped: Arc<dyn Filter> }`：输入侧大小写适配器。重复执行 `toLower` 时只克隆同一个 `Arc`，不会再次转换规则。
- `struct allFilter` 与 `pub fn All()`：零状态恒真实现及其公开构造器。

## 执行流程

1. 调用方把序列化规则传给 `Parse`。每个参数由 `parser.rs::tableRulesParser::parse` 解析；`can_import=true` 允许顶层 `@file` 导入，解析错误以 `FilterError` 立即返回。
2. 所有规则解析成功后，`Parse` 反转列表。匹配阶段从索引 0 开始扫描，因此原始规则列表中越靠后的规则优先级越高。
3. `tableFilter::MatchTable` 查找第一条 schema matcher 与 table matcher 同时命中的规则，返回该规则的 `positive`；没有命中则返回 `false`。
4. `tableFilter::MatchSchema` 只要求 schema matcher 命中，但负规则只有在其 table matcher 能匹配所有字符串时才有权否决整个 schema。正规则即使只指定某张表，也足以说明该 schema 仍可能包含可处理对象。第一条满足该“有决定权”条件的规则提供 `positive`，无规则则拒绝。
5. 若调用方使用 `CaseInsensitive`，构造时先通过各 matcher 的 `toLower` 转换规则侧；调用 `loweredFilter::MatchTable` 或 `MatchSchema` 时再经 `go_lowercase` 转换输入侧，然后委托给已转换的内层过滤器。
6. `All` 跳过解析和规则扫描，返回 `allFilter`；两个匹配方法恒为 `true`。对它再调用 `CaseInsensitive` 仍保持恒真。

## 数据与状态

`tableFilter` 的唯一持久状态是 `Vec<tableRule>`。规则顺序本身就是优先级，任何排序、追加或删除都会改变可观察行为。每个 `tableRule` 的 matcher 是 trait object，具体可能是精确字符串、恒真或正则匹配器；这些类型和 `FilterError` 定义在 `matchers.rs`，文本到规则的转换定义在 `parser.rs`。

`loweredFilter` 持有 `Arc<dyn Filter>`。`CaseInsensitive` 会取得由 `toLower` 新建的过滤器所有权并放入 `Arc`；之后重复小写包装只通过 `Arc::clone` 共享同一只读对象。匹配调用只创建临时的小写 `String`，不修改规则状态。

`allFilter` 是零大小、无状态类型。三种实现均没有缓存、全局变量或可变静态状态。

## 依赖与调用关系

向下依赖如下：

- `Parse` → `parser.rs::tableRulesParser::parse` → `matcherParser`，负责语法、`@file` 导入与 matcher 构造；解析层再使用 `matchers.rs` 的精确、通配和正则实现。
- `tableFilter::MatchTable` → `tableRule.schema.matchString` 与 `tableRule.table.matchString`。
- `tableFilter::MatchSchema` → `matchString` 与 `matchAllStrings`；后者是区分“排除单表”和“排除整个 schema”的关键边。
- `tableFilter::toLower` → 两个 matcher 的 `toLower`，保留 `positive` 和列表次序。
- `loweredFilter::{MatchTable, MatchSchema}` → `go_lowercase` → 内层 `Filter` 对应方法。

向上调用的已核实实例是 `dumpling/export/column_filter.rs`：每条列过滤配置的 table matcher 通过 `Parse` 构造，大小写不敏感配置再套 `CaseInsensitive`，解析错误会被补充 `--column-filter` 的条目编号上下文。相邻 `compat.rs::ParseMySQLReplicationRules` 在 `rules == None` 时以 `All` 表示旧配置语义中的全量匹配，并在其他分支组合实现 `Filter` 的兼容过滤器。

RustCodeGraph 对目标文件识别出 24 个符号，但同名的 Go 方法、BR/Dumpling 桩和测试辅助类型较多；调用关系结论因此只采用目标文件源码、无歧义 crate 引用和上述直接生产调用点，不把图中的同名候选合并。

## 错误处理与边界

只有构造路径 `Parse` 返回错误。它对每个参数使用 `?`，所以首个语法、正则或文件导入错误立即终止，不返回部分过滤器。错误位置由 `matcherParser.fileName/lineNum` 跟踪：命令行参数起始为 `"<cmdline>":1`，导入文件由解析器更新路径与行号。独立测试覆盖非法正则、额外字符、非法转义、残缺字符类、缺失 schema/table 模式、未闭合正则/引号、行尾反斜杠、非法注释位置、递归导入和文件不存在。

匹配阶段不返回错误：空规则、无匹配规则均按默认拒绝处理。`MatchSchema` 的边界尤其重要：`!foo.bar` 只排除具体表，不能证明整个 `foo` schema 都不可处理；`!foo.*` 才能通过 `matchAllStrings` 否决 schema。新增 matcher 时若错误实现 `matchAllStrings`，会直接破坏 schema 级剪枝语义。

大小写转换故意不是 Rust 字符串的完整 Unicode lowercase 展开。`go_lowercase` 每个输入字符最多产生一个字符；`table_filter_test.rs::case_insensitive_uses_go_simple_unicode_lowercase` 用 `İ` 和希腊 sigma 验证这一 Go 兼容边界。不要把它替换成普通的 `str::to_lowercase()`，除非同时接受并验证兼容性变化。

## 并发与资源生命周期

`Filter` 要求 `Send + Sync`，所以实现可安全地作为跨线程 trait object 传递或共享。当前匹配逻辑只读规则，没有锁和内部可变性。`loweredFilter` 使用 `Arc` 管理共享内层过滤器的生命周期；最后一个引用释放时内层规则与 matcher 一并释放，不存在显式关闭步骤。

解析器及其 `Vec<tableRule>` 是 `Parse` 调用内的局部所有权：解析失败时已构造规则随栈展开释放，成功时整体移入 `tableFilter`。`@file` 的文件句柄生命周期由 `parser.rs` 的读取实现控制，本文件不持有打开文件。匹配时生成的小写字符串仅存活于单次委托调用。文件内没有任务、通道、事务、锁或 I/O 重试策略。

## 与 Go 版本的对应关系

`pkg/util/table-filter/table_filter.go` 与本文件在接口、三种实现、规则反转及首次决定规则语义上逐项对应：Go `Filter` 对应 Rust `Filter`，Go `tableFilter []tableRule` 对应 Rust `tableFilter(Vec<tableRule>)`，Go `loweredFilter`/`allFilter` 分别对应同名 Rust 私有结构，公开的 `Parse`、`CaseInsensitive`、`All` 名称与行为一致。

实现语言差异主要有四点：

- Go 使用接口值，Rust 返回 `Box<dyn Filter>`，并通过 `Debug + Send + Sync` 明确 trait object 约束。
- Go 用 `slices.Reverse` 原地反转，Rust 用 `Vec::reverse`；优先级结果相同。
- Go `loweredFilter.toLower` 返回自身；Rust 因所有权和 trait object 生命周期，在该方法中克隆 `Arc`，保持同一个已转换内层对象。`case_insensitive_repeated_wrapper_preserves_lowered_filter` 验证重复包装不会重复调用原过滤器的转换逻辑。
- Go 输入侧调用 `strings.ToLower`；Rust 用专门的 `go_lowercase` 避免 Unicode 展开差异。Rust 测试在 Go 原有表驱动测试之外补充了该兼容回归。

`table_filter_test.rs` 是独立 Rust 测试文件，通过 `lib.rs` 的 `#[cfg(test)]` 模块接入；它对照 `table_filter_test.go` 覆盖表匹配、schema 匹配、解析失败、文件导入、递归导入拒绝和 `All`。这满足测试逻辑不内嵌生产源文件的仓库约束。

## 扩展指南

- 新增规则优先级或默认策略时，修改焦点是 `Parse` 的反转约定及 `tableFilter::{MatchTable, MatchSchema}` 的首次命中逻辑；必须同步 `table_filter_test.rs` 的重叠正/负规则案例，并与 `table_filter_test.go` 的可观察行为核对。
- 新增 matcher 类型时应在 `matchers.rs` 和 `parser.rs` 接线，并为 `matchString`、`matchAllStrings`、`toLower` 三项语义同时提供测试。尤其要验证负表规则不会错误排除整个 schema。
- 调整大小写语义时，应同时检查 `go_lowercase`、各 matcher 的 `toLower`、`compat.rs` 中的兼容过滤器及 Unicode 回归；风险是 Go/Rust 对非 ASCII 标识符得出不同结果。
- 新增公开过滤器实现必须满足 `Debug + Send + Sync`，让 `toLower` 保持幂等或至少保证重复 `CaseInsensitive` 不改变语义。相关测试继续放在独立 `*_test.rs` 文件，由 `lib.rs` 条件引入，不应放回本生产文件。
- 性能上，`MatchTable`/`MatchSchema` 是按优先级线性扫描，大小写包装每次匹配都会分配一到两个 `String`。若引入索引或缓存，必须保留“原始后写规则优先”和 matcher 顺序，且应评估共享缓存所需同步成本，不能以重排规则换取速度。
- 变更 `Parse` 的错误包装时，应保留文件名、行号和底层原因，以免 Dumpling 等上层再次包装后丢失定位信息。

## 验证依据

本说明基于以下直接证据：

- 生产源码：`pkg/util/table-filter/table_filter.rs`（`Filter`、`tableFilter`、`Parse`、`CaseInsensitive`、`go_lowercase`、`loweredFilter`、`allFilter`、`All`）。
- crate 边界：`pkg/util/table-filter/Cargo.toml` 与 `pkg/util/table-filter/lib.rs`；确认 crate 名、依赖、Go 包映射、公开重导出及独立测试模块接线。
- 下游实现：`pkg/util/table-filter/parser.rs`、`matchers.rs` 与 `compat.rs`；确认解析入口、matcher 契约、`FilterError`、`matchAllStrings` 和 `All` 的兼容层调用。
- 上游调用：`dumpling/export/column_filter.rs`；确认 `Parse`、条件性 `CaseInsensitive` 及错误上下文的生产使用。
- Go 对照：`pkg/util/table-filter/table_filter.go` 与 `table_filter_test.go`。
- Rust 测试：`pkg/util/table-filter/table_filter_test.rs`，包括 `test_match_tables`、`test_match_schemas`、`test_parse_failures2`、`test_import2`、`test_recursive_import2`、`test_all`、`case_insensitive_uses_go_simple_unicode_lowercase` 和 `case_insensitive_repeated_wrapper_preserves_lowered_filter`。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/util/table-filter` 列出同目录 20 个已索引 Go/Rust 文件；`node --file .../table_filter.rs` 返回完整 144 行及 24 个符号；`query TableFilter`、`query CaseInsensitive` 用于识别同名候选并避免错误归并调用边。

本任务是纯文档分析，按计划未运行 Cargo。最终结构校验应确认该文件存在并且恰有本页列出的 11 个固定二级标题；人工复核重点是规则反转、`MatchSchema` 的负规则门槛、Go 兼容小写及独立测试位置均有源码依据。
