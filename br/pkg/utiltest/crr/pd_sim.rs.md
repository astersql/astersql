# `br/pkg/utiltest/crr/pd_sim.rs`

## 文件定位

本文件是 `astersql-br-pkg-utiltest-crr` 测试夹具 crate 内的 PD 状态仿真核心。crate 由 `br/pkg/utiltest/crr/Cargo.toml` 定义，`lib.rs` 通过 `pub mod pd_sim` 纳入模块并用 `pub use pd_sim::*` 再导出公开符号。它不是生产 PD 客户端，也不负责真实网络 RPC；它在内存中的 `fakecluster::Cluster` 之上提供静态 Region 拓扑、TSO、Region checkpoint、全局 checkpoint 和散射行为，供 CRR/checkpoint advancer 测试使用。

职责边界很明确：本文件维护 `PDSim` 的状态机；`pd_sim_service.rs` 把该状态机适配为 `TiKVClusterMeta`、`LogBackupService`、`StreamMeta` 等 streamhelper trait；`harness.rs` 再把 `PDSim`、`FlushSim`、复制 worker 和 `CheckpointAdvancer` 组合成完整测试线束。布局一旦由 `NewPDSimWithTestContext` 创建便不支持 split/merge。

## 核心职责

- `NewPDSimWithTestContext` 校验或补全 Region 边界，创建 store/region，配置 flush subscription，并把 fakecluster 时钟锚定到确定性的任务起始 TSO。
- `AllocTSO`、`CurrentTSO`、`RegionIDs`、`RegionSnapshot`、`RegionSnapshotsOnStore` 是对 fakecluster 的窄封装，把底层状态转换为本 crate 的 `RegionState`。
- `flushStore` 把某个 store 上所有 Region 的 checkpoint 推进到指定值，同时把本地 `Context` 的取消状态桥接到 fakecluster 的 context。
- `Scatter` 在已有 store 之间随机迁移 Region；实际迁移时更换 leader、递增 epoch，并把 Region checkpoint 重置到当前全局 checkpoint。
- `upload_v3_global_checkpoint`、`get_global_checkpoint`、`clear_v3_global_checkpoint`、`wait_global_checkpoint_advance` 共同实现按单一任务名约束的全局 checkpoint 状态机。

该模拟器只承诺测试所需行为。真实服务接口以及刻意不支持的锁扫描/锁解析位于 `pd_sim_service.rs`，不应把本文件理解为完整 PD 或 TiKV 实现。

## 主要符号

- `PDSimState`（crate 内可见）：保存 `task_name`、`task_start`、`global_checkpoint`、`checkpoint_gen` 和确定性 `rng`。这些字段受 `PDSim::state` 的同一把 `Mutex` 保护。
- `PDSim`（公开）：持有公开的 `cluster: fakecluster::Cluster`、内部状态锁和与其配对的 `checkpoint_cv: Condvar`。调用方通常通过 `Arc<PDSim>` 共享它。
- `NewPDSimWithTestContext(boundaries, taskName, tc) -> Result<Arc<PDSim>>`（公开）：唯一构造入口。空布局变为一个覆盖全键空间且位于 store 1 的 Region；空任务名变为 `defaultTaskName`。
- `validateBoundaries(&[RegionBoundary])`（私有）：要求每段 `StoreID != 0`、首段起点为空、末段终点为空、起点按字节序排列、相邻段严格首尾相接。
- `AllocTSO` / `CurrentTSO` / `RegionIDs` / `RegionSnapshot` / `RegionSnapshotsOnStore` / `GlobalCheckpoint` / `Scatter`（公开）：提供测试和相邻模块使用的查询或变更入口。
- `flushStore`（crate 内可见）：由 `flush_sim.rs` 的 `FlushSim::flushRegions` 调用；独立测试 `pd_sim_test.rs` 也直接验证其运行中取消行为。
- `upload_v3_global_checkpoint`、`get_global_checkpoint`、`clear_v3_global_checkpoint`、`wait_global_checkpoint_advance`（crate 内可见）：由 `pd_sim_service.rs` 中的 `StreamMeta` 实现和 `WaitGlobalCheckpointAdvance` 公开包装器调用。
- `task_name`、`task_start`（公开）：克隆任务名或读取任务起始 TSO，供 `pd_sim_service.rs::Begin` 构造 `EventAdd`。
- `toRegionState`（私有）：把 fakecluster 的拥有型快照映射到本 crate 的 `RegionState`。

## 执行流程

构造流程如下：

