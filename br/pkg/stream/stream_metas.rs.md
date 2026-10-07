# `br/pkg/stream/stream_metas.rs`

## 文件定位

本文件属于 `astersql-br-pkg-stream` library crate；`br/pkg/stream/Cargo.toml` 把 `lib.rs` 设为入口，`lib.rs` 以 `#[path = "stream_metas.rs"] pub mod stream_metas` 挂载本模块，并通过 `pub use stream_metas::*` 扁平导出公共项。它对应 Go 的 `br/pkg/stream/stream_metas.go`，位于 BR 流备份元数据读取、日志截断和 migration 持久化这一控制面，而不是 SQL 执行主链。

当前 Rust 文件把 Go 大文件中的四类能力集中在一起：`StreamMetadataSet` 管理 metadata 摘要和删除写回，`UpdateShiftTS*` 计算恢复窗口的前移时间戳，`GetTSFromFile`/`SetTSToFile` 持久化历史截断安全点，`MigrationExt`/`Migrations` 管理 `v1/migrations` 下的 BASE 与追加层。跨 crate 的直接生产调用证据包括 `br/pkg/restore/log_client/log_file_manager.rs` 对 `TryParseTaggedBackupMetaFileNameWrapper` 和 `UpdateShiftTS` 的调用；crate 内测试入口 `br/pkg/stream/export_test.rs::StreamMetadataSet::LoadFrom` 则以 `u64::MAX` 包装全量加载。

## 核心职责

1. `StreamMetadataSet::LoadUntilAndCalculateShiftTS` 通过 `stream_mgr::FastUnmarshalMetaData` 扫描 `[0, until]` 的 metadata，缓存每个 meta 的最小时间戳及 FileGroup 的 `MaxTS`、长度、KV 数，并同时求所有有效候选中的最小 shift TS。
2. `IterateFilesFullyBefore` 与 `RemoveDataFilesAndUpdateMetadataInBatch` 识别 `MaxTs < before/from` 的完整 FileGroup，先删除物理数据，再重算并写回或删除 metadata 文件。
3. `UpdateShiftTS` 优先使用带标签的 backupmeta 文件名统计；只有文件名无法解析或标签统计无效时，才由 `UpdateShiftTSFromMetadata` 扫描 write CF 文件的 `MinBeginTsInDefaultCf`。
4. `ReplaceMetadata` 和私有 `updateMetadataInternalStat` 维持 `Metadata.FileGroups` 与顶层 `MinTs`、`MaxTs`、`ResolvedTs` 的一致性。
5. `Migrations`、`MergeMigrations` 和 `MigrationExt` 读取、追加、合并 migration，并以 `BASE_TMP` 写入后 rename 到 `BASE`，再清理不高于目标序号的追加文件。
6. `hashMigration`、`hashMetaEdit`、`nameOf` 生成与 Go 对齐的 CRC64-ISO 内容指纹及 `{sn:08}_{hash:016X}.mgrt` 文件名；`isInsane` 为潜在前缀删除提供安全判定，但当前文件的 Rust 路径并未把它接入真实删除流程。

## 主要符号

