# `pkg/testkit/db_driver.rs`

## 文件定位

本文说明对应源码 [`db_driver.rs`](./db_driver.rs)。本文件属于 `astersql-testkit` crate（见 [`Cargo.toml`](./Cargo.toml) 的 `[package]` 与 `[lib] path = "lib.rs"`），位于测试工具箱的数据库适配边界。[`lib.rs`](./lib.rs) 以 `pub mod db_driver` 装配模块，并公开再导出这里的值、结果、驱动、行游标和 `Database` trait。它不是生产 SQL 协议驱动，而是让 Rust 测试以一套接近 Go `database/sql` 的接口连接 `TestKit` 会话或 mock store。

上游主要有两类：`pkg/testkit/testkit.rs`、`dbtestkit.rs`、`asynctestkit.rs`、`stepped.rs` 使用 `Database`/`DbValue`/`PreparedStatement` 组成测试 API；`pkg/testkit/db_driver_test.rs` 直接通过 `CreateMockDB` 验证 Go 风格的 `Exec`、`Query`、`Prepare`、`Next` 和 `Scan`。下游由具体 `Database` 实现决定；规范实现之一在 `pkg/testkit/mockstore.rs`，将参数转为会话协议参数并把记录集排空为 `QueryRows`。

## 核心职责

- 定义跨测试后端稳定的数据契约：`DbValue`、`ExecutionResult`、`QueryRows`、`PreparedResultField` 与 `AnalyzeStatsContext`。
- 用 `Database: Send + Sync + 'static` 抽象 SQL 执行、查询、会话创建/关闭、预编译语句和一组测试观测能力。只有 `execute` 与 `query` 是必需实现；其余多数方法提供委托、空值或明确的“不支持”错误。
- 用 `PreparedStatement` 和 `DbDriver` 提供持有 `Arc<dyn Database>` 的轻量转发层，不自行解析 SQL 或计算参数数量。
- 用 `MockDB`、`MockRows`、`MockStmt`、`MockRow` 和 `AssignFromDbValue` 提供接近 Go `database/sql` 的同步测试门面，包括惰性会话、游标前进、类型化扫描、首行查询和关闭。

## 主要符号

- `DbValue`：统一表示 `Null`、`Bool`、`I64`、`U64`、`F64`、`Bytes`、`String`。`Display` 将 NULL 写成 `<nil>`，字节使用 UTF-8 lossy 显示；`From` 实现覆盖常用绑定参数类型。
- `ExecutionResult`：保存 `affected_rows` 和 `last_insert_id`。`QueryRows` 保存列名与二维值矩阵，`string_rows` 对每个单元调用 `Display`。
- `PreparedResultField`：保存预编译 SELECT 的库、表、表别名、列和列别名元数据。`AnalyzeStatsContext` 是 `astersql_domain::DomainStatsContext` 的别名。
- `Database`：线程安全的对象安全后端接口。核心方法是 `execute`、`query`；内部 SQL 默认委托普通执行；预编译元数据/执行/删除及部分会话观测默认报不支持；会话创建默认 `Ok(None)`，关闭默认成功。
- `PreparedStatement`：保存后端 `Arc` 与原始 SQL `Arc<str>`，`execute`/`query` 原样转发。`DbDriver` 在其上提供 `prepare`、直接执行/查询与关闭。
- `AssignFromDbValue`：扫描目标协议，当前支持 `String`、`i32`、`i64`、`f64`、`bool`、`Vec<u8>`。
- `CreateMockDB`/`MockDB`：保存 store 和 `Mutex<Option<Arc<dyn Database>>>`；`ensure_session` 首次使用时调用 `create_session`，若返回 `None` 则直接复用 store。
- `MockRows`：拥有查询结果、下一行索引、关闭标志和预留错误槽；`Next` 选定当前行，`Scan` 写入目标，`Close` 停止迭代，`Err` 返回累计错误。
- `MockStmt`/`MockRow`/`IntoQueryArgs`：预编译查询门面。`QueryRow` 只保留第一行，空结果保存 `sql: no rows in result set`；参数既可为单值也可为 `Vec<DbValue>`。

## 执行流程

1. 调用者把 store 传给 `CreateMockDB`；此时只初始化空的会话缓存，不打开会话。
2. 首次 `MockDB::Exec`、`Query` 或 `Prepare` 进入 `ensure_session`。它在互斥锁内复用已缓存会话，或调用 `Database::create_session`，没有派生会话能力时回退到 store，然后缓存结果。
3. `Exec` 以空参数调用 `Database::execute`；`Query` 调用 `Database::query` 并把返回矩阵移动进 `MockRows`；`Prepare` 创建绑定相同会话和 SQL 的 `PreparedStatement`/`MockStmt`。
4. `MockRows::Next` 每成功一次把索引加一；随后 `Scan` 取 `rows[index - 1]`，先检查当前行和目标数，再逐列调用 `AssignFromDbValue::assign_from`。
5. `MockStmt::QueryRow` 通过 `IntoQueryArgs` 构造参数，调用 `PreparedStatement::query`，只取第一行；底层错误或无行状态延迟到 `MockRow::Scan` 返回。
6. `MockDB::Close` 从互斥缓存中 `take` 会话并调用其 `close`；缓存已空时幂等成功。`MockRows::Close` 只设置本地标志，`MockStmt::Close` 当前为空操作。

