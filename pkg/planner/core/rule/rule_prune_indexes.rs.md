# `pkg/planner/core/rule/rule_prune_indexes.rs`

## 文件定位

本文件属于 `astersql-planner-core-rule` crate；crate 入口 `pkg/planner/core/rule/lib.rs` 以公开模块 `rule_prune_indexes` 导出它。它实现逻辑优化阶段的“第一阶段索引剪枝”：在真正构造 range 和枚举物理访问路径之前，根据 WHERE、JOIN、ORDER BY 等来源汇总出的感兴趣列，对 `DataSource` 的候选表扫/索引扫路径做保守筛选，降低后续统计加载、range 构造和物理计划枚举成本。

生产调用链并不直接把完整的 `logicalop::DataSource` 传入本文件。`pkg/planner/core/optimizer_runtime.rs::prune_data_source_indexes` 先把真实 planner 对象投影成这里的 `DataSource`、`AccessPath`、`IndexInfo` 等轻量类型，调用 `prune_indexes_by_where_and_order`，再按返回路径的稳定 `id` 回写 `AllPossibleAccessPaths`、`PossibleAccessPaths` 和 `IsSingleScan`。因此，本文件是可执行规则核心，`optimizer_runtime.rs` 是适配和接线层。

文件第 25–602 行是块注释中的迁移参考草稿，不参与编译；活动 Rust 实现从 `use std::cmp::Ordering` 开始。分析当前行为时必须以活动实现和实际调用者为准。

## 核心职责

- `prune_indexes_by_where_and_order` 对路径分类：表路径、多值索引以及 Index Merge 提示显式点名的索引是必留类别；遇到普通强制路径则直接放弃本轮剪枝并原样返回。
- `score_index_path` 计算候选索引覆盖多少感兴趣列、从有效索引键起点连续覆盖多少列、覆盖了哪些列，以及是否覆盖了“非折扣”列。有效键可由调用者通过 `DataSource::effective_index_columns` 提供，从而包含真实索引列和追加的 clustered handle 列。
- `score_and_sort` 与 `calculate_score_from_coverage` 按覆盖数、连续前缀、全覆盖和单扫能力排名；排序还以是否可丢弃、连续前缀长度、键宽和索引 ID 作稳定决胜。
- `build_final_result` 先合并必留路径，再依据阈值进行“只删零价值路径”或有上限的两阶段选择；`IndexSelectionState` 防止保留覆盖集合相同但前缀被既有索引支配的冗余候选，同时保留不同访问次序。
- 所有剪枝都偏向安全：禁用阈值、路径过少、强制路径以及无法形成安全结果时返回原路径；静态剪枝信息不足时不虚构连续前缀。

## 主要符号

- `DEFAULT_MAX_INDEXES: usize = 10`：正阈值模式下的最低候选保留上限，`max_to_keep = max(threshold, 10)`。
- `IndexColumn { offset }`、`IndexInfo { id, name, columns, multi_value, condition_expression, affected_column_offsets }`：从真实元数据投影出的索引定义；偏移相对于 `DataSource::table_columns`。
- `AccessPath`：规则使用的候选路径值对象。`id` 代替 Go 指针身份并供调用者回写；`full_index_columns == None` 表示静态剪枝回退；其中的单个 `None` 表示无法解析的索引列；`single_scan` 表示无需回表。
- `IndexMergeHint`、`DataSource`：规则输入上下文。后者携带表列 ID、提示、fix control 52869 的计算结果、等值/IN 约束的 clustered-key 前缀集合以及按路径 ID 给出的有效索引键。
- `should_prefer_index_merge`：存在任意 Index Merge hint 或 `fix_52869` 为真时返回真。
- `prune_indexes_by_where_and_order`：公开主入口，消费路径所有权并返回保留路径。
- `IndexWithScore`、`ScoredIndex`：内部覆盖统计与排序记录。`droppable` 表示候选只覆盖已由 clustered table path 服务的折扣前缀且不是单扫；`covered_key` 是排序后的覆盖列集合，用于支配判断。
- `score_index_path`：部分索引预检、有效键遍历和静态回退打分入口。
- `score_and_sort`、`compare_scored`：生成非零分记录并确定可复现顺序。
- `IndexSelectionState::{new,is_dominated,record_coverage}` 与 `should_add_index`：维护两阶段名额、已见连续列以及“覆盖集合 → 已选前缀”映射。
- `find_single_interesting_column`：为无连续前缀、只命中一个兴趣列的候选寻找非折扣列；信息不足返回 `None` 并由上层保守保留。
- `calculate_score_from_coverage`：分数为 `命中数×10 + 连续前缀数×10 + 全覆盖奖励10 + 单扫奖励20`；空兴趣集不会获得全覆盖奖励。
- `build_ordering_key`：公开辅助函数，把列 ID 顺序编码成逗号分隔字符串；当前活动选择逻辑使用 `Vec<i64>` 作为键，该函数主要保留 Go 对应 API/测试用途。

