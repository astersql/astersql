# `pkg/util/topsql/reporter/reporter.rs`

## 文件定位

本文件实现远程 TopSQL reporter，是 `astersql-util-topsql-reporter` crate 中连接“进程内采集”和“远端数据接收端”的核心编排层。crate 根 `pkg/util/topsql/reporter/lib.rs` 以公开模块 `reporter` 挂载本文件，并重导出 `NewRemoteTopSQLReporter`、`RemoteTopSQLReporter` 与 `findKthNetworkBytes`；`pkg/util/topsql/reporter/Cargo.toml` 则声明它直接依赖 collector、stmtstats、state、tipb protobuf、reporter metrics 和 `crossbeam-channel`。

应用侧入口位于 `pkg/util/topsql/topsql.rs`：`default_pipeline` 构造 `RemoteTopSQLReporter`，注入 plan 解码与压缩函数，再把它包装成包级 `TopSQLReporter`。SQL 执行路径通过该包级门面注册 SQL/Plan 元数据，stmtstats 聚合器提交语句统计与 RU 增量，CPU collector 提交 SQL CPU 采样；本文件将这些输入周期性合并成统一 protobuf 载荷并发送给已注册 `DataSink`。

## 核心职责

1. `NewRemoteTopSQLReporter` 创建有界采集/上报通道、聚合状态、sink 注册器和弱引用 CPU collector，避免 reporter 与 collector 形成强引用环。
2. `Start` 启动 CPU collector，以及独立的 `collectWorker`、`reportWorker` 两个后台线程；`Close` 负责幂等取消、停止 collector、等待线程退出并关闭全部 sink。
3. `Collect`、`CollectStmtStatsMap`、`CollectRUIncrements` 为低延迟入口：空批次直接返回，非空批次用 `try_send` 非阻塞入队，拥塞时丢弃并累计指标。
4. `processCPUTimeData` 和 `processStmtStatsData` 以 CPU TopN、网络字节阈值及 CPU 淘汰标记共同决定保留明细还是汇入空 digest 的 `others` 记录。
5. `takeDataAndSendToReportChan` 在 report tick 上提取 CPU/Stmt、RU、SQL meta 与 Plan meta；`doReport`/`trySend` 将同一 `SinkReportData` fan-out 给全部 sink。
6. `RegisterSQL`、`RegisterPlan`、`BindKeyspaceName`、`BindProcessCPUTimeUpdater` 和 `DataSinkRegisterer`/collector trait 实现，构成上层包与本 reporter 之间的适配边界。

## 主要符号

- 常量 `reportTimeout`、`collectChanBufferSize`、`reportCollectedDataChanSize`：分别规定单次 sink 截止时间为 40 秒、三个采集通道容量为 2、报告通道容量为 2。
- 回调类型 `planBinaryDecodeFunc`、`planBinaryCompressFunc`：在报告组装阶段处理普通 plan 与大 plan，均返回 `Result<String, String>`。
- `TopSQLKey`：以 SQL digest 和 Plan digest 组成聚合键；默认空键专门表示 `others`。
- `TopSQLRecordItem` / `TopSQLRecord`：表示单时间点 CPU+语句统计，以及按 digest 聚合的时间序列；报告前补入当前 keyspace。
- `TopSQLCollecting`：内部 `records` 是 `TopSQLKey -> timestamp -> item`，`evicted` 记录某时间戳已被 CPU TopN 淘汰的键；`take` 转出记录并清空收集状态。
- `SQLMeta` / `PlanMeta`：分别保存 digest、规范化文本、内部 SQL 标志或编码 plan，以及 keyspace。
- `ReportData`：reporter 内部批次，汇集 TopSQL、TopRU、SQL meta 和 Plan meta；`to_sink_report_data` 将其转换成 `datasink::ReportData` 的 protobuf 集合。`from_sink_report_data` 仅在 `cfg(test)` 下存在。
- `MetaState`：保存尚未随报告取走的 SQL/Plan 元数据；同 digest 采用首次注册值。
- `ReporterMetrics`：以原子计数器记录 CPU、Stmt、RU、report 四类通道满丢弃次数。
- `WeakCPUCollector`：实现 `collector::Collector`，升级弱引用成功后才转发到 reporter。
- `RemoteTopSQLReporter`：持有四组数据通道、取消通道、互斥保护的聚合/元数据/keyspace 状态、`RUWindowAggregator`、sink 注册器、plan 回调、CPU collector、线程句柄与幂等状态位。
- `findKthNetworkBytes`：计算网络输入加输出的第 K 大阈值；数据量不超过 K 时返回 0。

