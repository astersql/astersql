# `pkg/resourcemanager/schedule.rs`

## 文件定位

`schedule.rs` 是 `astersql-resourcemanager` crate 的调度执行层。crate 入口 `pkg/resourcemanager/lib.rs` 将它声明为私有 `schedule` 模块，而其中三个方法都实现于公开类型 `rm::ResourceManager`：`schedule` 负责一次全池扫描，`schedulePool` 负责为单池选出命令，`Exec` 负责把命令落实为池容量变化。它不定义调度策略本身；策略由 `astersql-resourcemanager-scheduler` 依赖提供的 `Scheduler::Tune` 和 `Command` 表达。

运行时入口位于 `pkg/resourcemanager/rm.rs::ResourceManager::Start`：后台线程以 100 ms 的接收超时为节拍，超时后调用 `manager.schedule()`。因此本文件处在“池注册与生命周期管理”和“具体 CPU 等调度策略”之间，是把周期触发、策略决策和 `GoroutinePool::Tune` 串起来的桥接层。

## 核心职责

- `ResourceManager::schedule`：取得当前 `ShardPoolMap` 的共享快照，遍历所有已注册 `PoolContainer`；明确排除 `util::DistTask`，其余池依次执行决策与命令应用。
- `ResourceManager::schedulePool`：先用 `GoroutinePool::Running` 排除没有运行中 worker 的池，再按 `ResourceManagerInner::scheduler` 的既定顺序查询 `Scheduler::Tune`，返回第一个通过守卫的非 `Hold` 命令。
- `ResourceManager::Exec`：执行 `Hold` 快速返回、最小调容间隔检查、单步降容或升容，以及相对原始并发度的升容上限检查。

职责边界很明确：本文件不注册池、不启动线程、不采集 CPU，也不计算具体策略。注册/启停属于 `pkg/resourcemanager/rm.rs`，策略接口和实现属于 `pkg/resourcemanager/scheduler/`，池与组件抽象来自 `crate::util` 的再导出。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `ResourceManager::schedule(&self)` | `pub` 但带 `#[doc(hidden)]`。执行一次完整调度轮次；正常生产入口是 `ResourceManager::Start` 的后台循环，测试也可直接调用。 |
| `ResourceManager::schedulePool(&self, pool: &PoolContainer) -> Command` | `pub` 且隐藏文档。只选择命令，不修改池；按调度器列表顺序短路。 |
| `ResourceManager::Exec(&self, pool: &PoolContainer, command: Command)` | 公开命令执行入口，保留 Go 风格名称。它可能静默拒绝命令，也可能调用一次 `pool.Pool.Tune`。 |
| `Command::{Hold, Downclock, Overclock}` | 定义于 `pkg/resourcemanager/scheduler/scheduler.rs`；分别表示保持、减一和加一并发。 |
| `PoolContainer` | 定义于 `pkg/resourcemanager/util/util.rs`，把 `Arc<dyn GoroutinePool>` 与所属 `Component` 绑定。 |
| `MinSchedulerInterval` / `MaxOverclockCount` | 同样来自 `util.rs`；当前分别为原子可调的 200 ms 和固定增量上限 1。 |

文件没有模块级常量、独立类型、trait 或条件编译分支；全部业务逻辑集中在 `impl ResourceManager` 的三个方法内。

## 执行流程

一次生产调度轮次从 `rm.rs::ResourceManager::Start` 开始：后台线程对退出通道执行 `recv_timeout(100 ms)`，发生超时则进入 `schedule`，收到退出信号或通道断开则结束。

1. `schedule` 对 `inner.poolMap` 取读锁，克隆其中的 `Arc<ShardPoolMap>`，随即结束该读锁借用；之后在映射快照上调用 `Iter`。
2. 遍历到 `Component::DistTask` 时直接返回该次闭包调用，不查询调度器也不调容。
3. 对其他组件调用 `schedulePool`。若 `Running() == 0`，立即得到 `Hold`。
4. 若池正在运行，按 `inner.scheduler` 的向量顺序调用 `Scheduler::Tune(component, pool)`。`Hold` 仅表示当前调度器不采取动作，会继续询问后续调度器。
5. 对 `Downclock` 额外检查：容量已经是 1，或 `Running() > Cap()` 时忽略该命令并继续询问后续调度器；这避免降到零以及在实际运行数已超过容量时继续缩容。
6. 首个通过守卫的 `Downclock` 或任意 `Overclock` 立即返回；全部调度器均未给出可用命令时返回 `Hold`。
7. `schedule` 将结果交给 `Exec`。`Hold` 立即结束；非 `Hold` 只有在 `LastTunerTs().elapsed()` 严格大于 `MinSchedulerInterval.Load()` 时才继续。
8. `Downclock` 用 `Cap().wrapping_sub(1)` 计算新容量，记录 debug 日志后调用 `Tune`。`Overclock` 用 `wrapping_add(1)`，若新值大于 `GetOriginConcurrency().wrapping_add(MaxOverclockCount)` 则拒绝，否则记录日志并调用 `Tune`。

## 数据与状态

