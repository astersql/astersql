# `pkg/lightning/config/configlist.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-config`（`pkg/lightning/config/Cargo.toml`），实现 Lightning 服务模式所需的“待导入配置队列”数据结构。模块由 `pkg/lightning/config/lib.rs` 的 `pub mod configlist` 纳入，并通过 `pub use configlist::*` 从 crate 根再导出；队列元素是同一 crate 的 `Config`。

从接口语义看，它对应 Go 控制面在 `lightning/pkg/server/lightning.go` 中使用的 `config.List`：HTTP POST 入队，后台循环出队，GET 查询，DELETE 删除，PATCH 调整顺序。不过当前 Rust 接线尚未统一：RustCodeGraph 的调用者查询和全库检索显示，本文件的 `new_config_list` 及 snake_case 方法目前只被 `pkg/lightning/config/configlist_test.rs` 直接调用；`lightning/pkg/server/lightning.rs` 实际引用的是 `lightning/pkg/server/stubs.rs` 内另一套 `config::List`/Go 风格方法。因此，本文件是已实现并受独立测试覆盖的配置 crate API，但不能据此声称它已接入 Rust Lightning 服务主循环。

## 核心职责

- 用 `List` 为 `Arc<Mutex<Config>>` 提供线程安全 FIFO 存储，并允许按 `Config::task_id` 查询、删除和重排。
- 在 `List::push` 中生成单调不减时间基础上的严格递增任务 ID，写回 `Config::task_id`，同时维护 ID 索引和顺序队列的一致性。
- 在 `List::pop` 中协调生产者和消费者：有元素时原子地从顺序与索引中移除；空队列时通过条件变量等待，并周期性检查轻量 `Context` 的取消或超时状态。
- 用 `Context`/`ContextError` 表达本文件局部的取消和截止时间结果，而不依赖异步运行时或 Go 的 `context.Context`。

该实现的关键不变量是：正常状态下，`State::order` 中的每个 ID 都在 `State::entries` 中有且仅有一个配置；`all_ids` 的返回顺序就是后续 `pop` 的顺序。`push`、`pop`、`remove` 和重排方法都在同一个 `state` 互斥锁下同时更新两个容器，以维持该不变量。

## 主要符号

- `ContextError::{Cancelled, DeadlineExceeded}`：`pop` 可观察的两种正常终止原因；派生了 `Clone`、`Debug`、`Eq` 和 `PartialEq`，便于传递和断言。
- `Context { done: Arc<Mutex<Option<ContextError>>> }`：可克隆的共享终止状态。`cancel(&self)` 写入 `Cancelled`，`expire(&self)` 写入 `DeadlineExceeded`，私有 `error(&self)` 返回当前状态的克隆。后写入会覆盖先前原因，代码没有“一次写入”保护。
- `State`：`List` 的私有可变状态。`entries: HashMap<i64, Arc<Mutex<Config>>>` 提供按 ID 的平均常数时间索引；`order: VecDeque<i64>` 保存 FIFO/重排顺序；`last_id` 保存最近分配的最大 ID。
- `List { state: Mutex<State>, changed: Condvar }`：公开队列类型。调用者通常通过共享引用调用方法；内部互斥锁提供可变性和跨线程串行化。
- `new_config_list() -> List`：构造空队列。它返回裸 `List` 而非 `Arc<List>`，需要跨线程共享时由调用者包裹 `Arc`，如 `test_normal_push_pop` 和 `test_context_cancel`。
- `List::push(&self, Arc<Mutex<Config>>)`：分配 ID、修改配置、写入索引、追加队尾，并用 `notify_all` 唤醒等待者。
- `List::pop(&self, &Context) -> Result<Arc<Mutex<Config>>, ContextError>`：阻塞式消费入口。
- `List::{remove, get, all_ids}`：分别删除、克隆取得共享配置引用、快照当前顺序。
- `List::{move_to_front, move_to_back}`：存在时移动到队首/队尾，重复移动当前首/尾元素仍返回 `true`；不存在时保持队列不变并返回 `false`。

本文件没有 trait、模块级常量或条件编译项。

## 执行流程