## 执行流程

构造阶段，`NewRemoteTopSQLReporter` 创建容量固定的 crossbeam 通道，初始化各状态，并把 `Arc::downgrade(&reporter)` 包进 `WeakCPUCollector` 后交给 `collector::NewSQLCPUCollector`。调用 `Start` 时，`started.swap(true, SeqCst)` 保证只启动一次，随后启动 CPU collector，并各自持有 reporter `Arc` 运行收集线程和报告线程。

采集阶段有三条输入路径：

1. `Collect` 接收 `SQLCPUTimeRecord`；`collectWorker` 收到后以消费时的 Unix 秒调用 `processCPUTimeData`。后者按 `CPUTimeMs` 降序，只保留 `GlobalState.MaxStatementCount` 条，淘汰项按时间戳记入 `evicted`，CPU 总和累入空键。
2. `CollectStmtStatsMap` 接收语句统计；worker 以消费时刻暂存到 `stmtStatsBuffer[timestamp]`。report tick 到来后，`processStmtStatsData` 计算每批第 K 大网络字节阈值。网络流量严格大于阈值，或同时间戳未被 CPU 路径淘汰的键，保留原 digest；其余语句统计汇入空键。各数值字段使用 wrapping 加法，与现有迁移实现一致。
3. `CollectRUIncrements` 在生产者入口记录 Unix 秒并封装 `RUBatch`；worker 调用 `RUWindowAggregator::addBatch`。`OnRUVersionChange` 则直接以当前时间调用 `resetForHandover`，清理版本切换边界状态。

每个固定 report tick 先处理 stmtstats，再调用 `takeDataAndSendToReportChan`。若 report 通道已满，本轮 CPU/Stmt 收集状态会重置，RU 聚合器执行 `dropReportData`，元数据仍保留以便背压恢复后继续解释 digest；否则提取一个 RU 报告窗口、取走 TopSQL 记录和元数据、补入 keyspace，并对 plan 执行解码或压缩。普通 plan 解码失败、大 plan 压缩失败时保留原字符串到 `NormalizedPlan`，不会中止整个批次。

`reportWorker` 收到批次后等待 100 ms，再由 `doReport` 跳过完全空的批次，将内部模型转换为统一 wire 模型，并以 `Instant::now() + reportTimeout` 调用 `trySend`。`trySend` 获取 sink 快照，对每个 sink 传递同一 `Arc<SinkReportData>`；一个 sink 失败只记录警告，不阻止后续 sink。

## 数据与状态

- CPU 与语句统计最终共享 `TopSQLCollecting`。`BTreeMap<u64, TopSQLRecordItem>` 保证每条记录的时间点按时间戳输出；`HashSet<(timestamp, key)>` 只在本轮收集期内保存淘汰事实，`take` 后清空。
- `stmtStatsBuffer` 按消费时刻覆盖同一秒的后一次 `StatementStatsMap`，在 `processStmtStatsData` 中整体 `take`，因此处理与新到批次分离。
- SQL/Plan meta 分别受 `GlobalState.MaxCollect` 限制。已存在 digest 不受容量检查影响，但 `or_insert` 保留首次注册内容；成功组装报告时通过 `mem::take` 转移所有 meta。
- keyspace 是 reporter 级状态，组装每批报告时克隆到 TopSQL、TopRU 和 meta。它不是每条输入携带的快照，因此以报告阶段读取到的当前值为准。
- RU 状态由 `RUWindowAggregator` 自行管理窗口、版本交接和迟到数据；本文件只负责带时间戳入队、按 `GetTopRUItemInterval` 取记录，以及背压时通知丢弃窗口。
- `started`、`closed` 分别保证启动和关闭幂等；`channelDropCounts` 只提供进程内累计观察值，不会重置计数器。

