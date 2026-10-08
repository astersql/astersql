# `pkg/planner/core/operator/physicalop/physical_index_reader.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-core-operator-physicalop`（见同目录 `Cargo.toml`），定义根任务中的物理索引读取算子 `PhysicalIndexReader`。它包装一棵可下推到存储侧的 `IndexPlan`，把索引侧计划的 schema、统计信息、代价、解释文本和 protobuf 编码能力暴露给规划器及执行器。与需要按索引句柄回表的 `PhysicalIndexLookUpReader` 不同，本算子表达覆盖索引或只需索引侧结果的单读路径。

模块入口 `lib.rs` 通过 `mod physical_index_reader` 和 `pub use physical_index_reader::*` 导出本文件，并为该类型实现 `ConcretePhysicalOperator`、再由 `impl_concrete_physical_plan!(PhysicalIndexReader)` 接入统一 `PhysicalPlan` trait。这里有一个重要所有权约定：真实孩子保存在 `IndexPlan`，而不是 `BasePhysicalPlan.Children`；`lib.rs` 的 `children_operator`/`set_children_operator` 专门维持这一约定。

## 核心职责

- 保存索引侧下推树：`IndexPlan: Option<Box<dyn PhysicalPlan>>` 是唯一真实子树，`SetChildren` 取传入列表的第一个元素，并清空基座 children。
- 派生 reader 输出：`SetSchema` 对聚合、流聚合和投影使用下推树根节点 schema；其他形态递归寻找首个 `PhysicalIndexScan`，使用其 `DataSourceSchema`，随后同步 `OutputColumns`。
- 提供物理计划通用操作：克隆、相关列提取、索引解析、解释信息、代价委托、protobuf 编码和内存估算。
- 为优化器构造 reader：`GetPhysicalIndexReader` 注入 context、schema、统计和所需物理属性；具体 `IndexPlan` 通常在后续任务物化阶段挂接。
- 为执行器保留类型化边界：`pkg/executor/builder.rs` 对该类型向下转型，要求 `IndexPlan` 中存在 `PhysicalIndexScan`，并据表、索引和绑定信息构造类型化索引读取执行器。

当前文件还承担一部分迁移兼容逻辑，但不能视为 Go 版本已经完整移植：`LoadTableStats` 仅定位扫描节点，`AccessObject` 仅返回扫描的字符串描述，网络数据量和 cost 的算法也比 Go 路径更通用或更粗略。

## 主要符号

### `PhysicalIndexReader`

公开结构体包含四部分状态：

- `PhysicalSchemaProducer`：持有 `BasePhysicalPlan`、对外 schema、context、plan id/type、统计和所需属性。
- `IndexPlan`：索引侧计划树根；为空时算子尚未完整接线，多数只读查询返回空值或零值，但 `ToPB` 会显式报错。
- `OutputColumns`：reader 返回列的独立克隆，`ResolveIndices` 会相对 `IndexPlan.schema()` 原地解析其下标。
- `PlanPartInfo`：分区计划信息；由 `Clone` 深拷贝并计入内存估算。

### 构造与克隆

- `New(ctx)` 创建类型为 `plancodec::TypeIndexReader`、query block offset 为 0 的空 reader。
- `Init(self, ctx, offset)` 重设 context、类型和查询块偏移，保留其余字段。
- `Clone(&self, new_ctx)` 克隆基座、schema、下推树、输出列和分区信息，并把新 context 传递给整棵子树；任一子计划克隆失败都会通过 `expression::Error` 返回。
- `GetPhysicalIndexReader(ctx, schema, stats, props)` 是优化器使用的便捷工厂，只设置 reader 自身的 schema/统计/属性，不创建 `IndexPlan`。

### schema 与树操作

- `SetSchema` 忽略调用者传入的 `_schema`，只根据已有 `IndexPlan` 重算 schema；无下推树或找不到扫描时保持原状态。
- `SetChildren` 只接受第一个孩子作为 `IndexPlan`，额外孩子被丢弃；随后调用 `SetSchema` 并确保基座 children 为空。
- `find_index_scan` 深度优先返回计划树中的第一个 `PhysicalIndexScan`，供 schema、访问对象和统计加载入口复用。

### 计划接口方法

- `ExtractCorrelatedCols` 通过 `collect_correlated_columns` 遍历整棵下推树，把每个节点的相关列按先根后子的顺序追加；当前不去重。
- `ResolveIndices` 先解析 producer，再解析 `IndexPlan`，最后解析 `OutputColumns`；普通解析失败时尝试 `ResolveIndicesByVirtualExpr` 处理重复虚拟表达式列。
- `ExplainInfo`/`ExplainNormalizedInfo` 分别输出 `index:<explain_id>` 和 `index:<tp>`；空计划时返回空字符串。
- `GetPlanCostVer1`/`GetPlanCostVer2` 委托 `BasePhysicalPlan`；`GetNetDataSize` 用 `stats_count × schema 列数 × 8` 做近似估算。
- `ToPB` 直接委托 `IndexPlan.to_pb`；缺少下推树时返回 `index reader has no index plan`。
- `MemoryUsage` 汇总 producer、整棵下推树、输出列和分区信息。辅助函数 `subtree_memory_usage` 对每个节点调用 `memory_usage()` 后递归累加孩子。

