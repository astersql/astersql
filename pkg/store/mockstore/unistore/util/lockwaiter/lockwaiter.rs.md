# `pkg/store/mockstore/unistore/util/lockwaiter/lockwaiter.rs`

## 文件定位

本文件实现 unistore mock storage 的悲观锁等待队列。它以冲突键的 `keyHash` 为分组单位，让等待锁的事务在“锁释放、死锁通知或等待超时”三种终止条件之间竞争，并把结果编码为 `WaitResult`。crate 入口 `pkg/store/mockstore/unistore/util/lockwaiter/lib.rs` 公开 `lockwaiter` 模块并再导出本文件的 API；`Cargo.toml` 将它定义为 `astersql-store-mockstore-unistore-util-lockwaiter`，直接依赖 `crossbeam-channel` 和相邻的 `unistore_config`。

workspace 根 `Cargo.toml` 以 `facade_store_mockstore_unistore_util_lockwaiter` 引入该 crate，`pkg/lib.rs` 再把它放进兼容 facade。`pkg/store/mockstore/unistore/tikv/Cargo.toml` 仅在 Windows target 依赖该 crate；代码搜索未发现 tikv Rust 生产实现调用 `NewManager`、`NewWaiter`、`WakeUp` 或 `WakeUpForDeadlock`。因此当前可确认的是“crate 已装配并有独立测试”，不能宣称 Rust unistore 请求链已经接入它。完整业务位置可由同路径 Go 实现验证：`tikv/mvcc.go` 在遇到悲观锁冲突时登记等待者，`tikv/server.go` 等待并解释结果，提交/回滚路径负责唤醒，`tikv/deadlock.go` 把死锁结果送回管理器。

## 核心职责

- `Manager` 用一个互斥保护的 `HashMap<u64, Queue>` 保存每个冲突键上的等待者，并从 `config::Config.PessimisticTxn.WakeUpDelayDuration` 捕获延迟唤醒配置。
- `NewWaiter` 登记 `(startTS, lockTS, keyHash)`，创建容量为 32 的通知 channel，并设置不可延长的原始截止时间。
- `WakeUp` 对每个已释放的键按 `startTS` 选择最老等待者立即重试；同键剩余等待者只收到延迟通知且继续留在队列中，避免所有事务同时争锁。
- `Waiter::Wait` 统一处理正常唤醒、死锁响应、延迟重试窗口、channel 断开和超时。
- `CleanUp` 负责在 RPC 超时或取消后从队列移除等待者并清空残留通知；`WakeUpForDeadlock` 则精确定位死锁受害者、移出队列并传递死锁信息。

## 主要符号

- `LockNoWait: i64 = -1`：与 TiKV 悲观锁协议一致的“不等待”哨兵值。本文件不消费该值，由调用层决定是否创建 waiter。
- `WaitForEntry { Txn, WaitForTxn, KeyHash }`、`DeadlockResponse { Entry, DeadlockKeyHash }`：Rust 本地的最小死锁通知模型；分别描述等待边和触发环路的键哈希。
- `Queue { waiters }`：单键队列。`get_oldest_waiter` 每次先按 `Waiter.startTS` 升序排序，再移除下标 0；`remove_waiter` 使用 `Arc::ptr_eq` 删除确切 waiter 实例，而不是按字段值删除。
- `Manager { waitingQueues, wakeUpDelayDuration }`：队列所有者。公开入口是 `NewManager`、`NewWaiter`、`WakeUp`、`CleanUp`、`WakeUpForDeadlock`；`waiter_count` 仅在 `cfg(test)` 下存在。
- `WakeupWaitTime = i32` 及 `WaitTimeout (-1)`、`WakeUpThisWaiter (0)`、`WakeupDelayTimeout (1)`：等待结果的三态标记。这里的 `1` 是类别码，不是实际毫秒数；真实延迟来自 manager 配置。
- `WaitResult { DeadlockResp, WakeupSleepTime, CommitTS }`：等待完成值。私有构造器 `timeout` 保证普通超时的 `CommitTS` 为 0，而延迟通知后的计时到期保留通知中的提交时间戳。
- `Waiter`：保存原始 deadline、channel 两端、延迟配置、私有 `startTS` 以及公开的 `LockTS`、`KeyHash`。`Wait` 和 `DrainCh` 都只借用 `&self`，并发状态主要由 channel 消息和局部变量承载。

## 执行流程

