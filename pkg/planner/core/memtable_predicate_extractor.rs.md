# `pkg/planner/core/memtable_predicate_extractor.rs`

## 文件定位

本文件是 planner/core 中诊断类、集群类和若干 `INFORMATION_SCHEMA` 内存表的谓词抽取实现。它把调用方已经归一化成 `Predicate` 的过滤条件转成更适合远端请求或表读取器消费的集合、时间范围、正则模式及跳过标志，并返回仍必须由 SQL 层计算的谓词。源码由 `pkg/planner/core/lib.rs` 作为私有模块载入后整体 `pub use`；同时又被 `pkg/planner/core/memtable_extractors/lib.rs` 用 `#[path = "../memtable_predicate_extractor.rs"]` 载入并导出，因此同一实现服务于 `astersql-planner-core` 和更窄的 `astersql-planner-core-memtable-extractors` crate。

需要区分两个边界：本文件公开的是简化的 `Predicate`/`PredicateValue` API 和各具体抽取器的 `ExtractPredicates` 方法；它没有直接实现 `pkg/planner/core/operator/logicalop/logical_mem_table.rs` 中接收 `Schema`、`NameSlice`、完整 `Expression` 的 `MemTablePredicateExtractor` trait。因此当前可确认的直接使用面主要是两个 crate 的导出、相邻抽取器以及独立测试；不能仅凭同名类型推断它已经完整接入 `LogicalMemTable::PredicatePushDown`。RustCodeGraph 能定位该 trait 和 `PredicatePushDown -> Extract` 主链，但索引的文件清单没有收录本目标文件，目标文件内部关系由源码与 `rg` 补证。

## 核心职责

1. 定义抽取阶段的值和谓词中间表示：`PredicateValue` 覆盖字符串、有符号/无符号整数、浮点数和布尔值；`Predicate` 覆盖等值、`IN`、`LIKE`/`ILIKE`、`OR`、正则和四种范围比较；`LikeEscape` 区分常量、缺失、动态、延迟和参数化 ESCAPE。
2. 提供共用抽取原语：大小写归一化、值类型转换、同列多个 CNF 条件求交、闭区间时间范围合并，以及 LIKE 到正则的编译。
3. 为集群组件、日志、指标、巡检、热点 Region、慢查询、表空间统计、TiFlash、语句摘要和 TiKV Region 表维护专用过滤状态。
4. 通过 `SkipRequest`/`SkipInspection` 表示条件交集为空或时间区间矛盾，使下游可以完全避免无结果的远端或内存表读取。
5. 生成稳定、可读的 `ExplainInfo`，让提取出的范围和集合可进入计划说明；`BTreeSet`/`BTreeMap` 同时保证去重和确定性排序。

本文件只做计划期的纯内存分析，不访问 PD/TiKV/Prometheus，也不读取慢日志；实际执行器消费这些状态的接线不在本文件内。

## 主要符号

- `PredicateValue`、`Predicate`、`LikeEscape`：简化表达式模型。只有字段名匹配且值类型可转换的条件才被消费；无法安全处理的条件保留给上层复核。
- `extract_like_pattern`：支持单个 `LIKE`、常量 ESCAPE 的 `LIKE`/`ILIKE`、字符串等值、正则以及可完整处理的 `OR`。返回 `(pattern, prefilter)`；`prefilter=true` 表示远端模式只可缩小候选集，原标量谓词必须保留。`to_lower=true` 时拒绝 OR 合并；动态、延迟、参数化或缺失 ESCAPE 不会被抽取。
- `strings_with_case`、`strings`、`i64s`、`bools`、`values`：把等值/IN 的原始值变成有序集合。`i64s` 拒绝超过 `i64::MAX` 的 `u64`；`bools` 将整数 `1` 映射为真，其余整数映射为假。
- `intersect`：同一字段首次赋值，后续条件做集合交集，是识别互斥谓词的核心不变量。
- `extract_time_range`：消费指定字段的 `Eq/Ge/Gt/Le/Lt`，用毫秒整数表示闭区间；严格上下界分别饱和加一/减一，`0` 表示该侧未设置。
- `extract_string_set_field`：组合字段匹配、字符串转换和求交，返回剩余谓词、集合及空交集标志。
- `ClusterTableExtractor`、`ClusterLogTableExtractor`：分别抽取节点类型/实例，以及日志时间、消息模式和日志级别。
- `MetricTableExtractor`、`MetricSummaryTableExtractor`：处理指标标签、分位数、时间范围和指标名。指标标签会写入 `LabelConditions`，同时保留原谓词供 SQL 层校验；`value` 不作为标签。
- `InspectionResultTableExtractor`、`InspectionSummaryTableExtractor`、`InspectionRuleTableExtractor`：处理巡检规则、项目、指标名、分位数和类型；前者由 `string_filter_extractor!` 宏生成主体。
- `HotRegionsHistoryTableExtractor`：抽取更新时间、Region/Store/Peer ID、leader/learner 和读写类型；缺少角色或类型条件时补齐全部合法默认集合。`HotRegionTypeRead`、`HotRegionTypeWrite` 是默认类型常量。
- `TimeRange`、`SlowQueryExtractor`：保存闭区间时间提示，以及只取最小非零行数上限的 `SetRowLimitHint` 和降序提示 `SetDesc`。
- `TableStorageStatsExtractor`、`TiFlashSystemTableExtractor`：分别抽取 schema/table，以及 TiFlash 实例、TiDB database/table。
- `StatementsSummaryExtractor`：抽取 digest，并从 `summary_end_time` 下界与 `summary_begin_time` 上界形成粗粒度重叠区间。两个 DATETIME 边界常量及 `formatStatementsSummaryTime` 负责开放区间的展示。
- `TikvRegionPeersExtractor`、`TiKVRegionStatusExtractor`：抽取 Region/Store ID 或 table ID；后者通过 `GetTablesID` 输出排序后的表 ID。