## 执行流程

1. `optimizer_runtime.rs` 在收集 DataSource 的感兴趣列后读取 `tidb_opt_index_prune_threshold`，构造本文件需要的投影对象。它解析 pushed-down 的等值、null-safe 等值与全常量 IN 条件，得到可折扣的 clustered-key 连续前缀；还用 `HandleColsToAppend` 构造每条路径的有效键，并在调用前计算 `single_scan`。
2. 主入口在路径不超过一条或 `threshold < 0` 时原样返回。`threshold == 0` 或阈值大于总路径数时设置 `only_prune_zero_score`；其他非负阈值进入限额选择，但限额至少为 10。
3. 遍历路径时，表路径直接进入 `table_paths`，多值索引进入 `mv_index_paths`。没有索引元信息的非表路径被忽略；普通索引若 `forced` 为真则整批原样返回。由于分类顺序如此，表路径或多值索引上的 `forced` 不会取消其他路径剪枝。
4. `score_index_path` 先检查部分索引条件：非空条件表达式涉及的任一列偏移越界，或对应列不在兴趣集合中，都会让该候选保持零覆盖。正常模式优先读取 `effective_index_columns[path.id]`，否则读取 `full_index_columns`；仅两者都没有时才按索引列偏移映射 `table_columns`，且这种静态回退不推断连续前缀。
5. 有效键扫描会统计所有兴趣列；只有键位置恰好等于当前连续长度时才扩展连续前缀，因此第一个非兴趣位置会永久截断前缀。负数哨兵和 `None` 不计分。每个命中列同时决定候选是否到达折扣 clustered-key 前缀之外。
6. Index Merge 显式名称用 `unicode_equal_fold` 做 Unicode 大小写不敏感匹配，命中的路径进入必留列表；只有一般性提示或 fix control 时，具备任意覆盖或单扫能力的路径进入优选池。普通路径同样只有命中兴趣列或单扫时才进入优选池。
7. `build_final_result` 按输入分类顺序先加入表路径、多值索引和显式 Index Merge 路径，并用稳定路径 `id` 去重。优选池去除零分项，生成 `droppable` 和排序键后降序排列。
8. 只删零价值模式会保留所有非 `droppable` 的有分候选。限额模式把一半名额作为第一阶段高分池，其余名额用于覆盖多样性；即使第一阶段仍会跳过已被同覆盖集合、更长或相等连续前缀支配的候选。第二阶段也做相同支配检查；无连续前缀且只命中一列时，如果该列已被连续前缀覆盖则仅单扫候选仍可加入。
9. 若结果为空，或结果只有表/MV 路径、没有优选索引且不存在折扣列，主入口回退到原路径。存在折扣列时，只有表路径可能是有意结果，因为只覆盖 clustered-key 前缀的二级索引被判定为冗余。
10. 调用者按返回的 `AccessPath::id` 克隆真实路径，更新 `AllPossibleAccessPaths`；再以索引 ID 收缩 `PossibleAccessPaths` 并同步 `IsSingleScan`。随后才收集统计加载项，说明剪枝结果会直接减少下游工作量。

## 数据与状态

规则是一次调用内的纯内存变换，没有全局可变状态。公开入口取得 `Vec<AccessPath>` 所有权，但内部主要以原切片下标表示候选，最终只克隆被选中的路径。`AccessPath::id` 是跨适配边界的稳定身份；生产调用者当前把原路径位置编码为 `u64`，回写时又把它作为原向量下标，因此调用者必须维持“ID 可安全索引原向量”的不变量。

`ColumnRequirements::interesting` 和 `discounted` 使用 `BTreeSet<i64>`，分别表示完整兴趣集合和其中特殊的 clustered-key 等值/IN 连续前缀。折扣列仍参与覆盖和连续前缀计分，只在 `covers_non_discounted`/`droppable` 判定中用于剔除仅重复表路径能力的索引。

去重集合 `added: HashSet<u64>` 按路径 ID 处理必留和优选路径。`IndexSelectionState` 的 `remaining` 只约束优选索引数量，不限制预先加入的表路径、MV 索引和显式提示索引；因此最终结果数可以超过 `max_to_keep`。`covered_key` 由覆盖列 ID 排序得到，是与顺序无关的集合签名；只有覆盖集合相同的候选之间才比较连续前缀支配关系。

