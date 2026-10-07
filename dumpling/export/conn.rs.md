# `dumpling/export/conn.rs`

## 文件定位

`conn.rs` 属于 `astersql-dumpling-export` library crate。它没有单独的 `mod` 边界，而是由 [`dumpling/export/lib.rs`](lib.rs) 在 `retry.rs`、`util.rs` 之后、`sql.rs` 之前通过 `include!("conn.rs")` 拼入 crate 根，因此可以直接使用 crate 根中的 `Conn`、`Rows`、`WithRetry`、`Error`、`Field` 和 `tcontext` 等名字。Cargo 元数据把该 crate 对应到 Go 包 `dumpling/export`；当前 Cargo 注释还明确说明 SQL/MySQL 等能力由本地桩提供，所以这里实际操作的是 [`stubs.rs`](stubs.rs) 中的脚本化连接抽象，并非直接依赖数据库驱动。

在完整导出路径中，本文件位于“业务 SQL/元数据读取”与“连接/重试机制”之间：[`dump.rs`](dump.rs) 创建 `BaseConn` 并用它生产导出元数据，[`sql.rs`](sql.rs) 通过它执行 `SHOW`、`INFORMATION_SCHEMA`、隐式 `_tidb_rowid` 探测等查询；本文件本身不决定要导出的对象，也不生成 SQL。

## 核心职责

- `BaseConn` 把当前连接、一次 SQL 调用可复用的 backoff，以及可选的重建连接回调收拢到一个可变对象中。
- `QuerySQL`/`queryRows` 统一查询、消费结果集、检查行级错误、失败清理、日志、错误注释和重试。
- `QuerySQLWithColumns` 在通用查询循环上叠加按列名投影结果的便利接口。
- `ExecSQL` 把执行结果或执行错误交给调用方回调解释；回调返回值才决定本轮是否成功或进入 `WithRetry`。
- 每个公开操作完成后都调用 `backOffer.Reset()`，使同一个 `BaseConn` 的下一次 SQL 不继承前一次调用消耗的重试预算。

它不负责创建真实连接、判断所有错误是否可重试或关闭 `BaseConn`：错误分类及次数由 [`retry.rs`](retry.rs) 的 backoffer 决定，连接重建由构造时注入的回调决定，最终连接关闭由拥有者处理（例如 `dump.rs` 对 `DBConn.take()` 后调用 `Close`）。

## 主要符号

- `pub struct BaseConn`：包含三个公开字段。`DBConn: Option<Conn>` 保存当前连接；`backOffer: Box<dyn backOfferResettable>` 同时提供 `BackoffStrategy` 和重置能力；`rebuildConnFn: Option<Box<dyn Fn(&Conn, bool) -> Result<Conn> + Send>>` 在后续尝试前生成替代连接。方法都要求 `&mut self`，因为重试会替换连接并改变 backoff 状态。
- `pub fn newBaseConn(conn, should_retry, rebuild) -> BaseConn`：把初始连接包装为 `Some`，用 `newRebuildConnBackOffer(should_retry)` 选择指数退避或单次 noop 策略；只有 `should_retry` 为真时才保留重建回调。允许重试但回调为 `None` 时仍会在原连接上按 backoff 重试。
- `BaseConn::QuerySQL`：逐行接口。它用 `Rows::Next` 驱动游标，每行调用一次 `handle_one_row`，实际的重试与清理由 `queryRows` 完成。
- `BaseConn::queryRows`：结果集级核心入口。回调可以直接读取列元数据或自行遍历结果，因而被 `QuerySQL`、`QuerySQLWithColumns` 以及 `dump.rs::getColumnTypes` 复用。虽然方法声明为 `pub`，其用途仍是 crate 内部的底层组合点。
- `BaseConn::QuerySQLWithColumns`：调用 `GetSpecifiedColumnValuesAndClose`，返回 `Vec<Vec<String>>`；失败重试前清空上一次尝试写入的 `results`。
- `BaseConn::ExecSQL`：执行型接口。`can_retry(&SqlResult, Option<&Error>)` 在 `ExecContext` 成功和失败时都恰好调用一次；执行失败时传入默认 `SqlResult` 和错误引用，执行成功时传入真实结果和 `None`。