- `SupportedMigVersion: MigrationVersion::M2` 与 `NewMigration() -> Migration`：声明当前默认 migration 版本。Rust 的 `Creator` 固定为 `br;commit=unknown;branch=unknown`，不像 Go 版注入构建时 Git hash/branch。
- `StreamMetadataSet`：持有 `metadataInfos`、批大小提示、`MetadataHelper` 和测试写回钩子。`DryRun`、`MetadataDownloadBatchSize`、`Helper` 保留 Go 形状；当前加载代码直接调用静态 helper，未读取后两者，`DryRun` 也未改变删除路径。
- `FileGroupInfo` / `MetadataInfo`：为统计和过滤保留的轻量摘要，避免把完整 protobuf 长期留在内存中。
- `LoadUntilAndCalculateShiftTS`、`IterateFilesFullyBefore`、`RemoveDataFilesAndUpdateMetadataInBatch`：metadata 生命周期的主要入口。
- `TruncateSafePointFileName`、`GetTSFromFile`、`SetTSToFile`：以十进制文本保存截断 TS；历史文件名拼写 `trancate` 与 Go 兼容。
- `TryParseTaggedBackupMetaFileNameWrapper`、`UpdateShiftTS`、`UpdateShiftTSFromMetadata`：带标签文件名快速路径及 metadata 回退路径。
- `ReplaceMetadata`：替换 FileGroup 并恢复顶层时间范围不变量。
- `Migrations::{ListAll, MergeTo, MergeToBy}`：把 BASE 与按序号排序的追加层构造成视图或折叠结果。
- `MigrationExt::{LoadBase, Load, AppendMigration, MergeAndMigrateTo}`：对象存储上的 migration 读写入口；`GetReadLock` 与 `DryRun` 明确是测试桩。
- `MMOptSkipLockingInTest`、`MMOptAlwaysRunTruncate`、`MMOptAppendPhantomMigration`、`MMOptInteractiveCheck`：单次 merge 配置。当前 `MergeAndMigrateTo` 会消费 phantom/interactive 状态，但 `skip_locking` 和 `always_run_truncate` 仅被复位，没有驱动真实锁或 truncate。
- `MergeMigrations`、`mergeMetaEdits`、`mergeDeleteLogicalFiles`：按 meta path、逻辑文件 path 合并删除描述；`TruncatedTo` 取最大值，其余 compaction、destruct prefix、ingested SST 路径按顺序拼接。
- `migIdOf`、`isEmptyEdition`、`isInsane`、`isEmptyMetadata`、`hashMigration`、`hashMetaEdit`、`nameOf`：格式解析、空值判断、安全检查和稳定命名辅助函数。

## 执行流程

metadata 加载与 shift TS 流程如下：调用者构造或复用 `StreamMetadataSet`，`LoadUntilAndCalculateShiftTS` 扫描 metadata；每份原始内容经 `MetadataHelper::ParseToMetadataHard` 解码。若 `m.MinTs <= until`，函数聚合 FileGroup 摘要并按文件名插入临时 map。随后无论是否缓存该摘要，都调用 `UpdateShiftTS(filename, m, until, u64::MAX)`：标签可给出结果时直接使用，标签无有效 shift 时返回未找到，标签统计无效或文件名不合法时扫描 metadata。最终把最小候选写回 `metadataInfos` 并返回；没有候选时返回原 `until`。

截断写回流程从已加载的路径快照开始。对 `MetadataInfo.MinTS < from` 的 meta 重新读取并解析完整内容，收集所有 `MaxTs < from` 的 FileGroup 路径，逐个调用 `Storage::DeleteFile`；随后从 V2 `FileGroups` 和 V1 `Files` 中都移除与这些 FileGroup 同路径的项，调用 `updateMetadataInternalStat` 重算顶层统计。注意删除候选不会从扁平 `Files` 独立产生，因此纯 V1 metadata 的完整截断在当前 Rust 实现中并未得到源码证实。`BeforeDoWriteBack` 可令测试跳过写回；否则空 metadata 被删除，非空 metadata 重新 marshal 并覆盖写入。每份 meta 处理完成后，以候选路径数调用 `update_fn`。失败被聚合为 warnings；若 warnings 非空而 `not_deleted` 仍为空则整体返回错误，否则返回无法确认提交 metadata 更新的物理路径。

migration 流程由 `MigrationExtension` 建立默认 `v1/migrations` 前缀。`Load` 读取 BASE；只有确实存在持久化 BASE 时，才把旧 safepoint 合入 `TruncatedTo`，然后列举目录、跳过 `BASE`/`BASE_TMP`、解析八位十进制序号、反序列化并排序追加项。`AppendMigration` 取现有最大序号加一（空时从 1 开始），按内容哈希命名并写入。`MergeAndMigrateTo` 应用一次性选项，加载并折叠 `id <= seq` 的层，再合入 phantom；交互检查拒绝时以 warning 正常返回且不修改存储。接受后先写 `BASE_TMP`，rename 为 `BASE`，最后尽力删除已合并的追加文件。

