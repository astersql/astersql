# `br/pkg/checkpoint/manager.rs`

## 文件定位

`manager.rs` 属于 Cargo crate `astersql-br-pkg-checkpoint`。该 crate 的入口是 [`br/pkg/checkpoint/lib.rs`](./lib.rs)，入口先装配 `checkpoint`、`external_storage`、`storage` 等底层模块，再挂载本文件并通过 `pub use manager::*` 展平公开 API。Cargo 清单 [`br/pkg/checkpoint/Cargo.toml`](./Cargo.toml) 将它声明为 library crate；本文件直接使用 `serde` 的序列化约束，其余会话、对象存储、Domain 和错误类型由 crate 内 `stubs.rs` 提供。

本文件是恢复检查点的“后端选择与生命周期编排层”，不是检查点文件格式或 SQL 细节的实现层。它把快照恢复和日志恢复需要的管理操作抽象为 `SnapshotMetaManager`、`LogMetaManager`，再分别用 TiDB 系统表后端 `TableMetaManager` 和外部对象存储后端 `StorageMetaManager` 实现。实际刷盘循环在 [`checkpoint.rs`](./checkpoint.rs) 的 `CheckpointRunner`，表读写在 [`storage.rs`](./storage.rs)，对象路径和对象存储适配在 [`external_storage.rs`](./external_storage.rs)。

RustCodeGraph 的文件索引显示本文件由 `checkpoint_test.rs`、`restore.rs`、`log_restore.rs`、`storage.rs`、`external_storage.rs` 等 7 个文件使用。直接引用搜索进一步确认：正式的恢复适配函数 `StartCheckpointRunnerForRestore` 与 `StartCheckpointRunnerForLogRestore` 接收 trait 对象并调用本文件的 `StartCheckpointRunner`；当前 Rust 构造器的直接实例化主要出现在 checkpoint crate 测试、`br/pkg/task/stream_test.rs` 等测试路径，上层生产模块通常持有并消费 `dyn LogMetaManager`/`dyn SnapshotMetaManager`，不能据此推断所有 Go 构造接线均已迁移。

## 核心职责

1. 定义恢复检查点的稳定接口：通用的元数据、range 数据、checksum、存在性、清理、Runner 启动和关闭；日志恢复额外包括进度、摄入索引修复 SQL 与底层 Storage 暴露。
2. 选择持久化后端：`TableMetaManager` 把状态放在按 restore ID 隔离的 TiDB 数据库表中，`StorageMetaManager` 把状态放在按 cluster/task 隔离的对象路径中。
3. 组装 Runner：把后端转换为 `CheckpointStorage`，注入 value marshaler、可选 cipher 和 tick/retry 配置，然后启动恢复专用主循环。恢复路径不需要 checkpoint lock，因此 lock tick 固定为 `Duration::ZERO`。
4. 管理资源所有权：表后端为普通元数据操作和 Runner 各持有一个 session；启动 Runner 时转移 runner session，关闭 manager 时释放尚未转移的 session。对象存储后端持有 `Arc<dyn Storage>`，无需显式关闭。
5. 保持 Go 契约：数据库名、任务路径、方法集合和主要调用顺序对齐 [`manager.go`](./manager.go)，同时用 Rust trait 对象、`Arc`、`Mutex<Option<_>>` 和固定类型拆分表达所有权与线程安全。

本文件不负责 range 的合并/序列化算法、重试循环细节、表 DDL/SQL 或对象加解密实现；这些职责分别下沉到 `checkpoint.rs`、场景 marshaler、`storage.rs` 和 `external_storage.rs`。

## 主要符号

### 配置与接口

