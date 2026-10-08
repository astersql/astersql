# `pkg/planner/core/operator/physicalop/physical_cte_table.rs`

## 文件定位

本文件位于 `astersql-planner-core-operator-physicalop` crate，crate 根由同目录 `Cargo.toml` 的 `[lib] path = "lib.rs"` 指定。`lib.rs` 以公开模块 `physical_cte_table` 装配本文件，并重新导出其中的 `PhysicalCTETable`；因此外部 crate 可用 `astersql_planner_core_operator_physicalop::PhysicalCTETable` 访问 Go 形态的具体算子。

它描述“读取某个 CTE 临时存储”的物理叶子，但当前 Rust 文件同时保留了两套用途不同的表示：

- `PhysicalCteTable`、`find_best_task_for_cte_table` 是建立在 `physical_common_plans.rs` 简化计划模型上的选优语义载体，目前 RustCodeGraph 只找到独立测试调用，没有发现生产选优入口调用。
- `PhysicalCTETable` 使用 crate 的 `PhysicalSchemaProducer` / `BasePhysicalPlan` 体系；其公共物理计划 trait 接线位于 `lib.rs` 的 `ConcretePhysicalOperator for PhysicalCTETable` 和 `impl_concrete_physical_plan!`，并被 `pkg/executor/statement_ru_plan_walk.rs` 的 RU 计划遍历识别。

本文件只建模计划节点，不读取 CTE 数据。Go 执行期的实际读取发生在 `pkg/executor/builder.go::buildCTETableReader`：它按 `IDForStorage` 查找 CTE 存储并构造 `CTETableReaderExec`。目标 Rust 文件中没有对应执行器构造或存储 I/O。

## 核心职责

1. 用存储 ID 把递归 CTE 中的表读取节点与 CTE 生产者分配的中间存储关联起来（`PhysicalCteTable::id_for_storage`、`PhysicalCTETable::IDForStorage`）。这个 ID 与 `logicalop/logical_cte_table.rs::LogicalCTETable::IDForStorage` 以及 `logical_cte.rs::CTEClass::IDForStorage` 属于同一关联语义。
2. 在简化模型中，把无排序要求的 CTE 表候选转换为无子节点的 `PhysicalPlanNode { kind: PhysicalKind::CteTable, ... }`，复制 schema 和种子统计（`find_best_task_for_cte_table`）。
3. 为具体计划体系提供构造与跨上下文克隆（`PhysicalCTETable::New`、`PhysicalCTETable::Clone`）；EXPLAIN、归一化 EXPLAIN、索引解析、内存和代价接口由 `lib.rs::ConcretePhysicalOperator` 接线。
4. 让 RU v2 计划遍历把它视为零自有 RU 的编排叶子：`statement_ru_plan_walk.rs` 仅在节点为 Root 且没有孩子时接受该形状，实际输出行仍可供父算子计费。

该文件不负责 CTE 定义、种子/递归计划优化、物化循环或执行期缓冲管理；这些分别属于逻辑 CTE、物理 CTE/Sequence 和执行器层。

## 主要符号

- `PhysicalCteTable`：简化模型的数据结构，公开字段为 `id_for_storage: i64`、`seed_statistics: Stats` 和 `schema: Vec<i64>`。它派生 `Clone`、`Debug`、`PartialEq`，但没有实现统一的 `base::Plan` / `base::PhysicalPlan` trait。
- `PhysicalCteTable::explain_info(&self) -> String`：生成 `Scan on CTE_<storage-id>`。它只格式化标识，不访问统计或存储。
- `PhysicalCteTable::memory_usage(&self) -> i64`：返回 `size_of::<Self>() + schema.capacity() * 8`。这是简化结构自身的容量估计，不等同于具体计划体系的基类递归估算。
- `find_best_task_for_cte_table(&PhysicalCteTable, &PhysicalProperty) -> Result<Option<PhysicalPlanNode>, String>`：简化选优入口。排序项非空时返回 `Ok(None)`；否则返回一个 CTE 表叶子。签名保留错误通道，但当前函数没有产生 `Err` 的分支。
- `PhysicalCTETable`：贴近 Go 命名和计划基类布局的具体节点，包含 `PhysicalSchemaProducer` 与 `IDForStorage: i32`。字段名刻意保留 Go 风格，crate 根已允许 `non_snake_case`。
- `PhysicalCTETable::New(ctx, storage_id)`：建立类型名为 `CTETable`、offset 为 `0` 的 `BasePhysicalPlan`，再包入 `PhysicalSchemaProducer`。
- `PhysicalCTETable::Clone(&self, ctx) -> Result<Self, expression::Error>`：通过 `CloneWithNewCtx` 复制基类；若原节点有 schema，则深克隆 schema；原样保留存储 ID。它不是 Rust `Clone` trait，而是可能失败、可替换计划上下文的 Go 风格方法。
- `ConcretePhysicalOperator for PhysicalCTETable`（定义在 `lib.rs`）：提供 schema producer、EXPLAIN、`ResolveIndices`、内存估算及 v1/v2 代价委托；`impl_concrete_physical_plan!` 再统一实现计划 trait。

