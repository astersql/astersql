# `pkg/util/filter/filter.rs`

## 文件定位

本文件是 `astersql-util-filter` crate 的核心实现，提供与 MySQL replication rule 风格一致的库表过滤器。crate 入口 `pkg/util/filter/lib.rs` 将本模块公开并重导出其符号；`pkg/util/filter/Cargo.toml` 表明它直接依赖 `astersql-util-table-filter`（规则与 `Table`）、`astersql-util-table-rule-selector`（通配选择器）和 `regex`（`~` 前缀正则）。

生产代码中的直接使用点是 `pkg/util/regexpr-router/regexpr_router.rs`：`RouteTable::AddRule` 用 `New` 把 schema/table 路由模式编译成 `Filter`，`RouteTable::Route` 和扩展列查询路径再调用 `Filter::Match` 选择命中的路由规则。因此本文件位于“路由规则配置 -> 规则编译 -> 单表匹配”的工具链中，不负责 SQL 解析、数据扫描或路由冲突裁决。

## 核心职责

- `New` 将 `Rules` 中的 `DoDBs`、`DoTables`、`IgnoreDBs`、`IgnoreTables` 校验并编译为两类匹配结构：`~` 前缀模式进入 `patternMap`，普通/通配模式进入 `Selector`。
- `Filter::Match` 组合 schema 级和 table 级判定，并把最终保留/忽略结果缓存在 `cache` 中。
- `Filter::Apply` 与已废弃的 `Filter::ApplyOn` 对表列表保持输入顺序地过滤；二者在大小写不敏感模式下的返回值语义不同。
- 规则优先级遵循 Go 实现：schema 层有 `DoDBs` 时只看是否命中白名单，否则才使用 `IgnoreDBs`；table 层命中 `DoTables` 立即放行，命中 `IgnoreTables` 拒绝，有 `DoTables` 但未命中时拒绝。

本文件不解析配置文本，也不实现 trie；规则对象来自 `tfilter::MySQLReplicationRules`，通配匹配由 `selector::Selector` 完成。

## 主要符号

- `ActionType = bool`、`Do = true`、`Ignore = false`：缓存和返回判定使用的动作表示。
- `Table = tfilter::Table`、`Rules = tfilter::MySQLReplicationRules`：保持与 table-filter crate 及 Go 类型的公共契约。
- `cache`：以 `RwLock<HashMap<String, ActionType>>` 保存 `schema.table` 的最终匹配结果；`query` 返回动作与是否命中，`set` 写入或覆盖。
- `Filter`：持有 `Selector`、已编译正则 `patternMap`、可选原始 `rules`、结果缓存 `c` 与 `caseSensitive` 开关。构造完成后规则匹配结构不再变化，但结果缓存可并发更新。
- `New(caseSensitive, rules)`：公共构造入口；不区分大小写时先调用 `Rules::ToLower`，再由 `initRules` 建立匹配结构。返回 `Result<Box<Filter>, String>`。
- `dbRule`、`tblRuleFull`、`tblRuleOnlyDBPart`、`tblRuleOnlyTblPart` 与 `nodeEndRule`：标记 selector 末端负载属于纯 schema、双字面/通配、仅 schema 字面或仅 table 字面哪一种组合，并记录 Do/Ignore 身份。
- `initRules`、`initOneRegex`、`initSchemaRule`、`initTableRule`：构造期内部管线，分别负责遍历校验、正则去重编译、schema 规则分类和 table 规则四象限分类。
- `ApplyOn`：克隆每个输入；大小写不敏感时返回已小写化的克隆。源码标记为废弃兼容路径。
- `Apply`：用克隆/小写副本匹配，但返回原始 `Box<Table>`，所以保留调用者提供的大小写。
- `Match`：单表公共入口，负责输入规范化、结果缓存及 schema/table 两段合取。
- `filterOnSchemas`、`findMatchedDoDBs`、`findMatchedIgnoreDBs`、`matchDB`：schema 层优先级与具体匹配。
- `filterOnTables`、`matchTable`：table 层优先级以及 schema/table 正则与 selector 的组合匹配。
- `matchString`：优先使用 `patternMap` 中的已编译正则；没有对应正则时执行字符串相等比较。