## 依赖与调用关系

上游直接关系：

- `pkg/util/topsql/topsql.rs::default_pipeline` 调用 `NewRemoteTopSQLReporter`，并在其 `TopSQLReporter` trait 实现中转发启动、关闭、SQL/Plan 注册、Stmt/RU 收集与版本切换。
- `pkg/util/topsql/stmtstats/aggregator.rs` 通过 `RUCollector::CollectRUIncrements` 与 `OnRUVersionChange` 提交 RU 聚合结果和版本交接事件。
- 包级 `RegisterSQL`/`RegisterPlan` 最终转发到本文件；例如 `pkg/session/runtime/scan_adapter_runtime.rs` 通过 `astersql_util_topsql` 门面注册扫描执行中的 SQL 与 plan。
- `collector::SQLCPUCollector` 通过 `WeakCPUCollector::Collect` 回调 `RemoteTopSQLReporter::Collect`。

下游直接关系：

- `crate::ru_window_aggregator::RUWindowAggregator` 接收 `RUBatch`，并在报告 tick 提供 `tipb::TopRuRecord`。
- `crate::datasink::DefaultDataSinkRegisterer` 管理 `DataSink` 集合；`trySend` 调用每个 sink 的 `try_send`，`Close` 调用 registerer 的 `close`。
- `crate::stmtstats` 提供语句统计、RU map/version 与相应 collector traits；`crate::collector` 提供 CPU 记录、CPU collector 和进程 CPU updater。
- `crate::tipb_protobuf` 提供 TopSQL、SQL meta、Plan meta protobuf；`topsqlstate` 提供 TopN、meta 容量和 RU 间隔等动态配置。
- `reporter_metrics` 仅在 report 通道背压路径增加全局忽略计数；四类本地原子计数由 `channelDropCounts` 暴露给测试与诊断。

RustCodeGraph 的文件级关系显示本文件被 `pkg/util/topsql/reporter/ru_window_aggregator.rs` 和 `pkg/util/topsql/reporter/single_target_test.rs` 引用；对外重导出和更多调用者由 `lib.rs`、`topsql.rs` 及精确源码搜索确认。

## 错误处理与边界

- 三个采集入口和报告入队都采用 best-effort 策略：空输入忽略，满通道不阻塞调用者；丢弃通过原子指标观测。报告通道预检查满时会主动丢弃本轮 CPU/Stmt/RU 数据，但故意保留有容量上限的 SQL/Plan meta。
- `MaxStatementCount`、`MaxCollect` 先与 0 取最大值再转 `usize`。TopN 为 0 时所有 CPU 进入 `others`；`findKthNetworkBytes(k=0)` 在非空 map 上返回最大网络字节，因筛选使用严格 `>`，CPU 已淘汰项通常会进入 `others`。
- 计数累加使用 `wrapping_add`，溢出时按整数环绕，而非报错或饱和；文档或调用方不能假设它会拒绝异常大累计值。
- mutex poison 统一通过 `expect` 触发 panic；worker 本身没有 Rust 侧 panic 恢复层。这与 Go 版 worker 使用 `util.Recover` 的容错方式不同，是扩展时需特别评估的边界。
- `unixNow` 在系统时间早于 Unix epoch 时返回 0。时间来自墙钟而非单调时钟，报告截止时间才使用 `Instant`。
- plan 解码/压缩错误被降级为原始 normalized plan；sink 错误只记录日志，`trySend` 仍返回 `Ok(())`，调用者无法从返回值获知部分或全部 sink 失败。
- `ReportData::hasData` 任一载荷非空即上报，因此即使没有 TopSQL/RU 记录，单独 SQL/Plan meta 也会发送。

