# `br/pkg/metautil/lib.rs`

源文件：[`lib.rs`](lib.rs)

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-metautil` 的 crate 根；`br/pkg/metautil/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确指向它。它本身不实现元数据算法，而是把 `debug.rs`、`load.rs`、`metafile.rs`、`statsfile.rs` 组成一个 BR 备份元数据库，并通过根级 `pub use` 提供与 Go `br/pkg/metautil` 接近的扁平 API。

当前 workspace 的直接 Cargo 反向依赖是 `br/pkg/config/Cargo.toml`：`br/pkg/config/ebs.rs::NewMetaFromStorage` 使用 `astersql_br_pkg_metautil::metafile::MetaFile` 作为对象存储键。其他若干 BR Rust 模块仍引用自身 `stubs::metautil`，因此“本 crate 已实现某 API”不等于“所有 BR 路径已接线到该实现”。

## 核心职责

- 用 `#[path = "..."] pub mod ...` 挂载五个生产模块：`stubs`、`debug`、`load`、`metafile`、`statsfile`。
- 从 `stubs` 显式再导出 `kvproto`、`DecodeTableID`、`JSONTable`、`Key`、`PartitionStatisticLoadTask`、`StatsReadWriter`、`StatsTypesJSONTable`，为元数据读写算法提供精简的 protobuf、统计与 table-key 边界。
- 用 `pub use debug::*`、`load::*`、`metafile::*`、`statsfile::*` 将子模块公开符号同时暴露在 crate 根；调用方可选择 `metautil::NewMetaReader` 式扁平路径，也可选择 `metautil::metafile::NewMetaReader` 式显式路径。
- 仅在 `cfg(test)` 下挂载七个独立测试模块，保证 Rust 生产实现与测试代码分文件存放。

## 主要符号

`lib.rs` 本身没有定义常量、结构体、trait、函数或 `impl`；它的符号面全部由模块声明与再导出构成。关键出口如下：

- `stubs`：`kvproto::brpb` 元数据类型、`protobuf::Message`、`DecodeTableID(Key) -> i64`、`StatsReadWriter`。`br/pkg/metautil/stubs.rs` 明确说明这是 darwin arm64 等环境下的轻量依赖边界，不是完整 kvproto/统计引擎。
- `debug`：`DecodeMetaFile` 和 `DecodeStatsFile` 读取、解密、校验元数据叶子，并写入 `jsons/<name>.json`。
- `load`：`Database` 聚合 `DBInfo` 和 `Vec<Table>`；`LoadBackupTables` 调用 `MetaReader::ReadSchemasFiles` 并按库名分组。
- `metafile`：`MetaReader`、`MetaWriter`、`Table`、`ChecksumStats`、`AppendOp` 及 `MetaFile`/`LockFile`/`MetaV1`/`MetaV2` 等协议常量，承担 v1/v2 读写、加解密、索引叶遍历、批处理和兼容性检查。
- `statsfile`：`StatsWriter::BackupStats`/`BackupStatsDone`、`RestoreStats`、`downloadStats` 以及可原子调整的 `maxStatsJsonTableSize`/`inlineSize`，承担统计 JSON 的分块、内联/落盘和恢复时 ID 重写。

crate 根还用 `#![allow(...)]` 允许 Go 风格的命名与迁移期未使用符号；这是全 crate 生效的 lint 政策，不是运行时分支。

## 执行流程

`lib.rs` 在运行时不主动执行代码；它决定编译时模块图和 API 可见性。典型读取主链是：

1. 上层持有 `BackupMeta`、`Storage` 和可选 `CipherInfo`，调用 `NewMetaReader`。
2. `MetaReader::ReadSchemasFiles` 对 v1 内联内容与 v2 `MetaFile` 索引叶进行统一遍历，可通过 `SkipFiles`/`SkipStats` 裁剪工作。它解析 schema，依据 SST `start_key` 的 physical table ID 把文件挂到表/分区。
3. `LoadBackupTables` 在后台线程执行上述读取，主线程处理取消、错误和表通道，最终产生按 `DBInfo.Name.O` 索引的 `HashMap<String, Database>`。
4. 如需恢复统计，`RestoreStats` 并行运行下载生产者和 `StatsReadWriter::LoadStatsFromJSONConcurrently`，下载端解密、校验并将旧 physical ID 重写为新 ID。

写入主链是 `NewMetaWriter` 初始化版本/存储/密码边界，`StartWriteMetasAsync` 与 `Send` 收集分类项，`FinishWriteMetas` 结束后由 v1 内联或 v2 叶子落盘路径整理结果，最后 `FlushBackupMeta` 写出根 `backupmeta`。`StatsWriter` 作为该写链的统计侧路，返回要挂入 schema 的 `StatsFileIndex`。

