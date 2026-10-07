# `pkg/planner/core/operator/logicalop/logical_apply.rs`

## 文件定位

本文件实现逻辑计划节点 `LogicalApply`。Apply 是关联子查询与 `LATERAL` 派生表在逻辑优化阶段的二元算子：左孩子提供外层行和关联列，右孩子在这些列绑定后执行，再按内嵌 `LogicalJoin` 的连接类型组合结果。计划构造入口包括 `pkg/planner/core/logical_plan_builder_runtime.rs` 的 `buildApplyWithJoinType`、`buildSemiApply` 和 LATERAL join 构造分支；优化阶段由 `pkg/planner/core/optimizer_runtime.rs` 进行列裁剪、去关联、聚合上拉判断和 join reorder 前的关联谓词整理。

该文件属于 Cargo crate `astersql-planner-core-operator-logicalop`；`pkg/planner/core/operator/logicalop/Cargo.toml` 指定 `lib.rs` 为库入口，并直接依赖 `base`、`cardinality`、`expression`、`fd`、`planctx`、`property` 等本文件使用的本地 crate。文件不是执行器：它描述并改写逻辑计划，不逐行运行 SQL。

## 核心职责

- 用 `LogicalApply` 在 `LogicalJoin` 的连接语义上增加 `CorCols`、`NoDecorrelate`、`IsLateral` 和 Rust 专用的 `PrunedToLeft` 状态。
- 在 `PruneColumns` 中先裁右支，再依据外层完整 schema 重建关联列集合，把关联列强制加入左支保留列，最后合并输出 schema；非 LATERAL 的左外 Apply 若右列完全无用，则标记为可消除。
- 在 `DeriveStats` 中区分普通标量子查询、LATERAL inner/left join、semi/anti join，估计输出行数与列 NDV，并维护外层可继承的 `GroupNDVs`。
- 通过 `ExtractCorrelatedCols`、`ExtractFD`、`CanPullUpAgg` 和 `DeCorColFromEqExpr` 为去关联、函数依赖和聚合改写提供判断材料。
- 在 join reorder 前，仅从没有聚合、行数限制、窗口或外连接屏障的内连接子树上提关联谓词，避免产生悬空列引用或破坏标量子查询语义。

## 主要符号

- `ApplyCardinalityContext<'a>`：把 `base::PlanContext` 适配为基数模块所需的 `cardinality::CardinalityContext`，转发 session、表达式和 ranger 上下文。
- `contains_aggregation`：递归判断子树是否包含 `LogicalAggregation`；当前精确调用搜索未发现消费方，属于尚未接线的辅助逻辑。
- `extract_correlated_columns` / `extract_correlated_columns_by_schema`：按具体逻辑算子调用其关联列提取实现，递归汇总后以外层 schema 的 `UniqueID` 对齐和去重。
- `lift_correlated_selections`：自底向上处理右子树的 `DataSource.PushedDownConds` 与 `LogicalSelection.Conditions`；引用外层列的条件经 `Decorrelate` 后上提，本地条件留在原位，空 Selection 被移除。
- `LogicalApply`：核心节点。`LogicalJoin` 持有孩子、schema、连接类型和条件；`CorCols` 是右支引用的外层列；`NoDecorrelate` 对应禁止去关联提示；`IsLateral` 保留多行 LATERAL 语义；`PrunedToLeft` 通知优化器以左孩子替换本节点。
- `Init`、`ExplainInfo`、`ReplaceExprColumns`：分别建立类型为 `Apply` 的基类、复用 Join explain 文本、同步替换 Join 表达式与关联列。
- `PruneColumns`、`DeriveStats`、`ExtractColGroups`、`ExtractCorrelatedCols`：LogicalPlan 生命周期中的列、统计和关联依赖处理。
- `LiftInnerCorrelatedSelectionsForJoinReorder`：join reorder 的安全预处理入口；直接调用者见 `optimizer_runtime.rs` 的 `lift_correlated_predicates_for_join_reorder` 与 `reorder_inner_join_descendants`。
- `ExtractFD`、`CanPullUpAgg`、`DeCorColFromEqExpr`：去关联/聚合改写辅助 API。`ExtractFD` 在 Join FD 上补充投影后内层列与原关联列的等价关系；`CanPullUpAgg` 检查聚合上拉的连接与唯一键前提；`DeCorColFromEqExpr` 识别 `eq(column, correlated-column)` 的两种参数顺序。
- `findChildFullSchema`：穿过单孩子 Selection，查找 Join/Apply 保存的未裁剪 `FullSchema`，避免 USING/NATURAL join 的冗余列过早消失。
- `impl LogicalPlan for LogicalApply`：完成 trait 分派；谓词下推与 key 信息委托 `LogicalJoin`，其余核心行为回到本类型。

