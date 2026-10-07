# `pkg/executor/importer/table_import.rs`

[对应源文件](./table_import.rs)

## 文件定位

本文件属于 `astersql-executor-importer` crate；crate 根 `pkg/executor/importer/lib.rs` 通过 `mod table_import` 挂载并用 `pub use table_import::*` 对外再导出这里的公共类型和函数。它位于 IMPORT INTO / LOAD DATA 的单表执行层：上接 `LoadDataController`、分布式导入执行器和 `IMPORT FROM SELECT`，下接 Lightning 编码、local backend 引擎、mydump parser、TiKV Region 切分、校验和与元数据服务。

RustCodeGraph 将该文件列为 `pkg/dxf/importinto/task_executor.rs`、`pkg/executor/importer/import.rs` 等文件的依赖；其中分布式任务通过 `NewTableImporter`、`OpenDataEngine`、`OpenIndexEngine`、`ImportAndCleanup` 和失败时的 `CleanupAllLocalEngines` 驱动引擎生命周期，选行导入则经 `pkg/executor/import_into.rs` 调用 `ImportSelectedRows`。文件不是进程入口，也不负责 SQL 解析或任务持久化，而是把已经形成的 `Plan` 和数据源切片落实为本地排序、引擎导入及后处理。

## 核心职责

1. 用 `Chunk`、`TableRegion` 表示源文件切片，并由 `LoadDataController::PopulateChunks` 将 mydump 的 Region 划分结果按 engine ID 分组。
2. 用 `NewTableImporter` 准备 `{TempDir}/import-{Port}/{identifier}` 排序路径、编码表、Lightning backend、Region 切分参数和磁盘配额，组装一次单表导入会话。
3. 用 `OpenDataEngine` / `OpenIndexEngine`、`ImportAndCleanup` 和 `LocalEngineCleanup` 管理 local engine 从打开、写入、关闭、导入到清理的生命周期。
4. 实现 `TableImporterRuntime`，为 `crate::ProcessChunk` 提供数据源类型、表元信息、KV 编码器、parser 和 `IMPORT FROM SELECT` 的 chunk channel。
5. 用 `CheckDiskQuotaOnce`、`StartDiskQuotaCheck` 和 `DiskQuotaCheckHandle` 在本地排序期间检测配额，并串行化超限引擎的刷盘/导入。
6. 用 `PostProcess` 串联分配器 rebase 与 checksum；`VerifyChecksum`、`RemoteChecksumTableBySQL` 分别实现级别策略和降低 DistSQL 并发的重试策略。

生产环境细节被集中到 `TableImporterService` trait。该边界使本文件保留 Go 版编排语义，同时由宿主提供 backend、parser、容量查询、Region 规划、统计刷新和远端 checksum 等能力。

## 主要符号

- 常量：`CheckDiskQuotaInterval` 为 10 秒；`defaultMaxEngineSize` 为 `5 * 96 MiB`；`IndexEngineID = -1`，与正数数据引擎 ID 分离。
- `Chunk`：记录路径、文件大小、偏移、行号上下界、源类型、压缩方式和时间戳。`GetKey` 生成 `路径:偏移`；`GetSize` 委托 `ImportChunk` 的默认大小语义；`toSourceFileMeta` 构造 parser 所需元数据。其 `ImportChunk` 实现是 chunk 处理管线的适配层。
- `ImportRuntimeConfig`：宿主运行参数，包括临时目录、端口、PD 地址、二进制 keyspace 以及 Region split 大小/键数。
- `TableRegion`：服务层返回的单个 Region/engine 切片；`DiskQuotaState`：一次配额采样的超限 engine ID、正在导入数量和磁盘/内存总量。
- `TableImporterService`：外部依赖端口。必需实现编码表、backend、parser、Region 规划、配额、rebase、checksum、统计等操作；allocator 元数据相关方法带默认实现，未提供宿主绑定时会明确报错。
- `LocalEngineCleanup`：以 `Mutex<HashSet<i32>>` 跟踪尚未成功清理的引擎；`Record`、`Forget`、`CleanupAll` 分别登记、注销和兜底清理。
- `TableImporter`：核心会话对象，持有 controller、backend、`EngineManager`、表元信息、编码表、keyspace、Region 参数、配额锁、排序目录、可选查询 chunk 接收端和服务对象。
- `NewTableImporter` / `NewTableImporterForTest`：生产构造入口与等价测试入口。构造时 Region 参数取本地配置与 PD 值的较大者再乘 2；磁盘配额交给 `adjustDiskQuota` 限制。
- 引擎方法：`OpenIndexEngine` 计算索引 compaction 阈值；`OpenDataEngine` 打开数据引擎；`ImportAndCleanup` 先导入再清理并返回数据引擎 KV 数；`CleanupAllLocalEngines` 是失败重试前的兜底清理。
- 选行方法：`SetSelectedChunkCh` 安装 channel；`ImportSelectedRows` 执行打开双引擎、消费 channel、停止配额线程、关闭/导入引擎和后处理的完整流程。
- `DiskQuotaCheckHandle`：持有停止标志、条件变量与线程句柄；`Stop` 和 `Drop` 都走 `stop_and_join`，保证幂等唤醒并等待工作线程结束。
- 规划函数：`calculateSubtaskCnt`、`getAdjustedMaxEngineSize`、`LoadDataController::PopulateChunks` 决定子任务数、目标 engine 大小及 engine-to-chunk 映射。
- 后处理函数：`PostProcess`、`RebaseAllocatorBases`、`VerifyChecksum`、`RemoteChecksumTableBySQL`、`GetBackoffWeight`；辅助类型为 `RemoteChecksum` 与 `RemoteChecksumError`。
- 其它边界：`GetImportRootDir` 形成导入根目录；`FlushTableStats` 转发统计刷新；`newEtcdClientForAllocatorRebase` 从现有 metadata store 构造 namespaced etcd client。

