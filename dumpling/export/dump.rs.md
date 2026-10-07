# `dumpling/export/dump.rs`

## 文件定位

`dump.rs` 是 `astersql-dumpling-export` crate 的导出编排层。该 crate 的入口是 [`lib.rs`](lib.rs)，它用 `include!("dump.rs")` 将本文件与 `prepare.rs`、`sql.rs`、`consistency.rs`、`metadata.rs`、`writer.rs` 等文件拼成接近 Go `dumpling/export` 包的单一命名空间；因此本文件没有自己的 `use` 列表，所用的 `Config`、`DB`、`BaseConn`、`TaskEnum`、`Writer` 等符号由 crate 入口和相邻实现共同提供。

应用入口位于 [`../cmd/dumpling/main.rs`](../cmd/dumpling/main.rs)：`run` 将 `export::NewDumper` 作为工厂传入 `run_with_factory`，后者依次调用 `Dump` 和 `Close`。所以本文件既负责把静态配置初始化成一次 `Dumper` 会话，也负责把库表元数据转换为 writer 可消费的任务。

[`Cargo.toml`](Cargo.toml) 将该目录声明为 `astersql-dumpling-export` library，并以 `package.metadata.porting.go-package = "dumpling/export"` 标出 Go 对照包。直接依赖包括 dumpling 的 `cli/context/log`、CSV/SQL/Parquet 格式、对象存储接口、表过滤器与 schema parser；SQL、PD、HTTP、metrics 等迁移期接口还大量来自同 crate 的 [`stubs.rs`](stubs.rs)。本文件没有条件编译项，测试由 `lib.rs` 中独立的 `#[cfg(test)] #[path = "dump_test.rs"] mod dump_test;` 接入。

## 核心职责

1. `NewDumper` 校验并调整配置，建立可取消上下文、metrics、进度状态及外部资源，再按固定步骤初始化日志、存储、HTTP、数据库、服务端信息、一致性模式和 session 参数。
2. `Dumper::Dump` 建立一致性控制器，准备导出对象和列投影，记录全局元数据，生产 schema/data 任务，并用 writer 写入外部存储。
3. `dumpDatabases`、`dumpWholeTableDirectly`、`dumpSQL` 与 `newTaskTableData` 把 placement policy、数据库、表、视图、序列及自定义 SQL 转换为 `TaskEnum`。
4. `prepareTableListToDump*`、`dumpTableMeta` 和列投影函数负责确定对象清单、可写列、查询字段、列类型和投影后的建表 SQL。
5. `updateServiceSafePoint`、`updateKeyspaceGCBarrier` 与 `runGCProtectionUpdater` 提供 GC 保护点的续租、重试、取消和清理循环。
6. `Close` 集中取消会话并回收 HTTP、PD、数据库和 metrics 注册。

当前实现不是 Go 版本全部能力的等价实现。文件头注释和实际控制流都表明 Rust 路径是“单进程、单 writer”的可执行骨架；Go 中的多 writer、并行/分块表导出、连接重建、完整 PD 客户端和 snapshot 初始化并未全部接入本文件。

## 主要符号

