# `br/pkg/checkpoint/backup.rs`

源码：[backup.rs](./backup.rs)；Go 对照：[backup.go](./backup.go)。

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-checkpoint`（见 [`Cargo.toml`](./Cargo.toml)），是通用 `checkpoint.rs` 与“备份”场景之间的薄适配层。它不实现刷盘算法，而是把备份使用的键值类型、对象存储路径、序列化函数和元数据结构绑定到通用 `CheckpointRunner`。crate 入口 [`lib.rs`](./lib.rs) 通过 `pub mod backup` 加载本文件，并用 `pub use backup::*` 展平公开 API。

需要区分实现能力与整仓接线状态：RustCodeGraph 的文件关系显示当前直接使用本文件的 Rust 文件是 [`checkpoint_test.rs`](./checkpoint_test.rs)；Rust 备份客户端 [`../backup/client.rs`](../backup/client.rs) 当前调用的是其本 crate 的 [`../backup/stubs.rs`](../backup/stubs.rs) 中同名接口，而 `br/pkg/backup/Cargo.toml` 也尚未依赖 `astersql-br-pkg-checkpoint`。因此，本文件已有真实的异步落盘与锁适配实现，但尚不是 Rust 备份客户端主链实际使用的实现。

## 核心职责

- 定义备份 checkpoint 的固定目录布局：根目录、data、checksum、meta 与 lock 路径（`CheckpointBackupDir` 等五个常量）。
- 用 `flushPathForBackup` 将三个刷写路径交给 `externalCheckpointStorage`，并用 `valueMarshalerForBackup` 将 `RangeGroup<String, RangeType>` 编码成 JSON。
- 通过 `StartCheckpointRunnerForBackup` 和测试专用的 `StartCheckpointBackupRunnerForTest` 创建通用 runner、获取外部存储锁并启动后台循环。
- 用 `AppendForBackup` 把一个成功备份的半开 key range 及其文件列表转为通用 `CheckpointMessage`。
- 用 `WalkCheckpointFileForBackup` 回放 data 分片，用 `LoadCheckpointMetadata`/`SaveCheckpointMetadata` 管理元数据并在加载时合并 checksum 分片。
- 用 `RemoveCheckpointDataForBackup` 清理备份 checkpoint 目录下允许删除的 checkpoint 文件。

## 主要符号

- `BackupKeyType = String`、`BackupValueType = RangeType`：给通用 runner 的类型参数命名。当前 `AppendForBackup` 总是使用空字符串作为 `GroupKey`，所以所有备份 range 在一次内存批次中归入同一 `RangeGroup`。
- `CheckpointBackupDir`、`CheckpointDataDirForBackup`、`CheckpointChecksumDirForBackup`、`CheckpointMetaPathForBackup`、`CheckpointLockPathForBackup`：分别固定为 `checkpoints/backup` 及其 `data`、`checksum`、`checkpoint.meta`、`checkpoint.lock` 子路径。
- `flushPathForBackup() -> flushPath`：内部路径装配函数，只提供 data/checksum/lock 三类路径；meta 由独立的元数据 API 处理。
- `valueMarshalerForBackup(&RangeGroup<...>) -> Result<Vec<u8>>`：通过 `serde_json::to_vec` 生成 data 分片载荷，序列化错误直接进入 crate 的 `Result`。
- `StartCheckpointBackupRunnerForTest(...)`：公开测试入口，把同一个自定义 `tick` 同时用于 data flush、checksum flush、锁续期和失败重试，便于控制时序。
- `StartCheckpointRunnerForBackup(...)`：公开生产形态入口，分别使用 `defaultTickDurationForFlush`、`defaultTickDurationForChecksum`、`defaultTickDurationForLock`、`defaultRetryDuration`。
- `AppendForBackup(...)`：复制 `startKey`、`endKey`，取得 `files` 所有权，构造单元素 `RangeType` 后调用 `CheckpointRunner::Append`。
- `WalkCheckpointFileForBackup<F>(...) -> Result<Duration>`：只遍历 data 目录的 `.cpt` 文件；回调逐个接收反序列化后的 `(String, RangeType)`，返回历史执行累计耗时。
- `CheckpointMetadataForBackup`：落盘字段是 `GCServiceId`、`ConfigHash`、`BackupTS`，JSON 键分别为 `gc-service-id`、`config-hash`、`backup-ts`；`CheckpointChecksum` 与 `LoadCheckpointDataMap` 带 `#[serde(skip)]`，仅存在于运行时。
- `LoadCheckpointMetadata`、`SaveCheckpointMetadata`、`RemoveCheckpointDataForBackup`：固定路径上的读取并合并 checksum、写入、清理入口。