## 数据与状态

`StreamMetadataSet.metadataInfos` 是一次成功加载后的完整快照；函数先在局部 map 中构造，扫描成功后才整体替换，因此解析或遍历失败不会留下半更新 map。map 的迭代顺序未定义，`IterateFilesFullyBefore` 的回调不能依赖路径顺序。`BeforeDoWriteBack` 是可变、可跨调用保留的闭包状态，只适合受控测试或显式注入。

FileGroup 的删除边界严格使用 `<`：`MaxTS == before/from` 不属于“完全早于”。`UpdateShiftTSFromMetadata` 只考察时间窗口相交、`Cf != DefaultCF` 且 `MinBeginTsInDefaultCf != 0` 的文件，并取最小 begin TS。`ReplaceMetadata` 在空集合时把三个顶层时间字段全部清零；非空时分别取 group 的最小 `MinTs`、最大 `MaxTs` 和最小 `MinResolvedTs`。

Migration 的持久状态由一个可选 BASE 和有序 `(i32, Migration)` 追加列表组成。合并不去重 compaction、destruct prefix 或 SST 路径。对相同 `MetaEdit.Path`，右侧物理删除项追加，逻辑删除再按文件 path 合并 span；从左侧重建 edit 时故意不继承 `DestructSelf`，这是 `stream_metas_test.rs::test_merge_migrations_rebuilds_left_meta_edit_like_go` 锁定的 Go 行为。哈希采用各项 CRC64-ISO 的异或，因此对列表顺序不敏感，也不能视为密码学完整性校验。

## 依赖与调用关系

本 crate 的直接 Cargo 依赖中，本文件实际使用 `astersql-br-pkg-stream-backupmetas::ShiftTSStatus` 和 `astersql-br-pkg-utils-consts::DefaultCF`；protobuf 形状、`Storage`、错误和 metadata helper 来自 crate 内 `stubs`/`stream_mgr`。`serde_json` 用于 `GetReadLock` 的测试锁内容。`Cargo.toml` 还声明了 encryption、streamhelper、iter、regex、sha2、base64、hex、zstd 等 crate 级依赖，不能据此断言都是本文件的直接依赖。

向下调用的关键边是 `LoadUntilAndCalculateShiftTS -> FastUnmarshalMetaData -> MetadataHelper::ParseToMetadataHard -> UpdateShiftTS`，删除路径调用 `Storage::{ReadFile,DeleteFile,WriteFile}` 与 `MetadataHelper::Marshal`，migration 路径调用 `Migration::{Marshal,Unmarshal}` 及 `Storage::{ListFiles,Rename}`。向上调用方面，`br/pkg/restore/log_client/log_file_manager.rs::LogFileManager::ReadFilteredEntriesFromFiles` 使用 `UpdateShiftTS` 过滤恢复窗口；`br/pkg/stream/export_test.rs::LoadFrom` 包装加载入口。Go 生产对照还由 `MigrationExt::processTruncatedTo` 调用 metadata 加载与截断，但当前 Rust 文件没有等价的完整 `migrateTo/processTruncatedTo` 调用链。

RustCodeGraph 报告目标文件被 29 个文件引用，但对所查方法 `LoadUntilAndCalculateShiftTS`、`RemoveDataFilesAndUpdateMetadataInBatch`、`UpdateShiftTS`、`MergeAndMigrateTo` 的方法级 `callers/callees` 未输出边；因此上述精确边另由索引源码与仓库文本引用核对，不能把“29 个文件”理解成 29 个生产调用者。

## 错误处理与边界

