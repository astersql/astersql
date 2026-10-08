# `pkg/planner/core/operator/physicalop/physical_table_sample.rs`

## 文件定位

[源文件 `physical_table_sample.rs`](./physical_table_sample.rs) 定义 Rust 侧的物理表采样算子 `PhysicalTableSample`。它位于 `astersql-planner-core-operator-physicalop` crate；该归属由同目录 `Cargo.toml` 的 `[package]` 与 `[lib] path = "lib.rs"` 确认。`lib.rs` 公开声明并再导出 `physical_table_sample`，随后用 `impl_schema_leaf_operator!(PhysicalTableSample)` 把该类型接入通用 `base::Plan` / `base::PhysicalPlan` 接口。

需要区分“算子类型已具备物理计划接口”和“Rust 规划主链已使用该类型”。仓库内对 `PhysicalTableSample` 的 Rust 构造引用目前只有独立测试 `physical_table_sample_test.rs`；Rust `pkg/planner/core/find_best_task.rs::convertToSampleTable` 仍返回 `PlanNode::new(PlanKind::Other("TableSample"))`，并未构造本文件类型。Go 主链则在 `pkg/planner/core/find_best_task.go::convertToSampleTable` 中构造真实的 `physicalop.PhysicalTableSample`。因此，本文件当前是已移植并可通过统一 trait 使用、但尚未被 Rust 规划主链直接接线的实现。

## 核心职责

- `PhysicalTableSample` 聚合通用物理计划状态、采样描述、表对象、物理表 ID 和扫描方向，表示从基表返回采样行的叶子物理算子。
- `New` 创建带 `plancodec::TypeTableSample` 类型码的基础计划；`Init` 设置最终上下文、查询块偏移和固定为 `1.0` 的行数估计。
- `WithTableSampleInfo` 以 builder 形式装入共享的 `tablesampler::TableSampleInfo`。
- `Clone` 为新的计划上下文复制算子，复制 schema，并通过 `Arc` 共享表和采样元数据。
- `MemoryUsage` 按 Go 实现的计费项目估算内存；`ExplainInfo` 当前返回空字符串。

本文件不负责解析 `TABLESAMPLE` 语法、选择采样方法、执行 KV/Region 采样或产生行数据。采样 AST、完整 schema 与分区集合由 `pkg/planner/util/tablesampler/sample.rs::TableSampleInfo` 表示；执行行为在 executor 层。

## 主要符号

- `pub struct PhysicalTableSample`：公开算子类型。
  - `PhysicalSchemaProducer: PhysicalSchemaProducer`：嵌入 schema 生产者和 `BasePhysicalPlan`，由 `lib.rs` 的宏委托实现通用计划接口。
  - `TableSampleInfo: Option<Arc<tablesampler::TableSampleInfo>>`：可选采样 AST、完整 schema 和分区表列表的共享所有权。
  - `TableInfo: Option<Arc<dyn table::Table>>`：可选的目标表接口对象；使用 trait object 以容纳不同表实现。
  - `PhysicalTableID: i64`：实际扫描的物理表 ID。
  - `Desc: bool`：是否按逆向方向扫描。
- `New(ctx, physical_table_id, desc) -> Self`：建立 `BasePhysicalPlan::New(ctx, TypeTableSample, 0)`，其余可选字段为空，并保存物理表 ID 与方向。
- `WithTableSampleInfo(self, info) -> Self`：消费并返回自身，将 `TableSampleInfo` 设为 `Some`。
- `Init(self, ctx, offset) -> Self`：更新已有基础计划的上下文、类型码和查询块偏移，并安装 `StatsInfo { RowCount: 1.0, ..Default::default() }`。
- `Clone(&self, new_ctx) -> Result<Self, expression::Error>`：通过 `BasePhysicalPlan::CloneWithNewCtx` 克隆通用状态；若原 schema 存在，再调用 `schema.Clone()` 设置到新 producer；其余字段复制或克隆 `Arc`。
- `ExplainInfo(&self) -> String`：当前恒定返回空字符串，意味着通用 explain 接口不会从该方法得到算子专有详情。
- `MemoryUsage(&self) -> i64`：累计 producer、表接口槽位、布尔值及可选 `TableSampleInfo` 的估算；未单独加入 `PhysicalTableID`，与当前 Go 实现的计费项目一致。

文件中没有模块级常量、独立 trait 或条件编译项。测试模块的条件编译声明位于 `lib.rs`，不在本文件内。

## 执行流程

