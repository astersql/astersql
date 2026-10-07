# `cmd/importer/db.rs`

## 文件定位

本文件属于 `astersql-cmd-importer` crate，由 [`cmd/importer/lib.rs`](lib.rs) 以 `pub mod db` 导出。它位于 DDL 解析结果与导入 worker 之间：[`cmd/importer/main.rs`](main.rs) 使用 `createDBs`、`execSQL`、`closeDBs` 管理连接和执行建表语句，[`cmd/importer/job.rs`](job.rs) 的 `doInsert` 使用 `genRowDatas` 为事务批次生成 INSERT。crate 在 [`cmd/importer/Cargo.toml`](Cargo.toml) 中声明为二进制工具，Rust 侧不依赖真实 MySQL 驱动，而由本地 `stubs.rs` 提供数据库抽象。

## 核心职责

文件承担两组职责。第一组把 `parser::table` 及其列元数据转换成可直接执行的单行 INSERT：选择随机或递增取值，处理列级范围、候选集合和直方图，并按 MySQL 类型形成 SQL 字面量。第二组提供轻量数据库辅助：执行非空 SQL、按 worker 数创建连接、关闭单个或全部连接。它不解析 DDL、不调度线程，也不负责事务批处理；这些分别由 `parser.rs`、`main.rs` 和 `job.rs` 完成。

## 主要符号

- `intRangeValue(&column, minv, maxv) -> (i64, i64)`：仅当 `column.min` 非空时解析覆盖下界，并在 `column.max` 非空时覆盖上界；解析失败调用 `stubs::fatal`。
- `randStringValue`、`randInt64Value`：取值优先级均为直方图、显式 `set`、通用随机生成。整数 `set` 项解析失败时记录告警并返回 `0`。
- `nextInt64Value`：应用列级范围后初始化 `column.data` 的整数区间，再从有状态递增器取下一值。
- `intToDecimalString`：把整数尾部按 `decimal` 位切为小数部分，必要时补前导零；独立测试位于 [`cmd/importer/db_test.rs`](db_test.rs)。
- `genRowDatas`、`genRowData`：前者重复生成指定数量的行并在首个错误处返回；后者按 `table.columns` 顺序调用 `genColumnData`，拼出 `insert into <table> (<columns>) values (...);`。
- `genColumnData`：类型分派核心，覆盖 tiny/small/int/bigint、字符串与 blob、float/double、date/datetime/timestamp/duration/year、decimal；未知类型返回 `Error`。
- `execSQL`、`createDB`、`closeDB`、`createDBs`、`closeDBs`：数据库执行和句柄生命周期辅助。
- `pow10_i64`：为 decimal 精度计算十的幂；负指数返回 `0`，`n >= 19` 钳位到 `i64::MAX`。

文件没有模块级可变静态量、trait、结构体、`impl` 或条件编译项；上述函数均为 `pub`，但 crate 主要通过 `main.rs`、`job.rs` 和 crate 内测试使用它们。

## 执行流程

运行入口 `main::run_with_args` 先解析配置与表/索引 DDL，再调用 `createDBs` 建立与 worker 数对应的 `DB` 句柄；随后用首个句柄依次把非空的建表和建索引 SQL 交给 `execSQL`。DDL 成功后，`job::doProcess` 启动 worker，`job::doInsert` 在每个批次中调用 `genRowDatas`，然后逐条在同一事务中执行生成的 SQL并提交。主流程完成后调用 `closeDBs` 尽力关闭所有连接。

单行生成链为 `genRowDatas -> genRowData -> genColumnData`。`genColumnData` 先按 `column.incremental` 及其概率决定本轮是否递增，未命中时扣减正数 `remains`；若列出现在 `table.uniqIndices`，则强制递增。之后读取 unsigned flag，按 MySQL 类型选择范围和生成器，字符串及时间族加单引号，数值族输出裸字面量，decimal 通过 `pow10_i64` 和 `intToDecimalString` 排版。

## 数据与状态

输入表状态来自 `parser::table`：`name` 和 `columnList` 决定 INSERT 目标，`columns` 决定值的顺序，`uniqIndices` 决定唯一列是否强制递增。每个 `column` 的 `min`、`max`、`set`、`hist`、`incremental`、类型及 `data` 共同决定取值；`nextInt64Value` 和其他 `column.data.next*` 方法会推进列级递增状态，概率分支还会修改 `remains`。

连接状态由 `stubs::DB` 内部的共享锁保护。`DB::Exec` 记录 SQL，`DB::Close` 标记关闭；`DB` 克隆共享同一内部状态，供主流程与 worker 使用。`createDB` 把用户、密码、主机、端口和库名交给 `stubs::open_db`，当前 stub 仅组装 DSN 并返回内存句柄，不建立真实网络连接。

## 依赖与调用关系

上游调用关系：`main::run_with_args -> createDBs/execSQL/closeDBs`；`job::doInsert -> genRowDatas`；`genRowDatas -> genRowData -> genColumnData`。crate 内 `parity_test.rs` 还直接调用行生成、decimal 转换、连接创建、空 SQL 执行和关闭函数验证公共契约。

下游依赖分为四类：`config::DBConfig` 提供连接参数；`parser::{table, column}` 提供表与列元数据；`data::{randInt, randInt64, randString}` 和 `rand::{randDate, randTime, randTimestamp, randYear}` 提供随机值；`stubs` 提供 MySQL 类型常量、unsigned flag、错误、日志、随机概率和 `DB`。`Cargo.toml` 只有 `toml` 与 `serde_json` 两项直接外部依赖，因此本文件的数据库行为不是 `database/sql`/MySQL 驱动的真实 Rust 实现。