所有结构体和方法沿用 Go 版公开命名风格（例如 `SkipRequest`、`ExtractPredicates`、`ExplainInfo`），crate 根通过 `#![allow(non_snake_case)]` 接受这一风格。

## 执行流程

典型调用按以下阶段发生：

1. 上游把 SQL 表达式转为本文件的 `Predicate`。本文件本身不负责列 ID 到列名的解析，也不求值运行期参数。
2. 具体 `ExtractPredicates` 首先重置本次抽取拥有的状态；`SlowQueryExtractor` 是例外，它保留 `Limit` 和 `Desc` 提示，只清理时间抽取状态。
3. 对每个可识别列调用 `values` 和类型转换 helper。第一次条件建立集合，后续同列条件由 `intersect` 求交；不匹配的字段、类型或操作符原样进入 `remaining`。
4. 时间字段交给 `extract_time_range` 合并；`>`/`<` 被转换为毫秒粒度闭区间。LIKE 类条件由 `extract_like_pattern` 编译；若仅能安全地做预过滤，则模式被记录但谓词仍留在 `remaining`。
5. 如果任一已初始化集合变空，或闭区间满足 `end != 0 && start > end`，抽取器设置跳过标志，并通常返回空剩余列表，因为整个扫描已确定无结果。
6. 否则返回未消费或需要二次校验的谓词。读取器使用结构体字段缩小远端请求/本地扫描，SQL 层继续计算返回列表以保持语义正确。
7. `ExplainInfo` 按有序集合生成计划摘要；空过滤通常生成空字符串，跳过状态生成 `skip_request` 或 `skip_inspection`。

几个特殊流程值得单独注意：`MetricSummaryTableExtractor` 故意在原始谓词列表上抽取 `metrics_name`，因此返回值仍含 quantile 条件；`StatementsSummaryExtractor` 计算粗时间范围时保留时间谓词，以兼容只识别 digest 的旧读取路径；`ClusterLogTableExtractor` 对预过滤 LIKE 保留标量复核；热点 Region 在没有显式角色/类型过滤时填入 `{false,true}` 和 `{read,write}`。

## 数据与状态

抽取结果全部保存在调用方拥有的普通结构体中。字符串和数值集合使用 `BTreeSet`，标签映射使用 `BTreeMap<String, BTreeSet<String>>`，所以重复值被消除，Explain 和 PromQL 输出顺序稳定。同一字段的多个 AND 条件不是累加而是取交集；“从未出现该字段”和“出现但交集为空”由局部 `initialized` 标志区分，后者才触发跳过。

时间统一使用 `i64` 毫秒及闭区间。通用 helper 以 `0` 表示无界；`SlowQueryExtractor` 将无界端转为 `i64::MIN/MAX`，`StatementsSummaryExtractor` 则使用 MySQL DATETIME 的毫秒上下界 `STATEMENTS_SUMMARY_MIN_DATETIME_MS` 与 `STATEMENTS_SUMMARY_MAX_DATETIME_MS`。严格边界使用饱和运算，避免极值溢出。

大小写策略按字段语义不同：节点类型、日志级别、巡检维度、schema/table 和指标名通常转小写；实例地址、digest、TiFlash 实例和一般指标标签保留原大小写。PromQL 由 `GetMetricTablePromQL` 直接拼接已抽取的标签名和值；该函数不负责额外转义，调用方扩展值语法时必须同步处理安全性。

