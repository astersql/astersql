# `pkg/planner/core/memtable_infoschema_extractor.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate；`pkg/planner/core/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `lib.rs:154,218` 声明并公开重导出本模块。它提供一组面向 `INFORMATION_SCHEMA` 内存表的轻量谓词抽取器：把规划阶段已经简化成 `Predicate` 的条件转成元数据候选过滤状态，以减少后续枚举范围。输入谓词模型定义在同 crate 的 `memtable_predicate_extractor.rs:16-50`。

RustCodeGraph 把本文件识别为 586 行、65 个符号，并列出 `planbuilder_runtime.rs`、旧 Cascades `implementation_rules.rs` 以及两个测试文件为文件级使用者；但对本文件关键方法执行精确 `callers`/`callees` 查询没有返回静态调用边。结合全仓 Rust 引用搜索，能直接证实的构造、抽取和枚举调用主要位于独立测试。因此，本文件当前可确认的是“已公开的规划辅助 API 与已测试行为”，不能仅凭 Go 的接线推断 Rust 生产主链已经完整调用这些 API。

## 核心职责

- `InfoSchemaBaseExtractor` 统一保存等值/`IN` 条件、`LIKE` 条件和不可命中标志，并实现状态重置、条件求交、匹配和 Explain 摘要（`64-225`）。
- `base_extractor!` 为 14 类信息模式视图生成“基类字段 + 构造器 + 抽取入口”，各类型只暴露自己允许下推的列白名单（`304-320`、`323-325`、`379-520`、`559-563`）。
- 专用方法把抽取状态应用到元数据：表/库配对（`InfoSchemaTablesExtractor::ListSchemasAndTables`）、可见列及序号（`InfoSchemaColumnsExtractor::ListColumns`）和索引使用信息（`InfoSchemaTiDBIndexUsageExtractor::ListIndexes`）。
- `SchemaTableSorter` 为并行的 schema/table 切片提供同步排序，防止二者错位（`227-302`）。
- `InfoSchemaDDLExtractor` 特意只记录条件而不消费原谓词，保证 DDL 历史裁剪后仍由上层 Selection 精确复核（`361-377`）。

## 主要符号

- 列名常量 `TableSchema` 至 `DDLStateName`（`21-35`）是抽取器白名单与状态 map 的规范键。
- `predicate_value_as_string`（`37-45`）把字符串、整数、浮点和布尔字面量统一为小写/十进制文本；`like_matches`（`47-60`）先按原始 escape 编译模式，再做 Unicode 小写折叠并调用 `stringutil_dependency::string_util::DoMatch`。
- `InfoSchemaBaseExtractor`（`64-75`）包含 `ColPredicates`、用于 Explain 的 `LikePatterns`、用于真实匹配的 `LikeMatchPatterns`、逐模式 `LikeEscapes` 和 `SkipRequest`。
- `ExtractPredicates`（`83-142`）是核心入口；`push_like`、`merge`（`143-166`）维护状态；`ExplainInfo`、`Has`、`ListSchemas`（`168-224`）分别负责展示、单值匹配和候选 schema 过滤排序。
- `SchemaTableSorter::{new,Len,Less,Swap,sort}`（`241-301`）验证切片等长，并通过稳定目标排列同步交换两个切片。
- 宏生成的具体类型包括 Indexes、Tables、Views、KeyColumnUsage、TableConstraints、Partitions、Statistics、Schemata、CheckConstraints、TiDBCheckConstraints、ReferConst、Sequence、Columns、TiDBIndexUsage；DDL 类型单独实现。
- 专用查询方法包括 `HasTableName/HasTableSchema`、约束/主键/分区/索引判断，以及 `ListSchemasAndTables`（`328-359`）、`ListTables/ListColumns`（`521-549`）、`ListIndexes`（`564-584`）。
- `IndexUsageIndexInfo { Name, ID }`（`552-557`）是 `tidb_index_usage` 所需的最小索引投影。

## 执行流程

1. 调用方用具体视图的 `NewInfoSchema*Extractor` 创建默认实例；宏生成的方法把相应列白名单传给基类。
2. `ExtractPredicates` 首先清空四个 map 和 `SkipRequest`（`88-92`），所以同一实例的每次抽取都从干净状态开始。
3. 方法把白名单列名小写化，再逐个检查谓词：常量 escape 的 `LIKE`/`ILIKE` 进入模式状态；`Eq`/`In` 的值文本化后交给 `merge`；列不在白名单、escape 不能在规划期确定、`OR`/比较/regexp 等不支持形态均保留在返回列表（`94-134`）。
4. 同一列第一次出现时写入集合，后续条件求交。任一已初始化列得到空集时置 `SkipRequest=true`（`135-140`）。这是 CNF 条件“必不可能命中”的短路信号。
5. `LIKE` 与普通 `LIKE ... ESCAPE` 被保留给上层复核，同时也用于候选预过滤；常量 escape 的 `ILIKE` 在此模型中被消费。`Has` 要求等值集合命中且该列的所有 LIKE 模式都命中（`188-212`）。
6. 元数据枚举入口再应用 `Has`：表按 schema 小写名、表小写名排序；列跳过隐藏列并用“可见列序号”返回 ordinal；索引在 `PKIsHandle` 时把整型主键建模为 `primary/0`，再加入普通索引。
7. DDL 抽取器走同样的状态计算，但总是返回输入谓词原副本（`372-377`），避免把仅用于扫描裁剪的条件当作已经完成语义过滤。