`push` 的流程如下：

1. 从 `SystemTime::now().duration_since(UNIX_EPOCH)` 取得纳秒值；时钟早于纪元时通过 `unwrap_or_default` 退化为零，超过 `i64::MAX` 时截断到 `i64::MAX`。
2. 获取 `List::state` 锁，以 `max(now, last_id.saturating_add(1))` 避免同纳秒或时钟回退导致的普通 ID 冲突。
3. 获取传入 `Config` 自身的锁并写入 `task_id`，再更新 `last_id`、`entries` 和 `order`。
4. 在仍持有 `state` 锁时调用 `changed.notify_all()`；方法返回时释放锁，等待消费者才可继续检查队列。

`pop` 的流程如下：

1. 获取 `state` 锁并进入循环。
2. 尝试从 `order` 队首取 ID；若 `entries.remove(id)` 同时取得配置，立即返回该 `Arc`，完成一次原子的出队。
3. 只有当前没有可返回配置时才调用 `Context::error`。因此“队列已有元素且上下文同时终止”时优先交付任务，而不是返回上下文错误。
4. 若上下文仍有效，则用 `Condvar::wait_timeout` 释放 `state` 锁并最多等待 10 ms；收到 `push` 广播或超时后重新持锁并重试。这一循环同时处理虚假唤醒。

`remove` 先删 `entries`；若不存在则直接返回 `false`，存在则用 `VecDeque::retain` 删除顺序中的对应 ID。两个移动方法先查 `entries` 确认存在，再从 `order` 清除旧位置并压入指定端。`get` 和 `all_ids` 只在锁内形成克隆/快照，不把 `State` 的借用暴露给调用者。

## 数据与状态

队列拥有两层共享状态：`List::state` 保护队列元数据，单个元素的 `Arc<Mutex<Config>>` 保护配置本身。`get` 与 `pop` 返回的是同一配置对象的 `Arc` 克隆，不是 `Config` 值快照；调用方之后对配置的修改可被持有其他克隆者观察到。相对地，`all_ids` 返回独立的 `Vec<i64>` 快照，返回后队列变化不会回写该向量。

任务 ID 只在本 `List` 实例内根据 `last_id` 保证通常情况下严格递增；它不持久化，也不协调其他 `List` 实例。新建 `Config` 的 `task_id` 初值为零（`pkg/lightning/config/config.rs::new_config`），首次 `push` 会覆盖它。极端情况下若 `last_id` 已到 `i64::MAX`，`saturating_add(1)` 仍是 `i64::MAX`，后续入队会复用该 ID，使 `HashMap::insert` 覆盖旧映射而 `order` 继续追加重复 ID；当前代码与测试没有处理这一耗尽边界。

`Context` 的 `done` 初始为 `None`，克隆共享同一 `Arc`，所以一个线程调用 `cancel`/`expire` 后另一个线程的 `pop` 能观察到结果。它没有绝对截止时间字段或计时器；`expire` 只是由外部主动标记截止时间已到。

## 依赖与调用关系

下游依赖全部是标准库与 crate 内类型：`HashMap` 负责 ID 索引，`VecDeque` 负责顺序，`Arc`/`Mutex`/`Condvar` 负责共享与同步，`SystemTime`/`UNIX_EPOCH` 生成 ID，`Duration` 设置轮询间隔，`crate::Config` 是元素类型。`pkg/lightning/config/Cargo.toml` 没有为本文件引入专属第三方依赖或 feature；crate 本身还依赖 CPU、serde、TOML、URL 等库，但本文件不直接调用它们。

RustCodeGraph 对 `new_config_list` 的调用者给出四个独立测试：`test_normal_push_pop`、`test_context_cancel`、`test_get_remove`、`test_move_front_back`，均位于 `pkg/lightning/config/configlist_test.rs`。`lib.rs` 用 `#[path = "configlist_test.rs"] mod configlist_test` 将测试保持在独立文件中。对全库 Rust 调用点的补充检索没有发现本文件 API 的生产调用者。

