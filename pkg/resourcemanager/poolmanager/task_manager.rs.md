# `pkg/resourcemanager/poolmanager/task_manager.rs`

## 文件定位

本文件是 `astersql-resourcemanager-poolmanager` crate 的任务状态核心。crate 入口 `pkg/resourcemanager/poolmanager/lib.rs` 依次用 `include!` 合并本文件、`task_manager_iterator.rs` 和 `task_manager_scheduler.rs`，因此三个文件最终处于同一个模块作用域：本文件定义数据结构和基础操作，迭代器文件读取其私有字段选择调容候选，调度器文件公开 `Overclock`/`Downclock`。

`pkg/resourcemanager/poolmanager/Cargo.toml` 将 `lib.rs` 指定为库入口，声明 Go 包映射为 `pkg/resourcemanager/poolmanager`，直接外部依赖只有 `crossbeam-channel = "0.5"`。生产侧直接使用者是 `pkg/resourcemanager/pool/spool/spool.rs`：建池时构造 `TaskManager`，并发任务提交时创建和注册 `Meta`，池容量变化时请求升频或降频，worker 则通过 `Meta::recv_event` 消费任务或退出。

## 核心职责

- 用固定 `shard = 8` 个 `RwLock<HashMap<u64, Meta>>` 分散任务元数据，令不同 task ID 的注册、删除和候选扫描不必竞争同一把全局锁（`getShardID`、`TaskStatusContainer`、`TaskManager`）。
- 用 `Meta` 聚合一个逻辑并发任务的 ID、创建时间、初始并发度、当前 worker 数、任务通道和降频退出信号。
- 用 `TaskChannel` 把 `FnOnce + Send + 'static` 闭包排队，并提供“停止接受新任务但继续排空已有任务”的显式关闭语义。
- 用 `SignalChannel` 提供有界、非阻塞的退出通知；`Meta::recv_event` 把任务、退出和任务通道关闭统一为 `MetaEvent`。
- 暴露 Go 风格 API 和 Rust 风格别名，便于现有移植代码按两种命名方式接入。候选选择和调度动作本身位于同 crate 的另外两个 `include!` 文件中。

## 主要符号

- `const shard: usize = 8` 与 `fn getShardID(id: u64) -> usize`：按 `id % 8` 定位唯一分片。固定分片数与 Go 文件一致。
- `pub type Task = Box<dyn FnOnce() + Send + 'static>`：每个队列元素只能执行一次，可跨线程移动，不允许借用非静态数据。
- `TaskChannel`：持有无界数据通道、零容量关闭通道、共享关闭发送端、`gate` 互斥锁和 `closed` 原子标记。`new/default` 创建通道；`send`、`close`、`is_closed` 管理发送生命周期；`try_recv`、`recv` 负责消费；两个 receiver getter 供 `Meta` 组合选择。
- `SignalChannel`：持有指定容量的 crossbeam 通道和 `closed` 标记。`bounded` 构造，`try_send`/`try_recv` 非阻塞传递退出信号，`receiver` 供阻塞选择。
- `MetaEvent::{Task, Exit, Closed}`：worker 可观察的三种结果；携带任务、请求本 worker 退出、或任务源已关闭。
- `Meta`：`createTS`、`exitCh`、`taskCh`、`taskID`、共享 `AtomicI32 running` 和 `initialConcurrency`。`NewMeta/new` 构造；`TaskID`、`GetTaskCh`、`GetExitCh` 读取句柄；`IncTask/DecTask` 维护 worker 数；`recv_event` 驱动 worker。
- `TaskStatusContainer`：单分片的 `RwLock<HashMap<u64, Meta>>`。
- `TaskManager`：八个分片和创建时的 `concurrency`。`NewTaskManager/new` 构造；`RegisterTask/register_task` 覆盖式登记；`DeleteTask/delete_task` 删除；`GetOriginConcurrency/get_origin_concurrency` 返回原始并发度。

公开 API 同时保留 `NewMeta` 等 Go 风格名称与少量 snake_case 别名；`lib.rs` 允许非 Rust 命名规范。字段、`getShardID` 和 `TaskStatusContainer` 均不对 crate 外公开。

## 执行流程

