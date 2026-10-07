# `pkg/dxf/framework/scheduler/balancer.rs`

## 文件定位

本文件实现 DXF（Distributed eXecution Framework）调度器中的子任务负载均衡器。它不创建任务，也不执行子任务，而是在一次均衡周期内读取各任务的活跃子任务，结合节点作用域、任务自定义的可用实例、节点槽位容量和 `max_node_count`，决定哪些子任务需要改写 `exec_id`。模块由 `pkg/dxf/framework/scheduler/lib.rs` 公开为 `balancer` 并整体再导出。

生产侧有两条已确认入口：`SchedulerManager::balance`（`scheduler_manager.rs:358`）持有 manager 内部的 `Mutex<Balancer>` 后调用 `Balancer::balance`；分布式 `MODIFY COLUMN` 的运行循环则在 `pkg/session/runtime/modify_column_dist_backfill.rs:1111-1136` 直接构造 `Balancer`，并在每轮 `schedule_once` 后调用它。当前 `SchedulerManager::tick`（`scheduler_manager.rs:211`）本身不调用 `balance`，因此不能把 Go 文件中的独立定时均衡循环描述成 Rust manager 已自动接线的行为。

crate 边界由 `pkg/dxf/framework/scheduler/Cargo.toml` 定义，包名是 `astersql-dxf-framework-scheduler`，库入口为 `lib.rs`。本文件直接只使用本 crate 的 `interface`、`nodes`、`slots` API 和标准库集合/`Arc`；任务、子任务等协议类型经 crate 根从 `astersql-dxf-framework-proto` 再导出。Cargo 未为本模块声明独立 feature。

## 核心职责

1. `Balancer::balance` 为一个均衡 tick 建立统一的托管节点快照和空槽位账本，并按传入 scheduler 的顺序处理任务；顺序代表任务优先级，因为前面的任务会占用后面任务可见的槽位。
2. 对每个任务先应用 `target_scope`，再与 `Extension::eligible_instances` 的非空结果取交集；若扩展返回空列表，则沿用 scope 过滤结果。
3. `Balancer::balance_subtasks` 读取活跃子任务，剔除剩余容量不足的节点，再按 `max_node_count` 限制实际使用的节点数。
4. `rebalance_pending_subtasks` 以“每节点平均数，部分节点多一个”的目标重排任务：合格节点上的 running 子任务尽量不移动，pending 子任务可移动；不合格节点上的全部活跃子任务会被迁走以完成故障转移。
5. 成功写回新的 `exec_id` 后，按“一个任务在一个节点只占一次 `required_slots`”更新本 tick 的槽位账本，为后续低排名任务提供容量约束。

该文件只改变子任务归属，不负责把 running 子任务恢复为 pending、不负责节点存活探测，也不负责真实槽位的预留/释放；这些职责分别位于 task executor、`NodeManager` 和 `SlotManager` 等模块。

## 主要符号

- `pub struct Balancer { pub param: Param, current_used_slots: HashMap<String, i32> }`（`balancer.rs:28`）：`param` 汇集 `TaskManager`、`NodeManager`、`SlotManager` 等共享服务；`current_used_slots` 是仅在单次 `balance` 中跨任务累计的临时账本。
- `pub fn Balancer::new(param: Param) -> Self`（`:37`）：保存依赖并创建空账本，不读取节点或存储。
- `pub fn Balancer::balance(&mut self, schedulers: &[Arc<dyn Scheduler>]) -> Result<()>`（`:47`）：多任务入口。它只读取一次 managed-node 快照，按 slice 顺序处理 scheduler，并在首个错误处终止。
- `pub fn Balancer::balance_subtasks(&mut self, task: &Task, eligible_nodes: Vec<String>) -> Result<()>`（`:78`）：单任务入口，完成活跃子任务读取、容量/节点数过滤、重排、持久化和槽位记账。
- `fn Balancer::update_used_nodes(&mut self, task: &TaskBase, subtasks: &[SubtaskBase])`（`:115`）：按不同 `exec_id` 去重，每个出现过的节点累加一次 `required_slots`。
- `pub fn filter_nodes_by_max_node_count(...) -> Vec<String>`（`:127`）：超过上限时按节点现有子任务数稳定降序，保留较忙节点；`max_node_count == 0` 表示不限。
- `fn rebalance_pending_subtasks(...) -> Vec<SubtaskBase>`（`:152`）：纯内存重排核心，返回需要持久化的子任务副本；它不是公共 API。
- `pub fn filterNodesByMaxNodeCnt(...)`（`:231`）：保留 Go 风格名称的薄包装，行为完全委托给 snake_case 函数。