- `Dumper`：一次导出会话的状态聚合体。`tctx`/`cancel` 管取消与日志，`conf: Arc<Config>` 保存配置快照，`db`、`ext_storage`、`http`、`pd_client` 是外部句柄，`metrics`、`speedRecorder`、`status`、`totalTables` 保存共享统计。
- `NewDumper(Config) -> Result<Dumper>`：公开构造入口。先执行 `buildTLSConfig`、`validateSpecifiedSQL`、`adjustFileFormat`、`validateIncludeGeneratedColumns`，再通过 `runSteps` 顺序运行八个初始化步骤；任一步骤的 `Err` 都立即终止。
- `Dumper::{L, Close, Dump}`：分别暴露 logger、幂等式回收资源、执行完整导出。`Close` 通过 `Option::take` 防止同一句柄被重复关闭。
- `Dumper::{dumpDatabases, dumpWholeTableDirectly, dumpSQL, newTaskTableData}`：任务生产 API。普通对象生成 metadata/data 任务；`--sql` 生成名为 `result` 的匿名表和单个数据 chunk。
- `canRebuildConn`：编码 consistency 与 `TransactionalConsistency` 的连接重建矩阵。
- `runSteps` 及 `initLogger`、`createExternalStore`、`startHTTPService`、`openSQLDB`、`detectServerInfo`、`resolveAutoConsistency`、`validateResolveAutoConsistency`、`setSessionParam`：构造阶段的顺序步骤。`startHTTPService` 将非法地址视为致命错误，但端口占用等启动错误只记录 warning。
- `setDefaultSessionParams`：TiDB、含 TiKV 且版本不低于 6.2.0 时，用 `entry(...).or_insert("ON")` 默认打开 `tidb_enable_paging`，不覆盖用户值。
- `getListTableTypeByConf`、`prepareTableListToDump`/`prepareTableListToDumpInner`：选择表枚举方法并更新 `Config.Tables`。SQL 模式直接返回；显式表模式只调用 `filterTables`；普通模式枚举数据库及允许的 base/view/sequence 后再过滤。
- `dumpTableMeta`、`getColumnTypes`：构造 `tableMeta`。它优先复用缓存的 `columnProjection`，按对象类型读取 CREATE SQL，并对 TiDB 尝试探测隐式 row ID；该探测失败被忽略。
- `adjustDatabaseCollation`、`adjustTableCollation`：保留的排序规则兼容钩子；当前无论模式如何都原样返回 SQL，不能据此宣称已实现 Go 的 collation 重写。
- `PDSecurityOption`、`firstNonEmpty`、`pdSecurityOptionForGC`、`parseClusterSSLFlags`、`tidbResolveKeyspaceMetaForGC`：GC/PD 配置辅助。证书路径优先使用 `ClusterSSL*`，否则回落到通用 `Security`；后两项目前分别是透传和轻量探测/mock client 路径。
- `updateServiceSafePoint`、`updateKeyspaceGCBarrier`、`runGCProtectionUpdater`：以唯一 ID 更新 service safepoint 或 keyspace barrier。保护时间戳为 `snapshot_ts.saturating_sub(1)` 的等价分支，取消时执行清理。
- `columnProjection`：缓存原始列类型、选中列类型、SELECT 字段和可选投影 schema SQL。
- `prepareColumnProjection`、`buildColumnProjection`：按库表生成投影；需要输出 schema 且确实过滤列时，通过 `schema_projection` 解析/恢复 CREATE TABLE，并在所有 schema 建立后统一校验外键父表。
- `columnNamesToSelectFields`、`tableSourceColumnNames`、`tableSourceColumnTypes`、`GetPrimaryKeyAndColumnTypes`：字段引用和索引规划辅助函数；主键类型映射基于源列而非过滤后的选中列。

## 执行流程

1. CLI 调用 `NewDumper`。配置格式和 generated-column 选项在任何外部资源打开前完成校验；随后创建带取消函数的背景 context 和共享统计对象。
2. `runSteps` 严格按 `initLogger → createExternalStore → startHTTPService → openSQLDB → detectServerInfo → resolveAutoConsistency → validateResolveAutoConsistency → setSessionParam` 执行。`auto` 对 TiDB 解析成 `snapshot`，对 MySQL/MariaDB 解析成 `flush`，其他类型解析成 `none`。
3. `Dump` 先检查 column-filter 与 SQL 等选项组合，注册 metrics，并从 `db` 取得连接池。lock consistency 必须先列出表，以便一致性控制器建立锁定范围；其他模式在 `ConsistencyController::Setup` 后列出表。
4. 主闭包创建 metadata 连接，记录开始时间和全局元数据。记录全局元数据失败仅 warning，不终止导出；表清单、列投影或后续写出失败则向上传播。
5. `prepareTableListToDumpInner` 在非 SQL、非显式表路径枚举数据库与对象，并应用表过滤。`totalTables` 随后以 `SeqCst` 写入。
6. 有 column filter 且不是 SQL 模式时，`prepareColumnProjection` 预先缓存每张表的源列/选中列。若过滤影响 schema，则先建立所有 base-table 的投影 schema，再做外键父表校验，避免 `HashMap` 遍历顺序导致父表被误判为外部表。
7. 创建标准库 MPSC channel。`dumpDatabases` 同步生产 policy/database/table/view/sequence/data 任务；自定义 SQL 则由 `dumpSQL` 生产一个匿名 `TableData` 任务。发送失败统一报告 `task channel closed`。
8. 生产端返回后 sender 被消费并关闭，`rx.recv()` 循环以一个 `Writer` 顺序处理全部任务。回调更新 finished-table 和 completed-chunk metrics；随后停止进度记录、关闭 writer 连接、记录完成时间并写全局元数据。
9. 无论主闭包成功与否，`ConsistencyController::TearDown` 都会执行；其错误仅 warning，函数最终返回主闭包结果。CLI 再无条件调用 `Close`。

## 数据与状态

