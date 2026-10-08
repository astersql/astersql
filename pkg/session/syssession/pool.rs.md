# `pkg/session/syssession/pool.rs`

## 文件定位

本文件属于 `astersql-session-syssession` crate（`pkg/session/syssession/Cargo.toml`），实现可复用的系统内部会话池。crate 根 `pkg/session/syssession/lib.rs` 以私有模块 `pool` 装载本文件，再通过 `pub use pool::*` 公开导出 `AdvancedSessionPool`、`Pool`、`CancellationToken`、`Factory`、`PoolMaxSize` 和构造函数。

它位于“具体数据库会话适配器”和各内部 SQL 使用方之间：工厂产生实现 `SessionContext` 的底层上下文，本文件把它包装成 `session.rs` 定义的共享内部会话，借出时返回具有独占所有权的 `Session`，归还时清理状态并缓存。当前生产接线至少包括 `pkg/session/runtime/system_session.rs::SystemResources`（DDL/通用系统会话，空闲容量 5）与 `pkg/session/runtime/ttl_timer_store.rs::new_ttl_timer_session_pool`（TTL timer 专用会话）。

本文件只管理空闲会话和所有权转换，不实现 SQL、事务或注册表本身；这些行为由 `SessionContext` 具体实现以及同目录 `session.rs` 完成。

## 核心职责

- 将合法容量归一化，给每个池分配唯一 `Owner::Pool(id)`，并保存线程安全的会话工厂（`NewAdvancedSessionPool`）。
- 从 FIFO 空闲队列取出内部会话；队列为空时调用工厂创建，并借助 `new_internal_session_for_pool` 建立池所有权（`get_internal`）。
- 借出时把所有权从池转给新 `Session`；归还时先验证所有权，再拒绝未决事务、不可复用、回滚失败、重置失败、满池或已关闭池中的会话（`Get`、`Put`）。
- 为“一次借用”提供异常安全的模板：回调成功才归还，返回错误或 panic 都关闭会话，panic 清理后继续传播（`WithSession`）。
- 在执行需要阻塞 GC 的内部操作前持续尝试注册内部会话，同时允许调用者取消等待（`WithForceBlockGCSession`、`CancellationToken`）。
- 关闭池时原子标记关闭、排空并关闭全部空闲会话；重复关闭无副作用（`Close`、`IsClosed`）。

## 主要符号

- `pub const PoolMaxSize: usize = 1024 * 1024 * 1024`：容量非法时使用的哨兵上限，与 Go 常量数值一致。它是空闲队列上限，不限制同时借出的会话数。
- `pub type Factory = Arc<dyn Fn() -> Result<Box<dyn SessionContext>> + Send + Sync>`：池内保存的类型擦除工厂。公开构造函数接收泛型闭包，再包装为该别名。
- `pub struct CancellationToken(AtomicBool)`：单向取消令牌；`cancel` 以 `Release` 写入，`is_cancelled` 以 `Acquire` 读取，取消后不能恢复。
- `pub trait Pool`：对象安全的池接口，包含 `Get`、`Put`、`WithSession`、`WithForceBlockGCSession`。后两个方法以 `&mut dyn FnMut` 表达动态回调；`AdvancedSessionPool` 的固有方法则接受 `FnOnce`，trait 实现只做转发。
- `static NEXT_POOL_ID: AtomicU64`：从 1 开始以 `Relaxed` 递增，为 `Owner::Pool(u64)` 生成进程内标识；它只要求唯一性，不承载同步协议。
- `pub struct AdvancedSessionPool`：核心实现。`capacity` 是最大空闲数，`sessions: Mutex<VecDeque<SharedInternalSession>>` 是 FIFO 空闲队列，`factory` 创建底层上下文，`closed` 表示池已关闭。
- `NewAdvancedSessionPool(capacity, factory)`：容量小于等于 0 或大于 `PoolMaxSize` 时回退到上限；不会预创建会话。
- `owner()`：把池 ID 映射成当前池专属的 `Owner::Pool`。
- `get_internal()`：关闭时返回 `SessionError("session pool closed")`；否则优先 `pop_front`，缓存为空才调用工厂。
- `Get()`：为内部会话创建新的外层 `Session`，调用 `transfer_owner(pool → session)`；转移失败会以池 owner 尝试关闭内部会话并返回原错误。
- `Put()`：完成 `session → pool` 所有权转移、可复用性检查和有界入队；不返回错误，不可归还的对象由内部关闭逻辑回收。
- `WithSession()`：执行普通一次性回调，封装成功归还/失败销毁策略。
- `WithForceBlockGCSession()`：每 100 ms 重试 `SessionContext::register_internal_session`；注册成功后才调用业务回调。
- `Close()` / `IsClosed()`：分别执行幂等关闭和 acquire 读取。