本文件没有模块级常量、trait、enum 或条件编译项。

## 执行流程

一次 `balance` 的流程如下：

1. 从 `NodeManager::get_nodes` 获取一次 managed-node 快照，用所有节点 ID 将 `current_used_slots` 重置为 0。只取一次快照保证同一 tick 的 scope 过滤和容量账本基于同一节点视图。
2. 依次读取每个 `Scheduler::task()`。用 `filter_by_scope` 处理 `TaskBase::target_scope`；`nodes.rs:114` 表明，当 scope 为空且存在 background 节点时，有效 scope 会变成 `background`。
3. 调用 `Scheduler::extension().eligible_instances(&task)`。非空结果与 scope 节点取交集；空结果表示不追加应用级限制。最终集合为空时返回 `SchedulerError("no eligible nodes to balance subtasks")`。
4. `balance_subtasks` 通过 `TaskManager::active_subtasks(task_id)` 读取活跃子任务；没有子任务时立即成功，不记账。
5. `filter_nodes_with_enough_slots`（`slots.rs:247`）仅保留账本中存在且满足 `used + required_slots <= capacity` 的节点。随后 `filter_nodes_by_max_node_count` 在需要时优先保留已有子任务最多的节点。
6. 若过滤后无节点，当前任务被跳过且不更新账本，使后续较低排名任务仍有机会使用节点；这与 Go 的逐任务优先级语义一致。
7. `rebalance_pending_subtasks` 计算 `average = subtasks / nodes` 与余数，把子任务按原 `exec_id` 分组，并把 running 项插到每组前部。对仍合格但超额的组，只从尾部抽取 pending 项；对已经不合格的节点，则抽取该组全部 active 项，包括 running 项，以支持节点失效或容量不足时的故障转移。
8. 余数名额先由已经达到 `average + 1` 的节点占用，再按 `adjusted_nodes` 顺序分配剩余名额。抽出的任务依次填入未达目标数的节点，并改写副本的 `exec_id`。
9. 如有变化，`balance_subtasks` 先把变化映射回本地 `subtasks` 快照，再调用 `TaskManager::update_subtask_exec_ids` 持久化。写回成功后，`update_used_nodes` 根据更新后的完整快照累计槽位。

均衡目标受 running 任务不可搬迁约束，因此合格节点上的最终数量不保证严格相差不超过 1。例如一个合格节点上全部是 running 子任务时，算法会保留其不均衡分布，等待 task executor 后续状态处理。

## 数据与状态

`current_used_slots` 的生命周期是一轮 `balance`：入口以 managed-node ID 全量重建，单个任务完成后累计使用量，下一任务据此过滤容量。它不是 `SlotManager` 的全局真实占用表，也不会跨 tick 保留。节点若不在本轮初始 map 中，即使出现在扩展结果里也会被容量过滤函数排除。

槽位记账按 `(task, exec_id)` 而不是按子任务计数：同一任务在同一节点有一个或多个 active subtask 都只增加一次 `task.required_slots`。这一不变量由 `update_used_nodes` 先收集唯一节点集合实现，并由 Go `TestBalancerUpdateUsedNodes` 及 Rust 多任务测试的期望支撑。

重排使用若干短生命周期容器：`groups` 保存每个原执行节点的子任务副本；`pending_counts` 限制从合格节点移走的数量；`need_schedule` 保存待重新分配项；`one_more` 标记获得余数名额的节点。算法会克隆 `SubtaskBase`，真正的外部状态变更只通过 `update_subtask_exec_ids` 发生。

`filter_nodes_by_max_node_count` 使用稳定排序。子任务数相同的节点保持输入顺序，所以调用者传入的 eligible-node 顺序会成为并列决策依据；这一点对应 Go 的 `sort.SliceStable`。

## 依赖与调用关系

上游调用链：

- `SchedulerManager::new`（`scheduler_manager.rs:124`）构造并保存 `Mutex<Balancer>`；`SchedulerManager::balance`（`:358`）锁定它并传入当前 scheduler 列表。
- `pkg/session/runtime/modify_column_dist_backfill.rs:1096-1137` 构造 `BaseScheduler` 和 `Balancer`，在分布式修改列/索引回填循环中刷新节点、推进 scheduler、均衡子任务并等待下一轮。
- `balancer_test.rs` 直接调用 `Balancer::balance` 和 `filter_nodes_by_max_node_count` 验证局部语义。

下游依赖：

