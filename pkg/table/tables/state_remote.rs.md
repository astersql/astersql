# `pkg/table/tables/state_remote.rs`

## 文件定位

本文件属于 `astersql-table-tables` crate；crate 由 `pkg/table/tables/Cargo.toml` 定义，入口 `pkg/table/tables/lib.rs` 通过公开模块 `pub mod state_remote` 暴露它。它位于缓存表实现的远程锁状态边界：上层协议由 `pkg/table/tables/cache.rs` 中的 `StateRemote` trait 定义，本文件用泛型 `StateRemoteHandle<S>` 实现该协议，并把实际时间戳读取、锁行加载和锁行更新下沉到可注入的 `RemoteStore`。

这里的“远程”是协议角色而不是现成的网络客户端。文件内唯一具体后端 `MemoryRemoteStore` 只是进程内 `HashMap`；真正接入 `mysql.table_cache_meta` 或其他共享存储，需要仓库外或后续实现一个满足事务互斥约束的 `RemoteStore`。与之相比，Go 文件 `pkg/table/tables/state_remote.go` 直接使用内部 SQL 和事务访问系统表。

## 核心职责

- 用 `CachedTableLockType::{None, Read, Intend, Write}` 和 `LockRow` 表示缓存表锁元数据，并保持锁类型字符串与 Go/SQL 协议的 `NONE`、`READ`、`INTEND`、`WRITE` 一致。
- 用 `RemoteStore` 隔离时间戳和持久化操作，使租约状态机不依赖具体数据库客户端。
- 用 `StateRemoteHandle<S>` 实现读锁获取、写锁获取、读写租约续期，并维护一个允许陈旧的本地锁快照以减少安全窗口内的远程访问。
- 在读锁切换写锁时执行 `Read -> Intend -> Write`，保留旧读租约并计算等待时间，防止旧读者尚在安全窗口时写入。
- 提供 `MemoryRemoteStore` 与 `MemoryRemoteStoreError`，支持嵌入式使用和可控时钟的确定性独立测试；它不实现跨进程协调，也不兑现 `for_update` 的互斥语义。

## 主要符号

- `LOGICAL_BITS: u32 = 18`：TSO 混合时间戳的逻辑低位宽度；`wait_for_lease_expire` 用它提取物理毫秒。
- `CachedTableLockType`：四态锁枚举。`as_str()` 是无分配的协议字符串映射；不同于 Go 的 `String()`，Rust 枚举无法构造枚举定义外的非法值，因此没有 Go 的 panic 分支。
- `LockRow { lock_type, lease, old_read_lease }`：一行远端元数据。`lease` 是当前锁截止时间，`old_read_lease` 只在读转写协调时记录旧读锁安全窗口。
- `RemoteStore: Send`：存储适配器接口。`current_ts()` 给出当前 TSO，`load(table_id, for_update)` 读取行，`update(table_id, row)` 写回完整行。实现者必须保证 `for_update = true` 的 load/update 序列具有与 Go 悲观事务相当的独占性；trait 本身不能强制这一点。
- `MemoryRemoteStore`：保存 `now` 和 `HashMap<i64, LockRow>`。`new`、`set_current_ts`、`insert`、`row` 分别用于初始化时钟、推进时钟、预置和观察行。
- `MemoryRemoteStoreError`：缺行错误，显示文本为 `table_cache_meta tid not exist <table_id>`，与 Go `loadRow` 的错误语义对齐。
- `StateRemoteHandle<S>`：持有 `store` 以及本地 `lock_type`、`lease`、`old_read_lease`。`new` 从无锁零租约开始；`store`/`store_mut` 暴露后端；`local_state` 返回本地快照。
- `load_row`/`load`：前者读取并同步全部本地字段，后者执行非独占读取。
- `lock_for_read_inner`、`lock_for_write_once`、`renew_read_lease_inner`、`renew_write_lease_inner`：可直接测试的状态机核心。
- `impl StateRemote for StateRemoteHandle<S>`：公开协议适配层；写锁入口额外执行本地安全窗口快速路径和等待重试循环。
- `wait_for_lease_expire`：从两个混合时间戳计算等待量；同一物理毫秒但逻辑值尚未过期时返回最小 `1µs`。

## 执行流程

