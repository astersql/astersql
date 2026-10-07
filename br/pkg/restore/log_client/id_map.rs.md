# `br/pkg/restore/log_client/id_map.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-log-client` library crate；crate 根由 [`br/pkg/restore/log_client/Cargo.toml`](Cargo.toml) 的 `path = "lib.rs"` 指定，[`lib.rs`](lib.rs) 以 `pub mod id_map` 挂载本模块，并只把 `PITRIdMapBlockSize` 与 `PitrIDMapsFilename` 扁平再导出。其余函数都是 `LogClient` 的固有方法，供同 crate 的日志恢复编排代码调用。

它位于 PiTR 日志恢复的 ID 重写链路中：把 `TableMappingManager` 中“上游库表 ID → 下游恢复后 ID”的映射持久化，并在恢复续跑时重新加载。直接写入口是 [`client.rs`](client.rs) 的 `LogClient::SaveIdMapWithFailPoints`，直接读入口是同文件的 `LogClient::GetBaseIDMapAndMerge`；后者加载后调用 `TableMappingManager::MergeBaseDBReplace` 合并基础映射。

本目录没有 `doc.go`。包边界由 `Cargo.toml`、`lib.rs` 和 Go 同路径 [`id_map.go`](id_map.go) 共同核验。

## 核心职责

1. 用 `PitrIDMapsFilename(clusterID, restoredTS)` 生成绑定上游集群与恢复时间戳的对象名，避免不同恢复窗口共享同一外部存储文件。
2. 用 `saveIDMap`/`loadSchemasMap` 实现对称的介质选择：启用检查点且检查点管理器提供存储时优先使用该存储；否则若 `mysql.tidb_pitr_id_map` 存在则使用系统表；再否则回退到 `LogClient::storage`。
3. 在外部存储路径中将 `PitrDBMap` 包装为 `BackupMeta` 后整体写入或读回；在系统表路径中按 512 KiB 分段写入，并按连续 `segment_id` 拼接读取。
4. 兼容有、无 `restore_id` 列的两代系统表 schema；新 schema 用 `restoreID` 隔离恢复任务，旧 schema 只能以 `(restored_ts, upstream_cluster_id)` 区分映射。
5. 反序列化后执行备份元数据兼容性检查；`checkRequirements` 为真时拒绝不兼容元数据，为假时仅告警。
6. 仅在映射写成功后调用 `SaveCheckpointProgress(InLogRestoreAndIdMapPersisted)`，确保检查点不会先于映射持久化推进。

## 主要符号

- `PITRIdMapBlockSize: usize = 524_288`：系统表单段上限，与 Go 的 `PITRIdMapBlockSize` 一致；也用于读取时预估拼接缓冲区容量。
- `PitrIDMapsFilename(clusterID, restoredTS) -> String`：公开文件名协议，格式为 `pitr_id_maps/pitr_id_map.cluster_id:<cluster>.restored_ts:<ts>`。
- `LogClient::pitrIDMapTableExists() -> bool`：通过 `dom.InfoSchema().TableExists` 探测系统表；Rust 中 `dom == None` 时返回 `false`。
- `LogClient::pitrIDMapHasRestoreIDColumn() -> bool`：委托 `restore_misc::HasRestoreIDColumn` 探测 schema；`dom == None` 时返回 `false`。
- `LogClient::tryGetCheckpointStorage(...) -> Option<Arc<dyn Storage>>`：只有 `useCheckpoint` 为真时才返回管理器持有的存储。
- `LogClient::saveIDMap(...) -> Result<()>`：写路径总入口，先 `TableMappingManager::ToProto`，再选择介质，最后按条件推进检查点。
- `LogClient::saveIDMap2Storage(...)`：构造带 `ClusterId`、`DbMaps`、`BackupSchemaVersion` 的 `BackupMeta`，调用 `Marshal` 后执行 `Storage::WriteFile`。
- `LogClient::saveIDMap2Table(...)`：先删除同逻辑键旧段，再把序列化 `BackupMeta` 逐段 `REPLACE` 到系统表。
- `LogClient::loadSchemasMap(...) -> Result<Vec<PitrDBMap>>`：读路径总入口，使用与写路径相同的介质优先级。
- `LogClient::loadPITRIDMapBackupMeta(...)`：统一反序列化与兼容性检查入口；还处理 Rust stub 的 `BM` 简易编码。
- `LogClient::loadSchemasMapFromStorage(...)`：检查文件存在性、读取文件并返回其中的 `DbMaps`。
- `LogClient::loadSchemasMapFromTable(...)`：按 `segment_id` 排序查询、校验段连续且非空、拼接后统一反序列化。

