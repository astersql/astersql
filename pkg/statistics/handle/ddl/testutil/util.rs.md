# `pkg/statistics/handle/ddl/testutil/util.rs`

## 文件定位

本文件是 `astersql-statistics-handle-ddl-testutil` crate 的核心实现，源码入口由同目录 [`lib.rs`](lib.rs) 的 `mod util; pub use util::*;` 统一导出。它不是 DDL 生产处理器，而是统计信息相关测试在制造或截获 `SchemaChangeEvent` 后使用的辅助层：一组函数负责通过真实统计会话事务路径处理事件，另一组函数负责从事件接收端筛选目标 `ActionType`。crate 边界和四个直接依赖见同目录 [`Cargo.toml`](Cargo.toml)。

该文件位于 `pkg/statistics/handle/ddl/testutil`，对应 Go 包 `pkg/statistics/handle/ddl/testutil`。Rust 当前通过 `TransactionalDDLHandle` 抽象所需的 Handle 能力；同目录独立测试实现了该 trait，但本文件本身没有为完整生产 `statistics::handle::Handle` 提供实现。因此它应被理解为测试基础设施及未来生产 Handle 接线边界，而不是已接入应用 DDL 主循环的组件。

## 核心职责

1. `HandleDDLEventWithTxn` 从 Handle 取得 `SessionPool`，用 `FLAG_WRAP_TXN` 调用统计公共工具 `call_with_sctx`，并在回调中构造标记为 `InternalDDLNotifier` 的请求上下文后调用 Handle 的事件处理能力。
2. `HandleNextDDLEventWithTxn` 串联“从 Handle 的事件接收端取下一条消息”和“在事务中处理该消息”两个动作。
3. `FindEvent` 持续消费事件，丢弃类型不匹配的消息，直到返回首个匹配 `ActionType` 的事件。
4. `FindEventWithTimeout` 在一个固定的全局截止时间内执行同样的筛选；超时返回 `None`，而不是让每次收到非目标事件都重新获得完整超时时间。

这些 helper 的共同目标是让测试复用生产事务包装和请求来源标记，同时把事件通道中与当前断言无关的 DDL 消息过滤掉。它们会消费并永久丢弃非目标事件，不提供回放或放回通道的机制。

## 主要符号

- `pub trait TransactionalDDLHandle`：对 helper 暴露的最小能力集合。
  - `SPool(&self) -> &dyn SessionPool` 提供统计会话池，对应 Go 的 `h.SPool()`。
  - `HandleDDLEvent(&self, &Context, &dyn SessionContext, &SchemaChangeEvent) -> Result<(), StatsError>` 执行实际事件处理。
  - `DDLEventCh(&self) -> &Mutex<Receiver<SchemaChangeEvent>>` 提供独占接收端。`Mutex` 是 Rust 对单消费者 `Receiver` 共享访问的显式同步边界。
- `HandleDDLEventWithTxn<H: TransactionalDDLHandle>`：处理调用方已经取得的事件。成功或失败由 `call_with_sctx` 及 Handle 回调的 `StatsError` 结果表达。
- `HandleNextDDLEventWithTxn<H: TransactionalDDLHandle>`：阻塞读取恰好一条事件，再委托给 `HandleDDLEventWithTxn`。
- `FindEvent(&Receiver<SchemaChangeEvent>, ActionType) -> SchemaChangeEvent`：无限期阻塞筛选，匹配依据是 `SchemaChangeEvent::GetType()`。
- `FindEventWithTimeout(&Receiver<SchemaChangeEvent>, ActionType, isize) -> Option<SchemaChangeEvent>`：带秒级正整数超时的筛选；找到返回 `Some(event)`，截止时间到达返回 `None`。

文件没有模块级常量、自定义结构体、`impl`、feature 条件编译或后台任务。公开命名沿用 Go 风格；crate 根用 `#![allow(non_snake_case)]` 明确允许这些名称。

## 执行流程

`HandleDDLEventWithTxn` 的主流程如下：

