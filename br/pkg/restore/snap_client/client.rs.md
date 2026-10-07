# `br/pkg/restore/snap_client/client.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-snap-client` library crate；crate 根由 `br/pkg/restore/snap_client/Cargo.toml` 指向 `lib.rs`，`lib.rs` 以 `pub mod client` 加载本文件并通过 `pub use client::*` 扁平导出 API。它是快照恢复客户端的控制面：把备份元数据、PD/store 视图、DDL session、SST importer/restorer、checkpoint 和恢复模式集中到 `SnapClient`，并为相邻的 `import.rs`、`tikv_sender.rs`、`pipeline_items.rs`、`systable_restore.rs` 提供共享状态。

Cargo 元数据把该 crate 标记为 Go 包 `br/pkg/restore/snap_client` 的 Rust 移植，且注释明确当前 arm64 Darwin 构建不直接依赖真实 `kv/domain/kvproto/grpcio`，而是通过 `stubs.rs` 的本地 trait 和数据模型隔离外部系统。因此，本文件当前既是可执行的控制逻辑，也是迁移期适配层；不能把 `Mem*` 测试实现理解为生产 PD/TiKV 客户端。

## 核心职责

1. `NewRestoreClient` 建立零值兼容的恢复控制器，保存 `PdClient` 与 `StoreMeta`，其余外部资源延迟注入；`NewRestoreClientForTest` 仅为独立 Rust 测试补齐内存桩和最小默认值。
2. `InitConnections`、`LoadSchemaIfNeededAndInitClient`、`initClients` 按阶段装入 Domain/SQL session、备份元数据、split/import 客户端和 store 列表，并构造 `SnapFileImporter`。
3. `AllocTableIDs`、`CreateDatabases`、`CreateTables`、`ExecDDLs` 管理 schema 恢复；其中建表后会重新读取下游表信息并生成旧 ID 到新 ID 的 rewrite rule。
4. `InitCheckpoint` 恢复或创建快照恢复 checkpoint，校验命令 hash、上游集群、恢复时间戳和日志恢复时间戳；`execAndValidateChecksum` 复用或写入 checkpoint checksum。
5. `GetFilesInRawRange`、`ResetTS`、placement policy、全量/增量判定等方法提供各恢复模式的边界控制。
6. `SetSpeedLimitCallbacks` 管理 TiKV 下载限速的设置、定期续租和关闭清零，`Close` 统一释放 importer、restorer 与数据库 session。

## 主要符号

- 常量：`STRICT_PLACEMENT_POLICY_MODE` / `IGNORE_PLACEMENT_POLICY_MODE` 定义 placement policy 策略字符串；`RESET_SPEED_LIMIT_RETRY_TIMES` 为关闭时清零限速的三次重试；`DEFAULT_DDL_CONCURRENCY`、`MAX_SPLIT_KEYS_ONCE`、`MIN_BATCH_DDL_SIZE` 保留与 Go 控制参数的对应关系。
- `SnapClient`：核心状态容器。连接类字段包括 `pdClient`、`pdStore`、`meta_client`、`import_client`、`dom`、`db`、`dbPool`；恢复类字段包括 `backupMeta`、`databases`、`ddlJobs`、`preallocedIDs`、`policyMap`、`checkpointRunner` 和 `checkpointChecksum`；配置类字段包括各并发度、限速、rewrite/policy 模式和全量恢复开关。
- `NewRestoreClient(pd_client, pd_store)`：生产形状的构造函数，不主动联网，也不创建 importer/session；默认 rewrite mode 为 legacy，多数配置保持 Go 零值。
- `LoadSchemaIfNeededAndInitClient(...)`：元数据与 importer 初始化的主要入口。RawKV/TxnKV 不加载 schema 和 DDL；TiDB full 模式加载它们，并依据模式选择 importer API version。
- `AllocTableIDs(...)`：新分配或复用 checkpoint 中的 ID，检测用户表 ID 是否可复用，将分配器注册到所有 session，并识别临时系统表已经改名的恢复状态。
- `CreateTables(...)`：优先走批量建表；仅当错误满足 `fallBack2CreateTable` 的“不支持 batch DDL”条件时回退逐表路径，其他错误直接传播。
- `InitCheckpoint(...)` / `WaitForFinishCheckpoint(...)`：checkpoint 元数据、已完成 range、checksum 和 runner 生命周期入口。
- `SetSpeedLimitCallbacks(...)` / `SetSpeedLimitFn(...)` / `setSpeedLimitForTask(...)`：为所有 TiKV store 并发下发 task 级限速，并生成 importer create/close 回调。
- `getMinUserTableID`、`makeDBPool`、`needLoadSchemas`、`SortTablesBySchemaID`：分别处理用户表/分区最小 ID、session 池的部分成功返回、模式判定和确定性建表排序。