本文件没有模块级常量、trait、条件编译项或异步函数。重试常量与 trait 定义在 `retry.rs`，连接和结果集类型定义在 `stubs.rs`。

## 执行流程

查询路径如下：

1. `QuerySQL` 把逐行回调包装成结果集回调，或 `QuerySQLWithColumns` 准备一个可清空的结果容器，然后进入 `queryRows`。
2. `queryRows` 将尝试计数置零，把 `tctx.Done()` 和尝试闭包交给 `WithRetry`。
3. 每轮先增加 `retry_time`；从第二轮起，如果存在 `rebuildConnFn`，就以当前连接和固定参数 `false` 调用它，并用返回值替换 `DBConn`。
4. 当前连接执行 `QueryContext(query)`。成功取得 `Rows` 后运行 `handle_rows`，仅当回调成功时继续检查 `rows.Err()`；随后无论该组合结果成功与否都尝试 `rows.Close()`，但这次兜底关闭的返回值被忽略。
5. 任一步返回错误时记录重试次数、SQL 和错误，调用 `reset()` 撤销调用方在本轮累积的部分结果，再把错误注释为 `sql: <query>, args: []` 交给 `WithRetry`。
6. `WithRetry` 根据 backoffer 的剩余次数与 `NextBackoff` 决定是否再次调用；循环结束后 `queryRows` 重置 backoffer 并返回最终结果。

`ExecSQL` 的重建步骤相同，但调用 `ExecContext` 后不自行解释驱动错误，而是把成功或失败统一交给 `can_retry`。回调返回 `Ok(())` 即把本轮视为成功，包括调用方明确吞掉某类执行错误的情形；返回 `Err` 才记录日志并进入 backoff。结束时同样重置 backoffer。

## 数据与状态

`BaseConn` 的关键不变量是 `DBConn` 在调用任一方法时必须为 `Some`。`newBaseConn` 会建立该不变量，重建只有成功后才赋新值；但字段公开，而且拥有者会在结束时用 `take()` 取走连接，因此取走后再次调用方法会在 `as_ref().unwrap()` 处 panic。扩展代码不能把“`Option` 表示方法可在无连接状态运行”作为假设。

`retry_time` 是单次方法调用的局部状态，第一次尝试为 1，重建从第二次开始。backoffer 则跨方法保存在结构体中，但每次方法返回前重置：可重试策略恢复到 `dumpChunkRetryTime`，noop 策略恢复到一次尝试。`QuerySQLWithColumns` 用 `RefCell` 让查询回调和 reset 回调共享结果；普通 `QuerySQL` 的累计状态由调用方拥有，必须通过传入的 `reset` 自行恢复。

当前 `Conn` 是 `stubs.rs` 中的 `Clone` 类型，其内部查询脚本、失败队列和执行日志由 `Arc<Mutex<...>>` 共享；`Rows` 则持有独立游标、关闭标志、行级错误和可注入的关闭错误。这些是当前 Rust 实现的真实运行模型，不能据此推断已经接入真实 MySQL 连接池。

## 依赖与调用关系

RustCodeGraph 给出的直接下游边包括 `newBaseConn -> newRebuildConnBackOffer`、`QuerySQL -> queryRows`，源码进一步确认核心链为 `queryRows/ExecSQL -> WithRetry`，查询链调用 `Conn::QueryContext`、`Rows::{Next, Err, Close}`，执行链调用 `Conn::ExecContext`，列投影调用 `sql.rs::GetSpecifiedColumnValuesAndClose`。

上游调用者方面，RustCodeGraph 对 Rust 方法调用未产出完整 callers，因此以精确源码搜索补齐：

