# `br/pkg/streamhelper/advancer.rs`

## 文件定位

本文件实现日志备份 Checkpoint V3 的核心推进状态机，crate 入口 [`lib.rs`](lib.rs) 以 `#[path = "advancer.rs"]` 注册模块并重新导出主要构造器、推进器类型和 resolve-lock 辅助函数。所属 crate 是 `astersql-br-pkg-streamhelper`；[`Cargo.toml`](Cargo.toml) 声明它是 `br/pkg/streamhelper` Go 包的 library 移植，直接依赖 `config` 与 `spans` 两个相邻 crate。

运行入口不在本文件内部自建循环，而在 [`advancer_daemon.rs`](advancer_daemon.rs) 的 `CheckpointAdvancer::OnTick`：每个 Owner daemon tick 先调用 `refreshLogBackupFlushInterval`，再进入本文件的 `tick`。`OnStart` 会调用 `StartTaskListener` 装载任务快照，`OnBecomeOwner` 安装 flush subscriber，`OnStop` 则关闭存储边界并清理订阅。因此，本文件是任务元数据、Region flush TS、PD 全局检查点与 GC safe point 之间的协调核心，不负责 Owner 选举本身。

## 核心职责

- 通过 `onTaskEvent` / `StartTaskListener` 接收任务添加、删除、暂停、恢复和监听错误，维护当前 `StreamBackupTaskInfo`、任务 key 范围及初始检查点。
- 通过 `SetTask` 建立 `ValueSortedFull` 区间树；通过 `GetCheckpointInRange`、`tryAdvance` 和 `optionalTick` 扫描 Region、采集各 Store flush TS，并把成功结果合并回区间树。
- 通过 `importantTick` 读取区间树的最小值，将其上传为任务 V3 全局检查点，检查滞后上限，并把 GC safe point 推到 `checkpoint - 1`。
- 管理命令行/TiDB 两类配置，以及从 TiKV flush interval 派生的 resolve-lock 间隔和轮询阈值。
- 管理 flush subscriber 的安装、拓扑维护、错误收集和清理，并用互斥锁保证停止动作等待正在执行的 `subscribeTick`。
- 提供 TSO 上下界、ScanLock locked 错误识别和降低 `maxVersion` 的有界 resolve-lock 重试辅助函数。

当前 Rust 实现是精简移植：Go `advancer.go` 中的阈值式 `CalculateGlobalCheckpointLight`、自动 `tryResolveLocksForCheckpoint`/异步范围解锁、持续 watch 重连，以及真实外部存储检查点写入没有在本文件接线。`closeGlobalCheckpointStorage` 明确是空操作，`lastExternalStorageCheckpoint` 目前没有生产写入；这些能力不能仅因同名字段或辅助函数存在而视为已支持。

## 主要符号

- 常量：`streamBackupGlobalCheckpointPrefix` 保留历史外部存储路径；`resolveLockMaxVersionMaxRetry = 2` 限制 locked 错误后的额外重试；`resolveLockRetryLowerBoundLag = 10s` 定义重试窗口下界；`logBackupConfigRefreshInterval` 与 `logBackupConfigFetchTimeout` 保留配置刷新契约；私有 `PHYSICAL_SHIFT_BITS = 18` 用于 TSO 物理毫秒与逻辑位之间的换算。
- `Checkpoint`：保存 `[StartKey, EndKey)`、`TS` 和本地 `resolveLockTime`。`safeTS` 对非零 TS 返回 `TS - 1`；`equal` 忽略时间只比较范围和 TS；`needResolveLocks` 按本地 `Instant` 判断停滞时间。
- `newCheckpointWithTS` / `newCheckpointWithSpan`：分别构造无范围快照和从 `Valued` 区间构造快照，都会把 resolve-lock 计时重置为当前时刻。
- 私有 `Cfg::{Command, TiDB}`：将两类配置统一适配到 `Config` trait。`UpdateConfigCommand`、`UpdateConfigTiDB` 替换整个变体；`updateConfigWithForTest` 只供 crate 内测试修改当前 `CommandConfig`（TiDB 模式修改其嵌套字段）。
- `CheckpointAdvancer`：核心共享状态。`env: Arc<dyn Env>` 汇总 Region 元数据、日志备份 RPC、任务元数据、锁解决和配置读取能力；其余任务、配置、区间树、订阅器和检查点缓存均由锁或原子保护。
- `NewTiDBCheckpointAdvancer` / `NewCommandCheckpointAdvancer`：按运行场景装入不同默认配置。测试便利构造器 `NewCheckpointAdvancer` 在 Rust 中确定性选择 TiDB 配置，而 Go 测试导出层会随机选择两类配置。
- `SetTask`：设置任务和范围，把空范围解释为 `Full()` 全键空间，再以初值 0 初始化排序区间树。
- `tryAdvance`：先 `Collapse` 合并重叠输入范围，再顺序迭代 Region；若提供成功 hook，会先把结果合并进共享检查点树，再调用外部 hook。
- `optionalTick` / `importantTick` / `tick`：构成一次推进的三个层次；前者采集，第二个上传与推进 GC，最后一个负责跳过条件和聚合两侧错误。
- `resolveLockTargetUpperBound`、`resolveLockRetryLowerBound`、`isScanLockLockedError`、`lowerResolveLockMaxVersion`、`resolveLocksForRangeWithMaxVersionRetry`：实现与 Go 对应的 TSO 窗口计算和最多两次二分降版重试；当前未由 Rust `optionalTick` 自动调用。