1. `Pool::new` 在 `pkg/resourcemanager/pool/spool/spool.rs` 中调用 `TaskManager::NewTaskManager(size)`，管理器建立八个空分片并保存初始池容量。
2. `Pool::run_with_concurrency` 为一组任务生成 ID，调用 `Meta::new(task_id, tasks, concurrency)`；该构造器自动创建容量为 1 的退出通道。随后 `RegisterTask` 通过 `getShardID` 选择分片，在写锁内插入 `Meta`，并为当前可用并发槽启动 worker。
3. `spool.rs::run_task` 先调用 `Meta::IncTask`，然后循环调用 `recv_event`。收到 `MetaEvent::Task` 就执行闭包；收到 `Exit` 或 `Closed` 就退出。局部 Drop 守卫保证正常返回或闭包 panic 展开时都调用 `DecTask`。
4. `recv_event` 克隆三个 receiver，并用公平的 `crossbeam_channel::select!` 同时等待数据任务、退出信号和关闭通知。关闭通知先尝试从数据队列再取一个元素，使关闭前已排队的任务仍可被消费；队列已空时才返回 `Closed`。
5. 池扩容时，`Pool::tune` 调用 `Overclock`；该方法在 `task_manager_scheduler.rs` 转发到 `task_manager_iterator.rs::getBoostTask`，后者读取各分片并选出低于初始并发或较新的任务，再启动一个额外 worker。池缩容且当前运行数超过新容量时，`Downclock` 选择超频或更老的运行任务并对其 `exitCh` 做非阻塞发送。
6. `DeleteTask` 只从对应分片移除登记，不关闭任务通道、不发送退出信号，也不等待 worker。当前仓库生产路径没有调用该方法；现有直接调用仅见独立迁移测试，因此不能把自动注销描述为已实现行为。

## 数据与状态

`Meta` 被按值存入 map，但其 `Clone` 具有混合语义：`Instant`、ID 和初始并发度按值复制；两个通道句柄以及 `running` 的 `Arc<AtomicI32>` 共享底层状态。因此管理器中的 `Meta`、worker 捕获的 `Meta` 和调用者保留的 `Meta` 看到同一队列、退出信号和运行计数。

`RegisterTask` 对同一 task ID 使用 `HashMap::insert`，会静默替换旧 `Meta`；它不检查 ID 冲突，也不清理被替换对象的 worker。ID 到分片的映射稳定，ID `1` 和 `9` 会落在同一分片，但仍是两个独立键。`concurrency` 在构造后不可变，仅由 getter 返回；调容后的实时池容量属于 `spool::PoolInner.capacity`，不是这里的字段。

`TaskChannel::close` 在 `gate` 内把 `closed` 原子置为 true，并取走共享 `Option<Sender<()>>` 中唯一的关闭发送端。零容量关闭通道因发送端析构而断开，所有关闭 receiver 均被唤醒。数据 `sender` 仍然存在，但 `send` 在同一 `gate` 内先检查 `closed`，所以关闭与发送被串行化：关闭之前成功入队的任务可继续排空，关闭之后的新任务返回 `SendError<Task>`。

`SignalChannel::closed` 在当前实现中只于构造时写入 `false`，没有 `close` 方法或写成 `true` 的路径，所以 `is_closed()` 当前始终为 false。实际退出通知依赖容量和 receiver 生命周期：容量 1 的默认通道最多积压一个信号，重复 `try_send` 可能返回 `Full`；调用方 `pauseTask` 有意忽略该错误以保持调度循环非阻塞。

## 依赖与调用关系

上游生产调用边（由 `rg` 在 Rust 源码中核对）：

- `spool.rs::Pool::new -> TaskManager::NewTaskManager`。
- `spool.rs::Pool::run_with_concurrency -> Meta::new -> TaskManager::RegisterTask`。
- `spool.rs::Pool::tune -> TaskManager::Overclock/Downclock`；后两者定义于相邻的 `task_manager_scheduler.rs`。
- `spool.rs::run_task -> Meta::IncTask/recv_event/DecTask`。

下游依赖主要是标准库的 `HashMap`、`Arc`、`Mutex`、`RwLock`、`AtomicBool`、`AtomicI32`、`Instant`，以及 `crossbeam_channel::{Sender, Receiver}` 和 `select!`/`select_biased!`。`TaskChannel::recv` 使用偏置选择，任务分支优先于关闭分支；`Meta::recv_event` 使用非偏置选择，使同时就绪的任务、退出和关闭分支随机公平竞争。

