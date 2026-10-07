# `pkg/ingestor/ingestctrl/local.rs`

## 文件定位

[`local.rs`](local.rs) 是 `astersql-ingestor-ingestctrl` crate 的 local backend 控制面实现。它位于“已排序 KV / 外部有序数据”与“按 Region 写入并 ingest SST”之间：向上提供 `Backend`、`BackendConfig`、版本检查和辅助查询，向下把 Engine 生命周期交给 [`engine_mgr.rs`](engine_mgr.rs)，把实际并发导入交给 [`import_pipeline.rs`](import_pipeline.rs) 与 [`job_worker.rs`](job_worker.rs)。模块由 [`lib.rs`](lib.rs) 的 `pub mod local` 暴露。

仓库级 [`pkg/ingestor/doc.go`](../doc.go) 将 `ingestor` 定义为直接向底层存储 ingest SST，并负责本地/全局排序、Region split/scatter 和导入环境准备的包；本文件实现其中本地 Engine 的编排层，而不是 SST 编码器、RegionJob 的重试细节或底层存储客户端本身。当前可见的生产接线在 [`pkg/session/runtime/import_sst.rs`](../../session/runtime/import_sst.rs)：它以 `StoreBridge` 同时实现 `StoreHelper` 和导入客户端工厂，构造 `NewBackend`，再将其适配为 Lightning `backend::Backend`。

## 核心职责

- 集群兼容性前置检查：`Version`、`checkTiDBVersion`、`CheckTiFlashVersionForTables` 和 `TargetInfoGetter::CheckRequirements` 检查 TiDB/TiKV/PD 最低版本及旧 TiDB 上的 TiFlash 表冲突。
- 配置收敛：`BackendConfig::default` 给出本地目录、并发、Region 阈值和重试退避；`adjust` 保证并发、打开文件数、大小和键数阈值不会落到无效下限。
- Backend 生命周期：`NewBackend` 校验 rlimit、创建 `EngineManager`；`Close` 幂等关闭客户端工厂与 EngineManager；其余方法负责打开、关闭、清理、重置、刷盘和创建 Writer。
- 导入编排：`Backend::ImportEngine` 根据 Engine 类型分流到本地或外部数据路径，组织 split/scatter、任务生成、worker 创建、取消检查和导入统计校验。
- 状态与工具：维护每个 Engine 的已导入 KV 数，提供重复键控制器、磁盘占用快照、磁盘余量检查、Range 属性切分、Region 参数协商、SST 目录和后继键计算。

## 主要符号

- 常量：`DIAL_TIMEOUT`、`MAX_RETRY_TIMES`、`DEFAULT_RETRY_BACKOFF_TIME`、三项 gRPC keepalive/backoff 常量、两项 Range 属性采样距离、`OPEN_FILES_LOWER_THRESHOLD`、`SCAN_REGION_LIMIT`、`MAX_WRITE_AND_INGEST_RETRY_TIMES`、`UNLIMITED_RPC_RECV_MSG_SIZE`；`FORCE_PARTITION_REGION_THRESHOLD` 是可原子修改的全局阈值。部分常量在本文件未直接消费，是与 Go API/相邻模块共享的控制参数。
- `Version { major, minor, patch }`：支持有无 `v` 前缀、忽略 `-` 后预发布后缀；缺失的 minor/patch 以 `0` 补齐，非数字返回 `Error::InvalidData`。
- `ImportClient` / `ImportClientFactory`：抽象按 Store 创建客户端、写入本地 `Engine` 或通用 `engineapi::IngestData`，并显式关闭资源。`WriteAndIngestData` 的默认实现返回不支持外部数据的参数错误。
- `TargetCatalog` / `TargetInfoGetter`：隔离远端库表、组件版本和 TiFlash 副本查询。`DatabaseModel`、`TableModel` 是精简模型。
- `BackendConfig`：当前 Rust 所需的本地目录、worker 并发、重复键开关、打开文件数、keyspace/resource group/task type、前置检查开关、TiKV 磁盘检查、Region 阈值和重试退避。
- `SplitClient`：只暴露 `SplitKeysAndScatter`，使 Region 拆分可替换和测试。
- `Backend`：持有 `Mutex<BackendConfig>`、`Arc<EngineManager>`、可选工厂/分裂客户端、原子并发与关闭标记，以及 `Mutex<HashMap<EngineId, i64>>` 导入计数。
- 核心方法：`OpenEngine`、`CloseEngine`、`CleanupEngine`、`LocalWriter`、`ImportEngine`、`UnsafeImportAndReset`、`ResetEngineSkipAllocTS`、`SetTSBeforeImportEngine`、`RegisterExternalEngine`、`GetDupeController`。
- 纯函数：`splitRangeBySizeProps`、`verifyImportedStatistics`、`checkDiskAvail`、`engineSSTDir`、`GetRegionSplitSizeKeys`、`NextKey`。