`Config` 以 `Arc` 共享，但初始化和准备阶段需要修改时使用 `clone_for_mutate()` 生成副本，再整体替换 `Dumper.conf`。这避免了内部可变引用跨 writer/回调共享，也意味着持有旧 `Arc<Config>` 的局部变量只看到替换前快照；例如 `dumpDatabases` 开始时明确克隆当前配置。

`Config.Tables` 是数据库名到 `Vec<TableInfo>` 的映射，是表数统计、任务生产和列投影的共同事实源。`Config.columnProjection` 以 `(database, table)` 为键缓存 `columnProjection`；当启用了 column filter 而缓存缺项时，`dumpTableMeta` 必须报错，不能临时按已经可能变化的 filter 重算。相关测试专门验证修改 filter 后仍使用已缓存投影。

`columnProjection.sourceTypes` 保留完整可写源列，`selectedTypes` 仅保留过滤后的列；前者供主键/索引类型判断，后者决定导出列和 writer schema。无需显式字段、没有删列且未启用 `CompleteInsert` 时 `selectField` 为 `*`，否则用反引号转义后的字段列表。

`metrics.totalChunks` 在 `newTaskTableData` 中增加；data task 完成时增加 `finishedTablesCounter` 和 `completedChunks`。当前实现把每个 data task 都当作完成表回调，虽然整表直出通常只有一个 chunk，但未来引入多 chunk 时必须重新检查“任务完成”和“表完成”的区别。

`totalTables`、`metrics.progressReady` 与 chunk 计数使用 `SeqCst` 原子顺序。`status`、`speedRecorder` 使用 `Arc<Mutex<_>>`，writer 回调捕获独立的 metrics `Arc`。外部资源用 `Option` 表示初始化阶段和已关闭状态。

## 依赖与调用关系

上游主链为 `dumpling/cmd/dumpling/main.rs::run_with_factory → export::NewDumper → Dumper::Dump → Dumper::Close`。RustCodeGraph 将目标文件标记为被 `dump_test.rs` 和 `prepare_test.rs` 使用；精确路径搜索还确认 CLI 的 `DumpSession for Dumper` 显式转发这三个方法。

`NewDumper` 的配置校验来自 `config.rs`，logger/context 来自三个 dumpling 基础 crate，存储创建来自 `Config::createExternalStorage`，HTTP 服务来自 `http_handler.rs`，数据库/连接来自 `stubs.rs` 与 `conn.rs`，server 探测和表/schema 查询来自 `sql.rs`。

`Dump` 向下依赖：

- `consistency.rs` 的 `NewConsistencyController`、`Setup`、`TearDown`；
- `metadata.rs` 的 `newGlobalMetadata`、record/write 方法；
- `prepare.rs` 的数据库与表清单准备；
- `schema_projection.rs` 的 schema 解析、投影恢复和外键验证；
- `task.rs` 的各类 `NewTask*` 构造器与 `TaskEnum`；
- `ir.rs`/`ir_impl.rs` 的 `TableMeta`、`TableDataIR`、`tableMeta`、`newTableData`；
- `writer.rs` 的 `NewWriter`、回调和 `handleTask`；
- `status.rs`/`metrics.rs` 的表数、进度和计数器。

需要注意 RustCodeGraph 对 `Dumper::Dump` 的同名查询同时返回 Go 和 Rust 候选，且一次无文件限定的 `callers NewDumper` 未在 30 秒内返回；本文对上游 Rust 边采用 `dumpling/**/*.rs` 的精确路径搜索消歧，没有把 Go 调用边当作 Rust 事实。

## 错误处理与边界

- 构造前配置校验、初始化步骤、表清单、column projection、任务生产和 writer 处理使用 `Result` 与 `?` 快速失败。
- `Dump` 在 `db` 或 `ext_storage` 未初始化时分别返回 `db not open`、`no storage`，而不是 panic；但 `detectServerInfo` 对 `d.db` 使用 `unwrap`，其安全性依赖 `runSteps` 中 `openSQLDB` 固定先执行这一不变量。
- 全局元数据读取失败、HTTP 非非法地址的启动失败、TiDB 隐式 row ID 探测失败和 consistency teardown 失败属于降级/最佳努力路径；其中前两者与 teardown 会记录 warning，row ID 探测失败只退化成 `false`。
- `dumpSQL` 忽略 channel send 的错误，而普通任务发送会返回 `task channel closed`。若未来让消费者并发启动或提前退出，应统一该错误语义。
- column filter 与 SQL 的冲突在 `Dump` 运行时再次验证，防止调用方绕过 CLI 直接构造或修改 `Config`。
- 有活动列过滤、需要输出 schema 且包含 view 时明确拒绝，因为当前 schema projection 不能安全重写 view；非 base 对象的 projection 为空。
- 过滤后没有可写列的精确行为由 `column_filter` 与测试共同约束：仅生成列且无过滤时允许空 projection；活动过滤选不到可写列时报错。
- `getColumnTypes` 使用 `LIMIT 1` 读取 driver column metadata，即便没有数据行也必须保留类型信息；闭包失败时清空临时向量。
- GC updater 每周期最多尝试 11 次，失败间隔为 10ms，周期为 `max(ttl/2, 1s)`；它不把持续更新失败返回给主线程，只能通过 context 取消退出。`ttl <= 0` 仍会被钳制为 1 秒周期，但 keyspace 路径的 `Duration::from_secs(ttl as u64)` 对负值没有业务校验，调用方必须保证 TTL 非负。

