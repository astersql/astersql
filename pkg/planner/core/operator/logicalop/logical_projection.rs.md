# `pkg/planner/core/operator/logicalop/logical_projection.rs`

## 文件定位

该文件实现逻辑计划树中的投影算子 `LogicalProjection`。投影位于 SQL 逻辑优化阶段，表示 SELECT 列表或优化器为计算派生表达式而插入的中间层：它从唯一子节点取得行，以 `Exprs` 逐项计算输出，并用自身 `Schema` 描述计算结果。源码入口是 [`LogicalProjection`](logical_projection.rs)；crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 以 `mod logical_projection` 装配模块，并通过 `pub use logical_projection::*` 将公共 API 暴露给 planner、规则和物理算子层。

本模块属于 Cargo 包 `astersql-planner-core-operator-logicalop`（`pkg/planner/core/operator/logicalop/Cargo.toml`）。与本文件直接相关的包依赖包括表达式层 `expression`、逻辑计划基础层 `base`、函数依赖 `fd`、物理属性 `property`、计划编码 `plancodec`、规划器工具 `planner_util`、AST 常量 `parser_ast` 和整数集合 `intset`。它不是执行器：实际投影执行由后续物理化产生的 `PhysicalProjection` 承担。

## 核心职责

`LogicalProjection` 集中维护投影表达式、输出 schema 与子计划之间的对应关系，并为各优化阶段提供以下行为：

- 用 `Init` 建立类型为 `plancodec::TypeProj` 的基础逻辑计划，用 `ExplainInfo` 和 `HashCode` 提供可解释文本及结构哈希。
- 在 `PredicatePushDown` / `breakDownPredicates` 中把引用投影输出的谓词改写为子侧表达式，并隔离 `get_var` / `set_var` 副作用。
- 在 `PruneColumns` 中同步裁剪 `Exprs` 与 `Schema.Columns`，保留有副作用的表达式，并计算子节点真正需要的输入列。
- 在 `BuildKeyInfo`、`DeriveStats`、`ExtractColGroups`、`getGroupNDVs` 与 `ExtractFD` 中，把唯一键、列 NDV、列组 NDV 和函数依赖穿过投影映射。
- 在 `PushDownTopN`、`PreparePossibleProperties` 与 `TryToGetChildProp` 中把排序及 MPP 分区要求转换成子节点可理解的属性。
- 用 `SubstituteProjectionExpr`、`ReplaceExprColumns`、`AppendExpr` 和 `InjectExpr` 支持优化规则重写表达式或为已有计划增加计算列。

这些职责共同依赖一个核心不变量：`Exprs[i]` 计算 `Schema.Columns[i]`，两者在语义上必须一一对应。`BuildKeyInfo`、统计推导、属性转换和表达式替换都按同一索引访问两组数据；调用方构造或裁剪投影时必须同时维护它们。

## 主要符号

- `pub struct LogicalProjection`：投影节点主体。
  - `LogicalSchemaProducer` 保存基础计划、输出 schema、子节点、统计与 FD 等公共状态。
  - `Exprs: Vec<Expression>` 是装箱的动态表达式列表，与输出列按位置对应。
  - `CalculateNoDelay` 对应 Go 中 DO 语句根投影需要立即计算的标志；本文件仅保存并在物理化时由调用方传递。
  - `Proj4Expand` 标记由 Expand 生成、即使表达式看似纯列引用也不能宽松消除的投影。
