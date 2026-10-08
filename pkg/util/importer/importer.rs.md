# `pkg/util/importer/importer.rs`

## 文件定位

`importer.rs` 是 `astersql-util-importer` crate 的顶层编排文件。它不实现 SQL 解析、连接、数据生成或线程调度细节，而是用唯一的公开函数 `process` 把这些子模块串成一次完整导入：解析 DDL、建立每个 worker 所需的连接、执行建表与建索引语句、并发生成和插入数据，最后关闭全部连接（`pkg/util/importer/importer.rs:21-57`）。

crate 边界由 `pkg/util/importer/Cargo.toml` 定义，库入口是 `pkg/util/importer/lib.rs`；后者声明 `pub mod importer` 并通过 `pub use importer::*` 再导出 `process`。根 workspace 又以 `facade_util_importer` 引入该 crate（根 `Cargo.toml:1474`），并由 `pkg/lib.rs:1917` 继续公开。仓库引用搜索没有发现 `process` 的 Rust 生产调用点，因此当前事实是“API 已公开但仓库内尚未接入实际入口”，不能把它描述为已进入 `tidb-server` 主链。

## 核心职责

- 用 `Table::new` 建立空表元数据，并按固定顺序调用 `parse_table_sql`、`parse_index_sql`；索引解析依赖前一步已经填充的列和表信息。
- 在触碰 `databases[0]` 前拒绝 `worker_count == 0`，避免创建空连接数组后发生越界，同时返回可处理的 `ImporterError::InvalidConfig`。
- 调用 `create_databases` 建立恰好 `worker_count` 个连接，使后续 `process_jobs` 能为每个 worker 分配一个连接。
- 只在第一个连接上顺序执行建表 SQL和建索引 SQL，再把解析后的 `Table` 放入 `Arc` 交给并发作业层共享。
- 无论 DDL 或数据导入成功还是失败，都在主体闭包结束后调用 `close_databases`；关闭错误不替换主体结果。

该文件的职责是生命周期编排和错误优先级，而不是实现事务或并发算法；这些行为分别位于 `db.rs` 和 `job.rs`。

## 主要符号

### `pub fn process(config: &Config, connector: &dyn DatabaseConnector) -> Result<ProcessReport, ImporterError>`

文件内唯一的函数，也是唯一生产符号（RustCodeGraph 节点 `pkg/util/importer/importer.rs:30:function:process`）。

- `config: &Config` 提供 `table_sql`、`index_sql`、`db_config`、`worker_count`、`job_count` 和 `batch`；函数只借用配置，不修改它。
- `connector: &dyn DatabaseConnector` 把具体数据库驱动留给调用方注入；`DatabaseConnector::open` 返回共享的 `Arc<dyn Database>`（`pkg/util/importer/db.rs`）。
- 成功值 `ProcessReport` 来自 `process_jobs`，包含请求的 job 数、耗时和粗略 TPS（`pkg/util/importer/job.rs`）。
- 失败值统一为 `ImporterError`，解析、配置、连接、DDL、数据生成、事务以及 worker panic 都通过 `Result` 返回。

本文件没有模块级常量、类型、trait、`impl`、宏或条件编译项。

## 执行流程

1. 创建空 `Table`。
2. 解析 `config.table_sql`。失败立即返回，尚未创建数据库连接。
3. 解析 `config.index_sql`。失败同样立即返回，尚未创建连接。
4. 检查 `worker_count`。值为零时返回 `InvalidConfig("worker-count must be positive")`。
5. 调用 `create_databases(connector, &config.db_config, worker_count)`。如果中途打开失败，`create_databases` 会先关闭此前已打开的连接，再把原错误返回（`pkg/util/importer/db.rs`）。
6. 在局部闭包中形成导入主体：先在 `databases[0]` 执行 `table_sql`，再执行 `index_sql`，最后调用 `process_jobs(Arc::new(table), &databases, job_count, worker_count, batch)`。
7. 闭包通过 `?` 保留遇到的第一个主体错误；闭包退出后，无条件调用 `close_databases(&databases)`。
8. 丢弃收集到的关闭错误，原样返回主体的 `Result<ProcessReport, ImporterError>`。

顺序不变量是“解析先于连接、建表先于建索引、DDL 先于数据写入、关闭晚于全部主体操作”。本函数不做 DDL 回滚：如果建表成功而建索引或导入失败，数据库中的既有副作用仍然存在。

## 数据与状态

`process` 自身没有全局或静态状态。局部 `Table` 在解析阶段独占可变，完成后转成 `Arc<Table>`；其列元数据以及唯一值生成器随后由 worker 共享。`databases` 是 `Vec<Arc<dyn Database>>`，长度在成功路径上等于 `worker_count`，第一个元素同时承担 DDL 执行和第一个 worker 的数据写入。

配置和返回值的语义来自相邻模块：