## 数据与状态

- crate 根不持有全局业务状态。`MetaReader` 持有可克隆的 `BackupMeta`、`Arc<dyn Storage + Send + Sync>` 和可选密码信息；`MetaWriter` 使用受锁写状态、通道和后台线程。
- `Table` 同时携带数据库/表定义、校验和计数、按 physical ID 分组的文件、TiFlash 副本数、可选统计和统计文件索引。`Database` 另有非公开 `reusedByPITR` 标志，默认为 `false`。
- `StatsWriter` 暂存 `StatsFile` 块、累计序列化字节数和已生成索引。第一个足够小的 payload 可内联；超过 `maxStatsJsonTableSize` 时落盘，索引记录名称、SHA-256、加密前后大小和 IV。
- `maxStatsJsonTableSize` 和 `inlineSize` 是 `AtomicUsize`，测试会修改它们以强制分支；`statsfile_test_support.rs::StatsConfigTestGuard` 用于串行化并恢复这两个全局阈值。

## 依赖与调用关系

`Cargo.toml` 把该 crate 连到 BR 的 `errors`、`logutil`、`summary`、`utils` 以及全局 `meta-model`、`objstore-storeapi`、`util`、`util-encrypt`、`errors`；`serde`/`serde_json` 用于 Go 兼容 JSON，`bytesize` 给出元文件大小阈值，`tracing` 用于观测。测试额外依赖内存对象存储、parser AST 和 testsetup。Cargo 没有定义 feature 开关；生产/测试差异由 `cfg(test)` 控制。

内部调用方向为 `load -> metafile`、`debug -> metafile + stubs`、`metafile -> statsfile + stubs`、`statsfile -> metafile + stubs`。`metafile` 与 `statsfile` 之间是有意的交叉：元数据写入器创建统计写入器，统计文件又复用元文件的加密和摘要工具。

RustCodeGraph 对 `lib.rs` 报告 1 个符号，并显示 `metafile.rs` 被 45 个已索引文件使用。对精确 Rust 节点 `LoadBackupTables`、`NewMetaReader`、`DecodeMetaFile`、`RestoreStats`、`NewMetaWriter` 执行 `callers/callees` 未返回边，所以本文不用该结果断言“无调用者”，而以 Cargo 反向依赖和精确源码引用补证。

## 错误处理与边界

- crate 根没有自己的错误类型。子模块主要返回 `SharedError`，并用 `Trace`、BR 分类错误或带语境的 I/O 错误保留故障位置。
- 元数据和统计外部文件在解密后验证 SHA-256；不匹配返回 `ErrInvalidMetaFile`。统计恢复找不到 physical ID rewrite 规则时返回 `ErrRestoreInvalidRewrite`。
- `DecodeMetaFile(None)` 是成功空操作；但子元文件仍包含更深 `meta_files` 时会拒绝，防止静默丢层。`ReadSchemasFiles` 遇到无法解出 table ID 的数据键（ID 0）会 panic，`Database::GetTable` 遇到缺少 `Table.Info` 也会 panic；这些是当前与 Go 失效语义对齐的不变量，不应在无兼容性评估时改成静默跳过。
- `Context` 取消在 schema、meta 解码和 stats 下载路径上会中止工作；线程 panic 在 `RestoreStats` 中被转换为 `downloadStats worker panicked`，而其他使用 `join`/`expect` 的内部不变量仍可观察为 panic。

## 并发与资源生命周期

`MetaReader::ReadSchemasFiles` 分别启动 schema 读取线程、8 个 schema 解析 worker、调度线程，并在未 `SkipFiles` 时启动文件读取线程。通道 sender 的 drop 是结束信号；读端在通道断开、错误或取消后停止。`LoadBackupTables` 再用单独后台线程包装该过程，以 10 ms `recv_timeout` 轮询取消与异步错误。

`MetaWriter` 的异步写入通过 channel、`Mutex` 状态和 `JoinHandle` 管理；`FinishWriteMetas` 必须在对应 `AppendOp` 后收口，`MetaWriterDoneGuard` 在 drop 时通知 `WaitGroup`，避免异常返回丢失完成信号。对象存储以 `Arc<dyn Storage + Send + Sync>` 跨线程共享。

`RestoreStats` 使用容量 8 的 `sync_channel` 对下载产生者施加背压；`downloadStats` 使用 4 个 worker，共享的首错槽只保留第一个错误，等待已启动任务后 drop sender，使统计加载端通过 EOF 结束。

## 与 Go 版本的对应关系

