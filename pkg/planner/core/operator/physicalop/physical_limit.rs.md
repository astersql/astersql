# `pkg/planner/core/operator/physicalop/physical_limit.rs`

## 文件定位

本文件实现规划器中的物理 `Limit` 算子。它位于 `astersql-planner-core-operator-physicalop` crate；该 crate 由同目录 `Cargo.toml` 定义，以 `lib.rs` 为库入口，并通过 `mod physical_limit` 和 `pub use physical_limit::*` 导出这里的符号。`lib.rs` 中的 `direct_operator_core!(PhysicalLimit, PhysicalSchemaProducer)` 再把本文件的方法接到统一的 `ConcretePhysicalOperator`/`PhysicalPlan` 接口上。

在优化主链中，`base_physical_plan.rs` 的规范路由把 `logicalop::LogicalLimit` 分派给 `ExhaustPhysicalPlans4LogicalLimit`。本文件负责把逻辑层的行数约束变成候选物理计划、维护 Limit 自身的元数据，并在需要下推时编码为 `tipb::Executor`；真正逐行跳过 `OFFSET`、停止于 `COUNT` 的执行发生在后续执行器或存储端，不在本文件中。

## 核心职责

- `PhysicalLimit` 保存 `Offset`、`Count`、分区键、参数标记和前缀截断元数据，并复用 `PhysicalSchemaProducer` 保存上下文、统计、Schema、孩子和孩子所需物理属性。
- `ExhaustPhysicalPlans4LogicalLimit` 在所需属性不含排序项时，为 Cop 单读、Cop 多读、Root，以及条件允许时的 MPP 枚举候选。若上层仍要求排序，裸 Limit 不能产生该顺序，函数返回空候选，交由 TopN 等有序算子处理。
- `ResolveIndices` 把分区列、输出 Schema 和可选前缀列绑定到第一个孩子的列下标；`ExplainInfo`/`ExplainNormalizedInfo` 形成可展示或可归一化的计划文本；`Clone`、`MemoryUsage` 和两套代价入口支持通用计划生命周期。
- `ToPB` 生成 `tipb::Limit`。TiFlash 的 PB 需要内嵌孩子和 executor ID；TiKV 的表达式为空时不必取得 PB client。

这里的“分区 Limit”由 `PartitionBy` 携带按组取前 N 行的键；前缀优化由 `PrefixCol` 生成 `truncate_key_expr`，`PrefixLen` 用于计划解释、克隆、缓存和内存核算。本文件本身不读取 `PrefixLen` 来改变 PB 字段，实际长度语义由周边优化/执行链使用。

## 主要符号

- `pub struct PhysicalLimit`：唯一的生产类型。`PhysicalSchemaProducer` 是通用物理计划载体；`Offset`/`Count` 是当前计划形态的截断值；`OffsetParam`/`CountParam` 保存 prepared statement 参数位置；`CountIncludesOffset` 标识 cop 侧副本的 `Count` 已经是根节点 `OFFSET + COUNT`；`PartitionBy`、`PrefixCol`、`PrefixLen` 保存分区和部分有序优化信息。
- `PhysicalLimit::New(ctx, offset, count)`：构造 `TypeLimit` 节点并给所有可选字段设置空值。它尚未安装统计、查询块偏移、Schema、孩子或孩子属性。
- `PhysicalLimit::Init(ctx, stats, qb_offset, props)`：重建 `BasePhysicalPlan`，安装统计及孩子所需属性。候选枚举通常先 `New`、补充 Limit 专有字段和 Schema，再调用 `Init`。
- `GetPartitionBy`、`ExtractCorrelatedCols`：分别借出分区键切片和报告空关联列集合。后者表达的是 Limit 自身没有可提取的关联表达式，不代表孩子没有关联列。
- `Clone(new_ctx)`：在新计划上下文中克隆基类和 Schema，逐项克隆 `PartitionBy`，并克隆 `PrefixCol`；所有标量及参数标记保持不变。
- `ExplainInfo`、`ExplainNormalizedInfo`：前者遵循会话 redact 策略输出分区、offset/count 和可选前缀信息；后者归一化分区表达式并固定输出 `offset:?, count:?`，不输出前缀字段。
- `ResolveIndices`：先处理通用 Schema producer，再解析专有列；重复输出列采用输出 Schema 与孩子 Schema 单调同步扫描，而不是按 `UniqueID` 每次从头查找。
- `Attach2Task`：委托 `base::PhysicalPlan::attach_to_task` 把一元算子挂到任务树。
- `GetPlanCostVer1`、`GetPlanCostVer2`：直接委托 `BasePhysicalPlan`，本文件没有额外的 Limit 成本公式。
- `ToPB`：序列化 `Count`、`PartitionBy` 和可选 `PrefixCol`；TiFlash 额外序列化第一个孩子并设置当前节点的 explain ID。
- `MemoryUsage`：在 `PhysicalSchemaProducer` 用量之上计入两个 `u64`、一个 `usize`、前缀列指针槽及可选列内容。它刻意与 Go 的既有核算口径对齐，不是 Rust 结构体所有字段的完整堆栈占用测量。
- `ExhaustPhysicalPlans4LogicalLimit(logical, property)`：本文件的逻辑到物理转换入口，返回 `Vec<Box<dyn PhysicalPlan>>`。

