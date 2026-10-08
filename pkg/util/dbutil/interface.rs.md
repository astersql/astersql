# `pkg/util/dbutil/interface.rs`

## 文件定位

本文件是 `astersql-util-dbutil` crate 的数据库执行边界，位于 `pkg/util/dbutil`。crate 入口 `pkg/util/dbutil/lib.rs` 以 `pub mod interface` 暴露模块，并再次导出 `DBExecutor`、`QueryExecutor`、`Transaction`、`Value`、`QueryResult` 和 `DbError`，因此其他模块既可从 `crate::interface` 使用，也可从 crate 根使用这些类型。

它不建立连接、不选择数据库驱动，也不执行具体 SQL。它只规定 dbutil 辅助函数需要的最小数据形状和同步调用协议。`pkg/util/dbutil/common.rs`、`index.rs`、`query.rs`、`variable.rs` 是直接消费者；`pkg/util/dbutil/Cargo.toml` 将 crate 根设为 `lib.rs`，当前文件自身只依赖标准库 `std::fmt`，没有直接使用清单中的外部依赖或条件编译项。

## 核心职责

1. 用 `Value` 表达 dbutil 查询参数和结果单元格可携带的七类通用值，隔离具体 SQL 驱动的值类型。
2. 用 `QueryResult` 保存有序列名与二维行数据，供 `common.rs`、`index.rs` 和 `query.rs` 做扫描、转换和元数据解析。
3. 用 `DbError` 统一携带 MySQL/TiDB 错误码、可选 SQLSTATE 与消息，并实现标准错误接口。
4. 用 `QueryExecutor`、`DBExecutor` 和 `Transaction` 划分只读查询、直接写入/开事务、事务内写入/终结三层能力。
5. 为单行查询提供默认实现：`QueryExecutor::QueryRowContext` 复用实现者的 `QueryContext`，避免每个测试替身重复首行提取逻辑。

当前实现是可编译的抽象协议，不是完整数据库适配层。仓库搜索只在 `pkg/util/dbutil/*_test.rs` 中找到这些 trait 的实现；生产代码接受 `&dyn QueryExecutor` 或 `&dyn DBExecutor`，但本 crate 内没有把真实 Rust MySQL/TiDB 客户端接成实现者。

## 主要符号

- `pub enum Value`：变体为 `Null`、`Bool(bool)`、`I64(i64)`、`U64(u64)`、`F64(f64)`、`String(String)`、`Bytes(Vec<u8>)`。它派生 `Clone`、`Debug`、`PartialEq`，但没有日期、时间、十进制或驱动专属类型。
- `impl From<&str/String/i64/u64> for Value`：为常用字符串和整数提供无失败转换；布尔、浮点、字节与空值必须显式构造对应变体。
- `pub struct DbError`：公开字段 `code: u16`、`sql_state: Option<String>`、`message: String`。`Display` 只输出 `message`，`std::error::Error` 使用默认实现；结构体还派生 `Default`、`Eq` 和 `PartialEq`，便于测试按字段比较。
- `pub struct QueryResult`：`columns: Vec<String>` 与 `rows: Vec<Vec<Value>>` 都是公开且拥有所有权的数据。类型不会自行检查每行长度是否与列数一致。
- `pub trait QueryExecutor: Send + Sync`：要求实现者可在线程间转移并共享。必需方法 `QueryContext(&self, query, args)` 返回完整 `QueryResult`；默认方法 `QueryRowContext` 返回首行。
- `pub trait Transaction: QueryExecutor`：增加需要 `&mut self` 的 `ExecContext`，以及消费 `Box<Self>` 的 `Commit`、`Rollback`。消费式终结使同一个 trait object 在成功提交或回滚后不能再被调用。
- `pub trait DBExecutor: QueryExecutor`：增加 `BeginTx(&self) -> Result<Box<dyn Transaction>, DbError>` 和连接级 `ExecContext(&self, ...) -> Result<u64, DbError>`；返回值语义是受影响行数。

