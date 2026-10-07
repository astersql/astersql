# `pkg/planner/core/operator/logicalop/logical_sequence.rs`

## 文件定位

本文件定义逻辑计划节点 `LogicalSequence`，位于 `astersql-planner-core-operator-logicalop` crate。模块由 `logicalop/lib.rs` 声明并通过 `pub use logical_sequence::*` 对外导出；crate 的 `Cargo.toml` 用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/planner/core/operator/logicalop`。

`LogicalSequence` 表示一个有顺序约束的 CTE（公用表表达式）计划组：前面的子节点是 CTE producer，最后一个子节点是主查询。节点本身不执行 CTE，也不保存 CTE 专属字段；它主要维持子计划顺序，并把对外可见的 schema、谓词、列需求和统计语义落到主查询子节点。Rust 优化器在 `pkg/planner/core/optimizer_runtime.rs` 的 `push_down_sequence_owned`/`attach_pushed_sequence` 中识别和重建该节点。

当前移植边界必须特别说明：Rust 的逻辑节点及 Sequence 下推规则已经存在并被独立测试覆盖；但 `pkg/planner/core/operator/physicalop/physical_sequence.rs` 中 `ExhaustPhysicalPlans4LogicalSequence` 的完整逻辑仍以注释形式保留。因此，不能仅凭本文件断言 Rust 已具备 Go 版本从逻辑 Sequence 到物理 Sequence 的完整物理化链路。

## 核心职责

- `LogicalSequence::Init` 创建类型名为 `"Sequence"` 的基础逻辑计划，保留规划上下文与查询块偏移，并由 `NewBaseLogicalPlan` 分配计划 ID。
- `LogicalSequence::Schema` 将最后一个孩子视为主查询并返回其 schema；若节点尚无孩子，则返回 `BaseLogicalPlan` 的空或显式设置 schema。这一空节点回退是 Rust 的防御性行为，不是 Go 实现的数组索引语义。
- `LogicalSequence::PredicatePushDown` 只把谓词交给最后一个孩子，避免将主查询谓词错误地应用到前置 CTE producer。
- `LogicalSequence::PruneColumns` 只裁剪最后一个孩子，前置 producer 的列需求不由此入口重写。
- `LogicalSequence::DeriveStats` 从最后一个孩子重新派生统计，缓存一份克隆到当前节点，并原样返回孩子报告的 `changed` 标志。
- `LogicalSequence::PreparePossibleProperties` 使用基类规则汇总所有孩子的 TiFlash 可用性：孩子列表非空且每个布尔值都为 `true` 时结果才为 `true`。
- `impl LogicalPlan for LogicalSequence` 提供动态向下转型和基类访问，并将上述四个专用行为接入统一的逻辑计划 trait 分派。

## 主要符号

`pub struct LogicalSequence { pub BaseLogicalPlan: BaseLogicalPlan }`

该结构没有 Sequence 专属数据字段。上下文、类型名、计划 ID、查询块偏移、子节点、schema、输出名、统计、函数依赖、TiFlash 标记和任务缓存都由 `BaseLogicalPlan` 持有。公开字段名保持 Go 风格，以便逐项移植和对照。

`Init(self, ctx: base::ContextRef, offset: i32) -> Self`

以值接收并返回初始化后的节点。它调用 `NewBaseLogicalPlan(ctx, "Sequence", offset)`；该构造器会通过上下文分配新计划 ID，并设置类型和查询块偏移，其余字段使用默认值。

`Schema(&self) -> &Schema`

读取 `Children().last()` 的 schema；无孩子时回退到基类 schema。正常不变量是至少存在一个主查询孩子，因此回退主要用于默认构造、哈希/相等测试或构造中的临时状态。

`PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>>`

取得最后一个可变孩子后调用 `PredicatePushDownPlan`。该 helper 会调用孩子的 `PredicatePushDownRoot`，若孩子返回替换计划则原位替换该孩子，并返回仍需保留在上层的残差谓词。

`PruneColumns(&mut self, columns: &[Column]) -> Result<()>`

直接调用最后一个孩子的 trait 方法 `PruneColumns`，错误原样向上传播。

`DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)>`

调用最后一个孩子的 `DeriveStats(reload)`，随后通过 `SetStats(stats.clone())` 更新当前 Sequence 缓存。返回的统计对象与孩子一致，`changed` 不重新计算。

`PreparePossibleProperties(&mut self, children_have_tiflash: &[bool]) -> bool`

委托 `BaseLogicalPlan::PreparePossibleProperties`。其实现为 `!slice.is_empty() && slice.iter().all(...)`，同时将结果写入基类的 `has_ti_flash` 缓存。

`impl LogicalPlan for LogicalSequence`

`as_any`/`as_any_mut` 支持规则层按具体类型识别节点；`base`/`base_mut` 暴露共同状态；`Schema`、`PredicatePushDown`、`PruneColumns`、`DeriveStats` 显式转发到同名固有方法，避免落入基类默认的通用多子节点行为。

此外，邻接生成文件 `hash64_equals_generated.rs` 为该类型实现 `Hash64` 和 `Equals`：两者按 `Schema().Columns`/`Schema().Equal(...)` 工作，而不是比较整个子树或 Sequence 自身的计划 ID。这意味着无孩子默认节点会按其基类 schema 比较，有孩子节点则按主查询 schema 比较。

## 执行流程

1. 构造阶段调用 `LogicalSequence::default().Init(context, query_block)`，得到具有上下文、类型和 ID，但尚无孩子的节点。
2. 调用方按依赖顺序组装孩子：零到多个 CTE producer 在前，主查询在最后。`optimizer_runtime.rs::attach_pushed_sequence` 先把主查询追加到保存的 `ctes`，再调用 `SetChildren`，保证这一顺序不变量。
3. Sequence 下推规则 `push_down_sequence_owned` 遇到现有 Sequence 时，用 `TakeChildren` 取得全部孩子、`pop` 出主查询，把其余孩子累积为 CTE 列表；随后沿一元算子继续向下，在数据源、`LogicalCTE`、多叉节点或其他边界处由 `attach_pushed_sequence` 重建 Sequence。
4. 逻辑优化调用通用 `LogicalPlan` 接口时，schema、谓词下推、列裁剪和统计派生都通过本文件的覆盖实现只作用于末孩子。前置 CTE producer 保持其定义与顺序。
5. 可能属性准备阶段将所有孩子的 TiFlash 可用性布尔值交给基类汇总；任何孩子不可用或输入为空都会使当前节点的 `has_ti_flash` 为 `false`。
6. 后续物理计划阶段在 Go 中由 `ExhaustPhysicalPlans4LogicalSequence` 为 producer 与主查询构造不同子属性；Rust 同名物理文件目前没有可执行的对应函数，因此该步骤是已知迁移缺口，而非本逻辑文件已实现的流程。

## 数据与状态

本文件自身唯一字段是 `BaseLogicalPlan`。与 Sequence 行为直接相关的基类状态包括：

- `children: Vec<LogicalPlanRef>`：有序保存 CTE producer 与末尾主查询。顺序具有语义，不能排序或去重。
- `schema: Schema`：通常不是 Sequence 输出的首选来源，因为 `Schema` 优先读取末孩子；无孩子时才作为回退，也被默认构造与哈希测试使用。
- `stats: Option<StatsInfo>`：`DeriveStats` 成功后缓存末孩子统计的克隆，使 Sequence 自身的 `StatsInfo` 与主查询一致。
- `has_ti_flash: bool`：由 `PreparePossibleProperties` 写入，表达全部孩子是否都具备 TiFlash 可行性，而不是只看主查询。
- `ctx`、`id`、`query_block_offset`、`output_names`：由初始化或调用方设置。`attach_pushed_sequence` 会从主查询浅拷贝输出名，再将 Sequence 包装回计划树。

关键不变量是“最后一个孩子必为主查询”。`Schema` 的空节点回退使只读访问安全，但会修改计划或派生统计的三个方法都要求该孩子存在。前置孩子的依赖方向也应保持从较后 producer 指向较早 producer；Go 文件用 `c0/c1/c2` 示例说明禁止较早 producer 反向依赖较后的 producer。

## 依赖与调用关系

本文件通过 `use crate::*` 使用同一 logicalop crate 导出的 `BaseLogicalPlan`、`LogicalPlan`、`LogicalPlanRef`、`NewBaseLogicalPlan`、`PredicatePushDownPlan`、`PlannerError`，以及从其他 workspace crate 再导出的 `Expression`、`Column`、`Schema`、`StatsInfo`。直接标准库依赖只有 `std::any::Any`。

已核实的上游关系：

- `pkg/planner/core/optimizer_runtime.rs::push_down_sequence_owned` 通过 `as_any().is::<LogicalSequence>()` 识别节点，拆出末孩子并维持 CTE 顺序。
- `pkg/planner/core/optimizer_runtime.rs::attach_pushed_sequence` 初始化节点、设置输出名和孩子，是生产 Rust 代码中直接构造 `LogicalSequence` 的明确入口。
- `pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs::push_down_sequence_moves_sequence_below_unary_main_query` 构造真实逻辑节点并验证优化后 Sequence 穿过一元 Projection、仍只出现一次。
- `pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.rs::TestLogicalSequence` 通过默认节点和显式 schema 验证生成的相等/哈希语义。

已核实的下游关系：

- `PredicatePushDown` 调用 `base_logical_plan.rs::PredicatePushDownPlan`，该 helper 允许孩子用替换计划更新当前位置。
- `PruneColumns` 与 `DeriveStats` 通过 `dyn LogicalPlan` 分派到末孩子的具体实现。
- `PreparePossibleProperties` 调用 `BaseLogicalPlan::PreparePossibleProperties`，写入 TiFlash 聚合缓存。
- `Hash64`/`Equals` 由 `hash64_equals_generated.rs` 读取本节点的动态 `Schema`。

RustCodeGraph 将目标文件标为被多个逻辑算子和 memo 文件使用，但这些结果主要来自共享名称/导出上下文；本次以精确源码入口为准。`find_best_task.rs` 中另有一个简化 `LogicalPlan { sequence: bool }` 及 `IterationMode::LogicalSequence`，没有直接持有本文件的 `LogicalSequence` 类型，不应把名称相同误判为直接调用边。

## 错误处理与边界

`PredicatePushDown`、`PruneColumns`、`DeriveStats` 在没有末孩子时都会返回 `PlannerError("LogicalSequence requires a main-query child")`。它们不 panic，也不会静默忽略请求。孩子方法的任何错误通过 `?` 原样传播；`DeriveStats` 只在孩子成功后才更新自身统计缓存，因此失败不会写入部分结果。

`Schema` 对空孩子采取不同策略：返回基类 schema。这允许默认构造节点参与 `Equals`/`Hash64` 测试，但也可能掩盖调用方尚未完成组装的临时状态。需要真正执行优化操作的调用方仍必须建立末孩子不变量。

`PredicatePushDown` 只维护主查询树。它返回孩子无法消费的谓词，责任仍在上层调用者；本方法不会把残差谓词复制到 CTE producer。`PruneColumns` 同样不尝试推导 producer 的内部列需求。扩展这些行为时必须先证明不会破坏 CTE 的共享、物化和依赖语义。

`PreparePossibleProperties` 接受由外部遍历准备的布尔切片，当前没有在本方法中验证切片长度等于孩子数。空切片结果为 `false`；长度不匹配不会报错，但可能代表调用方证据不完整。

当前最重要的系统边界是物理化缺口：Rust `physical_sequence.rs` 中相关完整实现仍被注释，不能用逻辑节点方法的成功来替代端到端 CTE Sequence 可执行性的验证。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄或网络资源。所有操作都在调用者持有的可变逻辑计划树上同步执行。

子节点使用 `Box<dyn LogicalPlan>` 独占所有权。`PredicatePushDownPlan` 可用返回的 replacement 原位替换末孩子；`push_down_sequence_owned` 则通过 `std::mem::replace` 和 `TakeChildren` 转移整棵子树的所有权，避免共享可变别名。`DeriveStats` 克隆 `StatsInfo` 后分别交给返回值与当前节点缓存，这里是值生命周期管理，不是跨线程共享。

`ContextRef` 的具体共享策略由 planner base crate 定义；本文件只克隆或保存其句柄，没有自行施加同步。Sequence 的孩子顺序与末孩子身份依赖独占可变访问维护，因此新增变换应优先使用 `TakeChildren`/`SetChildren` 或受控的 `Children_mut`，并在所有提前返回分支恢复合法树形。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/logical_sequence.go`。结构与核心意图一致：两者都只嵌入基础逻辑计划，规定前置孩子为 CTE producer、末孩子为主查询，并只对主查询执行 schema、谓词下推、列裁剪和统计语义。

