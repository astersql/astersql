# `br/pkg/utiltest/crr/pd_sim_service.rs`

## 文件定位

本文件是 `astersql-br-pkg-utiltest-crr` 测试夹具 crate 中的服务适配层。crate 入口 `br/pkg/utiltest/crr/lib.rs` 通过 `#[path = "pd_sim_service.rs"] pub mod pd_sim_service` 将它纳入编译；`br/pkg/utiltest/crr/Cargo.toml` 则表明该 crate 是对应 Go 包 `br/pkg/utiltest/crr` 的 library，并直接依赖 `streamhelper`、`streamhelper/config` 与 `utiltest/fakecluster`。它不创建 `PDSim` 的状态，而是为 `br/pkg/utiltest/crr/pd_sim.rs` 定义的 `PDSim` 补齐 checkpoint advancer 所需的 streamhelper trait 和少量 Go 兼容方法。

这是一条测试基础设施路径，不是真实 PD、TiKV 或 gRPC 服务。`br/pkg/utiltest/crr/harness.rs` 构造 `PDSim` 后，把 `pd.clone()` 交给 `NewCommandCheckpointAdvancer`，并以 `&dyn StreamMeta` 调用 `Begin` 注册初始任务。因此本文件存在的直接目的，是让内存中的 fakecluster/PDSim 能以生产 streamhelper 接口参与 CRR（跨区域复制）测试线束。

## 核心职责

1. 通过 `impl TiKVClusterMeta for PDSim` 暴露 region 扫描、store 枚举、GC safepoint 控制和当前 TSO 查询。
2. 通过私有 `StoreClientAdapter` 把 `Arc<fakecluster::Store>` 转为对象安全的 `Arc<dyn LogBackupClient>`，并在 fakecluster 与 streamhelper 的请求、响应、错误和 flush 事件类型之间转换。
3. 通过 `impl LogBackupService` 和 `impl LogBackupFlushIntervalGetter` 提供按 store 获取客户端、清缓存及测试用 flush 间隔。
4. 通过 `impl StreamMeta` 把 `PDSim` 的单任务状态暴露为任务发现与全局 checkpoint API；真正的任务名/checkpoint 状态机仍在 `pd_sim.rs`。
5. 明确封闭 DRR harness 不支持的锁扫描、锁解析和真实 `tikv.Storage` 路径，避免测试误把桩当成完整实现。

文件不负责 region 布局创建、checkpoint 状态加锁或 Condvar 等待；这些由 `NewPDSimWithTestContext`、`upload_v3_global_checkpoint`、`wait_global_checkpoint_advance` 等 `pd_sim.rs` 符号实现。

## 主要符号

- `impl TiKVClusterMeta for PDSim`
  - `RegionScan(key, endKey, limit)`：使用后台 fakecluster context 扫描 region；负数 `limit` 被夹到 `0` 后转为 `usize`。
  - `Stores()`、`BlockGCUntil(at)`、`FetchCurrentTS()`：薄封装 fakecluster 同名能力，并把底层错误转成字符串。
  - `UnblockGC()`：底层解除 GC 阻塞成功后，再以 `Ordering::SeqCst` 把 `ServiceGCSafePoint` 清零。
- `StoreClientAdapter { store: Arc<fakecluster::Store> }`：私有适配对象，生命周期由返回的 trait object 持有。
- `impl LogBackupClient for StoreClientAdapter`
  - `GetLastFlushTSOfRegion(req)`：逐项转换 `RegionIdentity`，调用 store，再构造 streamhelper `RegionCheckpoint`。fakecluster 错误消息恰为 `"epoch not match"` 时设置 `EpochNotMatch`，恰为 `"not found"` 时设置 `NotLeader`。
  - `SubscribeFlushEvents()`：订阅 fakecluster 流，启动转发线程，解码 key 后经 `mpsc::Receiver<Vec<FlushEvent>>` 输出批次。
