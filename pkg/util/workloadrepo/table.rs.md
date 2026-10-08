# [`pkg/util/workloadrepo/table.rs`](./table.rs)

## 文件定位

该文件属于 `astersql-util-workloadrepo` crate；crate 根 `pkg/util/workloadrepo/lib.rs` 以私有 `mod table` 装配它，并用 `pub use table::*` 再导出其中的公开函数。`pkg/util/workloadrepo/Cargo.toml` 将 `lib.rs` 指定为库入口，当前直接外部依赖只有 `chrono`，Go 包映射为 `pkg/util/workloadrepo`。

它位于工作负载仓库启动和采集链的表结构层：`worker::startRepository` 在 owner 节点调用 `Worker::createAllTables`，随后所有节点调用 `Worker::checkTablesExists`；`sampling.rs::Worker::samplingTable` 与 `snapshot.rs::Worker::snapshotTable` 则在第一次写入前调用 `buildInsertQuery`。文件不负责定时调度、快照编号分配或旧分区清理，这些分别位于 `worker.rs`、`snapshot.rs` 和 `housekeeper.rs`。

## 核心职责

- `identifier` 对动态标识符加反引号，并把内部反引号翻倍，供生成 SQL 时安全引用 schema、表和列名。
- `buildCreateQuery` 从 `RepositoryBackend::source_columns` 取得源表列定义，生成目标历史表的 `CREATE TABLE IF NOT EXISTS` 主体；快照表额外增加 `SNAP_ID`，所有非元数据表都增加 `TS` 和 `INSTANCE_ID`。
- `buildInsertQuery` 生成并缓存 `INSERT ... SELECT` 到 `repositoryTable::insertStmt`；快照表绑定 `SNAP_ID` 与实例 ID，采样表只绑定实例 ID，并保留配置的 `whereClause`。
- `Worker::createAllTables` 跳过已存在的目标表，为缺失表选择动态或预置建表语句，追加 RANGE 分区定义，重试执行 DDL，最后确保所有表都有所需分区。
- `Worker::checkTablesExists` 与 `checkTableExistsByIS` 把“就绪”定义为表存在，且在传入时间时最晚分区日期严格晚于次日。

## 主要符号

- `fn identifier(value: &str) -> String`：文件内私有 SQL 标识符转义器；不处理 SQL 值，列注释由 `buildCreateQuery` 单独把单引号翻倍。
- `pub fn buildCreateQuery(backend: &dyn RepositoryBackend, table: &repositoryTable) -> Result<String, String>`：动态建表 SQL 生成入口。它先查询源列，再拒绝 `metadataTable`，因此源表查询错误优先于类型错误；这一顺序由 `table_test.rs::metadata_create_propagates_source_lookup_error_before_type_error_like_go` 固定。
- `pub fn buildInsertQuery(backend: &dyn RepositoryBackend, table: &mut repositoryTable) -> Result<(), String>`：插入 SQL 生成入口，成功时修改 `table.insertStmt`。同样先查询源列再拒绝元数据表，对应测试为 `metadata_insert_propagates_source_lookup_error_before_type_error_like_go`。
- `pub fn Worker::createAllTables(&self, now: DateTime<Local>) -> Result<(), String>`：建表和初始补分区入口。`metadataTable` 使用预置 `createStmt` 并按 `BEGIN_TIME` 分区；其余表在没有预置语句时调用 `buildCreateQuery`，按 `TS` 分区。
- `pub fn Worker::checkTablesExists(&self, now: DateTime<Local>) -> bool`：对 `workloadTables` 执行全量短路检查。
- `pub fn checkTableExistsByIS(backend: &dyn RepositoryBackend, tableName: &str, now: Option<DateTime<Local>>) -> bool`：单表就绪判定；`None` 只检查表存在，`Some(now)` 还检查分区。

## 执行流程

启动路径从 `worker.rs::Worker::startRepository` 开始。它先用 `fillInTableNames` 补齐 `HIST_<源表>` 目标名；只有 `RepositoryBackend::is_owner` 为真时才调用 `createAllTables`，但无论是否 owner 都继续调用 `checkTablesExists`，失败时返回 `repository tables are not ready`。

`createAllTables` 对 `workloadTables` 的快照逐表处理：

1. `table_exists(destTable)` 为真时跳过建表。
2. `createStmt` 非空时直接采用预置 DDL；否则由 `buildCreateQuery` 查询源列并拼出 DDL。
3. 调用 `utils.rs::generatePartitionDef`，元数据表选择 `BEGIN_TIME`，其它类型选择 `TS`。该工具会追加未来分区的 RANGE 定义。
4. 调用 `worker.rs::execRetry` 执行 DDL，最多尝试五次；任一步失败立即返回。
5. 全部缺失表处理完成后调用 `housekeeper.rs::Worker::createAllPartitions`，为已存在或刚创建的所有目标表补齐分区。