1. 调用 `handle.SPool()` 取得真实 `SessionPool`。
2. 调用 `aster_sql_statistics_handle_util::call_with_sctx`，传入闭包及唯一 flag `FLAG_WRAP_TXN`。
3. 闭包从 `Context::default()` 创建上下文，经 `WithInternalSourceType(..., InternalDDLNotifier)` 标为内部 DDL notifier 请求。
4. 闭包调用 `handle.HandleDDLEvent(context, session_context, event)`，其结果原样交回事务包装层。
5. 下游 `call_with_sctx` 先同步统计会话变量；检测到 `FLAG_WRAP_TXN` 后进入 `wrap_txn`，执行 `BEGIN PESSIMISTIC`。回调成功后 `finish_transaction` 执行 `COMMIT`；回调失败时尝试 `rollback`，忽略 rollback 自身错误并返回原始错误。

`HandleNextDDLEventWithTxn` 先取得 `DDLEventCh()` 的互斥锁，再在持锁期间阻塞 `recv()`；成功得到事件后结束锁表达式并调用 `HandleDDLEventWithTxn`。因此等待消息时其他需要同一 receiver 锁的代码不能并行接收，而后续事务处理不持有该锁。

`FindEvent` 在循环中调用 `recv()`：类型相同立即返回，类型不同继续下一次接收。`FindEventWithTimeout` 在进入循环前计算一次 `deadline = Instant::now() + Duration::from_secs(...)`；每轮以 `deadline.saturating_duration_since(Instant::now())` 作为 `recv_timeout` 的剩余预算。目标事件立即返回，截止前的非目标事件继续循环，超时或截止后收到非目标事件返回 `None`。

## 数据与状态

本文件不保存全局或长期可变状态。事件以拥有所有权的 `SchemaChangeEvent` 在线程间通道传递；`HandleDDLEventWithTxn` 只借用调用方持有的事件，两个查找函数则从 `Receiver` 取走并返回目标事件。

`SchemaChangeEvent::GetType` 从事件内部对象读取 `ActionType`；若事件没有内部对象，下游 notifier 实现返回 `ActionNone`。因此空内部事件只有在调用方查找 `ActionNone` 时才会匹配，否则会像其他非目标事件一样被消费。

超时状态仅由局部 `Instant deadline` 和每轮重新计算的 `Duration remaining` 构成。`timeout_seconds` 先验证大于零，再转换成 `u64`，避免负数转换为极大时长。事务状态归属于下游 `SessionContext`，本文件只通过 flag 请求包装，不自行保存 transaction 对象。

## 依赖与调用关系

- crate 入口：[`lib.rs`](lib.rs) 私有声明 `util`，随后公开再导出本文件全部公开符号；测试通过 `#[path = "util_test.rs"]` 保持测试逻辑在独立文件中。
- `aster_sql_ddl_notifier::SchemaChangeEvent`：事件载体；其 `GetType` 是筛选依据。
- `aster_sql_meta_model::ActionType`：调用方指定的目标事件类型。
- `aster_sql_kv::{Context, WithInternalSourceType, InternalDDLNotifier}`：建立内部请求来源。`WithInternalSourceType` 设置 `RequestSourceInternal = true` 及对应来源字符串。
- `aster_sql_statistics_handle_util::{SessionContext, SessionPool, StatsError, call_with_sctx, FLAG_WRAP_TXN}`：提供会话获取、事务包装和统一错误类型。
- 标准库 `std::sync::mpsc::Receiver` 与 `RecvTimeoutError`：同步单消费者通道；`Mutex` 为 Handle 暴露的 receiver 提供共享引用下的独占接收。
- 标准库 `Instant` 与 `Duration`：实现单一全局截止时间。

RustCodeGraph 的函数级下游边确认：`HandleDDLEventWithTxn → SPool/HandleDDLEvent`，`HandleNextDDLEventWithTxn → DDLEventCh/HandleDDLEventWithTxn`。函数级 callers 查询未解析出调用者；文件级索引和精确文本核验显示，当前可执行 Rust 调用集中在 [`util_test.rs`](util_test.rs)，而若干其他 Rust 移植测试仍只在注释中的 Go 原文引用这些名字。Go 对照 helper 则被多个统计、planner 测试广泛调用，说明其设计用途主要是测试 DDL 触发后的统计更新行为，而非生产请求链。

