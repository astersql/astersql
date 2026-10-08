# `pkg/planner/core/operator/logicalop/logical_top_n.rs`

## 文件定位

该文件定义逻辑规划阶段的 `LogicalTopN` 节点，表达“按若干表达式排序，跳过 `Offset` 行，再保留 `Count` 行”的关系代数语义，对应 SQL 中常见的 `ORDER BY ... LIMIT ... OFFSET ...`。它属于 Cargo 包 `astersql-planner-core-operator-logicalop`；包入口 `pkg/planner/core/operator/logicalop/lib.rs` 以 `mod logical_top_n` 纳入实现并通过 `pub use logical_top_n::*` 对外导出。

`LogicalTopN` 位于逻辑优化与物理计划枚举之间：上游规则可创建、改写或下推该节点；下游 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 识别该类型并调用 `ExhaustPhysicalPlans4LogicalTopN`，由 `physical_topn.rs` 枚举物理 TopN 和保序 Limit 候选。本文件本身不执行排序，也不访问存储。

## 核心职责

- 保存 TopN 的排序项、分区项、行数参数和下推偏好，见 `LogicalTopN`。
- 提供计划展示文本，见 `ExplainInfo`。
- 配合逻辑优化完成表达式列替换、列裁剪、键与基数推导、有序属性推导和相关列提取，见 `ReplaceExprColumns`、`PruneColumns`、`BuildKeyInfo`、`DeriveStats`、`PreparePossibleProperties`、`ExtractCorrelatedCols`。
- 在接入子计划时完成两个语义保持的简化：直接折叠 `LogicalTableDual` 的行数，或将没有排序项的 TopN 改写成 `LogicalLimit`，见 `AttachChild`。
- 通过 `LogicalPlan` trait 暴露基类、动态类型转换和列裁剪入口，使通用优化流程能够操作该具体节点。

本文件不负责生成 `PhysicalTopN`、计算执行代价、真正排序或维护哈希等价逻辑。物理化位于 `physical_topn.rs`，而 `Hash64`/`Equals` 位于生成式邻接实现 `hash64_equals_generated.rs`。

## 主要符号

### `LogicalTopN`

公开结构体内嵌 `LogicalSchemaProducer`，因此继承逻辑计划的上下文、子节点、schema、统计信息及 `MaxOneRow` 等状态。其业务字段如下：

- `ByItems: Vec<ByItems>`：排序表达式及升降序方向；为空时 `IsLimit` 返回 `true`。
- `PartitionBy: Vec<SortItem>`：扩展 TopN 的分区键，供分区内 TopN/窗口派生场景使用。
- `Offset`、`Count`：跳过和保留的行数。
- `OffsetParam`、`CountParam`：Rust 侧保留的参数位置；`AttachChild` 在降级成 `LogicalLimit` 时继续传递它们。
- `PreferLimitToCop`：提示后续流程优先把 Limit 下推到 coprocessor。

### 初始化与说明

- `Init(self, ctx, offset) -> Self`：调用 `NewBaseLogicalPlan(ctx, "TopN", offset)`，安装计划上下文、节点类型名和查询块偏移。依赖上下文的方法应在初始化后调用。
- `ExplainInfo(&self) -> String`：按顺序输出分区键、排序键（降序项追加 `:desc`）以及 `offset/count`；表达式字符串使用计划上下文中的求值上下文。

### 优化辅助方法

- `ReplaceExprColumns`：按列哈希码映射调用 `rule_util::ResolveExprAndReplace`，逐项替换 `ByItems` 内的列引用。
- `PruneColumns`：提取排序表达式实际依赖的列，将它们与父节点可见列合并后下推到唯一子节点；随后刷新自身 schema，并只向父侧内联原先可见列。
- `BuildKeyInfo`：先委托 `LogicalSchemaProducer`，再在 `Count == 1` 时设置 `MaxOneRow`。
- `DeriveStats`：在允许复用缓存时直接返回；否则要求至少一个子统计，并用 `property::DeriveLimitStats(child, Count)` 限制输出基数。
- `PreparePossibleProperties`：把纯列排序项转成一个候选顺序，同时继承第一个子属性的 `HasTiFlash`；无法形成列顺序时返回空 `Orders`。
- `ExtractCorrelatedCols`：从全部排序表达式中收集相关列。
- `GetPartitionBy`、`IsLimit`：分别提供分区键只读视图及“无排序项”的判定。
- `AttachChild`：接入子节点并执行 Dual 折叠或 Limit 规范化。

