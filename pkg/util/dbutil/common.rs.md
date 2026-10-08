# `pkg/util/dbutil/common.rs`

## 文件定位

`common.rs` 是 `astersql-util-dbutil` crate 的通用数据库辅助层，与 Go 的 `pkg/util/dbutil/common.go` 对应。它不持有真实驱动连接，而是通过 `interface.rs` 定义的 `QueryExecutor` / `DBExecutor` / `Transaction` trait 组装 SQL、解析驱动无关的 `QueryResult`，并统一返回 `DbError`。crate 边界由 `pkg/util/dbutil/Cargo.toml` 声明：包名为 `astersql-util-dbutil`，库入口是 `lib.rs`，直接生产依赖仅有 `astersql-infoschema`。

`lib.rs` 公开 `common` 模块，并在 crate 根再导出 `ColumnName`、`DBConfig` 和 `TableName`。workspace 根又以 `facade_util_dbutil` 引入该 crate，`pkg/lib.rs` 将它放入 facade 导出面。当前仓库中可明确复核的生产调用是 `pkg/util/dbutil/index.rs::ShowIndex -> common::TableName`；其余本文公开函数虽已实现并有独立 Rust 测试覆盖，但未在当前 Rust 生产源中找到稳定的直接调用点。

## 核心职责

- 保存连接配置并从 `MYSQL_HOST`、`MYSQL_PORT`、`MYSQL_USER`、`MYSQL_PSWD` 构造默认配置；`DBConfig::fmt` 特意不输出密码。
- 组装 MySQL/TiDB 标识符、DSN 和查询，并对 `SHOW CREATE TABLE`、行数、随机采样、最小/最大值、schema/table/view 列表等结果做轻量扫描。
- 生成数据 CRC32 校验 SQL，解析 TiDB `SHOW STATS_BUCKETS` 的直方图桶，并将 packed 时间边界还原为可读字符串。
- 读取 TSO、数据库版本、会话变量和时区偏移，提供 TiDB 判定与 SQL mode 封装。
- 实现幂等 DDL 错误忽略、可重试错误重试、批量事务执行和分批删除。

## 主要符号

- 常量：`DefaultRetryTime = 10`、`DefaultTimeout = 10s`、`SlowLogThreshold = 200ms`、`DefaultDeleteRowsNum = 100_000`。当前 Rust 实现真正使用前者和删除批量；超时与慢日志阈值仅作为兼容常量保留。
- 配置与名称：`DBConfig`、`GetDBConfigFromEnv`、`BuildDSN`、`TableName`、`ColumnName`、`escapeName`、`ReplacePlaceholder`。`BuildDSN` 对会话变量按 key 排序，因而输出稳定；`TableName` / `ColumnName` 通过翻倍内部反引号处理 MySQL 标识符。
- 通用值转换：私有 `as_string`、`as_i64`、`first_row` 把 `Value` 转换成后续函数需要的形状，并将空结果集映射为 `DbError`。
- 查询辅助：`GetCreateTableSQL`、`GetRowCount`、`GetRandomValues`、`GetMinMaxValue`、`GetTables`、`GetViews`、`GetSchemas`、`GetCRC32Checksum`。条件文本由调用方传入，只有值参数通过 `Value` 绑定。
- 时区：`DurationOffset(i64)` 以秒为单位；`ParseTimeZoneOffset`、`GetTimeZoneOffset`、`FormatTimeZoneOffset` 分别负责解析、查询和格式化。
- 统计桶：`Bucket { LowerBound, UpperBound, Count }`、`GetBucketsInfo`、`AnalyzeValuesFromBuckets`、`DecodeTimeInBucket`。前者按列/索引名归组，后两者将复合边界拆分并按 MySQL 字段类型解码时间。
- 状态查询：`GetTidbLatestTSO`、`GetDBVersion`、`GetSessionVariable`、`GetSQLMode`、`IsTiDB`。
- 写路径：`ExecSQLWithRetry`、`ExecuteSQLs`、`DeleteRows`，以及私有 `ignoreDDLError` / `ignoreError`。
- parser 边界：`ParserConfig { sql_mode }`、私有 `getParser`、`GetParserForDB`。这里的 `ParserConfig` 是轻量封装，不是 Go 版返回的完整 parser 实例。