## 执行流程

1. 启动阶段由 `advancer_daemon.rs::OnStart` 调用 `StartTaskListener`。它让 `Env::Begin` 填充一个事件向量，并按交付顺序调用 `onTaskEvent`；当前接口处理的是快照/批次，不在本文件启动 Go 式长期监听 goroutine。
2. `EventAdd` 读取已有全局检查点，读取失败时用 0，再与任务 `StartTs` 取最大值；随后 `SetTask` 初始化范围树、更新 `lastCheckpoint`，并 best-effort 调用 `BlockGCUntil(checkpoint - 1)`。`EventDel` 清空任务、范围、区间树和订阅，恢复非暂停状态，并 best-effort 清除 V3 检查点和解除 GC 阻塞。Pause/Resume 只在事件名称匹配当前任务时切换原子标志；`EventErr` 返回监听错误。
3. Owner daemon 的 `OnTick` 先刷新 TiKV flush interval。非零 interval 被写入 `resolveLockInterval`，并按 `flush * 4 / 3` 写入 `tryAdvanceThreshold`；订阅错误阈值读取该值的 `9 / 20`。这些阈值保持 Go 算术，但当前 Rust `optionalTick` 没有使用轮询阈值筛选范围。
4. `tick` 在没有任务或已暂停时直接成功返回；否则先运行 `optionalTick`，再运行 `importantTick`。两步即使前一步失败也都会尝试，最终用 `"; "` 拼接错误。
5. `optionalTick` 先运行 `subscribeTick`。拓扑更新失败只打印错误，随后处理订阅错误；若 `PendingErrors` 返回错误，该错误被本方法刻意忽略，继续走 polling。然后克隆全部 `taskRange`；空范围立即返回，否则创建合并 hook，并通过 `tryAdvance` 扫描全部任务范围。
6. `tryAdvance` 折叠重叠范围，为每次调用新建 `ClusterCollector`，用 `IterateRegion` 分批枚举 Region 并调用 `CollectRegion`。`collector.Finish()` 聚合 Store 结果；成功 hook 把每段 TS 合并到共享 `ValueSortedFull`。
7. `importantTick` 从区间树获取 `MinValue`。无任务、树未初始化或最小值为 0 时不上传；否则用最小区间刷新 `lastCheckpoint`（拒绝 TS 回退和完全相同快照），调用 `UploadV3GlobalCheckpointForTask`。之后检查全局检查点相对当前 TSO 是否超过配置上限：超限则调用 `PauseTask`、立即置暂停并返回错误；未超限则调用 `BlockGCUntil(checkpoint - 1)`。
8. resolve-lock 辅助流程由调用者先计算目标上界与重试下界，再调用 `resolveLocksForRangeWithMaxVersionRetry`。首次调用失败时，只有错误串同时包含 `unexpected scanlock error` 与 `locked`、下界有效且未超过两次额外重试，才把当前版本向下界取中点后重试；其他错误原样返回。

## 数据与状态

`task`、`taskRange`、`cfg`、`lastCheckpoint`、`checkpoints` 和 `subscriber` 分别使用 `Mutex`，允许从共享 `&self` 更新；`env` 用 `Arc<dyn Env>` 跨推进器、collector 和 subscriber 共享。`checkpoints` 是 `Option<ValueSortedFull>`：`None` 表示尚未绑定/已删除任务，`Some` 中每段保存该 key 范围最近成功采集的 TS，其 `MinValue` 是全任务推进的水位。

