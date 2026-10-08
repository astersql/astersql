# `pkg/planner/core/operator/physicalop/physical_sequence.rs`

## 文件定位

本文件位于物理算子 crate `astersql-planner-core-operator-physicalop` 中；crate 入口在 [`lib.rs`](./lib.rs) 以 `pub mod physical_sequence` 暴露模块，并把 `TypedPhysicalSequence` 重导出为 crate 级 `PhysicalSequence`。因此要区分两个同文件类型：`physical_sequence::PhysicalSequence` 是便于属性穷举与独立测试的轻量模型，`TypedPhysicalSequence` 才是接入 `base::Plan`/`base::PhysicalPlan` trait 体系的具体算子类型。

该算子对应 Go 的 [`physical_sequence.go`](./physical_sequence.go)：Sequence 把若干 CTE producer 和最后一个主查询组织为同一物理节点。孩子顺序是核心协议——`children[..len-1]` 是 CTE producer，`children[len-1]` 是主查询；输出 Schema 和任务统计均以最后一个孩子为准。

[`Cargo.toml`](./Cargo.toml) 声明该目录为独立 crate（`lib.rs`、`autotests = false`），并通过本地依赖连接 `base`、`expression`、`logicalop`、`property`、`plancodec` 等规划组件。当前文件的可执行轻量层直接复用同 crate 的 `physical_common_plans`，typed 层直接使用 `base`、`expression` 和 crate 根导出的基础计划类型。

## 核心职责

1. `PhysicalSequence` 保存有序孩子、简化 Schema、查询块偏移和会话 MPP 开关，为 CTE Sequence 的属性枚举提供可执行模型。
2. `exhaust_physical_sequence` 根据父属性请求构造 Root 或 MPP 子属性组合，并为每一种合法组合生成一个 `PhysicalKind::Sequence` 候选。
3. `output_schema`、`attach_to_tasks` 固化“最后一个孩子是主查询”的输出协议；`attach_to_tasks` 同时把主查询统计带到组装后的节点。
4. `TypedPhysicalSequence` 组合真实 `PhysicalSchemaProducer`，负责新建和跨上下文克隆；crate 根随后为它实现 `ConcretePhysicalOperator` 及通用物理计划 trait。
5. 文件前半的长注释是 Go 完整语义的迁移参考，不参与编译；当前事实应以第 182 行后的 Rust 实现和 `lib.rs` 接线为准。

## 主要符号

- `pub struct PhysicalSequence`：轻量可执行模型。`children: Vec<PhysicalPlanNode>` 保留 producer/主查询顺序；`schema` 是简化列 UniqueID 列表，但输出查询应调用 `output_schema` 读取最后一个孩子；`block_offset` 用于候选 ID 与简化 EXPLAIN ID；`mpp_allowed` 保存逻辑 Sequence 所在会话是否允许 MPP。
- `PhysicalSequence::set_mpp_allowed(&mut self, bool)`：在穷举前注入会话 MPP 能力，是 Root 请求是否增加第二个 MPP 候选的条件之一。
- `PhysicalSequence::memory_usage(&self) -> i64`：累计结构体本体、`schema` 已分配容量以及各 `PhysicalPlanNode::memory_usage`；它没有另计 `children` 向量缓冲区，调用者不应把结果理解为精确分配器账单。
- `PhysicalSequence::explain_id(&self) -> String` 与 `explain_info(&self) -> &'static str`：分别返回 `Sequence_{block_offset}` 和固定文本 `Sequence Node`。typed 层的通用 EXPLAIN 则由 `lib.rs` 中 `ConcretePhysicalOperator::explain_operator` 提供固定文本。
- `PhysicalSequence::output_schema(&self) -> Option<&[i64]>`：安全读取最后一个孩子的 Schema；空孩子返回 `None`。
- `PhysicalSequence::attach_to_tasks(self) -> Result<PhysicalPlanNode, String>`：空孩子时报错；否则生成 `PhysicalKind::Sequence`，Schema/统计取最后孩子，ID 取所有孩子最大 ID 加一，并按原顺序转移孩子所有权。
- `pub fn exhaust_physical_sequence(...) -> (Vec<PhysicalPlanNode>, bool)`：物理候选枚举入口；第二个返回值恒为 `true`，对应 Go `ExhaustPhysicalPlans4LogicalSequence` 的 `canAddEnforcer`。
- `pub struct TypedPhysicalSequence`：真实算子外壳，仅包含 `PhysicalSchemaProducer`；`New` 用类型名 `Sequence` 创建 `BasePhysicalPlan`，`Clone` 通过 `CloneWithNewCtx` 换上下文并深克隆可选 Schema。
- `lib.rs` 中的 `pub use physical_sequence::TypedPhysicalSequence as PhysicalSequence`：决定 crate 外部看到的是 typed 类型，而非轻量同名类型；同文件的轻量类型仍可通过模块全路径访问。