## 执行流程

1. 调用者把 `caseSensitive` 和可选 `Rules` 交给 `New`。若不区分大小写，规则先整体转为小写；随后创建空 trie selector、正则表和结果缓存。
2. `initRules` 按 `DoDBs -> DoTables -> IgnoreDBs -> IgnoreTables` 的顺序处理。空 schema 或空 table 规则立即报错。
3. `initSchemaRule` 将 `~pattern` 编译进 `patternMap`；其他模式作为 `dbRule` 插入 selector。`initTableRule` 按 schema/table 是否以 `~` 开头分四种情况：双正则只预编译；schema 正则 + table 非正则按 table 建索引；schema 非正则 + table 正则按 schema 建索引并在节点负载保存正则；双非正则按完整 schema/table 路径建索引。
4. `Match` 在大小写不敏感模式下克隆并小写化输入，以 `Table::to_string()` 作为缓存键。缓存未命中时计算 `filterOnSchemas(tb) && filterOnTables(tb)`，写回后返回是否为 `Do`。
5. `filterOnSchemas` 若存在 `DoDBs`，未命中即拒绝，`IgnoreDBs` 在该分支不再参与；否则若存在 `IgnoreDBs`，命中即拒绝。
6. `filterOnTables` 对空表名直接放行，以支持 create/drop/alter database 一类 schema 语句；非空表名先检查 DoTable，再检查 IgnoreTable，最后以“是否没有 DoTables”决定默认动作。
7. `matchDB` 和 `matchTable` 先处理必须逐项组合的正则路径，再读取 selector 的 `RuleSet`，将动态负载还原为 `nodeEndRule` 并同时核对 `kind` 与 `isAllowList`。
8. `Apply`/`ApplyOn` 对列表逐项调用 `Match`，保持原顺序，不排序也不去重。

## 数据与状态

`Rules` 是构造期输入并保存在 `Filter::rules`。大小写不敏感构造会修改传入的已装箱规则对象（该对象的所有权已经移入 `New`），之后规则不提供更新接口；因此 selector、`patternMap` 与规则集合可视为构造后只读。

`patternMap` 以去掉 `~` 的原始模式为键，避免同一模式重复编译。selector 节点保存 `Arc<nodeEndRule>`，让 trait object 负载可在线程间共享。`nodeEndRule::r` 只用于“schema 非正则、table 正则”的组合，其余组合通过 `patternMap` 或完整 selector 路径求值。

结果缓存键由规范化后的 `Table::to_string()` 产生，值只是最终 `Do`/`Ignore`。它没有容量上限和失效机制；这与规则构造后不可变相配，但意味着长生命周期过滤器面对无界不同表名时缓存会持续增长。缓存命中绕过全部规则匹配。

## 依赖与调用关系

上游直接调用关系（由 RustCodeGraph 文件索引与 `rg` 精确补查确认）：

- `pkg/util/regexpr-router/regexpr_router.rs::RouteTable::AddRule -> filter::New`：分别把库级规则构造成 `DoDBs`，把表级规则构造成 `DoDBs + DoTables`。
- `pkg/util/regexpr-router/regexpr_router.rs::RouteTable::Route -> Filter::Match`：把匹配结果分为 table rule 和 schema rule，再由 router 处理多规则冲突与目标名选择。
- 同文件的扩展列查询路径也调用 `Filter::Match`，确保扩展列只取自匹配路由规则。
- workspace 根 facade 在 `pkg/lib.rs` 重导出 `facade_util_filter::*`；多个 Cargo manifest 声明该 crate，但在当前 Rust 源码精确搜索中，没有发现除 regexpr-router 与测试外直接调用本文件 API 的生产实现，不能仅凭依赖声明推断运行时调用。

下游关系：