读锁路径由 `StateRemote::lock_for_read` 转给 `lock_for_read_inner`：

1. 若本地快照是 `Intend` 或 `Write`，且本地租约不小于申请值，直接返回 `false`，避免远程访问。
2. 读取 `current_ts`，再用 `load_row(table_id, false)` 加载远端行并刷新本地快照。
3. 若远端租约已经过期（严格条件 `now > row.lease`），只有 `new_lease > now` 才把行改成 `Read` 并更新；否则失败。
4. 未过期的 `Write`/`Intend` 拒绝读者；`None`/`Read` 接受读者，并且只在新租约更大时更新，租约绝不回退。

写锁公开路径由 `StateRemote::lock_for_write` 驱动：

1. 本地已是 `Write` 时，以 `lease_from_ts(now, lease_duration / 2)` 形成安全阈值；本地租约超过阈值便直接复用。
2. 否则循环调用 `lock_for_write_once`。单次调用先取当前时间，以 `load_row(table_id, true)` 请求独占读取，并计算目标租约 `target`。
3. 已过期行直接改成 `Write`。有效的 `None` 直接变 `Write`；有效的 `Read` 变 `Intend`，保存 `old_read_lease`，远端租约取 `max(target, old_read_lease)`，并返回旧读租约剩余等待时间。
4. `Intend` 仅在 `now > old_read_lease` 后变成 `Write`，否则继续返回等待时间；已有 `Write` 只在 `target` 更大时延长。
5. 公开入口在非零等待期间用 `thread::sleep` 阻塞当前线程，然后重试，直到返回零等待。

读续租 `renew_read_lease_inner` 先读取当前时间和远端行。远端租约在 `now` 时刻已到期（`now >= lease`）或锁型不是 `Read` 时返回 `0`。若调用方旧租约与远端不一致，只在 `now < old_local_lease` 时返回远端租约，使两个不可写时间区间连续；否则返回 `0` 防止 ABA 场景误用旧缓存。匹配时只增不减地更新租约。

写续租 `renew_write_lease_inner` 同样要求未过期的 `Write`；需要时只增不减地更新远端值，但成功后本地 `lease` 按 Go 兼容语义采用调用方传入值，即使该值小于远端当前租约。

## 数据与状态

锁状态机的主要转换是 `None -> Read`、`None -> Write`、`Read -> Intend -> Write`；过期的任意锁还可直接被新 `Read` 或 `Write` 覆盖。有效的 `Write`/`Intend` 阻止新读锁，有效的 `Read` 在转写时必须保留旧租约等待。

时间戳是 `u64` 混合值，高位为物理毫秒、低 18 位为逻辑部分。租约过期判断有意区分严格和非严格边界：获取锁时使用 `now > lease`，续租时使用 `now >= lease`。等待函数在物理毫秒差为正时返回毫秒差；若物理毫秒相同但完整 TSO 仍未越过，则返回 `1µs` 防止零等待忙循环。

`StateRemoteHandle` 的本地字段是缓存而非权威状态。每次 `load_row` 都覆盖它；但部分成功更新刻意保留 Go 的本地快照行为。例如过期行直接写锁后不会立即把本地快照改成 `Write`，读锁更新后本地仍可能是更新前快照，写续租甚至可让本地租约小于不会回退的远端租约。调用者只能依赖各 API 明确允许的快速路径，不能把 `local_state()` 当作远端真值。

`MemoryRemoteStore` 的行按 `table_id: i64` 索引，缺行不会自动创建；只有显式 `insert` 或对已有调用流程的 `update` 才写入。它的 `_for_update` 参数被忽略，因此仅适用于单句柄、外部已串行化或测试场景。

## 依赖与调用关系

直接模块依赖很小：`crate::cache::{StateRemote, lease_from_ts}` 提供上层协议和租约计算；标准库 `HashMap` 提供内存行存储，`fmt` 实现错误展示，`thread`/`Duration` 负责等待。`Cargo.toml` 没有为本文件单独引入外部 crate，也没有控制它的条件 feature；`state_remote` 在 `lib.rs` 中无条件公开。

