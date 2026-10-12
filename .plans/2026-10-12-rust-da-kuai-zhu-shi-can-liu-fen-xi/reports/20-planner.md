# Planner 大块注释残留审计

范围：`reports/00-candidates.tsv` 中全部 `pkg/planner/**` 候选。Planner 契约以
`pkg/planner/core/base/doc.go`（抽象接口应保持最小、避免依赖具体实现）和
`pkg/planner/funcdep/doc.go`（函数依赖理论说明）为入口核对。候选行号沿用候选全集，
后续源文件变更可能使行号漂移。

分类口径：

- **已实现重复档案**：同文件已有可执行 Rust 对应符号，并有相邻测试或明确调用证据；注释本身不执行。
- **部分实现且注释仍描述缺口**：存在可执行 Rust，但相较 Go 仍使用内存模型、局部抽象或缺少真实 planner/session 接线。
- **非执行测试档案**：Go 测试或 Rust 草稿被注释/放进原始字符串；是否已有替代测试另行记录。
- **合法说明/示例误报**：算法说明或生成器输出模板，不应按残留代码删除。

## Cascades 与 case test

候选：`pkg/planner/cascades/cascades_test.rs:22-90`

- 分类：非执行测试档案（低处置风险）。来源为 Go `TestCascadesDrive` 等测试。
- 现状/证据：同文件可执行测试 `cascades_optimizer_initializes_memo_and_explores_root_group`、`cascades_context_exposes_rule_mask` 和 `test_cascades_drive` 已覆盖 memo 初始化、规则掩码和 planner 开关。
- 建议：替代测试稳定后删除注释副本；保留 Go 文件作为来源，不需要在 Rust 再存一份。

候选：`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_test.rs:21-148`

- 分类：非执行测试档案（低风险）。原始字符串保存 Go 中被跳过的 intermediary apply 测试。
- 现状/证据：可执行测试 `decorrelation_rewrites_uncorrelated_apply_and_preserves_children`、`intermediary_apply_is_removed_and_correlated_apply_is_retained` 直接调用生产转换并检查子组与 correlated apply 保留。
- 建议：删除归档字符串前保留测试为何曾跳过的短注释；行为已有更强的 Rust 断言。

候选：`pkg/planner/core/casetest/ch/ch_test.rs:23-34`

- 分类：非执行测试档案（低风险），是紧邻原始字符串的迁移说明。
- 现状/证据：`ch_q2_style_five_way_join_is_fully_reordered_and_schema_preserved`、`ch_q5_style_seven_way_join_resolves_under_greedy_solver` 以及真实 cascades Q2/Q5 测试覆盖结构与 golden plan。
- 建议：将仍有价值的“统计驱动且保持输出 schema”压缩成测试注释，其余删除。

候选：`pkg/planner/core/casetest/ch/ch_test.rs:35-115`

- 分类：非执行测试档案（低风险），完整 Go `TestQ2/TestQ5` 原始字符串。
- 现状/证据：同文件已使用 `setup_ch_schema`、suite fixture 和生产 join reorder 执行对应场景。
- 建议：可删除；Go 同名文件仍是逐提交来源。

候选：`pkg/planner/core/casetest/ch/main_test.rs:21-30`

- 分类：非执行测试档案（低风险），描述 Go `TestMain` 和 schema helper。
- 现状/证据：可执行 `setup_ch_schema` 返回真实 `Domain`/`TestKit` 并被 `ch_test.rs` 使用。
- 建议：保留一行 helper 契约即可，删除迁移叙述。

候选：`pkg/planner/core/casetest/ch/main_test.rs:31-194`

- 分类：非执行测试档案（低风险），Go suite 初始化和建表源码原始字符串。
- 现状/证据：`setup_ch_schema` 完成对应建表与 TiFlash replica 接线；Q2/Q5 测试消费相同 golden 数据。
- 建议：删除字符串，若需核对直接引用同路径 Go 文件。

候选：`pkg/planner/core/casetest/main_test.rs:22-105`

