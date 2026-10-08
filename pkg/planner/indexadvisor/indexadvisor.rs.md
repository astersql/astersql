# `pkg/planner/indexadvisor/indexadvisor.rs`

源文件：[`indexadvisor.rs`](./indexadvisor.rs)

## 文件定位

本文件属于 `astersql-planner-indexadvisor` crate，是索引顾问面向“调用者已经提供 SQL 文本”的编排入口。crate 由同目录 [`lib.rs`](./lib.rs) 暴露 `indexadvisor` 模块；[`Cargo.toml`](./Cargo.toml) 的 `package.metadata.porting.go-package` 将它明确映射到 Go 包 `pkg/planner/indexadvisor`。根 workspace 和 `pkg/planner/Cargo.toml` 均收录该 crate，`pkg/executor/Cargo.toml` 也声明了依赖，但当前仓库搜索与 RustCodeGraph 调用图均未找到生产 Rust 代码调用 `advise_indexes_for_sql`，已确认的调用者都在独立测试文件中。因此它目前是可复用的公开库 API，而不是已经接入 session/executor 的完整 `RECOMMEND INDEX` 服务入口。

文件顶部第 23—124 行的大段内容全部位于块注释中，是 Go 管线的机械翻译草稿，不参与编译。实际活跃实现从 `use crate::algorithm::advise_indexes` 开始，只处理显式 SQL、内存中的查询与推荐结果；它不读取 statement summary、不保存 `mysql.index_advisor_results`、不追加 session warning，也不持有 session context。

## 核心职责

1. `advise_indexes_for_sql` 把 SQL workload 规范化并按 digest 去重，重复 SQL 通过 `Query.frequency` 累计权重。
2. 它补齐默认 schema、过滤系统表查询、抽取可用于索引的谓词列，再把查询集和列集交给 [`algorithm.rs`](./algorithm.rs) 的 `advise_indexes` 搜索候选索引。
3. `prepare_recommendations` 对每个候选索引独立做 what-if 代价比较，生成名称、估算大小、筛掉无收益项，并构造对外 `Recommendation`。
4. `graceful_index_name` 按 Go 版顺序避让已有索引名：先尝试全部列，再尝试首列，最后尝试首列加 `0..29` 后缀。
5. 私有辅助函数提供加权 workload 代价、改善率、六位小数舍入、索引名存在性检查和 64 字节截断。其中 `workload_cost` 与 `improvement` 在当前文件的活跃路径中没有调用者，属于保留的内部辅助代码。

本文件刻意不承担候选枚举策略、SQL AST 分析或真实数据库访问：这些职责分别位于 `algorithm.rs`、`utils.rs` 和 `Optimizer` trait 的实现侧。

## 主要符号

- `pub fn advise_indexes_for_sql(optimizer: &dyn Optimizer, sqls: &[String], default_schema: &str, options: &AdvisorOptions) -> Result<Vec<Recommendation>, String>`：当前公开主入口。调用者注入 what-if 优化器、显式 SQL、默认 schema 和限制参数；返回稳定排序后的推荐或字符串错误。
- `pub fn prepare_recommendations(indexes: &BTreeSet<Index>, queries: &BTreeSet<Query>, optimizer: &dyn Optimizer) -> Result<Vec<Recommendation>, String>`：把算法输出转成用户可见结果。它为每个索引查询名称冲突和大小，分别计算每条查询及整个 workload 的改善率，并保留改善最大的三条查询。
- `pub fn graceful_index_name(...) -> Result<String, String>`：生成最多 64 字节的建议索引名。签名保留 `Result`，但内部 `index_name_exists` 会把元数据查询错误当作“不存在”，所以当前实现实际上不会因名称查询失败返回 `Err`。
- `fn workload_cost(...) -> Result<f64, String>`：按 `query_plan_cost × frequency` 汇总查询代价；当前无活跃调用者。
- `fn improvement(old, new) -> f64`：计算相对改善率，旧代价不为正时返回 `0.0`；当前无活跃调用者。
- `fn round(value, digits) -> f64`：用 `10^digits` 缩放后四舍五入。推荐路径固定用六位小数。
- `fn index_name_exists(...) -> bool`：调用 `Optimizer::index_name_exists`，统一把名称转成 ASCII 小写，并以 `unwrap_or(false)` 吞掉查询错误。
- `fn truncate_index_name(name) -> String`：将 UTF-8 字符串按字节长度限制到 64；若第 64 字节不在字符边界，`String::truncate(64)` 会 panic，这是非 ASCII 列名需要注意的边界。

本文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项；集合与状态均是函数局部值。

