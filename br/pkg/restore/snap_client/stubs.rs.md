# `br/pkg/restore/snap_client/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-restore-snap-client` crate 的本地兼容与依赖注入层，由 [`lib.rs`](lib.rs) 以 `pub mod stubs` 装配，但没有通过 crate 根的 `pub use` 扁平重导出。其存在目的由文件级注释和 [`Cargo.toml`](Cargo.toml) 共同限定：在 arm64 Darwin 等不接入 `kvproto`、`grpcio`、真实 `kv/domain` 的构建中，为快照恢复代码提供可编译、可测试的 PD、TiKV、存储、元数据和数据库边界。

该文件不是 Go 包中某个同名文件的一对一翻译。它把 Go 版本分散在 `br/pkg/restore/utils`、`br/pkg/restore/internal/import_client`、`br/pkg/restore/internal/prealloc_table_id`、`br/pkg/restore/split`、`br/pkg/conn` 以及 protobuf/model 包中的类型与接口压缩到一个 2298 行的 Rust 模块中。生产模块 `client.rs`、`import.rs`、`tikv_sender.rs`、`placement_rule_manager.rs`、`pipeline_items.rs`、`pitr_collector.rs`、`systable_restore.rs` 和 `systable_schema_update.rs` 都直接引用它，因此“stub”表示外部系统被替换，并不表示文件里的所有算法都无行为。

## 核心职责

1. 提供统一的 `Error`/`Result<T>`、`Context` 与 `berrors` 错误码，使上层能够表达错误注解、取消和超时，而不依赖 Go errors、gRPC status 或 Tokio runtime。
2. 定义 `backuppb`、`import_sstpb`、`metapb`、`model`、`metautil` 的最小数据子集，并实现 Go 风格 `Get*` 访问器，支撑恢复主链的机械对照。
3. 用 trait 描述真实边界：`DbSession`、`DomainLike`、`PdClient`、`StoreMeta`、`SplitClient`、`ImporterClient`、`ExternalStorage`、`Copier`、`SstRestorer`、checkpoint 与 checksum 接口；再由 `Mem*` 类型提供确定性的内存实现。
4. 保留若干会直接影响恢复结果的算法：memcomparable 字节编解码、表键编码、原始键重写、SST 范围归组/合并、分区 ID 对应、表 ID 预分配与 checkpoint 复用、TiFlash store 过滤和令牌队列。
5. 为测试保存可观察状态，例如 `MemDb.sqls`、`MemStatsHandler.saved`、`MemImporterClient.downloads/ingests/speed_limits`、`MemSplitClient.rules/regions` 与 `MemStorage.files`。

## 主要符号

- 基础语义：`Error::{new,with_code,Annotate,Wrap}` 保留消息和可选静态错误码；`Context::{Background,cancel,Err,WithCancellationSource,WithTimeout,Done}` 组合本地取消、父取消源与截止时间。`log::{Info,Warn,Error,Debug}` 是空操作，只有 `log::Panic` 会真正 panic。
- 编码和命名：`codec::{EncodeBytes,DecodeBytes}` 使用 8 字节分组、零填充和 marker；`tablecodec::EncodeTablePrefix` 对有符号表 ID 做符号位翻转后大端编码，`IsRecordKey` 检查 `t<id>_r`；`TemporaryDBName`、`StripTempDBPrefix*`、`IsSysOrTempSysDB` 和 `EncloseDBAndTable` 对齐系统表恢复的名称规则。
- 协议/模型子集：`backuppb::{File,BackupMeta,IngestedSSTs}`，`import_sstpb::{RewriteRule,SSTMeta,DownloadRequest,MultiIngestRequest}`，`metapb::{Store,Peer,Region}`，`model::{CIStr,TableInfo,DBInfo,Job}`，以及 `metautil::{Table,Database}`。这些是手写结构，不是完整 protobuf 或 TiDB model。
- 恢复聚合类型：`RewriteRules`、`CreatedTable`、`BackupFileSet`/`BatchBackupFileSet`、`RegionInfo`、`RangeStats`、`MergeRangesStat`、`CheckpointMetadata`、`ChecksumItem`、`PlacementRule` 和 `LabelConstraint`。
- 关键算法：`GetRewriteRawKeys`、`MergeAndRewriteFileRanges`、`GetPartitionIDMap`、`CollectTableIDs`、`ComputeSortedIDsHash`、`NewAndPreallocTableIDs`、`ReusePreallocatedTableIDs`、`GetAllTiKVStoresWithRetry`。
- 注入接口和内存实现：`StatsHandler`/`MemStatsHandler`、`DbSession`/`MemDb`、`DomainLike`/`MemDomain`、`PdClient`+`StoreMeta`/`MemPdClient`、`SplitClient`/`MemSplitClient`、`ImporterClient`/`MemImporterClient`、`ExternalStorage`+`Copier`/`MemStorage`、`SstRestorer`/`SimpleRestorer`。
- 并发辅助：`BuildWorkerTokenChannel`、`TokenCh`、`acquire_token`、`try_acquire_token`、`release_token`，以及原子递增的 `new_uuid_bytes`。

