# `pkg/dxf/framework/taskexecutor/slot.rs`

## 文件定位

本文件实现 DXF（Distributed eXecution Framework）执行节点上的 CPU slot 管理器。对应源码为 [`slot.rs`](slot.rs)。它属于 `astersql-dxf-framework-taskexecutor` crate；模块入口 [`lib.rs`](lib.rs) 以 `mod slot` 装配本文件，并通过 `pub use slot::*` 把公开符号重导出给同 crate 的执行管理器和测试使用。

在完整执行链中，`Manager::NewManager` 以节点资源 `NodeResource.TotalCPU` 调用 `newSlotManager`，因此一个 `slotManager` 对应一个执行节点的本地并发容量。它只管理“任务占多少 slot、能否分配、哪些低优先级任务可被抢占”这组内存状态；任务发现、取消、执行线程和持久化均由 `manager.rs`、`task_executor.rs` 及 `TaskTable` 承担。

## 核心职责

- 维护固定总容量 `capacity`、剩余容量 `Slots::available`、正在占用 slot 的 `Slots::tasks`，以及任务 ID 到数组位置的 `Slots::index`。
- 通过 `alloc` 原子地检查并登记一个任务的 `RequiredSlots`；只要真正释放其他任务才可能满足请求，`alloc` 就拒绝分配，不会自行取消任务或透支容量。
- 通过 `canAlloc` 只读评估当前任务能否立即运行，或能否在抢占一组更低优先级任务后运行。
- 通过 `free` 幂等释放已登记任务，通过 `exchange` 调整已运行任务的 slot 配额。
- 通过 `availableSlots` 和 `usedSlots` 暴露容量快照。

这些职责由 `slotManager` 的单个 `Mutex<Slots>` 串行化；本文件没有后台线程、异步任务、I/O 或持久化逻辑。

## 主要符号

- `Slots`：私有可变状态。`index: HashMap<i64, usize>` 指向 `tasks` 下标，`tasks: Vec<TaskBase>` 保存任务快照，`available: i32` 保存剩余 slot。
- `slotManager`：公开结构体但字段私有。`capacity` 创建后不变，`inner` 保护全部可变状态。
- `newSlotManager(capacity: i32) -> slotManager`：以空任务列表、空索引和 `available == capacity` 建立管理器。
- `rebuild(&mut Slots)`：在任务数组排序或删除后，清空并重建 ID 到下标的索引。
- `canAlloc0(&Slots, &TaskBase) -> (bool, Vec<TaskBase>)`：核心评估算法；返回是否最终可满足，以及需要先释放的任务快照。
- `alloc(&self, &TaskBase) -> bool`：仅在当前空闲量已经足够时扣减容量、保存任务、重新排序并重建索引。
- `free(&self, id: i64)`：若 ID 存在则归还其 `RequiredSlots`、删除任务并重建索引；未知 ID 直接返回。
- `canAlloc(&self, &TaskBase)`：加锁后委托 `canAlloc0`。
- `exchange(&self, &TaskBase) -> bool`：按同一任务 ID 计算新旧 `RequiredSlots` 差值，容量允许时更新剩余量与任务快照。
- `availableSlots` / `usedSlots`：分别返回剩余量与 `capacity - available`。
- `TasksForTest`：仅在 `cfg(test)` 下存在，返回排序后任务列表的克隆，供独立测试做白盒断言；它不是生产 API。

## 执行流程

1. `Manager::NewManager` 创建 `Arc<slotManager>`，初始剩余容量等于 `NodeResource.TotalCPU`。
2. `Manager::handleExecutableTasks` 对候选任务调用 `canAlloc`。若剩余量足够，结果为 `(true, [])`；若可通过抢占满足，结果为 `(true, tasks_to_free)`；否则为 `(false, [])`。
3. 当 `tasks_to_free` 非空时，管理器调用 `cancelTaskExecutors` 取消这些低优先级执行器，并结束本轮分配。取消本身不改变 slot 状态；执行器退出后的清理路径才调用 `free`。
4. 当任务可直接运行时，`Manager::startTaskExecutor` 调用 `alloc`。`alloc` 在同一把锁内重新检查容量；成功后扣减 `available`、保存 `TaskBase` 克隆，按优先级顺序排序并重建索引。
5. 若后续取得运行时、查找工厂或初始化执行器失败，管理器立即 `free`；正常执行线程结束时也在关闭、注销执行器后 `free`。
6. 已运行任务的 `RequiredSlots` 变化由 `BaseTaskExecutor::runLoop` 或 `tryModifyTaskRequiredSlots` 调用 `exchange`。缩容增加 `available`；扩容先检查差值是否可由当前剩余量覆盖。资源通知失败的扩容路径会用旧 `TaskBase` 再次 `exchange` 回滚。