## 执行流程

典型初始化流程如下：调用者先以 `NewRestoreClient` 保存 PD 能力，再通过 `InitConnections` 注入 Domain 和主 SQL session；外部初始化 split/import client 后，`LoadSchemaIfNeededAndInitClient` 保存 `BackupMeta`，按 `needLoadSchemas` 决定是否接收 database/DDL 元数据，拉取 store 列表并进入 `initClients`。`initClients` 选择 `KvMode::{Raw, Txn, TiDBFull}`，注册 Raw range、multi-ingest、peer-download-retry 和可选限速回调，最后构造 `SnapFileImporter` 并计算全量集群恢复标志。

schema 恢复时，`AllocTableIDs` 先完整获得或复用 `PreallocIDs`，再注册到主 session 与池中 session。`CreateDatabases` 可使用单 session 或按轮转分配给 scoped threads；`CreateTables` 先为增量恢复生成 rebase 集合，再按 `batchDdlSize` 和 session pool 决定批量或逐表创建。每张表创建后，`buildCreatedTables` 从 Domain 读取实际下游 `TableInfo`，检查 clustered-index (`IsCommonHandle`) 一致性，并以实际新表 ID 生成带 `new_ts` 的 rewrite rule。批量路径还调用 `setMergeOptionForTables` 更新表和分区的 merge option。

checkpoint 新建路径保存 cluster ID、恢复起止信息、命令 hash、预分配 ID、UUID 和 scheduler 配置，再启动 runner；恢复路径先逐项校验这些身份信息，加载已完成 range 与 checksum 后启动 runner。checksum 验证先由备份文件累积期望值；无 checkpoint 命中时调用 `ChecksumClient` 扫描并通过 runner 持久化，最终对 CRC64、KV 数和字节数三项严格比较。

RawKV 路径由 `GetFilesInRawRange` 验证请求 CF 和半开区间 `[start, end)` 必须被某个备份 `RawRange` 完整覆盖，再返回与请求区间相交且 CF 相同的文件；非 RawKV、部分覆盖和无覆盖分别返回带稳定 BR 错误码的错误。

## 数据与状态

`SnapClient` 是有状态对象，初始化存在先后约束：`backupMeta` 决定 raw/txn/full、增量和 restore TS；`dom` 决定 API version、目标 schema 查询与空集群检查；`db`/`dbPool` 执行 DDL；`meta_client`/`import_client` 是构造 importer 的前置条件。缺少这些依赖的方法会返回明确的 “not initialized” 错误，而不是隐式创建生产连接。

`databases` 和 `ddlJobs` 仅在 `needLoadSchemas` 为真时装载。`rebasedTablesMap` 每次 `generateRebasedTables` 都先清空，只有增量备份才记录所有待恢复表。`temporarySystemTablesRenamed` 由复用预分配 ID 时的 Domain 对照触发，随后 `CleanTablesIfTemporarySystemTablesRenamed` 排除已经处理的临时系统表。

`checkpointChecksum` 以目标表 ID 为键；`restoreUUID` 在 checkpoint 恢复时复用，首次恢复或 PiTR 安装时生成。`speed_limit_closed` 是跨回调共享的 `Arc<Mutex<bool>>`，防止重复 close 再次清零。`workerPoolSize` 当前按 `storeCount * 7186` 计算，与 `concurrencyPerStore` 独立；这一事实由 `test_download_worker_pool_scales_with_stores_independently_of_import_concurrency` 锁定，修改时不可把两者混为同一配置。

## 依赖与调用关系

向下依赖主要来自同 crate：`SnapFileImporter`、`KvMode`、`RewriteMode` 和 importer options 位于 `import.rs`；`PiTRCollDep/newPiTRColl` 位于 `pitr_collector.rs`；临时系统表判定位于 `systable_restore.rs`；`stubs.rs` 提供 `PdClient`、`StoreMeta`、`DomainLike`、`DbSession`、`CheckpointRunner`、`ChecksumClient`、`SstRestorer` 等抽象及迁移期模型。Cargo 直接依赖还包括 restore/utils/errors crate，以及用于 placement policy 解码的 `serde_json`。