- `New -> Rules::ToLower -> initRules`。
- `initRules -> initSchemaRule/initTableRule -> initOneRegex` 或 `selector::Selector::Insert`。
- `Apply/ApplyOn -> Match -> cache::query`；未命中时 `Match -> filterOnSchemas + filterOnTables -> matchDB/matchTable -> matchString/Selector::Match`，最后 `cache::set`。
- `regex::Regex` 只处理显式 `~` 前缀规则；`*`、`?`、`[range]` 等非 `~` 模式由 `astersql-util-table-rule-selector` 的 trie 处理。

## 错误处理与边界

- `New` 是唯一把规则错误暴露给调用者的公共入口。四类规则中的空 schema，以及 table 规则中的空 table，都会返回固定文本的 `String` 错误；部分已插入的内部状态随构造失败的 `Filter` 一起丢弃，不会泄露给调用者。
- `Regex::new` 的错误被转换为字符串并原样上抛。Rust `regex` 与 Go `regexp` 都不支持 look-around，`filter_test.rs::TestInvalidRegex` 和 `migration_aster_unit_test.rs::migration_invalid_rules_and_regex_are_rejected` 固化了拒绝前瞻模式的行为。
- `rules == None` 表示不过滤：`initRules` 成功空返回，`Match` 恒为 `true`，`Apply`/`ApplyOn` 原样返回输入列表。
- 空表名在 table 层无条件通过，但仍须先通过 schema 层。这是 schema 级 DDL 的显式边界，不应改成普通 table 匹配。
- `DoDBs` 相对 `IgnoreDBs` 具有分支优先权；`DoTables` 命中又先于 `IgnoreTables`。同一对象同时出现在 Do 与 Ignore 规则时，不是简单的“Ignore 总优先”。
- selector 负载通过 `downcast_ref::<nodeEndRule>().expect(...)` 还原，读写锁通过 `expect(...)` 获取；类型契约被内部写入破坏或锁中发生 panic 后再次访问会 panic，而不是返回可恢复错误。
- 非 `~` 且不含 `*`、`?`、`[` 的模式在 `matchDB`/`matchTable` 中有直接相等快速路径，明确保护非 ASCII 字面量的精确匹配；通配语义仍交给 selector。

## 并发与资源生命周期

`Filter` 的规则结构在 `New` 返回前一次性建立，之后公共匹配方法只读 selector、正则与规则。`selector::Selector` trait 要求 `Send + Sync`，`nodeEndRule` 用 `Arc` 存入 selector；结果缓存用 `std::sync::RwLock` 允许并发读与互斥写，因此同一个 `Filter` 可被多个只读调用者共享。

缓存未采用原子“查询并填充”事务：两个线程可同时未命中、分别计算并先后写入相同值。由于规则不可变且计算确定，这只造成重复工作，不改变结果。读锁在 `cache::query` 返回前释放，写锁在 `cache::set` 插入后释放，没有跨正则或 selector 匹配持锁。

