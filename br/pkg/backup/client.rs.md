# `br/pkg/backup/client.rs`

## 文件定位

`client.rs` 属于 Cargo 包 `astersql-br-pkg-backup`。该包由 [`br/pkg/backup/Cargo.toml`](./Cargo.toml) 声明为 library，入口 [`br/pkg/backup/lib.rs`](./lib.rs) 通过 `#[path = "client.rs"] pub mod client` 注册本模块，并用 `pub use client::*` 平铺导出公共 API。它对应 Go 文件 [`br/pkg/backup/client.go`](./client.go)，承担 BR 快照备份客户端的编排职责，而真正拆分并发送单个 store 请求、维护 range 树和备份 schema 的细节分别下沉到 `store.rs`、`limit.rs`、`stubs.rs` 与 `schema.rs`。

RustCodeGraph 显示本文件被 `br/cmd/br/debug.rs`、`br/pkg/kms/aws.rs` 以及 `client_test.rs`、`parity_test.rs`、`schema_test.rs` 直接使用；其中调试命令会读取 `Client::GetStorage`。但核心构造函数 `NewBackupClient` 的 Rust 调用者目前是 `NewTableBackupClient` 和测试夹具，`BackupRanges` 的直接调用链也停留在本模块和测试；`br/pkg/task/backup.rs` 当前仍通过 task 层接口/桩组织备份。因此，本文件是已接入 crate 导出面的 Rust 移植实现，不能据此宣称 Rust BR 命令主链已经完全改用该实现。

源文件顶部已有 `// Copyright 2026 AsterSQL.` 与 PingCAP Apache-2.0 版权行；本任务只新增说明，不修改运行时代码。

## 核心职责

本文件把一次备份分成四类工作：

1. `Client` 保存 PD/存储/GC/锁解析依赖和本次备份配置，选择或复用 backup TS，并绑定外部存储、加密参数、GC TTL 与 checkpoint。
2. `BuildBackupRangeAndInitSchema`、`BuildBackupSchemas` 和 `WriteBackupDDLJobs` 从指定快照的元数据生成 key ranges、schema、placement policy 与增量 DDL 元数据。
3. `BackupRanges` 建立 `ProgressRangeTree`，启动 store 拓扑观察，组装 `MainBackupLoop`，再由 `RunLoop` 向存活 TiKV 多轮派发未完成范围。
4. `OnBackupResponse` 将成功响应写入 checkpoint/进度树，收集锁冲突，并把不可恢复错误转换成备份失败；最终通过进度树是否清空及 checksum map 判断任务是否完整。

本模块不直接创建真实网络连接：`ClientMgr` 提供 PD、TiKV BackupClient、KV Storage、GC Manager 与 LockResolver；外部存储由 `objstore::New` 创建；RPC 的单 store 发送由 `store::startBackup` 执行。

## 主要符号

