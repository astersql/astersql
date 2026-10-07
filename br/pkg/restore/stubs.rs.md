# `br/pkg/restore/stubs.rs`

## 文件定位

本文件是 `astersql-br-pkg-restore` crate 的本地兼容层。crate 根入口 [`br/pkg/restore/lib.rs`](lib.rs) 以 `#[path = "stubs.rs"] pub mod stubs` 挂载它，并通过 `pub use stubs::*` 将其公开符号扁平再导出；因此 `misc.rs`、`restorer.rs`、`import_mode_switcher.rs` 等生产模块会直接依赖这里的接口。RustCodeGraph 将该文件识别为 1,150 行、153 个符号，并给出 66 个使用文件，说明它虽名为 `stubs`，却是当前 Rust restore 代码的公共适配面，而非可以随意删除的测试夹具。

[`br/pkg/restore/Cargo.toml`](Cargo.toml) 声明 crate 名为 `astersql-br-pkg-restore`、Go 包映射为 `br/pkg/restore`、库入口为 `lib.rs`。源文件注释明确其目标是用本地类型隔离 PD、TiKV、连接、domain、storage、summary、checkpoint 等尚未完整迁入的边界；它不建立真实 PD/TiKV RPC。不过 Cargo manifest 当前仍直接声明 `grpcio` 与带 tag 的 `kvproto` 依赖，这些依赖用于同一 crate 的其他实现/测试，不能把“本文件不做真实 RPC”扩大解释为“整个 crate 不依赖 RPC 库”。

## 核心职责

文件承担四类职责：

1. 提供 Go 风格的基础语义适配：`Error`/`Result`、`Context`/`CancelFunc`、`ComposeTS`、`CIStr`、日志与 summary 门面。
2. 定义 restore 生产逻辑所需的依赖反转接口：`PdClient`、`SplitClient`、`ImportSstSwitcher`、`ConnMgr`、`Storage`、`InfoSchema`、`MetaReader`、`Domain`、`RestoreCheckpoint`、`SplitStrategy`、`PipelineRegionsSplitter`。
3. 提供可注入、可观测的内存实现：`MemPdClient`、`MemSplitClient`、`RecordingImportSstSwitcher`、`MemConnMgr`、`MemStorage`、`MemRestoreCheckpoint`、`MemInfoSchema`、`MemMetaReader`、`MemDomain`。
4. 为迁移后的 restore 流程补齐轻量执行设施：`WorkerPool`、`ErrorGroup`、`WaitGroup`、`WithRetryAggressive`、`ScanRegionsWithRetry`。

这些实现的保证范围是编译、局部逻辑联调和独立测试中的可观察行为；文件头的“未实现生产 RPC”是重要边界，内存实现不能作为真实集群兼容性、持久性或网络错误处理的证明。

## 主要符号

- 错误与取消：`Error { msg, code }` 支持 `new`、`with_code`、`Trace`、`Annotate`、`Annotatef`、`Errorf`；`berrors::ErrRestoreNotFreshCluster` 附带稳定错误码。`Context` 通过 `Arc<Mutex<Option<Error>>>` 保存本地取消原因，可链接父 context 和 deadline；`CancelFunc::call` 写入 `context canceled`。
- 时间戳与协议最小模型：`PhysicalShiftBits = 18`，`ComposeTS` 将 physical 左移后与 logical 合并；`import_sstpb::SwitchMode`、`metapb::{Store, Region}`、`RegionInfo` 只保留当前调用所需字段。
- 集群边界：`PdClient::{GetTS, GetAllStores}`、`SplitClient::ScanRegions`、`ImportSstSwitcher::SwitchMode`、`ConnMgr` 的 scheduler 移除/恢复接口。`UndoFunc` 是线程安全的 context 回调；`Nop`/`nop_undo` 提供空恢复动作。
- 存储与元数据：`Storage` 定义 walk/read/write/delete；`InfoSchema`、`MetaReader`、`Domain` 提供表、库和简单表元数据读取；`PiTRIdTracker` 用三个 `HashSet<i64>` 查询 table、partition、database ID。
- checkpoint 与汇总：`RestoreCheckpoint::{AppendFile, AppendRangeKey}`；`SummaryCollector` 和 `summary` 模块记录失败、耗时和成功单元。
- 并发：`WorkerPool::ApplyOnErrorGroup` 限制同时活跃任务；`ErrorGroup::{with_context, spawn, Wait}` 创建线程、收集首错并取消子 context；`WaitGroup::{Add, Done, Wait}` 模拟 Go 等待组。
- region 与重试：`ScanRegionsWithRetry` 校验输入范围并最多调用 `SplitClient` 三次；`WithRetryAggressive` 最多尝试六次，并在重试间进行 1、2、4、8、16 毫秒量级退避。
- 测试替身：`RecordingImportSstSwitcher` 记录 `(address, mode)`；`MemConnMgr` 记录 scheduler 移除；`MemStorage` 持有路径到字节数组的映射；`MemRestoreCheckpoint` 记录文件与 range；`MemPdClient` 可注入前三次 TSO 错误；`MemSplitClient` 返回固定 region 序列；`Mem*Schema`/`MemDomain` 提供内存元数据。

