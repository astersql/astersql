# `br/pkg/task/restore_lifecycle.rs`

## 文件定位

该文件属于 `astersql-br-pkg-task` crate，由 `br/pkg/task/lib.rs` 以 `pub mod restore_lifecycle` 挂载。它位于任务编排层与 `astersql-br-pkg-restore`、`astersql-br-pkg-restore-log-client` 两个恢复实现 crate 之间：任务层先从备份元数据筛选文件，再通过 `Glue::GetRestoreLifecycle` 取得由宿主注入的真实 PD、调度器、导入器及已准备文件集，最后由本文件统一执行恢复前置工作、SST 导入和退出清理。

`br/pkg/task/Cargo.toml` 将本 crate 定义为对应 Go 包 `br/pkg/task` 的 library，并直接依赖 `../restore`、`../restore/log_client` 与 `../conn`；因此本文件不是独立命令入口，也不是仅供测试的桩。公开模块入口及直接调用点见 `br/pkg/task/lib.rs`、`br/pkg/task/restore.rs`、`br/pkg/task/restore_raw.rs`、`br/pkg/task/restore_txn.rs` 和 `br/pkg/task/stream.rs`。

## 核心职责

1. 用 `RestoreKind` 标识 Snapshot、Raw、Txn、Stream 四种恢复入口，使宿主工厂能够按任务类型构造正确的运行时资源。
2. 用 `RestoreImporter` 保留普通 `FileImporter` 与快照专用 `BalancedFileImporter` 的能力差异，避免快照多表恢复退化为简单导入路径。
3. 用 `RestoreLifecycle` 聚合一次恢复所需的 context、PD 客户端、scheduler 管理器、导入模式传输层、文件集、checkpoint runner、checkpoint 已恢复大小和可选 key ranges。
4. 用 `RestoreGlue` 在不改变既有 `Glue` 行为的前提下，仅覆盖 `GetRestoreLifecycle`，把运行时资源的所有权留给嵌入服务。
5. 用 `Start`/`RestoreSession::drop` 成对管理 import mode 与 scheduler 暂停/恢复，并分别驱动普通 SST 与日志恢复 compacted SST。
6. 用 `ValidateFiles` 防止工厂构造的 `BatchBackupFileSet` 静默遗漏或替换任务层已经选中的备份文件。

## 主要符号

- `RestoreKind`：可复制的公开枚举。`Snapshot` 选择 balanced、多表 restorer；`Raw`、`Txn` 选择 simple restorer；`Stream` 进入 compacted SST 路径。
- `RestoreImporter::{Simple, Snapshot}`：保存 `Arc<dyn FileImporter>` 或 `Arc<dyn BalancedFileImporter>`。`FileImporter()` 对两种变体都返回基础 trait object 的克隆，供统一 `Close` 和 simple/log-client 接口使用。
- `RestoreLifecycle`：公开运行时资源包。`file_sets` 保存表 ID、rewrite rules 和完整 SST 元数据；`key_ranges` 仅在需要细粒度暂停 scheduler 时有意义；两个 checkpoint 字段分别提供状态写入器和 compacted SST 已完成字节数。
- `RestoreLifecycleFactory`：`Arc<dyn Fn(RestoreKind, &[File]) -> Result<RestoreLifecycle> + Send + Sync>`，允许多个任务入口并发共享工厂，同时把选中的文件传回资源拥有方准备精确文件集。
- `RestoreGlue<'a>`：委托 `GetVersion`、`StartProgress`、`Record`、`ConsoleOutWrite` 给 `inner`，只把 `GetRestoreLifecycle` 转给 `factory`。
- `RestorePD`：把 `astersql_br_pkg_conn::StoreMeta` 适配为 restore crate 的 `PdClient`；`GetAllStores(true)` 保留 store labels，供 TiFlash 等过滤逻辑使用。
- `RestoreSession`：内部 RAII 守卫；持有 mode switcher、undo 函数、online 标记和是否恢复 scheduler 的决策位。
- `RestoreLifecycle::{ValidateFiles, Start, RestoreFiles, RestoreCompactedSST}`：分别承担输入一致性、前置资源切换、普通备份文件恢复和日志 compacted SST 恢复。

