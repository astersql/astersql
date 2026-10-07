# `br/pkg/utiltest/fakecluster/core.rs`

## 文件定位

[`core.rs`](core.rs) 是 `astersql-br-pkg-utiltest-fakecluster` library crate 的核心实现文件。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod core` 加载它并扁平再导出其公开符号；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 指向 `br/pkg/utiltest/fakecluster`，表明它是同目录 Go 测试夹具的 Rust 移植，而不是真实 TiKV、PD 或 gRPC 服务。

它位于 BR 测试支撑链上：上层 [`../crr/pd_sim_service.rs`](../crr/pd_sim_service.rs) 把 `Cluster` 适配成 streamhelper 的 `TiKVClusterMeta`、`LogBackupService`，并把 `Store` 包装成 `LogBackupClient`；[`../crr/pd_sim.rs`](../crr/pd_sim.rs) 的 `flushStore` 直接调用 `Cluster::ApplyCheckpointToStore`。因此本文件存在的目的，是在内存中提供可控的 store/region 拓扑、checkpoint、flush 订阅、TSO 与 GC safepoint，使 CRR/streamhelper 测试不依赖真实集群。

该文件没有条件编译项。测试由 [`lib.rs`](lib.rs) 中的 `#[cfg(test)]` 分别装入 [`core_test.rs`](core_test.rs) 和 [`parity_test.rs`](parity_test.rs)，测试逻辑没有内嵌在生产源文件中。

## 核心职责

- 建模集群层级：`Cluster` 保存全部 `Store`、排序后的 region 视图、ID/TSO 分配器及 GC safepoint；`Store` 保存本节点可见的 region 和 flush 客户端状态；`Region` 保存半开键区间、leader、epoch、checkpoint、锁与 flush 状态。
- 模拟拓扑变更：`SplitAt`、`SplitAndScatter`、`TransferRegionTo`、`RemoveStore`、`SetRegionLeader` 修改共享拓扑，并保持 Go 夹具中“多个 store 指向同一个 region 对象”的语义。
- 提供 streamhelper 所需查询：`RegionScan`、`Stores`、`GetLogBackupClient`、`GetLastFlushTSOfRegion` 暴露 region/store/checkpoint 视图和可注入错误。
- 模拟 flush 协议：`FlushSimulator` 判断未 flush 或 epoch 不匹配；`Store::SubscribeFlushEvent` 和 `trivialFlushStream::Recv` 构造带取消能力的内存订阅；`FlushExcept`、`ApplyCheckpointToStore` 产生编码后的 flush 事件。
- 模拟时间与保护状态：`AllocTSO`、`AdvanceClusterTimeBy`、`AdvanceCheckpoints` 和 `BlockGCUntil`/`UnblockGC` 支撑 checkpoint advancer 与 GC 保护测试。
- 保留测试可观测性：hook、快照方法、稳定排序和 `Display` 输出让测试能观察内部状态并精确注入失败。

## 主要符号

- `FlushSimulator { FlushedEpoch, Enabled }`：`makeError(requestedEpoch)` 在未启用时放行；启用后依次返回 `not flushed`、`flushed epoch not match` 或成功。`fork()` 只继承 `Enabled`，把已 flush epoch 重置为零。
- `Region` 与 `NewRegion(...) -> Arc<Region>`：region 是跨 store 共享的 `Arc`。`Region::SplitAt` 把原区间截成 `[start,key)` 与 `[key,end)`，两侧 epoch 均为旧值加一、checkpoint 和 leader 继承、flush 状态清零；`Flush` 将 `FlushedEpoch` 设为当前 epoch。
- `trivialFlushStream`：持有 `Receiver<SubscribeFlushEventResponse>` 与 `Context`。`Recv` 返回事件、通道关闭时的 `EOF`，或取消状态；其余 gRPC stream 形态方法是最小测试替身。
- `Store`：`RegionMap` 是该 store 的 peer 视图；私有 `StoreClientState` 管理订阅能力、bootstrap 代数和有界订阅通道。关键方法为 `SubscribeFlushEvent`、`GetLastFlushTSOfRegion`、`FlushExcept`、`FlushNow`。
- `RegionState` 与 `stateFromRegion`：把共享可变 `Region` 复制成只读快照，供断言及 `ApplyCheckpointToStore` 返回。
- `Cluster`：总控对象。构造入口是 `New` 和 `NewBasicCluster`；查询入口包括 `RegionList`、`RegionScan`、`Stores`、`RegionSnapshot(s)`；变更入口包括 `AddRegion`、`SplitAt`、`SplitAndScatter`、checkpoint/leader 更新和 store 移除；协议入口包括 `GetLogBackupClient`、GC/TSO 方法和 `NewTaskEvent`。
- 私有 `hex::encode`：仅服务 `Display for Region`，避免为调试输出再引入依赖。

