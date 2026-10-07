# `br/pkg/task/stubs.rs`

## 文件定位

`stubs.rs` 属于 Cargo 包 `astersql-br-pkg-task`。包根 [`br/pkg/task/lib.rs`](lib.rs) 在其他业务模块之前以 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并在末尾通过 `pub use stubs::*` 把公开符号平铺到 crate 根。因而它既是 `backup.rs`、`restore.rs`、`stream.rs` 等同 crate 模块的依赖，也是命令层可见的兼容 API 面。

这个文件不是 BR 的真实基础设施实现。文件级注释与 [`br/pkg/task/Cargo.toml`](Cargo.toml) 都把它限定为 arm64 或缺少 TiKV、PD、`kv/domain/kvproto/grpcio` 环境时使用的本地替身：它用内存对象和精简数据结构隔离外部系统，使 task 层逻辑和单元测试能够执行。真实生产能力分布在 `br/pkg/glue`、`br/pkg/storage`/`pkg/objstore`、`br/pkg/conn`、`br/pkg/metautil`、PD/TiKV client 及 protobuf 类型中，不能把本文件的成功返回视作这些能力已接线。

## 核心职责

文件承担五类兼容职责：

1. 提供 `Error`、`Result<T>`、`berrors` 常量，以及 `backuppb`、`encryptionpb`、`metapb` 的最小字段镜像，使迁移代码保留 Go 风格命名和错误调用面。
2. 提供 `FlagSet`/`FlagValue`、配置常量和轻量配置结构，支撑 task 配置解析，而不实现完整 `pflag`、TLS 或 gRPC 行为。
3. 定义 `Progress`、`Glue`、`Storage`、`Mgr`、`BackupClient` 等 task 层边界，并以 `MemProgress`、`MemGlue`、`MemStorage`、`MemMgr`、`MemBackupClient` 提供可观测的测试替身。
4. 用 `MetaWriter` 和 thread-local `SummaryCollector` 模拟 meta 完成/刷新及命令摘要状态，供备份、恢复流程断言调用顺序和结果。
5. 提供没有外部 I/O 的纯函数：storage URI 和 key 解析、TSO 编解码、TiDB key 编码、表过滤、SQL 标识符引用、归档大小、rewrite rule 与系统库识别。

这些实现的共同不变量是“保留 task 层当前需要的契约，但不模拟集群”。例如 `MemBackupClient::BackupRanges` 只设置 `backup_called` 并返回注入的 `archive_size`，`MetaWriter::StartWriteMetasAsync` 不启动后台任务，`BackendOptions::ParseFromFlags` 不读取 flag。

## 主要符号

- `Error` / `Result<T>`：以单个 `msg` 字符串表达失败；`Errorf`、`Annotate`、`Annotatef`、`Trace`、`Wrapf`、`Cause` 保留 Go 风格调用名。`Trace` 是恒等函数，`Cause` 不遍历错误链。
- `berrors::{ErrInvalidArgument, ErrBackupInvalidRange, ErrRestoreModeMismatch, ErrStorageUnknown, ErrUnknown}`：task 路径所需的字符串 sentinel 子集，不具有 Go typed error 或错误链身份语义。
- `encryptionpb`、`backuppb`、`metapb`：定义加密枚举、master key、storage backend、backup request/meta/file/range、PD region/store 等最小镜像。字段只覆盖当前调用点；没有 protobuf wire API、未知字段或完整拓扑。
- `FlagSet` / `FlagValue`：支持 `Define*`、`Set`、`Lookup`、`Changed`、`Get*`、`Visit`。只有 `Set` 会标记 changed；缺失或类型不符返回 `Error`，但 `GetString` 对非字符串值采用 Debug 字符串这一测试友好退化。
- `Progress` / `MemProgress` 与 `Glue` / `MemGlue`：前者用原子计数和关闭标记表达进度，后者以互斥锁保存 metrics 与 console 字节；`Glue::GetRestoreLifecycle` 默认明确报错，要求恢复路径显式注入生命周期。
- `Storage` / `MemStorage`：以 `Arc<Mutex<HashMap<String, Vec<u8>>>>` 实现读、覆写、存在性检查和前缀遍历。路径只是 map key，没有目录、权限、对象版本或远端一致性语义。
- `Mgr` / `MemMgr`：抽象 GC manager、版本、region 计数、scheduler 移除/恢复、TLS 和 PD 调度更新。内存实现返回固定注入值并用原子标志记录调用；scheduler 恢复闭包是 no-op。
- `BackupClient` / `MemBackupClient`：抽象 cluster/TSO/storage/range/backup 操作。默认 `BuildBackupRanges` 返回一个空边界的全 keyspace range；内存实现不访问 PD/TiKV。
- `MetaWriter`：在 `Mutex<BackupMeta>` 和原子标志中保存更新、finished、flushed、archive size 状态；`snapshot`、`was_finished`、`was_flushed` 是测试观测入口。
- `SummaryCollector` 及 `CollectInt`、`SetSuccessStatus`、`Summary`、`TakeSuccessStatus`：每线程保存摘要整数和成功位；`Summary` 本身不输出。
- `ParseBackend`、`ParseKey`/`unescaped_key`、`IsEffectiveEncryptionMethod`、`EncloseName`/`EncloseDBAndTable`、`oracle::*`、`EncodeTablePrefix`、`EncodeBytes`、`ArchiveSize`：承担无需外部系统的格式转换。
- `TableFilter`、`ParseFilter`、`CaseInsensitive`：只支持空规则/`*.*`、精确 `db.table`、反引号精确形式和 `schema.*`，不是 Go filter 包的完整 glob 解析器。
- `DBReplace`、`TableReplace`、`SchemasReplace`、`RewriteRules`、`GetRewriteRuleOfTable`：保存恢复 ID 映射；当前 rewrite helper 忽略 index map 与 collation 参数。
- `OperationContext` / `NewOperationContext`：保存 UUID、开始时间和 hint map；构造函数忽略 command，也不向 etcd 注册 operation。

