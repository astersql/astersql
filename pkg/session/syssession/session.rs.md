# `pkg/session/syssession/session.rs` 逻辑说明

## 文件定位

`session.rs` 是 `astersql-session-syssession` crate 的会话包装层，由 `pkg/session/syssession/lib.rs` 以 `mod session; pub use session::*;` 对 crate 内外导出。它不负责 SQL 解析、优化或存储访问；它定义底层会话必须实现的 `SessionContext` 契约，并在其上提供独占 owner 校验、操作计数、并发误用检测、关闭时序和 SQL 代理。

crate 边界由 `pkg/session/syssession/Cargo.toml` 确认：库入口是 `lib.rs`，移植元数据指向 Go 包 `pkg/session/syssession`。Cargo 中原 Go 依赖的 Rust crate 声明位于 `target.'cfg(any())'.dependencies`，该 cfg 恒为 false；本文件当前实际只直接依赖 Rust 标准库。完整应用中的具体对接位于 `pkg/session/runtime/system_session.rs`，其 `ConcreteSystemContext` 实现 `SessionContext`，`SystemSessionPool` 内持有 `AdvancedSessionPool`。

## 核心职责

1. 以 `SessionContext` 隔离具体数据库会话，使池和调用者只依赖关闭、owner hook、事务清理、注册以及六类 SQL 操作契约。
2. 以 `Owner::{Pool, Session, Closed}` 建模会话独占所有权，由 `transfer_owner` 保证池和外层 `Session` 之间的交接有来源校验且不与进行中操作重叠。
3. 由 `Session::with_context` 为所有对外代理方法提供统一的进入/退出协议，在 panic 时仍恢复计数并标记 `avoid_reuse`。
4. 由 `close_internal`/`close_context` 保证注销、owner 辞任和底层 close 的生命周期，包括有进行中操作时的延迟关闭。
5. 由 `ThreadBoundSession<T>` 把非 `Send` 值的创建、使用、清理和析构固定到同一工作线程，只允许 `Send` 的操作闭包与返回值跨线程。

## 主要符号

- `RecordSet` / `Statement` / `SqlValue` / `Row`：基于 `Any` 的类型擦除 ABI。结果集、语句和行要求 `Send`，参数额外要求 `Sync`；具体适配器负责 downcast 与语义校验。
- `SessionError` 与 `Result<T>`：仅保存规范化字符串的轻量错误边界，实现 `Display` 和 `Error`。
- `SessionContext`：`Send` trait；除 SQL 代理外，还包含 `on_became_owner`/`on_resign_owner`、`has_pending_transaction`、`rollback_transaction`、`reset_state`、内部会话注册/注销和可选 `as_any_mut`。
- `Owner`：crate 内枚举。`Pool(u64)` 和 `Session(u64)` 的 ID 分别由池和会话全局原子计数器分配，`Closed` 对应 Go 实现中的 nil owner。
- `InternalSession`：在 `SharedInternalSession = Arc<Mutex<_>>` 内共享的核心状态，字段为 `context`、`owner`、`sequence`、`in_use`、`unsafe_count` 和 `avoid_reuse`。
- `new_internal_session` / `new_internal_session_for_pool` / `new_internal_session_impl`：创建内部会话。只有初始 owner 为 `Session` 时才调用 became-owner hook；池专用入口在 hook 失败时关闭已创建 context。
- `transfer_owner`：校验当前 owner、目标非 `Closed`、`in_use == 0`；完成 Session owner 的 resign/became hook，hook 返错或 panic 时将状态设为 `Closed` 并关闭 context。
- `close_internal` / `close_context` / `close_context_only`：关闭原语。`close_context` 按需调用 resign hook，之后始终尝试 unregister 和 close，最后按 close、unregister、resign 的优先级重抛捕获的 panic。
- `Session`：公开代理句柄，通过 `Close`、`IsOwner`、`AvoidReuse`、`WithSessionContext`、`Execute`、`ExecuteInternal`、`ExecuteStmt`、`ParseWithParams`、`ExecRestrictedStmt` 和 `ExecRestrictedSQL` 暴露受控访问。`Default` 生成没有内部会话的已关闭空壳。
- `ThreadBoundSession<T>`：内含可取走的 `ThreadBoundWorker`；`new`启动工作线程，`call`同步往返一次操作，`close` 丢弃 sender 后 join，`Drop` 再次保底关闭。