- 常量 `MaxResolveLocksbackupOffSleepMs = 600_000`、`IncompleteRangesUpdateInterval = 15s`、`RangesSentThreshold = 30_000_000` 与 Go 同名常量对应。前两者分别约束单轮锁解析 backoff 和未完成 range 刷新节奏；`RangesSentThreshold` 在本文件中仅定义，实际消费应到发送/限流实现中核对。
- `ClientMgr: Send + Sync` 是连接和基础组件抽象。`GetBackupClient`/`ResetBackupClient` 区分连接复用与重建，其他 getter 暴露 PD、KV、GC 和锁解析器；`Close` 的调用不由本文件负责。
- `ProgressUnit` 及 `UnitRange`/`UnitRegion` 区分“整个逻辑 range 完成”和“单个 region 响应完成”两种进度事件。
- `MainBackupLoop` 是一次 `BackupRanges` 的可变运行态：包含请求模板、并发度、全局进度树、标签过滤、重试通道、内存限流器、进度回调和客户端获取回调。Rust 额外把 Go 的双向 `StateNotifier` 拆成 `Sender` 与只可 `take` 一次的 `state_rx`。
- `MainBackupSender::SendAsync` 在线程中调用 `startBackup`。非取消错误通过 `BackupRetryPolicy { One: storeID }` 通知重试；退出前向 store 私有通道发送 `None` 作为关闭标记。
- `MainBackupLoop::CollectStoreBackupsAsync` 把多个 store 的 `Receiver` 汇聚到全局通道。它用 `try_recv` 加 1ms sleep 模拟 Go 的 `reflect.Select`，并在所有 producer 结束或上下文取消时发送全局 `None`。
- `Client` 是长期配置和本次备份状态的所有者。字段包括 `mgr`、`clusterID`、可选 `storage/backend/cipher/checkpointMeta/checkpointRunner`、`apiVersion`、`gcTTL`、`tableRange` 和 `skipChecksum`。
- `NewBackupClient` 从 PD 读取 cluster ID 并初始化默认状态；`NewTableBackupClient` 额外设置 `tableRange = true`，使进度树以解码出的 table ID 作为 checksum 的 physical ID。
- 配置/查询方法包括 `SetCipher`、`SetSkipChecksum`、`GetCurrentTS`、`GetTS`、`SetLockFile`、`GetSafePointID`、`SetGCTTL`、`GetGCTTL`、`GetStorageBackend`、`GetStorage`、`GetClusterID`、`GetApiVersion` 与 `SetApiVersion`。
- 存储和 checkpoint 方法包括 `SetStorageAndCheckNotInUse`、内部方法 `CheckStorageNotInUse`、`CheckCheckpoint`、`StartCheckpointRunner`、`GetCheckpointRunner`、`WaitForFinishCheckpoint`、`SetStorage` 和自由函数 `CheckBackupStorageIsLocked`。
- 备份主链方法包括 `BuildBackupRangeAndSchema`、`BuildProgressRangeTree`、`BackupRanges`、私有 `getBackupStores`、`OnBackupResponse` 与 `RunLoop`。
- 元数据自由函数 `BuildBackupRangeAndInitSchema`、`BuildBackupSchemas`、`skipUnsupportedDDLJob`、`WriteBackupDDLJobs` 分别负责 range/schema 初始化、schema 物化、DDL 类型过滤和增量 DDL 写入。
- `pub use glue::Progress` 是兼容性重导出，不在本文件实现进度对象。

## 执行流程

典型调用顺序如下：

1. 调用方用 `NewBackupClient` 或 `NewTableBackupClient` 注入 `ClientMgr`。构造函数立即从 PD 读取 cluster ID。
2. 调用方设置外部存储、加密、GC TTL 和 checksum 策略。`SetStorageAndCheckNotInUse` 先创建存储句柄，再拒绝已有 `backupmeta` 的目录；若存在 checkpoint 元数据则加载并允许续跑，否则执行 lock+SST 冲突检查。
3. `GetTS` 优先复用 checkpoint 的 `BackupTS`；否则选择显式 TS 或当前 PD TSO，并按 `duration` 回拨，最后用 `gc::CheckGCSafePoint` 验证历史版本仍可读取。
4. 如启用 checkpoint，`CheckCheckpoint` 防止配置哈希漂移，`StartCheckpointRunner` 首次保存 `GCServiceId/ConfigHash/BackupTS`，续跑时设置 `LoadCheckpointDataMap`，随后启动追加完成区间的 runner。
5. `BuildBackupRangeAndInitSchema` 遍历匹配的数据库和表，用 `distsql::BuildTableRanges` 生成表/索引范围并经过 storage codec 编码；全量备份还序列化 placement policy。它预先物化 `BuildBackupSchemas` 的结果，构造可重复消费的 `Schemas` 回调。
6. `BackupRanges` 调用 `BuildProgressRangeTree` 插入所有原始范围；续跑时回放 checkpoint 文件、补写 data-file 元数据、合并 checksum 并校正 summary 开始时间。随后它启动 `ObserveStoreChangesAsync`，建立重试通道和内存限流器，进入 `RunLoop`。
7. `RunLoop` 每轮重新计算未完成范围；为空即成功。它从 PD 列出 TiKV（跳过 TiFlash），可按 replica label 收窄目标，然后为每个存活 store 获取或重置客户端，调用 `SendAsync`。
8. `CollectStoreBackupsAsync` 汇聚响应。`RunLoop` 同时处理拓扑/单 store 重试通知和响应：集群级变化开启新一轮；单 store 通知在同一 round 内重建该 store 的发送与汇聚；成功响应交给 `OnBackupResponse`；锁冲突累积到本轮结束后统一 `ResolveLocksForRead`。
9. `OnBackupResponse` 在成功时先追加 checkpoint，再把响应覆盖范围和 SST 文件放入进度树，并学习 TiKV API version；锁错误返回 `Lock`；其他错误交给 `HandleBackupError`，只有 `GiveUp` 策略立即上抛。
10. `RunLoop` 返回后，`BackupRanges` 再检查进度树长度；非零表示存在遗漏并报错，清空后返回按 physical ID 汇总的 checksum map。

