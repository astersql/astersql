# `pkg/planner/core/operator/logicalop/logical_partition_union_all.rs`

## 文件定位

本文件定义逻辑计划节点 `LogicalPartitionUnionAll`。它表示静态分区裁剪把一张分区表展开为多个分区分支后，对这些分支执行不去重合并的专用 `UNION ALL`。节点复用 `LogicalUnionAll` 的 schema、子节点、统计信息和通用逻辑计划状态，但通过 `Init` 把计划类型设为 `"PartitionUnion"`，使后续物理规划能够保留“分区合并”身份。

模块入口 [`lib.rs`](lib.rs) 以私有模块 `logical_partition_union_all` 装配本文件，并通过 `pub use logical_partition_union_all::*` 对 crate 外导出类型。最近的 [`Cargo.toml`](Cargo.toml) 将它归入 `astersql-planner-core-operator-logicalop` crate；该 crate 直接依赖 `astersql-planner-core-base`、`astersql-expression`、`astersql-planner-property`、`astersql-planner-planctx` 和 `astersql-util-plancodec` 等规划基础 crate，且 `package.metadata.porting.go-package` 指向 Go 包 `pkg/planner/core/operator/logicalop`。

当前 Rust 接线并不完整：[`rule_partition_processor.rs`](../../rule/rule_partition_processor.rs) 中创建 `LogicalPartitionUnionAll` 的静态分区裁剪代码仍是注释式迁移文本。因此，在已检索的 Rust 生产代码中，本类型已有物理计划分派和实现，但尚未由 Rust 分区裁剪规则实际构造；当前可执行构造证据主要来自独立单元测试。不能据此宣称 Rust SQL 主链已经能产生该节点。

## 核心职责

- `LogicalPartitionUnionAll` 以组合方式包装一个公开字段 `LogicalUnionAll`，让分区合并节点复用普通 UnionAll 的 `LogicalSchemaProducer`、`BaseLogicalPlan`、children、schema 和 stats。
- `Init` 建立类型为 `"PartitionUnion"` 的 `BaseLogicalPlan`，保存规划上下文、分配计划 ID，并记录 query-block offset。
- `PruneColumns` 把列裁剪原样委托给内层 `LogicalUnionAll::PruneColumns`，从而在所有分区分支上按相同输出位置裁剪列并在需要时补投影。
- `PushDownTopN` 为每个分区分支分别复制一个 TopN，其行数上限为父 TopN 的 `offset + count`，同时保留原 TopN 在合并节点之上，以保证全局排序、偏移和最终计数语义。
- `LogicalPlan` 实现把通用状态访问和统计推导委托给内层 UnionAll，同时保留本类型的动态类型，使物理计划分派能够识别分区 Union。

## 主要符号

- `pub struct LogicalPartitionUnionAll { pub LogicalUnionAll: LogicalUnionAll }`：唯一的数据类型。`#[derive(Default)]` 使未初始化实例可用于构造和测试；真正进入规划树前应调用 `Init`。
- `Init(self, ctx: base::ContextRef, offset: i32) -> Self`：用 `NewBaseLogicalPlan(ctx, "PartitionUnion", offset)` 替换内层基类。`NewBaseLogicalPlan` 会调用 `PlanContext::alloc_plan_id`，并保存 context、类型和 query-block offset。
- `PruneColumns(&mut self, columns: &[Column]) -> Result<()>`：直接调用 `self.LogicalUnionAll.PruneColumns(columns)`。错误类型为本 crate 的 `PlannerError`。
- `PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef>`：本文件的主要专用算法；克隆排序项和下推标志，为所有 children 递归下推分支 TopN，最后重组返回树。
- `impl LogicalPlan for LogicalPartitionUnionAll`：`as_any`/`as_any_mut` 支持运行时向下转型；`base`/`base_mut` 暴露内层基类；trait 入口 `PruneColumns`、`PushDownTopN` 调回本类型固有方法；`DeriveStats` 委托 `LogicalUnionAll::DeriveStats`。
- [`hash64_equals_generated.rs`](hash64_equals_generated.rs) 中另有生成式 `Hash64`/`Equals`：两者都委托内层 `LogicalUnionAll`，因此 schema producer 的变化会改变哈希与相等性结果。这些方法不是在本文件中实现，但属于本类型的实际行为。

## 执行流程