- 分类：部分实现且注释仍描述缺口（中风险）。注释保存未执行的 `TestData`/BookKeeper 草稿。
- 现状/证据：可执行 `load_suite`、各 suite getter 和 `load_named_sql_cases` 已真实解析 fixture；`casetest_suite_inputs_are_independent_parser_runs` 等测试覆盖加载行为，但 Rust 没有 Go record-mode BookKeeper 写回能力。
- 建议：先决定 record mode 是否由统一测试框架提供；仅在该能力明确范围外后删除草稿并保留差异说明。

候选：`pkg/planner/core/casetest/plan_test.rs:22-737`

- 分类：非执行测试档案（高风险）。716 行注释含多组 Go planner/cost/golden 测试，而非单一说明。
- 现状/证据：文件后部有可执行 plan-shape 比较辅助和测试，但注释明确声明不会创建真实 store、执行 SQL 或读写 testdata；不能据此证明全部 Go 用例已迁移。
- 缺失行为/建议：逐个 Go 测试名与当前 `plan_test.rs`、其他 casetest 文件建立映射；未映射用例另建任务，禁止整块直接删除。

候选：`pkg/planner/core/casetest/rule/main_test.rs:26-103`

- 分类：部分实现且注释仍描述缺口（中风险）。原始字符串中的 BookKeeper/TestMain 不执行。
- 现状/证据：该块自身没有可执行实现；实际 suite 加载由父目录 `main_test.rs` 和各规则测试完成。缺少集中 record-mode 生命周期。
- 建议：统一 suite loader 后删除；此前保留为待接线证据，不得误判为已执行测试。

候选：`pkg/planner/core/casetest/rule/rule_outer2inner_test.rs:34-287`

- 分类：非执行测试档案（低风险）。Go 测试完整归档在字符串中。
- 现状/证据：`outer2inner_fixture_inventory_matches_go` 和 `outer2inner_go_golden_plans_execute_in_both_planners` 使用真实 TestKit，在 legacy/cascades 两种 planner 下核对 fixture。
- 建议：可删除归档字符串，保留 fixture 来源的一行注释。

候选：`pkg/planner/core/casetest/stats_test.rs:24-171`

- 分类：非执行测试档案（中风险）。注释测试草稿覆盖 GroupNDV/explain。
- 现状/证据：可执行 `group_ndv_inputs_collect_group_and_aggregate_columns`、`group_ndv_inputs_collect_both_join_sides_and_equal_columns` 和 golden fixture 测试覆盖核心统计输入。
- 建议：核对注释中的所有 SQL case 已在 fixture inventory 后删除；不要保留伪 `#[test]`。

## Join order

候选：`pkg/planner/core/joinorder/bitset_bench_test.rs:36-199`

- 分类：非执行测试档案（低风险）。Go benchmark 在原始字符串中。
- 现状/证据：可执行 `BenchRule`/`BenchEdge` 和测试执行同样的边/规则可用性路径，但 Rust 当前是 correctness test，不是性能 benchmark。
- 建议：性能基准若仍需要应转成 Criterion/仓库统一 benchmark；否则删除 Go 字符串并明确只保留正确性覆盖。

候选：`pkg/planner/core/joinorder/conflict_detector.rs:22-594`

- 分类：已实现重复档案（中风险）。注释是 `ConflictDetector` 的早期 Rust/Go 形状。
- 现状/证据：行 603 后已有可执行 `Rule`、`Edge`、`Node`、`ConflictDetector`，并实现边适用、TES 与方向规则；`conflict_detector_test.rs` 覆盖 equality TES、方向规则和 connected component。
- 建议：可删除重复实现，保留算法不变量注释；删除前用生产测试确认 outer/semi/anti join 规则矩阵。

候选：`pkg/planner/core/joinorder/join_order.rs:21-395`

- 分类：部分实现且注释仍描述缺口（高风险）。注释明确日志、session hint 等外部依赖未接通。
- 现状/证据：行 401 后 `JoinOrder`、DP/greedy、笛卡尔连接和成本比较可执行；`join_order_test.rs`、`rule_join_reorder_{dp,greedy}_test.rs` 覆盖成本选择和计划树。
- 缺失行为/建议：真实 session hint/warning 和 tracing 接线不能由内存 `PlanNode` 测试证明；先建立端到端 explain/hint 回归，再删除档案。

候选：`pkg/planner/core/joinorder/join_order_test.rs:31-92`