`canAlloc0` 的抢占扫描依赖 `tasks` 的排序不变量：低优先级候选位于前部。它只累加满足 `running.Compare(task) >= 0` 的任务；遇到更高优先级任务（`Compare < 0`）立即停止，避免抢占优先级更高的工作。累计可释放量加当前空闲量达到请求值时，返回已收集列表。

## 数据与状态

核心守恒关系是 `usedSlots() == capacity - availableSlots()`。正常输入下，成功 `alloc` 使 `available` 减少任务的 `RequiredSlots`，成功 `free` 恢复登记快照中的数量，成功 `exchange` 使其减少 `new.RequiredSlots - old.RequiredSlots`。`slot_test.rs::test_slot_manager_used_slots` 直接验证这一关系。

`tasks` 保存 `TaskBase` 的值拷贝，而不是对调用者对象的共享引用。因此调用者修改原任务不会隐式改变已登记配额，必须显式调用 `exchange`。`index` 是派生缓存：`alloc` 排序后和 `free` 删除后调用 `rebuild`；`exchange` 不改变任务 ID 或数组位置，因此只替换对应元素，不重建索引。

排序使用 `TaskBase::Compare`。其定义位于 `interface.rs`，依次比较 `Priority`、`CreateTime`、`ID`，数值更小/时间更早/ID 更小返回 `-1`。`alloc` 的排序闭包使低优先级任务排在前面，供抢占扫描使用。独立测试 `slot_test.rs::test_task_base_compare_uses_priority_then_create_time_then_id` 验证比较键顺序。

本实现未在入口校验负数 `capacity` 或负数 `RequiredSlots`，也未阻止同一 ID 重复 `alloc`。调用方必须保证容量和请求量非负，并保证一个任务只登记一次；否则容量守恒与索引唯一性可能失效。

## 依赖与调用关系

直接类型依赖只有 crate 根重导出的 `TaskBase`，以及标准库 `HashMap`、`Mutex`。`Cargo.toml` 将本文件归入 `astersql-dxf-framework-taskexecutor`；与完整 DXF 的大多数 crate 依赖目前位于 `target.'cfg(any())'.dependencies`，不会在常规配置启用。本文件本身没有 feature 或条件编译分支，只有 `TasksForTest` 受 `cfg(test)` 控制。

主要上游调用边如下：

- `manager.rs::NewManager -> newSlotManager`
- `manager.rs::handleExecutableTasks -> canAlloc`
- `manager.rs::startTaskExecutor -> alloc/free`
- `manager.rs` 的执行线程收尾及失败分支 `-> free`
- `task_executor.rs::runLoop -> exchange`
- `task_executor.rs::tryModifyTaskRequiredSlots -> exchange`，必要时再次调用以回滚

主要下游调用为 `alloc/canAlloc -> canAlloc0`，`alloc/free -> rebuild`，以及排序和抢占判断 `-> TaskBase::Compare`。`usedSlots -> availableSlots`。RustCodeGraph 的文件查询确认 `slot.rs` 已被索引并显示 13 个符号；文件反向关系报告它被 24 个文件使用。精确 callers/callees 查询在当前索引上超时且未产生边列表，因此上述直接调用边另由目标目录源码搜索和对应函数源码核对。

## 错误处理与边界

该 API 不返回错误类型：容量不足、需要先抢占、未知任务等业务结果通过布尔值或 `(bool, Vec<TaskBase>)` 表达。`free` 对未知 ID 幂等无操作；`exchange` 对未知 ID 或扩容不足返回 `false`，且不修改状态；`canAlloc0` 在无法通过允许的抢占集合满足请求时返回 `(false, [])`，不会返回不完整的候选集合。

互斥锁中毒使用 `expect("slot lock poisoned")`，因此持锁线程 panic 后，后续访问会继续 panic，而不是恢复或把错误上交。调用者需把这视为进程内不变量破坏。

`canAlloc` 只是评估，不为调用者预留容量。锁释放后，其他线程可能先完成 `alloc` 或 `exchange`，所以 `Manager::startTaskExecutor` 必须并且确实再次调用 `alloc`；其失败是正常竞争结果。抢占评估同样只返回任务快照，真正释放发生在被取消执行器走到 `free` 之后。

## 并发与资源生命周期

`slotManager` 通常被包在 `Arc` 中，由管理器轮询线程、任务执行线程及参数监控线程共享。`Mutex<Slots>` 把评估、分配、释放、交换以及读快照都串行化，确保 `tasks`、`index`、`available` 的一次操作彼此一致。与 Go 版的 `RWMutex + atomic.Int32` 不同，Rust 版没有读写锁或独立原子计数；即使读取剩余量也会获取同一互斥锁，模型更简单但读操作无法并行。