## 数据与状态

`BTreeMap`/`BTreeSet` 同时提供去重与确定性顺序，使 `ExplainInfo` 和测试输出稳定。所有字段名以及用于元数据比较的值均按小写规范化；原始 LIKE 模式另存于 `LikeMatchPatterns`，因为先小写 escape 字符会破坏转义语义。`LikePatterns` 与 `LikeMatchPatterns`、`LikeEscapes` 的同列 vector 依赖相同下标对齐，这是 `push_like` 必须原子式同步追加的内部不变量。

`SkipRequest` 只由本轮抽取重新计算；未指定某列时，`Has` 对该列采取“全部允许”。空 `IN` 会形成已初始化的空集合并触发短路，但 `HasPartitionPred`/`HasPartitionIDPred` 只在集合非空时报告“存在可用谓词”。`ListColumns` 的 ordinal 从 1 开始，仅对非隐藏列递增，因此过滤后的列仍保留其在可见列序列中的原始位置。

`SchemaTableSorter` 借用两个可变切片，不拥有元数据；`new` 强制等长。`ListSchemasAndTables` 则先构造拥有所有权的 `(CIStr, TableInfo)` 副本，排序后 `unzip`，天然保持配对。

## 依赖与调用关系

- 上游数据模型：`crate::{Predicate, PredicateValue}` 来自 `memtable_predicate_extractor.rs`；模块经 `pkg/planner/core/lib.rs:218` 对外公开。
- 下游依赖：`model_dependency::{ColumnInfo, TableInfo}` 提供表、列、索引元数据；`parser_ast_dependency::CIStr` 提供原始名 `O` 与小写名 `L`；`stringutil-dependency` 负责编译和执行 SQL LIKE 模式。后三者均由 `pkg/planner/core/Cargo.toml` 声明。
- 已证实的直接行为调用者：`memtable_extractors/memtable_extractors_test.rs` 覆盖状态重置、列白名单、DDL 保留、元数据列表、escape/大小写；`operator/logicalop/.../logical_mem_table_predicate_extractor_test.rs:355-427` 覆盖 COLUMNS、prepared statement 状态隔离与约束交集；`panicrisk_regression_test.rs:51-99` 覆盖配对排序和长度不一致。
- SQL 层回归：`tests/extractor/memtable_infoschema_extractor_test.rs:505-646` 枚举多类 information_schema 表和条件组合，`650` 起覆盖自定义 escape。
- RustCodeGraph 的文件级关系还指向 `planbuilder_runtime.rs`、`cascades/old/implementation_rules.rs` 和 `pkg/util/stmtsummary/v2/record_test.rs`，但精确符号调用边未产出；全仓 Rust 搜索也未发现生产代码直接调用 `NewInfoSchema*`/`List*`。因此生产接线状态标记为“未验证完整”，不把 Go 的 `logical_plan_builder.go:5503-5531` 构造链当作 Rust 已接线证据。

## 错误处理与边界

大多数 API 是纯过滤并返回普通值，不产生 `Result`。不支持的谓词形态不会报错，而是原样进入 remaining，交给上层执行完整语义；动态、延迟、参数化或缺失的 escape 也属于该保守路径。`SchemaTableSorter::new` 是显式错误边界：并行切片长度不同时返回包含两侧长度的 `String`，避免排序时越界或错配。

需要注意的语义边界有：数值/布尔都转成字符串比较；浮点使用 Rust `to_string()`；所有等值值被小写化；多个 LIKE 采用 AND；`OR` 不抽取；普通 LIKE 即使已用于预过滤仍保留供复核；空集合触发全请求短路。`like_matches` 不返回编译错误，因为底层 `CompilePattern`/`DoMatch` 接口本身不是 fallible API。

`ListSchemasAndTables` 只过滤调用方已提供的候选，不像 Go 版本那样持有 `InfoSchema`、context 并负责查表，因而不会报告 schema 查询、取消或存储错误。扩展时不能误把该 Rust 方法视为 Go 全量枚举流程的等价替代。

## 并发与资源生命周期

本文件没有锁、原子变量、异步任务、通道、网络请求或事务；抽取器以 `&mut self` 更新本地状态，以 `&self` 读取，线程共享策略由调用方决定。类型未显式包裹 `Arc`/`Mutex`，典型生命周期是“一次计划/一次抽取实例”。prepared statement 测试证明两个实例状态互不污染（`logical_mem_table_predicate_extractor_test.rs:396-407`）。

