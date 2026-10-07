# `pkg/planner/core/operator/logicalop/logical_expand.rs`

## 文件定位

本文件定义 Rust 规划器中的逻辑 `Expand` 算子，目标是为 `ROLLUP`、`CUBE` 和 `GROUPING SETS` 把一行输入展开为多个分组层级。它属于 `astersql-planner-core-operator-logicalop` crate；`Cargo.toml` 以 `lib.rs` 为 crate 入口，`lib.rs` 通过 `mod logical_expand` 装入本文件并以 `pub use logical_expand::*` 对外导出。

在当前 Rust 主链中，`optimizer_runtime.rs::resolve_expand_descendants` 后序遍历逻辑计划树并对 `LogicalExpand` 调用 `GenLevelProjections`；`expression_rewriter.rs` 在改写 `GROUPING()` 时读取该算子的分组列、GID 和分组标记；`logical_apply.rs::extract_correlated_columns` 会把它纳入关联列提取。仓库内生产 Rust 代码尚未检索到 `LogicalExpand { ... }` 或 `LogicalExpand::default().Init(...)` 的实际构造点，`planbuilder_runtime.rs` 目前只保存 `outerBlockExpand`/`currentBlockExpand` 字段，因此“由 Rust plan builder 从 SQL 创建 Expand”在现有直接证据中仍未接线或未验证。Go 对照的物理化入口是 `physicalop.ExhaustPhysicalPlans4LogicalExpand`，Rust 中未找到同名物理化入口。

## 核心职责

- `LogicalExpand` 保存展开所需的原始分组表达式、投影后的唯一分组列、分组集合、每层 GID、列到 GID 集合的映射，以及最终的逐层投影 `LevelExprs`。
- `GenLevelProjections` 为每个 `GroupingSet` 生成一个与当前输出 schema 对齐的表达式向量：当前层未激活的分组列变为带类型的 `NULL`，普通列透传，`GID`/`GPos` 列变为无符号整数常量。
- `PredicatePushDown` 将 Expand 视为谓词下推屏障。原因是同一分组列在不同展开层的可空性不同；把上层谓词穿过 Expand 可能删除其他层仍需的行。
- `PruneColumns` 保留父节点使用列、`DistinctGroupByCol`、`GID` 和 `GPos`，向唯一子节点传递所需列后重新生成层级投影。
- `BuildKeyInfo` 清除展开后不再可靠的主键/唯一键信息，并仅在分组层数不超过一且子节点 `MaxOneRow` 时保留最多一行属性。
- `ExtractFD` 将基类取得的函数依赖投影到 Expand 当前输出列；`ExtractCorrelatedCols` 从已生成的所有层级表达式收集外层引用。

## 主要符号

- `GroupingMode::{ModeBitAnd, ModeNumericSet}`：GID 编码模式。前者按 `DistinctGroupByCol` 的逆序构造位图，后者给不同的分组列集合分配递增编号；默认是位图模式。
- `GroupingSet { ColumnIDs: BTreeSet<i64> }`：单个层级中处于激活状态的列 `UniqueID` 集合。`all_col_ids` 返回克隆，避免调用方直接修改内部集合。
- `GroupingSets(Vec<GroupingSet>)`：有序层级列表。顺序决定 `LevelExprs` 与 `RollupGroupingIDs` 的位置，也决定数值模式下首次出现的编号。
- `LogicalExpand`：算子主体。`LogicalSchemaProducer` 承载基类、schema、输出名和子节点；`DistinctGroupByCol` 与 `DistinctGbyExprs` 按位置配对；`RollupID2GIDS` 用于数值模式的 `GROUPING()` 标记；`GID`/`GPos` 及名称字段描述生成列。
- `Init`：用名称 `"Expand"`、规划上下文和查询块偏移初始化 `BaseLogicalPlan`。
- `GenerateGroupingIDModeBitAnd`、`GenerateGroupingIDIncrementModeNumericSet`：分别生成位图 GID 和层级序号 GID。
- `GenerateGroupingMarks`：去重分组集合、刷新 `DistinctSize`/`RollupGroupingIDs`/`RollupID2GIDS`。名称沿用 Go，但 Rust 版本不接收源列，也不直接返回 `GROUPING()` marks。
- `GenLevelProjections`：层级投影生成的总入口；由 `optimizer_runtime.rs::resolve_expand_descendants` 在逻辑优化流程中调用，也会在列裁剪结束时调用。
- `ResolveGroupingFuncArgsInGroupBy`、`TrySubstituteExprWithGroupingSetCol`：按表达式 `CanonicalHashCode` 把原始 GROUP BY 表达式替换为对应的投影列。
- `impl LogicalPlan for LogicalExpand`：提供动态向下转型，并把谓词下推、列裁剪和键信息构建委托给本文件的具体实现；其余基类能力来自 `LogicalSchemaProducer`/`BaseLogicalPlan`。