## 执行流程

1. 构建器创建 `LogicalApply`，调用 `Init` 写入计划上下文与 query block offset，再设置 schema、输出名和两个孩子。普通关联子查询由 `buildApplyWithJoinType`/`buildSemiApply` 创建；LATERAL 分支还设置 `IsLateral=true` 和 `FullSchema`。
2. 逻辑列裁剪调用 trait 的 `PruneColumns`，转入固有方法：用 `LogicalJoin::ExtractUsedCols` 切分左右需求；只有“非 LATERAL + LeftOuterJoin + 无右列需求”才设置 `PrunedToLeft`。`optimizer_runtime.rs::eliminate_pruned_applies` 随后真正以左孩子替换节点。
3. 未消除时先裁剪右孩子，使用 `findChildFullSchema` 取得左侧完整 schema，递归重算 `CorCols`，把这些外层列并入左侧需求并按 `UniqueID` 排序去重，裁剪左孩子后调用 `MergeSchema`。
4. 统计派生首先复用缓存（但刷新 `GroupNDVs`）；需要重算时要求恰有两个孩子并递归派生其统计。LATERAL inner/left join 有等值 key 时调用 `EstimateFullJoinRowCount`，无 key 时使用“外行数 × 单次内层行数”；left join 至少保留外行数。Semi/Anti 使用 0.8 选择率，其余 Apply 默认等于外行数。
5. 统计结果复制左侧列 NDV；LeftOuterSemi/AntiLeftOuterSemi 的末尾布尔标记列 NDV 固定为 2，其余右侧输出列 NDV 以输出行数作为当前估计上界；符合左侧保留语义的连接继承外侧 `GroupNDVs`。
6. Join reorder 规则进入 `LiftInnerCorrelatedSelectionsForJoinReorder`。只有右支确实含 Join，且不含聚合、`MaxOneRow`、Limit、TopN、Window 或非 inner join，才递归上提关联 Selection/DataSource 条件，并去重后附加到 Join ON 条件。
7. 去关联流程在 `optimizer_runtime.rs` 查询 `CanPullUpAgg`，并消费 Apply 的 join/关联信息；若最终消除关联性，可将 Apply 改写为普通 Join。`NoDecorrelate` 会在上层规则中阻止该改写。

## 数据与状态

`LogicalApply` 的长期状态主要继承自 `LogicalJoin.LogicalSchemaProducer.BaseLogicalPlan`，包括上下文、孩子、输出 schema、输出名、统计和 FD。该文件维护的附加不变量如下：

- 正常节点应有两个孩子，索引 0 是外层、索引 1 是内层；`DeriveStats` 对数量不符返回错误。部分辅助方法对缺孩子采取空结果或提前返回，以容忍改写中的瞬时状态。
- `CorCols` 中的列必须来自左侧 schema。列裁剪时会用左侧真实列对象替换同 `UniqueID` 的递归提取结果，确保类型和元数据与外层一致。
- `IsLateral` 决定右支是否可能为每个外行产生多行；它既禁止“右列无用即删除 Apply”的标量优化，也改变基数公式。
- `PrunedToLeft` 只是改写请求，不直接改变孩子布局；真正替换在 `eliminate_pruned_applies` 完成，因此调用列裁剪后必须继续经过该清理阶段。
- `FullSchema` 属于内嵌 Join，用于保存 USING/NATURAL join 被输出裁剪隐藏的列；`findChildFullSchema` 只穿过单孩子 Selection。
- FD 等价关系来源于内层输出列的 `CorrelatedColUniqueID` 与新 `UniqueID`。`ExtractFD` 把结果缓存回 base，供后续规则使用。

