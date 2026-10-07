# `br/pkg/restore/restorer.rs`

## 文件定位

[`restorer.rs`](restorer.rs) 位于 `astersql-br-pkg-restore` library crate 中；crate 由 [`Cargo.toml`](Cargo.toml) 声明，并由 [`lib.rs`](lib.rs) 以 `pub mod restorer` 挂载后扁平再导出。它是 Go [`restorer.go`](restorer.go) 的 Rust 语义镜像，处在“上层恢复生命周期/快照或日志客户端”和“底层文件导入器、Region 分裂器、checkpoint”之间，负责把备份文件组调度为异步导入任务。

当前代码不是未接线的占位：`br/pkg/task/restore_lifecycle.rs::RestoreFiles` 会为 Snapshot 恢复构造 `NewMultiTablesRestorer`，为其他恢复构造 `NewSimpleSstRestorer`，然后执行 `GoRestore` 和 `WaitUntilFinish`；`br/pkg/restore/log_client/client.rs::InitSSTFileRestorer` 也会为 compacted SST 恢复构造 `NewSimpleSstRestorer`。不过该 crate 仍处于迁移期，`Context`、`ErrorGroup`、`WorkerPool`、checkpoint 和 split client 等边界来自本 crate 的 `stubs.rs`，不能据此推断已经验证真实 TiKV 的吞吐或网络行为。

## 核心职责

该文件承担五类职责：

1. 用 `BackupFileSet` / `BatchBackupFileSet` 表达表、SST 文件、重写规则之间的绑定，并提供 Raw/Txn 场景的文件组构造函数。
2. 定义 `SstRestorer`、`FileImporter`、`BalancedFileImporter` 三层契约，分离高层异步调度、实际导入和多表背压。
3. 提供 `SimpleRestorer`、`BatchRestorer`、`MultiTablesRestorer` 三种恢复策略，分别面向逐文件组导入、按重叠 Region 合并导入、带背压的多表批量导入。
4. 保证导入成功后才写 checkpoint 和上报进度，并通过 `ErrorGroup` 将 worker 错误汇总到 `WaitUntilFinish`。
5. 用 `PipelineRestorerWrapper::WithSplit` 在迭代管道中筛除跳过项、积累分裂材料并按策略触发 Region 分裂。

此外，`MarshalLogObjectForFiles` / `ZapBatchBackupFileSet` 汇总文件名和 KV/字节数量，服务于恢复日志；Rust 端返回字符串，不是 Go 端的 `zap.Field`。

## 主要符号

- `BackupFileSet { TableID, SSTFiles, RewriteRules }`：一个表级恢复单元。`TableID` 对 Raw/Txn 为 `0`；`RewriteRules` 归属于单表；`SSTFiles` 是要一起导入的 SST 列表。
- `BatchBackupFileSet = Vec<BackupFileSet>`：一次 worker 投递或 importer 调用的外层批次。
- `zapBatchBackupFileSetMarshaler::MarshalLogObject`：遍历批次，统计文件数、文件名、`TotalKvs`、`TotalBytes`。由于本地 `backuppb::File` 没有 Go 的 `Size_` 字段，Rust 的 `totalSize` 暂以 `TotalBytes` 代替，这是已知迁移差异。
- `CreateUniqueFileSets(files)`：把每个文件拆成独立 `BackupFileSet`，并令 `TableID = 0`、`RewriteRules = None`，对应 Raw/Txn 非 table-ID 文件。
- `NewFileSet(files, rules)`：构造 `TableID = 0` 且带重写规则的文件组。
- `SstRestorer`：公开高层接口。`GoRestore` 负责投递，`WaitUntilFinish` 收束所有已投递任务并返回异步错误，`Close` 转发给 importer 释放资源。
- `FileImporter`：底层 `Import` / `Close` 抽象；默认 `ConfigureDownloadRetry` 明确返回“不支持能力探测”错误，要求实际 compacted-SST importer 覆盖该能力，而不是静默假定 TiKV 支持。
- `BalancedFileImporter: FileImporter`：增加 `PauseForBackpressure`，只被多表策略使用。
- `SimpleRestorer` / `NewSimpleSstRestorer`：每个 `BackupFileSet` 单独投递一个 worker 任务。
- `BatchRestorer` / `NewBatchSstRestorer`：先调用 `GroupOverlappedBackupFileSetsIter` 按 Region 重叠关系重新分组，再为每组投递导入任务。
- `MultiTablesRestorer` / `NewMultiTablesRestorer`：每个 `BatchBackupFileSet` 投递前执行背压，并用 `file_count`、`start` 汇总成功/失败和耗时。
- `GetFileRangeKey`：从文件名删除最后一个下划线及其后缀，使同一 range 的 default/write CF 共用 checkpoint key；不含下划线时 panic。
- `PipelineRestorerWrapper::WithSplit`：组合 `FilterOut` 和 `TryMap`；非跳过项进入策略累积，达到阈值时调用 `ExecuteRegions`，成功后清空累积。
- `PipelineFromSlice`：把切片转换为 `TryNextor`，主要用于独立测试和简单调用。
- `_keep_split_helper`：仅用于保持 `SplitHelperIterator` 类型引用，带 `dead_code` 允许项，不参与业务路径。

