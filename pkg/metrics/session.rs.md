# `pkg/metrics/session.rs`

## 文件定位

`pkg/metrics/session.rs` 是 `astersql-metrics` crate 中 Session/事务执行路径指标的定义文件。`pkg/metrics/lib.rs` 以私有 `mod session` 装入它并 `pub use session::*`，因此文件内的公开静态量、标签常量和函数会成为 crate 的公开接口。crate 边界由 `pkg/metrics/Cargo.toml` 定义；本文件直接使用同 crate 的 `bindinfo` 兼容层以及包级标签。

它只构造 Prometheus collector，不负责观测业务事件，也不自行注册 collector。主 crate 的 `pkg/metrics/metrics.rs::InitMetrics` 调用 `InitSessionMetrics` 创建句柄，随后 `RegisterMetrics` 将这些句柄纳入注册表。相同源码还被 `pkg/executor/metrics/lib.rs` 通过 `#[path]` 装入、被 `pkg/session/metrics/lib.rs` 通过 `include!` 装入，供拆分后的子 crate 复用；所以这里既是主 metrics crate 的实现，也是两个指标门面 crate 的共享定义。

## 核心职责

本文件承担三类职责。

1. 定义 23 个进程级 `Option<prometheus::...>` 静态句柄，覆盖 SQL 解析、编译、执行，事务语句数和总耗时，重试和 schema lease 错误，事务状态转换，悲观锁、非事务 DML、资源组及 Fair Locking 等指标。
2. `InitSessionMetrics` 按 Go 版本的顺序、名称、help、桶和标签集合构造全部句柄；四个私有工厂 `counter`、`counter_vec`、`histogram`、`histogram_vec` 统一写入 `tidb` namespace，并转交 `metricscommon::New*` 注入包级常量标签。
3. 导出跨 metrics 子模块共用的标签名和值。`TxnStatusEnteringCounterVec` 与 `TxnDurationHistogramVec` 额外提供按需初始化并克隆事务状态 collector 的安全使用入口，避免消费方另建同名但未注册的指标。

该文件不创建 Session、不访问 PD、不执行事务，也不决定何时打点；调用侧负责选择标签并调用 `inc`/`observe`。

## 主要符号

- `InitSessionMetrics()`：唯一的全量构造入口，把所有静态 `Option` 从 `None` 替换为新 collector。主要指标组如下：
  - SQL 阶段：`AutoIDReqDuration`、`SessionExecuteParseDuration`、`SessionExecuteCompileDuration`、`SessionExecuteRunDuration`。
  - 重试和事务结果：`SessionRetry`、`SessionRetryErrorCounter`、`StatementPerTransaction`、`TransactionDuration`、`SessionRestrictedSQLCounter`。
  - 悲观事务/锁：`StatementDeadlockDetectDuration`、`StatementPessimisticRetryCount`、`StatementLockKeysCount`、`StatementSharedLockKeysCount`、`LazyPessimisticUniqueCheckSetCount`、`PessimisticDMLDurationByAttempt`、`FairLockingUsageCount`、`PessimisticLockKeysDuration`。
  - 状态与其他边界：`SchemaLeaseErrorCounter`、`ValidateReadTSFromPDCount`、`NonTransactionalDMLCount`、`TxnStatusEnteringCounter`、`TxnDurationHistogram`、`ResourceGroupQueryTotalCounter`。
- `TxnStatusEnteringCounterVec() -> CounterVec`、`TxnDurationHistogramVec() -> HistogramVec`：若相应静态量仍为 `None`，先调用 `InitSessionMetrics`，之后 `expect` 并克隆 collector。`pkg/session/txninfo/txn_info.rs` 用它们绑定事务状态以及 `has_lock` 标签。
- `counter` / `counter_vec` / `histogram` / `histogram_vec`：私有描述符工厂，固定 namespace 为 `tidb`，接收 subsystem、metric name、help、桶和可变标签。
- `Lbl*` 常量：既包含标签名（如 `LblType = "type"`、`LblSQLType = "sql_type"`、`LblHasLock = "has_lock"`），也包含枚举值（如 `LblInternal`、`LblPessimistic`、`LblFairLockingTxnUsed`）。消费方必须区分两者，且保持字符串兼容性。