文件没有 trait、enum、独立 struct 或条件编译项。除两个常量/函数经 crate 根再导出外，方法虽声明为 `pub`，当前实际接线仍局限于 crate 内部的 `LogClient` 编排与测试导出层。

## 执行流程

写入流程以 `saveIDMap` 为中心：

1. `manager.ToProto()` 生成 `Vec<PitrDBMap>`。
2. `tryGetCheckpointStorage` 若返回存储，调用 `saveIDMap2Storage`；否则检查 `pitrIDMapTableExists`，存在时调用 `saveIDMap2Table`；表不存在时要求 `self.storage` 非空并写入该备份存储。
3. 外部存储分支用 `GetClusterID(ctx)` 与 `self.restoreTS` 生成文件名，把映射封装进 `BackupMeta`，序列化后整文件覆盖。
4. 系统表分支先序列化整个 `BackupMeta`。若有 `restore_id` 列，删除 `(restore_id, restored_ts, upstream_cluster_id)` 对应旧行；否则删除 `(restored_ts, upstream_cluster_id)` 对应旧行。随后从 `segment_id = 0` 起，以 `PITRIdMapBlockSize` 切片并逐段执行 `REPLACE`。
5. 只有介质写入全部成功且 `useCheckpoint` 为真，才把检查点进度写为 `InLogRestoreAndIdMapPersisted`。

读取流程以 `loadSchemasMap` 为中心：

1. 按“检查点存储 → 系统表 → `self.storage`”选择来源，选择规则与写路径相同。
2. 存储分支先 `FileExists`；文件不存在返回空向量，存在则 `ReadFile`，再交给 `loadPITRIDMapBackupMeta`。
3. 表分支按 schema 选择查询键并 `ORDER BY segment_id`，以 `kv::WithInternalSourceType(..., InternalTxnBR)` 标记内部查询。无行时返回空向量；有行时要求行号与 `segment_id` 严格等于 `0..n-1` 且每段非空，然后拼接字节。
4. `loadPITRIDMapBackupMeta` 调用 `BackupMeta::Unmarshal`。对本地 stub 的 `BM` 格式，它还按 `magic + cluster_id + count + (name_len, name)*` 重建 `DbMaps`；之后执行兼容性检查并返回 `BackupMeta`。
5. `GetBaseIDMapAndMerge` 取得结果后调用 `MergeBaseDBReplace`，使后续日志重写沿用此前恢复阶段的 ID 对应关系。

## 数据与状态

本文件不定义持久状态，所有状态来自 `LogClient`、入参和外部介质：

- `LogClient::restoreTS` 决定写入键与外部文件名；读取可显式传入 `restoredTS`。
- `upstreamClusterID` 参与系统表逻辑键；外部路径的 cluster ID 则由 `GetClusterID(ctx)` 获取。
- `restoreID` 仅在新表 schema 中参与删除、写入和读取，可隔离相同集群/时间戳上的不同恢复任务。
- `useCheckpoint` 同时控制检查点存储是否参与介质选择，以及写成功后是否推进检查点进度。
- `dom` 用于表和列探测；`unsafeSession` 用于系统表内部 SQL；`storage` 是无系统表时的回退对象存储。
- 系统表中的 `id_map` 段是同一个序列化 `BackupMeta` 的连续字节片段，不可独立解释。关键不变量是 `segment_id` 从 0 连续递增、每段非空、拼接顺序稳定。
- 空结果语义是“尚无保存的映射”：不存在的外部文件和系统表零行都返回 `Vec::new()`，不视为故障。

## 依赖与调用关系

上游直接关系：

- [`client.rs`](client.rs) `SaveIdMapWithFailPoints` 直接调用 `saveIDMap`，为日志恢复编排提供保存入口。
- [`client.rs`](client.rs) `GetBaseIDMapAndMerge` 直接调用 `loadSchemasMap`，随后合并到 `TableMappingManager`。
- [`export_test.rs`](export_test.rs) 的 `TEST_saveIDMap` 与 `TEST_initSchemasMap` 经上述入口向 Go 对齐测试暴露行为。
- [`lib.rs`](lib.rs) 挂载模块并再导出文件名函数和块大小常量。

