# `pkg/statistics/handle/autoanalyze/priorityqueue/calculator.rs`

## 文件定位

本文件是 `astersql-statistics-handle-autoanalyze-priorityqueue` crate 中的自动 ANALYZE 作业打分组件。crate 入口 `pkg/statistics/handle/autoanalyze/priorityqueue/lib.rs` 将 `calculator` 声明为内部模块并公开重导出其符号；`Cargo.toml` 将该 crate 映射到 Go 包 `pkg/statistics/handle/autoanalyze/priorityqueue`，且本文件本身不引入该 crate 唯一的外部工作区依赖 `astersql-statistics-handle-logutil`。

它位于调度链的“作业指标已经形成、作业即将进入最大堆”这一段：`queue.rs::AnalysisPriorityQueue::push_locked` 调用 `PriorityCalculator::CalculateWeight`，再通过 `AnalysisJob::SetWeight` 把结果写回作业，最后交给 `PqHeapImpl::AddOrUpdate`。因此本文件决定不同作业的相对优先级，但不负责采集指标、维护堆、执行 ANALYZE 或启动后台任务。

## 核心职责

- 用 `AnalysisJob::GetIndicators` 提供的变化比例、表规模和距上次分析时长计算一个 `f64` 权重；权重越高，越早从 `heap.rs` 的最大堆中 `Peek`/`Pop`。
- 用 `AnalysisJob::HasNewlyAddedIndex` 识别新增索引事件，并给予固定的 `2.0` 加分，使这类作业明显前移。
- 保持 Go `calculator.go` 的公式、权重常量、构造入口及导出命名，便于两种实现逐项对照。

本文件不是归一化器或合法性校验器：它直接对调用方提供的浮点指标套用公式，也不保证最终权重为正数、有限值或非 NaN。生产调用链应提供符合业务含义的非负指标。

## 主要符号

- `EVENT_NONE: f64 = 0.0`：没有特殊事件时的附加值。
- `EVENT_NEW_INDEX: f64 = 2.0`：作业包含新索引时的固定附加值。
- `CHANGE_RATIO_WEIGHT: f64 = 0.6`：变化比例项的系数。
- `SIZE_WEIGHT: f64 = 0.1`：表规模项的系数；规模经对数变换后以负向贡献进入总分。
- `ANALYSIS_INTERVAL: f64 = 0.3`：距上次分析时长项的系数。该名称与 Go 的 `analysisInterval` 一致，实际存放的是权重系数而不是一个时间值。
- `PriorityCalculator`：无字段的零尺寸类型，派生 `Default`，只承载打分方法，不保存配置或历史状态。
- `NewPriorityCalculator() -> PriorityCalculator`：返回一个新的无状态计算器。它使用 Go 风格名称，并由 crate 级 `#![allow(non_snake_case)]` 允许。
- `PriorityCalculator::CalculateWeight(&self, job: &dyn AnalysisJob) -> f64`：主入口，从 trait 对象读取指标并计算总权重。
- `PriorityCalculator::GetSpecialEvent(&self, job: &dyn AnalysisJob) -> f64`：根据是否存在新增索引返回两种固定事件分值；公开主要是为了与 Go API 和测试方式一致。

文件没有泛型、宏、条件编译项、可变静态量或私有辅助函数。

## 执行流程

`CalculateWeight` 的流程如下：

1. 调用 `job.GetIndicators()`，取得一个按值返回的 `Indicators` 快照。
2. 将 `ChangePercentage` 乘以 `100`，得到公式使用的 `change_ratio`。随后计算 `0.6 * log10(1 + change_ratio)`；变化比例越大，这一项越大。
3. 计算 `0.1 * (1 - log10(1 + TableSize))`；在其他指标不变时，表越大，该项越小，从而降低大表优先级。
4. 将 `LastAnalysisDuration` 通过 `AnalysisDuration::as_secs_f64` 转为秒，先开平方，再计算 `0.3 * log10(1 + sqrt(seconds))`；距上次分析越久，该项越大，平方根和对数共同抑制增长速度。
5. 调用 `GetSpecialEvent`。若 `job.HasNewlyAddedIndex()` 为真，加 `EVENT_NEW_INDEX`，否则加 `EVENT_NONE`。
6. 返回四项之和，不修改作业。调用方 `queue.rs::push_locked` 负责调用 `SetWeight` 并入堆。

除首次入堆外，`queue.rs::RefreshLastAnalysisDuration` 会只替换现有作业指标中的 `LastAnalysisDuration`，再次调用 `CalculateWeight`，再用 `PqHeapImpl::Update` 恢复堆序。由此，时间项会随后台刷新影响既有作业的相对位置。

## 数据与状态

输入来自 `job.rs`：