## 执行流程

1. 优化器识别索引 gather。旧 cascades 路径 `pkg/planner/cascades/old/implementation_rules.rs` 和传统路径 `base_physical_plan.rs` 都调用 `GetPhysicalIndexReader`，先创建仅含输出 schema、统计和 child property 的 reader。
2. 任务物化或索引连接探测逻辑挂接索引侧子树。通用路径通过 `PhysicalPlan::set_children` 最终进入本文件 `SetChildren`；`index_join_probe.rs` 的单索引扫描路径也可直接写入 `IndexPlan`。
3. `SetChildren` 保存首个子计划并重算输出。若根是 `PhysicalHashAgg`、`PhysicalStreamAgg` 或 `PhysicalProjection`，reader 暴露根 schema；否则递归找到 `PhysicalIndexScan.DataSourceSchema`。成功后 schema 和 `OutputColumns` 同步克隆。
4. 优化/执行准备阶段可调用 `ResolveIndices`：先解析子计划，再把 reader 的输出列绑定到子计划 schema；虚拟表达式匹配作为普通 unique-id/index 匹配失败后的兼容回退。
5. 计划展示、计费和序列化分别经 explain、cost、`GetNetDataSize`、`ToPB` 等入口访问 `IndexPlan`。`pkg/executor/statement_ru_plan_walk.rs` 还要求该算子位于根部、恰有一个扁平化孩子且 `IndexPlan` 非空，才按 reader 类别统计 RU 扫描字节。
6. 本地类型化执行器构建路径 `pkg/executor/builder.rs` 递归查找 `PhysicalIndexScan`，验证 TableInfo、IndexInfo 和数据绑定后创建索引读取 executor；因此一个非空但不含 scan 的计划仍会在执行器边界报错。

`GetPhysicalIndexReader` 本身不会调用 `SetChildren`，所以“构造 reader”和“reader 可执行”是两个不同阶段；扩展或新调用点不得把工厂返回值直接视为完整计划。

## 数据与状态

reader 的核心不变量是 `IndexPlan` 与基座 children 分离。统一 trait 从 `lib.rs::ConcretePhysicalOperator::children_operator` 把 `IndexPlan` 暴露成单孩子，因此通用树遍历仍可工作；若代码绕开该适配层直接查看 `BasePhysicalPlan.Children`，会错误地认为 reader 没有孩子。

schema 有两层相关状态：`PhysicalSchemaProducer` 保存正式输出 schema，`OutputColumns` 保存可单独解析下标的列副本。`SetSchema` 同时更新二者；直接设置 producer schema（如工厂或某些专用构造路径）则不会自动填充 `OutputColumns`。因此在挂接最终子树后应走 `SetChildren`/`SetSchema`，并在执行前调用 `ResolveIndices`。

`IndexPlan` 使用 `Box<dyn PhysicalPlan>` 独占整棵下推树；克隆时调用动态分派的 `clone_physical(new_ctx)` 生成新树。`PlanPartInfo` 是可选值，克隆通过其 `Clone()` 结果取得独立副本。`OutputColumns` 逐列深拷贝，避免解析下标时污染原计划。

统计状态分为 reader 自身的 `BasePhysicalPlan.stats` 与子计划统计。`GetNetDataSize` 读取子树根的行数和 schema，而工厂设置的是 reader 自身统计；调用者必须确保下推树也具有有效统计，否则网络数据量会退化为不准确结果。

## 依赖与调用关系

直接 crate 依赖由 `Cargo.toml` 声明：`base` 提供 `ContextRef`、`PhysicalPlan`、`PlanContext` 和 protobuf 构建上下文；`expression` 提供 schema/列/相关列及错误类型；`property` 提供统计、任务类型与物理属性；`costusage` 提供 cost 类型；`kv`、`tipb` 是存储类型和下推 executor 编码边界；`plancodec` 提供算子类型常量。

主要上游证据如下：

