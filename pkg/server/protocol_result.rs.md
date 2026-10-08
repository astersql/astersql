# `pkg/server/protocol_result.rs`

## 文件定位

本文件是 `astersql-server` crate 内部的结果集线程边界适配层，由 [`pkg/server/lib.rs`](lib.rs) 以私有模块 `protocol_result` 挂载。它位于会话执行与网络协议编码之间：`pkg/server/runtime.rs::run_session_worker` 拥有真正的 `ConcreteRecordSet`，而网络连接线程只持有实现 `pkg/server/conn.rs::ProtocolResultSet` 的 `ResultHandle`。两端通过 `SessionRequest::ResultOperation` 通信，因而 `chunk::Row`、惰性游标和底层结果集不会离开其会话工作线程。

`pkg/server/Cargo.toml` 将该边界所需的 `astersql-session`、`astersql-server-internal-resultset`、`astersql-server-internal-column`、`astersql-util-chunk` 和 `astersql-util-sqlexec` 都声明为同一 workspace 内的路径依赖；该文件不是独立 crate，也没有自己的 feature 开关。

## 核心职责

- `ResultHandle` 把协议线程的取块、游标取行、推进、完成通知、抓取完成通知和关闭动作转换成同步的 `SessionRequest::ResultOperation` 请求。
- `CanonicalResultSet` 把 `astersql_session::runtime::ConcreteRecordSet` 适配成 `astersql_server_internal_resultset::ResultSet`，以便复用服务器已有的 chunk 与惰性游标实现。
- `WorkerResult` 在同一个底层结果集上按需选择普通 chunk 读取或 `WrapWithLazyCursor` 游标读取，并确保一旦进入游标模式，后续生命周期操作仍落在同一包装对象上。
- `WorkerResults` 在会话工作线程中登记多个活动结果集、分配句柄 ID、执行操作并在关闭时移除所有权。
- `owned_row` 与 `runtime.rs::protocol_value` 把工作线程中的 chunk 行复制为协议层拥有的 `Vec<Value>`，保留 NULL 与二进制哨兵的协议语义。

该模块不负责 SQL 执行、MySQL 包编码或 prepared statement 状态管理；这些职责分别由 `pkg/server/runtime.rs`、`pkg/server/conn.rs` 和 `pkg/server/conn_stmt.rs` 承担。

## 主要符号

- `Operation`：crate 内可见的操作枚举。`Chunk` 对应普通结果集批量读取；`Current`/`Advance` 对应服务端游标的查看与推进；`Finish`、`FetchReturned`、`Close` 对应协议生命周期边界。
- `OperationResult`：工作线程的返回联合体。`Rows(Vec<Vec<Value>>)` 返回一个 chunk，`Row(Option<Vec<Value>>)` 用 `None` 表示游标结束，`Done` 表示无数据返回的操作成功。
- `ResultHandle { id, sender, closed }`：网络线程可共享的轻量句柄。`sender` 是会话请求通道的强引用，`closed: AtomicBool` 令显式关闭和析构关闭幂等。
- `ResultHandle::request`：为每次操作创建容量为 1 的同步响应通道，发送带 `id` 的请求并等待工作线程答复；发送或接收通道断开都映射为 `ConnError::Io`。
- `impl ProtocolResultSet for ResultHandle`：把六个协议侧方法映射到相应的 `Operation`；返回变体与操作不匹配被视为内部不变量破坏并触发 `unreachable!()`。
- `CanonicalResultSet`：持有 `ConcreteRecordSet`、列元数据、关闭标志和 chunk 大小。测试构建下额外持有 `BoundaryProbe`，其定义位于独立文件 `pkg/server/protocol_result_test_support.rs`。
- `impl resultset::ResultSet for CanonicalResultSet`：提供 `Columns`、`FieldTypes`、`NewChunk`、`Next`、`Close`、`Finish`、`TryDetach`、`OnFetchReturned` 等 Go 风格接口。`TryDetach` 固定返回 `(None, false)`，RU v2 tracker 钩子当前为空实现。
- `WorkerResult { regular, cursor, init_chunk_size, max_chunk_size }`：同一结果集的普通形态与可选惰性游标形态。`cursor()` 首次调用时消费 `regular` 并创建惰性游标；`source()` 总是返回当前有效形态。
- `WorkerResults { results, next_id, sender }`：工作线程拥有的活动结果表。`register` 构造元数据和句柄，`operate` 是所有跨线程结果操作的唯一执行入口。
- `session_error`、`owned_row`：分别统一把下游可显示错误映射为 `ConnError::Session`，以及把 chunk 行复制并规范化为协议值。

