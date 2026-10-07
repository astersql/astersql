# `pkg/planner/core/operator/logicalop/logical_aggregation.rs`

## 文件定位

本文件实现 Rust 规划器的逻辑聚合算子 `LogicalAggregation`，表示 SQL 中的全局聚合、`GROUP BY` 聚合以及 `SUM`/`COUNT`/`MIN`/`MAX`/`FIRST_ROW` 等聚合函数在物理计划生成前的节点形态。它属于 crate `astersql-planner-core-operator-logicalop`（`pkg/planner/core/operator/logicalop/Cargo.toml`），由 `lib.rs` 声明为私有模块后将公开符号重导出。

真实主链上，`logical_plan_builder_runtime.rs::build_aggregation_runtime`构造节点，填充 `AggFuncs`/`GroupByItems`/输出 schema/子计划与 hint，并开启聚合相关优化规则。`optimizer_runtime.rs::logical_optimize_in_place` 随后通过 `LogicalPlan` trait 调用列裁剪、键推导与谓词下推。本文件不执行聚合计算；物理算子选择和执行属于后续层。

## 核心职责

- 保存聚合描述符、分组表达式、聚合 hint、可能的有序属性、子输入行数和 coprocessor 下推禁止位（`LogicalAggregation`）。
- 将只依赖分组键或可回写的 `firstrow` 输出的谓词下推到聚合之下，对 CNF/DNF 保留必要的上层残差（`splitCondForAggregation`、`split_cnf_for_aggregation`、`split_dnf_for_aggregation`）。
- 根据父节点需求裁掉无用聚合输出，同时保留副作用、空输入基数语义和子节点必需列（`PruneColumns`）。
- 推导唯一键/`MaxOneRow`、函数依赖、分组列组、分组行数与输出 NDV（`BuildKeyInfo`、`ExtractFD`、`ExtractColGroups`、`DeriveStats`）。
- 为物理化提供流聚合有序属性、分区键、DISTINCT/ORDER BY/模式/hint/coprocessor 能力查询。

## 主要符号

- `AggregationCardinalityContext<'a>`：将 `base::PlanContext` 的会话、表达式与 ranger 上下文适配为 `cardinality::CardinalityContext`，仅供统计估算使用。
- `LogicalAggregation`：主状态结构。`LogicalSchemaProducer` 内含公共逻辑计划基座；`AggFuncs[i]` 通常与 `Schema().Columns[i]` 对齐；`GroupByItems` 保存分组表达式；`PreferAggType`/`PreferAggToCop` 保存 hint；`PossibleProperties` 缓存分组可利用的排序前缀；`InputCount` 记录子统计行数；`NoCopPushDown` 是额外禁止位。
- `Init`：用类型名 `Aggregation` 初始化 `BaseLogicalPlan`并分配 plan ID。
- `ExplainInfo`、`ReplaceExprColumns`：分别生成简要 explain 文本，以及按 `UniqueID` 递归替换聚合参数、聚合 `ORDER BY` 和分组表达式中的列。
- `PredicatePushDown`/`splitCondForAggregation`：对外的谓词边界与核心拆分器；`predicate_is_group_invariant` 以列 ID 集合判定安全性，`substitute_aggregate_outputs` 将单参 `firstrow` 输出递归替换回子表达式。
- `PruneColumns`：保持 schema/聚合函数索引对齐，处理副作用、占位聚合、常量分组项和子列需求。
- `BuildSelfKeyInfo`/`BuildKeyInfo`、`DeriveStats`、`ExtractFD`：分别推导 schema 键、计算分组基数、建立“分组键严格决定非 firstrow 输出”的 FD。
- `GetGroupByCols`、`GetUsedCols`、`ExtractCorrelatedCols`、`GetPotentialPartitionKeys`：为优化规则提供直接分组列、引用列、关联列和分区键。
- `HasDistinct`、`HasOrderBy`、`IsPartialModeAgg`、`IsCompleteModeAgg`、`CanPullUp`、`DistinctArgsMeetsProperty`、`CanPushToCop`：对外暴露聚合形态和可改写能力。
- `impl LogicalPlan for LogicalAggregation`：将 explain、谓词下推、列裁剪、键信息与统计推导接入 trait-object 优化主链。

## 执行流程

