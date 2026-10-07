# `dumpling/export/column_filter.rs`

## 文件定位

本文件属于 `astersql-dumpling-export` library crate；crate 清单位于 [`dumpling/export/Cargo.toml`](Cargo.toml)，入口 [`dumpling/export/lib.rs`](lib.rs) 通过 `include!("column_filter.rs")` 把它并入与 Go `dumpling/export` 包相近的单包作用域。因此，文件中的 `Config`、`Result`、`errors_new`、`escapeString` 和 `Arc` 来自 crate 级共享作用域，而不是本文件自己的 `use`。

它位于“命令行配置”与“导出列投影”之间：[`Config::ParseFromFlags`](config.rs) 读取 `--column-filter`、`--column-filter-file` 和 `--case-sensitive` 后调用 `Config::parseColumnFilterOptions`；运行期的 [`buildColumnProjection`](dump.rs) 再调用 `columnFilterConfig::applyToColumns`，把数据库返回的可写列裁剪成查询字段及其原始位置。该文件本身不执行导出、SQL 查询或写文件，只负责配置解码、规则编译、选项约束和列选择。

## 核心职责

1. 用 `columnFilterConfig` 保存已经编译的表匹配器与列规则，使后续每张表应用过滤时不必重新解析文本。
2. 将两种输入统一为相同配置：`parseColumnFilterArgs` 解析重复的内联 TOML 规则，`parseColumnFilterConfig` 读取含 `[[filters]]` 的 TOML 文件，二者最后都进入私有的 `columnFilterFromToml`。
3. 保持规则优先级：表规则按配置的逆序检查，后写的匹配组先决定某列；组内列规则由 `astersql-util-table-filter` 按其规则顺序返回首个匹配结果。
4. 拒绝会产生歧义或无效导出的配置，包括未知 TOML 键、字段类型错误、空 matcher/columns、非法匹配表达式、同时指定内联规则与文件、以及列过滤器与 `--sql` 同用。
5. 为导出投影同时返回列名和原始下标；[`buildColumnProjection`](dump.rs) 用下标从完整 `sourceTypes` 中构造 `selectedTypes`，从而保持名称、类型和结果列位置一致。

## 主要符号

- `pub struct columnFilterConfig { pub Filters: Vec<Arc<columnFilterRule>> }`：可克隆的已编译规则集合。`Default` 代表未启用列过滤；`Filters` 为公开字段是迁移期跨文件配置与测试所需的接口。
- `struct columnFilterRule`：单个已编译规则组，包含 `Box<dyn column_filter_lib::Filter>` 表匹配器和 `ColumnFilterRules` 列规则。原始 `matcher`、`columns` 文本不在此结构中长期保存。
- `columnFilterConfig::applyToColumns(&self, database, table, source)`：主要运行期入口，返回 `(选中列名, 选中列在 source 中的下标)`，或者在规则命中表却没有可写列入选时返回错误。
- `fn columnFilterFromToml(value, case_sensitive, option)`：两种输入格式共享的解码、未知键检查、必填项检查与规则编译核心；`option` 用于生成与实际命令行选项一致的错误上下文。
- `pub fn parseColumnFilterArgs(args, case_sensitive)`：把每个参数包装成 `filter = <arg>` TOML 文档，验证仅含 `matcher` 和 `columns`，然后组装为统一的 `filters` 数组。
- `pub fn parseColumnFilterConfig(path, case_sensitive)`：读取文件、验证 UTF-8、解析 TOML，并交给 `columnFilterFromToml`。
- `pub fn validateColumnFilterOptions(conf, option)`：拒绝列过滤器与 `Config.SQL` 同时存在；注释明确 `NoSchemas` 限制已由 Go 提交 `dfc06738174f7e15c383a76a536568a4005d2adc` 移除。
- `Config::parseColumnFilterOptions(args, path, case_sensitive)`：配置接线入口。先拒绝同时指定参数与文件，再验证互斥选项并只在成功解析后替换 `self.columnFilter`。

本文件没有常量、枚举、trait、条件编译项或内嵌测试。

## 执行流程

配置阶段有两条入口，但在编译处汇合：

