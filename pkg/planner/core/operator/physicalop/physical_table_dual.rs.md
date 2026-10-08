# `pkg/planner/core/operator/physicalop/physical_table_dual.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-core-operator-physicalop`。同目录 `Cargo.toml` 以 `lib.rs` 为 crate 根，并通过 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `pkg/planner/core/operator/physicalop`；`lib.rs` 用 `mod physical_table_dual` 纳入实现、用 `pub use physical_table_dual::*` 公开符号，并仅在测试配置下装入独立的 `physical_table_dual_test.rs`。

直接说明对象是 [`physical_table_dual.rs`](physical_table_dual.rs)，以下结论均以该源文件的当前实现为准。

`PhysicalTableDual` 是不访问真实表的叶子物理算子，承接 `LogicalTableDual`，为无 `FROM` 的常量查询、已知空结果等场景提供固定行数的行源。它位于逻辑计划枚举与执行器构建之间：本文件创建和初始化物理计划，`pkg/executor/builder.rs::build_typed_physical_plan` 再将其转换为 `TypedTableDual`。目标包没有 `doc.go`，包契约以 `Cargo.toml`、`lib.rs`、统一枚举路由和 Go 对照实现为准。

## 核心职责

- 定义 `PhysicalTableDual`，同时保存通用 Schema 生产者、固定输出行数 `RowCount` 和该算子专有的输出字段名 `names`。
- 通过 `New`/`Init` 建立 `plancodec::TypeDual` 计划，绑定会话上下文、计划 ID、查询块偏移、统计信息和逻辑 Schema。
- 通过 `ExhaustPhysicalPlans4LogicalTableDual` 将 `LogicalTableDual` 枚举成唯一物理候选；IndexJoin 属性，或“要求排序且行数大于一”的属性不能由 Dual 直接满足。
- 提供输出名读写、跨上下文克隆、EXPLAIN 文本和内存估算；`lib.rs` 的 `impl_schema_leaf_operator!` 宏把这些方法接到通用 `PhysicalPlan`/Schema 叶子接口。
- 为计划缓存快照提供包内的 `cache_names`/`restore_cached_names`，使私有输出名可以被 `cache_snapshot.rs::CachedTableDual` 捕获和恢复。

## 主要符号

- `pub struct PhysicalTableDual`：公开结构。`PhysicalSchemaProducer` 和 `RowCount: i32` 可公开访问，`names: NameSlice` 仅本模块直接访问。本文件没有模块常量、枚举、trait 定义或条件编译项。
- `cache_names(&self) -> &NameSlice` / `restore_cached_names(&mut self, NameSlice)`：crate 内缓存适配接口，分别暴露只读名称集合和恢复快照名称。
- `New(ctx: ContextRef, row_count: i32) -> Self`：创建 `TypeDual` 的基础物理计划，初始查询块偏移为 `0`，名称为空；基础构造同时从上下文分配计划 ID。
- `Init(self, ctx, stats, offset) -> Self`：消费算子，重绑上下文，重申计划类型，写入查询块偏移与统计信息后返回自身；不会再次构造基础计划，因此不会额外分配计划 ID。
- `Clone(&self, new_ctx) -> Result<Self, expression::Error>`：在新上下文克隆基础计划和可选 Schema，保留行数，并对每个非空 `FieldName` 调用 `Clone` 后放入新的 `Arc`。
- `OutputNames` / `SetOutputNames`：读取时返回 `NameSlice::Shallow()`，写入时替换整个名称集合。
- `ExplainInfo(&self) -> String`：生成 `rows:<RowCount>`。
- `MemoryUsage(&self) -> i64`：累计 Schema 生产者、行数字段、名称向量本体/容量以及每个实际字段名的内存。
- `ExhaustPhysicalPlans4LogicalTableDual(logical, required) -> Vec<Box<dyn PhysicalPlan>>`：公开的逻辑到物理枚举入口；成功时恰有一个候选，拒绝时为空。

