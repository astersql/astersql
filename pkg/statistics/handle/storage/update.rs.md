# `pkg/statistics/handle/storage/update.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-storage` crate 的增量更新模块。crate 入口 `pkg/statistics/handle/storage/lib.rs` 以 `pub mod update` 声明模块，并通过 `pub use update::*` 将本文件的公开项提升到 crate 根，因此调用方通常写成 `astersql_statistics_handle_storage::update_stats_meta`，而不是经过 `update` 模块名。

它位于统计信息持久化链路的写端，直接生成并执行针对 `mysql.stats_meta`、`mysql.stats_table_locked` 及其他 `mysql.stats_*` 系统表的 SQL。它不负责收集行数变化、开启事务或刷新内存统计缓存；这些职责分别属于上游统计收集/DDL/导入流程和 `SqlStore` 的具体实现。`pkg/statistics/handle/storage/Cargo.toml` 将该目录定义为独立的 `astersql-statistics-handle-storage` 包，Go 来源包记录为 `pkg/statistics/handle/storage`。

## 核心职责

- `update_stats_version`：取得 `SqlStore::start_ts`，依次把全部 `stats_meta` 和 `stats_histograms` 记录的 `version` 推进到该时间戳，语义上对应 Flashback 后触发统计重载。
- `update_stats_meta`：批量合并表级 `modify_count` 与行数 `count`。它先区分统计锁定与非锁定记录，再把非锁定记录按行数增量正负分组，以便生成三种不同的 UPSERT。
- `change_global_stats_id`：在表从非分区形态切换到分区形态或反向切换时，把六张统计系统表里的全局统计 `table_id` 从旧 ID 改为新 ID。
- `update_stats_meta_version_and_last_histogram_version`：针对单个物理表同时推进 `version` 和 `last_stats_histograms_version`，供统计 GC 与耗时较长的 ANALYZE 保存路径使用。

本文件只做 SQL 语句编排和错误短路传播，不读取查询结果，也不拥有统计对象缓存。

## 主要符号

- `TableDelta { count: i64, delta: i64 }`：单表的一次变化。这里的 `count` 对应需要累加到 `modify_count` 的修改量，`delta` 对应表行数的有符号变化；字段命名沿用 Go 的 `variable.TableDelta` 语义。
- `DeltaUpdate { delta, table_id, is_locked }`：把变化量与目标物理表 ID、统计锁状态组合为一次待写入操作。`new_delta_update` 是按值构造它的公开便利函数。
- `update_stats_meta(store, start_ts, updates)`：核心批处理入口。空切片立即成功；非空切片先锁定目标行，再按类别调用 `exec_delta`。
- `exec_delta(store, table, values, negative)`：私有 UPSERT 生成器。`negative` 只用于非锁定负行数变化，控制 `count` 是加法还是带零下限的减法。
- `CHANGE_GLOBAL_STATS_TABLES`：ID 迁移覆盖的固定表集合：`stats_meta`、`stats_top_n`、`stats_fm_sketch`、`stats_buckets`、`stats_histograms`、`column_stats_usage`。
- `join_ids`：把内部的 `i64` ID 列表转换为逗号分隔的 SQL `IN` 子句内容。

本文件没有 trait、异步函数、条件编译项或模块级可变状态。`Error` 与 `SqlStore` 从同 crate 的 `stats_read_writer.rs` 经 crate 根重导出后引入。

## 执行流程

`update_stats_meta` 的具体顺序如下：

1. `updates` 为空时不获取锁、不执行 SQL，直接返回 `Ok(())`。
2. 遍历每个 `DeltaUpdate`。锁定记录进入 `locked_ids`/`locked`，并保留 `delta` 的原始符号；非锁定记录进入 `unlocked_ids`，写入值时将 `delta` 转为 `unsigned_abs()`，再按原始符号分入 `positive` 或 `negative`。
3. 若存在锁定 ID，对 `mysql.stats_table_locked` 执行 `SELECT ... FOR UPDATE`；若存在非锁定 ID，对 `mysql.stats_meta` 执行同类行锁查询。这一步必须先于 UPSERT，以降低并发 dump 的写冲突。
4. 依次执行锁定记录的正向合并、非锁定正增量合并、非锁定负增量合并。锁定记录即使 `delta < 0` 也使用 `count + values(count)`，所以负值会原样减少锁表中暂存的行数；`update_test.rs::locked_negative_delta_keeps_its_sign_like_go` 固定了这一语义。
5. 非锁定负增量使用 `if(count > values(count), count - values(count), 0)`，保证持久化行数不会降到零以下；所有类别都执行 `modify_count = modify_count + values(modify_count)`，并用本批 `start_ts` 覆盖 `version`。
6. 任一 `SqlStore::execute` 返回错误时立即退出，后续 SQL 不再执行。

其他入口均为顺序流程：`update_stats_version` 获取一次时间戳后更新两张表；`change_global_stats_id` 按常量数组顺序更新六张表；单表版本刷新获取一次时间戳、更新一行并返回该时间戳。