独立的 `DbDriver` 路径更薄：`DBTestKit::new` 在 `pkg/testkit/dbtestkit.rs` 中构造驱动，`MustPrepare` 得到 `PreparedStatement`，其他 `Must*` 方法调用转发层并在错误时 panic。

## 数据与状态

`DbValue` 与两个结果结构都是拥有型数据，跨 trait 边界不借用后端内部缓冲。`QueryRows::string_rows` 只创建字符串副本，不改变原矩阵。`PreparedStatement` 和 `DbDriver` 的 clone 只克隆 `Arc` 与 SQL 的 `Arc<str>`，共享同一后端。

`MockDB` 的可变状态只有 `session: Mutex<Option<Arc<dyn Database>>>`。该缓存保证同一个 `MockDB` 的操作复用同一派生会话；`Close` 清空缓存，因此后续操作会重新创建会话。`MockRows` 将完整结果保存在内存中，`index` 的不变量是：零表示尚无当前行，成功 `Next` 后当前行为 `index - 1`，达到行数后不再前进。`err` 当前构造为 `None` 且文件内没有写入路径，是为 `Err` 接口保留的状态。

## 依赖与调用关系

- crate 边界：`pkg/testkit/Cargo.toml` 声明 `astersql-domain` 与 `astersql-session` 等路径依赖；本文件直接使用前者的 `DomainStatsContext` 和后者的 `RuntimeStaleReadState`，错误边界来自 crate 根的 `TestError`/`TestResult`。
- 导出边：`pkg/testkit/lib.rs` 再导出本文件的主要公开符号。RustCodeGraph `node CreateMockDB` 还确认其被四个 `db_driver_test.rs` 测试调用，并实例化 `MockDB`。
- 上游边：`pkg/testkit/dbtestkit.rs` 的 `DBTestKit` 持有 `DbDriver`；`pkg/testkit/testkit.rs` 持有 store 和派生 `Database` 会话并创建 `PreparedStatement`；异步和 stepped 工具也以 `Arc<dyn Database>` 为执行边界。
- 下游边：`pkg/testkit/mockstore.rs` 至少实现 `MockStore`、`AnalyzeSessionDatabase`、`AnalyzeStatsStore` 三类 `Database`。其中 `AnalyzeStatsStore::create_session` 产生独立会话，`AnalyzeSessionDatabase` 负责真实请求、首结果集选择、预编译语句及关闭。
- 数据转换边：`mockstore.rs::protocol_arguments` 将每个 `DbValue` 映射为会话协议参数；`drain_record_set` 把结果集转换为 `QueryRows`，并把 `<nil>`/内部 NULL 标记恢复为 `DbValue::Null`。

## 错误处理与边界

所有可恢复错误通过 `TestResult<T>`/`TestError` 返回。`Database` 的可选能力分三类：安全默认值（例如无统计上下文、空 trace、零 tracker 值）、对普通执行的默认委托（内部执行/查询）、明确报错的不支持能力（例如预编译元数据、行编码器、连接排序规则）。新增后端不能把默认值误当成能力已实现。

扫描边界是严格的：未先成功 `Next`、当前行越界、目标数量与列数不同、NULL 写入当前所有非空目标类型、整数越界、字符串解析失败或类型不兼容都会返回错误。逐列赋值不是事务性的；前面列可能已写入，后面列才失败，调用者不应假设失败时目标保持原值。`f64` 接收整数时使用 Rust `as` 转换，超大整数可能丢失精度。`Bytes` 显示和转字符串使用 lossy UTF-8，不保证字节往返。

`MockStmt::QueryRow` 丢弃第二行及后续行；无行错误延迟到 `Scan`，与 Go 的 `QueryRow` 风格一致。`PreparedStatement` 不数 SQL 中的 `?`，参数词法和数量由具体后端负责，避免把字符串或注释内问号误判为占位符。`MockRows::Err` 目前不会从扫描错误自动积累错误，扫描失败由 `Scan` 的返回值直接处理。

## 并发与资源生命周期

`Database` 强制 `Send + Sync + 'static`，实例通常由 `Arc` 共享。`MockDB::ensure_session` 在创建会话期间持有 mutex，因此同一个 `MockDB` 的并发首次访问只会发布一个缓存会话；代价是会话创建期间其他访问会等待。锁中毒使用 `expect`，会 panic 而非转成 `TestError`。

