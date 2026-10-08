# `pkg/testkit/asynctestkit.rs`

## 文件定位

`pkg/testkit/asynctestkit.rs` 属于 `astersql-testkit` crate；crate 根由 `pkg/testkit/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/testkit/lib.rs` 通过 `pub mod asynctestkit` 暴露本模块。它是测试基础设施而不是数据库服务主链的一部分：调用者把一个实现了 `Database` 的存储或会话工厂交给 `NewAsyncTestKit`，随后通过线程安全的命令发送端驱动一个只驻留在 worker 线程中的 `TestKit`。

当前可见的生产式使用点是 `pkg/executor/write_concurrent_test.rs::batch_insert_with_on_duplicate`：每个外层并发线程各构造一个 `AsyncTestKit`，从而获得相互独立的测试会话。本文件自身不创建 Tokio runtime，也不暴露 Rust `async fn`；名称中的“异步”具体指“SQL 在独立 OS 线程执行，并通过通道请求/响应”。

## 核心职责

- `AsyncTestKit::new` 建立一个多生产者、单消费者的 `std::sync::mpsc` 命令通道，启动名为 `testkit-worker` 的线程，并在线程内创建唯一的 `TestKit`。
- `Exec` 与 `Query` 将拥有所有权的 SQL 字符串、参数和一次性回复发送端装入 `Command`，再同步等待 worker 返回。这样 `TestKit` 不需要跨线程共享或加锁，同一实例的数据库操作保持串行。
- `MustExec` 与 `MustQuery` 把可恢复的 `TestResult` 转为带 SQL 上下文的 panic，供“必须成功”的测试调用。
- `Sync` 发送 FIFO 屏障，确认 worker 已处理通道中排在屏障之前的命令。
- `Drop` 发送关闭命令、等待 worker 退出，并由 worker 调用 `TestKit::Session().close()` 释放派生会话。

它刻意只提供执行、查询、屏障和关闭这组小接口；Go 版本中的 context 会话管理、批量并发编排和错误消息断言目前不在此 Rust 文件中。

## 主要符号

- `enum Command`：模块私有协议。
  - `Execute { sql, args, reply }` 返回 `TestResult<()>`；底层 `ExecutionResult` 被丢弃。
  - `Query { sql, args, reply }` 返回 `TestResult<Result>`；worker 先把 `QueryRows` 转成字符串矩阵。
  - `Barrier(Sender<()>)` 只确认此前命令已按接收顺序完成。
  - `Close` 终止接收循环。
- `pub struct AsyncTestKit`：保存可克隆但未公开的 `commands: Sender<Command>` 和只允许取走一次的 `worker: Option<JoinHandle<()>>`。类型没有实现 `Clone`；对外方法都只需 `&self`，因此可在外层用 `Arc<AsyncTestKit>` 共享发送入口，但所有 SQL 仍由单 worker 串行执行。
- `AsyncTestKit::new(database: Arc<dyn Database>) -> Self`：实际构造器。`Database: Send + Sync + 'static` 的边界定义在 `pkg/testkit/db_driver.rs`，满足把输入移入新线程的要求。
- `AsyncTestKit::{Exec, Query}`：返回错误的基础 API。二者每次调用都新建回复通道，因此响应不会串线。
- `AsyncTestKit::{MustExec, MustQuery}`：断言式包装；失败消息包含 `sql={sql:?}`。
- `AsyncTestKit::Sync`：显式屏障。由于当前 `Exec`/`Query` 已经等待各自响应，它常用于表达顺序点，也为以后增加不等待响应的命令保留清晰同步语义。
- `impl Drop for AsyncTestKit`：生命周期收口点，顺序为发送 `Close`、取走 `JoinHandle`、`join`。
- `NewAsyncTestKit`：保持 Go 风格命名的公开自由函数，直接委托 `AsyncTestKit::new`；模块由 `lib.rs` 公开，但该函数未在 crate 根再次 `pub use`，调用路径通常是 `astersql_testkit::asynctestkit::NewAsyncTestKit`。

## 执行流程