增量元数据链独立于上述 RPC 循环：`WriteBackupDDLJobs` 读取前后快照的 schema version，合并当前 DDL 列表与 history iterator，保留 `(lastSchemaVersion, backupSchemaVersion]` 内处于 Done/Synced 的支持作业，清除 placement 信息，按 job ID 升序序列化并写入 `MetaWriter`。

## 数据与状态

`Client` 的状态是逐步装配的，而非构造后立即可运行。`storage` 与 `backend` 在 `SetStorage*` 后才存在；调用 `SetLockFile`、checkpoint 或 checkpoint 回放前必须已设置 storage。`checkpointMeta` 同时决定 TS、GC service ID、配置哈希、历史 checksum 和是否回放已完成区间，因此续跑时不得用新的配置或时间戳覆盖它。

`ProgressRangeTree` 是完成性事实来源。每个输入 `KeyRange` 成为一个 `ProgressRange`；普通备份的 `PhysicalID` 为 0，表级备份通过 `tablecodec::DecodeTableID(StartKey)` 得到表 ID。成功响应调用 `Put` 缩小洞位；checkpoint 回放使用 `PutForce(..., files=None, false)`，因为 SST 元数据已经持久化。`skipChecksum` 只阻止 checksum 累加，不改变 range 完成判定或数据写出。

`BackupRequest.SubRanges` 不是固定输入：`RunLoop` 每轮及运行中定期用 `GetIncompleteRanges` 覆盖它。锁解析成功后，`ResolvedLocks` 与 `CommittedLocks` 被追加到请求的可选 context；raw/txn 请求没有 context 时只完成解析，不写这两个列表。

`apiVersion` 初始为 V1，并由成功响应的数值映射为 V1、V1TTL 或 V2。`clusterID` 在构造时固定。`gcTTL` 的非正输入会回落到 `DefaultBRGCSafePointTTL`，但构造函数本身将其初始化为 0，因此调用方仍应显式调用 `SetGCTTL`。

## 依赖与调用关系

上游关系以 RustCodeGraph 为准：`BackupRanges -> BuildProgressRangeTree -> getProgressRange`，随后 `BackupRanges -> RunLoop`；`RunLoop -> getBackupStores / BackupSender::SendAsync / CollectStoreBackupsAsync / OnBackupResponse / LockResolver::ResolveLocksForRead`。`OnBackupResponse -> SetApiVersion`，`BuildBackupRangeAndSchema -> BuildBackupRangeAndInitSchema -> BuildBackupSchemas`，`WriteBackupDDLJobs -> skipUnsupportedDDLJob`。

主要下游依赖如下：

- `crate::store`：`BackupSender`、`MainBackupSender` 调用的 `startBackup`、store 拓扑观察和 `ResponseAndStore`。
- `crate::limit`：`NewResourceMemoryLimiter` 限制同时序列化/在途请求资源。
- `crate::schema`：`NewBackupSchemas` 和 `Schemas` 承接 schema/统计元数据写入。
- `crate::stubs::rtree`：进度树、原始 key range 和 checksum 聚合。
- `checkpoint` 与 `metautil::MetaWriter`：续跑元数据、完成区间和文件/DDL payload 的持久化。
- `conn`、`PdClient`、`BackupClient`：store 发现、集群身份、TSO 与备份 RPC。
- `meta`、`ddl`、`distsql`、`tablecodec`、`model`：快照元数据、DDL 历史、表范围与 physical ID。
- `gc`、`txnlock`、`utils`：GC safepoint、读锁解析、错误分类、文件摘要和 store 存活检查。