## 执行流程

`SimpleRestorer::GoRestore` 的流程如下：

1. 展开所有外层批次和其中的 `BackupFileSet`。
2. 克隆 importer、子上下文、可选 checkpoint runner 和进度回调，并通过 `WorkerPool::ApplyOnErrorGroup` 投递闭包。
3. worker 调用 `FileImporter::Import`；失败即保存错误，不写 checkpoint，也不上报进度。
4. 导入成功后，若配置 checkpoint，则按文件调用 `RestoreCheckpoint::AppendFile(TableID, file_name)`。
5. 导入与 checkpoint 都成功后，逐文件按 `TotalKvs` 调用进度回调；错误以 `Error::Trace` 交给 `ErrorGroup`。

`BatchRestorer::GoRestore` 先把输入扁平化，再用一个外层 worker 执行 `GroupOverlappedBackupFileSetsIter`。回调得到按重叠关系组织的 `batch_set` 后，再向同一个 error group 投递内层导入任务。内层一次导入整组文件，成功后按每个文件写 `AppendFile` 并以各文件 `TotalKvs` 上报进度。`br/pkg/restore/parity_test.rs` 证明两个重叠 SST 可合为一次 importer 调用且进度仍累计为两个文件的 KV 总数。

`MultiTablesRestorer::GoRestore` 在每次调用开始时重置开始时间和文件计数。每个外层批次投递前先检查 `ectx.Err()`；发现取消或先前任务错误时停止继续投递。随后累计该批文件数，调用 `PauseForBackpressure`，再投递整批 `Import`。成功后，按每个 `BackupFileSet` 建立 `HashSet`，对同一 range 的多个 CF 只调用一次 `AppendRangeKey`，最后以 `on_progress(1)` 表示完成一个外层批次。循环结束时再次返回 `ectx.Err()`，避免“父上下文已取消但 error group 中没有运行任务”被误判为成功。

`MultiTablesRestorer::WaitUntilFinish` 等待 error group：失败时记录一个 failure unit 并返回 trace 后的错误；成功时根据 `start` 和 `file_count` 写入 duration 与 success summary。`SimpleRestorer` 和 `BatchRestorer` 的 `WaitUntilFinish` 则直接返回 `ErrorGroup::Wait`。

`PipelineRestorerWrapper::WithSplit` 是惰性管道：`ShouldSkip` 为真的元素先被 `FilterOut` 丢弃；其余元素在被拉取时先 `Accumulate`。若 `ShouldSplit` 为真，则提取当前累积、以调用者传入的 `Context` 执行 `ExecuteRegions`，成功才 `ResetAccumulations`；失败返回错误并中断管道，同时不会执行 reset。

## 数据与状态

文件组的所有权随 `GoRestore` 输入移入 worker：`SimpleRestorer` 每任务拥有一个 `BackupFileSet`，`BatchRestorer` 的分组任务拥有一个 `BatchBackupFileSet`，`MultiTablesRestorer` 的任务拥有一个完整外层批次。共享对象（importer、checkpoint、回调、split client）通过 `Arc` 跨 worker 使用。

三个 Restorer 都在构造时由 `ErrorGroup::with_context(ctx)` 产生 `eg` 和派生 `ectx`，因此首个任务错误可通过派生上下文阻止后续投递，并在 `WaitUntilFinish` 汇总。`MultiTablesRestorer` 额外以 `Mutex<usize>` 保存 `file_count`，以 `Mutex<Option<Instant>>` 保存开始时刻；每次 `GoRestore` 会覆盖两者，因此调用契约隐含“同一实例不要并发启动多个恢复轮次”。

checkpoint 有两种粒度：Simple/Batch 使用 `(TableID, file_name)` 的 `AppendFile`；MultiTables 使用 `(TableID, range_prefix)` 的 `AppendRangeKey`。后者通过 `HashSet` 在单个文件组内去重，但不会跨不同 `BackupFileSet` 或不同批次全局去重。

`PipelineRestorerWrapper` 本身只保存线程安全的 splitter；可变的 `SplitStrategy` 由 `Arc<Mutex<dyn SplitStrategy<T>>>` 保护。锁覆盖 `Accumulate`、阈值判断、读取累积、执行 split 和 reset 的整个区段，因此同一策略不会交错更新，但慢速 `ExecuteRegions` 会延长持锁时间。

## 依赖与调用关系

上游直接证据：

