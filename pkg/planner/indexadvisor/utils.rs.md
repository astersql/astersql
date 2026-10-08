# `pkg/planner/indexadvisor/utils.rs`

## 文件定位

本文件是 [`utils.rs`](./utils.rs) 的逻辑说明。它属于 `astersql-planner-indexadvisor` crate 的 SQL 辅助层，由 `lib.rs` 以公开模块 `utils` 暴露。它位于用户 SQL 与索引推荐算法之间：先把 SQL 归一化并补全 schema，过滤不适合分析的查询，再从谓词、排序和分组位置提取候选列；算法枚举索引集合时，又通过本文件统一计算 workload 代价。生产主链可在 `indexadvisor.rs::advise_indexes_for_sql` 和 `algorithm.rs::advise_indexes` 中核对。

`Cargo.toml` 将该 crate 映射到 Go 包 `pkg/planner/indexadvisor`。完整 TiDB parser、domain、infoschema、sessionctx 等依赖只声明在 `cfg(windows)` 下；当前 `utils.rs` 在通用目标上只依赖标准库以及本 crate 的 `model`、`optimizer`，因此用内部轻量词法器模拟 Go AST 访问器的关注边界，而不是提供完整 SQL parser。

## 核心职责

- SQL 入口与指纹：`parse_one_sql` 做最低限度的语句检查；`normalize_digest` 删除两类注释、折叠空白、统一大小写，并用 `?` 替换字符串和数字字面量后生成 digest。
- SQL 结构提取：`collect_table_names` 收集实体表并排除 CTE；`collect_select_columns`、`collect_order_by_columns`、`collect_dnf_columns` 为单表场景提取 SELECT、ORDER BY 和合格 DNF 分支中的列。
- workload 清洗：`restore_schema_name` 补默认 schema 并重写未限定表名；`filter_invalid_queries` 以优化器能否取得计划代价判定有效性；`filter_system_queries` 排除无表语句及访问系统 schema 的语句。
- 候选列与代价：`collect_indexable_columns` 只扫描范围/等值谓词、`IN`、`BETWEEN`、`ORDER BY`、`GROUP BY`，再经 `Optimizer` 消歧和类型过滤；`evaluate_index_set_cost` 计算按查询频次加权的索引集合成本及稳定平局键。

这些职责共同限制“哪些 SQL 和列可以进入候选搜索”，但不负责生成索引排列、检查已有索引前缀或最终推荐展示；这些分别属于 `algorithm.rs`、`optimizer.rs` 和 `indexadvisor.rs`。

## 主要符号

公开函数如下，文件中没有公开类型、trait、常量、`impl` 或条件编译项：

- `parse_one_sql(&str) -> Result<String, String>`：接受白名单中的首关键字，并验证括号与引号闭合；成功时返回去除首尾空白的原 SQL。它不构造 AST，也不验证完整语法。
- `normalize_digest(&str) -> (String, String)`：返回规范化 SQL 和 `DefaultHasher` 的 16 位十六进制结果。`indexadvisor.rs::advise_indexes_for_sql` 用 digest 聚合同形 SQL 并累计 `Query::frequency`。
- `collect_table_names(&str, &str) -> Result<Vec<String>, String>`：按 token 遍历顺序收集 `FROM`、`JOIN`、`UPDATE`、`INTO` 后的表名，未限定名加默认 schema，CTE 名不作为实体表返回。
- `collect_select_columns(&Query) -> Result<BTreeSet<Column>, String>`：仅在恰有一个表时收集 SELECT 列；跳过 `*`、关键字、函数名和字段别名。
- `collect_order_by_columns(&Query) -> Result<Vec<Column>, String>`：仅接受单表且 ORDER BY 项均为普通列的情况；任何不支持的表达式使整个结果为空。
- `collect_dnf_columns(&Query) -> Result<BTreeSet<Column>, String>`：从 WHERE 的顶层 CNF 项中寻找含多个 OR 分支的组，并仅接受每个分支都是“列 = 常量”或“常量 = 列”。
- `restore_schema_name(&str, BTreeSet<Query>, bool) -> Result<BTreeSet<Query>, String>`：补 `Query::schema_name`、校验并改写 SQL 表引用；`ignore_error` 决定跳过坏查询还是返回错误。
- `filter_invalid_queries(&dyn Optimizer, BTreeSet<Query>, bool)`：对每条 SQL 以空假设索引调用 `Optimizer::query_plan_cost`，保留成功项。
- `filter_system_queries(BTreeSet<Query>, bool)`：排除不引用表的 SQL，以及任一表属于 `mysql`、`information_schema`、`performance_schema`、`metrics_schema`、`sys` 的 SQL。
- `collect_indexable_columns_for_query_set(&dyn Optimizer, &BTreeSet<Query>)`：逐查询调用 `collect_indexable_columns` 并取并集；当前生产主链直接逐查询调用后者，本函数仍是公开的批量门面。
- `collect_indexable_columns(&Query, &dyn Optimizer)`：提取相关列名，通过 `possible_columns` 找出可能实体列，再通过 `column_type` 和 `is_indexable_column_type` 过滤。
- `evaluate_index_set_cost(&BTreeSet<Query>, &dyn Optimizer, &BTreeSet<Index>)`：逐查询调用 `query_plan_cost`，乘 `frequency` 后求和，并生成总索引列数与排序后的 `Index::key` 拼接串。
- `is_indexable_column_type(&FieldType) -> bool`：拒绝 `Json`、`Blob`、`Geometry`、`Vector`，接受其余 Rust `FieldType`。