1. 理想的规划输入来自逻辑数据源上的采样信息。Go 路径由 `logical_plan_builder.go` 调用 `tablesampler.NewTableSampleInfo` 并在 `find_best_task.go::convertToSampleTable` 中创建本类算子；Rust 当前对应路径仍生成通用的 `PlanNode::Other("TableSample")`，所以以下是本类型自身可执行的生命周期，而不是已证实的 Rust 端到端调用链。
2. 调用 `New(ctx, physical_table_id, desc)` 时，`BasePhysicalPlan::New` 分配计划基础状态，类型设为 `TypeTableSample`，初始 query-block offset 为 `0`。
3. 可通过 `WithTableSampleInfo` 装入采样元数据。表对象 `TableInfo` 和 schema 需要由调用方通过公开字段或通用 setter 补齐；`New` 不验证这些字段。
4. `Init(ctx, offset)` 在同一基础计划上更新上下文、类型和 offset，并把估计行数设为一。独立测试证明 `Init` 不重新分配计划 ID。
5. `lib.rs::impl_schema_leaf_operator!` 将该对象作为 schema 叶子算子暴露：schema/children/resolve/成本计算等操作委托给 `PhysicalSchemaProducer` 或 `BasePhysicalPlan`；explain 和内存查询回调本文件方法。
6. 计划缓存或通用物理计划克隆通过宏生成的 `clone_for_plan_cache` / `clone_physical` 进入 `Clone`。克隆失败时，前者返回 `(None, false)`，后者传播 `expression::Error`。
7. 真正的表采样读取不在本文件进行。Go 集成测试 `pkg/executor/sample_test.go` 覆盖空表、多 Region、schema 变化与 explain 中出现 `TableSample` 等端到端行为，但不能据此宣称 Rust 主链已经使用本类型。

## 数据与状态

`PhysicalSchemaProducer` 拥有计划 ID、上下文、类型、统计信息、query-block offset、schema 和通用子计划状态。`Init` 的重要不变量是保持构造时的计划 ID，同时把 `RowCount` 固定为 `1.0`；`physical_table_sample_test.rs::init_and_memory_usage_match_go_contract` 对这两点有直接断言。

`TableSampleInfo` 使用 `Arc` 共享。其定义位于 `pkg/planner/util/tablesampler/sample.rs`，包含可选 `AstNode`、可选 `FullSchema` 和 `Vec<Arc<dyn PartitionedTable>>`。`PhysicalTableSample::Clone` 只增加 `Arc` 引用计数，不深拷贝其中内容。`TableInfo` 也采用同样的共享策略。该文件没有提供修改这些共享对象内部状态的路径。

`New` 只初始化 `PhysicalTableID` 与 `Desc`，`TableSampleInfo`、`TableInfo` 均为 `None`。类型系统允许对象在元数据不完整时存在；本文件没有“已完整初始化”标志，也没有在 `Init`、`Clone` 或 `MemoryUsage` 中校验必需字段。

`MemoryUsage` 是近似账目而非递归独占内存大小：producer 由其自身算法计费，表对象仅计一个 `Option<Arc<dyn table::Table>>` 槽位，采样信息则调用其 `MemoryUsage`。多个克隆共享同一 `Arc` 时，各克隆仍会报告该采样信息的估算值，因此不能将多个实例的返回值简单相加当作进程实际独占内存。

## 依赖与调用关系

上游与装配关系：

- `pkg/planner/core/operator/physicalop/lib.rs` 声明、再导出本模块，并通过 `impl_schema_leaf_operator!(PhysicalTableSample)` 为该类型实现通用计划 trait。
- `pkg/planner/core/operator/physicalop/physical_table_sample_test.rs` 是当前仓库内唯一直接调用 Rust `New`、`WithTableSampleInfo`、`Init` 和 `MemoryUsage` 的位置。
- Rust 生产规划路径 `pkg/planner/core/find_best_task.rs::convertToSampleTable` 当前使用通用 `PlanNode`，所以未发现生产调用者；Go 对应路径 `pkg/planner/core/find_best_task.go::convertToSampleTable` 是语义参照而非 Rust 调用边。

直接下游依赖：

- `base::{ContextRef, Plan, PhysicalPlan}`：计划上下文、基础访问器、统计设置和 trait 接口。
- `BasePhysicalPlan` / `PhysicalSchemaProducer`：计划通用状态、schema、克隆、resolve、代价与内存估算。
- `property::StatsInfo`：保存固定的一行基数估计。
- `plancodec::TypeTableSample`：标识 explain/codec 所用计划类型。
- `tablesampler::TableSampleInfo`：采样参数、完整 schema 与分区元数据。
- `table::Table`：抽象目标表。
- `expression::Error`：克隆基础计划或 schema 相关流程的错误类型。

