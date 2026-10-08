# `pkg/util/topsql/stmtstats/rustats.rs`

## 文件定位

本文件是 `astersql-util-topsql-stmtstats` crate 的 TopRU 基础数据模型层：它不主动采样、调度或上报 RU（Request Unit），而是定义这些阶段共同使用的版本值、聚合键、单次执行上下文和增量容器。crate 入口 `pkg/util/topsql/stmtstats/lib.rs` 以私有 `mod rustats` 装载本文件，再通过 `pub use rustats::*` 将其所有公开项暴露给同 crate 的 `stmtstats.rs`、`aggregator.rs` 以及 reporter 等下游 crate。

`pkg/util/topsql/stmtstats/Cargo.toml` 将该 crate 命名为 `astersql-util-topsql-stmtstats`，并通过 `execdetails-dependency` 引入 `astersql-util-execdetails`；`lib.rs::execdetails` 再重导出 `RUDetails`，因此本文件的 `crate::execdetails::RUDetails` 实际来自执行详情 crate。根 `Cargo.toml` 用 `facade_util_topsql_stmtstats` 指向本 crate。

## 核心职责

1. 用 `RUVersion`、`RU_VERSION_V1`、`RU_VERSION_V2` 与 `DEFAULT_RU_VERSION` 固定 TopRU 数据路径中的协议版本取值，并由 `DefaultRUVersion`、`NormalizeRUVersion` 统一处理缺省版本。
2. 用 `RUVersionProvider` 抽象动态版本来源，使 `aggregator.rs::Aggregator::current_ru_version` 不依赖具体的集群/PD 实现。
3. 用 `RUKey` 把用户名、规范化 SQL digest 和计划 digest 组合成可哈希的完整聚合维度，避免不同用户或不同计划之间串账。
4. 用 `ExecutionContext` 保存一条活跃语句跨聚合 tick 所需的采样水位和 RU 明细句柄。
5. 用 `RUIncrement`、`RUIncrementMap` 与 `RUIncrementMapMerge` 表达并归并一个采样窗口中的 RU、执行次数和执行时长。

该文件只提供数据结构和无 I/O 的合并操作；TopRU 是否启用、何时采样、版本切换清理、key 数量上限以及向 reporter 投递均由相邻实现负责。

## 主要符号

- `type RUVersion = i32`：协议版本的线网整数表示。`RU_VERSION_V1 = 1`、`RU_VERSION_V2 = 2`，默认值为 v1。
- `trait RUVersionProvider: Send + Sync`：唯一方法 `GetRUVersion(&self) -> RUVersion`。`Send + Sync` 允许实现被 `Arc<dyn RUVersionProvider>` 安全共享给聚合后台线程。
- `DefaultRUVersion() -> RUVersion`：直接返回 `DEFAULT_RU_VERSION`。
- `NormalizeRUVersion(version) -> RUVersion`：只把零值改写为默认 v1，其他整数原样保留；它不验证未知非零版本。
- `type SharedRUDetails = Arc<RwLock<RUDetails>>`：在执行路径和周期采样路径之间共享 RU 明细。`Arc` 管理共享所有权，`RwLock` 管理内部并发访问。
- `RUKey { User, SQLDigest, PlanDigest }`：派生 `Clone + Debug + Eq + Hash + PartialEq`，可作为 `HashMap` 键。`RUKey::new` 接受用户名和两个字节切片，并复制 digest 字节到拥有所有权的 `BinaryDigest`。
- `ExecutionContext { RUDetails, Key, LastRUTotal, RUVersion }`：一条活跃执行的采样状态。`RUDetails` 可以为空；`LastRUTotal` 是已发出累计量的水位；`RUVersion` 决定 v1/v2 取值路径。
- `RUIncrement { TotalRU, ExecCount, ExecDuration }`：一个 key 的窗口增量，默认三个字段均为零；`ExecDuration` 的单位是纳秒。
- `RUIncrement::Merge(&mut self, other)`：逐字段相加，不修改 `other`。
- `type RUIncrementMap = HashMap<RUKey, RUIncrement>`：以拥有值而非指针保存增量。
- `RUIncrementMapMerge::Merge(&mut self, other)`：消费源 map；已有 key 调用 `RUIncrement::Merge`，新 key 通过 `or_default` 建槽后累加。

本文件没有条件编译项、异步函数、I/O、显式错误返回或后台任务。

## 执行流程

真实调用链由本文件的数据类型和相邻文件共同构成：

