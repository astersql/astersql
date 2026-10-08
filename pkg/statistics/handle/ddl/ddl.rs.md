# `pkg/statistics/handle/ddl/ddl.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-statistics-handle-ddl`；包入口 `pkg/statistics/handle/ddl/lib.rs` 将 `ddl` 与 `subscriber` 两个模块公开并重新导出其符号。它位于统计信息对 DDL（schema 变更）作出响应的边界：本文件负责持有事件 FIFO 和调用订阅者，具体的事件分派与统计元数据写入规则位于同 crate 的 `subscriber.rs`。

源文件：[ddl.rs](./ddl.rs)。

当前 Rust 接线必须与设计意图区分开。`DdlHandler<B>` 使用本 crate 自己的 `subscriber::SchemaChangeEvent` 枚举；全仓 Rust 引用检索只发现 `ddl_test.rs` 直接构造和调用它。生产测试辅助 `testutil/util.rs` 使用的则是 `aster_sql_ddl_notifier::SchemaChangeEvent`，并通过 `TransactionalDDLHandle` 抽象访问真实会话池和 `Receiver`。因此，本文能确认本文件的队列与分发逻辑已实现并有独立测试，但不能确认它已接到 Rust 生产 DDL notifier 主链。

## 核心职责

1. 用 `DdlHandler<B>` 把一个实现 `StatsBackend` 的后端包装成 `Subscriber<B>`，同时创建容量目标为 1,000 的本地 FIFO（`new`）。
2. 提供显式有界入队和出队接口（`enqueue`、`next_event`），在达到 `DDL_EVENT_CHANNEL_CAPACITY` 时拒绝新事件，避免无限积压。
3. 提供单事件处理入口 `handle_ddl_event`，将事件交给 `Subscriber::handle`；订阅者错误通过后端的 `warn_ignored_event_error` 报告后被吞掉，入口仍返回成功，以对齐 Go 当前的 best-effort 确认语义。
4. 暴露 `subscriber`/`subscriber_mut`，让调用方或测试访问同一订阅者及其后端状态。
5. 提供测试专用包装 `update_stats_with_count_delta_and_modify_count_delta_for_test`，不复制算法，只转发到 `subscriber.rs` 的真实增量更新函数。

## 主要符号

- `DDL_EVENT_CHANNEL_CAPACITY: usize = 1_000`：逻辑队列上限。它既用于 `VecDeque::with_capacity` 的初始预留，也用于 `enqueue` 的硬上限判断。
- `DdlHandler<B>`：泛型处理器；`B` 在实现块上受 `StatsBackend` 约束。私有字段 `events: VecDeque<SchemaChangeEvent>` 保存未消费事件，`subscriber: Subscriber<B>` 保存具体统计后端。
- `DdlHandler::new(backend) -> Self`：取得后端所有权并构造订阅者。没有线程、会话或事务参数。
- `enqueue(&mut self, event) -> Result<(), Error>`：当 `len() == 1_000` 时返回文本为 `DDL event channel is full` 的 `subscriber::Error`，否则从队尾插入。
- `next_event(&mut self) -> Option<SchemaChangeEvent>`：从队首移出事件；空队列返回 `None`。
- `handle_ddl_event(&mut self, event) -> Result<(), Error>`：调用 `Subscriber::handle`；若失败，调用同一订阅者后端的 `warn_ignored_event_error(event, error)`，随后固定返回 `Ok(())`。
- `subscriber(&self)` 与 `subscriber_mut(&mut self)`：分别公开共享和独占借用，不转移后端所有权。
- `update_stats_with_count_delta_and_modify_count_delta_for_test(backend, table_id, count_delta, modify_count_delta)`：公开测试缝，直接调用下游同名非 `_for_test` 函数并原样传播其结果。

本文件没有 trait、枚举、条件编译项或异步函数。事件种类、`Error`、`StatsBackend` 和 `Subscriber` 都定义在 `subscriber.rs`。

## 执行流程

构造与队列消费路径如下：

1. 调用方把 `StatsBackend` 的具体实例交给 `DdlHandler::new`；处理器预留 1,000 个队列元素的空间，并通过 `Subscriber::new` 保存后端。
2. `enqueue` 在每次插入前检查当前长度。未满时 `push_back`，满时不修改队列并返回错误。
3. 消费者调用 `next_event`，由 `pop_front` 保证先进先出；本文件不会自动循环消费，也不会在出队后自动调用处理入口。
4. 调用方把取得的事件传给 `handle_ddl_event`。该方法同步调用 `Subscriber::handle`；后者根据 `SchemaChangeEvent` 变体执行建表伪统计插入、截断/删除的延迟删除、分区与全局统计调整、加列统计初始化、Flashback 版本刷新等规则。
5. 下游成功时直接返回 `Ok(())`；失败时先让后端记录被忽略错误，再仍返回 `Ok(())`。调用者无法从返回值区分“统计更新成功”和“失败但已告警”。