本文件自身不拥有可变状态。它读取 `ResourceManagerInner` 中的两类共享数据：`RwLock<Arc<ShardPoolMap>>` 保存池集合，`Vec<Box<dyn Scheduler + Send + Sync>>` 保存有序策略链。`schedulePool` 的结果受调度器顺序影响：较早出现且通过守卫的非 `Hold` 命令优先，后续调度器不会再执行。

每个池的动态状态通过 `GoroutinePool` trait 读取：`Running`、`Cap`、`LastTunerTs`、`Name` 和 `GetOriginConcurrency`。唯一写动作是 `Tune(new_capacity)`，具体容量存储、时间戳更新以及 worker 调整由池实现负责；本文件不缓存这些值，也不自行更新最近调容时间。

两个重要不变量来自执行守卫：正常 `schedulePool` 路径不会在 `Cap() == 1` 时产生 `Downclock`，而 `Overclock` 在普通数值范围内不会超过“原始并发度 + `MaxOverclockCount`”。但 `Exec` 是公开方法，调用者可绕过 `schedulePool`；因此 `Exec(Downclock)` 本身不保证最小容量为 1。

## 依赖与调用关系

上游调用关系为 `ResourceManager::Start` → `ResourceManager::schedule` → `ResourceManager::schedulePool` → `Scheduler::Tune`，随后 `schedule` → `ResourceManager::Exec` → `GoroutinePool::Tune`。RustCodeGraph 的精确节点查询确认 `schedule` 在 `pkg/resourcemanager/schedule.rs:33` 调用同文件的 `schedulePool`（第 53 行）和 `Exec`（第 76 行）；`rm.rs:131` 给出生产周期入口。

直接依赖如下：

- `std::sync::Arc`：从 `RwLock` 内克隆池映射共享指针，使遍历不需要持续持有外层读锁。
- `crate::rm::ResourceManager`：被扩展的核心类型及其内部池映射、调度器列表。
- `crate::scheduler::Command`：策略结果枚举；`crate::scheduler` 在 `lib.rs` 中再导出 `scheduler_dependency`。
- `crate::util::{PoolContainer, DistTask, MinSchedulerInterval, MaxOverclockCount}`：池抽象、组件分类及调容限制；`crate::util` 再导出调度器依赖 crate 的公共工具类型。
- `log::debug!`：仅在真正准备调用 `Tune` 的升/降容分支记录池名、原容量和目标容量。

`pkg/resourcemanager/Cargo.toml` 将本 crate 命名为 `astersql-resourcemanager`，库入口为 `lib.rs`；与本文件直接相关的声明依赖是 `log = "0.4"` 和本地 `scheduler_dependency = astersql-resourcemanager-scheduler`。metadata 明确 Go 对照包为 `pkg/resourcemanager`。

## 错误处理与边界

这些方法不返回 `Result`。策略拒绝均用 `Hold` 或提前返回表达：空闲池、冷却期未过、超频上限已到以及无有效调度器命令都不是错误。`LastTunerTs().elapsed()` 若因时间戳位于未来而失败，会通过 `unwrap_or_default()` 视为经过 0 时长，从而安全地跳过调容。

可观察的失败边界主要是锁中毒：`schedule` 对 `poolMap` 读锁调用 `expect("resource manager pool map lock poisoned")`，发生中毒会 panic。调度器的 `Tune` 和池的查询/调谐 trait 均无错误返回，因此实现方的 panic 会沿调用栈传播，本文件没有捕获层。

冷却判断使用 `<=` 返回，所以只有“严格大于”最小间隔才执行。升降运算刻意使用 `wrapping_add` / `wrapping_sub` 对齐 Go `int32` 的运行时回绕；极值输入不会只在 Rust debug 构建下溢出 panic。需要注意，极值回绕会使通常的容量上下限语义失真，这是兼容行为而非有效池容量建议。

## 并发与资源生命周期

`ResourceManager` 通过 `Arc<ResourceManagerInner>` 可跨线程克隆，调度循环由 `rm.rs::Start` 交给 `WaitGroupWrapper::Run`。`Stop` 停止 CPU observer、发送退出信号并等待线程结束；这些生命周期动作不在本文件内，但决定 `schedule` 的周期性调用范围。

`schedule` 只在克隆 `Arc<ShardPoolMap>` 时短暂持有外层 `RwLock` 读锁，随后通过映射自己的 `Iter` 遍历。这让 `Reset` 可以替换外层映射，而已经开始的轮次仍安全地完成旧快照遍历。池对象本身是 `Arc<dyn GoroutinePool>` 且 trait 要求 `Send + Sync`；调度器对象也要求 `Send + Sync`，具体实现必须保证查询和 `Tune` 的线程安全。

本文件没有创建线程、异步任务或通道，也没有显式释放池。调容节流依赖每个池正确实现 `LastTunerTs` 并在 `Tune` 后更新它；若实现不更新（例如 `schedule_test.rs` 使用的 `MockGPool` 始终返回十秒前），连续直接调用 `Exec` 仍可能每次通过时间守卫。

## 与 Go 版本的对应关系