- `decode_tikv_key(encoded)`：私有 TiDB 升序 memcomparable bytes 解码器；以 8 字节数据组加 1 字节 marker 解析，校验长度、marker 和零填充。
- `impl LogBackupService for PDSim`：`GetLogBackupClient(storeID)` 返回适配器；`ClearCache(storeID)` 透传清理。
- `impl LogBackupFlushIntervalGetter for PDSim`：`GetLogBackupFlushInterval()` 返回 `DefaultCommandConfig().GetResolveLockInterval()`，这是测试占位而非从 store 动态读取的 flush 周期。
- `impl StreamMeta for PDSim`
  - `Begin(ch)`：向调用方提供的向量追加一个 `EventAdd`，包含当前任务名、起始 TSO 和一个默认空 `KeyRange`（表示全范围）。
  - `UploadV3GlobalCheckpointForTask`、`GetGlobalCheckpointForTask`、`ClearV3GlobalCheckpointForTask`：分别委托 `pd_sim.rs` 的上传、读取和清空方法。
  - `PauseTask(taskName)`：只校验任务名；当前 Rust 环境没有实时任务通道，成功时不会实际向已启动的 advancer 推送暂停事件。
- `impl RegionLockResolver for PDSim::ResolveLocksForRange`：固定返回 unsupported。
- `impl PDSim` 的 Go 兼容入口：`WaitGlobalCheckpointAdvance` 委托内部等待状态机；`Identifier` 固定返回 `"drr-pd-sim"`；`GetStore` 固定 panic；两个单 region 锁方法固定返回错误。

## 执行流程

初始化与任务注册流程如下：

1. `harness.rs::newLocalTestHarness` 调用 `NewPDSimWithTestContext` 创建带 fakecluster、任务状态、`Mutex` 和 `Condvar` 的 `Arc<PDSim>`。
2. `NewCommandCheckpointAdvancer(pd.clone())` 利用本文件为 `PDSim` 实现的元数据、日志备份和任务接口组装 advancer。
3. `harness.rs::start_task_listener` 以 `&dyn StreamMeta` 调用 `Begin`。`Begin` 分别读取 `task_name()` 和 `task_start()`，生成唯一的 `EventAdd`；listener 再以 `SetTask(info, ranges)` 同步注册任务。
4. advancer 扫描 region/store、获取各 store 的 `LogBackupClient`，并查询 checkpoint 或订阅 flush 事件。相关调用经本文件转换后落到 fakecluster。
5. advancer 上传全局 checkpoint 时，`UploadV3GlobalCheckpointForTask` 转入 `pd_sim.rs::upload_v3_global_checkpoint`：校验任务名、拒绝回滚、更新全局值与 generation，然后唤醒 Condvar 等待者。

flush 订阅的单批处理流程是：`SubscribeFlushEvents` 创建可取消的 fakecluster context并取得 stream，创建 Rust `mpsc` 通道，再由后台线程循环 `Recv`。每个底层事件的起止 key 都先经过 `decode_tikv_key`；任一 key 解码失败会用 `filter_map` 丢弃该事件，其余事件组成一个向量发送。底层流结束或接收端消失后循环退出，最后调用 `cancel.cancel()`。

`GetLastFlushTSOfRegion` 则是同步的一问一答：请求 identity 浅转换、调用 fakecluster store、逐 checkpoint 还原 region（缺失 region 时用默认值）、映射 checkpoint 数值与两类可重试 region 错误。

## 数据与状态

本文件自身唯一持久字段是 `StoreClientAdapter::store: Arc<fakecluster::Store>`。任务名、任务起始 TSO、全局 checkpoint、checkpoint generation 与随机数生成器都保存在 `pd_sim.rs::PDSimState` 中，并由 `PDSim::state: Mutex<PDSimState>` 保护；本文件只经 `task_name()`、`task_start()` 及三个内部 checkpoint 方法访问它们。