## 执行流程

普通 Snapshot/Raw/Txn 流程如下。

1. `restore.rs`、`restore_raw.rs` 或 `restore_txn.rs` 从元数据得到 `File` 列表，以相应 `RestoreKind` 调用 `Glue::GetRestoreLifecycle`。
2. 调用者立即执行 `ValidateFiles`。该函数分别提取选中文件和 `file_sets[*].SSTFiles` 的 `(Name, StartKey, EndKey, Cf, Size_)`，排序后比较；集合不一致即停止。
3. `RestoreFiles` 调用 `Start`。快照传入 `key_ranges`，其他类型不传；有 ranges 时走 `FineGrainedRestorePreWork`，否则走 `RestorePreWork`。
4. checkpoint 被请求但没有 `checkpoint_runner` 时返回配置错误。并发数通过 `concurrency.max(1)` 保证 worker pool 至少一个 worker。
5. Snapshot 必须匹配 `RestoreImporter::Snapshot`，并创建 `NewMultiTablesRestorer`；其他 kind 创建 `NewSimpleSstRestorer`。随后顺序调用 `GoRestore` 和 `WaitUntilFinish`。
6. 闭包结束后无论前置工作、配置检查还是导入是否失败，都尝试调用 importer 的 `Close`；关闭错误被忽略，主恢复结果原样返回。
7. `RestoreSession` 离开作用域时执行后置工作。普通恢复总是恢复 scheduler；checkpoint 模式下若导入失败，则只停止 mode 定时刷新并保留 scheduler 暂停状态供重试，成功时仍执行完整 `RestorePostWork`。

Stream 流程由 `stream.rs::restoreStreamBody` 以空的初始文件参数取得 lifecycle，再调用 `RestoreCompactedSST`。该方法根据 `ExplicitFilter` 和非空 `key_ranges` 决定细粒度、全局或不暂停 scheduler，调用 `LogClient::InitSSTFileRestorer` 注入 importer/checkpoint runner，建立能观察父 context 取消状态的 log-client context，最后把 `file_sets`、模式切换器、online 标志、总数据量和 checkpoint 已恢复量交给 `RestoreSSTFileSets`。守卫退出时总是执行相应清理。

## 数据与状态

`RestoreLifecycle` 本身不在恢复过程中重写文件元数据；`BatchBackupFileSet` 被克隆后交给 restorer，因而表 ID、rewrite rules、KV/byte 统计仍由创建 lifecycle 的工厂负责准确准备。`ValidateFiles` 有意只比较任务选择身份字段，不比较 `TotalKvs`、`TotalBytes`、表 ID 或 rewrite rules；这些额外数据会影响进度和重写语义，扩展工厂时仍必须独立保证正确。

`key_ranges: None` 表示使用全局前置工作；`Some(non-empty)` 可驱动细粒度 scheduler 暂停。Stream 的显式过滤场景还区分 `Some(empty)`：它会令 `pause=false`，以 `nop_undo` 跳过 scheduler 暂停。Snapshot 调用者会对全量/增量恢复清空 ranges，而部分快照恢复若工厂未提供 ranges 会在 `restore.rs` 提前报错。

`checkpoint_compacted_size` 只传入日志客户端用于 compacted SST checkpoint 进度；`checkpoint_runner` 同时可供快照多表 restorer 和日志客户端写 checkpoint。`RestoreSession::restore_schedulers` 是局部状态机：初始为 `true`，`RestoreFiles` 根据 checkpoint 与最终结果调整，`Drop` 决定完整恢复还是仅停止刷新。

## 依赖与调用关系

上游调用边：

- `restore.rs::RunRestore` → `GetRestoreLifecycle(Snapshot, files)` → `ValidateFiles` → `RestoreFiles`。
- `restore_raw.rs::RunRestoreRaw` → `GetRestoreLifecycle(Raw, files)` → `ValidateFiles` → `RestoreFiles`。
- `restore_txn.rs::RunRestoreTxnWithStorage` → `GetRestoreLifecycle(Txn, files)` → `ValidateFiles` → `RestoreFiles`；该入口固定 `online=false`。
- `stream.rs::restoreStreamBody` → `GetRestoreLifecycle(Stream, &[])` → `RestoreCompactedSST`。