## 执行流程

1. `runtime.rs::run_session_worker` 创建 `WorkerResults::new(result_sender)`，使结果注册表与 `ConcreteSession` 位于同一工作线程。
2. `runtime.rs::execute_on_session` 或 `execute_prepared_on_session` 在 streaming 路径得到 `ConcreteRecordSet` 后调用 `WorkerResults::register`。该方法先升级弱请求发送端；若连接上下文已关闭，返回 `ConnError::Session("session is closed")`。
3. `register` 通过 `runtime.rs::result_metadata` 生成协议列与原生类型，再把协议列复制成 `astersql_server_internal_column::Info`；随后递增 `next_id`，将 `CanonicalResultSet` 放入 `results`，并把 `Arc<ResultHandle>` 写入返回的 `QueryResult::result_set`。
4. 普通查询写出时，`conn.rs::write_result_chunks` 调用 `ResultHandle::next_chunk`。工作线程的 `operate(Chunk)` 在当前 `ResultSet` 上创建 chunk，调用 `Next`，再逐行执行 `owned_row`。`CanonicalResultSet::Next` 先读取 `RequiredRows`、重置 chunk，然后最多拉取该数量的 canonical 行并逐列 `AppendString`。
5. 非游标结果读到空 chunk 后，协议线程调用 `finish`、写 EOF，再调用 `close`。工作线程先执行 `Finish`；`Close` 则从 `HashMap` 移除结果并调用底层 `Close`。
6. prepared cursor 路径由 `conn.rs::write_prepared_cursor_fetch` 调用 `current_row`/`advance`。首次游标操作触发 `WorkerResult::cursor()`，用配置的初始/最大 chunk 大小创建 `WrapWithLazyCursor`；`Current` 或 `Advance` 调用其行迭代器，并在返回前检查 `iter.Error()`。
7. 每次 `COM_STMT_FETCH` 的行写出完成后，协议线程调用 `on_fetch_returned`；耗尽时设置 `SERVER_STATUS_LAST_ROW_SENT`、清除 cursor-exists 状态，并关闭结果集。相关次序由 `conn_stmt_test.rs` 的事件断言覆盖。

## 数据与状态

- 结果所有权保存在工作线程的 `HashMap<u64, WorkerResult>` 中；协议侧的 `ResultHandle` 只保存 ID 和通道，不直接借用底层对象。
- `next_id` 从 0 开始、注册前递增，因此本进程内第一个句柄 ID 为 1。当前代码没有溢出处理；这是长期运行且累计注册达到 `u64::MAX` 才可能触发的理论边界。
- `CanonicalResultSet::columns` 保存原始列元数据；`FieldTypes` 则有意统一构造 `TypeVarString`，因为 canonical 行单元在此边界是文本/哨兵表示，真正的 MySQL wire 元数据仍由 `QueryResult::columns` 驱动。
- `init_chunk_size` 与 `max_chunk_size` 来自 `runtime.rs::protocol_chunk_sizes`，默认分别为 32 和 1024；普通读取创建 chunk、惰性游标缓冲都使用这两个值。
- `CanonicalResultSet::closed` 与 `ResultHandle::closed` 是两级状态：前者保证底层 `ConcreteRecordSet::close` 至多执行一次，后者保证一个句柄只发送一次关闭请求。
- `protocol_value` 将 canonical NULL 哨兵及旧式 `"<nil>"` 转为 `Value::Null`，并识别二进制十六进制前缀；因此 `owned_row` 不应自行推断数值类型。

## 依赖与调用关系

上游调用链为：

`ConcreteSession::execute` / `execute_protocol_statement` → `runtime.rs::{execute_on_session, execute_prepared_on_session}` → `WorkerResults::register` → `QueryResult::result_set(ResultHandle)`。

协议消费链有两条：

- 文本或非游标二进制结果：`conn.rs::write_result_chunks` → `ProtocolResultSet::{next_chunk, finish, close}` → `ResultHandle::request` → `run_session_worker` → `WorkerResults::operate` → `CanonicalResultSet`。
- prepared cursor：`conn.rs::write_prepared_cursor_fetch` → `current_row` / `advance` / `on_fetch_returned` / `close` → 同一请求链 → `WorkerResult::cursor` → `resultset::WrapWithLazyCursor`。

