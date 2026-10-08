# [`pkg/session/runtime/ttl_timer_store.rs`](./ttl_timer_store.rs)

## 文件定位

本文件位于 `astersql-session` crate 的运行时层，由 `pkg/session/runtime.rs` 以公开模块 `ttl_timer_store` 装配。它不是 Timer 表存储本身，而是 `pkg/timer/tablestore/store.rs` 与 session 运行时之间的适配层：把持有 `Domain` 的真实 `ConcreteSession` 包装成 `astersql_session_syssession::SessionContext`，再构造可供 `NewTableTimerStore` 使用的 `AdvancedSessionPool`。

当前生产入口只有 `pkg/session/runtime/ttl_runtime.rs`：TTL manager 成为 owner 并完成 timer 元数据同步后，在 `timer_runtime` 尚未创建时调用 `new_ttl_timer_session_pool(domain, 8)`，随后把池传给 `NewTableTimerStore(1, ..., "mysql", "tidb_timers", notifier)`。因此本文件处在“TTL 调度运行时 → 通用表式 TimerStore → SQL session”链路上，不负责 TTL 扫描、删除或 timer 调度策略本身。

`pkg/session/Cargo.toml` 将该文件归入包 `astersql-session`，其直接使用的 crate 依赖包括 `astersql-domain`、`astersql-session-syssession`、`astersql-timer-tablestore` 与 `chrono`；文件内没有 feature gate 或条件编译项。

## 核心职责

1. `new_ttl_timer_session_pool` 为每个池内 session 创建一个 `TimerSessionContext`，而每个 context 又拥有一个专属 SQL worker 线程。
2. `TimerSessionContext::query` 通过通道把 SQL 发送到 worker，使同一租约中的 `BEGIN`、读写、`COMMIT`/`ROLLBACK` 始终落在同一个 `ConcreteSession` 和同一个线程上。
3. `bind_parameters` 把通用 TimerStore 生成的 `%?` 参数 SQL 转换为当前 session 可执行的完整 SQL文本，包括字符串转义、字节十六进制字面量、布尔/整数/NULL，以及 `FROM_UNIXTIME(%?)` 的时间转换。
4. `decode_row` 把 `ConcreteSession` 返回的 `Vec<String>` 恢复成 TimerStore 需要的 `SqlRow(Vec<SqlCell>)`，对 NULL、二进制、整数、JSON 和时间列做定向类型恢复。
5. 实现 `SessionContext` 所需的最小能力，并明确拒绝本链路不使用的 parsed statement、restricted statement 与 restricted SQL API。

## 主要符号

- `BINARY_PREFIX: &str`：值为 `__astersql_binary_hex__:`，与 `pkg/session/runtime/row_codec.rs` 的 `BINARY_RUNTIME_PREFIX` 一致。真实 session 用此前缀把二进制列编码成字符串，本文件据此还原 `SqlCell::Bytes`。
- `Command`：worker 控制消息。`Execute(String, SyncSender<Result<Vec<Vec<String>>, String>>)` 携带完整 SQL 与单次回复通道；`Stop` 终止循环。
- `TimerSessionContext`：私有 `SessionContext` 实现。`sender` 是无界命令发送端，`thread` 保存可被 `take` 的 `JoinHandle`，`transaction_open` 是池复用检查所需的事务状态镜像。
- `TimerSessionContext::new(domain)`：命名并启动 `ttl-timer-sql-session` 线程，在该线程内只创建一次 `ConcreteSession`，顺序处理命令，并把执行或逐行读取错误转为字符串回复。
- `TimerSessionContext::query(sql)`：同步发命令、等回复、维护事务标志，再逐行调用 `decode_row`。这是本文件所有文本 SQL 的统一入口。
- `impl SessionContext for TimerSessionContext`：`execute` 返回一个装有 `SqlResult` 的类型擦除结果集；`execute_internal` 先调用 `bind_parameters`；`rollback_transaction` 复用 `query("ROLLBACK")`；生命周期方法负责池协议与关闭。
- `impl Drop for TimerSessionContext`：兜底调用 `close`，保证发送停止消息并 join worker。
- `new_ttl_timer_session_pool(domain, capacity)`：本文件唯一公开函数。它捕获 `Arc<Domain>`，将 context 工厂交给 `NewAdvancedSessionPool`，返回 `Arc<AdvancedSessionPool>`。
- `bind_parameters(sql, args)`：私有参数绑定器，只接受实际类型为 `SqlArg` 的 `syssession::SqlValue`。
- `decode_row(sql, row)`：私有结果解码器，依据 SQL 形状和列下标恢复 TimerStore 所需类型。

## 执行流程

生产主流程如下：