采集路径中，`sampling.rs::samplingTable` 或 `snapshot.rs::snapshotTable` 先取得目标表的可变引用；仅当 `insertStmt` 为空时调用 `buildInsertQuery`。生成器依次构建目标列清单、SELECT 表达式、源 schema/表和可选 WHERE 条件，调用者随后分别绑定 `[INSTANCE_ID]` 或 `[SNAP_ID, INSTANCE_ID]` 执行，因此占位符顺序是运行时契约。

就绪检查先调用 `table_exists`。若 `now` 为 `None`，存在即成功；若为 `Some`，还要成功取得非空分区列表、解析最后一个分区名，并满足 `last_date > now + 1 day`。`checkTablesExists` 对全部目标表应用这一规则。

## 数据与状态

输入表描述 `worker.rs::repositoryTable` 保存源 `schema/table`、`tableType`、目标 `destTable`、原样 SQL 过滤片段 `whereClause`、可选预置 `createStmt` 和缓存 `insertStmt`。本文件只持久修改 `insertStmt`；建表流程克隆表清单后工作，不回写 DDL。

`worker.rs::ColumnDefinition` 的 `name`、`type_description`、`comment` 分别进入列标识符、类型文本和 COMMENT 字面量。名称与注释在本文件转义；`type_description` 和 `whereClause` 被视为后端/配置提供的可信 SQL 片段，不在这里解析或验证。

`Worker::workloadTables` 是 `Mutex<Vec<repositoryTable>>`。`createAllTables` 在锁内克隆整个列表后释放锁，避免执行后端 DDL 时长期占锁；`checkTablesExists` 在迭代期间保持该锁。日期使用 `chrono::DateTime<Local>`，分区名由 `utils.rs::parsePartitionName` 按本地日期解析。

## 依赖与调用关系

上游调用边由源码检索确认：`worker.rs::startRepository` 调用 `createAllTables` 和 `checkTablesExists`；`sampling.rs::samplingTable`、`snapshot.rs::snapshotTable` 调用 `buildInsertQuery`；本文件内部 `createAllTables` 调用 `buildCreateQuery`，`checkTablesExists` 调用 `checkTableExistsByIS`。RustCodeGraph 还识别到 `buildInsertQuery -> identifier`、`createAllTables -> buildCreateQuery`，但对部分 trait 动态调用和上游方法调用未产出完整边，因此以上缺口用这些直接源码引用核验。

下游依赖包括 `RepositoryBackend::{source_columns,table_exists,partitions}`、`utils.rs::{generatePartitionDef,parsePartitionName}`、`worker.rs::execRetry` 和 `housekeeper.rs::Worker::createAllPartitions`。`crate::*` 还带入 `workloadSchema`、三种表类型常量及相关公开项。

crate 外调用者通过 `lib.rs` 的再导出可见公开自由函数；`Worker` 方法通过公开类型别名 `WorkloadRepoWorker` 可达。实际数据库访问完全经 `RepositoryBackend`，所以该文件没有直接会话池、InfoSchema 或网络依赖。

## 错误处理与边界

`buildCreateQuery`、`buildInsertQuery`、`createAllTables` 使用 `Result<_, String>` 逐层传播后端、分区生成和 SQL 执行错误。元数据表不允许走动态建表或动态插入生成器，但为保持 Go 可观察顺序，两者都会先执行 `source_columns`；源表不存在时返回源查询错误，而不是元数据类型错误。

`checkTableExistsByIS` 是布尔探测接口：表不存在、分区查询失败、分区为空、最后一个名称无法解析、日期加一天溢出，都会折叠为 `false`。它假定后端按日期顺序返回分区，代码只检查 `partitions.last()`，并不自行排序或寻找最大日期。边界比较是严格的大于：恰好等于 `now + 1 day` 仍不算就绪。

SQL 生成边界包括：标识符内部反引号被转义，列注释内部单引号被翻倍；但 `type_description` 与 `whereClause` 原样拼接。未知的非元数据 `tableType` 在 `buildCreateQuery` 中按普通非快照表处理，在 `buildInsertQuery` 中也走仅含 `now(), INSTANCE_ID` 的非快照分支；调用者应只传 `snapshotTable`、`samplingTable` 或 `metadataTable`。

对 `Mutex` 的访问使用 `lock().unwrap()`，锁中毒会 panic 而不是返回 `String`。`execRetry` 只覆盖 DDL 执行，列查询、表/分区探测与 SQL 生成不会在此文件重试。

## 并发与资源生命周期

本文件不创建线程、通道或事务，也不持有数据库会话；后端由 `Worker` 的 `Arc<dyn RepositoryBackend + Send + Sync>` 共享。owner 仲裁发生在 `worker.rs::startRepository`，不是 `createAllTables` 方法内部，因此直接调用该方法不会再次验证 owner 身份。

`createAllTables` 克隆锁内配置后逐表执行，允许其它路径在 DDL 期间取得 `workloadTables` 锁，但其克隆看不到随后发生的表配置修改。相比之下，`checkTablesExists` 在所有后端探测期间持锁，慢后端会延长竞争时间。采样和快照调用者在生成并缓存 `insertStmt` 时持有表锁，释放后才执行 INSERT，保证同一 Worker 内不会为同一条目并发重复写缓存。

