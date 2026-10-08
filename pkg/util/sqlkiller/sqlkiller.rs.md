# `pkg/util/sqlkiller/sqlkiller.rs`

## 文件定位

本文件是 `astersql-util-sqlkiller` crate 的核心实现，由同目录 `lib.rs` 以 `pub mod sqlkiller` 暴露。它为一个会话或一条语句保存取消原因，把内存超限、执行超时、连接失活等异步事件统一转换成执行层 `SharedError`，并为需要等待取消的组件提供一次性广播事件。crate 边界见 `pkg/util/sqlkiller/Cargo.toml`：直接依赖执行错误定义、日志工具、`fail` 与 `rand`，没有可选 feature；工作区内的 session、executor、statistics、memory、servermemorylimit、distsql 等 crate 直接或经再导出使用它。

该文件不是进程级信号处理器，也不主动轮询所有查询。生产者调用 `SendKillSignal*` 写入会话级状态，执行路径在检查点调用 `HandleSignal` 才把状态变成错误。`pkg/util/memory/lib.rs` 和 `pkg/util/servermemorylimit/lib.rs` 对本模块再导出，`pkg/distsql/context/context.rs` 则在分离后台游标时有意共享同一个 `SQLKiller`。

## 核心职责

1. 定义与 Go `pkg/util/sqlkiller/sqlkiller.go` iota 顺序一致的七个 `KillSignal` 值；`0` 表示未取消，其余六种分别代表显式中断、最长执行时间、单查询内存、实例内存、runaway 管控和内存仲裁器。
2. 用 `Signal.compare_exchange(0, reason, SeqCst, SeqCst)` 实现“首个信号胜出”。后续生产者仍可触发已关闭的事件语义，但不能覆盖最初的错误类别。
3. 用 `KillEventChan` 将一次 kill 广播给多个等待者；`Reset` 关闭旧的未触发事件并建立下一语句可用的新世代，避免旧 waiter 永久阻塞。
4. 在 `HandleSignal` 中执行连接存活检查、读取信号与仲裁描述，并调用 `getKillError` 生成 TiDB 执行错误。
5. 用独立的 `FinishFuncLock` 串行化结果集 `Finish` 回调的设置、调用与清除，为网络写结果集卡住时释放执行资源提供入口。
6. 记录 kill 开始、kill 完成以及实例内存信号被消费的警告日志；日志包含连接 ID 和映射后的原因。

## 主要符号

- `pub type KillSignal = u32`：信号编码。`UnspecifiedKillSignal` 到 `KilledByMemArbitrator` 的数值必须保持 `0..=6`，与 Go 常量及跨 crate 判断一致。
- `KillEvent`：私有的 `Mutex<bool> + Condvar` 状态；布尔值是永久关闭标志。
- `KillEventChan`：可克隆接收端。`is_closed` 非阻塞读取状态，`wait_timeout` 等待关闭并仅在超时时返回 `false`。`new`、`close` 只供模块内部创建和触发。
- `KillEventState`：在同一互斥区内保存可选通道、仲裁描述 `desc` 与 `triggered`。它与 `Signal` 写入由 `killEvent` 锁串行化。
- `AtomicPointer<T>`：以 `Mutex<Option<T>>` 模拟 Go `atomic.Pointer` 的 `Load`/`Store`；读取要求 `T: Clone`。这里分别承载 `Instant` 和连接存活回调。
- `ConnectionAliveFn`：`Arc<dyn Fn() -> bool + Send + Sync>`；返回 `false` 时产生 `QueryInterrupted`。
- `FinishFn`：可变、可发送且只拥有一次实例的 boxed 回调。
- `SQLKiller`：公共主类型。`ConnID`、`Signal`、`InWriteResultSet` 是原子字段；`Finish` 和 `FinishFuncLock` 管理回调；`killEvent` 管理事件、描述和世代；`lastCheckTime` 与 `IsConnectionAlive` 管理存活探测。`Default`/`new` 创建无信号、无回调、无事件通道的实例。
- `GetKillEventChan`：惰性创建当前世代通道；若信号已在通道创建前触发，返回已关闭通道。
- `SendKillSignal` / `SendKillSignalWithKillEventReason`：公共生产入口；后者先保存描述，主要供内存仲裁器使用。私有 `sendKillSignalLocked` 完成首写 CAS，`triggerKillEventLocked` 广播，`logKillSignal` 在锁外记录首个成功信号。
- `GetKillSignal` / `HandleSignal` / `CheckConnectionAlive`：分别负责原始信号读取、检查并返回执行错误、立即探测连接。
- `FinishResultSet` / `SetFinishFunc` / `ClearFinishFunc`：在固定锁顺序下调用、替换或移除完成回调。
- `Reset`：原子取走旧信号，在仍持有 `killEvent` 锁时重置事件状态，随后清除连接探测节流时间。