- `br/pkg/task/restore_lifecycle.rs::RestoreFiles`：Snapshot 选择 `NewMultiTablesRestorer`，其他 kind 选择 `NewSimpleSstRestorer`；调用顺序为 `GoRestore(...).and_then(WaitUntilFinish)`，外层结束时关闭 importer。
- `br/pkg/restore/log_client/client.rs::InitSSTFileRestorer`：先枚举存活且非 TiFlash 的 store，执行 `ConfigureDownloadRetry`，再创建 `NewSimpleSstRestorer` 供 compacted SST 路径使用。
- `br/pkg/restore/snap_client/tikv_sender.rs::RestoreSSTFiles`：从 client 取得 Restorer，投递文件组后立即等待完成。

下游直接依赖：

- `astersql-br-pkg-restore-utils::{RewriteRules, stubs::backuppb}` 提供重写规则和文件元数据。
- `astersql-br-pkg-utils-iter::{FilterOut, TryMap, TryNextor, FromSlice}` 提供惰性迭代组合器。
- `crate::misc::GroupOverlappedBackupFileSetsIter` 为 BatchRestorer 按 Region 边界组织导入批次。
- `crate::stubs` 提供取消上下文、错误组、worker pool、split/checkpoint trait、日志与 summary；这些是当前 crate 的迁移边界。

RustCodeGraph 对 `restorer.rs::GoRestore` 的被调用关系确认了 `GroupOverlappedBackupFileSetsIter`、`PauseForBackpressure`、checkpoint/summary 相关调用；索引对同名 trait/impl 方法存在歧义，因此具体上游又以 `rg` 定位并读取上述调用文件核实。

## 错误处理与边界

- `GoRestore` 的“成功返回”通常只表示任务已成功投递；实际 importer 或 checkpoint 错误由 `WaitUntilFinish` 返回。调用方若省略 wait，会漏掉异步错误。
- Simple/Batch/Multi 都只在 importer 和 checkpoint 全部成功后上报进度；`restorer_test.rs::test_simple_restorer_with_error_in_import` 验证失败文件不会计入进度。
- checkpoint 写入发生在导入之后。若导入成功而 checkpoint 失败，本轮返回错误，重试可能再次导入同一数据；这是避免把未导入数据误记为完成所需的保守顺序。
- `MultiTablesRestorer::GoRestore` 在投递前和循环后检查派生上下文；已取消的上下文会直接返回错误，已投递任务仍应由 `WaitUntilFinish` 收束。
- `GetFileRangeKey` 要求文件名至少包含一个下划线；非法名字会 panic，而不是返回 `Result`，与 Go `strings.LastIndex < 0` 的 panic 行为一致。
- `FileImporter::ConfigureDownloadRetry` 默认失败，防止 compacted-SST 路径在没有能力探测时继续运行。
- `WithSplit` 的 `Mutex::lock().unwrap()` 会在策略锁中毒时 panic；`ExecuteRegions` 错误会转换为带操作描述的字符串错误，停止管道且保留当前累积，便于调用方决定重试方式。
- 空输入不会创建 worker。MultiTables 在从未成功设置开始时间时调用 wait，会以零时长汇总；生产调用应保持 `GoRestore` 后再 `WaitUntilFinish` 的顺序。

## 并发与资源生命周期

`WorkerPool::ApplyOnErrorGroup` 控制实际并发，`ErrorGroup` 负责等待和首错传播。Simple 每个文件组一个 worker；Batch 有一个负责 Region 分组的外层 worker，并为每个分组再提交内层 worker；MultiTables 每个外层批次一个 worker，并在提交前同步执行背压。背压阻塞的是生产者投递速度，不等同于 worker pool 并发上限。

`Arc<dyn FileImporter>` / `Arc<dyn BalancedFileImporter>` 要求实现满足 `Send + Sync`。进度回调同样必须 `Send + Sync`，可能被多个 worker 并发调用，因此调用方需要自行同步共享计数；测试分别使用原子值或 `Mutex`。checkpoint runner 也通过 `Arc` 共享，其实现必须保证追加操作的并发安全。

`Close` 仅转发 importer 的关闭操作，不隐式等待任务；安全顺序是 `GoRestore → WaitUntilFinish → Close`。当前文件没有 `Drop` 自动清理，构造 Restorer 的上层生命周期负责最终关闭。任务错误会取消派生上下文并停止新的有效工作，但已经提交的闭包仍由 error group 负责结束。

## 与 Go 版本的对应关系

结构和主流程与 [`restorer.go`](restorer.go) 对齐：三种 Restorer、两层 importer trait、导入后 checkpoint、成功后进度、MultiTables 背压与 range-key 去重、以及 pipeline split 的顺序均保持一致。Rust 独立测试文件 [`restorer_test.rs`](restorer_test.rs) 映射 Go [`restorer_test.go`](restorer_test.go) 的 Simple 成功/失败、MultiTables 成功/失败/取消、WithSplit 不触发/触发 reset 等用例；Rust 还增加了“取消上下文原样传给 `ExecuteRegions`”的检查。