`resolveLockInterval` 与 `tryAdvanceThreshold` 用纳秒 `AtomicI64` 存储，0 表示回退到 `cfg`；`inResolvingLock` 预留给解锁防重入状态，但本文件当前没有把它置为 true 的生产流程；`isPaused` 是 tick 的快速门禁。所有原子操作使用 `SeqCst`，优先保证清晰的一致顺序而非最弱内存序。

`lastCheckpoint` 的单调规则由 `setCheckpoint` 保证：较旧 TS 和范围、TS 完全相同的快照不会替换；相同 TS 但范围变化可替换，以保留 Go 为不同阻塞范围触发解锁的语义。`resolveLockTime` 使用进程本地单调时钟，不是 PD 时间。`lastExternalStorageCheckpoint` 字段以原子形式存在，但当前只有初始化，没有真实存储写入路径。

## 依赖与调用关系

上游主链为 `advancer_daemon.rs::OnTick → CheckpointAdvancer::tick → optionalTick / importantTick`。RustCodeGraph 还显示 `NewCheckpointAdvancer`、`HasTask`、`HasSubscriptions`、`SetPaused`、`GetInResolvingLock` 等被 [`advancer_test.rs`](advancer_test.rs) 直接调用；模块入口 [`lib.rs`](lib.rs) 对外重新导出构造器和 resolve-lock 辅助 API。

下游依赖包括：

- [`advancer_env.rs`](advancer_env.rs) 的 `Env`：组合 `TiKVClusterMeta`、`LogBackupService`、`StreamMeta`、`RegionLockResolver` 和 `LogBackupFlushIntervalGetter`，承载所有外部 I/O。
- [`regioniter.rs`](regioniter.rs) 的 `IterateRegion`：按 key 范围枚举 Region。
- [`collector.rs`](collector.rs) 的 `NewClusterCollector` / `OnSuccessHook`：按 Store 获取 flush TS、汇总失败子范围并回调成功结果。
- [`flush_subscriber.rs`](flush_subscriber.rs) 的 `FlushSubscriber`：维护 Store 拓扑、消费错误状态并清理订阅。
- `astersql-br-pkg-streamhelper-spans` 的 `Full`、`Collapse`、`Sorted`、`ValueSortedFull` 和 `Valued`：描述、合并及按值查询 key 区间。
- `config` crate 的 `Config`、`CommandConfig`、`TiDBConfig`：提供 tick、退避、轮询、解锁和滞后上限配置。

Go 索引显示完整 Go 构造器还由 `br/pkg/task/stream.go::RunStreamAdvancer` 与 `pkg/domain/domain.go::initLogBackup` 调用；RustCodeGraph 对 Rust 生产侧仅确认 daemon 适配和 crate 导出，不能据此宣称同等的顶层进程接线已经完成。

## 错误处理与边界

- `Mutex::lock().unwrap()` 将 poisoned mutex 视为不可恢复并 panic；接口层没有把锁中毒转换为 `String` 错误。
- `onTaskEvent(EventAdd)` 对读取旧检查点失败回退为 0，对首次 GC 阻塞失败忽略；`EventDel` 的清理检查点和解除 GC 也均为 best-effort。任务添加事件缺少 `Info`、或 `EventErr`，则明确返回错误。
- `subscribeTick` 忽略拓扑刷新错误、打印后继续处理；`optionalTick` 又忽略 `subscribeTick` 的最终错误并改走全范围轮询。与 Go “缩短轮询阈值后 polling”相比，Rust 当前直接扫描全部任务范围。
- Region 迭代、collector 采集/完成、PD 上传、暂停任务和 GC safe point 更新错误会向上返回。`tick` 会同时尝试 optional/important 两侧并聚合错误，因此调用者不能假设第一个错误会短路全轮。
- `importantTick` 在最小值为 0 时不上传；GC 目标使用 `saturating_sub(1)`，避免零值下溢。滞后检查自身失败通过 `unwrap_or(false)` 降级为“不滞后”，不会阻止后续 GC 更新。
- TSO 算术使用饱和加减，避免整数溢出/下溢；但它直接按 `Duration` 毫秒左移 18 位，刻意忽略逻辑位细节。resolve-lock 只凭错误文本分类，错误文案变化会使其停止重试。
- `resolveLocksForRangeWithMaxVersionRetry` 最多进行首次尝试加两次额外尝试；无有效下界、版本间隙不大于 1、非 locked 错误都会立即返回原错误。
- 输入任务范围为空时，`SetTask` 初始化为全键空间；但 `optionalTick` 读取的原始 `taskRange` 仍为空并直接返回。这是当前代码可观察到的边界，扩展时必须先确定空范围究竟代表“全空间”还是“无需扫描”，不可仅修改其中一侧。