## 执行流程

1. 构造阶段应先准备 `DistinctGroupByCol`、同位置的 `DistinctGbyExprs`、有序 `RollupGroupingSets`、输出 schema，以及可选的 `GID`/`GPos`。`Init` 只初始化基类，不验证这些字段间的一致性。
2. 逻辑优化期间，通用框架通过 `LogicalPlan::PredicatePushDown` 调到本算子。它用空谓词递归处理第一个子节点，再把子节点返回的残留谓词与当前全部谓词一起返回，因而没有谓词穿过 Expand。
3. `PruneColumns` 计算必须保留的 `UniqueID` 集合，裁剪自己的 schema；若有第一个子节点，则从子 schema 中挑选同 ID 列并递归裁剪。之后立即调用 `GenLevelProjections`，使投影与新 schema 同步。
4. `optimizer_runtime.rs::resolve_expand_descendants` 还会在遍历完子树后调用 `GenLevelProjections`。该函数先调用 `GenerateGroupingMarks`：保留分组集合的原顺序，但按集合内容计算不同集合数；重复集合在数值模式下共享首次出现的 GID。
5. 对每个层级，`GenLevelProjections` 逐列判断角色：匹配 `GID` 列则写当前 GID 常量，匹配 `GPos` 列则写层级下标，属于分组列但本层未激活则写 `NULL`，否则克隆列引用。每个结果向量追加到 `LevelExprs`。
6. 改写 `GROUPING(args...)` 时，`expression_rewriter.rs` 先调用 `ResolveGroupingFuncArgsInGroupBy`；随后验证每项确实能向下转型为 `Column`，取得 GID 列，并按模式直接计算位标记或读取 `RollupID2GIDS`，再构造 GROUPING 表达式元数据。
7. 键信息推导时，`BuildKeyInfo` 先让基类递归构建，再清空当前 schema 的 `PKOrUK` 和 `NullableUK`。多层展开一定把 `MaxOneRow` 设为 false；零层或一层时还要求首个子节点本身最多一行。

## 数据与状态

`DistinctGroupByCol[i]` 与 `DistinctGbyExprs[i]` 是重要的位置不变量，`TrySubstituteExprWithGroupingSetCol` 找到表达式哈希后用相同下标取列。当前实现用安全的 `get`，长度不一致时静默保留原表达式；构造者仍应保持等长，否则 GROUPING 参数可能无法替换。

`RollupGroupingSets.0` 的顺序同时定义输出行层级、`GPos`、`RollupGroupingIDs` 和 `LevelExprs` 的顺序。`GenerateGroupingMarks` 每次都会清空并重建两个缓存，但 `GenLevelProjections` 也会先清空 `LevelExprs`，所以重复调用不会累积旧层级。

位图模式遍历 `DistinctGroupByCol` 的逆序，每遇到激活列写入 1；例如列序 `[a,b]` 且仅 `a` 激活时得到二进制 `01`。数值模式按“不同集合首次出现”的顺序编号，重复集合共享编号；独立测试 `numeric_grouping_ids_deduplicate_sets_and_marks_track_active_levels` 验证 `[a],[a],[b]` 得到 `[0,0,1]`。

`RollupID2GIDS` 只为某列处于激活状态的层级记录 GID。`LevelExprs` 的每层长度等于生成当时的 schema 列数。生成常量时 Rust 使用通用 `NewUInt64Const`；分组列 `NULL` 优先沿用列的 `RetType`，缺少类型时退化为无类型 `NewNull`。

## 依赖与调用关系

上游直接证据包括：`optimizer_runtime.rs::resolve_expand_descendants -> LogicalExpand::GenLevelProjections`；`expression_rewriter.rs -> ResolveGroupingFuncArgsInGroupBy` 并读取 `GroupingMode`、`DistinctGroupByCol`、`RollupID2GIDS`、`GID`；`logical_apply.rs::extract_correlated_columns -> LogicalExpand::ExtractCorrelatedCols`。`hash64_equals_generated.rs` 还为本类型实现 `Hash64`/`Equals`，覆盖基类、分组列/表达式、集合、层级表达式及 GID/GPos 标识。

