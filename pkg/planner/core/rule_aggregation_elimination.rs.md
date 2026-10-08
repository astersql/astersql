# `pkg/planner/core/rule_aggregation_elimination.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate，由 [`lib.rs`](lib.rs) 以公开模块 `rule_aggregation_elimination` 装配；同目录独立测试 [`rule_aggregation_elimination_test.rs`](rule_aggregation_elimination_test.rs) 仅在 `cfg(test)` 下装配。它移植并精简表达 Go 文件 [`rule_aggregation_elimination.go`](rule_aggregation_elimination.go) 的聚合消除规则：当分组列覆盖子计划唯一键时，将“一组最多一行”的聚合改写为投影；同时消除由唯一键保证无重复的聚合函数 `DISTINCT`，以及 Semi/AntiSemi Join 内侧只做去重的聚合。

需要区分两个层次：本文件以 `crate::task::{Expression, FieldType, JoinType, PlanNode}` 构造独立的精简 `LogicalPlan`，并被 `rule_aggregation_push_down.rs`、`rule_aggregation_skew_rewrite.rs`、`rule_resolve_grouping_expand.rs` 复用；完整应用的逻辑优化入口 `optimizer_runtime.rs::LogicalOptimize` 则在 `LogicalRule::EliminateAgg` 分支调用 `eliminate_aggregation_descendants`，直接处理 `logicalop::LogicalPlanRef`。因此本文件是可执行、可测试的规则移植模型和同组规则公共 IR，但不是当前生产优化流水线唯一的实现位置。

`Cargo.toml` 将本目录声明为包 `astersql-planner-core`（`[lib] path = "lib.rs"`、`autotests = false`），未为本模块设置专属 feature；`nextgen` feature 与本文件逻辑无直接条件编译关系。本文件自身只直接使用 crate 内 `task` 模块和标准库 `HashSet`。

## 核心职责

1. 用 `AggFuncName`、`AggMode`、`AggFuncDesc` 和 `LogicalAggregation` 描述聚合函数及聚合算子所需的最小状态。
2. 用 `LogicalPlan` 表达聚合、投影、连接、`UnionAll`、`Expand` 和叶子节点，并提供 schema、唯一键和最多一行属性的查询方法。
3. 由 `AggregationEliminator::Optimize` 自底向上遍历计划树，先优化孩子，再处理 Semi/AntiSemi 内侧去重聚合，最后处理当前聚合的冗余 `DISTINCT` 与聚合转投影。
4. 由 `aggregationEliminateChecker` 集中执行安全性判定：分组键必须覆盖非空唯一键；`GROUP_CONCAT` 和明确禁止消除的聚合不得转投影；聚合下推后的旧聚合还可启用外连接可空侧检查。
5. 由 `rewriteExpr` 及其辅助函数把单行分组上的聚合语义降为普通标量表达式，包括 `COUNT` 的 NULL 语义、位聚合的空值恒等元以及必要的返回类型转换。

## 主要符号