## 执行流程

池化会话的主链是 `AdvancedSessionPool::Get` → `new_internal_session_for_pool`（无空闲项时）→ `Session::from_internal` → `transfer_owner(Pool, Session)`。转移中会为新 `Session` 调用 `SessionContext::on_became_owner`；成功后调用者才获得可用句柄。该直接调用边由 `pkg/session/syssession/pool.rs` 中的 `get_internal`、`Get` 和 `Put` 确认。

代理方法统一进入 `Session::with_context(false, ...)`。它先拒绝空壳、`Closed` 或 owner ID 不匹配的会话；然后增加 `unsafe_count` 并拒绝第二个线程不安全操作，再增加 `in_use`。它释放 `InternalSession` 锁后才锁定 context 执行闭包，避免在内部状态锁下调用适配器。退出阶段无论闭包返回还是 panic 都递减 `in_use`、复位 `unsafe_count`；panic 另会设置 `avoid_reuse`并在清理后原样重抛。

归还链是 `AdvancedSessionPool::Put` → `transfer_owner(Session, Pool)` → 检查 `avoid_reuse`/未决事务→ rollback → reset→入队或 `close_internal`。关闭时若 `in_use == 0`，立即执行 resign/unregister/close；若尚有操作，先把 owner 标记为 `Closed`，最后一个 `with_context` 退出时才关闭底层 context。

`ThreadBoundSession::new` 创建命名为 `system-session` 的线程：工厂在该线程构造 `T`，用一次性 ready 通道向创建者报告成功/失败，然后循环处理闭包。`call` 使用回复通道等待 `Result<R>`；闭包 panic 被转为 `system session operation panicked`，同时返回 `false` 终止 worker，随后执行 cleanup 并析构 `T`。

## 数据与状态

`InternalSession` 有两层同步：外层 `Mutex<InternalSession>` 保护 owner 和计数，内层 `Mutex<Box<dyn SessionContext>>` 保护具体会话。`Arc` 使池、`Session` 包装和进行中操作可持有同一实例，但 owner 枚举而非 `Arc` 强引用决定谁有权调用。

`sequence` 每次 owner 转移增加，当前仅保留为诊断状态，不参与正确性判定。`in_use` 记录已成功进入 context 的操作；`unsafe_count` 在第二个线程不安全操作被拒绝时保留为 2，由首个操作退出时归零。`avoid_reuse` 是粘性标志：`AvoidReuse` 或代理闭包 panic 可将其设为 true，本文件不会清除它，池会据此关闭而非复用会话。

`NEXT_SESSION_ID` 使用 `AtomicU64` 与 `Relaxed` 顺序产生身份 ID；ID 只用于区分 owner，不承担状态发布。`ThreadBoundSession` 的 `worker: Mutex<Option<_>>` 使 close 与 call 的接受阶段串行化，`Option::take` 实现幂等关闭。

## 依赖与调用关系

- 上游直接调用：`pkg/session/syssession/pool.rs` 调用 `new_internal_session_for_pool`、`Session::from_internal`、`transfer_owner` 和 `close_internal`，完成池的 Get/Put/Close。
- 具体运行实现：`pkg/session/runtime/system_session.rs::ConcreteSystemContext` 实现全部 `SessionContext` 方法，其 `worker` 是 `ThreadBoundSession<ConcreteSession>`；`SystemSessionPool::new` 通过 `NewAdvancedSessionPool` 的工厂构造该适配器。
- 另一实际适配：`pkg/session/runtime/ttl_timer_store.rs::TimerSessionContext` 实现 `SessionContext`，并由 `new_ttl_timer_session_pool` 构造 `AdvancedSessionPool`。
- 消费者示例：`pkg/timer/tablestore/store.rs` 接收 `syssession::Session` 执行定时器表 SQL；`pkg/executor/internal/exec/executor.rs` 从 system pool Get 会话并在操作后 Put。
- 下游仅是标准库：`Any`、`Arc`、`Mutex`、`AtomicU64`、`mpsc`、`JoinHandle` 与 panic 捕获 API。本文件不直接引用 Cargo 中 `cfg(any())` 下的兼容依赖。

