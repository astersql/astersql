# `br/pkg/stream/stream_mgr.rs`

## 文件定位

本文件属于 `astersql-br-pkg-stream` library crate；`br/pkg/stream/Cargo.toml` 将该 crate 标为 Go 包 `br/pkg/stream` 的迁移实现，`br/pkg/stream/lib.rs` 通过 `#[path = "stream_mgr.rs"]` 装配模块并以 `pub use stream_mgr::*` 扁平导出 API。它对应 Go 的 `br/pkg/stream/stream_mgr.go`，位于 BR 日志备份/PiTR 的公共支撑层：一端面向外部存储中的 `v1/backupmeta` 文件，另一端为流备份启动、日志恢复、元数据截断和测试工具提供观察范围、元数据读取与版本兼容能力。

RustCodeGraph 将该文件识别为 550 行、52 个符号，并显示它被 26 个文件引用。当前 Rust 生产调用证据包括 `br/pkg/stream/stream_metas.rs`、`br/pkg/restore/log_client/log_file_manager.rs`、`br/pkg/task/stream.rs` 和 `br/pkg/utiltest/crr/*`；`StreamManager` 自身只是本文件末尾的轻量门面，并非 Go `br/pkg/task/stream.go` 中负责命令生命周期的 `StreamMgr`。

## 核心职责

1. 用 `GetStreamBackupMetaPrefix`、`GetStreamBackupGlobalCheckpointPrefix` 固化外部存储路径协议。
2. 用 `BuildObserveDataRanges`、`BuildObserveMetaRange` 构造 TiKV record/meta 键空间的左闭右开观察范围；筛选表时排除内存系统库，并将分区表展开为分区物理 ID。
3. 用 `MetadataHelper` 缓存组合数据文件，按 offset/length 取片，随后执行密文 SHA-256 校验、解密和 ZSTD 解压。
4. 在 `Metadata` V1/V2 之间提供兼容视图：软解析把所有 V1 `Files` 放入一个匿名 group，硬解析为每个 file 创建独立 group，写回 V1 时再展平。
5. 从 backup-meta 文件名提取时间边界，通过 `FilterPathByTs` 跳过与恢复窗口不相交的文件；解析不了的未来命名格式保守放行。
6. 通过 `FastUnmarshalMetaData` 或 `FastUnmarshalMetaDataWithOptions` 扫描元数据文件并把原始字节交给调用者；后者提供跳过条件和有界线程并发。

## 主要符号

- `streamBackupMetaPrefix` / `streamBackupGlobalCheckpointPrefix`：分别为 `v1/backupmeta`、`v1/global_checkpoint`；只定义和返回路径，本文件不读写 global checkpoint。
- `appendTableObserveRanges(Vec<i64>)`：对每个物理表 ID 调用 `tablecodec::GenTableRecordPrefix`，以 `PrefixNext` 生成半开区间。
- `ObserveDataSource`、`ObserveTableFilter`：把 snapshot catalog 和表过滤器抽象成两个 trait，令 `BuildObserveDataRanges` 不直接依赖 Go 中的 `kv.Storage`、`meta.Reader` 和 table-filter 实现。
- `buildObserveTableRange`：普通表使用 `table.ID`；分区表只使用每个 `PartitionDefinition.ID`。
- `buildObserverAllRange`：返回 `[b"t", PrefixNext(b"t"))`，即全部 table record 前缀。
- `BuildObserveDataRanges`：`filter_str == ["*.*"]` 时直接走全范围；否则按备份 TS 枚举数据库/表，排除 `information_schema`、`performance_schema`、`metrics_schema`，再应用 schema/table filter。
- `BuildObserveMetaRange`：返回 `[b"m", b"n")`，覆盖 meta 前缀下数据库、表、序列、策略等元数据。
- `ContentRef`：单路径缓存条目，保存 `Path`、当前 `Ref`、初始 `init_ref` 和可选整文件 `data`。
- `MetadataHelper`：持有路径到 `Arc<Mutex<ContentRef>>` 的 map，以及可选 `EncryptionManager`；`NewMetadataHelper`/`new` 创建无加密管理器的空 helper，`with_encryption_manager` 注入管理器。
- `EncryptedFileInfo`：将加密参数和可选密文 checksum 绑定到一次读取。
- `InitCacheEntry`：仅登记正数引用预算，暂不触发 IO。
- `ReadFile` / `ReadFileWithEncryption`：共享 `read_file_content`，前者读取未加密内容，后者按“校验密文 → 解密 → 解压”处理。
- `ParseToMetadata` / `ParseToMetadataHard` / `Marshal`：V1 兼容转换的三个入口。
- `FilterPathByTs`：根据 `astersql-br-pkg-stream-backupmetas::ParseName` 的 `MinBeginTsInDefaultCf`、`MinTS`、`MaxTS` 判断文件是否与 `[left, right]` 相交。
- `FastUnmarshalMetaData`：串行列举和读取 `.meta` 文件；`FastUnmarshalMetaDataWithOptions`：先筛选路径，再以 `worker_pool_size` 个 scoped thread 并行读取和回调。
- `StreamManager`：组合 `MetadataHelper` 与 `Arc<dyn Storage>`，转发 `MetaPrefix`、`ObserveMetaRange` 和 `ParseDBKeyFromMetaField`。