## 并发与资源生命周期

推进器可被 daemon tick、任务事件和 Owner 生命周期钩子共享。细粒度互斥锁避免 Rust 数据竞争，但 Rust `tick` 不像 Go 版本那样在整个 tick 期间持有统一的 `taskMu`：它分别克隆任务与范围，因此任务事件可能在 optional/important 两阶段之间发生。新增异步监听或解锁任务时，应明确任务代次/取消语义，避免旧任务结果合并进新任务区间树。

`subscribeTick` 从取得 `subscriber` 锁开始，一直持有到测试 hook、拓扑更新、错误处理和 `PendingErrors` 完成；`stopSubscriber` 必须等待该临界区结束，再 `take()` subscriber 并调用 `Clear()`。`test_owner_dropped` 用两个线程验证了这一等待关系以及停止后 polling 仍可继续。

当前文件不会生成后台线程。`SpawnSubscriptionHandler` 仅构造并保存 subscriber；Go 版本另起 goroutine 消费 flush event，而 Rust 的事件流行为封装/简化在相邻 subscriber 实现和测试夹具中。resolve-lock 函数也是同步调用；Go 的 worker pool、后台 goroutine、`inResolvingLock` CompareAndSwap 与成功后刷新 `resolveLockTime` 尚未移植到 Rust 主链。

任务删除与 Owner 停止都会清理 subscriber；删除事件还清空范围树并解除 GC。外部存储资源在 Go 中需要 Close，Rust 的 `closeGlobalCheckpointStorage` 当前为空操作，因此没有实际句柄生命周期。引入真实存储后，应同时实现创建、单调写入、超时、关闭和任务切换时重置，不能只填充关闭函数。

## 与 Go 版本的对应关系

直接对照文件是 [`advancer.go`](advancer.go)，相关测试为 [`advancer_test.go`](advancer_test.go) 与 Rust 的 [`advancer_test.rs`](advancer_test.rs)。Rust 保留了 Checkpoint/Advancer 的核心字段、两类构造器、任务事件语义、区间树最小值上传、暂停判断、GC safe point、flush interval 的 `4/3` 派生规则，以及 ScanLock locked 时二分降低 `maxVersion`、最多重试两次的算法。

已确认的差异如下：

- Go `tryAdvance` 使用 worker pool 并发扫描，将候选范围与 `taskRange` 再求交；Rust 顺序扫描调用者传入的折叠范围，未二次求交。
- Go `CalculateGlobalCheckpointLight` 只轮询低于阈值的慢区间；Rust `optionalTick` 每次轮询全部非空任务范围，读取到的阈值目前没有参与筛选。
- Go `optionalTick` 首先自动尝试 resolve-lock，并在后台并发处理目标范围；Rust 主链没有调用相应辅助函数，`inResolvingLock` 也没有生产置位路径。
- Go `StartTaskListener` 包含失败退避、后台 channel 消费和断线重开；Rust 只同步应用 `Env::Begin` 返回的事件批次。
- Go flush subscriber 启动事件消费 goroutine；Rust 本文件只持有 subscriber 并在 tick 中维护拓扑/错误。
- Go 把全局检查点以小端 8 字节写入任务外部存储，并以 tick timeout 限时、失败仅告警；Rust 存储工厂测试接口和字段是 slim-port 边界，`closeGlobalCheckpointStorage` 为空操作，没有真实落盘。
- Go tick 用 context timeout 限制 optional 部分并在整个 tick 持有任务锁；Rust API 不接收 context，也没有单次 tick 超时或整轮任务锁。
- Go 的 lag-check 读取失败仅告警；Rust同样降级为不滞后。Go Pause 附带解释消息和 error severity，Rust `Env::PauseTask` 只传任务名，并提前设置本地暂停标志。
- Rust `NewCheckpointAdvancer` 固定采用 TiDB 默认配置，Go 的该名称来自测试导出辅助并随机选构造器。

