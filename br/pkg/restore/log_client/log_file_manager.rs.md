# `br/pkg/restore/log_client/log_file_manager.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` crate，是 PiTR 日志恢复中“发现备份元数据、按恢复时间窗筛选文件、读取 MetaKV 内容、暴露压缩/摄取 SST”的文件管理层。crate 根在 [`lib.rs`](./lib.rs)，通过 `pub mod log_file_manager` 挂载本模块并扁平再导出其公开 API；[`Cargo.toml`](./Cargo.toml) 将该 crate 定义为 library，并声明对 `stream`、`restore`、`restore-utils`、`utils-iter` 等本地 crate 以及 `sha2`、`serde_json` 的依赖。

应用侧直接入口是 [`client.rs`](./client.rs) 的 `LogClient::InstallLogFileManager`：它组装 `LogFileManagerInit` 后调用 `CreateLogFileManager`。RustCodeGraph 还表明 Go 主链中 `RunStreamRestore -> restoreStream -> PreSplitRegions -> LoadDMLFiles`，而 Rust `client.rs` 已调用本文件的构造函数；本文件因此是恢复编排与对象存储日志归档之间的边界，而不是实际向 TiKV 导入 KV 的执行器。

## 核心职责

1. `CreateLogFileManager` 固化恢复窗口 `[startTS, restoreTS]`，扫描 `v1/backupmeta` 计算 Default CF 需要向前延伸的 `shiftStartTS`，并把结果同步给 `WithMigrationsBuilder`。
2. `streamingMeta` 从注入列表及 `Storage::WalkDir` 枚举 `.meta`，跳过 tagged empty metadata、按文件名和 metadata 的 TS 范围过滤，再解析为 `MetaName` 流。
3. `FilterDataFiles`、`FilterMetaFiles` 在 `WithMigrations` 视图上展开物理组和逻辑文件，分别产生 DML 文件流与按物理路径聚合的 DDL/meta 文件组。
4. `ReadFilteredEntriesFromFiles` 校验指定 range 的 SHA-256，解析 MetaKV 事件，执行恢复窗口过滤、Write CF 类型过滤、auto-ID 去重，并按 `filter_ts` 拆成两批。
5. `GetCompactionIter`、`GetIngestedSSTs` 和 `CountExtraSSTTotalKVs` 暴露迁移产生的额外 SST；`Subcompactions` 与 `LoadMigrations` 提供辅助迭代入口。

本文件只管理描述信息和读取/筛选过程，不负责 region split、KV apply 或重写 schema；这些动作由 `client.rs`、`migration.rs`、`ssts.rs` 等上层或相邻模块消费其结果。

## 主要符号

- 类型别名 `Meta`/`Log`、`GroupIndex`/`FileIndex` 及各 `*Iter`：把 protobuf 桩类型与 `TryNextor` 组合成稳定的模块 API。`LogIter` 是 DML 输出，`MetaGroupIter` 是 DDL/meta 分组输出，`SSTIter` 是迁移 SST 输出。
- `MetaName { meta, name }`：把解析后的 `Metadata` 与对象存储路径绑定，路径同时用于诊断和后续分组。
- `LogDataFileInfo`：复制 `DataFileInfo` 的文件、range、CF、TS、压缩、校验和和加密字段，并增加 `MetaDataGroupName`、`OffsetInMetaGroup`、`OffsetInMergedGroup` 三个检查点定位字段。`from_data_file` 完成转换；`GetSha256` 返回副本；`AppliedFile` 实现暴露起止 key。
- `LogFilesStatistic`：以 `AtomicI64`/`AtomicU64` 保存条目数、文件数和字节数，供 `FilterMetaFiles` 可选累加。
- `DDLMetaGroup { Path, FileMetas }`：同一物理 group 路径下的 meta 文件集合，供 helper 初始化缓存引用计数。
- `LogFileManagerInit`：构造参数，包括时间窗、`Arc<dyn Storage>`、迁移 builder/视图、下载批大小和可选加密管理器。当前实现没有把 `EncryptionManager` 接入 `MetadataHelper`。
- `LogFileManager`：核心状态。`startTS`/`restoreTS`/`shiftStartTS` 定义时间窗；`storage` 和 `helper` 负责枚举、读取、解析；`withMigrationBuilder`/`withMigrations` 叠加迁移；`Stats` 是可选统计；`injected_metas` 是测试或无 walk 场景的附加输入。
- `CreateLogFileManager`、`BuildMigrations`、`ShiftTS`、`ValidateRetainLatestMVCCCompactionCoverage`：生命周期和迁移配置入口。
- `ShouldFilterOutByTsStatic`：文件级 TS 判定的权威纯函数；`ShouldFilterOutByTs` 是实例包装。
- `ReadFilteredEntriesFromFiles`、`getKeyTS`、`countReadableMetaKVFiles`、`shouldReadMetaKVFile`：MetaKV 内容及可读文件规则。
- `Subcompactions`、`LoadMigrations`：独立辅助函数。前者从指定前缀读取 JSON 并过滤 TS，后者当前仅包装调用方传入的迁移列表。

