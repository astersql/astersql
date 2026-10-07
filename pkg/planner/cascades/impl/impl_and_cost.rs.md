# `pkg/planner/cascades/impl/impl_and_cost.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-impl` crate，是 Cascades 优化器从逻辑 Memo 搜索空间进入物理计划选择与代价计算的 Rust 移植边界。crate 入口 `pkg/planner/cascades/impl/lib.rs` 将本模块与 `find_best_task_router` 一并私有声明后公开重导出；`pkg/planner/cascades/impl/Cargo.toml` 未声明外部依赖，并用 `autotests = false` 将测试显式挂在 `lib.rs` 下。

当前 Rust 文件自带一套简化的 `Group`、`GroupExpression`、`PhysicalProperty`、`Task` 和 `PhysicalPlan` 抽象，并没有直接使用 `pkg/planner/cascades/memo` 等其他 crate 的真实 Memo 类型。全仓 Rust 引用搜索只发现本文件内部调用、Go 风格包装导出和相邻路由测试对基础类型的使用，未发现生产 Rust 入口调用 `implement_memo_and_cost` 或 `implement_group_and_cost`。因此它是已实现但尚未接入完整 Rust 规划主链的兼容/移植单元，不能描述成已经替代 Go 优化入口。

## 核心职责

- `implement_group_and_cost` 针对一个 `Group` 和一个 `PhysicalProperty` 查找、比较并缓存最佳 `Task`；缓存项也可以是无效任务，用来表示该属性已经完整搜索。
- `implement_memo_and_cost` 作为根 Memo 门户，构造 Root 属性，取得根任务，恢复会话瞬态字段，传播警告，解析物理计划列下标并计算最终代价。
- `CostEngine` 把候选生成、任务代价读取、候选比较和无效任务构造留给调用方注入，模拟 Go 中 `physicalop` 与 `utilfuncp` 的导入环回调边界。
- `BasicTask`、`CallbackCostEngine` 等类型提供最小可用实现，既支撑上述流程，也供相邻 `find_best_task_router` 及其独立测试复用。

## 主要符号

- `Error(String)` 与 `Result<T>`：模块内轻量错误协议；`Error::new` 接受任意可转为字符串的消息，并实现标准 `Display`/`Error`。
- `TaskType::{Root, Cop, BatchCop, Mpp}`：物理任务执行位置；默认值是 `Root`。
- `PhysicalProperty { task_type, expected_count }`：当前缓存属性仅包含任务类型与期望行数。私有 `cache_key` 用 `expected_count.to_bits()` 生成 `(TaskType, u64)`，保留浮点位模式而非做数值归一化。
- `PhysicalPlan`：要求计划可 `resolve_indices`，并可按 `TaskType` 返回代价；trait 受 `Send` 约束。
- `Task`：定义深复制、有效性检查、计划借用/可变借用/所有权取出以及警告读取；同样受 `Send` 约束。
- `BasicTask`：保存可选计划、`estimated_cost`、警告、无效标记，以及用于复制 trait object 计划的 `Arc<Fn>` 回调。`BasicTask::invalid` 用无计划和正无穷代价表示哨兵。
- `GroupExpression { name }`：传给代价引擎的最小逻辑表达式身份，不包含真实 Memo 的输入组、统计信息或逻辑算子。
- `SessionState`：保存 `task_map_backup_timestamp`、TiFlash 解耦开关、MPP 许可和语句警告。
- `Group`：持有逻辑表达式、会话状态及私有 `best_tasks`。`get_best_task` 返回缓存借用，`set_best_task` 按属性键覆盖缓存。
- `CostEngine`：四个回调分别对应找任务、取计划代价、比较任务和构造无效任务。`CallbackCostEngine` 用闭包逐项实现该接口，其中找任务闭包是 `FnMut`，其余是 `Fn`。
- `implement_group_and_cost` / `ImplementGroupAndCost`：Rust 风格实现及仅转发的 Go 风格导出。
- `implement_memo_and_cost` / `ImplementMemoAndCost`：根门户实现及仅转发的 Go 风格导出。