RustCodeGraph 将 `session.rs` 标记为被 67 个文件使用，但这包含重导出和跨语言符号关联；本文档对主链的结论限定在上述可直接定位的 Rust 调用点，不把搜索命中数当成运行时调用次数。

## 错误处理与边界

owner 不匹配、已关闭、传入 `Closed` 目标、仍在使用中、并发的线程不安全操作以及 worker 停止都返回 `SessionError`。底层 `SessionContext` 的普通 `Result` 错误原样透传；类型擦除后的类型错误由具体适配器负责报告。

`transfer_owner` 的重要失败不变式是：resign 或 became hook 一旦返错/panic，该内部会话不再恢复到任一 owner，而是标记 `Closed` 并关闭。这避免 hook 只完成一半后继续复用未知状态。`new_internal_session` 与池专用变体有一个刻意差异：前者在 became hook 失败时返错但不关闭 context，后者会关闭，因为池已接管工厂产物的清理责任。

panic 边界分两类：`Session::with_context` 捕获业务操作 panic，做完计数/关闭清理后重抛；`ThreadBoundSession::call` 则将跨线程的操作 panic 转为错误并停止 worker，因为 panic payload 不跨该通道重抛。标准 `Mutex` poison 在大多数内部路径上通过 `expect` 升级为 panic；`ThreadBoundSession::call` 获取 worker 锁时例外地转换为 `SessionError`。

## 并发与资源生命周期

`SessionContext: Send` 表示 context 容器可被移动，但不表示其 SQL 方法可并发使用。所有公开 SQL 代理当前都以 `thread_safe = false` 进入，所以第二个重叠操作会在进入 context 前失败。context 自身的 mutex 还会串行化实际调用；`unsafe_count` 则把本可被锁顺序执行的误用提升为显式错误。

关闭与在途操作不争用 context：`close_internal` 在 `in_use > 0` 时只封闭 owner，使新操作无法进入；已接受操作保留 context 的 `Arc`，并在退出时完成唯一一次底层关闭。`Close` 对重复调用幂等，caller 参数还可防止旧 owner 误关闭已转移的会话。

`ThreadBoundSession` 为每个值保有一条专属线程和无界 mpsc 请求队列。`call` 在持有 worker mutex 时等待回复，因此 close 不会与已接受请求的清理并发；close 取走 worker、断开 sender，worker 处理完已接受请求后退出循环，在所属线程调用 cleanup、drop `T`，最后 join。`pkg/session/syssession/thread_bound_test.rs` 验证创建/调用/析构的线程一致性、工厂失败、操作 panic 后清理，以及 close 等待已接受操作。

## 与 Go 版本的对应关系

Go 权威对照是 `pkg/session/syssession/session.go`。Rust `InternalSession` 对应 Go 私有 `session`，`Session` 对应 Go 公开代理，`owner`/`in_use`/`unsafe_count`/`avoid_reuse` 对应 `owner`/`inUse`/`unsafe`/`avoidReuse`。Rust 用带 ID 的 `Owner` 值替代 Go `sessionOwner` 接口实例，用 `Closed` 替代 nil，以便在不保留 owner 对象引用的前提下执行身份校验。

Go `EnterOperation` 返回 context 和 exit 闭包；Rust 将它收敛到 `with_context`，利用 `catch_unwind` 保证退出逻辑。两者都在重叠线程不安全操作时拒绝后来者，在已关闭但仍 `inUse > 0` 时延迟 context close，并在代理操作 panic 后禁止复用。Rust 的 `sequence` 只在 owner 转移时增加，不像 Go `seq` 同时记录操作进出并用于日志；Rust 也没有 Go `reportErrorWithoutLock` 的 zap/测试断言路径，而是把进入失败直接返给调用者。

Go `SessionContext` 嵌入完整 `sessionctx.Context`，SQL 代理通过 `GetSQLExecutor`/`GetRestrictedSQLExecutor` 调用真实 TiDB 类型。Rust 当前用独立 trait 和 `Any` 擦除边界，具体 SQL 参数、statement 与 row 语义由适配器实现。Go `Session::onBecameOwner`/`onResignOwner` 直接调用 infosync 登记表；Rust 的公开 `Session` 没有自身 hook，hook 在 `SessionContext` 具体实现中完成。`ThreadBoundSession` 是 Rust 为容纳非 `Send` `ConcreteSession` 增加的适配层，Go 同文件没有对应类型。