## 执行流程

1. 上层用 `NewAdvancedSessionPool` 注入 `SessionContext` 工厂。构造只初始化 ID、容量、空队列和关闭标志。
2. `Get` 调用 `get_internal`。若池已关闭立即报错；若队列非空则从队首取得最早归还的内部会话，否则先执行工厂，再由 `new_internal_session_for_pool(context, Owner::Pool(id))` 建立共享状态。
3. `Get` 为该内部对象创建新的 `Session` 包装，随后调用 `transfer_owner`。`session.rs::transfer_owner` 要求当前 owner 精确匹配、`in_use == 0`，并在转向 `Owner::Session` 时调用 `on_became_owner`。成功后，只有新包装可操作底层上下文。
4. `Put` 对空壳 `Session` 直接返回。正常对象先执行 `transfer_owner(session → pool)`；重复归还、已关闭对象、旧包装或仍在使用的对象因 owner/in-use 校验失败而不会入队。
5. 转回池后，`Put` 在内部状态锁下检查 `avoid_reuse`，再锁住 `SessionContext`：先拒绝 `has_pending_transaction()`，然后依次执行 `rollback_transaction()` 和 `reset_state()`。任一条件失败都以池 owner 关闭对象。
6. 清理成功后取得队列锁，并在同一临界区检查 `closed` 与执行入队，以免 `Close` 排空后又被并发 `Put` 塞回会话。池已关闭或队列长度已达 `capacity` 时关闭当前对象，否则 `push_back`。
7. `WithSession` 在 `Get` 后捕获业务回调的 unwind：`Ok(())` 调用 `Put`，普通错误调用 `Session::Close` 后原样返回，panic 也先关闭再 `resume_unwind`。
8. `WithForceBlockGCSession` 使用相同清理模板，但回调前循环：先检查取消，再通过 `Session::WithSessionContext` 调用 `register_internal_session`；返回 `false` 时休眠 100 ms 后重试。注册成功才执行业务回调。
9. `Close` 用 `closed.swap(true, AcqRel)` 选出唯一关闭者，在队列锁下 `drain(..)`，释放队列锁后逐个 `close_internal`。后续 `Get` 被拒绝，后续 `Put` 清理后关闭而非缓存。

## 数据与状态

单个空闲内部会话的典型状态环为：`Owner::Pool(id)`（新建或已归还）→ `Owner::Session(session_id)`（借出）→ `Owner::Pool(id)`（干净归还）。关闭路径把 owner 置为 `Owner::Closed`，此后不能再转移。外层 `Session` 每次借出都会新建，复用的是其内部 `Arc<Mutex<InternalSession>>` 和底层上下文，而不是旧的包装对象。

`sessions` 只存空闲对象，并按 `pop_front` / `push_back` 形成 FIFO。`capacity` 约束的是空闲缓存数；缓存为空时每次 `Get` 都可新建，所以并发借出数可超过容量。队列中对象应满足：owner 是本池、未标记 `avoid_reuse`、没有未决事务，并已成功回滚和重置。该不变量由 `Put` 建立，由 `Get` 的 owner 转移消费。

