# `pkg/planner/core/operator/physicalop/physical_apply.rs`

## 文件定位

本文件定义物理计划节点 `PhysicalApply`，位于 `astersql-planner-core-operator-physicalop` crate。它表示仍保留相关性的子查询执行：外侧子计划每产生一行，内侧子计划都可能基于该行携带的相关列重新求值。节点复用 `PhysicalHashJoin` 的连接类型、条件、统计信息、子节点和 schema 基础设施，但在 `Init` 中明确使用 `plancodec::TypeApply`，因此不会在诊断和 `EXPLAIN` 中被误认成已经解相关的 HashJoin。

模块由 `physicalop/lib.rs` 的 `mod physical_apply` 装配并以 `pub use physical_apply::*` 再导出；同文件中的 `impl ConcretePhysicalOperator for PhysicalApply` 和 `impl_concrete_physical_plan!(PhysicalApply)` 将本文件的方法接到通用 `Plan`、`PhysicalPlan` 动态分发路径。crate 边界和直接依赖见同目录 `Cargo.toml`：本文件直接使用 `base`、`costusage`、`expression`、`property`、`cardinality` 与 `plancodec`。

## 核心职责

`PhysicalApply` 的职责集中在以下几件事：

1. 用 `PhysicalHashJoin` 保存 Apply 与普通物理连接共享的连接类型、连接条件、子树和 schema，同时保留 Apply 自身的缓存、并发、保序、相关列和禁止解相关标记。
2. 通过 `Init` 建立类型为 `TypeApply` 的基础物理计划，安装统计信息和两个孩子各自要求的物理属性。
3. 通过 `Attach2Task` 把两个孩子都转换成 Root 任务，克隆节点和孩子，重建连接输出 schema，形成新的 `RootTask`。
4. 通过 `ExtractCorrelatedCols` 与 `ResolveIndices` 管理尚未绑定的相关列，并针对 Apply 的连接条件求值语义重新解析等值条件索引。
5. 提供旧式局部成本公式 `GetCost`、计划成本入口、克隆和内存估算，供通用物理计划 trait 及 Cascades 适配层调用。

本文件只描述规划期节点，不执行子查询。真正的运行时 Apply 执行器不在此文件；这里产出的物理计划由后续执行器构建阶段消费。

## 主要符号

- `pub struct PhysicalApply`：唯一的生产类型。`PhysicalHashJoin` 是共享骨架；`CanUseCache` 表示规划器是否允许缓存内侧结果；`Concurrency` 是 Apply 并发配置；`KeepOrder` 表示并行执行时是否必须保持外侧行序；`OuterSchema` 保存外侧向内侧绑定的 `CorrelatedColumn`；`NoDecorrelate` 记录因提示而保留未解相关 Apply 的事实。
- `PhysicalApply::New(join)`：构造默认节点。缓存关闭、并发度为 `1`、不保序、相关列为空、未禁止解相关；它不初始化基础计划类型和统计信息，调用方还需执行 `Init`。
- `PhysicalApply::Init(ctx, stats, offset, props)`：创建 `NewBasePhysicalPlan(..., TypeApply, ...)`，设置统计信息与孩子所需属性，并写入内嵌 Join 的 `PhysicalSchemaProducer`。
- `PhysicalApply::Attach2Task(tasks)`：要求恰好两个孩子；转换为 Root、克隆孩子和自身、安装孩子、调用 `BuildPhysicalJoinSchema`，最后返回 `RootTask`。孩子数错误会 panic，克隆失败也通过 `expect` 终止。
- `PhysicalApply::PhysicalJoinImplement()`：恒为 `false`，保留 Go 中有意让 Apply 不满足普通 `PhysicalJoin` 接口的区分语义。
- `PhysicalApply::Clone(new_ctx)`：深克隆 HashJoin 骨架和每个相关列，按值复制标量配置，并传播 `expression::Error`。
- `PhysicalApply::ExtractCorrelatedCols()`：先复用 HashJoin 的相关列提取，再删除已经包含于第一个（外侧）孩子 schema 的列；无孩子时不会过滤。
- `PhysicalApply::GetCost(left, right, left_cost, right_cost)`：实现 Apply 的旧式局部代价公式，包括左右过滤条件、连接条件、半连接折扣和“每个外侧行重复执行内侧”的成本。
- `PhysicalApply::GetPlanCostVer1/Ver2(...)`：当前 Rust 实现直接委托内嵌 `PhysicalHashJoin` 的对应方法；这与同路径 Go 文件调用 Apply 专用成本函数不同，不能把 `GetCost` 的公式自动推断为这两个入口当前一定采用的完整公式。
- `PhysicalApply::MemoryUsage()`：累加 HashJoin 占用、三个布尔值、`OuterSchema` 向量及其容量、每个相关列的堆内存增量。当前公式没有单独计入 `Concurrency: i32`，也没有 nil 接收者语义。
- `PhysicalApply::ResolveIndices()`：先解析 HashJoin，再按 `UniqueID` 去重并把 `OuterSchema` 解析到外侧 schema；随后把 `EqualConditions` 和 `NAEqualConditions` 重新解析到左右 schema 的合并结果。