## 并发与资源生命周期

`RemoteTopSQLReporter` 预期通过 `Arc` 共享。采集发送端可由多个线程同时调用；通道为有界 MPMC，热点路径不等待锁外的下游处理。聚合、stmt buffer、meta、keyspace、CPU collector 和 worker 句柄分别由独立 `Mutex` 保护，RU aggregator 自行提供线程安全内部状态；丢弃计数使用 relaxed 原子，幂等生命周期位使用顺序一致原子。

`collectWorker` 是 CPU/Stmt/RU 接收端和 `reportTx` 的唯一正常发送者，使“检查 report 通道是否已满，再发送”的背压约定成立。`reportWorker` 串行消费报告，100 ms 等待继承自 Go 版用来避开并发元数据写入的历史约定；Rust 当前已通过 `mem::take` 转移 meta，这段等待仍保留语义对齐。

`Close` 首次调用向容量 2 的取消通道发送两次，分别唤醒两个 worker；随后停止并取走 CPU collector，逐一 `join` worker，最后清空并关闭 sink。重复调用由 `closed` 直接返回。构造后未调用 `Start` 也可 `Close`，此时没有 worker 句柄需要等待。`WeakCPUCollector` 确保 reporter 释放后 collector 回调不会延长 reporter 生命周期。

## 与 Go 版本的对应关系

Rust 文件直接移植自 `pkg/util/topsql/reporter/reporter.go`，核心结构保持一致：三个容量为 2 的采集通道、容量为 2 的报告通道、CPU TopN 和 `others`、网络第 K 大阈值、固定 tick、RU 窗口、meta 延迟处理、100 ms report 延迟、40 秒 sink deadline、fan-out 和关闭通知。

已确认的实现差异如下：

- Go 以 `context.CancelFunc` 取消 goroutine；Rust 用两次取消通道发送并 `join` 两个线程，因此 Rust `Close` 等待线程结束。
- Go 的 `Start`/`Close` 本身没有显式幂等位；Rust 增加 `started`/`closed` 原子保护。
- Go 通过独立 `normalizedSQLMap`/`normalizedPlanMap` 类型处理容量与 protobuf 转换；Rust 合并为 `Mutex<MetaState>`，在组装阶段显式构造 `SQLMeta`/`PlanMeta`。
- Go `findKthNetworkBytes` 使用 quickselect；Rust 对 scratch 全量降序排序。结果规则相同，但 Rust 是 `O(n log n)`，大 map 下可能有额外 CPU 成本。
- Go worker 和处理函数用 `util.Recover`；Rust mutex poison 会 panic，worker 没有等价恢复。
- Go `doReport` 在 failpoint 下可缩短 timeout；Rust 固定使用 40 秒，当前没有该测试钩子。
- Rust sink 层使用公开 `datasink::ReportData`，让 PubSub 与 SingleTarget 共享统一 protobuf 载荷；`reporter_2_aster_unit_test.rs` 验证同一 reporter 向两种真实 sink fan-out。

独立 Rust 测试 `pkg/util/topsql/reporter/reporter_test.rs` 基本对应 `reporter_test.go` 的收集、TopN、容量、内部 SQL、多 sink、worker、stmtstats、背压和 TopRU 场景；`reporter_2_aster_unit_test.rs` 增补 RU 并发、迟到数据、公开 sink 注册和统一 fan-out 等迁移边界。

## 扩展指南

