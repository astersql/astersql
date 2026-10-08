# `pkg/resourcemanager/poolmanager/task_manager_scheduler.rs`

## 文件定位

本文件是 `astersql-resourcemanager-poolmanager` crate 的调度门面。crate 入口 `pkg/resourcemanager/poolmanager/lib.rs` 依次用 `include!` 合并 `task_manager.rs`、`task_manager_iterator.rs` 和本文件，因此这里的 `impl TaskManager` 可以直接使用前两者定义的 `TaskManager`、`Meta`、`getBoostTask` 与 `pauseTask`，而无需单独的 `use` 声明。

`pkg/resourcemanager/poolmanager/Cargo.toml` 将 `lib.rs` 指定为库入口，端口元数据指向 Go 包 `pkg/resourcemanager/poolmanager`，唯一直接外部依赖是 `crossbeam-channel = "0.5"`。本文件没有常量、结构体、trait、条件编译项或独立状态，只为已有 `TaskManager` 增加四个公开方法。

实际应用接线位于 `pkg/resourcemanager/pool/spool/lib.rs`：该 crate 通过 `#[path = "../../poolmanager/lib.rs"]` 将 poolmanager 作为模块引入。`Pool::tune`（`pkg/resourcemanager/pool/spool/spool.rs`）是当前检索到的生产调用入口。

## 核心职责

本文件把资源池的容量调整意图转换为对任务管理器内部选择逻辑的调用：

- `Overclock` 查询一个适合增加 worker 的任务，保留任务 ID 与 `Meta` 两部分结果，供 Go 兼容调用者使用。
- `Downclock` 查询一个适合减少 worker 的任务，并由下层逻辑向该任务的退出通道发送信号。
- `overclock` 与 `downclock` 是 Rust 风格的小写别名；前者只暴露 `Meta`，后者保持无返回值。

文件本身不修改池容量、不创建或回收线程、不增减 `Meta::running`，也不登记或删除任务。候选选择在 `task_manager_iterator.rs`，worker 创建和池级计数调整在 `Pool::tune`/`run_task` 所在的 `spool.rs` 中完成。

## 主要符号

- `TaskManager::Overclock(&self) -> (u64, Option<Meta>)`：Go 风格公开 API，原样返回 `self.getBoostTask()`。返回的 `u64` 是候选任务 ID；无候选时 `Option<Meta>` 为 `None`，调用者必须以 `Option` 为准，不能仅凭 ID 判断是否命中，因为默认 ID 为 `0`。
- `TaskManager::overclock(&self) -> Option<Meta>`：Rust 风格便利方法，调用 `Overclock()` 后丢弃元组的任务 ID，仅保留候选元数据。
- `TaskManager::Downclock(&self)`：Go 风格公开 API，委托 `self.pauseTask()` 完成选择与通知。
- `TaskManager::downclock(&self)`：Rust 风格便利方法，直接调用 `Downclock()`。

这些方法都只借用 `&self`。并发可变性来自 `TaskManager` 内部分片的 `RwLock<HashMap<u64, Meta>>`、`Meta::running` 的 `AtomicI32` 以及可克隆的通道句柄，而不是门面方法持有 `&mut self`。

## 执行流程

扩容主流程如下：

1. `Pool::tune(size)` 在 `admission` 互斥锁保护下更新池容量。
2. 当新容量大于旧容量且当前运行数小于新容量时，调用 `task_manager.Overclock()`。
3. `Overclock` 调用 `getBoostTask`；后者以 `canBoost` 为谓词遍历 8 个任务分片。
4. `canBoost` 优先立即选中 `running < initialConcurrency` 的候选，否则倾向创建时间更晚的任务。迭代器要求先找到一个 `running != 0` 的基准任务；全部任务都未运行时返回 `None`。
5. 若返回 `Some(Meta)`，`Pool::tune` 将池级 `running` 加一，并用 `spawn_reserved` 启动执行该任务队列的新 worker；本文件不执行这一步。

缩容主流程如下：

1. `Pool::tune(size)` 检测到池级运行数仍高于新容量时调用 `task_manager.Downclock()`。
2. `Downclock` 调用 `pauseTask`；后者以 `canPause` 遍历任务。
3. `canPause` 优先选中 `running > initialConcurrency` 的已超频任务并停止继续扫描，否则倾向创建时间更早且仍在运行的任务。
4. `pauseTask` 对候选的 `exitCh` 执行非阻塞 `try_send(())`。worker 在 `Meta::recv_event` 中收到退出事件后离开，后续计数清理由 worker 生命周期代码负责。

小写别名不引入新分支：`overclock -> Overclock -> getBoostTask`，`downclock -> Downclock -> pauseTask`。

## 数据与状态

本文件不声明数据，但它传递和触发以下相邻状态：

