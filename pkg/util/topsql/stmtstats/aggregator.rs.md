# `pkg/util/topsql/stmtstats/aggregator.rs`

## 文件定位

该文件实现 `astersql-util-topsql-stmtstats` crate 的进程级语句统计聚合器。crate 入口 `pkg/util/topsql/stmtstats/lib.rs` 通过 `mod aggregator; pub use aggregator::*;` 导出这里的 API；`pkg/util/topsql/stmtstats/Cargo.toml` 则表明它直接依赖执行详情、TopSQL reporter 指标和 TopSQL/TopRU 全局开关三个相邻 crate。

在应用主链中，`pkg/util/topsql/topsql.rs::SetupTopProfiling` 将 reporter 包装成 `Collector`，在 reporter 支持 RU 时再包装成 `RUCollector`，绑定 `RUVersionProvider` 后调用 `SetupAggregator`。会话侧由 `pkg/util/topsql/stmtstats/stmtstats.rs::CreateStatementStats` 创建 `StatementStats` 并注册到 `global_aggregator()`。因此本文件位于“各会话累积统计”与“reporter 消费批次”之间，负责定时抽取、跨会话合并、开关门控和生命周期清理，而不负责产生单条语句统计或把批次发送到外部服务。

## 核心职责

- 维护最多 `MAX_STMT_STATS_SIZE`（1,000,000）个 `Arc<StatementStats>`，并按 `Arc` 指针身份去重和注销。
- 维护语句收集器 `Collector` 与 RU 收集器 `RUCollector` 两套独立订阅者，保持 TopSQL 与 TopRU 输出解耦。
- `start` 启动单一后台线程，每隔一秒调用 `aggregate_all`；`close` 发出关闭信号并等待线程退出。
- 每轮必须先执行 `drain_and_push_ru`，再执行 `drain_and_push_stmt_stats`。后者会注销 `Finished()` 会话，先排空 RU 才能保住会话结束时的尾部 RU 增量。
- 发现 RU 版本变化时清空各 `StatementStats` 中与旧版本相关的 RU 状态，并通知所有 RU 收集器；该轮不发送 RU 增量，避免跨版本混合。
- 将单轮 distinct RU key 数限制在 `MAX_RU_KEYS_PER_AGGREGATE`（10,000），对超限的新 key 丢弃并累计 reporter 指标。

## 主要符号

- `Collector: Send + Sync`：语句批次消费接口，唯一方法 `CollectStmtStatsMap(StatementStatsMap)` 接收一份已合并快照。
- `RUCollector: Send + Sync`：RU 消费接口。`CollectRUIncrements(RUIncrementMap, RUVersion)` 接收增量及其版本；`OnRUVersionChange(RUVersion)` 接收版本切换事件。
- `Aggregator`：核心共享对象。其字段分别保存 RU 版本提供者、已注册 stats、两类 collector、运行标志、上次 RU 版本、关闭通道发送端和 worker 线程句柄。
- `Aggregator::new() -> Arc<Self>`：构造未启动的实例；`last_ru_version` 初始化为 `DefaultRUVersion()`。
- `set_ru_version_provider` / `current_ru_version`：设置可选 provider，并将 provider 返回值经 `NormalizeRUVersion` 归一化；没有 provider 时使用默认版本。
- `start` / `close` / `closed`：控制后台线程。`start` 通过 `AtomicBool::compare_exchange` 拒绝重复启动，`close` 通过 `swap` 实现幂等关闭。
- `register` / `register_with_limit` / `unregister`：按容量和指针身份管理 `StatementStats`；`register_with_limit` 是供同 crate 测试容量边界使用的内部入口。
- `register_collector`、`unregister_collector`、`register_ru_collector`、`unregister_ru_collector`：按 trait object 的 `Arc` 指针身份去重或删除订阅者。
- `aggregate_all`：单轮总入口，固定按 RU、语句统计的顺序执行。
- `drain_and_push_stmt_stats`：调用每个 stats 的 `Take()`，合并到 `StatementStatsMap`，清理 finished stats，并在 TopSQL 开启且结果非空时推送。
- `drain_and_push_ru`：处理版本切换，或调用每个 stats 的 `MergeRUInto()` 合并 RU、执行 key 上限和指标记录，再按 TopRU 开关推送。
- `record_dropped_ru`：将超限 key 数和对应 RU 总量写入 `IgnoreExceedRUKeysCounter`、`IgnoreExceedRUTotalCounter`；指标句柄未初始化时容忍缺失。
- `GLOBAL_AGGREGATOR` / `global_aggregator`：惰性创建的进程级 `Arc<Aggregator>` 及其访问函数。
- `SetupAggregator`、`BindRUVersionProvider`、`CloseAggregator`、`RegisterCollector`、`UnregisterCollector`、`RegisterRUCollector`、`UnregisterRUCollector`：面向 crate 使用者的全局实例门面。