Rust 实现逐分支对应 `pkg/resourcemanager/schedule.go`：同样遍历 `poolMap`、跳过 `DistTask`、空闲时 `Hold`、按调度器顺序忽略 `Hold`、拒绝不安全的 `Downclock`，并在 `Exec` 中应用最小间隔和最大超频增量。`pkg/resourcemanager/Cargo.toml` 的 `package.metadata.porting.go-package` 也将该 crate 映射到 `pkg/resourcemanager`。

已核对的实现差异主要是语言适配：

- Go 使用 `time.Since(last) > interval`；Rust 使用 `SystemTime::elapsed().unwrap_or_default()` 后保持相同的严格大于语义，并额外安全处理未来时间戳。
- Go 的 `int32` 加减自然按二进制回绕；Rust 显式使用 `wrapping_add/sub`，由 `migration_aster_unit_test.rs::exec_wraps_i32_capacity_edges_like_go` 固化。
- Go 的指针接收者和接口值对应 Rust 的共享引用、`Arc<dyn GoroutinePool>` 与 `Box<dyn Scheduler + Send + Sync>`。
- Go 测试 `schedule_test.go::TestSchedulerOverloadTooMuch` 与 Rust 独立测试 `schedule_test.rs::TestSchedulerOverloadTooMuch` 都验证原始容量 1 时最多升到 2；Rust 的 `migration_aster_unit_test.rs` 进一步覆盖了命令顺序、缩容守卫、冷却期、`DistTask` 跳过和生命周期。

当前证据未显示本文件存在未接线、占位或简化实现；它已由 `ResourceManager::Start` 接入生产后台循环。

## 扩展指南

新增调度命令时，应同时修改 `scheduler/scheduler.rs::Command`、`schedulePool` 的筛选语义和 `Exec` 的执行分支，避免出现“策略能返回但执行层静默不处理”的状态；新增命令应在独立测试文件中覆盖 `Hold` 类行为、冷却期、容量边界与日志前后的实际 `Tune` 次数。

新增组件例外时，接入点是 `schedule` 中的组件过滤。需明确它是完全跳过所有策略，还是只限制某些命令；同步扩展 `pkg/resourcemanager/migration_aster_unit_test.rs`，不要把测试逻辑嵌入生产源文件。

调整多调度器组合规则时，修改 `schedulePool`，并保留“顺序优先”和“被守卫拒绝后是否继续询问后续调度器”的明确约定。这里的改动可能改变不同策略相互覆盖的兼容性，也会让每轮调用更多策略，带来性能影响。

修改调容间隔或上限时，应优先检查 `util::MinSchedulerInterval`、`util::MaxOverclockCount` 及池实现的时间戳更新契约。性能风险集中在 100 ms 全池扫描、每池多调度器调用和 `Tune` 的实现成本；正确性风险集中在公开 `Exec` 可绕过 `schedulePool`、整数极值回绕，以及 `Cap`/`Running` 在并发变化下只是瞬时快照。

建议同步的独立测试位置是 `pkg/resourcemanager/schedule_test.rs`（Go 原测试一一对应）和 `pkg/resourcemanager/migration_aster_unit_test.rs`（更完整的 Rust 边界与生命周期回归）；若改变 Go 对齐语义，还应同步核对 `pkg/resourcemanager/schedule_test.go`。

## 验证依据

- RustCodeGraph：`status` 显示当前仓库索引含 11,467 个文件；`node --file pkg/resourcemanager/schedule.rs` 读取完整 117 行，并报告该文件被 11 个文件使用；精确 `node 'schedule.rs::schedule'` 确认 `schedule → schedulePool` 和 `schedule → Exec` 两条文件内调用边。按路径执行 `files` 和自然语言 `explore` 未返回摘要，`callers/callees` 对重名符号解析有歧义，因此上游入口再由 `rm.rs` 源码核实，不把歧义输出当作调用事实。
- 生产源码：`pkg/resourcemanager/schedule.rs`（三个目标方法）；`pkg/resourcemanager/rm.rs`（`Start`/`Stop`、内部共享状态和注册入口）；`pkg/resourcemanager/lib.rs`（私有模块装配与测试模块）；`pkg/resourcemanager/scheduler/scheduler.rs`（`Command` 与 `Scheduler`）；`pkg/resourcemanager/util/util.rs`（池 trait、容器、组件与限制常量）。
- crate 边界：`pkg/resourcemanager/Cargo.toml`，确认库入口、本地 scheduler 依赖、日志依赖和 Go 包迁移映射。
- Go 对照：`pkg/resourcemanager/schedule.go` 与 `pkg/resourcemanager/schedule_test.go`。
- Rust 独立测试：`pkg/resourcemanager/schedule_test.rs` 与 `pkg/resourcemanager/migration_aster_unit_test.rs`；后者直接验证空闲/过载/最小容量守卫、调度器顺序、冷却期、超频上限、整数回绕、`DistTask` 跳过以及启停生命周期。
- 人工复核结论：本文件存在是为了把周期性全池遍历、策略选择和池调容分层连接起来；安全扩展必须同时维护命令选择守卫、执行守卫和上述独立测试。任务为纯文档分析，按计划不运行 Cargo。
