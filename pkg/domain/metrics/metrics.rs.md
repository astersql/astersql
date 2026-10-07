# `pkg/domain/metrics/metrics.rs`

## 文件定位

本文件是 `astersql-domain-metrics` crate 的 Domain 专用指标绑定层：它不创建或注册 Prometheus collector，而是把 `pkg/metrics/stats.rs` 中已经创建的共享 `CounterVec`/`Gauge` 选成 Domain 业务需要的具体句柄。crate 入口 `pkg/domain/metrics/lib.rs` 通过 `domain_metrics` 模块挂载本文件，并把 `stats.rs` 中的 `HistoricalStatsCounter`、`PlanReplayerTaskCounter`、`PlanReplayerRegisterTaskGauge` 再导出到本文件使用。

进程级 Rust 初始化链位于 `pkg/util/metricsutil/common.rs::initMetrics`：它先通过 `initParentMetricsCollectors` 调用 `astersql_domain_metrics::stats::InitStatsMetrics` 创建父 collector，再调用本文件的 `InitMetricsVars` 绑定 label 子句柄。`Cargo.toml` 将本目录声明为独立的 `astersql-domain-metrics` library crate，直接外部依赖仅为 `prometheus = "0.14"`；`pkg/util/metricsutil/Cargo.toml` 以路径依赖接入该 crate。

当前迁移状态需要特别区分：绑定和初始化链已经存在，但 `pkg/domain/historical_stats.rs` 与 `pkg/domain/plan_replayer.rs` 中对这些 Rust 句柄的业务更新仍是注释代码；因此不能声称 Rust Domain 主业务路径已实际产生这些指标。Go 对照业务路径已接线，见 `pkg/domain/historical_stats.go`、`pkg/domain/plan_replayer.go` 和 `pkg/domain/plan_replayer_dump.go`。

## 核心职责

1. 用七个公开静态槽位保存六个 `prometheus::Counter` 和一个 `prometheus::Gauge` 句柄。
2. `InitMetricsVars` 从共享父 collector 取得固定 label 组合：历史统计使用 `generate × success/fail`；Plan Replayer 使用 `dump × success/fail` 和 `capture × send/discard`。
3. 通过 `replace` 在初始化或重新初始化时原子地替换每个槽位内的句柄，使后续读取者指向最新创建的父 collector。
4. `init` 提供与 Go 包级 `init()` 对应的显式 Rust 包装入口；Rust 不会自动执行普通 `fn init`，当前进程初始化实际直接调用 `InitMetricsVars`。

本文件不负责 collector 的名称、help、label schema、注册表注册或业务时机。那些职责分别落在 `pkg/metrics/stats.rs::InitStatsMetrics`、metrics 注册流程和 Domain 业务调用点。

## 主要符号

- `GenerateHistoricalStatsSuccessCounter` / `GenerateHistoricalStatsFailedCounter`：`LazyLock<RwLock<Option<prometheus::Counter>>>`，分别绑定 `HistoricalStatsCounter` 的 `("generate", "success")` 与 `("generate", "fail")` 子序列。
- `PlanReplayerDumpTaskSuccess` / `PlanReplayerDumpTaskFailed`：同类 Counter 槽位，绑定 `PlanReplayerTaskCounter` 的 `("dump", "success")` 与 `("dump", "fail")`。
- `PlanReplayerCaptureTaskSendCounter` / `PlanReplayerCaptureTaskDiscardCounter`：同类 Counter 槽位，绑定 `("capture", "send")` 与 `("capture", "discard")`。
- `PlanReplayerRegisterTaskGauge`：`LazyLock<RwLock<Option<prometheus::Gauge>>>`，保存共享注册任务 Gauge 的 clone；Prometheus handle 的 clone 仍共享底层时序，而不是复制当前数值。
- `init()`：公开、无返回值，唯一动作是调用 `InitMetricsVars()`。它用于表达 Go 包初始化语义，但并非 Rust 语言钩子。
- `replace<T>(slot, value)`：私有泛型辅助函数，取得写锁并把 `Option<T>` 覆盖为 `Some(value)`；若锁已 poisoned，则以 `into_inner()` 恢复并继续写入。
- `InitMetricsVars()`：公开核心入口。它在 `unsafe` 块中读取三个 `static mut Option<_>` 父句柄，校验初始化顺序，派生六个 Counter label handle，并 clone 一个 Gauge handle 后写入上述槽位。

文件没有自定义类型、trait、impl、编译条件或返回错误类型。`#![allow(non_snake_case, non_upper_case_globals)]` 用于保留 Go 导出符号的命名形态。

## 执行流程

正常启动流程如下：

