# `pkg/util/topsql/reporter/ru_window_aggregator.rs`

## 文件定位

本文件属于 `astersql-util-topsql-reporter` crate 的 RU（Request Unit）在线聚合层。`lib.rs` 以 `mod ru_window_aggregator` 装入模块并公开重导出；生产入口 `reporter.rs` 的 `RemoteTopSQLReporter` 持有一个 `RUWindowAggregator`，收集线程把 `RUBatch` 写入它，周期上报时再取出 `tipb::TopRuRecord`。因此它位于“语句 RU 增量 → 15 秒基础桶 → 60 秒闭合窗口 → TopRU protobuf”链路中，不负责采集 RU、注册 SQL/Plan 元数据或把报告发送给 sink。

crate 边界由 `pkg/util/topsql/reporter/Cargo.toml` 确认：本文件直接使用同 crate 的 `ru_datamodel`，使用 `topsql_stmtstats`（在 crate 中重导出为 `stmtstats`）承载输入，并使用启用 `protobuf-codec` 的 `tipb` 生成输出；迟到丢弃指标来自路径依赖 `reporter_metrics`。本模块没有 feature 条件编译项。

## 核心职责

1. `addBatch` 将批次时间戳向下对齐到 15 秒，校验 RU 版本和 handover 丢弃边界，并把增量累加到对应 `ruCollecting`。
2. `rotateBucketsBefore` 关闭已经完整越过边界的基础桶，以 `maxTopUsers` / `maxTopSQLsPerUser` 做第一次 Top-N 压缩，关闭后不再写入。
3. `takeReportRecords` 每个对齐的 60 秒窗口最多取一次；它移出该窗口的桶，再由 `buildReportRecords` 按 15、30 或 60 秒粒度重组、二次压缩并编码为 protobuf。
4. `resetForHandover` 在 RU 版本切换时清空旧状态，并跳过切换时尚未完整结束的窗口，避免跨版本混算。
5. 对已经上报窗口的迟到数据采用 best-effort：先移到最早仍可上报的窗口；若目标桶已被并发上报压缩，则丢弃并同时更新实例内原子计数与全局 Prometheus 计数器。

## 主要符号

- `ruBaseBucketSeconds = 15`、`ruReportWindowSeconds = 60`：基础桶和报告窗口的不变量。`ruReportTopNUsers = 100`、`ruReportTopNSQLsPerUser = 100`：每个输出时间片最终保留的用户/每用户 SQL 上限。
- `RUBatch { data, timestamp, version }`：公开输入载荷；`data` 是 `(用户, SQL digest, Plan digest) → RUIncrement`，另有兼容 Go 命名的 `ruBatch` 别名。
- `RUPointBucket`：内部桶状态。`collecting: Some` 表示仍可写；旋转后取走 `collecting`，将压缩快照置于 `compacted`。`start` 是已对齐的桶起点。
- `AggregatorState`：互斥锁内的可变状态，包括 `buckets`、首次有效批次或 handover 指定的 `currentVersion`、`dropUntilTs` 和防止重复报告的 `lastReportedEndTs`。
- `RUWindowAggregator`：线程安全外壳；`state: Mutex<AggregatorState>` 串行化窗口状态变化，两个 `AtomicU64` 分别保存迟到丢弃键数和 `f64` RU 的位模式。`ruWindowAggregator`、`newRUWindowAggregator` 保留 Go 风格调用面，`Default` 等价于 `new()`。
- `alignToInterval(timestamp, interval)`：向下对齐；间隔为零时原样返回，避免除零。
- `add_batch`、`reset_for_handover`、`take_report_records`：蛇形 API，分别只构造/转发到 Go 风格主方法。
- `addBatch`、`resetForHandover`、`dropReportData`、`takeReportRecords`：生产状态机的四个主要操作。
- `rotateBucketsBefore`、`buildReportRecords`：内部旋转和报告构造函数；后者不持有聚合器锁。
- `addAtomicF64`、`incrementLateCompactedMetrics`：用 CAS 累加实例 RU 丢弃量，并在指标已初始化时增加全局计数器。

## 执行流程

