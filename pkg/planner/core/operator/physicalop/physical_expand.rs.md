# `pkg/planner/core/operator/physicalop/physical_expand.rs`

## 文件定位

本文件位于物理计划算子 crate `astersql-planner-core-operator-physicalop` 中，由同目录 [`lib.rs`](./lib.rs) 以 `pub mod physical_expand` 导出。它用一个轻量 Rust 数据模型描述 Expand 物理算子：把同一输入行按多个 grouping level 投影成多行，供 `ROLLUP`、`CUBE`、`GROUPING SETS` 等聚合语义使用。crate 归属和本地依赖以 [`Cargo.toml`](./Cargo.toml) 为准；本文件实际只直接依赖同 crate 的 [`physical_common_plans.rs`](./physical_common_plans.rs)。

当前接线状态需要与 Go 主实现区分：仓库搜索与 RustCodeGraph 均只发现 `exhaust_physical_expand` 被独立测试直接调用，未发现生产 Rust 调用者；[`task.rs`](../../task.rs) 中另有基于通用 `PlanNode`/`PlanKind::Expand` 的任务附着路径，但没有直接消费本文件的 `PhysicalExpand`。因此，本文件已经表达并测试若干 Go 对齐规则，但不能据此断言它已完整接入 Rust 优化器主链。

## 核心职责

- `PhysicalExpand` 保存每个展开层的表达式、生成列名、grouping id/位置、输出 schema 和唯一子计划。
- `memory_usage` 给出结构体、投影表达式数量和生成列字符串容量的近似内存占用；它不是递归、精确的堆内存统计。
- `explain_info` 生成与 Go `explainInfoV2` 形状相近的 level projection 与 schema 摘要。
- `resolve_indices` 按孩子 schema 校验所有 level 表达式中的列引用。
- `to_pb` 生成 crate 内部的 `ExpandExecutor` 描述；当前不是 `tipb::Executor` 序列化，也不因存储类型改变形状。
- `exhaust_physical_expand` 根据排序、任务类型和 MPP 分区要求枚举物理候选，并返回是否允许上层继续处理/enforce 的布尔值。

本文件不负责生成 grouping levels。生成 NULL 补位、grouping id 等逻辑位于逻辑 Expand；直接证据见 [`logical_relational_aster_unit_test.rs`](../logicalop/logical_relational_aster_unit_test.rs) 的 `expand_generates_null_filled_rollup_levels_and_grouping_ids`，以及 [`optimizer_logical_entry_aster_unit_test.rs`](../../optimizer_logical_entry_aster_unit_test.rs) 的 `resolve_expand_generates_rollup_level_projections`。

## 主要符号

- `pub struct PhysicalExpand`：算子本体。
  - `levels: Vec<Vec<PhysicalExpr>>`：外层是一组展开层，内层是该层对输出列的投影表达式。
  - `generated_column_names: Vec<String>`：额外生成列的名称，例如 `grouping_id`。
  - `grouping_ids: Vec<u64>` 与 `grouping_pos: Vec<usize>`：保存 grouping 元数据；当前文件只转交这些值，不校验彼此长度或范围。
  - `schema: Vec<i64>`：输出列 ID；同时用于 EXPLAIN 和候选计划输出。
  - `child: Option<PhysicalPlanNode>`：唯一孩子；`None` 仍是可构造状态，主要被单元测试使用。
- `PhysicalExpand::{memory_usage, explain_info, resolve_indices, to_pb}`：内存估算、解释、列解析和执行描述转换入口。
- `fn format_expr`：私有 EXPLAIN 格式化器。列、NULL、其他常量、标量函数、相关列和 DEFAULT 分别采用不同摘要；标量函数只显示函数名，不展开参数。
- `pub struct ExpandExecutor`：下推描述的轻量值对象，克隆保存 levels、生成列名及 grouping 元数据。
- `pub fn exhaust_physical_expand(...) -> (Vec<PhysicalPlanNode>, bool)`：候选枚举入口。返回值第一项为候选，第二项沿用 Go 枚举接口的“是否正常完成/是否可继续处理”语义；特别地，有排序要求时返回 `(空, false)`，表示 Expand 本身不保序而上层可考虑 Sort enforcer。

所有公开数据类型均派生 `Clone`、`Debug`、`PartialEq`；`PhysicalExpand` 另派生 `Default`，所以调用方可以构造尚无孩子、schema 或 level 的中间状态。

