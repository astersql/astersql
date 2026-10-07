# `pkg/planner/core/operator/logicalop/logical_limit.rs`

## 文件定位

本文件实现逻辑优化阶段的 `LogicalLimit` 节点，表达 SQL `LIMIT count OFFSET offset`（以及增强 TopN 优化使用的可选分区键），位于 SQL AST 已构造成逻辑计划、但尚未枚举物理执行计划的中间层。crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 通过 `mod logical_limit` 编译该模块，并用 `pub use logical_limit::*` 将类型导出；crate 边界由同目录 `Cargo.toml` 的包 `astersql-planner-core-operator-logicalop` 定义。

常规入口是 `pkg/planner/core/logical_plan_builder_runtime.rs::build_limit_runtime`：它读取 AST 中的 offset/count 和预处理参数位置，设置 `FLAG_PUSH_DOWN_TOP_N`，在结果恒为空时直接生成 `LogicalTableDual`，否则初始化 `LogicalLimit`、复制子节点 schema/输出名并挂接唯一子节点。下游 `pkg/planner/core/operator/physicalop/physical_limit.rs::ExhaustPhysicalPlans4LogicalLimit` 和 `pkg/planner/cascades/old/implementation_rules.rs::ImplLimit` 将该逻辑节点实现为物理 Limit。

## 核心职责

- 保存 Limit 的窗口参数：`Offset` 是跳过行数，`Count` 是最多返回行数；`OffsetParam`/`CountParam` 记录预处理语句参数索引。
- 充当谓词下推屏障。Limit 会改变“第几行被保留”，所以父谓词不能穿过它；但子树自身仍以空谓词继续递归优化，其残差必须重新物化为 `LogicalSelection`。
- 在列裁剪后让自身 schema 与子节点保持一致，并对父节点仍可见的列执行 `LogicalSchemaProducer::InlineProjection`。
- 维护优化器元信息：`Count == 1` 时标记 `MaxOneRow`；按 `Count` 截断行数和 NDV 统计；生成稳定的语义哈希和 explain 文本。
- 在 TopN 下推阶段把自身转换为无排序键的 `LogicalTopN`，交给子树继续下推，必要时再把来自父层的 TopN 挂回结果之上。

这些职责由 `LogicalLimit` 的固有方法实现，并在 `impl LogicalPlan for LogicalLimit` 中接入统一逻辑计划接口。

## 主要符号

- `pub struct LogicalLimit`：节点状态。`LogicalSchemaProducer` 内嵌公共逻辑计划基座；`PartitionBy: Vec<SortItem>` 是增强 TopN/分区 Limit 元数据；`PreferLimitToCop` 是下推偏好；`IsPartial` 是部分 Limit 标志。当前文件只存储 `IsPartial`，仓库 Rust 生产代码中未发现读取点，不能据此宣称它已参与规划决策。
- `Init(self, ctx, offset) -> Self`：以类型名 `"Limit"` 调用 `NewBaseLogicalPlan`，分配计划 ID，保留 `ContextRef` 和 query block offset。需要计划上下文的方法应使用初始化后的节点。
- `ExplainInfo(&self) -> String`：把 `PartitionBy` 经 `SortItem::String` 连接；有分区键时输出 `partition by ..., offset:..., count:...`，否则只输出 offset/count。
- `HashCode(&self) -> [u8; 24]`：按大端序编码 4 字节物理类型 ID、4 字节 query block offset、8 字节 offset、8 字节 count。`LogicalPlan::HashCode` 将该固定数组转成 `Vec<u8>`。
- `PredicatePushDown(&mut self, predicates) -> Result<Vec<Expression>>`：不给子节点传入父谓词；对子树返回的残差调用 `AttachSelectionToPlan`；最后原样把父谓词返回给 Limit 上层。
- `PruneColumns(&mut self, parent_used_cols) -> Result<()>`：先复制父列集合，再递归裁剪第一个子节点，克隆子 schema 到自身，最后内联投影。
- `BuildKeyInfo(&mut self)`：先委托 `LogicalSchemaProducer` 构建键信息；当且仅当本方法观察到 `Count == 1` 时设置 `MaxOneRow(true)`。
- `PushDownTopN(&mut self, upper) -> Option<LogicalPlanRef>`：取走第一个子节点，调用私有 `convertToTopN` 生成本地 TopN 并下推；若有父层 TopN，则用 `AttachChild` 将其挂到下推结果上。
- `DeriveStats(&mut self, child_stats, reloads) -> (StatsInfo, bool)`：允许复用缓存；否则调用 `property::DeriveLimitStats` 并缓存新统计。
- `GetPartitionBy(&self) -> &[SortItem]`：只读暴露分区键。
- `convertToTopN(&self) -> LogicalTopN`：复制 offset/count、参数索引和 cop 偏好，保留上下文与 query block；刻意不复制 `PartitionBy` 和 `IsPartial`。