1. 预期上游在静态分区裁剪得到多个分区数据源时构造本节点，设置 children，并复制原数据源 schema。Go 的真实入口位于 [`rule_partition_processor.go`](../../rule/rule_partition_processor.go) 的 `PartitionProcessor.prune`；Rust 同名规则目前只有注释文本。
2. `Init` 调用 `NewBaseLogicalPlan`，将节点标识设为 `PartitionUnion`，并建立上下文、ID 和 query-block offset。
3. 优化阶段调用 `PruneColumns` 时，内层 UnionAll 先根据父层引用列计算保留位图，再对所有 children 递归裁剪；若子分支仍有多余列，普通 UnionAll 会在该分支上补 `LogicalProjection` 以对齐输出 schema。
4. 优化阶段调用 `PushDownTopN(Some(top_n))` 时，方法先确认传入节点能向下转型为 `LogicalTopN`，提取 `Count + Offset`、`PreferLimitToCop`、`ByItems`、planner context 和 query-block offset。对每个 child，它创建 `Offset = 0`、`Count = Count + Offset` 的新 `LogicalTopN`，递归调用 child 的 `PushDownTopN`，并用非空返回值替换该 child。
5. 所有分支处理后，`std::mem::take(self)` 把当前 Union 节点移入返回树，同时把原位置恢复为默认值。若原 TopN 存在，就将该 Union 设为原 TopN 的唯一 child 并返回原 TopN；否则直接返回 Union。
6. 统计推导通过内层 `LogicalUnionAll::DeriveStats` 完成：若可复用缓存则返回缓存；否则累加各分支行数，并按输出列 `UniqueID` 累加各分支 NDV，然后缓存结果。
7. 物理化阶段，[`base_physical_plan.rs`](../physicalop/base_physical_plan.rs) 对本类型做 `downcast_ref` 分派到 `ExhaustPhysicalPlans4LogicalPartitionUnionAll`；[`physical_union_all.rs`](../physicalop/physical_union_all.rs) 再调用共享的 `build_union_plans`，但传入 `plancodec::TypePartitionUnion` 来生成候选 `PhysicalUnionAll`。

## 数据与状态

本类型不另存分区 ID、分区名称或分区表达式；“哪些分区参与”完全体现在 `BaseLogicalPlan.children` 中。每个 child 是一个 `LogicalPlanRef`，代表一个已裁剪分区的逻辑子树。输出 schema、output names、stats、函数依赖、任务缓存和 TiFlash 标记等均位于内层 `LogicalUnionAll.LogicalSchemaProducer.BaseLogicalPlan`。

TopN 下推读取并复制三类状态：`Count + Offset` 决定每个分支最多需提供的候选行数；`PreferLimitToCop` 保留执行位置偏好；`ByItems` 保留排序表达式与升降序。分支 TopN 使用父 TopN 自身的 planner context 和 query-block offset 初始化。上层原 TopN 保持原来的 `Offset`、`Count` 和排序项，因此跨分区合并后仍执行一次全局截断。

`PushDownTopN` 会取得 `&mut self` 的所有权内容：`std::mem::take(self)` 后，调用者手中的原对象成为默认值，真正节点位于返回的 boxed 计划树中。这是 Rust API 的重要所有权约束；调用方必须使用返回值替换原计划位置，不能继续把原变量当作已初始化节点。

## 依赖与调用关系

上游事实分为 Go 完整链路和 Rust 当前链路：

- Go `PartitionProcessor.prune` 在裁剪结果为零个分区时返回 `LogicalTableDual`，一个分区时直接返回该 child，多于一个分区时才构造 `LogicalPartitionUnionAll`。这说明节点存在的必要条件是“静态裁剪后仍有多个分区分支”。
- Rust [`rule_partition_processor.rs`](../../rule/rule_partition_processor.rs) 保留了同一段逻辑，但尚处于注释状态；RustCodeGraph 对 Rust 类型的直接 callers 查询为空，源码搜索也未发现测试以外的 Rust 构造点。因此 Rust 当前不能声称具有上述生产入口。
- 优化器通过 `LogicalPlan` trait 调用 `PruneColumns`、`PushDownTopN` 和 `DeriveStats`。专用实现下游依赖 `LogicalUnionAll`、`LogicalTopN`、`Column`、`ByItems`、`StatsInfo`、`BaseLogicalPlan` 与 `base::ContextRef`。
- 物理化分派在 `base_physical_plan.rs` 中显式识别本类型，调用 `ExhaustPhysicalPlans4LogicalPartitionUnionAll`；后者依赖 `build_union_plans` 和 `plancodec::TypePartitionUnion`。
- 生成式 `Hash64`/`Equals` 依赖内层 UnionAll 的 schema producer，用于 memo/计划比较一类场景；相关独立测试位于 [`logicalop_test/hash64_equals_test.rs`](logicalop_test/hash64_equals_test.rs)。