## 执行流程

候选枚举 `exhaust_physical_expand` 的决策顺序如下：

1. 若 `property.sort_items` 非空，立即返回空候选和 `false`。Expand 复制输入行后不承诺保留所需顺序；[`physical_expand_test.rs`](./physical_expand_test.rs) 的 `ordered_property_allows_sort_enforcer_like_go` 固定了该返回形状。
2. 仅接受 `TaskType::Root` 或 `TaskType::Mpp`；`Cop` 要求返回空候选和 `true`。
3. 对 MPP 要求，仅接受 `PartitionType::Any`；Hash/Broadcast/Single 等具体分区要求返回空候选和 `true`，因为当前模型不声明 Expand 保留输入分区。
4. 闭包 `make_plan` 把算子输出 schema、可选孩子和统计复制到 `PhysicalPlanNode { kind: PhysicalKind::Expand, ... }`，并为孩子写入一个仅覆盖 `task_type` 的默认物理属性。
5. 明确要求 MPP 时只生成一个 MPP 候选；Root 请求则按 `Mpp, Cop, Mpp, Root` 生成四个候选。源码注释说明简化模型把 Go 的 `CopSingleRead`/`CopMultiRead` 合并为 `Cop`，但实际数组只有一个 `Cop`、两个 `Mpp`；因此只能把该顺序记录为当前已测试行为，不能视为 Go 四种孩子任务的一一映射。

其他方法的流程较短：`resolve_indices` 先从 `child` 取 schema（无孩子时使用空切片），再按 level/表达式展平顺序调用 `PhysicalExpr::resolve_indices`，遇到第一个错误即停止；`to_pb` 仅克隆四组执行元数据；`explain_info` 先格式化每个 level，再追加输出 schema。

## 数据与状态

`PhysicalExpand` 是普通可克隆值，没有内部可变性或隐藏全局状态。候选枚举按值取得 `expand`，每个候选克隆 `schema`、`child` 和 `stats`；因此候选之间不共享可变计划节点。`PhysicalPlanNode.id` 在这里固定为 `0`，后续若进入统一计划树，应由公共的 ID 重分配流程处理。

关键数据不变量目前主要依赖上游，而非本文件强制检查：

- `levels` 中引用的列应存在于孩子 schema；只有显式调用 `resolve_indices` 才会验证。
- `generated_column_names`、`grouping_ids`、`grouping_pos` 与 level/schema 的对应关系没有在构造或 `to_pb` 时验证。
- `child` 在枚举中可缺省，此时生成无孩子的 `PhysicalPlanNode`；真正执行/附着通常应有一个孩子。通用任务附着函数 `attach2Task4PhysicalExpand` 在没有子任务时会产生 invalid task，但它使用另一套 `PlanNode` 类型。
- `memory_usage` 使用表达式“元素个数 × `size_of::<PhysicalExpr>()`”，没有递归计入标量函数参数、常量字符串、`schema`、grouping vectors、孩子计划或 vector 额外容量，故只能作为局部近似量。

## 依赖与调用关系

直接下游依赖全部来自 `physical_common_plans`：`PhysicalExpr::resolve_indices` 递归校验列/相关列是否存在于输入 schema；`PhysicalProperty` 提供任务、排序与分区要求；`PhysicalPlanNode`、`PhysicalKind::Expand`、`Stats` 承载候选结果；`Datum` 和 `TaskType` 分别服务解释文本和候选/PB 接口。

已验证的上游关系如下：

- [`lib.rs`](./lib.rs) 公开本模块，并在 `#[cfg(test)]` 下装配独立测试模块。
- [`physical_expand_test.rs`](./physical_expand_test.rs) 直接构造 `PhysicalExpand`，覆盖排序拒绝、Root 候选顺序、MPP 分区拒绝、存储类型无关的 PB 形状及 EXPLAIN 文本。
- 逻辑层测试验证 levels/grouping ids 的来源，但没有直接调用本物理文件。
- 全仓 Rust 搜索未发现测试之外对 `exhaust_physical_expand` 或 `ExpandExecutor` 的引用。RustCodeGraph 对 `expand` 这个常见名称给出的 blast radius 混合了测试构造器和逻辑 Expand 测试，因此不能把那些结果误作本函数的生产调用边。