- 新增采集维度时，应在 `RemoteTopSQLReporter` 增加独立通道/状态，在 `collectWorker` 增加接收分支，在 `ReportData::to_sink_report_data` 补齐 wire 转换，并同步 `datasink::ReportData`、tipb 模型和独立测试；不要在调用者热点路径执行解码、排序或网络 I/O。
- 调整 TopN/others 规则时，主要修改 `processCPUTimeData`、`processStmtStatsData` 和 `findKthNetworkBytes`，同时更新 `reporter_test.rs` 中 TopN、eviction、stmtstats 交互测试及对应 Go 测试意图。特别注意阈值比较是严格大于、相同网络值可能少于 N 条，以及 CPU 淘汰标记按时间戳隔离。
- 改变背压策略时，应同时审查 `takeDataAndSendToReportChan` 对 CPU/Stmt、RU、meta 的不同保留策略，保证生产者不被阻塞，并扩展 `go_merge_39_report_backpressure_preserves_metadata_and_discards_samples`、通道满和 blocking sink 场景。
- 扩展 plan 处理时，应保持普通 plan 与大 plan 的字段互斥约定及失败回退，并在 `reporter_test.rs` 覆盖成功/失败的 decode/compress；CPU 密集工作应继续留在报告阶段。
- 增加 sink 时优先实现 `DataSink` 并通过 `Register` 接入，避免在 reporter 内建立特例。必须验证单 sink 失败不会妨碍其他 sink，以及 `Close` 会通知并释放它。
- 生命周期或线程模型变更应覆盖未启动关闭、重复 Start/Close、worker panic、取消时通道已有数据等边界。Rust 单元测试必须继续放在独立测试文件，不能嵌入 `reporter.rs`；本 crate 现有入口是 `reporter_test.rs` 与迁移补充 `reporter_2_aster_unit_test.rs`。
- 若追求与 Go 更严格的性能对齐，可评估用选择算法替换 `findKthNetworkBytes` 的全排序；这属于行为/性能代码变更，必须以重复值和 `k=0`/`len<=k` 回归测试证明阈值语义不变。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`node --file pkg/util/topsql/reporter/reporter.rs --offset 1 --limit 1200` 返回目标文件完整 867 行、全部符号与文件级引用。图查询对 Go/Rust 同名 `RemoteTopSQLReporter` 存在消歧限制，因此未把缺失的方法边当作事实，改以精确源码搜索补齐。
- 目标实现：`pkg/util/topsql/reporter/reporter.rs`，重点核对 `NewRemoteTopSQLReporter`、`Start`、三个 Collect 入口、`processCPUTimeData`、`processStmtStatsData`、`takeDataAndSendToReportChan`、`doReport`、`trySend`、`Close` 和 `findKthNetworkBytes`。
- crate 边界：`pkg/util/topsql/reporter/Cargo.toml` 与 `pkg/util/topsql/reporter/lib.rs`，核对依赖、模块可见性、重导出和独立测试挂载方式。
- 应用入口与直接调用者：`pkg/util/topsql/topsql.rs`、`pkg/util/topsql/stmtstats/aggregator.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`。
- 下游实现：`pkg/util/topsql/reporter/datasink.rs`、`pkg/util/topsql/reporter/ru_window_aggregator.rs`、`pkg/util/topsql/reporter/single_target.rs`。
- Go 对照：`pkg/util/topsql/reporter/reporter.go`；测试对照：`pkg/util/topsql/reporter/reporter_test.go`。
- Rust 独立测试：`pkg/util/topsql/reporter/reporter_test.rs`、`pkg/util/topsql/reporter/reporter_2_aster_unit_test.rs`、`pkg/util/topsql/reporter/topru_case_runner_test.rs`、`pkg/util/topsql/reporter/single_target_test.rs`。
- 本任务为纯文档分析，依计划不运行 Cargo；交付验证只检查目标文档存在且固定章节恰为 11 个，并人工复核上述符号、调用边、边界和 Go/Rust 差异均有直接源码依据。
