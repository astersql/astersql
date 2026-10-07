# `br/pkg/task/operator/stubs.rs`

## 文件定位

[`stubs.rs`](stubs.rs) 是 `astersql-br-pkg-task-operator` crate 的共享边界适配层。`lib.rs` 最先以 `pub mod stubs` 挂载它，随后 `config.rs`、`checksum_table.rs`、`crr_checkpoint.rs`、`force_flush.rs`、`list_migration.rs`、`migrate_to.rs`、`prepare_snap.rs`、`test_storage.rs` 均直接导入其中的类型或函数。它不是 Go 包中某个同名文件的逐行移植，而是把 Go operator 文件依赖的多个外部包压缩成一组本地 trait、数据结构、内存实现和注入钩子。

`Cargo.toml` 将该目录声明为 library crate，并明确注明 arm64 Darwin 不接入完整的 `kv/domain/kvproto/grpcio`，而使用 local traits/stubs。因而本文件的真实角色是让 operator 子命令能在当前 Rust 迁移阶段编译并进行契约测试；除少数纯数据处理能力外，它不能被理解为生产级 PD、TiKV、etcd 或云存储客户端。

## 核心职责

- 提供跨模块通用基础类型：`Result<T>`、`Error`、`Context`、`Config`、`FlagSet`、TLS/keepalive 配置和 Go 同名常量。
- 以 `ExternalStorage`、`PDClient`、`Glue`、`KVStorage`、`KVClient`、`Domain`、`InfoSchema`、`Session`、`GCManager` 等 trait 定义 operator 与外部系统的最小边界。
- 提供 `MemStorage`、`MemPDClient`、`MemGlue`、`MemKVStorage`、`MemKVClient`、`MemGCManager` 等确定性的内存替身，支撑独立 Rust 测试且避免真实集群和云资源。
- 承载 migration、CRR checkpoint、checksum、GC safepoint、prepare-snapshot 和 force-flush 路径所需的最小数据模型与辅助行为。
- 通过全局 `DIAL_HOOKS` 暴露 PD、StoreManager 和连接管理器构造边界，允许测试注入成功或失败路径。
- 对尚未接通的能力保持显式简化：例如 `NewStorage` 总是返回 `MemStorage`，`NewMgr` 默认组装内存客户端，`NewCRRService` 只验证任务名并保存配置，`SetExplicitRequestSourceType` 和 `NewProgressBarHooks` 是空操作。

## 主要符号

基础契约包括 `Error` 及其 `Annotate`/`Wrapf`/`Trace`，共享取消标志的 `Context`，以及保存不同 flag 值的私有 `FlagValue` 和公开 `FlagSet`。`Config::default` 固定 schema 并发 64、checksum 并发 4、gRPC keepalive 10 秒/3 秒，并让 `TableFilter` 默认匹配全部表；`Config::ParseFromFlags` 只读取已注册的部分字段。

存储族由 `ExternalStorage`、`ExternalReader`、`ExternalWriter` 定义，`MemStorage` 以 `Arc<Mutex<HashMap<String, Vec<u8>>>>` 保存对象，以 `AtomicBool` 记录关闭状态。入口函数为 `ParseBackend`、`NewStorage`、`CreateStorage`、`GetStorage`；`SaveJSONEffectsToTmp` 是少数真实文件系统副作用，会把 dry-run effect 写到系统临时目录。

PD/TiKV 族包括 `PDClient`、`MemPDClient`、`PdController`、`StoreManager`、`FlushResult` 和 `metapb::Store`。`IsTiFlash` 只检查 `engine=tiflash` 标签；`StoreManager::FlushNow` 从测试注入表返回结果或带 store ID 的错误。

migration 族包括 `Migration`、`MigrationLayer`、`Migrations`、`MergeAndMigratedTo`、`MigrationExt` 和 `MigrationExtension`。`MigrationExt` 可从 `migrations.json` 加载 JSON，记录 dry-run effects，执行交互确认，并产生合并结果；它模拟 Go `br/pkg/stream` 的公开表面而非实现完整 migration 引擎。

CRR 族包括 `ObjectSyncChecker`、`ExistenceSyncChecker`、`FixedSyncChecker`、`PersistentState`、`ResumeStateStore`、`CRRServiceConfig`、`CRRDeps` 和 `CRRService`。固定状态文件名由 `GetStatusFileName` 返回 `crr-checkpoint/resume-state.json`。