## 执行流程

在 SST 恢复路径中，`tikv_sender.rs::RestoreSSTFiles` 把备份文件和 `RewriteRules` 传给 `MergeAndRewriteFileRanges`。后者按原始 `(StartKey, EndKey)` 归组，累计 CF 文件数、字节数和 KV 数，调用 `GetRewriteRawKeys` 生成排序/切分所用的编码键，按起止键排序后，在 `split_size_bytes` 与 `split_key_count` 两个阈值均允许时合并相邻项，并返回 `RangeStats` 与统计值。随后 sender 使用 `GetAllTiKVStoresWithRetry` 获取并过滤 store，交给导入侧继续下载与 ingest。

`import.rs` 用 `BuildWorkerTokenChannel` 为 store 或扫描并发建立固定数量的 permit；工作线程用 `acquire_token`/`release_token` 控制并发。`ImporterClient` 把下载、批量下载、最新 MVCC 下载、multi-ingest、限速及强制分区范围暴露给上层。其默认 `BatchDownloadSST`/`BatchDownloadLatestMVCC` 逐级委托，而 `IsBatchDownloadLatestMVCCSupported` 对每个 store 发空请求：错误码为 `Unimplemented` 时返回 `false`，其他错误附加 store ID 后传播。

在 schema 恢复路径中，`client.rs::InitBackupMeta` 根据是否存在 checkpoint，在 `ReusePreallocatedTableIDs` 与 `NewAndPreallocTableIDs` 之间选择。新分配先由 `CollectTableIDs` 收集表和分区 ID、排序并计算 SHA-256，再读取 `DbSession::GetGlobalID`，保留仍落在安全区间内的 ID，把其余 ID 顺序映射到 `ReusableBorder` 之后，最后通过 `AdvanceGlobalIDs` 一次推进全局 ID。复用路径必须同时通过最大 ID 边界和哈希一致性检查。

放置规则与系统表路径分别通过 `SplitClient`、`DomainLike` 和 `DbSession` 操作；测试时 `MemSplitClient` 保存 rule/region，`MemDomain` 查询表映射，`MemDb` 记录 SQL/DDL。PiTR collector 则通过 `ExternalStorage`/`Copier` 读写或复制对象，`MemStorage` 以互斥保护的内存 map 模拟这一过程。

## 数据与状态

大部分协议结构是 `Clone + Default` 的拥有型值，字节字段使用 `Vec<u8>`，可选 protobuf message 使用 `Option<T>`。`backuppb::File::GetSize_` 在 `Size_ == 0` 时回退到 `TotalBytes`；`import_sstpb::SSTMeta::GetRange` 对缺失 range 返回默认空范围。这些便利默认值只服务当前 Rust 调用面，不能推导真实协议端一定会提供合法数据。

跨调用可变状态主要由 `Mutex` 管理：summary 的全局采集向量、`MemStatsHandler.saved`、`MemDomain.tables`、`MemSplitClient.rules/regions`、`MemImporterClient` 的请求记录和 `MemStorage.files`。`metautil::Database.reused_by_pitr` 使用 `AtomicBool(SeqCst)`；`new_uuid_bytes` 使用全局 `AtomicU64(Relaxed)`，只保证进程内生成值不同，前八字节恒零，不具备随机 UUID 的安全性或跨进程唯一性。

`PreallocIDs` 的核心不变量是半开区间 `[Start, End)`：`AllocID` 只接受映射后落入该区间的值；checkpoint 只保存区间、复用边界和哈希，不保存 `AllocRule`；复用时必须从当前表集合重新构造映射。`RewriteRules.Data` 的匹配当前采用首个前缀命中，规则顺序因而有语义。

## 依赖与调用关系

crate 直接依赖 `serde`/`serde_json` 和 `sha2`；本文件实际使用 `serde` 派生备份/PiTR 数据序列化，并用 `Sha256` 固化排序后 ID 集合。`Cargo.toml` 明示该 crate 是 Go 包 `br/pkg/restore/snap_client` 的 library 移植，且刻意不引入 kvproto/grpcio/domain。

