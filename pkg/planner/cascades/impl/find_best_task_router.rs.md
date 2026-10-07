# `pkg/planner/cascades/impl/find_best_task_router.rs`

## 文件定位

本文件属于独立 crate `astersql-planner-cascades-impl`，crate 根为同目录的 `lib.rs`，并由该文件通过 `pub use find_best_task_router::*` 公开重导出。根工作区在 `Cargo.toml` 中把 `pkg/planner/cascades/impl` 列为成员，并以 `facade_planner_cascades_impl` 指向它；该 crate 自身的 `Cargo.toml` 没有声明外部依赖，因此本文件只依赖标准库和同 crate 的 `impl_and_cost` 模块。

它是 Cascades Memo 中“为一个逻辑 GroupExpression 在指定物理属性下选择最低代价 Task”的适配层：一侧用 `LogicalPlan`、`RoutePhysicalPlan`、`RoutedGroupExpression` 和 `RouteGroup` 表达路由所需的最小模型，另一侧复用 `impl_and_cost::{PhysicalPlan, PhysicalProperty, Task, TaskType}` 的任务与代价接口。仓库搜索未发现测试以外对 `route_find_best_task`、`InstallDefaultCascadesFindBestTaskRouter` 或其 Go 风格别名的调用，因此当前可确认的是 crate 内能力与测试接线，不能据此声称它已经进入 Rust SQL 优化主链。

## 核心职责

1. 定义路由所需的逻辑计划和物理计划扩展接口：逻辑计划能够报告具体类型并穷举物理实现，物理实现能够声明每个孩子所需的属性并把子 Task 挂接成新 Task。
2. 用 `RouteGroup` 保存一组等价逻辑表达式，并按 `(TaskType, expected_count.to_bits())` 缓存已求得的最优 Task。
3. 对每个逻辑表达式穷举候选物理计划，递归求解输入 Group，按“本地物理计划代价 + 所有子候选代价”比较候选。
4. 用进程级 `OnceLock<FindBestTaskHandler>` 提供一次性 handler 注入点，隔离“如何把 GroupExpression 路由到具体 FindBestTask 实现”的依赖方向。
5. 提供 snake_case API 及 `InspectLogicalPlanRoute`、`ExhaustPhysicalPlans4GroupExpression`、`FindBestTask4GroupExpression` 等 Go 风格兼容入口。

本文件没有实现完整 Go Cascades 优化器的 cost limit、InvalidTask、hint 优先级、task-type 满足性检查或全量具体逻辑算子分派；这些不能从此文件的简化接口推断为已支持。

## 主要符号

- `LogicalPlan`：`Any + Send + Sync` trait。`as_any` 提供具体 Rust `TypeId`，`plan_type` 提供稳定的可读类型名，`exhaust_physical_plans(&PhysicalProperty)` 返回当前属性下的候选 `RoutePhysicalPlan`。
- `RoutePhysicalPlan: PhysicalPlan`：通过 `child_required_properties` 暴露逐孩子属性要求，通过消费 `Box<Self>` 的 `attach_to_task(children)` 生成 Task；消费语义保证同一候选不会被重复挂接。
- `GroupRef = Arc<Mutex<RouteGroup>>`：允许多个表达式共享输入 Group，并为递归缓存提供内部可变性。
- `RoutedGroupExpression`：持有一个 `Box<dyn LogicalPlan>` 和有序的输入 `Vec<GroupRef>`；`new` 不额外验证输入数，匹配检查发生在候选枚举时。
- `RouteGroup`：持有 `Vec<Arc<RoutedGroupExpression>>` 和私有 `best_tasks`。`cached_task`、`set_best_task` 都通过 `Task::copy_task` 避免把缓存中的 Task 所有权交给调用者。
- `LogicalPlanRoute`：携带 `is_group_expression`、具体 `wrapped_type_id` 和字符串 `wrapped_plan_type`。`inspect_logical_plan_route` 对普通逻辑计划固定返回 `is_group_expression = false`；对 `RoutedGroupExpression` 构造的路由身份为 `true`。
- `FindBestTaskHandler` 与 `FIND_BEST_TASK_HANDLER`：函数指针类型及一次性全局槽。它们不捕获环境，安装后不可替换或卸载。
- `InstallFindBestTaskRouterError::HandlerAlreadyInstalled`：二次安装的唯一专用错误，`Display` 文本为 `cascades find-best-task handler has already been installed`。
- `PlannedTask`：内部候选，联合保存 Task 与用于当前递归比较的累计 `f64` 代价。
- `cached_group_task`：复制缓存 Task，再从其 `plan()` 重新计算指定 `TaskType` 的代价。
- `find_best_task_for_group`：Group 级缓存、表达式枚举、最优候选选择和回写入口。
- `find_best_task_for_expression`：内部路由入口；有 handler 时委托 handler，否则调用默认枚举实现，并从返回 Task 的计划读取代价。
- `find_best_task_for_expression_default`：默认递归实现，按严格小于号更新最优候选。
- `exhaust_physical_plans_for_group_expression`：薄委托，仅调用被包装逻辑计划的穷举方法。
- `find_best_task_for_group_expression`：默认 handler，直接执行默认递归并返回 Task；参数中的 `LogicalPlanRoute` 当前未使用。
- `install_cascades_find_best_task_router` / `install_default_cascades_find_best_task_router`：分别安装自定义函数指针或默认 handler。
- `route_find_best_task`：要求 handler 已安装，构造 GroupExpression 路由身份并委托；未安装时返回错误，不自动回退。
- `property_key`：以 `TaskType` 和 `expected_count` 的 IEEE-754 位模式构造缓存键。
- 五个首字母大写的 Go 风格函数：只转发到对应 snake_case 实现，不增加行为。

