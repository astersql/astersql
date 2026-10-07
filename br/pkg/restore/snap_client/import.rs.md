# `br/pkg/restore/snap_client/import.rs`

## 文件定位

本文件是 `astersql-br-pkg-restore-snap-client` crate 的快照 SST 导入实现。crate 入口 `br/pkg/restore/snap_client/lib.rs` 以 `pub mod import` 装载它并通过 `pub use import::*` 暴露其公开 API；`br/pkg/restore/snap_client/Cargo.toml` 表明该 crate 是 Go 包 `br/pkg/restore/snap_client` 的 Rust 移植单元，依赖恢复公共边界 `astersql-br-pkg-restore`、BR 工具和错误 crate，而 PD、TiKV、kvproto/gRPC 能力目前通过本 crate 的 `stubs` trait/数据结构抽象。

上游装配点是 `br/pkg/restore/snap_client/client.rs::SnapClient::initClients`：它根据备份类型选择 `KvMode`，准备 Raw 范围、MultiIngest 和 peer-retry 能力探测回调，再调用 `NewSnapFileImporter`。文件尾部的 `SnapshotFileImporter` 把具体实现适配为 `astersql_br_pkg_restore::{FileImporter, BalancedFileImporter}`；因此 `br/pkg/restore/restorer.rs` 中的 `SimpleRestorer`、`BatchRestorer` 和 `MultiTablesRestorer` 可以经统一 trait 调用 `Import`、`Close` 与 `PauseForBackpressure`。

## 核心职责

- 定义导入模式 `KvMode` 和键改写模式 `RewriteMode`，决定扫描范围如何编码，以及构造 SST 元数据时如何处理 Region 边界和 rewrite prefix。
- 由 `SnapFileImporterOptions` 构造并初始化 `SnapFileImporter`，在真正导入前执行能力探测、Raw 范围设置、限速等创建回调。
- 对一批 `BackupFileSet` 计算覆盖键范围，经 `SplitClient::PaginateScanRegion` 找到 Region，然后为各 Region peer 下载 SST，并向 Region leader 发起 `MultiIngest`。
- 用下载令牌池、ingest 令牌池和独立 PD 请求令牌池限制并发；向 `BalancedFileImporter` 提供提交侧背压。
- 探测 batch download、latest-MVCC download、peer download retry 和 multi-ingest 能力；对下载 RPC 进行分类退避和取消感知的重试。
- 在导入前后运行扩展回调，成功后更新 KV/字节 summary，并在关闭时运行清理回调及关闭导入客户端。

## 主要符号