Cargo 清单只声明 `serde` 和 `serde_json`；TiDB/BR 侧能力来自 crate 内本地模块和 `stubs.rs`，清单还明确标注该移植避免 arm64 上的 kv/domain/kvproto/grpcio/distsql 重依赖。这意味着本模块接口形状与 Go 对齐，但真实生产依赖的完备程度必须结合 stubs 与上层接线判断。

## 错误处理与边界

- `GetTS` 拒绝未来显式 TS、回拨越过 Unix epoch/发生回绕的超大 `duration`，并拒绝早于 GC safepoint 的版本。Rust `std::time::Duration` 无法表达负值，所以 Go 的负 `timeago` 分支不能由正常 Rust 类型输入直接触发；测试只保留同类错误契约说明。
- `SetLockFile`、checkpoint 启动和 checkpoint 回放在 storage 未设置时返回 `storage not set`；`CheckStorageNotInUse` 内部直接 `unwrap` storage，仅应由已经成功执行 `SetStorage` 的封装入口或受控测试调用。
- 已有 `backupmeta` 表示目录包含完成备份，必须拒绝覆盖。无 checkpoint 时，lock 文件与任意 `.sst` 共存也拒绝；仅 lock 或普通文件不构成冲突。存在 checkpoint 元数据时允许 lock+SST 并加载续跑状态。
- checkpoint 配置哈希不一致必须失败；`BuildBackupRangeAndSchema` 只在同时存在 `Schemas` 和 checkpoint checksum 时注入历史 checksum。
- schema 构建跳过未匹配 schema/table、内存库与模板系统库；空数据库仍计入 schema 并回调 `(db, None)`；没有任何匹配对象时返回空 ranges、`None` schemas 和空 policies。表元数据版本高于当前支持上限立即报错。
- `BuildBackupSchemas` 将 AutoInc/AutoRand 值转换为“下一个可用 ID”，对非全量备份清除 placement，仅保留 public 索引，并把 cached table 标记为普通表。分离自增表的 RowID 读取失败在已有全局 AutoID 时被容忍，否则失败。
- replica label 非空但无 store 匹配时失败；失活 store 在当前轮跳过。获取 store 或客户端失败被视为可重试并进入下一轮，可能在持续故障下无限重试，退出依赖 context 取消。
- `OnBackupResponse` 对 tree 外的完整不相交响应不推进进度；交叠但不完全包含由进度树返回错误。锁错误延迟到轮末解析；其他错误由策略决定重试或 `GiveUp`。锁解析失败当前被忽略，等待下一轮再次处理。
- `WriteBackupDDLJobs` 过滤 placement/attribute 类作业、缺少合适 BinlogInfo 或不在版本窗口中的作业；序列化或 `MetaWriter::Send` 失败直接传播。

## 并发与资源生命周期

Rust 用 `std::thread::spawn` 和 `std::sync::mpsc` 模拟 Go goroutine/channel。每个 store 的 `MainBackupSender` 线程必须在退出前发送一次 `None`；汇聚线程收到 `None` 或断连后移除相应 receiver，全部结束后向 global channel 发送 `None`。如果新增发送实现遗漏关闭标记，汇聚循环可能永久等待。

`MainBackupLoop::state_rx` 通过 `Option::take` 保证只有一个 `RunLoop` 消费者；对同一个 loop 二次调用会返回 `state notifier receiver missing`。`GlobalProgressTree`、限流器和 checkpoint runner 用 `Arc` 跨线程共享，store receiver 又包在 `Arc<Mutex<_>>` 中供汇聚线程管理。

每轮创建 `mainCtx` 和 `handleCtx`：前者控制发送，后者控制汇聚/处理。全量拓扑变化同时取消两者并重开一轮；单 store 重试取消旧 handle context，复用 main context 重建该 store 和全局汇聚通道。外层 context 取消会尽快中止，但 `ClientMgr::Close` 与 `WaitForFinishCheckpoint` 仍由调用方在更高层生命周期中显式执行。