## 执行流程

1. `pkg/util/topsql/topsql.rs::SetupTopProfiling` 先启动 reporter/data sink，再注册 `Collector` 和可选 `RUCollector`，绑定 RU 版本 provider，最后调用 `SetupAggregator`。
2. `SetupAggregator` 经 `global_aggregator().start()` 将当前规范化 RU 版本写入 `last_ru_version`，创建关闭通道并启动 worker。worker 在 `recv_timeout(Duration::from_secs(1))` 超时时聚合；收到关闭消息或发送端断开时退出。
3. SQL 会话通过 `CreateStatementStats` 创建并注册 stats。执行过程由 `StatementStats` 自身累积语句数据和 RU 数据，本文件只持有共享引用。
4. `aggregate_all` 先调用 `drain_and_push_ru`。若当前 RU 版本不同于 `last_ru_version`，它逐一调用 `ResetRUStateOnVersionChange`，通知 `OnRUVersionChange`，更新版本并立即结束本轮 RU 阶段。版本未变时，它抽取各 stats 的 RU 增量：已有 key 调用 `RUIncrement::Merge`，新 key 在 10,000 上限内插入，超限则累计丢弃值。
5. RU 数据无论 TopRU 是否开启都会先被抽取并执行版本/容量维护；只有合并结果非空且 `TopRUEnabled()` 时才克隆批次并调用各 `RUCollector::CollectRUIncrements`。
6. 随后的 `drain_and_push_stmt_stats` 对当前 stats 快照逐个处理：先观察 `Finished()` 并从注册表注销，再调用 `Take()` 并通过 `StatementStatsMapMerge::Merge` 汇总。数据同样会先被抽取；仅在结果非空且 `TopSQLEnabled()` 时发送给 `Collector`。
7. `CloseAggregator` 调用 `close`：原子地切换运行状态，取出并触发关闭发送端，再取出 worker 句柄并 `join`，保证正常关闭返回时后台线程已退出。

## 数据与状态

`stats`、`collectors` 和 `ru_collectors` 都是 `Mutex<Vec<Arc<_>>>`。注册表保存强引用，因此 stats 必须通过显式注销或 finished 清理才能从聚合器释放；collector 同理必须通过对应注销 API 移除。遍历前会在锁内克隆 `Vec`，之后释放注册表锁再调用 stats/collector，这既避免持有集合锁跨越外部回调，也保证本轮使用稳定快照。结果是并发注销只影响后续轮次：已进入本轮快照的对象仍可能收到一次回调。

`ru_version_provider` 用 `RwLock<Option<Arc<dyn RUVersionProvider>>>` 允许读多写少地更换 provider。`last_ru_version: AtomicI32` 与 `running: AtomicBool` 均使用 `SeqCst`；前者是版本切换水位，后者是 worker 生命周期门闩。`shutdown` 和 `worker` 用互斥锁保存可取走的一次性资源。

语句批次和 RU 批次向每个 collector 分别克隆，避免一个消费者取得所有权后妨碍其他消费者。RU 容量约束针对“单轮合并结果中的 distinct key”，已有 key 即使达到上限仍继续合并，因此高频 key 的后续增量不会仅因容量已满而被丢弃；哪些新 key 被保留取决于 stats 快照及各 RU map 的迭代顺序，代码没有稳定排序承诺。

## 依赖与调用关系

上游生产调用链有两条：

- `pkg/util/topsql/topsql.rs::SetupTopProfiling` → `RegisterCollector` / 可选 `RegisterRUCollector` → `BindRUVersionProvider` → `SetupAggregator`。收集器适配器最终把合并结果交给 TopSQL reporter。
- SQL 执行侧 → `pkg/util/topsql/stmtstats/stmtstats.rs::CreateStatementStats` → `global_aggregator().register(...)`，将会话统计纳入周期聚合。

文件内部主调用边为 `start` → `aggregate_all` → (`drain_and_push_ru`, `drain_and_push_stmt_stats`)。RU 路径继续调用 `current_ru_version`、`StatementStats::ResetRUStateOnVersionChange` 或 `StatementStats::MergeRUInto`、`RUIncrement::Merge`、`record_dropped_ru` 以及 `RUCollector` 回调；语句路径调用 `StatementStats::Finished`、`unregister`、`StatementStats::Take`、`StatementStatsMapMerge::Merge` 和 `Collector` 回调。