- `KvMode::{TiDBFull, Raw, Txn, TiDBCompacted}`：数值与 Go 常量保持一致。`getKeyRangeByMode` 中 Raw 使用原始键，Txn 对非空边界做 memcomparable 编码，其余模式调用 `GetRewriteRawKeys`。
- `RewriteMode::{RewriteModeLegacy, RewriteModeKeyspace}`：Legacy 会在下载请求中编码 rewrite prefix；Keyspace 模式在 `getSSTMetaFromFile` 中先解码 Region 边界。
- `gRPCTimeOut` 与 `DownloadRateLimitTTLSeconds`：下载单次请求超时为 200 分钟，限速设置 TTL 为 3600 秒；`SetDownloadSpeedLimit` 将 task ID、速率和 TTL 下发给指定 Store。
- `storeTokenChannelMap`：以 `Mutex<HashMap<store_id, TokenCh>>` 保存每 Store 令牌池。`acquireTokenCh` 惰性补建未知 Store 的池；`ShouldBlock` 仅在至少存在一个池且所有池都无可用令牌时返回真。
- `SnapFileImporterOptions` / `NewSnapFileImporterOptions`：汇集 cipher、后端、Split/Importer client、Store 列表、扫描与每 Store 并发、能力开关及创建/关闭回调。options 构造函数本身不校验参数。
- `SnapFileImporter` / `NewSnapFileImporter`：持有任务 ID、storage cache key、模式、客户端、三类限流状态、回调及 Raw 范围。构造器拒绝 `concurrencyPerStore == 0`，创建令牌池，依序执行 `createCallbacks`，任一失败即终止构造。
- `CheckBatchDownloadSupport`、`CheckBatchDownloadLatestMVCCSupport`、`CheckPeerDownloadRetrySupport`、`CheckMultiIngestSupport`：只把 `StoreState::Up` 的 ID 交给客户端。普通 batch 探测设置 `mergeSst`；latest-MVCC 严格探测成功后开启 peer retry；独立 peer-retry 探测失败则记录警告并回退 legacy retry，而不是令初始化失败。
- `Import`：具体导入入口；执行 before-ingest 回调、计算范围、扫描 Region、逐 Region `download`/`ingest`、执行延迟回调、最后累计 summary。
- `buildDownloadRequest`：为与 Region 相交且能匹配 rewrite rule 的文件构造 `DownloadRequest` 和稳定的 `SSTMeta`；匹配规则采用最长 `OldKeyPrefix`。
- `downloadWithOptionalPeerRetry`：选择普通或 peer-aware backoff；每次尝试创建带 200 分钟超时的子 context，按 BR 错误码或 RPC 状态分类错误，并在退避等待期间轮询父 context 取消。
- `getSSTMetaFromFile` / `GetSSTMetaFromFile`：生成新 UUID、识别 default/write CF、求 rewrite prefix 范围与 Region 范围的交集，并填入长度、Region epoch 和 cipher IV；后者是公开兼容包装。
- `SnapshotFileImporter`：用 `Mutex<SnapFileImporter>` 满足共享 trait 的 `&self` 接口；`snapshot_context` 将公共恢复 context 的取消/错误桥接到 snap-client context，并转换两边的 `BackupFileSet`/rewrite rule/error 类型。

## 执行流程

1. `SnapClient::initClients` 取得 TiKV Store，按 Raw/Txn/TiDBFull 选择模式，并把 `SetRawRange`、`CheckMultiIngestSupport`、`CheckPeerDownloadRetrySupport` 及可选限速设置包装成创建回调。
2. `NewSnapFileImporter` 校验每 Store 并发非零，为已知 Store 创建 download/ingest 令牌池；`scanConcurrency > 0` 时再创建 PD 请求池。随后按顺序执行创建回调，因此 importer 对外可见前已经完成必要配置/探测。
3. 上层 restorer 经 `SnapshotFileImporter::Import` 进入具体实现；适配器先复制公共类型，锁住 importer，再调用 `SnapFileImporter::Import`。
4. `Import` 先执行所有 `beforeIngestCallbacks`，收集它们返回的延迟回调。任一前置回调失败会带序号返回，后续扫描和导入不会开始。
5. `getKeyRangeForFiles` 遍历所有 SST，按模式计算每个范围并取全局最小 start、最大 end；`paginateScanRegion` 在调用 `SplitClient` 前后申请/释放独立 PD 令牌。
6. 对每个扫描结果，`download` 遍历文件组和文件。`buildDownloadRequest` 跳过无匹配规则或不与 Region 相交的文件；开启 `retainLatestMVCCVersion` 时，当前 Region 中没有 write CF 的整个文件组会被跳过。
7. 每个请求对 Region 的每个 peer 获取对应 Store 的 download token。RPC 根据开关选择 `BatchDownloadLatestMVCC`、`BatchDownloadSST` 或 `DownloadSST`，并经 `downloadWithOptionalPeerRetry` 重试；无论 RPC 成败都会先释放 token，再传播错误。成功请求的 meta 被加入本 Region 的待 ingest 列表。
8. `ingest` 对空 meta 直接成功；否则要求 Region leader 存在，获取 leader Store 的 ingest token，构造带 Region ID/epoch/peer 的 `MultiIngestRequest`。RPC 后释放 token，传输错误或响应内错误都会终止本次导入。
9. 所有 Region 完成后才按顺序运行延迟回调；全部成功后按每个源文件的 `TotalKvs`、`TotalBytes` 更新 summary。checkpoint 和用户进度不在本文件写入，而由 `br/pkg/restore/restorer.rs` 在 `FileImporter::Import` 成功返回后处理。

## 数据与状态