目标文件没有模块级常量、trait、条件编译项或内部私有辅助函数。

## 执行流程

简化选优流程如下：

1. 调用者传入已包含存储 ID、种子统计和输出列 ID 的 `PhysicalCteTable`，以及请求的 `PhysicalProperty`。
2. `find_best_task_for_cte_table` 检查 `property.sort_items`。CTE 表本身不提供有序性，非空时以 `Ok(None)` 表示没有可用候选。
3. 无排序要求时构造 `PhysicalPlanNode`：计划 ID 和 `PhysicalKind::CteTable.id` 都取存储 ID，schema 与统计被克隆，`children` 和 `required_properties` 均为空。
4. 该返回值表示 Root 侧叶子候选。函数刻意不根据 `property.task_type` 拒绝 Cop/Mpp 请求；`physical_cte_table_test.rs::non_root_property_still_builds_root_cte_table_like_go` 固化了这一 Go 兼容行为。

具体计划流程则是：

1. `PhysicalCTETable::New` 分配统一计划 ID 的基类并记录 `IDForStorage`。
2. 上层通过 `PhysicalSchemaProducer` 设置 schema、孩子与统计等公共状态；本类型语义上应保持叶子。
3. 跨上下文复制时调用 `PhysicalCTETable::Clone`；基类复制失败会原样传播 `expression::Error`，成功后再复制 schema 和存储 ID。
4. `lib.rs` 的统一 trait 接线让该节点参与扁平化、EXPLAIN、索引解析和代价查询。`statement_ru_plan_walk.rs` 在扁平树中把它归为 `Wrapper`，要求 `IsRoot == true` 且 `ChildrenIdx` 为空，并只增加算子数，不添加自身 CPU/扫描工作量。
5. Go 完整运行时随后由 `executorBuilder.buildCTETableReader` 用 `IDForStorage` 取得 `IterInTbl` 并创建读取执行器；Rust 本文件没有这一步的实现，不能据此宣称 Rust 已具备完整 CTE 读取执行链。

## 数据与状态

- 存储关联：`id_for_storage` / `IDForStorage` 是跨逻辑定义、物理扫描和执行器存储表的关联键，不是数据库表 ID。简化节点同时把它用作 `PhysicalPlanNode.id`；具体节点的计划 ID 则由 `BasePhysicalPlan::New` 独立生成，两者不要混用。
- 统计：简化节点持有值类型 `Stats`，生成候选时克隆。Go 原实现从 `LogicalCTETable.StatsInfo()` 安装统计；具体 Rust `PhysicalCTETable::New` 本身不接收统计，公共统计位于其基类体系。
- schema：简化节点持有 `Vec<i64>` 并按值克隆。具体节点的 schema 在 `PhysicalSchemaProducer` 中为可选值，`Clone` 仅在原 schema 存在时复制。
- 计划形状：简化候选始终 `children.is_empty()`、`required_properties.is_empty()`；RU 桥也把具体节点限定为 Root 叶子。给它增加孩子会改变扁平计划和 RU 支持判定。
- 生命周期：所有这些字段都由计划对象拥有。目标文件没有全局缓存、锁、通道、事务或外部句柄；真正的 CTE 存储由执行器层按 ID 管理。

## 依赖与调用关系

上游关系：

- `lib.rs` 声明并公开 `physical_cte_table` 模块，重新导出 `PhysicalCTETable`，再为它实现统一物理算子接口。
- `physical_cte_table_test.rs` 直接构造 `PhysicalCteTable` 并调用 `find_best_task_for_cte_table`；RustCodeGraph 没有发现该函数的其他调用者，因此它目前是已测试但未接入生产主选优分派的简化路径。
- `physical_cte_table_test.rs` 和 `physical_sequence_test.rs` 用 `PhysicalCTETable::New` / `Clone` 验证具体节点及其作为 Sequence 孩子的行为。
- `pkg/executor/statement_ru_plan_walk.rs` 通过 `Any::is::<op::PhysicalCTETable>()` 识别具体节点；对应测试 `statement_ru_plan_walk_test.rs::go_merge_187_mpp_cte_site_shared_forest` 及其前一测试片段构造该节点并验证 RU 树。
- Go 主选优入口 `base_physical_plan.go::FindBestTask` 对 `LogicalCTETable` 分派到 `findBestTask4LogicalCTETable`。当前 Rust 简化函数没有同等调用边，文档因此不把 Go 主链接线投射成 Rust 现状。

