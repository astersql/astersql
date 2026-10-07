# `pkg/planner/core/operator/logicalop/logical_join.rs`

## 文件定位

本文件实现逻辑优化阶段的二元连接节点 `LogicalJoin`。它位于 `astersql-planner-core-operator-logicalop` crate，由同目录 `lib.rs` 声明为 `mod logical_join` 并整体 `pub use`，因此 planner builder、优化规则和物理计划枚举代码都通过 logicalop crate 使用它。其职责处在“逻辑计划已建成、物理连接尚未选择”的中间层：维护连接语义和条件，接受谓词下推、列裁剪、键/统计/函数依赖推导，并保存后续物理化需要的 hint 与属性。

RustCodeGraph 将该文件标为 1,577 行、112 个符号，并显示它被 `pkg/planner/core/logical_plan_builder_runtime.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`pkg/planner/core/lateral_join_test.rs` 等 9 个文件直接使用。文件末尾的 `impl LogicalPlan for LogicalJoin` 把本地实现接到统一 `LogicalPlan` trait；`PredicatePushDown`、`PruneColumns`、`BuildKeyInfo` 和 `DeriveStats` 均从这里进入递归优化主链。

## 核心职责

1. 用 `JoinType` 表示 inner、left/right/full outer、semi、anti-semi 和带布尔标记的 outer-semi 等连接语义，并让不同语义决定输出 Schema、谓词可下推方向、基数下界和键继承规则。
2. 将表达式划分为 `EqualConditions`、`NAEQConditions`、`LeftConditions`、`RightConditions` 与 `OtherConditions`。`ExtractOnCondition`/`extract_on_condition` 识别跨左右子树的 `eq`/`nulleq` 列对，`AttachOnConds` 和 `updateEQCond` 维护分类结果。
3. 在 `PredicatePushDown` 中进行外连接简化、DNF 公因子/宽松谓词提取、两侧分流、`IS NOT NULL` 派生、子树递归下推和等值条件/键刷新；`PredicatePushDownRoot` 还处理 NOT 下推、谓词简化与恒假 `LogicalTableDual` 替换。
4. 在 `ExtractUsedCols`、`PruneColumns`、`MergeSchema`、`BuildKeyInfo` 中维持输出列、完整 Schema、半连接输出形状和 `PKOrUK` 信息。
5. 在 `DeriveStats` 中递归取得两侧统计，按连接键 NDV 估算 inner 部分，再应用外连接下界或 semi 选择率，并传播列 NDV 与 group NDV。
6. 提取关联列、列组、潜在分区键和函数依赖；保存连接算法/顺序偏好、去关联来源、半连接改写来源与冗余列映射等优化状态。

## 主要符号

- `LogicalJoin`：核心公开结构。`LogicalSchemaProducer` 提供基类、子节点、Schema、输出名和统计；`JoinType` 控制语义；五组条件字段保存谓词；`FullSchema` 保留裁剪前视图；`RedundantColsToOutputIdx` 支持 inner join 冗余列回映射；`EqualCondOutCnt` 保存等值连接估算；`FromDecorrelatedApply`、`FromSemiJoinRewrite`、`FromInSubqueryRewrite` 记录改写来源。
- `JoinCardinalityContext<'a>`：把 object-safe 的 `base::PlanContext` 适配为 `cardinality::CardinalityContext`，只转发 session、expression 和 ranger context，供 `EstimateFullJoinRowCount` 使用。
- `effective_join_column_ndv`：把零或缺失 NDV 视为 pseudo statistics 的“未知”，退化为 `min(RowCount * 0.8, 8000)`，避免等值连接被错误估成笛卡尔积。
- `ExtractOnCondition`、`extract_on_condition`、`ConditionSide`：按左右 Schema 对条件定向；IN 子查询来源的等式留在 `OtherConditions`，普通跨侧 `eq`/`nulleq` 才进入等值条件。
- `PredicatePushDown`、`PredicatePushDownRoot`、`derive_inner_join_not_null_conditions`、`derive_relaxed_dnf`：谓词优化核心。它们分别处理节点级分流、根级简化、由列比较派生非空条件及跨侧 DNF 的单侧安全放宽。
- `ExtractUsedCols`、`PruneColumns`、`MergeSchema`、`BuildKeyInfo`：列与 Schema 生命周期。半/反半连接只输出左侧，outer-semi 额外保留 marker，full outer join 清除两侧 NOT NULL 标记。
- `DeriveStats`、`getGroupNDVs`、`GetJoinKeys`、`GetNAJoinKeys`：统计与连接键入口。普通等值和 null-aware 等值键分开保存、提取和传给基数估计器。
- `ExtractFD` 及 `ExtractFDFor*`：合并子节点函数依赖；inner join 增加连接键等价类，最后投影到实际输出列。
- `SetPreferredJoinType*`、`PreferAny`、`containDifferentJoinTypes`：保存 Hash/Merge/Index 及禁止 Index 变体的 bit flag，并检测冲突偏好。
- `RegisterRedundantColumnMapping`、`ResolveRedundantColumn`：只允许 inner join 在输出下标有效、输出名存在且类型兼容时将冗余列解析到可见列。
- `impl LogicalPlan for LogicalJoin`：将同名 trait 方法委托到上述固有方法，是统一优化器遍历该节点的真实入口。

