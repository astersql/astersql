# `pkg/statistics/handle/usage/predicatecolumn/predicate_column.rs` 逻辑说明

## 文件定位

本文件是 `astersql-statistics-handle-usage-predicatecolumn` crate 的业务实现，crate 入口 `pkg/statistics/handle/usage/predicatecolumn/lib.rs` 通过 `mod predicate_column` 加载它并重新导出全部公共符号。它负责以受限 SQL 访问系统表 `mysql.column_stats_usage`，在 Rust 侧承载 Go 包 `pkg/statistics/handle/usage/predicatecolumn` 中列统计使用时间的读取、谓词列查询、残留记录清理和保存语义。

该 crate 已列入根 `Cargo.toml` workspace，直接依赖由同目录 `Cargo.toml` 声明：`chrono`/`chrono-tz` 提供时间转换，`astersql-meta-model` 提供 `TableItemID`，`astersql-statistics-handle-types` 提供统计时间、会话、InfoSchema 和受限 SQL 抽象。其移植元数据状态仍是 `candidate`。上层 `pkg/statistics/handle/usage/Cargo.toml` 和 `pkg/statistics/handle/storage/Cargo.toml` 对此 crate 的依赖位于 `target.'cfg(any())'.dependencies`，而生产 Rust 源码中没有这些公共函数的调用点；因此当前事实是“实现和独立测试已存在，但尚未接入 Rust 应用主链”，不能把 Go 主链的可用性等同于 Rust 已接线。

## 核心职责

- 用 `LOAD_ALL_SQL`、`LOAD_TABLE_SQL`、`PREDICATE_COLUMNS_SQL`、`CLEANUP_DROPPED_COLUMNS_SQL` 和 `REPLACE_USAGE_SQL` 固定系统表访问协议，包括 `%?` 参数占位和会话时区/UTC 的 `CONVERT_TZ` 转换。
- 用 `PredicateColumnExecutor`、`PredicateColumnInfoSchema`、`PredicateColumnSession` 把业务逻辑与实际 SQL 会话、最新 InfoSchema 解耦，以便生产适配器和测试 Mock 共用同一流程。
- 用 `ExecRowsPredicateColumnExecutor` 将上述窄接口接到 `stats_types::ExecRows`，并把通用 `StatsRow` 解码为本文件的两类记录；用 `ExecRowsPredicateColumnInfoSchema` 和 `ExecRowsPredicateColumnSession` 组合最新表结构视图与执行器。
- 把查询结果转换为 `HashMap<TableItemID, ColStatsTimeInfo>`，保持 Go 版对空 ID、空时间、无效谓词时间和已删除列的处理规则。
- 逐条执行 `REPLACE` 保存用量，任何 SQL 或解码错误立即向上传播，不伪装为成功。

## 主要符号

