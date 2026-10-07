# `pkg/dxf/framework/scheduler/slots.rs`

## 文件定位

本文件实现 DXF owner 侧调度链中的资源准入与节点容量过滤。DXF 把单个节点上的一个 CPU 核抽象成一个 slot，把“每个受管节点各一个 slot”抽象成一个 stripe；这里的 `SlotManager` 保存单节点容量、任务预留和各节点已用 slot 快照，为任务调度器回答“任务现在能否启动、应否绑定一个具体节点、哪些节点优先承载新子任务”。资源抽象及 owner/follower 分工可由 `pkg/dxf/framework/doc.go` 复核。

crate 边界是 `astersql-dxf-framework-scheduler`（`pkg/dxf/framework/scheduler/Cargo.toml`，库入口为 `lib.rs`）。`lib.rs` 以 `pub mod slots` 声明模块，并通过 `pub use slots::*` 再导出其公共 API。文件直接依赖同 crate 的 `interface::{Result, TaskBase, TaskManager}` 和 `nodes::NodeManager`，不自行访问存储；`TaskManager` 的实际适配器再委托给框架存储管理器。

## 核心职责

1. `SlotManager::update` 从 `TaskManager::used_slots_on_nodes` 拉取节点用量，并以 `NodeManager::get_nodes` 的当前受管节点为边界生成新快照：陈旧节点被丢弃，未上报的受管节点按零使用量补齐。
2. `SlotManager::can_reserve` 先按任务排名计算 stripe 准入；stripe 容量不足时，再尝试在单个节点上按“已用 + 已预留 + 本任务需求”进行最低资源回退。
3. `reserve` / `unreserve` 成对维护按排名排列的 stripe 预留、任务 ID 索引，以及回退节点上的 slot 预留。
4. `update_capacity` 接收节点 CPU 数并只采纳正值；`adjust_eligible_nodes` 与公共函数 `filter_nodes_with_enough_slots` 为子任务调度和负载均衡提供容量过滤。
5. next-gen 模式由实例级 `next_gen` 开关表达：调度层假定集群控制器会按需扩容，因此准入恒成功且不登记/释放本地预留。

本文件只管理调度视图和预留账本，不实际限制执行器 CPU、内存或磁盘，也不负责创建、运行或迁移子任务。

## 主要符号

- `TaskStripes { task: TaskBase, stripes: i32 }`：单个任务的 stripe 预留快照。保存完整 `TaskBase` 是为了后续按 `TaskBase::compare` 排名；保存 `stripes` 是为了快速累计高排名任务的需求。
- `Reservations`：受同一把 `RwLock` 保护的复合状态。`stripes` 按任务排名升序排列；`task_to_index` 把任务 ID 映射到 `stripes` 下标；`slots` 记录具体执行节点上的回退预留。
- `SlotManager`：公共资源管理器。`capacity: AtomicI32` 和 `next_gen: AtomicBool` 是低成本独立状态；`reservations` 保护需要原子一致更新的三个预留结构；`used_slots` 保存可独立整表替换的节点用量快照。
- `SlotManager::new` / `Default::default`：以 `std::thread::available_parallelism()` 初始化容量，查询失败时退回 1，next-gen 默认为关闭，两个映射视图为空。
- `set_next_gen(enabled)`：以 Release 写入 next-gen 模式；其他路径以 Acquire 读取。
- `update(node_manager, task_manager) -> Result<()>`：先取得用量报告，成功后才构造并发布新快照；查询失败直接传播错误，原快照不变。
- `set_used_slots(slots)`：直接替换快照，主要供独立测试或外部注入使用，不校验节点是否受管。
- `can_reserve(task) -> (String, bool)`：返回 `(exec_id, ok)`；stripe 成功或 next-gen 成功时 `exec_id` 为空，单节点回退成功时返回节点 ID，失败时返回空 ID 和 `false`。
- `reserve(task, exec_id)` / `unreserve(task, exec_id)`：登记与释放同一任务的资源预留。调用方必须用相同任务需求和 `exec_id` 配对。
- `capacity()` / `update_capacity(cpu_count)`：Acquire 读取容量；只用正 CPU 数进行 Release 更新。
- `adjust_eligible_nodes(eligible_nodes, required_slots)`：优先返回容量足够的候选节点；若一个也没有，则原样返回全部候选，以保留 Go 的过载调度回退语义。
- `rebuild_task_indexes`：每次 stripe 增删或排序后重建任务 ID 到向量下标的映射。
- `filter_nodes_with_enough_slots`：公共纯函数，按候选列表原顺序保留满足 `used + required <= capacity` 且存在于用量映射中的节点。