私有辅助函数分为四组：布尔表达式切分（`trim_parentheses`、`matching_outer_parentheses`、`split_boolean_terms`），SQL/token 边界（`tokenize`、`balanced_delimiters`、`clause_end`），名称识别与改写（`is_identifier`、`normalize_identifier`、`split_table`、`cte_names`、`qualify_table_references`），以及候选判断（`relevant_column_names`、`is_column_token`、`is_constant`、`is_sql_keyword`、`is_system_schema`）。

## 执行流程

1. `advise_indexes_for_sql` 对输入 SQL 调用 `normalize_digest`，按 digest 聚合为 `BTreeMap<String, Query>`，相同规范化语句累加频次。
2. 聚合结果先进入 `restore_schema_name`。每条查询补默认 schema，经 `balanced_delimiters` 和 `parse_one_sql` 检查后，由 `qualify_table_references` 给未限定的实体表加 schema；坏 SQL 在主链传入的 `ignore_error = true` 下被跳过。
3. `filter_system_queries` 调用 `collect_table_names`：无表语句、系统表语句和解析失败语句被过滤，剩余查询才进入候选提取。
4. `collect_indexable_columns` 先取得查询涉及的 schema，再由 `relevant_column_names` 扫描比较运算符、`IN`/`BETWEEN` 以及 ORDER/GROUP BY 区段。限定列只查询指定 schema；未限定列会在默认 schema 和 SQL 中出现的 schema 范围内调用 `Optimizer::possible_columns`。
5. 每个可能实体列经 `Optimizer::column_type` 和 `is_indexable_column_type` 筛选，放入 `BTreeSet<Column>` 去重并稳定排序。`advise_indexes` 用这些列构造和扩展候选索引。
6. 算法在单查询候选筛选、组合比较和贪心补充阶段反复调用 `evaluate_index_set_cost`。该函数把每条查询在给定假设索引集合下的计划代价乘出现频次，返回 `IndexSetCost`；其列数和索引键供 `IndexSetCost::less` 在代价接近时确定性裁决。

独立的 SELECT、ORDER BY、DNF 收集函数没有出现在当前生产主链中，但与 Go 包公开辅助函数对应，并由 `utils_test.rs` 直接验证。

## 数据与状态

本文件自身不保存全局或可变共享状态。输入输出主要是 `model.rs` 中的值类型：`Query` 携带 SQL、默认 schema 和频次，`Column` 以 `schema.table.column` 标识实体列，`Index` 保存有序索引列，`IndexSetCost` 保存加权代价、总列数和平局键。

集合主要使用 `BTreeSet`，因此去重和遍历顺序由数据模型的 `Ord` 决定，生成的候选集合与代价键可重复比较；`relevant_column_names`、CTE 集合使用 `HashSet`，但结果在进入 `BTreeSet<Column>` 后恢复确定顺序。表名使用 `Vec<String>` 保留词法遍历次序，因为 SELECT/ORDER/DNF 辅助函数以“恰好一个表”为前置条件。

`normalize_digest` 的 hash 只用作当前进程内 workload 分组键。标准库 `DefaultHasher` 不承诺跨 Rust 版本或实现的长期稳定格式，因此不能把该 digest 当作与 Go `parser.NormalizeDigest` 完全相同、可持久化交换的协议值。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 文件关系和精准源码搜索共同确认：