- `SnapshotMetaManagerT = dyn SnapshotMetaManager`、`LogMetaManagerT = dyn LogMetaManager`：与 Go 的公开别名对应的 trait-object 别名。实际传递时通常还需 `&dyn ...`、`Box<dyn ...>` 或 `Arc<dyn ...>`。
- `tickDurationConfig`：包含 `tickDurationForFlush`、`tickDurationForChecksum`、`retryDuration`。字段公开但类型名不是公开命名风格；场景测试会覆盖字段以缩短等待。
- `DefaultTickDurationConfig()`：从 `checkpoint.rs` 的 `defaultTickDurationForFlush`、`defaultTickDurationForChecksum`、`defaultRetryDuration` 构造正式默认配置。
- `SnapshotMetaManager: Send + Sync`：固定使用 `RestoreKeyType`、`RestoreValueType` 和 `CheckpointMetadataForSnapshotRestore`，提供 data/checksum/meta 的加载保存、存在性、清理、Runner 启动与关闭。
- `LogMetaManager: Send + Sync`：固定使用 `LogRestoreKeyType`、稀疏写入值 `LogRestoreValueType`、落盘读取值 `LogRestoreValueMarshaled` 和 `CheckpointMetadataForLogRestore`；额外提供 progress、ingest-index repair SQL 与 `TryGetStorage()`。

### 表后端

- `TableMetaManager`：字段 `se` 与 `runnerSe` 均为 `Mutex<Option<Box<dyn Session>>>`；`dom` 用于 InfoSchema 查询和删表；`dbName` 是真实 checkpoint 数据库名；`kind` 记录构造角色。当前方法分派由两个 trait impl 决定，`kind` 本身未参与分支判断。
- `TableManagerKind::{Snapshot, Log}`：构造时写入的角色标记，目前仅保留角色信息，没有运行时行为。
- `NewSnapshotTableMetaManager()`、`NewLogTableMetaManager()`：各创建两个 session，并把数据库名变为 `{base}_{restoreID}`，分别返回 `Box<dyn SnapshotMetaManager>` 与 `Box<dyn LogMetaManager>`。
- `TableMetaManager::with_session()`：在 `se` 的 mutex 锁内执行闭包；`Close()` 已取走 session 时返回 `Error("session closed")`。
- 两个 trait impl：通用 data/checksum/meta 操作委托给 `selectCheckpointData`、`selectCheckpointChecksum`、`selectCheckpointMeta`、`insertCheckpointMeta`；日志 impl 还用 progress/ingest 表名复用同一套单行元数据读写函数。

### 外部存储后端

- `StorageMetaManager`：保存 `Arc<dyn Storage>`、可选 `CipherInfo`、字符串形式的 `clusterID` 和 `{clusterID}/{prefix}_{restoreID}` 形式的 `taskName`；`kind` 与表后端一样目前不驱动逻辑。
- `StorageManagerKind::{Snapshot, Log}`：记录构造角色，实际 API 仍由 trait impl 隔离。
- `NewSnapshotStorageMetaManager()`、`NewLogStorageMetaManager()`：不做 I/O，直接构造相应 trait object；同一个 Storage 可以通过不同 prefix/restore ID 隔离任务。
- 两个 trait impl：data 使用 `walkCheckpointFile`，checksum 使用 `loadCheckpointChecksum`，meta/progress/ingest 使用路径辅助函数加 `loadCheckpointMeta`/`saveCheckpointMetadata`，存在性直接调用 `Storage::FileExists`。
- `_unused_table_info()`、`_unused_ser()`：仅维持迁移期泛型/类型约束引用，未进入业务流程，不应被当成公开扩展点。

## 执行流程

### 构造与后端选择

表后端构造器依次调用 `Glue::CreateSession(dom.Store())` 两次，分别供同步元数据访问和异步 Runner 写入使用，然后生成带 restore ID 的数据库名。外部存储构造器只保存共享 Storage、cipher 和任务路径。选择由上层完成；本文件不会自动在两种后端间切换或降级。

### 保存和恢复元数据