1. 调用方用 `NewManager` 创建管理器；构造时只读取一次 `WakeUpDelayDuration`，后续配置对象变化不会反映到既有 manager。
2. 锁冲突且允许等待时，`NewWaiter` 先在锁外创建 bounded channel 和 `Arc<Waiter>`，再锁住 `waitingQueues`，把 waiter 追加到 `keyHash` 对应队列。
3. 调用方执行 `Waiter::Wait`。它以 `deadlineTime` 为当前有效截止时间，通过 `recv_timeout` 等待通知；若没有消息、channel 断开或 deadline 已到，返回超时结果。
4. 锁持有者完成后，`Manager::WakeUp` 遍历 `keyHashes`。每个非空队列经排序后弹出最小 `startTS` waiter；空队列立即从 map 删除，仍有成员的队列保留。
5. `WakeUp` 释放 mutex 后才发送消息：弹出的 waiter 收到 `WakeUpThisWaiter` 和 `commitTS`；仍在队列中的 waiter 收到 `WakeupDelayTimeout` 和同一 `commitTS`。两类通知均用 `try_send`，满 channel 时静默丢弃。
6. `Wait` 收到延迟通知后记录 `commitTS`，把延迟毫秒数按不小于 0 处理，并只在“延迟截止早于原始 deadline”时缩短有效等待窗口；到期后返回 `WakeupDelayTimeout`。普通唤醒或死锁消息则直接返回。
7. 等待结束后，调用方应执行 `CleanUp`。它按 `KeyHash` 找队列、按 `Arc` 身份删除 waiter、删除空队列，释放 mutex 后再调用 `DrainCh`。
8. 死锁检测路径调用 `WakeUpForDeadlock`：按 `Entry.KeyHash` 选队列，再用 `Entry.Txn + Entry.KeyHash` 匹配 waiter；匹配项先从队列移除，随后在锁外发送含 `DeadlockResp` 的结果。

## 数据与状态

核心共享状态只有 `Manager.waitingQueues`，其不变量是每个 map key 对应同一 `KeyHash` 的 waiter 集合。队列采用 `Vec`，登记是追加操作，只有唤醒时才排序，因此“最老”按最小事务 `startTS` 定义，而不是按实际入队先后定义。若 `startTS` 相同，Rust 稳定排序保留原有相对次序，但代码没有把重复 `startTS` 声明为公共契约。

`WakeUp` 弹出的最老 waiter 不再由 manager 持有；剩余 waiter 即使已收到延迟消息仍在队列中，必须由后续 `WakeUp`、`WakeUpForDeadlock` 或 `CleanUp` 移除。`Wait` 的 `commit_ts`、`wakeup_delayed`、`active_deadline` 是单次调用的栈上状态，不写回 `Waiter`；因此 API 没有为同一 waiter 多次或并发调用 `Wait` 定义可复用语义，多个接收者会竞争同一 channel。

所有权方面，map 中的 `Arc<Waiter>` 保持待处理 waiter 存活；waiter 自身同时持有 sender 和 receiver，所以即使离开 manager，channel 也不会自然断开。`main_test.rs` 验证清理/唤醒后释放外部 `Arc` 会释放 waiter，manager 被销毁时也会释放仍在队列中的 waiter；死锁 payload 则由未读消息持有，`CleanUp` 的 drain 或 waiter 析构会释放它。

## 依赖与调用关系

下游依赖很窄：`std::sync::{Arc, Mutex}` 负责共享所有权和队列互斥，`HashMap`/`Vec` 保存队列，`Instant`/`Duration` 计算 deadline，`crossbeam_channel::bounded(32)`、`recv_timeout`、`try_send` 承载唤醒通知，`unistore_config` 提供延迟配置。内部调用边为 `NewWaiter -> bounded/HashMap::entry`，`WakeUp -> Queue::get_oldest_waiter -> Sender::try_send`，`CleanUp -> Queue::remove_waiter -> Waiter::DrainCh`，`WakeUpForDeadlock -> Sender::send`，`Wait -> Receiver::recv_timeout -> WaitResult::timeout`。

RustCodeGraph 能识别本文件 18 个符号并返回完整源码，但对这些方法执行 `callers/callees` 没有返回跨 crate 生产调用边。普通代码搜索进一步确认：本 crate 内调用仅位于 `lockwaiter_test.rs`、`migration_aster_unit_test.rs` 和 `main_test.rs`；workspace facade 在 `pkg/lib.rs` 再导出 API，tikv crate 的 Windows 条件依赖尚无对应 Rust 使用点。