文件没有模块级常量、条件编译项或异步函数。

## 执行流程

1. `build_limit_runtime` 从 AST/已绑定参数解析 offset 和 count，并以 `u64::MAX - offset` 限制 count，避免构建阶段求和溢出；零窗口直接折叠成空 `LogicalTableDual`。
2. 非空窗口构造 `LogicalLimit`，通过 `Init` 取得上下文/计划 ID，继承输入 schema、输出名和唯一子计划。提示可设置 `PreferLimitToCop`。
3. 逻辑优化通过 `LogicalPlan` trait 调用本文件覆写的钩子：谓词停在 Limit 上方，列裁剪递归到子树，键信息产生 `MaxOneRow`，统计由子统计按 count 截断。
4. TopN 下推时，`PushDownTopN` 先把 Limit 映射为无排序项的 `LogicalTopN`，再调用子节点的 `PushDownTopN`。子节点拒绝或只执行默认挂接时，转换后的节点仍包住原子树；若传入 `upper`，它最后位于整个结果之上。
5. 物理化时，经典 Cascades 的 `ImplLimit` 或 `ExhaustPhysicalPlans4LogicalLimit` 创建 `PhysicalLimit`。后者只接受无排序要求的属性，为孩子设置 `ExpectedCnt = Offset + Count`，并继续传递参数索引、`PartitionBy`、schema 和 query block 信息。
6. 执行或 explain 阶段分别消费物理计划或 `ExplainInfo`；本文件本身不读取数据行，也不实现运行时截断。

## 数据与状态

`LogicalLimit` 是单子节点逻辑算子。公共状态（上下文、ID、query block、children、schema、输出名、统计缓存、`max_one_row` 等）位于 `LogicalSchemaProducer.BaseLogicalPlan`；本文件只直接维护 Limit 专属字段。

重要不变量和缓存规则如下：

- `Offset`/`Count` 是 `u64`；逻辑语义是窗口，而统计只按 `Count` 截断，不从估计行数中再减 `Offset`，与 Go 实现一致。
- `HashCode` 只包含计划类型、query block、offset 和 count，不包含 `PartitionBy`、参数索引、下推偏好或 `IsPartial`。另一个生成实现 `hash64_equals_generated.rs` 的 `Hash64`/`Equals` 会比较 `PartitionBy`、offset、count 及 schema producer；两套哈希服务于不同接口，不应混用其相等语义。
- `DeriveStats` 在 `reloads` 首项为 false 且已有缓存时返回缓存并报告“未重新计算”。重算由 `DeriveLimitStats` 令 `RowCount = min(child.RowCount, Count)`、各列 NDV 不超过新行数、保留直方图引用、清空 GroupNDV。
- `OffsetParam`/`CountParam` 沿 `LogicalLimit -> LogicalTopN/PhysicalLimit -> cache_snapshot` 传播，使预处理 LIMIT 参数能够进入计划快照；它们不参与本文件的 24 字节 `HashCode`。
- `BuildKeyInfo` 只在 count 恰为 1 时把 `MaxOneRow` 置 true；本方法不显式清除旧值，因此复用并修改节点字段时必须保证公共优化流程会重建一致状态。

## 依赖与调用关系

上游直接证据：

- `pkg/planner/core/logical_plan_builder_runtime.rs::build_limit_runtime` 是 SQL LIMIT 的构建入口。
- `pkg/planner/core/operator/logicalop/logical_top_n.rs::LogicalTopN::AttachChild` 在没有 `ORDER BY` 时把纯 TopN 重新降为 `LogicalLimit`。
- `pkg/planner/core/optimizer_runtime.rs` 的规则会识别、构造或移动 `LogicalLimit`；`pkg/planner/cascades/old/transformation_rules.rs` 也创建部分/合并后的 Limit。
- `pkg/planner/cascades/memo/group_expr.rs` 通过 `LogicalPlan` trait 转发谓词、列裁剪、TopN 和统计接口，并将 `LogicalLimit` 纳入 memo 的 hash/equality 路由。