- `pub type Result<T> = std::result::Result<T, String>`：本组精简规则共用的结果类型。当前文件的递归过程没有主动构造错误，但接口保留错误传播形态供同组规则复用。
- `AggFuncName`：支持 `Count`、`Sum`、`Avg`、`FirstRow`、`Max`、`Min`、`GroupConcat`、三种位聚合等名称；JSON 聚合、近似去重计数和 `Other` 被保留为不可改写类别。
- `AggMode`：记录 Complete、Partial、Final、Dedup 等阶段。本文件仅保存该字段，`rewriteExpr` 当前不按 mode 分支。
- `AggFuncDesc`：保存名称、参数、`distinct`、mode、返回类型和 `order_by`。`tryToEliminateDistinct`、Semi Join 判定和表达式改写都读取它。
- `LogicalAggregation`：保存聚合函数、分组项、单个孩子、输出 schema/列和 `no_eliminate` 标志；`no_eliminate` 为真时禁止聚合转投影。
- `LogicalPlan`：规则私有 IR。`schema()` 返回节点输出类型；`unique_keys()` 仅从 `Node` 读取、经 `Projection` 透传；`max_one_row()` 仅识别叶子标志，但当前消除路径没有使用该方法。
- `AggregationEliminator`：公开规则入口。`Optimize` 返回 `(新计划, changed)`，`Name` 返回稳定规则名 `aggregation_eliminate`。
- `aggregationEliminateChecker`：Go 同名 checker 的精简移植。`old_agg_elimination_check` 控制聚合下推旧聚合的额外外连接检查。
- `tryToEliminateAggregation`：验证禁用标志、函数白名单、分组列对唯一键的覆盖和可选外连接检查，再调用 `ConvertAggToProj`。
- `tryToEliminateDistinct`：仅当某个 DISTINCT 聚合的所有参数都是列，且这些参数覆盖孩子的某个唯一键时清除 `distinct`。
- `canEliminateSemiJoinInnerDistinct`：识别有分组项、孩子不含 Limit、所有聚合均为单参数且无 DISTINCT/ORDER BY 的 `FirstRow` 形态。
- `CheckCanConvertAggToProj`：当孩子是 Left/Right Outer Join 时，拒绝改写引用可空补齐侧列的聚合参数。
- `ConvertAggToProj`：逐个调用 `rewriteExpr`；任一函数不支持即整体失败，否则保留原孩子并构造表达式数目对应聚合函数数目的 `Projection`。
- `rewriteExpr`、`rewriteCount`、`rewriteBitFunc`、`wrapCastFunction`：完成各聚合函数的标量化；不支持的函数返回 `None`，类型相同时避免多余 cast。
- `has_limit`、`optimize_children`：分别递归探测 Limit 和递归重建孩子计划。两者都是模块内部函数。

本文件没有模块级常量、trait、宏、异步函数或条件编译项。

## 执行流程

`AggregationEliminator::Optimize(plan)` 的执行顺序如下：

1. 调用 `optimize_children`。Aggregation、Projection、Expand 递归一个孩子，Join 递归左右孩子，UnionAll 递归全部分支；每个孩子返回的 `changed` 以 OR 汇总。叶子直接返回。
2. 若当前节点是 Semi 或 AntiSemi `LogicalPlan::Join`，且右孩子是 `LogicalAggregation`，调用 `canEliminateSemiJoinInnerDistinct`。满足条件时以该聚合的孩子替换右孩子并立即返回 `changed = true`；这里不要求右孩子拥有唯一键，因为半连接只关心匹配是否存在。
3. 若当前节点不是 Aggregation，返回递归处理后的计划和累计 `changed`。
4. 对 Aggregation 调用 `tryToEliminateDistinct`：逐函数收集参数的 `Expression.column`。只要存在非列参数就保持原状；否则只要参数列集合覆盖某个孩子唯一键，就清除该函数的 `distinct`。
5. 调用 `tryToEliminateAggregation`。它先拒绝 `no_eliminate` 或包含 `GroupConcat` 的聚合，再收集分组项中的列下标，要求其覆盖某个非空孩子唯一键。若 `old_agg_elimination_check` 为真，还必须通过 `CheckCanConvertAggToProj`。
6. `ConvertAggToProj` 逐个改写聚合函数。`Count` 进入 `rewriteCount`；三种位聚合进入 `rewriteBitFunc`；Sum/Avg/FirstRow/Max/Min 经过 `wrapCastFunction`；近似计数、JSON 聚合及未知函数使整个转换失败。成功后投影沿用聚合孩子，并优先使用 `agg.schema`，其为空时使用调用方传入的孩子 schema。

`rewriteCount` 对无参数计数生成常量 `1`；有参数时生成语义字符串 `if(isnull(x) or ...,0,1)`。`rewriteBitFunc` 对 NULL 使用位运算恒等元（BitAnd 为 `u64::MAX` 的十进制文本，其余为 `0`）。`wrapCastFunction` 只在参数类型与聚合返回类型不同的时候生成 cast。

一个细节是：清除 `distinct` 本身没有更新 `changed`；聚合成功转为投影时返回的也是此前孩子累计的 `changed`。只有 Semi/AntiSemi 内侧聚合删除明确返回 `true`。这是当前 Rust 文件的实际标志语义，调用方不应把该布尔值当作检测所有字段级改写的完备信号。