对表后端，`SaveCheckpointMetadata` 先通过 `initCheckpointTable` 确保 data/checksum 表存在，再通过 `insertCheckpointMeta` 写 meta 表；加载时由 restricted SQL executor 调用 `selectCheckpointMeta` 并填充默认构造的元数据对象。progress 与 ingest repair SQL 使用各自表名复用这套机制。存在性检查只查询 InfoSchema 中对应 meta/progress/ingest 表。

对对象存储后端，保存/加载直接根据 `taskName` 计算 meta、progress 或 ingest-index 路径并调用通用 JSON 元数据辅助函数。存在性调用 `FileExists`，不会读取并反序列化对象；因此“存在”不等于内容可解析。

### 加载已完成范围与 checksum

表后端借用 `se` 的 restricted SQL executor，分别调用 `selectCheckpointData` 和 `selectCheckpointChecksum`。对象存储后端以 data/checksum 目录调用 `walkCheckpointFile` 和 `loadCheckpointChecksum`。两种后端都返回历史累计耗时；data 通过调用方提供的 `FnMut` 逐条消费，回调错误会沿 `Result` 返回。

### 启动 Runner

快照和日志 trait 的 `StartCheckpointRunner` 流程相同，但键值类型和 marshaler 不同：

1. 表后端从 `runnerSe` 中 `take()` session，构造 `tableCheckpointStorage`；对象存储后端调用 `newExternalCheckpointStorage(ctx, storage, None, flushPathForRestore(taskName))`。
2. 调用 `newCheckpointRunner`，表后端不传 cipher，对象后端传入 manager 的可选 cipher。
3. 调用 `startCheckpointMainLoop`，传入 flush/checksum/retry 周期，并把 lock 周期固定为零。
4. 返回已经启动的 `CheckpointRunner`。快照入口由 `restore.rs::StartCheckpointRunnerForRestore` 注入 `valueMarshalerForRestore`；日志入口由 `log_restore.rs::StartCheckpointRunnerForLogRestore` 注入 `valueMarshalerForLogRestore`。

表后端启动成功后 manager 不再拥有 runner session，因此同一实例第二次启动会得到 `runner session missing`。对象后端不消耗 Storage，代码层面允许再次构造 Runner；是否应并发运行多个同 taskName 的 Runner 不由本文件协调，调用方必须避免路径级写入竞争。

### 清理与关闭

`RemoveCheckpointData` 在表后端一次传入 data、checksum、meta、progress、ingest 五张表给 `dropCheckpointTables`；对象后端删除 `checkpoints/restore-{taskName}` 前缀。`Close` 对表后端从两个 mutex 中取走并关闭仍在 manager 内的 session；已交给 Runner 的 session 不会被 manager 再次关闭。对象后端 `Close` 是空操作，底层 `Arc` 按引用计数释放。

## 数据与状态

- 表后端命名不变量：`dbName = format!("{base}_{restoreID}")`。restore ID 将同一基础数据库名下的不同恢复任务隔离。
- 对象后端命名不变量：`clusterID` 单独用于 `String()` 展示；`taskName = "{clusterID}/{prefix}_{restoreID}"` 用于所有任务文件路径。快照、日志和 SST 恢复应由调用方传入不同 prefix，否则同 restore ID 会共享路径。
- 快照 data 的写入与读取值都是 `RestoreValueType`；日志写入 Runner 接收 `LogRestoreValueType`，marshaler 压缩后读取为 `LogRestoreValueMarshaled`。这一区别被编码在两个 trait 的函数签名中。
- checksum 的公共返回形态是 `HashMap<i64, ChecksumItem>` 加历史耗时。表和对象后端只决定来源，不在 manager 层更改聚合语义。
- `CheckpointProgress` 和 `CheckpointIngestIndexRepairSQLs` 仅属于 `LogMetaManager`。`br/pkg/restore/log_client/id_map.rs::saveIDMap` 先持久化 ID map，成功后才保存 `InLogRestoreAndIdMapPersisted`，防止进度领先于数据。
- `TryGetStorage()` 是后端能力探测：表后端为 `None`，对象后端克隆 `Arc`。日志恢复 ID-map 逻辑据此优先把映射写到 checkpoint Storage，否则回退系统表或备份 Storage。
- `kind` 字段和 `_unused_*` 函数目前没有业务状态转换；它们属于迁移期结构，不应在文档或新代码中赋予未经实现的语义。