1. `ttl_runtime.rs` 的 owner 循环调用 `new_ttl_timer_session_pool(..., 8)`，把池交给 `NewTableTimerStore`。
2. `TableTimerStoreCore::with_session` 从池借出 session，先执行 `ROLLBACK`，读取原时区并将 session 时区设为 UTC。
3. TimerStore 的 SQL builder（`pkg/timer/tablestore/sql.rs`）生成带 `%?` 的 SQL 和 `Vec<SqlArg>`；`syssession::Session` 最终调用本 context 的 `execute_internal`。
4. `bind_parameters` 从左到右消费参数与 `%?`。普通值被转换成安全的 SQL 字面量；如果占位符恰在 `FROM_UNIXTIME(...)` 内，则先验证参数是 Unix 秒、用 `chrono::DateTime::from_timestamp` 转成 `%Y-%m-%d %H:%M:%S`，再去掉函数外壳并写入引号字符串。
5. `query` 创建容量为 1 的回复通道，经 `Command::Execute` 把完整 SQL 交给专属线程，并阻塞等待结果。
6. worker 调用 `ConcreteSession::execute`，遍历所有 record set，并通过 `next_row` 收集所有字符串行；任一执行或取行错误都会作为 `Err(String)` 回传。
7. `query` 在 SQL 成功后根据开头的 `BEGIN`、`COMMIT` 或 `ROLLBACK` 更新 `transaction_open`，然后用 `decode_row` 生成 `SqlRow`。
8. `decode_row` 先处理通用表示：`<nil>` 为 NULL，二进制前缀为 bytes；再按 TimerStore 的固定查询形状恢复字段类型：完整 19 列 timer 查询、更新前的 `EVENT_ID/VERSION` 查询、`@@LAST_INSERT_ID` 与 `ROW_COUNT()` 标量查询。
9. TableTimerStore 完成业务操作后回滚并恢复原时区；`AdvancedSessionPool::Put` 再检查未决事务并调用 context 的回滚/重置协议，决定复用或关闭 session。

关闭流程是：TimerStore 关闭 notifier；池的 `Close` 拒绝新租约并清空空闲 session；每个 `TimerSessionContext::close` 发送 `Stop`、取出并 join worker。`Drop` 重复调用是安全的，因为 `thread.take()` 只会成功一次。

## 数据与状态

- `Domain` 以 `Arc` 被池工厂捕获；每次工厂调用都会克隆它并创建独立 `TimerSessionContext`。
- 每个 context 恰有一个 `ConcreteSession`，它只存在于 worker 闭包内，不跨线程来回移动，也不会被两个租约同时使用。
- `mpsc::Sender<Command>` 是无界命令通道；每条执行命令自带一个容量为 1 的同步回复通道，回复值为 `Result<Vec<Vec<String>>, String>`。
- `transaction_open` 不是数据库事务对象，只是根据成功 SQL 前缀维护的本地布尔镜像。它供 `AdvancedSessionPool::Put` 的 `has_pending_transaction` 检查使用。
- 完整 timer 行的类型恢复与 `pkg/timer/tablestore/sql.rs::buildSelectTimerSQL` 的 19 列顺序绑定：下标 0（ID）和 18（VERSION）为 `U64`，9（ENABLE）为 `I64`，8/14/16/17（WATERMARK、EVENT_START、CREATE_TIME、UPDATE_TIME）为时间戳，10（TIMER_EXT）为 JSON，3/13/15（TIMER_DATA、EVENT_DATA、SUMMARY_DATA）为 bytes，其余保持字符串。
- 更新约束查询 `SELECT EVENT_ID, VERSION, ...` 仅把下标 1 的 VERSION 转为 `U64`；`SELECT @@LAST_INSERT_ID` 和 `SELECT ROW_COUNT()` 的首列也转为 `U64`，下游 `SqlRow::i64` 允许从可表示的 `U64` 窄化。

## 依赖与调用关系

上游：

- `pkg/session/runtime.rs` 声明该模块；测试模块仅在 `cfg(test)` 下装配。
- `pkg/session/runtime/ttl_runtime.rs` 是唯一生产调用者。RustCodeGraph 对 `new_ttl_timer_session_pool` 的节点追踪也给出从该文件导入的上游 trail，源码调用点位于创建 `NewTableTimerStore` 之前。
- `pkg/session/runtime/ttl_timer_store_test.rs` 直接构造容量 2 的池，覆盖真实 SQL 的 Create/List/Update/Delete/Watch/Close 往返。

下游：

