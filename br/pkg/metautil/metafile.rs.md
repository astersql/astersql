# `br/pkg/metautil/metafile.rs`

## 文件定位

该文件是 `astersql-br-pkg-metautil` crate 的备份元数据核心实现，由 [`br/pkg/metautil/lib.rs`](lib.rs) 以 `pub mod metafile` 挂载并在 crate 根再导出。它位于 BR 的备份写入与恢复读取之间：写端把文件、schema 和 DDL 组织成 `BackupMeta` 的 v1 内嵌布局或 v2 索引分片布局；读端反向遍历这些布局，恢复为表级 `Table` 数据；兼容性入口还在恢复解析后阻止旧程序静默忽略新 protobuf 语义。

crate 边界由 [`br/pkg/metautil/Cargo.toml`](Cargo.toml) 定义。该 crate 直接依赖 BR 错误、日志、汇总、加密工具，以及 meta model、对象存储接口、通用工具和加密实现；`brpb`、`CipherInfo`、`JSONTable`、`DecodeTableID` 等当前来自同 crate 的 [`stubs.rs`](stubs.rs)，这是 Cargo 注释所述的 darwin arm64 精简依赖方案，而不是完整 kvproto 直接依赖。

## 核心职责

1. 维护备份元数据公开契约常量：`LockFile`、`MetaFile`、`MetaJSONFile`、`MaxBatchSize`、`MetaFileSize`、`CrypterIvLen`、`MetaV1`、`MetaV2` 和 `BACKUP_SCHEMA_VERSION`。
2. 用 `Encrypt`、`DecryptFullBackupMetaIfNeeded` 处理明文与 AES-CTR 元数据；顶层 `backupmeta` 使用“IV 前缀 + 密文”，v2 子分片则把 IV 放在索引节点的 `cipher_iv` 中。
3. 用 `walkLeafMetaFile` 下载、解密、校验并解析 v2 `MetaFile` 索引树的叶子。
4. 用 `MetaReader` 同时读取 v1 内嵌数据和 v2 索引数据，并通过 `ReadSchemasFiles` 将 schema、统计信息和物理数据文件聚合成 `Table`。
5. 用 `MetaWriter` 异步接收 `MetaItem`，按 `AppendOp` 写入 v1 或切分、加密、落盘并索引为 v2，最后由 `FlushBackupMeta` 写出顶层对象。
6. 用字节级 protobuf wire 扫描检查顶层和嵌套未知字段，并结合 `backup_schema_version` 执行向后兼容保护。
7. 提供归档大小、校验统计、DDL 片段合并和 SHA-256 等辅助能力。

## 主要符号