`Begin` 每次调用都根据当前状态构造新事件，不缓存订阅者。一个默认 `KeyRange` 的空起止 key 表示全键空间。`UnblockGC` 在 fakecluster 操作成功后显式清空原子 `ServiceGCSafePoint`，形成“底层先成功、本地镜像后清零”的顺序。

flush 订阅有两层临时状态：fakecluster 的可取消 stream context，以及标准库无界 `mpsc` 通道。每次底层响应对应一次 `send(Vec<FlushEvent>)`；即使该响应内所有事件因 key 非法而被过滤，也会发送空向量。`decode_tikv_key` 对空输入直接返回空 key；非空输入必须由完整的 9 字节组构成，末组必须以非零 padding 数结束，否则没有正常终止路径。

## 依赖与调用关系

上游直接证据：

- RustCodeGraph 的文件节点显示本文件被 `br/pkg/utiltest/crr/harness.rs`、`br/pkg/utiltest/crr/pd_sim.rs` 和 `br/pkg/utiltest/crr/pd_sim_service_test.rs` 引用。
- `harness.rs::newLocalTestHarness` 创建 `PDSim` 并调用 `NewCommandCheckpointAdvancer(pd.clone())`；`start_task_listener` 经 `StreamMeta::Begin` 消费本文件生成的 `TaskEvent`。
- `pd_sim_service_test.rs` 直接调用 `GetLogBackupClient`，继而覆盖 `GetLastFlushTSOfRegion` 与 `SubscribeFlushEvents`。

下游依赖：

- `astersql_br_pkg_streamhelper::regioniter` 提供 `TiKVClusterMeta`、`RegionWithLeader` 与 `Store`。
- `astersql_br_pkg_streamhelper` 提供 `LogBackupClient`、`LogBackupService`、`StreamMeta`、`RegionLockResolver` 以及相关请求/响应/事件数据结构。
- `astersql_br_pkg_streamhelper_config` 提供默认命令配置。
- `astersql_br_pkg_utiltest_fakecluster` 提供真实的内存 region/store/TSO/GC 和 flush stream 行为。
- `crate::pd_sim::PDSim` 提供被适配对象及 checkpoint 状态机；`crate::stubs` 提供本地 `Context`、`Error`、`Result`。

RustCodeGraph 对 `decode_tikv_key` 给出唯一符号 `pd_sim_service.rs::decode_tikv_key`，其源码调用点位于 `SubscribeFlushEvents`。针对部分 trait 方法的 `callers/callees` 查询没有返回可用静态边，因此这些动态派发关系以 trait impl 源码、`harness.rs` 的构造/调用点和独立测试的直接调用为准，不把缺失的图边解释为“没有调用”。

## 错误处理与边界

- fakecluster 的大多数错误统一 `to_string()`；跨层后不再保留结构化错误类型。
- `RegionScan` 特意将负 `limit` 归零，避免有符号转无符号造成极大扫描上限或底层异常。零值的具体扫描语义由 fakecluster 决定。
- `GetLastFlushTSOfRegion` 的 region 错误分类依赖精确字符串匹配；新增底层错误文案时不会自动归入现有布尔类别。独立测试 `log_backup_client_preserves_region_error_kind` 证明 epoch 不匹配被保留为 `EpochNotMatch=true`、`NotLeader=false`。
- `decode_tikv_key` 拒绝不足一组、marker 下溢、padding 超过 8 或 padding 字节非零的数据。订阅转换对解码错误选择丢弃单个事件而非终止整个流，也不会把错误传给 receiver。
- `SubscribeFlushEvents` 创建底层 stream 失败时立即返回字符串错误；后台阶段的 `Recv` 错误仅结束线程。标准 `mpsc::Receiver` 只能通过通道断开观察结束原因，不能得到原始 stream 错误。
- `Begin` 当前不会失败，但接口保留 `Result`。`PauseTask` 对未知任务返回 `unknown task`，对已知任务只返回成功，并不保证 advancer 已暂停。
- 上传 checkpoint 拒绝未知任务和数值回滚；查询/清空也拒绝未知任务。等待接口对任务名不匹配和 context 取消返回本地 `Error`。
- `GetStore` 是有意 panic 的硬边界；`ResolveLocksForRange`、`ScanLocksInOneRegion`、`ResolveLocksInOneRegion` 是可返回错误的软边界。调用方不得把此 harness 用于需要真实锁解析的测试。