`Cargo.toml` 的 `package.metadata.porting.go-package = "br/pkg/metautil"` 给出了明确的包级对应。Rust 生产子模块与 Go 文件直接对应：`debug.rs` ↔ `debug.go`，`load.rs` ↔ `load.go`，`metafile.rs` ↔ `metafile.go`，`statsfile.rs` ↔ `statsfile.go`。Rust 的 crate 根以再导出模拟 Go 同包符号的自然可见性。

主要语义对齐包括：`MetaV1 = 0`/`MetaV2 = 1`，`backup.lock` 与 `backupmeta` 对象键，v1 内联与 v2 索引叶格式，`SkipFiles`/`SkipStats` 函数式选项，`backupmeta.schema.stats.%09d` 命名，统计下载 4 worker/任务通道容量 8，以及加密后先解密再验证明文 SHA-256。

差异与迁移边界必须保留在视野中：Rust 在 `stubs.rs` 中使用 serde/JSON 形状的轻量 `Message` 和统计投影，Cargo 注释也明确未拉入完整 kvproto/grpcio/statistics-handle/tablecodec。因此这些边界不应被宣称为与 Go 生产 protobuf 栈完全等价；当前可证明的是本 crate 内部契约和已接线调用点。

## 扩展指南

- 新增生产模块时，在 `lib.rs` 中用明确 `#[path] pub mod` 挂载；只有需要保持 Go 包级 API 的公开符号才应根级再导出，并检查与其他 glob re-export 的名字冲突。
- 修改 v1/v2 格式、加密、索引校验或写入序列时，主要接入点在 `metafile.rs` 的 `MetaReader`、`MetaWriter`、`walkLeafMetaFile`、`Encrypt`、`CheckBackupMetaCompatibilityFromBytes`；需同步独立 `metafile_test.rs` 和 Go `metafile_test.go`。
- 修改 schema/文件归属时，聚焦 `MetaReader::ReadSchemasFiles`、`parseSchemaFile`、`LoadBackupTables`，并同步 `load_test.rs`/`load_test.go`，覆盖普通表、分区表、取消、`SkipStats` 和非法 table ID。
- 修改统计文件格式或并发时，聚焦 `StatsWriter`、`RestoreStats`、`downloadStats`，并同步 `statsfile_test.rs`、`parity_test.rs` 及 Go `statsfile_test.go`；必须重验 inline/落盘阈值、密码类型、全字段 JSON 往返、rewrite 缺失和通道关闭。
- 修改调试旁路输出时，同步 `debug_test.rs`/`debug_test.go`，验证对象名、JSON 字段、多层索引拒绝、解密与校验顺序。
- 若目标是让更多 BR Rust 包使用真实 crate，应逐个替换调用方的本地 `stubs::metautil` 并在其 `Cargo.toml` 添加路径依赖；这是跨 crate 迁移任务，不应只通过扩大 `lib.rs` 的再导出来假装完成。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7032 个 Rust 文件；`files --filter br/pkg/metautil` 列出 22 个相关 Rust/Go 文件；`node --file br/pkg/metautil/lib.rs` 确认根文件的 5 个生产模块、7 个 `cfg(test)` 模块和全部再导出。
- RustCodeGraph 符号查询：`query` 精确定位 Rust/Go 两侧的 `LoadBackupTables`、`NewMetaReader`、`DecodeMetaFile`、`RestoreStats`、`NewMetaWriter`；对相应 Rust 节点执行了 `callers/callees`，但本索引未输出边，故改用下述源码/Cargo 证据确定连线。
- Rust 源码：`br/pkg/metautil/lib.rs`、`stubs.rs`、`debug.rs`、`load.rs`、`metafile.rs`、`statsfile.rs`；Cargo 边界：`br/pkg/metautil/Cargo.toml`、`br/pkg/config/Cargo.toml`；直接外部使用点：`br/pkg/config/ebs.rs::NewMetaFromStorage`。
- Go 对照：`br/pkg/metautil/debug.go`、`load.go`、`metafile.go`、`statsfile.go`。Rust 独立测试：`debug_test.rs`、`load_test.rs`、`metafile_test.rs`、`statsfile_test.rs`、`parity_test.rs`、`main_test.rs`、`statsfile_test_support.rs`；Go 对照测试：同目录 `*_test.go`。
- 测试证据覆盖：普通/分区表文件归属与取消（`load_test.rs`），叶节点、无效校验和兼容性（`metafile_test.rs`），明文与 AES-CTR、落盘/rewrite/JSON 完整往返（`statsfile_test.rs`、`parity_test.rs`），以及 meta/stats JSON 旁路产物（`debug_test.rs`）。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用任务规定的 11 章结构命令、`git diff --check` 与仅目标文档的 diff 自审；不宣称运行时测试结果。