- 五个 SQL 常量：`LOAD_ALL_SQL` 读取全表，`LOAD_TABLE_SQL` 按 `table_id` 读取，`PREDICATE_COLUMNS_SQL` 查询 `last_used_at IS NOT NULL` 的候选，`CLEANUP_DROPPED_COLUMNS_SQL` 删除不在当前 schema 中的列，`REPLACE_USAGE_SQL` 覆盖写入一列的两种时间。
- `PredicateColumnError(String)`：本地错误封装；`new` 接受可转字符串的消息，并实现 `Display` 与标准 `Error`。
- `SqlArg`：SQL 边界参数，支持 `I64`、用于 `NOT IN (%?)` 的 `StringList`，以及时间或字面量 `"NULL"` 使用的 `String`。
- `ColumnStatsUsageRecord`：加载查询的中间行，保留可空表/列 ID 和两个 UTC 朴素时间。保留可空 ID 是为了在业务层执行与 Go 一致的整行跳过规则。
- `PredicateColumnRecord`：谓词查询的中间行，包含列 ID 及 `CONVERT_TZ(last_used_at, ...)` 是否为空的标记。
- `PredicateColumnExecutor`：三个操作组成的 SQL 端口，分别解码用量查询、谓词查询和执行写语句。
- `ExecRowsPredicateColumnExecutor`：生产 SQL 适配器。`arguments` 将 `SqlArg` 转为 `StatsSqlValue`；`rows` 调用 `stats_types::ExecRows`；其 trait 实现负责按列解码。
- `rowValue`、`nullableI64`、`nullableTime`：内部严格解码器，分别检查列是否存在、接受有符号/可安全转换的无符号整数、接受字符串/UTF-8 字节形式的 MySQL 时间。
- `PredicateColumnInfoSchema` 与 `ExecRowsPredicateColumnInfoSchema`：把 `InfoSchema::TableByID`/`Table::Meta().columns` 收窄为“按表取得当前列 ID”；表不存在返回 `None`。
- `PredicateColumnSession` 与 `ExecRowsPredicateColumnSession`：统一提供 SQL 执行器和最新 InfoSchema；`new` 用一个 `SessionContext` 与一个 `InfoSchema` 建立借用型生产适配器。
- `localizedTime`：把查询已经转换到 UTC 的 `NaiveDateTime` 解释为 UTC，再变换到调用方时区，最后构造 MySQL `TIMESTAMP`（`DefaultFsp`）。
- `loadColumnStatsUsage`：加载流程的内部公共实现；`LoadColumnStatsUsage` 和 `LoadColumnStatsUsageForTable` 分别固定全量 SQL 和按表 SQL。
- `cleanupDroppedColumnStatsUsage`：基于最新 schema 构造保留列字符串列表并执行清理；表已消失时安全空操作。
- `GetPredicateColumns`：先清理再查询，过滤时区转换后为空的时间并返回列 ID。
- `timeArgument` 与 `SaveColumnStatsUsageForTable`：将可空 `Time` 转成 Go 兼容参数并逐行保存。

## 执行流程

加载全量或单表用量时，公共入口选择 SQL 和参数后进入 `loadColumnStatsUsage`。它先通过 `session.executor().query_usage` 取得规范化记录；生产适配器经 `ExecRows` 执行 SQL，并在 ID 有效时才解码时间。业务层再次匹配 `table_id`/`column_id`，任一为空便跳过该行；有效行构造 `TableItemID { IsIndex: false, IsSyncLoadFailed: false }`，两个可空 UTC 时间分别经 `localizedTime` 转为调用方时区的 `stats_types::Time`，最后插入 HashMap。重复键按 HashMap 插入语义由后行覆盖前行。

查询谓词列时，`GetPredicateColumns` 首先调用 `cleanupDroppedColumnStatsUsage`。若最新 InfoSchema 找不到该表，则不执行 DELETE；否则把当前列 ID 转成十进制字符串列表，执行 `table_id = %? AND column_id NOT IN (%?)`。清理成功后才运行 `PREDICATE_COLUMNS_SQL`；查询适配器只检查第二列是否为 SQL NULL，业务层过滤转换结果为空的记录并按查询返回顺序产出列 ID。清理失败会阻止后续查询。

保存时，`SaveColumnStatsUsageForTable` 遍历输入 HashMap。每项绑定表 ID、列 ID，以及由 `timeArgument` 生成的两个字符串参数；有值时使用 `Time::String`，无值时绑定字面字符串 `"NULL"`，由 SQL 内的 `CONVERT_TZ` 处理。每项执行一次 `REPLACE`，首个失败通过 `?` 立即返回；此前成功写入的行不会由本函数回滚。

## 数据与状态

本文件自身不持有全局可变状态。SQL 文本是静态常量；生产适配器只借用外部 `SessionContext`/`InfoSchema`。读取结果由调用栈内的 `Vec` 和 `HashMap` 承载，保存输入是借用的 HashMap。

时间边界分两段：SQL 用 `CONVERT_TZ(..., @@TIME_ZONE, '+00:00')` 将库内会话时间转到 UTC，`nullableTime` 将文本解析为不带时区的 UTC 值；`localizedTime` 再按传入的 `Tz` 生成业务层 MySQL 时间。保存方向则把 `Time::String` 视作 UTC 字符串，并由 `CONVERT_TZ(%?, '+00:00', @@TIME_ZONE)` 写回会话时区。`ColStatsTimeInfo` 的两个时间独立可空。

`TableItemID` 在此始终表示列而非索引，故 `IsIndex` 固定为 `false`；`IsSyncLoadFailed` 也固定为 `false`。当前列集合来自最新 InfoSchema，而不是查询快照，体现清理操作有意使用最新 schema 的约束。

## 依赖与调用关系