文件不定义 struct、enum、trait、`impl` 或条件编译分支；公开 API 主要是静态 collector、两个访问器、初始化函数和标签常量。

## 执行流程

主 crate 的标准流程是：

1. `pkg/metrics/metrics.rs::InitMetrics` 在全局 `Once` 保护的初始化序列中调用 `crate::session::InitSessionMetrics()`。
2. `InitSessionMetrics` 逐项构建 collector。例如 parse/compile 使用从 40 微秒起、倍增 28 次的指数桶；execute 使用从 100 微秒起、倍增 30 次的桶；`SessionRetry` 使用 0 到 20 的 21 个离散边界；事务状态耗时从 0.5 毫秒起倍增 29 次。
3. 向量按用途声明固定标签次序，例如 `StatementPerTransaction` 为 `[txn_mode, type, scope]`，`TxnDurationHistogram` 为 `[type, has_lock]`，`ResourceGroupQueryTotalCounter` 为 `[name, resource_group]`。下游 `with_label_values` 必须严格采用该次序。
4. `pkg/metrics/metrics.rs::RegisterMetrics` 读取这些 `Option` 并把已创建的 collector 注册到 Prometheus registry；构造和注册是两个阶段。
5. Session/Executor 等消费模块将向量预绑定为具体标签组合后，在业务路径中 `inc` 或 `observe`。例如 `pkg/session/metrics/metrics.rs::InitMetricsVars` 绑定 internal/general 的解析、编译、重试与事务结果句柄；`pkg/executor/metrics/metrics.rs` 绑定执行耗时和 Fair Locking，并记录悲观锁次数、键数和耗时。

事务状态子链略有不同：`pkg/session/txninfo/txn_info.rs::InitMetricsVars` 调用两个 `*Vec` 访问器确保 collector 存在，再由 `TxnStatusEnteringCounter` 和 `TxnDurationHistogram` 将运行状态映射到 `executing_sql`、`acquiring_lock` 等标签值。

## 数据与状态

所有 collector 都存放在 `pub static mut Option<T>` 中，`None` 表示尚未初始化，`Some` 表示已有句柄。这是对 Go 包变量零值/初始化模型的迁移表达，不是按 Session 实例隔离的数据；collector 的计数和样本是进程级共享状态。`CounterVec`/`HistogramVec` 的克隆共享底层指标族，不会复制一套独立计数。

指标 identity 由 namespace、subsystem、name、常量标签和可变标签共同决定。大多数指标使用 `session` subsystem；`AutoIDReqDuration` 使用 `meta`；`PessimisticLockKeysDuration` 为兼容 client-go 历史名称保留 `tikvclient`。改变这些字段会改变暴露给监控、仪表盘和告警的时间序列，属于兼容性变更。

`metricscommon::New*` 最终进入 `pkg/metrics/common/wrapper.rs`：构造前读取并注入包级 const labels，构造器用 `expect` 校验描述符。因此 const labels 应在初始化/注册前设置；初始化后再改变全局标签不会回写已创建的 collector。

## 依赖与调用关系

上游入口和调用者：