资源关闭需要区分层次：`MockDB::Close` 真正调用缓存会话的 `Database::close` 并释放该 `Arc`；若 `close` 报错，会话已被 `take`，再次关闭不会重试。`DbDriver::close` 直接关闭其共享后端，其他 clone 仍持有同一对象但后端可能已经关闭。`MockRows` 已拥有全部行，关闭不触及后端记录集；真正的记录集关闭发生在 `mockstore.rs::drain_record_set`。`MockStmt::Close` 当前不释放服务端 statement，因为此门面只保存 SQL 并在执行时委托后端；若改成持有真实 statement ID，必须同时实现错误路径清理与关闭幂等性。

## 与 Go 版本的对应关系

Go 对照为 `pkg/testkit/db_driver.go`，回归测试为 `pkg/testkit/db_driver_test.go`；Rust 对应测试是独立文件 `pkg/testkit/db_driver_test.rs`。两边都覆盖同一 store 上建表、写入、按序扫描、影响行数、TestKit 交叉可见性、预编译参数查询和关闭。

Rust 保留了 Go 的外观语义，但不是逐类型复刻：Go 通过全局注册的 `database/sql` driver、`tkMap` 和原子 ID 由 `sql.Open` 建连接；Rust 直接保存 `Arc<dyn Database>`，用每个 `MockDB` 的 mutex 惰性创建会话，不存在全局注册表。Go 的 `testKitRows` 从 `RecordSet`/chunk 流式拉取并由 `Close` 关闭结果集；Rust 的 `QueryRows` 已在后端排空，`MockRows` 是内存游标。Go 使用 `driver.Value` 和 `database/sql.Scan` 转换；Rust 以封闭的 `DbValue` 与 `AssignFromDbValue` 明确列出支持类型。

Go `testKitStmt::NumInput` 返回 `-1`，Rust 对应做法是不在 `PreparedStatement` 内统计占位符。Rust 测试额外固定了首行/空结果、NULL 拒绝扫描，以及字符串字面量中的 `?` 不计为参数；这些是 Go 接口语义在 Rust 后端上的直接防回归证据。

## 扩展指南

- 新增值类型时，先扩展 `DbValue`，同步 `Display`、必要的 `From`/`AssignFromDbValue`，再更新 `mockstore.rs::protocol_arguments` 与结果解码；应在独立的 `pkg/testkit/db_driver_test.rs` 增加正常、NULL、越界/解析失败测试。不要把测试内嵌回生产文件。
- 新增 `Database` 观测能力时，判断“不支持”应是错误、`None` 还是安全默认值；规范会话实现通常位于 `mockstore.rs::AnalyzeSessionDatabase`，store 层能力位于 `AnalyzeStatsStore`。同步检查 `TestSession`/`TestKit` 的公开门面。
- 若让 `PreparedStatement` 使用 `prepare_statement` 返回的真实 ID，需要定义创建失败、执行失败、显式关闭和 Drop 的所有权，并保证 `drop_prepared_statement` 恰当调用；现有 SQL 转发语义不可无意改变。
- 若改变 `MockRows` 为流式读取，需要重新设计错误累计和记录集关闭，保持 `Next`/`Scan`/`Err` 顺序、不同行数目标错误和提前关闭行为，并评估锁持有时间与内存占用。
- `CreateMockDB`/`MockDB::ensure_session` 的会话缓存变更会影响隔离性与关闭语义；至少同步 `db_driver_test.rs`，并检查 `mockstore.rs` 的活跃会话计数与 shutdown 路径。兼容风险主要是 Go `database/sql` 行为差异，性能风险主要是当前全结果集物化及同一 `MockDB` 串行初始化。

## 验证依据

- 源码：`pkg/testkit/db_driver.rs`（全部 771 行）、`pkg/testkit/lib.rs`、`pkg/testkit/dbtestkit.rs`、`pkg/testkit/testkit.rs`、`pkg/testkit/mockstore.rs`。
- crate 配置：`pkg/testkit/Cargo.toml`，确认 crate 名、根模块和 `astersql-domain`/`astersql-session` 等依赖；没有 feature 条件控制本文件。
- Go 对照：`pkg/testkit/db_driver.go` 与 `pkg/testkit/db_driver_test.go`。
- Rust 独立测试：`pkg/testkit/db_driver_test.rs`，覆盖 `TestMockDB`、`mock_stmt_query_row_is_first_row_and_reports_empty_result`、`mock_rows_reject_scanning_null_into_non_nullable_destinations`、`mock_stmt_does_not_count_question_marks_inside_sql_literals_as_parameters`。
- RustCodeGraph：`status` 显示目标已索引；`files --filter pkg/testkit/db_driver.rs` 报告该文件 109 个符号；`query` 定位 `DbValue`、`Database`、`PreparedStatement`、`DbDriver`、`MockDB`、`MockRows`、`MockStmt`、`MockRow`；`node CreateMockDB` 确认 Rust 构造边、四个测试调用边和 `lib.rs` 导入边，`node MockStmt` 确认由 `MockDB::Prepare` 实例化。
- 人工复核：文档区分了 trait 默认能力与具体实现、Rust 内存行集与 Go 流式结果集、显式扫描错误与 `MockRows::Err` 预留状态，没有将未实现能力描述为已支持。