## 扩展指南

- 新增会话操作时，先在 `SessionContext` 增加最小契约，再在 `Session` 增加通过 `with_context` 的代理；不应绕过 owner、`in_use` 和 panic 清理。同步修改 `pkg/session/runtime/system_session.rs::ConcreteSystemContext`、`pkg/session/runtime/ttl_timer_store.rs::TimerSessionContext` 及所有测试 context 实现。
- 只有经过证明可并发访问的操作才应使用 `thread_safe = true`；否则保持当前公开代理全部为 false 的契约，并在 `session_test.rs` 增加重叠调用回归。
- 修改 owner 转移时，必须保留“校验 from→拒绝 Closed 目标→拒绝 in-use→resign→became→失败则彻底关闭”的顺序，并更新 `session_test.rs` 的错误、panic、同 owner、已关闭和在途操作用例以及 `pool_test.rs` 的 Get/Put 用例。
- 修改关闭逻辑时，需同时考虑幂等、错 owner、进行中操作、hook 返错/panic、unregister/close panic 的顺序，并保持“已接受操作先结束”。主要回归文件是 `session_test.rs`、`session_integration_test.rs` 和 `pool_test.rs`。
- 扩展 `ThreadBoundSession` 时不得让 `&mut T` 或非 `Send` 结果逃离 worker；需在 `thread_bound_test.rs` 继续验证线程亲和性、启动失败、panic 后 worker 停止、close/call 排序和 drop 时机。每个实例一条 OS 线程，扩大池容量或增加实例时必须评估线程/通道开销。
- 变更类型擦除 ABI 时，须同步检查具体适配器的 downcast 和错误文本，并保持 Go 版本 SQLExecutor/RestrictedSQLExecutor 的参数、多结果集与行返回语义；不能以类型擦除为理由简化实际行为。

## 验证依据

- 源文件：`pkg/session/syssession/session.rs` 全部 568 行；符号和流程重点为 `SessionContext`、`InternalSession`、`new_internal_session_impl`、`transfer_owner`、`close_internal`、`Session::with_context` 和 `ThreadBoundSession::{new, call, close}`。
- RustCodeGraph：`status` 确认索引可用；`files --filter pkg/session/syssession` 列出 17 个 Rust/Go 相关文件；`explore "SysSession session.rs pkg/session/syssession"` 返回 owner、池、关闭和运行会话相关候选；`node --file pkg/session/syssession/session.rs --offset 1 --limit 500` 及 `--offset 501 --limit 100` 验证了全文与被使用关系。独立 `callers transfer_owner` 查询未在 30 秒内返回，因此用索引已定位的 `pool.rs` 和精确文本搜索补齐直接调用边，未据此推测其他调用。
- crate 与模块：`pkg/session/syssession/Cargo.toml`、`pkg/session/syssession/lib.rs`、`pkg/session/syssession/pool.rs`。运行接线：`pkg/session/runtime/system_session.rs`、`pkg/session/runtime/ttl_timer_store.rs`、`pkg/timer/tablestore/store.rs`、`pkg/executor/internal/exec/executor.rs`。
- Go 对照：`pkg/session/syssession/session.go` 中 `session`、`newInternalSession`、`TransferOwner`、`EnterOperation`、`doCloseWithoutLock` 和公开 `Session` 代理方法。
- Rust 测试：`pkg/session/syssession/session_test.rs` 验证错误文本、owner hook/转移、延迟关闭、非 owner 拒绝、panic 清理、未决事务、reset、禁复用和线程不安全竞争；`session_integration_test.rs` 验证空壳会话、内部注册生命周期和脏会话归还；`pool_test.rs` 验证池复用/拒收与工厂失败清理；`thread_bound_test.rs` 验证非 `Send` 值的线程生命周期。Go 回归对照在 `pkg/session/syssession/session_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；以固定 11 个二级标题的结构命令与人工事实复核作为交付验证。
