# `pkg/resourcemanager/poolmanager/task_manager_iterator.rs`

## 文件定位

本文件是 `astersql-resourcemanager-poolmanager` crate 内部的任务候选选择器。crate 入口 [`lib.rs`](lib.rs) 依次用 `include!` 合并 `task_manager.rs`、本文件和 `task_manager_scheduler.rs`，因此这里的私有 `TaskManager` 方法可以直接访问前一文件定义的私有字段，并由后一文件的公开调度门面调用。crate 边界由 [`Cargo.toml`](Cargo.toml) 声明，运行时外部依赖只有 `crossbeam-channel = "0.5"`；本文件自身使用的 `Instant`、`Ordering`、`TaskManager` 和 `Meta` 都来自同一合并模块的已有导入或定义。

在完整资源调度链中，[`Pool::tune`](../pool/spool/spool.rs) 根据容量变化调用 `TaskManager::Overclock` 或 `TaskManager::Downclock`；[`task_manager_scheduler.rs`](task_manager_scheduler.rs) 再分别转入本文件的 `getBoostTask` 与 `pauseTask`。所以本文件不决定何时调容，也不直接创建或销毁 worker，而是在已注册任务中选择应扩容或缩容的具体对象。

## 核心职责

- `getBoostTask` 为扩容选择候选：优先接受 `running < initialConcurrency` 的任务；没有这种立即命中的候选时，用较新的 `createTS` 更新候选。
- `pauseTask` 为缩容选择候选：优先接受已经超出初始并发度的任务；否则在仍有 worker 运行的任务中倾向更早创建的任务，并向最终候选的 `exitCh` 非阻塞发送一次信号。
- `iter` 统一完成八个任务分片的只读扫描、基准初始化、谓词比较和提前终止，避免升降容复制遍历逻辑。
- `canBoost`、`canPause` 只表达候选比较规则；它们不获取锁、不修改 `Meta`，返回值第二位用于通知 `iter` 是否可停止整个扫描。

本文件只负责“选谁”和发送缩容信号。任务登记、分片、计数及通道类型在 [`task_manager.rs`](task_manager.rs)，公开 API 在 [`task_manager_scheduler.rs`](task_manager_scheduler.rs)，worker 启停和池级容量计数在 [`../pool/spool/spool.rs`](../pool/spool/spool.rs)。

## 主要符号

- `TaskManager::getBoostTask(&self) -> (u64, Option<Meta>)`：私有扩容入口，以 `canBoost` 调用 `iter`。返回任务 ID 和共享底层状态的 `Meta` 克隆；无候选时为 `(某个扫描到的 ID 或 0, None)`，调用者必须以 `Option` 为准，不能只判断 ID。
- `TaskManager::pauseTask(&self)`：私有缩容入口，以 `canPause` 调用 `iter`。找到候选后检查 `exitCh.is_closed()`，再调用 `try_send()`；发送结果被有意忽略，使调度线程不会因满通道或接收端状态而阻塞。
- `TaskManager::iter(&self, f: fn(&Meta, Instant) -> (bool, bool)) -> (u64, Option<Meta>)`：通用扫描器。函数指针参数限制为不捕获环境的同步谓词；返回候选 ID 和克隆的元数据。
- `canPause(m: &Meta, minv: Instant) -> (bool, bool)`：若 `initialConcurrency < running` 且仍在运行，返回 `(true, true)` 立即选中并停止；否则仅当任务更早创建且仍在运行时返回 `(true, false)`。
- `canBoost(m: &Meta, maxv: Instant) -> (bool, bool)`：若 `running < initialConcurrency`，返回 `(true, true)` 立即选中并停止；否则仅当任务更晚创建时返回 `(true, false)`。

文件没有模块级常量、类型、trait 或条件编译项，也没有公开符号；所有 API 都依附于同一 `include!` 后的 crate 模块。

## 执行流程

扩容路径如下：