1. 计划构建器把 AST 聚合函数和 `GROUP BY` 转成 `AggFuncDesc`/`Expression`，调用 `Init`，然后安装 schema、输出名、唯一子计划与 hint（`logical_plan_builder_runtime.rs:3923-3946`）。
2. 谓词下推时，`splitCondForAggregation` 建立 `firstrow` 输出 ID 到原参数的替换表，递归拆 CNF/DNF。一个 DNF 的每个分支都必须至少有可下推部分，才能组成放松后的 DNF；任一分支完全不可下推时保留原条件。如果替换后仍有非冗余残差，聚合上方保留原始谓词，不在聚合边界评估替换后的子表达式。
3. `PredicatePushDown` 将可下推部分交给第一个子计划的 `PredicatePushDownPlan`，将子节点返回的残差与聚合边界残差合并后返回上层。`LogicalSelection::PredicatePushDown` 也在特殊的恒假 Selection/聚合组合中直接调用该拆分器。
4. 列裁剪时，`PruneColumns` 同步过滤输出 schema 和 `AggFuncs`；含副作用参数的函数不能删除。若全被裁掉，先尝试保留参数与分组项相等的 `firstrow`，否则新建 `count(1)` 或全 firstrow 时的 `firstrow(1)` 占位输出，以保持空输入的行数语义。无列且无副作用的分组项被删除；原本有分组但全部被删除时补常量 `1`。最后汇总仍被聚合参数、聚合 `ORDER BY` 和分组项引用的列，递归裁剪子计划。
5. 键与统计阶段，非 `Partial1` 聚合先继承 schema 生产者的键信息，再用“全部分组项均为直接列”推导输出唯一键；无分组项则设置 `MaxOneRow`。`DeriveStats` 递归获取子统计，用 `EstimateColsNDVWithMatchedLen` 估算分组行数，将该数值作为每个输出列的保守 NDV，并保存匹配的 `GroupNDVs`。
6. 物理计划候选生成前，`PreparePossibleProperties` 从子节点排序中保留覆盖全部分组列的前缀，无分组项时提供空属性；其他查询方法为 hash/stream、partial/complete、coprocessor 下推和分区要求提供决策信息。

## 数据与状态

`LogicalAggregation` 是可变的计划节点，关键不变量是 `AggFuncs` 与输出 `Schema().Columns` 按索引对齐；`splitCondForAggregation`、`ExtractFD`、`getAggFuncsColsForConstResult` 和 `getAggFuncsColsForFirstRow` 都依赖这一关系。`IsPartialModeAgg`/`IsCompleteModeAgg` 直接访问 `AggFuncs[0]`，因此空函数列表对这两个 API 是非法状态，会 panic，而不是返回 `false`。

表达式对象在计划改写时通过 `CloneExpr`、`clone_scalar` 或 `Clone` 复制；对标量函数替换参数后会调用 `CleanHashCode`，避免使用与新参数不一致的缓存 hash。列身份通常以 `UniqueID` 比较；分组列提取时 `GetGroupByCols` 只接受直接列，保留顺序和重复项，而 `GetUsedCols` 会按 ID 排序去重。

`InputCount`、计划统计、schema 键/FD、`PossibleProperties` 都是优化阶段的派生状态；更改分组项、聚合函数或子计划后，调用方必须确保相关派生信息重新计算，不能沿用过期缓存。

## 依赖与调用关系

上游直接证据包括：

- `logical_plan_builder_runtime.rs::build_aggregation_runtime` 在 SQL 计划构建时创建本类型。`expression_rewriter.rs` 也会为子查询/表达式改写构造聚合节点。
- `optimizer_runtime.rs::logical_optimize_in_place` 通过 trait 调用 `PruneColumns`、`BuildKeyInfo` 和谓词下推主链；`logical_selection.rs` 直接调用 `splitCondForAggregation`处理 Selection 与聚合的边界。
- RustCodeGraph 的文件反向引用列表还包含 `logical_apply.rs`、`logical_join.rs`、`logical_projection.rs`、生成的 hash/equals 和浅引用实现等；精确方法 caller/callee 查询未产生边，因此本文档不将缺失的方法级图边当作已验证事实。