`SnapFileImporter` 的长期配置包括 `cipher`、`apiVersion`、`backend`、`kvMode`、`rewriteMode`、`concurrencyPerStore` 和能力开关。`taskId` 标识限速任务；`cacheKey` 填入下载请求的 `StorageCacheId`。两者在构造时由随机 UUID 的尾部生成，并在 importer 生命周期内保持稳定。

可变状态包括 Raw 边界、`mergeSst`、`retainLatestMVCCVersion`、`peerDownloadRetry`、回调列表以及令牌池。能力探测会改变三个布尔开关中的相应状态。当前 `rawStartKey`/`rawEndKey` 由 `SetRawRange` 保存，但本文件当前的 `getKeyRangeForFiles` 仍从文件自身范围求边界；不要据此宣称 Raw 范围字段已经参与扫描裁剪。

下载请求的 `SSTMeta::Uuid` 在 `buildDownloadRequest` 时生成，随后同一个 request 被重试闭包重复使用，因此同一次逻辑下载的不同 RPC 尝试保持 UUID 不变；`import_test.rs::test_download_retry_preserves_uuid_for_all_rpc_modes` 固化了该契约。`getSSTMetaFromFile` 的范围是 `[max(new-prefix, region-start), min(new-prefix + 10*0xff, region-end)]`，并在 start 大于 end 时 panic，表示调用者违反了已完成相交过滤的不变量。

## 依赖与调用关系

上游主链为 `SnapClient::initClients` → `NewSnapFileImporter` → `SnapshotFileImporter` → `restorer::{SimpleRestorer, BatchRestorer, MultiTablesRestorer}` → `FileImporter::Import`。其中 `MultiTablesRestorer::GoRestore` 在投递 worker 前调用 `BalancedFileImporter::PauseForBackpressure`，并仅在 Import 成功后写 checkpoint 和报告进度。

下游依赖集中在 `crate::stubs`：`SplitClient` 提供 Region 分页扫描，`ImporterClient` 提供能力探测、限速、三种下载 RPC、`MultiIngest` 和 `CloseGrpcClient`；`backuppb`、`import_sstpb`、`metapb` 表达文件、请求与 Region/Store 元数据；`codec` 完成键编码。`astersql-br-pkg-utils::backoff` 提供普通与 peer-aware 下载退避，`astersql-br-pkg-errors` 提供可分类的 BR 错误，`astersql-errors` 将本地错误转换成退避策略能识别的 error chain。

RustCodeGraph 显示生产源文件本身目前只被 `import_test.rs` 直接作为文件依赖记录，但这不等于没有生产接线：`lib.rs` 重导出其符号，`client.rs` 构造 importer，`SnapshotFileImporter` 通过跨 crate trait 被 `restorer.rs` 间接调用。分析调用关系时必须同时考虑模块重导出和 trait 动态分发。

## 错误处理与边界

- 构造阶段明确拒绝零 `concurrencyPerStore`；创建回调的错误直接返回，但已经执行过的回调不会在构造失败路径由本文件自动回滚。
- `SetRawRange` 只允许 `KvMode::Raw`，其他模式返回带 `ErrRestoreModeMismatch` 的注解错误。空文件组没有显式拒绝：范围将保持空值并交给 Region 扫描端处理。
- PD 请求 token 在 `PaginateScanRegion` 返回错误时仍会释放；download/ingest token 也在 RPC 结果被 `?` 传播前释放，避免错误路径永久耗尽池。
- 下载前检查 context；下载重试循环和退避等待也检查取消。`snapshot_context` 让公共 restorer 的取消可中断具体 importer。测试 `test_parent_cancellation_interrupts_peer_backoff_without_another_rpc` 与 `test_probe_error_falls_back_and_real_context_cancel_stops` 覆盖此行为。
- 下载 RPC transport 错误按已知 BR code、context canceled、带 code 的未知 RPC 状态或普通错误分类；尝试耗尽时把历次消息用分号聚合。响应中的 `Error`、ingest 响应中的 `Error`、缺失 leader 都转换为本地 `Error`。
- `CheckPeerDownloadRetrySupport` 的探测错误是可降级条件；`CheckBatchDownloadLatestMVCCSupport` 的错误则保持严格并阻止启用对应路径。离线 Store 不参与能力探测。
- `Close` 会运行全部关闭回调，忽略并记录回调错误，仍以 `CloseGrpcClient` 的结果作为返回值；回调临时取出后又放回，因此重复 Close 会重复执行它们，`test_close_ignores_callback_errors_and_repeats_callbacks` 对此有断言。
- `Mutex::lock().unwrap()`、`Condvar::wait(...).unwrap()` 会在锁中毒时 panic；`SnapshotFileImporter` 也串行化所有 Import/Close/背压调用。这些是当前实现边界，不是可恢复错误路径。

