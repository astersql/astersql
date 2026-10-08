# `pkg/planner/indexadvisor/algorithm.rs`

## 文件定位

该文件属于 `astersql-planner-indexadvisor` crate（见同目录 `Cargo.toml`），实现索引顾问的代价驱动候选枚举和选择核心。crate 入口 `lib.rs` 将它公开为 `algorithm` 模块；当前生产调用链是 `indexadvisor.rs::advise_indexes_for_sql` 在完成 SQL 归并、schema 补全、系统表过滤和可索引列收集后调用 `algorithm.rs::advise_indexes`，再把返回的索引交给 `prepare_recommendations` 生成用户可见建议。

本文件只负责在已给定的 workload、可索引列、what-if 优化器和参数快照上搜索索引集合，不负责解析用户选项、持久化结果、真正创建索引或维护数据库连接。源码含 3 个公开函数和 7 个私有辅助函数，没有类型、trait、模块常量、条件编译项或全局可变状态。

## 核心职责

- `advise_indexes` 是生产主入口：从单列候选开始，逐步增加索引宽度，对每条查询保留有限候选，再按整个 workload 的估算代价选出不超过数量上限的集合。
- `select_index_candidates`、`usable_candidates` 将候选约束到查询实际引用的首列，并排除已被数据库现有索引前缀覆盖的候选。
- `choose_best_indexes` 组合“空集/单索引/小候选集中的索引对”枚举与后续贪心扩展，在搜索成本和组合收益之间折中。
- `extend_indexes` 只从上一宽度选中的索引继续追加同表未使用列，形成主流程下一轮候选；列顺序有意义。
- `create_multi_column_indexes` 提供独立的、按表生成所有宽度不超过上限的有序排列的公开辅助 API。它当前不在 `advise_indexes` 生产链中，仓库内直接调用证据来自 `indexadvisor_test.rs::multi_column_candidates_match_go_permutation_enumeration`。
- `cut_down` 删除被同表更长索引以前缀覆盖的冗余索引；`check_timeout` 为搜索阶段提供统一时间预算检查。

## 主要符号

- `pub fn advise_indexes(queries, indexable_columns, optimizer, options) -> Result<BTreeSet<Index>, String>`：算法入口。`queries` 和候选列使用有序集合，`optimizer` 是 `optimizer.rs::Optimizer` trait 对象，`options` 提供最大索引数、最大宽度和超时。
- `fn select_index_candidates(...)`：逐查询调用 `collect_indexable_columns`，从可用候选中反复选择一个最佳索引并移除，最多保留 3 个，然后合并各查询结果。
- `fn single_column_indexes(...)`：为每个输入列构造 `idx_<column>`。实际身份由 `Index::with_columns` 根据首列的 schema/table 重建。
- `fn usable_candidates(...)`：可选地要求候选首列在本查询引用列中，并通过 `Optimizer::prefix_contain_index` 排除已存在索引覆盖；每个候选前检查超时。
- `fn choose_best_indexes(...)`：先评估空集和每个单索引；候选不超过 50 且允许至少 2 个索引时还评估所有索引对；随后每轮加入一个能进一步降低代价的候选，直到达到上限或无改善。
- `fn extend_indexes(...)`：仅当索引恰为 `current_width` 且新列与其同表、尚未出现时追加该列，名称为 `idx_<ordered_columns>`。
- `pub fn create_multi_column_indexes(columns, max_width)` 与 `fn append_permutations(...)`：按 `(schema, table)` 分组、排序，并深度优先生成无重复列的所有非空有序前缀，宽度不超过 `max_width`。
- `pub fn cut_down(indexes)`：保留集合中不存在更长前缀覆盖者的元素；它不按代价删减，也不单独执行数量上限裁剪。
- `fn check_timeout(...)`：当 `started.elapsed() > options.timeout` 时返回文本错误；等于预算时尚不超时。

## 执行流程

