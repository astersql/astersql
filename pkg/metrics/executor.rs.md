# `pkg/metrics/executor.rs`

## 文件定位

本文件属于 `astersql-metrics` crate（见 `pkg/metrics/Cargo.toml`），由 `pkg/metrics/lib.rs` 以公开模块 `pub mod executor` 暴露。它不是执行器本身，而是执行器、语句调度和相关事务/MPP/IndexLookUp 路径共用的 Prometheus 指标定义与初始化层。包级初始化中枢 `pkg/metrics/metrics.rs::InitMetrics` 调用 `executor::InitExecutorMetrics`，随后 `RegisterMetrics` 把本文件拥有的向量 collector 注册到默认 registry。

当前 Rust 运行链上，本文件还提供 `IncStatementCounter`，供 `pkg/session/runtime/dispatch.rs::record_statement_metric` 在识别语句 AST 后更新语句计数。RustCodeGraph 将 `pkg/session/runtime/dispatch.rs` 标为 `executor.rs` 的直接使用者；`rg` 进一步定位到该调用发生在 `dispatch.rs:595`。

## 核心职责

1. 声明 22 个惰性初始化的全局指标句柄，覆盖昂贵执行器次数、语句类型、执行阶段耗时、进行中事务时长、MPP 协调器、影响行数、网络传输和 IndexLookUp 行为（`ExecutorCounter` 至 `IndexLookUpCopTaskCount`）。
2. 在 `InitExecutorMetrics` 中按 Go 指标名、帮助文本、标签顺序及桶配置构造 collector，维持已有 Prometheus 时序兼容性。
3. 从 `AffectedRowsCounter` 派生 Insert/Update/Delete/Replace、四种 NT-DML 以及 `PurgeMLog` 的固定标签 counter；这些句柄共享同一个底层 collector，而不是各自注册新的指标族。
4. 通过 `IncStatementCounter` 同时维护三标签的 `StmtNodeCounter` 与两标签的 `DbStmtNodeCounter`，并在尚未初始化时安全地不做任何更新。

本文件只定义和初始化指标，不决定何时观测执行阶段、MPP 延迟、影响行数或 IndexLookUp 数据；这些写入点位于各消费模块。注册也由 `pkg/metrics/metrics.rs::RegisterMetrics` 统一完成。

## 主要符号

- `InitExecutorMetrics()`: 唯一的 collector 构造入口。它先持有 `crate::metrics::PACKAGE_INIT_LOCK`，创建所有指标和固定标签句柄，再在一个 `unsafe` 块内将局部值写入全局 `Option`。
- `IncStatementCounter(statement_type, database, resource_group)`: 语句计数便捷入口；按 `[statement_type, database, resource_group]` 更新 `StmtNodeCounter`，按 `[database, statement_type]` 更新 `DbStmtNodeCounter`。
- `ExecutorCounter`、`StmtNodeCounter`、`DbStmtNodeCounter`: 分别记录昂贵执行器、按类型/库/资源组划分的语句、按库/类型划分的语句。
- `ExecPhaseDuration`、`OngoingTxnDurationHistogram`: 分别是阶段耗时 SummaryVec 和进行中事务时长 HistogramVec。后者使用 `ExponentialBuckets(60.0, 2.0, 15)`。
- `MppCoordinatorStats`、`MppCoordinatorLatency`: MPP 协调器状态 GaugeVec 与操作延迟 HistogramVec；延迟桶为 `ExponentialBuckets(0.001, 2.0, 28)`。
- `AffectedRowsCounter` 及九个 `AffectedRowsCounter*` 固定标签句柄: 指标族标签名为 `LblSQLType`，固定值包括普通 DML、`NTDML-*` 和 `PurgeMLog`。
- `NetworkTransmissionStats`: 按 `LblType` 记录查询网络传输字节数。
- `IndexLookUpExecutorDuration`、`IndexLookRowsCounter`、`IndexLookUpExecutorRowNumber`、`IndexLookUpCopTaskCount`: 分别记录 IndexLookUp 耗时、下推行数、单次扫描行数与 cop task 数；耗时和行数直方图分别采用 `(0.0001, 2, 30)` 与 `(1, 2, 10)` 的指数桶。