`Cargo.toml` 明确声明了 `base`、`expression`、`property`、`plancodec`、`table`、`tablesampler` 等本地 crate 依赖；没有为本文件设置 feature gate。RustCodeGraph 能识别本文件、结构体及六个函数，但 callers/callees 查询对 `New`、`Init`、`Clone`、`MemoryUsage` 等通用名称产生跨仓库歧义；调用者结论因此以精确 `rg` 引用核验为准。

## 错误处理与边界

- `Clone` 是本文件唯一返回 `Result` 的入口。`BasePhysicalPlan::CloneWithNewCtx(new_ctx)?` 的错误原样作为 `expression::Error` 传播；没有吞错或替换错误上下文。
- `New`、`WithTableSampleInfo` 与 `Init` 不返回错误，也不检查 `TableSampleInfo`、`TableInfo`、schema、物理表 ID 或扫描方向之间的一致性。调用者必须在进入执行层前保证计划完整。
- Rust 方法都要求有效的 `&self`/`self`，不存在 Go `(*PhysicalTableSample)(nil).MemoryUsage() == 0` 的 nil receiver 情形。可选字段为 `None` 时不会 panic：`MemoryUsage` 通过 `map_or(0, ...)` 跳过采样信息，`Clone` 原样保留空值。
- `ExplainInfo` 返回空串是当前事实；算子名称仍可由基础计划的 `TypeTableSample` 提供，但没有专有参数说明。新增 explain 内容时必须同时考虑 normalized explain 的行为，因为叶子宏让二者都调用同一方法。
- `MemoryUsage` 使用 `std::mem::size_of::<Option<Arc<dyn table::Table>>>()` 表示 Go 的接口槽位计费，并用 `size_of::<bool>()` 对齐布尔字段；这是一项移植契约，不代表 Rust 堆对象的完整实际占用。
- `Init` 固定 `RowCount = 1.0` 是从 Go 移植的估计，不是对实际返回行数的保证。Go executor 测试显示多 Region 场景可能返回多行。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或显式 I/O。所有 builder/初始化方法都消费 `self`，避免同一值被并发地半初始化。

`Arc<TableSampleInfo>` 与 `Arc<dyn table::Table>` 提供线程安全引用计数层面的共享生命周期；最后一个引用释放时对象才会销毁。不过，能否跨线程发送或共享仍取决于具体 trait/object 的约束，本文件没有额外声明或启动跨线程使用，不能仅凭 `Arc` 推断整个对象一定是 `Send + Sync`。

克隆后的 producer 拥有新的基础计划对象和克隆后的 schema，而采样信息、表接口继续共享。`TableSampleInfo::NewTableSampleInfo` 会克隆传入 schema，这使最初保存的完整 schema 不随调用方后续修改而变化；其独立测试 `sample_test.rs::new_table_sample_info_owns_an_independent_schema_clone` 覆盖该生命周期边界。

## 与 Go 版本的对应关系

直接参照文件是 `pkg/planner/core/operator/physicalop/physical_table_sample.go`。

- 字段一一对应：schema producer、`TableSampleInfo`、`TableInfo`、`PhysicalTableID`、`Desc`。Go 指针/接口在 Rust 中分别表示为 `Option<Arc<_>>` 和 `Option<Arc<dyn table::Table>>`。
- Go 没有单独的 `New`；常见调用是结构体字面量后 `.Init(ctx, offset)`。Rust 拆为 `New` 和 `Init`：前者先创建基础计划，后者更新上下文/类型/offset/统计。独立 Rust 测试确认这种拆分保持 plan ID，不等同于重新构造。
- Go `Init` 调用 `NewBasePhysicalPlan(..., &p, offset)` 并设置 `RowCount: 1`；Rust `Init` 对已有 base plan 调用 setter，并设置相同的一行估计。外部可观察目标相同，但内部构造时序不同。
- Go 当前未定义显式 `Clone` 或 `ExplainInfo` 于该文件；Rust 为统一物理计划 trait 补充了上下文克隆和空 explain 实现。
- 两边 `MemoryUsage` 都计算 producer、表接口、布尔值和非空采样信息，均未单独加入 `PhysicalTableID`。Go nil receiver 返回零；Rust 不存在 nil receiver。Rust 对表接口使用自身 `Option<Arc<dyn Table>>` 的槽位大小，而不是直接引用 Go `size.SizeOfInterface` 常量。
- Go 生产主链已经在 `find_best_task.go::convertToSampleTable` 填充采样信息、表对象、物理表 ID、方向与 schema；Rust `find_best_task.rs` 当前仅生成同名通用节点。这是明确的迁移/接线差距，不应把 Go 的完整行为描述成 Rust 已支持。