- `Config` 的 `worker_count` 决定连接数和线程数，`job_count` 决定待生成行数，`batch` 决定每个事务包含的行数（`pkg/util/importer/config.rs`）。
- `process_jobs` 使用容量为 `16 * worker_count` 的同步通道传递空 job token；worker 收到 token 后才生成行数据，并按 `batch` 开启事务、执行 INSERT、提交（`pkg/util/importer/job.rs`）。
- `ProcessReport.jobs` 记录请求的 `job_count`，而不是从数据库回读的实际行数；成功返回意味着所有 worker 及生产者均正常 join，且 worker 错误已传播。

## 依赖与调用关系

直接下游调用边可由源码导入和 `process` 函数体确认：

- `config::{Config, ImporterError}`：输入配置与统一错误类型。
- `parser::{Table, parse_table_sql, parse_index_sql}`：构造表模型并解析两类 DDL。
- `db::{DatabaseConnector, create_databases, execute_sql, close_databases}`：抽象连接器、连接生命周期与非空 SQL 执行。
- `job::{ProcessReport, process_jobs}`：并发批量导入及统计报告。
- `std::sync::Arc`：把解析完成的表模型转为跨 worker 的共享所有权。

上游公开链是 `importer.rs::process` → `pkg/util/importer/lib.rs` 的通配再导出 → 根 workspace 的 `facade_util_importer` → `pkg/lib.rs` 的 facade 再导出。RustCodeGraph 的精确符号查询确认 `process` 节点存在，但精确 callers/callees 命令未在 30 秒查询窗口内返回；随后使用全仓库引用搜索，没有发现生产或测试代码直接调用该函数。因此这里只记录“公开可用”，不声称存在运行时调用者。

`pkg/util/importer/Cargo.toml` 的普通 `[dependencies]` 当前为空；列在 `target.'cfg(any())'.dependencies` 下的解析器与 dbutil crate 因 `cfg(any())` 恒假而不参与正常构建。当前 `process` 仅依赖本 crate 子模块和标准库。

## 错误处理与边界

- 两个解析调用、连接创建、两个 DDL 执行和 `process_jobs` 都使用 `?`，因此保留最先发生的错误并停止后续主体步骤。
- `worker_count == 0` 在连接创建前被显式拒绝。这既是配置约束，也是安全访问 `databases[0]` 的前置条件。
- `batch == 0` 不在本文件检查，而由 `process_jobs` 返回 `InvalidConfig`；由于检查发生在 DDL 执行之后，此时表和索引可能已经创建。若希望避免此副作用，应在扩展时谨慎评估是否把完整配置验证提前。
- `index_sql` 可以为空：解析器应接受空输入，`execute_sql` 对空字符串直接成功（`pkg/util/importer/db.rs::execute_sql`）。`table_sql` 是否有效由 `parse_table_sql` 决定。
- `create_databases` 的部分成功由该函数内部清理；完全成功后的所有路径则由 `process` 的闭包后清理。
- `close_databases` 会尝试关闭每一个连接并收集全部错误，但 `process` 有意忽略该向量。关闭失败不会把成功改为失败，也不会覆盖已有导入错误。
- 本函数没有 panic 分支；下游 worker panic 被映射为 `ImporterError::WorkerPanic`。数据库 trait 实现或测试替身内部仍可能自行 panic，这不属于本函数可恢复契约。

## 并发与资源生命周期

`process` 本身在调用线程同步运行。并发只在 `process_jobs` 内启动：一个生产者线程向有界同步通道发送 token，`worker_count` 个 worker 线程共享被 `Mutex` 保护的接收端，每个 worker 独占一个 `Arc<dyn Database>` 克隆并共享同一个 `Arc<Table>`。协调器等待所有 worker 和生产者退出后才返回。

连接生命周期有两层保障：`create_databases` 在部分打开失败时关闭已成功连接；一旦完整连接向量返回，`process` 就把后续操作放进闭包，并在闭包结果生成后统一关闭。这里没有异步任务逃逸，因而 `close_databases` 发生在 `process_jobs` join 全部线程之后。`Arc` 只管理 Rust 对象共享所有权，不替代显式的数据库 `close`。