文件没有模块级常量、自由函数、宏、`unsafe`、异步函数或条件编译项。公开 API 使用 Go 风格的大写方法名，与同路径移植代码的命名保持一致。

## 执行流程

查询主流程由调用者把 `&dyn QueryExecutor`、SQL 文本和 `&[Value]` 传给 `QueryContext`。具体实现者负责执行 SQL、绑定参数并构造 `QueryResult`；本文件不修改 SQL，也不验证参数个数。`common.rs` 的 `GetCreateTableSQL`、`GetRowCount` 等函数和 `index.rs::ShowIndex` 沿此路径消费结果，`query.rs` 再把结果转换为二维值或按列名映射。

单行查询走 `QueryRowContext`：先调用同一对象的 `QueryContext`；若查询失败，`?` 原样传播 `DbError`；若成功，则消费 `QueryResult.rows`，取第一行并忽略其余行；若没有行，则构造 `code = 0`、`sql_state = None`、消息为 `sql: no rows in result set` 的错误。列名不会随首行返回。

直接写入由 `DBExecutor::ExecContext` 完成。事务流程由 `DBExecutor::BeginTx` 返回动态分发的 `Box<dyn Transaction>`，调用者可顺序调用事务的 `ExecContext`，最后恰好消费一次句柄进行 `Commit` 或 `Rollback`。真实用例 `common.rs::ExecuteSQLs` 在任一写入失败时尝试回滚并返回原始写入错误，全部成功时提交；这些策略属于调用者而非本文件。

## 数据与状态

所有公共数据类型都拥有内部数据，不借用数据库游标。`Value::String`、`Value::Bytes`、`DbError` 字符串以及 `QueryResult` 的列和行会随返回值转移；这使结果脱离连接生命周期，但意味着实现者必须先物化完整结果集。`QueryRowContext` 也会消费整个 `QueryResult` 后只保留首行。

`Value::F64` 使 `Value` 只能派生 `PartialEq` 而不能安全派生 `Eq`；相反，未含浮点的 `DbError` 可派生 `Eq`。`QueryResult::default()` 是空列、空行，`DbError::default()` 是错误码 0、无 SQLSTATE、空消息；默认值只提供结构初始化，不表示一次真实数据库操作成功。

接口本身不保存连接、事务状态、重试计数或上下文。事务状态完全由 `Transaction` 实现者持有，`QueryExecutor`/`DBExecutor` 也不定义超时、取消、隔离级别或连接关闭状态。

## 依赖与调用关系

- 模块装配：`pkg/util/dbutil/lib.rs` 声明并再导出本文件的六个公共符号。
- 只读上游：`pkg/util/dbutil/common.rs` 大量函数接受 `&dyn QueryExecutor`；`pkg/util/dbutil/index.rs::ShowIndex` 与 `FindSuitableColumnWithIndex` 使用它获取索引元数据；`pkg/util/dbutil/variable.rs` 用它查询变量和授权。
- 读写上游：`pkg/util/dbutil/common.rs::ExecSQLWithRetry`、`ExecuteSQLs`、`DeleteRows` 接受 `&dyn DBExecutor`。其中 `ExecuteSQLs` 是 `BeginTx`、事务 `ExecContext`、`Commit`、`Rollback` 的直接调用链。
- 数据消费者：`pkg/util/dbutil/query.rs::ScanRowsToInterfaces` 消费 `QueryResult.rows`，`ScanRow` 对所有 `Value` 变体编码；`common.rs` 和 `index.rs` 也根据变体解析数字、字符串及 NULL。
- 错误消费者：`pkg/util/dbutil/retry.rs::IsRetryableError` 根据 `DbError.code` 和 `message` 判断是否可重试；`common.rs` 还用错误码识别可忽略的幂等 DDL 错误。
- 下游依赖：本文件的唯一显式导入是 `std::fmt`。默认 `QueryRowContext` 调用 trait 的必需方法 `QueryContext` 并实例化 `DbError`；其他 trait 方法均只有签名，没有被调用函数。