向下调用关系是：四个公共业务入口 → `PredicateColumnSession` → `PredicateColumnExecutor`/`PredicateColumnInfoSchema`；生产实现继续到 `stats_types::ExecRows`、`SessionContext`、`InfoSchema::TableByID`、表元数据列集合，以及 `stats_types::NewTime`/`FromGoTime`。`ExecRowsPredicateColumnExecutor::arguments` 是本地参数类型通往统计 SQL 通用类型的唯一转换点。

RustCodeGraph 在目标文件中识别出 56 个符号，并能定位四个公共入口、内部加载/清理函数与生产适配器。对精确入口执行 `callers`/`callees` 时本次索引未产出边，因此又用仓库源码搜索复核：除 `predicate_column_test.rs` 外没有生产 Rust 调用点；`pkg/statistics/handle/types/interfaces.rs` 只声明了相近的 `StatsUsage::LoadColumnStatsUsage`/`GetPredicateColumns` 接口，并未连接本 crate。Cargo 层仅能证明 workspace 成员关系和被禁用的候选依赖，不能证明运行时已接线。

Go 主链则已明确接通：`pkg/statistics/handle/usage/predicate_column.go` 的 `statsUsageImpl::LoadColumnStatsUsage`/`GetPredicateColumns` 从 session pool 取得上下文并调用同路径 Go 包；规划/ANALYZE 等上层通过 `StatsUsage` 接口消费结果。该 Go 调用关系仅用于说明移植目标，不能当作 Rust 调用证据。

## 错误处理与边界

- `ExecRows` 的任何错误被字符串化为 `PredicateColumnError`；SQL 执行、查询和保存入口均原样向上传播。
- 行缺列由 `rowValue` 报出列下标；整数列只接受 `Null`、`Integer` 或能放入 `i64` 的 `Unsigned`，类型不符或溢出时报错。
- 时间只接受 `Null`、UTF-8 `Bytes` 或 `String`，格式必须匹配 `%Y-%m-%d %H:%M:%S%.f`；非法 UTF-8、类型或格式均报错。
- 用量行的表 ID 或列 ID 为空时，生产适配器刻意不解码后续时间，业务加载层随后跳过整行；这避免本应忽略的行因坏时间而失败。
- 谓词记录用 `unwrap_or_default` 将空列 ID 解码为 `0`，与 Go `GetInt64` 的零值倾向一致；真正过滤依据是转换后时间是否为空。若将来要强化空列 ID 校验，必须先确认 Go 兼容要求。
- 表在最新 InfoSchema 中不存在时，清理为空操作但谓词查询仍继续；这是 Go 版的安全退化，不是错误。
- 当前列列表为空时，本函数仍会把空 `StringList` 交给执行器；最终 SQL 展开行为属于 `ExecRows`/SQL 参数层，本文件没有额外保护，扩展时需专门验证。
- 保存不是批量事务封装：HashMap 迭代顺序不稳定，失败前可能已经落盘若干行，调用方不能假定全有或全无。

## 并发与资源生命周期

三个边界 trait 都要求 `Send + Sync`，允许会话、执行器和 InfoSchema 抽象跨线程安全地被引用；本文件不创建线程、异步任务、锁或通道。生产适配器使用显式生命周期借用外部对象，不拥有也不关闭 session、事务、结果集或 InfoSchema。

每次 SQL 调用的资源管理委托给 `stats_types::ExecRows`。本文件只接收已经物化的 `Vec<StatsRow>`，随后同步解码并释放局部集合。`SaveColumnStatsUsageForTable` 串行写入，不并发提交；`GetPredicateColumns` 的“读取最新 schema、执行 DELETE、再 SELECT”也没有在本文件内建立原子快照或事务，是否处于同一事务由外层 session/执行选项决定。

测试中的 `Mutex<VecDeque<...>>` 和 `Arc` 只用于独立 Mock/适配器验证，不是生产状态模型。

## 与 Go 版本的对应关系

Rust 的 `loadColumnStatsUsage`、`LoadColumnStatsUsage`、`LoadColumnStatsUsageForTable`、`GetPredicateColumns`、`cleanupDroppedColumnStatsUsage`、`SaveColumnStatsUsageForTable` 与同目录 Go 文件中的同名逻辑逐一对应，五条 SQL 的字段、过滤条件和时区方向保持一致。