1. `NewPDSimWithTestContext` 为缺省布局和任务名填充值，并调用 `validateBoundaries`；无效布局在创建任何 fakecluster 状态之前返回错误。
2. 从 `TestContext::RNG("pd-sim")` 取得可复现 RNG，以 `defaultTaskStartPhysical + rng.Int63n(1 << 20)` 作为物理部分，经 `oracle::ComposeTS(..., 0)` 生成 `task_start`。
3. 创建 `PDSim`，初始 `global_checkpoint` 和 `checkpoint_gen` 均为 0，再通过 `SetCurrentTS(task_start)` 设置集群时钟。
4. 对每条边界执行 `EnsureStore`，启用 flush subscription、关闭 legacy Region checkpoint RPC，并把 flush 任务名设为 `drr`；随后以 epoch 1、checkpoint=`task_start`、非 pending 状态创建单副本 Region。

刷盘链由 `flush_sim.rs::FlushSim::FlushStore` 发起：它先分配 checkpoint/flush TSO、写备份元数据，再经 `flushRegions` 调用本文件的 `flushStore`。`flushStore` 创建 fakecluster 可取消 context，启动一个短轮询线程把本地取消信号转发过去，执行 `ApplyCheckpointToStore`，标记完成并回收线程，最后转换所有 Region 快照。因而函数返回时取消桥接线程已经结束，不会遗留后台任务。

全局 checkpoint 链由 `pd_sim_service.rs` 接线。上传先校验任务名，再拒绝小于当前值的回滚；成功写值、递增 `checkpoint_gen` 并 `notify_all`。等待方先校验任务名；若当前值已经严格大于参数则立即返回，否则记住起始 generation，以 50 ms 超时循环等待 condvar，并在 context 取消、generation 增加或 checkpoint 严格推进时退出。相同数值的合法重复上传也会增加 generation，因此会唤醒已有等待者，这与 Go 版每次成功上传都会关闭当前 waiter channel 的效果一致。

`Scatter` 在持有状态锁期间获取 store 列表和全局 checkpoint，逐个 Region 用确定性 RNG 选择目标 store。选中原 store 时跳过；迁移时按 `TransferRegionTo → SetRegionLeader → BumpRegionEpoch → SetRegionCheckpoint(global)` 的顺序更新，最后排序并返回受影响 Region ID。

## 数据与状态

- `task_name` 是单任务绑定键。checkpoint 的上传、查询、清空和等待均拒绝不匹配名称；当前模型不支持同时存在多个日志备份任务。
- `task_start` 是构造时生成的 TSO，也是所有初始 Region checkpoint 和 fakecluster 当前时钟的基线。相同 seed 与输入可复现该值，但不同用例仍通过确定性抖动避免固定撞值。
- `global_checkpoint` 初始为 0，上传时只能单调不减；`clear_v3_global_checkpoint` 是显式测试重置入口，允许直接归零，且不会递增 generation 或通知等待者。
- `checkpoint_gen` 不表示 TSO，只表示成功上传次数。它解决了 condvar 无事件缓存的问题，并保留 Go waiter channel 对“重复上传”的唤醒语义。
- `rng` 与其他 checkpoint 状态共用 `state` 锁，所以并发调用 `Scatter` 时随机序列和迁移决策被串行化，可复现且不会数据竞争。
- Region 的真实拓扑和 checkpoint 存在 `fakecluster::Cluster` 内；本文件只在对外返回时生成 `RegionState` 快照。`RegionSnapshot` 对不存在 ID 返回 `(default, false)`，store 查询则保留底层错误。

重要不变量是：合法构造后的 Region 序列覆盖从空起点到空终点的完整键空间且无缝相接；初始 Region epoch 为 1、checkpoint 为 `task_start`；全局 checkpoint 普通上传不回滚；迁移 Region 的 epoch 必须递增且 checkpoint 回落到迁移时的全局值。

## 依赖与调用关系

直接依赖来自 `Cargo.toml` 中的 `astersql-br-pkg-utiltest-fakecluster`，以及 crate 内的 `stubs`、`types`。本文件使用 fakecluster 的 Cluster/Store/Region/TSO/context 能力；`types.rs` 提供 `RegionBoundary`、`RegionState`、`TestContext`、`DeterministicRNG` 和默认任务常量；`stubs.rs` 提供本地 `Context`、`Error`、`Result`。

已核验的上游调用关系包括：