本文件没有条件编译项；测试由 `lib.rs` 中独立的 `#[cfg(test)] mod table_import_test` 和 `table_import_testkit_test` 挂载，未内嵌在生产源文件中。

## 执行流程

### 文件导入/分布式子任务

1. 上游创建 `LoadDataController`，调用 `PopulateChunks`。该方法用 `getAdjustedMaxEngineSize` 请求 `TableImporterService::MakeTableRegions`，把每个 `TableRegion` 转成带统一 Unix 时间戳的 `Chunk`，并无条件补入 `IndexEngineID` 的空槽位。
2. `NewTableImporter` 读取 `RuntimeConfig`，调用 `prepareSortDir` 清理同 identifier 的残留目录，再建立编码表、backend 和 `EngineManager`。Region split 参数取配置值与 PD 值的最大值后饱和乘 2；配额取用户值、默认值或磁盘容量 80% 中适用者。
3. 执行器按分配的 engine ID 调用 `OpenDataEngine` 和 `OpenIndexEngine`。每次成功打开都会在 `LocalEngineCleanup` 登记；索引引擎还按“所有源文件真实大小 × 非聚簇索引数”估算 compaction 阈值。
4. `ProcessChunk` 通过 `TableImporterRuntime` 获取 parser 和 encoder，把源行编码进数据/索引引擎。文件首 chunk 先跳过 `Plan.IgnoreLines` 并设置初始 RowID；非首 chunk 用 `SetPos(offset, PrevRowIDMax)` 定位。Parquet 显式传入 location，其他格式由服务创建 parser。
5. 引擎关闭后，`ImportAndCleanup` 用构造时的 Region 参数执行 `ClosedEngine::Import`，记录非索引引擎的 KV 数，再执行 `Cleanup`。只有清理成功才从兜底集合移除；调用者遇错可调用 `CleanupAllLocalEngines`，使同 ID 子任务可安全重试。

### IMPORT FROM SELECT

1. 上游先用 `SetSelectedChunkCh` 安装 `mpsc::Receiver<QueryChunk>`，然后调用 `ImportSelectedRows`。
2. 方法打开 ID 1 的数据引擎和 ID -1 的索引引擎，启动配额检查线程，并以默认 `Chunk` 调用一次 `ProcessChunk`；实际选行数据由 `TakeQueryChunks` 提供的共享接收端消费。
3. 无论 `ProcessChunk` 成功还是失败，都会先调用 `quota_checker.Stop()`；这保证最终关闭/导入引擎时，不会与配额线程触发的 flush/import 竞争。
4. 成功路径依次关闭并导入清理数据引擎、索引引擎，锁住累计的 `KVGroupChecksum`，再调用 `PostProcess`。返回值只计数据引擎 KV 数，索引引擎返回 0。

### 后处理