## 依赖与调用关系

### 上游调用者

- [`restore.rs`](./restore.rs)：`StartCheckpointRestoreRunnerForTest` 和 `StartCheckpointRunnerForRestore` 调用 `SnapshotMetaManager::StartCheckpointRunner`；`AppendRangesForRestore` 随后向返回的 Runner 追加完成项。
- [`log_restore.rs`](./log_restore.rs)：对应的 test/production 启动函数调用 `LogMetaManager::StartCheckpointRunner`；`GetCheckpointTaskInfo` 等流程读取 meta/progress。
- [`br/pkg/restore/log_client/id_map.rs`](../restore/log_client/id_map.rs)：通过 `TryGetStorage` 选择 ID-map 落点，并在保存映射后调用 `SaveCheckpointProgress`。
- `br/pkg/task/stream.rs` 和 `br/pkg/restore/log_client/client.rs` 持有或接收 `dyn LogMetaManager`；直接构造器在当前 Rust 搜索结果中主要由测试调用。Go 生产接线集中在 `br/pkg/task/restore.go`，这是判断 Rust 迁移完整度时必须保留的差异。

### 下游依赖

- [`checkpoint.rs`](./checkpoint.rs)：`CheckpointRunner`、`RangeGroup`、默认周期、外部文件遍历、meta/checksum 保存加载和前缀删除。
- [`storage.rs`](./storage.rs)：表名常量、建表/插入/查询/删表，以及 `tableCheckpointStorage`。
- [`external_storage.rs`](./external_storage.rs)：恢复目录格式、各类路径函数、flush 路径和 `newExternalCheckpointStorage`。
- [`restore.rs`](./restore.rs) 与 [`log_restore.rs`](./log_restore.rs)：固定场景数据类型、元数据类型和 value marshaler。
- [`stubs.rs`](./stubs.rs)：`Glue`、`Session`、`Domain`、`Storage`、`Context`、`CipherInfo`、`Result` 等迁移边界抽象。

Cargo 清单的直接依赖中，`serde` 为本文件末尾的泛型约束所用；实际 JSON 读写由下游模块使用 `serde_json`。本文件没有 feature 条件或条件编译项，测试则由 `lib.rs` 的 `#[cfg(test)]` 独立挂载，符合“源文件与测试文件分离”的仓库规则。

## 错误处理与边界

- 两个表构造器用 `?` 传播 session 创建失败；若第二次创建失败，当前 Rust 函数直接返回错误，没有在本函数中显式关闭第一次创建的 session，具体释放取决于 `Box<dyn Session>` 的 Drop 实现。Go 对照也没有在该失败分支显式 `Close`。
- `with_session` 把“已 Close”转换为 `session closed` 错误，但对 poisoned mutex 使用 `unwrap()`，线程 panic 后再次加锁会 panic 而不是返回 `Result`。
- 表 Runner session 只能消费一次；重复启动返回 `runner session missing`。如果构造 `tableCheckpointStorage` 后的后续步骤出现 panic，session 已不在 manager 中。
- `SaveCheckpointMetadata` 的 Rust 参数是非空引用，因此总会写 meta；Go 版本允许 `nil`，此时只初始化 data/checksum 表而不写 meta。这是可观察的接口差异，新增兼容调用时不能假定 Rust 支持 Go 的 nil 分支。
- `Exists*` 只检查表/文件是否存在，不验证内容完整性、JSON 类型或 checksum。调用方仍需处理随后的 load 失败。
- 所有 I/O、序列化、回调和下游 SQL 错误都通过 `Result` 原样向上传播；本层没有统一重试。Runner 的周期性重试由 `checkpoint.rs` 实现，`retryDuration` 只是配置输入。
- `RemoveCheckpointData` 是任务级批量删除；错误可能留下部分表或对象，manager 层没有事务性回滚承诺。
- `String()` 仅用于可读位置描述，不包含完整 taskName：对象存储只展示 cluster 级目录模板，不能用它唯一定位 prefix/restore ID。