Cargo 直接依赖映射为：`execdetails-dependency` 支撑同 crate 的 RU 执行详情类型，`reporter-metrics-dependency` 提供丢弃计数器，`topsql-state-dependency` 提供 `TopSQLEnabled`/`TopRUEnabled` 门控。`aggregator.rs` 还依赖同 crate 从 `rustats.rs`、`stmtstats.rs` 重导出的 RU 版本、RU map、语句 map 与 stats 类型。

RustCodeGraph 的目标文件节点列出直接使用者 `pkg/util/topsql/reporter/reporter.rs`、`pkg/util/sqlkiller/sqlkiller.rs` 以及相关测试；精确符号图进一步确认 `CreateStatementStats` 注册全局 stats、`SetupTopProfiling` 完成收集器与启动接线。调用图对泛型/trait/同名符号可能产生不精确边，例如把 `is_empty` 解析到无关实现，因此本文只采用同时能由源码位置复核的边。

## 错误处理与边界

本 API 不返回 `Result`。锁中毒、worker panic 等内部不变量破坏通过 `expect(...)` 触发 panic；`close` 的 `join` 也会传播 worker panic。关闭信号发送失败被忽略，因为接收端已消失同样表示 worker 无需再被唤醒。`record_dropped_ru` 对尚未初始化的全局指标句柄采用 `if let Some`，早期启动阶段不会因此失败。

达到 stats 容量时 `register_with_limit` 静默拒绝新对象。重复注册同一 `Arc` 不新增；注销不存在的对象不报错。空批次不会回调 collector。需要特别注意：TopSQL/TopRU 开关只门控“发送”，不门控 `Take()`/`MergeRUInto()`；关闭期间产生并在 drain 中取出的数据会被丢弃，而不是留待以后重新开启。

RU 版本改变时，本轮旧状态被重置且只发送版本变化通知，不发送数据；这是防止旧、新口径混合的有意边界。10,000 key 上限只丢弃超限的新 key，并记录 key 数与其 `TotalRU`，不会把被丢弃的完整条目交给 collector。

`Drop for Aggregator` 只尝试发送关闭信号，不 `join`。正常资源回收应调用 `close`；尤其运行中的 worker 自己持有 `Arc<Aggregator>`，不能把 `Drop` 当作等价的同步关闭协议。

## 并发与资源生命周期

`Aggregator` 通过 `Arc` 共享，trait 也要求 `Send + Sync`。`start` 的原子 compare-exchange 保证同一时刻只创建一个 worker；`close` 的 swap 保证重复关闭直接返回。worker 的循环只在一秒超时后聚合，不在启动时立即聚合。关闭消息会使循环退出；`join` 给调用者提供确定的停止点。

聚合采用“锁内克隆、锁外工作”的快照方式。这样 collector 回调可以阻塞而不占用 collector 注册表锁，并发 unregister 不会使正在执行的回调失效。`pkg/util/topsql/stmtstats/aggregator_test.rs::TestAggregatorDrainTailIncrementMatrix` 明确验证：RU collector 已进入回调后，即使并发注销 stats 和 RU collector，本轮快照仍保留尾部 RU，下一轮才反映注销。

`aggregate_all` 的 RU-first 顺序是跨对象生命周期不变量：`drain_and_push_stmt_stats` 可能看到 `Finished()` 后注销 stats，如果顺序反转，最后一段 RU 将失去聚合入口。独立测试 `TestAggregatorRunOrderKeepsFinishedRU` 和 `aggregator_drains_ru_before_unregistering_finished_stats` 都锁定了这一行为。