## 依赖与调用关系

上游直接生产调用者是 `pkg/planner/core/optimizer_runtime.rs::prune_data_source_indexes`；RustCodeGraph 的 callers 查询没有返回边，因此该调用边由精确源码检索和调用点 `prune_indexes_by_where_and_order(&requirements, paths, &interesting, threshold)` 核实。测试调用者集中在独立文件 `pkg/planner/core/rule/rule_prune_indexes_test.rs`。`pkg/planner/core/rule/rule_collect_plan_stats.rs` 只包含迁移注释中的旧式调用示意，不是活动调用边。

本文件活动代码只直接依赖标准库的 `Ordering`、`BTreeSet`、`HashMap` 和 `HashSet`，并定义自己的边界 DTO；它没有直接导入 `logicalop`、`expression`、`model` 或 planner util。完整依赖由适配层承担。`pkg/planner/core/rule/Cargo.toml` 声明 crate 名 `astersql-planner-core-rule`、入口 `lib.rs`，常规依赖包含 meta model、logicalop 和 rule util，另有大量仅在 `cfg(windows)` 下启用的迁移依赖；本文件当前并不直接使用这些 crate 依赖，也没有自身条件编译项。

下游内部调用关系是：主入口调用 `should_prefer_index_merge`、`score_index_path`、`index_is_explicitly_hinted` 和 `build_final_result`；`build_final_result` 调用 `score_and_sort`、`IndexSelectionState::new`、`should_add_index`；排序调用 `calculate_score_from_coverage` 与 `compare_scored`；选择过程调用状态的支配/记录方法及 `find_single_interesting_column`。最终消费者是适配层对真实 DataSource 路径集合的回写，以及紧随其后的统计加载与后续 range/物理计划流程。

## 错误处理与边界

本文件没有 `Result`、错误类型、panic 分支、日志或 failpoint；异常输入通过跳过候选或保守回退处理。部分索引的受影响列偏移使用 `get`，越界直接产生零覆盖；索引列偏移静态回退同样安全跳过。缺失 `index` 的非表路径不进入结果，但若这导致没有任何安全候选，最终空结果保护会恢复原输入。

关键阈值语义是：负数完全禁用；零只保留有价值索引和必留路径；大于总路径数也采用“只删零价值路径”；其余正数将索引选择上限设为 `max(threshold, 10)`。表路径、MV 索引和显式 Index Merge 提示不消耗这个上限。任何普通 forced 索引让整批不剪枝，但 forced 的表/MV 路径因更早分类而不会触发该保护，这一点由 Rust 独立测试固定。

静态模式缺少 `full_index_columns` 时无法证明前缀顺序，也无法定位 `find_single_interesting_column` 的具体列；实现分别选择“不记录连续前缀”和“返回 None 后允许保留”，避免因信息不足过度剪枝。空的部分索引条件不触发约束预检；非空条件的无效偏移则得到零覆盖。Unicode hint 匹配逐字符比较原字符、小写迭代和大写迭代，测试覆盖了 Å/å 与希腊 Σ/ς 的组合。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量、通道、事务、I/O 或外部句柄。所有集合、打分记录和选择状态都在单次函数调用栈及其拥有的堆对象内创建，返回后自动释放；因此规则自身没有跨调用生命周期或并发共享问题。