所有公开 API 都沿用 Go 风格的大写命名；crate 根在 [`lib.rs`](lib.rs) 中允许相应 lint。这是移植兼容选择，不应被解释为一般 Rust API 风格。

## 执行流程

1. `NewBasicCluster(n, simEnabled)` 先以 `AllocID` 创建 `n` 个 store，再创建一个覆盖全键空间的初始 region；初始 leader 是第一个 store，region peer 最多放入前三个 store。调用方必须保证 `n > 0`，否则访问 `stores[0]` 会 panic。
2. 上层通过 `PDSim::RegionScan` 调用 `Cluster::RegionScan`。它先按 `StartKey` 排序 region，再用 spans crate 的 `Overlaps` 过滤查询区间，按 `limit` 组装 `RegionWithLeader`；遇到起点已越过查询起点的非匹配 region 时提前结束。
3. 查询旧式 checkpoint 时，`PDSim` 取得 `Store` 并调用 `GetLastFlushTSOfRegion`。该方法先检查 legacy 开关和 hook，再逐个请求验证“region 存在且本 store 是 leader”、flush epoch、请求 epoch，最后返回 checkpoint 或逐 region 错误。
4. 订阅路径先以 `SetSupportFlushSub(true)` 提升 bootstrap 代数并打开能力，再由 `SubscribeFlushEvent` 建立容量 1024 的同步通道，登记 subscriber，并启动取消监听线程。`FlushExcept` flush 本 store 领导且未被排除键覆盖的 region，将边界编码成 TiKV memcomparable key 后非阻塞广播。
5. `PDSim::flushStore` 调用 `ApplyCheckpointToStore`：先解析并排序 store 上的 region ID；先整体校验新 checkpoint 严格大于每个当前值，避免校验失败时部分写入；然后更新所有 checkpoint、必要时抬高 `CurrentTS`、构造快照与事件，最后逐订阅者发送。通道满时循环等待，但 `Context` 取消可终止等待并返回错误。
6. 拓扑变更时，`Cluster::SplitAt` 找到包含 key 的 region、分配新 ID、调用 `Region::SplitAt`，再把新 region 加到持有旧 peer 的每个 store。`SplitAndScatter` 在集群粗锁下完成全部分裂，为每个 region 随机选三个 store、迁移 peer 并随机选择 leader。
7. checkpoint 推进会增加每个 region 的值并把 `FlushedEpoch` 清零，随后必须再次 flush 才能通过 flush simulator。TSO 分配则把当前物理时间加一并把逻辑位清零。

## 数据与状态