`SlowQueryExtractor::Limit` 和 `Desc` 是跨 `ExtractPredicates` 调用保存的执行提示：limit 只会收紧为更小的非零值，desc 由最后一次设置决定。其余抽取器通常以 `*self = Self::default()` 开始，避免上一次计划状态泄漏。

## 依赖与调用关系

- 标准库依赖只有 `BTreeMap`、`BTreeSet` 和基础转换/格式化能力。
- 唯一直接外部算法依赖是 `stringutil_dependency::string_util::CompileLike2Regexp`，由 `pkg/planner/core/Cargo.toml` 和 `pkg/planner/core/memtable_extractors/Cargo.toml` 分别映射到 `pkg/util/stringutil`。
- `pkg/planner/core/lib.rs` 声明并再导出该模块；`pkg/planner/core/memtable_extractors/lib.rs` 以 path 模块复用同一源码。后者的 crate manifest 仅声明 `stringutil`、`model`、`parser-ast`，但本文件当前实际只直接使用 `stringutil`。
- `pkg/planner/core/memtable_infoschema_extractor.rs` 复用本文件的 `Predicate`、`PredicateValue`、`LikeEscape` 与 LIKE 编译语义。
- RustCodeGraph 显示完整逻辑计划入口是 `LogicalMemTable::PredicatePushDown`，其下游为 trait 对象的 `Extract`；本文件没有该 trait impl，因而当前不存在可由静态图确认的 `PredicatePushDown -> 本文件具体 ExtractPredicates` 调用边。
- 非测试 Rust 搜索没有找到这些 planner 抽取器结构在其他生产文件中的直接实例化。`pkg/executor/metrics_reader.rs` 和 `pkg/executor/stmtsummary.rs` 存在同名但不同定义的执行器侧结构，不能视为本文件类型的调用者。
- 直接行为覆盖位于 `pkg/planner/core/memtable_predicate_extractor_test.rs`、`pkg/planner/core/memtable_extractors/memtable_extractors_test.rs` 和 `pkg/planner/core/operator/logicalop/logicalop_test/logical_mem_table_predicate_extractor_test.rs`；执行层另有 `pkg/executor/memtable_reader_test.rs` 使用公开的日志抽取器验证预过滤再校验场景。

## 错误处理与边界

API 不返回 `Result`，所有“不支持/无法安全转换”都走保守退化路径：谓词不被消费，留给 SQL 层执行。例子包括非字符串集合、超出 `i64` 的 `u64`、非数值时间、动态/参数化 ESCAPE、字段名不匹配，以及无法完整合并的 OR LIKE。这样会损失下推收益，但不应损失过滤正确性。

确定无结果的条件通过跳过标志表达，而不是错误：同一列等值/IN 交集为空、显式空 IN、时间下界大于上界都会触发。多数抽取器在跳过时返回空列表；这依赖下游尊重跳过标志，扩展调用链时必须同时验证该契约。

`extract_like_pattern` 的 `prefilter` 是重要边界：大小写折叠的普通 LIKE、在未折叠输入上处理 ILIKE 等情况可能产生比 SQL 谓词更宽的正则，必须保留原谓词复核。等值转正则时会转义正则元字符并加 `^...$` 锚点。`need_regexp=false` 返回原始展示模式而非编译正则。

当前简化 Rust 版没有 Go helper 的表达式求值错误、时区解析错误和慢日志 row-key 解码错误通道；这些属于移植差异，不应在此层伪造成功。`GetMetricTablePromQL` 也没有对标签值中的引号或反斜杠做独立转义，这是新增输入形式时的兼容/安全审查点。

## 并发与资源生命周期

文件中没有线程、异步任务、锁、通道、事务、网络连接或文件句柄。抽取器通过 `&mut self` 串行更新，集合和字符串均为自有数据；编译期 trait 自动属性以字段类型为准，但本文件没有承诺跨线程共享同一个可变实例。

