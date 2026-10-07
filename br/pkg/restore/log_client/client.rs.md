# `br/pkg/restore/log_client/client.rs`

## 文件定位

该文件是 `astersql-br-pkg-restore-log-client` library crate 的核心编排层。crate 根 [`lib.rs`](lib.rs) 以 `pub mod client` 挂载它，并通过 `pub use client::*` 将其公开 API 扁平再导出；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 指向 Go 包 `br/pkg/restore/log_client`。它服务于 BR 的日志恢复/PiTR 链路，把 PD、外部存储、日志文件管理、MetaKV 处理、日志文件导入、压缩 SST 恢复、检查点和内部 SQL session 汇总到 `LogClient`。

生产调用已经接入两条明确主链：[`br/pkg/task/stream.rs`](../../task/stream.rs) 在启用检查点时调用 `LogClient::LoadOrCreateCheckpointMetadataForLogRestore`；[`br/pkg/task/restore_lifecycle.rs`](../../task/restore_lifecycle.rs) 的 `RestoreCompactedSST` 依次调用 `InitSSTFileRestorer` 和 `RestoreSSTFileSets`。MetaKV 子链则由 [`batch_meta_processor.rs`](batch_meta_processor.rs) 调用 `SeparateAndSortFilesByCF`、`LoadAndProcessMetaKVFilesInBatch`、`RestoreBatchMetaKVFiles` 等入口。

该文件不是所有 `LogClient` 行为的唯一实现位置：ID map/迁移存储相关的同类型 `impl` 位于 [`id_map.rs`](id_map.rs)，压缩 SST 流控估算与配置位于 [`flow_control.rs`](flow_control.rs)。同时，当前 Rust 移植仍依赖 [`stubs.rs`](stubs.rs) 提供 PD、RawKV、session、protobuf 风格数据结构等本地边界；因此本文严格区分“已执行真实逻辑”“委托给 trait/客户端”和“只记录状态或直接成功的桩”。

## 核心职责

1. 定义 `LogClient`、`LogRestoreManager`、`SstRestoreManager` 和 `restoreStatistics`，集中保存一次日志恢复所需的服务句柄、时间戳、检查点开关、ID、进度缓存与统计。
2. 负责客户端生命周期装配：`NewLogClient` 建立最小状态，`Init` 创建内部 session 并缓存 cluster ID，`InitClients` 创建日志 importer/工作池并准备 SST manager，`InstallLogFileManager` 绑定外部存储和恢复时间窗，`Close` 按依赖顺序释放资源。
3. 提供 MetaKV 文件的过滤、排序、跨 CF 分批和处理回调协议；保持 default CF 先于对应 write CF 的时序，避免只恢复 write 记录造成 meta 事务不完整。
4. 提供 DML KV 文件的单文件/批量调度：Put 文件先提交，Delete 文件延迟到所有 Put 回调返回后提交；批量路径按当前 Rust 数据模型可表达的表 ID 与 CF 分组。
5. 驱动压缩 SST 恢复：初始化真实 `SstRestorer`，执行流控调整、离线 import/normal mode 切换、恢复等待和原子统计累计。
6. 读取或创建日志恢复检查点元数据，使重试任务复用 GC ratio、RocksDB 后台任务数和 snapshot 恢复大小。
7. 暴露若干兼容入口和边界桩，包括 GC delete-range 缓存、schema 刷新、TiFlash 校验、MetaKV 写回和预切分；这些入口保持 Go API 形状，但并不都具有 Go 版完整副作用。

## 主要符号