1. `pkg/util/metricsutil/common.rs::initMetrics` 调用 `initParentMetricsCollectors`。
2. `initParentMetricsCollectors` 调用 `pkg/metrics/stats.rs::InitStatsMetrics`，创建 `HistoricalStatsCounter`、`PlanReplayerTaskCounter` 与 `PlanReplayerRegisterTaskGauge`。两个 CounterVec 的 label schema 都是 `[LblType, LblResult]`。
3. `initMetrics` 随后调用 `domain_metrics::InitMetricsVars`。
4. `InitMetricsVars` 对每个父级 `Option` 使用 `as_ref().expect(...)`；若存在，则通过 `with_label_values` 获取固定 label 子句柄。
5. 每个子句柄交给 `replace`，在独占写锁下把初始 `None` 或旧句柄更新为新的 `Some(handle)`。
6. 将来业务消费者需要先读 `RwLock`、确认 `Option` 已初始化，再对 Counter 调用 `inc`/`inc_by` 或对 Gauge 调用 `set`。目前仓库中的真实 Rust Domain 业务调用仍被注释，只有迁移测试执行这一步。

重初始化流程与首次初始化相同：先重新创建父 collector，再调用 `InitMetricsVars`。测试 `register_gauge_shares_the_go_package_handle_and_reinit_rebinds` 证明旧句柄会被新句柄覆盖，新父 collector 的初始值为 0。

## 数据与状态

七个槽位都是进程全局状态。`LazyLock` 只负责延迟构造各自的 `RwLock<Option<_>>`；它不会初始化 Prometheus handle。`Option::None` 明确表示尚未执行成功的绑定流程，`Some` 表示已持有一个具体 label 子句柄或 Gauge handle。

父级状态在 `pkg/metrics/stats.rs` 中以 `static mut Option<_>` 保存：

- `HistoricalStatsCounter` 对应 `tidb_statistics_historical_stats`，labels 为 `type`、`result`。
- `PlanReplayerTaskCounter` 对应 `tidb_plan_replayer_task`，labels 同样为 `type`、`result`。
- `PlanReplayerRegisterTaskGauge` 对应 `tidb_plan_replayer_register_task`，没有可变 label。

调用 `with_label_values` 得到的 Counter 与父 CounterVec 的对应 label 时序共享状态；Gauge 的 `clone` 也共享底层状态。`InitMetricsVars` 本身不增减任何指标。重新运行父初始化会创建新 collector，本文件随后必须重新绑定，否则槽位仍指向旧 collector。

## 依赖与调用关系

上游：

- 生产初始化：`pkg/util/metricsutil/common.rs::initMetrics -> domain_metrics::InitMetricsVars`；其必要前序为 `initParentMetricsCollectors -> astersql_domain_metrics::stats::InitStatsMetrics`。
- 本文件包装：`init -> InitMetricsVars`。
- 独立测试：`pkg/domain/metrics/migration_aster_unit_test.rs::initialize` 和 `register_gauge_shares_the_go_package_handle_and_reinit_rebinds` 直接调用 `InitMetricsVars`。

下游：

- `InitMetricsVars -> replace` 是 RustCodeGraph 确认的直接函数调用边。
- 它读取 `crate::metrics` 再导出的三个父句柄，并调用 Prometheus `CounterVec::with_label_values` 或 `Gauge::clone`。
- `replace` 依赖标准库 `LazyLock`、`RwLock::write` 与 poisoned lock 恢复逻辑。

业务关系：Go 版本的 `GenerateHistoricalStats*` 在 `historical_stats.go` 更新，Plan Replayer dump/capture/register 指标分别在 `plan_replayer_dump.go` 与 `plan_replayer.go` 更新。Rust 对应文件目前只有注释形式的调用，因此这些是对照意图，不是已接线的 Rust 调用边。

## 错误处理与边界

`InitMetricsVars` 没有 `Result` 返回值，初始化顺序错误通过 panic 暴露：三个 `.expect(...)` 分别要求 HistoricalStats、PlanReplayer CounterVec 和注册任务 Gauge 已先初始化。固定 label 数量与父 collector 的两个 label 一致；如果未来父 label schema 改变而本文件未同步，`with_label_values` 会在运行期失败。

`replace` 对 poisoned `RwLock` 采取恢复策略：不再次 panic，而是取得 poisoned guard 的内部值并覆盖。它不处理读侧的 `None`；消费者必须自行保证初始化顺序并检查/解包 `Option`。测试代码中的直接 `unwrap` 只用于在受控初始化之后断言，不代表通用调用约定。

本文件的 `unsafe` 来自读取共享的 `static mut` 父句柄。`RwLock` 只保护本文件七个槽位，不能保护父级 `static mut` 的并发读写；所以 `InitStatsMetrics` 与 `InitMetricsVars` 必须由启动/重初始化编排串行执行。文件也不注册 collector，不处理重复注册错误或注册表生命周期。

## 并发与资源生命周期