主要上游边如下：

- `client.rs` 使用数据库、domain、PD/import/split、checkpoint、ID 预分配、编码和 store 查询接口；其 `InitBackupMeta` 直接调用 `NewAndPreallocTableIDs`/`ReusePreallocatedTableIDs`。
- `import.rs` 使用 `ImporterClient`、`SplitClient`、`Context`、rewrite/protobuf 类型和 token channel；它还把外层 restore crate 的取消上下文桥接成本文件的 `Context::WithCancellationSource`。
- `tikv_sender.rs` 调用 `MergeAndRewriteFileRanges`、`GetPartitionIDMap` 与 `GetAllTiKVStoresWithRetry`。
- `placement_rule_manager.rs` 使用 `PlacementRule`、`SplitClient`、`CreatedTable` 与 store 查询；`pitr_collector.rs` 使用存储接口、文件集合和 UUID；系统表模块使用 `DbSession`、model 与名称辅助函数。
- 独立测试 `client_test.rs`、`import_test.rs`、`tikv_sender_test.rs`、`placement_rule_manager_test.rs`、`pitr_collector_test.rs`、`pipeline_items_test.rs` 和 `parity_test.rs` 注入 `Mem*` 实现并检查记录状态；测试逻辑没有内嵌在 `stubs.rs`。

主要下游不是网络库，而是 Rust 标准库容器/同步原语、`sha2` 和本文件的手写协议结构。真实 PD/TiKV RPC、连接池、TLS、重试退避和持久化均不在此文件实现。

## 错误处理与边界

`Error` 只保存字符串和可选 code；`Trace` 原样返回，`Annotate`/`Wrap` 拼接上下文并保留 code。它没有 Go `errors` 的 cause 链、栈信息，也不会自动识别 gRPC status。`Context` 的 `Mutex::lock().unwrap()` 以及其他所有内存实现的锁都可能在锁中毒时 panic；正常业务错误使用 `Result` 返回。

明确错误边界包括：`DecodeBytes` 拒绝不完整分组或非法 marker；`GetPartitionByName`、`MemDomain::TableInfoByName`、`MemStorage::ReadFile` 在对象缺失时失败；`GetRewriteRawKeys` 在非空键找不到 rewrite rule 时失败；ID 收集、分配和 checkpoint 复用会拒绝过大的范围、哈希不一致或越界映射；`ImporterClient` 的默认 `DownloadSST`/`MultiIngest` 返回“未实现”。

必须注意已验证的简化差异：Rust `ValidateFileRewriteRule` 当前无条件成功；`MergeAndRewriteFileRanges` 不执行 Go 版本的“同 start 不同 end” panic、未知 CF 错误和 range tree 重叠检测；`GetAllTiKVStoresWithRetry` 名称含 Retry，但实现只调用一次 `StoreMeta::GetAllStores` 并过滤 `engine=tiflash`，没有 Go `conn.GetAllTiKVStoresWithRetry` 的退避与取消分支；`MemSplitClient::GetPlacementRule` 在规则缺失时返回合成默认规则而非 not-found；日志函数多数吞掉消息。这些均应作为兼容边界，而不是“已完整支持”的依据。

## 并发与资源生命周期

所有共享内存桩通过 `Arc` 与 `Mutex` 组合供多线程调用，相关 trait 普遍要求 `Send`/`Sync`。锁的临界区较短，但没有异步等待、条件变量或公平性保证。`TokenCh` 本质是 `Arc<Mutex<VecDeque<()>>>`：`acquire_token` 在队列为空时释放锁并 `thread::yield_now()` 忙等；`release_token` 无容量检查，重复释放会扩大 permit 数。因此调用方必须保证每次成功获取恰好释放一次，且不应将它视为高负载生产 semaphore。

`Context::WithTimeout` 保存创建时计算的 `Instant`，并通过闭包继承父 context；它不生成通知 channel，`Done` 只是轮询 `Err`。`ImporterClient::CloseGrpcClient` 和 `DbSession::Close` 是生命周期钩子，但内存实现分别空操作或只设置 `closed`；`SimpleRestorer::Close` 同样只记录状态。`MemStorage` 没有显式 close，数据随拥有它的 `Arc` 生命周期释放。

## 与 Go 版本的对应关系