- `Init`：写入计划上下文、`TypeProj` 和 query-block offset。依赖上下文的方法（例如分配列 ID）要求先初始化。
- `ExplainInfo` / `HashCode` / `ReplaceExprColumns`：分别生成说明、稳定编码投影结构、按列哈希递归替换表达式。
- `PredicatePushDown`、`breakDownPredicates`：完成谓词分类、表达式替换、对子节点递归下推及残留谓词处理。
- `PruneColumns`：按父节点所需列、表达式血缘和副作用裁剪投影，并递归裁剪孩子。
- `BuildKeyInfo` / `buildSchemaByExprs`：只允许纯列映射把子节点主键或唯一键传到输出。
- `PushDownTopN`：把 TopN 排序表达式替换为投影输入；删除常量和相关列排序项。
- `PullUpConstantPredicates` / `rewriteProjectionConstantPredicates`：仅对可宽松消除的纯列投影，把孩子的单列常量谓词改写为输出列。
- `DeriveStats` / `getGroupNDVs` / `ExtractColGroups`：推导行数、列 NDV 与列组 NDV；计算表达式不会被当作可逆的列组映射。
- `PreparePossibleProperties`、`TryToGetChildProp`、`tryTransformSortItems`：在投影上下两侧转换顺序、局部顺序与 MPP hash 分区属性。
- `ExtractFD`：为纯列建立等价关系，为常量登记常量列，为确定性标量函数建立严格函数依赖并传播非空信息。
- `AppendExpr` / `InjectExpr`：向已有投影追加计算表达式，或先在任意逻辑计划外包恒等投影再追加。
- `SubstituteProjectionExpr`：递归克隆标量函数并把其中的输出列替换成对应输入表达式；替换后清理函数哈希缓存。
- `replaceProjectionColumns`：按表达式哈希替换普通列或相关列，也递归处理标量函数。
- `hasGetSetVarFunc` / `hasAssignSetVarFunc`：递归识别变量读写及赋值型 `set_var`，作为重排屏障。
- `canProjectionBeEliminatedLoose`：仅当不是 Expand 投影且所有表达式都是直接列引用时返回真。
- `impl LogicalPlan for LogicalProjection`：把 trait 动态分派接到本类型的谓词下推、裁剪、统计和键信息实现；trait 版 `DeriveStats` 还强制恰有一个孩子。

## 执行流程

典型生命周期如下：

1. 计划构造器或优化规则创建 `LogicalProjection { Exprs, ..Default::default() }`，调用 `Init(ctx, qb_offset)`，然后设置与表达式等长、同序的 schema 和一个孩子。`InjectExpr` 可自动完成“恒等投影 + 新表达式”的包装。
2. 逻辑优化进行谓词下推。若任一投影表达式含赋值型 `set_var`，`PredicatePushDown` 不跨投影移动传入谓词，只让孩子处理空谓词并把残留 Selection 挂回；否则 `breakDownPredicates` 检查谓词引用是否能由投影输出、输入或聚合分组/`firstrow` 参数解释，再用 `SubstituteProjectionExpr` 改写。
3. 孩子返回的残留谓词按是否含相关列分组：无相关列的谓词在投影下方组成新的 `LogicalSelection`，相关谓词与原本不可下推谓词向上返回。新 Selection 复用原孩子的 schema、输出名和 query-block offset。
4. 列裁剪阶段用 `expression::GetUsedList` 标出父层实际使用的输出。`projection_lineage_is_used` 补充聚合 `firstrow` 血缘，`ExprHasSetVarOrSleep` 保留有副作用表达式。裁剪时从后向前同步删除 `Exprs` 和 `Schema.Columns`，再从剩余表达式提取输入列交给孩子。
5. 若孩子是 `LogicalTableDual` 且所有输出均无用，`PruneColumns` 仍保留一个新建的常量零列，避免无来源 SELECT 的投影失去输出载体；新列 ID 由 session vars 分配。
6. 属性与统计阶段从孩子向上转换信息。`BuildKeyInfo` 只映射完整可见的纯列键；`DeriveStats` 继承行数并将输入 NDV 映射到输出 ID；`getGroupNDVs` 只有在组内每一列都有纯列映射时才保留组 NDV；`ExtractFD` 汇总并投影函数依赖。
7. 物理化时，`pkg/planner/cascades/old/implementation_rules.rs::ImplProjection::OnImplement` 和 `pkg/planner/core/operator/physicalop/physical_projection.rs::ExhaustPhysicalPlans4LogicalProjection` 调用 `TryToGetChildProp`。排序列若映射为输入列则下推，常量类项被省略，映射为标量函数则拒绝该属性转换。MPP hash 键若是计算列，Rust 实现把孩子要求放宽为 `AnyType`，让 Exchange 在投影计算之后满足父层要求。

表达式替换遵循写时复制：`SubstituteProjectionExpr` 和 `replaceProjectionColumns` 克隆标量函数再递归修改参数，不原地修改可能被别处共享的原表达式；修改参数后调用 `CleanHashCode`，避免继续使用旧哈希。

