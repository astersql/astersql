# `pkg/ddl/session/session_pool.rs`

## 文件定位

本文件说明的真实源文件是 [`session_pool.rs`](./session_pool.rs)。它属于 workspace 成员 `astersql-ddl-session`（[`Cargo.toml`](./Cargo.toml)），由模块入口 [`lib.rs`](./lib.rs) 以 `pub mod session_pool` 声明并整体再导出。它位于 DDL 后台 SQL 执行所需的会话抽象之上：相邻文件 [`session.rs`](./session.rs) 定义 `SessionContext`、`SessionVariables`、`ExecutionContext` 和 `SessionError`，本文件则把任意实现了 `ResourcePool` 的底层池包装成 DDL 内部会话池 `Pool`。

根 `Cargo.toml` 将该 crate 纳入 workspace，并以 `facade_ddl_session` 登记；`pkg/ddl/Cargo.toml`、`pkg/ddl/jobsubmit/Cargo.toml`、`pkg/ddl/ingest/Cargo.toml`、`pkg/ddl/notifier/Cargo.toml`、`pkg/session/Cargo.toml` 和 `pkg/executor/Cargo.toml` 声明了路径依赖。不过，对 Rust 生产源码的精确检索没有发现 `new_session_pool`、本文件 `Pool` 或 `ResourcePool` 的实际调用，当前可核实的直接使用者仅为 `pkg/ddl/session/session_pool_test.rs`。因此它是已经形成独立 API 和测试的 Go 语义移植边界，但不能据现有证据声称已经接入 Rust DDL 运行主链。

## 核心职责

`Pool` 的职责不是创建具体数据库会话，而是在底层 `ResourcePool` 之上维护以下协议：

1. `Pool::get` 拒绝已关闭池，检查资源确实是 `SessionContext`，并为 DDL 后台使用设置自动提交、受限 SQL、语句时区和磁盘接近满时的写入策略。
2. 借出后把 `session_id` 登记到全局 `INTERNAL_SESSIONS`；`put` 或 `destroy` 成功进入回收流程时移除登记。
3. `Pool::put` 与 `Pool::destroy` 都要求会话不再携带有效事务，然后执行防御性回滚并清除临时磁盘策略，避免状态污染下一次复用。
4. `Pool::destroy` 根据底层池能力，在显式销毁、关闭会话并归还空槽、或退回资源后报告不支持三种策略间分派。
5. `Pool::close` 串行且幂等地关闭底层池；`is_closed` 暴露包装层状态。

本文件不执行 DDL job、schema state transition、reorg/backfill 或元数据持久化；它只是这些潜在上层消费者可使用的内部会话生命周期边界。

## 主要符号

- `Resource`：底层 `get` 的判别联合。`Session(Arc<dyn SessionContext>)` 是唯一可借出的正确资源；`Other(String)` 保存实际类型名，用于产生 `SessionError::InvalidResource`。
- `ResourcePoolKind`：销毁能力枚举。`Destroyable` 支持 `ResourcePool::destroy`；`SlotCounting` 要求每次获取都归还槽位；`Other` 表示未知能力。
- `ResourcePool`：线程安全的底层池 trait，要求实现 `get`、`put`、`close`；`destroy` 默认为空操作，`kind` 默认为 `Other`，`type_name` 默认返回实现类型。实现者必须让 `kind` 与实际回收能力一致，否则可能静默不销毁或走不支持分支。
- `INTERNAL_SESSIONS`：`LazyLock<RwLock<HashSet<u64>>>` 全局集合，保存当前由此模块借出且尚未归还/销毁的会话 ID。
- `internal_session_ids()`：取得读锁并返回 ID 快照。返回顺序未定义，因为底层是 `HashSet`。
- `store_internal_session` / `delete_internal_session`：私有登记辅助函数，分别由 `get` 和回收路径调用。
- `PoolState { closed }`：受 `Mutex` 保护的包装层关闭状态。
- `Pool { state, resource_pool }`：公开包装类型；底层池以 `Arc<dyn ResourcePool>` 共享。
- `Pool::new` / `new_session_pool`：等价构造入口；后者与 Go 的 `NewSessionPool` 命名对应。
- `Pool::get`：借出并初始化内部会话。
- `Pool::validate_idle`：调用 `SessionContext::transaction(false)`；只要返回有效事务，就生成 `SessionError::Transaction`。
- `Pool::put` / `Pool::destroy`：普通归还与不可复用资源的销毁入口。
- `Pool::close` / `Pool::is_closed`：幂等关闭与状态查询。

