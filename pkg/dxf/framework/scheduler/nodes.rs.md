# `pkg/dxf/framework/scheduler/nodes.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-scheduler` crate 的节点视图层。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod nodes` 声明模块并通过 `pub use nodes::*` 再导出其公开项；[`Cargo.toml`](Cargo.toml) 则表明该 crate 直接依赖 DXF 的 proto、调度状态、存储和指标 crate。本文件自身不负责发现集群节点，也不直接访问 SQL/元数据表，而是通过 [`TaskManager`](interface.rs) 抽象接收持久化节点数据。

它位于调度主链的共享状态位置：[`scheduler_manager.rs`](scheduler_manager.rs) 构造唯一的 `Arc<NodeManager>` 并注入调度器、槽位管理器和均衡器；[`scheduler.rs`](scheduler.rs) 与 [`balancer.rs`](balancer.rs) 读取其 managed 节点快照来选择子任务执行节点；[`slots.rs`](slots.rs) 用同一快照过滤节点资源用量。

## 核心职责

`nodes.rs` 提供三组相互关联但边界清晰的能力：

1. `NodeManager::maintain_live_nodes` 比较本轮服务发现给出的执行器 ID 与上一轮快照，从 `TaskManager::all_nodes` 返回的 managed 节点中识别失联节点，并调用 `TaskManager::delete_dead_nodes` 删除它们。
2. `NodeManager::refresh_nodes` 从存储刷新 managed 节点缓存，同时以列表中第一个正 `cpu_count` 更新全局 `SlotManager` 容量。
3. `NodeManager::get_nodes` 提供隔离的节点列表副本，`filter_by_scope` 再按节点角色和任务 `target_scope` 生成有序的执行器 ID 列表。

这里维护的是两个不同视图：`previous_live_nodes` 是服务发现快照，只用于判断是否需要清理；`nodes` 是框架持久化的 managed 节点快照，才是调度、均衡和槽位统计的输入。二者不能互相替代。

## 主要符号

- `pub struct NodeManager`：节点状态容器。它没有显式生命周期参数，也不拥有后台线程；并发共享由调用方通常包装为 `Arc<NodeManager>`。
- `previous_live_nodes: RwLock<HashSet<String>>`：上一轮成功发布的存活执行器集合。集合语义会消除输入 `live_exec_ids` 中的重复项，并忽略输入顺序。
- `nodes: RwLock<Arc<Vec<ManagedNode>>>`：当前 managed 节点快照。内部 `Arc` 让整表替换保持原子式边界，但公开读取最终仍深克隆 `Vec<ManagedNode>`。
- `NodeManager::new() -> Self`：调用 `Default`，初始存活集合和 managed 列表均为空。
- `maintain_live_nodes(&self, task_manager: &dyn TaskManager, live_exec_ids: &[String]) -> Result<Vec<String>>`：返回本轮实际识别并成功删除的死节点 ID；相同存活集合时返回空向量且不访问 `TaskManager`。
- `refresh_nodes(&self, task_manager: &dyn TaskManager, slot_manager: &SlotManager) -> Result<Vec<ManagedNode>>`：返回刚读取的 managed 节点，同时发布缓存副本。
- `get_nodes(&self) -> Vec<ManagedNode>`：返回稳定副本；修改返回值不会影响内部快照。
- `set_nodes(&self, nodes: Vec<ManagedNode>)`：直接覆盖缓存，主要用于测试和显式注入；不会同步容量，也不会修改存活集合。
- `filter_by_scope(nodes: &[ManagedNode], target_scope: &str) -> Vec<String>`：保留输入顺序，返回角色严格匹配有效 scope 的节点 ID。

本文件没有模块级常量、trait、枚举、条件编译项或异步函数。

## 执行流程

存活节点维护流程如下：