## 执行流程

典型调用链有三条：

1. `misc.rs::GetTSWithRetry` 调用 `WithRetryAggressive`，闭包再经 `PdClient::GetTS` 获取 `(physical, logical)`，成功后交给 `ComposeTS`。context 已取消时重试器立即返回；普通失败保留最后一次错误。
2. `misc.rs` 的 region scanner 调用 `ScanRegionsWithRetry`。函数先拒绝非空且反向的 `[start_key, end_key)`，再最多三次调用 `SplitClient::ScanRegions`，成功立即返回，失败返回最后一个错误。`MemSplitClient` 从覆盖 `key` 的 region 开始复制结果并遵守正数 `limit`。
3. `import_mode_switcher.rs` 从 `PdClient::GetAllStores` 取得 store，通过 `WorkerPool` 向 `ErrorGroup` 投递 `ImportSstSwitcher::SwitchMode`；pre-work 再调用 `ConnMgr` 暂停 scheduler，post-work 执行 `UndoFunc` 并切回 normal。`RecordingImportSstSwitcher` 和 `MemConnMgr` 让测试能够断言地址、模式与调用发生事实。

restore 数据导入路径中，`restorer.rs` 通过 `WorkerPool::ApplyOnErrorGroup` 异步导入 SST，完成后由 `ErrorGroup::Wait` join 全部已登记线程；可选 `RestoreCheckpoint` 记录已完成文件/range。`misc.rs` 的 blocklist 文件遍历则通过 `Storage::WalkDir` 得到排序后的路径，在 worker 中读取并处理数据。

## 数据与状态

多数状态用 `Arc`、`Mutex` 或 atomic 包装，以便 trait object 跨线程共享。`Context` 的 clone 共享自身取消槽，同时通过 `parent: Option<Arc<Context>>` 递归观察父取消；deadline 只在调用 `Err`/`Done` 时按 `Instant::now()` 惰性判断，没有定时器线程。

`MemStorage.files`、`MemInfoSchema.tables/schemas`、`MemMetaReader.tables`、`MemPdClient.stores/ts` 都是进程内易失状态。`MemStorage::WalkDir` 按 `SubDir` 前缀过滤并排序后回调，给测试稳定次序；写入复制数据，读取返回副本。`SUMMARY` 是 `thread_local!`，所以每个线程拥有独立 collector，而不是全局聚合器。

`WorkerPool.active` 只统计已获得槽位的任务；每个任务在线程内部自旋/yield 等待槽位。`ErrorGroup.handles` 保存 join handle，`inflight` 防止 `Wait` 在嵌套任务尚可能继续登记 handle 时提前退出。`WaitGroup.count` 与 condvar 协调完成通知，但 `done_flag` 一旦变为 true 不会在后续 `Add` 时重置。

## 依赖与调用关系

本文件只直接使用 Rust 标准库集合、格式化、线程、时间、锁和原子类型。它的上游入口是 `lib.rs` 的公开再导出；主要直接调用者包括：

- [`br/pkg/restore/misc.rs`](misc.rs)：使用 context、错误、PD/region/storage/domain 接口、worker、重试器和 `PiTRIdTracker`。
- [`br/pkg/restore/import_mode_switcher.rs`](import_mode_switcher.rs)：使用 PD、mode transport、连接管理、worker/error group 和 wait group。
- [`br/pkg/restore/restorer.rs`](restorer.rs)：使用 error group、worker、checkpoint 与 pipeline split 接口。
- `br/pkg/restore/snap_client/**`：复用 region、storage、PD 及若干本地模型边界；RustCodeGraph 的文件级关系也显示大量测试与相邻 restore crate 使用本文件。