文件没有条件编译项、泛型类型或模块级数值常量。

## 执行流程

### 借出

`Pool::get` 先短暂持有 `state` 互斥锁检查 `closed`，已关闭时立即返回 `SessionError::PoolClosed`。随后调用 `ResourcePool::get` 并原样传播底层错误。若结果为 `Resource::Other`，返回带类型信息的 `InvalidResource`；若为会话，则依次调用 `set_autocommit(true)`、`set_restricted_sql(true)`、`set_statement_time_zone_from_location()`、`set_disk_full_allowed_on_almost_full()`，最后登记会话 ID 并返回共享会话。

关闭检查与底层获取不是同一个原子操作：释放 `state` 锁后，另一个线程可以调用 `close`。底层 `ResourcePool` 必须自行定义并发 `get`/`close` 的语义，本包装层只保证在观察到 `closed == true` 后不再发起新获取。

### 普通归还

`Pool::put` 首先执行 `validate_idle`。如果 `transaction(false)` 本身失败，或发现 `Transaction { valid: true }`，函数立即返回错误；此时不会回滚、不会清除选项、不会归还底层池，也不会删除内部会话登记。验证通过后，它调用 `rollback_transaction` 作为清理措施，清除 disk-full 选项，先把 `Some(context)` 归还底层池，再从 `INTERNAL_SESSIONS` 删除 ID。先归还后删登记的顺序是刻意的：槽位计数池的 `close` 可能等待全部已借出资源归还。

### 销毁

`Pool::destroy` 同样先验证不存在有效事务，再回滚、清理 disk-full 选项并删除内部会话登记。随后按 `ResourcePool::kind()` 分支：

- `Destroyable`：调用底层 `destroy(context)`，不调用 `put`。
- `SlotCounting`：先调用 `SessionContext::close()` 关闭具体会话，再调用 `put(None)` 只归还容量槽；下一次获取应创建新会话。
- `Other`：把原会话放回池以避免槽位泄漏，然后返回 `SessionError::UnsupportedPool(type_name)`。此分支报告失败，但资源已经被归还且内部登记已经移除，调用者不能重试销毁同一所有权值。

### 关闭

`Pool::close` 在持有 `state` 互斥锁期间检查幂等标记、调用底层 `close`，最后设置 `closed = true`。trait 的 `close` 不返回错误，因此包装层没有关闭失败状态。`put`/`destroy` 不获取 `state` 锁，允许已借出的资源在关闭期间继续走回收协议。

## 数据与状态

状态分为三层：

- 包装层关闭状态：`PoolState::closed` 由每个 `Pool` 自己的 `Mutex` 保护，初始为 `false`。
- 底层池状态：完全由 `ResourcePool` 实现管理，本文件只通过 trait 方法交互。
- 进程级内部会话登记：`INTERNAL_SESSIONS` 是所有 `Pool` 实例共享的 ID 集合，而不是按池分组。重复 ID 会被 `HashSet` 合并，所以 `SessionContext::session_id()` 应在所有同时借出的内部会话间保持唯一；否则一次删除可能掩盖另一个仍在使用的会话。

`get` 写入且 `put`/`destroy` 删除登记，构成正常路径的不变量。错误路径并不完全对称：底层 `get` 失败或资源类型错误时尚未登记；`validate_idle` 失败时保留登记和借出所有权，要求调用者先结束事务后重试回收。`SessionVariables` 中自动提交与受限 SQL 在归还时不会复原，而 disk-full 选项会清除；这意味着池内资源的基线状态依赖 DDL 专用会话约定。

## 依赖与调用关系

文件只使用标准库与同 crate 类型，没有第三方 crate 依赖；`pkg/ddl/session/Cargo.toml` 的 `[dependencies]` 为空。

已由 RustCodeGraph 核实的文件内调用边包括：

- `Pool::get → ResourcePool::get`、`Pool::get → store_internal_session`。
- `Pool::put → Pool::validate_idle`、`ResourcePool::put`、`delete_internal_session`。
- `Pool::destroy → Pool::validate_idle`、`ResourcePool::kind`，并按分支调用 `ResourcePool::destroy`、`SessionContext::close`、`ResourcePool::put` 或 `ResourcePool::type_name`。
- `Pool::close → ResourcePool::close`。
- `new_session_pool → Pool::new`。

相邻 `session.rs` 提供所有下游会话操作：`SessionContext::session_variables`、`transaction`、`rollback_transaction`、`close`，以及相应的 `SessionError` 变体。模块入口 `lib.rs` 将这些类型和本文件符号统一再导出。