所有全局句柄均为 `pub static mut Option<...>`。这保留了 Go 包级变量“初始化前为空、初始化后全局可取”的迁移形态，但也要求访问者遵守初始化和同步约束。

## 执行流程

初始化与采集分成两条链：

1. `pkg/metrics/metrics.rs::InitMetrics` 通过 `INIT_METRICS_ONCE` 保证包级初始化只执行一次，并按子系统顺序调用 `executor::InitExecutorMetrics`。
2. `InitExecutorMetrics` 获取 `PACKAGE_INIT_LOCK`；依次构造 executor、statement、transaction、MPP、affected rows、network 和 IndexLookUp collectors。
3. 对 `AffectedRowsCounter` 克隆句柄并调用 `WithLabelValues`，提前绑定各 SQL 类型。`PurgeMLog` 在最终写全局状态时从同一 CounterVec 派生。
4. 函数最后集中写入所有全局 `Option`，使注册中枢和业务调用者可以读取这些句柄。
5. `pkg/metrics/metrics.rs::RegisterMetrics` 使用 `register_option` 注册各向量 collector；固定标签 counter 不单独注册，因为它们属于 `AffectedRowsCounter`。

语句计数链为：session dispatch 解析/持有 AST → `record_statement_metric` 识别 CreateTable、Insert/Replace、Delete、Update、Select 或 Prepare → 用空数据库名和 `"default"` 资源组调用 `IncStatementCounter` → 两个 CounterVec 各增加一次。其他 AST 类型在当前 Rust 调度实现中不调用本函数。

## 数据与状态

指标描述由 `Namespace = "tidb"`、`Subsystem = "executor"`、具体 `Name`、`Help`、标签名和可选桶边界组成。标签顺序是外部契约：例如 `StmtNodeCounter` 必须是 `[LblType, LblDb, LblResourceGroup]`，而 `DbStmtNodeCounter` 必须是 `[LblDb, LblType]`；交换顺序会把值写入错误的时序维度。

状态生命周期为 `None → Some(collector)`。包级正常入口只初始化一次，但 `InitExecutorMetrics` 本身是公开函数，测试也会直接调用它；内部互斥锁只防止同时替换全局句柄，并不赋予该函数独立的一次性语义。业务计数是 Prometheus collector 内部的原子/同步状态，本文件没有额外缓存、事务状态或持久化数据。

固定标签 counter 与 `AffectedRowsCounter` 共享指标族。因此扩展新的影响行数类型应从该 CounterVec 派生句柄，不能另建同名 collector，否则注册时可能发生描述符冲突。

## 依赖与调用关系

- crate 边界：`pkg/metrics/Cargo.toml` 将库入口设为 `lib.rs`，直接依赖 `astersql-metrics-common`、`astersql-util-promutil` 和 `prometheus = "0.14"`；本文件通过 `crate::bindinfo` 的兼容模块取得 Go 风格构造器和 `WithLabelValues`/`inc` 等适配方法。
- 模块入口：`pkg/metrics/lib.rs` 公开 `executor`，并从 crate 根提供 `LblType`、`LblDb`、`LblResourceGroup`、`LblPhase`、`LblInternal`、`LblSQLType` 等标签常量。
- 上游初始化：`pkg/metrics/metrics.rs::InitMetrics → InitExecutorMetrics`。
- 上游业务调用：`pkg/session/runtime/dispatch.rs::record_statement_metric → IncStatementCounter`。当前搜索未发现其他 Rust/Go 调用 `IncStatementCounter`。
- 下游构造：`metricscommon::NewCounterVec`、`NewGaugeVec`、`NewHistogramVec`、`NewSummaryVec`，以及 Prometheus 兼容层的 `WithLabelValues`、`inc`。
- 下游注册：`pkg/metrics/metrics.rs::RegisterMetrics` 注册本文件的所有向量 collector；由固定标签派生的 counter 不重复注册。
- 其他消费者：例如 `pkg/metrics/telemetry.rs` 读取 `StmtNodeCounter`，`pkg/executor/metrics/metrics.rs` 从 `IndexLookUpExecutorDuration` 派生具体标签句柄。Go 侧还有更多直接消费者，不能据此推断所有 Rust 消费点已经完成迁移。