## 并发与资源生命周期

download 和 ingest 各有一套每 Store 令牌池，容量均为 `concurrencyPerStore`。预建列表之外的 Store 会在第一次获取时惰性创建同容量池。`releaseToken` 同时归还 token 并唤醒 `PauseForBackpressure` 所等待的条件变量。背压判断是全局的：download 或 ingest 任一类池中“所有已建 Store 都没有空闲 token”即阻塞上游投递。

PD 扫描使用 `pdReqTokens`，其容量来自 `scanConcurrency`；值为零时不启用限制。该池与 Store 下载/ingest 池相互独立，避免 Region 元数据请求与数据面 RPC 争用同一计数器。

当前 Rust `SnapFileImporter::Import` 在持有适配器互斥锁时按 Region、文件组、文件和 peer 顺序执行，没有像 Go 版本那样创建 Region worker pool，也没有在单个 Region 内按文件组/peer 并行发起 batch 请求。因此令牌池与背压接口已存在并可保护来自多个上层任务的资源，但 `SnapshotFileImporter` 的外层互斥锁会使通过该适配器进入的导入串行。`import_test.rs` 中以“parallelizes”命名的用例当前主要验证请求数量和汇总后的 ingest metas，不足以证明真实并行度或性能。

构造回调属于初始化阶段；before-ingest 回调返回的延迟清理仅在所有 Region 成功后执行，当前没有 scope guard 保证中途错误时执行。关闭回调会在每次 `Close` 被调用时重复运行，最后关闭 gRPC client。调用者应在所有 restorer worker `WaitUntilFinish` 后再 Close，避免关闭仍在使用的客户端。

## 与 Go 版本的对应关系

Rust 的枚举值、令牌池/背压、options/构造器、限速 TTL、能力探测、模式键范围、SST meta 范围、Import 前后回调、summary、下载重试 UUID 稳定性及 FileImporter 适配方向与 `br/pkg/restore/snap_client/import.go` 对齐。`br/pkg/restore/snap_client/import_test.rs` 也对应 Go `import_test.go` 的键范围、meta、非法并发、Raw/TiDB 导入、PD 扫描流控、batch/latest-MVCC 和 retry 场景。

但当前 Rust 是较窄的本地 trait/stub 移植，不能视为 Go 文件的完整等价实现。直接核对 Go 源码可见以下重要差距：

- Go `Import` 用 ImportSST backoff 重试完整的 scan/download/ingest，并按能力选择 Region worker pool；Rust只在单个下载 RPC 层重试，Region 顺序处理。
- Go `download` 对 Raw/Txn、TiDBCompacted latest-MVCC、TiDBCompacted merge 和普通 TiDB 分派到四套实现；Rust统一 `buildDownloadRequest` 后仅按两个能力布尔值选择 RPC，没有 Go `downloadRawKVSST` 的 Raw API 细节。
- Go batch download 会按 CF/文件组聚合 `Ssts`、验证 rewrite rule 一致性、根据响应 range 重写结果 meta、处理空 range，并行向 peer 请求；Rust当前逐文件请求并直接保留请求前 meta。
- Go `buildDownloadRequest` 还设置时间范围过滤、Keyspace request type 和 resource/request-source context；Rust request 没有这些接线。
- Go 对解密 SST 错误会去掉 cipher 重试；Rust没有 `isDecryptSstErr` 降级。
- Go ingest 处理 NotLeader、EpochNotMatch、KeyNotInRegion 等 Region error 并可更新 leader；Rust只做一次 `MultiIngest`，把响应消息包装为普通错误。