1. [`Config::ParseFromFlags`](config.rs) 取得重复的 `column-filter` 参数、`column-filter-file` 路径和大小写开关，调用 `Config::parseColumnFilterOptions`。
2. 若参数与非空白文件路径同时存在，立即报互斥错误；若二者都没有，保持默认空配置并成功返回。
3. 内联路径逐项把参数解析为 TOML `filter` 表，先检查表形状、数组类型、字符串元素和未知键，再把规则复制进统一 `filters` 数组。文件路径则先读完整文件并要求 UTF-8，再解析为 TOML 值。
4. `columnFilterFromToml` 要求根为表、`filters` 为数组；它先解码所有规则的 `matcher`/`columns` 字符串数组，再汇总未知键。之后要求至少一个过滤器，逐项要求 matcher 和 column 规则非空，并调用 `column_filter_lib::Parse` 与 `ParseColumnFilterRules` 编译。
5. `case_sensitive == false` 时，表过滤器再经过 `column_filter_lib::CaseInsensitive` 包装；列规则依赖库自身按小写匹配的语义。
6. 导出准备阶段，[`prepareColumnProjection` / `buildColumnProjection`](dump.rs) 获取基本表的可写列，再调用 `applyToColumns`。非基本表在到达列过滤前直接返回默认投影。
7. `applyToColumns` 逆序收集所有命中当前库表的规则组。没有组命中时原样复制全部源列并返回 `0..source.len()`；有组命中时，对每个源列按规则组优先级取第一个给出结果的 `match_rule`。选中列保留源顺序，并同步记录下标。
8. 有表规则命中但最终没有列入选时返回错误；否则 `buildColumnProjection` 根据列数、生成列和 `CompleteInsert` 决定使用 `*` 还是显式字段，并用下标映射列类型。

## 数据与状态

`columnFilterConfig` 的状态只有已编译规则向量。向量顺序就是配置顺序，但应用时通过 `.iter().rev()` 使后写规则组具有更高优先级。每项放在 `Arc` 中，使 `Config::clone_for_mutate` 克隆配置时共享不可变规则，而不复制 trait object；本文件没有提供规则的内部可变入口。

`applyToColumns` 不修改配置。输入 `source: &[String]` 被视为权威的可写列顺序；输出列名是入选元素的克隆，输出 `usize` 下标严格对应原切片位置。未命中任何表规则与“命中但所有列被排除”是两个不同状态：前者表示不过滤并成功，后者表示配置会使该表无法写出而报错。

解析时的 `decoded: Vec<(Vec<String>, Vec<String>)>` 是短生命周期中间态。它让字段类型检查先于未知键报告和匹配器编译，贴合 Go TOML 解码的错误顺序；编译成功后原始字符串由依赖库转换为匹配对象，不再由本文件保存。文件读取内容与 TOML 树同样只存在于解析调用期间。

## 依赖与调用关系

- 上游配置调用：[`dumpling/export/config.rs`](config.rs) 的 `Config::ParseFromFlags` 调用 `self.parseColumnFilterOptions(...)`；`Config` 持有 `columnFilterConfig`，默认值为空，并在 `clone_for_mutate` 中克隆它。
- 上游运行期保护：[`Dumper::Dump`](dump.rs) 在配置非空时再次调用 `validateColumnFilterOptions`。这覆盖不经 flags 构造或随后被修改的 `Config`。
- 上游列投影：[`buildColumnProjection`](dump.rs) 调用 `applyToColumns`；其结果进入 `columnProjection`，后续查询和 writer 由该投影决定实际导出字段。启用过滤但投影缓存缺失时，`dump.rs` 另行报错，而不是静默回退到所有列。
- 下游规则库：[`pkg/util/table-filter`](../../pkg/util/table-filter/) 提供 `Filter::MatchTable`、`Parse`、`CaseInsensitive`、`ColumnFilterRules::match_rule` 和 `ParseColumnFilterRules`。`Filter` 要求 `Debug + Send + Sync`；`match_rule` 返回 `Option<bool>`，从而区分“显式包含/排除”和“本组无匹配”。
- 下游通用设施：`toml` 负责有序 TOML 解码（Cargo 启用了 `preserve_order`），`std::fs::read` 负责配置文件读取，`errors_new` 生成 crate 的错误类型，`escapeString` 转义错误消息中的库表名。
- crate 边界：[`dumpling/export/Cargo.toml`](Cargo.toml) 明确依赖本地 `astersql-util-table-filter`，并以 `package.metadata.porting.go-package = "dumpling/export"` 标注 Go 对照包。