Go 主链更完整：[`physical_expand.go`](./physical_expand.go) 的 `ExhaustPhysicalPlans4LogicalExpand` 从 `LogicalExpand` 构造物理候选，`PhysicalExpand.Attach2Task` 进入任务附着，`ToPB`/`toPBV2` 生成 tipb executor；其文件级调用关系还连接到 Go `pkg/planner/core/task.go`。这些是语义对照证据，不是 Rust 已接线证据。

## 错误处理与边界

`resolve_indices` 是当前唯一会按数据内容失败的方法。`PhysicalExpr::Column` 或 `CorrelatedColumn` 的 ID 不在孩子 schema 中时，它返回 `"column {id} is absent from child schema"`；标量函数参数递归验证；常量与 DEFAULT 不需要列解析。由于无孩子会使用空 schema，任何列引用都会失败，而纯常量 levels 可以成功。

`to_pb` 的签名返回 `Result<ExpandExecutor, String>`，但当前实现没有失败分支，也未验证输入一致性；调用方不应把这个 `Ok` 保证外推到未来的真实 protobuf 转换。`exhaust_physical_expand` 用空候选表达不可满足的属性，不返回错误。`explain_info` 对空 levels 生成 `level-projection:; schema: [...]`，对未知/复杂表达式采用摘要格式，不保证可反解析。

与 Go 相比，当前 Rust 边界还包括：没有 session 级 `IsMPPAllowed` 开关；没有按 `expected_count` 缩放统计；没有 `tipb` 表达式编码失败；没有 TiFlash 时递归编码 child/ExecutorId；也没有旧版 `GroupingSets` 分支。扩展时应把这些列为显式迁移项，而不是依赖现有无错路径。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、异步任务、事务或 I/O。所有方法同步执行；`&self` 方法只读并克隆输出，`resolve_indices(&mut self)` 独占修改 levels。候选计划拥有克隆后的孩子和统计，不引入引用生命周期或跨线程共享。

主要资源成本来自克隆：`to_pb` 深克隆 level 表达式与字符串；Root 候选枚举最多四次克隆整棵可选孩子计划。如果孩子树较大，这会放大 CPU 与内存开销。当前测试只验证值语义，没有并发测试或大计划性能基准；在接入主链前，应评估是否改用共享/所有权转移或延迟构造。

## 与 Go 版本的对应关系

Rust 字段大体对应 Go `PhysicalExpand` 的 V2 路径：`levels` 对应 `LevelExprs`，`generated_column_names` 对应 `ExtraGroupingColNames`，`schema`/`child` 对应 `PhysicalSchemaProducer` 管理的输出与孩子。Rust 额外显式保存 `grouping_ids`、`grouping_pos` 并转交 `ExpandExecutor`；Go `Expand2` protobuf 在本文件所读路径中主要写 `ProjExprs` 与 `GeneratedOutputNames`。

对齐之处包括：有排序要求时返回空候选和 `false`；仅支持 Root/MPP 请求；MPP 不接受具体分区要求；Root 请求枚举 MPP 与本地执行所需的多种孩子任务；EXPLAIN 使用 `level-projection:...; schema: [...]` 形状；levels 相对孩子 schema 解析索引。

尚未等价之处包括：

- Go 受 session `IsMPPAllowed()` 控制，Rust 无上下文参数并始终为 Root 请求生成 MPP 候选。
- Go 区分 `CopSingleReadTaskType` 与 `CopMultiReadTaskType`；Rust 类型系统只有一个 `Cop`，但当前 Root 候选实际为 `Mpp, Cop, Mpp, Root`。它保持了四候选数量，却不是 Go 任务类型的一一映射。
- Go `Init` 设置计划上下文、类型、属性与按 expected count 缩放的统计；Rust 直接组装轻量节点，原样克隆 `Stats`。
- Go `ToPB`/`toPBV2` 生成真正的 `tipb::Executor`，做表达式转换，并仅在 TiFlash 下嵌入 child 与 executor ID；Rust `to_pb` 忽略 `_store`，总是返回相同的内部结构。测试 `protobuf_shape_is_independent_of_non_tiflash_store_selection` 明确固定的是当前简化行为。
- Go 同时保留旧版 `GroupingSets`/`GroupingIDCol` 和 V2 `LevelExprs` 路径；Rust 只有 levels 路径。
- Go `Clone`、`MemoryUsage`、`ResolveIndices` 还复用基类行为；Rust 没有对应完整基类，并且内存估算更窄。

