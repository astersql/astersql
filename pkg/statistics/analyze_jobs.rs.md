# `pkg/statistics/analyze_jobs.rs`

## 文件定位

本文件属于 `astersql-statistics` crate；`pkg/statistics/Cargo.toml` 将 `lib.rs` 设为库入口，`pkg/statistics/lib.rs` 通过 `mod analyze_jobs;` 装配本模块，再以 `pub use analyze_jobs::*;` 将其公开项重导出。它定义 ANALYZE 作业的共享元数据、状态/类型常量和进度节流器，本身不执行 SQL，也不直接读写 `mysql.analyze_jobs`。

在当前 Rust 接线中，`pkg/statistics/handle/types/interfaces.rs::StatsAnalyze` 以本 crate 的 `AnalyzeJob` 和 `JobType` 声明插入、启动、更新进度和结束作业的接口。需要注意，`pkg/statistics/handle/autoanalyze/autoanalyze.rs` 另有一套蛇形命名的 `AnalyzeJob`、`AnalyzeProgress` 和 `JobType` 及 SQL 生命周期函数；它们不是本文件类型的实现块，不能据此断言本文件的方法已经直接驱动该模块的 SQL。

## 核心职责

- 用 `AnalyzePending`、`AnalyzeRunning`、`AnalyzeFinished`、`AnalyzeFailed` 固定作业状态字符串，供生命周期边界与持久化层保持一致。
- 用 `JobType`、`TableAnalysisJob`、`GlobalStatsMergeJob` 区分表/分区分析和全局统计合并。
- 用 `AnalyzeJob` 汇集起止时间、持久化 ID、库表分区名、作业说明、采样率原因和进度状态。
- 用 `AnalyzeProgress` 累计尚未持久化的处理行数，并同时以行数阈值和时间窗口限制进度更新频率。
- 不负责作业调度、SQL 拼装、失败原因记录或最终剩余进度刷盘；这些属于调用本数据模型的 Handle/执行层。

## 主要符号

- `AnalyzePending` / `AnalyzeRunning` / `AnalyzeFinished` / `AnalyzeFailed: &str`：四种持久化状态值，分别为 `pending`、`running`、`finished`、`failed`。
- `JobType = i32`：作业类型的整数别名。`TableAnalysisJob = 1`，`GlobalStatsMergeJob = 2`；别名本身不会阻止其他 `i32` 值进入接口。
- `maxDelta = 10_000_000`：进度触发阈值。`Update` 使用严格大于，因此累计值恰为 10,000,000 时不会返回可落库增量。
- `dumpTimeInterval = 5s`：最小刷新间隔，同样使用严格大于；恰好 5 秒不触发。
- `AnalyzeJob`：公开字段的数据载体。`ID: Option<u64>` 表示尚未插入时无持久化 ID；`Progress` 内嵌每个作业独立的节流状态。
- `AnalyzeJob::default()`：将两个时间设为 `SystemTime::UNIX_EPOCH`、ID 设为 `None`、字符串清空并创建空进度。
- `AnalyzeProgress`：以私有 `Mutex<SystemTime>` 保存上次刷新时间，以 `AtomicI64` 保存未刷新增量；调用方只能通过方法访问这两项状态。
- `AnalyzeProgress::Update(&self, row_count) -> i64`：原子累加并判断是否应刷新；触发时返回累加后的数量，同时把内部计数清零并更新时间，否则返回 0。
- `GetDeltaCount`、`SetLastDumpTime`、`GetLastDumpTime`：分别读取未刷新增量、写入和读取上次刷新时间。

## 执行流程

典型生命周期由外层组件组织，而本文件提供以下状态变化：

1. 调用方建立 `AnalyzeJob`，填写库、表、分区、说明等字段；默认 ID 为空，起止时间为 Unix 纪元。
2. 作业持久化并启动后，外层应设置 ID/开始时间，并用 `SetLastDumpTime` 建立刷新时间基线。Go 对照中的 `startAnalyzeJob` 明确执行这一步；Rust `StatsAnalyze` 接口也保留相应生命周期入口。
3. 扫描过程调用 `Update(row_count)`。方法先以 `SeqCst` 原子加法取得新的累计值，再获取时间互斥锁。
4. 仅当累计值 `> 10_000_000` 且当前时间距离上次刷新 `> 5s` 时，方法以 `SeqCst` 将计数写回 0、更新时间并返回本批增量；调用方看到非 0 返回值后才应更新持久化进度。
5. 条件不满足时返回 0，累计值留待后续调用。作业结束时是否把剩余值写出不是 `Update` 的职责；Go `finishAnalyzeJob` 会对 `TableAnalysisJob` 读取 `GetDeltaCount` 完成尾刷。