进度刷新默认为 15 秒；测试可通过 `should_skip_round_sleep` 跳过每轮 200ms sleep。汇聚器在没有消息时每 1ms 轮询一次，这与 Go 的阻塞 `reflect.Select` 不同，可能增加空闲 CPU 消耗，是性能审查点。`rangeLimit` 通过 `ResourceConcurrentLimiter` 控制请求资源；共享 `FreeListG(10240)` 降低大量 range 节点分配成本。

## 与 Go 版本的对应关系

结构上，Rust 逐项对应 Go 的 `ClientMgr`、`ProgressUnit`、`MainBackupLoop`、`MainBackupSender`、`Client` 及同名函数/方法；常量、主要分支、错误类别、checkpoint 语义、schema 清洗和 DDL version 窗口都保留。独立测试文件中的注释和用例名称也直接对应 Go 的 `TestGetTS`、`TestGetHistoryDDLJobs`、`TestSkipUnsupportedDDLJob`、`TestCheckBackupIsLocked`、`TestOnBackupResponse`、`TestMainBackupLoop`、`TestBuildProgressRangeTree` 与 `TestObserveStoreChangesAsync`。

需要特别留意的移植差异：

- Rust 的 `BuildBackupRangeAndInitSchema`/`BuildBackupSchemas` 接受注入的 `meta::Reader`，因为本地 `Storage`/snapshot 是轻量抽象；Go 在函数内从指定 snapshot 新建 reader。Rust 为满足 `'static` schema 回调，先物化 DB/table 结果和错误。
- Rust 的 `WriteBackupDDLJobs` 同样额外注入 `lastSnapMeta`、`snapMeta`、`newestMeta`；Go 从 `store` 的三个版本快照内部构建这些 reader。
- Go 使用 goroutine、阻塞 channel 和 `reflect.Select`；Rust 使用 OS thread、`mpsc`、`Option<ResponseAndStore>` 关闭标记及轮询。
- Go 直接比较 `context.Canceled`；Rust `MainBackupSender` 以错误消息包含 `context canceled` 或 `ctx.Done()` 近似判断。错误包装变化可能影响是否错误触发单 store 重试。
- Go 的 `time.Duration` 可为负，Rust `Duration` 不可为负；Rust 的负 `timeago` 检查实际上不可由类型安全调用触发。
- Go 的 `OnBackupResponse` 使用协议枚举类型；Rust 根据响应的整数值映射 API version，未知值回落到 V1。
- Go checkpoint 回放假定 `metaWriter` 可用并直接发送文件；Rust 将其建模为 `Option<Arc<dyn MetaWriter>>`，为 `None` 时仍推进树和 checksum，但不发送文件元数据。
- 当前 Rust 核心 API 的图上调用者主要是测试，生产命令接线尚不能从本文件证明；Go `RunBackup` 等生产调用链不能直接外推为 Rust 已接线事实。

## 扩展指南