## 执行流程

1. `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的统一路由识别 `logicalop::LogicalTableDual`，调用 `ExhaustPhysicalPlans4LogicalTableDual`。
2. 枚举入口先检查所需属性。`IndexJoinProp` 非空时立即拒绝；若排序项非空且 `logical.RowCount > 1`，由于多行 Dual 无法保证目标顺序，也返回空候选。零行或一行对顺序不敏感，所以可接受排序属性。
3. 函数从逻辑节点克隆 `ContextRef`；逻辑节点没有上下文时返回空候选，而不是 panic。
4. 函数用逻辑行数调用 `PhysicalTableDual::New`，复制逻辑 Schema，再用逻辑统计信息（缺失时取默认值）和 `QueryBlockOffset` 完成 `Init`，最后装箱为唯一 `PhysicalPlan` 候选。
5. 另一条旧 Cascades 路径由 `pkg/planner/cascades/old/implementation_rules.rs::ImplTableDual` 构造同一类型；其 `Match` 只接受空排序属性，`OnImplement` 同样复制行数、统计、Schema 和查询块偏移，再交给 `NewTableDualImpl`。
6. 执行阶段，`pkg/executor/builder.rs::build_typed_physical_plan` 对计划做 `PhysicalTableDual` 下转，校验行数在 `0..=1` 后，用 Schema 各列的返回类型构建 `TypedTableDual`；执行器直接产生零行或一行，不发起存储扫描。

## 数据与状态

`PhysicalSchemaProducer` 承载计划上下文、计划 ID、类型、查询块偏移、统计信息、Schema 和通用物理计划状态。`RowCount` 是 Dual 唯一的执行基数参数；常见语义为 `0`（空结果）或 `1`（单行常量结果）。`names` 与 Schema 分开保存，因为点查计划构建期间可能直接初始化 Dual，仍需保留输出列名称；其元素是可空的共享 `FieldName` 引用。

`OutputNames` 是浅层复制：返回新的 `NameSlice` 容器，但其中的 `Arc<FieldName>` 仍共享。相比之下，`Clone` 对每个字段名做值克隆并创建新 `Arc`，从而使跨上下文计划克隆不共享可识别的字段名对象。`cache_snapshot.rs::CachedTableDual` 则复制名称容器中的 `Arc`，以快照捕获/恢复计划元数据。

`Init` 保留 `New` 已分配的计划 ID。独立测试 `init_reuses_the_constructor_plan_id` 验证初始化前后 ID 相同，下一次 `New` 才获得连续的新 ID；因此扩展初始化逻辑时不能无意重建 `BasePhysicalPlan`。

## 依赖与调用关系

上游和接线点包括：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs`：统一物理候选路由直接引用 `ExhaustPhysicalPlans4LogicalTableDual`；MPP 候选转 Root 时还专门允许 Dual 原样通过，因为内存中的零/一行结果不需要 TiFlash exchange 边界。
- `pkg/planner/cascades/old/implementation_rules.rs::ImplTableDual`：旧 Cascades 规则直接调用 `PhysicalTableDual::New(...).Init(...)`。
- `pkg/planner/core/operator/physicalop/lib.rs`：模块公开、Go/Rust 类型契约登记、缓存契约登记，并通过 `impl_schema_leaf_operator!(PhysicalTableDual, ...)` 生成叶子物理算子的通用接口。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs::CachedTableDual`：捕获和恢复 Schema 生产者、行数和私有名称集合。

本文件的直接下游依赖是：`base::{ContextRef, PhysicalPlan}` 的上下文和 trait；`logicalop::LogicalTableDual` 的上下文、Schema、统计和偏移；`property::{PhysicalProperty, StatsInfo}` 的候选约束与统计；`plancodec::TypeDual` 的稳定计划类型；`types::metadata::NameSlice` 的输出名称；当前 crate 的 `BasePhysicalPlan` 与 `PhysicalSchemaProducer` 的通用实现；`expression::Error` 的克隆错误通道。

后续消费者包括 `pkg/executor/builder.rs` 的 typed executor 构建、`pkg/executor/statement_ru_plan_walk.rs` 的资源计量计划遍历，以及多种计划组合和缓存测试。RustCodeGraph 的文件节点报告目标文件被 11 个文件使用，并点名 `statement_ru_plan_walk.rs`、`implementation_rules.rs`、`cache_snapshot.rs` 及相关测试；精确源码搜索进一步确认上述直接边。

## 错误处理与边界

- `ExhaustPhysicalPlans4LogicalTableDual` 不返回错误对象：属性不兼容或上下文缺失都以空候选表示。IndexJoin 属性即使有 enforce hint 也不在本算子内补偿。
- 排序拒绝条件只针对 `RowCount > 1`；零行或一行天然不会违反顺序要求。统计信息缺失时采用 `StatsInfo::default()`。
- `CloneWithNewCtx` 的失败通过 `expression::Error` 原样传播；本文件没有吞错或错误改写。其他方法对有效 Rust 引用操作，不存在 Go 的 nil 接收者分支。
- 当前规划结构的 `RowCount` 类型是任意 `i32`，枚举函数也不会把它夹到零或一；但 typed executor builder 明确只接受 `0..=1`，否则返回 `BuildError("invalid row count for dual table")`。因此文档不能声称多行 Dual 已可执行；若未来允许多行，必须同步构建器和执行器语义。
- `MemoryUsage` 按名称向量容量而非长度计费，并累计每个非空字段名；修改 `NameSlice` 表示或所有权策略时需要同步公式。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。计划对象在规划期被构造、初始化、克隆、缓存和下转；执行资源直到 executor builder 创建 `TypedTableDual` 后才存在。

`ContextRef` 与字段名使用共享引用所有权。普通 `OutputNames` 调用保留字段名的 `Arc` 共享关系；跨上下文 `Clone` 则创建独立的字段名值。`New` 从计划上下文分配一次 ID，`Init` 只更新既有对象；缓存恢复用调用者提供的新上下文重建计划，再恢复快照状态。上述生命周期均无本地同步要求，线程安全边界由 `ContextRef`、`Arc` 及其承载类型决定。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/physicalop/physical_table_dual.go`。