- `harness.rs::newLocalTestHarness → NewPDSimWithTestContext`，随后将 `Arc<PDSim>` 注入 `NewCommandCheckpointAdvancer` 与 `FlushSim`。
- `flush_sim.rs::FlushSim::flushRegions → PDSim::flushStore → fakecluster::Cluster::ApplyCheckpointToStore`。
- `flush_sim.rs::FlushStore` 调用 `RegionSnapshotsOnStore`、`AllocTSO` 和 `GlobalCheckpoint` 来生成一次 flush。
- `pd_sim_service.rs::StreamMeta::{UploadV3GlobalCheckpointForTask, GetGlobalCheckpointForTask, ClearV3GlobalCheckpointForTask}` 分别委托本文件三个内部 checkpoint 方法；其 `WaitGlobalCheckpointAdvance` 包装器委托 `wait_global_checkpoint_advance`。
- `pd_sim_service.rs::Begin` 使用 `task_name` 和 `task_start` 生成单任务 `EventAdd`；同文件其他 trait 实现直接使用公开 `cluster` 提供 Region/store/GC 服务。
- `parity_test.rs` 直接验证构造、布局拒绝、初始 checkpoint、上传、回滚和未知任务错误；`pd_sim_test.rs` 直接验证 flush 进行中取消。

RustCodeGraph 的文件级索引还报告目标文件被 `harness.rs`、`pd_sim_service.rs`、`parity_test.rs` 等文件使用；精确查询确认 `NewPDSimWithTestContext` 的 Rust 调用者包括上述文件以及 `pd_sim_service_test.rs`。该仿真器位于测试支持链，不在 TiDB/BR 的线上 PD 请求主链中。

## 错误处理与边界

`validateBoundaries` 返回带 Region 下标的本地 `Error`，分别覆盖零 store ID、首段非空、起点乱序、相邻空洞或重叠、末段未闭合。调用者不能传空切片触发末元素访问，因为构造函数已先将空切片替换为缺省 Region；该私有函数当前只有该入口调用。

`RegionSnapshotsOnStore` 和 `flushStore` 把 fakecluster 错误文本映射为本地 `Error`。`flushStore` 在调用前已取消时会立即取消底层 context；调用期间取消则由 1 ms 轮询桥转发。错误在桥接线程被 join 后传播，保证资源清理先于返回。

checkpoint 接口的错误文案是可测试契约：上传/读取/清空未知任务返回 `unknown task "..."`；等待任务不匹配返回 `task name mismatch: ... and ...`；回滚返回 `checkpoint rollback: old -> new`；等待取消返回 context 的错误消息或缺省 `context canceled`。上传允许等于当前 checkpoint，这不是回滚。

锁获取、condvar 等待和取消桥线程 join 使用 `unwrap`：锁中毒或线程 panic 会使测试进程 panic，而不是转换成 `Result`。这是测试夹具的当前边界。`Scatter` 对少于两个 store 返回空列表，对瞬间查不到的 Region 静默跳过；它不创建 store，也不模拟 split/merge。`clear_v3_global_checkpoint` 不唤醒等待者，行为与 Go 版一致。

## 并发与资源生命周期

`PDSim` 通过 `Arc` 共享。`state: Mutex<PDSimState>` 串行化任务元数据、全局 checkpoint、generation 和 RNG；`checkpoint_cv` 必须始终与这把锁配合。上传在持锁时更新值与 generation 并通知全部等待者；等待用 `wait_timeout` 原子释放并重新取得同一把锁，50 ms 周期也让没有上传通知时的 context 取消最终可见。

`flushStore` 每次调用创建一对 fakecluster context/cancel handle 和一个临时桥接线程。`AtomicBool finished` 以 `SeqCst` 协调主线程与桥接线程：底层调用结束后主线程置位并 join；若调用方 context 先结束，桥接线程调用 `cancel_with` 后退出。最坏情况下正常完成会等待约一个 1 ms 轮询周期。大量并发 flush 会为每次调用各占一个短生命周期线程，这是测试实现的性能边界。

`Scatter` 在整个迁移循环中持有 `state` 锁，保证 RNG 与读取的全局 checkpoint 是一致快照，也意味着上传、查询和等待方在大布局散射期间会被阻塞。Cluster 内部同步由 fakecluster 自己负责。本文件不拥有长期任务线程、文件句柄或网络连接；构造出的 Region/store 生命周期跟随 `Arc<PDSim>` 及其 `cluster`。

## 与 Go 版本的对应关系

直接对照文件为 `br/pkg/utiltest/crr/pd_sim.go`，服务侧对照为 `pd_sim_service.go`。Rust 保留了 Go 的缺省布局/任务名、边界检查、确定性 TSO、store flush 配置、单副本 Region 初始化、查询封装、Scatter 更新顺序以及 checkpoint 错误文本。