## 执行流程

任务启动主链由 `scheduler_manager.rs` 驱动：

1. `Manager::new` 创建并共享 `Arc<SlotManager>`；`Manager::start` 先调用 `NodeManager::refresh_nodes`，后者从受管节点的首个正 `cpu_count` 更新容量。
2. 每轮 `Manager::start_schedulers` 先调用 `SlotManager::update`，让准入判断使用当前受管节点和存储统计。
3. 对 Pending、Running、Resuming 状态的候选任务，管理器调用 `can_reserve`。经典模式下，若没有节点快照立即拒绝；否则累计所有严格高于当前任务排名的 stripe 预留。若 `task.required_slots + higher_rank_reserved <= capacity`，允许 stripe 预留。
4. stripe 放不下时，遍历节点用量快照，寻找满足 `used_slots + reservations.slots[exec_id] + task.required_slots <= capacity` 的节点；找到即返回其 ID。因为底层是 `HashMap`，多个节点同时满足时不承诺固定选择顺序。
5. 调度器工厂创建且 `init` 成功后，`Manager::start_scheduler` 才调用 `reserve`。该函数克隆任务、追加 stripe、按 `TaskBase::compare` 排序、重建索引；若 `exec_id` 非空，还累计该节点的 slot 预留。
6. 调度器完成或 `Manager::stop` 时，以启动时保存的任务快照和节点 ID 调用 `unreserve`。它删除 stripe、重建索引、扣减节点预留，并在值归零时移除节点条目。

节点选择主链有两处：`scheduler.rs::schedule_subtasks` 在建立子任务前执行 `update`，再调用 `adjust_eligible_nodes`；`balancer.rs::balance_subtasks` 直接调用 `filter_nodes_with_enough_slots`，容量不足时不为该任务记账，使低排名任务本轮仍有机会。

## 数据与状态

- 容量是“每节点 slot/stripe 总数”的单个 `i32`，框架假定受管节点同构；`nodes.rs::refresh_nodes` 采用第一个正 CPU 数，而不是维护逐节点容量。
- `used_slots` 是观测到的真实使用量快照；`reservations.slots` 是尚未完全反映到异步用量统计中的单节点预留。回退判断同时计算两者，避免连续启动任务时过量承诺。
- `reservations.stripes` 即使来源于单节点回退也会记录任务，和 Go 注释描述一致；因此其总和可以超过容量，但排名准入只累计当前任务之前的条目。
- 排名由 `TaskBase::compare` 定义为 `priority`、`create_time`、`id` 依次升序，返回较小值表示排名更高（`interface.rs`）。`can_reserve` 的 `take_while(compare(task).is_lt())` 依赖 `stripes` 始终按这一顺序排列。
- `task_to_index` 是派生索引，不是独立事实源；`rebuild_task_indexes` 在排序、删除后全量重建，保证 `unreserve` 能定位移动后的条目。
- `adjust_eligible_nodes` 的空过滤结果不表示完全禁止调度，而是退回原候选集；公共过滤函数自身则严格返回空列表。调用者必须区分这两种契约。

## 依赖与调用关系

上游调用者及入口：

- `scheduler_manager.rs::Manager::{new,start_schedulers,start_scheduler,drive_schedulers,stop}` 分别持有管理器、刷新快照、检查准入、登记预留和释放预留，是完整生命周期的主调用链。
- `nodes.rs::NodeManager::refresh_nodes` 调用 `update_capacity`，把节点发现结果接入容量状态。
- `scheduler.rs::schedule_subtasks` 调用 `update` 和 `adjust_eligible_nodes`，决定新子任务的轮询候选集。
- `balancer.rs::balance_subtasks` 调用 `filter_nodes_with_enough_slots`，再结合最大节点数过滤进行子任务重分配。
- `integrationtests/resource_control_test.rs` 直接使用公开过滤函数，验证 Go 顺序、缺失节点和容量边界语义。

下游依赖：