1. 调用者执行 `NewAsyncTestKit(database)`，进入 `AsyncTestKit::new`。
2. 构造器创建命令通道并启动 `testkit-worker`。worker 调用 `TestKit::new(database)`；后者在 `pkg/testkit/testkit.rs` 中先安装 planner expression factory，再调用 `Database::create_session()`，若返回 `Some` 则使用派生会话，若返回 `None` 则沿用输入对象，创建错误会 panic。
3. `Exec`/`Query` 把 `&str` 复制为 `String`，连同 `Vec<DbValue>` 和专属回复发送端写入命令队列；调用线程随后阻塞在回复接收端。
4. worker 的 `receiver.recv()` 循环一次只处理一个命令。`Execute` 调用可变 `TestKit::Exec` 并把成功值映射为 `()`；`Query` 调用 `TestKit::Query`，再经 `QueryRows::string_rows` 和 `Result::new` 规范化结果。
5. worker 尝试发送回复。若请求方已离开，`reply.send(...)` 的错误被忽略；worker 继续处理后续命令。
6. `Barrier` 在队列轮到它时发送空 ACK。`Close` 则跳出循环。
7. 循环结束后，worker 无论是收到 `Close` 还是所有命令发送端断开，都会调用 `testkit.Session().close()`；关闭失败会在 worker 中 panic。外层 `Drop` 等待该线程结束，但当前忽略 `join` 返回的 panic 载荷。

`pkg/executor/write_concurrent_test.rs::batch_insert_with_on_duplicate` 展示了跨会话并发方式：外层启动多个线程，每个线程构造自己的 `AsyncTestKit`，各自 worker 内串行设置会话变量并执行写入；不是让一个 `AsyncTestKit` 同时执行多条 SQL。

## 数据与状态

- 长期状态只有命令发送端和 worker 句柄。实际 `TestKit`、派生数据库会话、最近一次执行结果等都只属于 worker 闭包，不会由调用线程直接访问。
- `Command` 拥有 `String` 与 `Vec<DbValue>`，所以请求入队后不借用调用者数据。`DbValue` 的支持集合由 `pkg/testkit/db_driver.rs` 定义，包括 NULL、布尔、整数、浮点、字节串和字符串。
- 查询数据先由 `QueryRows::string_rows` 使用各 `DbValue` 的 `Display` 转成 `Vec<Vec<String>>`；NULL 是 `<nil>`，字节串采用 UTF-8 lossy 显示。`Result::new` 生成的断言结果没有额外 comment。
- `mpsc::channel()` 是无界队列；本文件没有背压或队列长度指标。不过公开 `Exec`/`Query` 都同步等待，所以普通单调用方最多只有当前请求悬而未决；多调用方共享同一实例时才可能积压。
- `worker: Option<JoinHandle<()>>` 只为在 `Drop` 中通过 `take()` 转移句柄所有权，保证最多 join 一次。

## 依赖与调用关系

- 上游：`pkg/testkit/lib.rs` 声明公开模块；`pkg/testkit/asynctestkit_test.rs::worker_serializes_commands_and_closes_its_session` 直接验证构造、执行、查询、屏障和析构；`pkg/executor/write_concurrent_test.rs::batch_insert_with_on_duplicate` 用它创建并发测试会话。
- 下游：`AsyncTestKit::new → TestKit::new → Database::create_session`，会话创建的真实行为由传入的 `Database` 实现决定。
- 执行链：`AsyncTestKit::Exec → Command::Execute → TestKit::Exec → Database::execute`。
- 查询链：`AsyncTestKit::Query → Command::Query → TestKit::Query → Database::query → QueryRows::string_rows → Result::new`。
- 清理链：`AsyncTestKit::drop → Command::Close → worker loop break → TestKit::Session → TestSession::close → Database::close`。
- 标准库依赖仅为 `Arc`、`mpsc`、`thread::Builder` 和 `JoinHandle`。本文件直接引用的 crate 内抽象来自 `db_driver.rs`、`result.rs`、`testkit.rs` 与 `lib.rs` 中的 `TestError`/`TestResult`；`pkg/testkit/Cargo.toml` 没有为本模块设置 feature gate。