主要下游边：

- `Start` → `restore::NewImportModeSwitcher` → `FineGrainedRestorePreWork` 或 `RestorePreWork`。
- `RestoreSession::drop` → `RestorePostWork` 或 `ImportModeSwitcher::StopRefreshing`。
- `RestoreFiles` → `NewWorkerPool` → `NewMultiTablesRestorer`/`NewSimpleSstRestorer` → `SstRestorer::GoRestore` → `WaitUntilFinish` → `FileImporter::Close`。
- `RestoreCompactedSST` → `LogClient::InitSSTFileRestorer` → `LogClient::RestoreSSTFileSets`。
- `RestorePD` → `StoreMeta::{GetTS, GetAllStores}`，并把连接层错误转换为 restore stub 错误。

## 错误处理与边界

- 所有跨 crate 错误都通过 `Error::new(e.to_string())` 转换，保留可读消息，但不保留原始 Rust 错误类型链。
- 已取消的 context 在 `RestorePD::GetTS`/`GetAllStores` 访问底层客户端前优先返回；日志客户端 context 也通过 cancellation source 映射父 context 的消息与 code。
- `ValidateFiles` 将文件视为可重复的多重集合：排序比较可忽略顺序，但不会忽略重复项。它不验证 key range 连续性、rewrite rule 或统计字段。
- Snapshot 配普通 importer 会明确报 `snapshot balanced importer is not configured`；checkpoint 缺 runner 会明确报错。这两项检查发生在 `Start` 之后，因此仍由 RAII 守卫清理已进入的模式和 scheduler 状态。
- `concurrency=0` 被提升为 1；不会创建零 worker 的池。
- importer `Close` 的错误被刻意丢弃，不能覆盖恢复主错误；如果调用方需要观察关闭失败，必须先调整返回错误优先级契约和测试。
- `Drop` 中的 `RestorePostWork` 没有可返回通道；按当前 restore crate API，其失败无法传播给调用者。

## 并发与资源生命周期

工厂要求 `Send + Sync` 并放在 `Arc` 中；客户端、manager、importer 和 checkpoint runner 也以 `Arc<dyn ...>` 共享。真正的数据导入并发由 `NewWorkerPool(concurrency.max(1), "restore files")` 控制，本文件不创建裸线程。

`RestoreSession` 是关键生命周期边界：`Start` 成功后，所有正常返回和 `?` 提前返回都会触发 `Drop`。完整后置工作同时使用 context、mode、undo 和 online 状态；checkpoint 失败的特殊分支只停止周期性刷新，不执行 undo，从而保持 scheduler 配置供下一次重试复用。`RestoreCompactedSST` 借用 `&mut LogClient` 和 `&mut session.mode`，保证一次调用中不会并发修改同一客户端或 mode switcher。

`RestoreGlue` 借用外部 `Glue`，不接管其生命周期；factory 与 lifecycle 中的 `Arc` 则延长具体资源到恢复完成。`FileImporter::Close` 在 `RestoreFiles` 外层统一调用，覆盖 pre-work 后的错误路径；Stream 路径的 importer 生命周期由 `LogClient` 初始化/关闭流程负责，本方法不额外调用 `Close`。

## 与 Go 版本的对应关系

Rust 将分散在 Go 任务文件中的生命周期接线集中到本文件，没有同名 `restore_lifecycle.go`。