RustCodeGraph 的精确符号查询识别了 `internal_session_ids`、`validate_idle` 和 `new_session_pool`，但 callers 查询没有返回可用的跨文件调用者；随后对 Rust 生产源码进行精确文本复核，也未找到实际构造或调用。`session_pool_test.rs` 是当前唯一直接调用面，覆盖构造、借出、归还和两类销毁策略。

## 错误处理与边界

- `Pool::get` 可返回底层 `ResourcePool::get` 的 `SessionError`、`PoolClosed` 或 `InvalidResource`。对 `Resource::Other` 不执行归还；实现者若把需要回收的实体编码为 `Other`，必须自行避免资源泄漏。
- `validate_idle` 使用 `transaction(false)?`，既不会为了检查而激活新事务，也会传播事务查询错误。有效事务被视为调用协议错误，而不是由 `put`/`destroy` 自动回滚后接受。
- `rollback_transaction`、变量设置、`ResourcePool::put/destroy/close` 和 `SessionContext::close` 的 trait 签名均无返回值，相关失败无法由本层表达。
- `ResourcePoolKind::Other` 的销毁会先归还资源再返回 `UnsupportedPool`，保证容量优先于“真正销毁”的语义；与 Go 版本记录 warning 并在 `intest` 断言失败相比，Rust 用可返回错误替代日志与测试态断言。
- 所有标准库锁都通过 `unwrap()` 获取；若持锁线程 panic 导致 poison，后续访问会继续 panic，而不是转换成 `SessionError`。
- `close` 在调用底层实现后才置位。如果底层 `close` panic，包装层不会标记为已关闭。
- 当前 Rust 测试未直接覆盖 `PoolClosed`、`InvalidResource`、`UnsupportedPool`、有效事务拒绝、重复 `close`、锁 poison 或 `get`/`close` 竞态；这些行为来自源码检查，不能误写成已有回归覆盖。

## 并发与资源生命周期

`ResourcePool: Send + Sync`、`Arc<dyn ResourcePool>` 和 `Arc<dyn SessionContext>` 允许跨线程共享。`state: Mutex<PoolState>` 串行化关闭操作；全局会话 ID 集合使用 `RwLock`，允许并发读取快照并串行增删。会话变量在 `session.rs` 内通过原子变量和互斥锁维护。

正常生命周期为“底层池创建/持有资源 → `get` 借出并登记 → 上层执行和结束事务 → `put` 归还并注销”。不可复用资源走 `destroy`：可销毁池直接消费会话；槽位池关闭会话并归还空槽；未知池退回会话并报错。`Pool` 没有实现 `Drop`，遗漏 `put`/`destroy` 不会自动归还资源或删除登记，因此调用者必须显式配对。

`session_pool_test.rs::test_pessimistic_txn` 用两个会话和共享 `Condvar` 证明会话可跨线程承载悲观事务模拟：第二个更新在第一个事务提交释放锁前保持阻塞。不过这个测试验证的是会话与池共同使用的并发场景，不等于验证了 `Pool::get` 与 `Pool::close` 的竞态。

关闭时仍允许 `put`，这是槽位计数池能够等待 outstanding gets 归还的关键条件。相反，`close` 持有包装层状态锁调用底层 `close`；底层实现若等待归还，不会阻塞本文件的 `put`，因为 `put` 不获取该状态锁。

## 与 Go 版本的对应关系

直接对照文件为 [`session_pool.go`](./session_pool.go)，测试对照为 [`session_pool_test.go`](./session_pool_test.go) 与 [`session_pool_test.rs`](./session_pool_test.rs)。