- 键范围遵循 `[StartKey, EndKey)`；空 `EndKey` 表示正无穷。`FindRegionByKey` 依赖全部 region 构成连续键空间，否则 panic。
- `Arc<Region>` 是拓扑共享单位。同一 region 在多个 `Store::RegionMap` 中不是副本，leader、epoch、checkpoint 或范围变化对所有 peer 视图立即可见。
- 高频标量以 `AtomicU64` 保存，并统一使用 `Ordering::SeqCst`：包括 region leader/epoch/checkpoint、flush epoch、ID 高水位、TSO 以及 GC 标志。布尔状态用 `0/1` 原子值表达。
- 复合状态由 `Mutex` 保护：region 范围和锁列表、store region map、subscriber 表、cluster store/region 集合和 hook。`Cluster::mu` 额外串行化 TSO、GC 与 split/scatter/remove-store 等复合操作。
- `StoreClientState::bootstrap_at` 每次 `SetSupportFlushSub` 都递增，不论新值与旧值是否相同；上层 `Stores` 将其作为 `BootAt` 暴露，用于检测 store 客户端能力代数变化。
- `MaxTS` 当前只作为公开状态字段存在，本文件没有读写它；实际时间路径使用私有 `CurrentTS`。这应视为当前实现事实，而非已接通功能。
- `RegionState::StoreID` 实际填入当前 leader ID，而不是生成快照的 store 参数；该命名和行为与 Go 对照保持一致。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 确定：只直接依赖 `astersql-br-pkg-streamhelper`、其 `spans` 子 crate、`rand` 和 `tracing`。真实 proto、oracle、codec 和 context 没有作为外部重依赖接入，而由同 crate 的 [`stubs.rs`](stubs.rs) 提供；这符合 Cargo 注释中为 arm64 Darwin 保持轻量移植的说明。

上游直接接线如下：

- [`../crr/pd_sim_service.rs`](../crr/pd_sim_service.rs) 的 `impl TiKVClusterMeta for PDSim` 委托 `RegionScan`、`Stores`、`BlockGCUntil`、`UnblockGC`、`FetchCurrentTS`。
- 同文件的 `impl LogBackupService for PDSim` 委托 `GetLogBackupClient` 和 `ClearCache`；`StoreClientAdapter` 再委托 `GetLastFlushTSOfRegion` 与 `SubscribeFlushEvent`，并把 fakecluster 事件转换成 streamhelper 类型。
- [`../crr/pd_sim.rs`](../crr/pd_sim.rs) 的 `flushStore` 建立取消桥接后调用 `ApplyCheckpointToStore`，把返回的 `RegionState` 转为 CRR 测试状态。

主要下游关系为：`Cluster` 调用 `Store`/`Region`；`Store` 调用 `FlushSimulator`、`codec::EncodeBytes` 和 `spans::CompareBytesExt`；`Cluster::RegionScan` 调用 `spans::Overlaps` 并构造 streamhelper 类型；时间方法调用 stub `oracle`；scatter 使用 `rand`；调试和推进记录使用 `tracing`。

RustCodeGraph 的文件级索引显示 `core.rs` 有 99 个符号，并给出 64 个使用该文件的索引关系；精确 `query` 能同时定位 Rust/Go 的 `NewBasicCluster` 和 `ApplyCheckpointToStore`。索引未为部分 impl method 建立可用的 callers/callees 输出，因此上述接线以直接适配器源码和独立测试补证，而没有把缺失图边猜成不存在调用。

## 错误处理与边界