1. `aggregator.rs::Aggregator::current_ru_version` 从可选 `RUVersionProvider` 读取版本；未绑定 provider 时调用 `DefaultRUVersion`，读取后调用 `NormalizeRUVersion`。
2. `stmtstats.rs::StatementStatsInner::add_ru_on_begin` 调用 `RUKey::new`，建立 `ExecutionContext`，把 `LastRUTotal` 初始化为 `0.0`，并给该 key 的 `ExecCount` 加一。
3. 周期聚合调用 `StatementStats::MergeRUInto`。对于 v1 活跃语句，`sample_active_ru_delta` 经 `current_ru_total` 读取 `SharedRUDetails` 的 `RRU() + WRU()`，仅发送相对 `LastRUTotal` 的正增量，随后推进水位以避免重复计量。v2 在途采样为零，最终总量由 finish 信息提供。
4. `stmtstats.rs::add_ru_on_finish` 首先用用户、SQL digest、Plan digest 重建 key；只有它与活跃 `ExecutionContext::Key` 相等才结算。v1 使用 RUDetails 当前总量，v2 使用 `ExecFinishInfo::TotalRUV2`，再减去水位并把正 delta 与执行时长写入 `RUIncrement`。
5. `aggregator.rs::drain_and_push_ru` 遍历各 `StatementStats::MergeRUInto` 的结果。相同 key 调用 `RUIncrement::Merge`，不同 key 插入总 map；随后把 `RUIncrementMap` 和当前版本传给 `RUCollector::CollectRUIncrements`。
6. reporter 的 `reporter.rs::RemoteTopSQLReporter::CollectRUIncrements` 接收该 map，交给 `ru_window_aggregator.rs::addBatch` 按时间窗口累计，后续由 `ru_datamodel.rs::addBatch` 按用户和 SQL/计划维度整理上报数据。

版本发生切换时，`Aggregator::drain_and_push_ru` 先调用各会话的 `ResetRUStateOnVersionChange` 清除旧版本缓冲和执行上下文，通知 collector，并跳过本轮增量投递，避免两个计量口径混入同一批次。

## 数据与状态

`RUKey` 的等价性覆盖三个字段，因此同一 SQL/计划由不同用户执行时仍是不同 key；任一 digest 变化也会产生新 key。digest 在 `RUKey::new` 中被复制，调用方后续修改原字节缓冲不会改变 map 键。

`ExecutionContext::LastRUTotal` 是累计量水位，不是本 tick 的 delta。其不变量是：成功采样后更新为当时的累计值；finish 只应累加 `current_total - LastRUTotal` 的正值。`RUDetails: None` 被相邻采样代码视为总量零。`RUVersion` 在 begin 时已经过规范化，但消费处仍重复规范化以容忍手工构造或兼容调用。

`RUIncrement::ExecCount` 采用 begin-based 语义：每次启用 TopRU 的执行在 begin 时贡献一次，跨多个 tick 的后续 RU delta 不再重复计数。`ExecDuration` 仅在 finish 产生正 RU delta 时累加。`RUIncrementMapMerge` 的加法是结合式累计模型，调用者可以把多个 tick、会话或 reporter batch 归并到同一 key。

数值层面没有饱和、溢出检查或浮点清洗：`TotalRU` 遵循 `f64` 运算规则，整数计数/时长使用普通 `u64` 加法。当前生产路径只接受正 RU delta，从而过滤计数器回退产生的负值；该约束位于 `stmtstats.rs`，不由本文件的 `Merge` 强制。

## 依赖与调用关系

上游直接使用者及职责如下：

- `pkg/util/topsql/stmtstats/stmtstats.rs`：构造 `RUKey`/`ExecutionContext`，维护 `RUIncrementMap`，使用版本常量选择 v1/v2 采样算法。
- `pkg/util/topsql/stmtstats/aggregator.rs`：实现 `RUVersionProvider` 的绑定与读取，跨 `StatementStats` 合并 `RUIncrement`，并将 map 交给 `RUCollector`。
- `pkg/util/topsql/reporter/reporter.rs`、`ru_window_aggregator.rs`、`ru_datamodel.rs`：消费 `RUIncrementMap`，按时间窗口、用户和 digest 继续汇总。
- 相关测试通过 `lib.rs` 的公开重导出直接构造这些类型，验证采样、合并、版本切换和 key 隔离。

本文件的直接下游依赖很小：标准库 `HashMap`、`Arc`、`RwLock`，crate 内 `BinaryDigest`，以及由 `execdetails-dependency` 重导出的 `RUDetails`。它不直接依赖 reporter、TopSQL 全局开关或 PD 客户端；PD 版本语义在 Rust 中以本地 `i32` 常量表达。