## 并发与资源生命周期

`Dumper` 创建一个统一取消域。`Close` 先调用 cancel，再停止 HTTP、关闭 PD client、关闭 DB，最后注销 metrics；各 `Option::take` 让重复 `Close` 不会重复操作已取走的句柄。CLI 即使 `Dump` 失败也会调用 `Close`。

GC 保护函数本身是阻塞循环，预期由调用方放入后台线程/任务；`dump_test.rs` 用 `thread::spawn` 验证这一用法。循环在更新重试和等待周期中都频繁检查 `tctx.Done()`，取消后先执行删除 safepoint/barrier 的 cleanup，再退出。唯一 ID 由前缀和当前纳秒时间组成，避免多个导出会话覆盖同一保护记录。

任务通道虽然是 MPSC，但当前 `Dump` 先同步调用 `dumpDatabases`，待 producer 返回后才进入 `rx.recv()` 消费。标准库 channel 是无界的，因此不会在生产阶段阻塞；代价是大量任务可全部驻留内存。writer 也只有一个，和 Go 的 `conf.Threads` 个 writer + errgroup 并发模型不同。

进度记录器在任务生产前启动，writer 循环结束后停止。错误路径上 `?` 可能在显式 `progress.stop()` 前返回；其最终清理能力取决于 `startLogProgress` 返回句柄的 `Drop` 实现，本文未把该点宣称为已验证。数据库连接大多以显式 `Close` 回收，部分 close 错误被有意忽略以保留主错误。

## 与 Go 版本的对应关系

直接对照文件是 [`dump.go`](dump.go)，相关 Go 回归是 [`dump_test.go`](dump_test.go)。Rust 保留了这些核心语义：初始化步骤串联、auto consistency 的基本类型映射、lock 模式提前列表、consistency setup/teardown、metadata 记录、schema/data 任务分类、column projection、外键在所有 schema 建立后验证、GC 保护点使用 `snapshotTS-1`、11 次重试和取消清理，以及 TiDB 6.2+ 的 paging 默认参数。

已确认的主要差异如下：

- Go `NewDumper` 还串联 `tidbSetPDClientForGC`、`tidbGetSnapshot`、`tidbStartGCSavepointUpdateService`；Rust 构造步骤没有这些接线，`tidbResolveKeyspaceMetaForGC` 也只是查询后确保 mock client 存在。
- Go 的 `resolveAutoConsistency` 会实际探测 MySQL `FLUSH TABLE WITH READ LOCK` 权限并回退为 lock；Rust 仅按 server type 改字符串。
- Go `validateResolveAutoConsistency` 只检查 snapshot 参数与 consistency 的组合；Rust还直接拒绝非 TiDB 的 snapshot consistency。
- Go `prepareTableListToDump` 针对 TiDB、sequence 能力和显式表分别选择/更新元数据；Rust 使用较简单的 consistency 选择，并在 `SpecifiedTables` 分支只过滤既有表。
- Go `Dump` 初始化列类型集合、严格 collation 映射、连接重建、位置重记、多 writer、并发/分块表扫描和 summary；Rust 当前走一个无界 channel、一个 writer、整表单 chunk。Rust 的 `adjustDatabaseCollation`/`adjustTableCollation` 仍是 no-op。
- Go 的 GC 重试等待为 1 秒并使用带超时的 cleanup context；Rust重试仅 sleep 10ms，cleanup 同步最佳努力执行。
- Rust 已额外接入独立的 `schema_projection` Rust 模块来完成投影 schema 的解析与恢复，其外部行为由 Rust 测试对照 Go 语义，而不是简单字符串删列。