- `TaskManager::task: Vec<TaskStatusContainer>`：固定 8 个分片；每个分片以 `RwLock` 保护 `taskID -> Meta` 映射。
- `Meta::taskID`：`Overclock` 元组的 ID 来源。
- `Meta::createTS`：候选优先级的时间依据；加速倾向较新任务，降速倾向较老任务。
- `Meta::running: Arc<AtomicI32>`：当前 worker 数；使用 `Ordering::SeqCst` 读取，是 `canBoost`/`canPause` 的核心条件。
- `Meta::initialConcurrency`：任务注册时的目标并发；用于识别未达初始并发和已经超频的任务。
- `Meta::exitCh: SignalChannel`：容量通常为 1 的退出信号通道；`Downclock` 最终尝试向它写入一个单位信号。

`Overclock` 返回克隆的 `Meta`，其原子计数和通道内部通过 `Arc` 共享，因此调用者操作的是同一任务生命周期状态，而不是一份脱离管理器的快照。遍历同一分片内的 `HashMap` 没有稳定顺序；除明确的谓词优先级外，不应依赖相同条件候选之间的固定选择顺序。

## 依赖与调用关系

上游生产调用边：

- `pkg/resourcemanager/pool/spool/spool.rs::Pool::tune -> TaskManager::Overclock`：扩容时获取候选，并在成功后增加池级运行数、启动 worker。
- `pkg/resourcemanager/pool/spool/spool.rs::Pool::tune -> TaskManager::Downclock`：缩容时请求一个 worker 退出。

本文件内部及下游调用边：

- `TaskManager::overclock -> TaskManager::Overclock -> TaskManager::getBoostTask -> TaskManager::iter(canBoost)`。
- `TaskManager::downclock -> TaskManager::Downclock -> TaskManager::pauseTask -> TaskManager::iter(canPause)`。
- `pauseTask -> SignalChannel::try_send`；worker 侧由 `Meta::recv_event` 接收 `MetaEvent::Exit`。

RustCodeGraph 将本文件识别为 5 个符号（文件节点加四个方法），并显示 `overclock` 调用 `Overclock`、`downclock` 调用 `Downclock`。图索引未解析出跨 `include!` 文件的 `Overclock -> getBoostTask` 和 `Downclock -> pauseTask` 边，因此这两条边以本文件方法体及 `task_manager_iterator.rs` 的精确符号定义交叉核验。生产上游则通过 `rg` 与 `spool.rs` 源码核验。

## 错误处理与边界

- 四个公开方法均不返回 `Result`，也不自行记录日志。
- 没有可加速任务时，`Overclock` 返回迭代器结果 `(0, None)`；`overclock` 返回 `None`。ID `0` 不是独立的“未找到”判据。
- 没有可降速任务时，`Downclock` 无操作并正常返回。
- `pauseTask` 的发送是非阻塞的，且忽略 `try_send` 的错误；通道已满或接收端不可用时，本次降速请求可能不产生新的退出信号，但不会阻塞调度循环或向上传播错误。
- 遍历分片时使用 `stats.read().unwrap()`；若锁已中毒，会发生 panic。这个行为来自下层 iterator，不由本门面捕获。
- `Overclock` 只是选择候选，不会原子地为候选预留 worker。调用者负责在池级串行化调容；当前 `Pool::tune` 使用 `admission` 锁缩小重复调容的竞态窗口。
- `Downclock` 只发一次尽力而为的通知，不保证调用返回时 worker 已退出；池级运行数的下降是异步结果。

## 并发与资源生命周期

所有门面方法均为同步、短调用，本文件不生成线程或异步任务。`getBoostTask`/`pauseTask` 按分片获取读锁，克隆选中的 `Meta` 后释放锁；任务注册和删除使用相同分片的写锁。`running` 的顺序一致原子读保证跨线程可见，但“选择候选”和调用者随后启动/停止 worker 不是由本文件合并成单一原子事务。

扩容时，`Pool::tune` 在持有池级 `admission` 锁期间调用 `Overclock`，随后先增加池级运行计数再启动保留线程。worker 使用共享 `Meta` 消费 `TaskChannel` 并维护任务级计数。缩容时，`Downclock` 将信号放入有界 `SignalChannel`；实际退出发生在 worker 的接收循环观察到 `MetaEvent::Exit` 时。因此，方法返回表示“已完成选择/已尝试通知”，不表示容量变化已经收敛。

