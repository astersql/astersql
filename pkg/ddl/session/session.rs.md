# `pkg/ddl/session/session.rs`

## 文件定位

本文件是独立 crate `astersql-ddl-session` 的核心会话抽象。crate 入口 `pkg/ddl/session/lib.rs` 将本模块与 `session_pool` 模块公开并再导出二者的公共符号；`pkg/ddl/session/Cargo.toml` 指定 `lib.rs` 为入口、声明 Go 对照包 `pkg/ddl/session`，且自身没有第三方依赖。它位于普通 SQL 会话与 DDL 后台逻辑之间：上层拿到 `Session` 后使用统一的事务和内部 SQL API，具体数据库行为由 `SessionContext` 的实现提供。

生产适配器在 `pkg/session/runtime/system_session.rs`：`ConcreteDdlContext` 实现 `SessionContext`，把事务方法和内部 SQL 执行映射到真实 `ConcreteSession`。一个直接的 DDL 消费者是 `pkg/ddl/notifier/store.rs`，其中 `DdlSessionBackend` 用本文件的 `Session` 开启、提交或回滚事务，并执行通知器系统表 SQL。因此，本文件提供的是 DDL 内部 SQL 会话的能力边界，不负责 DDL job 持久化、owner 调度、schema 状态迁移、reorg 或 schema version 同步。

## 核心职责

1. 用 `SessionContext` 描述 DDL 内部会话必须具备的最小能力：事务进入、语句/事务提交与回滚、事务快照、内部 SQL、会话变量及关闭生命周期（`session.rs:236-267`）。
2. 用 `Session` 将底层 trait 包装成稳定的高层操作：乐观/悲观 begin、commit、rollback、reset、execute 和事务回调（`session.rs:269-384`）。
3. 定义跨适配器传递的轻量数据模型，包括 `SessionError`、`Transaction`、`SqlValue`、`Row`、`ExecutionContext` 和 `RequestSource`（`session.rs:27-121`）。
4. 保存 DDL 内部会话需要的线程安全变量子集，并提供 Acquire/Release 读写和互斥保护的时区字符串（`SessionVariables`，`session.rs:137-234`）。
5. 为 SQL 执行提供耗时/成败观察点，并为 `RunInTxn` 的 Go failpoint 语义提供进程级双线程会合设施（`DurationObserver`、`run_begin_transaction_failpoint`）。

该抽象刻意不拥有连接池；借出、归还、空闲事务校验和内部会话 ID 登记由相邻的 `pkg/ddl/session/session_pool.rs` 完成。

## 主要符号

- `SessionError`：统一错误枚举。`Transaction` 与 `Sql` 携带底层消息；`PoolClosed`、`InvalidResource`、`UnsupportedPool` 同时供相邻会话池使用。其 `Display` 为每类错误添加稳定前缀，且实现标准 `Error`（`session.rs:27-54`）。
- `TransactionMode::{Optimistic,Pessimistic}`：传给 `SessionContext::enter_new_transaction` 的事务模式（`session.rs:56-63`）。
- `Transaction { start_ts, valid }`：只暴露池校验与诊断所需的 MVCC 开始时间戳和有效标志（`session.rs:65-72`）。
- `SqlValue` 与 `Row`：内部 SQL 的参数/结果表示，覆盖 NULL、有/无符号整数、字符串、字节串和布尔值；一行按列序保存 `Vec<SqlValue>`（`session.rs:74-96`）。
- `RequestSource`、`ExecutionContext`：记录请求来源；默认是 `Unspecified`，`Session::execute` 会将其补为 `Ddl`（`session.rs:98-113, 341-345`）。
- `RecordSet`：可发送但不要求同步共享的结果集，要求实现一次批量 `drain` 和显式 `close`（`session.rs:115-121`）。
- `DurationObserver`、`NoopDurationObserver`：接收标签、耗时和成功标志；默认观察者不产生副作用（`session.rs:123-135`）。
- `SessionVariables`：通过原子变量保存事务、自动提交、restricted SQL 和磁盘近满写入开关，通过 `Mutex<String>` 保存会话位置与语句时区（`session.rs:137-234`）。
- `SessionContext`：对象安全的 `Send + Sync` trait。`as_any` 默认返回 `None`，生产适配器可覆盖以支持受控 downcast；其余方法是会话后端必须实现的契约（`session.rs:236-267`）。
- `Session`：持有 `Arc<dyn SessionContext>` 与 `Arc<dyn DurationObserver>`；`new`、`with_observer` 负责组装，`context` 返回共享的底层上下文（`session.rs:269-290, 366-369`）。
- `MOCK_DDL_ONCE`、`set_notify_begin_transaction_mode`、`run_begin_transaction_failpoint`：进程级测试握手状态、配置入口及私有执行逻辑（`session.rs:386-436`）。