## 并发与资源生命周期

两个 manager trait 都要求 `Send + Sync`。表后端用独立 mutex 保护 `se` 和 `runnerSe`，避免同步查询与 Runner 所有权转移发生 Rust 数据竞争；`with_session` 在整个 SQL 闭包执行期间持锁，因此同一 manager 上的同步表操作被串行化。InfoSchema 的 `Exists*` 只读 `dom`，不获取 session 锁。

`StartCheckpointRunner` 对 `runnerSe` 的 `take()` 是原子的单次所有权转移。之后：

- manager 的 `Close()` 只会关闭仍留在 `Option` 中的 session；
- Runner 持有的 `tableCheckpointStorage` 负责其 session 的后续生命周期；
- 同一 manager 无法启动第二个表 Runner；
- `se` 仍可用于 load/save/remove，直到 `Close()` 将其取走。

对象后端通过 `Arc<dyn Storage>` 共享句柄，`TryGetStorage` 和 Runner 创建都会增加引用计数。`cipher` 被克隆到 Runner。`Close()` 不取消 Runner、不开关 Storage，也不等待 flush；完成与强制结束应调用 `CheckpointRunner::WaitForFinish` 等 Runner API。

manager 本身没有 Drop 实现，因此调用方应显式 `Close()` 表后端，尤其是在 Runner 尚未启动时；否则是否及时释放 session 取决于字段对象的析构行为。关闭与另一个正在执行 `with_session` 的线程会竞争同一 mutex，关闭会等待当前闭包结束；关闭后新的表操作得到 `session closed`。

## 与 Go 版本的对应关系

[`manager.go`](./manager.go) 是直接语义基准。主要对应关系如下：

| Go | Rust | 说明 |
| --- | --- | --- |
| 泛型 `MetaManager[K,SV,LV,M]` | `SnapshotMetaManager` | Rust 固定快照类型，避免公开泛型组合。 |
| 泛型 `LogMetaManager[...]` | `LogMetaManager` | Rust 固定日志类型，并保留额外 progress/ingest/storage 方法。 |
| 泛型 `TableMetaManager[...]` | 单个 `TableMetaManager` + 两个 trait impl | Rust 用 trait impl 区分快照/日志。 |
| 泛型 `StorageMetaManager[...]` | 单个 `StorageMetaManager` + 两个 trait impl | 路径、cipher 和 Storage 语义对齐。 |
| `manager.runnerSe = nil` | `Mutex<Option<_>>::take()` | 都把 runner session 单次转移；Rust 重复调用显式报错。 |
| restore lock tick `0` | `Duration::ZERO` | Runner 启动参数对齐。 |
| table `TryGetStorage() == nil` | `None` | 后端能力探测对齐。 |
| storage 返回底层 Storage | `Some(self.storage.clone())` | Rust 用 `Arc` 表达共享所有权。 |

数据库名 `{dbName}_{restoreID}`、taskName `{clusterID}/{prefix}_{restoreID}`、五类表清理、progress/ingest 独立位置及 Runner 构造顺序均与 Go 对齐。明确差异包括：Rust trait 要求 `Send + Sync`；table session 有 mutex 和关闭后错误；Rust meta 保存不接受 nil；Rust 构造器返回 trait object；当前 `kind` 字段是 Rust 侧角色记录而非 Go 字段。