- 分类：非执行测试档案（低风险），保存 Go greedy-start 测试。
- 现状/证据：可执行 `join_order_optimize_avoids_costly_greedy_start`、DP/greedy 一致性和 clone 独立性测试直接使用生产 API。
- 建议：可删除字符串。

候选：`pkg/planner/core/joinorder/ordered_leading.rs:23-221`

- 分类：部分实现且注释仍描述缺口（中风险）。注释说明只检查内存 schema/index/expression，不访问数据库。
- 现状/证据：可执行 `find_ordered_leading_choice`、`try_annotate_ordered_leading`；`ordered_leading_test.rs` 覆盖 join-group anchor、已有 hint 拒绝和 bushy/unsupported 情况。
- 建议：补真实 LEADING hint explain/warning 用例后再删除大块草稿；算法说明可压缩保留。

候选：`pkg/planner/core/joinorder/util.rs:22-290`

- 分类：已实现重复档案（中风险）。注释保存计划、hint 与表达式变换草稿。
- 现状/证据：后续可执行 helper 被 join-order 实现调用，`util_test.rs` 覆盖列解析、连接组构造和 schema 保持。
- 建议：按函数逐段删除重复体，仅保留 hint 边界与 schema 不变量。

## 物理算子与任务

候选：`pkg/planner/core/operator/physicalop/foreign_key.rs:22-675`

- 分类：部分实现且注释仍描述缺口（高风险）。注释声称保留 DML 外键检查/级联完整控制流，但不执行。
- 现状/证据：后续 Rust 数据结构/构建逻辑可执行，`foreign_key_test.rs` 覆盖 DML 外键拒绝、级联及 plan-cache 合约；真实存储级 cascade 执行不由这些 planner 单测证明。
- 建议：以 INSERT/UPDATE/DELETE explain + 执行回归确认全部分支后才删除来源档案。

候选：`pkg/planner/core/operator/physicalop/fragment_test.rs:20-43`

- 分类：非执行测试档案（低风险），说明 Go fragment 测试迁移关系。
- 现状/证据：同文件及 `fragment_aster_unit_test.rs` 有可执行 singleton/fragment 行为测试。
- 建议：压缩成一行来源说明。

候选：`pkg/planner/core/operator/physicalop/fragment_test.rs:47-120`

- 分类：非执行测试档案（低风险），Go 测试原始字符串。
- 现状/证据：可执行 Rust fragment 测试覆盖初始化、singleton 和计划切分。
- 建议：可删除。

候选：`pkg/planner/core/operator/physicalop/physical_common_plans.rs:19-311`

- 分类：部分实现且注释仍描述缺口（中风险）。大块草稿混合多种 common physical plan 语义。
- 现状/证据：文件后部可执行结构由 `physical_common_plans_test.rs`、`physical_plan_misc_test.rs` 覆盖 explain、clone、内存估算等局部行为。
- 建议：先生成“注释符号—可执行符号—测试”表；未映射的 protobuf/attach-to-task 分支另建任务，禁止整体删除。

候选：`pkg/planner/core/operator/physicalop/physical_cte.rs:19-105`

- 分类：已实现重复档案（中风险）。
- 现状/证据：后续 `PhysicalCTE` 可执行，`physical_cte_test.rs` 覆盖 seed/recursive、共享统计和 possible properties。
- 建议：删除重复代码，保留 CTE seed/recursive task 选择不变量。

候选：`pkg/planner/core/operator/physicalop/physical_cte_table.rs:19-79`

- 分类：已实现重复档案（低风险）。
- 现状/证据：可执行 CTE table 叶节点由 `physical_cte_table_test.rs` 覆盖 explain 格式与 clone/schema。
- 建议：可删除草稿。

候选：`pkg/planner/core/operator/physicalop/physical_index_hash_join.rs:21-108`

- 分类：部分实现且注释仍描述缺口（中风险）。注释只声明优化器节点形状。
- 现状/证据：可执行 index hash join 参与 `physical_index_join_test.rs`、`index_join_probe_test.rs` 的 attach/probe 路径；缺少该文件一一对应的独立行为测试。
- 建议：补 hash join clone/explain/attach 精确回归，再删除大块档案。

候选：`pkg/planner/core/operator/physicalop/physical_index_merge_join.rs:21-110`