## 执行流程

### 事务边界

`Session::begin` 和 `begin_pessimistic` 先分别调用 `enter_new_transaction(Optimistic)` 或 `enter_new_transaction(Pessimistic)`；只有底层调用成功后才把 `SessionVariables::in_transaction` 设为 true。传入的 `ExecutionContext` 当前未向下传递，这一点由参数名 `_context` 明示（`session.rs:292-306`）。

`Session::commit` 固定先调用无返回值的 `statement_commit`，再调用可能失败的 `commit_transaction`（`session.rs:308-312`）。是否清除 `in_transaction` 由后端负责；生产 `ConcreteDdlContext::commit_transaction` 在真实 `COMMIT` 成功后清除该标志（`pkg/session/runtime/system_session.rs`）。`transaction` 总以 `activate = true` 查询底层事务（`session.rs:314-317`）。

`rollback` 使用默认上下文依次执行 `statement_rollback(..., false)` 与 `rollback_transaction`；`reset` 只执行前者，用来清理语句中间态而不结束整个事务（`session.rs:319-330`）。

### 内部 SQL

`Session::execute` 的顺序为（`session.rs:332-364`）：

1. 记录 `Instant::now()`，克隆调用方的 `ExecutionContext`。
2. 若来源仍为 `Unspecified`，将克隆中的来源设为 `RequestSource::Ddl`；调用方对象本身不被修改。
3. 调用 `SessionContext::execute_internal`。若后端返回 `None`，本方法返回 `Ok(None)`。
4. 若返回结果集，调用一次 `drain(8)`，随后无条件调用一次 `close()`。
5. `drain` 成功时返回 `Ok(Some(rows))`；`drain` 失败时返回该错误。观察者最后收到标签、总耗时及 `result.is_ok()`。

生产适配器 `ConcreteDdlContext::execute_internal` 会绑定参数、调用真实 session，并将当前查询所得行包装成 `RecordSet`；`pkg/ddl/notifier/store.rs` 也会绕过 `Session::execute`，通过 `Session::context()` 直接循环 `drain(1024)`，以消费多批结果。

### 回调事务

`run_in_transaction` 先用默认上下文执行乐观 `begin`，再进入 begin failpoint 会合点；回调失败时执行完整 `rollback` 并原样返回回调错误，回调成功时执行 `commit` 并返回提交结果（`session.rs:371-383`）。提交失败后本函数不会额外回滚，清理策略由调用方或池归还路径承担。

## 数据与状态

`Session` 自身不保存“当前事务”对象；真实事务由 `SessionContext` 拥有，`Transaction` 只是查询得到的轻量快照。`Arc<dyn SessionContext>` 允许会话池、包装器和适配器共享同一上下文，`Arc<dyn DurationObserver>` 允许跨线程共享观察者。

`SessionVariables` 的四个布尔值使用 Acquire 读取与 Release 写入，保证跨线程发布/观察这些标志；`location` 与 `statement_time_zone` 分别由互斥锁保护。默认值是：不在事务中、非自动提交、非 restricted SQL、未开启磁盘近满写入，两个时区字符串均为 `UTC`。`set_statement_time_zone_from_location` 先克隆 `location`，再写入另一个锁，避免同时持有两个字符串锁（`session.rs:153-233`）。相邻 `Pool::get` 会将自动提交和 restricted SQL 置为 true、同步语句时区并开启磁盘近满写入；`put/destroy` 会清理磁盘选项（`pkg/ddl/session/session_pool.rs`）。

failpoint 状态是整个进程共享的：模式存于 `AtomicI32`，`MOCK_DDL_ONCE` 存于公开的 `AtomicI64`，`RendezvousState.pending` 由静态 `Mutex`/`Condvar` 保护。模式 1 的线程设置标志并等待；模式 2 仅在标志为 1 时等待 pending 就绪、清除 pending/标志并唤醒对端（`session.rs:386-436`）。

## 依赖与调用关系