## 数据与状态

本模块自身不保留状态。所有持久状态都在 `mysql.stats_*` 表中，版本号来自调用方传入的 `start_ts` 或 `SqlStore::start_ts()`。`TableDelta` 和 `DeltaUpdate` 都是 `Copy` 值类型，批处理期间只创建本地 ID 与 SQL value 向量。

需要维持的关键不变量是：

- `modify_count` 总是累加 `TableDelta.count`。
- 非锁定 `stats_meta.count` 按 `TableDelta.delta` 增减，负向更新以零为下限。
- 锁定表的变化不写入 `stats_meta`，而是原符号暂存在 `stats_table_locked.count`，等待解锁流程处理。
- `change_global_stats_id` 必须对 `CHANGE_GLOBAL_STATS_TABLES` 中所有表使用相同的 `from`/`to`，否则一个全局统计对象会被拆成不一致的 ID 集合。
- 单表版本刷新将 `version` 与 `last_stats_histograms_version` 设置为完全相同的时间戳。

SQL 由数字 ID、时间戳、计数和固定表名拼接而成，没有接收自由文本参数；不过原子性仍依赖外部事务，而不是字符串构造方式。

## 依赖与调用关系

下游依赖只有 crate 内共享抽象：`SqlStore::start_ts` 提供版本，`SqlStore::execute` 执行 SQL，`Error` 原样承接失败。`SqlStore: Send + Sync` 定义在 `pkg/statistics/handle/storage/stats_read_writer.rs`，但本文件不会创建线程或并行调用它。

已核实的 Rust 上游链路包括：

- `pkg/dxf/importinto/scheduler.rs::FlushImportStatsProduction` 在导入任务完成事务内构造非锁定 `DeltaUpdate`，以导入行数同时作为修改量和正行数增量调用 `update_stats_meta`。
- `pkg/statistics/handle/storage/stats_read_writer.rs::StatsReadWriter::change_global_stats_id` 直接委托本文件的同名函数；DDL 订阅者的 `AlterTablePartitioning` 和 `RemovePartitioning` 分支经 `StatsBackend::change_global_stats_id` 触发该能力。
- `StatsReadWriter::update_stats_meta_version_for_gc` 调用单表版本刷新，随后记录 `schema_change` 历史；`StatsReadWriter::save_analyze_result` 在保存耗时达到租约一半时也调用它重新推进版本。
- `pkg/statistics/handle/ddl/subscriber.rs` 的 `FlashbackCluster` 分支经过后端的 `update_all_stats_versions` 抽象表达全局版本更新；当前源码检索未发现 `update_stats_version` 的直接 Rust 生产调用实现，因此不能据此声称该入口已完整接线。

RustCodeGraph 将 `update.rs` 标记为被 `pkg/dxf/importinto/scheduler.rs` 和 `update_test.rs` 使用；其 `callees update_stats_meta` 查询确认内部调用 `exec_delta`。图上的 `execute` 解析到了测试实现，故具体存储后端和事务边界仍以调用方源码为准。

## 错误处理与边界

所有公开操作返回 `Result<_, Error>`，并用 `?` 保留第一个 `start_ts` 或 SQL 执行错误。没有重试、日志、补偿或错误聚合。特别是 `change_global_stats_id` 可能在前几张表已更新后失败，`update_stats_version` 也可能只更新第一张表；必须由调用方事务保证整体回滚。Go 对照的 `UpdateStatsMeta` 明确要求在事务内调用，Rust 的函数签名本身无法验证这一条件。

空批次是显式无操作。代码没有去重相同 `table_id`：同一批中重复 ID 会同时出现在锁查询与 VALUES 中，结果由 SQL UPSERT 的执行语义决定。它也不校验 `from == to`、负 `count`、异常大的计数或目标行是否存在；INSERT 分支负责补建缺失的 meta/lock 记录。

采用 `unsigned_abs()` 可安全表示 `i64::MIN` 的绝对值，但 SQL 字段最终能否接受该无符号数值取决于实际系统表类型与数据库执行器，本文件没有本地范围检查。负向 count 使用严格的 `count > values(count)`，相等时落到 `0`，符合零下限要求。

## 并发与资源生命周期

`update_stats_meta` 在写入前对两类目标行分别执行 `SELECT ... FOR UPDATE`，锁的持有时长由 `SqlStore` 背后的外部事务决定。函数不会开始、提交或回滚事务；若每次 `execute` 不共享同一事务，前置行锁就无法覆盖后续 UPSERT。调用者因此必须确保整段调用处于一致事务中。