RustCodeGraph 的精确节点显示 `QueryExecutor` 被 `common.rs`、`index.rs` 导入，`QueryResult` 被 `common.rs`、`query.rs` 导入，`Value` 被三者导入；对 `QueryRowContext` 的图结果确认其内部调用 `QueryContext` 并构造 `DbError`。图对同名 Go/Rust 方法存在跨语言噪声，因此实际 Rust 实现者与引用又用限定为 `*.rs` 的仓库搜索复核。

## 错误处理与边界

`QueryContext`、两种 `ExecContext`、`BeginTx`、`Commit`、`Rollback` 都通过 `Result<_, DbError>` 交由实现者表达失败，本文件不包装这些错误。`QueryRowContext` 唯一新增的错误分支是空结果集，并刻意使用与 Go `database/sql` 相同的消息文本；错误码为 0 且无 SQLSTATE，因此调用者不能靠数据库错误码区分它。

接口不校验 `QueryResult.columns.len()` 与任意一行的长度，不校验 SQLSTATE 恰为五字符，也不限制错误码是否来自 MySQL。它还不表达流式读取、每行扫描错误、多结果集、last-insert-id 或驱动原生 `sql.Result`。参数是位置数组，参数与 `?` 占位符数量是否一致由实现者或数据库处理。

`QueryRowContext` 会静默丢弃第二行及之后的数据，这符合“取一行”的接口含义，但调用方若需要确认唯一性，必须另行检查完整 `QueryResult`。`Transaction::Commit`/`Rollback` 只返回最终错误，不提供未知提交结果、自动回滚或析构回滚保证。

## 并发与资源生命周期

`QueryExecutor: Send + Sync` 使 `dyn QueryExecutor` 和继承它的 `dyn DBExecutor`/`dyn Transaction` 具备跨线程边界所需的超 trait 约束；实现者必须自行保证共享查询状态的同步。该约束不代表方法会并行执行，也没有在本文件中创建线程、锁、任务或通道。

`QueryContext` 和连接级 `ExecContext` 只借用 `&self`，允许实现者通过内部可变性管理连接池或状态。事务写入要求 `&mut self`，在类型层面阻止同一事务句柄同时进行两个可变写调用。`Commit(self: Box<Self>)` 与 `Rollback(self: Box<Self>)` 消费堆分配的动态事务对象，明确终结所有权；若调用者直接丢弃 `Box<dyn Transaction>`，trait 本身没有规定是否回滚，行为取决于实现者的 `Drop` 语义。