## 数据与状态

最重要的状态关系是 `Exprs`、输出 `Schema.Columns` 和孩子 schema 三者之间的映射：

- 输出位置 `i` 由 `Exprs[i]` 产生，输出列身份是 `Schema.Columns[i].UniqueID`。
- 纯 `Column` 表达式能够把键、排序、NDV、列组和 FD 映射到孩子；常量和计算表达式通常只能产生输出侧信息，不能反向保证孩子属性。
- `HashCode` 编码计划物理类型 ID、query-block offset、表达式数量以及每个表达式哈希的长度和值，避免简单拼接产生边界歧义。
- `DeriveStats` 缓存于基础计划。未要求 reload 且缓存存在时复用列 NDV，但会按当前列组重新计算 `GroupNDVs`；新统计的 `RowCount` 与孩子相同。
- 对直接列引用，输出 NDV 取孩子同 ID 列的 NDV，缺失时退回孩子行数；对计算表达式，取所引用列 NDV 的最大值，限制不超过行数，无输入列时为 1。
- `ExtractFD` 用表达式哈希在 `FDSet` 中登记常量或标量函数的代表列 ID。确定性标量函数由其普通列和相关列共同决定；非确定性函数只登记身份而不建立稳定依赖。
- `AppendExpr` 对已是列的表达式直接返回该列，不修改投影；对计算表达式先按现有投影替换，再分配新 ID、推导返回类型，并同时追加表达式和 schema 列。

`LogicalProjection` 自身不持有事务、存储句柄或执行期行缓冲；其可变状态局限于逻辑计划树、schema、统计、FD 和表达式集合。

## 依赖与调用关系

上游入口和调用者：

- `pkg/planner/core/operator/logicalop/lib.rs` 装配并公开重导出本模块；RustCodeGraph 显示该 crate 入口被 186 个文件使用，目标文件本身被 15 个文件引用。
- 逻辑计划优化通过 `dyn LogicalPlan` 调用 `PredicatePushDown`、`PruneColumns`、`DeriveStats` 和 `BuildKeyInfo`；本文件的 trait 实现完成动态分派。
- `pkg/planner/core/optimizer_runtime.rs` 多处直接调用 `logicalop::SubstituteProjectionExpr`，用于优化规则跨投影改写条件或表达式。
- `pkg/planner/core/operator/logicalop/logical_selection.rs` 调用公开的 `breakDownPredicates`，处理 Selection 与 Projection 相邻时的谓词重写。
- Cascades 实现规则和 `ExhaustPhysicalPlans4LogicalProjection` 调用 `TryToGetChildProp`，随后创建 `PhysicalProjection` 候选。

主要下游依赖：

- `expression` 提供动态表达式、列/schema、列提取、哈希、类型推导、常量和标量函数检查。
- `BaseLogicalPlan` / `LogicalSchemaProducer` 提供上下文、孩子、schema、统计和 FD 存储，以及默认优化行为。
- `property` 提供 `PhysicalProperty`、`SortItem`、MPP 分区列、`StatsInfo` 和 `GroupNDV`。
- `fd::FDSet` 与 `intset` 承载等价、常量、严格依赖、非空列及最终列投影。
- `planner_util::IsNullRejected` 判断标量函数结果是否可标为非空。
- `plancodec` 提供 Projection 类型编码；`parser_ast` 提供 `SetVar` / `GetVar` 函数名常量。

RustCodeGraph 的精确 `callers` / `callees` 查询未为 `SubstituteProjectionExpr` 和 `InjectExpr` 返回边；上述自由函数调用关系因此由索引文件引用信息和精确源码调用点搜索共同确认，不能把“无图边”解释为“无调用者”。

## 错误处理与边界