## 执行流程

默认递归求解按以下顺序进行：

1. `find_best_task_for_group(group, property)` 首先调用 `cached_group_task`。命中时复制缓存 Task，通过 `task.plan().cost(property.task_type)` 重新取得代价并立即返回。
2. 未命中时，在持锁区内克隆 `logical_expressions`，随后释放 Group 锁。这样递归进入子 Group 时不会继续持有父 Group 的互斥锁。
3. 逐个表达式调用 `find_best_task_for_expression`。如果全局 handler 已安装，该函数构造 `is_group_expression = true` 的 `LogicalPlanRoute` 并委托 handler；否则走 `find_best_task_for_expression_default`。
4. 默认实现调用 `LogicalPlan::exhaust_physical_plans(property)`。对每个候选读取 `child_required_properties`；其长度与 `expression.inputs` 不等时直接跳过该候选。
5. 对长度匹配的候选先计算本地 `physical.cost(property.task_type)`，再依输入顺序递归调用 `find_best_task_for_group(child, child_property)`，累加子代价并收集子 Task。
6. 所有孩子成功后调用 `physical.attach_to_task(child_tasks)`，形成 `PlannedTask { task, cost: local_cost + child_cost }`。
7. 表达式内和 Group 内都只在 `candidate.cost < current.cost` 时替换当前最优项；代价相等时保留先枚举到的候选，因此表达式和物理候选顺序是稳定的平局裁决条件。
8. Group 求得最优 Task 后，通过 `set_best_task` 复制写入缓存，再把原候选返回。空 Group 返回 `no supported physical task exists for memo group`；单个表达式没有任何可用物理候选时返回 `no physical task satisfies the property`。

公开的 `FindBestTask4GroupExpression` 并不读取 `FIND_BEST_TASK_HANDLER`，而是直接调用默认 handler；公开的 `route_find_best_task` 则必须先安装 handler。两条入口用途不同，扩展时不能互换其“未安装时回退”的语义。

## 数据与状态

- Group 的逻辑表达式在构造后仍为公开字段，但正常求解只读取其克隆快照；求解期间外部若通过锁修改原 Vec，本次递归不会看到中途新增的表达式。
- `best_tasks` 是按属性缓存的 Task 副本。键的 `expected_count.to_bits()` 保留浮点位级差异，因此 `+0.0` 与 `-0.0`、不同 NaN 载荷会形成不同缓存项；这是真实实现，不是数值相等语义。
- `PlannedTask.cost` 是递归期间显式累加的比较值；缓存命中和 handler 返回路径则从 `Task::plan().cost(task_type)` 重新读取。正确性因而要求挂接后的 Task 计划代价能够代表后续缓存比较所需的完整代价。测试中的简化 Task 只用于覆盖控制流，不能证明复杂计划的累计代价编码已经与生产模型一致。
- `LogicalPlanRoute.wrapped_type_id` 只在当前进程内标识具体 Rust 类型，`wrapped_plan_type` 才是可读名称；二者都没有序列化或跨进程稳定性承诺。
- 全局 `FIND_BEST_TASK_HANDLER` 是进程级永久状态。安装结果会跨同一测试进程中的测试用例保留，相关测试因此允许第一次安装已经返回 `HandlerAlreadyInstalled`。

## 依赖与调用关系

上游方面，`lib.rs` 将本文件全部公开项重导出，并在 `#[cfg(test)]` 下挂载 `find_best_task_router_test.rs` 与 `find_best_task_router_aster_unit_test.rs`。RustCodeGraph 对 `route_find_best_task`、`find_best_task_for_group` 和 Go 风格入口未给出文件外调用方；仓库级 `rg` 也只找到定义与这两个测试模块，因此生产接线状态应标为“未发现”。