- `pkg/planner/cascades/old/implementation_rules.rs::OnImplement`：`TiKVSingleGather.IsIndexGather` 时创建 `PhysicalIndexReader` 并包装为 index reader implementation。
- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 中 gather 到物理计划的转换：索引 gather 调用 `GetPhysicalIndexReader`。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs`：索引连接探测构造单扫描 reader，写入过滤后的 `IndexPlan`。
- `pkg/executor/builder.rs`：消费完整 reader 并向下寻找 scan，形成 planner 到 executor 的直接边。
- `pkg/executor/statement_ru_plan_walk.rs`：读取 `IndexPlan` 统计 statement RU 证据。

主要下游调用为：`PhysicalSchemaProducer`/`BasePhysicalPlan` 的 schema、统计、属性、代价与 clone API；`PhysicalPlan` 的 `children`、`schema`、`stats_count`、`resolve_indices`、`to_pb`、`memory_usage` 和 `extract_correlated_cols`；`PhysicalIndexScan` 的 `DataSourceSchema` 与 `AccessObject`；`Column` 的克隆、下标解析和内存估算。

RustCodeGraph 对目标文件报告其被 24 个文件使用，并能定位本文件、Go 对照及 `GetPhysicalIndexReader` 等符号；当前 CLI 的 `explore/callers/callees` 未输出边明细，因此上述边进一步用精确源码搜索和相邻实现核验，而不是从空图结果推断。

## 错误处理与边界

- `Clone`、`ResolveIndices`、cost 与 `ToPB` 使用 `Result<_, expression::Error>` 传播错误，不在 reader 内吞掉子计划错误。
- `ToPB` 对空 `IndexPlan` 明确报错；相比之下 explain、网络大小、相关列、访问对象、统计加载和内存估算对空计划返回空/零或无操作。调用者不能用这些宽容结果证明计划完整。
- `ResolveIndices` 在没有 `IndexPlan` 时解析 producer 后成功返回；有计划时，虚拟表达式回退只在普通列解析失败后运行，且回退失败时保留并返回原始错误。
- `SetChildren` 不验证孩子数量：空列表将清空 `IndexPlan`，多个孩子仅保留第一个。这符合单 reader 单下推树模型，但新的调用点应在上游保证恰好一个孩子，避免静默丢失计划。
- `SetSchema` 对普通根要求树中存在 `PhysicalIndexScan` 且该 scan 有 `DataSourceSchema`；否则不更新现有 schema。聚合/投影根不要求 scan 即可采用根 schema。
- `find_index_scan` 对分支树返回深度优先遇到的第一个 scan。当前 reader 预期是一条索引侧下推树；若未来允许多个 scan，schema/访问对象语义必须先明确，不能沿用“第一个”而不加验证。
- 类型化 executor 另行检查 `IndexPlan`、scan、TableInfo 和 IndexInfo；这些检查不在本文件，因此 planner 阶段可能持有尚不可执行的 reader。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道或锁，也不自行持有网络/磁盘资源。`PhysicalIndexReader` 依靠 Rust 的 `Box`/`Vec`/`Option` 所有权管理计划树、列和分区信息；reader 释放时这些对象随之释放。

`ContextRef` 通常是共享引用语义，`Clone` 把调用者提供的新 context 传给基座和每个子计划，而不是沿用旧 context。计划改写方法接收 `&mut self`，因此本文件没有内部可变性或自行提供的并发写保证；并发共享和同步责任属于更上层的计划生命周期。

`MemoryUsage` 是估算而非资源释放动作。它递归访问每个下推节点；极深计划树会带来递归栈和线性遍历成本。需特别注意：若某个节点自身的 `memory_usage()` 已包含孩子，再使用 `subtree_memory_usage` 会重复计数；新增节点时应核对该 trait 的包内约定。

`LoadTableStats` 当前没有加载资源或缓存，只执行一次 scan 定位并丢弃结果，因此不能依赖它完成 Go 版本的统计预热生命周期。

## 与 Go 版本的对应关系

同路径 `physical_index_reader.go` 是直接语义基准，`Cargo.toml` 的 `[package.metadata.porting]` 也声明 Go package 为 `pkg/planner/core/operator/physicalop`。对应关系如下：

- 结构体字段基本对应，但 Go 同时保存树根 `IndexPlan` 与叶到根扁平表 `IndexPlans`；Rust 只保存树根，并用递归辅助函数实现扫描定位、相关列和内存遍历。
- `SetSchema` 保留了 Go 的核心分支：聚合/投影使用根 schema，普通计划使用索引扫描的 `DataSourceSchema`。Rust 递归寻找 scan，比 Go 的 `IndexPlans[0].(*PhysicalIndexScan)` 更宽容；找不到时静默保留原 schema，而 Go 会因不满足结构假设而 panic。
- `Clone`、`ExtractCorrelatedCols`、explain、`ResolveIndices` 的总体意图一致；Rust 对可选计划做了空值处理，并保留虚拟表达式解析回退。
- Go `LoadTableStats` 会基于 scan 的表和物理表 ID 调用统计加载；Rust 当前只探测 scan，是尚未完成的移植缺口。
- Go `AccessObject` 根据动态分区裁剪开关、表别名与 `PlanPartInfo` 生成分区访问对象；Rust 只委托 `PhysicalIndexScan::AccessObject` 并返回字符串，未覆盖动态分区语义。
- Go cost 调用 `GetPlanCostVer14PhysicalIndexReader`/`GetPlanCostVer24PhysicalIndexReader` 专用算法；Rust 当前委托基座 cost。Go `GetNetDataSize` 用直方图和平均行大小，Rust 固定按每列 8 字节估算，精度和类型敏感性不同。
- Go `MemoryUsage` 的源码对 `IndexPlan.MemoryUsage()` 未把返回值加入 `sum`，随后遍历 `IndexPlans`；Rust 明确递归累计整棵树。两边最终是否等价取决于 Go 扁平列表和各节点 memory 约定，不能仅凭形式声称完全一致。
- Go 工厂接收 `TiKVSingleGather` 并从中取 context/offset；Rust 工厂直接接收 context 且 `New` 默认 offset 为 0。传统 Rust 路径的调用点目前没有把 gather offset 传给工厂；专用路径可通过 `New(...).Init(..., offset)` 保留偏移。

相关 Rust 独立测试 `physical_index_reader_test.rs` 已覆盖普通 index scan 使用 `DataSourceSchema`、projection 保留根 schema，以及虚拟表达式列解析回退。Go 同目录没有 reader 专属测试文件；`physical_utils_test.go` 仅覆盖 reader 的工具分类/包含关系，不能证明上述 Go 方法的全部行为。

## 扩展指南

- 新增 reader 字段时，应同步修改 `New`、`Clone`、`MemoryUsage`，并检查 `cache_snapshot.rs` 中 `PhysicalIndexReader` 的 capture/restore 状态，否则计划缓存恢复会丢字段。
- 改变孩子模型时，应同时修改本文件 `SetChildren`、`lib.rs` 的 `ConcretePhysicalOperator` 实现、扁平计划构建、RU 遍历和 executor builder；不要只改 `BasePhysicalPlan.Children`。
- 扩展可接受的下推树根类型或 schema 规则时，优先修改 `SetSchema`/`find_index_scan`，并在独立文件 `physical_index_reader_test.rs` 增加根类型、缺失 scan、分支树和输出列同步测试。测试逻辑不得内嵌回生产源文件。
- 完成 Go 对齐时，`LoadTableStats`、动态分区 `AccessObject`、专用 cost 和按真实平均行宽计算的 `GetNetDataSize` 应分别接入对应 Rust 子系统；这些是不同职责，不宜用一个粗略实现互相替代。
- 修改 `ResolveIndices` 时必须保留虚拟表达式重复匹配回退，并测试普通成功、虚拟表达式成功和两者均失败三条路径。
- 若新增多个孩子或多个 scan 的能力，应先定义输出 schema、访问对象、protobuf 和内存计数的组合规则，并把 `SetChildren` 当前静默忽略额外孩子的行为改为显式处理或报错。
- 影响用户可见执行行为的改动除本文件单元测试外，还应检查 `pkg/executor/typed_index_reader_test.rs`；涉及 optimizer 物化时检查 cascades/传统 planner 的相关测试，涉及 plan cache 时检查 `cache_snapshot_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/planner/core/operator/physicalop/physical_index_reader.rs --offset 1 --limit 500` 返回目标文件完整 286 行，并报告 24 个使用文件；`query PhysicalIndexReader --limit 20` 定位 Rust/Go 类型、工厂及 Go 方法。`explore`、`callers`、`callees` 在本次 CLI 运行中未返回边明细，调用关系改由下列源码搜索核验。
- 目标源码：`pkg/planner/core/operator/physicalop/physical_index_reader.rs`，核对结构体、12 个公开方法、3 个私有递归辅助函数和工厂函数。
- crate 与模块：`pkg/planner/core/operator/physicalop/Cargo.toml`、`pkg/planner/core/operator/physicalop/lib.rs`，核对依赖、Go porting 元数据、导出及 `PhysicalPlan` 适配。
- 上下游源码：`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`、`pkg/planner/core/operator/physicalop/index_join_probe.rs`、`pkg/executor/builder.rs`、`pkg/executor/statement_ru_plan_walk.rs`。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_index_reader.go`；Go 辅助测试证据来自 `pkg/planner/core/operator/physicalop/physical_utils_test.go`。
- Rust 测试：`pkg/planner/core/operator/physicalop/physical_index_reader_test.rs`；补充接线测试位置包括 `pkg/executor/typed_index_reader_test.rs`、`cache_snapshot_test.rs` 和 `physical_projection_test.rs`。
- 包内未找到 `doc.go`，因此没有额外包契约文件可读；本说明以 crate/module 声明、实现和测试为准。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工核对唯一生产物、源码引用、Go 差异和无运行时文件修改。