- `dump.rs::Dumper::Dump`（所在实现块）创建元数据 `BaseConn`，传入后续 `dumpDatabases`，并在生产完成或投影准备失败后取出和关闭连接。
- `dump.rs::getColumnTypes` 直接调用 `queryRows`，读取 `ColumnTypes`，并在失败重试前清空已收集类型。
- `sql.rs` 的 `ShowCreateDatabase`、`ShowCreateTable`、`ShowCreatePlacementPolicy`、`ShowCreateView`、主键/索引/分区等元数据函数广泛调用 `QuerySQLWithColumns`。
- `sql.rs::SelectTiDBRowID` 调用 `ExecSQL`，通过回调把 1054/unknown-column 类错误解释为“无隐式 rowid”，其他错误继续向上返回。

crate 边界由 `Cargo.toml` 与 `lib.rs` 确认；本文件本身使用的大部分 SQL 类型来自同 crate 的 `stubs.rs`，日志上下文来自 `astersql-dumpling-context` 和 `astersql-dumpling-log`。

## 错误处理与边界

- `QueryContext`、处理回调或 `Rows::Err` 的错误都会触发日志、`reset`、SQL 上下注释及 backoff。日志会包含完整 SQL，未来接入敏感查询参数时需评估脱敏。
- `Rows::Close` 有两层行为：处理回调内部显式关闭的错误可以传播（`QuerySQLWithColumns` 正是如此），`queryRows` 最后的兜底 `Close` 返回值则被忽略。`conn_test.rs::query_columns_retries_close_and_propagates_final_row_errors` 证明了前一种关闭错误能触发重试。
- `ExecSQL` 有意把驱动错误交给 `can_retry`。`conn_test.rs::exec_sql_uses_callback_result_for_execution_errors` 证明回调可将底层执行错误转换成成功；新增调用者必须明确这种策略，不能假设所有 `ExecContext` 错误都会返回给上层。
- `WithRetry` 在进入尝试前检查 `tctx.Done()` 的布尔值；当前 Rust 桩不是持续监听的取消通道。取消时返回 `context canceled`，且本文件仍会在退出后重置 backoffer。
- 允许重试但缺少重建回调不是错误：系统会复用原连接。相反，`DBConn == None` 会 panic，而不是返回结构化错误。
- Rust 接口目前只接受 SQL 字符串，没有 Go 版的可变 `args ...any`，错误注释也固定为 `args: []`；需要参数绑定的扩展不能假装已由该层支持。

## 并发与资源生命周期

所有操作都是同步调用；没有启动线程、任务或 channel。方法接收 `&mut BaseConn`，Rust 借用规则阻止同一个包装器被两个调用并发修改。重建回调要求 `Send`，但 `BaseConn` 整体未声明额外的 `Sync` 保证；调用方若跨线程共享，需要在外层建立所有权或锁策略。

查询取得的 `Rows` 在尝试闭包结束前关闭。`QuerySQLWithColumns` 的辅助函数会先关闭，`queryRows` 随后再次兜底关闭；当前桩允许重复关闭。`BaseConn` 没有 `Drop` 实现，也不会在替换连接时显式关闭旧连接；重建回调和底层连接语义必须负责避免资源泄漏。最终持有者必须像 `dump.rs` 那样 `take()` 并 `Close()` 当前连接。

重试状态串行复用：无论成功或失败，方法返回前都会恢复预算。`conn_test.rs::query_rows_handles_empty_results_and_retries_with_reset_and_rebuild` 验证了失败尝试清空部分结果、第二轮重建一次，以及完成后剩余次数恢复为 `dumpChunkRetryTime`。

## 与 Go 版本的对应关系

Rust 的 `BaseConn`、`newBaseConn`、`QuerySQL`、`queryRows`、`QuerySQLWithColumns` 和 `ExecSQL` 与 [`conn.go`](conn.go) 逐一对应；第二轮开始重建、重建参数为 `false`、失败时 reset、每次顶层调用后重置 backoffer、由 `ExecSQL` 回调解释错误等主干语义保持一致。Go 侧 `sql_test.go` 的“build order by clause with retry”用连续两次连接错误和第三次成功验证了重建重试意图，Rust 的 `conn_test.rs` 则直接覆盖了重建和状态清理。

已确认的差异如下：