RustCodeGraph 显示 Rust 侧明确调用边包括：`LoadSchemaIfNeededAndInitClient -> initClients`、`initClients -> SetSpeedLimitCallbacks`、`CreateTables -> createTablesBatch/createTablesSingle`、`InitCheckpoint -> CreatePreallocIDCheckpoint`，以及 `tikv_sender.rs` 的 `RestoreSSTFiles -> SnapClient::GetRestorer`。`pipeline_items.rs`、`tikv_sender.rs`、`systable_restore.rs` 通过对 `SnapClient` 的扩展 `impl` 共享本文件状态；测试侧 `export_test.rs` 串联 `AllocTableIDs -> CreateTables` 并装配限速回调。

Go 生产上游由 `br/pkg/task/restore.go::runSnapshotRestore` 创建并配置 `SnapClient`，随后调用 checkpoint、连接初始化、schema/DDL、policy 和恢复相关 API。RustCodeGraph 对 Rust `br/pkg/task/restore.rs` 只确认了部分配置桥接，未证明与 Go `runSnapshotRestore` 等价的完整生产主链已经接通；因此当前文档只把 Go 链路作为语义对照，不宣称 Rust 已完成端到端生产接线。

## 错误处理与边界

- 依赖未初始化：`AllocTableIDs`、`GetPreAllocedTableIDRange`、`InitCheckpoint`、`LoadSchemaIfNeededAndInitClient`、建库建表等均在使用前检查对应 `Option`，返回 `Error`。
- checkpoint 恢复：cluster ID、命令 hash、`EndVersion` 和 `log_restored_ts` 任一不一致都拒绝续跑，避免把旧进度套到另一恢复任务。
- 建表：只有 `BR:Restore:ErrUnsupportedBatchDDL` 或等价消息允许由 batch 回退 single；worker panic 被转换为错误。下游 clustered-index 模式不一致返回 `ErrRestoreModeMismatch`。
- RawKV：区间采用半开语义；空 end 表示无穷上界。文件过滤保留 end 等于请求 start 的 Go 对齐行为，测试明确覆盖边界。非 Raw 模式返回 `ErrRestoreModeMismatch`，覆盖不足返回 `ErrRestoreRangeMismatch`。
- placement policy：缺少或格式错误的 name 会失败；`SetPlacementPolicyMode` 对未知值回落到 `STRICT`。
- checksum：没有文件 checksum 时直接跳过；否则 CRC64、KV 数、字节数任一不符均返回 `ErrRestoreChecksumMismatch`。
- 限速：获取 store 或任一 RPC 失败时停止后续分配并返回首个错误；close 清零最多重试三次。锁中毒处使用 `unwrap`，thread panic 会在建表路径显式转为错误，但限速刷新线程的 join 错误被忽略，这是当前实现边界。

## 并发与资源生命周期

建库、批量建表和逐表建表使用 `std::thread::scope`，session 按 worker 独占分配，不跨线程共享可变 `DbSession`；主线程 join 全部 worker，并将 panic 转为 `Error`。批量任务先按 schema ID/table ID 排序，再分批轮转给 worker；返回集合的拼接顺序取决于 worker assignment，不应被调用者当作原输入顺序。

限速下发使用原子索引在最多 `min(concurrency, stores.len())` 个 scoped worker 间分配 store，并用 `AtomicBool` 首错停止。importer create callback 首次执行时启动一个刷新线程，每 180 秒重发带 TTL 的限速；close callback 通过 `Condvar` 唤醒线程、join 后将限速清零，并以共享标志保证成功关闭幂等。若清零三次均失败，标志不会置位，后续 close 仍可重试。

`InstallPiTRSupport` 将 before-ingest 和 close callback 注册进 importer，使 collector 生命周期附着于导入器；增量恢复遇到已启用的 log backup 时先关闭 collector 再报错。`Close` 依次 take 并关闭 importer/restorer，关闭主 session 和池中 session 后清空所有权；checkpoint runner 不由 `Close` 隐式等待，调用者应显式调用 `WaitForFinishCheckpoint`。

## 与 Go 版本的对应关系