## 执行流程

候选穷举由 `exhaust_physical_sequence` 驱动：

1. 若 `sequence.children` 为空，立即返回空候选和 `true`，避免访问不存在的主查询。
2. 若父请求是 MPP 且 `cte_producer_status == SomeFailedMpp`，说明已有 CTE producer 无法 MPP，立即拒绝整个 MPP Sequence。
3. 构造公共的 `any_mpp` producer 属性：MPP 任务、`f64::MAX` 期望行数、任意分区、允许 enforcer，并传播 `no_cop_push_down`。
4. MPP 父请求只产生一种组合：所有 producer 使用 `any_mpp` 且状态改为 `AllCanMpp`，最后的主查询完整继承请求属性。
5. 非 MPP 父请求必产生 Root 组合：producer 使用 Root 属性，producer 与主查询的 CTE 状态都标成 `SomeFailedMpp`。只有请求未失败、会话允许 MPP 且无排序项时，才额外产生全 MPP 组合。
6. 对每种组合，把 producer 属性重复 `child_count - 1` 次，再追加主查询属性；候选 Schema 取最后孩子，孩子与统计分别从输入 Sequence 和参数克隆，节点种类固定为 `PhysicalKind::Sequence`。

节点组装的另一条路径是 `attach_to_tasks`：先检查非空，再从最后孩子复制 Schema/统计，计算新 ID，最后把全部孩子移动进新节点。真实 planner 的 `pkg/planner/core/task.rs::attach2Task4PhysicalSequence` 则把各任务升为 Root、取出计划并按序组装；它和轻量 helper 的数据结构不同，但都保持孩子顺序。

typed 路径从 `TypedPhysicalSequence::New` 开始，经 `lib.rs` 的重导出和 `impl_concrete_physical_plan!` 获得通用 `Plan`/`PhysicalPlan` 行为。`Clone` 先为新上下文克隆 `BasePhysicalPlan`，再克隆已有 Schema；孩子顺序由宏/基础计划的克隆逻辑保持，独立测试对此有断言。

## 数据与状态

- 孩子向量的顺序具有语义，不是可任意重排的集合。最后元素是主查询，其余元素属于 CTE producer；Go 的 `fragment.go` 也按该约定把前置孩子转换为 CTE storage/sink，并单独继续处理最后孩子。
- `PhysicalProperty` 中与本文件直接相关的状态包括 `task_type`、`sort_items`、`partition_type`、`cte_producer_status`、`no_cop_push_down`、`can_add_enforcer` 和 `expected_count`。候选构造只克隆值，不共享可变引用。
- `CteProducerStatus::SomeFailedMpp` 是阻止错误 MPP 组合的哨兵；`AllCanMpp` 只在父请求本身为 MPP、所有 producer 必须成功时写入 producer 属性。
- `mpp_allowed` 是构造候选时的会话快照，不含锁或内部同步；调用方必须在穷举前通过 `set_mpp_allowed` 设置真实会话值。
- 轻量 `schema` 字段可与最后孩子 Schema 不同，测试故意设置为 `[999]`；`output_schema`、候选生成和附着均以最后孩子为权威，防止陈旧缓存泄漏到输出。
- typed 类型把计划上下文、Schema、孩子、统计等状态交给 `PhysicalSchemaProducer`/`BasePhysicalPlan` 管理；`Clone` 只显式处理上下文和 Schema，通用 trait 实现负责其余基础行为。

## 依赖与调用关系

上游方面，RustCodeGraph `explore` 将 `exhaust_physical_sequence` 的直接调用定位到 [`physical_sequence_test.rs`](./physical_sequence_test.rs) 的四个属性枚举测试；当前仓库搜索未发现生产 Rust 路径调用这个轻量穷举函数，因此它目前主要是已移植语义的可执行验证面。`output_schema`、`attach_to_tasks` 和解释接口也由同一独立测试覆盖。

typed `PhysicalSequence` 由 `lib.rs` 重导出后进入生产 trait 图：`ConcretePhysicalOperator` 提供 producer 访问、Explain、索引解析、内存和代价委派；`impl_concrete_physical_plan!` 生成通用计划实现。`pkg/executor/statement_ru_plan_walk.rs` 把它识别为有孩子的 Root wrapper，相关测试创建 CTE table 与主查询孩子并确认 RU 遍历完成。