## 执行流程

物理节点的主要生成路径有两条。

传统物理计划枚举路径位于 `base_physical_plan.rs`：逻辑 Apply 先被转换为 HashJoin 骨架和两个孩子属性；外侧属性继承排序并禁止 cop 下推，内侧属性要求无限期望行数；随后调用 `PhysicalApply::New(...).Init(...)`，复制逻辑节点的 `CorCols`，基于外侧行数和相关列 NDV 决定 `CanUseCache`，并传播 `NoDecorrelate`。

旧 Cascades 路径位于 `pkg/planner/cascades/old/implementation_rules.rs::ImplApply`：`Match` 要求父排序列全部来自外侧 schema；`OnImplement` 构造 HashJoin，复制相关列，初始化 Apply 和输出 schema，再包装为 `NewApplyImpl(PlanAdapter<PhysicalApply>)`。同文件的 `ApplyCostPlan for PlanAdapter<PhysicalApply>` 把 `SelfCost` 转发给本文件的 `GetCost`。

节点接入任务树时，`Attach2Task` 执行如下步骤：

1. 将输入向量严格拆成左右两个 `Task`。
2. 使用当前计划上下文把两者转换为 `RootTask`。
3. 克隆左右计划，避免直接复用并改写原任务持有的节点。
4. 用同一上下文深克隆 Apply，并安装两个克隆孩子。
5. 根据 JoinType 和完整 Apply 节点调用 `BuildPhysicalJoinSchema`，写回 `PhysicalSchemaProducer`。
6. 用该 Apply 创建新的 `RootTask`。

通用物理计划遍历通过 `ConcretePhysicalOperator` 路由到本文件的索引解析、内存统计、相关列提取和两版成本入口；`flat_plan.rs` 还会将 Apply 的 `InnerChildIdx` 用于标注 Build/Probe 侧。

## 数据与状态

`PhysicalApply` 自身不持有运行时行数据或缓存内容，只保存规划配置。可变状态主要位于内嵌的 `BasePhysicalPlan`（上下文、统计信息、孩子、孩子属性）、`BasePhysicalJoin`（JoinType、左右/其他条件、InnerChildIdx）和 `PhysicalHashJoin`（等值条件与 null-aware 等值条件）。

`OuterSchema` 的顺序和身份以 `CorrelatedColumn.column.UniqueID` 为关键。`ResolveIndices` 使用“反转、保留首次、再反转”的实现，使重复 `UniqueID` 保留原始序列中的最后一个对象，这与 Go 先写入 map 时“后值覆盖前值”的选择一致；但 Rust 同时保持了幸存列之间的确定性相对顺序，而 Go map 遍历顺序不稳定。相关列随后只对外侧孩子 schema 解析。