本文件不创建线程、异步任务、通道、文件句柄或数据库事务。所有表、规则和过滤器由 Rust 所有权管理；`Apply` 消耗输入 `Vec` 并把被保留的原对象移入结果，`ApplyOn` 消耗输入后返回克隆对象。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/filter/filter.go`，Rust 保留了 Go 的公开名称、四类规则、初始化顺序、优先级、selector 负载分类、缓存流程以及 `Apply`/`ApplyOn` 的行为差异。

主要表达差异如下：

- Go 的指针和 nil 用 Rust 的 `Box`、借用与 `Option<Box<Rules>>` 表达；Rust 方法不存在 nil receiver，只有 `rules == None` 的不过滤路径。
- Go 的 `sync.RWMutex + map` 对应 Rust 的 `RwLock<HashMap<...>>`；Go 锁操作无返回值，Rust 在 poisoned lock 上选择 panic。
- Go selector 的 `any` 负载类型断言对应 Rust `Arc<dyn Any + Send + Sync>` 的 downcast。
- Go 返回 `error` 并用 `errors.Trace` 包装正则错误；Rust 将错误收敛为 `String`，不保留结构化错误链。
- Rust 的 `ApplyOn` 总返回 `Vec`，无法保留 Go nil slice 与空 slice 的类型区别；`filter_test.rs` 用 `ExpectedTables::Nil` 只断言结果为空。
- Rust 为普通非通配字面量增加直接字符串相等路径，目的是在 selector 的字节 trie 行为之外保持 Go 对 Unicode schema/table 字面量的精确语义；相关测试为 `TestCaseInsensitiveUnicodeLiteral` 和 `migration_case_insensitive_matching_lowercases_unicode`。

Go 测试 `pkg/util/filter/filter_test.go` 与 Rust 的 `pkg/util/filter/filter_test.rs` 对应覆盖表驱动规则、大小写、`Apply`/`ApplyOn`、非法正则和 bool 返回；`pkg/util/filter/migration_aster_unit_test.rs` 另覆盖迁移优先级、缓存重复命中、Unicode 与空规则。

## 扩展指南

- 新增规则种类时，应同步修改 `Rules` 来源类型、`initRules`、规则分类常量/`nodeEndRule`、具体匹配函数和优先级函数；只在初始化阶段插入规则而不在匹配阶段识别 `kind` 会造成静默不命中。
- 改动大小写策略时，必须同时检查规则侧 `Rules::ToLower`、输入侧 `Match`、`Apply` 与 `ApplyOn` 的返回对象语义，并补充 Unicode 用例。不要把测试内嵌到 `filter.rs`；应更新同目录独立文件 `filter_test.rs`，迁移差异可补到 `migration_aster_unit_test.rs`，同时对照 `filter_test.go`。
- 改动优先级时，应建立 DoDB/IgnoreDB 与 DoTable/IgnoreTable 交叉用例，尤其保留空表名的 schema DDL 行为，并检查 `regexpr-router` 的表级优先和冲突逻辑是否仍成立。
- 新增可变规则 API 前，必须设计 selector、`patternMap` 与结果缓存的同步失效；当前缓存正确性依赖规则构造后不可变。
- 若改变缓存策略，应评估高基数表名导致的内存增长、锁竞争和重复未命中计算；若引入容量限制，要保持逐次 `Match` 的确定性。
- 若改变错误类型或正则引擎，需要同步 `RouteTable::AddRule` 的错误上下文，并证明与 Go regexp 的可接受语法仍兼容。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、目标目录 10 个文件；`files --filter pkg/util/filter` 确认目标、Go 对照和独立测试均在索引中；`node --file pkg/util/filter/filter.rs --offset 1/261` 完整读取 504 行源码并显示 34 个符号。针对 `New`、`Match` 等通用名称的全库 `explore/query/callers/callees` 结果存在同名歧义，故按技能规则用精确源码搜索补齐直接调用边，未把歧义结果当作本文件事实。
- 源与 crate 边界：`pkg/util/filter/filter.rs`、`pkg/util/filter/lib.rs`、`pkg/util/filter/Cargo.toml`。
- 直接生产调用：`pkg/util/regexpr-router/regexpr_router.rs`（`RouteTable::AddRule`、`RouteTable::Route` 及扩展列匹配路径）与 `pkg/util/regexpr-router/Cargo.toml`。
- 下游类型/匹配接口：`pkg/util/table-filter/table_filter.rs` 与 `pkg/util/table-rule-selector/trie_selector.rs`。
- Go 对照：`pkg/util/filter/filter.go`、`pkg/util/filter/filter_test.go`。
- Rust 独立测试：`pkg/util/filter/filter_test.rs`、`pkg/util/filter/migration_aster_unit_test.rs`；它们覆盖四类 Do/Ignore 组合、通配与正则、大小写、Unicode、空规则、缓存命中、无表名、非法规则及 `Apply`/`ApplyOn` 差异。
- 本任务是纯文档分析，依计划不运行 Cargo。交付前另执行任务指定的 11 章节结构检查，并人工检查唯一新增生产物、`plan.md` 未修改、文档未建议把测试写入源文件。