需要注意的迁移差异：

- Go 日志编码器输出结构化 zap 字段，并从 protobuf 的 `Size_` 统计 `totalSize`；Rust 返回格式化字符串，且暂以 `TotalBytes` 代替 `Size_`。
- Go 构造函数返回 `SstRestorer` interface；Rust 返回具体结构，调用方可再装箱为 `Box<dyn SstRestorer>` 或放入 `Arc`。
- Go `MultiTablesRestorer` 的计数和时间字段为普通字段并通过可变接收者串行使用；Rust 因 trait 方法接收 `&self` 而用 `Mutex` 提供内部可变性。
- Go 使用真实的 `context.Context`、`errgroup.Group`、TiDB worker/checkpoint/split 类型；本 crate 当前使用本地 stubs 和瘦身依赖。控制流有测试证据，但真实 TiKV 性能、RPC 与故障恢复不由这些单元测试证明。
- Rust `FileImporter` 多出 `ConfigureDownloadRetry` 能力探测入口，以支撑 `log_client` 的 compacted-SST 接线；默认实现刻意报错。

## 扩展指南

- 新增恢复策略时，应实现 `SstRestorer`，保持“投递与等待分离、导入成功后写 checkpoint、所有成功条件满足后才回调进度、Close 释放 importer”的共同契约，并在独立 `*_test.rs` 中覆盖成功、导入失败、checkpoint 失败、取消和关闭。
- 修改批次粒度时，重点检查 `SimpleRestorer::GoRestore`、`BatchRestorer::GoRestore`、`MultiTablesRestorer::GoRestore` 的进度单位：前两者按文件 `TotalKvs`，后者按完成批次数 `1`。改变单位会影响上层进度显示。
- 扩展 checkpoint 格式时，Simple/Batch 的 `AppendFile` 和 MultiTables 的 `AppendRangeKey` 必须同步评估；多 CF 合并规则还要同步 `GetFileRangeKey` 和 `br/pkg/restore/parity_test.rs` 中的 range 去重用例。
- 调整 Region 合并时，应修改 `crate::misc::GroupOverlappedBackupFileSetsIter` 而不是在 BatchRestorer 内复制分组逻辑，并同步覆盖重叠/不重叠、分组失败和取消。
- 扩展 pipeline 策略时，保持 `ShouldSkip → Accumulate → ShouldSplit → GetAccumulations → ExecuteRegions → ResetAccumulations` 的顺序；增加失败测试，证明 split 失败时错误可见且不会错误清空状态。
- 若用真实依赖替换 `stubs.rs`，需要核对 `Context` 取消传播、`ErrorGroup` 首错语义、worker 饱和行为、checkpoint 并发安全和 `Close` 幂等性，不能只以编译通过作为等价证明。
- 测试必须继续放在独立文件；首选扩展 `br/pkg/restore/restorer_test.rs`，公开 Go/Rust 契约和 Batch/checkpoint 边界可同步扩展 `br/pkg/restore/parity_test.rs`，不要把测试内嵌进 `restorer.rs`。

## 验证依据

本说明使用了以下直接证据：

- RustCodeGraph 索引状态：共 7032 个 Rust 文件；读取了 `br/pkg/restore/restorer.rs` 全部 568 行，并查询了 `SimpleRestorer`、`BatchRestorer`、`MultiTablesRestorer`、`PipelineRestorerWrapper`、`GoRestore`、`WithSplit` 等符号及调用边。
- 源码与 crate 边界：`br/pkg/restore/restorer.rs`、`br/pkg/restore/Cargo.toml`、`br/pkg/restore/lib.rs`、`br/pkg/restore/misc.rs`。
- 直接 Rust 上游：`br/pkg/task/restore_lifecycle.rs::RestoreFiles`、`br/pkg/restore/log_client/client.rs::InitSSTFileRestorer`、`br/pkg/restore/snap_client/tikv_sender.rs::RestoreSSTFiles`。
- Go 对照：`br/pkg/restore/restorer.go` 全部 458 行和 `br/pkg/restore/restorer_test.go` 全部 290 行。
- Rust 测试：`br/pkg/restore/restorer_test.rs` 全部 572 行；`br/pkg/restore/parity_test.rs` 中 Simple/Batch/Multi、checkpoint、pipeline 与非法文件名边界段落。
- 人工复核结论：该文件存在是为了统一不同备份格式的 SST 异步调度与收束；运行时由上层选择策略、投递、等待并关闭；安全扩展必须维护错误、checkpoint、进度、取消、背压和资源关闭的先后关系。

本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题。