这些差距是扩展和后续移植的风险清单，不应通过改变文档措辞掩盖，也不属于本次纯文档任务要修复的范围。

## 扩展指南

- 增加新的下载模式或能力开关时，优先修改 `KvMode`/`SnapFileImporter` 状态、`SnapClient::initClients` 的探测接线和 `download` 分派；同时在独立的 `br/pkg/restore/snap_client/import_test.rs` 增加模式、探测失败和 RPC 选择回归，不要把测试写入生产文件。
- 补齐 Go batch 语义时，应围绕 `buildDownloadRequest` 和 `download` 拆出清晰的 Raw、单文件、merge、latest-MVCC helper；保持 rewrite-rule 一致性验证、响应 range、空 range 和同 UUID retry 契约，并评估请求聚合的内存与并发上限。
- 增加 Region/ingest 重试时，应复用可分类 BR error 和父 context，明确 NotLeader 更新、epoch 变化是否需要重新下载；测试需覆盖 token 在每个错误/取消分支都被释放以及 checkpoint 只在最终成功后写入。
- 调整令牌/背压时必须分别审视 PD、download、ingest 三类池；避免持锁执行 RPC，归还 token 后必须通知 condvar。若要让适配器真正并行，需重新设计 `SnapshotFileImporter(Mutex<...>)` 与内部可变字段，而不是绕过锁。
- 新增回调时要定义失败时的补偿语义。当前延迟回调不是失败路径 cleanup；如果改为资源 guard，需与 Go 的执行顺序对齐并增加“前置成功、导入失败、延迟回调是否运行”的测试。
- 修改 `getSSTMetaFromFile` 或键模式时，要同步检查空结束键、Keyspace Region 解码、CF 名推断、prefix 上界和 start/end 不变量；对应 Rust 测试应继续放在 `import_test.rs`，并与 Go `import_test.go` 的断言逐项比较。
- 若把本 crate 从 stubs 接到真实 TiKV/PD 客户端，需要同步审视 `Cargo.toml` 的依赖策略、request context 字段和实际异步运行时；当前测试只验证内存桩控制流，不能替代真实集群兼容与性能验证。

## 验证依据

- RustCodeGraph：`status` 确认索引可用；`files --filter br/pkg/restore/snap_client` 确认目标源、独立测试和相邻模块；`node --file br/pkg/restore/snap_client/import.rs --offset 1/500/970` 阅读全部 996 行；`explore "SnapFileImporter Import paginateScanRegion downloadSST ingestSST ..."` 核对构造、Import、能力探测、客户端调用及测试调用边。
- 生产 Rust：`br/pkg/restore/snap_client/import.rs`（本文件全部符号）、`br/pkg/restore/snap_client/lib.rs`（模块装载/重导出）、`br/pkg/restore/snap_client/client.rs::initClients`（构造与能力探测接线）、`br/pkg/restore/restorer.rs::{FileImporter,BalancedFileImporter,SimpleRestorer,BatchRestorer,MultiTablesRestorer}`（上游调用、背压、checkpoint 与进度边界）。
- crate 边界：`br/pkg/restore/snap_client/Cargo.toml` 的 package metadata、path dependencies 和“local traits/stubs only”说明。
- Go 对照：`br/pkg/restore/snap_client/import.go` 的 `getKeyRangeByMode`、`Import`、`getSSTMetaFromFile`、`download`、`downloadWithOptionalPeerRetry`、`buildDownloadRequest`、batch/latest/raw/single download、`ingest`/`ingestSSTs`；`br/pkg/restore/snap_client/import_test.go` 的对应测试清单。
- Rust 测试：`br/pkg/restore/snap_client/import_test.rs` 中键范围/meta、零并发、Close、离线 Store、Raw/TiDB 导入、PD 流控、batch/latest-MVCC、同 UUID retry、探测降级、适配器和父 context 取消用例。
- 本任务是只读分析加 Markdown 产物，按计划不运行 Cargo；结构验证要求目标文件存在且恰有十一个固定二级标题。