1. `Pool::tune(size)` 发现 `old < size` 且池的实际运行数小于新容量。
2. 它调用公开的 `TaskManager::Overclock`，后者调用 `getBoostTask`。
3. `getBoostTask` 将 `canBoost` 交给 `iter`。`iter` 按固定分片顺序扫描，但每个分片内部是 `HashMap`，迭代次序不稳定。
4. 尚无候选时，当前条目总会更新 `tid` 和时间基准；只有 `running != 0` 才把该 `Meta` 设为候选。若候选已存在，`canBoost` 可选择低于初始并发的任务并立即结束，或用更晚创建的任务替换候选。
5. `Pool::tune` 仅在返回的 `Option<Meta>` 为 `Some` 时增加池级 `running` 并启动一个执行 `run_task(meta)` 的保留线程。

缩容路径如下：

1. `Pool::tune(size)` 发现池级运行数仍大于新容量，调用 `TaskManager::Downclock`。
2. `Downclock` 调用 `pauseTask`，后者用 `canPause` 扫描。
3. 已超出 `initialConcurrency` 的任务是最高优先级，并触发全局提前结束；否则比较创建时间，保留较老且 `running != 0` 的候选。
4. `pauseTask` 对候选的有界退出通道执行非阻塞发送。若信号已在容量为 1 的通道中，第二次发送可以失败，但不会阻塞或传播错误。
5. [`run_task`](../pool/spool/spool.rs) 的 `recv_event` 收到 `MetaEvent::Exit` 后退出；其 `RunningTask` 守卫在 `Drop` 中调用 `Meta::DecTask`，池所在线程包装层再回收池级运行数。

`iter` 每次只持有一个分片的 `RwLock` 读锁。谓词请求提前停止时，内层循环先退出，读守卫在离开该轮分片作用域时释放，再退出外层循环。

## 数据与状态

候选判断读取 [`Meta`](task_manager.rs) 的四类状态：`createTS: Instant` 表示注册元数据的创建时刻；`running: Arc<AtomicI32>` 表示该任务当前 worker 数；`initialConcurrency: i32` 是注册时并发基准；`exitCh: SignalChannel` 是缩容通知通道。`Meta::clone` 会克隆通道句柄和 `Arc<AtomicI32>`，因此返回值不是独立快照，后续仍观察并操作同一运行计数与通道。

`TaskManager::task` 是固定八分片的 `Vec<TaskStatusContainer>`，每个 `stats` 是 `RwLock<HashMap<u64, Meta>>`。`iter` 的局部状态包括：`compareTS`（当前时间比较基准）、`tid`（候选 ID，初值 0）、`result`（候选元数据）和每分片的 `break_find`。不存在稳定的全局排序：分片按向量顺序访问，但 `HashMap` 内次序不保证；只有“低于/高于初始并发时立即命中”和相对创建时间比较是代码保证。

一个容易误用的不变量是 `tid` 与 `result` 并非始终同步表示有效候选：在 `result.is_none()` 分支中，即使当前任务的 `running == 0`，代码仍更新 `tid` 和 `compareTS`。因此所有调用方都必须把 `Option<Meta>` 作为是否选中的权威标志。

## 依赖与调用关系

上游调用边由模块源码和精确引用确认：

- `Pool::tune` → `TaskManager::Overclock` → `getBoostTask` → `iter(canBoost)`。
- `Pool::tune` → `TaskManager::Downclock` → `pauseTask` → `iter(canPause)`。
- `Pool::run_with_concurrency` 创建 `Meta` 并调用 `RegisterTask`，为本文件提供待扫描数据。
- `run_task` 调用 `Meta::IncTask`、循环等待 `Meta::recv_event`，并在退出时调用 `DecTask`，使这里读取的 `running` 与实际 worker 生命周期关联。

下游数据和同步原语来自 [`task_manager.rs`](task_manager.rs)：`TaskManager.task`、`TaskStatusContainer.stats`、`Meta.createTS`、`Meta.running`、`Meta.initialConcurrency`、`Meta.exitCh` 和 `SignalChannel::try_send`。原子读统一使用 `Ordering::SeqCst`，与同文件的 `IncTask`/`DecTask` 一致。