1. `maintain_live_nodes` 将 `live_exec_ids` 复制成 `HashSet<String>`，取得 `previous_live_nodes` 写锁。
2. 若新旧集合相同，立即返回空结果，避免 `all_nodes` 和删除存储调用。
3. 若集合变化，调用 `TaskManager::all_nodes`，将“不在当前存活集合”的每个 `ManagedNode.id` 收集为 `dead_nodes`。
4. 非空时调用 `TaskManager::delete_dead_nodes`；只有读取与删除均成功，才把 `previous_live_nodes` 替换为当前集合并返回删除列表。

managed 节点刷新流程如下：

1. `refresh_nodes` 调用 `TaskManager::all_nodes`；错误直接返回，旧缓存与容量均保持不变。
2. 按存储返回顺序寻找第一个大于零的 `cpu_count`，找到后调用 `SlotManager::update_capacity`。没有正值时保留既有容量。
3. 将完整 `new_nodes` 克隆进 `nodes` 快照并返回原列表；空列表因此会清空可调度节点，但不会清零已有容量。

调度消费流程是：`Manager::start` 首次调用 `refresh_nodes`；`BaseScheduler::switch_to_next_step` 读取 `get_nodes`，在扩展未指定实例时调用 `filter_by_scope`，然后规划并派发子任务；`Balancer::balance` 同样先读取快照并按 scope 过滤，再与扩展返回的 eligible 实例取交；`SlotManager::update` 则只为 `get_nodes` 中仍受管理的节点建立已用槽位快照。

## 数据与状态

`ManagedNode` 定义在 [`interface.rs`](interface.rs)，由 `id`、`role`、`cpu_count` 三个字段组成。`id` 是调度和清理使用的身份键；`role` 是 scope 过滤依据；`cpu_count` 是单节点 slot capacity 的参考值。

`previous_live_nodes` 的提交点有意放在死节点删除成功之后。这保证删除失败时旧快照仍在，下一轮传入相同的新服务发现集合仍会重新访问存储并重试删除。反之，一旦提交成功，后续相同集合会快速返回。

`nodes` 采用“构造完整新值后整表替换”的快照模型。`get_nodes` 不暴露锁守卫或内部 `Arc`，消费者拿到的是独立 `Vec`；因此消费者可排序、截断或清空自己的结果而不会回写 `NodeManager`。节点顺序源自 `TaskManager::all_nodes`，`refresh_nodes` 和 `filter_by_scope` 均不重排。

容量不是本文件独占状态，而是写入 [`SlotManager::capacity`](slots.rs)。选用“第一个正 CPU 数”隐含当前调度器把该值视为全局单节点容量，而不是求和、最小值或逐节点容量表；节点异构时，存储返回顺序会影响所选容量。

## 依赖与调用关系

上游与装配关系：

- [`scheduler_manager.rs`](scheduler_manager.rs) 的 `Manager::new` 创建 `Arc<NodeManager>`，同时注入 `Param` 和 `Balancer`；`Manager::start` 是当前 Rust 生产代码中 `refresh_nodes` 的直接入口。
- `maintain_live_nodes` 在当前 Rust 生产代码中没有直接调用者；[`nodes_test.rs`](nodes_test.rs) 直接验证它。也就是说，函数具备清理语义，但 Rust 管理器目前未像 Go 一样启动周期存活维护循环。
- `set_nodes` 用于同 crate 测试构造节点快照，不应被当成正常存储刷新路径。

下游关系：

- `maintain_live_nodes` 依赖 `TaskManager::all_nodes` 和 `TaskManager::delete_dead_nodes`；`refresh_nodes` 依赖 `TaskManager::all_nodes` 与 `SlotManager::update_capacity`。
- [`scheduler.rs`](scheduler.rs) 的 `BaseScheduler::switch_to_next_step` 调用 `get_nodes` 和 `filter_by_scope`，无候选节点时返回 `no available TiDB node to dispatch subtasks`。
- [`balancer.rs`](balancer.rs) 的 `Balancer::balance` 调用同一对 API；它还与业务扩展的 `eligible_instances` 求交，无候选节点时返回 `no eligible nodes to balance subtasks`。
- [`slots.rs`](slots.rs) 的 `SlotManager::update` 读取 `get_nodes`，丢弃已不受管理节点的上报，并把缺失上报按零已用槽位处理。