主要差异如下：

- 初始化：Go 用 `plancodec.TypeSequence` 并将自身指针传给 `NewBaseLogicalPlan`；Rust 使用字面量 `"Sequence"`，当前基类构造签名不需要 self 指针。
- 空孩子：Go 的 `Schema`、谓词下推和列裁剪都直接用 `ChildLen()-1` 索引，依赖非空前置条件；Rust 的 `Schema` 回退基类，其余三个可失败方法返回明确 `PlannerError`。
- 谓词下推：Go 返回“残差谓词、新计划、自身/错误”，并显式 `SetChild`；Rust 的节点接口返回残差谓词，`PredicatePushDownPlan` 在 helper 内处理可选 replacement。
- 列裁剪：Go 可能返回替换后的逻辑计划；Rust trait 以原位可变对象操作，返回 `Result<()>`。
- 统计派生：Go 接收预先派生的 `childStats` 与 `reloads`，只读取末项，并根据末孩子 reload 标志决定结果；Rust 直接调用末孩子 `DeriveStats(reload)`，缓存其克隆并透传孩子的 `changed`。目标语义相近，但调用协议并非逐参数等价。
- 可能属性：Go 遍历 `PossiblePropertiesInfo`，忽略 `nil` 元素，并聚合 `HasTiFlash`；Rust 接收纯 `&[bool]`，不存在 nil，要求非空且全部为真。
- 哈希/相等：Go 生成代码按 `BaseLogicalPlan`（其中用 plan ID 区分 Sequence）计算；当前 Rust `hash64_equals_generated.rs` 按动态 schema 计算。Rust 独立测试明确覆盖了这一现状，因此它是可观察差异，不应描述为完全等价。
- 物理化：Go `physicalop/physical_sequence.go::ExhaustPhysicalPlans4LogicalSequence` 已按 Root/MPP 与 CTE producer 状态生成候选；Rust对应文件只保留注释参考，尚不能作为已接线能力。