## 依赖与调用关系

上游直接证据：

- `pkg/planner/core/logical_plan_builder_runtime.rs` 构造普通、semi/anti 和 LATERAL Apply，并填充 `CorCols`、join type、schema、输出名及提示状态。
- `pkg/planner/core/optimizer_runtime.rs` 在 join reorder 前调用 `LiftInnerCorrelatedSelectionsForJoinReorder`，在列裁剪后消费 `PrunedToLeft`，在去关联过程中调用 `CanPullUpAgg`。
- `pkg/planner/cascades/old/transformation_rules.rs` 克隆/转换 `LogicalApply`，保留 `NoDecorrelate`、`IsLateral` 和 `PrunedToLeft`。
- `LogicalPlan` trait 以动态分派调用本文件的 `PredicatePushDown`、`PruneColumns`、`BuildKeyInfo` 和 `DeriveStats`。

下游主要依赖：

- `LogicalJoin`：复用条件拆分、schema 合并、谓词下推、key 信息、FD 与 explain 表达。
- `expression`：提取普通/关联列、表达式去关联、重复条件消除和 schema 操作。
- `cardinality::EstimateFullJoinRowCount`：有等值连接键的 LATERAL 基数估计；`ApplyCardinalityContext` 提供所需上下文。
- `fd::FDSet` / `fd::intset`：记录关联列与内层重新投影列的函数依赖等价关系。
- `property::StatsInfo`：承载行数、单列 NDV 与列组 NDV。

RustCodeGraph 索引（11467 文件、307296 节点）确认了目标文件及上述构建器、优化器、测试符号。由于 `PruneColumns`、`DeriveStats` 等名称在 Go/Rust 多算子中高度重载，图的裸名称 callees 查询返回多候选；本文对这些边以精确路径源码交叉核验，不把模糊候选当成唯一调用关系。

## 错误处理与边界

- `PruneColumns` 使用 `Result<()>` 传播左右孩子裁剪错误；缺少某个孩子时部分步骤跳过，但后续索引访问假设结构完整，因此生产构建器必须保持二元节点不变量。
- `DeriveStats` 明确拒绝非两个孩子的 Apply，错误为 `LogicalApply requires two children`；带 join key 的 LATERAL 若缺少 plan context，则返回 `LogicalApply has no plan context`。
- LATERAL left join 的行数通过 `max(估计值, 外行数)` 保留 null-extension 下限；普通 scalar Apply 不乘以内层行数，因为其语义由最多一行保证约束控制。
- Semi/Anti 当前固定使用 0.8，属于经验估计而非精确选择率；修改会影响物理计划选择。
- `DeCorColFromEqExpr` 仅接受函数名严格为 `eq` 且恰有两个参数；尽管注释提到 eq/nulleq，当前代码不接受 `nulleq`，文档以实现为准。它只返回列对，不像 Go 版本那样创建新的布尔等值表达式。
- join reorder 的谓词上提主动拒绝聚合、行数屏障、窗口与外连接；跨越这些节点需要 Go 去关联器中的 FIRST_ROW/null-extension 等额外改写，当前函数不会猜测执行。
- `lift_correlated_selection_below_max_one_row` 是私有辅助函数，当前精确搜索未发现调用；它描述了 MaxOneRow→Selection 的局部上提逻辑，但不能据此声称生产流程已接线。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件描述符或网络资源。所有改写都在调用方持有的可变逻辑计划树上同步完成。