- `indexadvisor.rs::advise_indexes_for_sql` 调用 `normalize_digest`、`restore_schema_name`、`filter_system_queries`、`collect_indexable_columns`；`prepare_recommendations` 也调用 `normalize_digest` 生成推荐理由中的规范化 SQL。
- `algorithm.rs::advise_indexes`、`select_index_candidates`、`choose_best_indexes` 调用 `collect_indexable_columns` 或 `evaluate_index_set_cost`，把本文件接入候选搜索和成本比较。
- `utils_test.rs` 直接覆盖解析、表名、SELECT/ORDER/DNF、schema 恢复、系统查询过滤和候选列消歧。
- RustCodeGraph 报告该文件被 `utils_test.rs` 和 `pkg/planner/util/path.rs` 使用；后者属于索引返回的文件级关系，精准符号搜索未发现它调用本文件公开函数，因此不能据此声称存在额外生产调用边。

下游依赖包括 `model::{Query, Column, Index, IndexSetCost}`、`optimizer::{Optimizer, FieldType}` 以及标准库集合和哈希接口。`Optimizer` 是关键反转边界：`possible_columns`/`column_type` 提供 schema 元数据，`query_plan_cost` 提供 what-if 计划成本；本文件不直接访问 catalog、session 或存储。

## 错误处理与边界

所有可失败的公开函数使用 `Result<_, String>`。结构提取首先传播 `parse_one_sql` 或表名拆分错误；优化器错误按函数语义处理：`filter_invalid_queries` 可跳过或立即传播，`evaluate_index_set_cost` 传播首个代价错误，而 `collect_indexable_columns` 对单个 schema 的 `possible_columns` 错误和单列 `column_type` 错误采取跳过策略，以便其他候选继续参与。

轻量实现的边界必须明确：

- `parse_one_sql` 只检查首 token 白名单和定界符，不会拒绝所有语法错误；`tokenize` 也不是完整 TiDB lexer。调用方不能把成功理解为 TiDB parser 已接受该 SQL。
- 表名、CTE、别名、派生表和子查询依靠 token 状态机处理。现有测试证明逗号连接、简单别名、CTE 和嵌套 FROM 的给定案例，但不能推导为覆盖完整 MySQL/TiDB 语法。
- `collect_select_columns`、`collect_order_by_columns`、`collect_dnf_columns` 明确限制为单表或简单表达式；遇到不支持形状通常返回空集合，而非错误。
- 未限定列按“可能 schema 中所有同名列”展开。`utils_test.rs::utility_collects_same_named_columns_from_all_schema_tables` 证明这是一种保守消歧语义，可能产生多个候选，而不是绑定到 SQL 别名指向的唯一表。
- Rust 类型过滤仅排除四类 `FieldType`；Go `isIndexableColumnType` 则只接受数值/时间类型以及长度不超过 512 的字符串，并对 `nil` 返回 false。两者当前不是严格等价，应视为迁移差异，扩展时不能仅依据函数名假定一致。
- SQL 重写通过 `output.join(" ")` 重排空白，适合后续分析而非保真展示；引号内容虽作为单 token 保留，但完整转义和 dialect 语义仍受轻量词法器限制。

## 并发与资源生命周期

本文件没有锁、通道、异步任务、事务或 I/O 资源。所有集合和字符串均为函数局部所有权，`restore_schema_name`、过滤函数消费输入集合并构造新集合；借用的 `&dyn Optimizer` 生命周期由调用方管理。