## 执行流程

构造阶段：

1. `CreateLogFileManager` 将 `shiftStartTS` 暂设为 `startTS`，保存 storage、迁移对象和批大小。
2. `loadShiftTS` 遍历 `v1/backupmeta` 下的 `.meta`。若 tagged 文件名能直接给出 shift TS，就取所有命中值的最小值；若文件名不能得出有效结论，则先用 `FilterPathByTs` 判定候选，再读取并解析 metadata，通过 `UpdateShiftTS` 计算候选值。
3. 最终值为 `min(startTS, 最小候选)`；没有候选时仍为 `startTS`。随后调用 `WithMigrationsBuilder::SetShiftStartTS`，构造失败则不返回半初始化 manager。

文件发现阶段：

1. `streamingMeta` 先复制 `injected_metas`，再枚举 storage；只接受 `.meta`，跳过 tagged empty 文件及不与 `[shiftStartTS, restoreTS]` 相交的路径。
2. 每个候选文件通过 `Storage::ReadFile` 和 `MetadataHelper::ParseToMetadata` 转成 `MetaName`；读取和解析错误分别附加具体路径。
3. 最后一层 `FilterOut` 再以 metadata 的 `MinTs`/`MaxTs` 排除完全在窗口外的内容。

DML 路径：`LoadDMLFiles -> streamingMeta -> FilterDataFiles`。后者先调用 `WithMigrations::Metas`，再逐层执行 `Physicals` 和 `Logicals`；排除 `IsMeta` 或 TS 不合格文件。`MetaVersion > 1` 时逻辑文件的 `Path` 被物理 `DataFileGroup::Path` 覆盖。输出 `LogDataFileInfo` 时保存 group/file 枚举下标和首个 group 路径作为检查点键。该路径保持惰性，不会一次物化全部 DML 文件。

DDL 路径：`LoadDDLFiles -> streamingMeta -> FilterMetaFiles -> collectDDLFilesAndPrepareCache`。`FilterMetaFiles` 对每个 group 先修正 v2 路径和执行 TS 过滤，可选更新统计，然后只收集 `IsMeta` 文件。收集函数会物化全部 group，为每个路径调用 `MetadataHelper::InitCacheEntry(path, countReadableMetaKVFiles(...))`，最后返回扁平文件列表；这是为后续排序/读取准备的非惰性路径。

MetaKV 内容路径：

1. `read_file_slice` 读取整个对象后按 `RangeOffset..RangeOffset+RangeLength` 复制目标区间；测试配置下可改用注册的 fake helper。越界返回错误。
2. `ReadFilteredEntriesFromFiles` 对目标区间计算 SHA-256，必须与 `DataFileInfo::Sha256` 相同。
3. `decode_kv_entry` 按“小端 key 长度 + key + 小端 value 长度 + value”推进游标；仅保留 `mD` 前缀相关键，并由 `getKeyTS` 从键尾 8 字节按 `DecodeUintDesc` 语义得到 TS。
4. 丢弃超出 restore 窗口、低于对应 CF 下界或空 value 的条目。Write CF 还要求 value 至少 9 字节，只接受 `P`/`D`，跳过 `L`/`R`，其他类型报错。
5. DDL job history 只在非 Write CF 中直接保留。auto-ID 类键（`IID:`、`TID:`、`SID:`、`TARID`）以 `TruncateTS` 后的逻辑键去重，保留最大 TS，并用首次出现顺序保证稳定输出；其他 mDB 键不去重。
6. `append_entry` 以 `ts < filter_ts` 放入第一个向量，否则放入第二个向量。去重在拆分前完成。

SST 路径：`GetCompactionIter` 将迁移 compaction 包装为 `CompactedSSTs`；`GetIngestedSSTs` 展平 ingested group 并保留 rewrite 信息；`CountExtraSSTTotalKVs` 串接二者，对每个 `SSTs::GetSSTs()` 的 `TotalKvs` 做饱和累加。