## 执行流程

`implement_group_and_cost` 的流程如下：

1. 用 `PhysicalProperty::cache_key` 查询 `Group::best_tasks`。命中时先调用 `Task::copy_task`，避免把缓存对象本身交给调用方并被后续 `take_plan` 消耗。
2. 通过 `CostEngine::task_plan_cost` 同时取得代价与引擎判定的无效状态。若无效，返回一个新建的无效任务；若有效且代价不超过 `cost_limit`，返回副本；超过上限则返回 `Ok(None)`，对应 Go 的 `(nil, nil)`。
3. 未命中缓存时，以 `invalid_task` 初始化最佳任务，按 `Group::logical_expressions` 的顺序调用 `find_best_task`。每个候选经 `candidate_is_better` 与当前最佳任务比较，较优时替换。
4. 枚举完成后把最佳任务的副本写入缓存，包括无效任务，再返回原任务。任一引擎回调报错均通过 `?` 立即终止，错误之后的表达式不会继续枚举。

`implement_memo_and_cost` 的流程如下：

1. 根组没有逻辑表达式时立即返回 `root group must contain a logical expression`，避免 Go 版本从链表首元素取会话上下文时的隐式前置条件变成崩溃。
2. 构造 `TaskType::Root`、`expected_count = f64::MAX` 的根属性，并以 `f64::MAX` 为代价上限调用 `implement_group_and_cost`；理论上的 `None` 会转换为引擎无效任务。
3. 无论任务是否有效，先把 `root.session.task_map_backup_timestamp` 清零，与 Go 门户调用后的语句状态恢复顺序一致。
4. 若任务无效，返回通用“找不到物理计划”错误；当 `disaggregated_tiflash` 为真且 `mpp_allowed` 为假时追加开启 `tidb_allow_mpp` 的提示。
5. 对有效任务，先把任务警告复制到会话，再取得可变计划执行 `resolve_indices`，随后以 `TaskType::Root` 计算代价，最后通过 `take_plan` 转移计划所有权并返回 `(plan, cost)`。

## 数据与状态

`Group::best_tasks` 是文件内最重要的持久状态。键由任务类型和 `f64` 原始位模式组成，因此 `0.0` 与 `-0.0` 会形成不同键，不同 NaN 位模式也不会合并；这与 Go `PhysicalProperty` 的完整哈希/等价语义并非一一对应，是扩展时必须显式评估的兼容点。缓存值是 `Box<dyn Task>`，对外返回前必须复制；根门户最终会 `take_plan`，所以若直接返回缓存对象会破坏缓存。

`BasicTask::invalid()` 同时设置 `invalid = true`、`plan = None` 和 `estimated_cost = +∞`。`Task::invalid` 还把 `plan.is_none()` 视为无效，因此“标记有效但缺少计划”在进入根门户的无效分支前就会被拒绝。另一方面，引擎的 `task_plan_cost` 也返回独立的 `invalid` 布尔值，缓存命中时以引擎判断为准决定是否替换为新无效任务。

`SessionState::warnings` 仅在根计划成功、解析索引之前扩展；后续 `resolve_indices` 或 `cost` 报错不会回滚已经加入的警告。`task_map_backup_timestamp` 同样在有效性检查前被清零，错误路径仍保留该状态变化。这些顺序属于可观察行为。

## 依赖与调用关系

文件只直接依赖标准库的 `HashMap`、`fmt` 和 `Arc`。`lib.rs` 公开重导出本模块符号，并让相邻 `find_best_task_router.rs` 使用 `BasicTask`、`PhysicalProperty`、`Task` 等接口；两个独立 Rust 测试 `find_best_task_router_aster_unit_test.rs` 和 `find_best_task_router_test.rs` 通过这些类型验证叶子/一元任务构造、代价读取与子枚举错误传播。