## 执行流程

常规取消链如下：生产者调用 `SendKillSignal(reason)`；函数持有 `killEvent` 锁，以 CAS 尝试把 `Signal` 从 `0` 改为 `reason`，关闭已经创建的 `KillEventChan` 并置 `triggered=true`；释放锁后，仅 CAS 胜出的调用记录一次 `kill initiated`。执行器、会话锁等待或统计计算等检查点调用 `HandleSignal`，读取原子状态，经 `getKillError` 映射后返回 `Err(SharedError)`，上层用 `?` 或显式转换终止当前工作。

带描述的仲裁链由 `pkg/util/memory/tracker.rs` 的 `TrackerArbitrateHelper::Stop` 发起：`SendKillSignalWithKillEventReason(KilledByMemArbitrator, reason.String())` 在同一锁内写描述、CAS 信号并触发事件。`HandleSignal` 发现该信号后重新在 `killEvent` 锁内读取信号与描述，避免把来自不同状态世代的值拼接，再生成 `ErrQueryExecStopped(desc, ConnID)`。

超时链在 `pkg/session/runtime/scan_adapter_runtime.rs` 的 `KillSignal` 检查 deadline，超期后发送 `MaxExecTimeExceeded`。单查询内存链在 `pkg/util/memory/action.rs` 的 `PanicOnExceed::Action` 发送 `QueryMemoryExceeded` 后立刻消费错误；实例内存链在 `pkg/util/servermemorylimit/servermemorylimit.rs` 选择最大内存消费者后发送 `ServerMemoryExceeded`。`pkg/executor/typed_hash_join.rs`、`typed_index_lookup.rs` 以及 session runtime 的循环在工作检查点消费这些错误。

连接探测有两条路径：`HandleSignal` 首次只保存 `Instant`，之后只有间隔超过测试态 1ms、普通态 1s 才调用回调；`CheckConnectionAlive` 不节流并立即调用。回调返回 `false` 时，私有 `sendKillSignal` 只写信号和日志，不关闭 kill 事件，这一行为与 Go 对照实现一致。

语句结束或新语句开始时调用 `Reset`：持有 `killEvent` 锁，`swap` 清零 `Signal`，关闭尚未触发的旧通道、丢弃旧通道和描述并清除 `triggered`；有旧信号时在锁外记录 `kill finished`，最后清空 `lastCheckTime`。下一次 `GetKillEventChan` 因而得到新的开放事件。

## 数据与状态

`Signal` 是取消结果的判定源，首写胜出且以 `SeqCst` 访问。`killEvent.triggered` 是当前事件世代是否已经广播的记录，`killEvent.desc` 只在内存仲裁错误中成为用户可见参数；二者不能脱离 `killEvent` 锁解释。事件通道关闭是单向状态：`KillEventChan::close` 幂等，克隆接收端共享同一个 `Arc<KillEvent>`，所以一次 `notify_all` 可唤醒全部 waiter。

`Reset` 是世代边界。若旧世代尚未触发但已有 waiter，重置会主动关闭旧通道，使 waiter 不会悬挂；随后把 `ch` 设为 `None`，下一次获取创建新通道。若 kill 已先触发，旧通道已经关闭，重置不重复关闭。

`ConnID` 只用于错误参数和日志，不参与信号竞争。`InWriteResultSet` 在本文件内不读写，是为调用方保留的 Go 同步契约字段。`Finish` 自身是 `Mutex<Option<FinishFn>>`，所有公共访问还先取得 `FinishFuncLock`；固定顺序是外层 `FinishFuncLock`、内层 `Finish`，禁止扩展代码反向取锁。`lastCheckTime` 和 `IsConnectionAlive` 的“原子指针”实际由互斥锁与克隆实现，不具备无锁性能特性。

## 依赖与调用关系

下游依赖包括：`crate::errors::{ErrorArg, SharedError}` 与 `crate::exeerrors` 用于错误构造；`crate::logutil::log` 用于后台告警；标准库原子、`Mutex`、`Condvar`、`Arc` 与时间类型用于并发状态；`fail` 和 `rand` 仅支撑注入路径。它们由 `lib.rs` 对执行错误和日志 crate 做本地再导出，具体依赖声明在同目录 `Cargo.toml`。

已核验的生产调用边包括：