- 两版结构都嵌入/持有 `PhysicalSchemaProducer`，保存 `RowCount` 和专有 `names`，并提供初始化、输出名读写、内存估算和 `rows:N` EXPLAIN 文本。
- Go `Init` 在方法内新建 `BasePhysicalPlan`；Rust 将这一步拆为 `New` 与 `Init`，从而可以验证初始化不会重复分配计划 ID。Go 返回指针，Rust 消费并返回值。
- Go `OutputNames` 直接返回切片；Rust 用 `NameSlice::Shallow` 复制容器并共享内部 `Arc`。Rust `Clone` 显式深克隆 Schema 与字段名，用于跨上下文计划复制；Go 的计划缓存克隆由生成文件 `plan_clone_generated.go::CloneForPlanCache` 处理。
- Go `MemoryUsage` 对 nil 接收者返回零，并用 `size.SizeOfInt`、`size.SizeOfSlice`、指针容量和字段名内存计费；Rust 有效引用排除了 nil，并按 `i32`、向量本体、容量和字段名内存计算。两者意图一致，但 Rust 的 `RowCount` 固定为 32 位。
- Go `findBestTask4LogicalTableDual` 返回 RootTask，拒绝 IndexJoin 或“多行且需排序”的属性；Rust `ExhaustPhysicalPlans4LogicalTableDual` 保留相同拒绝条件和 Schema/统计/偏移复制，但只返回物理候选，由统一任务枚举层包装。
- Go 文件还包含 `findBestTask4LogicalMockDatasource`，用一行 Dual 代替 MockDataSource；该逻辑不在本 Rust 文件中。Rust 另有旧 Cascades `ImplTableDual` 路径，其排序匹配条件比本文件的零/一行特例更严格。
- Go 结构本身没有在同目录发现专属 `physical_table_dual_test.go`；最近的 Go 使用证据包括 `pkg/planner/core/common_plans_test.go` 和多个 executor 测试。Rust 的专属回归集中在独立文件 `physical_table_dual_test.rs`。