## 并发与资源生命周期

`PDSim` 通常由 `Arc` 共享，内部 checkpoint 状态的并发规则定义在 `pd_sim.rs`：上传持有 `Mutex` 更新 checkpoint/generation 后 `notify_all`；等待通过 `Condvar::wait_timeout` 周期性复查 generation、checkpoint 和 context 取消。公开 `WaitGlobalCheckpointAdvance` 只是委托，不额外加锁。

每次 `SubscribeFlushEvents` 启动一个未保存 `JoinHandle` 的独立线程。线程拥有 fakecluster stream、取消句柄和发送端；调用方拥有 receiver。生命周期结束条件包括底层 `Recv` 失败/结束，或 receiver 被丢弃导致发送失败。循环退出后总会调用 `cancel.cancel()`；当前 API 没有显式 join，调用方也不能直接取消仍有数据但暂时闲置的订阅，只有丢弃 receiver 使下一次发送失败或等待底层流结束。

`Arc<fakecluster::Store>` 保证 adapter/订阅使用期间 store 对象仍存活。`UnblockGC` 使用 `SeqCst` 原子写保证清零在各线程间以最强顺序可见。`Begin` 使用普通 `Vec` 同步交付事件，不创建任务监听线程；所以 Go 版本运行期经 `taskCh` 推送 Pause 的生命周期在 Rust 侧不存在。

## 与 Go 版本的对应关系

主要对照文件是 `br/pkg/utiltest/crr/pd_sim_service.go`，而 Go 的 `Stores`、`BlockGCUntil`、`UnblockGC`、`FetchCurrentTS` 位于相邻 `pd_sim.go`。Rust 将这些能力集中为 `TiKVClusterMeta` trait impl；Go 则依靠 `PDSim`/嵌入的 Cluster 方法满足接口。

语义保持部分：默认 flush 间隔取 `DefaultCommandConfig().GetResolveLockInterval()`；`Begin` 产生单个全范围 `EventAdd`；上传、读取、清空和等待全局 checkpoint 的任务校验及回滚限制一致；标识符均为 `drr-pd-sim`；真实 store 获取会 panic；两种单 region 锁操作明确不支持。Rust 的等待实现用 `Mutex + Condvar + generation` 替代 Go waiter channel，但成功上传（包括等值上传）都会唤醒等待方，保留 Go 关闭 waiter 的效果。

已确认差异：

- Go `Begin` 保存 `taskCh` 并向 channel 发送，Rust `Begin` 只向调用方的 `Vec` 同步追加事件。
- Go `PauseTask` 在保存过的 `taskCh` 非空时发送 `EventPause`；Rust 没有实时通道，只校验任务名后成功。因此 `harness.rs::start_task_listener` 虽能识别 Pause/Resume，当前本文件不会在启动后产生这些事件。
- Go 直接返回生成的 `logbackup.LogBackupClient`；Rust fakecluster 类型与 streamhelper 类型不同，必须经 `StoreClientAdapter` 转换。Rust 额外包含 memcomparable key 解码和后台 mpsc 转发逻辑。
- Go 方法普遍接收调用方 `context.Context`；对应 Rust trait 签名没有 context 参数时，本文件使用 `FcContext::background()`，所以这些同步调用不能继承上游取消信号。
- Rust 的锁扫描兼容方法只返回 `Result<()>`，没有复刻 Go 的 locks/location 返回值；这与该路径固定 unsupported 的当前事实一致，但不能作为真实接口替代品。