需要注意，`Database` trait 要求 `Send + Sync`，`DatabaseConnector` 同样要求 `Send + Sync`；这是将连接跨线程使用的类型边界（`pkg/util/importer/db.rs`）。`Table` 中的共享可变生成状态由相邻 `Datum` 实现负责同步，本文件只负责在解析完成后再发布 `Arc<Table>`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/importer/importer.go` 的 `DoProcess`：两者都先 `newTable`/`Table::new`，依次解析建表和建索引 SQL，按 worker 数创建连接，先执行表 DDL 再执行索引 DDL，最后把表、连接、job 数、worker 数和 batch 交给作业调度。

主要差异如下：

- Go `DoProcess` 无返回值，错误路径调用 `log.Fatal` 终止进程；Rust `process` 返回 `Result`，把错误处置交给尚未出现的上层调用者。
- Go 用 `defer closeDBs(dbs)` 保证成功建连后的清理；Rust 用立即执行闭包捕获主体结果，再无条件调用 `close_databases`，实现相同的清理时机。
- Go 的 `closeDBs` 记录关闭错误；Rust 收集后忽略，保持“关闭错误不覆盖导入结果”的优先级，但当前没有日志副作用。
- Rust 显式拒绝零 worker，防止索引空连接数组；Go 版本没有对应前置检查，随后访问 `dbs[0]` 会不安全。
- Go `doProcess` 只打印统计结果；Rust `process_jobs` 返回结构化 `ProcessReport`。
- Rust 的 trait 注入替代 Go 对 `*sql.DB` 和 MySQL 驱动的直接依赖，使独立测试可以观察执行、提交与关闭动作。

同目录没有 Go `*_test.go`。Go 语义依据来自 `importer.go`、`db.go`、`job.go` 和 `parser.go` 本身；Rust 分层回归测试位于独立的 `tests.rs`、`db_test.rs`、`job_test.rs`、`parser_test.rs`，符合测试不内嵌生产文件的仓库约束。

## 扩展指南

- 新增导入阶段时，最可能修改 `process` 闭包。必须明确它位于表 DDL、索引 DDL和 `process_jobs` 的哪一侧，并保持任何 `?` 提前返回后仍会执行连接关闭。
- 新增配置校验时，优先在建立连接或执行 DDL 前完成，尤其是会被 `process_jobs` 拒绝的参数；同时确认错误文本与 Go 兼容要求。
- 改变 DDL 使用的连接时，应维持“每个 worker 有可用连接”的不变量，并评估 DDL 与首个 worker 共用 `databases[0]` 的驱动并发约束。
- 若要暴露关闭错误，需要先定义主体错误与多个关闭错误的合并规则；直接用关闭错误覆盖主体错误会偏离现有 Go 语义。
- 若接入真实数据库驱动，应在上层提供 `DatabaseConnector` 实现，而不是把具体驱动硬编码回 `process`；同时更新 Cargo 的正常依赖区，而非恒假的 `cfg(any())` 区。
- 建议在独立测试文件中增加对 `process` 的端到端替身测试，至少覆盖：完整成功及 DDL 顺序、表解析失败时零连接、第二条 DDL 失败仍关闭全部连接、`process_jobs` 失败仍关闭、关闭失败不覆盖主体结果、零 worker 在任何连接前失败。目前 `tests.rs` 已覆盖解析/作业组合和部分建连失败清理，但没有直接调用 `process`。

兼容风险集中在错误处置差异（Go 终止、Rust 返回）、DDL 已产生的不可回滚副作用和关闭错误的优先级；性能风险主要来自 worker/连接一一对应、固定通道容量以及首连接承担 DDL。修改时应同步检查 `pkg/util/importer/tests.rs`、`db_test.rs`、`job_test.rs` 和 `parser_test.rs`，不要把测试写回 `importer.rs`。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 7,032 个 Rust 文件；`files --filter pkg/util/importer` 列出目标、Go 对照和独立测试。
- RustCodeGraph `node --file pkg/util/importer/importer.rs --offset 1 --limit 260`：读取完整 58 行，确认唯一函数及其直接调用顺序。
- RustCodeGraph `query process --kind function --limit 5000 --json`：定位精确节点 `pkg/util/importer/importer.rs:30:function:process`。常见符号名导致宽泛 `explore` 混入无关结果；精确 `callers/callees` 在 30 秒窗口内无输出，因此调用关系另由目标源码和全仓库引用搜索复核。
- 已读 Rust 文件：`pkg/util/importer/importer.rs`、`lib.rs`、`config.rs`、`db.rs`、`job.rs`、`parser.rs`、`tests.rs`；测试入口还通过文件/符号搜索核对了 `config_test.rs`、`data_test.rs`、`db_test.rs`、`job_test.rs`、`parser_test.rs`、`rand_test.rs`。
- 已读配置/装配：`pkg/util/importer/Cargo.toml`、根 `Cargo.toml:1474`、`pkg/lib.rs:1917`；未发现该目录的 `doc.go`。
- 已读 Go 对照：`pkg/util/importer/importer.go`、`db.go`、`job.go`，并通过引用搜索核对 `parser.go` 的解析入口；同目录不存在 Go 测试文件。
- 全仓库 `rg` 仅找到 crate 的 workspace/facade 接线与函数定义，没有找到 `process` 的 Rust 调用点；这只能证明当前检出版本的静态引用情况，不证明外部 crate 不会调用公开 API。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且固定二级标题恰好为 11 个，并人工检查没有把未接线 API描述为已运行功能。