下游方面：

- `find_best_task_for_group` 调用 `cached_group_task` 和 `find_best_task_for_expression`。
- `find_best_task_for_expression_default` 调用 `LogicalPlan::exhaust_physical_plans`、`RoutePhysicalPlan::child_required_properties`、递归的 `find_best_task_for_group`、`RoutePhysicalPlan::attach_to_task`，并调用继承自 `PhysicalPlan` 的 `cost`。
- 缓存和路由错误使用 `impl_and_cost::Error/Result`；Task 复制、计划读取及代价计算使用同模块的 `Task`、`PhysicalPlan` 和 `TaskType`。
- 并行共享与一次性注入完全来自标准库 `Arc`、`Mutex`、`OnceLock`；动态类型身份来自 `Any`/`TypeId`。

需要特别区分同 crate `impl_and_cost.rs` 中另一套 `Group`/`GroupExpression`/`CostEngine` 模型：两者名称和 Go 语义相关，但本文件的 `RouteGroup`/`RoutedGroupExpression` 是独立类型，当前源码没有二者之间的转换或直接调用边。

## 错误处理与边界

- 任一 `Mutex` 中毒都转换成 `Error::new("memo group lock is poisoned")`，不会 panic，也不会清除中毒状态。
- 缓存 Task 没有计划时返回 `cached task has no plan`；handler 返回的 Task 没有计划时返回 `routed task has no plan`。
- 逻辑计划穷举、物理计划计价、子 Group 求解、Task 挂接中的任一错误都由 `?` 原样向上传播。`find_best_task_router_test.rs` 明确验证：子 Group 中失败表达式无论位于有效表达式之前还是之后，都会终止枚举并保留 `child enumeration failed`，不会用已找到的有效候选掩盖错误。
- 子属性数与输入 Group 数不一致不是错误，而是跳过该物理候选；若全部候选均被跳过，最终返回“无满足属性的物理任务”。
- 空表达式 Group、空物理候选集合都有明确错误，不生成占位 Task。
- 使用普通 `f64` 加法和比较；NaN 代价不会触发专用错误，且 `<` 对 NaN 为 false，可能使选择结果依赖候选顺序。源码未提供有限值或非负值校验。
- `route_find_best_task` 在 handler 未安装时返回 `cascades find-best-task handler is not installed`；二次安装则返回独立的 `HandlerAlreadyInstalled`。
- 默认递归没有环检测。它依赖 Memo 输入图无环或至少不会沿当前属性形成未缓存的递归环；否则可能无限递归。源码也没有并发中的“求解中”占位状态。

## 并发与资源生命周期

`GroupRef` 允许跨线程共享 Group，因为逻辑和物理 trait 都要求或继承可在线程间使用的对象约束。缓存读写分别获取 `Mutex`；表达式列表在锁内克隆后立即释放锁，昂贵的物理枚举和递归不会占用父锁。最终写缓存时重新加锁。

该“检查缓存—解锁计算—重新加锁写入”不是单飞机制：多个线程可同时为同一 Group/属性重复计算，最后写入者覆盖先写入者。若所有输入与代价函数确定且比较规则一致，结果应等价，但源码没有防止重复工作，也没有验证两个并发结果一致。

`Arc<RoutedGroupExpression>` 和 `Arc<Mutex<RouteGroup>>` 管理共享所有权；Task 在缓存边界通过 `copy_task` 复制。`attach_to_task` 消费物理候选，明确结束该候选对象的独立生命周期。`OnceLock` 中的 handler 是 `'static` 函数指针，成功安装后存活到进程结束，不能释放、替换或捕获短生命周期状态。

## 与 Go 版本的对应关系

Go 的主链证据分散在三个文件：

- `pkg/planner/cascades/impl/impl_and_cost.go::ImplementGroupAndCost` 先查询 Group 的属性缓存，再遍历 GroupExpression，通过 `physicalop.FindBestTask` 求候选、用 `CompareTaskCost` 更新最优值，遇错立即停止，最后回写缓存。本文件的 `find_best_task_for_group` 对齐了缓存、表达式遍历、错误短路和最优候选回写，但没有 Go 的 `costLimit`、InvalidTask 或 `CompareTaskCost` 抽象。
- `pkg/planner/core/find_best_task.go::prepareIterationDownElems` 在识别到 `memo.GroupExpression` 后选择 `iteratePhysicalPlan4GroupExpression`；后者逐个读取物理计划对子节点的属性要求，并递归调用 `impl.ImplementGroupAndCost`。本文件以 `child_required_properties`、`expression.inputs` 和递归的 `find_best_task_for_group` 对齐这条结构，但没有 Go 的 `taskTypeSatisfied`、InvalidTask 跳过及 cost-limit 传播。
- `pkg/planner/cascades/memo/group_expr.go::ExhaustPhysicalPlans4GroupExpression` 根据被包装逻辑算子的具体 Go 类型分派到各算子穷举函数；`GroupExpression.FindBestTask` 再转到 `physicalop.FindBestTask`。本文件把具体算子分派下沉到 `LogicalPlan::exhaust_physical_plans` trait 实现，并用 `TypeId`/`plan_type` 形成路由身份；源码中没有 Go `switch` 的全量算子列表，也没有 `hintCanWork` 和二维候选切片语义。