测试增量路径独立于事件队列：`update_stats_with_count_delta_and_modify_count_delta_for_test` 直接进入 `subscriber.rs`。下游先读取锁表集合和 `start_ts`；锁定表使用允许负值的增量更新，非锁定表读取现值后将 `count` 与 `modify_count` 的新绝对值钳制到不小于零再写回。

## 数据与状态

`events` 是处理器独占的 `VecDeque`。`with_capacity(1_000)` 只是预分配，而真正的有界性来自 `enqueue` 的长度检查；所有写入只能通过需要 `&mut self` 的方法，因此在普通安全 Rust 中同一实例不能并发执行两个队列操作。`next_event` 会转移事件所有权，出队后长度减一，可再次入队。

`subscriber` 拥有 `B`，后端状态与队列生命周期都跟随 `DdlHandler`。`subscriber_mut` 允许调用方绕过本文件直接使用订阅者或后端，因此本文件不提供更强的封装不变量。`handle_ddl_event` 借用事件而不消费它，也不把已处理状态写入队列；入队、出队、处理是三个彼此独立的操作。

本文件的 `SchemaChangeEvent` 是 `subscriber.rs` 中面向统计子系统的值类型枚举，而不是生产 notifier crate 的同名结构。事件有效性、表/分区 ID 含义与具体统计副作用由下游订阅者负责。

## 依赖与调用关系

- 标准库下游：`std::collections::VecDeque` 提供 FIFO 存储。
- crate 内下游：`DdlHandler::new -> Subscriber::new`；`handle_ddl_event -> Subscriber::handle`，失败分支再到 `Subscriber::backend_mut -> StatsBackend::warn_ignored_event_error`；测试包装函数转发到 `subscriber::update_stats_with_count_delta_and_modify_count_delta`。
- 模块边界：`lib.rs` 声明并重新导出 `ddl` 与 `subscriber`；`Cargo.toml` 的 `[lib] path = "lib.rs"`，并用 `package.metadata.porting.go-package` 指向 `pkg/statistics/handle/ddl`。该 crate 自身没有列出第三方依赖。
- 已验证的 Rust 上游：`ddl_test.rs` 构造 `DdlHandler<RecordingBackend>`，调用事件处理、队列和测试包装接口。RustCodeGraph 为目标文件建立了 9 个符号，但精确查询与全仓 Rust 引用检索均未找到本文件 API 的生产调用者。
- 相邻但不同的生产边界：`testutil/util.rs` 的 `TransactionalDDLHandle` 接收 `aster_sql_ddl_notifier::SchemaChangeEvent`，并通过 `call_with_sctx(..., FLAG_WRAP_TXN)` 处理；它不是本文件 `DdlHandler` 的实现证据。
- Go 主链：`pkg/ddl/ddl.go` 把统计 Handle 的 `DDLEventCh()` 保存到 DDL；`pkg/statistics/handle/ddl/ddl.go` 的 `ddlHandlerImpl` 消费 notifier 事件并调用 Go `subscriber`。这说明 Go 生产接线存在，不代表 Rust 同路径已经接通。

## 错误处理与边界

队列唯一的本地错误是满队列：第 1,001 个未消费事件被拒绝，原有 1,000 个事件保持不变。判断使用 `len() == capacity`；在所有队列修改都经本类型方法完成的前提下，长度不会超过该常量。空队列不是错误，而是 `None`。

事件处理采用刻意的 best-effort 边界。`Subscriber::handle` 的任何 `Error` 都不会从 `handle_ddl_event` 传播，只会传给 `warn_ignored_event_error`；该 trait 方法默认可以是空实现，所以“已调用告警钩子”不保证一定产生可见日志。与 Go 类似，入口通过返回 `nil`/`Ok(())` 避免 notifier 因统计维护失败而无限重试，但 Rust 版本没有复刻 Go 中对允许错误类型的 `intest.Assert` 检查。

测试包装函数不吞错，锁表查询、时间戳、元数据读取或写入错误会原样向测试调用者传播。系统表过滤也不在本文件内完成：`TestSystemTableDDLHasNoEvent_is_caller_policy` 明确把 `mysql.*` 过滤定义为入队方责任。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务或 I/O 资源。队列是进程内同步容器，所有操作要求 `&mut self`，因此并发共享必须由外层自行选择 `Mutex` 等同步方式；本类型没有内部同步，也没有背压等待机制。满队列立即返回错误，不阻塞生产者。

`DdlHandler` 取得后端所有权；处理器被丢弃时，队列中尚未消费的事件和 `Subscriber<B>` 一并正常释放。没有关闭/排空钩子，也没有 Drop 实现。一次 `handle_ddl_event` 是同步调用，但其统计更新是否具有事务性取决于 `StatsBackend` 的具体实现；本文件既不开始事务，也不回滚部分下游副作用。相邻 `testutil/util.rs` 的事务包装属于另一条接口路径，不能外推到这里。

## 与 Go 版本的对应关系