## 执行流程

1. `base_physical_plan.rs` 的路由识别 `LogicalLimit`，调用 `ExhaustPhysicalPlans4LogicalLimit`。
2. 枚举入口首先拒绝带 `SortItems` 的所需属性，并要求逻辑节点存在 `SCtx`。它结合 `ShouldCheckTiFlashPushDown`、子树的 TiFlash 标记和 `IsMPPAllowed` 判断 MPP 是否可选；当调用方明确要求 MPP 而 MPP 不可用时直接返回空候选。
3. 对每个允许的任务类型，入口创建孩子 `PhysicalProperty`：`ExpectedCnt` 为 `Offset.wrapping_add(Count)`，并传递 `CTEProducerStatus`、`NoCopPushDown`。之后经 `AdmitIndexJoinTypes` 过滤，构造 `PhysicalLimit`，复制参数标记、分区键、逻辑 Schema、统计和查询块偏移。
4. 通用优化器随后为候选选择并挂接孩子。`Attach2Task` 走公共一元物理计划逻辑；`canonical_router_aster_unit_test.rs` 证明 `LogicalTableDual -> LogicalLimit` 能形成 `PhysicalLimit -> PhysicalTableDual` 且成本有限。
5. 执行前的索引解析调用 `ResolveIndices`。它先解析通用 producer，然后用孩子 Schema 解析 `PartitionBy`；输出 Schema 与孩子 Schema 按顺序匹配相同 `UniqueID`，确保两个重复输出列分别绑定到不同孩子槽位；最后解析 `PrefixCol`。
6. 计划下推时 `ToPB` 写入 `Count`。每个分区键经 `SortByItemToPB` 转换；前缀列经 `ExpressionsToPBList` 写入 truncate-key 表达式。TiFlash 分支还要求一个孩子，将整个孩子执行器嵌入 Limit PB，并设置 executor ID；其他存储类型不内嵌孩子且 ID 为空。
7. 相邻 `base_physical_plan.rs::push_operator_through_projection` 在复制 Limit 到 reader 内侧时，将副本改成 `Offset = 0`、`Count = saturating_add(Count, Offset)`，并设置 `CountIncludesOffset = true`。因此存储侧最多取足根节点完成 offset 后仍需的行；根侧 Limit 是否保留由 reader 形态决定。
8. 另一入口 `physical_topn.rs::getPhysLimits` 在孩子已经能提供 ORDER BY 时，以同样的属性和参数标记构造 Limit，从而避免额外 TopN 排序。

## 数据与状态

`PhysicalLimit` 是可变的计划节点，不拥有运行时结果集。结构中的核心不变量如下：

- `Offset` 表示要跳过的行数，`Count` 表示当前节点最多接收/返回的行数；普通根节点保留二者，cop 侧派生副本则将 offset 折入 count 并用 `CountIncludesOffset` 记录形态。
- `OffsetParam`、`CountParam` 不是数值本身，而是缓存计划重新绑定参数的索引。`cache_snapshot.rs::CachedLimit::restore` 从上下文读取新绑定值：普通形态限制 `count <= u64::MAX - offset`，cop 派生形态用饱和加法重算 `offset + count`。
- 孩子需求的 `ExpectedCnt` 使用 `wrapping_add`，而下推复制与缓存恢复使用 `saturating_add`。这是当前源码的真实差异；扩展行数算术时不能假定所有路径采用同一种溢出策略。
- `PartitionBy` 和 `PrefixCol` 包含列对象，克隆和缓存恢复时必须按表达式上下文重建，不能只复制外层容器。
- `ResolveIndices` 只查看第一个孩子，符合一元 Limit 的结构。如果没有孩子，它在通用 producer 解析成功后返回 `Ok(())`；TiFlash PB 序列化则明确要求孩子存在。
- `PhysicalSchemaProducer` 持有 Schema、上下文、统计和孩子关系。`New` 与 `Init` 都会创建基类，因此调用者应按现有的“构造、补字段、设 Schema、Init”顺序避免丢失基类状态。

## 依赖与调用关系

上游接线包括：