## 错误处理与边界

- `HandleDDLEventWithTxn` 返回 `Result<(), StatsError>`。Handle 回调失败时，下游 `finish_transaction` 尝试 rollback 并保留原始错误；BEGIN 或 COMMIT 的 SQL 错误也会返回。需要注意，当前 `call_with_sctx` 会捕获 session-pool 闭包路径上的 panic 并转成 `Ok(())`，这是下游已有语义，本 helper 没有额外改变它。
- `HandleNextDDLEventWithTxn` 对 poisoned mutex 使用 `expect("DDL event receiver lock is poisoned")`，对关闭通道使用 `expect("DDL event channel is closed")`；这两类基础设施失效会 panic，而不是返回 `StatsError`。
- `FindEvent` 在通道关闭时 panic。若通道保持连接但永远没有目标事件，它会永久阻塞。
- `FindEventWithTimeout` 要求 `timeout_seconds > 0`，否则以 `non-positive interval for NewTicker` panic，以对齐 Go `time.NewTicker` 的边界。超时是正常的 `None`；通道断开则 panic。
- 查找函数会丢弃先到的非目标事件。调用方若后续仍需这些事件，不能使用这些 helper，或必须在更上层增加缓冲/分发机制。
- 时间判断存在预期的截止边界竞争：若非目标消息恰在截止处到达，分支返回 `None`；目标消息由 `recv_timeout` 成功返回时会立即被接受。

## 并发与资源生命周期

Rust `std::sync::mpsc::Receiver` 是单消费者端。本文件没有克隆 receiver，也不创建线程；调用者负责 sender/receiver 的创建、发送端存活和线程调度。`TransactionalDDLHandle::DDLEventCh` 返回 `Mutex<Receiver<_>>`，确保通过 Handle 共享引用访问时一次只有一个接收者。锁中毒和通道断开被视为测试基础设施错误并显式 panic。

`HandleNextDDLEventWithTxn` 等待事件期间持有 receiver mutex，但事件取得后释放锁，事务执行期间不会继续占有它。`FindEvent` 和 `FindEventWithTimeout` 接受裸 `&Receiver`，并发独占责任由调用者保证。

超时 helper 不创建 ticker 或定时线程，`Instant`/`Duration` 均为栈上值，不需要清理；函数返回即结束等待资源生命周期。事务资源由 `call_with_sctx` 所取得的会话管理：正常路径提交，事件处理错误路径回滚，会话池控制会话借用的开始与结束。

## 与 Go 版本的对应关系

Go 对照文件是 [`util.go`](util.go)，四个同名函数逐一对应：

- Go `HandleDDLEventWithTxn(*handle.Handle, *SchemaChangeEvent)` 直接依赖具体 Handle；Rust 用泛型 `H: TransactionalDDLHandle` 暂代生产 Handle 接线，但仍强制从 Handle 取得 pool 并走真实 `call_with_sctx(FLAG_WRAP_TXN)`，没有把事务行为降级为可替换闭包。
- 两个版本都在事件处理前将上下文来源设为 `InternalDDLNotifier`，并调用 Handle 的 DDL 事件处理入口。
- Go `HandleNextDDLEventWithTxn` 直接从 `<-h.DDLEventCh()` 读取；Rust 用 `Mutex<Receiver<_>>` 表达接收端的独占访问，并对关闭通道给出明确 panic 文本。行为意图相同，但 Rust 的失效位置更明确。
- Go `FindEvent` 反复读取并跳过非目标事件；Rust 保持相同行为，并显式处理 disconnected。
- Go 超时版使用一次 `time.NewTicker(timeout seconds)`，首次 tick 后返回 `nil`；Rust 用固定 deadline 和 `recv_timeout` 返回 `None`。两者都采用总超时而非逐消息重置超时。Rust 明确断言正超时，以复现 `time.NewTicker` 对非正 duration 的 panic。
- Go 从关闭通道读取到 `nil` 后在 `event.GetType()` 处失败；Rust 不能产生 nil 事件，因而在 `recv`/`recv_timeout` 返回断开时主动 panic，保留“关闭是可见失败”的测试语义。