RustCodeGraph 对目标文件识别出 47 个符号。精确查询定位 `StateRemoteHandle`、`lock_for_read_inner`、`lock_for_write_once`、`renew_read_lease_inner` 和 `renew_write_lease_inner`；调用流显示四个 inner 方法分别由 `StateRemote` 实现中的同名公开协议方法调用，并由 `pkg/table/tables/state_remote_test.rs` 中的边界测试直接覆盖。`lock_for_write` 调用 `lock_for_write_once`；各状态机方法再调用 `RemoteStore::current_ts/load/update`，写锁路径还调用 `lease_from_ts` 与 `wait_for_lease_expire`。

应用侧的拥有者是 `pkg/table/tables/cache.rs`：`CachedTable<R>` 把远端句柄装入 `TokenLimit<R>`，用容量一语义串行化这个非线程安全状态对象。需要注意，RustCodeGraph 的宽名称搜索会把大量同名 `load`/`update` 误归为候选；本文只采用目标文件限定后的符号和直接源码调用边，不把这些同名结果当成真实上游。

## 错误处理与边界

所有存储失败都以 `S::Error` 原样经 `?` 传播；本文件不包装、不记录也不吞掉错误。`MemoryRemoteStore::load` 对缺行返回 `MemoryRemoteStoreError`，而 `update` 当前恒成功并可插入行。生产适配器需要自行定义事务、超时、冲突和 I/O 错误。

`RemoteStore` 的接口存在一项必须由实现者保证的边界：`load(..., true)` 到随后 `update` 的组合必须独占。当前 API 没有事务 guard 或原子 compare-and-swap 类型约束，错误实现会让两个写者同时基于同一旧行推进状态。非独占的读锁与读续租路径也依赖底层存储提供与 Go 乐观事务相容的一致性。

公开写锁调用可能无限重试并阻塞线程：没有取消令牌、超时或最大重试次数；若时钟不推进或旧读租约一直未过，它会持续 sleep/retry。`Duration / 2`、`lease_from_ts` 的饱和计算在 `cache.rs` 中处理极大租期，但本文件的物理毫秒差转换仍假定输入是同一 TSO 编码域。

租约更新遵守“远端绝不减小”的不变量。调用者还必须理解边界差异：`now == lease` 时新锁获取不会把行视为已过期，但续租会失败；这是当前 Rust/Go 对齐的实际条件，不应擅自统一比较符号。

## 并发与资源生命周期

`RemoteStore: Send` 和 `StateRemote: Send` 只允许对象在线程间转移，不代表共享访问安全；所有操作都要求 `&mut self`。`StateRemoteHandle` 自身没有锁，Go 对照也明确声明句柄非线程安全。仓库内 `cache.rs::TokenLimit<R>` 用 `Mutex<Option<R>> + Condvar` 一次只借出一个远端句柄，是上层串行化边界。

`MemoryRemoteStore` 不包含 `Arc`、互斥锁或事务，其 `Clone` 会复制时钟和整个行表，克隆后双方不共享状态。`StateRemoteHandle` 拥有 store，随句柄析构一并释放，没有后台任务、连接关闭或显式清理逻辑。

