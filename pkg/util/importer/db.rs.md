# `pkg/util/importer/db.rs`

## 文件定位

`db.rs` 属于 `astersql-util-importer` crate 的数据库与数据生成边界。crate 入口 `pkg/util/importer/lib.rs` 将 `db` 声明为公开模块并重导出其公开项；`pkg/util/importer/Cargo.toml` 以 `lib.rs` 为库入口，且普通 `[dependencies]` 为空，所以本文件的运行时依赖都是标准库或同 crate 的 `config`、`parser`、`rand` 模块。`target.'cfg(any())'.dependencies` 永远不会启用，不能视为当前实现的运行时接线。

在完整导入链中，`pkg/util/importer/importer.rs::process` 调用本文件的 `create_databases`、`execute_sql` 和 `close_databases`，负责连接建立、DDL 执行和最终清理；`pkg/util/importer/job.rs::do_insert` 经 `generate_row_data_batch` 生成 INSERT，并通过 `Database::begin` 得到的事务逐条执行、最后提交。因此本文件同时处在“配置/表元数据到 SQL 文本”和“导入流程到数据库驱动”两个边界上。

当前仓库在该 crate 的生产代码中没有 `Database`、`DatabaseTransaction` 或 `DatabaseConnector` 的真实驱动实现；可见实现位于独立测试替身中。这意味着本文件提供可注入协议和纯生成逻辑，真实连接器必须由调用方提供，不能把 Go 版直接使用 `database/sql` 的接线视为 Rust 版已经完成。

## 核心职责

1. 用 `DatabaseTransaction`、`Database`、`DatabaseConnector` 三个 trait 隔离事务、连接和按 DSN 开连接的具体驱动，使导入流程可以共享连接并注入测试替身。
2. 根据 `parser.rs::Table`、`Column`、`FieldKind` 以及列的 `range`、`set`、`step` 规则，为支持的 SQL 类型生成字段字面量。
3. 将一组字段字面量拼成单行 INSERT，或连续生成指定数量的 INSERT，供 `job.rs` 的事务批处理消费。
4. 统一处理空 SQL、批量建连接失败时的已建连接回收，以及批量关闭时的错误收集。

本文件只生成 SQL 字符串并定义驱动协议，不解析 DDL、不调度线程、不决定事务批次，也不实现重试、回滚或真实 MySQL I/O；这些职责分别位于 `parser.rs`、`job.rs` 和调用方提供的 trait 实现中。

## 主要符号

- `pub trait DatabaseTransaction: Send`：可在线程间移动的事务句柄。`execute(&mut self, sql)` 串行执行一条 SQL；`commit(self: Box<Self>)` 消耗事务对象，类型层面阻止提交后再次使用。trait 没有 `rollback`，失败后的回滚语义只能由具体实现的析构/驱动行为承担。
- `pub trait Database: Send + Sync`：可被多个线程安全共享的连接抽象。`execute` 用于非事务 SQL，`begin` 创建装箱事务，`close` 显式释放连接。`job.rs` 实际上为每个 worker 传入一个 `Arc<dyn Database>`。
- `pub trait DatabaseConnector: Send + Sync`：用 `open(&str)` 将 `DbConfig::dsn()` 生成的 DSN 转成 `Arc<dyn Database>`，把驱动选择与连接构造留给 crate 外部。
- `integer_range(column, minimum, maximum)`：内部辅助函数。列没有 `minimum` 时原样返回类型默认区间；否则解析自定义下界，并在 `maximum` 为空时沿用默认上界。
- `random_integer(column, minimum, maximum)`：内部非唯一整数生成器。`column.set` 非空时均匀选择一个字符串并解析为 `i64`；否则经 `integer_range` 后调用闭区间随机函数 `rand_i64`。
- `unique_integer(column, minimum, maximum)`：内部唯一整数入口。先解析区间，再以 `column.step` 初始化列共享的 `Datum`，随后返回 `Datum::unique_i64()`。
- `pub fn generate_column_data(table, column)`：字段生成主分派。是否走唯一序列由 `table.unique_indices.contains(&column.name)` 决定；整型还读取 `column.field_type.unsigned`。
- `pub fn generate_row_data(table)`：按 `table.columns` 顺序生成字段，以逗号连接，并使用 `table.name` 与 `table.column_list` 生成一条 `insert into ... values (...);`。
- `pub fn generate_row_data_batch(table, count)`：重复调用 `generate_row_data`，遇到首个错误立即返回错误；`count == 0` 时返回空向量。
- `pub fn execute_sql(database, sql)`：空字符串直接成功，非空字符串原样委托给 `Database::execute`。
- `pub fn create_databases(connector, config, count)`：对同一 `config.dsn()` 连续打开 `count` 个连接；任一次失败都会调用 `close_databases` 清理此前成功的连接，然后返回原始打开错误。
- `pub fn close_databases(databases)`：尝试关闭每个连接，不因单个失败中断，并按输入顺序收集所有关闭错误。