- 常量：`MetaKVBatchSize = 64 MiB` 控制 MetaKV 聚合上限；`maxSplitKeysOnce = 10240`、`maxReadMetaKVFilesConcurrency = 128`、`rawKVBatchCount = 64`、`defaultRepairIndexSessionCount = 10` 保留 Go 数值；`operationHintRestoreID = "restore_id"` 是 operation metadata 的 hint 键。本文件中部分常量只作为对外兼容面，未在该文件的当前流程消费。
- `LogRestoreManager { fileImporter, workerPool }`：日志文件导入器及并发工作池。`NewLogRestoreManager` 创建工作池；与 Go 不同，检查点 manager 参数尚未接线，也没有 checkpoint runner 字段。
- `SstRestoreManager { closed, storeCount, replicaCount, workerPoolSize, restorer }`：压缩 SST 恢复状态。`restorer` 在 `InitSSTFileRestorer` 后才可用；`Close` 委托 `SstRestorer::Close` 并把 `closed` 置真。
- `restoreStatistics`：四个 `AtomicU64` 分别累计 SST KV 字节数、KV 数、物理大小和耗时纳秒；使用 Relaxed 原子顺序，只保证无数据竞争的计数聚合，不建立跨字段快照一致性。
- `LogClient`：中心状态对象。关键成员包括 `pdClient`/`pdHTTPClient`、`storage`、`unsafeSession`、`LogFileManager`、两个 restore manager、`currentTS`/`restoreTS`、`upstreamClusterID`/`restoreID`、`operationContext`、`useCheckpoint`、GC query 缓存和恢复统计。
- 构造与注入 API：`NewLogClient`、`SetRestoreID`、`SetOperationContext`、`SetRawKVBatchClient`、`SetRateLimit`、`SetCrypter`、`SetUpstreamClusterID`、`SetStorage`、`SetCurrentTS`、`SetRestoreTS`。`SetCurrentTS(0)` 是唯一在 setter 层显式拒绝的值。
- 生命周期 API：`Init`、`InitClients`、`InstallLogFileManager`、`InitSSTFileRestorer`、`Close`。调用顺序不是由类型系统强制，而是由 `Option` 检查及运行时错误保护。
- MetaKV API：`SortMetaKVFiles`、`SeparateAndSortFilesByCF`、内部 `sort_meta_kv_files`、`LoadAndProcessMetaKVFilesInBatch`、`RestoreBatchMetaKVFiles`、`filterAndSortKvEntriesFromFiles`。批处理协议由 `BatchMetaKVProcessor::ProcessBatch`（定义于 `batch_meta_processor.rs`）承接。
- DML apply API：`ApplyKVFilesWithBatchMethod` 和 `ApplyKVFilesWithSingleMethod`。两者消费 `LogIter`，同步调用调用方闭包，并传播迭代或闭包错误。
- SST API：`rewriteRulesFor`、`InitSSTFileRestorer`、`RestoreSSTFileSets`；后者调用 [`flow_control.rs`](flow_control.rs) 中同一 `LogClient` 的 `adjustTiKVFlowControlForCompactedSSTRestore`。
- 检查点 API：`LoadOrCreateCheckpointMetadataForLogRestore`。已存在时加载并返回持久配置；不存在时写入当前恢复上下文。
- 边界/辅助 API：`CleanUpKVFiles`、`PreSplitRegions`、`PutRawKvWithRetry`、`liveTiKVStoreCount`、`maxReplicaFromReplicateConfig`，以及仅供测试的 `TEST_NewLogClient*`。

## 执行流程

典型日志恢复初始化与执行的局部流程如下：

1. 上层以 PD client 和 PD HTTP client 调用 `NewLogClient`；构造器仅填默认值并将 `checkRequirements` 设为真，不会主动连接任何外部服务。
2. `Init` 通过 `Glue::CreateSession` 创建内部 SQL session，然后从 PD 读取 cluster ID。当前 Rust 版不会像 Go `Init` 那样同时取得并保存 domain；需要 domain 的 ID map/系统表路径必须由其他接线注入。
3. 上层先 `SetStorage`，再调用 `InstallLogFileManager(startTS, restoreTS, metadataDownloadBatchSize)`。函数拒绝未设置 storage 的状态，构造空 migration 列表及 `LogFileManagerInit`，成功后同时写入 `restoreTS` 和 `LogFileManager`。
4. `InitClients` 从 PD 获取 stores，排除 `engine=tiflash` 与 `tiflash_compute`，创建 `LogFileImporter` 和日志工作池；SST manager 记录存活 TiKV 数、PD replicate config 的最大副本数，以及 `7186 * stores.len()` 的工作池大小，但真正 restorer 仍为空。
5. 压缩 SST 路径由 `restore_lifecycle.rs::RestoreCompactedSST` 调用 `InitSSTFileRestorer`：再次读取 Up 且非 TiFlash 的 store ID，先让 importer 配置下载重试，再以 manager 预计算的 pool size 创建 `NewSimpleSstRestorer`。
6. `RestoreSSTFileSets` 对空集合立即成功；非空时先检查取消，再执行 TiKV 流控调整。离线模式先切 import mode；随后 `GoRestore`、`WaitUntilFinish`，遍历所有 SST 文件累计四项原子统计。只要已进入离线 import mode，正常模式切回会在恢复结果产生后执行，切回失败只记录警告，不覆盖恢复结果。
7. 检查点路径由 `stream.rs` 调用 `LoadOrCreateCheckpointMetadataForLogRestore`。它先设置 `useCheckpoint=true`；若元数据存在，加载并优先采用其中非空的 RocksDB jobs；若不存在，则持久化上游 cluster ID、恢复时间戳、rewrite TS、GC ratio、jobs、snapshot 大小和 TiFlash items。