## 执行流程

该文件没有独立入口；执行由调用它的 task 流程驱动。最完整的调用例子是 `backup.rs::run_backup_body`：

1. 调用 `Mgr::GetClusterVersion`、`GetRegionCount`，通过 `Glue::StartProgress` 创建进度对象，并从 `BackupClient::GetCurrentTS` 或配置取得 backup TSO。
2. 可选地取得 GC manager、移除 scheduler，并用 `BackupClient::SetStorageAndCheckNotInUse` 校验 storage 占用。
3. 构造 `backuppb::BackupRequest`，调用 `BuildBackupRanges` 后再调用 `BackupRanges`；内存 client 在此仅记录调用并返回预设大小。
4. 创建 `MetaWriter`，依次执行 `StartWriteMetasAsync`、`Update`、`FinishWriteMetas`、`FlushBackupMeta`。在桩实现中，这些步骤只改内存状态，不写 `backupmeta` 对象。
5. 通过 `Glue::Record` 和 `SetSuccessStatus(true)` 记录结果；`MemGlue` 与 thread-local summary 让测试读取这些副作用。

配置路径则先用各业务模块的 `Define*Flags` 调用 `FlagSet::Define*` 注册默认值，再由测试或适配器用 `Set` 注入显式值，最后由 `common.rs`、`backup_raw.rs`、`restore.rs` 等调用 `Get*`。key 路径中，`ParseKey` 对空串返回空字节；`hex` 解码十六进制，`raw` 取原始 UTF-8 字节，`escaped` 逐字节处理常用转义、两位十六进制和三位八进制；未知格式返回 `ErrInvalidArgument` 注释错误。

恢复和流备份路径把 `MemStorage` 当成对象存储：调用方写入 `MetaFile` 或 checkpoint 路径，`ReadFile` 按完整键读取，`WalkDir` 先在锁内复制符合前缀的条目、释放锁后再逐项调用回调。后一顺序避免回调期间持有 storage mutex，也允许回调返回错误中止遍历。

## 数据与状态

数据分为不可变契约常量、值对象和共享可变测试状态。`MetaFile`、checkpoint 文件名、默认并发/TTL、merge region 阈值、metric 名等常量用于让迁移代码保持与 Go 调用点相同的参数。protobuf 镜像、`KeyRange`、`RangeStats`、filter/rewrite/operation 结构均为进程内值，不携带真实连接。

共享状态通过以下方式管理：