## 执行流程

典型路径从 builder 构造 `LogicalJoin::default().Init(ctx, offset)` 开始，设置两个 child、连接类型和 ON 条件，再通过 `MergeSchema` 建立输出。优化阶段的主要流程如下：

1. `PredicatePushDownRoot` 先用表达式上下文执行 `PushDownNot` 和通用谓词简化。若 `Conds2TableDual` 判断条件恒假，则复制当前 Schema/输出名并返回零行 `LogicalTableDual`。
2. `PredicatePushDown` 读取左右 Schema。outer-semi 的正 marker 过滤会把节点改成 `SemiJoin` 并重建 Schema；随后以 `IsNullRejected` 检测两侧，调用 `ConvertOuterToInnerJoin` 将可安全简化的 outer join 降级。
3. inner/semi join 把已有 ON 条件与新 WHERE 谓词合并后做 DNF 提取及 join 专用简化；outer join 只简化传入谓词，避免改变空值补齐语义。
4. 每个谓词由 `side_of` 分成左、右、双侧或常量。单侧谓词仅在连接类型允许时下推；双侧谓词留在 join，同时 `derive_relaxed_dnf` 可产生左右单侧的必要条件；不能安全下推的谓词进入返回给父节点的 `retained`。
5. `derive_inner_join_not_null_conditions` 扫描等值/比较列对，为允许的一侧构造 `NOT(IS NULL(col))`。IN 来源等式、非最外层 CTE、semi rewrite 后的计算键会被明确排除。
6. 去重后通过 `PredicatePushDownPlan` 递归下推到两个 child；child 返回的残留条件由 `AttachSelectionToPlan` 就地包成 Selection。最后 `updateEQCond` 重新识别等值谓词，并依次重建 child 与 join 的键元数据。
7. 列裁剪时 `ExtractUsedCols` 汇总父引用和全部连接条件引用并按子树拆分；`PruneColumns` 递归裁剪、重建可见 Schema，同时保留既有 `FullSchema`，再让 `InlineProjection` 收紧输出。
8. 统计阶段 `DeriveStats` 先递归获取两侧统计并修正 pseudo NDV。无等值条件按笛卡尔积估算，否则调用 `EstimateFullJoinRowCount`；outer join 取保留侧行数为下界，semi/anti 取左侧 `0.8`。之后为等值键传播双方有效 NDV 的较小值（outer join 按保留侧处理），并保存 stats。

## 数据与状态

`LogicalJoin` 是一个可变优化节点，最重要的不变量是“恰有两个有序 child”：索引 0 为左侧、1 为右侧。多个方法在 child 数量不对时返回空结果或保留谓词，`DeriveStats` 则直接报错。跨侧等值表达式在存储时不保证原始参数顺序，但 `extract_keys_from` 总会按 child Schema 规范为 `(left_keys, right_keys)`。

五组条件不是互斥不变的永久分类：谓词下推和改写可能把表达式移入 `OtherConditions`，随后 `updateEQCond` 再升级为等值或单侧条件。`NAEQConditions` 独立于普通等值条件，`IsNAJoin`/`IsNAAJ` 据其是否为空判断 null-aware join。`FromInSubqueryRewrite` 会关闭非空推导，以保留空子查询/NULL 的 Go 语义。

Schema 状态分两层：`Schema()` 是当前可见输出，`FullSchema` 可保存裁剪前的完整列集合，供上层 join 继续解析已被下层隐藏的键。`MergeSchema` 对普通/outer join 合并两侧，对 semi/anti 只留左侧，对 outer-semi 复用现有末尾 marker。`PKOrUK` 的继承要求存在等值连接键且另一侧连接键覆盖唯一键；笛卡尔积不得继承任一侧唯一性。