建表采用 `CREATE TABLE IF NOT EXISTS` 且执行层带重试，能容忍多个执行者的常见竞争；Rust 测试 `worker_test.rs::TestRaceToCreateTablesWorker` 用共享内存后端启动两个 Worker 验证目标表最终存在。该测试后端不是事务数据库，不能证明真实 DDL 的全部竞争语义。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/workloadrepo/table.go`。Rust 保留了动态复制源列、快照/采样占位符顺序、元数据表拒绝路径、`BEGIN_TIME`/`TS` 分区列选择、建表后补分区，以及“最晚分区必须晚于明天”的核心语义。`table_test.rs` 专门固定了 Go 中先 `TableByName`、再拒绝元数据表的错误优先级。

主要适配差异如下：Go 通过 session transaction InfoSchema 读取表和列并从 session pool 执行 SQL；Rust 改由 `RepositoryBackend` 提供列、表、分区和执行能力。Go `createAllTables` 会显式创建缺失的 `WORKLOAD_SCHEMA` 并刷新 InfoSchema，Rust 方法没有建库步骤，后端或更外层必须保证 schema 可用。Go 的 `zeroTime` 表示仅查表，Rust 用 `Option<DateTime<Local>>` 表达相同模式，但 `Worker::createAllTables` 直接调用 `table_exists`。

Go 的 `buildInsertQuery` 只为明确的 `samplingTable` 写 `now(), %?`，而当前 Rust 对任何非 `snapshotTable`、非 `metadataTable` 类型都采用这一形式；正常三种类型输入下行为一致。Go 基于 InfoSchema 分区定义的最后一项检查日期；Rust 依赖 `RepositoryBackend::partitions` 返回有序名称列表。Go 测试 `worker_test.go::TestAddNewPartitionsOnStart`、`validatePartitionCreation` 覆盖真实 TiDB 建表和分区语义；Rust 的 `worker_test.rs` 使用内存后端覆盖启动竞争与分区维护，`table_test.rs` 目前只直接覆盖两个错误顺序用例。

## 扩展指南

新增源表列映射或 SQL 形态时，优先修改 `buildCreateQuery` 与 `buildInsertQuery`，并保持目标列顺序、SELECT 表达式和调用者参数数组严格对应。若新增表类型，应显式扩展两处生成器以及 `createAllTables` 的分区列选择，不能依赖当前“其它类型等同采样表”的默认分支；同时检查 `worker.rs::defaultWorkloadTables`、`sampling.rs` 和 `snapshot.rs` 的筛选逻辑。

改变“表就绪”定义时，应同步评估 `checkTableExistsByIS`、`utils.rs::generatePartitionRanges` 和 `housekeeper.rs::createPartition`，特别是分区列表排序契约、严格日期边界以及本地时区/DST 行为。改变锁粒度时需避免在持有 `workloadTables` 锁时回调会再次获取该锁的代码。

测试应保持在独立文件，直接单元用例放在 `pkg/util/workloadrepo/table_test.rs`；跨启动、建表和分区生命周期用例放在 `worker_test.rs`。与 Go 移植语义有关的修改还应对照 `table.go` 和 `worker_test.go` 的对应场景。建议补充成功 SQL 文本、标识符/注释转义、未知类型、分区查询失败、空/乱序分区和日期边界用例；不要把测试内嵌到 `table.rs`。

兼容性风险主要是生成 SQL 文本及参数顺序变化；正确性风险集中在原样 SQL 片段和分区排序假设；性能风险主要来自每次就绪检查持锁执行多次后端查询。若后端契约变化，应先更新 `RepositoryBackend` 的实现与测试替身，再调整本文件。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 `pkg/util/workloadrepo/table.rs`（155 行、9 个符号）；`node --file ...` 读取了完整文件；`files --filter pkg/util/workloadrepo` 确认相邻 Rust/Go 文件；call graph 确认 `buildInsertQuery -> identifier`、`createAllTables -> buildCreateQuery`，同时暴露 trait/方法边缺失，故未把空图结果当作“无调用者”。
- Rust 源码：`pkg/util/workloadrepo/table.rs`、`lib.rs`、`worker.rs`、`sampling.rs`、`snapshot.rs`、`housekeeper.rs`、`utils.rs`。
- crate 边界：`pkg/util/workloadrepo/Cargo.toml`，确认库入口、`chrono` 依赖及 Go 包映射。
- 独立 Rust 测试：`pkg/util/workloadrepo/table_test.rs` 验证元数据路径错误顺序；`worker_test.rs` 的内存 `RepositoryBackend`、`TestRaceToCreateTablesWorker`、`TestCreatePartition` 等提供建表和分区生命周期证据。
- Go 对照：`pkg/util/workloadrepo/table.go`；相关调用和测试来自 `sampling.go`、`snapshot.go`、`worker.go`、`worker_test.go::TestAddNewPartitionsOnStart` 及其分区辅助断言。
- 本任务是纯文档分析，未修改运行时代码，未运行 Cargo。交付前使用任务指定命令确认目标文档存在且恰有十一个固定二级章节，并人工检查所有行为陈述都可回溯到上述符号或文件。