- `MemStorage.files`、`MemGlue.records`、`MemGlue.console`、`MetaWriter.meta` 和 `OperationContext.hint_fields` 使用 map/vector；跨线程共享的前三类由 `Arc<Mutex<_>>` 或 `Mutex<_>` 保护。
- 进度、关闭、调用、finished/flushed、archive size 与 success 等单值使用 `AtomicBool`、`AtomicI64`、`AtomicU64`，统一采用 `Ordering::SeqCst`，便于测试得到确定的跨线程观察顺序。
- `SUMMARY` 使用 `thread_local!`。因此一个线程调用 `SetSuccessStatus` 后，另一个线程的 `TakeSuccessStatus` 不会看到同一实例；它不是跨线程的全局 summary。
- clone `MemStorage` 会共享同一个 `Arc` map；clone `MetaWriter` 同样共享 meta 与原子状态。`MemBackupClient::GetStorage` clone 内部 `MemStorage` 后装入 trait object，所以仍观察同一文件集合。
- `FlagSet::Define*` 用 `entry(...).or_insert(...)`，重复定义不会覆盖已有值；`Set` 覆盖值并永久标记 changed。`Visit` 的 HashMap 遍历顺序未定义，不应据此断言顺序。

## 依赖与调用关系

crate 边界由 [`br/pkg/task/Cargo.toml`](Cargo.toml) 声明：本文件直接使用标准库同步/时间/集合类型、`serde` 派生、`hex` 解码、`uuid` v4，以及 `astersql-br-pkg-gc::Manager`/`MakeSafePointID`。同一 Cargo 包还依赖 restore、aws、common、stream、conn、utils、registry、checkpoint 和 restore-log-client 等 crate，但不能据此推断它们都由 `stubs.rs` 直接调用。

RustCodeGraph 将该文件标记为至少被 `common.rs`、`restore.rs`、`restore_lifecycle.rs` 以及对应测试等 15 个文件使用；精确符号查询还给出 `BackupClient` 的生产调用者 `backup.rs::{RunBackup, run_backup_body}`、`backup_raw.rs::RunBackupRaw`、`backup_txn.rs::RunBackupTxn`。源码搜索补充了 `restore_raw.rs`、`restore_txn.rs`、`restore_data.rs`、`backup_ebs.rs`、`stream.rs` 对 storage、glue、编码和状态类型的直接引用。

关键下游关系如下：

- `Mgr` 的 GC 分支依赖 `astersql-br-pkg-gc::Manager`；默认 `GetGCManager` 为 `None`，不会自动建立 safepoint 保护。
- `Glue::GetRestoreLifecycle` 的返回类型来自同 crate `restore_lifecycle`；默认实现失败，使真实恢复生命周期不能静默退化为 no-op。
- `ParseKey` 的算法直接对照 `br/pkg/utils/key.go::ParseKey`；`ArchiveSize` 对照 `br/pkg/metautil/metafile.go::ArchiveSize`。
- `Glue`/`Progress` 的概念对照 `br/pkg/glue/glue.go`，`Mgr` 与 merge-region 常量对照 `br/pkg/conn/conn.go`，storage URI 的生产解析入口是 `pkg/objstore/parse.go::ParseBackend`。
- `MetaWriter` 的生产对照位于 `br/pkg/metautil/metafile.go`；Go 版本包含 channel、wait group、异步 goroutine、v1/v2 meta 分片与真实 storage flush，本地桩仅保留 task 当前需要的状态机表面。

## 错误处理与边界

所有本地失败统一收敛为 `Error { msg }`，通过 `?` 传播；没有 source chain、backtrace、typed sentinel 比较或 gRPC status。`Error::Annotate(base, msg)` 生成 `"msg: base"`，这一顺序由 `parity_test.rs` 对未知 key 格式断言。扩展错误时不能假定 `Cause` 可恢复底层类型。

主要边界包括：

- `ParseBackend` 拒绝空字符串；有 `://` 时按第一次出现处分割，否则默认为 `local`。它不验证 scheme、bucket、路径、凭据或可达性。
- `ParseKey` 的空输入在任何 format 校验前返回空向量；`hex` 错误和非法/截断 escaped 输入返回错误。escaped 解析按字节工作，不是 Unicode rune 转义器。
- `MemStorage::ReadFile` 对缺失键返回 `file not found: <name>`；`WalkDir` 的回调错误立即返回，但已经执行的回调不会回滚。
- `FlagSet` 缺失项和多数类型不匹配返回错误；整数窄化使用 `as`，不会检查溢出。`GetString` 对类型不匹配不报错，是需要保留或明确修正的特殊契约。
- 默认 `Glue::GetRestoreLifecycle` 返回 `restore PD/import lifecycle is not configured`，默认 `Mgr::GetGCManager` 返回 `None`。这些显式缺口防止部分生产安全流程被误认为可用。
- mutex 均使用 `lock().unwrap()`；持锁线程 panic 会导致后续访问 panic，而不是返回 `Result`。
- `EncodeTablePrefix` 对负 table ID 采用有符号序保持变换；`oracle::ComposeTS` 将有符号参数直接转换为 `u64`，调用方必须提供非负、合法范围值。
- `TableFilter`、rewrite helper、系统库名单和 proto 镜像都是受限子集；新增 Go 调用面时必须先核实当前简化是否仍足够。