本文件的主要下游依赖：

- `base::ContextRef` 与 `NewBaseLogicalPlan`：初始化计划身份和上下文。
- `BaseLogicalPlan`/`LogicalSchemaProducer`：子节点、schema、统计、输出名、MaxOneRow 等公共状态。
- `PredicatePushDownPlan` 与 `AttachSelectionToPlan`：递归优化子树并确保残差谓词不丢失。
- `LogicalTopN`：承载可继续下推的等价窗口；`AttachChild` 负责把父 TopN 接回树中。
- `property::DeriveLimitStats`：截断基数与 NDV。
- `plancodec::{TypeLimit, TypeStringToPhysicalID}`：生成兼容 Go 布局的哈希类型码。

物理消费者是 `physical_limit.rs::ExhaustPhysicalPlans4LogicalLimit` 和 `implementation_rules.rs::ImplLimit`。前者枚举 Root/Cop/可选 MPP 任务，后者服务经典 Cascades 实现规则。

## 错误处理与边界

- `PredicatePushDown` 使用 `?` 原样传播子树下推和残差 Selection 构造错误。`AttachSelectionToPlan` 在需要挂 Selection 但子计划没有上下文时返回 `PlannerError`；空残差和零行 `TableDual` 则直接成功。
- `PruneColumns` 原样传播子节点裁剪错误；只有子节点存在时才替换自身 schema，但无子节点时仍会执行内联投影。
- trait 版 `DeriveStats` 会先递归子节点并传播其错误。无子节点时使用默认 `StatsInfo`，而不是索引越界。
- `PushDownTopN` 无子节点时返回 `None`，不会 panic；但它用 `TakeChildren` 取走所有孩子并只消费第一个，因此调用方必须维持单子节点不变量，否则额外孩子会被丢弃。
- `convertToTopN` 要求节点已经 `Init`；缺少 planner context 会以明确的 `expect` 消息 panic。这是内部不变量检查，不是可恢复错误。
- `QueryBlockOffset` 在哈希中转成 `u32` 并以大端序编码，保持与 Go `uint32` 转换一致；负的 `i32` 会按二进制补码映射。
- `Offset + Count` 在物理属性中使用 `wrapping_add`，而 AST 构建入口预先压低 count 以避免常规 SQL 路径溢出。其他代码若手工构造节点，应自行维持该约束。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。节点通过独占的 `&mut self` 修改 children、schema 与统计缓存，计划树用 `Box<dyn LogicalPlan>` 表示所有权；`TakeChildren` 会把子列表移出当前节点，使 TopN 下推成为显式的所有权重组。