## 执行流程

`advise_indexes_for_sql` 的执行顺序如下：

1. 对每条输入 SQL 调用 `utils::normalize_digest`，得到规范化 SQL 和 digest。
2. 以 digest 为 `BTreeMap` 键：首次出现时创建 `Query { alias: digest, schema_name, text, frequency: 1 }`，再次出现则只增加频率。默认 schema 在这里转为 ASCII 小写。
3. 把 map 的值交给 `restore_schema_name(..., true)` 补齐表名前缀，再交给 `filter_system_queries(..., true)` 排除系统表访问；任一步错误都通过 `?` 原样返回。
4. 若过滤后没有查询，直接返回空推荐，而不是 Go 完整入口中的错误或 session warning。
5. 逐条调用 `collect_indexable_columns`，把结果合并到 `BTreeSet<Column>`；随后调用 `algorithm::advise_indexes`，其内部依据 `AdvisorOptions` 的最大索引数、最大宽度和 timeout 搜索候选。
6. 调用 `prepare_recommendations`。对每个索引先清理列名两侧的引号和空格，再生成索引名并估算大小；这两个元数据步骤发生在收益过滤之前。
7. 对每条查询分别求“无假设索引”和“仅有当前候选索引”的计划代价。代价乘以 `frequency` 后汇总为 workload 代价；单查询旧代价为零时，新旧代价都加 `0.1` 以避免除零。
8. 单查询改善率达到 `0.0001` 才进入 `top_impacted_queries`。该列表按改善率降序排序并截取前三项。
9. workload 总旧代价为零时同样对新旧总值加 `0.1`。总改善率低于 `0.000001`，或没有任何达到单查询阈值的查询时，丢弃该索引。
10. 用影响最大查询的规范化文本生成原因说明，组装 `Recommendation`。全部结果最后按 `(database, table, index_name)` 排序，保证稳定输出。

## 数据与状态

- 输入 workload 不会就地修改；`sqls`、`options` 和 `optimizer` 都以共享借用传入。
- `BTreeMap<String, Query>` 负责 digest 归并；键的有序性让转成查询集合时具备确定性。`BTreeSet<Query>`、`BTreeSet<Column>` 和 `BTreeSet<Index>` 同时去重并维持稳定遍历顺序。
- `Query.frequency` 是 workload 权重。相同 digest 的显式 SQL 被合并后，其频率参与 `workload_before` 和 `workload_after` 的加权累计。
- 每个候选索引独立评估：`before_indexes` 始终为空，`after_indexes` 只包含当前索引。这里评估的不是所有推荐索引组合后的联合收益。
- `Recommendation` 保存 schema、表、建议名称、去除外围引号后的列名、大小与理由、整体 workload 改善，以及最多三条受影响查询。
- 所有状态均为栈上局部集合或返回值；文件不含全局缓存、静态可变状态、数据库句柄或跨调用状态。

## 依赖与调用关系

上游方面，`lib.rs` 公开本模块。RustCodeGraph 对 `indexadvisor.rs::advise_indexes_for_sql` 的调用轨迹只列出 `indexadvisor_sql_test.rs::recommend`、`index_advisor_for_multiple_tables_obeys_go_result_limit`、`indexadvisor_test.rs::specified_sqls_are_not_limited_by_statement_summary_max_query` 和 `indexadvisor_tpch_test.rs::check_tpch_group` 等测试调用；仓库文本搜索同样没有找到生产调用。因此不能把 Cargo 依赖声明解释为已经完成运行时接线。

下游关系为：

- [`utils.rs`](./utils.rs)：`normalize_digest`、`restore_schema_name`、`filter_system_queries`、`collect_indexable_columns`，负责 SQL 规范化、schema 恢复、过滤和列识别。
- [`algorithm.rs`](./algorithm.rs)：`advise_indexes`，负责候选生成、what-if 比较、宽度扩展、数量裁剪与超时检查。
- [`optimizer.rs`](./optimizer.rs)：`Optimizer` trait 提供列/索引元数据、索引大小和假设索引下的计划代价。本文件直接使用 `index_name_exists`、`estimate_index_size`、`query_plan_cost`，间接依赖算法和工具函数使用其余方法。
- [`model.rs`](./model.rs)：定义 `Query`、`Index`、`Recommendation`、`IndexDetail`、`WorkloadImpact` 和 `ImpactedQuery`。
- [`options.rs`](./options.rs)：`AdvisorOptions` 默认限制为 5 个索引、宽度 3、最多 1000 条摘要查询和 30 秒；本入口处理显式 SQL，因此自身不使用 `max_num_query`，但把其他限制传给算法。
- 标准库 `BTreeMap`/`BTreeSet`：提供归并、去重和确定性顺序。