RustCodeGraph 将目标文件标记为被 17 个文件使用，并能识别 `get_nodes` 的调度/槽位消费者及 `filter_by_scope` 的调度、均衡和测试消费者；针对关联方法执行精确 `callers/callees` 命令未输出边，因此上述具体边还由这些已索引消费者源码交叉核验。

## 错误处理与边界

两项存储操作均使用 crate 的 `Result<T>`/`SchedulerError` 原样向上传播，不吞错、不记录日志，也不在本层重试。`maintain_live_nodes` 在 `all_nodes` 或 `delete_dead_nodes` 失败时不会更新 `previous_live_nodes`；`refresh_nodes` 在 `all_nodes` 失败时不会更新节点快照或容量。

锁中毒通过 `expect("... lock poisoned")` 触发 panic，而不是转成 `SchedulerError`。尤其 `maintain_live_nodes` 在持有 `previous_live_nodes` 写锁期间执行可能较慢的 `all_nodes` 和 `delete_dead_nodes`；这保证同一实例的维护调用串行且提交点简单，但也意味着存储延迟会阻塞其他存活维护调用。

边界行为包括：空 `live_exec_ids` 会把所有 managed 节点视为死亡；重复存活 ID 被集合去重；空 managed 刷新清空快照但保留已有容量；全部 `cpu_count <= 0` 时不更新容量；空 scope 在存在任一 `role == "background"` 节点时只选 background，否则只选空角色；显式 scope 始终严格按字符串相等匹配。未知角色不会自动回退。

## 并发与资源生命周期

`NodeManager` 只持有内存状态，没有文件、网络连接、任务句柄、定时器或析构逻辑。`RwLock` 允许并发 `get_nodes`，并将刷新、注入和存活快照提交串行化；两个字段使用不同锁，所以 managed 快照刷新与存活维护可以并发进行，并不存在跨两套视图的事务一致性保证。

`refresh_nodes` 在调用存储时尚未获取 `nodes` 写锁，只在发布完整快照时短暂持锁。`get_nodes` 在读锁内克隆向量，克隆完成即释放锁。`set_nodes` 接管传入向量并整表替换。相比之下，`maintain_live_nodes` 如上所述会跨存储调用持有存活集合写锁。

当前 Rust 文件没有创建周期任务；周期性调用必须由外部生命周期所有者安排。`Manager::start` 只做一次 `refresh_nodes`，`Manager::cancel/stop` 也无需清理本文件资源。若未来增加后台循环，必须同时定义取消、线程/任务 join、错误退避和锁持有边界，不能仅照搬同步方法反复调用。

## 与 Go 版本的对应关系

直接对照文件是 [`nodes.go`](nodes.go)，测试对照是 [`nodes_test.go`](nodes_test.go)。Rust 的 `NodeManager`、`maintain_live_nodes`、`refresh_nodes`、`get_nodes`、`filter_by_scope` 分别对应 Go 的同名驼峰实现；关键语义已经对齐：新旧存活集合相同即跳过、删除失败不发布新存活快照、选择第一个正 CPU 数、返回节点副本，以及 background 对空 scope 的优先级。Go 的 `SlotManager.updateCapacity` 本身也忽略非正值，所以 Rust 在调用前过滤正数与最终容量行为一致。

仍存在明确的迁移/接线差异：

