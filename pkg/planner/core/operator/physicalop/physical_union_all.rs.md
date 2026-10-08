# `pkg/planner/core/operator/physicalop/physical_union_all.rs`

## 文件定位

本文件是 `astersql-planner-core-operator-physicalop` crate 中 `UNION ALL` 物理算子的 Rust 实现，源码入口为 [`physical_union_all.rs`](physical_union_all.rs)。它处在逻辑计划和任务选择之间：`ExhaustPhysicalPlans4LogicalUnionAll`、`ExhaustPhysicalPlans4LogicalPartitionUnionAll` 将逻辑 Union 节点转换为 Root/MPP 物理候选，随后通用 `PhysicalPlan` 接口负责成本比较和任务挂接。

模块由 [`lib.rs`](lib.rs) 的 `mod physical_union_all` 纳入并通过 `pub use physical_union_all::*` 导出；同文件中的 `ConcretePhysicalOperator for PhysicalUnionAll` 和 `impl_concrete_physical_plan!(PhysicalUnionAll)` 将这里的固有方法接入统一物理计划接口。crate 边界及直接依赖由 [`Cargo.toml`](Cargo.toml) 声明，其中本文件直接依赖 `base`、`costusage`、`expression`、`logicalop`、`property`、`plancodec` 和 `vardef`。

## 核心职责

- 用 `PhysicalUnionAll` 保存通用 schema/基础计划状态以及 `Mpp` 执行标记；该算子只拼接各子计划输出，不执行去重。
- 用 `build_union_plans` 检查所需物理属性是否可由 Union All 满足，并为每个逻辑子节点生成对应的子属性。
- 在 MPP 被允许时枚举分布式候选：MPP 需求生成 MPP 主候选，Root 需求除 Root 主候选外再生成一个 MPP 子树候选，供后续 Gather/任务选择使用。
- 提供克隆、内存估算、v1/v2 成本计算和任务挂接入口，并让普通 Union 与分区 Union 复用同一候选构造流程，仅以计划类型码区分。

## 主要符号

- `pub struct PhysicalUnionAll`：包含 `PhysicalSchemaProducer` 与布尔字段 `Mpp`。前者承载 `BasePhysicalPlan`、schema、统计信息、子计划及子属性；后者决定候选是否按 MPP Union 处理。
- `PhysicalUnionAll::New(ctx)`：以 `plancodec::TypeUnion` 和 offset `0` 创建空节点，初始 `Mpp = false`。实际候选还需经过 `Init` 写入完整状态。
- `PhysicalUnionAll::Init(ctx, stats, offset, props)`：重建基础计划，写入统计信息和每个孩子的 required property；它不设置 schema，调用方必须单独执行 `SetSchema`。
- `PhysicalUnionAll::Clone(new_ctx)`：用新上下文克隆基础计划，并在 schema 存在时调用 schema 的 `Clone`；保留原 `Mpp` 标记，失败以 `expression::Error` 返回。
- `MemoryUsage`：返回 schema producer 的估算值加一个 `bool` 的大小；与 Go 不同，Rust 接收 `&self`，不存在 nil receiver 分支。
- `Attach2Task(tasks)`：调用 `base::PhysicalPlan::attach_to_task` 进入通用 trait 路由。由 [`lib.rs`](lib.rs) 的宏实现可知，`PhysicalUnionAll` 未覆盖 `attach_operator_to_task`，因此采用默认逻辑：克隆子任务计划和自身，设置孩子，再构造携带首个子任务 MPP 分区元数据的 `RootTask::NewWithMpp`。
- `GetPlanCostVer1`：若已有缓存且未要求重算则复用缓存；否则取所有子节点 v1 成本最大值，加上 `(1 + 子节点数) * TiDBOptConcurrencyFactor` 的 worker 开销并缓存。MPP 强制模式且非重算时再除以 `1_000_000_000`，使强制 MPP 候选具有压倒性的选择优先级。
- `GetPlanCostVer2`：取得基础计划聚合后的 v2 成本，按正数 `TiDBExecutorConcurrency`（非法、非正或缺失则用默认值）做除法；MPP 强制模式且非重算时同样再除以 `1_000_000_000`。此方法自身不写本文件的 v1 缓存字段。
- `ExhaustPhysicalPlans4LogicalUnionAll`：普通逻辑 Union 的公开枚举入口；调用 `build_union_plans(..., TypeUnion)` 并装箱为 `dyn PhysicalPlan`。
- `build_union_plans`：本文件的私有核心构造器，统一处理属性拒绝、上下文、统计缩放、子属性、schema、类型码及 Root/MPP 候选数。
- `ExhaustPhysicalPlans4LogicalPartitionUnionAll`：分区 Union 的公开入口，调用同一构造器但传入 `TypePartitionUnion`。