连接等值条件是 Apply 的特殊状态：HashJoin 的首次索引解析按普通连接路径完成后，本文件再次把 `EqualConditions` 和 `NAEqualConditions` 对合并 schema 解析，因为 Apply 在连接结果上求值这些条件。解析结果必须仍是 `ScalarFunction`，否则会触发 `expect`。

`CanUseCache`、`Concurrency`、`KeepOrder` 和 `NoDecorrelate` 在本文件中仅被初始化、克隆或计量，并不在这里实施缓存、并发、重排或解相关策略。特别是当前 `GetCost` 并未读取 `Concurrency`；不能依据字段注释推断本文件已经按并发度分摊成本。

## 依赖与调用关系

上游构造与消费关系：

- `physicalop/base_physical_plan.rs` 从逻辑 Apply 生成 `PhysicalApply`，负责缓存资格、相关列和 `NoDecorrelate` 的规划期赋值。
- `planner/cascades/old/implementation_rules.rs::ImplApply` 是另一条构造路径，并通过 `ApplyCostPlan` 调用 `GetCost`。
- `physicalop/lib.rs` 再导出类型，并把 `resolve_operator`、`memory_operator`、`correlated_operator`、`cost_v1`、`cost_v2` 和 protobuf 转换接到通用 trait；protobuf 路径目前复用 `PhysicalHashJoin::ToPB`。
- `planner/core/flat_plan.rs` 识别 `PhysicalApply` 并依据 `InnerChildIdx` 标注两侧角色。
- `planner/core/plan_cacheable_checker.rs` 将展开后的 `PlanKind::Apply` 判定为不可缓存计划；这和字段 `CanUseCache` 并不矛盾：前者是整个物理计划缓存，后者是执行 Apply 时复用内侧结果。

本文件的直接下游依赖包括：`base::{ContextRef, Plan, PhysicalPlan, Task}` 提供上下文、计划树和任务接口；`PhysicalHashJoin` 提供共享 Join 行为；`property` 提供统计信息、物理属性和任务类型；`expression` 提供相关列、schema 合并、索引解析和错误类型；`cardinality::SelectionFactor` 用于旧成本模型过滤选择率；`costusage::{CostVer2, PlanCostOption}` 定义成本入口；`plancodec::TypeApply` 保持节点身份。

RustCodeGraph 对 `PhysicalApply` 类型没有返回可靠的 Rust 上游调用边，且通用方法名的 callees 结果混入同名 Go/Rust 符号；因此上述装配与调用关系由索引的文件源码视图加 `rg` 精确引用结果交叉核对，不把噪声边作为结论。

## 错误处理与边界

- `New` 接受尚未初始化的 HashJoin 骨架；只有执行 `Init` 后才具备正确的 `TypeApply` 基础计划、统计信息和孩子属性。
- `Attach2Task` 的契约是恰好两个孩子。数量不符会以 `PhysicalApply requires exactly two child tasks` panic，而不是返回错误。
- `Attach2Task` 中孩子或 Apply 克隆失败会被 `expect` 转成 panic；它当前没有把 `expression::Error` 暴露给调用者。
- `ExtractCorrelatedCols` 对缺少外侧孩子采取保守行为：返回 HashJoin 提取的全部相关列。
- `ResolveIndices` 在缺少外侧孩子时，完成 HashJoin 解析后直接成功返回；只有一个孩子时会解析相关列，但跳过合并 schema 和 Apply 等值条件的第二次解析。
- 相关列或条件找不到对应 schema 列时，`expression::Error` 通过 `?` 向上传播。合并两个现存孩子 schema 失败、或已解析等值条件不再是 `ScalarFunction` 被视为内部不变量破坏并 panic。
- `GetCost` 使用浮点行数和成本，没有在此处校验负值、NaN、无穷或溢出；调用者必须提供有效的统计量。
- `MemoryUsage` 是估算而非分配器精确值；它依赖向量 `capacity`，且当前 Rust 与 Go 的对象布局和指针模型不同，数值不应跨语言逐字比较。