MetaKV 批处理流程：

1. `SeparateAndSortFilesByCF` 用 `shouldReadMetaKVFile` 去除不可读文件，将空 CF/`default` 与 `write` 分开；两组均按 `MinTs → MaxTs → ResolvedTs` 升序。
2. `LoadAndProcessMetaKVFilesInBatch` 扫描 default 文件，将时间范围重叠且总长度不超过 64 MiB 的文件聚成一批；遇到边界时以新文件的 `MinTs` 作为 `filterTS` 处理旧 default 批。
3. 每次 default 批边界也推进 write 指针，把 `MinTs < filterTS` 的 write 文件交给 processor；processor 返回的未消费 entries 保留到下一批，并按每条约 2560 字节计入下一批估算。
4. 结束时以 `u64::MAX` 依次冲刷剩余 default 和 write，即使文件和 entries 都为空也调用两次 processor；独立测试固定了这一契约。
5. 恢复 processor 最终调用 `RestoreBatchMetaKVFiles`。当前实现只过滤/排序已有 entries、累计统计并推进进度；它不会读取文件字节、执行 schema rewrite 或调用 RawKV 写入。

DML apply 流程：

1. 单文件路径和批量路径都先暂存 Delete 文件，立即处理 Put 文件。
2. 批量路径对长度达到 `batchSize` 的 Put 文件单独回调；其余 Put 按 `TableId + Cf` 聚合，在数量或字节阈值到达时冲刷。由于当前 `LogDataFileInfo` 没有 Go 分组所需的 `RegionId`，Rust 无法保持 Go 的完整 `TableId + RegionId + Cf` 分组维度。
3. 迭代结束后冲刷所有 Put 尾批；因为闭包是同步的，返回即构成 Put 完成屏障。之后 Delete 文件按全局数量/字节阈值批量回调。单文件路径同样在全部 Put 返回后逐个提交 Delete。

## 数据与状态

- `LogClient` 是有状态的分阶段对象，而非不可变配置。`Option` 字段表示尚未初始化的资源；`InstallLogFileManager`、`InitSSTFileRestorer` 和 `RestoreSSTFileSets` 会针对关键缺失状态返回错误，但并非所有方法都验证完整初始化顺序。
- `currentTS` 是 MetaKV rewrite/checkpoint 中的 rewrite TS，禁止为 0；`restoreTS` 是目标恢复时间，既可由 setter 写入，也会被 `InstallLogFileManager` 覆盖。`upstreamClusterID` 与 `restoreID` 分别用于检查点/ID map 和任务隔离。
- `SetRestoreID` 与 `SetOperationContext` 都会调用 `setOperationContextRestoreID`：非零 ID 写入十进制字符串，0 则写入空字符串，避免 operation context 遗留旧任务标识。
- `clusterID` 为 0 时 `GetClusterID` 每次委托 PD；非零时使用缓存。`Init` 会直接填充缓存。
- `deleteRangeQuery` 是进程内 `Vec<PreDelRangeQuery>`。`RecordDeleteRange` 追加，`GetGCRows` 借用查看，`InsertGCRows` 当前仅清空；`gcLoaderStarted` 也只是同步布尔标记。
- MetaKV entries 以 `Ts < filterTS` 进入当前批，`Ts >= filterTS` 结转；当前批按 `Ts` 再按 key 字节序排序。严格小于边界是跨 default/write 时序的重要不变量。
- `ApplyKVFilesWithBatchMethod` 的每个 Put batch 保存表 ID、CF、文件向量和累计长度；Delete 文件在独立全局向量中延迟处理。回调取得文件所有权，函数内部不会重试失败的批次。
- `SstRestoreManager.storeCount` 只计算 `State::Up` 的 store；`workerPoolSize` 却按排除 TiFlash 后的全部 store 数乘以 7186。`replicaCount` 从 PD HTTP `max-replicas` 读取，缺 client、取消、重试耗尽、缺字段或非法值均回退 3。
- SST 统计用 `wrapping_add` 计算单次日志展示总量，而长期字段使用 `AtomicU64::fetch_add(Relaxed)`；极端溢出会回绕，不会报错。