作为 Go 语义的应用位置证据，`lightning/pkg/server/lightning.go::RunServer` 创建队列并在循环中 `Pop` 后调用 `run`；任务 HTTP 处理器用 `Push`、`Get`、`Remove`、`AllIDs`、`MoveToFront` 和 `MoveToBack` 管理排队任务。Rust 版 `lightning/pkg/server/lightning.rs` 具有同形流程，但其 `crate::config` 来自该 server 的 `stubs.rs`，并非本 crate 的 `configlist.rs`；将两者统一是未来接线工作，而不是当前事实。

## 错误处理与边界

`pop` 的业务错误显式返回 `ContextError`；其余队列操作不返回错误，缺失 ID 用 `false`/`None` 表示。空队列并不是错误，`pop` 会持续等待。上下文终止的最坏观察延迟约受 10 ms 轮询周期加锁调度影响，因为 `cancel` 和 `expire` 不持有 `List::changed`，也不会主动唤醒条件变量。

所有 `Mutex::lock` 与 `Condvar::wait_timeout` 都使用 `expect`：任何持锁线程 panic 造成的 poison 会令后续调用 panic，接口没有恢复或转换成业务错误。`SystemTime` 早于 Unix 纪元不会报错，而会采用零；ID 上限耗尽可能破坏索引/顺序不变量，见“数据与状态”。

若内部不变量已因缺陷而出现 `order` 中有 ID、`entries` 中无 ID，`pop` 会丢弃该顺序项并继续循环，而不是向调用者暴露错误。正常公开方法都在同一锁下维护两者，因此常规路径不会产生这种状态。

## 并发与资源生命周期

`List` 没有后台线程，也没有显式 `close`/`Drop` 行为；等待发生在调用 `pop` 的线程上。生产者 `push` 通过条件变量广播唤醒所有消费者，获得 `state` 锁的消费者各自竞争队首任务。广播可能造成多个等待者醒来但只有一个获得新任务，其余线程会重新等待，这是循环谓词的预期用法。

锁的作用域覆盖每次队列结构修改，因此 `push`、成功 `pop`、`remove` 和移动操作相互线性化。返回 `Arc<Mutex<Config>>` 后，队列锁已经释放，配置可独立存活；从队列删除只释放队列持有的一个 `Arc`，不会使其他持有者失效。

维护者应留意锁顺序：`push` 先锁 `state`，再锁传入的 `Config`；其他队列方法不在持有 `state` 时锁配置。若外部代码先锁同一 `Config`，再调用一个需要 `state` 的方法，同时另一线程正在 `push` 该配置，可能形成锁顺序反转。安全用法是不要持有元素锁进入队列方法；若未来改变共享模型，应用测试覆盖这一并发约束。

`Context` 与 `List` 生命周期相互独立。终止 Context 不会关闭队列，也不会阻止后续 `push`；它只影响使用该 Context 且当时取不到任务的 `pop`。队列销毁时没有机制通知仍在其他共享所有权中等待的线程，因此调用者必须通过 Context 结束等待并自行 join 线程。

## 与 Go 版本的对应关系

本文件逐项移植 `pkg/lightning/config/configlist.go`：Go 的 `taskIDMap + container/list + sync.Cond + lastID` 对应 Rust 的 `HashMap + VecDeque + Mutex/Condvar + last_id`；公开操作也一一对应 `NewConfigList`、`Push`、`Pop`、`Remove`、`Get`、`AllIDs`、`MoveToFront`、`MoveToBack`。`pkg/lightning/config/configlist_test.rs` 的四个测试明确对照同目录 Go 测试的 `TestNormalPushPop`、`TestContextCancel`、`TestGetRemove` 和 `TestMoveFrontBack`，验证 FIFO、阻塞唤醒、取消、查询删除与幂等重排。

重要差异如下：

