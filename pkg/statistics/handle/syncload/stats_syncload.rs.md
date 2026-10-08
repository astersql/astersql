# `pkg/statistics/handle/syncload/stats_syncload.rs`

## 文件定位

本文件实现按需同步加载列/索引统计信息的核心调度器。它位于 `astersql-statistics-handle-syncload` crate 中，由同目录 `lib.rs` 以 `pub use stats_syncload::*` 再导出；`Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `package.metadata.porting.go-package = "pkg/statistics/handle/syncload"` 表明它是 Go 同路径包的 Rust 移植边界。该 crate 当前没有声明外部 Cargo 依赖，通道、锁、原子量、超时与 panic 隔离均使用标准库实现。

应用中的直接接线不在本文件内，而在 `pkg/session/runtime/planning.rs`：`SessionStatsSyncLoadAdapter::new_with_domain` 创建并按 Domain 共享 `statsSyncLoad`，依据全局配置计算队列大小与 worker 数量，启动 `SubLoadWorker`；其 `StatsLoadWaiter::SyncWaitStatsLoad` 实现把语句上下文中的 `astersql_meta_model::StatsLoadItem` 转成本文类型，调用 `SendLoadRequests` 和 `SyncWaitStatsLoad`。规划器的 `pkg/planner/core/optimizer_runtime.rs::sync_wait_stats_load_point` 在发现待加载项后取得该 waiter，使代价估计在统计加载完成或按配置退化到 pseudo statistics 后继续。

## 核心职责

- 用 `SendLoadRequests` 先排除缓存已经满足的项，再为每个剩余项建立有界结果通道和 singleflight 分组；相同 `StatsLoadItem::Key` 的并发请求只让一个 leader 入队，其他请求作为 waiter 接收同一结果。
- 用 `SubLoadWorker`、`HandleOneTask` 和 `drainColTask` 消费高优先级 needed 队列；已经超过语句期限的任务转入低优先级 timeout 队列，仅在没有新任务时处理。
- 用 `handleOneItemTaskWithSCtx` 判定列/索引是否还需要加载，处理跳过类型和未 ANALYZE 列，再由 `readStatsForOneItem` 从 `StatsStorage` 读取元数据、桶、CMSketch 与 TopN。
- 用 `updateCachedItem` 串行合并表级缓存快照，避免较弱的元数据加载覆盖已经 FullLoad 的对象。
- 用 `SyncWaitStatsLoad` 在一个语句级总截止时间内汇总所有结果，区分请求投递/等待失败与 worker 已交付的加载错误，并维护加载、超时和去重计数。

本文件同时定义了移植所需的轻量数据模型和依赖 trait（如 `Histogram`、`TableStats`、`StatsStorage`、`StatsHandle`）。它们使同步加载算法可独立测试；真正的统计缓存和存储由 `pkg/session/runtime/planning.rs::DomainStatsSyncLoadHandle` 适配，而不是由本文件直接持有 TiDB/AsterSQL 的完整 Handle 类型。

## 主要符号

- `RetryCount = 1`：一次失败后至多再尝试一次；`isVaildForRetry` 先增加 `NeededItemTask::Retry` 再比较上限，名称保留 Go 原拼写。
- `GetSyncLoadConcurrencyByCPU() -> usize`：按可用并行度返回 5、6、8 或 10；Domain 适配器仅在配置值为 0 时使用它。
- `Error` / `Result<T>`：覆盖队列关闭或满、退出、直方图元数据缺失、加载错误、panic、锁中毒和超时。`Display` 给跨适配层传播提供稳定文本。
- `TableItemID`、`StatsLoadItem`：以表 ID、列/索引 ID、是否索引和是否 FullLoad 描述请求；`Key` 将四个维度都纳入 singleflight 键，因而元数据加载不会与全量加载错误合并。
- `Column`、`Index`、`TableStats`、`TableInfo`：同步加载器使用的缓存与 schema 快照。`ColumnIsLoadNeeded` 同时返回当前列、是否需要加载、是否已分析；索引对应 `IndexIsLoadNeeded`。
- `StatsStorage`：三段式高优先级读取接口，依次提供直方图元数据、完整桶以及 CMSketch/TopN。
- `StatsHandle`：读取表缓存/schema、提交新缓存、取得存储器，并可提供跳过列类型和租约。默认跳过集合为空、租约为一秒。
- `StatementStatsLoad` / `StatementContext`：本文件的语句级状态，保存总超时、待加载项、私有结果接收端、开始时间与 worker 错误文本；`ResultCh` 仅用于等待器和测试接入接收端。
- `StatsLoadResult`：worker 或 leader 广播的结果。`RequestFailed = true` 表示任务投递或 singleflight 等待阶段失败；worker 已完成但加载出错时为 `false` 且 `Error` 非空。
- `NeededItemTask`：队列实体，含请求、绝对截止时间、给 leader 的结果发送端和重试次数。
- `statsSyncLoad` / `NewStatsSyncLoad`：调度器和构造函数；持有两条有界队列、缓存更新互斥锁、实例级 singleflight waiter 表及三个原子指标。
- `StatsWrapper`：单项读取过程中的内部容器，把列/索引元信息与最终 `Column`/`Index` 一起传递。

## 执行流程

1. 规划阶段先把所需统计项放入会话语句上下文；`sync_wait_stats_load_point` 通过 `SessionStatsSyncLoadAdapter` 进入本模块。
2. `SendLoadRequests` 调用 `removeHistLoadedColumns`。表缓存不存在、对象已经 FullLoad，或请求只要元数据且现有对象已足够时，不再排队。
3. 每个剩余项创建容量为 1 的结果通道，并在 `singleflight: WaiterMap` 中注册。首个请求是 leader，它启动一个线程，在同一绝对期限内反复尝试写入 needed 队列并等待 worker 结果；followers 只追加发送端。leader 最终用 `complete_waiters` 删除分组并广播克隆结果。
4. `SubLoadWorker` 循环调用 `HandleOneTask`。若没有重试任务，`drainColTask` 优先取 needed 项；过期项被尽力放入 timeout 队列。若先取到 timeout 项，会再次探测 needed 队列并优先让新任务执行。
5. `HandleOneTask` 以 `catch_unwind` 包围 `handleOneItemTask`。成功时 `finish_task` 交付无错结果；失败且仍可重试时返回原任务；超过 `RetryCount` 后交付带错误结果，不再让该任务阻塞后续队列。
6. `handleOneItemTaskWithSCtx` 重新读取最新表统计和表结构。索引或列若已不需加载则成功返回；列类型在 `AnalyzeSkipColumnTypes` 中时跳过；未 ANALYZE 的列写入 `Column::Empty`，防止同一不存在统计反复触发加载。
7. `readStatsForOneItem` 总是先读直方图元数据。`FullLoad = true` 时再读桶及 CMSketch/TopN；否则保留元数据级对象。缺失元数据由上层当作可忽略结果，其他存储错误进入重试路径。
8. `updateCachedItem` 在互斥区内再次获取最新表快照，检查是否已有更完整数据，合并存在性与表级 stats version，再调用 `StatsHandle::UpdateStatsCache`。
9. `SyncWaitStatsLoad` 对所有接收端共用一个截止时间。worker 错误会记录到 `ErrorMessages`，但其 item 已交付并从 `unchecked` 删除；`RequestFailed`、通道断开或总超时会保留未完成项并返回错误。结束前会清空 `NeededItems`，避免状态泄漏到后续规划。

## 数据与状态

`statsSyncLoad` 的队列容量在构造时固定。`needed_items_sender/receiver` 保存仍可能被当前 SQL 等待的工作，`timeout_items_sender/receiver` 保存已经不再紧急但仍可用于填充缓存的工作。标准库 `mpsc::Receiver` 不是 `Sync`，所以两个接收端分别放在 `Mutex` 内；发送端可克隆给 leader 线程。

singleflight 是调度器实例级的 `HashMap<String, Vec<SyncSender<StatsLoadResult>>>`，键包含 `TableID/ID/IsIndex/FullLoad`。leader 完成时整个向量被移除，避免永久保留 waiter。指标由 `AtomicU64` 保存：`sync_load_count` 统计等待接收次数，`sync_load_timeout_count` 统计超时或最终仍缺项的等待，`sync_load_dedup_count` 统计 leader 成功入队次数；`metrics` 只做 Relaxed 读取，适合观测计数而不承担同步语义。

缓存的加载强度由 `LoadedStatus::{Evicted, Full}` 表示。列只有在直方图确有数据时才采用请求对应状态；索引还要求 `stats_version != 0`。`updateCachedItem` 的核心不变量是：已有 FullLoad 不能被覆盖，元数据请求也不覆盖已有对象；对全量请求，只有新对象可实际提升缓存时才写回。列/索引的存在性映射与非零 stats version 同步更新。

## 依赖与调用关系

上游直接调用链为：`optimizer_runtime::sync_wait_stats_load_point` → `StatsLoadWaiter::SyncWaitStatsLoad` → `SessionStatsSyncLoadAdapter::SyncWaitStatsLoad` → `statsSyncLoad::{SendLoadRequests, SyncWaitStatsLoad}`。Domain 初始化链为 `SessionStatsSyncLoadAdapter::new_with_domain` → `NewStatsSyncLoad` → 多个 `SubLoadWorker` 线程；`DomainStatsLoadWorkers::drop` 设置共享 `AtomicBool` 后 join 全部线程。

本文件内部的主要下游链为：`SubLoadWorker` → `HandleOneTask` → `handleOneItemTask` → `handleOneItemTaskWithSCtx` → `readStatsForOneItem` / `updateCachedItem`。持久化读取通过 `StatsHandle::Storage` 落到 `StatsStorage::{HistMetaFromStorageWithHighPriority, HistogramFromStorageWithHighPriority, CMSketchAndTopNFromStorageWithHighPriority}`；缓存提交通过 `StatsHandle::UpdateStatsCache` 返回适配层。

`lib.rs` 是公开 API 边界并把测试放在独立的 `stats_syncload_test.rs`，符合源代码与测试分文件的仓库约定。Cargo 元数据没有 feature 或第三方依赖；运行时与完整统计模块的关系通过 trait 和会话适配器形成，而非 Cargo 直接依赖。

## 错误处理与边界

- needed 队列满时，leader 每毫秒重试直至原截止时间；断开或到期生成 `RequestFailed = true` 的结果并广播。`AppendNeededItem` 提供同类行为，但直接返回 `ChannelFull`/`ChannelClosed`，主要供测试或显式注入任务。
- leader 入队成功后只等待剩余期限；worker 未及时回传或 task 结果通道断开同样属于请求失败。`complete_waiters` 即使 waiter 已离开也忽略发送错误，以保证清理分组。
- worker 加载错误不会直接让结果通道失联：它先按 `RetryCount` 重试，最终用 `finish_task` 交付 `RequestFailed = false` 且带错误文本的结果。等待方记录该文本，并把该 item 视为已经得到 worker 回答；这与 leader 自身超时导致的“没有完成”不同。
- `catch_unwind` 将字符串或任意 panic payload 变为 `Error::Panic`，保持 worker 循环可继续。互斥锁中毒统一变为 `Error::Poisoned`。
- 表缓存、表结构或列/索引元信息在并发 DDL 下消失时，加载路径选择无操作成功；直方图元数据不存在也被 `handleOneItemTaskWithSCtx` 吸收。未分析列则写空占位。以上行为避免把 schema 漂移或“本就没有统计”无限重试成查询错误。
- `SyncWaitStatsLoad` 使用一个总 deadline，而不是为每个 receiver 重置 timeout；因此多个项不会将语句等待时间乘以项数。`Timeout = 0` 是立即超时，不代表使用默认值。
- `writeToTimeoutChan` 在低优先级队列满时允许丢弃任务；此时原 SQL 已不再依赖它，代价是失去一次后台填充缓存的机会。

## 并发与资源生命周期

构造函数建立两条有界通道，调度器通常由 `Arc` 共享。每个 singleflight leader 会启动一个短生命周期线程，线程在任务完成、超时或通道断开时广播结果并退出；相同键的 followers 不再创建队列任务。`singleflight` 锁只保护分组注册/移除，不覆盖存储 IO。

Domain 级 worker 的生命周期由 `pkg/session/runtime/planning.rs::DomainStatsLoadWorkers` 所有：同一 Domain 的适配器通过 `OnceLock<Mutex<HashMap<...Weak...>>>` 复用 worker 组；最后一个强引用释放时，`Drop` 以 Release 写退出标记并 join。`SubLoadWorker` 以 Acquire 读取标记，空队列时最多等待 10ms 后再次检查。失败或带回重试任务时按 `Lease()/10` 加微秒级抖动退避，降低同时重试造成的拥塞。

`mutex_for_stats_cache` 将“重新读取最新表快照—判断覆盖条件—提交缓存”串行化，保护多个 worker 更新同一表时的读改写序列。原子指标不参与控制流。需要注意：任务进入 timeout 队列后仍保留其结果发送端，但 leader 可能已经超时退出；后续 worker 的 `finish_task` 会忽略发送失败，任务对象随处理结束释放。

## 与 Go 版本的对应关系

核心算法逐项对应 `pkg/statistics/handle/syncload/stats_syncload.go`：`RetryCount` 与 CPU 分档一致；双通道优先级、singleflight 去重、一个总等待计时器、失败重试、panic 隔离、未分析列空占位、三段存储读取以及缓存防降级写回均被保留。独立 Go 测试 `stats_syncload_test.go` 提供端到端语义基准，Rust 测试 `stats_syncload_test.rs` 用内存 mock 对应这些场景。

存在以下适配差异，应视为当前实现事实：

- Go 使用包级 `globalStatsSyncLoadSingleFlight`，Rust 的 waiter 表属于每个 `statsSyncLoad` 实例；实际 Domain 适配器让同一 Domain 共享实例，但不同 Domain 不相互去重。
- Go 构造函数直接读取全局配置，Rust `NewStatsSyncLoad` 接收显式 `queue_size`；并发度、队列大小和 worker 生命周期移到 `SessionStatsSyncLoadAdapter::new_with_domain`。
- Go worker 从系统 session pool 取得 session context，设置高优先级，并从全局变量解析跳过列类型；Rust 核心改为 `StatsHandle`/`StatsStorage` trait。适配器负责连接真实缓存与存储，`AnalyzeSkipColumnTypes` 是显式依赖。
- Go 使用 `singleflight.Result.Err` 区分请求层错误，用 `StatsLoadResult.Error` 表示 worker 错误；Rust以 `StatsLoadResult::RequestFailed` 明确保存这一区别，防止相同错误文本被误分类。
- Go 有日志和直方图指标；Rust 核心只保留三个原子计数并向调用方返回错误文本。日志、告警及 pseudo statistics 回退发生在外层规划/会话代码。

## 扩展指南

- 增加新的统计载荷时，优先扩展 `StatsStorage`、`StatsWrapper` 与 `readStatsForOneItem`，同时更新 `Column`/`Index` 的完整性判定；不得让较弱加载覆盖 FullLoad。对应 mock 与断言放在独立的 `stats_syncload_test.rs`。
- 改变请求身份或合并规则时必须同步 `StatsLoadItem::Key`。任何影响结果等价性的字段都必须进入 key，否则不同请求会错误共享结果；修改后重点覆盖两个 statement 同 key、不同 FullLoad、leader 失败和 follower 交付。
- 改变超时语义时同时审查 `SendLoadRequests` 的 leader deadline、`NeededItemTask::ToTimeout`、`drainColTask` 的降级以及 `SyncWaitStatsLoad` 的总 deadline。保持零超时立即失败，并覆盖队列容量为 0/满、无 worker 和“失败结果先于等待计时器到达”的竞态。
- 改变重试策略时修改 `RetryCount`/`isVaildForRetry` 和 worker 退避；需要证明错误任务不会永久占用 worker，且最终一定向所有 waiter 完成。panic 与普通存储错误都应保留测试。
- 改变缓存合并时集中修改 `updateCachedItem`，继续在互斥区内重新读取最新快照，并验证列/索引、meta/full、已分析/未分析及并发更新。适配层的 `DomainStatsSyncLoadHandle::UpdateStatsCache` 也需同步审查。
- 若把更多真实系统类型移入该 crate，应先调整 `Cargo.toml` 和适配边界，避免并存的轻量类型与 `astersql_meta_model`/`astersql_statistics_handle` 类型发生静默语义漂移。
- 回归测试首选同目录 `stats_syncload_test.rs`；跨规划链行为还应覆盖 `pkg/session/runtime_test/session.rs` 和 `pkg/planner/core/casetest/planstats/plan_stats_test.rs`。测试逻辑不要嵌入本生产文件。

## 验证依据

- Rust 源码：`pkg/statistics/handle/syncload/stats_syncload.rs`，重点核对 `SendLoadRequests`、`SyncWaitStatsLoad`、`SubLoadWorker`、`HandleOneTask`、`handleOneItemTaskWithSCtx`、`readStatsForOneItem`、`drainColTask`、`updateCachedItem`、`complete_waiters`。
- crate 边界：`pkg/statistics/handle/syncload/Cargo.toml` 与 `pkg/statistics/handle/syncload/lib.rs`。
- 直接应用接线：`pkg/session/runtime/planning.rs` 的 `DomainStatsLoadWorkers`、`SessionStatsSyncLoadAdapter`、`DomainStatsSyncLoadHandle`；规划入口为 `pkg/planner/core/optimizer_runtime.rs::sync_wait_stats_load_point`，接口为 `pkg/planner/core/base/plan_base.rs::StatsLoadWaiter`。
- Go 对照：`pkg/statistics/handle/syncload/stats_syncload.go` 中同名类型/函数；重点比较全局 singleflight、system session、高优先级读、双队列和缓存更新。
- 测试证据：Rust `pkg/statistics/handle/syncload/stats_syncload_test.rs` 覆盖并发加载、零超时、panic/失败后重试、重试耗尽、leader 等待超时、零容量队列、存储无对象、CPU 分档、请求层失败与 worker 错误分类及混合结果；Go `pkg/statistics/handle/syncload/stats_syncload_test.go` 覆盖对应端到端 SQL 行为和 issue #67629 的竞态回归。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件、目标文件有 93 个符号；`files --filter pkg/statistics/handle/syncload` 确认 Rust/Go 源与测试；`query SyncLoad`、`query StatsLoad` 定位上述关键符号。`explore` 和精确 callers/callees 查询在本次限定时间内未返回有效边，因此上游/下游边改由已索引符号位置和直接源码引用交叉验证，不采用未返回结果作为结论。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令验证文档形态，并人工复核所有“已支持”陈述均能回指上述源码或测试。