## 依赖与调用关系

RustCodeGraph 将该文件标记为被 `br/pkg/task/restore_lifecycle.rs`、`br/pkg/task/stream.rs`、`import_retry.rs` 及相关测试使用。由于索引的 `callers` 子命令未返回这些方法级上游，源码精确检索补充确认了以下生产边：

- `br/pkg/task/stream.rs` → `LogClient::LoadOrCreateCheckpointMetadataForLogRestore`：日志恢复启动时保存或复用全局 TiKV 配置。
- `br/pkg/task/restore_lifecycle.rs::RestoreCompactedSST` → `InitSSTFileRestorer` → `RestoreSSTFileSets`：压缩 SST 的生命周期编排。
- `batch_meta_processor.rs::RestoreMetaKVProcessor::RestoreAndRewriteMetaKVFiles` → `SeparateAndSortFilesByCF` → `LoadAndProcessMetaKVFilesInBatch` → `BatchMetaKVProcessor::ProcessBatch` → `LogClient::RestoreBatchMetaKVFiles`。
- `batch_meta_processor.rs::MetaKVInfoProcessor::ReadMetaKVFilesAndBuildInfo` 复用同一批框架，但其 processor 只解析映射与表历史，不写回集群。

主要直接下游如下：

- [`log_file_manager.rs`](log_file_manager.rs)：`CreateLogFileManager`、DML 迭代器、MetaKV 可读性判定、文件/entry 类型。
- [`import.rs`](import.rs)：`NewLogFileImporter` 与 `LogFileImporter::{ClearFiles, Close}`。
- [`log_split_strategy.rs`](log_split_strategy.rs)：`PreSplitRegions` 使用的检查点过滤和累计策略。
- [`migration.rs`](migration.rs)：按 start/restore TS 构造 migration 视图。
- [`ssts.rs`](ssts.rs) 与 `astersql-br-pkg-restore`：rewrite rule 适配、`SstRestorer`、`BatchBackupFileSet` 和 import mode switcher。
- [`flow_control.rs`](flow_control.rs)：SST 恢复前的 pending compaction 估算与 TiKV 参数调整。
- [`id_map.rs`](id_map.rs)：`SaveIdMapWithFailPoints`、`GetBaseIDMapAndMerge` 所委托的 `saveIDMap`/`loadSchemasMap`。
- `stubs.rs`：PD、PD HTTP、RawKV、Glue/Session、Storage、checkpoint、stream mapping、logging 和统一 `Error/Result` 边界。

Cargo 直接依赖包括 `astersql-br-pkg-stream`、`utils`、`restore`、`checkpoint`、`restore-split`、`restore-utils`、`utils-iter` 和 `astersql-errors`，另有 `serde_json` 用于 PD replicate config。manifest 明确说明当前 arm64 Darwin 路径避免直接引入 kv/domain/kvproto/grpcio，优先使用 slim path dependencies 与本地 stubs；不能据此文档声称已接通 Go 的全部真实外部客户端。

## 错误处理与边界