存储字符串错误统一转换为本地 `Error`，metadata/migration 编解码失败直接向上传播。safepoint 文件不存在返回 `0`；内容不是 UTF-8 或十进制 `u64` 时返回错误。`migIdOf` 接受精确 `BASE` 为序号 0，其余名称至少要有八字节十进制前缀；非 ASCII 名称在 `s[..8]` 处存在字符边界 panic 风险，因此不能把任意不可信 Unicode 文件名直接视为安全输入。

`UpdateShiftTSFromMetadata` 对空 FileGroup 或完全不相交时间窗返回 `(0, false)`。标签解析成功但状态是 `ShiftTSNotFound` 时不会扫描 metadata，这是标签统计的明确结论；仅 `ShiftTSInvalidStats` 才回退。`TryParseTaggedBackupMetaFileNameWrapper` 只去最后一个路径分量及末尾 `.meta`。

删除流程没有回滚事务：物理文件删除成功而 metadata 写回失败时，会把相关路径放入 `not_deleted`，其含义更接近“未能确认完整删除/提交”而非“物理对象一定还存在”。单个物理删除失败只记 warning，但代码仍从 metadata 中移除该路径；最终 warnings 与 `not_deleted` 的组合决定是返回聚合错误还是路径列表。`MergeAndMigrateTo` 对已合并追加文件的删除错误直接忽略；`LoadBase` 也把任何读取错误都当成 BASE 不存在，而非只识别 NotFound。这些都是扩展错误语义时需要优先收紧的边界。

## 并发与资源生命周期

所有 API 均为同步调用；共享存储以 `Arc<dyn Storage>` 持有，但 `StreamMetadataSet` 的内部 map 和钩子由 `&mut self` 串行修改。Rust 实现逐 meta、逐文件顺序执行，不包含 Go 版 `FastUnmarshalMetaData` 回调并发时使用的 mutex，也没有 Go `RemoveDataFilesAndUpdateMetadataInBatch` 所描述的并行批处理。`Arc` 只保证存储句柄共享，不代表本对象可并发调用。

截断遵循“先删物理数据，再修改 metadata 引用”，以减少 metadata 指向已计划删除对象的窗口，但两步没有原子事务。migration BASE 的写入采用 `BASE_TMP -> Rename(BASE)`，降低半写 BASE 风险；rename 后再删除旧追加层，使读取者优先看见覆盖了旧层语义的新 BASE。当前实现没有真实远程写锁：`GetReadLock` 只是写测试标记，`skip_locking` 不参与控制流。因此多进程并发 append/merge 时，序号分配、BASE 更新和清理均可能竞争，不能按 Go 版锁保证解释。

一次 `MergeAndMigrateTo` 会通过 `mem::take`/`Option::take` 消费 phantom 与交互闭包，并复位两个布尔选项，避免复用 `MigrationExt` 时重放单次配置。`DryRun` 会执行传入闭包但恒返回空 effects，不能用于审计真实变更。`uuidish` 依赖系统时间并对时钟早于 Unix epoch 使用 `unwrap`；它仅服务测试锁名，不是分布式唯一性保证。

## 与 Go 版本的对应关系

Rust 的数据形状、主要命名、shift TS 分支、safepoint 文本格式、metadata 顶层统计、migration CRC64-ISO 命名以及 BASE 临时写入顺序均直接对照 `br/pkg/stream/stream_metas.go`。独立 Rust 测试 `br/pkg/stream/stream_metas_test.rs` 对照 `stream_metas_test.go`，覆盖 V1/V2 截断、严格时间边界、safepoint 往返、shift TS、migration hash/merge/append/retry、用户取消和 phantom migration；`br/pkg/stream/parity_test.rs` 另覆盖部分纯函数奇偶性。