一次调用按固定顺序同步执行，内部没有任务、通道、后台线程或长期资源。局部 `Vec` 与格式化 SQL 在返回时释放。`SqlStore` 的 `Send + Sync` 允许实现被并发共享，但本函数不为多个并发调用提供进程内互斥；数据库行锁才是冲突协调机制。`change_global_stats_id` 和两类版本更新没有自行取得行锁，同样依赖上层事务/数据库更新语义。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/handle/storage/update.go`。Rust 保留了 Go 的五组核心行为：全局版本推进、`DeltaUpdate` 构造、locked/unlocked 与正负 delta 分流、六张表的全局统计 ID 迁移、单表双版本推进。三种 UPSERT 的算术表达式与执行顺序也保持一致，尤其是锁定负增量保留符号、非锁定负增量钳制到零。

可见差异如下：

- Go 使用 `context.Context`、`sessionctx.Context` 和带占位符的 `ExecWithCtx`；Rust 抽象为 `SqlStore`，并把受控数字格式化进 SQL。
- Go 的 `DeltaUpdate` 持有 `variable.TableDelta` 指针对象；Rust 在本 crate 内定义 `Copy` 的 `TableDelta`/`DeltaUpdate` 并以切片批量传递。
- Go 为向量预分配容量并收集 `cacheInvalidateIDs`（当前函数中未进一步使用）；Rust 使用普通 `Vec::new()`，没有该未消费列表，不改变最终 SQL 行为。
- Go 注释明确 `UpdateStatsMeta` 必须在事务中调用；Rust 实现只通过先执行 `FOR UPDATE` 隐含这一前置条件。
- Go 的 DDL/usage 路径已广泛调用这些入口，包括 Flashback、统计 delta dump 与分区全局统计调整；Rust 当前直接生产调用较少，不能把 Go 的完整接线状态等同于 Rust 现状。

相关 Go 验证包括 `pkg/statistics/handle/storage/stats_read_writer_test.go::TestUpdateStatsMetaVersionForGC` 和 `pkg/statistics/handle/ddl/ddl_test.go::TestDDLHistogram` 等，它们验证版本/历史写入或 `last_stats_histograms_version` 的外部效果；它们不是 Rust 测试，不能替代 Rust 接线验证。

## 扩展指南

- 新增一种 delta 分类或修改行数算术时，应优先调整 `update_stats_meta` 的分组和 `exec_delta` 的表达式，并在独立的 `pkg/statistics/handle/storage/update_test.rs` 增加正增量、非锁定负增量零下限、混合 locked/unlocked、空批次和错误短路测试；不要把测试内嵌进生产源文件。
- 增减全局统计系统表时，应同步修改 `CHANGE_GLOBAL_STATS_TABLES`，并与 Go 的 `changeGlobalStatsTables` 保持一致；还应补充失败中途回滚或事务集成验证，避免遗漏表造成孤立统计。
- 接入新的调用方时，必须保证同一个事务覆盖两次 `FOR UPDATE` 和全部 UPSERT，并明确失败是否向上传播或按业务语义忽略。若 `SqlStore` 实现不能提供这一点，应先扩展存储抽象，而不是假定多次 `execute` 自动原子化。
- 改动版本推进逻辑时，应同步检查 `StatsReadWriter::update_stats_meta_version_for_gc`、慢 ANALYZE 的租约分支、DDL Flashback 后端实现及历史元数据记录，避免只改 SQL 而破坏缓存可见性。
- 当前 SQL 按整批拼接，超大批次可能产生长 SQL 和大临时向量；若增加分批，应维持锁定范围、类别顺序、单事务原子性，并评估每批版本一致性。
- 兼容性风险集中在系统表列语义、Go/Rust 行为漂移和未完整接线；性能风险集中在大 `IN`/VALUES 列表及逐表执行六条 ID 迁移 SQL。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/statistics/handle/storage` 确认目标、Go 对照及独立 Rust 测试均在索引中；`node --file pkg/statistics/handle/storage/update.rs --offset 1 --limit 260` 完整读取 166 行源码，并报告直接使用文件；`query` 核对四个公开入口；`callees update_stats_meta` 核对其到 `exec_delta` 的内部调用。`callers update_stats_meta` 在工具超时窗口内未返回结果，因此上游关系另用精确源码检索核验，未将缺失图边推断为“无调用者”。
- Rust 源码与模块边界：`pkg/statistics/handle/storage/update.rs`、`lib.rs`、`stats_read_writer.rs`、`pkg/dxf/importinto/scheduler.rs`、`pkg/statistics/handle/ddl/subscriber.rs`。
- crate 配置：`pkg/statistics/handle/storage/Cargo.toml`；其中依赖位于 `target.'cfg(any())'` 端口记录区，当前文件实际只使用 crate 内 `Error`/`SqlStore` 与标准库能力。
- Go 对照与上游：`pkg/statistics/handle/storage/update.go`、`pkg/statistics/handle/usage/session_stats_collect.go`、`pkg/statistics/handle/ddl/subscriber.go`。
- 测试证据：`pkg/statistics/handle/storage/update_test.rs::locked_negative_delta_keeps_its_sign_like_go`；Go 侧参考 `pkg/statistics/handle/storage/stats_read_writer_test.go` 与 `pkg/statistics/handle/ddl/ddl_test.go`。Rust 直接测试目前只覆盖锁定负增量 SQL，其他边界在本文中明确列为应补覆盖项。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；验收以源码/调用证据、人工事实复核和固定 11 节结构检查为准。