checksum/元数据族包括 `DBInfo`、`TableInfo`、`MetaTable`、`BackupMeta`、`MetaReader`、`ChecksumRequest`、`ChecksumResponse`、`ChecksumExecutor` 和 `ExecutorBuilder`。`ExecutorBuilder::Build` 优先采用 `old_table` 的 ID，否则使用当前表 ID，并把时间戳和并发度写入请求。

生命周期与注入符号包括 `BRServiceSafePoint`、`StartServiceSafePointKeeper`、`ConnMgr`、`Preparer`、`DialHooks`、`DIAL_HOOKS` 及三个 `set_*_hook`/`clear_dial_hooks` 函数。

## 执行流程

1. crate 入口先加载 `stubs`，其余 operator 模块从中取得配置、错误、trait 和内存实现。
2. CLI 配置路径先由各模块调用 `FlagSet` 注册默认值，再经 `Config::ParseFromFlags` 或 `CRRServiceConfig::Parse` 读取；未定义或类型不符的 flag 返回 `Error`。
3. 存储路径由 `ParseBackend` 拆分 URI 的 scheme/path，`GetStorage` 或 `CreateStorage` 调用 `NewStorage`。当前 `NewStorage` 不按 scheme 分派真实后端，而总是构建按实例隔离的 `MemStorage`。
4. migration 路径通过 `MigrationExtension` 包装存储；`Load` 检查并解析 `migrations.json`；`MergeAndMigrateTo` 选择 BASE 或指定层、执行可选确认回调、记录 `merge` effect，再返回注入结果或默认结果。`DryRun` 清空并收集 effect，若闭包没有记录则补入 `noop`。
5. CRR 路径选择 `ExistenceSyncChecker` 或上游自带的 `ObjectSyncChecker`，用 `ResumeStateStore` 保存 `PersistentState`，并把依赖和 `CRRServiceConfig` 交给 `NewCRRService`；本文件中的构造函数仅拒绝空 `TaskName`。
6. checksum 路径由 `ExecutorBuilder` 生成单分片 `ChecksumExecutor`；`Execute` 先按 `Len` 次数调用进度回调，再将请求交给 `KVClient::Checksum`。
7. snapshot/GC 路径由 `GetTSWithRetry` 获取物理/逻辑时间并经 `ComposeTS` 合成 TSO；`StartServiceSafePointKeeper` 在写入 manager 前校验 ID 非空且 TTL 大于零；`Preparer` 依次触发连接回调、传播注入错误，并在 `Finalize` 标记完成。

## 数据与状态

大部分可变状态用 `Arc` 共享所有权、用 `Mutex` 保护容器：对象数据、PD store 列表、flush 结果、migration effects、控制台输出、GC safepoint 和注入错误都属于此类。关闭、取消、finalize 等单比特状态用 `AtomicBool`，统一采用 `SeqCst` 顺序。

`MemStorage` 的数据只在同一克隆族内共享；两次对同一 URI 调用 `NewStorage` 仍会得到彼此独立的 map，因此 URI 不是全局存储注册键。`MemWriter` 在 `Close` 时一次性提交缓冲区，未关闭的 writer 不会持久化数据。`WalkDir` 在释放 map 锁后调用回调，按路径排序，并故意忽略 `ListCount` 对结果集合的截断。

`PersistentState` 的 JSON 字段固定为 `last_checkpoint`、`synced_ts` 和可省略的 `synced_by_store`；`BackupMeta::Unmarshal` 优先解析测试友好的 JSON，失败时保留不透明原始字节。`SQLRow` 在类型不匹配或索引越界时返回零值而非错误，这是桩接口的宽松边界。

全局 `DIAL_HOOKS: LazyLock<Mutex<DialHooks>>` 在进程内共享，测试必须调用 `clear_dial_hooks` 恢复默认值，避免并行用例相互污染。`DUMP_GOROUTINE_WHEN_EXIT` 同样是全局原子状态。

## 依赖与调用关系