## 执行流程

1. 构造：`NewBackend` 先调用 `BackendConfig::adjust`，再通过 `local_unix::VerifyRLimit` 检查进程打开文件上限，随后以配置和 `StoreHelper` 创建 `EngineManager`。任何一步失败都直接返回错误，不产生半初始化的 `Backend`。
2. 写入准备：调用者用 `OpenEngine` 创建/恢复 Engine，以 `LocalWriter` 获得批量 Writer，必要时调用 `FlushEngine` 或 `FlushAllEngines`，最后 `CloseEngine` 结束写入但保留本地数据。
3. 本地导入：`ImportEngine` 首先检查取消令牌；若不是外部 Engine，则以 `IMPORT_MUTEX_STATE_IMPORT` 锁定 Engine，调用 `finishWrite`，取得 Region split keys，按需调用 `SplitKeysAndScatter`，并把相邻 split key 组成半开 `KeyRange`。没有可组成的窗口时回退到 Engine 的完整键范围。
4. 本地任务流水线：有 `import_factory` 时，方法创建 `LocalEngineSource`、把每个范围映射成 `RegionJob`，并为 worker 创建指定 Store 的 `ClientWorker`，最后调用 `import_pipeline::do_import`；没有工厂时仅采用 `Engine::KVStatistics`，用于本地/测试型统计路径。
5. 提交成功状态：流水线返回后再次 `token.check()`，避免父任务取消后仍登记成功；然后执行 `FinishImport`、`verifyImportedStatistics`，最后写入 `imported_counts`。无论成功失败都会在闭包之后 `engine.unlock()`。
6. 外部导入：已注册的 `ExternalEngine` 由 `import_external_engine` 处理。空数据直接成功；非空数据先 split/scatter，且必须存在 `import_factory`。它使用 `ExternalEngineSource` 和 `ExternalClientWorker`，在 worker pool 启动时把 pool 交给 Engine，并在完成后要求流水线 count、`GetTotalLoadedKVsCount` 与 `ImportedStatistics().1` 三者一致。
7. 重置与复用：`UnsafeImportAndReset` 先导入再以 `alloc_ts=true` 重置；`ResetEngineSkipAllocTS` 以 `false` 重置，调用者随后必须用 `SetTSBeforeImportEngine` 写入有效 TS。该 Rust 方法只原子更新内存元数据，不像 Go 版本那样在 `ts == 0` 时向 PD 取 TS 并持久化元数据。
8. 清理：显式调用 `CleanupEngine`/`CleanupAllLocalEngines` 回收 Engine 数据；`Close` 只关闭工厂和管理器且幂等。生产适配器 [`pkg/session/runtime/import_sst.rs`](../../session/runtime/import_sst.rs) 的 `Drop` 还会清理所有 Engine 并删除任务临时目录。

## 数据与状态