- `CheckBackupMetaCompatibilityFromBytes(&[u8], &BackupMeta) -> Result<()>`：先拒绝高于 `BACKUP_SCHEMA_VERSION` 的元数据，再调用 `checkBackupMetaUnknownFieldsFromBytes` 扫描原始 wire bytes。扫描必须使用原始字节，因为普通反序列化可能丢失未知字段。
- `Encrypt(Vec<u8>, Option<&CipherInfo>) -> Result<(Vec<u8>, Vec<u8>)>`：`None`、空内容和 `Plaintext` 原样返回；AES-128/192/256-CTR 从 `/dev/urandom` 取得 16 字节 IV；未知算法返回 `ErrInvalidArgument`。
- `DecryptFullBackupMetaIfNeeded`：仅对有效加密方法拆出前 16 字节 IV；Rust 额外检查载荷长度，过短时返回参数错误，避免 Go 当前切片路径可能发生的越界。
- `walkLeafMetaFile` / `walkLeafMetaFileDyn`：`None` 成功返回，无子节点的对象直接回调；有子节点时并行 `ReadFile`，随后解密、对明文计算 SHA-256、解析 `MetaFile` 并递归。
- `Table`：聚合 `DBInfo`、可选 `TableInfo`、schema 校验统计、按物理表/分区 ID 分组的 `FilesOfPhysicals`、TiFlash 副本、JSON 统计和 `StatsFileIndex` 等恢复所需状态。
- `MetaReader` / `NewMetaReader`：持有共享 `Storage`、`BackupMeta` 和可选 cipher。公开读取面是 `ReadDDLs`、`ReadSchemasFiles`、`GetBasic`。
- `ArchiveSize`、`ArchiveTablesSize`、`ArchiveTableSize`：只汇总数据文件 `File.size`；`ChecksumStats::ChecksumExists` 与 `Table::CalculateChecksumStatsOnFiles` 分别判断和聚合 CRC64 XOR、KV 数、字节数。
- `ReadSchemaOption`、`SkipFiles`、`SkipStats`：函数式配置。`SkipFiles` 不构建文件映射；`SkipStats` 在 schema 进入解析队列前清除内嵌 stats 与 stats index。
- `parseSchemaFile`：解析数据库、可选表和可选统计 JSON，并复制 schema 的统计、TiFlash 和 merge-option 字段到 `Table`。
- `AppendOp` 与 `MetaItem`：操作类别和类型化载荷。`AppendOp::appendFile` 要求二者变体匹配，否则 panic，属于调用方编程错误。
- `sizedMetaFile` / `NewSizedMetaFile`：记录当前分片 protobuf 估算大小、数据文件大小和条数；追加后仅当 `size > sizeLimit` 才要求 flush。
- `MetaWriter` / `NewMetaWriter`：核心写入器。构造时把 DDL 初始化为 `[]` 并设置 schema 版本；公开面包括 `Update`、`Send`、`StartWriteMetasAsync`、`FinishWriteMetas`、`FlushBackupMeta`、大小查询、`Backupmeta` 和 `NewStatsWriter`。
- `detect_unknown_protobuf_fields`、`protobuf_field_map`、私有 `protowire` 模块：最小 wire 解析器和已知字段表，递归检查 bytes 类型的嵌套 message；支持 varint、fixed32/64、bytes 和旧式 group。

## 执行流程

读取流程如下。

1. 上层先取得并解密顶层 `BackupMeta`，用 `NewMetaReader` 绑定对象存储和 cipher。
2. `ReadDDLs` 对 v1 直接返回内嵌 DDL；对 v2 遍历 `ddl_indexes` 叶子并用 `mergeDDLs` 合成 JSON 数组。
3. `ReadSchemasFiles` 启动 schema 读取线程、8 个 schema 解析 worker 和一个任务分发线程。`readSchemas` 总是先发送顶层内嵌 schema，再遍历 `schema_index`。
4. 未启用 `SkipFiles` 时，另一个线程通过 `readDataFiles` 收集内嵌文件与 `file_index` 叶子文件。主线程先把全部文件按 `DecodeTableID(start_key)` 得到的物理 ID 放入内存 map，随后才处理表，保证表与分区文件归属完整。
5. `receiveBatch` 最多收集 `MaxBatchSize` 条；50ms 超时时只要已有数据就提前交付。每张表按表 ID 关联自身及分区定义的文件，库级占位则按数据库 ID 入批，最后逐表调用用户回调。
6. [`br/pkg/metautil/load.rs`](load.rs) 的 `LoadBackupTables` 是直接消费者：它调用 `ReadSchemasFiles`，可按需传入 `SkipStats`，再按数据库名聚合结果。

写入流程如下。

1. `NewMetaWriter` 建立共享状态；调用方可用 `Update` 填写顶层版本、集群等字段。
2. 每种 `AppendOp` 开始前调用 `StartWriteMetasAsync`。它重置当前分片和计数，建立 item/error channel，并启动一个消费线程；操作类型由本次 Start 捕获，`Send` 的 `_op` 参数不参与分派。
3. 消费线程把每个 `MetaItem` 追加到 `sizedMetaFile`。v1 只累计到 Finish；v2 超过体积限制时立即调用 `flush_metas_v2`。
4. v2 flush 序列化当前 `MetaFile`，加密并以 `backupmeta.<type>.<9 位序号>` 写入对象存储；索引记录明文 SHA-256、明文大小和 cipher IV，然后挂到 `schema_index`、`file_index` 或 `ddl_indexes` 并重置分片。
5. `FinishWriteMetas` 关闭发送端，等待并 join 消费线程，传播异步错误；随后 v1 把分片搬入顶层字段，v2 刷出残余分片。AppendDataFile 还会上报成功单元。
6. `FlushBackupMeta` 固化 v1/v2 版本和 schema 版本，计算数据文件、元数据分片与顶层 protobuf 的总大小，序列化并按“IV + 密文”布局写到配置名称（空名称默认 `backupmeta`）。
7. [`br/pkg/backup/schema.rs`](../backup/schema.rs) 的 `Schemas::BackupSchemas` 直接按 `StartWriteMetasAsync(AppendSchema)`、有序 `Send`、`FinishWriteMetas` 驱动这一生命周期。