- `ConcreteSession::new`、`execute`：执行真实 session SQL 并产出字符串行。
- `quote_argument`：对字符串与 JSON 参数生成 SQL 字面量，避免本文件自行重复转义规则。
- `astersql_session_syssession::{SessionContext, AdvancedSessionPool}`：定义上下文契约、独占所有权、租借归还、事务清理和池关闭行为。
- `astersql_timer_tablestore::{SqlArg, SqlCell, SqlResult, SqlRow}`：定义通用 TimerStore 与 session 之间的参数和结果协议。
- `chrono`：验证 Unix 秒范围并解析/格式化无时区的 SQL DATETIME 文本。
- `std::sync::mpsc` 与 `std::thread`：实现线程隔离和同步请求/响应。

本文件不直接依赖 etcd notifier；notifier 由 `ttl_runtime.rs` 传给 `NewTableTimerStore`，存储操作成功后的事件发布发生在 `pkg/timer/tablestore/store.rs`。

## 错误处理与边界

- worker 创建失败会包装为 `SessionError`，使池工厂取 session 失败。
- 命令通道关闭时报 `TTL timer SQL worker closed`；worker 未回复时报 `TTL timer SQL worker dropped response`；SQL 执行与 `next_row` 错误保留其字符串消息。
- 参数多于 `%?`、少于 `%?`、参数不是 `SqlArg`、`FROM_UNIXTIME` 收到非整数或越界整数时均显式失败。`U64` 时间先用 `i64::try_from` 检查范围。
- `decode_row` 的数值解析、时间解析、UTF-8/十六进制解析错误都转为 `SessionError`。NULL 依赖运行时约定字符串 `<nil>`；bytes 依赖 `BINARY_PREFIX`。
- `execute_statement`、`parse_with_params`、`exec_restricted_statement` 和 `exec_restricted_sql` 总是返回“不支持”错误。这是有意缩小的适配面，不能把该 context 当作通用系统 session。
- SQL 类型恢复依赖规范化查询前缀和固定列顺序；改变 TimerStore 的 SELECT 列表、大小写/前导形式或新增查询形状时，必须同步审查 `decode_row`。未知查询默认把非 NULL、非二进制值保留为 `SqlCell::String`，下游若期待其他类型会得到类型错误。
- 二进制十六进制解码以两个字符为一组；当前生产编码器保证偶数长度。若未来允许外部构造此前缀，应该增加奇数长度与非法字符的明确验证。
- `transaction_open` 只在 SQL 执行和接收成功后更新；新增复合语句或不同事务语法时，不能假设当前前缀判断自动覆盖。

## 并发与资源生命周期

线程隔离是本文件存在的核心原因。`ConcreteSession` 在 worker 线程内构造并始终留在该线程；调用者只持有通道。单个 context 的命令由 receiver 串行处理，所以一次租约中的事务语句有严格顺序。池可以创建多个 context，因此不同租约可并行执行，但每个租约仍绑定自己的 worker/session。

`AdvancedSessionPool` 用独占 owner 转移与内部锁保证一个 session 同时只属于池或一个租约。归还时，如果 context 报告存在事务、回滚失败、重置失败、被标记不可复用、池已关闭或空闲队列已满，底层 session 会被关闭而不是重新入池。`TimerSessionContext::register_internal_session` 返回 `true`，但 owner 回调和注册/注销方法本身不做额外状态操作。

`close` 忽略发送 `Stop` 与 `join` 的错误，以确保清理路径不覆盖原业务错误；正常情况下 `Stop` 让 worker 跳出循环，随后 join 回收线程。若所有 sender 都提前消失，`receiver.recv()` 返回错误，worker 也会自然退出。调用方必须关闭 pool；`ttl_runtime.rs` 将 pool 与 store 保存在 timer runtime 资源中，测试则显式调用 `store.Close()` 和 `pool.Close()` 并验证关闭后 `Get()` 失败。

## 与 Go 版本的对应关系

Go 仓库没有与本文件同路径的一一对应实现。Go 的 `pkg/ttl/ttlworker/job_manager.go::jobLoopWithSession` 直接把 `m.sessPool`（Domain 提供的 `AdvancedSysSessionPool`）传给 `pkg/timer/tablestore/store.go::NewTableTimerStore`；Go session 原生实现 `syssession` 所需接口，无需额外线程和字符串类型桥接。

Rust 的对应主链保持了 Go 语义：owner 生命周期内创建 table timer store，数据库/表为 `mysql.tidb_timers`，存储层借用系统 session，先回滚、切换 UTC，执行操作后回滚并恢复时区；更新使用事务和 VERSION 约束，成功的 Create/Update/Delete 触发 notifier。Rust 本文件是为现有 `ConcreteSession` 接口补上的局部接线，不替代 `pkg/timer/tablestore/store.rs` 对 Go `store.go` 的主体移植。