- 分类：已实现重复档案（中风险）。
- 现状/证据：可执行 merge join 由 `physical_index_merge_join_test.rs` 覆盖 key/order/attach 行为。
- 建议：删除重复体，保留排序前置条件。

候选：`pkg/planner/core/operator/physicalop/physical_lock.rs:21-147`

- 分类：已实现重复档案（中风险）。
- 现状/证据：可执行 `PhysicalLock` 由 `physical_lock_test.rs` 覆盖 absent TopN child、锁模式 round-trip 和 child 行为。
- 建议：删除草稿，保留 `SELECT ... FOR UPDATE` 的缓存/锁契约。

候选：`pkg/planner/core/operator/physicalop/physical_sequence.rs:21-181`

- 分类：已实现重复档案（中风险）。
- 现状/证据：可执行 sequence/CTE producer 由 `physical_sequence_test.rs` 覆盖 schema、属性组合和 attach。
- 建议：删除重复代码；保留 producer 标记语义。

候选：`pkg/planner/core/operator/physicalop/physical_shuffle.rs:21-95`

- 分类：已实现重复档案（中风险）。
- 现状/证据：可执行 shuffle 由 `physical_shuffle_test.rs` 覆盖 worker/tail 计划边界与 explain。
- 建议：可删除草稿，保留线程分工说明。

候选：`pkg/planner/core/operator/physicalop/plan_clone_generated.rs:21-592`

- 分类：部分实现且注释仍描述缺口（高风险）。注释明确“仅用于人工审阅”，且是大量生成 clone 代码。
- 现状/证据：文件后部存在可执行 clone 实现，`plan_clone_generated_test.rs` 与 cache snapshot 测试覆盖部分运行时字段不复制、context/统计共享契约。
- 建议：不要手工删改生成映射；先用 generator 重生并比对所有物理结构，确认生成源是唯一真源后移除归档。

候选：`pkg/planner/core/operator/physicalop/task_base.rs:16-132`

- 分类：部分实现且注释仍描述缺口（高风险）。注释是 task 基础类型草稿并标为人工审阅。
- 现状/证据：行 658 后有可执行 warning/root/mpp/cop task；`task_base_test.rs`、`task_test.rs` 覆盖成本溢出、task 计数、属性和 clone 局部行为。
- 建议：逐类型核对 Go 接口后删除，尤其不能遗漏 invalid/cost/warning 语义。

候选：`pkg/planner/core/operator/physicalop/task_base.rs:134-650`

- 分类：部分实现且注释仍描述缺口（高风险）。第二块保存 warning 指针复用、Root/Mpp/CopTask 等完整草稿。
- 现状/证据：后续可执行实现存在，但注释中的指针共享/层级 warning 语义需与 `task_base_test.rs` 精确映射；当前测试不能自动证明每个字段一致。
- 建议：生成字段级 parity 清单，补 warning 跨层传播回归后再删除。

候选：`pkg/planner/core/operator/physicalop/tiflash_predicate_push_down.rs:45-419`

- 分类：部分实现且注释仍描述缺口（高风险）。包含 late materialization 算法、阈值和表达式分组的旧实现。
- 现状/证据：后续 `handle_physical_tiflash_late_materialization`、阈值常量和表达式成本判断可执行；`tiflash_predicate_push_down_test.rs` 覆盖基本 gate、重表达式和列数/选择率路径。
- 建议：保留算法解释但删除代码形重复；先补真实 TiFlash scan explain 回归，防止内存模型与生产 scan 接线漂移。

## 逻辑优化规则与统计

候选：`pkg/planner/core/rule/rule_build_key_info.rs:16-38`

- 分类：已实现重复档案（低风险）。
- 现状/证据：后续 `BuildKeySolver` 实现 `LogicalRule` 并递归 `build_keys`；`rule_build_key_info_test.rs` 覆盖 PK、max-one-row、child key 映射和 index path。
- 建议：可删除草稿，保留规则名契约。

候选：`pkg/planner/core/rule/rule_collect_plan_stats.rs:16-259`