每个句柄槽位有独立 `RwLock`：多个业务读者可以并发取得读锁，初始化/重绑定者逐槽位取得短暂写锁。七次替换不是一个整体事务；若并发消费者在重绑定期间读取，可能观察到新旧句柄混合。因此安全用法是在开放业务并发之前完成初始化，并把进程内重初始化视为受控测试或停机式操作。

`LazyLock` 的锁容器生命周期为整个进程。Prometheus Counter/Gauge 是可 clone 的共享句柄，槽位覆盖只释放该槽位持有的旧 clone；其他 clone 若仍存在则可继续引用旧 collector。代码没有线程、异步任务、通道、事务、显式关闭或注销操作。

测试文件以全局 `TEST_LOCK: Mutex<()>` 串行化两项测试，避免它们并发重建相同父级 `static mut` 并重绑全局槽位。这一测试约束也侧面证明重初始化不是可无协调并发执行的操作。

## 与 Go 版本的对应关系

`pkg/domain/metrics/metrics.go` 是逐项语义基准：七个 Go 包变量、`init()` 和 `InitMetricsVars()` 均在 Rust 中保留，六组 label 字符串完全一致，Gauge 也保持共享句柄语义。Rust 为满足所有权与并发访问要求，将 Go 的直接包变量赋值改成 `LazyLock<RwLock<Option<T>>>`，并用 `replace` 完成覆盖；父 collector 仍为可空全局，因此 Rust 通过 `expect` 显式检查 Go 代码隐含的初始化前提。

两端初始化机制不同：Go 运行时自动执行包 `init()`；Rust 的同名普通函数不会自动运行，生产路径在 `metricsutil::initMetrics` 中显式直接调用 `InitMetricsVars`。Rust 测试覆盖 label 对应、共享时序和重绑定清零行为；未发现同目录 Go 独立测试直接引用这些变量，Go 行为证据主要来自实际业务调用点。

迁移并未全部完成：Go 的历史统计和 Plan Replayer 业务路径会真实更新指标，而 Rust 对应调用目前在 `historical_stats.rs`、`plan_replayer.rs` 中被注释。扩展或完成移植时必须以这些 Go 调用点为行为基准，不能仅因句柄绑定测试通过就认定端到端指标已启用。

## 扩展指南

新增 Domain 指标时应按职责分层修改：先在拥有 collector 的 `pkg/metrics/*.rs` 中按 Go 基准定义并初始化父 collector，再由 `pkg/domain/metrics/lib.rs::metrics` 再导出需要的句柄，最后在本文件新增独立槽位与 `InitMetricsVars` 绑定。若只是为现有 CounterVec 增加固定 label 组合，只需新增槽位和 `with_label_values` 分支，但必须核对 label 顺序、拼写与基数。

测试应继续放在独立的 `pkg/domain/metrics/migration_aster_unit_test.rs`，不要内嵌进生产源文件。至少应验证：新 label 句柄能反映到父向量；重复初始化后槽位转向新 collector；Gauge/Counter clone 保持共享状态；初始化前提失败的行为若被改变，应有明确测试。涉及生产消费时，还应在对应 Domain 独立测试中覆盖业务分支实际更新指标，而不能只测绑定层。

兼容性风险包括改变公开静态符号或 label 文本导致监控查询断裂；正确性风险包括父初始化遗漏、label 数量不匹配以及重初始化期间的新旧句柄混用；性能风险主要是业务热路径每次访问都需取得 `RwLock` 读锁。若未来优化访问方式，应同时保留安全重绑定能力，或明确取消进程内重初始化契约并调整测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/domain/metrics` 确认目标 Rust、crate 入口、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file pkg/domain/metrics/metrics.rs`：核对七个静态槽位、`init`、`replace`、`InitMetricsVars` 及全部固定 label。
- RustCodeGraph `query/callers/callees InitMetricsVars`：定位目标定义，确认测试与 `init` 调用者，并确认 `InitMetricsVars -> replace` 直接调用边。由于同名函数较多，生产入口另以精确文件查询核对。
- RustCodeGraph `node --file pkg/util/metricsutil/common.rs`：核对 `initParentMetricsCollectors` 先创建父 collector、`initMetrics` 后绑定 Domain 句柄的顺序。
- RustCodeGraph `node --file pkg/metrics/stats.rs`：核对父 collector 类型、名称、label schema 与初始化位置。
- 读取 `pkg/domain/metrics/Cargo.toml`、`pkg/domain/metrics/lib.rs`、`pkg/domain/metrics/metrics.go`：核对 crate 边界、模块再导出和 Go 逐项语义。
- 读取 `pkg/domain/metrics/migration_aster_unit_test.rs`：核对两项独立测试对 label 共享、Gauge 共享、串行化和重绑定归零的验证。
- `rg` 精确检索 `pkg/domain`：核对 Go 的真实业务消费者，以及 Rust `historical_stats.rs`、`plan_replayer.rs` 中尚处于注释状态的调用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行固定十一章节结构校验并人工复核事实边界。