- `Param`（`interface.rs:489`）提供 `Arc<dyn TaskManager>`、`Arc<NodeManager>`、`Arc<SlotManager>` 以及其他调度参数；本文件实际使用前三者。
- `Scheduler`（`interface.rs:512`）提供任务快照和 `Extension`；`Extension::eligible_instances`（`:468`）提供任务类型特定的节点限制。
- `filter_by_scope`（`nodes.rs:114`）负责角色/scope 过滤。
- `filter_nodes_with_enough_slots`（`slots.rs:247`）依据本轮账本、统一容量和任务所需槽位筛选节点。
- `TaskManager::active_subtasks` 与 `update_subtask_exec_ids`（`interface.rs:412,420`）形成存储读取/写回边界。

RustCodeGraph 将 `balance_subtasks -> filter_nodes_with_enough_slots / filter_nodes_by_max_node_count / rebalance_pending_subtasks / update_used_nodes` 识别为直接调用边，并将 `Balancer::balance -> balance_subtasks / filter_by_scope` 识别为直接调用边。图索引对同名 method 的 caller 解析不完整，因此 manager 与 session runtime 的入口另由对应源码位置核验。

## 错误处理与边界

- `eligible_instances`、`active_subtasks`、`update_subtask_exec_ids` 的错误均通过 `?` 原样向上传播；`balance` 在首个任务错误处停止，后面的 scheduler 不再处理。
- scope 与应用限制相交后为空是硬错误；容量过滤或 `max_node_count` 过滤后为空则是可接受的“本任务本轮不均衡”，返回成功且不占账。这两个空集合发生阶段不同，语义不可合并。
- 活跃子任务为空时成功返回，不调用存储更新，也不占用槽位。
- `rebalance_pending_subtasks` 用节点数作除数，但调用者已在 `adjusted.is_empty()` 时返回，因此正常入口不会除以零。若未来把该私有函数独立暴露，必须保留此前置条件。
- `max_node_count` 的类型是 `i32`，当前实现仅为 0 定义“不限”；负值会转换成巨大的 `usize` 并走“不截断”路径。源码和测试未声明负值是合法输入，扩展校验时应在任务元数据边界明确其约束。
- 内部 `groups.get_mut(...).expect("group was just indexed")` 依赖 keys 先从同一 map 克隆出的不变量；当前循环不删除 key，因此不会触发。manager 侧互斥锁中毒则在 `SchedulerManager::balance` 以 `expect` panic，不属于本文件的 `Result` 错误面。
- 持久化失败时不会执行 `update_used_nodes`。本地 `subtasks` 快照已被改写，但它不是共享存储对象；错误向上传播后，本轮账本不应被视为完整结果。

## 并发与资源生命周期

`Balancer::balance` 和 `balance_subtasks` 都要求 `&mut self`，因此一个实例不能在安全 Rust 中被并发修改。`SchedulerManager` 用 `Mutex<Balancer>` 串行化共享实例的均衡调用；scheduler 自身以 `Arc<dyn Scheduler + Send + Sync>` 共享，任务、节点和槽位服务也由 `Arc` 持有。

本文件不创建线程、异步任务、通道、定时器或 context，也不持有数据库事务。直接生产入口 `modify_column_dist_backfill.rs` 在外层负责停止标志、100ms sleep、scheduler 关闭与槽位释放；manager 负责其 Balancer 的所有权和锁。Go 版本的 `balanceLoop` 定时器与 context 取消不在本 Rust 文件内，不能据此推断 Rust 会自行周期运行。