## 错误处理与边界

`InitExecutorMetrics` 没有返回值。它唯一显式失败路径是 `PACKAGE_INIT_LOCK.lock().expect(...)`：锁若被持有线程 panic 污染，初始化会 panic。collector 构造经兼容层表现为直接返回值，因此本函数没有可传播的构造错误；真正的重复注册或描述符冲突由后续 `RegisterMetrics() -> Result<(), prometheus::Error>` 报告。

`IncStatementCounter` 对两个全局 `Option` 分别判断：初始化前调用会静默跳过对应计数，不 panic，也不返回“未初始化”错误。如果只设置了其中一个全局值，它仍会更新可用的那一个，因此两个指标之间的同步增加不是类型系统强制的不变量。

调用者必须提供与向量定义完全一致的标签数。当前便捷函数固定传入正确数量，但若直接访问公开 CounterVec，错误标签数的行为取决于兼容层/Prometheus API。标签值还会形成新时序，新增高基数数据库名、资源组或类型前应评估内存和抓取成本。

## 并发与资源生命周期

初始化期间使用全包共享的 `PACKAGE_INIT_LOCK: Mutex<()>`，避免多个直接 `Init*Metrics` 调用并发改写 `static mut`；正常包级入口再由 `INIT_METRICS_ONCE` 提供一次性保证。锁在 `InitExecutorMetrics` 返回时释放，collector 随全局句柄存活到进程结束。

运行期计数操作委托给 Prometheus collector 的并发安全实现。本文件没有线程、异步任务、channel、文件句柄或显式析构逻辑。风险集中在 `static mut`：读取和替换发生在 `unsafe` 边界，单独直接调用初始化函数可能替换仍被其他线程持有/读取的全局句柄；生产代码应走包级 `InitMetrics`，而不是重入本函数。

注册生命周期与构造生命周期分离。collector 构造完成并不代表已暴露给抓取端；只有 `RegisterMetrics` 成功后才进入 registry。反之，固定标签句柄只是共享 collector 的访问视图，不拥有独立注册/注销生命周期。

## 与 Go 版本的对应关系

`pkg/metrics/executor.go` 是直接对照文件。两边的指标名、Help、标签顺序、指数桶参数以及普通 DML、NT-DML、`PurgeMLog` 标签值一致；Rust 用 `Option` 表达 Go 指针型包变量在初始化前未赋值，用 clone 表达多个句柄共享 collector。

关键实现差异有三点：

1. Go 的 `InitExecutorMetrics` 直接赋包变量；Rust 先构造局部值、持有 `PACKAGE_INIT_LOCK`，再集中写入 `static mut Option`。
2. Go 本文件没有 `IncStatementCounter`。Go 的 `pkg/executor/compiler.go` 根据 type/db/resource group 直接同时增加 `DbStmtNodeCounter` 和 `StmtNodeCounter`；Rust 将此动作封装为函数，由 session dispatch 调用。
3. 当前 Rust dispatch 只识别有限 AST 类型，并传入空数据库名与 `"default"` 资源组；Go compiler 会从语句上下文取得数据库标签和资源组，也包含更完整的语句分类路径。因此指标定义已对齐不等于所有生产采集点均已完全对齐。

Go 的 `pkg/session/test/nontransactionaltest/nontransactional_test.go` 验证 NT-DML 计数增加且普通 DML 指标不增加；Rust 同路径 `nontransactional_test.rs` 保留了相同断言意图。该测试覆盖这些全局指标的消费语义，但不是 `executor.rs` 的独立同名单元测试。

## 扩展指南