本文件通过 `use crate::*` 依赖 logicalop crate 的统一再导出：表达式 `Column`/`Expression`、元数据 `FieldName`/`NameSlice`、函数依赖 `fd`、规划基类 `base`、`LogicalSchemaProducer` 与 `LogicalPlan`。`Cargo.toml` 声明本地路径依赖 `expression`、`base`、`fd`、`plancodec`、`types` 等，并用 `[package.metadata.porting] go-package = "pkg/planner/core/operator/logicalop"` 标识 Go 对照包。

下游关键调用为：对子节点的 `PredicatePushDownPlan` 和 `PruneColumns`；表达式层的 `ExtractCorColumns`、`NewUInt64Const`、`NewNullWithFieldType`、`NewNull` 与 `CanonicalHashCode`；FD 层的 `ProjectCols`；基类的 `BuildKeyInfo`、`ExtractFD`、`SetFDs` 与 `SetMaxOneRow`。

## 错误处理与边界

`PredicatePushDown` 和 `PruneColumns` 返回 crate 级 `Result<T, PlannerError>`，对子节点调用使用 `?` 原样传播错误。本文件没有主动构造业务错误；缺少子节点时谓词下推会保留所有输入谓词，列裁剪只处理自身 schema。

多个前置条件没有显式报错：`Init` 不检查字段；`GenLevelProjections` 允许空分组集合、空 schema 和缺失 GID/GPos；`ResolveGroupingFuncArgsInGroupBy` 对找不到的表达式直接原样返回。真正的 GROUPING 参数合法性由 `expression_rewriter.rs` 在替换后检查是否为 `Column`，并产生 `ErrFieldInGroupingNotGroupBy`；超过 64 个参数也由重写器产生 `ErrInvalidNumberOfArgs`。缺少 GID 时重写器返回 `GROUPING requires an Expand GID column`。

位图编码存放在 `u64` 中，本文件本身不限制分组列数；当前仅在 `GROUPING()` 参数数上看到 64 的检查。若 `DistinctGroupByCol` 超过 64，反复左移会丢弃高位，构造阶段必须选择/切换 `ModeNumericSet`，但 Rust 生产构造路径目前未找到，不能断言该切换已经自动完成。

与 Go 相比，Rust 的 `ResolveGroupingFuncArgsInGroupBy` 不返回 `Result`，且不会在此方法内验证“参数必须属于 GROUP BY”；错误被推迟到重写器的列类型检查。`TrySubstituteExprWithGroupingSetCol` 只比较 canonical hash，没有二次结构相等检查，依赖哈希语义足以代表表达式等价这一既有约定。

## 并发与资源生命周期

该算子没有锁、原子变量、异步任务、通道、事务或外部资源句柄。所有状态都归 `LogicalExpand` 所有，修改方法要求 `&mut self`；计划优化预期以单线程、独占的计划树变换方式调用它。

子节点由 `LogicalPlanRef`（装箱 trait object）拥有。`PruneColumns` 和谓词下推仅操作第一个子节点，符合一元 Expand 的结构假设；多余子节点不会被本文件处理。`TakeChildren` 不在本文件使用，因此这里不转移子树所有权。表达式与列在生成层级时采用克隆，`LevelExprs` 生命周期随算子结束；每次再生成前清空旧缓存，避免陈旧投影继续被消费。

## 与 Go 版本的对应关系

共同语义包括：Expand 是谓词屏障；列裁剪必须保留 distinct GROUP BY 列；`GetUsedCols` 为空；层级投影对未激活分组列填 `NULL`；位图模式按分组列逆序构造 GID；数值模式用于位图容量不足的情形；展开清除唯一键；原始 GROUP BY 表达式按 canonical hash 映射到投影列。

当前 Rust 并非逐签名等价移植：