- `GetLastFlushTSOfRegion` 的 RPC 级失败包括 legacy RPC 被禁用和 hook 错误；单 region 失败保存在响应内，顺序为 `not found`、flush simulator 错误、`epoch not match`。这个顺序会影响相同请求同时触发多个条件时观察到的错误。
- `SubscribeFlushEvent` 在能力未启用时返回 `Code::Unimplemented`/`meow?`；`trivialFlushStream::Recv` 在取消后先尝试取出一个已排队事件，没有事件才返回 `Canceled`，通道断开返回文本 `EOF`。
- `emitFlushEvents` 对满通道直接丢弃事件，确保常规 flush 不被慢订阅者阻塞；`ApplyCheckpointToStore` 则模拟 Go 的阻塞发送，并通过 context 取消退出。两条发送路径的背压语义不同，扩展时不能互换。
- `ApplyCheckpointToStore` 拒绝不存在的 store，且要求新 checkpoint 严格大于该 store 上每个 region 的当前值。它在任何写入前完成全量校验，因此这一类错误不会产生部分 checkpoint 更新。
- `BlockGCUntil` 禁止 safepoint 回退；`GetLogBackupClient`、`RegionSnapshotsOnStore` 对未知 store 返回错误。
- 为对齐 Go 的失败方式，若 `NewBasicCluster(0, ...)`、`chooseStores(n)` 的 store 数不足、`shuffleLeader` 没有 peer、`FindRegionByKey` 遇到键空间缺口、`RemoveStore`/`UpdateRegion` 使用未知 ID，Rust 会 panic。不能为了“更安全”静默忽略，否则会掩盖测试夹具错误；[`core_test.rs`](core_test.rs) 明确锁定未知 store/region 的响亮失败。
- 所有 `Mutex::lock()` 都直接 `unwrap()`；持锁线程 panic 后的 poison 会继续触发 panic。该文件是测试夹具，没有恢复 poisoned state 的策略。

## 并发与资源生命周期

`Cluster`、`Store` 和 `Region` 通过 `Arc`、`Mutex` 与原子字段支持测试线程共享。原子字段使用最强的顺序一致性，简化夹具可观测性；复合修改仍必须依赖相应 mutex，不能把单字段原子性误当成拓扑事务。

每次成功订阅都会启动一个后台线程等待其 `Context` 取消；取消后线程从 `subscribers` 删除对应发送端。接收端 `trivialFlushStream` 自身被丢弃并不会主动取消 context，所以正常调用者应持有并触发 cancel；[`parity_test.rs`](parity_test.rs) 轮询确认取消后 `subscriber_count()` 归零。

订阅使用容量 1024 的 `sync_channel`。普通 flush 在持有 client mutex 时 `try_send`，满则丢弃；checkpoint 应用先复制发送端、释放 client mutex，再逐个重试，以免等待期间阻止订阅注册/删除。其循环每 1ms 检查 context；`Recv` 每 10ms 检查取消。后台线程没有 join 句柄，生命周期由取消和 `Arc<Store>` 捕获决定。

需注意锁顺序。现有路径通常从 cluster 集合锁进入 store region map，再短暂进入 region 字段锁；`ApplyCheckpointToStore` 在发送前释放所有 region/client 锁。新增代码应避免在持有 subscriber/client mutex 时进行阻塞发送，也应避免在 region map 锁内回调用户 hook。

## 与 Go 版本的对应关系

直接对照文件是 [`core.go`](core.go)。公开类型和方法集合基本逐项对应：`FlushSimulator`、`Region`、`trivialFlushStream`、`Store`、`RegionState`、`Cluster` 以及构造、扫描、拓扑、checkpoint、flush、TSO、GC 和 task event 方法均保留 Go 名称和主要分支顺序。

Rust 为安全共享做了机械映射：Go 指针变为 `Arc`，map/slice 变为 `Mutex<HashMap/Vec>`，原子 checkpoint 扩展到更多并发标量；protobuf、gRPC、TiDB codec/oracle 类型由本地 stubs 模拟。`Store` 的 Go 公共字段 `SupportsSub`/`BootstrapAt` 在 Rust 被收进私有 `StoreClientState`，通过只读方法暴露；hook 通过 setter 注入。接口形态不同，但测试可观察语义保持一致。

以下 Go 边界被有意保留：分裂后两侧 epoch 同步增加且 flush 状态清零；初始 region 最多复制到前三个 store；scatter 固定选三个 store，数量不足时 panic；region scan 按起点排序并提前结束；GC safepoint 不回退；取消订阅移除 subscriber；unknown store/region 在 Go 会解引用 nil 的路径，Rust 以明确 `expect` 达到同样的失败效果。

Rust 侧当前存在适配边界而非真实生产能力：gRPC metadata/message 方法是空实现，错误是本地 `Error`/`StatusError`，锁解析等更高层能力不在本文件实现。不能仅凭方法名宣称它已兼容真实 TiKV RPC。