`Cargo.toml` 把 crate 根设为 `lib.rs`，并记录 Go 包映射。其 AsterSQL 依赖当前全部位于 `cfg(windows)` 表中；不能仅凭该清单推断本文件在所有目标平台会连接真实 session、domain 或 infoschema 服务。

## 错误处理与边界

- 规范化后的 schema 恢复、系统查询过滤、可索引列收集、候选算法、索引大小估算和计划代价计算的错误均作为 `String` 经 `?` 向上传播；函数不会补充调用阶段上下文。
- 空输入或过滤后空集合返回 `Ok(Vec::new())`。这与 Go 的 `prepareQuerySet` 在部分路径返回错误、完整入口追加空结果 warning 的行为不同。
- `prepare_recommendations` 先执行名称生成和大小估算，再做收益计算。`recommendation_checks_index_size_before_discarding_no_benefit_index` 明确验证：即使候选最终无收益，大小查询错误仍必须返回。
- `index_name_exists` 故意/沿袭 Go 语义忽略名称查询错误，可能把未知状态当作名称可用；调用方不能从 `graceful_index_name` 的 `Result` 判断该错误。
- 零代价通过给新旧代价同时加 `0.1` 避免 NaN；负代价没有专门拒绝逻辑。浮点 NaN 若从优化器进入，`total_cmp` 虽能排序，但收益比较会使相关项通常无法通过阈值。
- 推荐保留阈值有两级：单查询至少 `0.0001`，整体 workload 至少 `0.000001`；即使整体受益，只要没有单条查询达到前一阈值也会丢弃。
- `graceful_index_name` 在 32 个尝试（全列名、首列名、30 个后缀）都冲突后仍返回最后一个冲突名称；空列集合则只尝试 `idx_`。算法正常输出应含列，但公开函数本身未校验。
- 64 的限制按 UTF-8 字节实现，非 ASCII 名称可能在截断时 panic。当前 Rust 活跃入口没有 Go `recover` 对应的 panic 边界。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或显式事务。所有集合在一次同步函数调用内创建并在返回时释放；`optimizer: &dyn Optimizer` 只借用到调用结束，本文件不要求或声明 `Send`/`Sync`。

算法 timeout 由 `algorithm::advise_indexes` 使用 `AdvisorOptions.timeout` 和 `Instant` 检查，本文件的规范化、过滤、列收集以及推荐结果的逐查询代价计算不受该 timeout 直接保护。大 workload 或昂贵 `Optimizer` 实现可能在这些阶段长时间运行。

计划代价与元数据读取按索引、按查询串行执行，没有并行化。粗略看，结果整理阶段会对每个候选执行一次大小估算、若干名称存在性查询，以及每条查询两次 `query_plan_cost`；扩展时应关注 `候选数 × 查询数` 的调用量。真实数据库连接、快照一致性和事务生命周期完全由调用者提供的 `Optimizer` 决定，本文件不管理这些资源。

## 与 Go 版本的对应关系

直接对照文件是 [`indexadvisor.go`](./indexadvisor.go)。当前 Rust 活跃实现保留了 Go 的核心推荐语义：显式 SQL workload、按频率加权的代价、候选逐个独立比较、零代价保护、六位小数舍入、单查询/整体收益阈值、前三条受影响查询、索引大小先于收益过滤，以及索引名的三段回退顺序。

主要差异如下：

- Go 的公开 `AdviseIndexes` 接受 `context.Context`、`sessionctx.Context` 和 AST 选项；Rust 只接受已构造的 `Optimizer`、显式 SQL 和 `AdvisorOptions`。
- Go 在未指定 SQL 时从 `information_schema.statements_summary_history` 加载 workload，并使用 `MaxNumQuery`；Rust 活跃实现没有摘要加载路径，显式 SQL 不受 `max_num_query` 限制。
- Go 入口执行 nil 检查、日志记录和 panic recover；Rust 无这些 session 级错误边界。
- Go 会保存到 `mysql.index_advisor_results`，并在空结果时向 statement context 追加 warning；Rust 只返回内存结果。
- Go 的 `prepareRecommendation` 保持算法集合的遍历顺序；Rust 最后按数据库、表、索引名排序，使输出确定但顺序不必与 Go 完全一致。
- Rust 使用 `BTreeMap` 按 digest 归并显式 SQL；Go `prepareQuerySet` 先放入 set，完整 Go 管线的摘要聚合由 SQL 查询完成。