## 并发与资源生命周期

`Progress`、`Glue`、`Storage`、`Mgr`、`BackupClient` 都要求 `Send + Sync`，使 task 流程可通过 `Arc<dyn Trait>` 在线程间共享。内存实现的计数和标志使用顺序一致原子；复合容器用 mutex。`MemStorage::WalkDir` 特意在调用用户回调前释放锁，避免回调重入同一 storage 时死锁。

资源关闭是可观测状态而非真实资源释放：`MemProgress::Close` 和 `MemMgr::Close` 仅置位，`MemMgr::RemoveSchedulers` 返回的恢复闭包不做工作，`MetaWriter::FinishWriteMetas`/`FlushBackupMeta` 仅设置标志。`MetaWriter::StartWriteMetasAsync` 不创建线程、channel 或任务，因此也没有 join/cancel 生命周期；生产 Go `MetaWriter` 则必须 close channel、等待 goroutine 并传播 flush 错误。

`SummaryCollector` 的 thread-local 生命周期随线程结束；不能用它验证跨线程任务汇总。`NewOperationContext` 只生成本地 UUID 和当前时间，不注册、续租或清理 etcd operation。任何需要真实 scheduler 恢复、GC safepoint、对象存储落盘、PD/TiKV RPC 或 operation 注册的路径，都必须注入生产适配器，而不能直接使用 `Mem*`。

## 与 Go 版本的对应关系

本文件是一组跨包 Go API 的聚合移植，不存在同路径 `stubs.go` 一一对应：

- `ParseKey`/escape 规则对应 `br/pkg/utils/key.go`；独立 Rust `parity_test.rs::stub_key_and_filter_contracts_match_go_dependencies` 覆盖常用 escape、截断反斜杠和未知格式。
- `Glue`、`Progress` 对应 `br/pkg/glue/glue.go`，但 Rust trait 只保留 task 当前使用的版本、进度、记录、console 与恢复生命周期接口，省略 domain/session/storage ownership 等生产能力。
- `Mgr`、`VersionCheckerType`、merge-region 默认值对应 `br/pkg/conn/conn.go`；Rust 只保留固定 region 计数、TLS 占位、scheduler 和 GC manager 接口。
- `MetaWriter`、`ArchiveSize` 对应 `br/pkg/metautil/metafile.go`。`ArchiveSize` 保持对 `Size_` 求和；`MetaWriter` 则刻意不复刻 Go 的 channel、异步写入、meta v1/v2、加密与 storage I/O。
- `ParseBackend` 的生产语义对应 `pkg/objstore/parse.go`；Rust 桩只解析 scheme/path。
- `IsEffectiveEncryptionMethod` 对应 `br/pkg/utils/encryption.go`，SQL 标识符和系统库 helper 对应 `br/pkg/utils/schema.go`，rewrite helper 对应 `br/pkg/restore/utils/rewrite_rule.go`。
- 常量来自多个 Go 包：checksum 并发来自 `pkg/sessionctx/vardef`，GC TTL 来自 `br/pkg/gc`，schema 并发/range 阈值来自 `br/pkg/backup`，merge-region 阈值来自 `br/pkg/conn`。其中 Rust `RangesSentThreshold = 1024` 与当前 Go `br/pkg/backup/client.go` 的 `30000000` 不同；当前文件明确是局部桩，不能把该值当作生产默认值。

测试没有内嵌在 `stubs.rs`。直接契约由 `br/pkg/task/parity_test.rs` 和 `common_test.rs` 覆盖；流程注入由 `backup_test.rs`、`backup_raw_test.rs`、`backup_txn_test.rs`、`restore_test.rs`、`restore_raw_test.rs`、`restore_txn_test.rs`、`restore_data_test.rs`、`restore_ebs_meta_test.rs`、`restore_lifecycle_test.rs` 与 `stream_test.rs` 覆盖。相邻 Go 测试包括 `common_test.go`、`config_test.go`、`backup_test.go`、`restore_test.go` 和 `stream_test.go`。