- `Indicators::ChangePercentage: f64` 表示相对上次分析的数据变化比例。
- `Indicators::TableSize: f64` 表示供公式使用的表规模。
- `Indicators::LastAnalysisDuration: AnalysisDuration` 是与 Go `time.Duration` 对齐的有符号纳秒值；本文件经 `as_secs_f64` 按秒使用。
- `AnalysisJob::HasNewlyAddedIndex()` 抽象不同作业类型判断“新增索引”的方式；计算器不读取具体索引集合。

`PriorityCalculator` 自身没有状态，所有常量均为编译期 `f64`。一次调用只持有局部 `Indicators` 副本和中间浮点值，不缓存结果，也不改变传入的 `AnalysisJob`。权重的持久化位置是具体作业实现的 `Weight` 字段，其写入由队列调用链完成。

## 依赖与调用关系

直接下游依赖只有 `crate::job::AnalysisJob` 及其三个读取接口：`GetIndicators`、`HasNewlyAddedIndex`，以及 `Indicators::LastAnalysisDuration.as_secs_f64()`；数学运算使用 Rust `f64` 的固有 `sqrt` 和 `log10`，没有错误返回。

主要上游调用边为：

- `queue.rs::NewAnalysisPriorityQueue` → `NewPriorityCalculator`：创建队列时把计算器包装为 `Arc<PriorityCalculator>`。
- `queue.rs::AnalysisPriorityQueue::push_locked` → `CalculateWeight`：所有通过重建、Push、DML 刷新或失败重试进入堆的作业，在 `SetWeight` 和 `PqHeapImpl::AddOrUpdate` 之前打分。
- `queue.rs::AnalysisPriorityQueue::RefreshLastAnalysisDuration` → `CalculateWeight`：刷新时间指标后重新打分，再更新堆。
- `CalculateWeight` → `GetSpecialEvent` → `AnalysisJob::HasNewlyAddedIndex`：在连续指标得分之上加入离散事件分值。
- `heap.rs::PqHeapImpl::less` → `AnalysisJob::GetWeight`：消费计算结果，以更大权重作为更高堆优先级。

RustCodeGraph 的文件节点还把 `calculator_test.rs`、`calculatoranalysis/calculator_analysis_test.rs` 和 `job_test.rs` 标为文件使用者；其中前两者直接构造/调用计算器，`job_test.rs` 通过 crate 重导出和共享作业类型形成文件级关联，不应据此误判为生产调用者。

## 错误处理与边界

本文件没有 `Result`、显式报错或 panic 分支。`&dyn AnalysisJob` 保证对象引用有效，但 trait 契约在类型层面不限制指标取值，因而需要注意浮点定义域：

- `1 + 100 * ChangePercentage` 小于 `0` 时，`log10` 产生 NaN；等于 `0` 时产生负无穷。
- `1 + TableSize` 小于 `0` 时产生 NaN；等于 `0` 时，规模项会因减去负无穷而成为正无穷。
- `LastAnalysisDuration` 为负时，`sqrt` 产生 NaN。虽然 `AnalysisDuration` 可表示 Go 风格负时长，正常调度指标的业务含义要求“距上次分析时长”非负。
- 极大、非有限或 NaN 输入会按 IEEE 754 传播；计算器不会钳制、排序兜底或记录日志。NaN 进入堆比较会破坏“权重全序”的业务假设，因为 `heap.rs::less` 直接使用 `>`。

对正常的非负有限输入，三个连续项分别满足：变化比例单调增加、表规模单调降低、分析间隔单调增加。`calculator_test.rs::TestCalculateWeight` 验证这些方向及一个“刚分析过的高变化表不应压过间隔更久的表”的组合场景；它没有覆盖非法输入或精确数值等式。

## 并发与资源生命周期

`PriorityCalculator` 无字段、无内部可变性、无锁、无 I/O，也不创建线程、任务、通道或堆分配资源。`CalculateWeight` 和 `GetSpecialEvent` 都只共享借用 `self` 与 `job`，因此计算阶段没有自身的资源清理或状态竞争问题。

并发边界由上游管理：`AnalysisPriorityQueue` 把计算器放在 `Arc` 中，并在持有 `QueueState` 的 `MutexGuard` 时从 `push_locked` 或刷新流程调用它。计算器只读取 `AnalysisJob`；作业权重的修改和堆更新仍发生在队列锁保护的调用方中。本文件不延长 job 引用生命周期，也不保存回调或跨线程句柄。

## 与 Go 版本的对应关系

Rust `calculator.rs` 与同路径 `calculator.go` 是直接移植关系：