Go 的计划构造入口 `logical_plan_builder.go` 会过滤无需物化的 CTE、按定义顺序创建 `LogicalCTE`、把主查询追加到最后并设置 `FlagPushDownSequence`。本次搜索未发现 Rust 逻辑计划 builder 中等价的直接构造入口；Rust 已确认的生产构造点是 Sequence 下推规则的重建路径。这是迁移状态说明，不代表 CTE 的其他 Rust 构造路径绝对不存在。

## 扩展指南

新增 Sequence 行为时，优先修改最窄的责任点：节点局部的主查询委托放在本文件；计划树位置变换放在 `optimizer_runtime.rs::push_down_sequence_owned`/`attach_pushed_sequence`；哈希或相等字段变更同步修改生成器来源及 `hash64_equals_generated.rs`，不要只手改生成结果；物理属性组合与执行接线属于 `physical_sequence.rs`，不应塞入逻辑节点。

必须保持以下兼容约束：孩子顺序稳定、末孩子始终为主查询、谓词和列裁剪不误触前置 producer、统计与可见 schema 对应主查询、TiFlash 聚合覆盖所有孩子。若增加 Sequence 自有字段，还应决定该字段是否参与 hash/equals、克隆、解释输出和计划缓存键。

测试必须放在独立测试文件，不要内嵌到 `logical_sequence.rs`。建议同步扩展：