## 执行流程

字段生成流程从 `generate_column_data` 开始：

1. 用列名查询 `Table::unique_indices`，并读取类型的 `unsigned` 标记。
2. `TinyInt`、`SmallInt`、`Int`、`BigInt` 走局部 `integer` 闭包。唯一列统一从 0 到该类型的唯一值上界；非唯一无符号列从 0 起；非唯一有符号列使用代码中明确的默认上下界。`BigInt` 的有符号随机默认范围刻意是 `i32::MIN..=i32::MAX`，而不是完整 `i64`，与 Go 文件一致。
3. `Varchar`、`String`、`Blob`：唯一列调用 `Datum::unique_string(length)`；非唯一列生成长度在 `1..=length.max(1)` 的随机字符串；结果以单引号包围。
4. `Float`、`Double`、`Decimal` 复用整数分支，再转换为 `f64` 字符串；当前实现不使用声明精度/小数位产生分数值。
5. `Date`、`DateTime`/`Timestamp`、`Time`、`Year`：唯一列调用列级 `Datum` 的递进方法，非唯一列调用 `rand_*` 并传入列的字符串边界；结果以单引号包围。
6. 任一解析或随机区间错误以 `ImporterError` 返回。

行生成流程是 `generate_row_data` 对全部列依次调用上述函数，只有全部成功才拼接 SQL。`generate_row_data_batch` 再按数量重复该流程。`job.rs::do_insert` 的已验证下游顺序是：先生成整批语句，再 `Database::begin`，逐条 `DatabaseTransaction::execute`，最后 `commit`；因此生成阶段失败时尚未开启事务，执行阶段失败时不会调用 `commit`。

连接流程是 `importer.rs::process` 先验证 worker 数，再调用 `create_databases(worker_count)`；随后用第一个连接执行可选的建表、建索引 SQL，再把连接交给 `process_jobs`。无论主体成功或失败，`process` 都调用 `close_databases`，但按照 Go 行为忽略返回的关闭错误，不用它覆盖主体结果。

## 数据与状态

本文件自身不拥有全局可变状态。生成状态来自 `Column` 中共享的 `Arc<Datum>`：唯一整数、字符串和时间值会修改 `Datum` 内部由 `Mutex` 保护的状态。`unique_integer` 每次都会调用 `set_init_int64_value`，但 `data.rs` 规定只有第一次初始化生效，所以同一列后续生成沿用首次确定的 `step` 与区间。

随机非唯一值依赖 `rand.rs` 的全局 `AtomicU64` LCG 状态；调用 `rand::seed` 可令测试可重复。`random_integer` 的 `set` 与 range 是互斥优先级：只要 `set` 非空，就完全忽略 `minimum`、`maximum` 和类型默认区间。

连接集合是 `Vec<Arc<dyn Database>>`。`Arc` 允许协调器和 worker 共享 trait 对象，但不代表多个 worker 必然共用同一连接：`create_databases` 每次循环都调用一次 `open`，正常情况下为每个 worker 创建独立对象。`count == 0` 时它返回空向量；当前正式入口 `process` 会在调用前拒绝 `worker_count == 0`。

## 依赖与调用关系

上游调用关系由精确源码搜索确认：

- `importer.rs::process` → `create_databases` → `DatabaseConnector::open`，失败清理边为 `create_databases` → `close_databases` → `Database::close`。
- `importer.rs::process` → `execute_sql` → `Database::execute`，分别处理建表与建索引 SQL；空索引 SQL在本文件被短路。
- `importer.rs::process` → `close_databases`，覆盖导入主体成功和失败两条退出路径。
- `job.rs::do_insert` → `generate_row_data_batch` → `generate_row_data` → `generate_column_data` → `random_integer`/`unique_integer` 或 `rand_date`、`rand_time`、`rand_timestamp`、`rand_year`。
- `job.rs::do_insert` → `Database::begin` → `DatabaseTransaction::execute` → `DatabaseTransaction::commit`。

数据依赖为 `config.rs::{DbConfig, ImporterError}`、`parser.rs::{Table, Column, FieldKind}`、`data.rs::Datum`（通过 `Column::data` 间接使用）和 `rand.rs` 的随机/时间函数。外部驱动不在 Cargo 的活动依赖中，且目标文件没有具体 trait 实现；这是当前可执行接线的明确限制。