`pkg/statistics/statistics_test.rs::analyze_progress_tracks_delta_and_persist_threshold` 直接验证两次小增量累计为 25，且仅设置刷新时间不会清空累计值。更完整的 Go 回归 `pkg/executor/test/analyzetest/analyze_test.go` 验证“小增量不刷、超过阈值且超过间隔时清零并刷新时间、紧接着再次大增量因时间窗口而保留、结束时尾刷”的链路。

## 数据与状态

`AnalyzeJob` 的字符串和时间字段由拥有该实例的调用方直接修改；本文件没有为它提供内部同步。`AnalyzeProgress` 则允许通过共享引用并发更新：计数和时间分别由原子变量与互斥锁保护。

默认 `lastDumpTime` 为 Unix 纪元，因此在正常现代系统时间下，首次累计超过阈值通常也满足时间条件。`SystemTime::duration_since` 在系统时钟回拨或传入未来时间时会报错；代码以 `unwrap_or_default()` 把该情况视为零时长，从而拒绝本次刷新但保留累计量。

`row_count` 没有非负校验，负值会减少累计量；上层必须保证它代表合法的已处理行数。计数使用 `i64`，发布构建中的溢出语义也不应被当作业务保护机制。`AnalyzeJob` 和 `AnalyzeProgress` 没有在本文件中实现 `Clone`、序列化或数据库映射。

## 依赖与调用关系

本文件只依赖标准库：`AtomicI64`/`Ordering`、`Mutex`、`Duration` 和 `SystemTime`，没有新增 `pkg/statistics/Cargo.toml` 的外部依赖。

模块边界为 `pkg/statistics/lib.rs -> analyze_jobs.rs -> std`。对外方面，`lib.rs` 的重导出使其他 crate 可用 `astersql_statistics::AnalyzeJob` 等符号；`pkg/statistics/handle/types/interfaces.rs::StatsAnalyze` 是已确认的直接类型消费者，`pkg/statistics/analyze.rs::AnalyzeResults::Job` 也持有 `Option<AnalyzeJob>`。

RustCodeGraph 将目标文件索引为 9 个符号，并报告文件级使用方包括 `pkg/statistics/histogram.rs`、`pkg/statistics/row_sampler.rs`、`pkg/statistics/index_test.rs`、`pkg/statistics/merge_global_test.rs` 和 `pkg/statistics/go_merge_47_test.rs` 等；但针对本文件 `Update`/访问器的 `callers`、`callees` 查询没有返回已解析调用边。精确文本检索确认当前 Rust 代码中 `Update`、`GetDeltaCount`、`SetLastDumpTime` 的直接行为测试位于 `pkg/statistics/statistics_test.rs`，未发现 Rust 生产调用点。因此，当前能证明的是公共模型和接口契约已经存在，不能证明本实现已贯穿完整 Rust ANALYZE SQL 执行链。

## 错误处理与边界

这些 API 不返回 `Result`。原子操作不会报告业务错误；`lastDumpTime.lock().unwrap()` 会在互斥锁曾被持有线程 panic 而中毒时继续 panic。时间回拨不会 panic，因为 `duration_since` 的错误被转换为零时长。

阈值和时间判断均为严格 `>`。`Update(0)` 仍会获取时间锁并检查已有累计量；如果已有累计量已超过阈值且时间满足，它可以触发刷新。负增量、整数溢出、无效 `JobType` 或空库表名均不在本文件校验范围内。

方法只把“应刷新多少”返回给调用方，无法知道外层持久化是否成功。它在返回前已经清零；因此外层 SQL 失败时，需要由生命周期层决定记录、补偿或容忍策略，不能假设计数仍留在本对象中。

## 并发与资源生命周期

`AtomicI64` 使用 `Ordering::SeqCst`，为计数读写提供最强的全局顺序；时间字段的所有访问都在同一 `Mutex` 下。对象不创建线程、任务、通道、文件句柄、事务或网络连接，释放时仅由 Rust 正常析构标准库字段。

计数累加发生在取得时间锁之前，而触发路径使用 `store(0)`，并非原子交换。若线程 A 得到 `new_count` 后等待/持有时间锁，线程 B 在 A 清零前又完成 `fetch_add`，A 随后的 `store(0)` 可能把 B 的新增量一并清掉，却只返回 A 较早观察到的 `new_count`。Go 对照也采用“原子 Add、加锁判断、原子 Store(0)”的同类顺序。扩展并发调用时必须保留现有兼容语义，或先用独立回归证明改为 `swap`/全程串行化不会改变统计和持久化行为。