- Go `maintainLiveNodes` 内部调用 `GetLiveExecIDs`，Rust 把 `live_exec_ids` 作为参数传入，因此服务发现属于调用方责任。
- Go 提供 `maintainLiveNodesLoop` 与 `refreshNodesLoop`，由 Go `Manager.Start` 启动 ticker；当前 Rust `Manager::start` 只同步调用一次 `refresh_nodes`，且没有生产调用连接 `maintain_live_nodes`。
- Go 版本包含 context 取消、采样日志、trace region/flight recorder 与 `syncRefresh` failpoint；Rust 文件没有对应机制，并将错误交给调用者。
- Go managed 快照使用 `atomic.Pointer`，Rust 使用 `RwLock<Arc<Vec<_>>>`；两者都提供整表发布和返回副本，但并发实现及锁中毒行为不同。
- Go 构造器按 server ID 配置日志器，Rust `new` 无参数且不拥有日志器。

这些差异应被视为当前代码事实；本文不据此宣称 Rust 已具备 Go 的周期维护、观测或故障注入能力。

## 扩展指南

若扩展节点选择规则，应优先修改 `filter_by_scope`，并同步 [`nodes_test.rs`](nodes_test.rs) 的表驱动用例；还要检查 [`scheduler.rs`](scheduler.rs) 与 [`balancer.rs`](balancer.rs) 是否仍应共享完全相同的规则。保持输入顺序很重要，因为后续截断和轮询分配可观察该顺序。

若增加节点字段或异构容量策略，应同时检查 `ManagedNode`、`refresh_nodes`、`SlotManager` 的容量/已用槽位模型及 Go `proto.ManagedNode`/`nodes.go`。直接把“第一个正 CPU 数”改成最小、最大或逐节点容量会改变 admission、均衡和兼容行为，必须有独立 Rust 测试，而不是把测试写进本源文件。

若接通存活维护生产路径，应从 `Manager` 生命周期或等价 owner 入口接线，服务发现结果显式传给 `maintain_live_nodes`；需要补充失败重试、取消和周期测试，并验证删除失败时快照不前移。不要让 `set_nodes` 绕过存储成为生产刷新捷径。

修改锁策略时需特别审查：`maintain_live_nodes` 的串行提交不变量、`get_nodes` 的副本隔离、两个视图无需原子同步这一现状，以及存储调用期间持锁的延迟风险。性能上最显著的成本是 `get_nodes` 每次深克隆所有 `String`；若改为共享快照 API，必须防止消费者原地修改内部数据。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 7,032 个 Rust 文件；`node --file pkg/dxf/framework/scheduler/nodes.rs` 读取了目标文件全部 127 行，并列出 17 个使用文件。
- RustCodeGraph：读取了 [`nodes_test.rs`](nodes_test.rs)、[`scheduler_manager.rs`](scheduler_manager.rs)、[`scheduler.rs`](scheduler.rs)、[`balancer.rs`](balancer.rs)、[`slots.rs`](slots.rs)、[`interface.rs`](interface.rs) 与 [`lib.rs`](lib.rs) 的相关已索引源码；图查询确认 `filter_by_scope` 被调度、均衡和 scope 测试消费，`get_nodes` 被调度、均衡和槽位更新消费。
- crate/模块证据：[`Cargo.toml`](Cargo.toml) 的 package、lib path、porting 元数据和依赖；[`lib.rs`](lib.rs) 的 `pub mod nodes`、公开再导出及独立 `nodes_test` 模块声明。
- Go 对照：[`nodes.go`](nodes.go)、[`nodes_test.go`](nodes_test.go) 及 [`slots.go`](slots.go) 的 `updateCapacity`，用于核对周期循环、失败提交点、容量更新、快照复制和 scope 语义。
- Rust 测试：[`nodes_test.rs`](nodes_test.rs) 覆盖死节点删除与重复快照快速返回、首个正 CPU 容量、返回副本隔离、空刷新保留容量和九组 scope 边界；[`slots_test.rs`](slots_test.rs) 额外覆盖刷新后槽位快照剔除缩容节点。
- 本任务是纯文档分析，按计划不运行 Cargo；只执行任务指定的 Markdown 结构验证，并人工核对公开符号、当前生产接线、Go 差异与安全扩展入口。