- `BackendConfig` 的副本由 `Mutex` 保护；目前读取它的关键路径是 `OpenEngine` 和 `RetryImportDelay`。worker 并发另存于 `AtomicI32`，运行中可通过 `SetWorkerConcurrency` 调整，不需要锁住整份配置。
- `closed: AtomicBool` 通过 `swap(true, AcqRel)` 为 `Close` 提供一次性门闩；后续关闭调用立即返回。
- `imported_counts` 仅在完整导入和统计验证成功后更新。`GetImportedKVCount` 先读该缓存；锁被毒化或没有缓存时回退到 `EngineManager` 的记录，因此查询不会向外传播锁错误。
- 本地 Engine 的成功不变量是 `reported count == Engine::ImportedStatistics().1 == Engine::KVStatistics().1`；外部 Engine 的不变量是 `pipeline count == total loaded count == imported count`。
- `splitRangeBySizeProps` 维护当前区间起点及累计 size/keys，任一阈值达到即切分；忽略不大于当前起点的属性，遇到超过总范围末端的属性即停止。若最后一次切分后没有新增键，则把极小尾部并入上一段。
- `checkDiskAvail` 把容量为零视作尚无有效心跳信息并放行；否则可用空间严格低于总容量 10% 才返回 `DiskQuotaExceeded`，恰好 10% 可用。
- `GetRegionSplitSizeKeys` 分别收集所有正 size/key。只有各维度恰有一个唯一值时采用 Store 值；缺失或不一致时该维度单独回退默认值。

## 依赖与调用关系

- 上游生产调用：[`pkg/session/runtime/import_sst.rs`](../../session/runtime/import_sst.rs) 的 `Backend::new_with_key_prefix` 调用 `local::NewBackend`；其 `backend::Backend` 适配方法把 `OpenEngine`、`CloseEngine`、`ImportEngine`、`CleanupEngine`、`FlushEngine` 和 `LocalWriter` 转发给本文件。`register_external` / `import_external_native` 构成全局排序 Engine 的注册与导入入口。
- Engine 下游：`NewBackend`、打开/关闭/清理/重置、Writer、文件大小、重复数据和外部 Engine 注册均委托 [`engine_mgr.rs`](engine_mgr.rs)；本地 Engine 的范围、统计和导入完成状态来自 [`engine.rs`](engine.rs)。
- 导入下游：`ImportEngine` 构造 [`import_pipeline.rs`](import_pipeline.rs) 的 Source、WorkerFactory 和 `ImportOptions`；任务载体是 [`job_worker.rs`](job_worker.rs) 的 `RegionJob`，实际 I/O 通过注入的 `ImportClient` 完成。
- 通用接口：外部数据依赖 `astersql-ingestor-engineapi`，worker 缓冲池依赖 `astersql-lightning-membuf`；两者由本模块 `pub use` 暴露。重复键路径依赖 [`duplicate.rs`](duplicate.rs)，磁盘占用接口来自 [`disk_quota.rs`](disk_quota.rs)。
- crate 边界：[`Cargo.toml`](Cargo.toml) 声明 `astersql-ingestor-engineapi`、`astersql-lightning-membuf`、workerpool、metrics、compress 等直接依赖；大量尚在迁移中的 TiDB 子 crate 被列在 `cfg(windows)` 依赖块。本文件本身只直接使用上述核心 crate 和同 crate 模块。
- RustCodeGraph 索引将该文件标为被 10 个文件使用，并列出 `pkg/session/runtime/import_sst.rs`、`pkg/ingestor/globalsort/engine.rs`、`engine_api.rs` 及相关测试等引用者；精确 `callers/callees` 命令未返回边，因此具体生产调用关系以上述源码引用核验为准。

## 错误处理与边界

- 错误统一使用 crate 的 `Result<T>` / `Error`。版本解析与统计不一致属于 `InvalidData`，缺少外部导入工厂和旧版本 TiFlash 冲突属于 `InvalidArgument`，不存在 Engine 属于 `NotFound`，共享锁毒化属于 `Poisoned`。
- `TargetInfoGetter::CheckRequirements` 会顺序传播 TiDB、每个 TiKV、每个 PD 的查询或版本错误。当前对非空 TiFlash 副本列表只读取最低版本常量，并没有把 TiDB 实际版本传给 `CheckTiFlashVersionForTables`；因此真正的表级冲突判断需要调用者显式调用后者，不能把 `CheckRequirements` 描述为已完成该校验。
- 本地 `ImportEngine` 找不到 Engine 时返回 `NotFound`；Go 对照实现会把缺失 Engine 当作可跳过成功，这是兼容差异。外部 Engine 空数据直接返回成功，本地 Engine 则仍进入 finish/range/统计流程。
- 取消在导入开始和成功状态提交前各检查一次；worker 内部取消与错误传播由 `import_pipeline` 负责。独立测试 `import_propagates_cancellation_from_active_client` 证明活跃客户端返回后取消仍阻止导入计数落账。
- `SetTSBeforeImportEngine` 在 Engine 不存在时失败；它不检查 `ts == 0`，也不执行磁盘持久化。扩展故障恢复语义时必须先对齐 Go 的 PD 取 TS、import 锁状态和 `saveEngineMeta` 行为。
- `checkDiskAvail` 使用 `saturating_mul(10)` 避免极大容量乘法溢出；错误中的 `used`/`quota` 转为 `i64`，调用方若允许超过 `i64::MAX` 的容量，需要补充边界策略。