因此维护时应以 Go 为语义基准，但先判断目标是补齐迁移还是保持当前 slim 边界；不能用现有 Rust 单元测试通过来推断上述未接线能力已经等价。

## 扩展指南

- 修改推进循环时，从 `tick`、`optionalTick`、`importantTick` 三层分别判断错误聚合、轮询/订阅和强制上传职责；同步更新 `advancer_test.rs` 中 `test_tick`、失败恢复、暂停/恢复、滞后和 GC 用例。
- 补齐轻量轮询时，建议新增与 Go `CalculateGlobalCheckpointLight` 对应的独立方法，复用区间树遍历并保留阈值差异；同时覆盖慢区间选择、范围交集、Region split 和 subscriber 错误回退，避免直接把全量扫描伪装成阈值轮询。
- 接入自动 resolve-lock 时，需要同时补齐 `checkpointToResolve`、目标区间筛选、CAS 防重入、后台任务取消/错误收集及成功后条件刷新 `resolveLockTime`；同步扩展独立的 `advancer_test.rs`，不要把测试嵌进源文件。
- 接入持续任务 watch 时，应在 `advancer_cliext.rs`/`Env` 边界设计事件流、取消、退避和重连，并处理事件与 tick 的任务代次竞争；`StartTaskListener` 目前的同步签名不足以表达 Go 生命周期。
- 接入外部存储时，应围绕 `lastExternalStorageCheckpoint` 实现只增写入、8 字节小端格式、`v1/global_checkpoint/global.ts` 路径、tick timeout、best-effort 失败以及任务删除/切换关闭；参考 Go 的 storage 测试，但为 Rust 使用独立测试文件。
- 修改锁或原子布局时，特别审查 subscriber 锁覆盖范围和 task/range/checkpoints 的一致快照。若引入多个锁的嵌套，必须固定锁顺序并新增并发停止/任务切换测试。
- 修改 TSO 算术或错误识别时，同步核对 Go `oracle` 行为以及 `test_resolve_lock_targets_use_upper_bound`、`test_resolve_lock_retry_*`、`test_resolve_lock_max_version`；性能风险主要来自全范围串行 Region 扫描和锁内 hook，兼容风险主要来自检查点单调性、暂停策略与 GC safe point 顺序。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/streamhelper` 确认本文件、Go 对照、模块入口及独立测试均被索引。
- RustCodeGraph `node --file br/pkg/streamhelper/advancer.rs`：分段读取全部 719 行，核对 5 个模块级公开常量、TSO 私有常量/函数、`Checkpoint`、`Cfg`、`CheckpointAdvancer`、构造器、推进方法和 resolve-lock 函数；文件没有生产条件编译，唯一条件编译项是 `#[cfg(test)] subscribeTickHook` 及其 setter。
- RustCodeGraph `query`：确认 Rust/Go 两份 `CheckpointAdvancer`、`NewTiDBCheckpointAdvancer`、`NewCommandCheckpointAdvancer`、`importantTick`、`optionalTick` 和 `resolveLocksForRangeWithMaxVersionRetry` 的对应符号；索引汇总确认 `advancer.rs` 由 `advancer_test.rs` 直接使用，`OnTick` 位于 `advancer_daemon.rs`。
- RustCodeGraph `explore`：确认 `NewCheckpointAdvancer` 的 Rust 测试调用者、`importantTick → newCheckpointWithSpan/newCheckpointWithTS`、`tick → importantTick`，以及任务/订阅/暂停/解锁相关测试调用关系。精确 `callers/callees` 子命令曾长时间无输出并被中止，因此本文没有用该次卡住查询推导额外边。
- 已读 Rust 路径：[`advancer.rs`](advancer.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`advancer_daemon.rs`](advancer_daemon.rs)、[`advancer_env.rs`](advancer_env.rs)、[`advancer_test.rs`](advancer_test.rs)。目录无 `doc.go`，故以 crate 级 `lib.rs` 作为最近模块契约。
- 已读 Go 路径：[`advancer.go`](advancer.go)；并以任务指定的 [`advancer_test.go`](advancer_test.go) 作为场景命名与完整 Go 行为的对照。Rust 测试实际覆盖基础推进、失败恢复、任务范围与 Region split、暂停/恢复、lag pause、GC、subscriber 停止互斥、flush interval 和 resolve-lock 重试；测试注释也明确外部存储属于 slim-port 空操作。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构检查，并人工复核本文没有把 Go 的未移植链路描述为 Rust 已支持。