`GetDeltaCount` 不获取时间锁，因此读取到的是调用时某一原子快照，不能与 `GetLastDumpTime` 组合成跨字段一致快照。`SetLastDumpTime` 只改时间，不影响计数，这一点已有 Rust 独立测试覆盖。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/statistics/analyze_jobs.go`。状态字符串、两个作业类型数值、10,000,000 行阈值、5 秒间隔、`AnalyzeJob` 字段集合和四个进度方法与 Go 版本逐项对应。

主要语言映射如下：Go 的 `time.Time` 对应 `SystemTime`，零值在 Rust 默认实现中显式选为 Unix 纪元；Go 的 `*uint64` 对应 `Option<u64>`；Go 命名整数类型在 Rust 中是 `i32` 别名；Go `atomic.Int64` 对应 `AtomicI64`；Go `sync.RWMutex` 对应普通 `Mutex<SystemTime>`，所以 Rust 的时间读取也独占锁而非共享读锁。Go 字段 `lastDumpTime` 的自然零时间早于 Unix 纪元，但二者在正常当前时间下都让首次大增量通过时间条件。

`Update` 的关键判定与重置顺序保持一致，包括严格大于、先原子累加、时间锁内清零和更新时间。Rust 对时钟回拨显式用零时长处理；Go 的 `t.Sub(lastDumpTime)` 为负数，同样不会满足 `> 5s`。

Go 生命周期实现位于 `pkg/statistics/handle/autoanalyze/autoanalyze.go`，并由 `pkg/executor/test/analyzetest/analyze_test.go` 覆盖完整数据库行为。Rust `pkg/statistics/handle/autoanalyze/autoanalyze.rs` 当前另有独立模型和小写方法，虽复刻相同节流参数和 SQL 语义，但不是本文件类型的直接调用证据；后续整合时应避免两套模型漂移。

## 扩展指南

- 新增作业状态或类型时，应先修改本文件相应常量，并同步检查 `pkg/statistics/analyze_jobs.go`、`StatsAnalyze` 接口、持久化 SQL、状态展示和清理条件；不要只增加字符串而遗漏生命周期转换。
- 修改 `AnalyzeJob` 字段时，应检查 `pkg/statistics/analyze.rs::AnalyzeResults` 的持有关系、Handle 接口和构造点，并保持 Go 字段语义及数据库列映射兼容。
- 修改阈值算法时，最直接接入点是 `AnalyzeProgress::Update`。测试应放在独立的 `pkg/statistics/statistics_test.rs`（或新建独立 `*_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 区域装配），不要把测试嵌入生产源文件。
- 至少补齐边界用例：累计量等于/刚超过阈值、间隔等于/刚超过 5 秒、时钟回拨、首次刷新、刷新后继续累计、负输入策略和多线程竞争。涉及数据库生命周期时还应与 Go 的 `pkg/executor/test/analyzetest/analyze_test.go` 场景对齐。
- 若把当前公共模型接入 `autoanalyze.rs`，应先决定如何消除或适配后者的重复类型，验证 SQL 失败后的计数语义，并关注互斥竞争与每行调用成本；不要以删除 Go 已有逻辑或简化生命周期换取接线通过。

## 验证依据

- RustCodeGraph：`status` 显示索引含目标仓库，目标文件被识别为 136 行、9 个符号；`files --filter pkg/statistics/analyze_jobs.rs` 确认文件入图；`node --file ...` 核对完整源码；`query AnalyzeJob`、`query AnalyzeProgress`、`query GetDeltaCount`、`query SetLastDumpTime`、`query GetLastDumpTime` 核对符号位置；对 `Update` 和访问器执行 `callers`/`callees` 未得到解析边，因此没有据此虚构生产调用。
- Rust 源与装配：`pkg/statistics/analyze_jobs.rs`、`pkg/statistics/lib.rs`、`pkg/statistics/analyze.rs`、`pkg/statistics/handle/types/interfaces.rs`、`pkg/statistics/handle/autoanalyze/autoanalyze.rs`。
- crate 边界：`pkg/statistics/Cargo.toml`，确认 crate 名、入口和目标文件仅使用标准库。
- Go 对照与生命周期：`pkg/statistics/analyze_jobs.go`、`pkg/statistics/handle/autoanalyze/autoanalyze.go`。
- 测试证据：`pkg/statistics/statistics_test.rs::analyze_progress_tracks_delta_and_persist_threshold`；完整 Go 行为参考 `pkg/executor/test/analyzetest/analyze_test.go` 的 ANALYZE 状态/进度场景。
- 文本补证：用 `rg` 精确检索模块声明、公开重导出、类型引用、四个进度方法和状态/类型常量，确认 Rust 直接行为测试及当前未发现生产方法调用点。
- 本任务是纯文档分析，按计划未运行 Cargo；结构验证单独执行并要求目标文档恰含上述 11 个固定二级标题。