下游关系：

- 简化路径依赖 `physical_common_plans::{PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats}`，只调用排序列表判空并构造 `CteTable` 叶子。
- 具体路径依赖 `base::ContextRef`、`PhysicalSchemaProducer`、`BasePhysicalPlan::CloneWithNewCtx`、schema 的 `Clone` 以及 `expression::Error`。
- crate 的直接依赖由 `Cargo.toml` 声明；与本文件直接相关的是 `base`、`expression`，统一 trait 接线还使用 `costusage`。`physical_common_plans` 是同 crate 模块，不是外部 Cargo 依赖。
- Go 执行路径的下游是 `executorBuilder.loadCTEStorages` 和 `CTETableReaderExec`，但 Rust 目标文件不依赖也不调用它们。

## 错误处理与边界

- 有序属性：简化选优返回 `Ok(None)`，表示无候选而非执行错误；测试 `ordered_property_is_rejected` 覆盖此边界。
- TaskType：即使请求属性标记为 Cop 或 Mpp，简化函数仍创建 Root 形态候选；这是对 Go `findBestTask4LogicalCTETable` 不检查 `TaskTp` 的保留，不代表 CTE 表可下推到 Cop/Mpp 执行。
- Index Join：Go 函数在 `IndexJoinProp != nil` 时返回无效任务，即便存在强制 hint；当前简化 Rust `PhysicalProperty` 路径未检查或表达此分支。这是明确的迁移差异，扩展属性模型时必须补齐并加独立测试。
- 克隆失败：`PhysicalCTETable::Clone` 唯一显式失败源是 `CloneWithNewCtx`，以 `expression::Error` 返回；schema 复制和 ID 复制本身不产生错误。
- 空 schema：两套结构都允许空 schema；目标文件不进行合法性校验。具体克隆只保持“有/无 schema”的原状态。
- 内存估算：`PhysicalCteTable::memory_usage` 按 `Vec` 容量估计；Go `MemoryUsage` 是 nil 安全的基类内存加 `size.SizeOfInt`。具体 Rust trait 接线使用基类内存加 `size_of::<i64>()`，而字段实际为 `i32`，因此不能把三者视为完全等价或用于精确分配统计。
- 执行期缺失存储：目标 Rust 文件没有对应错误。Go `buildCTETableReader` 会在存储或 `IterInTbl` 未建立时设置错误并返回 nil，这是执行器层边界。

## 并发与资源生命周期

目标文件自身不启动任务、不持有锁、不使用原子量、通道、事务或异步资源。`PhysicalCteTable` 的统计与 schema 在构造候选时克隆，返回计划不借用输入；`PhysicalCTETable::Clone` 也建立新的基类和 schema，从而避免两个计划实例共享这些可变容器。

共享 CTE 状态位于相邻逻辑/执行层：`LogicalCTETable::SeedStat` 是 `Arc<RwLock<StatsInfo>>`，用于从 CTE 种子共享统计；执行期物化存储由 CTE executor 创建并按 `IDForStorage` 查找。上述并发和资源生命周期不由本文件管理。本节点在 RU 计费中是零自有工作量的叶子，生产者/递归子树拥有实际工作，因而不能把消费者出现次数再次乘到生产者资源上。

## 与 Go 版本的对应关系

Go 对照文件是 `physical_cte_table.go`：

- Go `PhysicalCTETable` 的嵌入 `PhysicalSchemaProducer` 和 `IDForStorage int` 对应 Rust 具体 `PhysicalCTETable`。Rust 用显式字段组合和 `i32` 存储 ID。
- Go `Init(ctx, stats)` 同时安装计划基类和统计；Rust `New(ctx, storage_id)` 安装基类与存储 ID，但不接收统计。统计须由公共 producer/base 接线另行设置。
- Go `ExplainInfo` 对应 Rust `lib.rs::ConcretePhysicalOperator::explain_operator`，两者都输出 `Scan on CTE_<id>`；简化类型还提供相同格式的 `explain_info`。
- Go `MemoryUsage` 对 nil 指针返回 0，并累计基类与 `size.SizeOfInt`；Rust 引用方法天然要求有效实例，且两条 Rust 内存估算路径的口径不同，见“错误处理与边界”。
- Go `findBestTask4LogicalCTETable` 先拒绝 `IndexJoinProp`，再拒绝排序要求，从逻辑节点取得存储 ID、上下文、统计和 schema，最后包装成 `RootTask`。Rust `find_best_task_for_cte_table` 只覆盖排序拒绝、数据复制和 Root 叶子形状；输入已经是物理简化结构，没有从 `LogicalCTETable` 解包，也没有 Index Join 分支或显式 `RootTask` 类型。
- Go 执行器 `buildCTETableReader` 证明该物理节点最终读取生产者准备的 `IterInTbl`。Rust 当前证据仅覆盖计划表示、克隆、Sequence/RU 遍历，不足以证明执行器等价。