- Go `GenerateGroupingMarks(sourceCols)` 返回供 `GROUPING()` 使用的每参数 mark；Rust 同名方法改为无参缓存生成器，真正的 marks 在 `expression_rewriter.rs` 重新计算。
- Go `ResolveGroupingFuncArgsInGroupBy` 返回列或 `ErrFieldInGroupingNotGroupBy`；Rust 返回表达式，错误校验后移到调用方。
- Go `GenLevelProjections` 根据重复 grouping set 决定是否存在 GPos，并按 schema 尾列位置生成 GID/GPos；Rust按 `GID`/`GPos` 的 `UniqueID` 匹配任意 schema 位置，且只要 `GPos` 字段存在就生成层级下标。
- Go 列裁剪同步删除 `OutputNames`，Rust 本方法只裁剪 schema，未同步修改输出名；这是扩展或补齐移植时需要验证的兼容风险。
- Go `ExtractFD` 直接继承 `LogicalSchemaProducer` 的结果；Rust 额外把 FD 投影到当前输出列并缓存。Rust `BuildKeyInfo` 还显式计算 `MaxOneRow`，而 Go 对照方法只清空 key。
- Go 数值 GID 读取 `RollupGroupingIDs[offset]`；Rust用去重集合的首次出现位置重建相同的共享编号。独立 Rust 测试覆盖了重复集合共享 ID 的行为。

因此本文只能确认本文件已有的 Rust 行为与上述直接接线，不能据 Go 的完整 builder/physical planner 实现宣称 Rust 已端到端支持 SQL 的 ROLLUP/CUBE/GROUPING SETS。

## 扩展指南

新增或调整分组层级编码时，优先修改 `GroupingMode`、`GenerateGroupingMarks` 和两个 GID 生成函数，并同步 `expression_rewriter.rs` 中生成 GROUPING marks 的分支，避免算子缓存与表达式元数据采用不同算法。若增加超过 64 列/参数的策略，应把模式选择放在真实构造点并新增边界测试，而不是依赖 `u64` 自然截断。

改变输出列布局或新增生成列时，应集中修改 `LogicalExpand` 字段、`GenLevelProjections`、列裁剪保留集合、`hash64_equals_generated.rs` 以及物理 Expand 的消费端。必须验证 schema、输出名、每层表达式长度和字段类型一致；尤其要决定是否补齐 Go 的 OutputNames 裁剪语义。

收紧 GROUPING 参数验证时，需要协调 `ResolveGroupingFuncArgsInGroupBy` 与 `expression_rewriter.rs` 的错误归属，并保持 `ErrFieldInGroupingNotGroupBy` 的参数编号兼容。改变 canonical hash 替换策略时，应加入哈希碰撞或结构等价性测试。

测试应继续放在独立文件：本算子的局部行为放入 `logical_expand_test.rs`；跨算子关系放入 `logical_relational_aster_unit_test.rs`；键语义可同步 `logicalop_test/logical_operator_test.rs`；哈希/相等变化同步 `logicalop_test/hash64_equals_test.rs`。若未来接通 plan builder 和物理计划，还应在 planner core 的 SQL/优化器测试中证明 SQL 能构造 Expand、优化后生成全部层级并物理化执行。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust、Go、测试和调用文件均可通过精确文件节点读取。
- 主实现：`pkg/planner/core/operator/logicalop/logical_expand.rs`，已核对 339 行全部源码以及 `GroupingMode`、`GroupingSet(s)`、`LogicalExpand` 和全部 impl 方法。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml`、`pkg/planner/core/operator/logicalop/lib.rs`。
- Rust 直接入口与调用：`pkg/planner/core/optimizer_runtime.rs::resolve_expand_descendants`、`pkg/planner/core/expression_rewriter.rs` 的 GROUPING 重写、`pkg/planner/core/operator/logicalop/logical_apply.rs::extract_correlated_columns`、`pkg/planner/core/operator/logicalop/hash64_equals_generated.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_expand.go` 全文件；RustCodeGraph 还确认 Go 文件被 cascades memo、物理计划枚举、表达式重写及逻辑计划测试使用。
- 独立 Rust 测试：`pkg/planner/core/operator/logicalop/logical_expand_test.rs`；`pkg/planner/core/operator/logicalop/logical_relational_aster_unit_test.rs` 中 Expand 层级投影和 key/MaxOneRow 测试；`pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.rs::TestLogicalExpandBuildKeyInfo`；`logicalop_test/hash64_equals_test.rs::TestLogicalExpandHash64Equals`。
- 调用图限制：RustCodeGraph 的 `callers/callees` 子命令对精确节点 ID 返回了全库同名噪声，未将其作为调用证据；调用关系以索引的文件使用关系、精确符号查询及目标调用点源码交叉确认。生产 Rust 全局精确检索只发现上述消费点和 plan builder 状态字段，未发现实际构造点。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务规定的 11 章节结构命令验证文档形态。