资源生命周期限于一次计划抽取及随后执行/Explain 消费。大多数 `ExtractPredicates` 每次重置结构，确保计划复用时不残留旧条件；`SlowQueryExtractor` 有意保留行数和排序提示，调用者若跨无关查询复用同一实例必须显式重新构造或重设。LIKE 编译和集合求交会分配新字符串/集合，复杂度主要随谓词数和集合大小增长；`BTreeSet` 操作为对数级，OR 模式拼接会随分支总长度增长。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/memtable_predicate_extractor.go`。Rust 保留了 Go 版的类型分组、字段命名、同列集合求交、大小写策略、跳过语义、Explain 形态及若干刻意兼容行为：指标 `value` 不作为标签；数值 quantile 不做 `[0,1]` 范围拒绝；Metric Summary 的 metrics_name 从原谓词提取；Statements Summary 的粗时间条件仍保留给旧路径；热点 Region 的角色和读写类型有全量默认值。

但两者并非一比一完整移植：

- Go `extractHelper` 从完整 `expression.Expression`、schema 和字段名定位列，能求值常量、处理准备语句并记录被下推的表达式；Rust 从调用方提供的简化 `Predicate` 开始，尚缺该桥接层。
- Go 时间抽取使用 session/statement 时区和纳秒时间值并转为 `time.Time`；Rust 使用调用方给出的毫秒 `i64`，不持有时区。
- Go `SlowQueryExtractor` 还能从 coprocessor key range 解码时间；Rust 只有显式 time 谓词路径。
- Go 的 Region/Store/Peer ID 主要是无符号整数；Rust 状态采用 `i64`，只接受能安全落入 `i64` 的 `u64`。
- Go `TiKVRegionStatusExtractor` 遇到 table_id 解析失败会记录错误并放弃抽取；Rust 的类型化输入在更早阶段过滤非整数，没有日志分支。
- Go Explain 可从 `PhysicalMemTable` 取得 session 时区和慢日志文件；Rust Explain 由调用方传入文件名或直接格式化毫秒值。

因此本文件适合作为已经类型化后的抽取核心和移植语义测试面；若要声称完整替代 Go 规划链，还需要实现表达式桥接、trait 接线以及执行器消费验证。

## 扩展指南

新增字段过滤时，优先复用 `values`、类型转换和 `intersect`，并明确三件事：列名是否忽略大小写、值是否规范化大小写、抽取后是否仍需保留原谓词。新增范围类型时不要复用 `0` 作为合法值与“未设置”的双重含义，除非保持现有调用契约；新增时间精度时必须同步严格边界的加减单位和极值饱和行为。

新增 LIKE 能力应从 `extract_like_pattern` 接入，并为 ESCAPE 生命周期、OR 完整性、ILIKE 大小写折叠和 `prefilter` 复核分别补测试。绝不能为了扩大下推而移除需要 SQL 层二次计算的谓词。

新增抽取器时应：定义独立状态结构；每次抽取重置查询相关字段；空交集设置跳过标志；提供确定性 `ExplainInfo`；在 `pkg/planner/core/memtable_extractors/memtable_extractors_test.rs` 或同目录独立测试文件增加边界用例。若接入真实逻辑计划，还需在 `logical_mem_table.rs` 的 trait 边界实现适配，并验证物理计划/执行器确实消费状态，而不是只让零调用方的单元测试通过。

对照 Go 迁移时必须逐项检查 `pkg/planner/core/memtable_predicate_extractor.go`，尤其是表达式常量求值、时区、prepared statement、慢日志 key range 和 Explain 上下文。性能方面关注超长 IN 列表的多次集合复制、OR 正则拼接和标签笛卡尔式输出；兼容方面关注大小写、整数符号、时间单位和保留谓词规则。

## 验证依据

- 源码全量阅读：`pkg/planner/core/memtable_predicate_extractor.rs`，确认 1200 行内的枚举、helper、宏、14 类抽取器、常量和方法；文件无条件编译分支。
- crate 边界：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`、`pkg/planner/core/memtable_extractors/Cargo.toml`、`pkg/planner/core/memtable_extractors/lib.rs`。
- 规划接口与入口：`pkg/planner/core/operator/logicalop/logical_mem_table.rs`、`pkg/planner/core/base/misc_base.rs`。RustCodeGraph `query/node` 定位了两个同名 Rust trait，并显示 `LogicalMemTable::PredicatePushDown` 调用 trait 的 `Extract`；对目标路径执行 `files --filter` 和内部 helper 精确查询均无结果，故目标内部事实按技能规则由源码/`rg` 核验。
- Go 对照：`pkg/planner/core/memtable_predicate_extractor.go` 的 `extractHelper`、全部同名抽取器、`Extract`、`ExplainInfo` 及 Slow Query/Statements Summary 辅助流程。
- Rust 测试：`pkg/planner/core/memtable_predicate_extractor_test.rs` 验证 value 排除、quantile、Metric Summary 返回谓词和 LIKE 预过滤；`pkg/planner/core/memtable_extractors/memtable_extractors_test.rs` 验证状态重置、交集、时间边界、默认值、开放时间范围、ESCAPE/ILIKE/OR；`pkg/planner/core/operator/logicalop/logicalop_test/logical_mem_table_predicate_extractor_test.rs` 覆盖各主要抽取器的 Go 对齐行为；`pkg/executor/memtable_reader_test.rs` 覆盖日志预过滤后的标量复核。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只执行任务指定的 11 章节结构检查，并人工复核所有生产代码搜索结论均区分了同名执行器类型和真实调用边。