- `Init` 会传播 session 创建错误；PD `GetClusterID` 在当前 trait 中直接返回值。若 session 已写入而后续步骤扩展为可失败，需考虑半初始化清理。
- `InstallLogFileManager` 在 `storage=None` 时返回 `"storage unset"`；创建 manager 的错误原样传播。`PreSplitRegions` 在 manager 未安装时返回 `"log file manager not installed"`。
- `InitSSTFileRestorer` 在 SST manager 未由 `InitClients` 创建时失败；`RestoreSSTFileSets` 在 manager/restorer 缺失时失败。空 file set 在这些访问之前直接成功。
- `rewriteRulesFor` 只在 rewritten table ID 不等于原 TableID 时克隆规则并重写 source table ID；规则不存在时返回带上下文错误。与 Go 不同，它尚未为未设 TS 的 compacted SST 补充时间范围。
- `RestoreSSTFileSets` 在入口检查取消，在 PD store 查询所用的桥接 context 中也保留取消。`GoRestore` 或 `WaitUntilFinish` 的错误会返回；离线模式切回 normal 的错误只告警，避免覆盖主恢复错误。
- `LoadOrCreateCheckpointMetadataForLogRestore` 将 checkpoint crate 错误转换为本 crate `Error`。读取已存在元数据时，仅在保存值非空时覆盖传入 jobs；GC ratio 和 snapshot 大小始终采用已保存值。
- `LoadAndProcessMetaKVFilesInBatch` 和两个 apply 函数遇到迭代错误或回调错误立即返回，不自动补偿已处理批次。调用者若重试，必须依赖检查点或底层幂等性。
- 当前 `RestoreBatchMetaKVFiles` 在 `cur` 为空时不会调用进度回调，即使 `files` 非空；非空时每个输入文件推进一次。它没有真实读取文件、rewrite 或 RawKV Put，这是明确功能边界。
- `PutRawKvWithRetry` 名称保留 Go 语义，但当前仅调用一次 `RawKVBatchClient::Put`；本层没有 backoff/retry。
- `validateNoTiFlashReplica` 恒成功；`UpdateSchemaVersionFullReload` 与 `RefreshMetaForTables` 只记录日志；`InsertGCRows` 只清空缓存。这些 API 不能作为完整恢复副作用已完成的证据。
- `PreSplitRegions` 当前仅用 `LogSplitStrategy` 跳过/累计文件，在阈值处 `ResetAccumulations`；没有创建 PD splitter 或提交 region split。因此其成功只表示扫描完成，不表示 Go 版预切分已发生。
- `InitClients` 的 Go 版会探测 import 能力、限速、checkpoint runners 及 batch/simple restorer；Rust 版目前只创建日志 importer 和 SST manager 元数据。扩展时不能把这些缺口隐藏成默认成功。

## 并发与资源生命周期

`LogClient` 大多数状态通过 `&mut self` 串行修改，`unsafeSession` 的 Go 对照也明确非线程安全。该文件本身不启动 async runtime；工作并发由 `tidbutil::WorkerPool`、importer 和 `SstRestorer` 等下游对象承担。MetaKV 和 apply 回调均为同步 trait/闭包调用，函数返回前当前批已经完成或报错。

`Close` 的顺序是 session → `LogFileManager` → RawKV client → log restore manager/importer → SST restorer，最后记录关闭日志。各字段不会在关闭后置 `None`，也没有统一 `closed` 门闩；重复调用是否安全取决于下游 `Close` 实现，只有 `SstRestoreManager.closed` 被显式设置。`LogRestoreManager::Close` 和 `SstRestoreManager::Close` 都吞掉 close 错误并警告。

离线 SST 恢复的 mode 生命周期具有显式保护：只有 `GoSwitchToImportMode` 成功后才执行恢复，随后无论恢复成功还是失败都会尝试 `SwitchToNormalMode`。但是进程级崩溃仍需外部恢复机制。`InitSSTFileRestorer` 创建的 worker pool 所有权转移给 restorer，并由 manager 的 `Arc<dyn SstRestorer>` 保存。

四项统计使用原子字段，允许下游并发结束后安全累计；Relaxed 顺序适合独立计数，但读取者若需要四项一致快照必须另加同步。`deleteRangeQuery`、`LogFileManager`、manager options 等普通字段没有内部锁，不能在没有外层互斥的情况下并发修改。

资源规模方面，MetaKV batch 按文件 `Length` 加残留条目估算限制在约 64 MiB，但实际 processor 内存还取决于读取实现；apply 聚合由调用方传入的 count/size 双阈值控制。SST worker pool 固定为 `7186 * store_count`，是显著的资源参数，变更必须与 Go 行为及下游 worker pool 实现共同评估。