测试证据来自独立文件 [`checkpoint_test.rs`](./checkpoint_test.rs) 与 [`parity_test.rs`](./parity_test.rs)：两种后端都执行 snapshot/log meta 往返、progress 与 repair SQL 往返、Runner data/checksum、清理、重试和无重复写入；parity 测试额外核对公开 JSON 字段及 `InLogRestoreAndIdMapPersisted` 契约。Go 对照测试 [`checkpoint_test.go`](./checkpoint_test.go) 覆盖同类 table/storage 场景。

## 扩展指南

- 新增所有恢复后端都必须支持的能力时，同时修改 `SnapshotMetaManager`、`LogMetaManager`（若确为公共能力）、两个后端的相应 impl，并在独立的 `checkpoint_test.rs` 中分别覆盖 table/storage；不要把测试嵌入本文件。
- 新增仅日志恢复需要的状态时，优先扩展 `LogMetaManager`，为表后端分配明确表名、为对象后端分配明确路径函数，并同步 `RemoveCheckpointData` 的清理范围。还要检查 `GetCheckpointTaskInfo`、log client 的恢复决策和 Go `manager.go`。
- 新增后端时必须决定：命名隔离规则、存在性语义、批量清理边界、加密是否生效、Runner Storage 构造、是否能暴露底层 Storage，以及 `Close`/Drop 的所有权规则。
- 修改 Runner 周期或 lock 行为时，应从 `DefaultTickDurationConfig`、两个 `StartCheckpointRunner` impl 和 `checkpoint.rs::startCheckpointMainLoop` 联合检查；恢复当前明确传零 lock tick，不能照搬 backup 的锁周期。
- 修改表 session 模型时，保持“元数据 session 与 Runner session 分离”这一不变量，并增加重复启动、关闭后调用、并发关闭的独立测试。当前测试覆盖主功能，但直接搜索未发现对 `runner session missing` 或 `session closed` 的专门断言，这是值得补充的边界测试，不代表现有行为未实现。
- 修改 Go 对齐行为时，先比较同名 Go 方法。特别注意 Go `SaveCheckpointMetadata(nil)` 只建表的分支，若 Rust 需要支持，应设计显式 API，而不是用默认元数据偷偷替代 nil 语义。
- 性能风险主要在 `with_session` 的长时间持锁、对象目录遍历、清理大前缀及多个 Runner 写同一路径；兼容风险主要在数据库/对象路径命名、序列化字段和进度状态顺序。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标文件已索引；`files --filter br/pkg/checkpoint` 列出 `manager.rs`、Go 对照和全部独立测试。
- RustCodeGraph `node --file br/pkg/checkpoint/manager.rs --offset 1 --limit 420` 与 `--offset 421 --limit 600`：读取目标文件 951 行全貌，确认两个 trait、两个 manager、四个构造器、四组 trait impl、资源转移与所有路径。
- RustCodeGraph `query NewSnapshotTableMetaManager`、`query NewLogStorageMetaManager`：同时定位 Go/Rust 同名构造器。精确 `callers/callees` 命令在限定时间内未返回，因此使用 `rg` 的直接引用结果补充调用边，没有把缺失图结果猜成调用关系。
- 源码与配置：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`manager.go`](./manager.go)、[`restore.rs`](./restore.rs)、[`log_restore.rs`](./log_restore.rs)、[`br/pkg/restore/log_client/id_map.rs`](../restore/log_client/id_map.rs)。
- 独立测试：[`checkpoint_test.rs`](./checkpoint_test.rs)、[`parity_test.rs`](./parity_test.rs) 和 Go [`checkpoint_test.go`](./checkpoint_test.go)。重点场景为 `test_checkpoint_meta_for_restore_*`、`test_checkpoint_restore_runner_*`、`test_checkpoint_runner_retry_*`、`test_checkpoint_runner_no_retry_*`、`test_checkpoint_log_restore_runner_*`、`go_rust_public_contract_matches`。
- 任务是纯文档分析，按计划未运行 Cargo。交付结构检查要求本文恰含规定的 11 个二级标题，并人工复核公开符号、调用边、Go 差异、错误与生命周期均有源码依据。