虽然单个注册、注销和启动/关闭入口均由锁或原子量保护，但应用层仍应按 `SetupTopProfiling`/关闭流程成对管理全局 collector 和 worker；本文件没有跨多个全局门面调用的事务。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/topsql/stmtstats/aggregator.go`。Rust 保留了相同的核心语义：1 秒周期、RU-first、finished stats 清理、TopSQL/TopRU 独立门控、RU 版本切换重置、1,000,000 stats 上限、10,000 distinct RU key 上限、丢弃指标以及两套 collector API。

实现机制上的差异如下：

- Go 使用 `context.CancelFunc`、`time.Ticker`、goroutine 和 `sync.WaitGroup`；Rust 使用 `mpsc::recv_timeout`、OS thread 和 `JoinHandle`。
- Go 的集合是 `sync.Map`，另用 `atomic.Uint32` 统计 stats 数；Rust 用 `Mutex<Vec<Arc<_>>>`，容量检查、去重和长度在同一锁内完成。这使 Rust 的容量判定与插入原子化，也让重复注册不增加数量。
- Go collector 依赖接口值作为 map key；Rust 使用 `Arc::ptr_eq`，身份语义明确为同一个分配对象，而非业务值相等。
- Go 的 `lastRUVersion` 是普通字段，并在既定生命周期内访问；Rust 用 `AtomicI32`。provider 在 Go 中是接口字段，Rust 用 `RwLock<Option<Arc<dyn RUVersionProvider>>>` 支持并发读取/替换。
- Rust 向多个 collector 发送 `total.clone()`；Go map 直接交给各回调。两者约定 collector 消费聚合批次，但 Rust 在所有权层面隔离每个回调。
- Go 的指标句柄被直接使用；Rust 的 `record_dropped_ru` 容忍句柄尚未初始化。这是启动边界差异，不改变 key 上限本身。

对应测试也保持同名或同意图覆盖：Rust `aggregator_test.rs` 对照 Go `aggregator_test.go` 验证生命周期、注册、门控、尾部 RU、版本切换、并发注销和容量；`aggregator_1_aster_unit_test.rs` 另以独立 Rust 测试加强 RU 顺序、版本与上限语义。基准对应位于 `aggregator_bench_test.rs` / `aggregator_bench_test.go`。

## 扩展指南

- 新增语句批次处理规则时，优先修改 `drain_and_push_stmt_stats`，并在独立的 `aggregator_test.rs` 扩展测试；不要把测试内嵌进生产文件。必须保留“先 Take、再按开关决定是否发送”的现有清理语义，除非同步论证并修改 Go 对照行为。
- 新增 RU 聚合维度或裁剪策略时，修改 `drain_and_push_ru` 与 `record_dropped_ru`，同步检查 `RUKey`/`RUIncrementMap`（`rustats.rs`）、reporter 指标和 Go `aggregator.go`。风险包括单轮内存、克隆成本、热 key 被意外丢弃以及指标口径变化。
- 新增 collector 回调或版本事件时，应同步更新 `Collector`/`RUCollector`、`pkg/util/topsql/topsql.rs` 的适配器、reporter 实现及独立 mock。调用回调前继续采用快照，避免在注册表锁内执行外部代码。
- 调整周期或生命周期时，修改 `start`/`close` 并同时覆盖重复 start/close、关闭等待和阻塞回调情形。不要依赖 `Drop` 代替显式关闭。
- 调整 stats 容量或注册结构时，应保持并发容量检查与插入不可分割，并验证重复 `Arc`、并发注册和 finished 自动注销。现有直接测试包括 `go_merge_39_register_stops_at_capacity` 与 `go_merge_40_concurrent_registration_honors_capacity`。
- 任何行为变化都应先对照 `aggregator.go` 和 `aggregator_test.go`，保持移植语义；若有意产生差异，应在此文档和独立 Rust 测试中明确边界、兼容性与性能影响。

## 验证依据

- 目标实现：`pkg/util/topsql/stmtstats/aggregator.rs`，核对了全部 374 行、两个常量、两个 trait、`Aggregator` 字段及 impl、`Drop`、指标辅助函数、全局实例和七个全局门面函数。
- crate 边界：`pkg/util/topsql/stmtstats/Cargo.toml` 与 `pkg/util/topsql/stmtstats/lib.rs`；目标目录没有 `doc.go`。
- 生产入口：`pkg/util/topsql/topsql.rs::SetupTopProfiling`；stats 注册入口：`pkg/util/topsql/stmtstats/stmtstats.rs::CreateStatementStats`。
- Go 对照：`pkg/util/topsql/stmtstats/aggregator.go`；Go 回归测试：`pkg/util/topsql/stmtstats/aggregator_test.go`；Rust 独立测试：`pkg/util/topsql/stmtstats/aggregator_test.rs`、`pkg/util/topsql/stmtstats/aggregator_1_aster_unit_test.rs`，基准文件为两种语言的 `aggregator_bench_test.*`。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/util/topsql/stmtstats` 找到目标文件及同目录实现/测试；`node --file ...aggregator.rs` 读取完整目标并给出直接使用文件；`query`/`node` 核对了 `Aggregator`、`aggregate_all`、`drain_and_push_ru`、`drain_and_push_stmt_stats`、`SetupAggregator`、`RegisterCollector`、`current_ru_version`、`CreateStatementStats` 和 `SetupTopProfiling` 的定义与可复核调用轨迹。自然语言 `explore` 未在等待窗口内返回，已中止并改用精确查询，不将其作为结论依据。
- 人工复核结论：本文件存在的原因是把大量会话局部统计转换为受开关、版本和容量约束的 reporter 批次；安全扩展的关键不变量是 RU-first、版本切换隔离、快照后锁外回调、显式 close/join，以及 Rust/Go 行为和独立测试同步。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认文件存在且恰有十一个规定的二级标题。