## 与 Go 版本的对应关系

直接对照文件为 [`client.go`](client.go)，测试意图来自 [`client_test.go`](client_test.go)。主要一致点与差异如下：

- 常量数值、核心类型名称、setter、资源关闭顺序、MetaKV 双 CF 批处理、Put-before-Delete、replica config 默认 3、检查点字段和 SST 统计字段均按 Go 形状移植。
- Go `NewLogClient` 还保存 TLS/keepalive 并创建 delete-range channel；Rust 构造器没有这些字段。Go `Init` 创建 session 与 domain，Rust 只创建 session 并缓存 PD cluster ID。
- Go `InitClients` 自行创建 split/import clients，执行能力探测、速率限制回调、snapshot importer、checkpoint runner，并选择 batch/simple restorer；Rust 接收已建的 split/import client，只装配日志 importer 和尚无 restorer 的 SST manager，之后由 `InitSSTFileRestorer` 固定创建 simple restorer。
- Go `LogRestoreManager`/`SstRestoreManager` 会等待 checkpoint runner 收尾；Rust 对应结构没有 runner，构造参数也尚未接线。
- Go `rewriteRulesFor` 对 compacted SST 还会在规则未设 TS 时写入时间范围；Rust 当前只处理 rewritten table ID。
- 两版 MetaKV 批框架都以 default 为驱动、用严格 `write.MinTs < filterTS` 推进 write 流，并以最大 TS 冲刷尾批。Rust 增加 `ResolvedTs` 作为完全相同 `MinTs/MaxTs` 的稳定 tie-breaker；独立测试固定了该顺序。
- Go `RestoreBatchMetaKVFiles` 会并发读文件、重写 schema、RawKV batch put 并处理 delete range；Rust 当前只处理传入的残留 entry、统计和进度，属于显式边界桩。
- Go batch apply 按 `TableId + RegionId + CF` 分组并累计 KV count/size，等待异步 `WaitGroup` 后处理 Delete；Rust 数据结构没有 `RegionId`，只按 `TableId + CF` 分组，也不向回调提供聚合 KV count/size，但同步回调返回充当屏障。
- Go `PutRawKvWithRetry` 具有重试逻辑，Rust 只委托一次 Put。Go GC row loader 用 channel/wait group 和 SQL session，Rust 只保留向量与标记。
- Go `PreSplitRegions` 汇总所有有效 DML 文件范围并调用 `PipelineRegionsSplitter::ExecuteRegions`，返回是否执行 split；Rust 签名和行为均不同，只扫描 `LogSplitStrategy` 并复位累计，不执行 PD split。
- Go `RestoreSSTFileSets` 的模式切换、恢复等待与统计意图已保留；Rust 额外接收 restore crate 的独立 context，以弥合两个 crate 的 context 类型，并在入口明确检查 log context 取消。

Rust 独立测试 [`client_test.rs`](client_test.rs) 覆盖 operation hint、GC query 缓存、MetaKV 排序/可读性/双 CF 冲刷、统计与进度回调、batch/single apply 顺序、ID map、split strategy、RawKV 成功路径和 schema 冒烟；[`flow_control_test.rs`](flow_control_test.rs) 覆盖 `InitClients` 与 `RestoreSSTFileSets` 的流控/恢复路径；[`parity_test.rs`](parity_test.rs) 提供跨 Go 意图的组合契约。部分测试注释明确采用宽松 stub 断言，因此“测试存在”不等价于对应 Go 副作用已完整移植。

## 扩展指南