资源生命周期从成功 `alloc` 开始，到对应 `free` 结束。管理器对运行时租约获取失败、工厂缺失、执行器初始化失败和执行线程正常/异常返回均布置了释放路径。抢占只发出取消信号，不提前归还 slot，从而避免旧执行器尚未退出时新任务超额启动。

锁内只进行内存计算、克隆、排序和索引重建，不调用外部执行器、表存储或取消逻辑，因此没有跨组件锁嵌套。复杂度方面，`canAlloc0` 最坏扫描全部任务；`alloc` 排序后重建索引，`free` 删除后重建索引，均随节点上的并发任务数增长。

## 与 Go 版本的对应关系

Go 对照实现是 `pkg/dxf/framework/taskexecutor/slot.go`，Rust 独立测试对应 `slot_test.rs`，Go 测试对应 `slot_test.go`。核心语义保持一致：容量来自 CPU 数、按 `TaskBase.Compare` 排序、仅抢占更低优先级任务、`alloc` 不直接执行抢占、未知任务释放无操作，以及 `exchange` 以新旧 slot 差值更新容量。

实现形式存在以下差异：

- Go 的管理器保存 `*proto.TaskBase`，Rust 保存 `TaskBase` 克隆；Rust 返回的抢占列表也是独立快照。
- Go 用 `RWMutex` 保护任务表并用 `atomic.Int32` 保存可用量；Rust 把三项可变状态统一放在一个 `Mutex<Slots>` 内。
- Go 在 `free` 中删除单个索引并更新后续项；Rust 的 `rebuild` 清空后重建全部索引，结果等价但常数开销不同。
- Rust 增加 `cfg(test)` 的 `TasksForTest` 以替代 Go 测试对包内字段的直接访问。

`slot_test.rs` 移植了 Go 测试的主要场景：直接分配、容量不足、抢占候选及顺序、释放、未知任务交换、扩缩容、重复交换和扩容后阻止并发分配；Rust 还单独验证了 `usedSlots` 与 `TaskBase::Compare`。

## 扩展指南

- 若改变优先级或抢占规则，应同时修改 `canAlloc0`、`alloc` 的排序表达式以及 `TaskBase::Compare` 的契约，并扩展独立测试 `pkg/dxf/framework/taskexecutor/slot_test.rs`；还要核对 `Manager::handleExecutableTasks` “取消后中断本轮”的流程。
- 若增加预留、超卖、动态总容量或多维资源，不能只改 `available`；应重新定义 `capacity/available/tasks/index` 的守恒关系，并检查 `NewManager`、`startTaskExecutor`、全部 `free` 路径和运行时 `exchange` 回滚。
- 若优化性能，可考虑减少每次排序/全量 `rebuild`，但必须保持任务顺序和 ID 索引同步；测试应覆盖相同优先级下的 `CreateTime`、ID 顺序以及中间元素删除。
- 若增加输入校验，需明确负容量、零/负 `RequiredSlots` 和重复 ID 的期望行为；当前代码没有这些防线，不能在调用方仍依赖现状时静默改变。
- 测试逻辑应继续放在独立的 `slot_test.rs`，不要嵌入生产源文件；生产文件中的 `TasksForTest` 仅作为受 `cfg(test)` 限制的观察接口。

兼容性风险主要来自改变抢占次序或布尔/候选列表语义；正确性风险集中在容量重复扣减/归还和索引失配；性能风险集中在锁竞争、任务克隆、排序和全量索引重建。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；`files --filter pkg/dxf/framework/taskexecutor` 确认目标及相邻 Rust/Go 文件；`node --file pkg/dxf/framework/taskexecutor/slot.rs` 读取 127 行源码并报告 13 个符号与 24 个反向使用文件；`query` 定位 `slotManager`、`newSlotManager`、`canAlloc0`、`canAlloc`、`free`、`exchange`、`availableSlots`、`usedSlots`、`rebuild` 和 `TasksForTest`。精确 callers/callees 命令在当前索引上超时、无输出，未把它作为调用边证据。
- 生产源码：[`slot.rs`](slot.rs)、[`lib.rs`](lib.rs)、[`interface.rs`](interface.rs)、[`manager.rs`](manager.rs)、[`task_executor.rs`](task_executor.rs)。
- crate 边界：[`Cargo.toml`](Cargo.toml)。
- Go 对照：[`slot.go`](slot.go)。
- 独立测试：[`slot_test.rs`](slot_test.rs) 与 [`slot_test.go`](slot_test.go)；相邻管理器和执行器测试中的调用点通过目录级源码搜索核对。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求目标文档存在，并且固定的十一个二级标题各出现一次。