RustCodeGraph 的 `status` 显示目标仓库索引包含 `pkg/util/importer/db.rs` 的 23 个符号，`query` 精确定位了本节公开函数与内部辅助函数。该工具的 `callers/callees` 在本次环境中超时且未返回边，所以上述调用边用限定在 `pkg/util/importer` 与 `cmd/importer` 的精确 `rg` 结果补证，没有根据模糊的同名全仓结果推断。

## 错误处理与边界

- 自定义 range 或 set 元素不能解析为 `i64` 时返回 `ImporterError::InvalidRange(原字符串)`；随机函数还会拒绝下界大于上界。与 Go 版的 `log.Fatal` 或忽略 set 解析错误不同，Rust 版把错误交给调用者，避免在库函数内退出进程或静默变成 0。
- `generate_row_data`、批量生成和 `job.rs` 使用 `?` 保留首个错误；没有部分 SQL 返回。已经在事务内成功执行的早先语句如何处理，取决于具体事务实现，因为协议没有显式 rollback。
- `execute_sql` 只把完全空的 `""` 当作无操作；只含空白的字符串仍会交给驱动。
- `create_databases` 在第 N 次打开失败时清理前 N-1 个连接，但忽略这些清理错误并返回打开错误。`close_databases` 单独调用时会保留全部关闭错误；正式 `process` 当前主动忽略它们。
- SQL 文本直接插入表名、列清单和生成值。本文件没有参数绑定或 SQL 转义层；随机/唯一字符串只来自字母数字字母表，正常生成不会包含引号，但未来支持自由文本或自定义字符串 set 时必须先定义转义/绑定策略。
- `FieldKind` 是封闭枚举，match 已穷尽当前所有变体；新增类型会触发编译期非穷尽检查。与 Go 版的运行时 `unsupported column type` 默认分支相比，当前 Rust `ImporterError::UnsupportedColumn` 不会由本函数产生。
- `unique_i64` 达到区间上界后返回当前值且不再递增，因此“唯一”不等于无限基数；请求行数超过可用序列容量可能产生重复值并由数据库唯一约束报错。
- `generate_row_data` 对空 `columns` 不会 panic，而会形成空列/值清单 SQL；正常入口依赖解析器提供有效表结构，生成器本身不再次校验。

## 并发与资源生命周期

三个 trait 的约束体现线程模型：事务只需 `Send`，因为事务由单个 worker 独占；数据库和连接器要求 `Send + Sync`，数据库又包装在 `Arc` 中，允许跨线程传递和共享。实际 `job.rs::process_jobs` 为每个 worker 克隆一个连接 `Arc`，worker 内顺序执行其事务。

唯一值状态由每列的 `Datum::Mutex` 串行化，因此多个 worker 同时为同一唯一列生成数据时共享同一序列；非唯一随机状态由原子 LCG 更新，无需互斥锁。这里的锁中毒会在 `data.rs` 的 `lock().unwrap()` 处 panic，本文件不捕获该 panic。