- `pkg/metrics/lib.rs` 声明并再导出模块；`pkg/metrics/metrics.rs::InitMetrics` 是主初始化入口，`RegisterMetrics` 是主注册入口。
- `pkg/session/metrics/lib.rs` 内联本文件，`pkg/session/metrics/metrics.rs::InitMetricsVars` 从它取得已加标签的 Session 句柄。
- `pkg/executor/metrics/lib.rs` 路径装入本文件；`pkg/executor/metrics/metrics.rs` 初始化并观测执行、锁和 Fair Locking 指标。
- `pkg/session/txninfo/txn_info.rs` 通过两个访问器使用事务状态计数和状态时长。
- `pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 初始化拆分子 crate 中的本地父 collector。
- `pkg/metrics/telemetry.rs` 读取非事务 DML、延迟唯一性检查和 Fair Locking 计数，形成遥测快照。

下游依赖：本文件通过 `crate::bindinfo::{compat_metricscommon, compat_prometheus}` 使用兼容的 options、桶函数及 `NewCounter*`/`NewHistogram*` 工厂；实际 collector 类型来自 `prometheus`。`pkg/metrics/Cargo.toml` 声明 `prometheus = "0.14"` 以及本地 `astersql-metrics-common`，未为本文件设置 feature gate。

RustCodeGraph 将目标文件标为被 9 个文件使用，并能识别 Rust/Go 两个 `InitSessionMetrics` 候选；精确调用图查询未在命令窗口内返回，因此上述边均以模块入口、初始化/注册实现和 `rg` 的实际引用复核，不把无结果的图查询当作证据。

## 错误处理与边界

`InitSessionMetrics` 没有 `Result` 返回值。非法 Prometheus 名称、标签或 options 会在 `metricscommon::New*` 内通过 `expect` 触发 panic；当前文件中的名称和标签是编译期常量。两个事务状态访问器在初始化后仍找不到 collector 时也会通过 `expect` panic，但按其控制流，正常情况下会先补做 `InitSessionMetrics`。

本文件不处理注册冲突；重复或同名 collector 注册错误由 `pkg/metrics/metrics.rs::RegisterMetrics` 所在层处理。也不校验调用侧标签数量：`with_label_values` 的数量/顺序契约由 Prometheus API和消费方承担。

`InitSessionMetrics` 每次调用都会替换全部静态句柄，因此直接重复调用可能使先前克隆/预绑定的句柄与新静态句柄分离。主 crate 依靠 `metrics.rs::INIT_METRICS_ONCE` 避免这种情况；独立门面 crate 的调用侧则必须遵循各自初始化锁或在单线程启动阶段调用。不要把两个按需访问器理解为整个文件的并发一次性初始化器。

## 并发与资源生命周期

Prometheus collector 本身可被克隆并跨线程观测，但承载它们的 `static mut Option` 需要 `unsafe` 读写。本文件没有互斥锁、`OnceLock` 或原子状态；并发执行 `InitSessionMetrics`，或一边替换静态量一边读取，会违反其隐含生命周期约束。主 crate 的 `InitMetrics` 用 `Once` 串行并复用首次结果，Session/Executor 门面也在外围维护初始化锁，这些外围约束才使常规启动路径安全。

资源生命周期为进程级：初始化时分配 collector，注册表持有克隆，业务模块可能再持有带标签的子句柄，直到进程退出。无显式释放、后台任务、通道、网络连接或事务资源。重新赋值静态 `Option` 不会更新已在注册表或调用侧持有的旧克隆，所以扩展初始化逻辑时必须保持“一次构造、随后只读/观测”的不变量。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/metrics/session.go`。Rust 保留了 Go 的 23 个 collector、指标 namespace/subsystem/name/help、桶参数、标签次序及标签字符串；`SessionRetry` 的 Rust `(0..21)` 与 Go `LinearBuckets(0, 1, 21)` 等价。`PessimisticLockKeysDuration` 也保留 Go 注释要求的历史 `tikvclient` subsystem。

主要结构差异是：Go 包变量在 `InitSessionMetrics` 前为 nil 接口/指针，Rust 用 `Option<T>` 表示；Go 工厂直接返回 collector，Rust 通过兼容层把 Go 风格 options 转成 `prometheus` crate 类型；Rust 新增两个 `*Vec` 访问器，为拆分 crate 的事务状态代码提供按需初始化和共享句柄。Go 包初始化天然集中，Rust 因 crate 拆分还允许同一源码被路径装入或内联，因此每个 crate 实例拥有自己的静态量，不能假定不同 crate 实例自动共享同一 registry。