- `br/pkg/task/restore.go` 的 `NewImportModeSwitcher`、全量/增量时 `RestorePreWork`、部分恢复时 `FineGrainedRestorePreWork`、checkpoint 失败保留 scheduler 以及成功后的 `RestorePostWork`，对应 `Start`、`RestoreFiles` 和 `RestoreSession::drop`。
- `br/pkg/task/restore_raw.go` 与 `restore_txn.go` 都是 pre-work → `GoRestore` → `WaitUntilFinish` → deferred post-work；Rust 由 `RestoreFiles` 共用这一骨架，Txn 同样固定 offline。
- `br/pkg/task/stream.go` 根据 `ExplicitFilter` 选择细粒度/全局/跳过 scheduler 暂停，并调用 `LogClient::RestoreSSTFileSets`；Rust 的 `RestoreCompactedSST` 保留该分支和数据量/checkpoint 参数。
- Go 的 live client 通常在较大的函数中直接构造；Rust 通过 `RestoreLifecycleFactory` 注入，是为 crate 边界和平台精简依赖建立的适配层，不代表省略了 PD、importer 或 scheduler 行为。
- Rust 额外用 `ValidateFiles` 明确校验工厂产物未遗漏任务层选中文件，并用枚举在类型层区分 balanced importer；这是接线安全检查，不改变 Go 恢复算法。

## 扩展指南

- 新增恢复种类时，应同时扩展 `RestoreKind`、工厂构造分支、入口调用点以及 `RestoreFiles`/`RestoreCompactedSST` 的 restorer 选择；在独立的 `restore_lifecycle_test.rs` 增加入口与资源清理测试，不要把测试写入生产文件。
- 扩展文件一致性条件时修改 `ValidateFiles`，并覆盖顺序、重复项、字段差异和遗漏文件；谨慎决定是否把统计字段或 rewrite rule 纳入身份，否则可能拒绝合法的工厂增强数据。
- 修改 checkpoint 语义时重点复核 `session.restore_schedulers = !checkpoint || result.is_ok()`，它承载失败重试时保留 scheduler 的兼容契约；同时同步 Go `restore.go` 的 `canRestoreSchedulers` 行为。
- 修改 Stream scheduler 选择时必须分别覆盖 `ExplicitFilter=false`、显式过滤且 ranges 非空、显式过滤且 ranges 为空三种状态，并与 `br/pkg/task/stream.go` 对齐。
- 替换 importer 或 PD 适配器时保留 context 取消优先级、store labels、至少一个 worker、关闭覆盖所有返回路径等不变量。
- 性能敏感点是 `file_sets.clone()`、文件身份排序和 worker pool 并发度；若文件量很大而要消除克隆或排序，应先明确所有权及重复文件语义，不能只改容器类型。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/task` 确认目标、模块入口及独立测试；`node --file br/pkg/task/restore_lifecycle.rs --offset 1 --limit 500` 返回完整 304 行并标记直接使用文件为 `restore.rs`、`restore_lifecycle_test.rs`；`query` 分别定位 `RestoreFiles`、`RestoreCompactedSST`、`ValidateFiles`。精确 `callers/callees` 查询在 30 秒窗口内未返回内容，因此调用边又用源码搜索核验，未据此臆测。
- Rust 源与 crate：`br/pkg/task/restore_lifecycle.rs`、`br/pkg/task/lib.rs`、`br/pkg/task/stubs.rs`、`br/pkg/task/Cargo.toml`。
- Rust 直接入口：`br/pkg/task/restore.rs`、`br/pkg/task/restore_raw.rs`、`br/pkg/task/restore_txn.rs`、`br/pkg/task/stream.rs`。
- 独立 Rust 测试：`br/pkg/task/restore_lifecycle_test.rs` 覆盖 Snapshot/Raw/Txn 的 real restorer 接线、online/offline mode 生命周期、checkpoint 失败保留 import mode、文件遗漏拒绝和 Stream compacted SST 导入。
- Go 对照：`br/pkg/task/restore.go`、`br/pkg/task/restore_raw.go`、`br/pkg/task/restore_txn.go`、`br/pkg/task/stream.go`；下游实现位置由 `br/pkg/restore/import_mode_switcher.go`、`br/pkg/restore/restorer.go`、`br/pkg/restore/log_client/client.go` 核验。
- 本任务是只读逻辑分析与文档新增，按计划未运行 Cargo；交付前以固定章节命令验证文档恰含 11 个指定二级标题，并人工检查没有把测试代码嵌入生产源文件的建议。