- crate 边界：`pkg/ddl/session/Cargo.toml` 无 `[dependencies]` 条目，本文件仅使用 Rust 标准库；`pkg/ddl/session/lib.rs` 公开 `session`、`session_pool` 并再导出其符号。
- 下游调用：`Session` 委托给 `SessionContext`；结果读取再委托给 `RecordSet`；指标上报委托给 `DurationObserver`。相邻 `session_pool.rs` 使用 `SessionContext::session_id`、`session_variables`、`transaction`、`rollback_transaction` 和 `close` 管理借还生命周期。
- 生产实现：`pkg/session/runtime/system_session.rs` 的 `ConcreteDdlContext` 实现本 trait；事务方法执行 `BEGIN OPTIMISTIC/PESSIMISTIC`、`COMMIT` 或 `ROLLBACK`，SQL 方法调用真实执行路径。`pkg/session/Cargo.toml` 直接依赖 `astersql-ddl-session`。
- DDL 消费者：`pkg/ddl/notifier/store.rs::DdlSessionBackend` 使用 `begin`/`begin_pessimistic`/`commit`/`rollback`，并通过 `context().execute_internal` 完整排空查询结果。`pkg/ddl/notifier/Cargo.toml` 以 `ddl-session` 名称依赖本 crate。
- 仓库接线：`pkg/ddl/Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/ddl/jobsubmit/Cargo.toml` 和 `pkg/ddl/ingest/Cargo.toml` 也声明了该 path 依赖；具体是否使用某个符号应以各 crate 当前源码为准，不能仅凭 manifest 推断调用。
- RustCodeGraph 证据：索引将 `session.rs` 识别为 82 个符号，并报告 27 个文件使用它；精确 `callers/callees` 对本文件的常见方法名没有返回可用边，因此上述直接调用关系另以限定路径 `rg` 和源码读取核验。

## 错误处理与边界

`Session` 不改写底层 `SessionError`：begin、commit、transaction、execute 和回调事务均用 `?` 直接传播。`run_in_transaction` 只在回调错误时回滚；begin 失败时不会进入回调或 failpoint，commit 失败时不会自动再回滚。

`execute` 保证获得结果集后会尝试 `close`，即使 `drain` 失败亦然；但当前匹配逻辑只以 `drain` 结果决定返回值：`drain` 成功时即使 `close` 失败也返回成功，`drain` 失败时优先返回 drain 错误（`session.rs:346-360`）。此外它只调用一次 `drain(8)`，因此契约依赖具体 `RecordSet::drain` 在这次调用中返回完整结果；需要真正分页的消费者应像 notifier store 一样直接循环 drain，或在扩展本方法时同时更新测试。

所有 `Mutex`/`RwLock` 获取均使用 `unwrap`；锁中毒会 panic，而不是转换成 `SessionError`。`RecordSet::close`、`statement_commit`、`statement_rollback`、`rollback_transaction` 和 `close` 的部分 trait 方法无法返回错误，后端若失败只能自行降级或标记资源不可复用；生产回滚适配器在真实 `ROLLBACK` 失败时调用 `AvoidReuse`。

failpoint 会合没有超时：只启用模式 1 而没有匹配的模式 2 调用会永久等待。模式 2 在 `MOCK_DDL_ONCE != 1` 时直接跳过；该设施只适合受控测试，不应作为生产同步原语。

## 并发与资源生命周期

`SessionContext: Send + Sync` 和 `DurationObserver: Send + Sync` 允许 `Session` 在其字段满足条件时跨线程使用；`RecordSet` 只要求 `Send`，消费过程保持独占 `&mut self`。会话状态由后端负责同步，本文件不会串行化两个并发事务调用。

正常生命周期是：池借出 `Arc<dyn SessionContext>` → 构造 `Session` → begin/execute/commit 或 rollback → 丢弃包装器并将底层上下文归还池。`Session` 没有 `Drop` 实现，离开作用域不会自动 rollback、close 或归还；调用者必须显式结束事务并通过 `session_pool::Pool` 归还/销毁。`Session::context()` 增加一个 `Arc` 强引用，也不会转移所有权。

事务标志的职责是分层的：包装器在 begin 成功后设为 true，而生产后端在 commit/rollback 后设为 false。相邻池在归还前调用 `transaction(false)` 检查有效事务，因此仅修改原子标志不能代替真实事务结束。