下游主要依赖为 `aggregation` 的 `AggFuncDesc`/模式与描述符构造、`expression` 的表达式拆分/组合/列提取/空值评估、`cardinality` 的多列 NDV 估算、`property::StatsInfo` 的统计状态、`fd::FDSet` 的函数依赖、`base`/`planctx` 的计划上下文。`Cargo.toml` 还声明了本 logicalop crate 的完整工作区依赖，但本文件的显式路径主要经 `crate::*`、`base`、`cardinality`、`planctx`、`mysql` 和重导出的 `aggregation`。该 crate 没有特性开关，且设置 `autotests = false`；测试由 `lib.rs` 中的 `#[cfg(test)] mod ...` 显式接入。

## 错误处理与边界

- `PredicatePushDown`/`PruneColumns`/`DeriveStats` 用 crate 级 `Result<T, PlannerError>` 传播子计划或聚合描述符构造错误。创建占位聚合时，缺少计划上下文会返回 `aggregation has no plan context`，`NewAggFuncDesc` 错误被转换为 `PlannerError`。
- DNF 组合之前显式检查计划上下文；无上下文时不报错，而是放弃下推并保留原谓词。在已确认分支非空后，`ComposeCNFCondition`/`ComposeDNFCondition` 使用 `expect`；这是内部不变量失败而非可恢复计划错误。
- `IsPartialModeAgg`/`IsCompleteModeAgg` 对空 `AggFuncs` panic，这是与 Go 直接索引首项一致的 fail-fast 合同；调用前需保证节点状态有效。`BuildKeyInfo` 也会调用该判断，与 Go 对空列表的特判存在差异，扩展或构造异常聚合时必须特别测试。
- `AggFuncs` 与 schema 长度或顺序不一致会导致输出判定错位；部分路径使用 `get` 安全退化，另一些路径根据构建器不变量直接按索引访问。
- `predicate_is_group_invariant` 要求谓词至少引用一列，所以纯常量谓词在当前 Rust 实现中不会被这一路径当作可下推；这与 Go 将常量谓词同时下推和保留的逻辑不完全一致。
- `CanPullUp` 对表达式空值评估错误采用保守 `false`；有 `GROUP BY` 也立即返回 `false`。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部 I/O 资源。`LogicalAggregation` 在计划构建和优化期间由当前计划树独占可变访问；子计划通过 `Box<dyn LogicalPlan>` 持有，替换或裁剪随树的所有权完成。