- 分类：部分实现且注释仍描述缺口（高风险）。注释列出 domain/infoschema/statistics 外部接线。
- 现状/证据：后续 `CollectPredicateColumnsPoint`、`StatsLoader`、请求加载和 virtual column 收集可执行；`rule_collect_plan_stats_test.rs` 覆盖 predicate/join/partition 列与规则名。
- 缺失行为/建议：真实 domain async/sync load、超时和 plan-cache skip reason 需端到端验证；完成前不可把注释当纯重复。

候选：`pkg/planner/core/rule/rule_column_pruning.rs:21-59`

- 分类：已实现重复档案（低风险）。
- 现状/证据：后续 `ColumnPruner` 和 `prune` 可执行；`rule_column_pruning_test.rs` 覆盖 projection offset 与 Go 规则契约。
- 建议：可删除草稿。

候选：`pkg/planner/core/rule/rule_join_key_type_cast.rs:16-52`

- 分类：已实现重复档案（中风险）。
- 现状/证据：后续 rewriter、cast 识别、guard selection 和 column-id 分配可执行；`rule_join_key_type_cast_test.rs` 覆盖 mixed numeric、未投影 key 和 guard。
- 建议：删除重复说明，保留 signed INT/VARCHAR 安全转换约束。

候选：`pkg/planner/core/rule/rule_max_min_eliminate.rs:21-281`

- 分类：部分实现且注释仍描述缺口（中风险）。注释称外部 TiDB 类型仅作接线点。
- 现状/证据：后续 `MaxMinEliminator`、索引顺序判断可执行；`rule_max_min_eliminate_test.rs` 覆盖 extrema/count 与 index order。
- 建议：补真实 SQL explain/nullability/partitioned-index 用例后删除草稿。

候选：`pkg/planner/core/rule/rule_order_aware_join_reorder.rs:21-258`

- 分类：部分实现且注释仍描述缺口（高风险）。
- 现状/证据：后续规则可执行并处理 ordering/projection；`rule_order_aware_join_reorder_test.rs` 覆盖排序列改写和满足性。
- 缺失行为/建议：确认规则已进入生产 logical rule 列表，并用 SQL explain 验证实际触发；仅有类型和单测不等同已接线。

候选：`pkg/planner/core/rule/rule_outer_join_to_semi_join.rs:21-411`

- 分类：部分实现且注释仍描述缺口（高风险）。
- 现状/证据：后续 `OuterJoinToSemiJoin`、null-reject/comparison 判断可执行；规则测试及 casetest `rule_outer_to_semi_join_test.rs` 覆盖局部转换。
- 缺失行为/建议：核对规则注册、hint/charset/null-safe equality 和真实 explain；未接线前不可删除来源档案。

候选：`pkg/planner/core/rule/rule_partition_processor.rs:22-2229`

- 分类：部分实现且注释仍描述缺口（最高风险）。2208 行保存 Go partition processor 的广泛分支。
- 现状/证据：行 2236 后只有紧凑的 `PartitionRange`、`PartitionProcessor` 与 range/hash/list 选择实现；`rule_partition_processor_test.rs`、`rule_partition_pruning_test.rs` 覆盖 batch alignment、动态分区字符串和部分裁剪，但体量明显小于归档来源。
- 图证据：RustCodeGraph 的精确 impact 显示 Rust `PartitionProcessor` 只影响上述两个测试文件（列出 `incomparable_range_constant_aborts_pruning_like_go`、`range_partition_predicate_selects_canonical_partition`），而 Go 同名类型可追到 `optRuleList` 等 61 个符号；Rust 虽在 `optimizer_runtime.rs` 有同名属性，当前图中没有形成等价的生产调用半径。
- 缺失行为/建议：按 Go 函数清单审计 expression folding、NULL/MAXVALUE、collation、range columns、list columns、dynamic pruning、partition union 与错误路径；这是不可整块删除的 P0 后续任务。

候选：`pkg/planner/core/rule/rule_predicate_simplification.rs:21-630`

- 分类：部分实现且注释仍描述缺口（高风险）。
- 现状/证据：后续 `simplify_cnf`/`simplify_expr` 可执行；`rule_predicate_simplification_test.rs` 和 casetest golden 覆盖基础 CNF、常量与派生 TopN 组合。
- 缺失行为/建议：逐项核对 Go 的类型/collation/null semantics 与表达式函数白名单，补 SQL golden 后删除。