## 并发与资源生命周期

本类型没有锁、原子变量、通道、后台任务或显式 I/O 资源。`Concurrency` 和 `KeepOrder` 是传给后续执行阶段的规划元数据；本文件既不创建 worker，也不维护重排缓冲区。Go 注释说明 `KeepOrder` 为真时执行器需要用重排缓冲保证外侧输入顺序，但这只是该字段的跨阶段契约，不代表本文件实现了缓冲。

所有权边界较清晰：`New` 取得 HashJoin 所有权；`Init` 消费并返回 `self`；`Clone` 生成独立的 HashJoin 和 `OuterSchema`；`Attach2Task` 克隆孩子与自身后交给新的 `RootTask`。因此规划树附着不会有 Rust 层面的共享可变别名。计划上下文使用 `ContextRef` 共享，并由引用计数生命周期管理。

缓存生命周期也不在这里：`CanUseCache` 只表示规划器允许后续执行器缓存内侧结果，本类型不拥有缓存对象，也不负责配额申请、淘汰或释放。

## 与 Go 版本的对应关系

直接对照文件是同目录 `physical_apply.go`。类型字段、`Init`、`PhysicalJoinImplement`、深克隆、相关列提取、旧局部成本、内存估算和索引解析都有明确对应。

保持一致的关键语义包括：

- Apply 复用 HashJoin 数据结构，但 `Init` 必须建立 `TypeApply`。
- Apply 有两个孩子，附着到 Root，并按 JoinType 重建输出 schema。
- 外侧 schema 已提供的相关列不继续向上传递。
- 旧成本公式为条件 CPU 成本加外侧成本，再加“外侧过滤后行数 × 内侧成本”；半连接类连接条件乘 `0.5`。
- `OuterSchema` 按 `UniqueID` 去重，并对外侧 schema 解析；Apply 等值条件需要对合并 schema 再解析。

当前可见差异与迁移限制：

- Go `Attach2Task` 直接重用并改写接收者，并复制左右 `RootTask` 的 warnings；Rust 克隆接收者和孩子，当前返回的 `RootTask::New(..., None)` 没有在本方法中显式合并 warnings。
- Go `GetPlanCostVer1/Ver2` 由 `planner/core` 中 Apply 专用函数递归计算孩子成本，其中 v2 明确将内侧成本乘外侧行数；当前 Rust 本文件把这两个入口委托给 `PhysicalHashJoin`。只有 `GetCost` 本身和 `ApplyCostPlan` 适配路径体现 Apply 专用旧公式，故不能宣称两版计划成本已与 Go 完全一致。
- Go `MemoryUsage` 支持 nil 接收者并按指针切片计量；Rust 不存在 nil `&self`，按内联 `CorrelatedColumn` 向量容量计量，而且未显式加入 `i32 Concurrency`。
- Go 去重通过 map 保留最后一个同 ID 指针，但输出顺序不保证；Rust 保留最后一个对象且保持确定顺序。
- Go 假定两个孩子都存在；Rust 对 `ResolveIndices` 的零/单孩子情况有提前返回保护，但 `Attach2Task` 仍严格要求两个孩子。

相关 Rust 测试位于独立文件 `physical_apply_test.rs`，符合源文件与测试分离约束。当前没有发现同目录专门针对 `PhysicalApply` 的 Go 单测；Go 行为依据生产实现及 planner/core 的 Apply 专用辅助函数核对。

## 扩展指南