- 新增指标：在本文件新增独立全局 `Option` 和 `InitExecutorMetrics` 构造/赋值，同时把向量 collector加入 `pkg/metrics/metrics.rs::RegisterMetrics`；同步 Go 对照或明确迁移差异。不要把测试内嵌进本源文件，应扩展 `pkg/metrics/*_test.rs` 中最接近的独立测试。
- 新增影响行数类型：优先从 `AffectedRowsCounter` 派生固定标签句柄，并核对标签拼写与大小写；不要注册派生 counter。若 Go 已有对应标签，保持完全一致。
- 新增语句维度或分类：同时检查 `StmtNodeCounter`/`DbStmtNodeCounter` 的标签顺序、`pkg/session/runtime/dispatch.rs::record_statement_metric` 和 Go `pkg/executor/compiler.go`。数据库名或资源组改为真实值会增加时序基数，需评估兼容和内存风险。
- 调整桶：桶边界会改变导出的 histogram bucket 时序，属于监控兼容性变更；应同步 Go 配置、Grafana/告警查询及针对 bucket 的独立测试。
- 修改初始化：保持“先构造、后集中发布”的顺序，并通过包级 `InitMetrics`/`RegisterMetrics` 测试验证。不要绕过 `PACKAGE_INIT_LOCK` 写 `static mut`。

建议的测试落点是 `pkg/metrics/metrics_test.rs`（初始化与注册）、`pkg/metrics/metrics_internal_test.rs`（collector 元数据/内部约束）或新增独立的 `pkg/metrics/executor_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 区域挂载；跨模块读取可扩展 `telemetry_test.rs`，语句执行行为则扩展 session 的独立测试。按仓库规则，测试逻辑不得写入 `executor.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件且目标 `pkg/metrics/executor.rs` 已索引；`files --filter pkg/metrics` 显示该文件有 27 个索引符号；`node --file pkg/metrics/executor.rs` 显示完整 298 行源码并给出直接使用文件 `pkg/session/runtime/dispatch.rs`；`query InitExecutorMetrics` 找到 Go/Rust 两个对照定义，`query IncStatementCounter` 找到 Rust 唯一定义。`callers/callees`/`explore` 查询在本次环境中超时，调用边改由索引的 used-by 结果与精确 `rg` 位置交叉核验。
- 源码：`pkg/metrics/executor.rs`（22 个全局句柄、两个公开函数、标签和桶配置）；`pkg/metrics/lib.rs`（公开模块和标签常量）；`pkg/metrics/metrics.rs`（`PACKAGE_INIT_LOCK`、`INIT_METRICS_ONCE`、初始化与注册链）；`pkg/session/runtime/dispatch.rs`（语句分类和唯一 Rust 调用点）。
- crate/兼容层：`pkg/metrics/Cargo.toml`（`astersql-metrics` 边界和依赖）；`pkg/metrics/bindinfo.rs`、`pkg/metrics/common/wrapper.rs`（Go 风格指标构造/适配入口）。
- Go 对照：`pkg/metrics/executor.go`（指标定义与初始化）；`pkg/metrics/metrics.go`（包初始化/注册）；`pkg/executor/compiler.go`（语句 counter 的真实 Go 写入点）。
- 测试：`pkg/metrics/bindinfo_1_aster_unit_test.rs::all_metric_initializers_accept_go_metadata`（直接初始化冒烟）；`pkg/metrics/telemetry_test.rs::telemetry_getters_read_the_shared_session_and_executor_metrics`（共享 `StmtNodeCounter` 的跨模块读取）；`pkg/metrics/metrics_test.rs` 与 `metrics_internal_test.rs`（包级初始化/注册）；`pkg/session/test/nontransactionaltest/nontransactional_test.rs` 及其 Go 对照（NT-DML 与普通 DML 的指标隔离意图）。未发现专门只测试 `executor.rs` 的独立同名 Rust 测试。
- 本任务为纯文档分析，按计划不运行 Cargo；最终结构检查要求目标文件存在且恰有 11 个固定二级标题。