## 数据与状态

三个 TS 的不变量最重要：Write CF 的文件/条目下界是 `startTS`，Default CF 的下界是可能更早的 `shiftStartTS`，共同上界是 `restoreTS`。`shiftStartTS <= startTS`，用于保留提交时间在恢复窗内、但事务 start TS 更早的 Default CF 数据。

`LogDataFileInfo` 是拥有所有权的快照，不借用 protobuf 缓冲。`ReadFilteredEntriesFromFiles` 也为 key/value 构造独立 `Vec<u8>`；独立测试会在返回后污染 fake helper 缓冲，确认结果不被底层缓冲生命周期牵连。

`MetaDataGroupName` 取 metadata 首个 file group 的路径，`OffsetInMetaGroup` 和 `OffsetInMergedGroup` 来自两层 `Enumerate`。因此修改展开顺序、过滤时机或 v2 path 规则会影响检查点/skip map 的稳定定位，不能只按展示字段看待。

`Stats` 只在 `FilterMetaFiles` 中更新，并且统计的是通过 TS 过滤的全部文件，然后才根据 `IsMeta` 决定是否加入结果；它不是单纯的 DDL 文件统计。原子操作使用 `Relaxed`，只保证各计数原子，不提供跨字段一致快照。

`injected_metas` 会与 storage 枚举结果拼接，不会覆盖 storage。生产调用若误用注入 API，可能造成重复元数据；当前直接证据只在 `log_file_manager_test.rs` 使用它。

## 依赖与调用关系

上游关系（RustCodeGraph 及源码）：

- `client.rs::LogClient::InstallLogFileManager -> CreateLogFileManager`，安装后由 `LogClient` 持有 manager。
- `client.rs::PreSplitRegions` 使用 `LoadDMLFiles` 的同类入口；Go 完整编排中 `restoreStream` 也使用 `LoadDMLFiles`、`BuildMigrations`、`GetCompactionIter` 与 `GetIngestedSSTs`，`RunStreamRestore` 使用 `LoadDDLFiles`。Rust 图中部分同名方法当前只形成自边，说明上层 Rust 接线尚未达到 Go 的完整覆盖，不能据 Go 调用边宣称 Rust 已全部接通。
- `log_file_manager_test.rs` 直接覆盖 `CreateLogFileManager`、`LoadDMLFiles`、`ReadFilteredEntriesFromFiles` 和静态 TS 过滤；`export_test.rs` 提供 `TEST_NewLogFileManager`、`ReadStreamMeta` 等测试入口。

下游关系：

- `Storage::{WalkDir, ReadFile}` 提供对象存储抽象；`MetadataHelper` 解析 metadata、记录缓存引用并在 `Close` 时释放资源。
- `stream_metas::{TryParseTaggedBackupMetaFileNameWrapper, UpdateShiftTS}` 与 `stream_mgr::FilterPathByTs` 决定 metadata 名称和 TS 窗口语义。
- `WithMigrations::{Metas, Compactions, IngestedSSTs}`、`MetaWithMigrations::Physicals`、`PhysicalWithMigrations::Logicals` 在枚举过程中应用迁移/跳过规则。
- `astersql_br_pkg_utils_iter` 的 `FromSlice`、`FilterOut`、`Map`、`FlatMap`、`CollectAll`、`ConcatAll`、`TryNextor` 构成惰性管线；注意 `FilterOut` 谓词为 `true` 时丢弃。
- `CompactedSSTs`、`CopiedSST` 和 `SSTs` 来自 `ssts.rs`；`TruncateTS` 来自 restore utils；错误分类来自本 crate 的 `stubs::berrors`。

## 错误处理与边界

- 构造时任何 walk、读取或 metadata 解析失败都会经 `?` 返回；`streamingMeta` 对读取和解析补充文件路径上下文。
- tagged empty metadata 在读取前跳过，所以即使其内容无效也不应导致失败；Rust 测试 `test_log_file_manager_skips_empty_meta_by_name` 和 Go `TestLogFileManagerSkipsEmptyMetaByName` 都固定该行为。
- `FilterDataFiles` 在 metadata 没有 group 时用空字符串作为 `MetaDataGroupName`，不会像 Go 的 `m.meta.FileGroups[0]` 一样索引 panic；这是 Rust 当前的防御性差异。
- range 终点使用饱和加法，随后显式检查是否超过文件长度；事件编码不足 8 字节、长度字段越界或 value 越界统一报 `invalid buff`。
- SHA-256 不匹配、键短于 8 字节、Write CF value 太短或写类型未知均立即失败，不返回部分结果。键过短错误包含 hex 编码，便于定位损坏数据。
- `shouldReadMetaKVFile` 接受全部 Write CF 文件；Default CF 仅接受非 Delete 文件；其他 CF 返回 false。
- `Subcompactions` 遇到 walk、read 或 JSON 解码错误时返回 `Fail` 迭代器；TS 条件是区间相交，即排除 `InputMaxTs < shiftStartTS` 或 `InputMinTs > restoredTS`。
- `CountExtraSSTTotalKVs` 将迭代错误转换为模块 `Error`；计数使用 `saturating_add`，溢出时停在 `i64::MAX`。