返回的 `QueryResult` 完全物化，不持有游标或连接借用，因此资源释放点在具体 `QueryContext` 实现内部。接口没有异步或取消上下文，长查询的中断、截止时间和连接回收均未在此协议中表达。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/dbutil/interface.go`。Go 的 `QueryExecutor` 只声明 `QueryContext(context.Context, string, ...any) (*sql.Rows, error)` 与 `QueryRowContext(...) *sql.Row`；Rust 保留两种操作的语义名称，但去掉 `context.Context`，把可变参数改为 `&[Value]`，把惰性 `*sql.Rows`/`*sql.Row` 改为拥有数据的 `QueryResult`/`Vec<Value>`，并在 trait 内实现单行提取。

Go 的 `DBExecutor` 嵌入 `QueryExecutor`，并声明带 `context.Context` 和 `*sql.TxOptions` 的 `BeginTx`，以及返回 `sql.Result` 的 `ExecContext`。Rust 同样以 supertrait 复用查询能力，但 `BeginTx` 没有上下文和事务选项，返回 `Box<dyn Transaction>`；`ExecContext` 只返回 `u64` 受影响行数，不能提供 last insert ID。

Go 文件没有单独定义 `Transaction`、`Value`、`QueryResult` 或 `DbError`，而是直接使用 `database/sql` 与 `any`。这些 Rust 类型是为当前无驱动依赖的迁移层新增的边界类型。Go 还以空白赋值在编译期验证 `*sql.DB` 和 `*sql.Conn` 满足 `DBExecutor`；Rust 源码只在注释中保留这一来源说明，当前没有对应真实驱动实现或编译期兼容性断言。

因此两版的调用意图相近，但能力并不等价：Rust 目前缺少取消/超时、事务选项、游标式扫描、驱动值全集和真实连接接线。扩展时不能把 Go 的实现兼容性直接推断为 Rust 已具备。

## 扩展指南

若接入真实数据库驱动，最合适的入口是新增独立适配模块，为连接/连接池实现 `QueryExecutor` 与 `DBExecutor`，为驱动事务实现本文件的 `Transaction`；不要把网络或驱动逻辑塞进本接口文件。适配器必须完整映射驱动参数、列顺序、NULL、无符号整数、字节串、浮点值和数据库错误字段，并明确事务对象被丢弃时的回滚策略。

若新增 `Value` 变体，必须同步检查 `pkg/util/dbutil/common.rs` 的 `as_string`/`as_i64`、`pkg/util/dbutil/index.rs::text`、`pkg/util/dbutil/query.rs::ScanRow` 的穷尽匹配，并更新独立测试 `common_test.rs`、`index_test.rs`、`query_test.rs`。新增错误字段时还应检查 `retry.rs` 及 `retry_test.rs` 的分类逻辑。

若改变查询或事务 trait 签名，需同步所有独立测试实现：`common_test.rs` 的 `ScriptedDb`/`TestTransaction`、`index_test.rs::IndexFixture`、`variable_test.rs` 的多个 fixture。真实驱动接入后还应为默认 `QueryRowContext` 增加独立回归测试，覆盖查询错误、空结果、单行及多行只取首行；测试仍应放在独立 `*_test.rs`，不要内嵌到生产源文件。

兼容风险主要来自破坏 trait object、改变 Go 对齐的错误文本或丢失驱动值精度；性能风险主要来自 `QueryResult` 全量物化及 `QueryRowContext` 为取一行仍执行完整查询。若要支持流式结果或异步取消，应新增清晰的独立抽象，而不是在现有返回类型中隐式改变生命周期。

## 验证依据

- 目标源码：`pkg/util/dbutil/interface.rs`；逐项核对了全部公开枚举、结构体、转换实现、trait、默认方法和 supertrait 关系，确认无常量、自由函数、条件编译、异步或 `unsafe`。
- crate 边界：`pkg/util/dbutil/Cargo.toml` 与 `pkg/util/dbutil/lib.rs`；确认库入口、模块声明、再导出和依赖范围。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/util/dbutil` 确认相关源码与独立测试；`node interface.rs::QueryExecutor`、`node interface.rs::QueryExecutor::QueryRowContext`、`node interface.rs::DBExecutor`、`node interface.rs::Transaction`、`node interface.rs::DbError`、`node interface.rs::QueryResult`、`node interface.rs::Value` 核对定义、内部调用和直接导入边。
- Rust 调用方：`pkg/util/dbutil/common.rs`、`index.rs`、`query.rs`、`retry.rs`、`variable.rs`；重点核对了 `common.rs::ExecuteSQLs` 的事务生命周期、`ExecSQLWithRetry`/`DeleteRows` 的写入调用和各结果/错误消费者。
- Go 对照：`pkg/util/dbutil/interface.go`；核对 `database/sql` 的接口签名、上下文/事务选项、`sql.Rows`/`sql.Row`/`sql.Result` 返回形状及 `*sql.DB`、`*sql.Conn` 兼容性断言。
- 独立测试：`pkg/util/dbutil/common_test.rs` 的 `ScriptedDb`、`TestTransaction`，`index_test.rs::IndexFixture`，`variable_test.rs` 的查询 fixture，`query_test.rs` 的所有 `Value` 扫描边界，以及 Go 的 `common_test.go` 分批删除场景。未发现同名 `interface_test.rs` 或 `interface_test.go`，也未发现生产 Rust 类型实现这些 trait。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以规定命令验证文档存在且恰含 11 个固定二级章节，并人工复核所有“当前支持/未支持”陈述均可回指上述文件或符号。