## 错误处理与边界

- 创建 worker 失败会在 `AsyncTestKit::new` 中以 `expect("spawn async testkit worker")` panic；`TestKit::new` 创建派生会话失败也会 panic。这些属于测试基础设施无法建立的致命条件。
- `Exec`/`Query` 将命令发送失败统一映射为 `TestError("async testkit worker stopped")`，将回复通道过早断开映射为 `TestError("async testkit worker dropped response")`；底层数据库错误原样通过 `TestResult` 返回。
- `MustExec`/`MustQuery` 故意 panic，并加入 SQL 文本；参数没有加入 panic 文案，可能限制参数化语句的诊断信息。
- `Sync` 使用 `expect`，因此 worker 停止或屏障响应丢失会 panic，而不是返回 `TestResult`。
- worker 忽略回复发送失败，因为请求方消失不影响会话继续服务其他排队命令；`Drop` 同样忽略发送 `Close` 失败和 `join` 错误。因此，若 worker 已 panic，析构本身不会把原始 panic 再传播到拥有者线程。
- worker 最终的 `Session().close().expect(...)` 保证关闭错误可见于 worker panic，但结合被忽略的 `join` 错误，调用者未必直接观察到该失败。独立测试只覆盖成功关闭一次的路径。
- 不能从 worker 自身线程析构同一个 `AsyncTestKit`（公开 API 没有把所有权送入 worker 的入口）；正常调用下不存在自 join。若以后增加此类回调，必须防止死锁。

## 并发与资源生命周期

通道保证单个 receiver 按接收顺序逐项消费，因而同一 `AsyncTestKit` 的所有 `Execute`、`Query`、`Barrier` 和 `Close` 都在一个 worker 上串行处理。多个生产者同时 `send` 时，每条消息保持完整且会得到唯一回复，但不同生产者之间除实际入队顺序外没有更强的公平性或事务性保证。

`Drop` 是主要资源所有者：先尽力发送 `Close`，再 join worker。由于结构体字段按 `drop` 方法返回后才自动销毁，`commands` 在等待期间仍存活，但队列中的 `Close` 会在它之前已入队的命令处理完后终止循环；排在 `Close` 之后的竞争发送可能得不到处理并收到“响应丢失”。因此，若在多个线程共享外层 `Arc<AsyncTestKit>`，最后一个引用的析构自然发生在其他调用结束后；不应在仍有调用并发进行时人为设计关闭入口。

独立测试中的 `Store::create_session` 返回一个 `Session`，其 `close` 增加原子计数；作用域结束触发 `Drop` 后断言计数恰为 1，同时用互斥向量确认 `insert`、`select` 顺序。这提供了会话派生、命令串行和析构关闭的直接证据。测试没有验证 worker panic、关闭失败、多生产者竞争或通道积压。

## 与 Go 版本的对应关系

共同意图是为并发测试提供隔离会话上的 SQL 执行，并保留 `NewAsyncTestKit`、`Exec`、`MustExec`、`MustQuery` 等 Go 风格入口；Rust 的结果字符串化也延续 Go TestKit 用字符串矩阵做断言的习惯。

实现模型并非逐 API 等价：

- Go `AsyncTestKit` 只保存断言器和 `kv.Storage`，用 `context.Context` 携带每个 goroutine 的 session；Rust 对象拥有一个后台线程和一个派生 `Database` 会话，不接收 context。
- Go 提供 `OpenSession`、`CloseSession`、`ConcurrentRun`、`GetStack`、`MustGetErrMsg`、`ExecToErr` 和 `TryRetrieveSession`；Rust 本文件均未提供。Rust 调用者在 `write_concurrent_test.rs` 中用“每个线程一个 `NewAsyncTestKit`”替代 Go `ConcurrentRun` 的内部编排。
- Go `Exec` 在有参数时显式 prepare/execute/drop prepared statement，并返回可能需要关闭的 `RecordSet`；Rust 将参数交给 `Database::{execute,query}`，是否走 prepared 语义由具体实现决定，查询结果在 worker 内立即物化，不向调用者暴露可关闭的流式结果集。
- Go 的 `MustExec` 负责关闭非空结果集；Rust `Execute` 路径只返回 `ExecutionResult`，没有结果集资源。
- Rust 新增 `Sync` 和 RAII `Drop` 收口，Go 则由 `ConcurrentRun` 的 defer 显式关闭多个 context session。