## 并发与资源生命周期

`LogFileManager` 通过 `Arc<dyn Storage>` 共享 storage；文件统计使用原子字段，因此多个消费者可并发触发统计更新。`ReadFilteredEntriesFromFiles` 只读 manager 状态，临时缓冲、去重表和结果向量均为调用内局部状态；Rust 独立测试用四个线程和 gate 证明同一个 manager 可同时进行四次读取。

本文件本身不创建异步任务、线程或通道。惰性迭代器把实际工作推迟到消费者调用 `TryNext` 时；`LoadDDLFiles` 明确用后台 iterator context 将结果全部物化，而 `LoadDMLFiles` 把迭代生命周期交给调用方。调用者必须处理迭代过程中而非构造时才出现的错误。

`Close` 只调用 `MetadataHelper::Close`；`storage` 的生命周期由 `Arc` 引用计数管理。本类型没有 `Drop` 自动关闭，因此持有者（当前为 `LogClient`）应在自身关闭流程中显式调用。重复关闭是否幂等取决于 helper 实现，本文件没有额外状态保护。

Rust 当前 `streamingMeta` 是串行 `WalkDir + ReadFile`，没有使用 `metadataDownloadBatchSize` 控制并发；Go 的 `createMetaIterOver` 则以该值同时设置 transform buffer 和 concurrency。若补齐并发，必须保持路径级错误上下文、迭代取消、helper 线程安全和稳定的资源上限。

## 与 Go 版本的对应关系

同路径 [`log_file_manager.go`](./log_file_manager.go) 是主要语义基准。Rust 已对齐的关键点包括：三分支 TS 文件过滤；v2 metadata 使用物理 group path；DDL/meta 与 DML 分流；shift TS 取最早候选；缓存引用数规则；MetaKV range 校验和；只恢复 mDB/DDL history 范围；Write CF 跳过 Lock/Rollback；auto-ID 仅保留最高 TS；DDL history 旁路去重；按 `filterTS` 拆分；compacted/ingested SST 展平与计数；以及显式关闭 helper。

当前可见差异/未完成迁移边界：

- Go 构造 `stream.NewMetadataHelper(stream.WithEncryptionManager(...))`，Rust 固定为 `MetadataHelper`，`LogFileManagerInit::EncryptionManager` 未使用。加密 metadata/file 的真实生产可用性不能由本文件现状推断。
- Go `FastUnmarshalMetaData` 和 `createMetaIterOver` 使用 `metadataDownloadBatchSize` 并发下载；Rust 顺序扫描，字段虽保留但未生效。
- Go `LoadDDLFiles` 会先利用 tagged 文件名的 `HasDDLFiles` 缩小读取集合；Rust `LoadDDLFiles` 调用通用 `streamingMeta`，会扫描窗口内所有 metadata，再过滤内容，语义结果可一致但 I/O 成本可能更高。
- Go `ReadFilteredEntriesFromFiles` 经 helper 处理压缩和文件加密；Rust `read_file_slice` 直接 `Storage::ReadFile` 后切 range，未使用 `CompressionType`、`Length` 或 `FileEncryptionInfo`。
- Go `Subcompactions`/`LoadMigrations` 从对象存储反序列化 protobuf；Rust `Subcompactions` 读取简化 JSON，`LoadMigrations` 完全忽略 context/storage 并包装注入列表。
- Rust `decode_kv_entry` 是本地长度前缀协议，Go 使用 `stream.NewEventIterator`；两者只有在编码布局一致时等价。
- Go 的真实 helper 支持下载并发和缓存；Rust 测试 helper 的 gate 分支由 `#[cfg(test)]` 注册表接管，不应描述成生产机制。