写入流程从 `RemoteTopSQLReporter::CollectRUIncrements` 开始：它将当前 Unix 秒和版本包装为 `RUBatch`，经有界 `collectRUTx` 非阻塞入队；`collectWorker` 收到后调用 `RUWindowAggregator::addBatch`。空 map 立即返回。非空批次先按 15 秒对齐，随后持锁：初始版本为零时以 `NormalizeRUVersion(batch.version)` 建立当前版本；版本不匹配，或桶起点早于 `dropUntilTs`，都会静默忽略。

正常批次先执行 `rotateBucketsBefore(state, bucketStart)`，使结束时间不晚于新桶起点的旧桶变成只读压缩快照。目标桶不存在时以 `maxPreTopNUsers` / `maxPreTopNSQLsPerUser` 容量创建 `ruCollecting`，再用桶起点作为统一时间戳调用 `ruCollecting::addBatch`。若批次属于已经上报的窗口，桶起点先改写为 `lastReportedEndTs`；目标桶仍开放即可进入下一报告窗口，若已压缩则释放锁并记录丢弃量。

报告流程由 `RemoteTopSQLReporter::takeDataAndSendToReportChan(timestamp)` 驱动。报告通道已满时，它调用 `dropReportData`：删除当前 60 秒边界之前的闭合桶、推进 `lastReportedEndTs`，但保留仍开放窗口。通道可用时调用 `takeReportRecords`：将 `now` 向下对齐为 `windowEnd`，不足首个 60 秒窗口或窗口已取过时返回空；否则旋转所有在边界前闭合的桶，移出 `[windowEnd-60, windowEnd)` 的四个基础桶，推进已报告边界并清理更旧桶，然后在锁外构建记录。

`buildReportRecords` 只接受 15/30/60 秒输出粒度，其他值回退为 60。它逐个输出时间片合并已压缩基础桶，合并时把 item 时间戳重写为该时间片起点，并以 `ruReportTopNUsers` / `ruReportTopNSQLsPerUser` 再压缩。单时间片窗口直接编码；多时间片窗口先汇入一个容量按时间片数放大的收集器，保留各时间片 item，最后统一调用 `toTopRURecords(keyspaceName)`。

## 数据与状态

`buckets: HashMap<u64, RUPointBucket>` 的键和 `RUPointBucket::start` 都是 15 秒对齐值。一个桶只有两种有效阶段：开放时 `collecting=Some, compacted=None`；旋转后 `collecting=None, compacted` 为压缩结果（空收集器时可以为 `None`）。`rotateBucketsBefore` 的关闭条件是 `start + 15 <= boundary`，所以与边界相交的桶仍可写。

`currentVersion == 0` 表示尚未由批次建立版本。handover 会直接设为新版本并清空桶；`dropUntilTs` 为切换时刻所在 60 秒窗口的起点，若切换不在整点则再加 60 秒。这使非整点切换跳过当前残缺窗口；整点切换则允许从该边界开始收集。`lastReportedEndTs` 单调不减，是“一窗口只报告一次”和迟到数据重映射的共同水位线。

容量控制分三层：开放桶先受 `maxPreTopNUsers` / `maxPreTopNSQLsPerUser` 约束；基础桶关闭时压到 `maxTopUsers` / `maxTopSQLsPerUser`；每个输出时间片再压到 100×100。被淘汰的用户或 SQL 不直接丢失 RU，而由 `ruCollecting::compactWithLimits` 合入 others 记录；`toTopRURecords` 将 others 用户写成 `othersUserWireLabel`，并按时间戳排序 item。

迟到丢弃 RU 以 `f64::to_bits` 存入 `AtomicU64`，`addAtomicF64` 通过弱 CAS 重试累加；`dropped_late_ru` 再用 `from_bits` 读取。该表示适合统计而不提供跨多个原子字段的一致快照。

## 依赖与调用关系

上游生产调用者集中在 `reporter.rs`：`RemoteTopSQLReporter` 构造时创建聚合器；`collectWorker` 调用 `addBatch`；`OnRUVersionChange` 调用 `resetForHandover`；`takeDataAndSendToReportChan` 在正常路径调用 `takeReportRecords`，在 report 通道背压路径调用 `dropReportData`。取得的 `RURecords` 与 CPU、语句统计及 SQL/Plan 元数据一起进入 `ReportData`，再由 report worker 发往 sink。