RustCodeGraph 的精确结果确认了关键边：`NormalizeRUVersion` 被 `ResetRUStateOnVersionChange`、`add_ru_on_begin`、`add_ru_on_finish`、`current_ru_total`、`Aggregator::current_ru_version` 和 reporter 窗口逻辑调用；`GetRUVersion` 被 `Aggregator::current_ru_version` 调用；`RUIncrement::Merge` 出现在 aggregator 的同 key 合并路径。

## 错误处理与边界

本文件没有 `Result` 或可恢复错误：构造与合并均为内存操作。需要特别保留的边界行为是：

- `NormalizeRUVersion(0)` 返回 v1；未知非零整数不会被拒绝或降级。Go 注释写有“unknown versions”，但 Go 与 Rust 的实际实现都只特殊处理零值。
- `RUKey` 允许空用户名、空 SQL digest 或空 Plan digest；本层不会把它们折叠为特殊的 others key，也不做合法性校验。
- `ExecutionContext::RUDetails` 允许为空，实际采样由 `stmtstats.rs::current_ru_total` 返回零。
- `RUIncrement::Merge` 不限制负数、`NaN`、无穷值或整数溢出；安全性依赖生产者只产生有效增量。
- `RUIncrementMapMerge::Merge` 消费源 map，因此调用者若还需复用源数据必须先克隆。
- `SharedRUDetails` 的锁若中毒，当前调用路径使用 `expect("RUDetails lock poisoned")` 并 panic；本文件没有恢复策略。
- key 数量上限不在 map 类型内实现。`aggregator.rs` 在单次聚合达到 `MAX_RU_KEYS_PER_AGGREGATE` 后丢弃新的 distinct key 并记录指标；已有 key 仍继续合并。

`stmtstats.rs` 还承担两个重要保护：begin/finish key 不一致时不把旧执行的 RU 写入新 key；累计量回退或未增长时不发出非正 delta。这些不是 `rustats.rs` 类型系统自动保证的行为，扩展时不能绕过相邻入口直接写入未经验证的数据。

## 并发与资源生命周期

`RUVersionProvider: Send + Sync` 使 provider 可被 `Aggregator` 的 `RwLock<Option<Arc<dyn RUVersionProvider>>>` 和后台线程共享。`SharedRUDetails` 用 `Arc` 让执行方、finish 信息和采样方共享同一明细生命周期，用 `RwLock` 保证更新与读取互斥；本文件只定义句柄，锁的实际获取位于 `stmtstats.rs::current_ru_total` 等调用点。

`RUKey`、`RUIncrement` 和 `RUIncrementMap` 本身是拥有值的数据，不含内部锁。`StatementStats` 在外层以 `Mutex<StatementStatsInner>` 串行化 begin、finish 与 tick，所以 `ExecutionContext::LastRUTotal` 的读取和推进不会在未加锁状态下竞争。测试 `TestExecCountBeginBasedFinishAndTickConcurrent` 覆盖 finish/tick 竞态，验证总 RU 和 begin-based `ExecCount` 不重复。