- Go `Pool` 的 `mu.closed + resPool util.SessionPool` 对应 Rust `PoolState::closed + Arc<dyn ResourcePool>`。
- Go `NewSessionPool` 对应 `new_session_pool`/`Pool::new`。Go 用 `intest.AssertNotNil` 拒绝 nil；Rust 的 `Arc<dyn ResourcePool>` 类型本身没有 nil 值。
- Go `Get` 的类型断言、自动提交、restricted SQL、statement timezone、disk-full option 和 `infosync.StoreInternalSession`，分别对应 Rust `Resource` 判别、四个 `SessionVariables` 设置方法和本地 `store_internal_session`。Rust 登记的是 session ID；Go 的 infosync 路径最终面向内部会话可观测信息，二者存储模型并非同一实现。
- Go `Put` 在 `intest` 构建中断言没有有效事务，然后回滚、清选项、归还和注销；Rust `put` 把该不变量提升为所有构建均会返回的 `SessionError::Transaction`，并且会传播 `transaction(false)` 错误。
- Go `Destroy` 通过运行时类型断言区分 `util.DestroyableSessionPool` 与 `*pools.ResourcePool`；Rust 通过显式 `ResourcePoolKind` 分派。Go 未知类型会 warning、归还后触发 `intest.Assert(false)`；Rust 归还后返回 `UnsupportedPool`，但不记录日志。
- Go `Close` 记录日志并幂等关闭；Rust 保留幂等和锁范围，没有日志。
- 两边测试都覆盖基础借还、内部会话可见性、悲观事务阻塞、槽位池销毁后创建新资源、以及 Destroyable 池调用 destroy 而不 put。Rust 测试用 `MockSessionContext`/`CountingPool` 替代 Go 的 mock store、testkit 与真实 `pools.ResourcePool`，所以只能证明抽象协议，不证明与实际 Rust 数据库会话适配器的集成。

## 扩展指南

新增底层池实现时，应实现 `ResourcePool` 并特别核对 `kind`：支持真正丢弃资源的实现返回 `Destroyable`；要求 Get/Put 严格计数且接受空槽归还的实现返回 `SlotCounting`；若保留默认 `Other`，调用 `destroy` 会归还资源并向上报错。建议在独立的 `pkg/ddl/session/session_pool_test.rs` 增加该实现的 get/put/destroy/close 计数与资源关闭断言，不要把测试嵌入生产文件。

若扩展借出初始化，修改点是 `Pool::get` 中取得 `SessionVariables` 后的设置序列，并应增加“借出时设置、归还或再次借出时无状态泄漏”的测试。若新增需要复原的临时变量，普通归还和所有销毁分支都必须同步清理；还需与 Go `Pool.Get/Put/Destroy` 比较，避免两种语言的 DDL 后台会话语义漂移。

若改变事务归还策略，必须先决定是继续拒绝有效事务，还是自动回滚后接受；这会改变调用者发现生命周期错误的能力。测试至少应覆盖 `transaction(false)` 返回错误、有效事务拒绝后登记仍保留、结束事务后可重试回收。

若要把该池接入 Rust 生产主链，应先实现真实 `SessionContext` 和 `ResourcePool` 适配器，再在拥有会话工厂与关闭顺序的组装层构造 `Pool`。接线验证应覆盖 DDL 内部 SQL、关闭期间 outstanding session 归还、内部会话可观测性以及销毁后不复用；仅在 Cargo manifest 添加依赖不足以证明接入完成。

涉及并发语义时，应补充 `get`/`close` 交错、并发重复 `close`、并发读 `internal_session_ids` 和重复 ID 的测试。任何可能 panic 的真实底层 `close` 或锁 poison 策略也应明确，而不是依赖当前 `unwrap()` 的隐式行为。

## 验证依据

- RustCodeGraph 状态：索引覆盖 11,467 个文件，其中 Rust 7,032 个；目标文件完整索引为 209 行。
- RustCodeGraph 文件查询：`files --filter pkg/ddl/session` 确认 Rust/Go 源与对应独立测试文件；`node --file pkg/ddl/session/session_pool.rs --offset 1 --limit 500` 读取全部目标源码。
- RustCodeGraph 符号查询：确认 `internal_session_ids`（第 70 行）、`validate_idle`（第 138 行）、`new_session_pool`（第 207 行）；图输出核实了 `get`、`put`、`destroy`、`close` 的上述文件内调用边。跨文件 callers 因同名符号消歧未产生可靠结果，因此没有据此声称生产接线。
- 读取的 crate 与相邻定义：`pkg/ddl/session/Cargo.toml`、`pkg/ddl/session/lib.rs`、`pkg/ddl/session/session.rs`；根 `Cargo.toml` 与相关子 crate Cargo manifests 用于核对 workspace 和依赖声明。
- Go 对照：`pkg/ddl/session/session_pool.go`；Go 测试：`pkg/ddl/session/session_pool_test.go`。
- Rust 独立测试：`pkg/ddl/session/session_pool_test.rs`，包含 `test_session_pool`、`test_pessimistic_txn`、`test_session_pool_destroy_resource_pool`、`test_session_pool_destroy_destroyable_session_pool`。
- 精确源码检索：生产 Rust 文件未发现 `new_session_pool` 或本文件池类型的直接使用，只有目标文件及其测试；据此将当前运行主链接线标为未出现，而非推定已支持。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构校验要求本文恰有 11 个固定二级标题。