下游直接关系：

- `stubs::stream::TableMappingManager` 提供 `ToProto`，并在读取方提供 `MergeBaseDBReplace`。
- `stubs::backuppb::{BackupMeta, PitrDBMap}` 定义持久化容器和映射条目；当前 Rust crate 使用本地 stub 的 `Marshal`/`Unmarshal`。
- `stubs::storeapi::Storage` 提供 `FileExists`、`ReadFile`、`WriteFile`。
- `stubs::checkpoint::LogMetaManagerT` 提供检查点存储与进度写入。
- `stubs::glue`/`stubs::kv` 提供系统表 SQL 会话、参数、行读取和 `InternalTxnBR` 上下文。
- `restore_misc::HasRestoreIDColumn`、`metautil::CheckBackupMetaCompatibilityFromBytes` 分别承担 schema 演进探测和元数据版本检查。

`Cargo.toml` 将本 crate 标为对应 Go 包 `br/pkg/restore/log_client` 的 library，并声明 `stream`、`restore`、`checkpoint` 等路径依赖；但本文件源码通过 `crate::stubs` 使用瘦身接口，符合 manifest 中针对 arm64 Darwin 避开完整 kv/domain/kvproto/grpcio 依赖的说明。

RustCodeGraph 对目标文件列出 25 个符号，并确认其被 `lib.rs`、`id_map_test.rs`、`parity_test.rs` 使用。精确 `callers`/`callees` 查询未输出方法边，因此上述运行时边同时用已索引的 `client.rs` 源节点和局部引用检索核验，未把缺失图边当作“没有调用者”。

## 错误处理与边界

- `saveIDMap` 在回退存储为空时返回 `storage is nil`；表路径在 `unsafeSession` 为空时返回 `unsafeSession is nil`。错误通过 `?` 原样传播。
- `FileExists` 的访问错误会附加目标文件名；“访问失败”与“文件不存在”严格区分，后者返回空映射。
- 表查询错误附加 `failed to get pitr id map from mysql.tidb_pitr_id_map` 上下文。
- 任一缺失的 `segment_id` 或空段都会报错，防止将不完整字节流当作有效映射；完整拼接后仍须通过反序列化和兼容性检查。
- `BM` stub 格式拒绝短于 14 字节的头、截断的名称长度、溢出的长度加法和截断的名称；名称字节用 `String::from_utf8_lossy` 解码，因此非法 UTF-8 会被替换而不是报错。
- `checkRequirements == true` 时兼容性检查错误阻断恢复；为假时仅记录告警并继续。这是显式兼容模式，不代表元数据一定完全兼容。
- Rust 的 `dom == None` 被当作“表/列不存在”，从而可能落入存储路径；Go 对照实现直接使用 `rc.dom`，没有同样的空值保护。
- 旧 schema 不含 `restore_id`，同一 `(restored_ts, upstream_cluster_id)` 的并行任务会覆盖彼此。这是兼容限制，不应描述为任务隔离。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务，也不拥有外部资源的关闭责任；`Arc<dyn Storage>` 只延长检查点存储在调用期间的共享所有权。

外部存储写入表现为一次 `WriteFile`。系统表写入则是一次 `DELETE` 后多次独立 `REPLACE`，源码没有显式事务包裹：中途失败可能留下部分新段，后续读取会通过连续段/反序列化校验发现部分损坏，但不能保证并发写入隔离。上层应避免多个任务用相同逻辑键并发保存；新 schema 至少可借 `restoreID` 隔离不同恢复任务，旧 schema 不具备这一能力。

检查点进度的时序不变量是“映射持久化先成功，进度后推进”。若进度写入失败，映射可能已落盘但状态未推进，安全重试会覆盖同名文件，或通过 DELETE + REPLACE 重建同键分段。

## 与 Go 版本的对应关系

Rust 逐项对应 [`id_map.go`](id_map.go) 的常量、文件名函数以及 `LogClient` 方法，介质优先级、表 schema 分支、SQL 键、512 KiB 切段、缺段/空段检查、空结果语义和检查点推进顺序保持一致。Go 的 `client.go` 同样由 `GetBaseIDMapAndMerge` 调用加载逻辑，并由 `SaveIdMapWithFailPoints` 包装保存逻辑。