`DdlHandler<B>` 对应 `ddl.go` 的 `ddlHandlerImpl`，`Subscriber<B>` 对应其 `sub` 字段。Rust 的 `DDL_EVENT_CHANNEL_CAPACITY = 1_000` 对齐 Go `make(chan *notifier.SchemaChangeEvent, 1000)`；Rust 用显式 `VecDeque` 与满队列错误表示有界队列，Go 通道发送的阻塞/调度语义并未在本文件中复刻。

Rust `handle_ddl_event` 与 Go `HandleDDLEvent` 都把订阅者错误视为 best effort 并向调用者返回成功。差异是 Go 接收 `context.Context`、`sessionctx.Context` 与 notifier 事件，记录采样日志并在测试模式断言错误属于允许集合；Rust 接收本地事件枚举，只通过抽象后端告警钩子报告错误，没有上下文或会话参数。

Rust 的 `_for_test` 函数对应 Go `UpdateStatsWithCountDeltaAndModifyCountDeltaForTest`，但依赖注入层次不同：Go 传入 session context 并由存储/锁表工具执行 SQL；Rust 传入 `StatsBackend`，下游算法保留“锁定表允许负增量、非锁定表结果不小于零”的语义。`ddl_test.rs` 的锁表测试直接验证了这一分支。

Go `DDLEventCh()` 返回实际 channel 并已被 `pkg/ddl/ddl.go` 接线。Rust 本文件只提供 `enqueue`/`next_event`，且事件类型与 `testutil` 的生产 notifier 类型不同；生产适配、系统表过滤和持续消费循环均未在本文件中得到验证。

## 扩展指南

- 若新增队列策略，应修改 `enqueue`/`next_event` 并扩展同目录独立测试 `ddl_test.rs`；需要明确满队列究竟拒绝、阻塞、覆盖还是合并，不能仅改变预分配容量。
- 若新增 schema 事件或统计副作用，事件枚举和主要分派位于 `subscriber.rs`，本文件通常无需加入分支；应同步更新 `subscriber.rs` 的处理逻辑及 `ddl_test.rs` 的后端调用断言。
- 若要完成生产接线，必须显式实现 `aster_sql_ddl_notifier::SchemaChangeEvent` 到本地枚举的适配，或统一事件模型，并决定 `DdlHandler` 如何实现生产 `Handle`/`TransactionalDDLHandle` 边界。不能靠两个同名类型假定兼容。
- 若改变 best-effort 策略，应同时审查 Go `HandleDDLEvent` 的确认语义、重试风险和可观测性；若开始传播错误，调用方可能重复处理已有部分副作用的事件。
- 若改变增量更新规则，修改真实实现 `subscriber::update_stats_with_count_delta_and_modify_count_delta`，保留本文件测试包装的薄转发，并覆盖锁定/未锁定、缺失元数据、负数钳制和后端错误。
- 测试逻辑继续放在 `ddl_test.rs`，不要嵌入 `ddl.rs`。兼容性风险集中在事件确认和 Go 语义差异；性能风险集中在 1,000 条积压、事件载荷克隆成本以及同步逐事件处理。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/statistics/handle/ddl` 确认目标、Go 对照与独立测试均已索引；`node --file pkg/statistics/handle/ddl/ddl.rs --offset 1 --limit 500` 读取目标全部 96 行；精确 `query` 定位 `DdlHandler`、`handle_ddl_event`、`SchemaChangeEvent`、`StatsBackend` 与两个增量更新函数。
- 目标源码：`pkg/statistics/handle/ddl/ddl.rs`，核对常量、结构体字段、全部公开方法、错误吞并分支与测试转发函数。
- crate 边界：`pkg/statistics/handle/ddl/Cargo.toml`、`pkg/statistics/handle/ddl/lib.rs`，核对包名、库入口、Go 包映射、模块与重新导出。
- 直接下游：`pkg/statistics/handle/ddl/subscriber.rs`，核对本地事件枚举、`StatsBackend` 告警钩子、`Subscriber::handle` 和增量更新规则。
- Go 对照与生产接线：`pkg/statistics/handle/ddl/ddl.go`、`pkg/ddl/ddl.go`，核对 1,000 容量、best-effort 返回、会话/上下文差异和 Go channel 接线。
- 独立 Rust 测试：`pkg/statistics/handle/ddl/ddl_test.rs`，核对建表、截断、物化视图切换、分区、DropSchema、加列、Flashback、锁表增量、队列满/出队和调用方系统表过滤策略。
- 相邻接口证据：`pkg/statistics/handle/ddl/testutil/util.rs`，核对生产 notifier 事件、事务包装与接收端接口和本文件并非同一事件/队列抽象。
- 全仓 `rg` 复核：排除目标及其直接测试后未发现 `DdlHandler`、`handle_ddl_event`、`enqueue`/`next_event` 的生产调用；因此文中将生产 Rust 接线标为“未验证”，没有用 Go 架构推断 Rust 现状。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前用任务给定命令验证恰有 11 个固定二级标题，并人工复查没有把未接线能力写成已支持。