## 扩展指南

- 新增集群元数据能力时，优先在 `Cluster` 增加最小状态和操作，再在 [`../crr/pd_sim_service.rs`](../crr/pd_sim_service.rs) 的相应 trait impl 接线；如果需要协议类型，同步扩展 [`stubs.rs`](stubs.rs)，不要把真实重依赖直接引入轻量 crate 而不检查 Cargo 约束。
- 修改 region 拓扑时，必须同时维护 `Cluster::Regions` 和各 `Store::RegionMap` 的共享 `Arc<Region>` 关系，并审查排序、leader、peer、epoch、checkpoint 与 `FlushedEpoch` 不变量。分裂行为应同步对照 `Region::SplitAt`、`Cluster::SplitAt` 和 `SplitAndScatter`。
- 修改 flush 协议时，分别审查非阻塞 `emitFlushEvents` 与可取消阻塞的 `ApplyCheckpointToStore`，并保持键边界使用 `codec::EncodeBytes`。订阅资源改动需覆盖注册、context 取消、channel 断开和满缓冲。
- 修改 checkpoint 校验时，保留“先验证全部 region、后统一写入”的原子外观，并检查 `CurrentTS` 只前进不回退。修改 TSO/GC 时必须在 `Cluster::mu` 范围内维护跨字段不变量。
- 新增或改变行为时同步更新独立测试文件，而不是在 `core.rs` 内嵌测试：公共契约与 Go 对等场景放入 [`parity_test.rs`](parity_test.rs)，针对 panic/回归的聚焦用例放入 [`core_test.rs`](core_test.rs)。同时检查 Go [`core.go`](core.go) 的对应逻辑，避免 Rust 单方面简化。
- 兼容风险主要是上层 trait 适配、错误文案/分类及 Go 对等；性能风险主要是全局/集合 mutex、顺序一致性原子和每订阅一个线程。该 crate 面向测试，确定性和语义对齐优先于吞吐，但不应在持锁区加入不可控阻塞。

## 验证依据

- RustCodeGraph：`status` 确认当前索引包含 7032 个 Rust 文件；`files --filter br/pkg/utiltest/fakecluster` 确认 `core.rs`、`core_test.rs`、`parity_test.rs`、`lib.rs`、`stubs.rs` 与 Go 对照均被索引；`node --file br/pkg/utiltest/fakecluster/core.rs` 分段读取 1465 行实现；`query NewBasicCluster`、`query ApplyCheckpointToStore` 同时定位 Rust 与 Go 符号；对精确 callers/callees 的查询未产出可用 method 边，因此以直接源码引用补证。
- 源码与 crate：完整核对 [`core.rs`](core.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs) 与 [`stubs.rs`](stubs.rs) 的类型引用和边界；目标目录不存在 `doc.go`。
- Go 对照：读取 [`core.go`](core.go) 的全部类型/函数清单，并逐段核对建簇、region/store、扫描、flush 订阅、checkpoint、TSO、GC、拓扑和错误分支。
- 上游直接证据：读取 [`../crr/pd_sim_service.rs`](../crr/pd_sim_service.rs) 的 trait 适配与 Store client 包装，以及 [`../crr/pd_sim.rs`](../crr/pd_sim.rs) 的 `flushStore -> ApplyCheckpointToStore` 调用。
- 独立测试：[`core_test.rs`](core_test.rs) 验证未知 store/region 的 panic 对等；[`parity_test.rs`](parity_test.rs) 验证构造/分裂、checkpoint/flush、TSO、扫描边界、GC 回退、缺失对象、legacy/订阅错误、取消清理、事件广播，以及 store 少于三个时 scatter panic。
- 本任务是纯文档分析，按任务约束不运行 Cargo。交付前以任务文件规定的命令验证目标文档存在且恰有十一个固定二级章节，并人工复查重要陈述均可追溯到上述符号或路径。