下游方面，轻量实现依赖 `physical_common_plans::{PhysicalPlanNode, PhysicalProperty, Stats, PhysicalKind, TaskType, PartitionType, CteProducerStatus}`。typed 实现调用 `BasePhysicalPlan::New`、`BasePhysicalPlan::CloneWithNewCtx`、`PhysicalSchemaProducer::{New, SchemaRef, SetSchema}` 和 `expression::Schema::Clone`。

Go 主链提供更完整的接线依据：`ExhaustPhysicalPlans4LogicalSequence` 从 `LogicalSequence` 或 memo group 的最后输入取得 Schema，`Attach2Task` 通过 `utilfuncp.Attach2Task4PhysicalSequence` 路由到 `pkg/planner/core/task.go`；MPP fragment 处理在 `fragment.go` 中识别 Sequence 并拆出 CTE producer。Rust 的 `pkg/planner/core/task.rs` 已有按 `PlanKind::Sequence` 分发的 attach helper，但轻量 `PhysicalPlanNode` 与 typed trait 对象仍是两套表示。

## 错误处理与边界

- 空孩子：`output_schema` 返回 `None`；`attach_to_tasks` 返回 `Err("PhysicalSequence requires at least one child")`；`exhaust_physical_sequence` 返回空候选。这是 Rust 对 Go 代码“至少一个孩子”隐含前置条件的显式保护。
- MPP 不可行：MPP 请求遇到 `SomeFailedMpp` 返回空候选而非错误，表示合法地无可选物理计划；`can_add_enforcer` 仍为 `true`。
- 排序约束：Root 请求带任意 `sort_items` 时不额外尝试 MPP，因为当前 Sequence MPP 组合只表达任意分区，不能证明顺序属性。
- MPP 会话开关关闭时同样只保留 Root 组合；不能仅凭父属性默认值推断会话允许 MPP。
- `attach_to_tasks` 的 ID 使用 `max + 1`，在孩子 ID 为 `i64::MAX` 时可能在调试构建溢出；当前代码没有显式防护，扩展时不应依赖该轻量 ID 算法承担全局 ID 分配。
- typed `Clone` 通过 `Result<_, expression::Error>` 传播基础计划克隆失败；它不会吞掉错误。轻量属性穷举当前无错误返回类型，所有拒绝情形用空候选表达。
- 文件中的注释版 `ExhaustPhysicalPlans4LogicalSequence`、`Init`、`Schema` 等不是可调用符号；新增代码不能引用这些注释签名作为已实现 API。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络资源。所有轻量状态通过值、`Vec` 和克隆传递：`exhaust_physical_sequence` 消费 `PhysicalSequence`，但为每个候选克隆孩子、Schema 和统计；`attach_to_tasks` 消费自身并把孩子所有权移动到结果，避免悬垂引用。

`output_schema` 返回借用切片，其生命周期受 `&self` 约束；调用者不能在借用存续时改变或销毁孩子向量。`TypedPhysicalSequence::Clone` 接受引用计数式 `base::ContextRef`，把新上下文交给基础计划克隆，并复制 Schema，使克隆体不依赖原 Schema 的可变所有权。并发安全性由 `ContextRef`、trait 对象及其字段类型决定，本文件没有额外的 `Send`/`Sync` 保证。

内存方面，候选枚举最多建立两套属性和计划副本；孩子数量增加时，producer 属性向量和每个候选的孩子克隆均线性增长。任何减少克隆的优化都必须保持候选之间属性与孩子状态隔离。

## 与 Go 版本的对应关系

Rust `exhaust_physical_sequence` 保留了 Go `ExhaustPhysicalPlans4LogicalSequence` 的关键决策矩阵：MPP+失败状态拒绝；MPP 请求把 producer 标为 `AllCTECanMpp`；Root 请求把两侧标为 `SomeCTEFailedMpp`；允许 MPP、无排序且未失败时追加第二候选；每个 producer 复制统一属性而主查询使用另一属性；返回值允许 enforcer。

两者也有明确差异：

- Go 从 `LogicalSequence`/memo group、会话上下文、统计信息和查询块偏移直接构建真实 `base.PhysicalPlan`；Rust 轻量函数接收预组装 `PhysicalSequence`、显式 `Stats` 与布尔 MPP 开关，返回 `PhysicalPlanNode`。
- Go `Schema()` 直接索引最后孩子，空孩子会触发越界；Rust 对输出、附着和穷举分别用 `Option`、`Result` 或空候选处理。
- Go `ExplainID` 读取会话的 `IgnoreExplainIDSuffix` 并基于计划类型/ID 延迟格式化；轻量 Rust 只返回 `Sequence_{block_offset}`。crate 对外 typed 类型的通用 Explain 行为来自基础 trait，不能用轻量 `explain_id` 推断完整 Go 等价性。
- Go `Attach2Task` 在所有任务为 MPP 时可组装新的 MPP task，否则返回最后任务，并合并 warning；当前 Rust `pkg/planner/core/task.rs` 把任务统一升 Root，而轻量 `attach_to_tasks` 只组装节点。任务类型、分区列和 warning 合并尚未在本文件形成一比一实现。
- Go `PhysicalSequence` 直接嵌入 `PhysicalSchemaProducer`；Rust 用 `TypedPhysicalSequence` 承担这一角色，同时保留一个同名轻量模型。文件前半的注释记录了目标 Go 形状，但不是编译产物。