## 数据与状态

规则对象 `AggregationEliminator` 是无字段零大小类型；checker 只有 `old_agg_elimination_check: bool`。优化过程没有全局可变状态，所有计划变换通过取得 `LogicalPlan` 所有权、克隆孩子或重建枚举变体完成。

列身份在该精简 IR 中是 `usize` 下标：`Expression.column: Option<usize>` 表示纯列引用，唯一键是 `Vec<Vec<usize>>`，分组覆盖和 DISTINCT 覆盖通过临时 `HashSet<usize>` 判断。schema 仅保留 `FieldType` 列表，没有列 ID 映射；因此 `Projection::unique_keys()` 直接透传孩子唯一键，未按投影表达式重映射。这与完整 `logicalop` schema 的 `UniqueID` 语义不同，是扩展时必须注意的模型边界。

`LogicalPlan::Node` 同时携带 `unique_keys` 与 `max_one_row`。后者有查询方法但当前规则未使用；聚合消除实际只依赖非空唯一键。`LogicalAggregation.output_columns` 和 `AggMode` 也在本文件中只存储、不参与判定，供相邻聚合规则保留移植状态。

转换保持的主要不变量是：投影表达式顺序与 `agg_funcs` 顺序一致；投影孩子仍是原聚合孩子；输出 schema 优先保持聚合 schema；任一聚合函数无法安全标量化时不做部分转换。

## 依赖与调用关系

RustCodeGraph 的精确文件查询给出本文件内部主调用边：

- `AggregationEliminator::Optimize` → `optimize_children`、`tryToEliminateDistinct`、`tryToEliminateAggregation`。
- `tryToEliminateAggregation` → `LogicalPlan::unique_keys`、`LogicalPlan::schema`、`CheckCanConvertAggToProj`、`ConvertAggToProj`。
- `ConvertAggToProj` → `rewriteExpr`。
- `rewriteExpr` → `rewriteCount`、`rewriteBitFunc`、`wrapCastFunction`。
- `optimize_children` → `AggregationEliminator::Optimize`，形成按计划树深度递归的后序遍历。

上游装配由 `lib.rs` 的 `pub mod rule_aggregation_elimination` 提供。直接 Rust 使用者包括：`rule_aggregation_push_down.rs` 复用完整聚合 IR 与 `Result`；`rule_aggregation_skew_rewrite.rs` 复用聚合 IR；`rule_resolve_grouping_expand.rs` 复用 `LogicalPlan` 和 `Result`；独立测试直接调用 checker、规则入口及 `rewriteExpr`。

完整应用主链不是静态调用本文件的 `AggregationEliminator::Optimize`，而是 `optimizer_runtime.rs::LogicalOptimize` 按 `LOGICAL_RULES`/`FLAG_ELIMINATE_AGG` 调用 `eliminate_aggregation_descendants`。该运行时实现同样先递归孩子，再移除 semi 风格连接内侧去重聚合、消除 DISTINCT、检查唯一键并建立真实 `logicalop::LogicalProjection`。聚合下推路径也会在下推前后调用这一运行时函数。`rule/rule_init.rs::default_rule_names` 和 `optimizer.rs` 的规则名称列表均包含 `aggregation_eliminate`，提供规则排序/跟踪名称证据，但 `optimizer.rs::normalize` 的精简入口目前只真实实现投影消除。

## 错误处理与边界

本文件以“不改写”而非错误表示大多数不安全或不支持情况：`tryToEliminateAggregation` 返回 `None`，`rewriteExpr` 返回 `None`，`ConvertAggToProj` 返回 `(false, None)`。明确边界包括：