## 数据与状态

`MetaReader` 是可克隆的只读句柄：`Storage` 用 `Arc` 共享，`BackupMeta` 与 cipher 在 clone 时复制。`GetBasic` 返回 `BackupMeta` 克隆，避免把内部状态借出。

`MetaWriterState` 受 `Arc<Mutex<_>>` 保护，包含布局选择、顶层 `BackupMeta`、当前 `sizedMetaFile`、各类型大小、共享序号、起始时间、已刷条数、目标文件名、cipher 以及数据/元数据累计大小。`metafile_seq_num` 当前以固定键 `metafiles` 生成跨操作共享序号；`metafile_sizes` 按操作名累计估算大小，但当前没有公开读取入口。`total_meta_file_size` 计入 v2 分片密文长度和 schema 引用的 stats `size_enc`，`total_data_file_size` 计入 SST 的 `File.size`。

v1 把 files、schemas、DDL 直接放在 `BackupMeta`；v2 顶层只保留 `MetaFile` 索引根。索引节点的 `size` 和 `sha256` 针对加密前明文，存储对象是密文；读端必须先解密再校验。顶层 `backupmeta` 的 IV 是载荷前缀，分片 IV 则独立存在索引字段中，两种格式不可混用。

`Table::FilesOfPhysicals` 的键是逻辑表 ID 或分区 ID。`ReadSchemasFiles` 为完整归属先把全部文件留在内存中，这是与 Go 现有结构一致的明确内存/时间折中，大备份下会形成与文件总数成正比的内存峰值。

## 依赖与调用关系

上游直接证据包括：

- [`br/pkg/metautil/load.rs`](load.rs) 调用 `MetaReader::ReadSchemasFiles` 构建数据库到表的映射。
- [`br/pkg/backup/schema.rs`](../backup/schema.rs) 通过 writer trait 使用 `StartWriteMetasAsync`、`Send`、`NewStatsWriter`、`FinishWriteMetas`，把备份 schema 和统计索引写入本模块定义的布局。
- [`br/pkg/restore/log_client/id_map.rs`](../restore/log_client/id_map.rs) 在解析备份元数据后调用 `CheckBackupMetaCompatibilityFromBytes`；严格模式返回错误，关闭 requirements 检查时只告警。
- crate 根的再导出使调用者通常从 `astersql_br_pkg_metautil` 使用这些符号，而不必显式引用子模块。

下游依赖包括：`Storage::ReadFile/WriteFile` 提供对象存储 I/O；`Decrypt`、`IsEffectiveEncryptionMethod`、`AESEncryptWithCTR` 提供密码能力；`DBInfo`/`TableInfo` 承载 schema JSON；`DecodeTableID` 从文件 start key 取物理 ID；`CollectSuccessUnit` 上报数据文件写入；`statsfile::newStatsWriter` 复用同一 storage/cipher 写统计；protobuf `Message` 完成分片和顶层元数据序列化。

RustCodeGraph 的文件节点报告 `metafile.rs` 被 45 个文件使用，并列出 `br/pkg/metautil/load.rs`、`debug.rs` 等候选。精确 `callers` 命令在本次检查的 30 秒窗口内未返回，因此上述调用边均以仓库直接调用点复核，不将超时查询视为额外证据。

## 错误处理与边界