重复调用 `Downclock` 不会阻塞：容量为 1 的通道已有未消费信号时，后续 `try_send` 失败并被忽略。此设计避免调度线程因慢 worker 停顿，但也意味着调用方不能把调用次数等同于已退出 worker 数。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/resourcemanager/poolmanager/task_manager_scheduler.go`：

- Go `(*TaskManager).Overclock() (uint64, *Meta)` 直接返回 `getBoostTask()`；Rust `Overclock() -> (u64, Option<Meta>)` 用 `Option` 表达 Go 的 `nil`。
- Go `(*TaskManager).Downclock()` 直接调用 `pauseTask()`；Rust 大写方法保持相同委托结构。
- Rust 新增 `overclock`/`downclock` 两个小写别名，属于语言习惯适配，不改变 Go 兼容入口的语义。

下层 `task_manager_iterator.go` 与 Rust 版本保持相同策略：未达初始并发的任务优先 boost、已超过初始并发的任务优先 pause、其余情况按创建时间择新/择旧，并以非阻塞发送避免缩容路径等待。Go 用可空 `exitCh` 和 `select { case ...: default: }`；Rust 的 `Meta` 始终持有 `SignalChannel`，以 `is_closed` 检查加 `try_send` 表达尽力发送。两者都不向 `Downclock` 调用者返回发送错误。

Go 集成测试 `pkg/resourcemanager/pool/spool/spool_test.go::TestWithTaskManager` 验证池从 1 扩到 2、3，再缩到 2、1 的最终运行数。Rust 在 `pkg/resourcemanager/pool/spool/spool_test.rs::TestWithTaskManager` 保留了对应场景，并另有 poolmanager 独立迁移测试覆盖精确候选与非阻塞信号行为。

## 扩展指南

- 若只改变公开 API 的命名或返回形态，在本文件修改对应门面，并同时评估 Go 风格兼容方法和 Rust 风格别名；不要把候选策略复制进门面。
- 若改变 boost/pause 的优先级、扫描终止条件或空集合语义，应修改 `task_manager_iterator.rs` 的 `getBoostTask`、`pauseTask`、`iter`、`canBoost` 或 `canPause`，而不是本文件。
- 若改变 worker 启停、池计数或调容串行化，应修改 `pkg/resourcemanager/pool/spool/spool.rs::Pool::tune` 及 worker 生命周期代码。
- 若需要让降速失败可观察，必须设计 `Downclock`/`pauseTask` 的返回契约，并处理通道满、断开以及异步退出尚未完成的兼容影响；这会偏离当前 Go 的尽力而为语义。
- 新增或改变本文件行为时，测试逻辑应继续放在独立文件：候选和信号边界更新 `pkg/resourcemanager/poolmanager/migration_aster_unit_test.rs`，完整扩缩容生命周期更新 `pkg/resourcemanager/pool/spool/spool_test.rs`，并对照 `spool_test.go::TestWithTaskManager`。不要把测试内嵌进生产 `.rs` 文件。
- 性能方面应保留短持锁和非阻塞通知；若引入稳定排序或全局锁，需要评估跨 8 个分片扫描的锁竞争和调容热路径成本。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点与 1,848,419 条边；目标目录中的 `task_manager_scheduler.rs` 已索引为 5 个符号。
- RustCodeGraph `node --file pkg/resourcemanager/poolmanager/task_manager_scheduler.rs`：确认四个公开方法的完整实现及文件仅 41 行。
- RustCodeGraph `query/node/callers/callees`：确认 `Overclock`、`overclock`、`Downclock`、`downclock` 定义；确认两个小写别名到大写方法的调用边。跨 `include!` 边未被图解析，已用精确源码定义补证，未把缺失图边当成不存在调用。
- RustCodeGraph `node`：读取 `task_manager.rs::TaskManager`、`task_manager_iterator.rs::getBoostTask`、`pauseTask` 及 iterator 全貌，核对分片、锁、原子计数、候选策略和非阻塞退出信号。
- RustCodeGraph `node`：读取 `pool/spool/spool.rs::Pool::tune`，确认生产上游和扩缩容后的 worker 行为；读取 `pool/spool/spool_test.rs::TestWithTaskManager` 与 `poolmanager/migration_aster_unit_test.rs`，确认 Rust 独立测试覆盖。
- 文件读取：`pkg/resourcemanager/poolmanager/Cargo.toml`、`lib.rs`、`task_manager_scheduler.go`、`task_manager_iterator.go`、`pkg/resourcemanager/pool/spool/lib.rs` 及 Go `spool_test.go::TestWithTaskManager`，核对 crate 边界、模块装配和 Go 迁移语义。
- `rg` 检索 `Overclock`/`Downclock`：确认当前 Rust 生产调用位于 `pkg/resourcemanager/pool/spool/spool.rs`；未发现同名 `task_manager_scheduler_test.rs`，最近的独立 Rust 测试为上述 poolmanager 迁移测试和 spool 集成测试。
- 本文档仅描述已核对的当前实现；没有运行 Cargo，符合本任务和总计划对纯文档分析的限制。