因此维护时应以 Go 为行为基准，但不能假设 Go 文件中的所有分支已经在 Rust 主链可达；文档中的“当前支持”只以 Rust 源码和 Rust 测试为准。

## 扩展指南

- 扩展构造阶段：新增 `fn(&mut Dumper) -> Result<()>` 步骤并放入 `NewDumper` 的明确顺序位置；若步骤取得外部句柄，同时扩展 `Dumper` 字段和 `Close`。同步更新 `dump_test.rs` 或初始化校验所在的 `prepare_test.rs`。
- 补齐 PD/snapshot/GC 接线：优先对照 Go 的四个连续步骤，而不是只从 `setSessionParam` 启动 updater；必须测试 classical/keyspace 集群、显式 `--pd`、snapshot 获取失败、取消 cleanup 和 `Close` 的顺序。
- 引入多 writer 或分块：修改 `Dump`、`dumpDatabases`、`dumpWholeTableDirectly` 与 writer 回调；必须改为边生产边消费或明确通道容量策略，并纠正“每个 chunk 都增加 finished table”的统计。同步独立的 `writer_test.rs`、`writer_serial_test.rs`、`status_test.rs`，不要把测试写入本文件。
- 增加新对象类型：在 `TableType` 列举、`prepareTableListToDumpInner` 的允许类型、`dumpDatabases` 的 match、对应 `TaskEnum`/writer 处理及 schema 查询处成组接入，保持 metadata 先于 data 的顺序。
- 调整列过滤/schema projection：集中修改 `buildColumnProjection`、`prepareColumnProjection`、`dumpTableMeta` 和 `schema_projection.rs`；保持源列/选中列分离、全量 schema 后再验外键、缓存缺项即报错三个不变量。优先扩展 `dump_test.rs` 中 `column_projection_*` 独立测试。
- 实现 collation 兼容：不能只改两个 `adjust*Collation` 钩子，还需把调用接回 schema 生成主链，并对照 Go parser AST 变换；需要覆盖数据库、表、列已有/缺省 charset/collation 和解析失败。
- 改变公开 helper 时要注意 crate 的 `include!` 单包结构：这些 `pub fn` 可能被同 crate 任意实现文件和测试直接引用。先用带文件限定的 RustCodeGraph 查询或 `rg` 消歧，再改签名。
- 性能风险集中在无界任务积压、逐表 `SHOW`/`SELECT ... LIMIT 1`、schema parser 全量解析以及原子/锁更新；正确性风险集中在 consistency 生命周期、投影后约束、生成列和 GC 保护清理。

## 验证依据

事实核对使用了以下直接材料：

- Rust 源码：`dumpling/export/dump.rs`（1074 行，RustCodeGraph 报告 55 个符号）、`lib.rs`、`Cargo.toml`、`dumpling/cmd/dumpling/main.rs`。
- Rust 独立测试：`dump_test.rs`（关闭幂等、PD 配置、GC 重试/取消、表元信息、consistency 矩阵、session 参数、column projection、最终 SQL 导出状态等）与 `prepare_test.rs`（初始化/配置相关路径）；`lib.rs` 证明测试作为独立模块接入，符合测试不与源文件混放的仓库约束。
- Go 对照：`dump.go` 与 `dump_test.go`。重点逐段对照了 `NewDumper`、`Dump`、projection、表清单、metadata、GC updater 和 session 参数，而非仅依赖同名符号。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter dumpling/export` 确认目标及相邻文件；`node --file dumpling/export/dump.rs --offset 1 --limit 1200` 读取目标全貌；`query Dumper --kind struct` 确认 Go/Rust 同名候选；`callees 'Dumper::Dump'` 暴露同名消歧问题并提供相邻依赖候选。无文件限定的 `callers NewDumper` 在 30 秒内没有返回，因此上游 Rust 边改由精确路径搜索验证。
- 精确调用证据：`dumpling/cmd/dumpling/main.rs:64` 构造，`:149` 执行；`dump_test.rs` 直接覆盖 GC、metadata、projection 和 `Dump`；`prepare_test.rs` 直接覆盖 `NewDumper` 错误路径。

本任务是纯文档分析，未运行 Cargo。交付结构检查要求本文恰有“文件定位、核心职责、主要符号、执行流程、数据与状态、依赖与调用关系、错误处理与边界、并发与资源生命周期、与 Go 版本的对应关系、扩展指南、验证依据”十一个二级标题；链接均指向仓库内真实文件。人工复核结论是：本文能回答该文件为何存在、主链如何运行、哪些行为仍是简化实现，以及扩展时应改动和同步验证的入口。