候选：`pkg/planner/core/rule/rule_prune_indexes.rs:21-602`

- 分类：部分实现且注释仍描述缺口（高风险）。
- 现状/证据：后续 index scoring、column requirements、index merge preference 可执行；`rule_prune_indexes_test.rs` 覆盖 clustered prefix、covering/hinted path 等。
- 缺失行为/建议：真实 DataSource access path、hint 与 session 最大索引数接线须由 explain 验证；完成前保留来源映射。

候选：`pkg/planner/core/stats/stats.rs:23-158`

- 分类：部分实现且注释仍描述缺口（高风险）。注释明确依赖只是接线点。
- 现状/证据：后续 `StatsSource`、table lookup、stats load 和 used-stats 信息可执行；`stats_aster_unit_test.rs` 覆盖 pseudo/partition/目标函数局部语义。
- 缺失行为/建议：真实 infoschema/statistics handle、partition physical ID 和错误传播需要集成测试；再决定删除。

## Index advisor 与测试 SDK

候选：`pkg/planner/indexadvisor/indexadvisor.rs:24-124`

- 分类：部分实现且注释仍描述缺口（高风险）。旧草稿含 session、statement summary、超时与 option 流程。
- 现状/证据：后续 `advise_indexes_for_sql`、recommendation、workload cost、命名可执行；`indexadvisor_test.rs`、`indexadvisor_sql_test.rs`、TPCH 测试覆盖指定 SQL、收益与名称。
- 缺失行为/建议：核对真实 statement summary 读取、session 生命周期和 timeout/cancel；内存 optimizer 测试不能替代生产接线。

候选：`pkg/planner/indexadvisor/model.rs:34-85`

- 分类：已实现重复档案（低风险）。
- 现状/证据：后续 `Query`、`Column`、`Index`、impact/cost 模型可执行；`model_test.rs` 覆盖 index prefix、key/equality 和排序。
- 建议：可删除草稿，保留大小写/前缀语义说明。

候选：`pkg/planner/indexadvisor/optimizer.rs:22-85`

- 分类：部分实现且注释仍描述缺口（高风险）。注释描述全局 QueryPlanCostHook 与真实 infoschema。
- 现状/证据：后续 `Optimizer` trait 和 `InMemoryOptimizer` 可执行；`optimizer_test.rs` 证明 what-if 局部行为，但生产 hook/session context 并未由内存实现证明。
- 建议：建立真实 planner cost hook 集成测试后删除归档。

候选：`pkg/planner/indexadvisor/options.rs:21-112`

- 分类：部分实现且注释仍描述缺口（中风险）。草稿包含 AST/session option 写入形状。
- 现状/证据：后续常量、`AdvisorOptions`、`OptionStore`、解析/校验可执行；`options_test.rs` 覆盖默认值、非法整数、持续时间和顺序写入。
- 建议：补 SQL `SET`/调用入口测试确认 session store 接线，再删除草稿。

候选：`pkg/planner/util/coretestsdk/mock.rs:21-276`

- 分类：部分实现且注释仍描述缺口（高风险）。Go mock domain/infoschema/context 的大段 parity 参考。
- 现状/证据：后续只提供轻量 `FieldType`、元数据、`InfoSchema`、`PlanContext`；`coretestsdk_aster_unit_test.rs` 覆盖构造器，但 memo 测试注释也明确它不能替代真实 `base::ContextRef`。
- 建议：明确该 SDK 是轻量 fixture 还是完整 Go mock 的移植；若是前者，删除误导性“完整 parity”档案并记录范围外能力。

候选：`pkg/planner/util/coretestsdk/testkit.rs:21-114`

- 分类：部分实现且注释仍描述缺口（高风险）。注释明确 parser/infoschema/session/domain 尚未接入。
- 现状/证据：后续 `get_field_value`、`PlannerSuite` 和构造 helper 可执行，测试只覆盖组装与文本提取。
- 建议：不要把它当真实 TiDB TestKit；要么接入真实 planner suite 并补 SQL 测试，要么重命名/记录轻量范围后删除草稿。

## Memo 测试

候选：`pkg/planner/memo/expr_iterator_test.rs:21-41`