1. `advise_indexes` 先处理参数边界：`max_num_indexes == 0` 直接返回空集；`max_index_width == 0` 返回错误。随后记录唯一的起始时间 `Instant`。
2. `single_column_indexes` 为全部可索引列建单列候选，`usable_candidates(..., filter_referenced = false)` 立即排除被真实已有索引覆盖的候选。
3. 对宽度 `1..=max_index_width` 逐轮执行：先检查总超时；`select_index_candidates` 对每条查询重新识别引用列，只保留首列相关候选，并连续挑出最多 3 个最佳单索引；各查询候选并集再交给 `choose_best_indexes` 按整个 workload 选优。
4. 若尚未达到最大宽度，以本轮 `selected` 为基底，并入 `extend_indexes` 生成的下一宽度索引。这里保留已选的较窄索引，也只扩展本轮被选中的索引，不生成全体列排列。最后一轮则把 `potential` 留作该轮 `candidates`，供收尾补充。
5. 主循环后最多执行 3 轮贪心补充：若未达到数量上限，计算当前集合代价，逐一试加 `potential` 中未选候选，只接受严格优于当前代价且优于本轮已有最佳者的候选。
6. 每次补充后，从 `potential` 移除该候选以及与它存在任一方向前缀包含关系的候选，避免后续再加入同一前缀族。
7. 返回前调用 `cut_down`，删除仍被更长同表索引覆盖的较短索引。

`create_multi_column_indexes` 是另一条独立流程：拒绝零宽度，把列按表分组并排序，再由 `append_permutations` 递归生成长度 1 到 `max_width` 的排列。三列、宽度二会得到 3 个单列和 6 个有序双列，共 9 个候选；这一事实由 Rust 独立测试验证。

## 数据与状态

- `BTreeSet<Query>`、`BTreeSet<Column>` 和 `BTreeSet<Index>` 同时承担去重与确定性遍历。其顺序来自模型类型的 `Ord`，选择平局还由 `IndexSetCost::less` 使用总列数和排序后的索引键稳定裁决。
- `Index.columns` 是有序向量；`(a,b)` 可覆盖 `(a)`，但不覆盖 `(b)`。`extend_indexes` 和 `append_permutations` 都禁止同一列在一个索引内重复。
- `selected` 保存当前宽度轮次的全 workload 最佳集合；`potential` 是下一轮搜索或收尾贪心的候选池；`candidates` 是所有查询各自前三名的并集。
- `started: Instant` 在一次 `advise_indexes` 调用期间不重置，因此超时预算覆盖主搜索与收尾，而不是每轮独立计时。
- 代价来自 `utils.rs::evaluate_index_set_cost`：对每条查询调用 `Optimizer::query_plan_cost`，乘以 `Query.frequency` 后求和，并附带总索引列数与稳定键。代价为零在 `IndexSetCost::less` 中视为无效，不能胜过其他集合。
- 文件不缓存计划、不记录全部候选，也不修改优化器；是否有外部状态取决于传入的 `Optimizer` 实现。

## 依赖与调用关系

上游生产调用边为 `indexadvisor.rs::advise_indexes_for_sql -> algorithm.rs::advise_indexes`。RustCodeGraph 还显示测试调用来自 `algorithm_test.rs` 与 `indexadvisor_test.rs`；精确仓库检索确认 `indexadvisor_sql_test.rs` 和 `indexadvisor_tpch_test.rs` 通过 `advise_indexes_for_sql` 间接覆盖该算法。

主要下游关系如下：

- `advise_indexes -> single_column_indexes / usable_candidates / select_index_candidates / choose_best_indexes / extend_indexes / check_timeout / cut_down`。
- `select_index_candidates -> utils.rs::collect_indexable_columns`，并继续调用 `usable_candidates` 与 `choose_best_indexes`。
- `usable_candidates -> Optimizer::prefix_contain_index`，用于读取现有索引覆盖事实。
- `choose_best_indexes` 和收尾贪心 `-> utils.rs::evaluate_index_set_cost -> Optimizer::query_plan_cost`。
- `extend_indexes`、`single_column_indexes`、`append_permutations -> model.rs::Index::with_columns`；裁剪依赖 `Index::prefix_contains`，比较依赖 `IndexSetCost::less`。

同目录 `Cargo.toml` 将 crate 根设为 `lib.rs`。其中真实依赖目前全部位于 `target.'cfg(windows)'.dependencies`，而本文件自身仅直接使用 crate 内模块和标准库；文档不据此推断其他平台的完整应用接线已可用。