关键语义也保持一致：空表/列 ID 行先跳过；有效时间先按 UTC 解释再转目标 location；谓词查询前尝试清理已删列；表不存在时跳过清理；清理列 ID 转字符串列表；转换后的 `last_used_at` 为空时过滤；缺失保存时间使用字符串 `"NULL"`；逐行 `REPLACE` 且首错返回。Rust 额外引入 trait 和类型化中间记录，以替代 Go 直接依赖 `sessionctx.Context`、`chunk.Row` 和类型断言的方式。

独立 Rust 测试 `pkg/statistics/handle/usage/predicatecolumn/predicate_column_test.rs` 覆盖通用 `ExecRows` 参数/行适配、空 ID 时不解析坏时间、谓词时间只检查 NULL、时区转换、按表绑定、清理顺序、表缺失、`"NULL"` 参数和写错误传播。Go 的跨层回归位于 `pkg/statistics/handle/usage/predicate_column_test.go`，其中 `TestCleanupPredicateColumns` 证明删列后调用 `GetPredicateColumns` 只返回现存列；其余 ANALYZE 测试说明谓词列结果在 Go 应用主链中的用途。当前 Rust 测试没有证明上层生产接线或真实数据库 SQL 展开。

## 扩展指南

新增查询字段或改变 SQL 时，应同步修改对应 SQL 常量、`ColumnStatsUsageRecord`/`PredicateColumnRecord`、`ExecRowsPredicateColumnExecutor` 的列下标解码和独立测试中的 `StatsRow`，并与 Go 同路径实现核对列顺序、NULL 和时区语义。新增参数种类应集中扩展 `SqlArg` 与 `arguments`，避免业务入口绕过类型转换直接拼接 SQL。

若要将实现接入 Rust 主链，优先在上层 usage 实现中用真实 `SessionContext` 与最新 `InfoSchema` 构造 `ExecRowsPredicateColumnSession`，实现 `StatsUsage` 对应方法，并把依赖从 `cfg(any())` 候选区移入实际 target；这属于后续代码任务，不应在本文档任务中宣称已经完成。接线测试至少要覆盖 session pool/事务选项、真实 InfoSchema、系统表 SQL 参数展开以及 Go 集成测试表达的删列与 ANALYZE 行为。

改变清理行为时需关注空列集合、并发 DDL、最新 schema 与 SQL 操作之间的竞态；改变保存行为时需明确是否要求事务原子性、稳定顺序或批量性能。涉及时间时必须同时验证非 UTC 会话时区、夏令时边界、无效 MySQL 时间和小数秒。保持测试逻辑在独立的 `predicate_column_test.rs`，不要嵌入生产源文件。

## 验证依据

- 源码与边界：`pkg/statistics/handle/usage/predicatecolumn/predicate_column.rs`、同目录 `lib.rs` 和 `Cargo.toml`；根 `Cargo.toml` 的 workspace 成员与 facade 条目。
- RustCodeGraph：`status` 显示索引包含本目录的 Rust/Go 文件；`files --filter` 定位四个文件；`node --file ... --offset 1 --limit 500` 读取目标 418 行与 56 个符号；`query` 定位四个公共入口、内部加载/清理函数和 `ExecRowsPredicateColumnSession`。精确 `callers`/`callees` 本次没有返回边，故调用状态由仓库源码搜索交叉验证，而非据此假定无调用。
- Rust 调用与 crate 状态：源码搜索公共入口和适配器只命中本实现、独立测试以及 `StatsUsage` 的相近接口声明；`pkg/statistics/handle/usage/Cargo.toml`、`pkg/statistics/handle/storage/Cargo.toml` 将本 crate 放在 `cfg(any())` 依赖区。
- Go 对照：`pkg/statistics/handle/usage/predicatecolumn/predicate_column.go`；上层包装 `pkg/statistics/handle/usage/predicate_column.go`。
- 测试证据：`pkg/statistics/handle/usage/predicatecolumn/predicate_column_test.rs`；Go 集成测试 `pkg/statistics/handle/usage/predicate_column_test.go`，重点为 `TestCleanupPredicateColumns` 及谓词列 ANALYZE 场景。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前仅运行任务指定的 11 章节结构验证，并人工复核“职责、流程、扩展点、当前未接线限制”均有上述直接证据。