## 执行流程

1. 启动时，两个 `Start*Runner*` 入口先调用 `newExternalCheckpointStorage(ctx, storage, Some(timer), flushPathForBackup())`。构造函数立即执行 `initialLock`：取 TSO、检查已有锁、写入本实例锁，等待 3 秒后复检；任一步失败都不会返回 runner。
2. 锁成功后，`newCheckpointRunner` 创建共享 `meta`/`checksum` 状态、append/checksum/done/meta/lock/error 通道及工作线程句柄容器。随后 `startCheckpointMainLoop` 启动后台线程；生产入口使用默认周期，测试入口让四个周期相同。
3. 一个备份响应成功后，调用方可用 `AppendForBackup` 把 `[startKey, endKey)` 和输出 `File` 列表包装为 `CheckpointMessage { GroupKey: "", Group: [RangeType] }`。通用 `Append` 先检查 context、后台错误和关闭状态，再投递到无界 append 通道。
4. 主循环收到 append 消息后按空 `GroupKey` 合并到内存 `RangeGroup`；flush tick 将当前 map 整体 `take` 出来，底层刷写循环调用本文件提供的 JSON marshaler，并由 `externalCheckpointStorage` 写到 `data/<uuid>.cpt`。checksum 与 lock 使用各自 ticker 和通道独立推进。
5. 正常结束由调用方执行 `CheckpointRunner::WaitForFinish(ctx, flush)`；done 信号只发一次。主循环先排空 pending append/checksum，`flush=true` 时做最终交接，然后关闭内部发送端、等待刷盘线程，外层再 join 主线程并调用存储 `close`。
6. 续跑时先 `LoadCheckpointMetadata`：读取并反序列化 `checkpoint.meta`，再遍历 checksum 目录，把表 ID 到 `ChecksumItem` 的映射写入运行时字段。若需恢复已完成区间，再由 `WalkCheckpointFileForBackup` 解密/解析 data 分片并逐项回调。
7. 清理时 `RemoveCheckpointDataForBackup` 把根目录交给通用删除逻辑；该逻辑仅收集 `.cpt`、`.meta`、`.lock`，用 4 个线程删除，并在累计第 16 个失败时返回错误。

## 数据与状态

持久化数据分为三类。data 分片保存 `RangeGroup<String, RangeType>` 的 JSON（底层可按 cipher 加密）；每个 `RangeType` 保存 `StartKey`、`EndKey` 与产生的 `Files`。checksum 分片由通用 runner 保存 `ChecksumItem` 集合，加载时按 `TableID` 汇总为 `HashMap<i64, ChecksumItem>`。`checkpoint.meta` 只保存 GC 服务 ID、配置哈希与备份时间戳；checksum map 和“是否回放 data map”的布尔值不会进入 JSON。

runner 的可变状态位于 `checkpoint.rs` 的 `RunnerShared`：待刷 data map 和 checksum 列表由 `Mutex` 保护，首个致命错误由 `RwLock<Option<Error>>` 与 `err_closed` 记录。通道把接收、聚合与持久化阶段分开。备份适配层自身不持有全局可变状态；路径常量和类型绑定是静态的。

键区间语义由 `RangeType` 与调用方保证。本文件只复制字节，不验证 `startKey < endKey`、范围是否重叠、文件是否真实存在，也不按表拆分 `GroupKey`。扩展者不能把空分组键误解为业务数据库名或表名。

## 依赖与调用关系

向下依赖均来自同一 crate：`checkpoint.rs` 提供 `CheckpointRunner`、通用消息/范围/checksum 类型以及 walk/load/save/remove 算法；`external_storage.rs` 提供对象存储落盘和分布式锁；`stubs.rs` 提供 `Context`、`Storage`、`GlobalTimer`、`CipherInfo`、`File` 和统一 `Result`。外部 crate 依赖仅通过这些下层实现间接使用；本文件直接使用 `serde`/`serde_json` 和标准库的 `HashMap`、`Arc`、`Duration`。