RustCodeGraph 能识别本文件的 `canPause` 和 `canBoost`，也能确认 `task_manager_scheduler.rs` 被资源池及调度测试使用；但由于三个实现文件通过 `include!` 合并，索引未为 `getBoostTask`、`pauseTask`、`iter` 生成可查询的精确方法节点/调用边。上述私有调用关系因此由 `lib.rs` 的包含顺序、`task_manager_scheduler.rs` 和 `spool.rs` 的精确引用交叉核验，而不是依据缺失的图边推断。

## 错误处理与边界

本文件没有 `Result` 返回值或日志路径。`stats.read().unwrap()` 在分片锁中毒时会 panic，这是当前实现明确采用的失败策略。`pauseTask` 则相反：`try_send` 的 `Full` 或 `Disconnected` 错误均被丢弃，保证调度循环非阻塞，但调用方无法得知本次缩容信号是否真正入队。

空管理器或全部任务 `running == 0` 时，`iter` 返回 `None`；此时 ID 可能是 0，也可能是最后一次用作基准的任务 ID。`canPause` 不会选择运行数为 0 的任务。`canBoost` 在候选已经建立后可选择运行数为 0 且低于初始并发的任务，但最先扫描到的零运行任务本身不会成为初始 `result`；这是当前扫描初始化逻辑的实际行为。

`running` 与 `initialConcurrency` 使用有符号整数，本文件不校验负值，也不防御外部计数失衡。创建时间只来自同一进程内的单调 `Instant`，不能持久化或跨进程比较。候选选择期间其他分片仍可并发变化，因此结果是逐分片观察到的状态，不是所有分片的原子快照。

`SignalChannel::is_closed` 读取自身的 `closed` 原子标志；截至当前 [`task_manager.rs`](task_manager.rs) 没有修改该标志或关闭 `SignalChannel` 的公开方法。因此该预检查目前通常为 false，真正的发送可用性仍由 `try_send` 决定。扩展关闭语义时必须同时复核这里的检查与错误处理。

## 并发与资源生命周期

每个分片的 `RwLock` 允许登记/删除使用写锁，而候选扫描使用读锁。扫描不会同时锁住两个分片，降低锁范围和死锁风险；代价是跨分片选择不具备一致快照语义。`Meta.running` 使用 `SeqCst` 原子操作，可在不升级分片锁的情况下由 worker 增减、由选择器读取。

`Meta` 的克隆共享 `running`、任务通道和退出通道。扩容返回的克隆被移动到新线程，线程进入 `run_task` 后才递增任务级 `running`；选择完成与计数递增之间存在窗口，外层 `Pool::tune` 的 `admission` 互斥锁串行化池级调容，但本文件本身不提供全局保留/占位机制。

缩容通道是 `Meta::new` 创建的容量 1 有界通道。非阻塞发送避免调度线程等待 worker；worker 在 `recv_event` 中与任务、关闭和退出事件进行选择。发送成功只表示退出事件已排队，不表示 worker 已经退出；实际资源回收发生在 `run_task` 返回及其守卫析构之后。重复 `Downclock` 可能遇到满通道并静默失败，这是容量限制和忽略 `try_send` 结果共同形成的背压行为。

## 与 Go 版本的对应关系

直接对照文件是 [`task_manager_iterator.go`](task_manager_iterator.go) 和 [`task_manager_scheduler.go`](task_manager_scheduler.go)。Rust 保留了 Go 的方法名、两布尔值谓词协议、八分片扫描、候选初始化、时间偏好和提前退出规则：`canPause` 优先回收已 boost 的任务，否则偏向老任务；`canBoost` 优先补足未达初始并发的任务，否则偏向新任务。

同步语义也保持对应：Go 在每个分片上持有 `RLock`，Rust 使用 `RwLock::read`；Go 对 `exitCh` 使用带 `default` 的 `select`，Rust 使用 `try_send`，二者都不阻塞。Go 的 `*Meta` 直接共享状态，Rust 的 `Meta::clone` 通过 `Arc` 和可克隆 channel 句柄共享关键状态。