相关 Go 测试 `pkg/metrics/metrics_test.go` 覆盖整体初始化/注册；Rust 对应 `pkg/metrics/metrics_test.rs::test_register_metrics`。目标文件没有同名独立测试，细分行为由 `pkg/session/metrics/migration_aster_unit_test.rs`（标签预绑定别名）、`pkg/session/txninfo/migration_aster_unit_test.rs::metric_accessors_select_the_go_state_and_lock_labels`、`pkg/executor/metrics/migration_aster_unit_test.rs`（执行器预绑定与观测）以及 `pkg/metrics/telemetry_test.rs::telemetry_getters_read_the_shared_session_and_executor_metrics` 覆盖。

## 扩展指南

新增 Session 指标时，应同时完成以下局部接线：

1. 在本文件增加 `pub static mut Option<...>`，并在 `InitSessionMetrics` 用四个工厂之一构造；指标名、help、桶和标签先与 Go 对照或既有监控契约核准。
2. 若它属于主 metrics registry，在 `pkg/metrics/metrics.rs::RegisterMetrics` 的 session 列表中加入该静态量；只构造而不注册不会出现在主采集端。
3. 在真正的业务所有者中按标签顺序预绑定/观测。Session 侧通常接入 `pkg/session/metrics/metrics.rs`，Executor 侧接入 `pkg/executor/metrics/metrics.rs`；事务状态映射则接入 `pkg/session/txninfo/txn_info.rs`。
4. 与 Go 移植保持一致时同步 `pkg/metrics/session.go` 的语义证据；若 Rust 有意不同，必须在实现和独立测试中说明差异。不要在 `session.rs` 内嵌测试，遵守测试独立文件约束。
5. 根据影响面扩展现有独立测试：注册可见性放在 `pkg/metrics/metrics_test.rs`，Session 标签别名放在 `pkg/session/metrics/migration_aster_unit_test.rs`，Executor 绑定放在 `pkg/executor/metrics/migration_aster_unit_test.rs`，事务状态访问器放在 `pkg/session/txninfo/migration_aster_unit_test.rs`。

兼容风险集中于时间序列名称、label 名称/顺序和桶边界；性能风险集中于新增高基数标签及在热路径频繁查找 label child。并发风险集中于重复替换 `static mut`；除非整体迁移初始化模型，否则应复用已有一次性初始化路径。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/metrics` 找到 `session.rs`；`node --file pkg/metrics/session.rs` 读取了完整 428 行并报告该文件被 9 个文件使用；`query InitSessionMetrics --kind function` 找到 Rust 与 Go 两个定义。`callers/callees` 精确查询在 30 秒窗口内未返回结果，因此调用关系另以源码引用核验。
- 目标和 crate：`pkg/metrics/session.rs`、`pkg/metrics/lib.rs`、`pkg/metrics/Cargo.toml`、`pkg/metrics/common/wrapper.rs`、`pkg/metrics/bindinfo.rs`。
- 初始化、注册和消费：`pkg/metrics/metrics.rs`、`pkg/session/metrics/lib.rs`、`pkg/session/metrics/metrics.rs`、`pkg/executor/metrics/lib.rs`、`pkg/executor/metrics/metrics.rs`、`pkg/session/txninfo/txn_info.rs`、`pkg/util/metricsutil/common.rs`、`pkg/metrics/telemetry.rs`。
- Go 对照和测试：`pkg/metrics/session.go`、`pkg/metrics/metrics_test.go`、`pkg/metrics/metrics_test.rs`、`pkg/metrics/telemetry_test.rs`、`pkg/session/metrics/migration_aster_unit_test.rs`、`pkg/session/txninfo/migration_aster_unit_test.rs`、`pkg/executor/metrics/migration_aster_unit_test.rs`。
- 本任务为纯文档分析，按计划不运行 Cargo。仓库说明提及的 `.agents/skills/tidb-verify-profile` 在当前检出中不存在，因此无法执行其额外 Ready 工作流；任务文件指定的 11 章节结构检查是本次可执行的交付验证。