- Go 使用 `*sql.Conn` 与 `*sql.Rows`；Rust 当前使用 `stubs.rs` 的内存脚本化 `Conn/Rows`。
- Go 查询和执行接口接受 `args ...any` 并传入驱动；Rust 只有 `query: &str`，错误上下文固定写空参数。
- Go 的 `DBConn` 是非空指针约定；Rust 用公开的 `Option<Conn>` 支持拥有者 `take()`，但方法内部仍以 `unwrap` 强制非空。
- Go 的 `backOffer` 与 `rebuildConnFn` 是包内字段；Rust 三个字段均为 `pub`，扩大了破坏不变量的可能性。
- Go `GetSpecifiedColumnValuesAndClose` 在完全找不到目标列时返回空结果；Rust 同名函数对任一缺失列返回 `column <name> not found`。此外 Rust 辅助函数传播显式 `Close` 错误，而 Go 的 deferred `Close` 返回值被忽略。
- Go `QueryContext`/`ExecContext` 接收活的 context；Rust 当前把 `tctx.Done()` 求值为布尔值传给 `WithRetry`，不具备重试期间动态取消的同等语义。

因此本文件属于有真实控制流和单元测试的迁移实现，但其数据库、context 与参数绑定能力仍受当前桩模型限制，不能描述为已完全等价于 Go 生产驱动。

## 扩展指南

- 新增查询便利接口应优先组合 `queryRows`，并提供能够撤销每次失败尝试全部副作用的 `reset`；不要把测试写进 `conn.rs`，应同步扩展独立的 [`conn_test.rs`](conn_test.rs)。
- 修改重试次数、错误分类或等待策略应落在 [`retry.rs`](retry.rs) 及其独立测试，而不是在本文件复制策略；仍需验证每条退出路径执行 `backOffer.Reset()`。
- 接入真实数据库或参数化 SQL 时，应同步调整 `QuerySQL`、`queryRows`、`QuerySQLWithColumns`、`ExecSQL` 的参数传递与错误注释，并以 Go `conn.go`/`sql.go` 为行为基线，特别注意 context 取消和 `sql.Result` 在失败时可能为空的语义。
- 改造连接所有权时，优先消除“公开 `Option` + 内部 `unwrap`”的不一致，并明确旧连接在重建时由谁关闭；兼容风险集中在 `dump.rs` 当前的 `DBConn.take()` 清理方式。
- 修改关闭逻辑需区分处理器显式关闭和 `queryRows` 兜底关闭，覆盖关闭错误、行级错误与回调错误的优先级，避免重复关闭在真实驱动上产生新行为。
- 性能上，重试会重复整个查询及结果处理；大型结果的 reset 与重新物化可能昂贵。扩展回调必须可重复执行，且不能留下外部不可回滚副作用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `dumpling/export/conn.rs`；`files --filter dumpling/export` 确认 Rust/Go 对照文件与测试；`node --file dumpling/export/conn.rs` 读取了完整 182 行；`query` 定位 `BaseConn`、`newBaseConn` 及各方法；`callees` 确认 `newBaseConn -> newRebuildConnBackOffer`、`QuerySQL -> queryRows`。Rust 方法的 `callers` 没有返回完整结果，故调用者部分用精确 `rg` 和相邻源码核验，未采用错误的跨文件同名匹配。
- 已读实现：[`conn.rs`](conn.rs)、[`retry.rs`](retry.rs)、[`stubs.rs`](stubs.rs)、[`sql.rs`](sql.rs)、[`dump.rs`](dump.rs) 和 crate 入口 [`lib.rs`](lib.rs)。
- 已读边界：[`Cargo.toml`](Cargo.toml) 的 crate 名、`[lib] path`、Go package 元数据、依赖及本地 SQL/MySQL stubs 说明。
- 已读对照：[`conn.go`](conn.go)、`sql.go`，以及 Go `sql_test.go` 中重建连接重试场景。
- 已读 Rust 测试：[`conn_test.rs`](conn_test.rs) 三个测试，覆盖执行错误由回调解释、空结果元数据、行错误后的 reset/重建、列投影关闭错误重试、最终行错误注释和 backoff 重置；`lib.rs` 以独立 `#[cfg(test)]` 模块挂载该文件。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一节结构检查和人工事实复核作为验证。