每轮抽取会释放并重建 map/vector 内容；候选枚举通常克隆 `CIStr`/`TableInfo`，因此资源成本随候选和谓词数量增长。`ListColumns` 返回借用自 `TableInfo` 的列引用，生命周期参数保证结果不能长于表元数据；ordinal vector 独立拥有。`SchemaTableSorter` 的可变借用限定在排序期间，结束后不会保留引用。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/planner/core/memtable_infoschema_extractor.go`。两版共同保留：各视图的列白名单、同列条件收窄、`SkipRequest`、Explain 信息、大小写无关元数据匹配、DDL 不裁掉 Selection、隐藏列跳过、可见列 ordinal、整型主键索引 ID 0，以及 schema/table 配对排序。Rust 测试 `infoschema_extractors_use_the_go_column_lists` 和 `infoschema_listing_preserves_sorting_visibility_and_ordinals` 明确锁定这些迁移语义。

当前 Rust 是简化的规划数据模型，而不是 Go 实现的逐 API 完整副本：Go `Extract` 接收表达式 schema/context 并调用 `extractHelper`，Rust 接收已经归一化的 `Predicate`；Go 的 `ListSchemasAndTables` 会从 `infoschema.InfoSchema` 按 ID、分区 ID、名称枚举并传播错误，Rust 只过滤传入候选；Go 会编译 regexp，Rust用 `CompilePattern/DoMatch`；Go 的 Columns/IndexUsage 缓存谓词名，Rust每次直接调用 `Has`。这些差异说明 Rust 文件实现了抽取和候选过滤核心，但完整生产数据获取与表达式转译链不能由本文件单独证明。

配对排序对应 Go `schemaTableSorter`（Go `799-819`）；Rust额外在 `new` 阶段检查长度并以稳定置换同步交换。Go `findSchemasForTables` 的修复意图是删掉找不到 schema 的表并保持平行切片对齐（`752-796`），Rust回归测试则直接验证排序不会打乱配对。

## 扩展指南

- 新增可抽取列时，先增加/复用列常量，再修改目标 `base_extractor!` 白名单；同步检查 Go 构造器的 `extractableColumns` 和 `colNames`，并在独立测试文件增加“可消费、不可消费、交集为空”用例。
- 新增谓词形态时，修改 `InfoSchemaBaseExtractor::ExtractPredicates` 与必要的匹配状态；必须明确它是安全的精确消费还是仅作预过滤并保留 remaining。尤其不能在无法证明等价时消费 `OR`、动态 escape 或参数化条件。
- 修改 LIKE 时保持三组 vector 下标对齐，并覆盖默认 escape、自定义 escape、escape 大小写、Unicode 折叠、多模式 AND 和普通 LIKE/ILIKE 的 remaining 差异。
- 修改元数据列表时，在 `memtable_extractors/memtable_extractors_test.rs` 覆盖排序、隐藏列、ordinal、`PKIsHandle` 与 ID/name/LIKE 组合；修改 `SchemaTableSorter` 时同步维护 `panicrisk_regression_test.rs`，不得把 Rust 单元测试内嵌回生产源文件。
- 若接入真实规划主链，应在表达式到 `Predicate` 的转换点和具体内存表扫描消费者处补独立集成测试，并验证 `SkipRequest` 真能阻止底层访问，而不只是类型存在或零测试编译通过。
- 性能上应避免无界复制整个 `TableInfo` 集合；兼容性上应以 Go 的大小写、escape、remaining 和错误传播行为为基准，但对 Rust 尚未实现的 InfoSchema 查询链应单独移植，不能在本文件中用桩替代。

## 验证依据

- RustCodeGraph：`status` 显示索引含本文件；`files --filter` 报告 586 行、65 个符号；`node --file` 阅读了完整源文件，并读取 `lib.rs`、`memtable_predicate_extractor.rs` 及测试源码。对 `ExtractPredicates`、`ListSchemasAndTables`、`ListColumns`、`ListIndexes` 的精确 callers/callees 查询没有输出，因此相关生产调用没有被宣称为已验证。
- 源与配置：`pkg/planner/core/memtable_infoschema_extractor.rs`、`pkg/planner/core/lib.rs:154,218`、`pkg/planner/core/Cargo.toml`、`pkg/planner/core/memtable_predicate_extractor.rs:16-50`。
- Go 对照：`pkg/planner/core/memtable_infoschema_extractor.go:91-300,306-635,700-1040`；生产 Go 构造分派见 `pkg/planner/core/logical_plan_builder.go:5503-5531`。
- Rust 测试：`pkg/planner/core/memtable_extractors/memtable_extractors_test.rs:17-99,263-350,435-522`，`pkg/planner/core/operator/logicalop/logicalop_test/logical_mem_table_predicate_extractor_test.rs:355-427`，`pkg/planner/core/panicrisk_regression_test.rs:51-99`，`pkg/planner/core/tests/extractor/memtable_infoschema_extractor_test.rs:505-678`。
- 未运行 Cargo 或代码测试，符合本任务纯文档范围。交付前使用任务指定的命令验证文档存在且恰有 11 个固定二级标题，并人工复核所有“已支持”结论均能回指上述源码或测试。