RustCodeGraph 给出的 Go 文件使用者包括 `pkg/planner/core/rule/rule_partition_processor.go`、`pkg/planner/core/exhaust_physical_plans.go` 和 `pkg/planner/cascades/memo/group_expr.go`；对 Rust 物理枚举函数，图的 `node` 查询确认其调用 `build_union_plans`，但精确 callers/callees JSON 查询未返回边，宏分派关系由 `base_physical_plan.rs` 源码直接核验。

## 错误处理与边界

- `PruneColumns` 使用 `Result<()>` 传播任一 child 裁剪或补 projection 时产生的 `PlannerError`；本文件不吞掉或改写错误。
- `PushDownTopN` 假设非空参数的动态类型一定是 `LogicalTopN`。类型不符会在 `downcast_ref(...).expect(...)` 处 panic，而不是返回规划错误；调用方必须遵守 `LogicalPlan::PushDownTopN` 的协议。
- 非空 TopN 还必须保留 planner context，否则 `.SCtx().cloned().expect(...)` 会 panic。分支 TopN 的初始化依赖该上下文。
- `Count` 与 `Offset` 使用 `wrapping_add`，明确保持无符号计数溢出时的环绕语义，与 Go `uint64` 加法一致。扩展时不可随意改成饱和或报错而不评估兼容性。
- child 的 `PushDownTopN` 返回 `None` 时，本实现保留原 child；返回 `Some` 时才替换。空 children 也可安全通过循环，随后仍返回 Union 或挂在原 TopN 下。
- `PushDownTopN(None)` 不创建分支 TopN，并返回移动后的 `LogicalPartitionUnionAll`。独立测试锁定了这一边界。
- 未调用 `Init` 的默认实例没有 planner context、有效节点类型或已分配 ID；测试允许在不触碰上下文的路径使用默认 child，但生产规划节点应初始化后使用。

## 并发与资源生命周期

本文件没有锁、原子变量、channel、后台任务、事务或 I/O。所有变换都要求 `&mut self`，因此同一计划节点的 children、schema 和 stats 在 Rust 类型系统下按顺序独占修改；文件自身不提供跨线程共享策略。

`base::ContextRef` 是引用计数上下文：`Init` 把它存入基类，TopN 下推时又为各分支 TopN 克隆引用。`ByItems` 也按分支克隆，避免多个分支共享同一个可变 Vec 容器。计划节点由 `Box<dyn LogicalPlan>` 组成树；`std::mem::take` 和 `SetChildren` 负责转移所有权，不产生显式资源清理工作，树被丢弃时由 Rust 自动释放。

独立测试中的 `TestPlanContext` 用 `AtomicI32` 分配计划 ID，但那是测试上下文的线程安全实现，不代表本节点包含并发算法。此节点的性能敏感点是分支数量：TopN 与列裁剪都逐 child 遍历，TopN 还会为每个分支克隆排序项。

## 与 Go 版本的对应关系

Go 对照文件是 [`logical_partition_union_all.go`](logical_partition_union_all.go)。类型结构相同：Go 匿名嵌入 `LogicalUnionAll`，Rust 使用命名字段组合。两端 `Init` 都设置 `TypePartitionUnion`/`"PartitionUnion"`，并保留 query-block offset。

TopN 语义基本逐项对应：都在每个 child 上下推 `Count + Offset`，复制 `PreferLimitToCop` 和排序项，最后保留原 TopN 在 Union 之上；无 TopN 时都返回 Union 本身。Rust 使用 `Option<LogicalPlanRef>` 表达 Go 的 nil，并用 `wrapping_add` 明确复现 Go `uint64` 溢出行为。一个实现细节差异是 Go 用 partition union 的 `SCtx()` 初始化分支 TopN，而 Rust 从传入的上层 TopN 读取 context；在正常同一计划树中两者应一致，但若未来允许不同 context，必须决定并测试兼容语义。