函数本身没有内部共享可变状态，但线程安全性仍取决于传入的 `Optimizer` 实现。`evaluate_index_set_cost` 和查询过滤按集合顺序串行调用优化器，不并发发起计划估算，也不负责建立或清理假设索引状态；该契约属于 `optimizer.rs`。单条 SQL 的时间复杂度主要由 token 数线性扫描，以及候选列对 schema/元数据的查询次数构成；算法层会多次调用代价函数，因此 `query_plan_cost` 是更显著的成本来源。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/indexadvisor/utils.go`，相关 Go 测试为 `utils_test.go`。主要映射为：

- `parse_one_sql` ↔ `ParseOneSQL`，`normalize_digest` ↔ `NormalizeDigest`；Go 返回真实 AST 并使用 parser digest，Rust 返回字符串并使用轻量规范化/hash。
- `collect_table_names` ↔ `CollectTableNamesFromQuery`；Rust 的 `cte_names` 和 token 状态机模拟 Go `ast.Walk` 对 `WithClause`、`TableName` 的访问。
- 三个 `collect_*_columns` ↔ Go 的 SELECT、ORDER BY、DNF 收集函数；Rust 的括号裁剪和 AND/OR 切分对应 Go 的 `flattenCNF`、`flattenDNF`、`flattenColEQConst`。
- `restore_schema_name` ↔ `RestoreSchemaName`，但 Go 使用 `RestoreWithDefaultDB` 对 AST 恢复，Rust 只改写识别到的表 token。
- `filter_invalid_queries`、`filter_system_queries` ↔ `FilterInvalidQueries`、`FilterSQLAccessingSystemTables`；两边都有 `ignoreErr` 跳过/传播分支和无表语句过滤。
- 候选列提取对应 `CollectIndexableColumnsForQuerySet`/`CollectIndexableColumnsFromQuery`。Go 通过 AST 精确访问 Group/Order/Between/In/比较表达式，Rust 通过 token 邻接近似；两边都借助 optimizer 消歧，并在元数据查询失败时继续其他候选。
- `evaluate_index_set_cost` ↔ Go 私有 `evaluateIndexSetCost`：两边均按频次加权、累计索引列数、排序索引键后拼接。Go 的 `chooseBestIndexSet` 在 Rust 中由 `algorithm.rs` 的组合选择逻辑承接，不在本文件中。

Rust 文件没有移植 Go 文件末尾的 `exec` SQL 执行资源管理逻辑；当前 Rust indexadvisor 通过 `Optimizer` 抽象和内存实现工作。不能把该缺失描述为已由本文件支持。

## 扩展指南

- 增加 SQL 语法支持时，先判断是否应扩展 `tokenize`/状态机，还是引入真实 parser 边界。若继续轻量路线，应同步检查 `balanced_delimiters`、`collect_table_names`、`qualify_table_references` 和 `clause_end`，避免只让提取支持新语法而 schema 恢复仍误写。
- 扩展候选谓词时修改 `relevant_column_names` 和 `is_column_token`；必须在独立文件 `utils_test.rs` 增加正反例，证明 SELECT 投影或函数名不会被误收集。不要把测试嵌入生产 `utils.rs`。
- 改变单表 SELECT、ORDER 或 DNF 规则时，分别修改对应公开函数及布尔切分辅助函数，并同步 `utils_test.rs` 与 Go `utils_test.go` 的意图；表达式整体回退为空是兼容行为，不能无意改为部分结果。
- 调整可索引类型时同时核对 `optimizer.rs::FieldType` 和 Go `isIndexableColumnType` 的允许矩阵，尤其是字符串长度、未知类型和复杂类型；这是当前最明显的兼容风险点。
- 修改 digest 必须考虑 `advise_indexes_for_sql` 的聚合频次与 `prepare_recommendations` 的展示，并增加同形 SQL/不同字面量、注释和大小写测试。若 digest 需要持久化或跨语言一致，应采用明确稳定的算法而非依赖 `DefaultHasher`。
- 调整代价汇总时同步验证频次乘法、优化器错误传播、空索引集合、索引键排序以及 `IndexSetCost::less` 的三级比较。算法可能对每个候选组合重复调用该函数，新增昂贵处理会放大搜索开销。

## 验证依据

- RustCodeGraph：`status` 显示索引有效（11,467 文件、307,296 节点、1,848,419 边）；`files --filter pkg/planner/indexadvisor` 确认本模块 27 个已索引 Go/Rust 文件；`node --file pkg/planner/indexadvisor/utils.rs --offset 1/500` 读取了目标文件 784 行及文件级使用关系；`query --kind function` 确认 13 个公开函数均定位于本文件。批量 `callers/callees` 命令在 30 秒窗口内未返回明细，调用边改由精准符号搜索复核，未据缺失输出作推断。
- 已读生产源码：`pkg/planner/indexadvisor/utils.rs`、`lib.rs`、`model.rs`、`optimizer.rs`、`indexadvisor.rs`、`algorithm.rs`。
- 已读边界配置：`pkg/planner/indexadvisor/Cargo.toml`，确认 crate 名、`lib.rs` 入口、Go package 映射以及仅 Windows 启用的外部 AsterSQL 依赖。
- 已读测试：`pkg/planner/indexadvisor/utils_test.rs` 与 `utils_test.go`。Rust 用例覆盖限定/默认 schema、逗号 join、子查询、CTE、SELECT 别名、ORDER BY、CNF 内 DNF、反向等值、无表过滤和同名列保守展开；Go 用例提供 AST 版原始语义及 invalid/system 查询边界。
- 已读 Go 对照：`pkg/planner/indexadvisor/utils.go`，核对 parser/AST visitor、schema 恢复、系统表判断、类型白名单、优化器错误处理和加权代价公式。
- 调用边搜索：对 13 个公开函数在 `pkg/planner/indexadvisor/*.rs` 中执行精准 `rg`，确认生产调用集中于 `indexadvisor.rs` 和 `algorithm.rs`，测试调用集中于独立 `utils_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰含 11 个固定二级标题，并人工复核没有把轻量词法器描述为完整 AST/parser 实现。