上游调用者位于同 crate 的全部主要模块：`config.rs` 使用 flag/config 类型；`base64ify.rs` 使用 `Context`、`BackendOptions` 和错误类型；`list_migration.rs`/`migrate_to.rs` 使用存储、migration 与 console；`force_flush.rs` 使用 PD/store 模型；`crr_checkpoint.rs` 使用存储、CRR、resume-state 和连接边界；`checksum_table.rs` 使用 schema、session、checksum、GC 和 backup metadata；`prepare_snap.rs` 使用 PD、StoreManager、safepoint、Preparer 与 dial hooks；`test_storage.rs` 使用存储 reader/writer。

本文件直接依赖标准库同步原语和时间类型，以及 `serde`/`serde_json` 做模型序列化、`uuid` 生成 effect 临时文件名、operation ID 和 safepoint ID。虽然 crate 的 `Cargo.toml` 还声明 `astersql-metaservice`、`astersql-objstore`、`base64`、`regex`，这些依赖主要由相邻实现文件消费；`stubs.rs` 自身不直接调用真实 metaservice 或 objstore API。

RustCodeGraph 将本文件标记为被 160 个索引文件使用；文件级结果包含 operator 模块及跨 BR 测试。精确源码引用显示本 crate 内九个生产模块和四个测试模块通过 `crate::stubs` 消费它。由于许多符号采用 Go 风格常见名称，宽泛 symbol 查询会与仓库其他模块重名，调用关系应以文件路径和显式 import 为准。

## 错误处理与边界

统一错误仅保存字符串，不保留错误链、类型标识或堆栈；`Trace` 原样返回，`Annotate`/`Wrapf` 只拼接消息。因此调用者可验证错误关键字，但不能依赖 Go `pingcap/errors` 的 cause/stack 行为。

明确的失败条件包括：空 storage URI、缺失对象、rename 源不存在、flag 未定义或类型不匹配、缺失 `migrations.json` 且要求 not-found-is-error、用户拒绝 migration、空 CRR task name、未配置 domain/session、非法 safepoint ID/TTL，以及注入的 flush/load/prepare 错误。`Mutex::lock().unwrap()` 在锁中毒时会 panic，这与返回 `Result` 的业务错误路径不同。

若读取对象范围出现 `start > end` 或越界，`MemStorage::Open` 返回空切片而非错误。`DeleteFile`/`DeleteFiles` 对不存在对象仍成功。`TLSConfig::ToTLSConfig` 目前只依据 CA 是否为空生成布尔材料，并不读取或验证证书路径。`ParseBackend` 接受任意非空 scheme。以上行为均是当前桩边界，不能外推为真实后端契约。

## 并发与资源生命周期

公开边界 trait 普遍要求 `Send + Sync`，使 operator 的并发任务可共享存储、PD 和会话抽象。`Arc<Mutex<_>>` 保证内存替身的数据同步；原子标志负责取消与关闭可见性，但多数方法不会在 `closed=true` 后主动拒绝操作，因此 `Close` 更多用于测试清理断言，不等价于真实连接彻底失效。

`ExternalReader::Close` 只修改 reader 的私有布尔值；`ExternalWriter::Close` 才执行落盘。`PdController`、`StoreManager`、`ConnMgr`、`CRRService` 的 `Close` 仅设置标志。`Preparer::DriveLoopAndWaitPrepare` 是同步流程，不启动后台任务；`StartServiceSafePointKeeper` 也只做一次写入，不包含 Go keeper 的周期续租循环。

全局 dial hook 的写入由一个 mutex 串行化，但 hook 在被调用时若仍持有该锁可能带来重入约束，扩展时应检查相邻构造函数的取锁范围。测试侧必须用清理守卫或在断言后调用 `clear_dial_hooks`，尤其不能让失败分支跳过恢复。

## 与 Go 版本的对应关系

Go `br/pkg/task/operator` 没有 `stubs.go`；其同名业务文件直接导入 `br/pkg/task`、`br/pkg/stream`、`br/pkg/gc`、`br/pkg/pdutil`、`br/pkg/utils`、`pkg/objstore`、PD、TiKV、etcd 和 gRPC。Rust 本文件把这些分散依赖的最小公开形状集中到本地，以适应当前平台和迁移边界。