### `impl LogicalPlan for LogicalTopN`

`as_any`/`as_any_mut` 支持优化器按具体类型下转；`base`/`base_mut` 返回内嵌基类；trait 的 `PruneColumns` 转发到本类型同名实现。其余通用行为由 `BaseLogicalPlan`/`LogicalSchemaProducer` 提供。

## 执行流程

一个典型生命周期如下：

1. 计划构造或重写代码填写 `ByItems`、`PartitionBy`、`Offset`、`Count` 等字段，并调用 `Init` 绑定规划上下文。
2. 下推流程用 `AttachChild` 接回改写后的子树。若子节点是 `LogicalTableDual`，它直接按 `max(row_count - offset, 0)` 再取与 `Count` 的较小值，并移除 TopN；若 `ByItems` 为空，则构造等价 `LogicalLimit`；否则保存该子节点并保留 TopN。
3. 列裁剪阶段由通用 `LogicalPlan::PruneColumns` 调度到本实现。排序项先经 `pruneSortByItems` 清理，并把必需排序列加入下推集合；子节点裁剪完成后，本节点同步子 schema，再执行内联投影。
4. 属性推导阶段，`BuildKeyInfo` 传播键并识别最多一行，`DeriveStats` 计算限制后的基数，`PreparePossibleProperties` 暴露排序可能提供的顺序，`ExtractCorrelatedCols` 为相关子查询处理提供依赖。
5. 物理化阶段，`base_physical_plan.rs` 下转为 `LogicalTopN` 并调用 `ExhaustPhysicalPlans4LogicalTopN`。后者先检查所需物理属性是否匹配 `ByItems`，再生成物理 TopN 与保序 Limit 两类候选；实际执行逻辑不在本文件。

已确认的直接 Rust 调用链包括 `LogicalLimit::PushDownTopN -> LogicalTopN::AttachChild`。此外，分区 Union、内存表、Selection、优化器运行时和 Cascades 规则会构造或识别 `LogicalTopN`；这些属于本节点的使用方而非其内部逻辑。

## 数据与状态

节点拥有 `ByItems` 和 `PartitionBy` 两组顺序相关数据。`ReplaceExprColumns` 与 `PruneColumns` 会原地改写 `ByItems`；后者用 `std::mem::take` 暂时移出向量，再把处理后的项放回，避免克隆表达式对象。`PartitionBy` 在本文件中只被读取或在降级为 `LogicalLimit` 时转移所有权，不参与相关列提取或排序属性生成。

`Offset` 与 `Count` 使用 `u64`，但 Dual 的 `RowCount` 是 `i32`。`AttachChild` 先用 `max(0)` 消除负行数，再转为 `u64` 计算，最终结果不会超过原 Dual 行数或 `Count`。`Count == 1` 会把基类状态中的 `MaxOneRow` 置真；`Count == 0` 不触发该标记，但统计推导可得到零行估计。

统计信息缓存在基类中。`DeriveStats` 仅当 `reloads` 恰有一个元素且为真时强制重算；否则已有缓存会被复用。排序可能属性只保留能够由 `getPossiblePropertyFromByItems` 表达的列顺序，并从第一个子属性复制 `HasTiFlash`。

`OffsetParam`/`CountParam` 是 Rust 相对当前 Go 结构额外保存的参数元数据；本文件只在 TopN 降级为 Limit 时透传，不在展示或统计计算中读取。

## 依赖与调用关系

crate 边界由 `pkg/planner/core/operator/logicalop/Cargo.toml` 确认。本文件的直接关键依赖为：