RustCodeGraph 的文件关系显示 `column_filter.rs` 被 `config.rs`、`dump.rs`、`config_test.rs`、`dump_test.rs` 和 `dumpling/cmd/dumpling/config_flags_test.rs` 等引用；主要可见调用边包括 `Config::ParseFromFlags -> parseColumnFilterOptions -> parseColumnFilterArgs/parseColumnFilterConfig -> columnFilterFromToml`，以及 `buildColumnProjection -> applyToColumns -> Filter::MatchTable / ColumnFilterRules::match_rule`。

## 错误处理与边界

- TOML 根、`filters`、单条规则、`matcher` 和 `columns` 都做显式形状检查；数组元素必须是字符串。错误消息带 `--column-filter` 或 `--column-filter-file` 及规则下标，便于定位输入。
- 未知根键和规则键会被收集后一次报告。内联输入以 `filter.<key>` 命名，文件输入以 `filters.<key>` 命名；Cargo 的 `toml/preserve_order` 使报告顺序跟随输入顺序。
- `columnFilterFromToml` 先解码全部字段，再报告未知键，最后编译 matcher/column 表达式。这一顺序由 [`column_filter_decode_precedes_compile_and_unknown_keys_keep_input_order`](config_test.rs) 等 Rust 回归测试约束，修改时不能随意交换。
- 空参数列表或空文件最终形成空 `filters`，会报“requires at least one column filter”；缺少或清空任一规则的 matcher/columns 也会按规则下标报错。
- 文件不存在、读取失败、非 UTF-8、TOML 语法错误、表/列规则编译失败均通过 `Result` 原样停止配置；不会留下半编译配置，因为 `self.columnFilter` 只在右侧解析成功后赋值。
- `parseColumnFilterArgs` 在 TOML 成功解析后对 `as_table()` 和 `get("filter")` 使用 `unwrap`。这里的包装字符串固定创建根表和 `filter` 键，当前解析器契约下成立；若未来改变包装形式或 TOML 库行为，应把该不变量纳入测试或改成显式错误。
- 无表规则命中时，即使 `source` 为空也返回空列/空下标成功；若表规则命中而 `source` 为空或全部列被排除，则报“selects no writable columns”。错误中的数据库名和表名先经 `escapeString`。
- `validateColumnFilterOptions` 只处理 `--sql` 冲突；内联/文件互斥由 `parseColumnFilterOptions` 处理，其他选项兼容性由 `config.rs` 的相应验证逻辑处理。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、数据库连接或后台资源。解析阶段同步读取配置文件，`std::fs::read` 返回后文件句柄即由标准库关闭；没有长生命周期 I/O 资源。

编译后的表过滤器是 `Box<dyn Filter>`，而依赖库的 `Filter` trait 具有 `Send + Sync` 约束；规则对象再由 `Arc` 持有。因此克隆 `columnFilterConfig` 只增加引用计数，多个配置副本可共享只读匹配器。`applyToColumns` 仅借用 `&self` 并创建局部 `Vec`，不修改共享状态。并发安全仍依赖未来扩展继续保持规则对象不可变；若引入缓存或可变统计，必须显式选择同步策略并补并发测试。

投影生命周期由 `dump.rs` 管理：过滤器先用于准备并缓存 `columnProjection`，之后即使配置副本中的过滤器变化，已缓存投影仍保存源列和选中列信息。[`column_projection_split_source_columns_survive_cached_filter_mutation`](dump_test.rs) 覆盖了这一边界；本文件不负责缓存失效。

## 与 Go 版本的对应关系

直接对照文件是 [`dumpling/export/column_filter.go`](column_filter.go)。Rust 保留了 Go 的四段语义：从文件/参数解码、验证并编译规则、后写匹配组优先、未命中表时保留全部列。对应的 Go 测试集中在 [`dumpling/export/config_test.go`](config_test.go) 的 `TestColumnFilters`、`TestParseColumnFilterFile`、`TestParseColumnFilterFlag`、`TestColumnFilterOptions`，导出投影边界在 [`dumpling/export/dump_test.go`](dump_test.go) 中验证。

实现形态存在有意差异：Go 的 `columnFilterRule` 同时保存可由 TOML 反序列化的 `Matcher`/`Columns` 和编译字段，并通过 `compileForOption` 原地填充；Rust 用 `toml::Value` 手工解码，最终结构只保存编译字段。Go 用值切片保存规则，Rust 用 `Vec<Arc<_>>` 以支持配置克隆。Go 的 `matchColumnRules` 把逆序命中组的列规则连接后调用 `MatchColumn`；Rust 不复制规则，而是对逆序命中组执行 `find_map(match_rule)`。由于 `match_rule` 暴露每组首个显式包含/排除结果，这与 Go 的“后写组规则排在前面、首个匹配决定结果”一致。