`StatsInfo` 中 `EqualCondOutCnt` 记录 inner 部分估值，最终 `RowCount` 会按连接类型修正。semi/anti 的输出列 NDV 与行数同时乘 `SelectionFactor`；其他连接的列 NDV 不超过输出行数。`PreferJoinType` 及左右侧偏好是 bit mask，并不在本文件中选择物理算子。

## 依赖与调用关系

上游入口主要有三类：planner builder 创建并填充 join；统一逻辑优化器通过 `LogicalPlan` trait 调用谓词下推、裁剪、键和统计方法；join reorder、decorrelate、outer-join elimination 与 physical-plan exhaust 读取连接类型、条件和 hint。RustCodeGraph 的直接使用文件还包括 planner 运行时和多处 planner 测试。

下游依赖可从 `Cargo.toml` 与调用图核对：

- `base`/`LogicalSchemaProducer`：提供 context、children、Schema、stats、FD 和递归逻辑计划能力。
- `expression`、`parser_ast`、`mysql`、`types`：表达式分类/重写、DNF 提取、函数构造、列类型与 NULL 标记。
- `rule_util`、`planner_util`：join 谓词简化、NULL-rejected 判断和 Schema nullability 修正。
- `cardinality`、`property`、`cost`：完整 join 行数、`StatsInfo`/`GroupNDV` 及 semi 选择率。
- `fd`：函数依赖集合、等价类和输出列投影。

关键调用边包括：`PredicatePushDown -> ConvertOuterToInnerJoin / ExtractFiltersFromDNFs / ApplyPredicateSimplificationForJoin / PredicatePushDownPlan / AttachSelectionToPlan / BuildKeyInfo`；`DeriveStats -> child.DeriveStats / EstimateFullJoinRowCount / effective_join_column_ndv / getGroupNDVs / SetStats`；trait 实现再把统一入口委托回本文件固有方法。

## 错误处理与边界

本 crate 用 `PlannerError(String)` 统一错误。`derive_inner_join_not_null_conditions` 将 `expression::NewFunction` 的错误转成 `PlannerError`；`PredicatePushDownRoot` 在需要创建 `TableDual` 却缺少 context 时失败；`DeriveStats` 对非二元 child 或缺少 context 明确报错。子树谓词下推、Selection 附着、列裁剪和统计错误均使用 `?` 原样向上传播。

多数查询型辅助方法采用保守边界：child 不足时返回空键、空列或不分类；无法证明表达式属于单侧时按双侧处理；无法证明唯一性就不继承键；冗余列只在 inner join、合法下标和兼容类型同时满足时解析。外连接的 NULL 补齐侧不能接收会改变结果的 WHERE 谓词，full outer join 在 Schema 重建后必须再次清除 NOT NULL flag。

需要特别防止的语义陷阱是：IN 来源等式不能当普通等值键处理；mutable-effect 表达式在同路径 Go 实现中有专门保护，而 Rust 的 `extract_on_condition` 应在扩展时持续对照这一行为；连接键 CAST、重复 UniqueID、CTE 非最外层、semi rewrite 的计算键和 pseudo NDV 为零都已有针对性守卫或注释，修改时不可用一般化重写覆盖这些例外。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、网络连接或事务；其生命周期等同于一棵逻辑计划树在优化阶段的所有权生命周期。child 以 `LogicalPlanRef` 装在基类中，递归优化通过 `Children()`/`Children_mut()` 借用，表达式与 Schema 在需要跨修改边界时显式 clone。