- `base_physical_plan.rs` 的 `route!(logicalop::LogicalLimit, crate::ExhaustPhysicalPlans4LogicalLimit)` 是规范逻辑转物理入口。
- `physical_topn.rs::getPhysLimits` 在排序属性已由孩子满足时直接构造 `PhysicalLimit`。
- `base_physical_plan.rs::push_operator_through_projection` 克隆并改写 Limit，尝试将它推入 IndexLookUp/Table/Index reader。
- `cache_snapshot.rs::CachedLimit` 捕获和恢复所有 Limit 专有状态；`lib.rs` 的 `direct_operator_core!` 把本文件方法暴露给动态 `PhysicalPlan` 调用。

主要下游依赖为：`base`（计划、任务、PB 上下文）、`property`（统计、任务类型、排序与孩子属性）、`logicalop`（`LogicalLimit`）、`expression`（列解析、脱敏常量及 PB 表达式转换）、`planner_util`（TiFlash 下推判断）、`kv`（存储类型）、`tipb`（下推协议）、`plancodec`（节点类型）和 `costusage`（成本模型）。这些均在同目录 `Cargo.toml` 作为路径依赖或固定 revision 的 `tipb` 依赖声明；该 manifest 没有为本文件设置条件 feature。

RustCodeGraph 的文件节点显示本文件被执行器计划运行、MPP、RU 遍历及同 crate 测试等多处文件使用。具体方法多数经 trait/宏动态分派，因此索引的 `callers`/`callees` 命令未返回可用边；上述调用关系以代码图的文件使用信息、精确符号搜索和模块接线源码共同核验。

## 错误处理与边界

- `Clone`、`ResolveIndices`、两套成本方法和 `ToPB` 使用 `expression::Error` 向上传播失败，不吞掉下游错误。
- `ResolveIndices` 在输出 Schema 仍有列无法按顺序映射到孩子时，返回包含当前 explain ID 的错误；分区列和前缀列解析失败也直接传播。没有孩子时它不报错，因此“可构造空壳”与“可编码 TiFlash 子树”是两个不同边界。
- `ToPB` 仅当需要转换 `PartitionBy` 或 `PrefixCol` 时要求 PB client；缺少 client 返回 `PB client is required`。分区表达式不能下推时返回 `partition expression cannot be pushed down`。这避免把失败的表达式静默写成不完整 PB。
- TiFlash 分支没有孩子时返回 `limit requires one child`；孩子 `to_pb` 的错误继续上抛。TiKV 且无表达式的分支不访问 client，`physical_limit_test.rs` 对此有独立回归测试。
- 候选枚举对排序需求、缺失上下文和不可用的强制 MPP 都以空候选表示，而不是制造一个不能满足属性的计划。
- `ExplainInfo` 对 redact enable 输出问号，对 marker 模式使用标记包裹具体值，对其他模式输出明文；可选前缀信息遵循同一策略。新增可识别值时必须同步三条分支，避免日志泄漏。
- 本文件没有验证 `PrefixLen` 与列类型/索引定义是否一致，也不校验 `Offset + Count` 的业务上限；这些前置约束应由产生该优化元数据的规划路径负责。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。计划构建阶段通过拥有的 `Vec`、`Option<Column>` 和 `Box<dyn PhysicalPlan>` 管理数据；共享的计划上下文使用 `ContextRef`，克隆节点时显式换成 `new_ctx`。

`Clone` 对可变表达式状态做深克隆，避免不同计划候选共享并随后修改同一个 `PartitionBy`/`PrefixCol`。Schema 也经 `Schema::Clone` 安装到新 producer。`ToPB` 只在调用期间借用可变 `BuildPBContext`，生成的 protobuf 拥有自己的字段；它不会发送 KV 请求。孩子的所有权和资源生命周期由通用 `PhysicalPlan`/`Task` 树负责。

唯一与并发相关的直接测试细节是 `physical_limit_test.rs` 的测试上下文用原子计数器分配计划 ID；这是测试替身的线程安全实现，不表示 `PhysicalLimit` 自身执行并发控制。

## 与 Go 版本的对应关系

直接对照文件是同目录 `physical_limit.go`。两版都定义 `PhysicalLimit`，保留 `PhysicalSchemaProducer`、`PartitionBy`、`Offset`、`Count`、`PrefixCol`、`PrefixLen`，并覆盖候选枚举、初始化、分区键访问、克隆、内存、Explain、PB、索引解析和任务挂接。

关键对齐点：