failpoint 会合使用 `while` 循环重新检查 `pending`，可抵御条件变量的伪唤醒；但其模式与标志为全局单例，并行测试必须成对配置和复位，避免跨测试干扰。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/ddl/session/session.go`：

- Rust `Session::new`、`begin`、`begin_pessimistic`、`commit`、`transaction`、`rollback`、`reset`、`execute`、`context`、`run_in_transaction` 分别对应 Go `NewSession`、`Begin`、`BeginPessimistic`、`Commit`、`Txn`、`Rollback`、`Reset`、`Execute`、`Session`、`RunInTxn`。
- 两版 begin 都在底层事务创建成功后设置 in-transaction；commit 都先 statement commit 再提交事务；rollback/reset 的边界一致。
- 两版 execute 都在调用方未指定来源时补 DDL 内部来源、以批大小 8 排空结果、关闭结果集并记录带 label 的耗时/成败。Rust 将 Go 的 `context.Context`、`any` 参数、`chunk.Row`、metrics 和 error wrapping 分别抽象成 `ExecutionContext`、`SqlValue`、`Row`、`DurationObserver` 和 `SessionError`。
- Go `RunInTxn` 的 `NotifyBeginTxnCh` failpoint、`MockDDLOnce` 与 channel 握手被 Rust 映射为原子模式、公开标志以及 `Mutex`/`Condvar` 会合；语义目标相同，但 Rust 需要显式调用 `set_notify_begin_transaction_mode`，没有 Go failpoint 框架的动态注入。
- Rust 额外引入对象安全的 `SessionContext`/`RecordSet` trait 和线程安全的 `SessionVariables`，用于隔离生产适配器和 mock；Go 直接嵌入 `sessionctx.Context`。

独立 Rust 测试 `pkg/ddl/session/session_pool_test.rs` 对照 Go `pkg/ddl/session/session_pool_test.go`，验证乐观 begin/transaction/execute/commit 主链与悲观事务锁阻塞。Rust 测试使用 mock 行锁模拟冲突，不等同于 Go 测试中的真实 mock store/KV 锁；它证明的是包装层调用顺序及并发等待约束。`pkg/ddl/notifier/store_test.rs::table_store_uses_real_ddl_session_sql_transactions` 另验证 notifier 对此抽象的真实 SQL 事务接线。

## 扩展指南

- 新增会话能力时，先判断它属于通用包装层还是具体后端。通用事务/执行语义应扩展 `SessionContext` 与 `Session`；数据库特有行为应落在 `pkg/session/runtime/system_session.rs::ConcreteDdlContext`，避免把真实 SQL 引擎细节放入这个零外部依赖 crate。
- 修改事务顺序时，应同步 `pkg/ddl/session/session_pool_test.rs` 中的 mock 状态断言，并核对 Go `session.go`；涉及 notifier 的 begin/commit/rollback 时还要覆盖 `pkg/ddl/notifier/store_test.rs`。
- 扩展 `SqlValue` 时必须同步生产适配器的参数绑定和所有模式匹配；重点检查 NULL、二进制、引号/反斜杠转义以及整数边界。兼容风险在于消费者可能穷举该枚举。
- 修改 `execute` 的分页或 close 错误语义时，应新增独立 Rust 测试，至少覆盖：无结果集、超过单批容量、drain 失败、close 失败、观察者 success 标志与请求来源补全。不要把测试嵌入 `session.rs`；沿用同目录独立测试文件的仓库约定。
- 修改 `SessionVariables` 时保持线程安全访问器，不直接暴露可变字段；池借出/归还默认值需同步检查 `session_pool.rs` 及其测试。
- 修改 failpoint 会合时要维持 Go 的两端握手意图，并增加超时或隔离测试时评估全局状态兼容性；任何生产调用都不应依赖该测试设施。
- 性能方面，关注每次 execute 的上下文克隆、结果行聚合和参数绑定；正确性方面，确保事务失败不会把有效事务归还池；兼容性方面，保持 `SessionError::Display`、请求来源和 Go 行为对齐。

## 验证依据

- 目标源码与 crate：`pkg/ddl/session/session.rs`、`pkg/ddl/session/lib.rs`、`pkg/ddl/session/Cargo.toml`。
- 直接相邻实现：`pkg/ddl/session/session_pool.rs`。
- 生产适配器与真实消费者：`pkg/session/runtime/system_session.rs`、`pkg/session/Cargo.toml`、`pkg/ddl/notifier/store.rs`、`pkg/ddl/notifier/Cargo.toml`。
- Go 对照：`pkg/ddl/session/session.go`、`pkg/ddl/session/session_pool_test.go`。
- 独立 Rust 测试：`pkg/ddl/session/session_pool_test.rs`；消费者接线测试：`pkg/ddl/notifier/store_test.rs::table_store_uses_real_ddl_session_sql_transactions`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ddl/session` 列出该目录 7 个 Go/Rust 文件；`node --file pkg/ddl/session/session.rs --offset 1 --limit 500` 读取全部 436 行并报告 82 个符号、27 个使用文件；同样用 `node --file` 读取了 Go 对照和 Rust 独立测试。精确 callers/callees 未产出可用边，故调用点以 `rg` 和上述源码复核。
- 人工复核结论：本文件存在的理由是为 DDL 后台 SQL 提供可替换、可测试的会话边界；主要运行路径是池借出上下文后经 `Session` 委托到底层适配器；安全扩展必须同步 trait、生产适配器、独立 Rust 测试和 Go 语义。