RustCodeGraph 对目标符号显示：`implement_memo_and_cost` 下调 `implement_group_and_cost`、`Task::{invalid,warnings,plan_mut,plan,take_plan}` 和 `PhysicalPlan::{resolve_indices,cost}`；`implement_group_and_cost` 下调 `Group::{get_best_task,set_best_task}`、`Task::copy_task` 及四个 `CostEngine` 方法。精确全仓搜索没有发现目标文件外的 Rust 调用者；两个大写包装函数也只转发到小写实现。

Go 生产调用链则已接通：`pkg/planner/core/optimizer.go::physicalOptimize` 调用 Go `impl.ImplementMemoAndCost`；`pkg/planner/core/find_best_task.go` 在物理计划枚举子节点时多处调用 Go `impl.ImplementGroupAndCost`，形成从根 Group 到子 Group 的递归物理化。该调用关系只能证明 Go 参考实现的位置，不能证明当前 Rust 门户已被完整应用调用。

## 错误处理与边界

- 空根组、有效任务缺少计划、引擎回调失败、`resolve_indices` 失败和 `cost` 失败都返回 `Error`，不发生 panic；只有 `BasicTask::invalid` 的克隆回调含 `unreachable!`，但无效任务没有计划，正常 `copy_task` 不会调用该闭包。
- 缓存命中且计划有效但超过 `cost_limit` 是正常剪枝，以 `Ok(None)` 表示，不是错误；首次枚举路径当前不再用 `cost_limit` 过滤最终候选，这是与调用方契约有关的非对称行为。
- 缓存命中的 `task_plan_cost` 若报错会直接传播；若返回 `invalid = true` 则丢弃缓存副本并返回新的无效任务。
- 首次枚举的 `find_best_task` 或 `candidate_is_better` 一旦失败即停止，不写缓存；这一行为与 Go `ForEachGE` 在 `implErr` 后返回 `false` 一致。
- `expected_count` 使用原始浮点位作键，调用者若传入 NaN、正负零或语义等价但位模式不同的值，可能产生不同缓存项；本文件不校验有限性和非负性。
- 任务与计划只要求 `Send`，没有要求 `Sync`；调用者仍需负责对 `Group` 与可变引擎的串行可变访问。

## 并发与资源生命周期

该实现没有生成线程、异步任务、通道、锁、事务或外部 I/O。`implement_group_and_cost` 接收 `&mut Group` 和 `&mut dyn CostEngine`，Rust 借用规则确保一次调用期间不会并发改写同一组或引擎。`CallbackCostEngine::find_best_task_fn` 是 `FnMut`，明确允许回调维护串行状态。

`BasicTask` 独占 `Box<dyn PhysicalPlan>`；复制任务时通过调用方提供的 `clone_plan` 深复制计划，而 `Arc` 只共享线程安全的克隆函数本身。根门户对返回任务调用 `take_plan`，把计划所有权移交给调用方并将任务内部置为 `None`。缓存保存的是事先复制出的另一份任务，因此不受该所有权转移影响。警告也按 `Vec<String>` 克隆，缓存与返回任务不共享可变警告状态。

## 与 Go 版本的对应关系

直接参考文件是 `pkg/planner/cascades/impl/impl_and_cost.go`。两边都以根门户和单 Group 门户分工：根门户构造 Root/最大期望行数属性、实现根组、清理 `TaskMapBakTS`、处理无计划提示、收集警告、解析索引并计算 Root 代价；Group 门户先查属性缓存，未命中时遍历全部逻辑等价表达式、调用找任务路由并比较代价，最后缓存最佳任务。

关键差异如下：