`PostProcess` 先调用 `RebaseAllocatorBases`。只有目标表可能使用隐式 RowID、自增列或 AutoRandom 时才进入服务层 rebase；随后把组 checksum 合并成 `KVChecksum` 并调用 `VerifyChecksum`。checksum 级别为 `Off` 时完全跳过，`Optional` 会容忍查询失败或不匹配，`Required` 则传播错误。远端查询的可重试错误最多尝试 3 次，每次把 `DistSQLScanConcurrency` 的除数翻倍，并始终不低于 `MinDistSQLScanConcurrency`。

## 数据与状态

`TableImporter` 的长期状态以一次导入会话为界：`LoadDataController` 和 `Plan` 描述表、数据文件与策略；`table_info` / `encoding_table` 是编码元数据；`backend` / `engine_manager` 负责本地 SST 引擎；`keyspace` 作为多租户键前缀参与 checksum 和编码；`sort_directory` 与 `id` 标识本地资源归属。

`Chunk.Offset` / `EndOffset` 对 CSV/SQL 表示字节范围，对 Parquet 表示行范围，所以 `Chunk::GetSize` 经 `ImportChunk::GetSize` 对 Parquet 返回 `FileSize`。`PrevRowIDMax` 是该 chunk 编码起点，`RowIDMax` 是规划上界；`Timestamp` 被写入 `SessionOptions`，使同一规划批次具有一致时间语义。

子任务数量使用 `TotalRealSize` 而不是压缩文件大小：本地排序取 `round(total/max)`，至少为 1；global sort 再向上对齐到执行节点数的倍数。调整后的 engine 大小是 `ceil(total/subtask_count)`。`PopulateChunks` 的 map 键是不变量 engine ID，且始终含索引引擎键。

引擎清理集合只保存“已打开但尚未确认清理成功”的 ID。`CleanupAll` 先 drain，失败的 ID 会重新插回，防止一次失败永久丢失清理责任。磁盘配额锁只包住服务层 `FlushAndImportLargeEngines`，避免前台最终导入与后台超限导入并发操作同一 backend。

## 依赖与调用关系

上游直接证据：

- `pkg/executor/importer/lib.rs` 公开再导出本文件 API。
- RustCodeGraph 的文件关系显示 `pkg/executor/importer/import.rs` 使用本模块；`pkg/dxf/importinto/task_executor.rs` 构造 importer 并驱动分布式子任务的引擎生命周期。
- RustCodeGraph 调用关系显示 `pkg/executor/import_into.rs::importFromSelect` 调用 `NewTableImporter` 和 `ImportSelectedRows`；`pkg/dxf/importinto/task_executor.rs::build_parent_importer` 调用 Rust `NewTableImporter`。

主要下游依赖：

- `astersql_lightning_backend`：`Backend`、`EngineManager`、`OpenedEngine`、`ClosedEngine` 及 UUID/清理操作。
- `astersql_lightning_backend_encode`：`EncodingConfig`、`SessionOptions` 与 `EncodingTable`，构造行到 KV 的编码环境。
- `astersql_lightning_mydump`：源类型、压缩类型、文件元信息和 parser。
- `astersql_lightning_verification`：本地 KV/分组 checksum。
- `astersql_meta_model::TableInfo`、`astersql_lightning_backend_kv::AllocatorType`：表结构和 allocator 最大值。
- `astersql_metaservice`：allocator rebase 所需的 namespaced etcd client。
- crate 内 `ProcessChunk`、`HandleSkipNRows`、`NewTableKVEncoder`、`NewTableKVEncoderForDupResolve`：真实解析和编码管线。

`pkg/executor/importer/Cargo.toml` 将 crate 根定为 `lib.rs`，并声明上述 Lightning、mydump、ingestctrl、metaservice、meta-model 等本地 crate 依赖；没有控制本文件的 feature 开关。

## 错误处理与边界

本文件统一以 `Result<_, String>` 穿过服务边界，大多数底层错误用 `to_string()` 转换，因此保留可读信息但不保留具体 Rust 错误类型。需要调用者特别注意的边界如下：