向上关系当前以测试为主：[`checkpoint_test.rs`](./checkpoint_test.rs) 的 `test_checkpoint_backup_runner` 覆盖启动、两批 append、checksum、最终 flush、加密 walk 和 checksum 合并；`test_checkpoint_runner_lock` 覆盖锁冲突；`test_checkpoint_meta_for_backup` 覆盖元数据往返。[`parity_test.rs`](./parity_test.rs) 还核对公开契约和 JSON 路径。RustCodeGraph 没有找到来自已接线生产 crate 的直接调用边。

设计上的 Go 主链可从 [`../backup/client.go`](../backup/client.go) 观察：启动 checkpoint、回放完成范围、成功响应追加 range、最终清理分别调用这些包级接口。Rust [`../backup/client.rs`](../backup/client.rs) 也保留同形调用点，但它们目前解析到 `../backup/stubs.rs` 的本地模块，而不是本文件；真正接线需要先建立 crate 依赖并消除重复类型边界，不能仅改一处 `use`。

## 错误处理与边界

所有可能失败的公开操作都返回统一 `Result`，并以 `?` 原样传播存储、锁、JSON、加解密、通道和回调错误。启动入口尤其可能在 runner 创建前因取 TSO、读取/解析锁、写锁或 3 秒复检失败。`AppendForBackup` 会传播 context 已取消、后台首错、runner 已关闭和 append 通道关闭；它不等待实际刷盘完成，因此“返回成功”只代表消息已入队。

`WalkCheckpointFileForBackup` 忽略 data 目录中非 `.cpt` 文件；任一 `.cpt` 读取、解密、反序列化或用户回调失败都会中止遍历。`LoadCheckpointMetadata` 必须先成功读取 meta，随后 checksum 目录遍历也必须成功；如果 checksum 加载失败，不会返回只含 meta 的部分结果。`SaveCheckpointMetadata` 的两个运行时字段被静默跳过，这是数据格式约定而非丢失错误。

`RemoveCheckpointDataForBackup` 并非删除任意对象：通用实现只处理三种 checkpoint 后缀。单个删除错误在不足 16 个时会被容忍；达到阈值才返回首个带“failed to delete too many files”注解的错误。因此成功返回不等价于每个匹配文件都必然删除成功，这是从 Go 实现保留下来的容错边界。

## 并发与资源生命周期

`Arc<dyn Storage>` 让 runner 的外部存储可被主循环与刷盘线程共享。`startCheckpointMainLoop` 启动主线程，并在其中启动刷盘循环；三个 ticker 分别驱动 data、checksum 和 lock。append/checksum 输入通道是无界的，突发流量不会在发送端形成容量背压，未及时 flush 的压力会体现在内存中的 range/checksum 集合。

锁在 `newExternalCheckpointStorage` 阶段同步获取，runner 运行期间由 lock ticker 续期。锁冲突会阻止启动；运行期续锁失败成为致命后台错误，并使后续 append/checksum 快速失败。`WaitForFinish` 是必须的生命周期收尾点：它幂等发送 done、join 所有线程并调用 storage close；`flush=true` 才要求把尾部内存数据交给刷盘侧。测试 `test_checkpoint_runner_lock` 表明 runner 存活时其他 lock ID 无法启动。

清理函数另有最多 4 个 scoped worker；第 16 次删除失败设置原子 fatal 标志，使 worker 停止领取新任务。元数据 load/save 和 walk 本身同步执行，不创建后台任务。

## 与 Go 版本的对应关系

本文件逐项对应 [`backup.go`](./backup.go)：类型别名、五个路径、`flushPathForBackup`、JSON marshaler、两个 runner 启动入口、append/walk、元数据结构及 load/save/remove API 均保持同形。Rust 用 `Arc<dyn Storage>`/`Option<CipherInfo>` 表达 Go interface/指针，用 `Vec<u8>` 和 `Vec<File>` 取得数据所有权；Go 的 `[]byte` 和 `[]*backuppb.File` 则由调用方共享底层对象。

