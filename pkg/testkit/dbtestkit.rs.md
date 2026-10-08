# `pkg/testkit/dbtestkit.rs`

## 文件定位

本文件位于 `astersql-testkit` crate（`pkg/testkit/Cargo.toml`）中，由 crate 根模块 `pkg/testkit/lib.rs` 以 `pub mod dbtestkit` 装配，并以 `pub use dbtestkit::DBTestKit` 再导出。它不是 SQL 引擎生产请求链的一部分，而是测试侧的同步、失败即终止（panic）数据库操作门面：调用者提供一个 `Arc<dyn Database>`，它将 SQL 执行交给 `pkg/testkit/db_driver.rs` 中的 `DbDriver`。

RustCodeGraph 将目标文件识别为 10 个符号（文件、`DBTestKit` 及其 8 个方法）。仓库文本检索没有发现 `pkg/testkit/dbtestkit.rs` 和 `pkg/testkit/lib.rs` 之外的 Rust `DBTestKit` 构造或方法调用，因此当前 Rust 侧属于已导出、可复用但尚无直接消费点的测试辅助 API。不能据此推断它已经替代 Go server/handler 测试中的 `DBTestKit`。

## 核心职责

- `DBTestKit` 把线程安全的 `Arc<dyn Database>` 包装成 `DbDriver`，提供直接执行、直接查询和预编译句柄三类便利操作（`DBTestKit::new`、`MustExec`、`MustQuery`、`MustPrepare`）。
- 所有可能返回 `TestResult` 的执行/查询路径在错误时立即 panic，并把 SQL、参数或预编译 SQL 写入消息，适合表达测试中的“此步骤必须成功”前置条件（`MustExecPrepared`、`MustQueryPrepared`、`MustExec`、`MustQuery`）。
- `MustQueryRows` 只验证结果矩阵至少包含一行，不负责逐列断言；详细内容可由调用者取得 `MustQuery` 返回的 `QueryRows` 后检查。
- `GetDB` 暴露内部 `DbDriver` 的共享引用，供门面没有覆盖的 driver 能力使用；当前 `DbDriver` 公开 `prepare`、`execute`、`query` 和 `close`（`pkg/testkit/db_driver.rs:420-440`）。

## 主要符号

- `pub struct DBTestKit { driver: DbDriver }`：唯一状态是私有 driver；派生 `Clone`，克隆时复制 driver，而 driver 内部继续共享同一个 `Arc<dyn Database>`。
- `pub fn new(database: Arc<dyn Database>) -> Self`：公开构造入口，把数据库抽象传给 `DbDriver::new`。`Database` 要求 `Send + Sync + 'static`（`pkg/testkit/db_driver.rs:143-161`）。
- `pub fn MustPrepare(&self, query: &str) -> PreparedStatement`：调用 `DbDriver::prepare`。当前 prepare 只把数据库引用和 SQL 文本保存进 `PreparedStatement`，不会在构造时调用底层数据库或返回错误（`pkg/testkit/db_driver.rs:379-412,425-428`）。
- `pub fn MustExecPrepared(&self, statement: &PreparedStatement, args: Vec<DbValue>) -> ExecutionResult`：调用 `PreparedStatement::execute`；失败时 panic，消息包含 `statement.sql()` 和底层错误。
- `pub fn MustQueryPrepared(&self, statement: &PreparedStatement, args: Vec<DbValue>) -> QueryRows`：调用 `PreparedStatement::query`；失败时同样 panic。
- `pub fn MustExec(&self, sql: &str, args: Vec<DbValue>) -> ExecutionResult`：委托 `DbDriver::execute`，成功时返回 `affected_rows`、`last_insert_id` 组成的值对象（`pkg/testkit/db_driver.rs:102-109`）。
- `pub fn MustQuery(&self, sql: &str, args: Vec<DbValue>) -> QueryRows`：委托 `DbDriver::query`，成功时返回列名及 `Vec<Vec<DbValue>>` 结果矩阵（`pkg/testkit/db_driver.rs:111-118`）。
- `pub fn MustQueryRows(&self, sql: &str, args: Vec<DbValue>)`：复用 `MustQuery`，随后断言 `rows.rows` 非空；它不返回行集。
- `pub fn GetDB(&self) -> &DbDriver`：借用内部 driver，不转移所有权。

文件没有模块级常量、trait、条件编译项或内部自由函数；全部方法均为公开 API。方法名保留 Go 风格，因此 crate 根以 `#![allow(..., non_snake_case)]` 允许该命名（`pkg/testkit/lib.rs:9`）。

## 执行流程