- 兼容性检查的输入字节不能为空；更高 schema 版本、任意层级未知字段或 wire 解析失败都会返回错误。字段表必须与 protobuf 定义同步，否则漏登记会误报不兼容，错误登记嵌套类型则可能漏检。
- `walkLeafMetaFile` 在 storage、解密、校验或反序列化失败时停止并传播错误；子线程 panic 被转换为 `walkLeafMetaFile worker panicked`。`None` 根不回调，空 `MetaFile` 被视为一个叶子。
- 文件 `start_key` 解码得到 0 时直接 panic，这是数据不变量而非可恢复输入错误。`AppendOp` 与 `MetaItem` 不匹配、v1 收到不支持的操作也 panic。
- `Send` 在未 Start 时返回 `NotConnected`，发送端关闭时返回 `BrokenPipe`，并在投递前尝试取得消费线程错误。调用者必须遵守 Start → Send → Finish 顺序。
- `Context` 取消通过各线程中的轮询和接收循环传播，返回 `Interrupted`；它不是抢占式取消，正在执行的存储调用需由 storage/context 自身响应。
- `random_iv` 无法读取 `/dev/urandom` 时会 panic；这是当前 Rust 实现与 Go `crypto/rand.Read` 返回普通错误的差异。
- `mergeDDLs` 的空输入会在写 `b[0]` 时 panic；正常构造路径以 `[]` 初始化 DDL，并只在存在受支持写入流程时调用。扩展调用者不应直接把空向量传给它，除非先修复并补独立测试。
- 同长度错误 AES 密钥可能产生无错误的乱码；分片路径依赖 SHA-256 发现错误，顶层载荷需由后续 protobuf 解析/语义校验发现。

## 并发与资源生命周期

`walkLeafMetaFileDyn` 用 `thread::scope` 为当前节点的每个子索引并行下载和解码；scope 在返回前 join 全部子线程，然后主线程按句柄收集顺序递归并执行可变回调，因此回调本身不并发。Go 版本用容量为 8 的 worker pool 和 errgroup，并允许叶子处理出现在 worker 中；Rust 当前没有并发上限，宽索引节点会创建与子节点数量相同的线程，这是扩展树形或提高 fan-out 时的重要性能风险。

`ReadSchemasFiles` 使用多个无界 `std::sync::mpsc` channel、8 个解析 worker、schema 读取线程、分发线程以及可选文件读取线程。共享 receiver 由 `Mutex` 串行执行 `recv`，解析工作在锁外并行；`parse_err` 只保留首个错误。函数返回后没有显式 join 这些 detached 线程，它们依赖发送端/接收端关闭或 context 取消退出；修改通道拓扑时必须验证错误端和数据端都能关闭，否则可能泄漏线程或永久等待。

`MetaWriter` 的消费线程由 `consumer: JoinHandle` 保存，`WaitGroup::Add(1)` 与 `MetaWriterDoneGuard::drop -> Done()` 配对。`FinishWriteMetas` 按 close sender → Wait → join → 检查异步错误 → 最终 flush 的顺序释放资源。writer 的状态锁覆盖序列化和 `Storage::WriteFile`，因此 `Update`、查询和其他写操作在远端 I/O 期间会被阻塞；不要在 `Update` 闭包内重入 writer。

## 与 Go 版本的对应关系

对应实现是 [`br/pkg/metautil/metafile.go`](metafile.go)，符号和主要数据流近似一一对应：常量、`Table`、`MetaReader`、函数式选项、`AppendOp`、`sizedMetaFile`、`MetaWriter`、v1/v2 flush、DDL 合并和归档/校验统计均保持同名或等价命名。Rust 独立测试 [`metafile_test.rs`](metafile_test.rs) 明确对照 [`metafile_test.go`](metafile_test.go)，另有 [`parity_test.rs`](parity_test.rs) 检查公开常量、SHA-256、明文和非法 cipher 契约。

需要注意的实现差异：

- Go 使用真实 protobuf 反射构造未知字段信息；Rust 用 `ProtobufMessageKind` 和 `protobuf_field_map` 手工登记字段号与嵌套类型。行为目标一致，但 Rust 在 protobuf 演进时多一个必须人工同步的维护点。
- Go `ReadSchemasFiles` 通过输出 channel 返回表，Rust 改为同步 `FnMut(Table) -> Result<()>` 回调；Rust 因此可以直接传播回调错误。
- Go 的元数据 channel 带 `MaxBatchSize` 缓冲，Rust 当前 channel 无界；Rust `receiveBatch` 还增加 50ms 部分批次交付语义，而 Go 主要按满批或 channel 关闭返回。
- Go 的叶子下载受 8-worker pool 限制，Rust scoped thread 未限并发；Go 回调需线程安全，Rust 回调在收集阶段串行执行。
- Rust 对加密顶层载荷增加长度检查，对 writer 的未启动/通道关闭增加显式 I/O 错误；另一方面 Rust 随机源读取失败会 panic。
- Cargo 明确说明当前 `brpb`/JSON/table-ID 能力使用本地 stubs 精简。文档中的行为是当前 Rust crate 的真实行为，不应推断为完整 kvproto/grpcio 依赖已经接线。