- Go 使用真实 `memo.Group`、`base.Task`、`base.PhysicalPlan`、`property.PhysicalProperty`、全局配置和会话上下文；Rust 文件使用本地简化类型和注入式 `CostEngine`，尚未接入真实 Rust Memo/Session。
- Go 缓存命中时直接返回缓存任务；Rust 返回 `copy_task`，因为 Rust 根门户随后以 `take_plan` 消耗所有权。
- Go `GetTaskPlanCost` 返回错误或无效时返回 `base.InvalidTask` 与原错误；Rust 错误直接传播，无效时调用引擎生成新哨兵。
- Go 根组默认存在首个逻辑表达式并从中取得 `SCtx`；Rust 显式检查空组并返回错误。
- Go 最终 `GetPlanCost` 使用 `NewDefaultPlanCostOption()`；Rust 的 `PhysicalPlan::cost(TaskType::Root)` 没有暴露代价选项。
- Go 警告限定为 `RootTask` 的 warnings 对象；Rust `Task::warnings` 对所有任务统一返回字符串切片。

这些差异说明当前 Rust 代码保持了主要控制流与错误顺序，但不是 Go 数据模型和代价系统的完整替代品。

## 扩展指南

- 接入真实 Rust Cascades 主链时，优先让 `Group`/`GroupExpression`/`PhysicalProperty` 适配已有 Memo 与 property crate，避免继续扩充本文件的平行模型；同时在调用入口验证根组、会话和配置来源。
- 扩展缓存属性时必须同步修改 `PhysicalProperty::cache_key`、`Group::best_tasks` 键类型及等价性测试。排序、分区、引擎约束等任何影响物理计划选择的字段若遗漏，都会错误复用缓存。
- 增加新的任务形态时同步更新 `TaskType`、计划代价实现和回调引擎；不要只修改枚举而让缓存键或比较器忽略新语义。
- 改动门户执行顺序时需要保留 Go 可观察契约：先清时间戳、无效分支提示条件、成功路径警告传播、`resolve_indices` 先于最终 `cost`。若有意偏离，应先补兼容性说明和回归测试。
- 应新增同目录独立测试文件（例如 `impl_and_cost_test.rs`，并在 `lib.rs` 的 `#[cfg(test)]` 区声明），不要把测试嵌入生产源文件。最低覆盖应包括缓存命中/未命中、成本上限返回 `None`、无效任务缓存、候选比较与错误短路、空根组、TiFlash/MPP 提示、警告与索引/代价调用顺序、缺计划错误，以及 `take_plan` 不破坏缓存副本。
- 相邻路由行为继续同步 `find_best_task_router_aster_unit_test.rs` 和 `find_best_task_router_test.rs`；这些测试当前只间接覆盖 `BasicTask` 和 `PhysicalPlan` 接口，不能替代门户测试。
- 性能风险集中在每次缓存返回和写入都深复制计划，以及首次未命中时线性枚举全部表达式；引入共享计划或并行枚举前必须重新审视可变计划、警告与回调状态的所有权。

## 验证依据

- RustCodeGraph 索引状态：目标仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/cascades/impl` 确认目标源码、模块入口及两个独立测试均已索引。
- 源码与符号：`pkg/planner/cascades/impl/impl_and_cost.rs` 的 `PhysicalProperty::cache_key`、`Task`、`BasicTask`、`Group`、`CostEngine`、`implement_group_and_cost`、`implement_memo_and_cost` 及两个 Go 风格包装函数。
- crate 边界：`pkg/planner/cascades/impl/Cargo.toml` 和 `pkg/planner/cascades/impl/lib.rs`；前者确认独立 crate、无声明依赖、关闭自动测试，后者确认模块重导出与测试装配。
- Go 对照与生产调用：`pkg/planner/cascades/impl/impl_and_cost.go`、`pkg/planner/core/optimizer.go`、`pkg/planner/core/find_best_task.go`。
- Rust 相关测试：`pkg/planner/cascades/impl/find_best_task_router_aster_unit_test.rs` 验证叶子/一元路由与代价；`pkg/planner/cascades/impl/find_best_task_router_test.rs` 验证子枚举错误在有效候选之前或之后都立即传播。精确搜索未发现直接调用两个 Rust 门户函数的测试。
- 本任务是只读代码分析加文档新增，按计划不运行 Cargo；最终以固定十一个二级标题的结构命令检查文档形态，并人工核对以上路径、符号和调用关系。