## 执行流程

观察范围流程从 `BuildObserveDataRanges` 开始。通配符恰为单项 `*.*` 时，不查询 catalog，直接返回整个 `t` 前缀；否则使用同一 `backup_ts` 调用 `ObserveDataSource::ListDatabases` 与 `ListTables`，先按原始 schema 名应用过滤器、按小写名排除三个内存库，再把普通表或每个分区的 record prefix 加入结果。meta 观察范围独立由 `BuildObserveMetaRange` 构造，不受表过滤器影响。

组合文件读取流程先由调用方按未来切片次数调用 `InitCacheEntry(path, ref_count)`。`read_file_content` 短暂锁住全局 map 取得条目 `Arc`，随后释放 map 锁；单路径 mutex 内递减 `Ref`、首次读取整文件、检查 `offset + length` 溢出和边界并复制切片。引用耗尽时释放大块 `data`，并把 `Ref` 恢复为 `init_ref`，使同一路径可进入下一轮缓存周期。未登记的路径被视为 V1 整文件：仅 `(offset, length) == (0, 0)` 合法，否则返回未初始化错误。

`ReadFileWithEncryption` 对取得的字节先计算 SHA-256 并与 `Checksum` 比较；checksum 合格后才要求已注入的 encryption manager 并调用 `Decrypt`，最后与普通读取一样由 `decode_compressed` 处理 `UNKNOWN` 或 `ZSTD`。这一顺序保证损坏密文不会进入解密器。

元数据兼容流程以 `serde_json::from_slice` 开始。V1 且 `FileGroups` 为空时，软解析建立一个匿名 group；硬解析按每个 `DataFileInfo` 建 group，并复制 path、时间范围、resolved TS 和 length。截断等调用修改 groups 后，`Marshal` 在 group 数与原 `Files` 数不同时重建 `Files`，随后清空 V1 `FileGroups` 再序列化。

扫描流程先从 `v1/backupmeta` 列举对象，仅保留 `.meta`，再执行时间窗过滤。串行版本逐个读取并立即回调；带 options 版本还先应用 `skip_condition`，之后以 `max(1, worker_pool_size)` 且不超过文件数的线程数抢占原子下标。任一读取或 callback 报错会设置取消标志并记录首个错误，线程汇合后返回该错误。

## 数据与状态

`MetadataHelper.cache` 是长期状态，key 为完整路径；map 锁保护条目集合，每个 `ContentRef` 的独立锁保护引用计数与整文件字节。`init_ref` 是每轮预期消费次数，`Ref` 是当前剩余次数，`data: None` 表示尚未下载或本轮已释放。调用方若低估引用次数，会更早释放并在后续轮次重新下载；若高估，则内容会继续驻留直到 `Close` 或剩余消费发生。

`Metadata`、`DataFileGroup`、`DataFileInfo`、`CompressionType` 来自 `crate::stubs::backuppb`，说明该 crate 仍处在迁移期。观察范围使用 `crate::stubs::kv::KeyRange`；`Storage` 也是 crate 内的同步 trait。并行扫描的局部状态由 `AtomicUsize next`、`AtomicBool cancelled` 和 `Mutex<Option<Error>> first_error` 组成，生命周期只覆盖一次函数调用。

`StreamManager` 只拥有 helper 和共享 storage，没有任务名、PD client、etcd client或后台任务状态。`Close` 清空 helper 缓存并关闭可选 encryption manager；Rust ZSTD 使用每次调用的 `zstd::stream::decode_all`，因此没有 Go decoder 对象需要关闭。

## 依赖与调用关系

上游生产调用中，`br/pkg/stream/stream_metas.rs` 的 `StreamMetadataSet` 持有 `MetadataHelper`：`LoadUntilAndCalculateShiftTS` 通过 `FastUnmarshalMetaData` 扫描后调用 `ParseToMetadataHard`，批量删除/更新时再次硬解析并用 `Marshal` 写回。`br/pkg/restore/log_client/log_file_manager.rs` 调用 `FilterPathByTs` 剔除窗口外 meta，并以 `ParseToMetadata` 获取兼容视图。`br/pkg/task/stream.rs::getGlobalCheckpointFromStorage` 使用全局 checkpoint 前缀；`br/pkg/utiltest/crr/flush_sim.rs` 和 `harness.rs` 分别消费 meta 前缀与解析 API。