- `no_eliminate` 为真或包含 `GroupConcat` 时禁止聚合转投影。Go 注释说明 GROUP_CONCAT 还受 `group_concat_max_len` 截断语义约束。
- 分组项必须以列表达式覆盖至少一个非空唯一键；常量或复杂表达式不会贡献列覆盖。
- DISTINCT 消除要求所有参数都是列；混有常量或复杂表达式即保持 DISTINCT。
- Semi/AntiSemi 内侧消除要求非空分组、无下层 Limit、所有聚合函数都是单参数 FirstRow，且无 DISTINCT/ORDER BY。
- 外连接额外检查开启时，只要聚合参数直接引用可能被 NULL 补齐一侧的列，就拒绝转投影。
- 近似去重计数、JSON 聚合及未知函数没有标量化实现，整次聚合转换保持原计划。

`Optimize` 的 `Result` 错误会由递归调用通过 `?` 原样传播，但当前文件没有生成 `Err` 的分支。索引越界也被结构性避免：列下标只用于集合和区间判断，不直接索引向量；聚合无参数时 `rewriteExpr` 使用常量 `1` 作为默认参数。不过调用者仍应构造 schema、列下标和表达式类型彼此一致的计划，否则精简模型可能生成语义字符串正确但元数据不一致的投影。

## 并发与资源生命周期

本文件没有线程、锁、原子、通道、异步任务、事务、文件句柄或网络资源。每次优化完全由输入计划和 checker 标志决定，可并行在互不共享的计划值上调用。

生命周期由 Rust 所有权管理：`Optimize` 消费 `LogicalPlan`；递归时从 `Box` 解包孩子并重建父节点；聚合转投影或 Semi Join 内侧消除时会克隆需要保留的孩子。临时 `HashSet`、表达式向量和重建节点在函数返回后按所有权释放。深度等于计划树深度，因此极端深树的资源风险主要是递归栈，而不是泄漏或后台任务未收敛。

## 与 Go 版本的对应关系

符号基本一一对应：Rust `AggregationEliminator`/`aggregationEliminateChecker`、`tryToEliminateAggregation`、`tryToEliminateDistinct`、`canEliminateSemiJoinInnerDistinct`、`CheckCanConvertAggToProj`、`ConvertAggToProj`、`rewriteExpr`、`rewriteCount`、`rewriteBitFunc`、`wrapCastFunction` 和 `Optimize` 均可在同路径 Go 文件找到来源。规则名同为 `aggregation_eliminate`，总体顺序同为“递归孩子 → Semi 风格内侧去重 → DISTINCT 消除 → 聚合转投影”。

当前精简 Rust 文件与 Go 仍有可观察差异，不能把两者视为完全等价：

- Go 使用真实 `logicalop`、`expression`、session context 和 Schema；本文件使用 `task` 中的简化表达式、类型与计划枚举。生产运行时的更完整 Rust 对照位于 `optimizer_runtime.rs::eliminate_aggregation_descendants`。
- Go DISTINCT 检查同时遍历 `PKOrUK` 与 `NullableUK`；本文件的 `LogicalPlan::unique_keys()` 只有一个键集合，没有单独的 nullable unique key 表达。
- Go `CheckCanConvertAggToProj` 会从聚合参数表达式树提取列；本文件只检查参数顶层的 `Expression.column`。
- Go `rewriteExpr` 对 FinalMode 且非空的单参数 COUNT 可直接 cast，对 MAX/MIN 的 binary literal 会拒绝消除；本文件没有这两处分支。
- Go 位聚合执行双层整数转换并构造真实 IFNULL 表达式；本文件用名称字符串和目标类型近似表达。
- Go Semi Join 路径针对 `LogicalApply` 且覆盖更多 semi 风格 join 类型；本文件用 `LogicalPlan::Join`，只枚举 `Semi` 与 `AntiSemi`。完整 Rust 运行时同时识别 `LogicalApply`/`LogicalJoin` 和四种 semi 风格类型，还能穿过无副作用 Projection/MaxOneRow 找到内侧聚合。
- 本文件有 `no_eliminate` 字段检查，而同路径 Go checker 中没有直接读取同名字段。

相关 Rust 单元测试覆盖 GROUP_CONCAT 不消除、含非列参数的 DISTINCT 不消除、Semi Join 内侧去重不依赖唯一键、MAX 返回类型不同时 cast，以及优化器实际移除 Semi Join 右侧聚合。Go 集成测试 `casetest/rule/rule_correlate_test.go::TestCorrelatedInApplyEliminatesDistinct` 通过开关 `aggregation_eliminate` 比较 Explain 计划并校验查询结果。完整 Rust 运行时另由 `optimizer_logical_entry_aster_unit_test.rs` 验证唯一键分组转投影与可空 COUNT 语义；这些测试验证生产接线，但不是本文件精简 IR 的直接调用者。