- `optimizer_logical_entry_aster_unit_test.rs`：真实 `LogicalPlanRef` 上的 Sequence 下推位置、嵌套 Sequence 合并和孩子顺序。
- logicalop crate 下新建或复用独立 `logical_sequence_test.rs`：空孩子错误、只调用末孩子的谓词/列裁剪/统计、残差谓词与 replacement、TiFlash 全真/含假/空输入。
- `logicalop_test/hash64_equals_test.rs`：若字段或 schema 语义变化，验证 `Equals` 与 `Hash64` 一致。
- 物理接线完成时同步 `physical_sequence_test.rs`，覆盖 Root/MPP、排序属性和 CTE producer 状态组合。

性能风险主要来自误遍历或克隆全部 producer；当前热路径只定位末孩子，除统计值克隆外为常数级节点开销。正确性风险高于局部性能风险：任何孩子重排、主查询丢失或将谓词传播到 producer 都可能改变 CTE 语义。兼容性上还需对照 Go 的 reload、nil possible-properties、plan-ID hash 以及 MPP 候选规则。

## 验证依据

本说明基于以下本地证据，未运行 Cargo：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标 Rust 文件可读取且共 99 行。
- RustCodeGraph `explore "pkg/planner/core/operator/logicalop/logical_sequence.rs LogicalSequence"`：识别 `Schema`、`PredicatePushDown`、`PruneColumns`、`DeriveStats`、`PreparePossibleProperties` 以及优化器侧 Sequence 相关符号。
- RustCodeGraph `node --file pkg/planner/core/operator/logicalop/logical_sequence.rs`：核对目标文件全部定义和 trait 实现。
- RustCodeGraph `node` 核对 `base_logical_plan.rs` 的 `PredicatePushDownPlan`、`LogicalPlan`、`BaseLogicalPlan`、`NewBaseLogicalPlan` 与 `PreparePossibleProperties`；核对 `optimizer_runtime.rs` 的 `PushedSequence`、`push_down_sequence_owned`、`attach_pushed_sequence`；核对 `hash64_equals_generated.rs` 的 Sequence 哈希/相等实现。
- crate 与模块边界：`pkg/planner/core/operator/logicalop/Cargo.toml`、`pkg/planner/core/operator/logicalop/lib.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_sequence.go`、`pkg/planner/core/logical_plan_builder.go`、`pkg/planner/core/operator/physicalop/physical_sequence.go`。
- Rust 物理迁移边界：`pkg/planner/core/operator/physicalop/physical_sequence.rs`，其中逻辑 Sequence 穷举物理计划的主体仍为注释。
- 独立测试：`pkg/planner/core/optimizer_logical_entry_aster_unit_test.rs::push_down_sequence_moves_sequence_below_unary_main_query` 与 `pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.rs::TestLogicalSequence`。`pkg/planner/core/rule_push_down_sequence_test.rs` 还覆盖简化 `PlanNode` 模型的下推、非重复与嵌套顺序，但它不直接实例化本文件类型，故只作为规则意图的辅助证据。

结构验收应使用任务指定命令，要求目标文件存在且上述固定二级标题恰好 11 个。人工复核重点是：所有“已实现”陈述均可由当前 Rust 源码或独立测试支持；Go 完整实现和 Rust 注释骨架仅标为对照或迁移缺口。