## 错误处理与边界

- 所有可失败路径统一使用 `Result<_, String>` 并以 `?` 原样传播，包括索引构造、列收集、现有索引查询、计划代价评估和超时错误；本文件不包装错误类型或增加调用上下文。
- `max_num_indexes == 0` 优先返回空集，因此即使 `max_index_width == 0` 也不会报宽度错误；只有需要搜索且宽度为零时才返回 `"max index width must be positive"`。
- 空查询集或空候选集不会 panic：组合搜索仍会评估空集，若没有严格改善则返回空结果。但代价评估如何解释空 workload 由 `IndexSetCost::less` 的零代价规则共同决定。
- `Index::with_columns` 拒绝空列；本文件的正常生成路径总是传入至少一列。公开的 `create_multi_column_indexes` 对空输入返回空集，对 `max_width` 大于表列数时自然在无可追加列处停止。
- 候选超过 50 时跳过二索引穷举，以避免二次方组合膨胀；这可能漏掉只有成对使用才有收益的组合，是明确的搜索质量/性能折中。
- 超时只在初始化候选遍历、每个宽度轮开始、每条查询/候选过滤和每轮收尾开始检查；单次 `choose_best_indexes` 或代价钩子执行期间不会被抢占，故实际返回时间可能超过预算。
- 候选索引名仅按列名拼接，没有 Go 版 64 字节长度回退；最终对外推荐会在 `prepare_recommendations` 中另行生成可用名称。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件句柄或网络资源。一次调用的集合和 `Instant` 都是栈上局部状态，结束时自动释放。

`Optimizer` 以共享引用 `&dyn Optimizer` 串行调用；其 trait 文档明确实现通常不保证线程安全，本算法也没有并行化代价评估。若未来并行评估候选，必须先重新定义优化器的线程安全约束、代价调用的可重入性与稳定平局顺序，并保证共享超时预算仍然成立。