## 错误处理与边界

配置中的整数范围无法解析会进入 `stubs::fatal`，属于不可恢复配置错误；候选集合中的坏整数只告警并以 `0` 继续。未知列类型由 `genColumnData` 返回 `unsupported column type`，并经行/批生成链向上传播；`parity_test.rs` 使用伪造类型 `0xff` 验证此路径。`execSQL` 对空串直接成功，非空执行错误经 `stubs::trace` 返回；主流程随后把 DDL 错误提升为 fatal。

`genRowData` 假定表至少有一列：它总是在循环后删除末尾逗号；空列集合虽不会 panic，但会生成空 values 列表，未由本文件拒绝。`genColumnData` 直接按 `col_idx` 索引，越界会 panic。字符串/Blob 值只是包单引号，不在此处转义，因此调用者必须维持生成源可直接作为 SQL 字面量的约束。`createDBs` 在中途失败时立即返回且不清理先前句柄；`closeDBs` 则记录单个关闭错误并继续。零连接数返回空向量，但 `main.rs` 随后会访问 `dbs[0]`，正常配置必须保证至少一个 worker/连接。

## 并发与资源生命周期

本文件不创建线程或通道。并发由 `job.rs` 管理：表含递增列时强制单 worker，以保护有状态列生成顺序；否则多个 worker 可共享 `Arc<table>` 和克隆的 `DB`。目标文件中的注释也明确递增状态只应在单 worker 条件下直接访问。随机源的具体线程局部实现位于 `stubs.rs`，不由本文件管理。

连接在 `run_with_args` 中创建或由测试注入，worker 使用克隆句柄执行事务，全部 worker 完成后由 `closeDBs` 显式关闭。`doProcess` 本身不关闭连接，`parity_test.rs::contract_resource_cleanup` 验证事务完成后句柄仍开放，随后关闭才变为 closed。当前 stub 的锁中毒、已关闭执行和显式失败开关都会转为 `Result` 错误。

## 与 Go 版本的对应关系

Rust 函数基本逐一对应 [`cmd/importer/db.go`](db.go) 的同名函数，并保留直方图/集合/随机值优先级、唯一索引强制递增、各整数类型边界、时间族引号、空 SQL no-op、批量连接创建及尽力关闭语义。[`cmd/importer/db_test.go`](db_test.go) 与 `db_test.rs` 对 `intToDecimalString` 使用相同十组输入输出。

主要实现差异是连接层：Go `createDB` 构造 `go-sql-driver/mysql` connector 并返回 `*sql.DB`，Rust 当前调用本地 `stubs::open_db`，只生成可观测的内存 `DB`；因此 Rust 能验证调用顺序、错误传播和资源状态，但不能据此声称已验证真实 MySQL 网络/连接池行为。Rust 的 `pow10_i64` 对 `n >= 19` 直接返回 `i64::MAX`，以稳定替代 Go `math.Pow10` 到 `int64` 的溢出路径。Rust 生成列函数接收列下标而非列指针，越界边界也因此不同。

## 扩展指南

新增 MySQL 类型时应在 `genColumnData` 增加分支，并同步核对类型上下界、unsigned、递增/随机两条路径以及是否需要 SQL 引号；测试应放在独立的 `cmd/importer/db_test.rs` 或 crate 的 `parity_test.rs`，不要嵌入生产文件。修改取值规则时需同时检查 `parser.rs` 如何填充列元数据、`data.rs` 的递增器和 `rand.rs`/`stats.rs` 的随机与直方图语义。

若替换真实数据库实现，应保持 `execSQL` 的空串约定、`createDBs` 返回数量、错误 trace 边界和 `closeDBs` 的尽力清理语义，并补独立集成测试验证连接失败、部分创建失败和关闭错误。调整并发前必须联动 `job::doProcess` 的单 worker 约束；否则唯一列和递增列可能重复或乱序。字符串生成若允许外部 `set` 任意内容，应在明确 Go 兼容策略后增加转义，而不能只在一个分支局部处理。

## 验证依据

- RustCodeGraph：索引状态为 7,032 个 Rust 文件；读取 `cmd/importer/db.rs` 全部 416 行并确认 15 个符号；查询显示目标文件内部调用链 `genRowDatas -> genRowData -> genColumnData`、`createDBs -> createDB`、`closeDBs -> closeDB`，以及 parity 测试对各入口的直接调用。
- 运行链证据：`cmd/importer/main.rs::run_with_args` 调用 `createDBs`、两次 `execSQL`、`closeDBs`；`cmd/importer/job.rs::doInsert` 调用 `genRowDatas` 并在事务中执行结果。
- crate 边界：`cmd/importer/lib.rs` 导出 `db` 并将 `db_test.rs` 作为独立测试模块；`cmd/importer/Cargo.toml` 声明 `astersql-cmd-importer` 的 lib/bin 入口和轻依赖策略。
- 对照与测试：完整核对 `cmd/importer/db.go`、`cmd/importer/db_test.go`、`cmd/importer/db_test.rs`，并读取 `cmd/importer/parity_test.rs` 中正常、边界、错误和资源清理契约。
- 资源实现：`cmd/importer/stubs.rs::DB::{Exec, Begin, Close}`、`Tx::{Exec, Commit}` 与 `open_db` 说明当前为带锁、可注入失败的内存数据库替身。
- 本任务是纯文档分析，按计划未运行 Cargo；结构验证命令及退出状态在任务交付时记录。