相关 Rust 回归集中在 [`physical_sequence_test.rs`](./physical_sequence_test.rs)：覆盖 MPP 属性传播、失败拒绝、Root/MPP 双候选、排序或 MPP 禁用、主查询 Schema/统计、Explain 和 typed 克隆孩子顺序。仓库搜索未发现专门命名的 Go `physical_sequence_test.go`；Go 语义的直接依据是生产文件 `physical_sequence.go`、`task.go` 与 `fragment.go`。

## 扩展指南

- 修改候选选择条件时，优先改 `exhaust_physical_sequence`，并在 `physical_sequence_test.rs` 增加对应独立测试；至少覆盖 MPP/Root、三种 CTE 状态、排序有无、MPP 开关和 `no_cop_push_down` 传播，保持与 Go 决策矩阵一致。
- 修改孩子布局时，必须同步审计 `output_schema`、`attach_to_tasks`、候选属性下标、typed 克隆、`pkg/planner/core/task.rs::attach2Task4PhysicalSequence`、Go `fragment.go` 的 CTE producer 拆分，以及 RU wrapper 遍历。最后孩子协议属于跨模块兼容约束。
- 扩展真实算子字段应放在 `TypedPhysicalSequence`，并同步 `Clone`、`lib.rs::ConcretePhysicalOperator` 的内存/成本/解析逻辑；不要只扩展轻量同名结构后误以为 crate 对外类型已变化。
- 若把轻量穷举接入生产 planner，需要先明确 `PhysicalPlanNode` 与 typed trait 对象的转换边界、会话 MPP 状态来源、真实计划 ID 分配和错误类型；当前没有生产调用边证明这种接线已经完成。
- 若对齐 Go MPP attach，应在 `pkg/planner/core/task.rs` 的独立测试中验证 MPP 保留、分区信息和 warning 传播，不能通过简化 Root 转换掩盖差异。
- 性能风险主要是按候选深克隆所有孩子和属性；优化前先保持候选隔离与孩子顺序，并用多 producer 场景验证。兼容风险主要来自 EXPLAIN ID、MPP attach 和空孩子行为的 Rust/Go 差异。
- 测试逻辑应继续放在独立 `physical_sequence_test.rs`，不要嵌回生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示当前索引包含 11,467 个文件、307,296 个节点；`node --file pkg/planner/core/operator/physicalop/physical_sequence.rs --offset 1 --limit 500` 读取了全部 345 行，并报告该文件被 executor RU、CTE table/projection/Sequence 测试等文件使用；`query PhysicalSequence` 和 `query exhaust_physical_sequence --kind function` 核对了类型及函数位置。
- RustCodeGraph `explore "pkg/planner/core/operator/physicalop/physical_sequence.rs PhysicalSequence"` 给出 `exhaust_physical_sequence` 的四个直接测试调用、`sequence` 在 `find_best_task.rs`/测试中的引用，以及 Go `Schema`、`Init`、attach helper 的调用关联。单独 `callers/callees` 查询在 30 秒窗口内未返回，因此未据此声称完整调用图；缺口由模块接线和精确文本搜索补验。
- 源码：[`physical_sequence.rs`](./physical_sequence.rs) 的可执行轻量结构、候选函数和 typed 结构；[`lib.rs`](./lib.rs) 的模块声明、重导出、`ConcretePhysicalOperator` 与宏接线；[`physical_common_plans.rs`](./physical_common_plans.rs) 的公共属性/节点模型。
- crate：[`Cargo.toml`](./Cargo.toml) 的 `lib.rs` 入口、`autotests = false`、本地规划依赖及 `package.metadata.porting.go-package`。
- Go 对照：[`physical_sequence.go`](./physical_sequence.go) 的完整算子与穷举逻辑；`pkg/planner/core/task.go::attach2Task4PhysicalSequence` 的 MPP task 组装；[`fragment.go`](./fragment.go) 的 CTE producer/主查询拆分。
- 测试：[`physical_sequence_test.rs`](./physical_sequence_test.rs) 的六个独立测试；`pkg/executor/statement_ru_plan_walk_test.rs` 的 typed Sequence RU 遍历用例。未运行 Cargo，符合本任务纯文档约束。