当前最主要的资源风险是计算量：每轮按查询扫描候选，候选不超过 50 时枚举所有索引对，之后又逐候选贪心试算；每次试算会对整个 workload 调用 what-if 计划代价。`max_index_width`、`max_num_indexes`、每查询前三候选限制和超时共同控制这一开销。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/indexadvisor/algorithm.go`。Rust 保留了 Go auto-admin 算法的这些语义：从单列索引起步、按最大宽度逐轮扩展、候选首列必须被查询引用、每查询保留最多三个最佳索引、候选小于等于 50 时先枚举至多两个索引再贪心扩展、使用 what-if workload 代价择优，以及统一超时错误。

Rust 当前并非 Go 文件的完整同形实现，重要差异包括：

- Go `adviseIndexes` 还返回 `allCandidates` 并通过 `recordCandidates` 限量记录候选；Rust `advise_indexes` 只返回最终集合。
- Go 主流程依次执行 `heuristicMergeIndexes`、`heuristicCoveredIndexes`、`filterIndexes` 和按代价递归删减至数量上限的 `cutDown`；Rust 没有对应启发式阶段，Rust `cut_down` 仅删除前缀冗余。Rust 的数量约束主要在 `choose_best_indexes` 和收尾循环中实现。
- Go `filterIndexes` 会排除无收益候选、前缀冗余和被现有索引覆盖者；Rust 在早期用 `usable_candidates` 排除现有索引覆盖，在选择时只接受严格改善，最后再做集合内前缀裁剪，但阶段和覆盖范围不同。
- Go `createMultiColumnIndexes` 只扩展当前已选索引，并通过 `Optimizer::TableColumns` 取得表列；Rust 主流程的 `extend_indexes` 直接使用传入的 `indexable_columns` 达成相近扩展。Rust 公开的 `create_multi_column_indexes` 则生成全部有序排列，是测试辅助 API，不等同于 Go 同名方法的调用位置和输入形状。
- Go 临时索引名超过 64 字节时使用 UUID；Rust 候选名没有该回退。

因此，扩展或修复时应以当前 Rust 行为和独立测试为基线，同时把上述缺失项视为迁移差距；不能仅凭名称相似宣称覆盖索引或 IndexMerge 启发式已对齐。

## 扩展指南

- 修改候选搜索主链时优先落在 `advise_indexes`、`select_index_candidates`、`choose_best_indexes` 或 `extend_indexes`，并在独立的 `algorithm_test.rs` 增加回归测试；不要把 Rust 测试内嵌进生产文件。
- 若补齐 Go 的覆盖索引、IndexMerge 或 `filterIndexes`，应逐个对照 `algorithm.go` 的同名方法、相关列收集函数和 `indexadvisor_test.go` 中 `TestIndexAdvisorPrefix`、`TestIndexAdvisorCoveringIndex`、`TestIndexAdvisorExistingIndex` 等行为。不要为了让测试通过而省略候选记录、错误传播或收益判断。
- 若改变代价排序，必须同步审查 `model.rs::IndexSetCost::less` 和 `utils.rs::evaluate_index_set_cost`；频率加权、零代价语义、列数与键字符串平局规则都是结果确定性的一部分。
- 若改变已有索引过滤，需同步 `optimizer.rs::Optimizer::prefix_contain_index` 及 `indexadvisor_test.rs::advisor_algorithm_propagates_existing_index_lookup_errors`，保持元数据错误向上传播。
- 若改变多列生成，先决定目标是主链增量扩展 (`extend_indexes`) 还是独立全排列 API (`create_multi_column_indexes`)；二者不可互换。同步覆盖跨表隔离、列顺序、零宽度、宽度大于列数和重复列边界。
- 性能改动应关注计划代价调用次数和 50 候选阈值；并行化还需解决 `Optimizer` 非线程安全契约。兼容性改动应保持公开签名和确定性 `BTreeSet` 结果，或明确迁移调用方。

## 验证依据

- 目标源码：`pkg/planner/indexadvisor/algorithm.rs`，完整核对 338 行中的 10 个函数、公开性、分支和错误路径。
- crate 与模块边界：`pkg/planner/indexadvisor/Cargo.toml`、`pkg/planner/indexadvisor/lib.rs`。
- 生产入口与直接调用：`pkg/planner/indexadvisor/indexadvisor.rs::advise_indexes_for_sql`、`prepare_recommendations`。
- 数据/代价/优化器契约：`pkg/planner/indexadvisor/model.rs::{Index::with_columns, Index::prefix_contains, IndexSetCost::less}`、`utils.rs::{collect_indexable_columns, evaluate_index_set_cost}`、`optimizer.rs::Optimizer`。
- Rust 独立测试：`pkg/planner/indexadvisor/algorithm_test.rs::each_query_keeps_only_three_best_single_index_candidates`；`pkg/planner/indexadvisor/indexadvisor_test.rs::{multi_column_candidates_match_go_permutation_enumeration, advisor_algorithm_propagates_existing_index_lookup_errors, advisor_model_preserves_index_prefix_semantics}`；SQL 和 TPC-H 测试通过 `advise_indexes_for_sql` 间接覆盖主链。
- Go 对照：`pkg/planner/indexadvisor/algorithm.go` 的 `adviseIndexes`、`autoAdmin.calculateBestIndexes`、`selectIndexCandidates`、`enumerateCombinations`、`enumerateGreedy`、`createMultiColumnIndexes`、`filterIndexes`、`cutDown`、`timeout`；相关行为测试位于 `indexadvisor_test.go`。
- RustCodeGraph：`status` 显示索引包含该目录 27 个文件；文件节点显示 `algorithm.rs` 被 `indexadvisor.rs` 和 `indexadvisor_test.rs` 使用；精确符号查询确认 `advise_indexes`、`create_multi_column_indexes`、`cut_down`，调用图确认 `advise_indexes -> select_index_candidates/single_column_indexes/usable_candidates/choose_best_indexes/extend_indexes/cut_down/check_timeout` 以及 `advise_indexes_for_sql -> advise_indexes`。图工具对常见/限定名查询含同名噪声，未覆盖事实以同目录精确 `rg` 检索补证。
- 本任务是纯文档分析，按计划未运行 Cargo；交付前使用任务给定命令验证文档存在且恰有 11 个固定二级标题，并人工复核无测试内嵌建议、无把 Go 缺失能力写成 Rust 已支持的结论。