Go 的已接线调用链提供功能意图证据：`tikv/mvcc.go::handleCheckPessimisticErr` 调 `NewWaiter` 并发起死锁检测；`tikv/server.go::KvPessimisticLock` 调 `Wait`，随后清理死锁边和 waiter，再根据三态结果重试或返回；事务完成路径调用 `WakeUp`；`tikv/deadlock.go::DetectorClient` 持有 manager 并把检测响应交给 `WakeUpForDeadlock`。这些是 Go 行为对照，不是当前 Rust 生产接线证据。

## 错误处理与边界

本 API 不返回 Rust `Result`。等待失败通过 `WaitResult` 表达：deadline 到期和 channel 断开都折叠为超时；死锁由可选的 `DeadlockResp` 表达。`WakeUp` 的 `try_send` 错误（channel 满或断开）被忽略，意味着通知不是可靠队列；调用方仍依靠原 deadline 收敛。`WakeUpForDeadlock` 使用阻塞 `send`，也忽略返回错误；因为发送发生在 manager mutex 之外，不会锁住全局队列，但若同一 waiter 的容量 32 channel 被异常填满，该调用可能阻塞。

`waitingQueues.lock().unwrap()` 在 mutex 被 poison 时会 panic。`Queue::get_oldest_waiter` 假设队列非空；`WakeUp` 只对 map 中的队列调用它，而正常操作会删除空队列，因此该假设依赖“map 不保存空队列”的内部不变量。`Instant::now() + timeout` 及延迟 deadline 的加法没有显式处理超大 `Duration` 的溢出。负的配置延迟在 Rust 中被截为 0；这一点与 Go 直接转换为负 `time.Duration` 的细节并不完全等价。

`WakeUp` 的 `_txn` 参数当前未用于筛选，唤醒由 `keyHashes` 决定。`WakeUpForDeadlock` 也不校验 `WaitForTxn` 或 `DeadlockKeyHash`，只以等待事务和键哈希定位。Rust 的本地 `DeadlockResponse` 只含 `Entry` 与 `DeadlockKeyHash`，没有同路径 Go protobuf 响应在上层读取的 `WaitChain`；在真正接入 Rust RPC 链前必须补齐或明确转换边界。

## 并发与资源生命周期

所有队列结构修改均在单个 `Mutex<HashMap<...>>` 下串行化。`NewWaiter` 在加锁前分配对象；`WakeUp` 和 `WakeUpForDeadlock` 在锁内只选择/移除目标，在锁外发送；`CleanUp` 也在解锁后 drain，从而避免 channel 操作扩大临界区。不同 key 仍共享同一 mutex，因此高并发下按键隔离的是数据而非锁竞争。

`WakeUp` 与 `CleanUp`、死锁唤醒可以竞态，但队列锁保证只有先成功移除者取得该 waiter。若 `WakeUp` 已在锁内弹出 waiter，随后 `CleanUp` 找不到它，但仍会 drain 可能已经发送的消息；若 drain 先于锁外发送完成，之后消息仍可能到达，因此调用方应遵循“停止等待后清理并丢弃 waiter”的生命周期，而不能把 drain 视为跨线程发送屏障。`try_send` 限制普通唤醒不会阻塞；死锁发送则可能阻塞但不持 map 锁。

测试证据覆盖：`lockwaiter_test.rs::TestLockwaiterConcurrent` 用线程验证提交唤醒、死锁唤醒和清理后超时；`migration_aster_unit_test.rs::wakes_oldest_waiter_and_delays_the_rest` 验证同键排序及剩余成员仍留队；`deadlock_response_removes_only_matching_waiter` 验证精确移除；`main_test.rs` 验证 waiter 与死锁 payload 的释放。测试与生产源码分文件，符合仓库 Rust 测试组织要求。

## 与 Go 版本的对应关系

Rust 基本逐项对应 `lockwaiter.go`：`Manager`/`queue`/`Waiter`/`WaitResult` 类型、容量 32 的通知通道、按 `startTS` 选择最老 waiter、先解锁再通知、延迟唤醒仍留队、清理时 drain，以及按事务与键精确死锁唤醒均保持相同意图。Rust 把 Go `Waiter` 上的 `CommitTs`、`wakeupDelayed` 和 timer 状态改为 `Wait` 调用内局部变量与 `recv_timeout`，避免共享可变字段；Go 的指针身份删除对应 Rust 的 `Arc::ptr_eq`。