当前可见差异必须保留在认知中：

- Go 存储写路径使用 `metautil.NewMetaWriter(...).FlushBackupMeta` 和 protobuf；Rust 直接构造 `BackupMeta`、调用本地 stub `Marshal` 后 `WriteFile`。
- Rust 为 stub 的 `BM` 简易格式增加手工重建 `DbMaps` 的逻辑；Go 仅调用 protobuf `Unmarshal`。这属于当前移植环境适配，不是 Go 文件中的协议分支。
- Go 的 `pitrIDMapTableExists` 假设 domain 存在；Rust 在 `dom` 缺失时返回 `false`。Rust 也显式检查 `storage`/`unsafeSession` 是否存在并返回错误。
- Go 的完整测试 `client_test.go` 覆盖系统表、普通对象存储、检查点存储、大规模库表/分区映射、恢复任务隔离和新 schema 版本拒绝；当前 Rust 测试覆盖核心存储往返、进度、文件名、缺段以及截断 stub 元数据，但测试广度仍小于 Go。

因此，本文件实现了可执行的 Rust 路径，但“与 Go 完全等价”仍受本地 stubs 和测试覆盖范围约束。

## 扩展指南

- 调整介质选择时，应同时修改 `saveIDMap` 与 `loadSchemasMap`，保持读写优先级对称；同步扩展 [`client_test.rs`](client_test.rs) 的往返测试和 [`parity_test.rs`](parity_test.rs) 的契约测试。
- 修改文件名协议或块大小会影响跨版本、跨语言读取。必须同步 Go `id_map.go`、Rust/Go 文件名测试，并评估既有对象与系统表行的兼容迁移；不要只改常量。
- 增加系统表键列时，应成对修改 `pitrIDMapHasRestoreIDColumn`（或新增探测）、DELETE、REPLACE、SELECT 及其参数顺序，并为新旧 schema 各保留独立测试。
- 修改序列化格式时，应把 `saveIDMap2Storage`、`saveIDMap2Table` 与 `loadPITRIDMapBackupMeta` 作为一个协议面审查；同时验证表分段跨边界拼接、截断输入、版本过新、空映射和大映射。
- 若引入并发保存或要求崩溃原子性，优先在系统表路径设计显式事务/版本提交协议，而不是放宽读取校验；旧 schema 的键冲突也需要明确升级策略。
- 新增 Rust 测试必须继续放在独立测试文件中，不得内嵌到 `id_map.rs`。最接近的专用测试是 [`id_map_test.rs`](id_map_test.rs)，跨组件往返位于 [`client_test.rs`](client_test.rs)，Go 行为基线位于 [`client_test.go`](client_test.go)。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/restore/log_client` 定位目标、Go 对照和测试；`node --file br/pkg/restore/log_client/id_map.rs` 读取全部 410 行及 25 个符号；`query` 确认 Rust/Go 的 `PitrIDMapsFilename`、`saveIDMap`、`loadSchemasMap`、`loadPITRIDMapBackupMeta`；对精确 Rust 符号执行的 `callers`/`callees` 没有产生输出，此限制已在“依赖与调用关系”说明。
- 生产源码：[`id_map.rs`](id_map.rs)；直接接线：[`client.rs`](client.rs) 的 `SaveIdMapWithFailPoints`、`GetBaseIDMapAndMerge`；模块入口：[`lib.rs`](lib.rs)。
- crate 边界：[`Cargo.toml`](Cargo.toml)，确认 library 入口、Go package 对应关系与路径依赖。
- Go 对照：[`id_map.go`](id_map.go)；Go 集成语义：[`client_test.go`](client_test.go) 的 `TestPITRIDMap`、`TestPITRIDMapOnStorage`、`TestPITRIDMapOnCheckpointStorage` 和 schema 版本错误用例。
- Rust 测试：[`id_map_test.rs`](id_map_test.rs) 验证截断 `BM` 元数据被拒绝；[`client_test.rs`](client_test.rs) `test_pitr_id_map` 验证检查点存储往返、进度与文件名；[`parity_test.rs`](parity_test.rs) 验证块大小、文件名、往返和缺段错误。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核唯一生产物、源码链接、真实符号和限制说明。