## 执行流程

1. 上游优化器以逻辑 Union 和 `PhysicalProperty` 调用两个公开 `Exhaust...` 入口之一；Go 主链中的对应入口位于 `pkg/planner/core/exhaust_physical_plans.go` 和 cascades memo，Rust 图索引则确认两个 Rust 入口均下调 `build_union_plans`。
2. `build_union_plans` 先拒绝三类无法直接满足的属性：非空排序项；Flash 属性但任务不是 MPP；任务是 MPP 但分区类型不是 `AnyType`。拒绝以空候选集表达，而非错误。
3. 函数从逻辑节点取得 `ContextRef`；若上下文缺失同样返回空候选集。随后读取 session 的 `IsMPPAllowed`。
4. 闭包 `child_properties(task_type)` 为每个逻辑孩子创建一个默认 `PhysicalProperty`，只显式传播 `ExpectedCnt`、指定的 `TaskTp`、`CTEProducerStatus` 与 `NoCopPushDown`。排序、MPP 分区键等其余字段保持默认值。
5. 逻辑统计存在时按 `ExpectedCnt` 缩放，否则使用默认统计。主候选的 task type 仅在“Mpp 被允许且调用者明确要求 MPP”时取 `MppTaskType`，其余情况取 `RootTaskType`。
6. 构造主节点，复制逻辑 schema，调用 `Init` 写入统计、query block offset 和各子属性，再以传入的 `plan_type` 覆盖类型码；因此分区 Union 与普通 Union 共享行为但 explain/编码身份不同。
7. 若 MPP 可用且调用者要求 Root，再追加一个 `Mpp = true`、所有子属性均为 MPP 的备选节点。最终普通场景返回一个候选，此分支返回两个；各入口再将具体类型装箱为 `Box<dyn PhysicalPlan>`。
8. 后续成本搜索通过 `ConcretePhysicalOperator` 路由到 `GetPlanCostVer1/2`；选择候选后，`Attach2Task` 进入宏生成的 `PhysicalPlan` 实现，再调用默认 `attach_operator_to_task`：克隆各子任务中的物理计划、克隆 Union 自身并设置孩子，最后返回 `RootTask::NewWithMpp`。MPP 分区类型和 hash columns 取自第一个子任务；无子任务时使用 `AnyType` 与空列。

## 数据与状态

- 持久节点状态集中在 `PhysicalSchemaProducer`：其中的 `BasePhysicalPlan` 保存上下文、类型码、ID/offset、统计、孩子、孩子所需属性及成本缓存；producer 还保存输出 schema。`Mpp` 是本文件新增的唯一专有状态。
- `New` 只建立初值，`Init` 会替换内部 `BasePhysicalPlan`。安全构造顺序是：`New` → 设置 `Mpp`/schema → `Init` → 设置最终类型码；当前 `build_union_plans` 正是这一顺序。
- v1 成本使用 `PlanCost` 与 `PlanCostInit` 缓存。带 `COST_FLAG_RECALCULATE` 时跳过缓存读取，并且不应用强制 MPP 的十亿分之一偏置，但新结果仍写回 `PlanCost` 并将 `PlanCostInit` 设为真。
- 子成本按最大值而非求和聚合，表达 Union 多路 worker 并行时关键路径由最慢孩子决定；固定 worker 开销随子节点数线性增长。
- v2 使用结构化 `CostVer2` 和 `div_cost_ver2`，先按 executor concurrency 摊薄基础计划成本；配置解析失败或值不大于零时保证回退默认并避免除零。
- [`cache_snapshot.rs`](cache_snapshot.rs) 的 `CachedUnionAll` 同时捕获/恢复 producer 和 `Mpp`，说明计划缓存快照不能遗漏该标记。

## 依赖与调用关系