主要下游依赖是 `ConcreteRecordSet::{next_row, close}`、`resultset::{ResultSet, CursorResultSet, WrapWithLazyCursor}`、`chunk::{Chunk, Row}`、`sqlexec::{RecordChunk, Context, GoError}`，以及 `runtime.rs::{result_metadata, protocol_value}`。RustCodeGraph 对目标文件给出的直接使用者包括 `pkg/server/conn.rs`、`pkg/server/conn_stmt.rs`、`pkg/server/conn_stmt_test.rs`、`pkg/server/conn_test.rs` 等；精确接线由上述 `SessionRequest::ResultOperation` 和 `ProtocolResultSet` 引用进一步核对。

## 错误处理与边界

- 请求发送端或响应接收端断开时，`ResultHandle::request` 返回 `ConnError::Io`；这表示跨线程传输失败，而不是 SQL 执行错误。
- 工作线程不存在指定 ID 时，除 `Close` 外的操作返回 `ConnError::Session("result set is closed")`。`Close` 对未知或已移除 ID 返回成功，因此关闭幂等。
- `CanonicalResultSet::Next` 的 `ConcreteRecordSet::next_row` 错误通过 `sqlexec::GoError` 传播，再由 `operate(Chunk)` 的 `session_error` 转为 `ConnError::Session`。游标路径把 `RowIterator::Error` 直接转换成同类 session 错误。
- `Finish` 错误会传播；`OnFetchReturned` 没有返回值；`CanonicalResultSet::Close` 有意忽略底层 `close()` 的错误，因为 `resultset::ResultSet::Close` 接口本身无返回值。
- `Drop for ResultHandle` 绝不等待响应，只尽力发送 `Close`。这样可避免工作线程停机期间析构阻塞；但通道已断开时清理请求可能被丢弃，随后工作线程及其 `HashMap` 会随会话一起释放。
- `WorkerResult::cursor` 和 `source` 中的 `expect("result set")` 依赖内部状态不变量：`regular` 与 `cursor` 不会同时为空。`OperationResult` 的 `unreachable!()` 同样只防御模块内部协议失配，不处理外部输入。
- `CanonicalResultSet::Next` 按行枚举并按位置追加单元；它依赖 canonical 行宽与 `columns`/chunk 字段数一致，该文件没有额外修复或截断逻辑。

## 并发与资源生命周期

底层结果集只由 `run_session_worker` 的接收循环访问。即使多个协议侧调用者共享 `Arc<ResultHandle>`，所有操作也会先进入同一个 `mpsc::Sender<SessionRequest>`，再由单一 receiver 串行调用 `WorkerResults::operate`；这正是该模块存在的核心线程安全边界。

每个调用使用独立的容量 1 `sync_channel` 等待结果，形成请求/响应同步点。主请求通道本身是普通 `mpsc::Sender`；同一 sender 克隆的发送顺序与工作线程逐条接收共同保证正常生命周期中的操作次序。源码特别依赖 `Drop` 中的 Close 请求先进入该 FIFO，再进入响应生命周期的语句完成请求。

显式 `ResultHandle::close` 使用 `AtomicBool::swap(Ordering::AcqRel)` 竞争关闭所有权；只有第一个调用者会等待工作线程确认。析构走相同原子门闩但不等待。工作线程收到 Close 后先从表中移除再调用底层 Close，因此后续请求无法重新取得该对象。`CanonicalResultSet` 自身的 `Drop` 再调用一次幂等 Close，覆盖异常退出或容器整体销毁。

测试专用 `BoundaryProbe` 用 `Arc<Mutex<_>>` 记录 `Next`、`Finish`、`FetchReturned`、`Close` 的顺序并注入延迟/失败；生产构建不包含该字段或模块。不存在后台任务、异步 runtime、锁跨越底层结果操作或由本文件管理的事务。

## 与 Go 版本的对应关系

仓库中没有同路径 `pkg/server/protocol_result.go`；Rust 文件是为“会话工作线程拥有 canonical record set、协议线程消费 owned values”新增的桥接层。其复用的语义来自以下 Go 实现：