序列化契约保持一致：三个 meta JSON 字段使用相同的连字符名称，checksum 与回放标志都不落盘。Go `AppendForBackup` 未显式填写 `GroupKey`，其字符串零值为空；Rust 明确写 `String::new()`。Go load 返回 metadata 指针并在 checksum 失败时连同错误返回；Rust 使用值返回，任何失败直接返回 `Err`。

通用 Rust 实现也保留 Go 的关键生命周期语义，包括独立 flush/checksum/lock/retry 周期、结束时排空尾消息、锁复检、4 worker 清理与 16 次失败阈值。不过当前应用接线并不对等：Go 备份客户端使用真实 checkpoint 包，Rust 备份客户端仍使用本地简化 stub（其 `AppendForBackup` 是空操作、walk 直接返回零耗时、runner 不启动后台刷盘）。这属于迁移状态，不能据本文件的实现推断 Rust BR 备份主链已经获得持久化续跑能力。

## 扩展指南

- 新增或修改持久化元数据字段时，先改 `CheckpointMetadataForBackup` 并明确 serde 名称/default/skip，再同步 Go `backup.go`、元数据往返测试与兼容旧 JSON 的测试；切勿把 `CheckpointChecksum` 或 `LoadCheckpointDataMap` 意外写入 meta。
- 改变目录布局时，应同步五个常量、`flushPathForBackup`、Go 常量、锁与清理相关测试，以及所有按路径探测 checkpoint 的调用方。路径变化会影响旧备份续跑兼容性，应提供迁移或回退策略。
- 改变 range 编组或编码时，主要接入点是 `AppendForBackup` 和 `valueMarshalerForBackup`；还需验证 `walkCheckpointFile` 能读取既有 `.cpt`，并扩展 [`checkpoint_test.rs`](./checkpoint_test.rs) 的多批 append/加密回放用例。不能只让新写入与新读取彼此通过而忽略旧格式。
- 修改启动/结束语义时，应在独立测试文件中覆盖锁冲突、context 取消、后台刷写错误、尾消息 drain 与 `flush=false/true`；不要把测试内嵌到 `backup.rs`。
- 若要把本实现接入 Rust 备份客户端，需要在 `br/pkg/backup` 的 Cargo 依赖和类型边界上做独立迁移，替换 `backup/stubs.rs::checkpoint` 的同名桩，并验证 client 的续跑、响应追加、checksum 合并和资源收尾。该工作超出本文件文档任务，不能通过复制实现或保留两套权威 API 来完成。
- 性能风险主要是空 `GroupKey` 带来的单组积累、无界通道的突发内存增长、JSON/加密开销和对象存储小文件数量；兼容风险主要是路径、JSON 字段、cipher 与 Go 版本不一致。

## 验证依据

- RustCodeGraph：`status` 确认索引含 7,032 个 Rust 文件；`node --file br/pkg/checkpoint/backup.rs --offset 1 --limit 500` 读取完整 181 行并确认仅由 `checkpoint_test.rs` 使用；`query` 核对六个公开入口；`node`/按文件节点核对 `newCheckpointRunner`、`CheckpointRunner::Append`、`startCheckpointMainLoop`、`walkCheckpointFile`、`loadCheckpointMeta`、`loadCheckpointChecksum`、`saveCheckpointMetadata`、`removeCheckpointData` 与 `newExternalCheckpointStorage` 的下游行为。
- crate 与模块边界：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)；当前备份客户端边界：[`../backup/Cargo.toml`](../backup/Cargo.toml)、[`../backup/client.rs`](../backup/client.rs)、[`../backup/stubs.rs`](../backup/stubs.rs)。
- Go 对照：[`backup.go`](./backup.go)；Go 应用调用证据：[`../backup/client.go`](../backup/client.go)。
- Rust 测试：[`checkpoint_test.rs`](./checkpoint_test.rs) 中 `test_checkpoint_meta_for_backup`、`test_checkpoint_backup_runner`、`test_checkpoint_runner_lock`；[`parity_test.rs`](./parity_test.rs) 中 `go_rust_public_contract_matches`。Go 对照测试位于 [`checkpoint_test.go`](./checkpoint_test.go)，对应 `TestCheckpointMetaForBackup`、`TestCheckpointBackupRunner`、`TestCheckpointRunnerLock`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令检查文件存在且恰有 11 个固定二级章节，并人工复核实现事实与“尚未接线”结论没有混淆。