- 当前 crate：`BaseLogicalPlan`、`LogicalSchemaProducer`、`LogicalLimit`、`LogicalTableDual`、`LogicalPlan`、`Schema`、`StatsInfo`、`ByItems` 和公共辅助函数。
- `astersql-expression`（别名 `expression`）：表达式字符串化、相关列提取及表达式 trait。
- `astersql-planner-property`（别名 `property`）：`DeriveLimitStats` 及排序/统计属性。
- `astersql-planner-core-rule-util`（别名 `rule_util`）：表达式列替换。
- `astersql-planner-core-base`（别名 `base`）：`Init` 所需的 `ContextRef`。

RustCodeGraph 的索引状态为 11,467 个文件、307,296 个节点。针对本文件的查询确认：通用 `base_logical_plan.rs` 调用具体计划的 `PruneColumns`、`DeriveStats` 和 `PreparePossibleProperties`；`logical_limit.rs` 调用 `AttachChild`；`physical_topn.rs` 的 `ExhaustPhysicalPlans4LogicalTopN` 消费本节点。Go 对照图还显示 CTE、Join、Limit、MemTable 和 Projection 的 `PushDownTopN` 会调用 `AttachChild`，说明该方法是 TopN 下推后重接子树的标准边界。

## 错误处理与边界

- `PruneColumns` 返回 `Result<()>`，并用 `?` 原样传播子节点裁剪错误；没有子节点时不会报错，只跳过 schema 刷新并执行当前节点的内联投影。
- `DeriveStats` 在没有子统计时返回 `PlannerError("TopN requires child statistics")`，而不是索引越界；成功时更新缓存并返回“已重算”标志。
- `AttachChild` 将已经初始化的规划上下文视为不变量。纯 Limit 分支若缺失上下文会以 `expect("initialized TopN must retain planner context")` 终止，这属于构造协议违例，而非可恢复的规划错误。
- Dual 折叠显式处理 `RowCount < 0`、`Offset` 大于行数以及 `Count` 小于剩余行数的边界。
- `PreparePossibleProperties` 对空子属性列表安全返回 `HasTiFlash = false`；非列表达式无法形成候选顺序时返回空顺序集合。
- `ExplainInfo` 在没有上下文时仍可格式化表达式，但传给表达式字符串化的求值上下文为空；初始化后的正常路径会使用真实求值上下文。

本文件未验证参数索引的绑定与求值，这些字段只被保存和透传。修改参数化 LIMIT/OFFSET 行为时必须继续追踪 `LogicalLimit`、计划构造器和执行层，不能仅改本文件。

## 并发与资源生命周期

`LogicalTopN` 不创建线程、异步任务、锁、通道、文件句柄、网络连接或事务；它是规划期拥有型数据结构。优化方法普遍接收 `&mut self`，依靠 Rust 独占借用串行改写节点状态，没有内部同步机制，也未声明跨线程共享契约。

表达式和子计划由节点拥有：`AttachChild` 消费 `self` 与 `LogicalPlanRef`，保证重写后只返回一个新的树根；Dual 分支返回原子节点，Limit 分支把参数及分区数据移动到新节点，TopN 分支则把子节点放入自身。`PruneColumns` 暂时移出 `ByItems` 后在同一调用内恢复；错误发生在子节点裁剪时，此前排序项已完成规范化，因此调用者不应假设失败后节点完全保持调用前状态。