因此，本文件是“部分简化选优模型 + 已接入公共 trait/RU 桥的具体计划壳”，不是 Go 文件全部规划与执行语义的一比一完成移植。

## 扩展指南

- 接入 Rust 主选优链时，应复用 `PhysicalCTETable` 的统一计划体系，或明确把 `PhysicalCteTable` 转换成该体系，避免长期维护两套计划 ID、统计和内存语义。接线必须能从 `LogicalCTETable` 取得 `IDForStorage`、schema、统计与上下文。
- 扩充 `PhysicalProperty` 后应在 `find_best_task_for_cte_table` 补上 Go 的 Index Join 属性拒绝，并在 `physical_cte_table_test.rs` 增加回归；不要通过忽略属性或返回占位节点让测试通过。
- 若新增排序能力，必须同步修改选优返回、实际执行器能否兑现顺序、`required_properties`、EXPLAIN 和有序属性测试；仅删除当前拒绝分支会制造错误计划。
- 修改具体节点字段时，必须同步 `PhysicalCTETable::Clone`、`ConcretePhysicalOperator` 的内存/EXPLAIN/解析/代价接口、扁平计划遍历以及独立测试。Rust 测试继续放在 `physical_cte_table_test.rs` 或相关 executor 测试文件，不嵌入生产源文件。
- 修改存储 ID 类型或语义时，需联合检查 `LogicalCTETable`、`CTEClass`、CTE definition/consumer、Sequence、扁平计划和执行器存储映射；风险包括错误关联不同 CTE、计划 ID 冲突以及递归轮次读错缓冲。
- 若实现 Rust 执行器读取，应明确“生产者先建立存储、消费者后查找”的生命周期，复刻 Go 对缺失 storage / `IterInTbl` 的错误，而不是静默返回空结果。
- 性能风险主要来自不必要的 schema/统计深克隆和错误重复计量 CTE 生产者工作；兼容风险主要是 EXPLAIN 文本、Root 叶子形状及 Go 属性拒绝规则漂移。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件按行读取为 154 行。
- RustCodeGraph `query/node/callers/callees`：确认 `PhysicalCteTable`、`PhysicalCTETable`、`find_best_task_for_cte_table` 的定义；后者下游构造 `PhysicalKind::CteTable` 与 `PhysicalPlanNode`，上游仅出现独立测试调用。目标文件的直接使用文件为 `lib.rs`、`physical_cte_table_test.rs`、`physical_sequence_test.rs` 和 `pkg/executor/statement_ru_plan_walk.rs`。
- 生产源码：`physical_cte_table.rs`；装配与 trait 接线：`physicalop/lib.rs`；简化公共模型：`physical_common_plans.rs`；crate 边界：`physicalop/Cargo.toml`。
- Go 对照：`physical_cte_table.go`、`base_physical_plan.go::FindBestTask`、`pkg/executor/builder.go::buildCTETableReader`；逻辑关联证据：`logicalop/logical_cte_table.rs` / `.go` 与 `logicalop/logical_cte.rs` / `.go`。
- Rust 独立测试：`physical_cte_table_test.rs` 覆盖非 Root 请求仍生成候选、排序拒绝、具体节点跨上下文克隆及 EXPLAIN；`physical_sequence_test.rs` 覆盖具体节点作为 Sequence 孩子的克隆顺序；`pkg/executor/statement_ru_plan_walk_test.rs` 覆盖 CTE 表叶子的零自有 RU 和共享 CTE forest。
- Go 相关测试：`pkg/executor/test/executor/executor_test.go` 将 `PhysicalCTETable` 纳入物理算子覆盖清单；它是覆盖性证据，不单独证明本文件每个边界。
- 本任务是纯文档分析，未修改 Rust/Go/Cargo，也未运行 Cargo。交付前执行任务规定的 11 章节结构检查，并人工复核文档区分“当前 Rust 事实”“Go 对照行为”和“尚未接线能力”。