下游核心是 `ru_datamodel.rs`：`newRUCollectingWithCaps` 建立有界收集器，`addBatch` 累加，`compactWithLimits` 排序裁剪并把淘汰量归入 others，`mergeFrom` 在桶/输出粒度之间合并且可重写时间戳，`toTopRURecords` 生成 `tipb::TopRuRecord`。版本规范化和输入类型来自 `stmtstats`；全局迟到指标来自 `reporter_metrics::reporter_metrics`。

RustCodeGraph 的文件节点显示本文件直接被 `reporter.rs` 和 `ru_window_aggregator_test.rs` 使用；索引对同名 Go/Rust 方法的精确 method 查询存在解析歧义，因此生产调用边又由上述 `reporter.rs` 具体调用点交叉核验，而不是把模糊搜索结果当结论。

## 错误处理与边界

本模块没有 `Result` 返回面：空输入、旧版本、handover 丢弃区、尚未闭合窗口、重复 take 和无数据窗口均以无操作或空 `Vec` 表达。非法 `itemInterval` 在 Rust 中回退到 60 秒，避免零间隔造成循环不前进；公开契约仍应只传 15、30、60。

所有 `state.lock()` 都以 `expect("RU aggregator mutex poisoned")` 处理 poisoned mutex，说明线程 panic 后选择继续 panic，而非返回部分可信状态。`SystemTime`、通道错误及 sink 错误由上层 reporter 处理，不属于本文件。

迟到批次只有在重映射目标桶已经压缩时才真正丢弃；此时实例原子统计总会更新，全局 Prometheus 指标则仅在对应静态 counter 已初始化时更新。`Ordering::Relaxed` 只保证原子读改写，不提供与窗口状态的同步关系；窗口正确性由 mutex 保证。

时间戳和边界使用 `u64`。常规 Unix 秒范围不会接近溢出，但 `bucket.start + 15`、handover 边界加 60 以及窗口容量计算没有显式溢出恢复；调用者必须传合理时间值。Top-N 取舍由 RU 数据模型决定，HashMap 迭代次序不应被调用者当作最终记录顺序；只有单条记录内的 items 明确按时间排序。

## 并发与资源生命周期

`RUWindowAggregator` 的公开操作都接收 `&self`，可在 `Arc` 下由写入线程和报告线程并发调用。`Mutex<AggregatorState>` 覆盖版本判断、迟到重映射、桶旋转、桶增删和报告水位推进，使“关闭后不可再写”和“一窗口最多取一次”成为同一临界区内的不变量。

`takeReportRecords` 刻意在锁内只移走桶，在锁外执行 `buildReportRecords` 的多层合并和 protobuf 构造，缩短热点锁持有时间。`addBatch` 若命中已压缩迟到桶，也先 `drop(state)` 再遍历批次求和和更新指标。实例丢弃统计使用原子量，避免为了观测再次获取状态锁。

桶从创建、累加、压缩、被窗口取走到离开函数后释放，所有权路径明确；没有后台任务、异步 future、文件句柄或手工关闭资源。`dropReportData` 是背压时的显式回收路径：仅删除闭合窗口，保留当前开放窗口，以便下一个 tick 继续上报。

## 与 Go 版本的对应关系

同路径 `ru_window_aggregator.go` 是直接语义基准。Rust 的 `RUBatch`、`RUPointBucket`、`RUWindowAggregator` 及四个主方法分别对应 Go 的 `ruBatch`、`ruPointBucket`、`ruWindowAggregator`、`addBatch/resetForHandover/dropReportData/takeReportRecords`；15 秒桶、60 秒窗口、三层容量、迟到移位、handover 和窗口只取一次的规则一致。Rust 将 Go 结构体中分散的字段组合进 `AggregatorState`，将 Go 的指针 nil 状态表达为 `Option<Box<ruCollecting>>`，并保留 Go 风格别名/函数名以方便移植对照。

两端均在取桶后于锁外构建报告。Go 用方法 `takeBucketsForWindow` 和接收者方法 `rotateBucketsBefore` 拆分临界区；Rust 将前者内联进 `takeReportRecords`，将后者实现为接收 `&mut AggregatorState` 的私有函数，行为未简化。