- `MergeRangesStat`/`MergeAndRewriteFileRanges` 对照 `br/pkg/restore/utils/merge.go`；Rust 保留按阈值合并和统计目的，但以 `HashMap + Vec` 替代 `rtree.RangeStatsTree`，属于明确标注的简化版本。
- `RewriteRules`、`CreatedTable` 分别对照 `br/pkg/restore/utils/rewrite_rule.go` 与 `common.go`；Rust 只保留当前 snap client 使用字段，未覆盖 Go 的全部时间戳、old keyspace 和方法。
- `PreallocIDs`、`CollectTableIDs`、`NewAndPreallocTableIDs`、`ReusePreallocatedTableIDs` 对照 `br/pkg/restore/internal/prealloc_table_id/alloc.go`。核心的安全阈值、排序哈希、可复用区间和重写顺序一致，但 Rust 将收集、分配阶段合并为函数。
- `ImporterClient` 对照 `br/pkg/restore/internal/import_client/import_client.go`；Rust trait 保留 snap restore 所需 RPC，但省略 `ClearFiles`、`ApplyKVFile`、真实 `GetImportClient`、TLS/keepalive 和连接缓存。最新 MVCC 能力探测保留 `Unimplemented => false` 语义。
- `SplitClient` 对照 `br/pkg/restore/split/client.go`，`GetAllTiKVStoresWithRetry` 对照 `br/pkg/conn/conn.go`/`br/pkg/conn/util/util.go`，模型和协议结构则对应 TiDB model 与 kvproto；这些 Rust 定义均是局部子集。
- Rust 独立测试与同目录 Go 测试成对存在，例如 `client_test.rs`/`.go`、`import_test.rs`/`.go`、`placement_rule_manager_test.rs`/`.go`、`pitr_collector_test.rs`/`.go`，用于核对调用顺序、错误传播和可观察结果；它们不能验证真实 gRPC、PD 或对象存储互操作。

## 扩展指南

新增恢复能力时，先判断符号的真实归属。若是 snap client 的编排逻辑，应修改对应的 `client.rs`、`import.rs` 或 sender 文件，并只在此处补充必要的 trait 方法和最小测试替身；若是通用 key rewrite、预分配、PD/Importer 客户端能力，应优先在 canonical crate 实现，避免继续扩大本地复制。任何 trait 变更都要同步全部实现者，尤其 `MemImporterClient`、测试中的包装 client、`MemSplitClient` 和 `MemDb`。

修改编码、范围合并或 ID 预分配时，必须逐分支对照上述 Go 文件，不可用更简单算法替代通过测试。重点补充同目录独立 Rust 测试：编码/范围与导入行为放在 `import_test.rs` 或 `tikv_sender_test.rs`，ID/checkpoint 放在 `client_test.rs`，PD/placement 放在 `placement_rule_manager_test.rs`，存储复制放在 `pitr_collector_test.rs`；不要把测试加入 `stubs.rs`。

需要接入真实外部系统时，不应让 `Mem*` 悄悄获得网络行为；应新增清晰的 adapter 实现并处理连接关闭、TLS、超时、重试与错误码映射。对 `TokenCh` 的扩展必须维持固定容量不变量，或替换为有唤醒机制的有界 semaphore。新增共享状态时还应明确锁顺序和中毒策略。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/snap_client` 确认目标及同目录 38 个 Go/Rust 文件；`node --file .../stubs.rs` 分段覆盖 1–2298 行；`query` 精确定位 `MergeAndRewriteFileRanges`（1407）、`NewAndPreallocTableIDs`（1976）、`GetAllTiKVStoresWithRetry`（2260）和 `ImporterClient::IsBatchDownloadLatestMVCCSupported`（1754）。索引报告该文件被 15 个文件使用。
- crate/入口：`br/pkg/restore/snap_client/Cargo.toml`、`br/pkg/restore/snap_client/lib.rs`。
- Rust 上游与测试：`client.rs`、`import.rs`、`tikv_sender.rs`、`placement_rule_manager.rs`、`pipeline_items.rs`、`pitr_collector.rs`、`systable_restore.rs`、`systable_schema_update.rs`，以及对应的 `*_test.rs`、`parity_test.rs`。
- Go 对照：`br/pkg/restore/utils/merge.go`、`rewrite_rule.go`、`common.go`，`br/pkg/restore/internal/prealloc_table_id/alloc.go`，`br/pkg/restore/internal/import_client/import_client.go`，`br/pkg/restore/split/client.go`，`br/pkg/conn/conn.go` 与 `br/pkg/conn/util/util.go`，并参考同目录 `*_test.go`。
- 人工复核结论：该文件为外部依赖替身与局部算法的混合层；安全扩展的关键是维持 Go 可观察语义、明确简化边界、在独立测试文件中验证 trait adapter 和状态不变量，而不是把内存默认值解释为真实集群行为。