- 完成 MetaKV 恢复时，主要入口是 `RestoreBatchMetaKVFiles` 与 `filterAndSortKvEntriesFromFiles`：需要接入 `LogFileManager` 的真实读取、`SchemasReplace`、RawKV batch put/delete-range，并保持 `Ts < filterTS`、default-before-write、残留 entries 和进度回调契约。同步扩展 `client_test.rs` 的文件读取失败、rewrite、过滤边界、批次残留和幂等重试测试。
- 完成 region 预切分时，应以 Go `PreSplitRegions` 的当前增量为准，在 Rust 中汇总非 meta、规则命中的 DML 范围并真正调用 splitter；不得把现有 `ResetAccumulations` 当成提交 split。应新增独立测试验证空输入、排除表、迭代错误、split 错误和成功返回语义。
- 恢复完整 `InitClients` 时，要处理 capability checks、checkpoint runners、速率限制回调、batch/simple restorer 选择、TLS/keepalive 与 API version。外部 Rust 依赖若缺失，必须按仓库规则在独立上游仓库移植并以统一 tag 引用，不能复制到 `vendor`/`third_party` 或使用本地 `[patch]`。
- 修改 apply 分组时，需要先为 `LogDataFileInfo` 补齐可信的 `RegionId`，再恢复 Go 的三维分组和 KV count 回调；同步检查 `client_test.rs::test_apply_kv_files_batch_preserves_go_grouping_and_delete_order`、oversized put、Delete 最后提交及回调错误传播。
- 完成 GC/schema 路径时，分别在 `RunGCRowsLoader`/`InsertGCRows`、`UpdateSchemaVersionFullReload`、`RefreshMetaForTables` 接入 session/domain 行为，并保持 session 非线程安全约束；测试必须继续位于独立 `client_test.rs`，不要写回生产文件。
- 修改 SST 生命周期时，应联合检查 `InitClients`、`InitSSTFileRestorer`、`RestoreSSTFileSets`、`SstRestoreManager::Close` 与 `flow_control.rs`；重点风险是 pool 规模、store 过滤、副本估算、取消桥接、import mode 必须复原和统计溢出。
- 修改检查点字段或重试语义时，需同步 `stream.rs` 的调用、checkpoint crate 数据结构和 Go `LoadOrCreateCheckpointMetadataForLogRestore`；旧元数据兼容、空 jobs 的回退规则以及 `useCheckpoint` 设置时机都属于持久化兼容契约。
- 所有 Rust 行为修复都应与 `client.go`/`client_test.go` 的对应增量保持一致，并同步同目录独立 Rust 测试；不能通过删减逻辑或只保留成功桩来让测试通过。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 确认目标、Go 对照、模块和测试均被索引。
- RustCodeGraph `node --file br/pkg/restore/log_client/client.rs --offset ...`：分段读取 1,103 行目标源码，核对 6 个常量、4 个主要状态类型、构造/setter、生命周期、MetaKV、apply、checkpoint 和 SST 流程。
- RustCodeGraph `query`：定位 Rust/Go 的 `NewLogClient`、`InitClients`、`InstallLogFileManager`、`PreSplitRegions`、`LoadAndProcessMetaKVFilesInBatch`、`ApplyKVFilesWithBatchMethod`、`RestoreSSTFileSets`、`LoadOrCreateCheckpointMetadataForLogRestore`。
- RustCodeGraph `callees`：确认 `InitClients` → `NewLogRestoreManager`/`NewLogFileImporter`/`liveTiKVStoreCount`/`getMaxReplica`，`InstallLogFileManager` → `CreateLogFileManager`，`PreSplitRegions` → `NewLogSplitStrategy`/`ShouldSkip`/`Accumulate`/`ShouldSplit`/`ResetAccumulations`，以及 SST 恢复的 flow-control/restorer 下游。`callers` 对这些方法未返回结果，因此使用源码精确检索补齐上游证据。
- 直接读取的 Rust 文件：`Cargo.toml`、`lib.rs`、`batch_meta_processor.rs`、`flow_control.rs`、`id_map.rs`、`client_test.rs`、`flow_control_test.rs`、`parity_test.rs`、`br/pkg/task/stream.rs`、`br/pkg/task/restore_lifecycle.rs`。
- 直接读取的 Go 对照：`client.go` 的类型/构造、SST 恢复、Init/InitClients、checkpoint、LogFileManager、apply、MetaKV batch、PreSplitRegions；并以 `client_test.go` 的同路径测试作为原始测试意图来源。
- 人工复核结论：本文区分了生产接线、trait 委托与边界桩，明确记录了 Rust 相对 Go 的功能差异、资源生命周期和扩展测试位置，没有把预期设计写成当前已支持行为。
- 结构验证按任务要求检查目标文件存在且恰有十一个固定二级标题。本任务只新增文档，按计划不运行 Cargo。