- 上游：逻辑节点类型来自 `logicalop::LogicalUnionAll`/`LogicalPartitionUnionAll`；公开枚举入口是这两种节点到 `PhysicalUnionAll` 的转换边。RustCodeGraph 对精确函数的 `callers` 当前没有返回静态调用者，因此上游接线以导出关系和 Go 对照入口为证据，不臆测未被索引到的 Rust 调用点。
- 内部边：两个 `Exhaust...` → `build_union_plans` → `PhysicalUnionAll::New`/`Init`；`Clone` → `BasePhysicalPlan::CloneWithNewCtx`；v1/v2 成本 → 子/基础计划成本与 session variables；v2 → `costusage::div_cost_ver2`。
- trait 接线：[`lib.rs`](lib.rs) 的 `ConcretePhysicalOperator` 把 producer、索引解析、内存及成本方法映射到统一接口，宏实现再使节点可作为 `dyn PhysicalPlan` 使用。
- 下游任务：[`lib.rs`](lib.rs) 的 `impl_concrete_physical_plan!` 把 `attach_to_task` 映射到 `attach_operator_to_task`；本类型未覆盖后者，所以采用 trait 默认的 RootTask 包装逻辑。`pkg/planner/core/task.rs` 另有基于 `PlanNode` 的 Union 路由，但当前 `Attach2Task` 没有直接调用它，不能把两条路径混同。
- 下游执行：Go 版本最终由 `pkg/executor/builder.go::buildUnionAll` 构造执行器。当前 Rust 文件本身只描述和选择物理计划，不读取行、不管理执行器 worker。
- crate：[`Cargo.toml`](Cargo.toml) 将 `logicalop`、`property`、`base` 等都作为同一 workspace 的路径依赖，并将 Rust 独立测试所需的 `exprstatic` 声明为 dev-dependency；没有由本文件控制的 feature gate 或条件编译项。

## 错误处理与边界

- 属性不兼容或逻辑节点缺少上下文时，候选枚举返回空 `Vec`，不产生 `Result` 错误。调用方必须把“无候选”当作规划不可行分支，而不能当作成功生成计划。
- 排序要求当前一律不支持；源码注释明确保留未来 sort-merge Union 的方向。MPP 候选也只接受 `AnyType` 分区要求，当前不会透传指定分区信息。
- `Clone` 和两个成本方法会传播底层的 `expression::Error`；本文件不吞掉这类错误。session 变量的字符串解析失败则不是硬错误，而是回退默认并继续估算。
- `Init` 不验证 `props` 数量是否等于孩子数量；`build_union_plans` 通过对 `logical.Children()` 一一映射维持该不变量。外部直接调用 `Init` 时需自行保证对应关系。
- `Attach2Task` 的返回类型不是 `Result`。当前默认挂接会对计划克隆失败执行 `expect`，因此会 panic；空任务不会报 invalid task，而是建立无孩子、MPP 分区元数据为 `AnyType` 的 RootTask。调用者应维持 Union 至少有一个孩子的逻辑不变量。
- `MemoryUsage` 不包含 Rust 容器容量、共享对象所有权等所有深层分配的完整精确值，而是沿用各嵌套类型的估算契约。

## 并发与资源生命周期

- 本文件没有锁、原子变量、线程、异步任务、channel、事务或 I/O；物理计划构造与成本计算是同步的规划期操作。
- `ContextRef` 是共享上下文引用；`New`/`Init`/`Clone` 保存或替换该引用，但本文件不负责 session 生命周期。`Clone(new_ctx)` 明确把克隆计划绑定到调用者提供的新上下文。
- schema 与统计在候选间通过各自的 `Clone`/值克隆共享或复制其内部表示；Root 与 MPP 备选拥有独立的计划节点和子属性集合，因此后续修改一个候选的基础状态不应直接修改另一个候选。
- 真正的 Union 执行并发属于执行器层。规划成本中的 `TiDBOptConcurrencyFactor`/`TiDBExecutorConcurrency` 只是模型参数，不代表本文件启动 worker。
- 计划缓存生命周期由 `CachedUnionAll` 的 capture/restore 边界覆盖；扩充 `PhysicalUnionAll` 状态时必须同步快照表示，否则缓存恢复会丢字段。

## 与 Go 版本的对应关系

直接对照文件为 [`physical_union_all.go`](physical_union_all.go)。结构体字段、`Init`/`Clone`/`MemoryUsage`/`Attach2Task`/两套成本入口和两个 `Exhaust...` 名称均保持可辨认对应，候选过滤条件、四个子属性字段、统计缩放、Root 下追加 MPP 方案以及 `TypePartitionUnion` 语义也一致。

实现组织存在以下差异：