下游依赖被刻意抽象为 trait；真实实现由调用方注入，本文件本身不会打开网络连接、访问磁盘或启动异步 runtime。对 `ScanRegionsWithRetry`、`WithRetryAggressive` 和 `ApplyOnErrorGroup` 的 RustCodeGraph 精确符号查询存在同名歧义，故调用位置以图的文件使用边加 `rg` 的具体引用行核验；未将同名的 `br/pkg/restore/split/split.rs` 实现误算为本文件实现。

## 错误处理与边界

`Error` 只保存字符串和可选静态错误码，没有 source chain；`Annotate` 会保留 code 并拼接文本。所有 `Mutex::lock`、线程 `join` 之外的锁访问普遍使用 `unwrap`：锁中毒会 panic；`ErrorGroup::Wait` 会把 worker panic 转换为 `Error("worker panicked")`，但其他位置的 panic 不会统一封装。

关键边界如下：

- `ScanRegionsWithRetry` 拒绝 `start_key > end_key`，但与 Go `br/pkg/restore/split/split.go::ScanRegionsWithRetry` 相比，没有 `checkPartRegionConsistency`、错误注解、follower-read 首次选项或真实 backoff，只固定尝试三次。
- `WithRetryAggressive` 总计最多六次，context 取消优先；其毫秒级退避是轻量测试语义，不等价于 Go 的完整 backoff 策略。
- `MemStorage::ReadFile` 对缺失路径报错，`DeleteFile` 对不存在路径仍返回成功；所有 storage 方法当前忽略 context。
- `MemSplitClient` 找不到覆盖起点时从第一个 region 开始，空输出但源集合非空时也回退首项；这适合固定 fixture，不代表 PD 扫描契约。
- `PiTRIdTracker` 仅有三个集合；Go `br/pkg/utils/filter.go` 还维护 table 到 database 的多值映射和名称映射，因此跨库 rename 等语义未由本桩表达。
- `log::{Info, Warn, Error}` 是空操作，`summary` 是线程局部记录；不能依赖它们提供生产可观测性。

## 并发与资源生命周期

`WorkerPool` 每次投递都会创建一个 OS 线程；并发槽在线程内部获取，这一设计允许 `BatchRestorer` 的嵌套投递不在调用线程上死锁，但等待方式是 `thread::yield_now()`，高竞争下可能耗 CPU，也没有公平性保证。闭包结束后 active 计数递减；若闭包 panic，该递减语句不会执行，不过 `ErrorGroup` 仍能把 join panic 报为错误，池计数则可能泄漏槽位。

`ErrorGroup::Wait` 循环取出当前 handles 并 join；错误首次出现时调用内部 context 的 `cancel`，后续尚未启动执行的 worker 在入口检查该 context。已经执行中的闭包不会被强制终止，只能自行检查共享 context。返回值只保留首个观察到的错误。

`WaitGroup` 在计数从 1 降到 0 时唤醒全部等待者。调用方必须保证 `Done` 不会在计数为 0 时执行，否则 `AtomicUsize::fetch_sub` 下溢；`Add` 对任意负数只执行一次 `Done`，并非 Go `Add(delta)` 的完整负 delta 语义。`Context`、内存实现与 recording 实现依靠 `Arc`/锁共享所有权，不需要显式 close；真实网络、文件和 runtime 生命周期属于注入实现，不在本文件管理。

## 与 Go 版本的对应关系

本文件没有单一同名 Go 文件，而是把多个 Go 包边界集中成 Rust 兼容层：