- `TaskManager::used_slots_on_nodes`（`interface.rs`）提供节点用量；`storage_adapter.rs` 的实现委托给 `manager.GetUsedSlotsOnNodes(context())` 并把错误转换为 `SchedulerError`。
- `NodeManager::get_nodes` 提供稳定克隆的受管节点视图，用于约束快照范围。
- `TaskBase::compare` 提供排名不变量；`TaskBase::required_slots` 同时用于 stripe 需求、单节点回退和节点过滤。
- 标准库 `AtomicI32`、`AtomicBool`、`RwLock` 与 `HashMap` 提供进程内并发状态，不涉及网络连接、线程创建或异步运行时。

`Cargo.toml` 的无条件直接依赖是 proto、schstatus、storage 和 dxfmetric 四个相邻 crate；本文件经 crate 内 `interface` 间接使用协议/存储抽象。大量其余依赖仅在 Windows target 条件段声明，本文件没有条件编译项或平台专用分支。

## 错误处理与边界

- `update` 唯一的业务错误来自 `used_slots_on_nodes`，通过 `?` 原样传播；因为写锁在查询成功和新快照构造完成后才获取，失败不会清空或部分覆盖旧快照。Go 测试明确覆盖该不变量，Rust 测试覆盖成功刷新、缺失补零和缩容剔除。
- 所有 `RwLock` 获取都用 `expect(... lock poisoned)`；线程 panic 导致锁中毒时本文件选择继续 panic，而不是返回 `Result`。这是进程内一致性故障边界。
- 经典模式下 `used_slots` 为空会拒绝准入；next-gen 模式在此检查前直接成功。
- `update_capacity` 忽略零和负值，保留最后一个有效容量。`required_slots`、节点已用数和预留扣减没有在本文件做正数或溢出校验，依赖上游协议与成对调用保证数据有效。
- `unreserve` 找不到任务 ID 时静默返回；若给出的非空 `exec_id` 不存在，也不会报错。若任务需求或节点 ID与 `reserve` 时不同，账本可能不能正确抵消，所以配对参数是调用方契约。
- 重复对同一任务 ID 调用 `reserve` 会产生多个 stripe 条目，而 `task_to_index` 只能保存其中一个下标；当前主链通过运行中调度器表避免重复启动，本文件本身不防重。
- 过滤函数排除用量映射中不存在的候选节点，并使用含等号的容量边界；`adjust_eligible_nodes` 在严格过滤为空时又退回所有候选，这是有意的可用性优先策略，不应误改为硬拒绝。

## 并发与资源生命周期

`SlotManager` 设计为可通过 `Arc` 跨调度组件共享。`capacity`、`next_gen` 使用 Acquire/Release 原子访问；预留复合状态放在一把 `RwLock<Reservations>` 下，保证 stripe 向量、派生索引和节点预留不会被观察到半更新。节点已用量使用另一把 `RwLock`，允许存储刷新与预留操作分别推进；一次 `update` 以整张新映射替换快照。

`can_reserve` 同时持有 `used_slots` 和 `reservations` 的读锁，`reserve`/`unreserve` 只取得预留写锁，`update`/`set_used_slots` 只取得用量写锁；本文件不存在反向获取这两把锁的路径，因此当前实现没有锁顺序环。`adjust_eligible_nodes` 只持有用量读锁完成纯过滤。