RustCodeGraph 将目标文件索引为 38 个符号，并识别它由大量编译单元间接纳入；但针对本文件精确符号的 `callers/callees` 未给出静态调用边，按节点 ID 查询还出现无输出挂起。原因与 `include!` 合并及方法调用解析有关的可能性未验证，因此调用关系以目标源码、相邻 `include!` 文件和 `rg` 的精确调用点为准，不把索引缺边解释为“无调用者”。

## 错误处理与边界

- `TaskChannel::send` 在显式关闭后把原任务原样放入 `SendError` 返回；底层数据 receiver 断开时也由 crossbeam 返回同类错误。`try_recv` 区分空队列 `Ok(None)` 与断开错误，`recv` 则把关闭且无剩余任务归一为 `RecvError`。
- `SignalChannel::try_send` 保留 crossbeam 的 `Full`/`Disconnected` 错误，`try_recv` 保留 `Disconnected` 并把空队列转换为 `Ok(false)`。降频路径忽略发送失败，所以“请求过一次降频”不保证新信号一定入队。
- 所有标准库锁都直接 `unwrap()`。若持锁线程 panic 导致锁 poisoned，后续发送、关闭、注册、删除或扫描会继续 panic，而不是返回可恢复错误。
- `IncTask/DecTask` 使用 `AtomicI32::fetch_add/fetch_sub`，本文件不检查下溢、上溢或调用配对；正确性依赖 worker 生命周期按一次增、一次减调用。`spool.rs` 的 Drop 守卫提供当前生产路径的配对保证。
- `NewTaskManager` 接受任意 `i32`，`Meta::new/NewMeta` 也接受任意初始并发度；零值、负值及超大值不在本文件校验。池构造只在上游拒绝 `size == 0`。
- 删除不存在的 ID 是静默 no-op；重复注册会替换；本文件不拥有 task ID 生成或唯一性策略。
- `recv_event` 的公平选择意味着退出信号与已就绪任务同时存在时不承诺固定优先级；关闭分支虽会排空一个任务，也不保证在退出信号到达后排空全部队列。

## 并发与资源生命周期

分片 map 的写操作分别由各自 `RwLock` 排他保护，迭代选择则在扫描某一分片期间持读锁，跨分片逐个获取和释放；因此它得到的是逐分片视图，不是全局原子快照。并发注册或删除可能发生在已经扫过或尚未扫描的分片中，候选结果只保证各次加锁期间的数据安全，不保证全局时间点一致性。

`TaskChannel` 的所有 clone 共享 `gate`、`closed` 和关闭发送端，故任一 clone 调用 `close` 都会全局关闭逻辑发送面；`close` 幂等。数据 receiver 是 MPMC clone，多个 worker 每个任务只会由其中一个取走。任务闭包的资源在执行后释放；若关闭后无人继续消费，队列中闭包会随最后一个通道句柄析构才释放。

`Meta::running` 统计该逻辑任务当前 worker 数，而 `spool::PoolInner.running` 统计池级线程/槽位，两者由不同位置维护。调容选择读取前者；池容量判断读取后者。两者更新并非同一原子事务，设计允许短暂观测差异。

退出信号只要求某个竞争 receiver 收到一个单位值，因此一次 `Downclock` 最多使一个 worker 退出。`TaskChannel::close` 则通过断开的广播关闭通道唤醒所有等待 worker；每个 worker 会尝试再取一个已排队任务，然后在后续迭代观察关闭。`TaskManager` 自身没有 Drop 清理协议，map 内 `Meta` 的释放取决于显式删除或整个管理器析构；当前生产接线没有显式删除已完成任务。

## 与 Go 版本的对应关系

核心结构逐项对应 `pkg/resourcemanager/poolmanager/task_manager.go`：同为 8 分片、task ID 取模、每分片 map 加读写锁、`Meta` 保存创建时间/通道/ID/原子运行数/初始并发度，注册与删除语义以及原始并发度 getter 一致。`task_manager_iterator.go` 和 `task_manager_scheduler.go` 的候选规则及公开调度入口也被相邻 Rust 文件复刻。

Rust 为承载 Go 原生 channel 语义新增了显式封装：`TaskChannel` 替代 `chan func()`，`SignalChannel` 替代 `chan struct{}`，`MetaEvent` 替代 worker 中的 Go `select` 分支。Rust 的 `Meta` 按值返回并通过 `Arc` 共享可变状态，Go 则传递 `*Meta`；可观察的任务队列和计数仍然共享。