- 分类：非执行测试档案（低风险），解释为何使用最小 `TestPlanContext`。
- 现状/证据：可执行 iterator 测试覆盖 group child 绑定、reset 组合枚举和 engine type 过滤；该注释属于迁移历史而非测试行为。
- 建议：压缩成最小 context 不会触达其他方法的一行安全说明。

候选：`pkg/planner/memo/group_test.rs:21-44`

- 分类：非执行测试档案（中风险）。说明 Go 测试使用完整 Parse→Resolve→BuildLogicalPlan，而 Rust 改用手工 group。
- 现状/证据：可执行测试覆盖 fingerprint 去重、插入/删除和 key info 局部行为，但没有覆盖 parser 到 memo 的集成路径。
- 建议：保留“缺少 BuildLogicalPlanForTest 集成覆盖”的短 TODO；删除迁移叙述前另建集成测试任务。

## 合法说明与生成器模板误报

候选：`pkg/planner/core/expression_rewriter.rs:4089-4112`

- 分类：合法说明/示例误报。
- 依据：这是 MATCH/FTS alternative rounds 的控制流说明，没有被注释掉的声明或实现；紧邻可执行 rewriter 分支，解释 direct-boolean 与 scoring context 差异。
- 建议：保留。

候选：`pkg/planner/core/generator/hash64_equals/hash64_equals_generator.rs:84-106`

- 分类：合法说明/示例误报。
- 依据：`CODE_GEN_HASH64_EQUALS_PREFIX` 是生成文件的许可证/头部模板，被 `generate` 写入输出，不是归档源码。
- 建议：保留，并在扫描器中排除名称为 `CODE_GEN_*_PREFIX` 的生成模板。

候选：`pkg/planner/core/generator/plan_cache/plan_clone_generator.rs:736-762`

- 分类：合法说明/示例误报。
- 依据：`CODE_GEN_PLAN_CACHE_PREFIX` 是 plan-clone generator 的输出前缀，属于运行数据。
- 建议：保留。

候选：`pkg/planner/core/generator/shallow_ref/shallow_ref_generator.rs:82-106`

- 分类：合法说明/示例误报。
- 依据：`CODE_GEN_LOGICAL_OP_COW_PREFIX` 是生成器输出模板。
- 建议：保留。

候选：`pkg/planner/funcdep/doc.rs:101-129`

- 分类：合法说明/示例误报。
- 依据：这是 multi embedded left join 的表格示例，与 Go `funcdep/doc.go` 的包级理论文档相同；没有代码声明，直接说明 Cond-FD/null-constraint 推理。
- 建议：保留；若去重应整体统一 Rust/Go 文档，而不是按“代码形注释”删除。

## 汇总与风险排序

- 候选总数：56；覆盖文件：46。
- 分类计数：合法说明/示例误报 5；非执行测试档案 15；已实现重复档案 12；部分实现且注释仍描述缺口 24；完全未实现占位 0。
- P0：`rule_partition_processor.rs`。归档 2208 行远大于当前紧凑实现，必须做逐函数/逐分区类型 parity，而不是直接清理。
- P1：`plan_test.rs`、`plan_clone_generated.rs`、`task_base.rs` 两块、TiFlash predicate pushdown、stats loading、collect-plan-stats、indexadvisor 生产 hook、coretestsdk 真实上下文。
- P2：join order 的 session hint/warning、ordered leading、outer-to-semi/order-aware/predicate/index-prune 规则注册和端到端 explain。
- 可优先清理：5 个误报应加入扫描排除；已有明确替代测试的原始字符串，以及 build-key/column-pruning/model 等低风险重复档案。

RustCodeGraph 用于确认 joinorder、rule、indexadvisor 的可执行符号与测试影响；精确查询确认 Rust `JoinOrder` 位于 `join_order.rs:401`，`advise_indexes_for_sql` 影响 14 个符号并由 `indexadvisor_sql_test.rs` 的多组 SQL 用例及 `indexadvisor_test.rs` 覆盖。由于 `rule`、`options` 等名称高度通用，宽查询会混入仓库其他子系统，因此最终调用/测试证据以精确路径下的可执行符号和相邻测试为准。此次是只读审计，未运行 Cargo，也未修改 Planner 生产代码或测试。