- `Context`/`CancelFunc` 对应 `context.Context` 与 cancel function；只模拟取消、父传播和 deadline 查询。
- `ErrorGroup` 对应 `golang.org/x/sync/errgroup`；`WorkerPool` 对应 [`pkg/util/worker_pool.go`](../../../pkg/util/worker_pool.go) 的 `NewWorkerPool`/`ApplyOnErrorGroup`。Go 池复用带 ID 的 worker channel；Rust 版本创建线程并用计数限流，且未实现 `IdleCount`、`ApplyWithID` 等完整 API。
- `ScanRegionsWithRetry` 对应 [`br/pkg/restore/split/split.go`](split/split.go) 同名函数。输入范围检查和“失败返回错误”意图一致，但生产 Go 版本还验证 region 连续性并使用专用 backoff。
- `PdClient`、`ImportSstSwitcher`、`ConnMgr` 支撑 [`br/pkg/restore/import_mode_switcher.go`](import_mode_switcher.go) 的获取 store、并发切 mode、暂停/恢复 scheduler 流程。Rust recording/memory 类型替代真实 PD、gRPC 和 scheduler 管理。
- `Storage` 对应 BR storage API；`MemStorage` 用于 blocklist/checkpoint 等测试，不提供持久化和对象存储语义。
- `PiTRIdTracker` 对应 [`br/pkg/utils/filter.go`](../utils/filter.go)，当前 Rust 数据模型比 Go 简化，尤其不表达 table-to-DB 关系。

因此“对齐”应理解为当前被移植调用点所需的接口和测试意图对齐，而不是 Go 完整子系统的等价实现。扩展时应先确认目标行为来自哪个 Go 文件，避免继续把无关生产能力堆入集中桩文件。

## 扩展指南

新增 restore 能力时，优先在真实所属模块定义实现，仅在需要隔离未迁入依赖或为调用方注入边界时扩展本文件。修改步骤建议为：

1. 先定位 Go 权威实现及当前 Rust 调用者，明确需要保留的输入、错误、取消和生命周期语义。
2. 若是外部边界，先扩展最小 trait，再提供独立真实实现与内存实现；不要让生产路径默认落到 `Mem*` 类型。
3. 若修改 `WorkerPool`/`ErrorGroup`，必须覆盖嵌套投递、首错取消、panic、在飞任务收束和并发上限；避免在源文件内嵌测试，按仓库要求更新同目录独立测试文件。
4. region 行为优先在 `br/pkg/restore/split/` 的真实实现演进；若本桩也需一致，至少同步范围检查、连续性、空结果、limit、取消和重试耗尽测试。
5. 修改 storage、PD、domain 或 checkpoint 内存状态时，同步 `misc_test.rs`、`import_mode_switcher_test.rs`、`restorer_test.rs`/`parity_test.rs` 中相应 fixture，并明确它不验证真实网络/持久化。

兼容风险主要是公开再导出的 trait 签名变化会影响大量调用者；正确性风险集中在简化语义被误用于生产；性能风险集中在 OS 线程创建、自旋让出和粗粒度 mutex。若要生产化这些设施，应迁往独立模块并补真实集成验证，而不是静默增强测试桩。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/restore/stubs.rs` 命中目标；`node --file ...` 分段读取完整 1,150 行并报告 153 个符号、66 个使用文件；`query ScanRegionsWithRetry --kind function` 区分了本文件、Rust split 实现和 Go 同名实现。对同名符号的 `callers/callees` 查询未在限定时间内返回，调用边改由文件使用关系与下述文本引用交叉验证。
- crate/入口：读取 `br/pkg/restore/Cargo.toml` 与 `br/pkg/restore/lib.rs`，确认 package、依赖、模块挂载和公开再导出。
- Rust 调用者：读取/检索 `br/pkg/restore/misc.rs`、`import_mode_switcher.rs`、`restorer.rs`，并检索 `snap_client/**` 对桩接口的引用。
- Go 对照：读取 `br/pkg/restore/split/split.go::ScanRegionsWithRetry`、`br/pkg/restore/misc.go` 的并行 storage walk、`br/pkg/restore/import_mode_switcher.go` 的 mode/scheduler 流程、`pkg/util/worker_pool.go` 与 `br/pkg/utils/filter.go`。
- 独立测试：读取/检索 `br/pkg/restore/misc_test.rs`、`import_mode_switcher_test.rs`、`restorer_test.rs`、`parity_test.rs`、`split/split_test.rs`，确认重试、内存存储、context、mode 记录、worker/error group、checkpoint 和非法 region 范围均有相邻独立测试证据；本任务按计划不运行 Cargo。
- 人工复核：本文区分了 trait、内存替身和真实实现，明确列出简化项与未验证的生产能力，没有把测试通过或接口存在表述为真实 RPC/持久化已支持。