- `pkg/session/runtime/scan_adapter_runtime.rs::KillSignal -> SQLKiller::SendKillSignal(MaxExecTimeExceeded)`；
- `pkg/util/memory/action.rs::PanicOnExceed::Action -> SendKillSignal(QueryMemoryExceeded) -> HandleSignal`；
- `pkg/util/servermemorylimit/servermemorylimit.rs -> SendKillSignal(ServerMemoryExceeded)`；
- `pkg/util/memory/tracker.rs::TrackerArbitrateHelper::Stop -> SendKillSignalWithKillEventReason(KilledByMemArbitrator, ...)`；
- `pkg/session/runtime/control.rs` 的锁等待、编译配额和语句初始化路径消费 `HandleSignal`，并在新仲裁语句前调用 `Reset`；
- `pkg/executor/typed_hash_join.rs`、`typed_index_lookup.rs` 等执行器在循环检查点调用 `HandleSignal`；
- `pkg/distsql/context/context.rs` 的 `Detach` 继续共享 `SQLKiller`，使后台游标保留取消能力。

RustCodeGraph 索引将本文件识别为 47 个符号，并检出 session、executor、statistics、memory、server 等使用点。精确 callers/callees 命令在本次环境中超时，以上调用边均进一步由对应生产源码直接核验，不把搜索结果本身当作行为证明。

## 错误处理与边界

`getKillError` 的映射为：`QueryInterrupted -> ErrQueryInterrupted`，`MaxExecTimeExceeded -> ErrMaxExecTimeExceeded`，`QueryMemoryExceeded -> ErrMemoryExceedForQuery(ConnID)`，`ServerMemoryExceeded -> ErrMemoryExceedForInstance(ConnID)`，`RunawayQueryExceeded -> ErrResourceGroupQueryRunawayInterrupted("runaway exceed tidb side")`，`KilledByMemArbitrator -> ErrQueryExecStopped(desc, ConnID)`。未知值与 `UnspecifiedKillSignal` 都返回 `None`，所以 `HandleSignal` 返回 `Ok(())`；调用者若传入新常量却忘记扩展映射，会静默失去中断错误。

事件与状态相关锁大多用 `expect`，发生 mutex poisoning 会 panic；结果集回调两把锁则用 `PoisonError::into_inner`，即使回调此前 panic，后续仍能替换和调用回调。`wait_timeout` 处理虚假唤醒，只有关闭标志变真才成功。`HandleSignal` 的 `randomPanic` failpoint 名称来自 Go，但当前 Rust 逻辑实际随机写入 `0..4` 的信号值而不是直接 panic；它只在非零 `ConnID` 时启用，属于测试注入而非生产取消来源。

`SendKillSignalWithKillEventReason` 会在 CAS 前覆盖 `desc`；若首信号已经存在，错误类别仍不变，但描述字段可变化。因此只有 `KilledByMemArbitrator` 路径在持锁复读信号与描述，扩展时不应假设描述对所有失败 CAS 都不可变。`sendKillSignal`（连接失活内部路径）不触发 `KillEventChan`，等待事件的消费者不能用它替代显式 `SendKillSignal`。

Go 注释要求新增信号时同时更新 `store/driver/error/ToTiDBErr`，以保证客户端错误可转换；Rust 中同样需要检查 `pkg/store/driver/error` 的映射。信号数值是兼容边界，不能重排已有常量。

## 并发与资源生命周期

信号写入和事件状态变更的临界区统一由 `SQLKiller.killEvent` 保护；信号原子仍用于高频无锁读取。`SendKillSignal*` 与 `Reset` 的关键不变量是：清零/首写 `Signal` 和重置/触发事件不能交错成跨世代状态。`Reset` 因此在执行 `Signal.swap` 后仍持有锁，测试 failpoint 专门验证并发发送者必须等待。日志刻意放在锁外，避免日志系统扩大关键区或形成锁循环。

`KillEventChan` 的资源由 `Arc` 管理，发送端没有独立对象；关闭状态随最后一个 clone 释放。等待使用 `Condvar`，不会创建后台线程。`SQLKiller` 本身也不创建任务，调用方负责在执行循环设置检查点。