节点和子任务均采用快照式处理：managed nodes 在一轮开头读取一次，active subtasks 每个任务读取一次。均衡期间发生的外部节点变化要到下一轮才能反映；持久化冲突/失败则由 `TaskManager` 错误返回，没有本地重试或补偿。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/scheduler/balancer.go`，核心映射如下：

- Go `balancer` / `newBalancer` 对应 Rust `Balancer` / `Balancer::new`；`currUsedSlots` 对应 `current_used_slots`。
- Go `balance`、`balanceSubtasks`、`doBalanceSubtasks` 的逻辑在 Rust 中合并为 `balance` 和 `balance_subtasks`。Rust 直接通过 `Scheduler::extension().eligible_instances` 加 scope 交集实现 Go `getEligibleNodes` 的效果。
- Go `filterNodesByMaxNodeCnt` 对应 Rust `filter_nodes_by_max_node_count`，Rust 另保留同名 Go 风格兼容包装 `filterNodesByMaxNodeCnt`。
- Go `doBalanceSubtasks` 内的平均数、余数、running 前置、pending 尾部抽取、失效节点整体迁移和最终写回，对应 Rust 私有函数 `rebalance_pending_subtasks` 加外层持久化逻辑。
- Go `updateUsedNodes` 对应 Rust `update_used_nodes`，两者都按节点去重后每任务只记一次 `required_slots`。

已确认的结构差异是：Go 文件内含 `balanceCheckInterval`、`balanceLoop`、context、logger 和 `mockNoEnoughSlots` failpoint；Rust 文件不包含这些机制。Go 顶层 `balance` 遇到单任务错误时记录 warning 并结束本轮且不返回错误，Rust `balance` 返回 `Result` 给调用者。Rust 的错误可见性更强，但外层是否记录、重试或终止由调用方决定。

测试覆盖也不完全等量。Go `balancer_test.go` 有更大的表驱动集合，并显式覆盖 eligible-instance 错误、无实例、task-manager 读写失败、扩缩容组合和槽位记账；Rust `balancer_test.rs` 覆盖失效节点迁移、多任务槽位优先级、稳定的 max-node 筛选、无子任务、余数分布、合格节点 running 不移动等核心路径，但未逐项复刻 Go 的全部错误注入用例。文档不能据此声称所有 Go 测试分支都已在 Rust 独立测试中覆盖。

## 扩展指南

- 修改节点资格规则时，从 `Balancer::balance` 的 scope/extension 交集处接入，并同步检查 `nodes::filter_by_scope` 的 background 语义。新增过滤器要保持节点顺序，避免破坏 stable tie-break。
- 修改容量模型时，优先调整 `slots::filter_nodes_with_enough_slots` 与 `update_used_nodes` 的共同不变量；若从“每任务每节点一次”改成按子任务计费，必须同时修改多任务优先级测试并评估后续 scheduler 饥饿风险。
- 修改均分策略或 running 迁移规则时，集中在 `rebalance_pending_subtasks`。必须区分“合格节点上的 running 不移动”和“不合格节点的 active 全部故障转移”两种路径，不能为了得到数学上的绝对均匀而搬迁健康节点上的 running 项。
- 修改 `max_node_count` 行为时，更新 `filter_nodes_by_max_node_count` 及 Go 风格包装，并保留稳定排序或明确兼容性变化；还应补充 0、1、并列计数和非法负值的独立 Rust 测试。
- 修改存储写回顺序时，要保证槽位账本只在持久化成功后更新，否则后续任务会基于未落盘的分配被错误过滤。
- 测试必须继续放在独立的 `pkg/dxf/framework/scheduler/balancer_test.rs`，不要内嵌进生产文件。应至少同步对照 `balancer_test.go` 的相关场景；涉及完整 manager 或运行循环接线时，再选择 `scheduler_manager_*_test.rs` 或分布式 MODIFY 的上层测试面。

主要兼容风险是节点选择顺序和 Go 语义漂移；正确性风险集中在 running 状态、余数分配及错误后账本一致性；性能风险集中在每轮克隆全部 active `SubtaskBase`、建立多个 `HashMap`/`HashSet` 和稳定排序，复杂度大致为每任务 O(S + N log N)，其中 S 为活跃子任务数、N 为候选节点数。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录可见 `balancer.rs`、`balancer_test.rs`、`scheduler_manager.rs`、`nodes.rs`、`slots.rs` 等文件。
- RustCodeGraph 源码/符号查询：`node --file pkg/dxf/framework/scheduler/balancer.rs --offset 1 --limit 260`；`query balance --kind function --json --limit 50`；`node balancer.rs::balance`、`balance_subtasks`、`filter_nodes_by_max_node_count`、`rebalance_pending_subtasks`；`node scheduler_manager.rs::balance`、`tick`；`node nodes.rs::filter_by_scope`、`slots.rs::filter_nodes_with_enough_slots`、`interface.rs::Param`、`interface.rs::Scheduler`。
- 直接读取的 crate/模块证据：`pkg/dxf/framework/scheduler/Cargo.toml`、`pkg/dxf/framework/scheduler/lib.rs`。
- Go 对照与测试证据：`pkg/dxf/framework/scheduler/balancer.go`、`pkg/dxf/framework/scheduler/balancer_test.go`。
- Rust 独立测试证据：`pkg/dxf/framework/scheduler/balancer_test.rs`，包含 `test_balance_one_task`、`test_balance_multiple_tasks`、`test_balancer_update_used_nodes`、`test_balance_matches_go_table_driven_edge_cases`。
- 生产接线证据：`pkg/dxf/framework/scheduler/scheduler_manager.rs:123-148,358-362` 与 `pkg/session/runtime/modify_column_dist_backfill.rs:1096-1137`。
- 本任务为纯文档分析，按计划不运行 Cargo；验收只执行任务指定的 11 章节结构检查，并人工核对本文未把 Go 独有的 timer/logger/context/failpoint 描述成 Rust 当前能力。