关闭状态是单调的 `false → true`。`CancellationToken` 同样是单调状态。两者互不关联：关闭池不会自动取消已经借出的会话或 `WithForceBlockGCSession` 的注册循环；后者的终止依赖注册成功、显式取消、上下文错误或 panic。

## 依赖与调用关系

- crate 装配：`pkg/session/syssession/lib.rs` 声明 `mod pool` 并公开再导出。本文件的直接代码依赖只有 Rust 标准库和同 crate 的 `session` 模块；`Cargo.toml` 的普通路径依赖位于永不成立的 `cfg(any())` 下，实际 `SessionContext` 适配器由上层 crate 注入。
- 所有权与关闭下游：`pkg/session/syssession/session.rs::{new_internal_session_for_pool, transfer_owner, close_internal}`。其中 `transfer_owner` 调用 owner 生命周期 hook，`close_internal` 负责注销、关闭上下文并把 owner 标为 `Closed`。
- 通用系统会话上游：`pkg/session/runtime/system_session.rs::SystemResources` 持有 `sys::AdvancedSessionPool`；`ResourcePool::get` 调 `Get`，`put` 调 `Put`，`close` 调 `Close`。`SystemSessionPool::new_with_validator` 以容量 5 创建池，工厂构造线程绑定的 `ConcreteSystemContext`。
- TTL 上游：`pkg/session/runtime/ttl_timer_store.rs::new_ttl_timer_session_pool` 返回 `Arc<AdvancedSessionPool>`，工厂为每个内部会话创建 `TimerSessionContext` 及其专属 worker 线程。
- 测试可观测面：`pkg/session/syssession/session_test_util.rs::AdvancedSessionPool::Size` 读取空闲队列长度；它是测试辅助，不属于本文件的生产 API。
- RustCodeGraph 将 `pool.rs` 识别为 36 个符号，并给出 58 个文件级使用引用；精确方法级 `callers/callees` 因 Go/Rust 同名符号出现歧义，因此生产调用边又通过限定路径搜索和上述相邻源码核验。

## 错误处理与边界

`Get` 传播三类可恢复错误：池已关闭、工厂失败、所有权转移/hook 失败。工厂成功但包装失败时会关闭已经创建的内部会话，避免泄漏。`WithSession` 和 `WithForceBlockGCSession` 不包装这些错误；获取失败时业务回调不会运行。

`Put` 没有返回值。空壳 session 或所有权转移失败只返回；若转移已成功但会话不干净、清理失败、池满或池关闭，则关闭内部对象。尤其是 `has_pending_transaction == true` 时不会尝试回滚或重置，而是直接关闭；无未决事务时仍按顺序调用 rollback 和 reset。重复 `Put` 因 owner 已不再属于旧包装而成为无操作。

回调 panic 与归还清理 panic 被区别处理：两个 `With*` 方法会关闭会话后重新传播业务 panic；`Put` 对可复用性检查使用 `catch_unwind`，若检查、回滚或重置 panic，会先以池 owner 关闭再继续传播。所有 `Mutex::lock` 都使用 `expect`，锁 poison 会 panic，没有恢复策略；若随后执行的关闭 hook 也 panic，最终可见 panic 由底层关闭实现决定。

容量转换使用 `isize`：非正值和超过上限都回退到极大的 `PoolMaxSize`，不是报错。构造没有验证工厂非空的分支，因为 Rust 闭包参数在类型层面必然存在。

`get_internal` 只在入口读取一次 `closed`。因此与 `Close` 并发、且已经越过该检查的 `Get` 可能继续弹出/创建并完成借出；本文件保证的是关闭后新进入的 `Get` 被拒绝、空闲队列被排空，以及关闭临界区之后的 `Put` 不再入队，并未声明 Close 是所有在途借用的全局栅栏。

## 并发与资源生命周期