- trait 版 `DeriveStats` 要求恰有一个孩子，否则返回 `PlannerError("projection must have exactly one child")`。固有方法则要求调用方传入孩子统计和自身 schema。
- `PruneColumns` 在缺少表达式上下文时返回 `PlannerError`；Dual 特例和 `AppendExpr` 对已初始化上下文使用 `expect`，错误构造的未初始化节点会 panic。
- 多个方法以 `first()` / `first_mut()` 容忍缺少孩子，但这只是防御性退化，不代表零孩子投影是合法完整计划。例如谓词会保持在上层，键信息不变，TopN 下推返回 `None`。
- `Exprs` 与 schema 长度不一致会导致基于 `zip` 的逻辑静默忽略尾项，或在按索引访问的属性转换中产生越界风险。构造投影时必须维护一一对应不变量。
- `breakDownPredicates` 遇 `get_var` / `set_var` 会保留原谓词，防止变量读取或写入被重排。整个投影含赋值型 `set_var` 时，谓词和 TopN 都不跨该节点下推。
- `TryToGetChildProp` 遇输出排序列不在 schema 时跳过该项；映射为标量函数时返回 `(None, false)`；常量等其它表达式不形成孩子排序项。MPP hash 计算键无法映射时放宽孩子分区属性，而不是错误地宣称孩子能按尚未计算的列分区。
- 常量谓词上拉只接受恰好引用一列、且该列表达式哈希可在投影中找到的候选；多列谓词或投影不可见列会被丢弃。
- `ExtractFD` 明确跳过相关列自身，也不会为非确定性标量函数建立严格 FD，以免产生不成立的优化事实。

## 并发与资源生命周期

该模块没有线程、异步任务、锁、通道或显式 I/O。优化过程通过 `&mut self` 串行修改计划节点；Rust 借用规则保证同一时刻的独占可变访问。`LogicalPlanRef` 和 `Expression` 使用装箱 trait object 表示树与表达式，所有权在 `SetChildren`、`TakeChildren` 和返回值之间显式转移。

谓词下推插入 Selection 时，代码先 `TakeChildren` 取得原孩子所有权，再构造并设置新孩子，最后把 Selection 放回 Projection，避免悬空引用。表达式重写通过克隆保持调用者仍可能持有的原对象不变；独立测试 `logicalop_test/logical_operator_test.rs::TestReplaceColumnOfExprCopyOnWrite` 和 `TestResolveExprAndReplaceCopyOnWrite` 验证这一点。