Rust 文件与 `client.go` 的主要常量、`SnapClient` 字段组和方法名保持高度对应，核心语义包括：恢复模式选择、schema 是否加载、预分配 ID、checkpoint 身份校验、RawKV range 过滤、batch DDL 回退、全量/增量判定、限速回调和 checksum 校验。`client_test.rs` 前半覆盖 Go `client_test.go` 的建表、fresh cluster、重复建库、全量恢复、限速、排序和 ID 分配用例，后半增加 Rust 适配层的默认值、初始化、checkpoint、PiTR 和 RawKV 边界验证。

仍需注意实现载体差异：Go 使用真实 TiDB Domain、PD/TiKV/grpc 客户端、errgroup/worker pool 和 failpoint；Rust Cargo 注释明确当前通过本地 trait/stub 解耦这些依赖。Go 的生产入口 `runSnapshotRestore` 已由图查询确认调用广泛的 `SnapClient` API，而 Rust 完整入口接线未获得同等证据。Rust 使用 `HashMap` 代替部分 Go `sync.Map`，使用 scoped OS threads 代替 Go worker pool/errgroup，并把错误码/消息封装在本地 `Error`；扩展时应比较可观察语义，而非机械照搬并发原语。

## 扩展指南

- 新增恢复配置时，优先在 `SnapClient` 增加状态与成对 setter/getter，并同步 `NewRestoreClient` 的 Go 零值和 `NewRestoreClientForTest` 的可运行测试默认值；若影响 CLI 主链，还需核验 `br/pkg/task/restore.rs` 与 Go `restore.go::configureRestoreClient`。
- 新增 importer 能力探测或生命周期动作时，接入 `initClients` 的 create/close callback，而非绕过 `SnapFileImporter`；必须补充关闭幂等、失败传播和 callback 顺序测试。
- 修改 schema 恢复应保持 `AllocTableIDs -> CreateDatabases/CreateTables -> buildCreatedTables` 的不变量：只有完整 ID 映射才注册，真实下游 ID 必须来自 Domain，clustered-index 模式必须一致。相关测试放在独立的 `client_test.rs` 或已有相邻 `*_test.rs`，不要内嵌进源文件。
- 修改 checkpoint 字段时需同时维护首次保存、恢复校验、测试 manager 和 Go 对照；不能仅反序列化而不做任务身份校验。
- 修改 RawKV 区间逻辑时明确半开区间和空 end 语义，并扩展 `test_get_files_in_raw_range_matches_go_coverage_and_boundaries`。
- 修改并发或限速时评估 store 数、`concurrencyPerStore`、`workerPoolSize`、TTL 刷新线程与 close 重试的独立作用，避免泄漏线程或使关闭永久阻塞。
- 当前 crate 依赖本地 stubs；若接入真实外部 Rust 客户端，必须遵守仓库的独立上游移植/tag 依赖规则，不能在本 crate 内复制或本地 patch 依赖。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本文件；`files --filter br/pkg/restore/snap_client/client.rs` 确认目标含 109 个符号。`explore` 查询了 `SnapClient`、`NewRestoreClient`、`InitConnections`、`GetFilesInRawRange` 及 Go/Rust 恢复主链；`node --file ... --offset/--limit` 分段核对了全部 1635 行实现。调用图证据包括 `LoadSchemaIfNeededAndInitClient -> initClients`、`CreateTables -> createTablesBatch/createTablesSingle`、Go `runSnapshotRestore -> NewRestoreClient/configureRestoreClient` 和 `RestoreSSTFiles -> GetRestorer`。
- 源与模块边界：`br/pkg/restore/snap_client/client.rs`、`lib.rs`、`Cargo.toml`；相邻直接依赖 `import.rs`、`pitr_collector.rs`、`systable_restore.rs`、`tikv_sender.rs`、`pipeline_items.rs`、`stubs.rs`。
- Go 对照：`br/pkg/restore/snap_client/client.go`；生产调用证据来自 `br/pkg/task/restore.go` 的 `runSnapshotRestore`、`configureRestoreClient`、`createDBsAndTables`。
- 测试证据：`br/pkg/restore/snap_client/client_test.rs`、`client_test.go`、`export_test.rs`；重点核对建表与回退、fresh cluster、限速成功/失败/幂等关闭、ID 预分配、默认配置、模式初始化、checkpoint/checksum、PiTR 生命周期、RawKV 覆盖边界和 worker pool 计算。
- 本任务为纯文档分析，按计划不运行 Cargo；只执行固定章节结构检查并人工复核路径、符号和“当前 Rust 接线/Go 对照”措辞。