- `pkg/server/internal/resultset/resultset.go::ResultSet` 定义 `Columns/NewChunk/Next/Close/IsClosed/FieldTypes/Finish/TryDetach`；`CanonicalResultSet` 实现同一生命周期接口。Go `tidbResultSet` 用 `finishLock` 允许外部 goroutine 协调 Finish/Close，而 Rust 通过单工作线程消息串行化达到对应互斥目的。
- `pkg/server/internal/resultset/cursor.go::WrapWithLazyCursor` 与 `lazyRowIterator` 是 Rust `WorkerResult::cursor` 直接复用的行为模型：`Current` 首次触发读取，`Next` 跨 chunk 补充，空行作为 End，错误保存在 iterator 上供调用者检查。
- `pkg/server/conn.go::writeChunks` 同样要求先 `Next` 再发送列元数据，循环写 chunk，随后 `Finish` 和 EOF；Rust `conn.rs::write_result_chunks` 维持该边界次序，并另行 Close。
- `pkg/server/conn.go::writeChunksWithFetchSize` 在 `COM_STMT_FETCH` 中按 fetch size 使用 `Current/Next`，检查 iterator 错误，更新 cursor 状态，并把 `OnFetchReturned` 排除在写响应耗时外；Rust `write_prepared_cursor_fetch` 与 `Operation::{Current,Advance,FetchReturned}` 对齐这些行为。
- Go 的 `TryDetach` 可为部分结果集启用真正的 lazy execution；当前 `CanonicalResultSet::TryDetach` 明确返回不支持。RU v2 tracker 的 Rust 钩子也为空。因此这两项是已知迁移差异，不应描述为已支持。

## 扩展指南

- 新增跨线程结果操作时，应同时修改 `Operation`、必要的 `OperationResult`、`ProtocolResultSet`（若是协议公共能力）、`ResultHandle` 映射和 `WorkerResults::operate`；必须保持请求与返回变体一一对应，并在 `pkg/server/conn_stmt_test.rs` 或相邻独立测试文件中覆盖成功、错误和关闭后的行为。
- 修改 chunk 填充或值转换时，优先改 `CanonicalResultSet::Next` / `owned_row`，并同步核对 `runtime.rs::protocol_value`、列数不变量、NULL/二进制哨兵和文本/二进制两种 wire encoder。不要依据字符串内容猜测 native 数值类型。
- 增加底层生命周期钩子时，应判断它属于普通 source 还是游标 wrapper，并通过 `WorkerResult::source` 保持包装前后调用目标一致；对 Go `ResultSet`/`FetchNotifier` 的次序差异要增加事件断言。
- 若实现 `TryDetach`、prepared statement 归属或 RU v2 tracker，不能只填充空方法；需要对照 Go 的所有权转移、失败后结果集是否仍可用、tracker attach/report 时点，并增加独立 Rust 回归测试。
- 任何会阻塞的逻辑都不应放入 `ResultHandle::drop`。调整通道或关闭协议时须验证连接关闭、结果提前丢弃、工作线程先退出以及多个 `Arc` 同时释放的路径，避免死锁和资源滞留。
- 性能敏感点是每个协议操作一次通道往返、每个单元一次字符串复制/转换，以及 cursor 的逐行 `Current`/`Advance` 请求。批处理优化必须保留 Go 可观察的 fetch 边界、错误出现位置和事件次序。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件完整读取为 357 行。执行了 `status`、`files --filter pkg/server`、目标文件 `node --file`、符号 `query`、`callers/callees` 及聚焦 `explore`。通用名称的图查询噪声较大，因此用精确引用搜索补齐目标调用边。
- 目标与直接 Rust 证据：`pkg/server/protocol_result.rs`；`pkg/server/runtime.rs::{SessionRequest,run_session_worker,execute_on_session,execute_prepared_on_session,result_metadata,protocol_value}`；`pkg/server/conn.rs::{ProtocolResultSet,QueryResult,write_prepared_cursor_fetch,write_result_chunks}`；`pkg/server/lib.rs`；`pkg/server/Cargo.toml`。
- 独立测试证据：`pkg/server/protocol_result_test_support.rs::BoundaryProbe`；`pkg/server/conn_stmt_test.rs::{next_and_finish_are_excluded_and_errors_preserved,cursor_iteration_is_timed_and_notification_excluded,cursor_iterator_error_is_accounted,multichunk_next_failure_preserves_written_rows,empty_results_and_successful_flush_are_accounted,worker_releases_domain_when_context_is_dropped}`；`pkg/server/conn_test.rs::go_merge_139_cursor_consumer_only_synchronizes_response_bytes`。未发现同名 `protocol_result_test.rs`，测试逻辑没有写入生产源文件。
- Go 对照证据：`pkg/server/internal/resultset/resultset.go::{ResultSet,tidbResultSet}`；`pkg/server/internal/resultset/cursor.go::{CursorResultSet,WrapWithLazyCursor,lazyRowIterator,FetchNotifier}`；`pkg/server/conn.go::{writeResultSet,writeChunks,writeChunksWithFetchSize}`；`pkg/server/conn_stmt.go::{executeWithLazyCursor,writeExecuteResultWithCursor,handleStmtFetch}`。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收使用任务指定命令，要求文档存在且恰好包含上述 11 个固定二级标题。