可见差异是 Go 用 `nil` 判断退出通道，Rust 的 `SignalChannel` 不是可空类型，改为 `is_closed` 检查；当前 Rust 的 `closed` 标志没有写入路径，实际失败由 `try_send` 吞掉。Go 的零值 `time.Time` 用作初始比较值，Rust 先用 `Instant::now()`，但在每个 `result.is_none()` 的条目上都会立刻改写 `compareTS`，所以后续比较仍以扫描到的条目为基准。两侧都依赖 map 的非稳定遍历次序，不承诺同优先级候选的确定性。

测试对应关系：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 独立验证低于初始并发的任务被 `Overclock` 选中，以及超频任务收到 `Downclock` 信号且重复调用不阻塞；[`../pool/spool/spool_test.rs`](../pool/spool/spool_test.rs) 验证池从 1 扩到 3、再缩回 1 的端到端运行数变化；Go 的 [`../pool/spool/spool_test.go`](../pool/spool/spool_test.go) 中 `TestWithTaskManager` 验证同一调容序列。

## 扩展指南

若要改变候选优先级，首选修改 `canBoost` 或 `canPause`，并保持 `(is_find, pause_find)` 的含义清晰；需要更多上下文或捕获配置时，才考虑把 `iter` 的函数指针泛化为闭包。任何排序变化都必须明确处理 `HashMap` 非确定次序、跨分片非原子快照和相同创建时间/并发条件，不能把当前结果描述为稳定排序。

若要改变遍历或返回契约，应修改 `iter`，并同步检查 `getBoostTask`、`pauseTask` 以及公开门面。尤其不要让调用者仅凭返回 ID 判断成功；更安全的演进方向是让 ID 与 `Meta` 绑定在同一个 `Option` 中，但这会改变 `Overclock` API，必须同步 `spool.rs` 和 Go 兼容面。

若要增加可靠缩容确认、关闭语义或多信号积压，需要同时修改 `pauseTask`、`SignalChannel`、`Meta::recv_event` 和 `run_task` 生命周期；必须评估阻塞风险、丢信号行为和 worker 计数回收。若只增加选择测试，应继续放在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 或 [`../pool/spool/spool_test.rs`](../pool/spool/spool_test.rs)，不要把测试嵌入生产 `.rs` 文件。

建议补充的边界回归包括：空管理器、全部任务零运行、多个分片中同时存在可立即命中的任务、仅按创建时间替换候选、满退出通道的重复 `Downclock`，以及登记/删除与扫描并发发生时不死锁。若要求与 Go 完全对齐，应在 Rust 独立测试和 `spool_test.go` 的同类情景中同步维护意图。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的八个 Rust/Go 文件均在索引中；`node --file pkg/resourcemanager/poolmanager/task_manager_iterator.rs` 读取了目标文件完整 103 行。
- RustCodeGraph 精确查询：`query canPause --kind function` 与 `query canBoost --kind function` 同时返回 Rust 和 Go 对照符号；对两个 Rust 限定名运行 `callers`/`callees` 未返回边，结合 `include!` 结构记录为索引覆盖限制，未将空结果误写为“无调用者”。
- crate 与模块证据：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`task_manager.rs`](task_manager.rs)、[`task_manager_scheduler.rs`](task_manager_scheduler.rs)。
- 上下游运行证据：[`../pool/spool/spool.rs`](../pool/spool/spool.rs) 的 `Pool::tune`、`run_with_concurrency`、`run_task`，以及 [`../schedule.rs`](../schedule.rs) 的 `ResourceManager::Exec`。
- Go 对照证据：[`task_manager_iterator.go`](task_manager_iterator.go)、[`task_manager_scheduler.go`](task_manager_scheduler.go)、[`../pool/spool/spool.go`](../pool/spool/spool.go)。
- 测试证据：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `overclock_prefers_a_task_below_initial_concurrency`、`downclock_signals_an_overclocked_task_without_blocking`；[`../pool/spool/spool_test.rs`](../pool/spool/spool_test.rs) 和 [`../pool/spool/spool_test.go`](../pool/spool/spool_test.go) 的 `TestWithTaskManager`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付检查仅验证文档结构、链接目标和事实来源。