状态同步实现不同但目标语义一致：Go 使用 `sync.Mutex`、`checkpointWaiters []chan struct{}`，每次成功上传取出并关闭所有 waiter；Rust 使用 `Mutex<PDSimState>`、`Condvar` 和 `checkpoint_gen`，每次成功上传递增 generation 并唤醒全部等待者。两者都允许相同 checkpoint 的重复上传唤醒等待者，都拒绝更小值，都在 clear 时仅归零而不主动唤醒。

取消桥接是 Rust 特有接线：Go 可把 `context.Context` 直接交给 `ApplyCheckpointToStore`，Rust 的本地 `stubs::Context` 与 fakecluster context 类型不同，因此 `flushStore` 使用临时线程传播取消。`toRegionState` 在 Go 中用 `bytes.Clone` 显式复制 key；Rust 接收拥有型 `fakecluster::RegionState` 并移动其 `Vec<u8>` 字段，返回值同样不借用底层对象。

Go 的 `PDSim` 嵌入 `*fakecluster.Cluster`，Rust 改为具名公开字段 `cluster`；因此 Rust trait 适配器显式访问 `self.cluster`。Go 的 `taskCh` 和 `PauseTask` 实时事件属于服务侧；Rust `pd_sim_service.rs` 没有实时 task channel，`PauseTask` 仅校验名称。此差异不应在本文件中“补齐”，除非 streamhelper Env 的任务订阅模型同时改变。

## 扩展指南

- 增加布局规则时，修改 `validateBoundaries` 和构造循环，并在独立的 `pd_sim_test.rs` 或现有 `parity_test.rs` 增加正常与拒绝路径；不要把测试嵌入 `pd_sim.rs`。
- 增加新的 PD/streamhelper 接口时，状态应放在 `PDSimState`，核心转换放在本文件，trait/RPC 形状放在 `pd_sim_service.rs`。同时核对 Go 的 `pd_sim.go`/`pd_sim_service.go`，避免只实现简化桩而丢失错误或生命周期语义。
- 修改 checkpoint 等待规则时必须一起审查 `checkpoint_gen`、上传通知、clear 行为和 context 取消。若要求“值必须严格推进”，要注意当前 Go 与 Rust 都会因一次相等值上传唤醒既有 waiter。
- 修改 Scatter 时应保持“确定性 RNG、仅在真实迁移时 bump epoch、迁移后 checkpoint 回到全局值、返回 ID 排序”这些可复现性约束，并增加多 store 独立测试。
- 修改 `flushStore` 的取消实现时，应证明调用前取消、调用中取消、正常完成和底层错误四条路径都回收桥接资源。当前回归入口是 `pd_sim_test.rs::flush_store_observes_cancellation_after_the_call_starts`。
- 性能风险主要来自大布局下 `Scatter` 长时间持锁，以及每次 flush 新建轮询线程；若优化，不能牺牲 Go 对齐的取消与同步行为。兼容风险集中在错误字符串、默认任务名/起始 TS、Region epoch/checkpoint 更新顺序及 `pd_sim_service.rs` 的 trait 接线。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、7,032 个 Rust 文件；目标目录和 `pd_sim.rs` 均在索引内。`node --file br/pkg/utiltest/crr/pd_sim.rs` 核验了 391 行完整源码和文件级使用者。
- RustCodeGraph 精确符号/调用证据：查询确认 `PDSim`、`PDSimState`、`NewPDSimWithTestContext`、`flushStore`、`wait_global_checkpoint_advance`；`explore` 给出 `NewPDSimWithTestContext` 被 `harness.rs`、`parity_test.rs`、`pd_sim_test.rs`、`pd_sim_service_test.rs` 调用，`flushStore` 被 `pd_sim_test.rs` 调用，checkpoint 内部方法被 `pd_sim_service.rs` 调用。单独的 `callers/callees` 对这些 impl 内函数未返回边，因此未把缺失边当作“无调用”。
- 已读取生产与边界文件：`br/pkg/utiltest/crr/pd_sim.rs`、`Cargo.toml`、`lib.rs`、`pd_sim_service.rs`、`harness.rs`、`flush_sim.rs`。
- 已读取 Go 对照：`br/pkg/utiltest/crr/pd_sim.go`、`pd_sim_service.go`。
- 已读取独立 Rust 测试：`pd_sim_test.rs`、`pd_sim_service_test.rs`、`parity_test.rs`。当前目录不存在独立的 `pd_sim_test.go`；Go 语义以同路径生产文件及现有 CRR/streamhelper 测试引用为依据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令检查目标存在且固定二级标题恰好为 11，并人工复核文档没有把锁解析、split/merge、真实 RPC 或实时 Pause 事件写成已支持能力。