资源生命周期中唯一的全局式可变来源是 session vars 的 `AllocPlanColumnID`，用于 Dual 保底列和追加计算列；因此这些路径必须保留已初始化的计划上下文。统计与 FD 是计划节点级缓存，重写 schema 或表达式后，调用方应确保后续优化阶段重新推导受影响的缓存。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_projection.go`。Rust 保留了 Go 的主体类型、字段和大部分同名方法，尤其是哈希编码、变量副作用屏障、纯列键映射、常量谓词上拉、列组 NDV、FD 抽取、排序属性转换、宽松消除和表达式注入。以下差异是当前源码事实：

- Go `ExplainInfo` 使用 schema 与日志脱敏设置生成说明；Rust 逐表达式调用 `StringWithCtx`，未在本方法中读取 `EnableRedactLog` 或 schema。
- Go `PruneColumns` 在 schema 被完全裁空时可直接返回孩子；Rust 除 Dual 保底列外保留 Projection 节点，并额外用 `projection_lineage_is_used` 兼顾聚合 `firstrow` 血缘。
- Go `PushDownTopN` 在替换后执行常量折叠，检查函数不可变性及 projection 生成的 ID=0 列；Rust 当前只替换表达式并删除常量/相关列，没有这些 Go 安全检查。扩展这一路径时不能假定两端已完全等价。
- Go `DeriveStats` 调用 `cardinality::EstimateColsNDVWithMatchedLen`；Rust 采用直接列映射或“引用列最大 NDV”的局部估计，结果可能与 Go 的匹配长度/联合估算不同。
- Rust `TryToGetChildProp` 额外处理 `MPPPartitionTp == HashType`：纯列键映射到孩子，计算键则放宽为 `AnyType`。所读 Go 文件的对应方法只转换排序与 partial order。
- Go `AppendExpr` 复制 coercibility、repertoire，并清理 JSON 类型的 `ParseToJSONFlag`；Rust 当前只设置返回类型和 UniqueID。这是字符串排序规则、字符 repertoire 与 JSON 类型兼容的显式迁移风险。
- Go `buildSchemaByExprs` 为非列表达式分配新的占位列；Rust 复用相同位置的已有输出列，缺位时用默认列。Rust 调用该方法前必须保证输出 schema 已正确建立。

Go 测试 `pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.go::TestLogicalProjectionPushDownTopN` 从 SQL EXPLAIN 验证 TopN/Projection 的计划形态；Rust 同名测试验证排序输出列映射回输入列，但覆盖面小于 Go 的端到端场景。因此本文只把已读 Rust 实现和测试覆盖的行为标为已验证。

## 扩展指南

- 新增输出表达式种类时，先检查所有依赖“纯列 / 常量 / 标量函数”分类的路径：`SubstituteProjectionExpr`、`replaceProjectionColumns`、`tryTransformSortItems`、`DeriveStats`、`ExtractFD`、`ExtractColGroups` 和 `canProjectionBeEliminatedLoose`。未知表达式目前多采取保守保留或不映射策略。
- 修改谓词下推时，同步审查 `breakDownPredicates`、`child_accepts_group_column`、`projection_lineage_is_used` 以及 `logical_selection.rs` 的调用。必须保持变量函数副作用和相关谓词边界。
- 修改 `Exprs` 的任何代码都应同步维护 schema、清理相关表达式哈希，并考虑使统计/FD 缓存失效。不要只追加或删除其中一侧。
- 改进 TopN 下推时，应优先对齐 Go 的常量折叠、不可变函数与生成列保护，并为非确定函数、ID=0 派生列、常量排序项添加独立 Rust 回归测试。
- 改进 `AppendExpr` 时，应核对 Go 的 coercibility、repertoire 和 JSON flag 语义；这是类型兼容而非单纯元数据美化。
- 改进统计时，应说明是否引入 `cardinality` 估算器，以及缓存 reload、计算表达式、多列相关性和 GroupNDV 如何处理。
- MPP 属性变换必须保留“计算键在投影后才存在”的顺序；相关测试是 `logical_relational_aster_unit_test.rs::projection_keeps_mpp_hash_property_candidate_for_computed_output`。
- 测试必须继续放在独立文件。最邻近文件是 `logical_projection_test.rs`；跨算子行为目前还在 `logical_relational_aster_unit_test.rs`、`logical_d_aster_unit_test.rs` 和独立 crate 测试 `logicalop_test/logical_operator_test.rs`。不要把测试内嵌回生产源文件。
- 兼容性风险集中在谓词/TopN 重排的语义安全、类型与排序规则元数据、统计估值差异；性能风险主要是递归表达式克隆、重复列提取和 FD 哈希登记。更改前后应以目标规则的独立测试和 Go 对照用例验证。

## 验证依据

本说明基于以下直接证据完成：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust 文件完整索引为 882 行，并显示被 15 个文件使用。
- RustCodeGraph 文件/符号读取：`logical_projection.rs::LogicalProjection`、`SubstituteProjectionExpr`、`InjectExpr`，以及目标文件全部实现；精确自由函数 `callers` / `callees` 未返回边，已在“依赖与调用关系”说明限制。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml` 和 `pkg/planner/core/operator/logicalop/lib.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_projection.go` 全文，以及 `pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.go::TestLogicalProjectionPushDownTopN`。
- Rust 独立测试：
  - `pkg/planner/core/operator/logicalop/logical_projection_test.rs::projection_rewrites_pulled_predicate_to_output_column`；
  - `pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.rs::{TestReplaceColumnOfExprCopyOnWrite, TestResolveExprAndReplaceCopyOnWrite, TestLogicalProjectionPushDownTopN}`；
  - `pkg/planner/core/operator/logicalop/logical_relational_aster_unit_test.rs::{projection_trait_derives_output_column_statistics, projection_keeps_mpp_hash_property_candidate_for_computed_output}`；
  - `pkg/planner/core/operator/logicalop/logical_d_aster_unit_test.rs::projection_elimination_requires_only_direct_columns`。
- 生产调用点：`pkg/planner/cascades/old/implementation_rules.rs::ImplProjection::OnImplement`、`pkg/planner/core/operator/physicalop/physical_projection.rs::ExhaustPhysicalPlans4LogicalProjection`、`pkg/planner/core/optimizer_runtime.rs` 中的 `SubstituteProjectionExpr` 调用，以及 `logical_selection.rs` 对 `breakDownPredicates` 的调用。

本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文恰有且仅有上述十一个固定二级标题；运行结果和退出码在任务交付时记录。