## 扩展指南

- 新增或变更 streamhelper trait 能力时，优先在相应 `impl` 中做最薄的类型适配，把任务/checkpoint 状态规则继续留在 `pd_sim.rs`，把 store/region 行为留在 fakecluster；不要在三个层次重复状态。
- 增加 region checkpoint 错误种类时，应修改 `GetLastFlushTSOfRegion` 的结构化映射并扩展独立的 `pd_sim_service_test.rs`，避免继续依赖未经测试的错误字符串。兼容风险是 advancer 的重试分类发生变化。
- 修改 key 编码支持时，应围绕 `decode_tikv_key` 增加独立测试文件中的正常多组、空 key、非法长度、非法 marker、错误 padding 和无终止组用例；不要把测试内嵌进生产 `.rs`。错误处置若由“丢弃事件”改为“终止/上报”，需要同步评估订阅消费者行为。
- 若要支持真正的暂停/恢复，需同时设计可持续的任务事件通道、`Begin` 注册生命周期与 `PauseTask` 推送，更新 `harness.rs::start_task_listener` 的异步消费方式，并补并发测试；仅让 `PauseTask` 改返回值不足以实现语义。
- 若要支持锁解析，不能只移除错误或 panic；必须提供真实 store、扫描返回数据、region 重定位与取消/重试语义，并在独立测试中覆盖 leader/epoch 变化。当前 DRR harness 明确不应扩成生产 TiKV 客户端。
- 调整订阅线程时，应明确停止协议和 join 策略，避免长期无事件时仅丢 receiver 仍不能立即停止线程。性能关注点是每个订阅一个 OS 线程、无界 mpsc 和每批 key 解码分配。
- 修改 `UnblockGC` 时必须保持“底层成功后才清本地 safepoint”的失败原子性；修改 checkpoint 方法时必须保持任务绑定和单调不回滚不变量。

## 验证依据

已核对的直接材料：

- 目标源码：`br/pkg/utiltest/crr/pd_sim_service.rs`（RustCodeGraph file node，1–329 行）。
- 状态实现：`br/pkg/utiltest/crr/pd_sim.rs`，尤其是 `PDSimState`、`PDSim`、`upload_v3_global_checkpoint`、`get_global_checkpoint`、`clear_v3_global_checkpoint`、`wait_global_checkpoint_advance`。
- 上游接线：`br/pkg/utiltest/crr/harness.rs` 的 `newLocalTestHarness`、`start_task_listener`、`Tick` 和 `UploadGlobalCheckpoint`。
- crate 边界：`br/pkg/utiltest/crr/lib.rs` 与 `br/pkg/utiltest/crr/Cargo.toml`；未发现该目标包的 `doc.go`。
- Go 对照：`br/pkg/utiltest/crr/pd_sim_service.go` 全文，以及 `br/pkg/utiltest/crr/pd_sim.go` 中 `Stores`、`BlockGCUntil`、`UnblockGC`、`FetchCurrentTS`。
- 独立 Rust 测试：`br/pkg/utiltest/crr/pd_sim_service_test.rs` 的 `log_backup_client_preserves_region_error_kind` 与 `log_backup_client_forwards_flush_subscription`；后者证明一次 `store.Flush()` 会转发单个空范围事件，checkpoint 等于任务起始 TSO。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utiltest/crr` 包含目标、Go 对照、模块入口及测试；`query decode_tikv_key --json` 返回唯一目标符号；`query SubscribeFlushEvents --json` 和 `query WaitGlobalCheckpointAdvance --json` 用文件限定消除了同名候选。精确 `callers/callees` 未产生可用输出，故动态 trait 边用上述源码接线与直接测试交叉验证。

本任务是纯文档分析，未运行 Cargo。交付前另以任务指定命令校验本文恰含十一个固定二级标题，并人工复核没有把 Go 的实时 Pause、真实锁解析或生产 gRPC 能力误写成 Rust 当前已支持。