## 扩展指南

新增 task 功能时，先判断它需要的是可测试边界还是生产能力。若只是让业务逻辑可注入，应在对应 trait 增加最小方法，同时更新所有实现者和同目录独立 `*_test.rs`；若涉及真实对象存储、PD/TiKV、glue session、protobuf 或 KMS，应在 canonical crate 实现并通过适配器注入，不应继续把生产逻辑堆入本文件。

具体修改建议：

- 扩展 flag 时同步 `FlagValue`、对应 `Define*`/`Get*`、`as_string`，并在 `common_test.rs` 或目标配置测试覆盖默认值、`Changed`、缺失项、类型错误和窄化边界。
- 扩展 storage 时保持 `Storage` object-safe；在独立测试中覆盖不存在对象、覆盖写、前缀遍历、回调错误和 clone 共享。不要让 `WalkDir` 持锁执行外部回调。
- 扩展 `BackupClient`/`Mgr`/`Glue` 时核对 `backup.rs`、raw/txn/restore/stream 调用顺序，并为安全相关默认方法选择显式失败而不是静默成功。
- 扩展 `MetaWriter` 时先决定是否仍为状态桩。若引入异步写入，必须设计 channel 关闭、错误传播、取消、join 和 flush 幂等，并把测试放到独立文件；不要用“置位成功”代替落盘证明。
- 扩展 protobuf 镜像时核对真实 proto 枚举值、默认值和序列化兼容性。当前 serde JSON 不能证明 protobuf wire 兼容。
- 修改 key/TSO/memcomparable/rewrite 算法时，逐例对照 Go canonical helper，并扩展 `parity_test.rs`；重点风险是截断输入、符号位、空范围、大小写、反引号和编码尾组。
- 若某个替身开始用于生产路径，应优先删除该路径对桩的依赖并接入真实 crate，而不是逐步把 `Mem*` 演变成不完整的生产客户端。

兼容性风险主要是公开 Go 风格符号被 crate 根通配导出，改名会影响大量调用点；正确性风险是桩的成功返回掩盖缺失 I/O；性能风险主要出现在大对象 clone、`SeqCst` 原子和单 mutex map，但这些只应在测试规模内评估。

## 验证依据

- RustCodeGraph：`status` 确认仓库存在索引；`files --filter br/pkg/task/stubs.rs` 与 `node --file ... --offset/--limit` 读取了 1392 行完整源码，并报告该文件由 15 个 task 文件使用；`query` 核对了 `ParseKey`、`ParseBackend`、`ArchiveSize`、`NewOperationContext`、`MemStorage`、`MetaWriter`、`BackupClient`、`FlagSet`，其中 `BackupClient` 的生产调用者包括 `RunBackup`、`run_backup_body`、`RunBackupRaw`、`RunBackupTxn`。通用 `callers` 查询因同名符号较多未返回可归属边，文档未据此虚构调用关系。
- Rust 源与模块边界：`br/pkg/task/stubs.rs`、`br/pkg/task/lib.rs`、`br/pkg/task/Cargo.toml`；生产调用证据来自 `br/pkg/task/backup.rs`、`backup_raw.rs`、`backup_txn.rs`、`restore.rs`、`restore_data.rs`、`restore_raw.rs`、`restore_txn.rs`、`stream.rs`。
- Rust 独立测试：`br/pkg/task/parity_test.rs`、`common_test.rs`、`config_test.rs`、`backup_test.rs`、`backup_raw_test.rs`、`backup_txn_test.rs`、`restore_test.rs`、`restore_data_test.rs`、`restore_ebs_meta_test.rs`、`restore_raw_test.rs`、`restore_txn_test.rs`、`restore_lifecycle_test.rs`、`stream_test.rs`。
- Go 对照：`br/pkg/utils/key.go`、`br/pkg/glue/glue.go`、`br/pkg/conn/conn.go`、`br/pkg/metautil/metafile.go`、`br/pkg/utils/encryption.go`、`br/pkg/utils/schema.go`、`br/pkg/restore/utils/rewrite_rule.go`、`pkg/objstore/parse.go`，以及同目录 `*_test.go`。
- 结构校验使用任务指定命令，要求文件存在且固定二级标题恰好 11 个。本任务为纯文档分析，按计划不运行 Cargo；人工复核重点为明确区分桩与生产能力、所有重要结论带路径/符号依据、测试保持在独立文件中。