规划上下文通过基类中的引用持有，生命周期覆盖节点的解释、重写和物理化阶段。本文件不显式释放资源，节点树被丢弃时由 Rust 所有权自动回收。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/logicalop/logical_top_n.go`。两版共同实现 `Init`、`ExplainInfo`、`ReplaceExprColumns`、`PruneColumns`、`BuildKeyInfo`、`DeriveStats`、`PreparePossibleProperties`、`ExtractCorrelatedCols`、`GetPartitionBy`、`IsLimit` 和 `AttachChild`，核心分支保持一致：Dual 行数折叠、空 `ByItems` 转 Limit、`Count == 1` 标记最多一行，以及按 `Count` 推导统计。

需要注意的实现差异：

- Rust 结构额外保存 `OffsetParam` 和 `CountParam`，转为 `LogicalLimit` 时会保留；当前 Go 对照结构没有这两个字段。
- Go `PruneColumns` 在子节点裁剪后将 TopN schema 清空，再由 `InlineProjection` 重建；Rust 显式克隆子 schema 后调用 `SetSchema`，目标同为清除陈旧/重复的隐藏排序列。Rust 与 Go 的 `TestLogicalTopNPruneColumnsRefreshesSchemaBeforeInlineProjection` 都验证最终 schema 只含三个目标列。
- Go `DeriveStats` 直接取 `childStats[0]`；Rust 对缺失子统计返回可诊断错误。两者正常单子节点路径一致。
- Go 的 `PreparePossibleProperties` 将 `hasTiFlash` 缓存在节点内部；Rust 返回包含 `HasTiFlash` 的 `SortProperties`，本结构没有同名独立缓存字段。
- Rust 的 `AttachChild` 对负 `LogicalTableDual::RowCount` 先归零，避免有符号到无符号的异常转换；Go 正常规划路径假定行数非负。

独立测试 `logical_top_n_test.rs` 覆盖相关列替换与降序解释文本。`logicalop_test/logical_operator_test.rs` 对齐 Go 回归测试的 schema 刷新语义；`logicalop_test/hash64_equals_test.rs` 与 Go 同名测试核对排序表达式、方向、分区键、offset/count 及下推偏好对等价性和哈希的影响。

## 扩展指南

- 新增影响 TopN 语义的字段时，应同步检查 `LogicalTopN`、`AttachChild` 的 Limit 转换、`physical_topn.rs::new_topn` 的物理字段复制，以及 `hash64_equals_generated.rs` 的 `Hash64`/`Equals`。否则 memo 去重、规则匹配或执行计划可能忽略新语义。
- 改动排序表达式处理时，应同时审查 `ReplaceExprColumns`、`PruneColumns`、`PreparePossibleProperties`、`ExtractCorrelatedCols` 和 `ExplainInfo`，并补充独立测试文件 `logical_top_n_test.rs`；不要把测试嵌入生产 `.rs`。
- 改动下推或挂接语义时，应覆盖三种 `AttachChild` 结果：Dual 折叠、空排序项转 `LogicalLimit`、保留 `LogicalTopN`。还应复核 `logical_limit.rs::PushDownTopN` 以及 Projection、Join、CTE、MemTable 等上游规则。
- 改动基数估计时，应验证缓存命中、强制 reload、缺失子统计、`Count == 0/1` 和大 `Offset`。注意当前统计公式只使用 `Count`，offset 的成本/基数语义需与 property 层和 Go 版本一起评估。
- 改动 `PartitionBy` 时，应同步物理 TopN 枚举和哈希等价测试；当前相关列提取及可能顺序只基于 `ByItems`，若要改变这一约定必须提供 Go 对照或明确的迁移依据。
- 性能风险主要来自复制 schema/列集合、重复格式化表达式以及扩大物理候选集合；兼容风险集中在 EXPLAIN 文本、参数化 LIMIT/OFFSET、memo 哈希等价和 coprocessor 下推行为。

## 验证依据

事实核对使用了以下路径：

- 生产实现：`pkg/planner/core/operator/logicalop/logical_top_n.rs`（完整 247 行）。
- crate 与模块接线：`pkg/planner/core/operator/logicalop/Cargo.toml`、`pkg/planner/core/operator/logicalop/lib.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_top_n.go`。
- 直接测试：`pkg/planner/core/operator/logicalop/logical_top_n_test.rs`。
- 对齐与邻接测试：`pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.rs`、同名 `.go` 文件，以及 Rust/Go 的 `logicalop_test/hash64_equals_test.*`。
- 上下游实现：`logical_limit.rs`、`hash64_equals_generated.rs`、`physicalop/physical_topn.rs`、`physicalop/base_physical_plan.rs`。

RustCodeGraph 执行了 `status`、针对文件及 `LogicalTopN` 关键方法的 `explore`/`query`/`node` 查询。查询确认了源文件完整符号集合、逻辑 trait 调度入口、`LogicalLimit::PushDownTopN -> AttachChild` 调用边和 `ExhaustPhysicalPlans4LogicalTopN` 物理化出口。未运行 Cargo 或代码测试，符合本任务纯文档分析约束；交付验证仅采用任务指定的十一章节结构检查及人工事实复核。