## 执行流程

1. 读路径从 `QueryExecutor::QueryContext` 或默认的 `QueryRowContext` 进入。函数先用 `TableName` / `ColumnName` 拼接库表列名，再传入 `&[Value]` 绑定参数，最后通过 `as_string`、`as_i64` 或结果列名索引取值。
2. `GetCRC32Checksum` 遍历 `TableInfo.columns`，同时构造列值和 `ISNULL(column)` 序列，以 `BIT_XOR(CRC32(CONCAT_WS(...)))` 产生范围校验和；空校验值归一为 `0`。
3. `GetBucketsInfo` 按列名定位 `Column_name`、`Count`、`Lower_Bound`、`Upper_Bound`，把每行放入 `HashMap<String, Vec<Bucket>>`。如果表存在 `PRIMARY` 但结果键是单列主键的列名，则依 `TableInfo.indices` / `model_meta` 找到主键列并把该键规一为 `PRIMARY`。
4. `AnalyzeValuesFromBuckets` 先去掉外层括号，按 `", "` 拆分复合边界，并要求值数与列类型数相等。对 DATETIME/TIMESTAMP/DATE，若字符串不呈时间形状，则 `DecodeTimeInBucket` 按 TiDB packed uint64 位布局拆出年月日时分秒与微秒。
5. `ExecSQLWithRetry` 最多调用 `DBExecutor::ExecContext` 十次：成功立即返回；错误码 1007/1008/1050/1051/1060/1061 作为幂等 DDL 成功；`retry.rs::IsRetryableError` 判定为可重试时在后续尝试前休眠 10ms；其他错误直接传播。
6. `ExecuteSQLs` 先检查 SQL 与参数组数量相等，然后 `BeginTx`，顺序执行每条 SQL。首个执行错误会触发一次回滚并返回原错误；全部成功后提交。
7. `DeleteRows` 为固定 `LIMIT 100000` 的 DELETE 循环。受影响行数等于上限时继续，低于上限时认为删除完成。

## 数据与状态

本文不包含全局可变状态。`DBConfig`、`DurationOffset`、`Bucket` 和 `ParserConfig` 都是值类型；查询结果所有权在函数内部消费。`BuildDSN` 为消除 `HashMap` 遍历无序性而排序变量，但不缓存结果。

`GetBucketsInfo` 的主要不变量是所需列必须在结果集中存在且非 NULL；桶保留查询返回的行序。`AnalyzeValuesFromBuckets` 的不变量是拆分值与 `column_types` 一一对应。`ExecuteSQLs` 要求 `sqls.len() == args.len()`；`DeleteRows` 依赖每批删除能实际推进，否则影响行数永返回上限时可能一直循环。

## 依赖与调用关系

- 下游内部依赖：`interface.rs` 提供 `Value`、`QueryResult`、`DbError` 及数据库/事务 trait；`retry.rs::IsRetryableError` 定义可重试错误码和 1105 消息兼容逻辑；`types.rs::IsTimeTypeAndNeedDecode` 识别 7/10/12 三种时间类型。
- 下游 crate 依赖：`astersql_infoschema::TableInfo` 为校验和列表与统计桶主键规一提供元数据。
- 文件内主要调用边：`GetCreateTableSQL/GetRowCount/GetMinMaxValue/GetDBVersion -> first_row`；多数查询函数 `-> QueryExecutor::QueryContext`；`GetTimeZoneOffset -> QueryRowContext -> ParseTimeZoneOffset`；`GetTables/GetViews -> queryTables`；`AnalyzeValuesFromBuckets -> IsTimeTypeAndNeedDecode -> DecodeTimeInBucket`；`GetSQLMode -> GetSessionVariable`；`IsTiDB -> GetDBVersion`；`ExecSQLWithRetry -> ignoreError/IsRetryableError/DBExecutor::ExecContext`。
- 上游调用与导出：`lib.rs` 直接导出模块并再导出三个常用符号；`index.rs::ShowIndex` 调用 `TableName`；`pkg/lib.rs` 通过 workspace facade 再导出 crate。Cargo 还在 `pkg/ddl`、`pkg/util/importer`、`pkg/util/ddl-checker` 和 planner planstats 测试 crate 中声明了包依赖，但本次搜索未找到它们对本文具体 API 的 Rust 直接引用。
- RustCodeGraph `node --file` 为文件报告了多个“used by”文件，但其中包含 Go 文件和同名 AST 符号；因此本文不把那些同名命中当作 Rust 调用边，而以限定 crate/文件的源码搜索作为上游结论依据。