- 新增备份配置字段时，应同时检查 `Client`、构造函数、checkpoint 配置哈希的生成方、`StartCheckpointRunner` 元数据以及 `br/pkg/backup/parity_test.rs`，避免续跑接受不兼容配置。
- 修改发送、重试或拓扑行为时，入口是 `MainBackupSender::SendAsync`、`MainBackupLoop::CollectStoreBackupsAsync` 和 `Client::RunLoop`；必须保持每个 producer 的关闭标记、context 取消顺序、同轮单 store 重试及下一轮全量 reset 不变量。同步扩展 `client_test.rs::test_main_backup_loop` 和 `test_single_store_retry_stays_in_the_same_round`。
- 修改响应错误分类或 checkpoint 写入顺序时，应改 `Client::OnBackupResponse`，并覆盖可重试、GiveUp、tree 外响应、部分/完整区间、锁错误和 checkpoint 失败；现有入口是 `client_test.rs::test_on_backup_response`。
- 新增存储占用规则时，应共同审查 `SetStorageAndCheckNotInUse`、`CheckStorageNotInUse`、`CheckBackupStorageIsLocked` 和 checkpoint 加载路径；同步测试 `test_check_backup_is_locked` 与 `test_storage_lock_check_respects_checkpoint_mode`，防止破坏合法续跑。
- 新增可备份对象或 schema 字段时，应改 `BuildBackupRangeAndInitSchema`/`BuildBackupSchemas`，保持 codec 编码、空库、表版本、AutoID、placement 和 public-index 约束；同步 `schema_test.rs` 与 `test_build_backup_range_returns_working_schema_iterator`。
- 新增 DDL 类型时，应判断其是否可由恢复侧安全重放，再更新 `skipUnsupportedDDLJob` 和 `WriteBackupDDLJobs`，同时补 `test_skip_unsupported_ddl_job`、`test_get_history_ddl_jobs` 及 parity 测试。
- 若要把本模块接入 Rust BR 生产主链，应先从 `br/pkg/task/backup.rs` 的现有 task 接口追踪并替换局部接线，不应仅因 `lib.rs` 已导出符号就删除 task 侧兼容层。需额外验证真实 `ClientMgr`、真实对象存储、PD/TiKV RPC、取消与 checkpoint 收尾；当前本地 stub 测试不能替代这些集成证据。
- 性能敏感修改应评估 1ms 汇聚轮询、每 store OS thread、15 秒刷新间隔、`rangeLimit` 和 `FreeListG` 容量，避免为了 Go 表面一致而引入忙等或无界线程增长。

Rust 测试必须继续放在独立的 `client_test.rs`、`schema_test.rs` 或 `parity_test.rs`，不要内嵌到 `client.rs`。

## 验证依据

本说明依据以下直接证据编写：

- Rust 源码：[`br/pkg/backup/client.rs`](./client.rs)，完整检查了 1–1518 行的常量、trait、结构体、impl、自由函数及重导出；该文件无条件编译分支。
- crate 边界：[`br/pkg/backup/Cargo.toml`](./Cargo.toml) 与 [`br/pkg/backup/lib.rs`](./lib.rs)。前者确认 package、library 入口、本地轻依赖和 Go package 映射，后者确认模块注册、测试模块和公共重导出。
- Go 对照：[`br/pkg/backup/client.go`](./client.go)，核对了同名类型、发送/汇聚/多轮循环、Client 配置、存储/checkpoint、schema、DDL、进度树和响应处理。
- Rust 测试：[`br/pkg/backup/client_test.rs`](./client_test.rs)、[`br/pkg/backup/schema_test.rs`](./schema_test.rs)、[`br/pkg/backup/parity_test.rs`](./parity_test.rs)。关键覆盖包括 TS/GC 边界、DDL 过滤与排序、lock+SST、checkpoint 续跑、可工作的 schema iterator、响应分类、store 迁移/取消/同轮重试、进度树边界与 Go/Rust 公共契约。
- Go 测试：[`br/pkg/backup/client_test.go`](./client_test.go)，确认对应测试意图和命名，包括 `TestGetTS`、`TestGetHistoryDDLJobs`、`TestSkipUnsupportedDDLJob`、`TestCheckBackupIsLocked`、`TestOnBackupResponse`、`TestMainBackupLoop`、`TestBuildProgressRangeTree`、`TestObserveStoreChangesAsync` 与 range 拆分测试。
- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点、1,848,419 条边，其中 Rust 文件 7,032 个。执行了针对 `br/pkg/backup/client.rs` 的 `explore`、分段 `node --file`、`query` 以及 `node NewBackupClient`/`node WriteBackupDDLJobs`，据此核对本文件使用者、主要内部调用边和当前核心 API 的上游接线范围。

本任务是纯文档分析，按计划不运行 Cargo。人工复核结论是：本文能够回答该文件为何存在、配置和备份循环如何运行、状态/错误/并发生命周期如何约束，以及扩展时应修改哪些符号与独立测试；凡是不能由当前调用图证明的生产接线均已明确标为未完成证明，而没有按 Go 设计推测 Rust 现状。