可变操作具有顺序依赖：谓词分类后必须刷新等值条件和 key info；child 裁剪后必须重建 join Schema；统计必须先取得 child stats，释放可变 child 借用后才能读取/修改 join 自身。`ExtractFDForInnerJoin` 临时改写 `JoinType`，调用结束后立即恢复；当前代码无跨线程共享同步保证，调用方不应并发修改同一计划节点。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/logicalop/logical_join.go`，Cargo 的 `[package.metadata.porting]` 也将 go-package 指向同一目录。字段、核心方法名和总体算法沿用 Go：条件分组、outer-to-inner、谓词下推、列裁剪、键推导、join 基数、关联列/FD、hint 与冗余列映射均可找到对应项。Rust 中关于 IN/EXISTS、pseudo NDV、重复 UniqueID、full outer nullability 的注释明确说明了为保持 Go 语义而设置的分支。

两边并非逐方法等量移植。Go 的 `ExtractOnCondition` 接收 `deriveLeft/deriveRight` 并同时处理 mutable-effect、常量推送和更完整的派生；Rust 把部分非空派生拆到 `derive_inner_join_not_null_conditions`，接口也更窄。Go 的 `SetPreferredJoinTypeAndOrder` 从完整 `PlanHints` 和表别名计算左右偏好，Rust 当前只接受 bit mask 与布尔 order。Rust 的 `ConstantPropagation`、`PushDownTopN`、`ExtractFDForOuterJoin`、`ExtractFDForSemiJoin`、`SemiJoinRewrite` 等入口多为基类委托或保守实现，不能据此宣称已覆盖 Go 中对应方法的全部规则。Go 侧仍有更丰富的 TopN 下推、hint 类型、常量传播及 semi-join rewrite 结构变换。

测试对应关系分层存在：`logical_join_test.rs` 独立验证冗余列映射守卫；`logical_relational_aster_unit_test.rs` 验证外连接空值供给侧不下推、inner/outer 基数边界、semi NDV 缩放和 pseudo NDV 回退；`logical_plans_test.rs` 通过 fixture 验证 join predicate、NOT NULL 派生、outer WHERE 下推和 outer join 简化。Go 的同目录及 planner 测试仍是扩展复杂规则时的语义基准。

## 扩展指南

- 新增连接条件分类或下推规则时，优先修改 `extract_on_condition`、`PredicatePushDown` 和 `derive_inner_join_not_null_conditions`，逐一检查 inner、左右/全 outer、semi、anti 与 outer-semi；尤其验证 NULL、空子查询、mutable expression 和 DNF。
- 新增连接类型时，必须穷举检查 `MergeSchema`、`BuildKeyInfo`、`DeriveStats`、`ExtractFD`、`ConvertOuterToInnerJoin`、谓词分流和常量推送，不能只补 enum match 使其编译。
- 改动统计时同步检查行数下界、等值键 NDV、非键列 NDV、group NDV 和 semi 选择率；pseudo NDV 的零值是“未知”而非真实零。
- 扩展 hint 时不要只扩大 bit mask；还需核对 Go `SetPreferredJoinTypeAndOrder` 的左右表别名、正/负 hint 与冲突检测，并追踪物理计划枚举如何消费这些字段。
- 补齐当前保守入口时，应以同路径 Go 的对应方法为基准，避免把 `ConstantPropagation`、TopN、FD 或 semi rewrite 做成仅能通过局部测试的简化版本。
- Rust 单测必须保持在独立文件。局部数据结构守卫放在 `logical_join_test.rs`；跨算子细节可扩展 `logical_relational_aster_unit_test.rs`；用户可见计划形状与 Go fixture 对齐应扩展 `logical_plans_test.rs` 或相应 planner 独立测试，不能把测试内嵌进 `logical_join.rs`。
- 性能风险集中在表达式反复 clone、DNF 展开、重复 Schema 扫描、统计 HashMap 构造和 FD 合并；正确性风险集中在 NULL/outer/semi 语义与条件移动顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`files --filter pkg/planner/core/operator/logicalop` 确认目标、Go 对照及独立测试均已索引。
- RustCodeGraph：`query LogicalJoin` 定位 Rust `LogicalJoin`（第 69 行）和 Go 对照（第 46 行）；按文件 `node` 分段读取了目标 1–1,577 行并取得 112 个符号、9 个直接使用文件；`callers/callees PredicatePushDown`、`callers/callees DeriveStats` 核对统一 trait 入口和 expression/base/cardinality 下游调用。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml` 的 package、lib、dependencies 与 porting metadata；模块装配：同目录 `lib.rs` 的 `mod logical_join`、`pub use logical_join::*` 和独立 `#[cfg(test)] mod logical_join_test`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_join.go` 的结构字段和 `PredicatePushDown`、`PruneColumns`、`BuildKeyInfo`、`DeriveStats`、`ExtractFD*`、`ExtractOnCondition`、hint/TopN/semi rewrite 等方法。
- Rust 测试：`pkg/planner/core/operator/logicalop/logical_join_test.rs`、`pkg/planner/core/operator/logicalop/logical_relational_aster_unit_test.rs`、`pkg/planner/core/logical_plans_test.rs`；另由搜索确认 builder、optimizer entry、lateral join 和 main planner 测试也直接检查 `LogicalJoin`。
- 本任务是纯文档分析，按任务要求未运行 Cargo；结构验证用于确认本文存在且恰含规定的 11 个二级章节。