相关 Rust 测试位于独立文件 [`log_file_manager_test.rs`](./log_file_manager_test.rs)，遵守“源文件与测试逻辑分离”；Go 对照测试位于 [`log_file_manager_test.go`](./log_file_manager_test.go)。

## 扩展指南

- 修改恢复时间窗时，优先集中调整 `ShouldFilterOutByTsStatic`、`loadShiftTS` 和 `ReadFilteredEntriesFromFiles` 的条目级规则，并同步 Rust/Go 测试中的 Write CF、Default CF、边界相等值与 tagged empty metadata 场景。文件级和条目级过滤必须保持一致。
- 接入加密/压缩读取时，应改造 `CreateLogFileManager` 的 helper 构造和 `read_file_slice`，使用 `CompressionType`、原始 `Length`、`FileEncryptionInfo`；新增独立测试验证错误密钥、压缩损坏、range 与解压后缓冲的关系，不能只让字段“被读取”。
- 接入并发 metadata 下载时，应让 `metadataDownloadBatchSize` 同时约束 buffer 和并发度，并验证 `0`、`1`、大批量、读取失败和取消。不得改变 empty tagged 文件的读取前跳过行为。
- 增加新的 MetaKV 键族时，在 `is_db_or_ddl_job_history_key`、`is_meta_ddl_job_history_key`、`is_meta_auto_id_key` 中明确分类，并判断是否允许去重、是否跨 CF 引用、Write CF 是否被消费；同步 `log_file_manager_test.rs`，不要把测试内嵌回生产文件。
- 修改 auto-ID 去重时必须保留“先剔除空值及 Lock/Rollback，再按逻辑键取最大 TS，最后按首次出现顺序输出，之后才按 `filter_ts` 拆分”的次序，否则可能让无效高版本挤掉有效提交。
- 补齐 `Subcompactions`/`LoadMigrations` 时应对齐 Go 对象存储 protobuf 格式和错误传播，而不是继续扩展临时 JSON/注入接口；兼容旧测试夹具时应区分测试适配层与生产路径。
- 调整 group 展开时要同步核查 `MetaDataGroupName` 和两个 offset 对检查点/skip map 的影响，测试至少覆盖 v1/v2 metadata、多 group、多 file、迁移重写和空 group。
- 性能风险集中在 metadata 全量扫描/物化、对象文件整读后切 range、MetaKV 去重表大小以及 DDL group 全量收集；优化前应保留校验和作用范围、错误原子性和稳定输出顺序。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件，其中 Rust 7,032 个；目标文件被索引为 90 个符号，并显示由 `client.rs`、多个测试和 restore 相邻模块使用。
- RustCodeGraph 查询/调用证据：`query CreateLogFileManager` 同时定位 Go `log_file_manager.go:148` 与 Rust `log_file_manager.rs:190`；`explore` 给出 `client.rs::InstallLogFileManager -> log_file_manager.rs::CreateLogFileManager`，并列出 `LoadDMLFiles`、`LoadDDLFiles`、`FilterDataFiles`、`FilterMetaFiles`、`GetCompactionIter`、`GetIngestedSSTs`、`ReadFilteredEntriesFromFiles` 的直接调用关系及测试调用者。
- 已完整核对 Rust 源文件 [`log_file_manager.rs`](./log_file_manager.rs) 1–819 行，特别是 `CreateLogFileManager`/`loadShiftTS`、metadata 两条过滤管线、MetaKV 读取与去重、SST 迭代、`Subcompactions` 和 `LoadMigrations`。
- crate 边界证据：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)。前者确认依赖和 library 身份，后者确认模块挂载、公开再导出及独立测试文件。
- Go 对照证据：[`log_file_manager.go`](./log_file_manager.go) 1–646 行；重点核对构造/helper、并发 metadata 下载、TS 三分支、DDL/DML 管线、MetaKV 事件读取/去重、关闭及对象存储反序列化。
- Rust 测试证据：[`log_file_manager_test.rs`](./log_file_manager_test.rs) 1–733 行，覆盖 TS 过滤、auto-ID 去重、DDL history 旁路、空 value、Write CF Put/Delete/Lock/Rollback、返回值拷贝语义、range、四线程并发读取、注入 metadata、DML/meta 分流和 empty tagged metadata。
- Go 测试证据：[`log_file_manager_test.go`](./log_file_manager_test.go)，核对 v1/v2 metadata、shift TS、多恢复窗口、真实本地 storage 枚举及同类 MetaKV 行为。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级标题，并人工检查未把上述 Rust/Go 差异描述为已完成能力。