`AdvancedSessionPool` 的共享状态由两层同步保护：`closed: AtomicBool` 提供快速生命周期判定，`sessions: Mutex<VecDeque<_>>` 串行化 FIFO 操作。`Put` 刻意让“关闭检查 + 入队”与 `Close` 的 drain 使用同一个队列锁，消除先看到未关闭、随后在 drain 之后入队的竞态。`Close` 收集待关对象后释放队列锁再逐个关闭，避免在可能执行用户适配器 hook 时长期占用池锁。

底层会话还有自己的 `InternalSession` mutex 和 `SessionContext` mutex。`Put` 先锁内部状态再锁上下文；所有权转换也沿用这一顺序。对象只有在 owner 精确匹配且 `in_use == 0` 时才能跨池/包装转移，防止仍在执行的上下文被缓存给另一调用者。

业务回调在不持有池队列锁的情况下运行。`WithForceBlockGCSession` 的重试使用当前线程 `sleep(100 ms)`，不是异步定时器；若注册长期失败且未取消，会无限阻塞该线程。取消检查位于每次注册尝试之前，取消发生在一次注册调用或 sleep 中时，要到下一轮才被观察。

池只主动拥有并关闭空闲队列中的资源。借出对象由 `Session` owner 持有，`Close` 不遍历它们；它们随后显式 `Close` 或 `Put` 时才关闭。`AdvancedSessionPool` 本身没有 `Drop` 实现，上层必须显式调用 `Close`（`SystemResources::drop` 通过其 `ResourcePool::close` 做到这一点）。TTL 池的最终释放是否调用 `Close` 取决于其持有方，不可把丢弃 `Arc` 等同于本文件的显式 drain。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/syssession/pool.go`。两版都保留 `PoolMaxSize`、工厂、`Pool` 接口、按需创建、所有权借出/归还、满池丢弃、脏会话拒绝、成功回调归还、失败回调关闭、幂等关闭以及 GC-blocking 注册重试的核心语义。Rust `VecDeque + Mutex` 对应 Go 的有缓冲 channel，Rust `AtomicBool + drain` 对应 Go `mu.closed + close(channel) + range drain`。

所有权模型保持同一意图，但表示不同：Go 以对象身份作为 owner，并含 `noopOwnerHook`；Rust 用唯一数值的 `Owner::Pool/Session` 枚举，并在 `session.rs::transfer_owner` 中只对 Session owner 调用 `on_became_owner/on_resign_owner`，池 owner 天然是 no-op。Rust 在 `Get` 失败时以调用者 owner 有条件关闭，避免误关已经被并发转移给别人的对象。

归还清理存在实现形态差异。Go 调 `CheckNoPendingTxn` 和 `OwnerResetState(p.ctx, p)`，池还保存带 `kv.InternalTxnOthers` 的 context；Rust `SessionContext` 把它拆为 `has_pending_transaction`、`rollback_transaction`、`reset_state`，本池不持有 Go context。文档应以 Rust 当前调用顺序为准，不能假设其适配器拥有 Go context 的取消/内部来源标记。

GC 注册也有差异：Go 先用 `infosync.ContainsInternalSession` 检查，并在测试环境通过 failpoint 允许跳出重试；取消由 `context.Context` 提供。Rust 每轮直接调用抽象的 `register_internal_session`，没有 contains 快路径或 failpoint，取消由本地 `CancellationToken` 提供。因此 Rust 的 `false` 必然意味着继续重试，直至取消或成功。

Go 测试 `pkg/session/syssession/pool_test.go` 覆盖容量、复用、旧 owner/重复 Put、in-use、avoid-reuse、未决事务、panic、满池、关闭池、回调错误/panic 和幂等关闭。Rust 的可执行对照位于独立文件 `pool_test.rs` 后半段与 `session_integration_test.rs`；前半段 `GO_POOL_TEST_DRAFT` 只是保留文本，不是可执行 Rust，不能作为通过证据。

## 扩展指南

- 新增借用策略时优先复用 `Get`/`Put` 或 `WithSession`，不要绕开 `transfer_owner` 直接操作 `sessions`。任何新路径都要保持“一个内部会话同一时刻只有一个 owner”以及失败时关闭的规则。
- 修改可复用判定应集中在 `Put`，并维持未决事务短路、回滚、重置、入队的顺序。若增加新的脏状态，失败路径必须在 owner 已转回池后调用 `close_internal(..., Some(self.owner()))`。
- 调整关闭协议时必须保留 `Put` 的 closed-check 与 enqueue、`Close` 的 drain 使用同一临界区。若需要更强的 Close/Get 线性化语义，应新增明确同步协议并补并发测试，不能只增加第二次原子读取。
- 若把 GC 注册重试改为退避或异步等待，必须保持“注册成功前不执行回调”“取消/错误/panic 时关闭而不入池”和 100 ms 行为兼容评估；同时关注当前阻塞线程模型的调用方预期。
- 若给池增加 `Drop` 自动关闭，需要评估与显式 `Close`、借出会话、适配器 hook panic 及 `Arc` 最后持有者线程的交互；当前 API 不承诺析构时执行 drain。
- Rust 生产逻辑与测试必须继续分文件。优先扩展 `pkg/session/syssession/pool_test.rs` 的可执行测试，并在涉及真实适配器/注册生命周期时同步 `session_integration_test.rs`；与 Go 行为相关的改动还应核对 `pool.go` 和 `pool_test.go`。

兼容风险集中在所有权 hook 次数、错误后是否复用、关闭竞态和 GC 注册取消语义；性能风险集中在超大默认容量、每次归还的回滚/重置、两层 mutex，以及注册失败时固定 100 ms 轮询。新增指标或日志不能在持有队列/内部状态锁时执行可能阻塞的外部工作。

## 验证依据

- RustCodeGraph：`status` 确认本地索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/syssession` 确认目标、Go 对照和独立测试；`node --file pkg/session/syssession/pool.rs` 读取完整 297 行实现；`query AdvancedSessionPool`、`query NewAdvancedSessionPool`、`query WithForceBlockGCSession`、`query transfer_owner` 确认主要符号及位置。精确 `callers/callees` 查询受同名符号歧义影响，未把噪声边当作事实。
- 目标与 crate 边界：`pkg/session/syssession/pool.rs`、`pkg/session/syssession/lib.rs`、`pkg/session/syssession/Cargo.toml`；目标包及上层 `pkg/session` 下未发现需要额外读取的 `doc.go`。
- 所有权与资源下游：`pkg/session/syssession/session.rs::SessionContext`、`new_internal_session_for_pool`、`transfer_owner`、`close_internal`；测试可观测辅助为 `pkg/session/syssession/session_test_util.rs::AdvancedSessionPool::Size`。
- 生产上游：`pkg/session/runtime/system_session.rs::SystemResources::{get,put,close}` 与 `SystemSessionPool::new_with_validator`；`pkg/session/runtime/ttl_timer_store.rs::new_ttl_timer_session_pool`。
- Go 对照：`pkg/session/syssession/pool.go` 与 `pkg/session/syssession/pool_test.go`。
- 独立 Rust 测试：`pkg/session/syssession/pool_test.rs::{advanced_pool_close_is_idempotent_and_rejects_get, advanced_pool_reuses_clean_sessions_and_closes_avoid_reuse_sessions, test_new_session_pool, test_session_pool_with_session, test_session_pool_put_rejects_unclean_or_unstorable_sessions, test_session_pool_with_force_block_gc_session}`；`pkg/session/syssession/session_integration_test.rs::{force_block_gc_registers_then_returns_clean_session_to_pool, public_session_ownership_has_one_registry_lifecycle, test_domain_advanced_session_pool_put_back_dirty_session}`。
- 按任务约束未运行 Cargo。验证只执行固定十一章节结构检查，并人工复核“为何存在、如何运行、如何安全扩展”均可由上述源码与测试定位回答。