因此，本文件是 Go 路由与递归骨架的 Rust 最小接口化实现，而不是 Go 三个文件的完整一比一替代。与 Go 的差异应在扩展时逐项补证，不能以函数别名相似为由假设行为已齐全。

## 扩展指南

- 新增逻辑算子：实现 `LogicalPlan`，确保 `plan_type` 可诊断且 `as_any` 返回自身；在独立测试文件中覆盖属性相关的候选集合、无候选和穷举错误。
- 新增物理候选：实现 `PhysicalPlan` 与 `RoutePhysicalPlan`，保证 `child_required_properties` 的长度和顺序与 `RoutedGroupExpression.inputs` 完全一致，并让 `attach_to_task` 生成的计划代价与递归比较/缓存重读语义一致。
- 改变候选优先规则：修改 `find_best_task_for_expression_default` 和/或 `find_best_task_for_group` 前，先决定平价候选、NaN、hint、InvalidTask、task-type 满足性及 cost limit 是否需要对齐 Go；不要只把 `<` 替换掉而遗漏两层比较。
- 扩充缓存键：修改 `property_key` 时必须覆盖 `PhysicalProperty` 中所有影响计划合法性或代价的字段，并增加独立测试证明不同属性不会错误复用 Task。
- 接入生产主链：应在明确的 crate 边界增加 `RoutedGroupExpression` 与真实 Memo/逻辑算子类型的转换或适配，并用调用图证明安装时机早于 `route_find_best_task`；当前无生产调用证据，不应依赖测试中的隐式全局安装。
- 增强并发：若需要避免重复求解，必须设计“求解中/成功/失败”状态及递归环处理，不能在持有父 Group `Mutex` 时直接递归，否则容易形成锁顺序问题。
- 修改后测试仍应放在独立的 `find_best_task_router_test.rs` 或 `find_best_task_router_aster_unit_test.rs`，不要把 `#[cfg(test)]` 测试嵌入生产源文件；这也符合当前 `lib.rs` 的测试模块组织。

## 验证依据

- 生产源码：`pkg/planner/cascades/impl/find_best_task_router.rs`，核对了全部 trait、类型别名、结构体、枚举、静态量、内部函数、公开函数和 Go 风格转发入口。
- crate 边界：`pkg/planner/cascades/impl/Cargo.toml` 与 `pkg/planner/cascades/impl/lib.rs`；工作区成员和 facade 别名由根 `Cargo.toml` 核对。
- RustCodeGraph：`status` 显示索引包含目标文件；`query` 定位 `route_find_best_task`（第 255 行）、`find_best_task_for_group`（第 136 行）、`find_best_task_for_group_expression`（第 231 行）和 `FindBestTask4GroupExpression`（第 290 行）。`callees` 证实 Group 入口调用缓存与表达式入口，默认表达式入口调用物理枚举、子属性、递归 Group 求解和 Task 挂接；`callers` 未返回文件外调用方。
- Rust 测试：`find_best_task_router_test.rs` 验证子错误在有效候选之前或之后都立即传播；`find_best_task_router_aster_unit_test.rs` 验证具体逻辑类型身份、叶子/一元默认枚举与 Task 构造、物理候选数量及二次 handler 安装被拒绝。
- Go 对照：`pkg/planner/cascades/impl/impl_and_cost.go::ImplementGroupAndCost`、`pkg/planner/cascades/memo/group_expr.go::{ExhaustPhysicalPlans4GroupExpression, GroupExpression.FindBestTask}`、`pkg/planner/core/find_best_task.go::{prepareIterationDownElems, iteratePhysicalPlan4GroupExpression}`。
- 仓库搜索：使用 `rg` 核对公开入口的定义与引用，未发现上述路由 API 的 Rust 生产调用点；该结论仅表示当前检出版本没有静态文本引用，不等同于未来不会通过新增适配接线。
- 本任务为纯文档分析，按计划不运行 Cargo；完成前以任务指定命令验证文档存在且恰有十一个固定二级章节，并人工复核没有把 Go 的完整能力误写成当前 Rust 已支持。