`ContextRef` 是共享上下文引用；`convertToTopN` 克隆该引用，而不是复制会话状态。`StatsInfo` 在读取缓存或写回时被克隆，其中直方图集合自身是共享引用。调用并发安全性由更上层计划生命周期和上下文类型保证，本文件没有提供针对同一可变节点的并发访问机制。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_limit.go`。Rust 保留了 Go 的 `LogicalSchemaProducer`、`PartitionBy`、offset/count、cop 下推偏好、部分 Limit 标志，以及 `Init`、explain、24 字节哈希、谓词屏障、列裁剪、MaxOneRow、TopN 转换、统计推导和分区键访问器。

已经核实的对应点：

- 两端 `HashCode` 都是物理类型、query block、offset、count 的 24 字节大端布局。
- 两端都不把父谓词穿过 Limit；都让子树用空谓词继续处理。
- 两端都在 `Count == 1` 时设置 `MaxOneRow`，并调用 `DeriveLimitStats(child, Count)`。
- 两端 `convertToTopN` 都不复制 `PartitionBy`；Rust 测试 `limit_to_top_n_matches_go_field_mapping` 明确锁定该行为。

可见差异与迁移扩展：

- Rust 增加 `OffsetParam`/`CountParam` 并传播到 TopN/PhysicalLimit；当前 Go 结构没有这两个字段。
- Rust 的 explain 直接调用 `SortItem::String`；Go 通过表达式 eval context 与 `property.ExplainPartitionBy` 格式化。复杂表达式/时区相关格式是否完全一致，当前独立测试未覆盖，不能假定字节级相同。
- Go 的 `PruneColumns`、`PushDownTopN` 和 `DeriveStats` 直接索引唯一子节点；Rust 对缺失子节点作容错处理。合法计划仍应保持一个孩子，这些分支只避免畸形/测试节点立刻越界。
- Go `PruneColumns` 先把自身 schema 置空再内联投影；Rust 先复制裁剪后子 schema，再内联投影。现有测试没有单独证明所有 schema 细节完全等价，扩展时应重点回归。
- Rust 的 `convertToTopN` 复制参数索引但同 Go 一样不复制 `IsPartial`；`IsPartial` 在当前 Rust 生产代码中没有读取者，属于尚未接线的迁移状态。

## 扩展指南

- 新增 Limit 专属语义字段时，先决定它是否影响 `HashCode`、生成的 `Hash64`/`Equals`、`ExplainInfo`、`convertToTopN`、物理计划穷举、计划快照以及 Go 对照；这些入口代表不同用途，不能只改结构体。
- 改变窗口或 TopN 下推规则时，优先修改 `PushDownTopN`/`convertToTopN`，同时检查 `LogicalTopN::AttachChild` 的反向转换，避免字段在 Limit 与 TopN 往返时丢失。尤其要明确 `PartitionBy` 和 `IsPartial` 是否仍应被刻意排除。
- 改变谓词行为时必须维护 Limit 的顺序语义和残差不丢失不变量，并同步 `base_logical_plan.rs` 的 Selection 挂接契约。
- 改变统计时同时核对 `property::DeriveLimitStats`、缓存 reload 语义和物理 child `ExpectedCnt`；offset 是否进入基数估计必须与 Go 版本和代价模型共同决定。
- Rust 单元测试必须放在独立文件。直接扩展 `pkg/planner/core/operator/logicalop/logical_limit_test.rs`；哈希/统计公共契约现由 `logical_d_aster_unit_test.rs` 覆盖。构建入口变化还应更新 `logical_plan_builder_runtime_aster_unit_test.rs`，物理化变化应更新 physicalop/Cascades 对应独立测试。
- 兼容风险集中在 plan hash/cache 命中、prepared LIMIT 参数、explain 输出和 Go/Rust 计划等价性；性能风险集中在 `ExpectedCnt`、统计基数以及能否下推到 Cop/MPP。

## 验证依据

本说明基于以下直接证据：

- 目标实现：`pkg/planner/core/operator/logicalop/logical_limit.rs`（`LogicalLimit` 及其 `LogicalPlan` 实现）。
- crate/模块边界：`pkg/planner/core/operator/logicalop/Cargo.toml`、`pkg/planner/core/operator/logicalop/lib.rs`。
- 公共契约：`base_logical_plan.rs::{LogicalPlan, PredicatePushDownPlan, AttachSelectionToPlan, BaseLogicalPlan}`、`logical_schema_producer.rs`。
- 构建与反向转换：`logical_plan_builder_runtime.rs::build_limit_runtime`、`logical_top_n.rs::LogicalTopN::AttachChild`。
- 统计与物理化：`pkg/planner/property/stats_info.rs::DeriveLimitStats`、`physical_limit.rs::ExhaustPhysicalPlans4LogicalLimit`、`implementation_rules.rs::ImplLimit`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_limit.go`。
- 独立 Rust 测试：`logical_limit_test.rs::{limit_to_top_n_matches_go_field_mapping, logical_plan_hash_code_dispatches_to_limit_semantic_hash}`；`logical_d_aster_unit_test.rs::{limit_hash_covers_offset_count_and_query_block_layout, limit_trait_statistics_respect_count}`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query LogicalLimit --kind struct` 定位 Rust/Go 定义；`node --file ...logical_limit.rs`、`node --file ...logical_limit.go` 和 `node --file ...lib.rs` 核对源码与模块；`explore` 给出 builder、memo、物理计划和测试的使用范围。精确 `callers/callees` 命令在本次环境中超时且没有输出，因此具体构造/消费点又使用 `rg` 和定点文件读取复核，没有把超时结果当作证据。

人工复核结论：该文件存在于逻辑计划层，用窗口语义约束优化器变换，并把必要字段交给 TopN 下推、统计估算和物理 Limit 枚举；安全扩展需要同时维护 trait 接线、双向 Limit/TopN 字段映射、哈希/缓存与独立测试。本文没有把未接线的 `IsPartial` 或未覆盖的 explain/schema 等价性写成已支持事实。