文件顶部注释保留了上述未移植管线的草稿，但这些注释不是实现证据。独立 Rust 测试覆盖了当前可运行子集；Go 测试还覆盖非法查询、view、massive workload、错误 current DB、TPCC/Web3、运行时长、存储和建索引语句等更完整的 session 行为，不能宣称 Rust 已全部支持。

## 扩展指南

- 若新增显式 SQL 预处理步骤，应接在 `advise_indexes_for_sql` 的 digest 归并、schema 恢复或系统表过滤对应位置，并在 [`indexadvisor_sql_test.rs`](./indexadvisor_sql_test.rs) 添加独立测试，避免把测试写入生产源文件。
- 若改变收益定义、阈值、top-N 或 reason 文本，应修改 `prepare_recommendations`，同步 [`indexadvisor_test.rs`](./indexadvisor_test.rs) 的 Go 对齐断言，并核对 Go `prepareRecommendation`，防止频率权重或“每索引独立评估”语义漂移。
- 若改变命名规则，应修改 `graceful_index_name`/`truncate_index_name`，补充冲突耗尽、空列、长 ASCII 和多字节 UTF-8 名称测试。尤其应先决定如何处理 `index_name_exists` 错误及 30 次尝试后仍冲突的兼容行为。
- 若接入 statement summary、session warning 或结果落库，不应把真实 IO 塞进当前纯函数；应增加上层编排并复用本文件的显式 workload 核心，同时分别覆盖查询加载、事务/连接释放和部分写入失败。Go `AdviseIndexes`、`prepareQuerySet`、`loadQuerySetFromStmtSummary`、`saveRecommendations` 是对照点。
- 若接入 executor，先确认 `pkg/executor/recommend_index.rs` 的边界及 `Optimizer` 实现策略；当前只有 Cargo 依赖，不能假设调用链已存在。
- 性能修改要保留确定性集合和排序。并行调用 `Optimizer` 前必须先定义其线程安全、连接复用和计划缓存约束，并评估 `候选数 × 查询数 × 2` 次计划估算的资源上限。
- 修改 Rust 逻辑时应继续把测试放在同目录独立 `*_test.rs` 文件，并尽量与同名 Go 测试保持场景和断言一致。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/indexadvisor` 确认目标目录的 Rust/Go 文件均已索引。
- RustCodeGraph `query/node`：确认 `advise_indexes_for_sql` 位于第 138 行、`prepare_recommendations` 位于第 174 行、`graceful_index_name` 位于第 286 行；`node indexadvisor.rs::advise_indexes_for_sql` 给出的下游边包括 `algorithm.rs::advise_indexes` 和本文件 `prepare_recommendations`，上游边仅来自三个独立 Rust 测试模块。CLI 的独立 `callers/callees` 子命令本次未输出内容，因此又用 `node` trail 和仓库文本搜索交叉核验。
- 已完整阅读：[`indexadvisor.rs`](./indexadvisor.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、Go 对照 [`indexadvisor.go`](./indexadvisor.go) 和独立 Rust 测试 [`indexadvisor_test.rs`](./indexadvisor_test.rs)、[`indexadvisor_sql_test.rs`](./indexadvisor_sql_test.rs)、[`indexadvisor_tpch_test.rs`](./indexadvisor_tpch_test.rs)。
- 已定点核验：`algorithm.rs::advise_indexes`、`optimizer.rs::Optimizer`、`options.rs::AdvisorOptions`，以及 `utils.rs` 的规范化、schema 恢复、系统表过滤和列收集入口。
- Rust 测试证据：`recommendation_metrics_match_go_per_index_calculation` 验证 0.5 改善率和 reason；`graceful_index_name_uses_go_fallback_sequence` 验证命名回退；`specified_sqls_are_not_limited_by_statement_summary_max_query` 验证显式 SQL 不受摘要上限影响；`recommendation_checks_index_size_before_discarding_no_benefit_index` 验证错误顺序；SQL/TPC-H 测试验证单表、多表、类型、已有覆盖索引和 21 条 Go fixture workload。
- Go 测试证据：`indexadvisor_sql_test.go` 的四组 SQL 场景、`indexadvisor_tpch_test.go` 的四组 TPC-H 场景，以及 `indexadvisor_test.go` 中非法查询、频率、已有/覆盖索引、运行时长和持久化等完整入口场景用于界定已对齐部分与未接线部分。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核：文档区分了活跃代码与注释草稿，没有把测试调用或 Cargo 依赖误写为生产接线，也没有建议把 Rust 测试内嵌到源文件。