唯一主动等待发生在 `StateRemote::lock_for_write`：`thread::sleep(wait)` 占住当前 OS 线程。`lock_for_write_once` 把等待与状态转换拆开，便于测试，也为未来异步或可取消调度保留了替换入口；若改为异步，不应在 inner 方法内隐藏 sleep。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/table/tables/state_remote.go`，核心锁型、状态转换、严格/非严格过期条件、租约只增不减、读续租 ABA 检查、写租约本地缓存行为以及 `waitForLeaseExpire` 的最小微秒等待均被 Rust 保留。`pkg/table/tables/state_remote_test.go::TestStateRemote` 覆盖从 `NONE` 到读锁、读续租、读转写、持写时拒绝读操作及写续租；Rust 独立测试把这些行为拆成更确定的状态机测试。

关键实现差异在存储边界。Go 的 `stateRemoteHandle` 持有 `sqlExec`，`runInTxn` 显式开启乐观或悲观事务、关闭 session retry、读取 `@@tidb_current_ts`、提交/回滚；`loadRow` 查询 `mysql.table_cache_meta`，写路径使用 `FOR UPDATE`。Rust 不包含 SQL 执行、上下文、提交/回滚或真正系统表适配器，而把这些责任交给 `RemoteStore`，目前只有忽略 `for_update` 的内存实现。因此 Rust 已实现的是可复用的状态机和存储契约，不能表述为已经具备 Go 的多节点远程事务接线。

Go `CachedTableLockType.String()` 对非法整数 panic；Rust 使用封闭枚举和 `as_str()`，不存在同类非法枚举输入。Go 的 `updateRow` 通常只更新 `lock_type` 与 `lease`，所以 Rust 在若干分支通过 `set_local` 与完整行写回之间的差异，配合测试显式保留远端 `old_read_lease` 和 Go 本地快照语义。

## 扩展指南

接入真实共享存储时，应新增独立模块中的 `RemoteStore` 实现，不要把 SQL/客户端细节塞进状态机。实现必须使用同一事务时间戳兑现 `current_ts`，让 `load(table_id, true)` 与对应 `update` 处于独占事务，并保留缺行、提交失败和回滚失败的可诊断错误。还应增加独立测试文件，覆盖并发写者、事务冲突、缺行、时间戳边界和失败后的资源清理。

修改锁协议时，最可能涉及 `CachedTableLockType`、`LockRow`、`lock_for_read_inner`、`lock_for_write_once`、两个续租 inner 方法及 `wait_for_lease_expire`。必须同步更新 `pkg/table/tables/state_remote_test.rs`，并与 `pkg/table/tables/state_remote.go`、`pkg/table/tables/state_remote_test.go` 的语义逐分支核对；若上层 trait 签名改变，还要同步 `pkg/table/tables/cache.rs::StateRemote` 和 `CachedTable` 调用点。

性能上优先保留两项设计：安全窗口内复用本地写租约，以及只在新租约更大时写远端。兼容性上不得更改四个协议字符串、TSO 的 18 位逻辑布局、`old_read_lease` 的含义或 ABA 返回规则。若要加入取消/异步等待，推荐围绕 `lock_for_write_once` 组织调度，而不是改变其无 sleep、返回等待时长的契约。

测试逻辑应继续放在独立的 `state_remote_test.rs`，不要嵌入生产源文件。新增后端的测试也应独立成文件，并同时验证远端权威行与 `local_state()`，因为两者有意可能不同。

## 验证依据

- 生产源码：`pkg/table/tables/state_remote.rs`，核对 426 行完整实现、公开/内部符号、所有状态分支、错误传播与等待逻辑。
- 上层契约与直接依赖：`pkg/table/tables/cache.rs` 中 `lease_from_ts`、`StateRemote`、`TokenLimit`、`CachedTable`；模块入口 `pkg/table/tables/lib.rs` 中无条件 `pub mod state_remote` 及独立测试模块声明。
- crate 边界：`pkg/table/tables/Cargo.toml`，确认 crate 名 `astersql-table-tables`、入口 `lib.rs`、默认 `expression-runtime` feature；本模块自身只使用 crate 内 API 和标准库。
- Go 对照：`pkg/table/tables/state_remote.go`，核对 `StateRemote`、`stateRemoteHandle`、`LockForRead`、`LockForWrite`/`lockForWriteOnce`、两个续租方法、事务辅助、`loadRow`/`updateRow` 和等待计算。
- Rust 测试：`pkg/table/tables/state_remote_test.rs`，覆盖协议字符串、缺行、过期行读锁、写类锁拒读、读转意向写再转写、写锁只延长、本地/远端快照差异、读续租 ABA、写续租兼容行为、写锁安全窗口复用和 TSO 等待边界。
- Go 测试：`pkg/table/tables/state_remote_test.go::TestStateRemote` 与 `pkg/table/tables/cache_test.go::TestRenewLease`/`TestRenewLeaseABAFailPoint`，作为系统表接线及缓存续租语义的对照证据。
- RustCodeGraph：`status` 显示索引含目标文件（目标文件 47 个符号）；`explore` 和精确 `query --json` 定位 `StateRemoteHandle` 及四个 inner 状态机函数，并核对 trait 公开方法、独立 Rust 测试与 `RemoteStore` 方法之间的直接调用关系。宽名称的同名噪声未作为证据。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构检查要求本文恰有十一个固定二级标题，并人工复核“为何存在、如何运行、如何安全扩展”均有源码或对照文件支撑。