事务生命周期为 `begin` 返回 `Box<dyn DatabaseTransaction>`，逐条可变执行，`commit` 消耗 Box。执行或提交错误沿调用栈返回；协议没有异步任务、取消、超时、重试和显式回滚。连接生命周期由 `create_databases` 创建、`process` 结束路径统一 `close_databases`；失败打开路径也会主动清理已创建资源。`Arc` 的其他持有者若仍存在，`close` 的并发安全性与幂等性必须由具体 `Database` 实现保证。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/importer/db.go`：`integer_range`/`random_integer`/`unique_integer` 分别对应 `intRangeValue`/`randInt64Value`/`uniqInt64Value`；三层行生成函数对应 `genColumnData`、`genRowData`、`genRowDatas`；连接辅助对应 `execSQL`、`createDBs`、`closeDBs`。

已保持的核心语义包括：唯一索引决定唯一序列、整数有符号/无符号默认范围、BIGINT 和浮点的有符号随机默认范围仍为 32 位、字符串与时间值加单引号、空 SQL 不执行、每个 worker 建一个连接，以及关闭错误不覆盖主导入结果。`db_test.rs::signed_numeric_defaults_match_go_ranges` 专门锁定数值默认范围；`tests.rs::generation_and_job_processing_preserve_row_and_batch_counts` 锁定行数和事务批次。

有意或客观存在的 Rust 差异包括：

- Go 直接依赖 `database/sql` 和 MySQL 驱动，Rust 改为 trait 注入且当前无生产驱动实现。
- Go 的非法 range 调用 `log.Fatal`、非法 set 元素忽略解析错误；Rust 均返回 `ImporterError::InvalidRange`。
- Go 的 `createDBs` 在中途失败时直接返回，已打开连接没有在该函数内关闭；Rust 会回收此前连接，且 `tests.rs::create_databases_closes_connections_when_opening_fails` 明确验证此行为。
- Go 的 `closeDBs` 逐个记录错误且不返回；Rust `close_databases` 收集错误，正式入口再选择忽略。
- Rust 对非唯一零长度字符串类型用 `length.max(1)`，保证随机长度区间有效；Go 直接调用 `randInt(1, flen)`。
- Go 对未知数据库类型在运行时返回 unsupported 错误；Rust 的 `FieldKind` 穷尽匹配把新增类型遗漏转为编译期问题。

## 扩展指南

- 新增 SQL 类型：先在 `parser.rs::FieldKind` 和类型解析处建模，再扩展 `generate_column_data`；同时在独立的 `db_test.rs` 增加唯一/非唯一、有符号/无符号、默认与自定义边界测试。不要把测试内嵌进 `db.rs`。
- 新增字段生成规则：先明确它与 `set`、`range`、`step` 的优先级，再修改 `random_integer`、`unique_integer` 或相应 `rand_*` 路径，并同步 `parser_test.rs`（规则解析）与 `db_test.rs`（生成行为）。若允许用户提供字符串，必须加入可靠的 SQL 转义或参数绑定，不能复用当前直接加引号的做法。
- 接入真实数据库：在 crate 外实现三个 trait，并确保 `Database::close` 可安全处理失败清理与最终清理，事务对象在未提交即丢弃时能回滚，所有驱动错误映射为带上下文的 `ImporterError::Database`。还应补充独立集成测试；不要把驱动依赖复制进本文件或测试替身。
- 修改连接策略：以 `create_databases` 为切入点，保留“部分成功后失败必须清理”的不变量，并为关闭错误优先级作出显式决定。相关回归位置是 `tests.rs::create_databases_closes_connections_when_opening_fails`。
- 修改并发生成：必须保留 `Column::data` 跨 worker 共享和 `Datum` 内部同步，否则唯一索引列会发生竞态重复。性能优化应分别测量 `Datum::Mutex` 争用、全局随机原子争用和逐行 SQL 字符串分配，不能仅凭 trait 的 `Send + Sync` 推断无瓶颈。
- 修改 SQL 格式：同步检查 `tests.rs::generation_and_job_processing_preserve_row_and_batch_counts`、下游 `job.rs::do_insert` 以及 Go 的 `genRowData`，并关注标识符转义、值转义和数据库兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引 11,467 个文件、307,296 个节点，目标目录清单包含 `db.rs`；`node --file pkg/util/importer/db.rs` 读取完整 215 行；`query` 精确确认 3 个 trait、6 个公开函数及 3 个内部生成辅助函数。`callers/callees` 命令本次超时无结果，故未把它当作成功证据。
- 源码与模块：`pkg/util/importer/db.rs`、`pkg/util/importer/lib.rs`、`pkg/util/importer/config.rs`、`pkg/util/importer/parser.rs`、`pkg/util/importer/data.rs`、`pkg/util/importer/rand.rs`、`pkg/util/importer/importer.rs`、`pkg/util/importer/job.rs`。
- crate 边界：`pkg/util/importer/Cargo.toml`；活动依赖为空，porting metadata 指向 Go 包 `pkg/util/importer`。
- Go 对照：`pkg/util/importer/db.go` 与 `pkg/util/importer/importer.go`；前者提供数据生成和连接辅助的直接语义基线，后者确认完整流程中的调用顺序与关闭策略。
- 独立 Rust 测试：`pkg/util/importer/db_test.rs::signed_numeric_defaults_match_go_ranges`；`pkg/util/importer/tests.rs::generation_and_job_processing_preserve_row_and_batch_counts`、`create_databases_closes_connections_when_opening_fails`；测试替身还证明 trait 的执行、提交和关闭观测方式。
- 精确调用搜索：限定 Rust 路径的 `rg` 确认 `importer.rs` 对连接函数的调用、`job.rs` 对批量生成的调用，以及测试中的 trait 实现；限定 Go 路径的 `rg` 确认对应入口。未发现该 crate 生产代码中的真实数据库 trait 实现。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工复核本文能回答文件存在原因、运行链路、状态/错误/资源边界和安全扩展位置。