参数与结果语义也对齐 Go SQL builder：`%?` 是内部占位符，时间参数使用 `FROM_UNIXTIME`，bytes 不丢失，NULL/整数/JSON/时间按 Go 行读取期望恢复。Rust 的差异是先生成完整 SQL 文本再执行，并以查询前缀和下标恢复类型；Go 由原生 session 参数绑定与 typed row getter 完成这些工作。因此扩展 SQL 时，Rust 侧需要额外维护 `bind_parameters`/`decode_row` 的协议。

相关 Go 证据包括 `pkg/timer/tablestore/store.go`、`pkg/timer/tablestore/sql.go`、`pkg/ttl/ttlworker/job_manager.go`；Go 的广泛行为测试在 `pkg/timer/store_intergartion_test.go` 和 `pkg/timer/tablestore/sql_test.go`，而本适配层的 Rust 定向回归位于独立文件 `pkg/session/runtime/ttl_timer_store_test.rs`。

## 扩展指南

- 新增 `SqlArg` 变体时，必须在 `bind_parameters` 增加显式、可转义的映射，并在独立测试文件中覆盖正常值、边界值和非法类型；不要回退到 Debug/Display 拼接。
- 修改 `buildSelectTimerSQL` 的列或顺序、增加依赖 typed getter 的查询时，必须同步更新 `decode_row` 的查询识别与列映射，并同时检查 `pkg/timer/tablestore/store.rs`、`sql.rs` 及其独立测试。
- 新增事务语法或允许多语句 SQL 时，应扩展 `transaction_open` 的状态机，并验证池归还时不会复用仍有未决事务的 session。
- 如果要支持 parsed/restricted API，应在 `TimerSessionContext` 中完整实现相应 `SessionContext` 方法，并补独立测试；不能仅把错误改为成功桩，因为池和调用方依赖真实执行/资源语义。
- 调整并发模型时必须保留“同一租约、同一 `ConcreteSession`、同一线程、语句有序”的不变量，并验证关闭、worker 异常、通道断开、池满和池关闭竞态。
- 二进制编码协议变更应同时修改 `pkg/session/runtime/row_codec.rs` 与本文件，并保留包含 `0x00`、`0xff` 等非文本字节的往返测试。
- 测试逻辑继续放在 `pkg/session/runtime/ttl_timer_store_test.rs`，不要内嵌到生产文件。优先扩展现有真实 SQL roundtrip，另以小型独立用例覆盖参数数量、时间越界、奇数十六进制、错误 SQL 形状和事务清理。
- 兼容风险主要是 Go SQL/typed-row 语义漂移；性能风险主要是每个池 session 一个 OS 线程、无界命令通道、以及每次查询的字符串物化与类型二次解析。扩容前应以实际 TTL 并发量和池容量评估线程数与内存。

## 验证依据

- 目标源码：`pkg/session/runtime/ttl_timer_store.rs`，核对了全部常量、枚举、结构体、impl、公开函数及无条件编译事实。
- 模块与调用入口：`pkg/session/runtime.rs`、`pkg/session/runtime/ttl_runtime.rs`；生产调用点以容量 8 创建池并接入 `NewTableTimerStore`。
- crate 边界：`pkg/session/Cargo.toml`，核对 `astersql-session` 包及 domain、syssession、timer-tablestore、chrono 依赖。
- 下游契约：`pkg/session/syssession/pool.rs`、`pkg/session/syssession/session.rs`、`pkg/timer/tablestore/store.rs`、`pkg/timer/tablestore/sql.rs`、`pkg/session/runtime/row_codec.rs`。
- Rust 回归：`pkg/session/runtime/ttl_timer_store_test.rs`，覆盖真实 SQL Create/List/Update/Delete、watch 事件、bytes 与 timestamp 往返、store/pool 关闭；测试与生产文件分离。
- Go 对照：`pkg/ttl/ttlworker/job_manager.go`、`pkg/timer/tablestore/store.go`、`pkg/timer/tablestore/sql.go`、`pkg/timer/store_intergartion_test.go`、`pkg/timer/tablestore/sql_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query` 唯一定位 `new_ttl_timer_session_pool`、`TimerSessionContext` 及目标文件内的 `bind_parameters`、`decode_row`；`node new_ttl_timer_session_pool` 给出 `ttl_runtime.rs` 的上游 trail。按原始文件路径的 `files/explore` 未准确收敛，因此调用细节以精确符号节点和上述直接源码证据补足。
- 本任务是纯文档分析，按任务约束未运行 Cargo、代码测试或构建；交付验证只检查文档结构、范围与事实可追溯性。