因此扩展时应以现有 Rust `Database`/`TestKit` 抽象为事实边界；若追求 Go API 对齐，需要逐项验证语义，不能仅按同名方法机械补齐。

## 扩展指南

- 新增 worker 命令时，在 `Command`、worker `match` 和公开发送方法三处同步修改，并为新命令设计专属回复类型，避免共享响应通道造成类型或顺序混淆。
- 若新增真正的 fire-and-forget API，应明确队列容量与背压策略，并用 `Sync` 验证可见性；当前无界通道可能在生产速度持续超过 SQL 执行速度时增长内存。
- 若要公开显式关闭，建议让关闭过程返回 worker panic/`Database::close` 错误，并使后续发送有确定错误；必须保持 `Drop` 幂等，且避免并发发送与关闭的竞态含糊。
- 若补齐 Go 的并发编排，优先在本文件之外建立调度层，让每个并发 worker 持有独立 `AsyncTestKit`/会话；不要让多个逻辑 session 复用当前单个 `TestKit`，除非先扩展命令协议明确会话身份。
- 若补齐 prepared-statement 语义，应先检查具体 `Database` 实现和 `TestSession::{PrepareStmt, ExecutePreparedStmt, DropPreparedStmt}`，并覆盖 prepare 成功后 execute/drop 失败时的资源清理。
- 测试逻辑继续放在独立的 `pkg/testkit/asynctestkit_test.rs`，不要内嵌进生产源文件。至少应同步覆盖：底层执行/查询错误传播、worker 提前停止、关闭失败或 panic 的可观察策略、多生产者序列化，以及任何新增命令的屏障关系；跨模块真实使用可参考 `pkg/executor/write_concurrent_test.rs`。
- 本文件是既有 Rust 源码，已有 `// Copyright 2026 AsterSQL.` 标记；文档任务不修改其许可证或实现。

## 验证依据

- RustCodeGraph `status`：索引包含目标仓库；查询时报告 7,032 个 Rust 文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/testkit/asynctestkit.rs --offset 1 --limit 400`：核对了本文件 161 行全貌、`Command`、`AsyncTestKit`、全部方法、`Drop` 和 `NewAsyncTestKit`。
- RustCodeGraph `explore "pkg/testkit/asynctestkit.rs AsyncTestKit AsyncTestKitConfig"`：识别出 Rust `NewAsyncTestKit` 的独立测试调用者，以及 `Exec`/`Query`/`Sync` 的本文件调用关系；搜索项中不存在 `AsyncTestKitConfig`，说明本文件没有该配置类型。
- RustCodeGraph `node`/`explore` 对 `pkg/testkit/testkit.rs`、`pkg/testkit/db_driver.rs`、`pkg/testkit/result.rs` 和 `pkg/executor/write_concurrent_test.rs` 的结果：核对了 `TestKit::new/Exec/Query/Session`、`Database::create_session/execute/query/close`、`QueryRows::string_rows`、`Result::new` 及真实并发调用点。
- 读取 `pkg/testkit/Cargo.toml` 与 `pkg/testkit/lib.rs`：核对 crate 名、根模块路径、公开模块接线、测试文件的独立 `#[path]` 挂载，以及本模块没有 feature 条件。
- 读取 `pkg/testkit/asynctestkit.go`：核对 Go 的 context/session/`ConcurrentRun`/prepared-result-set 行为与 Rust 当前实现差异。
- 读取 `pkg/testkit/asynctestkit_test.rs`：核对命令执行顺序、查询物化、`Sync` 和析构时恰好关闭一次会话的断言。
- 通过 `rg` 检查 Rust 调用点：确认直接外部使用为 `pkg/executor/write_concurrent_test.rs`，crate 内直接覆盖为 `pkg/testkit/asynctestkit_test.rs`。