下游依赖包括：`crate::stubs::{Storage, meta, tablecodec}` 提供存储、DB key 解析和键编码；`astersql-br-pkg-stream-backupmetas::ParseName` 解析路径时间信息；`astersql-br-pkg-encryption::Manager` 解密；`sha2`/`hex` 完成 checksum；`zstd` 解压；`serde_json` 处理当前桩 Metadata。`Cargo.toml` 明确声明这些依赖，模块入口 `lib.rs` 再将符号暴露到 crate 根。

RustCodeGraph 的 `explore "br/pkg/stream/stream_mgr.rs StreamMgr"` 同时找到了 Go `br/pkg/task/stream.go` 的 `NewStreamMgr` 调用链和 Rust `br/pkg/task/stream.rs` 的同名任务管理器；这是名称相近但职责不同的证据。针对若干函数的独立 `callers` 查询在本地索引上超时，因此具体 Rust 上游以精确 `rg` 结果复核，并未据超时查询臆造静态调用边。

## 错误处理与边界

- catalog 的数据库或表枚举错误通过 `?` 原样传播；通配符快路径不会触碰数据源。
- `InitCacheEntry` 对 `ref_count <= 0` 静默忽略。未缓存路径只允许整文件读取；缓存切片对加法溢出、`u64 -> usize` 失败及越界统一返回 `read out of range`，避免 Go slice 越界 panic。
- `Mutex::lock().unwrap()` 假设锁未 poisoned；线程 panic 会使后续访问 panic，这是当前实现边界。
- 解压仅接受 stubs 中的 `UNKNOWN`、`ZSTD` 两个枚举值；ZSTD 错误附带 `failed to decode compressed data`。
- 有 checksum 时先报告 checksum mismatch；无 checksum 且缺少 manager 时报告 `need to decrypt data but encryption manager not set`；manager 解密错误转为本地 `Error`。
- V1 兼容转换仅在 `FileGroups.is_empty()` 时发生，已有 groups 不会被覆盖。JSON 解析/序列化失败直接返回错误。
- `FilterPathByTs` 对无法解析的文件名、`MinBeginTsInDefaultCf == 0` 或其大于 `MinTS` 的异常命名一律放行；只有明确满足 `right < min_begin` 或 `MaxTS < left` 才过滤。
- 扫描忽略非 `.meta` 对象；options 版本将 worker 数至少提升到 1、至多限制为候选文件数，空候选直接成功。并发失败采用“首错胜出”，已开始的另一个 callback 不能被强制中断。

## 并发与资源生命周期

缓存采用两级锁。全局 map 锁只负责查找/插入/清空；实际存储读取发生在单个 `ContentRef` mutex 下，因此同一路径并发切片只下载一次，而不同路径可同时进行 IO。`br/pkg/stream/stream_mgr_test.rs::concurrent_slices_of_one_cache_entry_download_once` 证明同路径单次下载；Go `stream_misc_test.go::TestMetadataHelperReadFile` 还以 gated storage 验证两个不同缓存条目的读取能够并发。

options 扫描用 `std::thread::scope`，worker 借用 storage、paths 和 callback，无需 `'static` 泄漏；scope 返回前所有线程必然 join。`Acquire/Release` 用于取消可见性，任务索引使用 `Relaxed` 即可，因为它只分配互异下标。callback 约束为 `Send + Sync`，若需要汇总可变结果，调用方必须自行加锁。

`MetadataHelper::Close` 是显式资源终点：清空缓存并调用 encryption manager 的 `Close`。类型没有 `Drop` 自动调用，因此拥有者应在生命周期末尾显式关闭；`StreamManager` 也未实现额外关闭协议。扫描函数取得的 `Arc<dyn Storage>` 在所有 scoped worker 退出后释放本次引用。

## 与 Go 版本的对应关系

结构上 Rust 对齐了 Go 的路径常量、表/分区范围算法、meta 前缀范围、引用计数缓存、checksum-before-decrypt、V1 软/硬转换、时间窗过滤和并发元数据读取意图。Rust 独立 trait `ObserveDataSource`/`ObserveTableFilter` 替代 Go snapshot reader 与 filter 接口；Rust options 扫描使用 scoped threads 和原子索引，对应 Go worker pool + `errgroup` 的有界并发及首错取消。