当前 Rust 版不是 Go 文件的完整等价移植。Go 的 metadata 加载使用配置的 helper/批大小并保护并发回调；Rust 当前忽略这两个字段。Rust 的删除候选仅来自 `FileGroups`，不等价于 Go 的 V1/V2 处理；虽然 Rust 测试 `truncate_log_common(false)` 宣称覆盖 V1，本任务未运行 Cargo，且该断言与当前候选生成源码存在张力，应视为待回归的实现缺口。Go 的删除经 `MigrationExtension`、hook、dry-run effects 和 `doTruncateLogs` 汇合；Rust 直接删除/回写。Go 的 `MergeAndMigrateTo` 获取远程写锁，保留更丰富的 Base/Source/MigratedTo 结果，执行 `migrateTo`、truncate、compaction、前缀和外部全备处理并收集删除警告；Rust 当前只合并、写 BASE、删 migration 文件，`always_run_truncate` 不产生行为。Go 会校验 migration 版本，Rust `Load` 当前直接反序列化。扩展时必须以 Go 的相应函数为语义基线，不能因现有 Rust 测试通过而删除这些差距。

## 扩展指南

若扩展 metadata 加载或截断，应优先修改 `StreamMetadataSet::{LoadUntilAndCalculateShiftTS,RemoveDataFilesAndUpdateMetadataInBatch}`，并同步独立的 `br/pkg/stream/stream_metas_test.rs`，不要把测试嵌入生产文件。新增时间过滤必须保留严格 `<` 边界、标签三态回退规则和 V1 `Files`/V2 `FileGroups` 兼容；若启用批量并发，需要显式定义 map 汇总、hook 调用和错误聚合顺序。

若补齐 migration，应围绕 `MigrationExt::{Load,AppendMigration,MergeAndMigrateTo}` 接入真实读写锁、版本校验和 Go 的 migrate/truncate 各处理阶段，同时保证 `BASE_TMP` rename 顺序及幂等重试。`AppendMigration` 的“读取最大序号再写入”必须纳入锁域。删除前缀时应让 `isInsane` 成为不可绕过的守卫，并为绝对路径、`..`、`.`、`v1` 等输入增加独立回归测试。

任何字段或协议调整都需同时检查 `br/pkg/stream/stream_metas.go`、`br/pkg/stream/stream_metas_test.go`、Rust 测试和 `br/pkg/restore/log_client/log_file_manager.rs`。性能风险主要来自全量列举/读取、线性路径匹配和顺序删除；兼容风险集中于历史 safepoint、migration 文件名/CRC、M1/M2 版本及 V1/V2 metadata；正确性风险集中于无锁并发、部分删除和错误被吞掉。

## 验证依据

- RustCodeGraph：`status` 确认本地索引可用；`files --filter br/pkg/stream` 确认目标与测试被索引；`node --file br/pkg/stream/stream_metas.rs --offset 1/261/521` 读取完整 818 行并确认符号、分支和文件级引用；`node --file br/pkg/stream/stream_metas_test.rs --offset 1/501` 读取完整 772 行测试。
- RustCodeGraph 调用查询：对 `LoadUntilAndCalculateShiftTS`、`RemoveDataFilesAndUpdateMetadataInBatch`、`UpdateShiftTS`、`MergeAndMigrateTo` 执行 `callers`/`callees`，本索引未返回方法级边；因此又核对了模块入口和直接引用，且在本文明确记录该限制。
- crate/入口：读取 `br/pkg/stream/Cargo.toml` 与 `br/pkg/stream/lib.rs`，确认 package 边界、模块挂载、公开再导出和测试文件独立挂载。
- Go 对照：读取 `br/pkg/stream/stream_metas.go` 的 metadata、shift TS、migration merge/migrate 与 `processTruncatedTo` 区段；用 `br/pkg/stream/stream_metas_test.go` 的测试列表核对覆盖主题和尚未移植的复杂场景。
- 直接调用与测试：核对 `br/pkg/restore/log_client/log_file_manager.rs`、`br/pkg/stream/export_test.rs`、`br/pkg/stream/parity_test.rs` 和 `br/pkg/stream/stream_metas_test.rs`。未运行 Cargo，符合本任务纯文档约束。