可观察差异包括：Rust manager 按值返回而非指针；死锁 protobuf 被缩成两个本地结构且缺少 `WaitChain`；Rust 将 channel 断开视为超时；Rust 对负延迟取 0；Rust 普通发送明确使用 `try_send`，对应 Go 的非阻塞 `select/default`；死锁发送在两端都是阻塞式。Rust `WakeUp` 保留但不使用 `txn`，Go 只用它记录日志，因此唤醒选择语义一致但 Rust 少了日志可观测性。Go `Waiter` 维护 `time.Timer`，Rust 每轮用绝对 deadline 计算剩余时间；二者都只在延迟窗口早于原 deadline 时缩短等待。

测试对应方面，`lockwaiter_test.rs` 移植 `lockwaiter_test.go` 的基础和并发案例；`migration_aster_unit_test.rs` 额外固定延迟、清理、精确死锁匹配等边界；`main_test.rs` 是 Rust 所有权模型专属的资源释放测试。新增或改变行为时，应同时核对 Go 文件和 Go 测试，避免 Rust 版本为了通过测试而简化既有语义。

## 扩展指南

- 若接入 Rust unistore 生产链，优先在 tikv 的锁冲突登记、事务完成、RPC 等待和 deadlock response 四个边界复用现有 API；同时决定 `DeadlockResponse` 如何承载 Go/protobuf 的完整 `WaitChain`，不能直接假设当前缩减结构足够。
- 若改变公平性或队列结构，修改 `Queue::get_oldest_waiter` 与 `Manager::WakeUp`，并在独立的 `lockwaiter_test.rs` 或 `migration_aster_unit_test.rs` 增加同键乱序、相同 `startTS`、多键批量唤醒测试。需评估每次唤醒排序的 `O(n log n)` 成本与单全局 mutex 的争用。
- 若改变延迟策略，修改 `Waiter::Wait` 和 `WaitResult::timeout`，同步验证延迟不越过原 deadline、连续延迟通知、零/负配置和 commitTS 保留规则，并与 `lockwaiter.go::Wait` 保持语义一致。
- 若改变通知可靠性或 channel 容量，必须同时审查 `WakeUp` 的丢弃策略、`WakeUpForDeadlock` 的阻塞风险和 `CleanUp`/`DrainCh` 竞态；资源释放测试应覆盖未读 payload。
- 若扩大死锁匹配条件，修改 `WakeUpForDeadlock` 并增加同键多个事务、错误 `WaitForTxn`、无匹配响应的测试。现有契约只按 `Txn + KeyHash` 匹配，不应在没有兼容性评估时悄然改变。
- Rust 测试继续放在相邻独立 `*_test.rs` 文件，不要内嵌到生产文件；本文件仅保留 `#[path = "lockwaiter_test.rs"] mod lockwaiter_test` 挂接。任何 Rust 生产修改完成后还需按仓库规则执行 `cargo fmt --all`，本次纯文档任务不触发该步骤。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/store/mockstore/unistore/util/lockwaiter` 返回本目录 8 个 Go/Rust 文件；`node --file .../lockwaiter.rs --offset 1 --limit 400` 读取 348 行及 18 个符号；对 `NewWaiter`、`WakeUp`、`Wait`、`WakeUpForDeadlock` 执行 `callers/callees` 未得到跨 crate 调用边。
- 实现与装配：`pkg/store/mockstore/unistore/util/lockwaiter/lockwaiter.rs`、`lib.rs`、`Cargo.toml`，workspace 根 `Cargo.toml`，`pkg/lib.rs`，以及 `pkg/store/mockstore/unistore/tikv/Cargo.toml`。
- Rust 独立测试：`lockwaiter_test.rs`、`migration_aster_unit_test.rs`、`main_test.rs`，分别覆盖基础/并发、迁移边界和资源生命周期。
- Go 对照：`lockwaiter.go`、`lockwaiter_test.go`；运行链证据来自 `pkg/store/mockstore/unistore/tikv/mvcc.go`、`server.go`、`deadlock.go`。
- 普通搜索证据：Rust 生产代码未发现上述 lockwaiter API 的调用，只有 facade 再导出、Windows 条件依赖和测试调用；因此文档将 Rust 生产接线明确标为尚未验证/当前未发现，而不把 Go 主链冒充 Rust 现状。
- 本任务只创建说明文档，未运行 Cargo。交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核源文件、调用边、Go 对照和测试证据可追溯。