- `prepareSortDirPath` 会删除同 identifier 的整个旧排序目录；若 import 根路径是普通文件，也会先删除再建目录。任一元数据、删除或创建错误立即返回。
- `NewTableImporter` 的目录、编码表、backend、PD Region 参数任一步失败都会中止构造；已创建资源的回收依赖服务/backend 自身及调用者的生命周期管理。
- `ImportAndCleanup` 即使导入失败也继续尝试 cleanup；最终用 `import_result.and(cleanup_result)` 返回错误。若两步都失败，返回值只保留前一个 `Result::and` 所决定的错误，区别于 Go 版 `multierr.Combine` 的聚合错误。
- `CleanupAll` 对单个清理失败不向上返回错误，而是重新登记该 ID；这是 best-effort 兜底，调用者无法从方法返回值获知失败详情。
- `CheckDiskQuotaOnce` 会传播 `FlushAndImportLargeEngines` 错误；后台 `start_disk_quota_check_with` 则刻意丢弃错误并在下一 tick 重试，与 Go 的“不因配额导入失败取消前台导入”语义一致。
- `TakeQueryChunks` 在未配置 channel 时返回明确错误；parser 首 chunk 的跳行和中间 chunk 的 seek 错误都会终止 chunk 处理。
- `adjustDiskQuota` 查询容量失败时，显式用户值优先，否则退回 `DefaultDiskQuota`；成功时上限为容量 80%。
- checksum `Optional` 会吞掉远端失败和不匹配；`Required` 传播查询或格式化后的 mismatch 错误。可重试查询耗尽后返回最后一次消息，非可重试错误立即返回。
- `newEtcdClientForAllocatorRebase` 在没有 metadata store 时明确返回 `TiKV store does not expose PD client`。

## 并发与资源生命周期

`TableImporterService: Send + Sync` 且以 `Arc<dyn ...>` 持有，允许导入器和配额线程共享。backend、配额锁与 channel 接收端也分别通过 `Arc`、`Mutex` 共享；锁中毒时采用 `into_inner()` 继续清理/停止流程，避免因另一个线程 panic 永久失去资源控制。

`DiskQuotaCheckHandle` 的 worker 使用 `Condvar::wait_timeout_while` 等待周期或停止信号。显式 `Stop(self)` 与析构 `Drop` 都设置停止标志、`notify_all` 并 `join`；worker 句柄用 `Option::take` 保证最多 join 一次。`ImportSelectedRows` 在关闭最终引擎之前停止并 join worker，这是防止配额导入与最终 import 并发的关键时序不变量。

`disk_quota_lock` 使同步检查与后台检查的 flush/import 临界区互斥。`LocalEngineCleanup.opened` 保护待清理 ID 集合；`CleanupAll` 不在持锁状态下调用 backend，而是先 drain，只有失败时短暂重新加锁，避免长时间持锁包围外部 I/O。