## 错误处理与边界

所有数据库交互错误都以 `DbError` 传播。本地生成错误使用 `code = 0`，包括空结果、必要列缺失/NULL、TSO 无法读取、SQL/参数数量不一致和主键桶元数据不完整。时区、packed 时间和 parser mode 的纯解析函数使用 `String` 错误，其数据库包装入口再转为 `DbError`。

`ExecSQLWithRetry` 对六个幂等 DDL 错误码返回成功，这是显式业务契约，不是丢弃所有错误。重试十次耗尽后返回最后一个可重试错误。`ExecuteSQLs` 回滚失败会被忽略，保留首个 SQL 执行错误；提交错误直接返回。

SQL 安全边界需要由调用方理解：schema/table/column 会做反引号转义，查询值通过 `Value` 绑定；但 `where_clause`、`limit_range`、`collation`、会话变量名和 `ReplacePlaceholder` 是文本拼接，不应接收未信任输入。`ReplacePlaceholder` 仅供日志展示，不理解 SQL 字面量，也不转义参数。`AnalyzeValuesFromBuckets` 沿用 Go 的简单 `", "` 分割，对自身含逗号或括号的值不是完整 SQL datum parser。

## 并发与资源生命周期

`QueryExecutor` 要求 `Send + Sync`，因此本文的查询辅助函数可被多线程共享执行器调用；函数本身不创建异步任务、线程池、通道或锁。`ExecSQLWithRetry` 使用当前线程 `sleep(10ms)`，会阻塞调用线程，且没有 cancellation/context 检查。

事务生命周期由 `ExecuteSQLs` 独占管理：`BeginTx` 成功后，顺序执行，错误路径消费 boxed transaction 执行 `Rollback`，成功路径消费它执行 `Commit`。`QueryResult` 是全量内存结果，所以 Rust 函数没有 Go `sql.Rows.Close` 式的游标资源；连接和真实驱动资源的生命周期属于 `QueryExecutor` / `DBExecutor` 实现方，本 crate 当前不提供 Go `OpenDB` / `CloseDB` 的等价实现。

## 与 Go 版本的对应关系

Rust 的常量、`DBConfig`、大部分查询函数、标识符转义、校验和、统计桶、时区、重试、事务和分批删除均与 `common.go` 同名逻辑对应。`common_test.go` 中的标识符转义、可忽略 DDL 错误、两批删除、parser mode、统计桶时间值和时区格式化场景，在独立 `common_test.rs` 的可执行测试区域中有对应断言；文件前半的 `GO_REFERENCE` 仅是未编译的迁移参考字符串。

已验证的重要差异如下：

- Go `OpenDB` / `CloseDB` 使用 mysql connector、Ping 和 Close；Rust 只有 `BuildDSN` 和执行器 trait，不打开真实连接。
- Go API 传递 `context.Context`、遍历 `sql.Rows` 并检查 `rows.Err()`；Rust API 一次性接收 `QueryResult`，没有 context 取消或流式游标错误。
- Go `ExecSQLWithRetry` 记录慢 SQL，并在重试等待时响应 context 取消；Rust 不记录日志，使用固定阻塞 sleep，因此 `DefaultTimeout` / `SlowLogThreshold` 没有接线到执行流程。
- Go `GetBucketsInfo` 按 9/10/11 列整行扫描，Rust 按不区分大小写的列名选取四个所需字段，对列顺序和额外列更宽容，但对所需值 NULL 显式报错。
- Go 通过 `types.ParseTime` 判断可读时间、通过 `Time::FromPackedUint` 校验 packed 值；Rust 使用简单字符串形状判定和位解码，不执行完整日期合法性校验。
- Go `GetSQLMode` 返回位掩码、`getParser` / `GetParserForDB` 返回真实 `parser.Parser`；Rust `GetSQLMode` 与 `ParserConfig` 只保留字符串，且 `GetParserForDB` 直接保留数据库返回值，没有调用私有 `getParser` 进行验证/规一。
- Go `DeleteRows` 递归分批，Rust 用循环，对外停止条件相同但避免递归栈增长。
- Go `ReplacePlaceholder` 经 `fmt.Sprintf` 执行批量替换；Rust 逐个问号消费参数，并对缺少的参数保留 `?`。