预留生命周期由 `Manager` 管理：任务在调度器成功初始化之后登记，正常完成或管理器停止时释放；初始化失败不登记。next-gen 下 `reserve` 与 `unreserve` 都是幂等空操作。文件不启动线程、不创建 channel、不持有文件句柄或事务；唯一外部生命周期是 `TaskManager` 查询调用。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dxf/framework/scheduler/slots.go`，Rust 保留了 Go 的核心算法与不变量：

- `TaskStripes` / `Reservations` 对应 Go 的 `taskStripes`、`reservedStripes`、`task2Index`、`reservedSlots`；Rust 将三者封装在同一个锁保护对象中。
- `update` 同样只保存当前 managed 节点，缺失报告按零处理，存储错误时保留旧快照。
- `can_reserve` 同样按排名累计更高排名 stripe，并在失败后扫描单节点空余；`reserve`/`unreserve` 同样排序、重建索引并维护节点预留。
- `adjust_eligible_nodes` 和过滤函数保留 Go 的候选顺序、容量等号边界、缺失节点排除，以及“无足够节点则退回全部候选”策略。
- Rust 独立测试 `slots_test.rs` 复刻 Go `slots_test.go` 的主要表格与状态变化，包括 next-gen、排名抢占、节点回退、释放后重新准入、缩容快照和无效容量更新。

可见实现差异也应在扩展时保留意识：Go 通过全局 `kerneltype.IsNextGen()` 判定模式，Rust 用每个 `SlotManager` 的 `AtomicBool` 和 `set_next_gen` 注入；Go 初始 CPU 数来自 `cpu.GetCPUCount()`，Rust 使用标准库可用并行度并在失败时回退 1；Go 的用量快照是原子指针，Rust 使用 `RwLock<HashMap<...>>`；Go `updateCapacity` 记录容量变化日志，Rust 当前只更新原子值。这些是当前代码事实，不代表应在本文件中自行补齐全局配置或日志接线。

## 扩展指南

- 修改准入策略时，首先保持 `TaskBase::compare` 的排名语义和 `stripes` 有序不变量；重点修改 `can_reserve`，并同步独立测试 `pkg/dxf/framework/scheduler/slots_test.rs` 与 Go 对照用例 `slots_test.go`。需覆盖高/低排名、总 stripe 超容量、单节点回退及多个可用节点时不依赖固定 HashMap 顺序。
- 增加逐节点异构容量时，单一 `capacity: AtomicI32`、`NodeManager::refresh_nodes` 的首个正 CPU 策略、`can_reserve`、两个节点过滤入口和 balancer 都必须一起重新设计；只改过滤函数会造成启动准入与子任务分配口径不一致。
- 改变预留结构时，保证 `reserve`/`unreserve` 是同一锁下的复合更新，并继续在排序或删除后维护 `task_to_index`。若要支持重复任务 ID，应先明确索引是一对一还是一对多，不能仅修改映射类型而忽略释放语义。
- 改变 next-gen 行为时，必须同时审查 `set_next_gen`、`can_reserve`、`reserve`、`unreserve` 及 `Manager` 的配置接线，避免检查路径和记账路径采用不同模式。
- 修改 `update` 时保留“外部查询失败不发布半成品快照”；新增错误路径应写入独立的 `slots_test.rs`，不要把测试内嵌到生产文件。
- 修改过滤回退时需同步检查 `scheduler.rs::schedule_subtasks`、`balancer.rs::balance_subtasks` 和 `integrationtests/resource_control_test.rs`。将软回退改为硬限制会影响过载时可用性，将缺失节点视为零使用则可能把陈旧或未知节点重新纳入。
- 性能方面，`reserve`/`unreserve` 每次排序或重建索引是线性/对数线性成本；任务数量通常受 `Manager` 并发上限约束。若优化为增量结构，必须证明排名遍历、任务删除及锁内一致性仍等价。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`pkg/dxf/framework/scheduler/slots.rs` 被识别为含 17 个符号的已索引文件。
- RustCodeGraph 源码/符号检查：`slots.rs` 全部 262 行及 `SlotManager` 查询；直接调用证据来自 `scheduler_manager.rs`（`update`、`can_reserve`、`reserve`、`unreserve`）、`scheduler.rs`（`update`、`adjust_eligible_nodes`）、`nodes.rs`（`update_capacity`）、`balancer.rs`（`filter_nodes_with_enough_slots`）。精确 `callers/callees` 子命令在本次环境中未返回结果，因此调用边又以这些已索引调用点逐一复核，没有据此推断未见调用。
- crate/模块证据：`pkg/dxf/framework/scheduler/Cargo.toml`、`pkg/dxf/framework/scheduler/lib.rs`。
- 语义定义与适配证据：`pkg/dxf/framework/doc.go`、`pkg/dxf/framework/scheduler/interface.rs`（`TaskBase::compare`、`TaskManager::used_slots_on_nodes`）、`pkg/dxf/framework/scheduler/storage_adapter.rs`。
- Go 对照：`pkg/dxf/framework/scheduler/slots.go`、`pkg/dxf/framework/scheduler/slots_test.go`。
- Rust 测试证据：`pkg/dxf/framework/scheduler/slots_test.rs`；额外的公开过滤边界证据来自 `pkg/dxf/framework/integrationtests/resource_control_test.rs::slot_filter_keeps_go_order_missing_node_and_capacity_boundaries`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本文能够回答文件存在目的、执行主链、状态不变量、错误/并发边界和安全扩展位置。