可见差异有两点。第一，Go 的迟到丢弃只更新全局 Prometheus counter，Rust 除同步更新这些 counter 外还保留 `droppedLateKeys/droppedLateRUBits` 及读取方法，支持独立并发断言。第二，Go 注释要求 `itemInterval` 必须为 15/30/60，函数本身不修正非法值；Rust 显式将非法值回退到 60 秒。这是 Rust 的防御性边界，不应据此鼓励传入其他值。另因 Rust `RUIncrementMap` 的 value 是值类型，不存在 Go 侧可能出现的 nil increment 分支。

Rust 测试 `ru_window_aggregator_test.rs` 与 Go 测试 `ru_window_aggregator_test.go` 对齐验证报告粒度、基础桶压缩、单窗口单次 take、handover、背压保留开放窗口、并发压力、迟到移位/可观测丢弃、100×100 上限、稀疏桶和热点保留；Rust 还直接断言实例级丢弃计数。

## 扩展指南

- 修改时间窗口或合法输出粒度时，应同时审查 `alignToInterval`、`rotateBucketsBefore`、`takeReportRecords` 和 `buildReportRecords` 中的边界/步长/容量计算，并同步 `ru_window_aggregator_test.rs` 与 Go 对照测试。尤其要保持循环步长非零、闭区间语义为 `[start, end)`、同一窗口不可重复发出。
- 调整 Top-N 策略时，优先在 `ru_datamodel.rs` 的 `compactWithLimits`、`mergeFrom` 及容量常量处实现；本文件只负责何时以及用什么层级上限调用它们。必须继续证明 RU 总量经 others 汇总后守恒、热点键保留、用户与 SQL 上限成立。
- 改动版本切换时，应围绕 `currentVersion`、`dropUntilTs`、`lastReportedEndTs` 建模，补充整点/非整点切换和错误版本测试，避免旧版本进入新窗口或丢弃第一个完整新窗口。
- 改动迟到策略或锁范围时，要保留“报告与迟到写入竞态下，迟到 RU 等于已报告量加可观测丢弃量”的测试，并检查实例原子计数与两个全局指标仍同步。不要把昂贵的压缩/编码重新放回状态锁内。
- 新增测试必须放在独立的 `pkg/util/topsql/reporter/ru_window_aggregator_test.rs`（必要时同步 Go 测试），不要嵌入生产源文件。若新行为进入 reporter 主链，还应扩展 `reporter_test.rs` 的集成断言。
- 兼容性风险集中在 protobuf 时间戳粒度、others 标签、keyspace 透传、RU 版本过滤和窗口去重；性能风险集中在高基数批次遍历、预容量放大、锁竞争及报告构建时的克隆/合并。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/topsql/reporter/ru_window_aggregator.rs` 确认目标文件有 33 个符号并由 `reporter.rs`、`ru_window_aggregator_test.rs` 使用；`node --file ... --offset 1 --limit 400` 阅读了目标文件完整 368 行；另用 `query`/`explore` 查询 `RUWindowAggregator`、`addBatch`、`takeReportRecords`、`rotateBucketsBefore`、`buildReportRecords`，并用 `node` 核对 `ru_datamodel.rs` 的 `addBatch/compactWithLimits/mergeFrom/toTopRURecords`。
- 源码与装配：`pkg/util/topsql/reporter/ru_window_aggregator.rs`、`pkg/util/topsql/reporter/reporter.rs`、`pkg/util/topsql/reporter/ru_datamodel.rs`、`pkg/util/topsql/reporter/lib.rs`。
- crate 与指标：`pkg/util/topsql/reporter/Cargo.toml`、`pkg/util/topsql/reporter/metrics/metrics.rs`。
- Go 对照：`pkg/util/topsql/reporter/ru_window_aggregator.go`、`pkg/util/topsql/reporter/reporter.go`、`pkg/util/topsql/reporter/ru_window_aggregator_test.go`。
- Rust 独立测试：`pkg/util/topsql/reporter/ru_window_aggregator_test.rs`；补充生产链断言见 `pkg/util/topsql/reporter/reporter_test.rs`。这些既有测试是行为证据，本次纯文档任务按计划未运行 Cargo。
- 人工复核结论：文件存在是为了隔离在线 RU 窗口状态机与 reporter 通道/sink；运行方式是写入对齐桶、逐层压缩、闭窗取出；安全扩展必须维护版本/窗口水位、锁内状态不变量、others 总量守恒和独立测试边界。