生产接线在可变借用单个 `logicalop::DataSource` 的期间完成投影、调用和回写。调用规则前读取真实路径，规则返回后 `std::mem::take` 暂时取得 `AllPossibleAccessPaths` 所有权，再按稳定位置 ID 重建它；这一顺序避免同时持有冲突借用，也要求剪枝函数不能生成调用者无法映射的新路径 ID。规则只是候选缩减，不管理统计加载任务或存储资源；这些生命周期由调用者后续流程负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/rule/rule_prune_indexes.go`。Rust 的 `DEFAULT_MAX_INDEXES`、路径分类、部分索引预检、覆盖打分、零分模式、两阶段选择、覆盖支配、稳定索引 ID 排序和安全回退，对应 Go 的 `defaultMaxIndexes`、`PruneIndexesByWhereAndOrder`、`scoreIndexPath`、`scoreAndSort`、`buildFinalResult`、`shouldAddIndex` 等逻辑。

两种实现的数据边界不同。Go 版本直接操作 `logicalop.DataSource`、`util.AccessPath`、`expression.Column` 和 `model` 元数据，并在本文件内执行 `collectEqOrInBoundColIDs`、`discountedHandlePrefixCols`、`effectiveIndexColumnIDs` 及 `IsSingleScan`。Rust 把这些 planner 依赖留在 `optimizer_runtime.rs::prune_data_source_indexes`，再把已计算的 `discounted_column_ids`、`effective_index_columns` 和 `single_scan` 输入轻量规则。Rust 活动实现因此没有对应的三个公开/私有解析辅助函数，但端到端职责仍由接线层加规则核心共同覆盖。

Go 用指针作为路径去重身份、字符串作为覆盖集合键；Rust 用 `AccessPath::id` 和排序后的 `Vec<i64>`。Go 的 `strings.EqualFold` 由 Rust 本地 `unicode_equal_fold` 承担。Go 在规则内更新 `path.IsSingleScan` 并有 `InjectCheckForIndexPrune` failpoint；Rust 由调用者预计算/回写单扫标记，活动实现没有该 failpoint。后续若要求严格的调试注入对齐，需要在适配边界或规则返回点另行设计，不能把块注释中的 Go 调用视作已生效。

Go 独立测试 `rule_prune_indexes_internal_test.go` 验证未解析索引列不会错误追加 handle，以及部分索引坏偏移得到零分。Rust 独立测试 `rule_prune_indexes_test.rs` 进一步固定静态模式不虚构前缀、空部分索引条件、forced MV 分类、Unicode hint、同覆盖候选选择、折扣前缀冗余、追加 handle 的不同访问顺序及坏偏移行为。

## 扩展指南

- 新增或改变剪枝维度时，优先扩展 `IndexWithScore`/`ScoredIndex`、`score_index_path`、`calculate_score_from_coverage` 与 `compare_scored`，并检查是否影响支配关系。修改权重必须评估索引枚举数量、统计加载量和计划稳定性，不能仅凭编译通过判断正确。
- 需要更多真实 planner 信息时，应在 `optimizer_runtime.rs::prune_data_source_indexes` 增加明确投影字段，并同步本文件的 DTO；不要在本规则中复制完整 planner 子系统。新增字段必须保持 `AccessPath::id` 映射和有效键与 `HandleColsToAppend`/range 构造的一致性。
- 改变必留类别或阈值语义时，应同时复核空结果保护、折扣列例外、forced 路径顺序、显式 Index Merge 提示和“必留路径不计入上限”这些安全不变量。
- 改变覆盖支配时，必须区分“相同覆盖集合的前缀支配”和“覆盖更多列”；当前设计明确不让更大覆盖集合支配更窄索引，以保留 cost model 的选择空间。
- 测试必须继续放在独立的 `pkg/planner/core/rule/rule_prune_indexes_test.rs`，不要嵌入源文件。涉及适配计算时还应同步覆盖 `optimizer_runtime.rs` 的现有上层测试；Go 语义变化则对照并同步 `rule_prune_indexes.go` 与 `rule_prune_indexes_internal_test.go`。
- 性能风险主要来自路径遍历、每个候选的覆盖向量排序以及支配映射增长；兼容风险主要来自 hint 大小写、partial index、common handle/PK handle、静态剪枝和可复现排序。

## 验证依据

- RustCodeGraph：`status` 显示项目索引含目标文件；`files --filter pkg/planner/core/rule` 显示目标 Rust/Go/测试文件；`node --file pkg/planner/core/rule/rule_prune_indexes.rs --offset 1 --limit 500` 与 `--offset 495 --limit 700` 读取了完整 1123 行和 36 个符号；`query prune_indexes_by_where_and_order --kind function` 定位入口第 707 行。`callers`/`callees` 未返回边，故按技能规则以精确源码检索补证。
- Rust 源与接线：`pkg/planner/core/rule/rule_prune_indexes.rs`、`pkg/planner/core/rule/lib.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/rule/rule_collect_plan_stats.rs`。
- crate 边界：`pkg/planner/core/rule/Cargo.toml`。
- Go 对照：`pkg/planner/core/rule/rule_prune_indexes.go`；测试：`pkg/planner/core/rule/rule_prune_indexes_internal_test.go`。
- Rust 独立测试：`pkg/planner/core/rule/rule_prune_indexes_test.rs`，共八个 `#[test]` 场景，覆盖本文列出的主要边界。
- 精确调用检索确认 `pkg/planner/core/optimizer_runtime.rs:2303` 是活动生产调用点；`lib.rs` 公开模块并以 `#[cfg(test)] mod rule_prune_indexes_test` 接入独立测试。
- 本任务仅新增说明文档，按计划不运行 Cargo。交付时运行任务规定的 11 章节结构命令，并人工复核源码链接、活动代码/注释草稿边界、调用链和 Go/Rust 差异。