Go 回归证据还包括 [`logical_plans_test.go`](../../logical_plans_test.go) 对优化后 level projections、NULL 补位、grouping id、nullable schema 与输出名的验证；这些主要约束上游逻辑 Expand，不能替代本文件的物理候选测试。

## 扩展指南

安全扩展应按职责落点：

- 新增属性约束或候选类型时修改 `exhaust_physical_expand`，同步 [`physical_expand_test.rs`](./physical_expand_test.rs) 的候选数量、顺序、布尔返回值和 required property 断言；尤其要核对 Go `ExhaustPhysicalPlans4LogicalExpand` 的增量，而不是仅让 Rust 测试通过。
- 接入真实 optimizer 时，应新增生产调用点，并验证从 `LogicalExpand` 的 `levels`、grouping ids/positions、schema、stats、child 到本结构的完整映射；不要把通用 `task.rs` 的 `PlanKind::Expand` 分支当作已经完成该转换。
- 丰富表达式显示时修改 `format_expr` 并增加嵌套 scalar、非 NULL 常量、相关列和 DEFAULT 的独立测试；注意 EXPLAIN 兼容性与日志脱敏需求。
- 强化 protobuf 下推时修改/替换 `to_pb` 与 `ExpandExecutor`，补充表达式转换失败、TiFlash child 嵌入、executor ID、存储类型差异的测试，并对照 Go `ToPB`/`toPBV2`。
- 增加构造校验时，应明确 levels、生成列名、grouping ids/positions 的长度和位置契约，并为无孩子、空 levels、缺列、嵌套 scalar 缺列写回归测试。
- 修改内存估算时应覆盖所有 vector 容量、嵌套表达式和孩子所有权，并评估四候选深克隆的性能风险。

测试必须继续放在独立的 [`physical_expand_test.rs`](./physical_expand_test.rs)，不要内嵌进生产源文件。兼容性风险集中在 EXPLAIN 文本、候选顺序/布尔语义、任务类型映射和 protobuf 字段；性能风险集中在表达式及孩子计划的重复深克隆。

## 验证依据

- 目标源码：[`physical_expand.rs`](./physical_expand.rs)，核对 `PhysicalExpand`、四个方法、`format_expr`、`ExpandExecutor` 与 `exhaust_physical_expand` 的全部实现。
- crate 与装配：[`Cargo.toml`](./Cargo.toml) 确认 crate 名、`lib.rs` 入口、`autotests = false` 与本地依赖；[`lib.rs`](./lib.rs) 确认公开模块和独立 `#[cfg(test)]` 测试装配。
- 共享类型：[`physical_common_plans.rs`](./physical_common_plans.rs) 的 `PhysicalExpr::resolve_indices`、`TaskType`、`PartitionType`、`PhysicalProperty`、`Stats`、`PhysicalKind::Expand`、`PhysicalPlanNode`。
- Rust 测试：[`physical_expand_test.rs`](./physical_expand_test.rs) 的五个测试；逻辑来源由 [`logical_relational_aster_unit_test.rs`](../logicalop/logical_relational_aster_unit_test.rs) 和 [`optimizer_logical_entry_aster_unit_test.rs`](../../optimizer_logical_entry_aster_unit_test.rs) 的 Expand 用例补证。
- Go 对照：[`physical_expand.go`](./physical_expand.go) 的 `PhysicalExpand`、`ExplainInfo`、`ResolveIndices`、`ToPB`/`toPBV2`、`ExhaustPhysicalPlans4LogicalExpand`；[`logical_plans_test.go`](../../logical_plans_test.go) 的 rollup level projection 回归。
- 任务附着边界：[`task.rs`](../../task.rs) 的 `attach2Task4PhysicalExpand` 与 `attach2Task`，证明通用 Expand task 路径存在，同时其类型与本文件轻量结构不同。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`explore "pkg/planner/core/operator/physicalop/physical_expand.rs PhysicalExpand"`、`query PhysicalExpand`、`query exhaust_physical_expand --kind function`、`query ExpandExecutor` 用于确认符号与 blast radius。精确 callers/callees 查询未返回额外边，因此又以全仓 `rg` 复核直接引用，未发现测试之外的本文件入口。
- 按本计划不运行 Cargo；交付验证只执行任务指定的 11 章节结构检查，并人工复核以上结论均区分当前事实、Go 对照与未接线边界。