## 并发与资源生命周期

- `Backend` 可跨线程共享：注入 trait 均要求 `Send + Sync`，EngineManager 使用 `Arc`，配置和计数用 `Mutex`，关闭状态和并发用原子变量。
- `ImportEngine` 对本地 Engine 获取专用 import 锁，并保证闭包退出后显式 `unlock`；`SetTSBeforeImportEngine` 使用读锁/`rUnlock`。新增早退分支必须保持解锁路径完整，优先继续置于现有闭包内。
- `Ordering::AcqRel` 用于关闭门闩，worker 并发读取/写入使用 Acquire/Release，Engine TS 写入使用 Release；这些顺序保证跨线程观察到状态切换，但不替代 Engine 内部同步。
- 外部流水线的 `on_pool_started` 把真实 worker pool 注册给 Engine，使其加载过程能感知/利用相同池；测试 `external_import_pipeline_tunes_and_closes_actual_region_workers` 覆盖 worker 调整与关闭。
- `ImportClientFactory::Close` 和 `ImportClient::Close` 是显式资源边界；`Backend::Close` 不实现 `Drop`，所以直接使用本类型的调用者必须负责关闭。当前 session 适配器用自身 `Drop` 补齐 Engine 清理、Backend 关闭和临时目录删除。
- 取消与 worker 收尾不能只看函数返回：`context_cancellation_waits_for_running_workers`、`worker_error_cancels_other_running_workers`、`import_pipeline_marks_success_after_worker_cleanup` 等测试验证了等待运行中 worker、错误扇出取消和清理后才标记成功的约束；这些实现位于相邻流水线模块，但由本文件的 `do_import` 调用继承。

## 与 Go 版本的对应关系

Rust 文件直接对应 [`local.go`](local.go)，保留了版本常量、`BackendConfig`/`Backend`、Range 属性切分、Engine 导入、磁盘检查、重复键入口和统计校验的主要概念，命名也刻意保留 Go 风格以便逐项对照。独立 Rust 测试 [`local_test.rs`](local_test.rs) 与 [`local_check_test.rs`](local_check_test.rs) 对应同目录 Go 测试，而非内嵌在生产源文件中。

当前 Rust 不是 Go 文件的完整等价移植，重要差异包括：

- Go `BackendConfig` 还包含 PD 地址、连接压缩与连接数、写批量、split 并发、checkpoint、Pebble 内存/块配置、限速、write-stall、PD scheduler 范围和 RaftKV2 模式等；Rust 配置只保留当前接线所需子集。
- Go `NewBackend` 构造 PD HTTP/split/import 客户端、检查 multi-ingest、初始化 limiter/metrics 并检查 TiKV 空间；Rust 全部通过 `StoreHelper`、`ImportClientFactory`、`SplitClient` 注入，只负责 rlimit 与 EngineManager。
- Go `ImportEngine` 会动态查询 Store Region 配置、按大表强制 partition、暂停指定范围 PD scheduler、切换 import mode，并处理更多 Region/RPC 行为；Rust 将切分阈值放在配置中，并把并发写入 ingest 委托给简化后的流水线。
- Go 找不到本地 Engine 时跳过，Rust 返回 `NotFound`；Go `SetTSBeforeImportEngine(0)` 向 PD 分配 TS 并保存元数据，Rust 仅写入传入值；Go 的外部 Engine 统计校验只针对 globalsort 类型，Rust 对本地和外部路径都执行更严格的计数一致性检查。
- Go 的 `checkDiskAvail` 解析 PD HTTP StoreInfo 并在错误中区分 TiKV/TiFlash 地址；Rust 接收已归一化的 `StoreInfo { capacity, available }`，只产生统一配额错误。