- `EVENT_NONE`/`EVENT_NEW_INDEX` 对应 Go `EventNone`/`EventNewIndex`，数值均为 `0.0`/`2.0`。
- 三个权重系数均为 `0.6`、`0.1`、`0.3`；Go 注释仍将其标为待配置项，Rust 当前同样是固定常量，没有配置接线。
- `PriorityCalculator` 在两边都无状态；Go 构造器返回指针，Rust 返回零尺寸值，队列再以 `Arc` 共享。
- `CalculateWeight` 的四项公式和执行顺序一致：变化比例先乘 `100`，表规模负向，对时长秒数先开平方再取对数，最后加特殊事件。
- Go 使用 `time.Duration.Seconds()`，Rust 使用 `AnalysisDuration::as_secs_f64()`；后者在 `job.rs` 中按整数秒与余纳秒拆分转换，以保持 Go 的表示语义。
- Go 接口值参数对应 Rust `&dyn AnalysisJob` 借用，避免转移作业所有权；两边均不在计算器内写入作业。

测试也保持同路径意图：`calculator_test.rs` 对应 `calculator_test.go` 的单调性和事件分值用例，并额外包含 `calculator_rewards_new_index_and_increasing_change_ratio` 轻量回归；`calculatoranalysis/calculator_analysis_test.rs` 用真实计算器生成并按权重降序排序数据，再与两种实现共享意图的 `testdata/calculated_priorities.golden.csv` 比较。

## 扩展指南

- 修改公式、系数或新增指标时，首要修改点是 `CalculateWeight` 及顶部常量；同时确认 `job.rs::Indicators` 的生产者、三个具体作业实现和刷新路径都能提供该指标，不能只改计算器。
- 新增特殊事件时，应扩展 `GetSpecialEvent` 及 `AnalysisJob` 可表达的事件信息，并同步检查非分区、静态分区、动态分区三类作业的实现。若事件可能叠加，需要明确从当前二选一固定值改成何种组合规则。
- 若把系数改成可配置项，`PriorityCalculator` 将不再是无状态零尺寸类型；需同步调整 `NewPriorityCalculator`、`AnalysisPriorityQueue.calculator` 的构造/共享方式，并考虑运行时配置更新后对堆内旧权重的全量重算。
- 任何会改变排序的修改，都应同时更新独立 Rust 测试 `calculator_test.rs` 和 Go 对照测试 `calculator_test.go`；涉及整体权重分布时还应更新并审查 `calculatoranalysis/testdata/calculated_priorities.golden.csv`，而不是在生产源文件中内嵌测试。
- 建议新增非法输入测试后再引入校验或钳制策略，尤其覆盖负时长、负表规模、NaN 与无穷值；这会改变当前“直接传播 IEEE 754 结果”的行为，必须评估 Go 兼容性和堆排序影响。
- 性能上当前每次打分包含三个 `log10` 和一个 `sqrt`。若把计算放到更高频路径或增加更多变换，应基于队列规模评估成本；不要通过缓存忽略 `LastAnalysisDuration` 会定期变化这一事实。

## 验证依据

- RustCodeGraph 状态：本地索引包含 `calculator.rs`，文件节点展示其 64 行源码、5 个符号，并报告文件级使用者 `queue.rs`、`calculator_test.rs`、`calculatoranalysis/calculator_analysis_test.rs` 与 `job_test.rs`。按名查询确认 Rust/Go 两侧均存在 `PriorityCalculator`、`NewPriorityCalculator`、`CalculateWeight` 和 `GetSpecialEvent`。精确 `callers`/`callees` 子命令在本地索引上未返回调用边且达到超时，因此具体调用边由同一索引的 `queue.rs`、`job.rs`、`heap.rs` 文件节点逐行核对。
- 生产 Rust 证据：`calculator.rs::{CalculateWeight, GetSpecialEvent}`；`job.rs::{AnalysisDuration::as_secs_f64, Indicators, AnalysisJob}`；`queue.rs::{NewAnalysisPriorityQueue, push_locked, RefreshLastAnalysisDuration}`；`heap.rs::{PqHeapImpl::less, AddOrUpdate, Update, Pop}`；`lib.rs` 的模块声明与公开重导出。
- crate 证据：`pkg/statistics/handle/autoanalyze/priorityqueue/Cargo.toml` 的包名、`lib.rs` 入口、唯一工作区依赖和 `package.metadata.porting.go-package`。
- Go 对照：`calculator.go::{PriorityCalculator, NewPriorityCalculator, CalculateWeight, GetSpecialEvent}` 及相同的五个常量/系数语义。
- 测试证据：`calculator_test.rs::{TestCalculateWeight, TestGetSpecialEvent, calculator_rewards_new_index_and_increasing_change_ratio}`；Go `calculator_test.go::{TestCalculateWeight, TestGetSpecialEvent}`；批量分布证据 `calculatoranalysis/calculator_analysis_test.rs::calculate_new_priorities` 与共享 golden CSV。
- 本任务只新增说明文档，没有修改 Rust、Go、Cargo 或运行时行为；按任务约束不运行 Cargo。结构完整性由任务指定命令验证，人工复核重点为公式、真实调用链、输入边界、Go 差异和独立测试位置。