## 扩展指南

- 新增可消除聚合函数时，先在 `AggFuncName` 补充或启用类别，再在 `rewriteExpr` 添加单行语义；若需要专门的 NULL/类型规则，应新增独立辅助函数。必须同步 `rule_aggregation_elimination_test.rs`，覆盖同类型、异类型、NULL 和不支持分支，并核对 Go `rewriteExpr` 与生产 `optimizer_runtime.rs::rewrite_single_row_aggregate`。
- 修改唯一键判定时，优先审查 `LogicalPlan::unique_keys`、`tryToEliminateAggregation` 和 `tryToEliminateDistinct`。投影若能重排列或计算表达式，就不能继续无条件透传列下标键；应建立显式映射，并增加复合键、可空唯一键及投影重排测试。
- 扩展 Semi Join 内侧消除时，修改 `canEliminateSemiJoinInnerDistinct`、`has_limit` 和 `Optimize` 的 Join 分支；需验证 Limit、ORDER BY、DISTINCT、多参数 FirstRow、非 FirstRow 以及 Projection/MaxOneRow 包装。生产行为还必须同步检查 `optimizer_runtime.rs::{semi_inner_distinct_aggregation,remove_semi_inner_distinct_aggregation}`。
- 修改外连接安全检查时，应覆盖 LeftOuter/RightOuter 的保留侧与可空侧、嵌套表达式列引用和聚合下推旧聚合路径；`old_agg_elimination_check` 的调用方也要同步审查。
- 若把本文件接入真实 `logicalop` 主链，不应直接扩大简化 IR；应先决定是复用生产 `eliminate_aggregation_descendants` 还是统一两套实现，并保持 Go 的 expression context、警告、类型转换、NullableUK、输出名和 query-block 元数据。
- 性能风险主要来自每个聚合重复克隆唯一键/孩子、递归重建整棵树以及集合构造；正确性风险集中在 NULL 语义、类型 cast、可空唯一键、列身份映射和 changed 标志。任何优化都不应以跳过这些语义检查为代价。

测试必须继续放在独立的 `pkg/planner/core/rule_aggregation_elimination_test.rs`，不要内嵌到生产源文件。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；随后以目标文件限定查询 `AggregationEliminator`、`tryToEliminateAggregation`、`ConvertAggToProj`、`rewriteExpr`、`Optimize` 及其 callers/callees。
- RustCodeGraph 完整读取 `pkg/planner/core/rule_aggregation_elimination.rs`（1–452 行），并确认上述内部调用边；callers 查询对部分递归/同名符号未返回稳定结果，因此上游使用者另由精确 `rg` 交叉核验。
- crate 与装配证据：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`、`pkg/planner/core/rule/rule_init.rs`。
- 直接相邻实现：`pkg/planner/core/rule_aggregation_push_down.rs`、`pkg/planner/core/rule_aggregation_skew_rewrite.rs`、`pkg/planner/core/rule_resolve_grouping_expand.rs`。
- 完整应用主链：`pkg/planner/core/optimizer_runtime.rs::LogicalOptimize`、`eliminate_aggregation_descendants`、`semi_inner_distinct_aggregation`、`rewrite_single_row_aggregate`；辅助规则名入口为 `pkg/planner/core/optimizer.rs`。
- Go 对照：`pkg/planner/core/rule_aggregation_elimination.go`（1–289 行）。
- 测试证据：`pkg/planner/core/rule_aggregation_elimination_test.rs`（1–145 行）、`pkg/planner/core/casetest/rule/rule_correlate_test.go::TestCorrelatedInApplyEliminatesDistinct`、`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs` 中两个聚合消除测试。

任务是纯文档分析，按计划未运行 Cargo。最终结构检查要求目标文件存在，并且恰好出现“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”十一个二级标题。