## 扩展指南

- 新增或改变 `BackupMeta`、`MetaFile`、`Schema` 等 protobuf 字段时，必须同步 `ProtobufMessageKind` 与 `protobuf_field_map`，确认嵌套消息的 `is_message/nested` 正确，并在 Rust/Go 独立测试中增加顶层、嵌套和深层未知字段用例；如语义不兼容，还要递增 `BACKUP_SCHEMA_VERSION` 并保持 Go 常量一致。
- 新增元数据载荷类型时，需要同时评估 `AppendOp`、`MetaItem`、`appendFile`、`fill_metas_v1`、`flush_metas_v2`、索引字段、文件命名、读取入口和大小统计，不能只加写端枚举。`Send` 的 op 参数当前不控制行为，Start 与 Finish 的 op 必须一致。
- 修改加密布局时必须区分顶层 IV 前缀和分片索引 IV，并同步 `Encrypt`、`DecryptFullBackupMetaIfNeeded`、`walkLeafMetaFileDyn`、checksum 计算顺序及对应测试；兼容旧备份是首要风险。
- 调整 `ReadSchemasFiles` 并发时，应重点测试取消、storage 错误、解析错误、回调错误、channel 关闭、尾批交付、分区文件归属与大文件集内存峰值。Rust 测试逻辑应继续放在独立 [`metafile_test.rs`](metafile_test.rs)，不要内嵌到生产文件。
- 调整分片阈值或大小计算时，同步验证“恰好等于限制不 flush、严格超过才 flush”、明文索引 size、密文累计 size、stats `size_enc` 和最终 `backup_size`。
- 优化树遍历时可考虑与 Go 一样设置并发上限，但须保持叶子完整性校验、错误短路与输出语义，并新增宽树、取消和多级树测试。
- `mergeDDLs`、`random_iv` 和 physical table ID 当前以 panic 表达部分边界；若改为可恢复错误，需要同时调整公开签名、上游错误传播和 Go 对齐判断，不能仅在 Rust 中静默吞错。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/metautil` 列出目标、Go 对照和独立测试；`node --file br/pkg/metautil/metafile.rs` 分段读取了全部 1973 行，并报告该文件被 45 个文件使用。`query NewMetaWriter`、`query CheckBackupMetaCompatibilityFromBytes` 同时定位到 Rust/Go 实现和对应 Go 测试；精确 `callers` 查询超时，调用边随后由直接搜索复核。
- 源与边界：[`br/pkg/metautil/metafile.rs`](metafile.rs)、[`br/pkg/metautil/Cargo.toml`](Cargo.toml)、[`br/pkg/metautil/lib.rs`](lib.rs)。该目录没有 `doc.go`，包职责以 crate 根文档、Cargo metadata 和代码为准。
- 直接调用证据：[`br/pkg/metautil/load.rs`](load.rs)、[`br/pkg/backup/schema.rs`](../backup/schema.rs)、[`br/pkg/restore/log_client/id_map.rs`](../restore/log_client/id_map.rs)。
- Go 对照：[`br/pkg/metautil/metafile.go`](metafile.go)，重点核对兼容性、加密、叶子遍历、schema 批读取和 `MetaWriter` 的同名实现。
- 测试证据：[`br/pkg/metautil/metafile_test.rs`](metafile_test.rs) 覆盖空/叶/错误校验和/多级索引树、全部 cipher 类型、分片边界、schema 版本和多层未知字段；[`br/pkg/metautil/parity_test.rs`](parity_test.rs) 覆盖公开契约；[`br/pkg/metautil/metafile_test.go`](metafile_test.go) 提供对应 Go 意图。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅执行固定 11 章节结构检查、链接/路径检查和 diff 自审。