## 扩展指南

- 改变候选属性时，首先修改 `ExhaustPhysicalPlans4LogicalTableDual`，并核对旧 Cascades `ImplTableDual::{Match, OnImplement}` 与 Go `findBestTask4LogicalTableDual`，尤其避免零/一行排序特例在两条 Rust 优化路径中继续漂移。
- 新增字段时，应同步 `New`、`Init`、`Clone`、`MemoryUsage`、`ExplainInfo`，以及 `cache_snapshot.rs::CachedTableDual::{capture, restore}` 和 `lib.rs` 的缓存契约；若字段影响执行，还需同步 `pkg/executor/builder.rs` 与 `TypedTableDual`。
- 改变 `RowCount` 合法范围时，必须明确规划与执行边界。目前 builder 仅接受 `0..=1`；不能只放宽本文件而留下执行器拒绝，也不能为通过测试把 Go 的行数语义悄然简化。
- 改变输出名所有权时，应扩展独立测试 `physical_table_dual_test.rs`，分别验证 `OutputNames` 的浅共享、`Clone` 的字段名独立性、缓存快照的预期共享/复制行为和内存容量计费。Rust 测试必须保留在独立测试文件，不能嵌入本源文件。
- 改变构造或初始化时，应保留“每次 `New` 只分配一个计划 ID、`Init` 不再分配”的不变量，并同步 `init_reuses_the_constructor_plan_id`。
- 兼容性风险主要在计划类型、EXPLAIN 文本、属性接受范围、Schema/名称克隆和 Go 对齐；性能风险主要在字段名深克隆、名称容量计费和缓存快照复制。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，Rust 与 Go 均已纳入。
- RustCodeGraph `query PhysicalTableDual`：定位到 Rust `physical_table_dual.rs::PhysicalTableDual` 和 Go 同名结构，并列出 Rust `New` 的 executor/cascades/测试消费者；`query ExhaustPhysicalPlans4LogicalTableDual` 定位到本文件第 129 行的公开枚举入口。
- RustCodeGraph `node --file pkg/planner/core/operator/physicalop/physical_table_dual.rs`：读取完整 148 行源码，并报告 11 个使用文件。精确 `callers/callees` 查询本次未返回边，因此又用下列直接源码接线核验，未把无输出解释成“无调用者”。
- 源、crate 与模块边界：`pkg/planner/core/operator/physicalop/physical_table_dual.rs`、`Cargo.toml`、`lib.rs`、`base_physical_plan.rs`。
- 规划与缓存路径：`pkg/planner/cascades/old/implementation_rules.rs`、`pkg/planner/core/operator/physicalop/cache_snapshot.rs`。
- 执行边界：`pkg/executor/builder.rs`；该文件直接证明当前 `RowCount` 仅允许 `0..=1`。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_table_dual.go`、`plan_clone_generated.go`；最近的 Go 测试使用点为 `pkg/planner/core/common_plans_test.go`。
- 独立 Rust 测试：`pkg/planner/core/operator/physicalop/physical_table_dual_test.rs`，验证输出名浅共享、跨上下文克隆的字段名独立性、容量相关内存估算，以及 `Init` 复用构造阶段计划 ID。
- 人工复核结论：该文件存在是为了将不依赖存储的固定基数逻辑行源转换为带 Schema、统计和稳定计划元数据的物理叶子；它通过属性拒绝、上下文缺失回退和克隆错误传播覆盖规划边界。安全扩展必须同步统一枚举、旧 Cascades、缓存快照、typed executor builder、独立测试和 Go 语义基线。