1. 测试后端实现 `Database`，再以 `Arc<dyn Database>` 调用 `DBTestKit::new`；构造过程仅建立共享所有权，没有 SQL、I/O 或会话创建副作用。
2. 直接执行路径为 `MustExec -> DbDriver::execute -> Database::execute`。底层返回 `Ok(ExecutionResult)` 时原样交回；`Err(TestError)` 时在门面处 panic。
3. 直接查询路径为 `MustQuery -> DbDriver::query -> Database::query`。返回的 `QueryRows` 已物化为内存中的列名和行矩阵，不是流式游标。
4. 预编译路径先经 `MustPrepare -> DbDriver::prepare -> PreparedStatement::new` 保存 SQL 与同一数据库对象；后续 `MustExecPrepared`/`MustQueryPrepared` 分别调用句柄的 `execute`/`query`，最终仍落到 `Database::execute`/`query`。也就是说，此处“prepared”句柄不调用 `Database::prepare_statement`，参数语义由实际执行后端校验。
5. 至少一行断言路径为 `MustQueryRows -> MustQuery`；查询错误先 panic，查询成功但 `rows.rows.is_empty()` 时再以 `query ... returned no rows` panic。

## 数据与状态

`DBTestKit` 自身只有 `DbDriver` 字段；`DbDriver` 与 `PreparedStatement` 都通过 `Arc<dyn Database>` 共享后端。`PreparedStatement` 另以 `Arc<str>` 持有 SQL，因此克隆句柄不会复制后端，也不需要借用构造时的 `&str`。

输入参数使用拥有所有权的 `Vec<DbValue>`，传到下层时临时借用为切片。`DbValue` 是 testkit 的类型化 SQL 值；输出分别是按值返回的 `ExecutionResult` 和完全物化的 `QueryRows`。本文件不保存最近一次结果、不维护事务状态、不缓存语句 ID，也不改变 `Database` 的可选测试状态。

`Clone` 的语义是多个 `DBTestKit`/driver 共享同一后端，因此后端内部的会话、事务或 mock 状态也会共享；是否允许并发操作由 `Database` 的具体实现决定，但 trait 的 `Send + Sync` 是最低线程安全契约。

## 依赖与调用关系

上游装配关系是 `pkg/testkit/lib.rs -> pub mod dbtestkit -> pub use DBTestKit`。当前精确 Rust 文本检索未找到实际 `DBTestKit::new` 或类型使用点；RustCodeGraph 的文件级“used by”结果包含共享依赖符号造成的宽泛关联，不能作为本类型已有调用者的证据。

下游关系全部位于同 crate：

- `DBTestKit::new -> DbDriver::new`；
- `MustPrepare -> DbDriver::prepare -> PreparedStatement::new`；
- `MustExecPrepared -> PreparedStatement::execute -> Database::execute`；
- `MustQueryPrepared -> PreparedStatement::query -> Database::query`；
- `MustExec -> DbDriver::execute -> Database::execute`；
- `MustQuery`/`MustQueryRows -> DbDriver::query -> Database::query`。

`pkg/testkit/Cargo.toml` 声明 crate 名为 `astersql-testkit`、库入口为 `lib.rs`，并以 `[package.metadata.porting] go-package = "pkg/testkit"` 标明 Go 对照包。目标文件本身只使用标准库 `Arc` 和同 crate 的 `db_driver` 类型，没有直接使用 Cargo 中列出的外部 crate。

## 错误处理与边界

该 API 故意不传播 `TestResult`：`MustExecPrepared`、`MustQueryPrepared`、`MustExec`、`MustQuery` 都在底层错误时 panic。直接 SQL 路径的消息包含调试格式 SQL、参数和错误；预编译路径包含原始 SQL 和错误，但不包含参数。敏感 SQL/参数可能因此进入测试日志，新增调用时应避免把真实凭据传入这些辅助函数。

`MustPrepare` 名称与 Go 版本类似，但 Rust 版本当前不会在 prepare 阶段失败：`DbDriver::prepare` 只是构造句柄。因此 SQL 或参数错误通常延后到 `MustExecPrepared`/`MustQueryPrepared`。这也意味着 `Database::prepare_statement`、`execute_prepared_statement`、`drop_prepared_statement` 的默认“不支持”错误不经过本文件的 prepared 路径。

`MustQueryRows` 只判断物化矩阵非空；它不验证列数、值、后续迭代错误或显式关闭。空结果会 panic，恰好一行或多行都会成功。`MustExec`/`MustQuery` 只依据 `Result` 判断成功，不额外断言返回对象非空；Rust 返回类型本身不是 Go 的可空接口/指针。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务。共享所有权由 `Arc` 管理，`Database: Send + Sync + 'static` 允许 `DBTestKit` 在后端实现满足约束时跨线程共享；本文件没有额外串行化措施。