因此扩展时应以 Go 行为作为语义目标，但必须结合当前 Rust 上游接线决定最小必要范围，不能因同名符号就假设网络、调度或持久化能力已经存在。

## 扩展指南

- 新增 Backend 行为时，先判断职责属于本文件的编排、`engine_mgr.rs` 的生命周期、`engine.rs` 的数据状态，还是 `import_pipeline.rs`/`job_worker.rs` 的并发与 RPC；不要把 worker 重试逻辑塞回 `ImportEngine`。
- 扩展配置时同步修改 `BackendConfig::default`、`adjust`、`NewBackend` 消费点和 [`pkg/session/runtime/import_sst.rs`](../../session/runtime/import_sst.rs) 构造处；对 Go 已有字段逐项说明是否接线，不要只增加未使用字段。
- 修改导入成功条件时同时维护本地 `verifyImportedStatistics`、外部三方计数检查和 `imported_counts` 的落账顺序。新增回归测试应放在独立 [`local_test.rs`](local_test.rs)，至少覆盖成功、取消、worker 错误、统计不一致与空 Engine。
- 修改版本/TiFlash/Region 参数逻辑时同步 [`local_check_test.rs`](local_check_test.rs)；特别是若补齐 `TargetInfoGetter::CheckRequirements` 的 TiFlash 判断，需要明确源表集合及真实 TiDB 版本从何而来。
- 修改 Range 切分时保持半开区间、无空洞/重叠、忽略越界属性和小尾段合并不变量，并扩展 `split_range_merges_tail_without_additional_properties` 与 Go `TestRangeProperties` 对应案例。
- 补齐 Go 的 PD scheduler、import mode 或动态 Region 配置属于跨模块能力，需先定义客户端 trait 和生命周期，再在本文件注入；同时评估取消后恢复 scheduler/mode 的清理保证，避免只接入成功路径。
- 性能风险主要在 worker 并发、split key 数量、锁持有时间和外部 Engine pool 生命周期；兼容风险主要在缺失 Engine、TS 分配、TiFlash 前置检查和计数校验与 Go 的差异。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ingestor/ingestctrl` 确认 `local.rs`、`local_test.rs`、`local_check_test.rs` 等均已索引；`node --file pkg/ingestor/ingestctrl/local.rs --offset ...` 完整读取 826 行并显示该文件被 10 个文件使用。精确查询确认 `local.rs::NewBackend`（371 行）和 `local.rs::splitRangeBySizeProps`（708 行）符号；`callers/callees` 对这些精确符号未输出调用边，此限制已用直接引用搜索补证。
- 已读生产与配置：[`local.rs`](local.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`engine_mgr.rs`](engine_mgr.rs) 的直接接口引用、[`pkg/ingestor/doc.go`](../doc.go)、[`pkg/session/runtime/import_sst.rs`](../../session/runtime/import_sst.rs)。
- Go 对照：[`local.go`](local.go) 中版本检查、`BackendConfig`、`NewBackend`、`splitRangeBySizeProps`、`verifyImportedStatistics`、`ImportEngine`、`SetTSBeforeImportEngine`；测试目录对照为 [`local_test.go`](local_test.go)。
- Rust 测试：[`local_test.rs`](local_test.rs) 的 `split_range_merges_tail_without_additional_properties`、`import_propagates_cancellation_from_active_client`、`test_check_disk_avail`、`test_backend_close_without_ti_kv_client`、`worker_error_cancels_other_running_workers`、`external_import_pipeline_tunes_and_closes_actual_region_workers`、`backend_imports_registered_external_data_and_verifies_loaded_statistics`；[`local_check_test.rs`](local_check_test.rs) 的 `test_check_requirements_ti_flash` 与 `test_get_region_split_size_keys`。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付结构以任务给定的 11 章节命令验证，并人工复核上述符号、调用方向、Go 差异及扩展风险均有源码或测试依据。