- Go 构造器返回 `*List`，Rust 返回 `List`，跨线程共享由调用者显式增加 `Arc`。
- Go 队列直接保存 `*Config`；Rust 保存 `Arc<Mutex<Config>>`，将配置内部可变性也纳入同步模型。
- Go `Pop` 为每次调用启动 goroutine，用无缓冲 channel 与 `select` 竞争结果和 `ctx.Done()`；Rust 不生成线程，调用线程用 10 ms 条件变量超时轮询轻量 Context。两者在队列/取消同时就绪时的选择细节不完全相同，Rust 明确先尝试取队首。
- Go Context 可由 deadline 自动完成并返回标准错误；Rust 需要外部显式调用 `expire`，返回本地 `DeadlineExceeded`。当前 Rust 测试只覆盖 `Cancelled`，没有覆盖 `expire`。
- Go 用链表元素使删除和移动为常数时间；Rust 只在 `HashMap` 中常数时间确认存在，随后 `VecDeque::retain` 线性扫描。Go 注释假设列表不会很长；Rust 对长队列的删除/重排成本更敏感。

## 扩展指南

- 若新增队列操作，应在持有 `state` 锁的一次临界区内同步维护 `entries` 与 `order`，并明确其相对于 `pop` 的线性化时刻；不要只更新其中一个容器。
- 若修改 ID 生成策略，应保持 `Config::task_id`、`State::last_id`、哈希键和顺序项四者一致，并新增时钟回退、同纳秒并发及 `i64::MAX` 边界测试。若 ID 需要跨进程稳定或持久化，应改由更高层分配，不能继续依赖 `SystemTime`。
- 若扩展取消/截止时间，最可能修改 `Context`、`ContextError` 和 `List::pop`。可考虑让状态改变主动唤醒等待者，但需要设计 Context 与一个或多个 List 的注册关系；同时补充 `expire`、取消先于入队、取消与入队竞态测试。
- 若将本实现接入 Rust Lightning server，需要统一 `pkg/lightning/config/configlist.rs` 与 `lightning/pkg/server/stubs.rs::config::List` 的所有权、命名、错误类型和 `Config` 表示，迁移 `lightning/pkg/server/lightning_server_serial_test.rs` 后再删除重复实现，不能只做方法名适配。
- 若队列可能变长，应评估 `remove`/移动方法的 O(n) 扫描；可引入稳定节点句柄或其他双向索引结构，但必须保持线程安全和 FIFO 可观察行为。
- 测试逻辑应继续放在独立的 `pkg/lightning/config/configlist_test.rs`，不要内嵌回生产文件；Go 对照行为变化时同步检查 `configlist_test.go`。建议补足 `DeadlineExceeded`、多消费者、ID 极值和锁顺序/竞态场景。

## 验证依据

- 源码与符号：`pkg/lightning/config/configlist.rs`；RustCodeGraph `node` 核对了 `ContextError`、`Context`、`State`、`List`，`query` 定位了 `new_config_list`、`all_ids`、`move_to_front`、`move_to_back`。
- 调用关系：RustCodeGraph `callers new_config_list --file pkg/lightning/config/configlist.rs` 返回四个 Rust 单元测试；`callees new_config_list` 为空。对重名方法使用文件限定查询时图索引仍产生跨仓库噪声，因此又以限定 Rust 路径的 `rg` 复核，确认生产侧没有本文件 snake_case API 的直接调用点，并记录这一索引限制而未采用噪声结果。
- crate 边界：`pkg/lightning/config/Cargo.toml` 确认包名、`lib.rs` 入口及依赖；`pkg/lightning/config/lib.rs` 确认模块公开、根级再导出和独立测试装配。
- 数据类型：`pkg/lightning/config/config.rs::Config` 与 `new_config` 确认 `task_id: i64` 及初值零。
- Go 对照：`pkg/lightning/config/configlist.go`、`pkg/lightning/config/configlist_test.go`；应用位置由 `lightning/pkg/server/lightning.go::{RunServer, handleGetTask, handleGetOneTask, handlePostTask, handleDeleteOneTask, handlePatchOneTask}` 核对。
- Rust 测试：`pkg/lightning/config/configlist_test.rs::{test_normal_push_pop, test_context_cancel, test_get_remove, test_move_front_back}`。Rust 服务当前重复实现由 `lightning/pkg/server/stubs.rs::config::{List, NewConfigList}` 及 `lightning/pkg/server/lightning.rs` 核对。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文区分了已验证实现、Go 设计对应和尚未统一的 Rust 生产接线。