列裁剪的 API 形态不同。Go 的 `PruneColumns` 返回一个可能被替换的 `base.LogicalPlan`，并在结果仍为 `*LogicalUnionAll` 时把它写回 wrapper；Rust 的 `LogicalUnionAll::PruneColumns` 是原地 `Result<()>`，因此 wrapper 只需委托。Rust 没有 Go 那个“裁剪后变成其他节点”的返回分支，这是底层 Rust API 的差异，不应误写成遗漏了错误传播。

Go 的静态分区裁剪生产入口与 UnionScan 重写均已实现；Rust `rule_partition_processor.rs` 中对应内容仍是注释，因此迁移状态尚不等价。另一方面，Rust 已具备物理计划分派、Hash/Equals、TopN/列裁剪/统计行为和相应独立测试。

## 扩展指南

- 若改变分区 Union 的构造条件，优先实现或修改 `pkg/planner/core/rule/rule_partition_processor.rs`，并保持 Go 的零分区、单分区、多分区三分支以及 UnionScan 重写语义；不要只在本类型内推断分区元数据，因为本类型并不保存这些信息。
- 若增加逻辑优化行为，应在本文件的固有方法与 `impl LogicalPlan` 转发入口同时接线；可复用普通 UnionAll 行为时优先委托，专属语义才在 wrapper 中实现。
- 若修改 TopN 下推，必须保持全局 TopN、每分支 `offset + count`、排序项方向、`PreferLimitToCop` 和 context/query-block offset。同步扩展 [`logical_partition_union_all_test.rs`](logical_partition_union_all_test.rs)，至少覆盖多 child、无 TopN、offset 溢出边界、child 拒绝下推和 context 一致性；测试逻辑继续放在独立文件，不嵌入生产源文件。
- 若改变 schema、统计或相等性，需同时核对 `logical_union_all.rs`、`hash64_equals_generated.rs` 及 `logicalop_test/hash64_equals_test.rs`，避免 wrapper 与内层状态失配。
- 若增加物理实现选择，修改 `physicalop/physical_union_all.rs` 的 `ExhaustPhysicalPlans4LogicalPartitionUnionAll` 或其共享 `build_union_plans`，并保留 `TypePartitionUnion` 编码身份；同时评估 explain 输出、memo 比较和候选计划数量。
- 性能风险主要随分区数和排序项数增长：每个分区都会新增 TopN 节点并克隆 `ByItems`。兼容风险主要来自 Go/Rust context 来源、溢出语义、计划类型字符串以及原 TopN 是否保留。

## 验证依据

- 目标实现：[`logical_partition_union_all.rs`](logical_partition_union_all.rs)，核对了 `LogicalPartitionUnionAll`、`Init`、`PruneColumns`、`PushDownTopN` 和完整 `LogicalPlan` 实现。
- 复用实现：[`logical_union_all.rs`](logical_union_all.rs) 的列裁剪、TopN 下推和 `DeriveStats`；[`base_logical_plan.rs`](base_logical_plan.rs) 的 `LogicalPlan` 协议、基类字段与 `NewBaseLogicalPlan`。
- crate 与装配：[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)。
- Go 对照与真实生产入口：[`logical_partition_union_all.go`](logical_partition_union_all.go) 和 [`rule_partition_processor.go`](../../rule/rule_partition_processor.go)。
- Rust 迁移状态：[`rule_partition_processor.rs`](../../rule/rule_partition_processor.rs) 中相应构造与 UnionScan 重写仍为注释文本。
- 物理化：[`base_physical_plan.rs`](../physicalop/base_physical_plan.rs) 的类型分派，以及 [`physical_union_all.rs`](../physicalop/physical_union_all.rs) 的 `ExhaustPhysicalPlans4LogicalPartitionUnionAll -> build_union_plans`。
- 测试：[`logical_partition_union_all_test.rs`](logical_partition_union_all_test.rs) 验证每分区克隆 TopN、`offset + count`、排序方向、cop 偏好和无 TopN 返回；[`logicalop_test/hash64_equals_test.rs`](logicalop_test/hash64_equals_test.rs) 及 Go 同名测试验证 schema 参与 Hash/Equals。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query LogicalPartitionUnionAll` 定位 Rust/Go 类型及 Rust/Go 物理枚举函数；`node` 核对 Rust 类型、Go 文件、Go 分区裁剪入口和 Rust 物理枚举函数；精确 Rust 类型 callers 返回空数组，物理枚举函数的 `node` trail 指向 `build_union_plans`。模糊 `explore` 结果仅用于发现候选，重要关系均回到精确节点或源码核验。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构验证要求文档存在且恰好包含上述 11 个固定二级标题。