对应关系示例：Rust `MigrationExt` 对应 Go `stream.MigrationExt` 的 operator 所需子集；`ExternalStorage` 对应 `objstore/storeapi.Storage`；`PDClient`/`PdController`/`StoreManager` 对应 PD 与 BR utils 管理器；`BRServiceSafePoint`/`GCManager` 对应 `br/pkg/gc`；`ExecutorBuilder`/`ChecksumExecutor` 对应 checksum 执行器；`CRRServiceConfig`/`CRRService`/`ObjectSyncChecker`/`ResumeStateStore` 对应 `br/pkg/stream/crr/service`。

关键差异必须保留在认知中：Go `NewCRRCheckpointService` 会打开两端存储、校验 `backup.lock`、创建 task manager、拨号 etcd、装配 watcher 并严格清理资源；Rust 的真实编排位于相邻 `crr_checkpoint.rs`，而本文件只提供依赖形状和简化服务对象。Go `AdaptEnvForSnapshotBackup` 启动并发 keeper、等待取消、续租 safepoint 并恢复 scheduler；本文件的 `Preparer` 和 safepoint 函数不实现这些后台生命周期。Go force-flush 会建立 gRPC 连接并发请求 TiKV；`StoreManager::FlushNow` 只读取注入表。

相关 Go 证据位于 `config.go`、`list_migration.go`、`migrate_to.go`、`crr_checkpoint.go`、`prepare_snap.go`、`force_flush.go` 和 `checksum_table.go`。Rust 测试 `parity_test.rs` 覆盖公开契约、错误和资源清理，`crr_checkpoint_test.rs` 覆盖 lock 校验、resume-state 路径、sync checker 选择及 etcd 配置；后者明确记录 arm64 下不接完整 kv/domain/kvproto/grpcio 的刻意差异。

## 扩展指南

新增 operator 能力时，应先判断它属于纯数据契约、可注入边界，还是必须接入真实客户端。只有前两类适合继续放入本文件；真实网络、持久化或后台任务实现应进入对应 canonical crate，并让 operator 依赖其公开接口，避免扩大“桩即生产实现”的误解。

扩展 trait 时必须同步所有实现和包装器，特别是 `MemStorage`、`crr_checkpoint_test.rs` 中的 `syncedStorage`、以及其他通过 `impl ExternalStorage` 提供测试夹具的文件。新增状态字段要同步 serde 字段名与 Go JSON 契约；修改 `GetStatusFileName`、`LockFile`、`MetaFile` 或 TSO 位布局属于兼容性变更。

修改资源构造或关闭语义时，应在相邻独立测试文件增加失败路径和清理断言，不要把测试嵌入 `stubs.rs`。存储行为优先扩展 `parity_test.rs` 或专用模块测试；CRR 行为同步 `crr_checkpoint_test.rs`；base64 和 test-storage 分别同步 `base64ify_test.rs`、`test_storage_test.rs`。若引入全局 hook，必须同时提供清理 API 并覆盖并行污染风险。

性能方面，`MemStorage::WalkDir` 会复制并排序全部匹配对象，`ReadFile`/reader 也复制完整 payload；它们适合小型测试数据，不应直接承担大对象或高并发基准。若要求生产可用，需要以真实后端实现替换调用点，而不是在这个共享文件中逐步堆叠完整子系统。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7032 个 Rust 文件；`node --file br/pkg/task/operator/stubs.rs` 完整检查了 1--1944 行，并确认文件级使用范围。
- RustCodeGraph `query`：核对 `NewStorage`、`MigrationExtension`、`NewCRRService`、`StartServiceSafePointKeeper`、`GetTSWithRetry`、`set_dial_pd_hook` 的定义位置；常见 Go 风格名称存在跨模块重名，故用文件路径与源码 import 消歧。
- Rust 源与 crate 边界：`br/pkg/task/operator/stubs.rs`、`lib.rs`、`Cargo.toml`，以及直接导入它的九个相邻生产模块。
- Go 对照：`config.go`、`list_migration.go`、`migrate_to.go`、`crr_checkpoint.go`、`prepare_snap.go`、`force_flush.go`、`checksum_table.go`。
- 独立测试：`parity_test.rs`、`crr_checkpoint_test.rs`、`base64ify_test.rs`、`test_storage_test.rs`；其中前两者直接覆盖本文件的共享替身与边界。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务指定命令校验恰有 11 个固定二级章节，并人工复核没有把内存桩描述为真实外部实现。