当前差异必须视为迁移限制：Go `Metadata` 使用 protobuf `Unmarshal/Marshal`，Rust 当前 stubs 使用 `serde_json`，不能直接声称兼容线上 protobuf 字节；源码模块注释也明确称其为桩实现。Go `ReadFile` 把可选 encryption info 合并在单一 API 中，Rust拆为 `ReadFile` 与 `ReadFileWithEncryption`。Go `FastUnmarshalMetaData` 本身含 worker 数和 skip condition，Rust保留一个简化串行入口并新增 `FastUnmarshalMetaDataWithOptions` 承载完整语义。Go 接收 context、支持 WalkDir/errgroup 取消并包装扫描错误；Rust同步 `Storage` 无 context，取消只能阻止领取后续路径。

Go 的 `MetadataHelper` 持有可复用 ZSTD decoder；Rust每次调用标准 decoder。Go `Close` 关闭 decoder 和 encryption manager但 cache map 由对象释放，Rust显式清空 cache并关闭 manager。Rust还对缓存切片加入受控越界错误，较 Go 直接 slice 更安全。Rust `BuildObserveDataRanges` 仅硬编码三个内存库名，语义目标对应 Go `metadef.IsMemDB`，后者若扩充集合需同步审计。

## 扩展指南

新增压缩格式应修改 `decode_compressed` 和 stubs `CompressionType`，并在独立 `br/pkg/stream/stream_mgr_test.rs` 增加合法帧、损坏帧及 raw length 行为测试；不要把测试嵌入生产文件。替换 JSON 桩为 protobuf 时，应集中调整 `ParseToMetadata`、`ParseToMetadataHard`、`Marshal`，并用 Go 生成的真实字节 fixture 验证双向兼容，避免只让 Rust 自编码/自解码通过。

扩展观察范围时应修改 `BuildObserveDataRanges`、`buildObserveTableRange` 或 `BuildObserveMetaRange`，同步核对 Go `stream_mgr.go`、`tablecodec` 前缀及内存库判定；分区表是否还需包含逻辑 table ID 是源码中已有的设计问题，不能在无上游语义证据时擅自改变。相关回归应放在 `stream_mgr_test.rs`，覆盖普通表、分区表、过滤、系统库和 catalog 错误。

修改缓存策略必须维持“同路径只下载一次、不同路径不互相阻塞、引用耗尽释放大块数据”的不变量，并关注 `ref_count` 不准确造成的重复 IO 或驻留内存风险。修改并行扫描应保持 `.meta` 筛选、时间窗过滤、skip 条件、首错返回和 worker 上限；性能评估重点是对象数量、callback 耗时与峰值元数据内存。

变更路径格式时必须同时审查 `astersql-br-pkg-stream-backupmetas::ParseName` 和 `FilterPathByTs` 的保守放行策略。加密流程扩展必须保留密文 checksum 在解密前校验的顺序，并同步 `br/pkg/encryption` 的 manager 生命周期测试。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`files --filter br/pkg/stream/stream_mgr.rs` 找到目标；`explore "br/pkg/stream/stream_mgr.rs StreamMgr"` 返回目标完整源码、52 个符号上下文及相关 Go/Rust `StreamMgr`；`node --file ... --offset/--limit` 分段核对全部 550 行。对 `callers` 的拆分查询超时，随后用精确符号搜索补足调用证据。
- Rust 源与 crate：`br/pkg/stream/stream_mgr.rs`、`br/pkg/stream/Cargo.toml`、`br/pkg/stream/lib.rs`。
- Rust 直接调用：`br/pkg/stream/stream_metas.rs`、`br/pkg/restore/log_client/log_file_manager.rs`、`br/pkg/task/stream.rs`、`br/pkg/utiltest/crr/flush_sim.rs`、`br/pkg/utiltest/crr/harness.rs`。
- Go 对照：`br/pkg/stream/stream_mgr.go`；相关 Go 测试位于 `br/pkg/stream/stream_misc_test.go`，覆盖 helper 读取与并行性、V1 group 保留、路径过滤和 skip condition；`br/pkg/stream/stream_metas_test.go` 覆盖 helper 在元数据集合加载/截断中的集成使用。
- Rust 独立测试：`br/pkg/stream/stream_mgr_test.rs` 覆盖任意合法 ZSTD 帧、V1 hard/Marshal 字段、加密校验顺序、meta 前缀、缓存单次下载、同路径并发、非 meta 跳过、并行 skip 及普通/分区观察范围。`br/pkg/stream/lib.rs` 仅在 `cfg(test)` 下挂载该测试文件，符合源文件与测试分离要求。
- 本任务是只读代码分析和文档新增，按计划不运行 Cargo；交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核未把 JSON 桩描述为线上 protobuf 支持。