资源生命周期体现在树所有权转移：`TakeChildren` 暂时移出孩子，递归改写后必须用 `SetChildren` 放回；移除空 Selection 时以唯一孩子替换原节点。条件向量通过 `std::mem::take` 转移，避免克隆并保证每个条件最终只进入“本地保留”或“上提”之一。`ContextRef` 在 `Init` 中保存，在统计估计时只克隆引用计数句柄；本文件不负责关闭外部资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_apply.go`。两者共同保留 `LogicalJoin`、`CorCols`、`NoDecorrelate`、`IsLateral`，并对齐 Explain、表达式替换、列裁剪、统计、列组、外部关联列过滤、FD、聚合上拉与等值关联列识别等职责。

主要差异：

- Go 的 `PruneColumns` 可直接返回左孩子；Rust trait 原地修改，故增加 `PrunedToLeft`，由 `optimizer_runtime.rs::eliminate_pruned_applies` 完成替换。
- Go 通过 fix-control 45822 控制 Apply 消除；当前 Rust 方法没有读取该开关，只保留非 LATERAL、LeftOuter、右列无用的语义门槛。
- Rust 增加了 join reorder 前的安全谓词上提与屏障检查；其注释明确指出尚未实现跨聚合/行数屏障/外连接所需的完整 Go 改写。
- Rust `ExtractFD` 先调用 `LogicalJoin.ExtractFD` 再增加等价关系；Go 按 join type 调用专用 FD 方法，并对不支持类型返回空集。后续调整应验证两者对各 join type 的结果一致性。
- Go `DeCorColFromEqExpr` 返回新表达式并验证关联列能在 Apply schema 中去关联；Rust仅返回 `(外层列, 内层列)`，且当前调用搜索未发现生产消费方，不能视作完整替代。
- 当前 Rust `contains_aggregation` 与 `lift_correlated_selection_below_max_one_row` 没有发现直接调用；它们是已有但尚未接入的局部能力。

## 扩展指南

- 新增 Apply 字段时，同步检查 `Default` 构造、`logical_plan_builder_runtime.rs` 的普通/LATERAL 构造、`cascades/old/transformation_rules.rs` 的克隆，以及 hash/equals 生成逻辑；语义字段不可在转换时丢失。
- 修改列裁剪时保持顺序：先右支、重算 `CorCols`、把关联列加入左支、再合并 schema。必须覆盖 LATERAL 多行语义、`FullSchema` 下 USING/NATURAL 隐藏列和 `PrunedToLeft` 的消费阶段。
- 修改统计时分别测试 scalar、LATERAL inner、LATERAL left、semi/anti、left-outer-semi marker；有 key 与无 key 的 LATERAL 路径不能合并，因为右侧统计已包含关联谓词选择率，重复除以关联列 NDV 会二次折减。
- 扩大谓词上提范围前，必须实现对应的 MaxOneRow、聚合、Limit/TopN/Window、外连接 null-extension 语义，而不能只删除 `contains_barrier` 条件。
- 修改 `ExtractFD` 或 `DeCorColFromEqExpr` 时应逐 join type 对照 Go 行为，并检查去关联规则的真实调用；若启用当前未接线辅助函数，应新增独立 Rust 测试而不是把测试写入本源文件。
- 测试应优先扩展同目录 `logical_apply_test.rs`；跨规则行为可扩展 `logical_relational_aster_unit_test.rs` 或 `pkg/planner/core/optimizer_runtime.rs` 对应的独立测试模块。遵守仓库要求，Rust 测试不得内嵌到生产源文件。

## 验证依据

- 目标源码：`pkg/planner/core/operator/logicalop/logical_apply.rs`，已核对全部模块级结构、`LogicalApply` 字段、固有方法和 `LogicalPlan` 实现。
- RustCodeGraph：运行 `status`、目标文件 `node`、`explore`，并查询 `LogicalApply`、`PruneColumns`、`DeriveStats`、`LiftInnerCorrelatedSelectionsForJoinReorder`、`ExtractFD`、`CanPullUpAgg`、`DeCorColFromEqExpr`、`findChildFullSchema`；调用图的重名歧义已由精确路径搜索补证。
- Crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml`。
- 构造与调用：`pkg/planner/core/logical_plan_builder_runtime.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/cascades/old/transformation_rules.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_apply.go`。
- 独立 Rust 测试：`pkg/planner/core/operator/logicalop/logical_apply_test.rs` 验证外侧列组传播、left-outer-semi marker NDV、Apply 自有 CorCols 不作为外部依赖；`pkg/planner/core/operator/logicalop/logical_relational_aster_unit_test.rs` 验证 LATERAL 单次内层基数相乘及 left join 外行下限。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令和人工事实复核验收。