`Close` 依次关闭 controller 和 backend，但不返回错误；引擎级清理由 `ImportAndCleanup` 或 `CleanupAllLocalEngines` 承担。allocator rebase 的绑定结构还携带 `ResetConnection: FnOnce`，表明宿主必须在 etcd 关闭后释放 AutoID discovery 连接，不过该回调的具体调用位于服务实现而非本文件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/importer/table_import.go`，Rust 基本保留了以下结构：`Chunk`、排序目录准备、Region 参数读取、`TableImporter` 构造、子任务/engine 大小计算、`PopulateChunks`、双引擎打开、导入清理、配额循环、选行导入、allocator rebase、checksum 策略与统计刷新。`pkg/executor/importer/table_import_test.go` 的排序目录、子任务计算和 Region 错误用例，在 Rust 独立测试中有对应覆盖。

当前可见差异必须按现实理解，而不能假定完全等价：

- Go 直接持有 TiDB session、KV storage、logger 和 concrete ingest backend；Rust 通过 `TableImporterService` 注入宿主能力，并将具体网络、日志、session 及错误分类留在服务实现。
- Go `Chunk` 还保存 `ParquetMeta`；Rust `Chunk` 没有该字段，而是对 Parquet 通过 controller 的 `OpenParquetParserWithLocation` 路径打开，并在小文件内存估算时重新构造 `SourceFileMeta`。
- Go `ImportSelectedRows` 按 `ThreadCnt` 启动多个 worker 并合并各自 checksum；本文件当前只调用一次 `ProcessChunk`，共享 channel 的并发度因此不在这里展开。扩展时不能凭 Go 版直接假定 Rust 已具有同样 worker 并发。
- Rust 新增 `LocalEngineCleanup` 跟踪真实打开的引擎，并暴露 `CleanupAllLocalEngines` 给分布式任务失败路径；Go 同方法的历史实现主要依赖 defer/上层清理。
- Go `ImportAndCleanup` 会转换重复键错误并用 `multierr` 合并 import/cleanup 错误；Rust 本层只把 backend 错误字符串化，重复键转换若存在应由 backend/服务边界提供。
- Go 远端 checksum 直接操作 session、取消 goroutine 和 SQL killer，并恢复 session 并发；Rust 只实现三次降并发决策，把一次查询的 session/SQL 细节委托给 `TableImporterService::RemoteChecksumTableBySQL`。
- Rust 的 allocator trait 已预留 metadata store、dial config 和绑定工厂，但本文件的 `RebaseAllocatorBases` 最终只调用服务的高层 rebase；连接关闭及 `ResetConnection` 时序必须由具体服务实现验证。

## 扩展指南

- 新增源格式或 parser 行为：优先扩展 `TableImporterRuntime::GetParser` 与 `TableImporterService::NewParserWithParquetLocation`，同时检查 `Chunk`/`TableRegion` 是否需要携带额外元数据；测试放在独立的 `table_import_test.rs` 或相关 `chunk_process_test.rs`，不要嵌入生产文件。
- 调整分片策略：修改 `calculateSubtaskCnt`、`getAdjustedMaxEngineSize` 或 `PopulateChunks` 时，必须同步 Rust/Go 的边界表，特别覆盖压缩数据使用 `TotalRealSize`、global sort 对齐节点数、零节点和四舍五入临界值。
- 调整引擎配置：索引 compaction 变更接入 `OpenIndexEngine`，数据引擎变更接入 `OpenDataEngine`；需确认聚簇主键索引计数、分布式每子任务独立索引引擎，以及 `LocalEngineCleanup` 的登记/注销不变量。
- 调整配额策略：保持最终 engine close/import 前必须停止并 join 配额线程；新增检查结果应放进 `DiskQuotaState`，实际 backend 操作经服务 trait 完成，并测试错误重试、停止延迟和锁竞争。
- 调整错误语义：若需要与 Go 的重复键转换或多错误聚合完全一致，应在 `ImportAndCleanup` 及 backend 错误类型边界实现，不能仅靠字符串匹配；应增加导入失败与 cleanup 失败同时发生的回归测试。
- 调整 checksum：`VerifyChecksum` 负责级别策略，`RemoteChecksumTableBySQL` 负责重试/并发退避，服务方法负责一次真实查询。三层职责应保持分离，并覆盖 Optional/Required、不可重试、三次耗尽和最低并发。
- 调整 allocator rebase：先核对 `DesiredTableInfo` 的 AutoRowID/AutoIncrement/AutoRandom 判定，再同步服务实现中的 etcd 生命周期；需要测试连接关闭后执行一次 `ResetConnection` 的顺序。
- 新增公共 API 后还应检查 `lib.rs` 的再导出是否仍合适；crate 依赖变化才修改 `Cargo.toml`。任何 Rust 行为修改都应同步同目录独立测试，并尽量与 Go 的控制流和测试意图一致。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/executor/importer/table_import.rs` 读取了完整 1,101 行并确认该文件被 `pkg/dxf/importinto/task_executor.rs`、`pkg/executor/importer/import.rs` 等使用。
- RustCodeGraph 主要查询：`explore "pkg/executor/importer/table_import.rs main symbols callers callees table import"`；`callers/callees NewTableImporter`；`callers ImportSelectedRows`；`callers ImportAndCleanup`。这些查询确认构造器的内部调用和分布式/选行入口关系。
- 已读生产与装配文件：`pkg/executor/importer/table_import.rs`、`pkg/executor/importer/lib.rs`、`pkg/executor/importer/Cargo.toml`。
- 已读 Go 对照：`pkg/executor/importer/table_import.go`；重点核对 `NewTableImporter`、`calculateSubtaskCnt`、`PopulateChunks`、`ImportAndCleanup`、`CheckDiskQuota`、`ImportSelectedRows`、`PostProcess`、`VerifyChecksum` 和 `RemoteChecksumTableBySQL`。
- 已读独立测试：`pkg/executor/importer/table_import_test.rs`、`pkg/executor/importer/table_import_test.go`、`pkg/executor/importer/table_import_testkit_test.rs`。Rust 测试验证配额失败会重试且停止不挂起、排序目录的文件系统分支、子任务/调整后 engine 大小、Region 参数错误透传，以及 Parquet/非 Parquet 的 chunk 大小语义；testkit 文件保留 IMPORT FROM SELECT 出错后的目录清理契约参考。
- `pkg/executor` 下没有 `doc.go`，因此没有可额外引用的包级契约文件。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；只执行任务指定的 11 章节结构验证，并人工复核本说明能够回答文件存在原因、执行主链、资源/错误边界和安全扩展位置。