需要注意的差异：Go 的 `pauseTask` 用 `exitCh != nil` 判断是否可发；Rust 的 `SignalChannel` 不是可空值，并改用当前恒为 false 的 `is_closed` 检查。Go task channel 由外部 `close(fns)` 关闭，Rust 必须显式调用 `TaskChannel::close`，单纯丢弃某个 clone 不等同于逻辑关闭。Go worker 的 `select` 与 Rust `Meta::recv_event` 都不规定多个就绪分支的固定优先级；Rust 额外确保任务通道关闭时仍尝试取出一个已排队任务。

独立 Rust 对照测试位于 `pkg/resourcemanager/poolmanager/migration_aster_unit_test.rs`，覆盖身份/计数/通道关闭、同分片精确删除、低于初始并发的升频选择、超频任务的非阻塞降频信号。`pkg/resourcemanager/pool/spool/migration_aster_unit_test.rs::task_meta_blocking_select_wakes_on_exit_without_polling` 进一步验证阻塞 worker 能被退出信号唤醒。本任务按要求只读取这些测试，不运行 Cargo。

## 扩展指南

- 增加任务级状态时，应优先放入 `Meta`，明确 clone 后是复制还是共享；需要跨 worker 一致的可变状态应使用 `Arc` 加原子或锁，并在独立的 `migration_aster_unit_test.rs` 扩展测试，不能把测试内嵌回生产文件。
- 修改注册/删除策略时，入口是 `RegisterTask`、`DeleteTask` 和 `getShardID`。若要拒绝重复 ID、自动注销或关闭被替换任务，必须同时定义 worker 与通道清理顺序，并验证 `spool.rs::run_with_concurrency/run_task`，避免遗留线程或丢弃待执行闭包。
- 修改升降频策略时，本文件的 `createTS`、`running`、`initialConcurrency` 是决策数据，但选择算法实际位于 `task_manager_iterator.rs::{iter,canBoost,canPause}`，公开动作位于 `task_manager_scheduler.rs`；三处和对应 Go 文件应同步审查。
- 若补全 `SignalChannel` 关闭语义，应新增共享关闭发送端或明确的 close 操作，并让 `closed` 真正随生命周期变化；同时覆盖重复关闭、receiver 断开、容量已满和并发 `try_send`。不能仅修改 `is_closed` 而不定义谁拥有关闭权。
- 若改变 `recv_event` 优先级或排空保证，应同步检查 `TaskChannel::recv` 与 `spool.rs::run_task`，并添加“任务与 Exit 同时就绪”“close 前存在多个排队任务”“多个 worker 同时等待”的确定性测试。公平性、关停延迟和任务丢弃风险需要明确记录。
- 调整分片数会改变锁竞争、内存占用及 task ID 到分片的分布；虽然不影响 map 逻辑键，仍需与 Go 的 `shard` 常量保持一致并补充并发注册/扫描基准或测试。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/resourcemanager/poolmanager` 列出本 crate 的 8 个 Rust/Go 文件；`node --file pkg/resourcemanager/poolmanager/task_manager.rs` 读取目标文件 325 行并确认 38 个符号；另用 `node --file` 阅读相邻 Rust/Go 实现与测试。
- 图查询限制：`query` 精确定位 `TaskManager`、`NewTaskManager`、`recv_event`、`RegisterTask`；对本文件精确 qualified name 的 `callers/callees` 返回空结果，按节点 ID 的查询无输出并被中止，因此没有伪造静态图调用边。
- crate 与装配证据：`pkg/resourcemanager/poolmanager/Cargo.toml`、`pkg/resourcemanager/poolmanager/lib.rs`；目标包不存在 `doc.go`。
- Go 对照证据：`pkg/resourcemanager/poolmanager/task_manager.go`、`task_manager_iterator.go`、`task_manager_scheduler.go`，以及生产接线 `pkg/resourcemanager/pool/spool/spool.go`。
- Rust 调用与生命周期证据：`pkg/resourcemanager/pool/spool/spool.rs`；用 `rg` 精确核对 `NewTaskManager`、`NewMeta/new`、注册/删除、升降频、`recv_event` 和计数方法的 Rust/Go 调用点。
- 测试证据：`pkg/resourcemanager/poolmanager/migration_aster_unit_test.rs` 与 `pkg/resourcemanager/pool/spool/migration_aster_unit_test.rs`。纯文档任务遵循总计划，不执行 Cargo；文档交付仅运行任务指定的 11 章节结构校验，并人工检查关键结论均可回溯到上述符号和路径。