资源关闭不是 `DBTestKit` 的自动职责：它没有 `Drop` 实现，也没有 `Close` 方法。若具体后端要求显式关闭，调用者只能通过 `GetDB().close()` 或持有的后端对象处理。`PreparedStatement` 只是 SQL 与数据库的共享句柄，没有语句 ID、`Drop` 或 `Close`；`QueryRows` 是内存值，也没有游标关闭步骤。`pkg/testkit/db_driver_test.rs:21-75` 对更接近 Go `database/sql` 的 `MockDB`/`MockStmt` 路径显式测试了 `rows.Close()`、`stmt.Close()` 和 `db.Close()`，但这些关闭行为不能外推为本文件已经执行。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/testkit/dbtestkit.go`。两边都提供构造、prepare、prepared exec/query、直接 exec/query、至少一行查询和取得底层 DB 的同名能力，并采用“错误立即使测试失败”的意图。

关键差异如下：

- Go `NewDBTestKit(t, *sql.DB)` 保存 `testing.T` 绑定的 `require.Assertions`/`assert.Assertions` 和连接池；Rust `new(Arc<dyn Database>)` 不持有测试上下文，失败通过 panic 表达。
- Go `MustPrepare` 立即调用 `sql.DB.Prepare` 并检查错误；Rust 只创建惰性的 `PreparedStatement`，执行时才到后端。
- Go 参数为可变参数 `...any`；Rust 为显式 `Vec<DbValue>`，调用者必须完成类型转换。
- Go 查询返回需要迭代、检查 `Err` 并关闭的 `*sql.Rows`；Rust 返回已经物化的 `QueryRows`。Go `MustQueryRows` 调用 `Next`、检查游标错误并 `Close`，Rust 只检查 `rows.rows` 非空。
- Go `MustExec`/`MustQuery` 还断言结果接口或行指针非 nil；Rust 的非可空返回类型省去了这一检查。
- Go `GetDB` 返回 `*sql.DB`，可直接 `Begin` 等；Rust 返回 `&DbDriver`，目前只有 prepare/execute/query/close，不等价于完整连接池 API。

Go 版本已由 `pkg/server/internal/testserverclient/server_client.go`、`pkg/server/tests/commontest/tidb_test.go` 及多个 handler 测试实际使用；当前 Rust 搜索没有对应消费点，所以迁移状态应描述为“公共门面已实现、调用侧尚未接线”，而不是完整替换。

## 扩展指南

新增断言式操作时，优先保持本文件为薄门面：通用数据库能力放入 `Database`/`DbDriver`，这里只负责测试友好的成功返回和失败上下文。若新增直接 SQL API，应同时考虑 `DbValue` 输入、`TestError` 信息是否泄露敏感值，以及 `ExecutionResult`/`QueryRows` 是否足以表达结果。

若要实现真正的 server-side prepare，不应只修改 `MustPrepare` 名称或参数计数逻辑；需要明确接入 `Database::prepare_statement`、`execute_prepared_statement` 和 `drop_prepared_statement`，为语句 ID 定义关闭/Drop 生命周期，并验证底层会话的 SQL 词法和参数绑定。现有薄句柄刻意把 `?` 的识别留给后端（`pkg/testkit/db_driver.rs:379-405`）。

若要补齐 Go `MustQueryRows` 语义，应先决定 Rust 的物化查询是否可能携带“迭代后错误”；当前 `QueryRows` 类型没有该状态，也无需 close，不能机械增加无效检查。若要补齐 `GetDB().Begin()` 类能力，则应扩展数据库/driver 的事务抽象，而非暴露某个具体后端并破坏 trait 边界。

测试必须放在独立文件，不能内嵌进 `dbtestkit.rs`。最接近的现有独立测试是 `pkg/testkit/db_driver_test.rs`，它验证共同下游的 query、exec、prepare、空结果、NULL 扫描、参数标记和资源关闭；针对本门面的 panic 文本、空行断言、克隆共享以及 prepared 延迟失败，宜新增独立的 `pkg/testkit/dbtestkit_test.rs`，并在 `pkg/testkit/lib.rs` 的 `#[cfg(test)]` 区域挂载。

## 验证依据

- RustCodeGraph `status`：索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/testkit` 确认目标、Go 对照、crate 根与相关测试均已索引。
- RustCodeGraph `node --file pkg/testkit/dbtestkit.rs --offset 1 --limit 260`：读取目标文件全部 92 行，确认 `DBTestKit`、8 个公开方法及无条件编译分支。
- RustCodeGraph `query DBTestKit`、`query MustQuery`、`query MustExec`：确认 Rust/Go 对照符号；同名方法过多，因此没有把未消歧的全仓调用结果作为目标调用边证据。
- RustCodeGraph `node --file pkg/testkit/lib.rs`：确认模块装配、公开再导出和 crate 级非 snake case 许可。
- RustCodeGraph `node --file pkg/testkit/db_driver.rs`：核对 `Database`、`ExecutionResult`、`QueryRows`、`PreparedStatement`、`DbDriver` 的实际委托关系和资源接口。
- `pkg/testkit/Cargo.toml`：核对 crate 名、库入口、Go 包映射和依赖边界；目标文件没有直接第三方依赖。
- `pkg/testkit/dbtestkit.go`：核对 Go API、测试断言机制、游标关闭和 nil 检查语义。
- `pkg/testkit/db_driver_test.rs`：核对共同下游的执行、查询、prepare、扫描边界及显式资源关闭；未发现同名独立 Rust `dbtestkit` 测试。
- 精确 `rg` 检索 `DBTestKit::new`、类型名和 `NewDBTestKit(`：确认 Rust 侧除定义/再导出外无直接调用，并确认 Go server/handler 测试已有实际调用。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以 11 个固定二级标题的结构命令和人工事实复核验收。