- 两版在有排序需求时拒绝裸 Limit，并为孩子要求 `offset + count` 行；都枚举 Cop 单读、Cop 多读、Root，且仅在 TiFlash 条件和会话 MPP 开关允许时加入 MPP。
- 两版 PB 都只把 `Count` 写入 `tipb::Limit`，序列化分区键及可选 truncate-key 表达式，并在 TiFlash 下内嵌孩子、设置 explain ID。
- Explain 的三种脱敏形态、克隆分区/前缀列、以及内存中两个 `uint64`、前缀指针、前缀长度和可选列内容的口径保持一致。
- Go 的 `ResolveIndices` 和 `Attach2Task` 分别委托 `utilfuncp.ResolveIndices4PhysicalLimit`、`Attach2Task4PhysicalLimit`；Rust 将索引解析写在本文件中，把挂接委托给 `base::PhysicalPlan::attach_to_task`。实现位置不同，但目标语义一致。

当前 Rust 还显式保存 `OffsetParam`、`CountParam`、`CountIncludesOffset`，用于参数化计划缓存和 cop 下推形态；这些字段不在当前同路径 Go 结构体中。Rust 的 PB 转换也把缺 client、分区表达式不可下推、TiFlash 缺孩子变成明确错误，而 Go 代码更依赖调用前置条件。Rust `ExhaustPhysicalPlans4LogicalLimit` 在 required task 明确为不可用 MPP 时直接拒绝，并经过 `AdmitIndexJoinTypes`；这些是 Rust 周边路由接线的显式约束，不能仅凭 Go 文件省略。

## 扩展指南

- 新增 Limit 状态字段时，至少同步 `New`、`Clone`、必要的 Explain/PB/`MemoryUsage`，并检查 `cache_snapshot.rs::CachedLimit` 的 capture/restore。若字段来自逻辑节点，还要同步 `ExhaustPhysicalPlans4LogicalLimit` 与 `physical_topn.rs::getPhysLimits`。
- 改动任务枚举或下推条件时，优先修改 `ExhaustPhysicalPlans4LogicalLimit`，同时核对 `base_physical_plan.rs` 的规范路由、reader 下推逻辑和 `AdmitIndexJoinTypes` 过滤；保持 task 枚举顺序，避免改变候选选择。
- 改动列语义时，在 `ResolveIndices` 同时处理 `PartitionBy`、输出 Schema 和 `PrefixCol`。不要把重复列匹配简化成每列独立查找；`physical_limit_test.rs::resolve_indices_keeps_duplicate_inline_projection_columns_distinct` 固化了顺序匹配的不变量。
- 改动 PB 时分别测试 TiKV 与 TiFlash：前者应覆盖有/无表达式和 client，后者应覆盖孩子嵌入、executor ID 与孩子错误。相关入口测试应继续放在独立的 `physical_limit_test.rs`，不要内嵌到生产文件。
- 改动参数化 LIMIT 或下推计数时，同步检查 `CountIncludesOffset`、缓存重新绑定和三种加法策略（wrapping、saturating、上限裁剪），避免重复加 offset 或溢出。
- 改动用户可见 Explain 字段时同步普通、marker、enable 和 normalized 输出，并评估日志兼容性及敏感值暴露风险。
- 性能风险主要在候选数量增加、表达式深克隆、列解析扫描及 PB 表达式转换；兼容风险主要在 tipb 字段、Explain 文本、prepared plan cache 和 Go/Rust 计划形态差异。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、目标文件可由 `node --file ... --offset 1 --limit 500` 完整读取；`query PhysicalLimit --kind struct` 和 `query ExhaustPhysicalPlans4LogicalLimit --kind function` 同时定位到 Rust/Go 定义。`callers`/`callees` 对目标 Rust 符号未输出调用边，未把该空结果当作“无调用者”，而以模块接线与精确搜索补证。
- 生产源码：`physical_limit.rs`；直接接线和状态消费者：`lib.rs`、`base_physical_plan.rs`、`physical_topn.rs`、`cache_snapshot.rs`；crate 边界：同目录 `Cargo.toml`。
- Go 对照：同目录 `physical_limit.go`。其结构、候选枚举、Explain、PB、克隆、内存及委托入口用于核对移植语义。
- 独立 Rust 测试：`physical_limit_test.rs` 覆盖 TiFlash executor ID、无表达式 TiKV 不需要 client、重复输出列索引和内存口径；`unary_aster_unit_test.rs` 覆盖 trait 与克隆状态；`canonical_router_aster_unit_test.rs` 覆盖逻辑路由、挂接和成本；`cache_snapshot_test.rs` 覆盖 Limit 状态随整棵计划缓存往返。
- 人工边界复核：目标文件没有条件编译项、模块级常量、独立 trait 或自有并发原语；公开生产符号为 `PhysicalLimit`、其固有方法和 `ExhaustPhysicalPlans4LogicalLimit`。文档区分了规划/序列化职责与实际执行职责，并明确记录 Rust 相对 Go 的附加缓存字段及错误边界。