- 新增或改变 Apply 字段时，应同步更新 `New` 默认值、`Clone`、`MemoryUsage`，以及 `physicalop/lib.rs` 中必要的 trait 路由；若字段来自逻辑 Apply，还要检查 `base_physical_plan.rs` 和 `ImplApply::OnImplement` 两条构造路径。
- 修改相关列绑定时，优先改 `ExtractCorrelatedCols` 或 `ResolveIndices`，并在独立的 `physical_apply_test.rs` 增加重复 `UniqueID`、缺列、零/单/双孩子和等值条件合并 schema 的用例。不要把测试内嵌进生产文件。
- 修改任务附着时，需维持“恰好两个孩子、两侧转 Root、重建输出 schema”的不变量，并重点核对 warnings 是否应像 Go 一样合并，以及克隆语义是否符合调用方预期。
- 修改成本时，必须同时审查 `GetCost`、`GetPlanCostVer1/Ver2`、`ApplyCostPlan for PlanAdapter<PhysicalApply>` 和 Go 的 `getCost4PhysicalApply`、`getPlanCostVer14PhysicalApply`、`getPlanCostVer24PhysicalApply`。当前 HashJoin 委托差异是首要兼容风险；不能仅调整测试期望来掩盖公式差异。
- 修改并发或保序行为时，本文件只适合承载规划属性；还需定位真正的执行器 worker、重排缓冲和缓存实现，验证 `Concurrency`、`KeepOrder` 与 `CanUseCache` 被消费。不要在没有执行器证据时宣称功能生效。
- 修改计划身份或序列化时，必须保持 `TypeApply`、`PhysicalJoinImplement == false` 和通用 trait 的 `PhysicalApply` 分支，否则可能影响 EXPLAIN、类型分发、计划缓存判断和执行器选择。
- 性能风险主要来自外侧行数放大内侧成本、缓存命中率判断、相关列去重和条件重复解析；兼容风险主要来自 schema 列索引、半连接折扣、孩子角色以及 Go/Rust 成本入口差异。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/planner/core/operator/physicalop/physical_apply.rs`，逐项核对 `PhysicalApply` 及其全部 11 个方法。
- crate 与装配：`pkg/planner/core/operator/physicalop/Cargo.toml`；`physicalop/lib.rs` 的模块声明、再导出、`ConcretePhysicalOperator` 实现和 `impl_concrete_physical_plan!`。
- 上游构造：`physicalop/base_physical_plan.rs` 中 `PhysicalApply::New(...).Init(...)`、相关列复制、缓存资格和 `NoDecorrelate` 赋值；`pkg/planner/cascades/old/implementation_rules.rs` 中 `ImplApply` 与 `ApplyCostPlan`。
- 下游消费：`pkg/planner/core/flat_plan.rs` 的 Apply 孩子角色标注；`pkg/planner/core/plan_cacheable_checker.rs` 的 `PlanKind::Apply` 排除规则。
- Go 对照：`physical_apply.go`；`pkg/planner/core/task.go::attach2Task4PhysicalApply`；`plan_cost_ver1.go::{getCost4PhysicalApply,getPlanCostVer14PhysicalApply}`；`plan_cost_ver2.go::getPlanCostVer24PhysicalApply`。
- Rust 辅助对照：`pkg/planner/core/task.rs::attach2Task4PhysicalApply` 与 `pkg/planner/core/plan_cost_ver1.rs::getCost4PhysicalApply`，用于确认仓库内扁平 `PlanNode` 路径也把 Apply 视为逐外侧行执行内侧的节点，但它们不是本类型方法的直接实现。
- 独立测试：`physical_apply_test.rs` 的 `resolve_indices_deduplicates_and_resolves_outer_schema_columns` 验证保留最后重复列并解析索引；`get_cost_matches_go_filter_and_semi_join_accounting` 验证过滤与半连接折扣；`attach_to_task_builds_the_join_output_schema` 验证二元输出 schema。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query PhysicalApply --kind struct` 定位 Rust/Go 两个类型；`node physical_apply.rs::PhysicalApply` 与按文件 `node` 核对源码；`callers` 未返回 Rust 类型上游，`callees` 对同名方法存在跨语言噪声，因此调用关系改用精确源码引用补证，未采用噪声边。

本任务是纯文档分析，未运行 Cargo 或代码测试。结构验证只检查目标文档存在且恰好具有计划要求的十一个二级标题；人工复核还需确认以上陈述都能回溯到列出的符号和文件，并且未把字段意图误写成已实现的运行时行为。