Rust 独立测试 [`dumpling/export/config_test.rs`](config_test.rs) 覆盖规则优先级、大小写、未命中表、零列错误、内联/文件解析错误及错误下标；[`dumpling/cmd/dumpling/config_flags_test.rs`](../cmd/dumpling/config_flags_test.rs) 覆盖真实 CLI flags 接线；[`dumpling/export/dump_test.rs`](dump_test.rs) 覆盖投影和 CSV 输出确实排除敏感列。当前 Rust 版本还保留 Go 提交哈希注释以说明 `--no-schemas` 已不再冲突。

## 扩展指南

- 新增 TOML 字段时，应同时修改 `columnFilterFromToml` 和 `parseColumnFilterArgs` 的允许键、类型解码及编译逻辑，避免文件配置与内联配置产生不同语义；同步更新 Rust `config_test.rs` 与 Go `column_filter.go`/`config_test.go` 的对照行为。
- 修改规则优先级时，核心位置是 `applyToColumns` 的 `.rev()` 和 `.find_map()`。必须先明确“规则组优先级”与依赖库“组内首匹配”两层顺序，并覆盖多个同时命中组、显式排除、无规则匹配列和大小写组合；性能风险是规则组数乘列数的线性扫描。
- 修改列选择返回值时，要同步审查 [`buildColumnProjection`](dump.rs) 对 `indexes` 的类型映射、查询字段构造、`columnProjection` 缓存以及 [`dump_test.rs`](dump_test.rs) 的源列/选中列断言。不可只验证列名而忽略下标。
- 新增命令行互斥条件时，应优先接入 `Config::parseColumnFilterOptions` 或 `validateColumnFilterOptions`，并保留 `Dumper::Dump` 对绕过 flags 的运行期保护；同步覆盖 `config_test.rs` 与 `dumpling/cmd/dumpling/config_flags_test.rs`。
- 若需要保留或展示原始规则，应谨慎扩展 `columnFilterRule`，确认 `Config` 克隆、调试输出和内存占用；不要破坏当前编译成功后只读共享的生命周期。
- 测试应继续放在独立的 `*_test.rs` 文件，不能内嵌到本生产文件。最接近的测试落点是 `dumpling/export/config_test.rs`（解析与匹配）、`dumpling/export/dump_test.rs`（投影/导出），跨 crate flags 行为放在 `dumpling/cmd/dumpling/config_flags_test.rs`；依赖库匹配语义放在 `pkg/util/table-filter/*_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件（其中 Rust 7,032 个）；`files --filter dumpling/export/column_filter.rs` 确认目标已索引；`explore` 确认文件有 9 个符号、被配置/导出/测试文件引用，并给出 `applyToColumns`、`parseColumnFilterArgs`、`parseColumnFilterConfig`、`validateColumnFilterOptions` 的调用者。索引没有为 Rust `impl Config::parseColumnFilterOptions` 生成可查询的方法节点，相关边因此以已索引文件源码和精确调用点补证，而非推测。
- 生产源码：[`column_filter.rs`](column_filter.rs)、[`lib.rs`](lib.rs)、[`config.rs`](config.rs)、[`dump.rs`](dump.rs)、[`Cargo.toml`](Cargo.toml)、[`pkg/util/table-filter/column_filter.rs`](../../pkg/util/table-filter/column_filter.rs)、[`pkg/util/table-filter/table_filter.rs`](../../pkg/util/table-filter/table_filter.rs)。
- Rust 测试：[`config_test.rs`](config_test.rs) 的 `column_filters_preserve_rule_priority_case_and_unmatched_tables`、`column_filter_inline_validation_preserves_errors_and_indices`、`column_filter_file_validation_and_flags_match_inline_rules`；[`dump_test.rs`](dump_test.rs) 的投影缓存和 CSV 过滤测试；[`dumpling/cmd/dumpling/config_flags_test.rs`](../cmd/dumpling/config_flags_test.rs) 的 CLI 优先级与互斥测试。
- Go 对照：[`column_filter.go`](column_filter.go)、[`config.go`](config.go)、[`dump.go`](dump.go)、[`config_test.go`](config_test.go)、[`dump_test.go`](dump_test.go)。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付时用任务指定命令确认本文档存在且恰有 11 个固定二级章节，并人工复核唯一新增生产物、相对链接目标、符号名称、错误边界与扩展入口。