- Go 的 `Attach2Task` 与成本方法通过 `utilfuncp` 函数指针进入 `pkg/planner/core`；Rust 在算子内实现成本逻辑，并通过 `lib.rs` 中的 trait 默认挂接构造 `RootTask`。
- Go 的分区入口先调用普通入口，再遍历候选改类型码；Rust 用私有 `build_union_plans` 接受 `plan_type`，构造时直接写入相应类型码。
- Go `Exhaust...` 返回 `([]PhysicalPlan, hintCanWork, error)`；Rust 返回候选向量，未在该签名中承载 hint 布尔值或错误。属性不满足和缺上下文都体现为空向量。
- Go v1/v2 的细节分别位于 `pkg/planner/core/plan_cost_ver1.go`、`plan_cost_ver2.go`；Rust 已把本算子成本移入本文件。独立 Rust 测试 [`physical_union_all_test.rs`](physical_union_all_test.rs) 以 0/2 个孩子验证 v1 保留 Go 的 worker 开销公式。
- Rust `MemoryUsage(&self)` 无 Go nil receiver 的零值语义；调用前必须已有有效引用。

这些差异是当前代码事实，不意味着 Go 主执行路径已被此单文件替代；本文件只负责 Rust 物理算子侧能力。

## 扩展指南

- 若让 Union 保序，应从 `build_union_plans` 的排序属性拒绝分支开始，定义孩子的排序 required property，并同步任务挂接/执行器是否真能维持顺序；不能只删除 `IsSortItemEmpty` 判断。
- 若支持指定 MPP 分区，应修改 `build_union_plans` 对 `MPPPartitionTp` 的限制和 `child_properties` 的传播字段，并验证 Exchange/Gather 接线；错误透传或分区键遗漏会产生错误的分布式结果或额外 shuffle。
- 若调整成本，优先修改 `GetPlanCostVer1/2`，保留重算标志、默认并发回退和 MPP enforced 偏置的契约；同步扩展 [`physical_union_all_test.rs`](physical_union_all_test.rs)，测试应继续放在独立文件，至少覆盖缓存/重算、子成本最大值、非法并发配置和 MPP enforced 分支。
- 若新增结构体字段，需同步 `Clone`、`MemoryUsage`、[`cache_snapshot.rs`](cache_snapshot.rs) 的 `CachedUnionAll` capture/restore，以及 `lib.rs` 中相应 trait 行为；必要时同步计划缓存契约测试。
- 若改变候选数或 task type，需同时检查普通与分区两个公开入口、Root/MPP 两种 required task、MPP 禁用会话，以及 `lib.rs` 的默认 attach/RootTask 语义。兼容风险主要是 plan shape 与 hint/MPP 选择变化，性能风险主要是成本偏置或并发摊薄不一致。
- Go/Rust 对齐修改应以 [`physical_union_all.go`](physical_union_all.go) 和相关 Go 成本/task 文件为语义基线，不应为通过单个 Rust 测试而删减候选、属性传播或错误边界。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，可用于本次 Rust/Go 符号核验。
- `rustcodegraph node --file pkg/planner/core/operator/physicalop/physical_union_all.rs --offset 1 --limit 320`：读取完整 246 行目标源码，确认 1 个公开结构体、7 个公开固有方法、2 个公开自由函数、1 个私有构造器，无常量、trait 定义或条件编译项。
- `rustcodegraph query PhysicalUnionAll` 与 `query physical_union_all`：确认 Rust/Go 对照符号、crate 导出/测试/缓存等相邻引用；`callers` 对两个 Rust `Exhaust...` 和 `build_union_plans` 未返回调用方，未把缺失图边写成已验证调用。
- `rustcodegraph callees build_union_plans` 及两个 `Exhaust...`：确认公开入口到私有构造器、再到 `Init` 等内部边；成本查询确认 v2 到 `div_cost_ver2`，并确认成本方法读取 session context/variables。
- 已核读路径：[`physical_union_all.rs`](physical_union_all.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`physical_union_all_test.rs`](physical_union_all_test.rs)、[`physical_union_all.go`](physical_union_all.go)、[`cache_snapshot.rs`](cache_snapshot.rs)、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`。补充 `rg` 用于图未返回 callers 时定位 Rust 模块/trait 接线和 Go 上游入口。
- 测试证据：`physical_union_all_test.rs::v1_cost_includes_go_union_worker_overhead` 断言零孩子成本为默认 concurrency factor，两个零成本孩子在强制重算时为 `4 * factor`，对应“最大子成本 + (1 + child count) * factor”。本任务按计划不运行 Cargo。
- 交付结构验证使用任务规定的命令，要求文件存在且固定二级标题恰好为 11 个；另人工复核本文没有把测试写入生产源文件，也没有声称未由索引或源码证明的 Rust 上游调用。