Go 的 `pkg/executor/sample_test.go` 验证 SQL 层的 Region 采样、空表、多表、schema 变化和 explain；Rust 同路径独立测试只验证本结构的初始化与内存契约。两类测试证明的范围不同。

## 扩展指南

- 若要完成 Rust 规划主链接线，最可能修改 `pkg/planner/core/find_best_task.rs::convertToSampleTable`，使其构造并填充本类型，而不是通用 `PlanNode::Other`；同时必须核对 Rust `DataSource` 是否已携带等价的 `TableSampleInfo`、真实 `table::Table`、物理表 ID 和方向，不能用占位值伪造 Go 行为。
- 新增或改变初始化字段时，同步修改 `New`、`Init` 和 `Clone`，确保计划缓存克隆不会丢字段；相应回归应放在独立的 `physical_table_sample_test.rs`，不要把测试嵌入生产源文件。
- 改变采样元数据时，应同时核对 `pkg/planner/util/tablesampler/sample.rs` 及其独立 `sample_test.rs`。新增共享字段需明确深拷贝还是 `Arc` 共享，并同步更新 `MemoryUsage`。
- 为 explain 增加细节时修改 `ExplainInfo`，并注意 `impl_schema_leaf_operator!` 同时把它用于普通和 normalized explain；任何不稳定标识都可能破坏 normalized explain 契约。
- 改动基数估计时修改 `Init` 中的 `StatsInfo`，并评估优化器成本和计划选择兼容性。`1.0` 是 Go 当前契约，而非实际样本行数上限。
- 改动内存估算时与 Go `physical_table_sample.go::MemoryUsage`、Rust `tablesampler::TableSampleInfo::MemoryUsage` 同步；主要风险是漏算新字段、对共享对象重复计费，或把 Rust 布局误当作 Go 接口布局。
- 完成生产接线后，除结构级单测外还需要规划器/执行器独立测试证明真正生成此具体类型并正确执行。当前 Go `pkg/executor/sample_test.go` 可作为行为清单，但 Rust 测试需基于 Rust 实际接口建立，不能以 Go 测试通过替代。

## 验证依据

- RustCodeGraph：`status` 显示索引可用（包含 Rust 与 Go）；`query PhysicalTableSample --json` 命中 Go 结构体和本文件 Rust 结构体；`query physical_table_sample.rs --json` 列出 `New`、`WithTableSampleInfo`、`Init`、`Clone`、`ExplainInfo`、`MemoryUsage`。图的通用名称 callers/callees 结果存在歧义，未用其跨文件误匹配作为事实依据。
- 目标源码：`pkg/planner/core/operator/physicalop/physical_table_sample.rs`，完整读取结构体和全部六个方法。
- crate 与装配：`pkg/planner/core/operator/physicalop/Cargo.toml`；`pkg/planner/core/operator/physicalop/lib.rs` 中的模块声明、再导出、`ConcretePhysicalOperator`、`impl_concrete_physical_plan!`、`impl_schema_leaf_operator!` 及其对本类型的调用。
- Go 对照与生产入口：`pkg/planner/core/operator/physicalop/physical_table_sample.go`；`pkg/planner/core/find_best_task.go::convertToSampleTable`、`validateTableSamplePlan`。
- Rust 主链现状：`pkg/planner/core/find_best_task.rs::convertToSampleTable`、`validateTableSamplePlan`，以及全仓精确搜索 `PhysicalTableSample::` / `PhysicalTableSample {`。
- 直接独立测试：`pkg/planner/core/operator/physicalop/physical_table_sample_test.rs::init_and_memory_usage_match_go_contract`。
- 直接依赖与其测试：`pkg/planner/util/tablesampler/sample.rs`、`sample.go`、`sample_test.rs`。
- 相关端到端 Go 测试：`pkg/executor/sample_test.go`；它用于说明 Go 行为覆盖面，不作为 Rust 已完成生产接线的证据。
- 未运行 Cargo 或代码测试：任务是纯文档分析，任务计划明确禁止运行 Cargo。交付前只运行任务指定的 11 章节结构检查，并人工复核无未经证实的 Rust 主链支持声明。