## 扩展指南

- 增加查询辅助时，优先使用 `QueryExecutor` 和 `Value` 绑定；新的标识符必须经 `TableName` / `ColumnName` / `escapeName`，条件 SQL 若必须由调用方传入，需在 API 和测试中明确“只接受可信 SQL 片段”。
- 扩展结果扫描时，按 `GetBucketsInfo` 的方式使用列名定位，而不假设服务器版本的列顺序；同时为缺列、NULL、非法数值和空结果添加 `common_test.rs` 回归。
- 扩展错误分类时，幂等忽略码改 `ignoreDDLError`，可重试分类改 `retry.rs::IsRetryableError`；两者语义不同，必须分别在 `common_test.rs` 和 `retry_test.rs` 覆盖。
- 如要达到 Go 版完整等价，应在独立执行器/连接层接入驱动、超时、context/cancellation 和日志，不应把这些资源职责塞入纯 SQL 辅助函数。
- 修改 packed 时间时需同步 `AnalyzeValuesFromBuckets`、`DecodeTimeInBucket`、`is_time_string`，并保留 `common_test.rs::bucket_time_decoding_matches_go_cases` 与 `common_test.go::TestAnalyzeValuesFromBuckets` 的六个 DATETIME/TIMESTAMP/DATE 场景。
- 修改事务路径时需补充 `common_test.rs` 中 commit 失败、exec 失败后 rollback、rollback 失败和 SQL/参数数量不等的独立测试；Rust 单元测试继续放在 `common_test.rs`，不内嵌到生产源文件。
- 任何与 Go 行为对齐的变更都应同时核对 `common.go` 和 `common_test.go`；不要为了编译或测试通过而删减 context、错误、parser 或结果集语义。

## 验证依据

- RustCodeGraph 索引状态：仓库已索引 11,467 个文件；使用 `rustcodegraph files --filter pkg/util/dbutil`确认 `common.rs`、`common_test.rs`、Go 对照与同 crate 模块均在索引中。
- 符号与源码：用 `rustcodegraph node --file pkg/util/dbutil/common.rs --offset 1 --limit 500` 和 `--offset 501 --limit 500` 读取全部 804 行；用 `query --kind function` 分辨 `GetBucketsInfo`、`AnalyzeValuesFromBuckets`、`ExecSQLWithRetry`、`ExecuteSQLs`、`DeleteRows`、`GetParserForDB`、`GetCRC32Checksum` 的 Go/Rust 同名定义。
- 直接依赖：用 RustCodeGraph 读取 `interface.rs`、`retry.rs`、`types.rs`，确认 trait 边界、错误分类和时间类型判定；读取 `index.rs` 确认 `ShowIndex -> TableName` 调用边。
- crate 与导出：读取 `pkg/util/dbutil/Cargo.toml`、`pkg/util/dbutil/lib.rs`、根 `Cargo.toml` 的 `facade_util_dbutil` 声明及 `pkg/lib.rs` facade 再导出；用 Cargo 声明搜索确认其他依赖 crate。
- Go 对照：读取 `pkg/util/dbutil/common.go` 和 `pkg/util/dbutil/common_test.go`，逐类核对配置、查询、统计桶、时区、重试、事务、删除和 parser 语义。
- Rust 测试：读取独立 `pkg/util/dbutil/common_test.rs`；其可执行区域覆盖查询结果形状、DSN/配置、时区、统计桶时间解码、DDL 错误忽略、重试、分批删除与基本事务成功路径。本任务按计划为纯文档分析，未运行 Cargo。