同目录 [`util_test.rs`](util_test.rs) 是 Rust 的独立回归测试，覆盖 Go 语义及 Rust 显式边界。Go 实际调用面还包括 `pkg/statistics/handle/ddl/ddl_test.go`、`pkg/statistics/handle/updatetest/update_test.go`、`pkg/statistics/handle/autoanalyze/priorityqueue/queue_ddl_handler_test.go` 等测试。

## 扩展指南

- 若接入真实 Rust statistics Handle，应在 Handle 所属 crate 实现 `TransactionalDDLHandle`，并确认不会引入 testutil 与 Handle crate 的循环依赖；不要在 helper 内复制生产 DDL 处理逻辑。接线后应在独立测试文件验证真实 pool、真实事件 channel 以及 Handle 错误传播。
- 若修改事务语义，入口是 `HandleDDLEventWithTxn` 的 flag/上下文构造；同时必须检查 `pkg/statistics/handle/util/util.rs` 中 `call_with_sctx`、`wrap_txn`、`finish_transaction` 的契约，并同步 [`util_test.rs`](util_test.rs) 的 BEGIN/COMMIT/rollback 与原始错误断言。
- 若增加筛选条件，可在 `FindEvent`/`FindEventWithTimeout` 附近新增明确命名的 helper，但需说明非匹配事件是丢弃、缓存还是转发。不能悄悄改变现有函数“消费并丢弃”的兼容行为。
- 若修改超时单位或类型，应同时对齐 Go `FindEventWithTimeout`、正值校验、总 deadline 不变量和边界测试。高事件速率下必须继续以固定 deadline 计算剩余预算，否则持续的非目标事件可能让调用永不超时。
- 新测试继续放在独立 [`util_test.rs`](util_test.rs)，不要内嵌到源文件。至少覆盖成功匹配、非目标跳过、截止超时、非正超时、通道关闭、事务成功提交和事件错误回滚。
- 兼容风险主要是 Go/Rust 边界差异和 panic/`Result` 的选择；性能风险主要是持锁阻塞接收及线性丢弃大量非目标事件。任何改变都应先核对现有测试调用是否依赖这些特性。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标目录列出 `lib.rs`、`util.rs`、`util_test.rs` 和 `util.go`，其中 `util.rs` 识别出 17 个符号。
- RustCodeGraph 源码与符号查询：[`util.rs`](util.rs) 的 `TransactionalDDLHandle`、`HandleDDLEventWithTxn`、`HandleNextDDLEventWithTxn`、`FindEvent`、`FindEventWithTimeout`；调用边为 `HandleDDLEventWithTxn → SPool/HandleDDLEvent` 和 `HandleNextDDLEventWithTxn → DDLEventCh/HandleDDLEventWithTxn`。
- 下游实现查询：`pkg/statistics/handle/util/util.rs` 的 `call_with_sctx`、`wrap_txn`、`finish_transaction`，确认会话变量同步、悲观事务、提交、回滚及原始错误保留；`pkg/kv/option.rs::WithInternalSourceType` 确认内部请求标记；`pkg/ddl/notifier/events.rs::GetType` 确认事件类型读取及空内部事件的 `ActionNone`。
- crate 与模块边界：同目录 [`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)。Cargo metadata 明确 Go 包对应关系和四个 path dependency，未声明 feature。
- Go 对照：同目录 [`util.go`](util.go) 的四个同名函数；精确引用搜索确认这些 helper 被统计 Handle、DDL、更新、自动分析等 Go 测试使用。
- Rust 测试：同目录 [`util_test.rs`](util_test.rs) 的 `handle_next_event_uses_real_wrapped_transaction`、`handle_event_error_rolls_back_and_propagates`、三个事件筛选测试以及关闭通道/非正超时 panic 测试。
- 目标目录及向上至 `pkg/statistics` 未找到 `doc.go`，因此没有额外的包级 Go 契约需要纳入。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核所有行为陈述均能追溯到上述源码、图查询或测试。