`base::ContextRef` 是共享上下文引用，`AggregationCardinalityContext` 只在 `DeriveStats` 调用期间借用它；适配器不延长资源生命周期。新建占位输出列时通过表达式上下文分配 plan column ID；其唯一性依赖上下文分配器，不应手工复用旧 ID。因为整个对象是可变状态，当前 API 不支持对同一节点并发运行优化通道；若未来并行化，需先分离可变缓存和计划树改写权。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_aggregation.go`。结构主字段、`Init`、谓词下推、列裁剪、键/统计/列组/排序属性/FD 推导、coprocessor 禁止位、模式判定和 `CanPullUp` 都有对应概念。Rust 通过 `pub use aggregation::{AggFuncDesc, AggFunctionMode, ...}` 复用共享聚合描述符；`logical_aggregation_descriptor_aster_unit_test.rs` 用 `TypeId` 验证该类型恒等关系及 Partial1/Complete/Dedup 判定。

已核对的关键一致点包括：直接分组列保留顺序和重复；全局聚合推导最多一行；分组 NDV 用共享多列估算器；裁剪时保留副作用和一个占位聚合；`MIN/MAX` 仅在无 DISTINCT、无 ORDER BY、单个上下文常量参数时可与参数结果等价；空函数列表不是模式判定的正常输入。

已确认的迁移差异/限制包括：

- Rust `ExplainInfo` 仅用分组表达式 hash 和函数名生成摘要，Go 使用完整的 explain 表达式/聚合函数格式化，因而文本精度不同。
- Go 谓词下推先对分组聚合中的常量 `MIN/MAX` 输出作列替换和常量折叠，且对纯常量谓词同时下推与保留；Rust 虽有 `getAggFuncsColsForConstResult`，但当前 `splitCondForAggregation` 未使用它，并且分组不变判定排除无列谓词。
- Go 谓词下推以 schema 替换和更细的 group-by/firstrow 回退流程实现；Rust 是基于 `UniqueID` 集合的收敛实现，虽支持 DNF 共同可下推部分，不应宣称已覆盖 Go 的全部细节。
- Go `BuildKeyInfo` 对空 `AggFuncs` 有显式保护；Rust 当前在调用 `IsPartialModeAgg` 前没有该保护。Go `DeriveStats` 在复用缓存统计时会刷新 `GroupNDVs`，Rust 早返路径直接复制缓存。
- Go `PossibleProperties` 同时携带 `HasTiFlash`；Rust 本类型字段仅保存排序列向量。Go 的 `NoCopPushDown` 已标记为由物理属性替代的废弃字段，Rust 仍直接在 `CanPushToCop` 中检查它。

## 扩展指南

- 新增聚合状态字段时，同步检查构建器、hint 复制（`CopyAggHints`）、hash/equals 生成实现、浅引用实现与物理化路径；不要只在结构体加字段。
- 修改谓词下推时，从 `splitCondForAggregation` 及 CNF/DNF 辅助函数接入，必须保持“聚合输出不能在子行上提前评估”的语义，并覆盖常量谓词、CNF/DNF 混合、`firstrow`、常量 `MIN/MAX`、`NOT(ISNULL(count))`、常量分组和无上下文退化。
- 修改列裁剪时，必须成对更新 `AggFuncs` 与 schema，把聚合参数、聚合 `ORDER BY` 及分组项所需列传给子节点，并验证空表下的全局/分组聚合行数差别、全 firstrow、副作用表达式与分组 firstrow 占位。
- 修改统计或键/FD 逻辑时，从 `DeriveStats`、`ExtractColGroups`、`BuildSelfKeyInfo`、`ExtractFD` 接入；检查缓存重用、多列 NDV、伪统计上限、表达式分组、直接列分组、Partial1 模式和 firstrow 不应产生假 FD 等风险。
- 修改模式或物理属性时，不要把 `Partial2`/`Final`/`Dedup` 当成 `Partial1` 或 `Complete`；同时评估 DISTINCT 参数、聚合 `ORDER BY`、分区键、coprocessor 禁止位和冲突 hint 处理。
- Rust 生产代码和测试必须继续分文件放置。主要同步测试是 `logical_aggregation_test.rs`、`logical_aggregation_descriptor_aster_unit_test.rs` 与 `logical_relational_aster_unit_test.rs`；如果对齐 Go 行为，还应对照 `logical_aggregation.go` 及上层 planner 的聚合回归测试，不得为通过测试而简化 Go 语义。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/core/operator/logicalop` 确认目标 Rust/Go/测试文件均在索引中。
- RustCodeGraph 源码读取：`node --file pkg/planner/core/operator/logicalop/logical_aggregation.rs --offset 1 --limit 500` 与 `--offset 500 --limit 500`，覆盖目标文件全部 864 行；索引报告该文件被 12 个文件引用。
- RustCodeGraph 符号/图查询：`query LogicalAggregation --kind struct --limit 10 --json`、`query PredicatePushDown --kind method --limit 20 --json`、`query PruneColumns --kind method --limit 20 --json`、`query DeriveStats --kind method --limit 20 --json`，并尝试了 `callers`/`callees` 查询。后者未返回可用方法边，所以上下游另由文件反向引用、构建器与优化器精确引用核验。
- 已读直接生产证据：`pkg/planner/core/operator/logicalop/Cargo.toml`、`lib.rs`、`base_logical_plan.rs`、`logical_selection.rs`，`pkg/planner/core/logical_plan_builder_runtime.rs`、`optimizer_runtime.rs`；目标包无 `doc.go`，已读最近的接口契约 `pkg/planner/core/base/doc.go`。
- 已读 Go 对照：`pkg/planner/core/operator/logicalop/logical_aggregation.go` 全部 843 行，重点核对结构字段、谓词下推、列裁剪、统计、FD、属性、模式判定与上拉条件。
- 已读独立 Rust 测试：`logical_aggregation_test.rs`、`logical_aggregation_descriptor_aster_unit_test.rs`、`logical_relational_aster_unit_test.rs`。它们分别验证直接分组列顺序/重复、常量分组与上拉、空值评估、聚合结果等价限制、描述符类型/模式、分组 NDV、全局聚合行数和分组不变谓词。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 与固定二级标题计数命令验证文档存在且恰有 11 个规定章节。