`FinishResultSet` 在持有 `FinishFuncLock` 时调用用户回调，这保证回调不会与替换/清除并发，但也意味着回调必须短小且不得重入 `SetFinishFunc`、`ClearFinishFunc` 或 `FinishResultSet`，否则会自锁。连接回调在 `AtomicPointer` 锁释放后的克隆对象上调用，因此不会持有该字段的互斥锁跨越外部代码。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/util/sqlkiller/sqlkiller.go`：信号常量和值、`SQLKiller` 字段布局意图、首信号 CAS、kill channel 的惰性创建/关闭、带描述的仲裁错误、Finish 回调协议、连接存活节流和 `Reset` 世代语义均保留。Go 的 `chan struct{}` 在 Rust 中实现为共享关闭标志加条件变量；Go `atomic.Pointer` 被 `AtomicPointer<T>` 的互斥锁实现替代；Go `error` 返回被表达为 `Result<(), SharedError>`。

Rust 相比 Go 多了 `Default`/`new`、可显式等待超时的 `KillEventChan::wait_timeout`，并用 `Arc`/`Box` 明确回调所有权。Rust 的 `Finish` 额外有内部 mutex，但仍保留 `FinishFuncLock` 以维持 Go 外部同步契约；对 poison 的恢复是 Rust 特有处理。

相关行为由 `pkg/util/sqlkiller/migration_aster_unit_test.rs` 覆盖首信号胜出、广播、Reset 新世代、六类错误映射、Finish 回调与连接节流；`pkg/util/sqlkiller/sqlkiller_test.rs` 和 `go_merge_34_test.rs` 对照 `pkg/util/sqlkiller/sqlkiller_test.go`，验证 Reset 与并发发送/日志之间的锁序。测试独立于生产源文件，新增行为应继续放在这些独立测试文件中。

## 扩展指南

新增 kill 原因时，应只在现有尾部追加常量，保持 Go/Rust 数值一致；同步修改 `getKillError`、Go `getKillError`、客户端错误转换（特别是 `pkg/store/driver/error`）、所有穷举测试与本文信号说明。若新原因需要附加数据，优先先定义清晰的锁内状态契约；当前单个 `desc` 只为内存仲裁路径设计，盲目复用可能被失败 CAS 的发送者覆盖。

新增生产者时，应选择正确入口：需要唤醒事件 waiter 时使用 `SendKillSignal`，需要错误描述时使用带原因版本；仅连接探测内部路径才使用不广播的私有 `sendKillSignal`。新增长循环或阻塞点时，在可安全退出、可传播 `SharedError` 的边界调用 `HandleSignal`，不要只读 `GetKillSignal` 后制造另一套错误文案。

调整 Reset 或事件逻辑时必须维持同一 `killEvent` 临界区覆盖原子清零与事件重置，并同步 `sqlkiller_test.rs`、`go_merge_34_test.rs` 及 Go 测试的竞态用例。调整 Finish 回调时保持锁顺序 `FinishFuncLock -> Finish`，并增加 panic、重入或并发替换测试；不要把 Rust 测试内嵌回本文件。优化 `AtomicPointer` 时需证明连接探测回调与时间戳的 Clone/Send/Sync 语义，并评估高频 `HandleSignal` 的锁开销。

性能风险主要在执行热循环：`Signal.load` 很轻，但启用存活回调后 `lastCheckTime.Load/Store` 会取得 mutex；不要在每行处理时加入额外分配或日志。兼容风险集中在信号编号、错误码/文案和 Reset 世代；正确性风险集中在锁顺序、首信号不被覆盖以及描述与信号的一致快照。

## 验证依据

- 源与 crate：`pkg/util/sqlkiller/sqlkiller.rs`、`pkg/util/sqlkiller/lib.rs`、`pkg/util/sqlkiller/Cargo.toml`。
- Go 对照：`pkg/util/sqlkiller/sqlkiller.go`、`pkg/util/sqlkiller/sqlkiller_test.go`。
- Rust 独立测试：`pkg/util/sqlkiller/migration_aster_unit_test.rs`、`pkg/util/sqlkiller/sqlkiller_test.rs`、`pkg/util/sqlkiller/go_merge_34_test.rs`。
- 直接调用证据：`pkg/util/memory/action.rs`、`pkg/util/memory/tracker.rs`、`pkg/util/servermemorylimit/servermemorylimit.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/session/runtime/control.rs`、`pkg/executor/typed_hash_join.rs`、`pkg/executor/typed_index_lookup.rs`、`pkg/distsql/context/context.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/sqlkiller` 列出本 crate 的 Rust/Go 源与测试；`query SQLKiller`、`query SendKillSignal`、`query HandleSignal` 等确认目标定义和跨模块候选。精确 `callers`/`callees` 查询 30 秒内未返回，故调用关系用上述生产源码逐一复核。
- 本任务为纯文档分析，按任务约束未运行 Cargo。结构校验要求本文恰好包含任务规定的十一个二级标题；人工复核重点为信号数值、CAS 首写、事件世代、错误映射、锁顺序和 Go 对应关系均有源码或测试依据。