聚合器每秒执行一轮，先 `drain_and_push_ru` 再 `drain_and_push_stmt_stats`，从而在已完成会话被注销前排空尾部 RU。`RUIncrementMapMerge::Merge` 不启动任务、不持有外部资源，也不跨调用保存借用；源 map 在合并完成后被释放。版本切换会主动清空旧上下文和缓冲，使不同协议版本的生命周期在批次边界上隔离。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/topsql/stmtstats/rustats.go`。主要语义一一对应：

- Go `rmclient.RUVersion` 对应 Rust `i32` 别名和本地 v1/v2 常量；两端默认版本当前都是 v1，零值都规范为默认值。
- Go `RUVersionProvider.GetRUVersion()` 对应 Rust trait 方法；Rust 额外声明 `Send + Sync` 以满足线程共享约束。
- Go `RUKey` 三字段对应 Rust 同名三字段；Rust 的 `RUKey::new` 是额外的拥有化便利构造器。
- Go `ExecutionContext.RUDetails` 是 `*util.RUDetails`；Rust 是 `Option<Arc<RwLock<RUDetails>>>`，用 `Option` 表达 nil、用共享所有权和锁表达并发可变性。
- Go `RUIncrementMap` 是 `map[RUKey]*RUIncrement`；Rust 是 `HashMap<RUKey, RUIncrement>`，避免空 value 指针，并以 `Default` 创建缺失槽位。
- Go 只给 `RUIncrement` 定义 `Merge`；Rust 因无法给标准库类型直接添加固有方法，额外定义 `RUIncrementMapMerge` trait 来保留 `map.Merge(...)` 风格和按键合并语义。

测试证据显示 Rust 保留了 Go 的关键行为：`stmtstats_test.rs::TestOnExecutionBeginFinishRU` 覆盖 v1 的 RRU+WRU；`go_merge_20_topru_v2_uses_finalized_total_only` 与 `go_merge_40_finish_uses_version_specific_ru_total` 覆盖 v2 只在 finish 使用最终总量；`TestMergeRUIntoInFlightSamplingAndFinishDedup` 覆盖跨 tick 去重；`aggregator_test.rs::TestAggregatorDetectsRUVersionHandover` 覆盖版本切换清理和通知；`aggregator_1_aster_unit_test.rs::aggregator_caps_distinct_ru_keys_and_records_drops` 覆盖 distinct key 上限。Go 的对应测试位于 `stmtstats_test.go` 和 `aggregator_test.go`。

## 扩展指南

新增 RU 版本时，应同步检查 `RUVersion` 常量、`DEFAULT_RU_VERSION`、`NormalizeRUVersion`，以及 `stmtstats.rs::current_ru_total`、`add_ru_on_finish` 和版本切换逻辑；仅增加常量而不增加采样分支会使未知非零版本落入现有非 v2 路径。还需同步 Go 的 `rustats.go`/PD 版本定义，并在独立的 `stmtstats_test.rs`、`aggregator_test.rs` 增加版本专用测试。

新增聚合维度时，应修改 `RUKey`、`RUKey::new` 和所有构造点，并同步 reporter 的分组/压缩逻辑及 Go `RUKey`。这是兼容性敏感改动：key 等价性改变会影响基数、内存、`MAX_RU_KEYS_PER_AGGREGATE` 丢弃率和上报结果，必须增加“同 digest 不同新维度不串账”的独立测试。

新增增量字段时，应修改 `RUIncrement` 的默认值和 `Merge`，同时更新 producer（`stmtstats.rs`）、aggregator、reporter data model/协议转换以及 Go 对照。遗漏任一合并层会在跨 tick 或跨会话时静默丢数。测试应放在现有独立文件中，不要把测试嵌入 `rustats.rs`；至少覆盖同 key 合并、异 key 隔离、跨 tick、finish、版本切换和并发顺序。

若要改变 `SharedRUDetails` 的锁或所有权模型，先审查执行详情的所有写入方及锁顺序。当前读锁范围很短；在锁内加入 reporter I/O 或获取 `StatementStats`/aggregator 锁会增加死锁和延迟风险。性能优化应关注 key 克隆、digest 字节复制、map 扩容和批次 clone，但不能以共享可变 key 或跳过完整三字段比较换取速度。

## 验证依据

- 源码与 crate 边界：`pkg/util/topsql/stmtstats/rustats.rs`、`lib.rs`、`Cargo.toml`、根 `Cargo.toml`。
- 直接生产调用：`pkg/util/topsql/stmtstats/stmtstats.rs` 的 `add_ru_on_begin`、`add_ru_on_finish`、`sample_active_ru_delta`、`current_ru_total`；`aggregator.rs` 的 `current_ru_version`、`aggregate_all`、`drain_and_push_ru`。
- 下游消费：`pkg/util/topsql/reporter/reporter.rs::CollectRUIncrements`、`ru_window_aggregator.rs::addBatch`、`ru_datamodel.rs::addBatch`。
- Go 对照：`pkg/util/topsql/stmtstats/rustats.go`，以及同目录 `stmtstats.go`、`aggregator.go` 的对应生产链。
- Rust 独立测试：`pkg/util/topsql/stmtstats/stmtstats_test.rs`、`aggregator_test.rs`、`aggregator_1_aster_unit_test.rs`；Go 独立测试：`stmtstats_test.go`、`aggregator_test.go`。这些文件验证版本口径、在途 delta、水位去重、begin-based 计数、key 隔离、finish/tick 并发、版本切换和 key 上限。
- RustCodeGraph：`status` 确认索引含 11,467 个文件、目标文件已索引；`files --filter pkg/util/topsql/stmtstats` 确认模块文件集；`node --file .../rustats.rs` 核对 131 行源码及 12 个使用文件；`query`/`explore` 核对 `RUIncrementMapMerge`、`GetRUVersion`、`NormalizeRUVersion`、`MergeRUInto`、`drain_and_push_ru` 与 reporter 的调用关系。宽泛同名 `Merge` 查询存在跨仓库噪声，因此结论只采用带目标路径的结果并用相邻源码复核。
- 未运行 Cargo 或代码测试：任务是纯文档分析，计划明确禁止 Cargo；完成验证采用固定章节结构检查与人工事实复核。
