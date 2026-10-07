# `pkg/executor/metrics/metrics.rs`

## 文件定位

本文件是 `astersql-executor-metrics` crate 中的“预绑定句柄层”：底层 Prometheus 向量由 `pkg/metrics/executor.rs`、`pkg/metrics/session.rs` 与 `pkg/metrics/server.rs` 创建，本文件再用固定标签取得 `Observer`、`Counter`、`Gauge` 句柄，供执行器热路径直接打点。crate 入口 `pkg/executor/metrics/lib.rs` 以 `executor_metrics` 模块加载本文件，并重导出上述父级指标定义；`pkg/executor/metrics/Cargo.toml` 声明 crate 名、`lib.rs` 入口和直接 `prometheus = "0.14"` 依赖，porting 元数据指向 Go 包 `pkg/executor/metrics`。

生产侧目前直接调用本文件辅助函数的入口集中在 `pkg/executor/adapter.rs`：语句执行结束、执行器锁统计、阶段耗时和补充指标都从这里进入。RustCodeGraph 的文件反向边也只列出该 adapter 与独立迁移测试 `pkg/executor/metrics/migration_aster_unit_test.rs`。其余大量公开静态句柄主要是对 Go 公共包变量的移植兼容面；当前 Rust 全仓直接引用搜索没有发现它们被其他生产 Rust 文件逐项消费。

## 核心职责

1. 定义 15 个稳定的执行阶段标签：锁定读/最终执行各自的 build、open、next、lock，2PC 的 prewrite、commit 和四类等待，以及写响应阶段（`PhaseBuildLocking` 至 `PhaseWriteResponse`）。
2. 在 `InitMetricsVars` 中把父级向量按固定标签预绑定，覆盖 general/internal 查询、悲观 DML 尝试、Fair Locking、昂贵执行器类型、事务回滚、MPP 协调器、跨可用区网络传输和 IndexLookUp。
3. 在 `InitPhaseDurationObserverMap` 中把阶段常量映射到 general/internal 两组预绑定 Observer，让调用方可按阶段字符串查表。
4. 通过五个 adapter 专用函数实施有条件打点：`RecordPhaseDuration`、`RecordFairLockingFinishMetrics`、`RecordExecLockMetrics`、`RecordSupplementaryFinishMetrics`、`RecordStatementExecuteRunDuration`。

本文件不创建或注册 Prometheus collector，也不负责导出 HTTP metrics；它要求父级指标先初始化，随后仅派生和缓存带固定标签的子句柄。

## 主要符号

- `Phase*` 常量：Prometheus `ExecPhaseDuration` 的 `phase` 标签值；冒号形式是实际时序标签。`RecordPhaseDuration` 接受的 adapter 输入却是下划线形式，例如 `build_locking`，函数内部再映射为 `build:locking`。
- `init()`：顺序调用 `InitMetricsVars()` 和 `InitPhaseDurationObserverMap()` 的完整初始化入口。Rust 没有 Go 包级自动 `init`，所以只有显式调用才会执行。
- `InitMetricsVars()`：通过局部 `bind!` 宏读取父级 `Option<*Vec>`，用 `with_label_values` 派生子句柄，并写入约八十个 `static mut Option<_>`。父级未初始化时，`expect("... must be initialized first")` 会 panic。
- `InitPhaseDurationObserverMap()`：克隆已绑定的阶段 Observer，构造两张各 15 项的 `HashMap<&'static str, Observer>`。任何阶段句柄仍为 `None` 时，`unwrap()` 会 panic；`ExecUnknown`/`ExecUnknownInternal` 虽被预绑定，却不加入 map。
- `RecordPhaseDuration(phase, internal, duration)`：只接受 14 个下划线式 adapter 阶段名（不接受 `write_response`），选择 internal/general map，并以秒为单位观察；未知阶段或 map 未初始化时直接返回。
- `RecordFairLockingFinishMetrics(...)`：四个布尔值彼此独立，为真时分别增加 statement/transaction 的 used/effective counter。
- `RecordExecLockMetrics(...)`：仅在重试数、独占键数、共享键数为正时观察对应 session 直方图；悲观锁已经开始且独占锁耗时大于零时才记录耗时。
- `RecordSupplementaryFinishMetrics(...)`：TiFlash 成功增加 `TotalTiFlashQuerySuccCounter`；失败则把 RFC 错误码经 `server::ExecuteErrorToLabel` 规范化，并在 `TiFlashQueryTotalCounter(error_label, error)` 上计数；读表缓存标志独立计数。
- `RecordStatementExecuteRunDuration(internal, duration)`：按 restricted/internal 标志选择会话执行 Run 阶段直方图并记录秒数。
- 各组 `static mut Option<_>`：初始化前明确为空；类型分别是 `Observer`、`Counter`、`Gauge`，阶段表为 `HashMap`。它们是全局可变兼容状态，不是按请求创建的状态。

## 执行流程

完整预期初始化顺序如下：

1. `pkg/util/metricsutil/common.rs::initParentMetricsCollectors` 先调用 executor-metrics crate 内的 `server::InitServerMetrics`、`session::InitSessionMetrics`、`metric_executor::InitExecutorMetrics`，创建本文件所依赖的父级向量。
2. `pkg/util/metricsutil/common.rs::initMetrics` 再调用 `executor_metrics::InitMetricsVars`，把固定标签绑定成可直接 `inc`/`observe` 的句柄。
3. 若调用完整的本地 `init()`，其后还会执行 `InitPhaseDurationObserverMap`，使阶段查表可用；独立迁移测试使用的就是父级初始化后调用 `subject::init()` 的完整路径。
4. `pkg/executor/adapter.rs::Exec` 在捕获执行 panic 后读取运行时锁统计并调用 `RecordExecLockMetrics`。
5. adapter 的语句完成路径依次记录网络/RU/慢查询等外部状态，然后调用补充 TiFlash/缓存指标、阶段耗时、Run 耗时及 Fair Locking 指标；`PhaseDurationObserver::Observe` 还会继续调用 runtime 自身的 `ObservePhase`，所以 Prometheus 辅助打点并非唯一阶段观测通道。

当前生产接线有一处必须如实区分：`pkg/util/metricsutil/common.rs::initMetrics` 只调用 `InitMetricsVars`，全仓直接搜索未发现生产代码调用本文件的 `init()` 或 `InitPhaseDurationObserverMap()`。因此现有生产初始化路径下，普通预绑定句柄可用，但 `PhaseDurationObserverMap`/`PhaseDurationObserverMapInternal` 仍为 `None`，`RecordPhaseDuration` 会静默跳过 Prometheus 阶段观测。独立测试覆盖的是完整 `subject::init()` 路径，不能证明这段生产接线已经完成。

## 数据与状态

状态分为三层：父级向量、固定标签子句柄、阶段查找表。父级向量位于 crate 通过 `#[path]` 纳入的 `pkg/metrics/*.rs`；`InitMetricsVars` 取得子句柄后写入全局 `Option`；阶段表再克隆这些 Observer 句柄。克隆的是指向同一 collector/label series 的句柄语义，迁移测试通过“从 map 观察后原句柄 sample count 增加”验证共享关系。

标签是不变量的一部分：general/internal 使用 `metrics::LblGeneral`/`LblInternal` 或 phase 指标的 `"0"`/`"1"`；事务回滚维度依次是事务模式、rollback、会话类型；网络、MPP、IndexLookUp 和执行器类型均使用与 Go 文件逐字一致的固定字符串。修改标签值或顺序会产生新的时序或破坏仪表盘兼容性。

本文件没有请求级缓存。所有计数和观测最终累积进 Prometheus collector；两个 `HashMap` 在初始化后只读使用，但类型层面仍是 `static mut`。重复调用初始化会覆盖全局句柄和 map，而非合并 Rust 侧容器。

## 依赖与调用关系

上游关系：

- `pkg/util/metricsutil/common.rs::{initParentMetricsCollectors,initMetrics}` 提供生产初始化顺序；前者创建父级 collector，后者调用 `InitMetricsVars`。
- `pkg/executor/adapter.rs::Exec` 调用 `RecordExecLockMetrics`。
- `pkg/executor/adapter.rs` 的语句结束路径调用 `RecordSupplementaryFinishMetrics`、`RecordStatementExecuteRunDuration` 与 `RecordFairLockingFinishMetrics`。
- `pkg/executor/adapter.rs::PhaseDurationObserver::Observe` 调用 `RecordPhaseDuration`。
- `pkg/executor/metrics/migration_aster_unit_test.rs::initializes_all_go_prebound_metrics_and_phase_maps` 是本文件的独立 Rust 回归测试。

下游关系：

- `crate::metrics` 汇集 `pkg/metrics/executor.rs`、`session.rs`、`server.rs` 的向量和公共标签；`bind!` 最终调用兼容层的 `with_label_values`。
- `crate::session::{StatementPessimisticRetryCount, StatementLockKeysCount, StatementSharedLockKeysCount, PessimisticLockKeysDuration}` 接收执行锁统计。
- `crate::server::{TiFlashQueryTotalCounter, ReadFromTableCacheCounter, ExecuteErrorToLabel}` 接收 TiFlash 失败及表缓存指标。
- `std::collections::HashMap` 仅服务于阶段名到 Observer 的分派。

RustCodeGraph 对五个 helper 及两个初始化函数的精确 `callers/callees` 查询没有返回函数边，因此上述函数级关系以全仓直接引用和相邻源码为补充证据；文件级图边与这些结果一致。

## 错误处理与边界

这些 API 都不返回 `Result`。初始化错误通过 panic 暴露：父级指标缺失触发 `bind!` 中的 `expect`，阶段子句柄缺失触发 map 初始化中的 `unwrap`。运行时辅助函数则偏向“指标失败不影响 SQL”：句柄为 `None` 时不打点，未知 phase 直接返回，零值/负的键数和零耗时按条件忽略。

边界细节包括：

- `RecordPhaseDuration` 当前只映射 14 个 adapter 名称，虽然 map 内还有 `write-response`；未知名称不会落到 `ExecUnknown`。`ExecUnknown` 仅被初始化，没有在该 helper 或 map 中使用。
- TiFlash 失败只有在 `tiflash == true && success == false` 时计数；成功路径不使用 RFC 错误码。表缓存计数与 TiFlash 分支相互独立。
- 锁耗时要求 `pessimistic_lock_started` 且 `exclusive_duration > Duration::ZERO`；共享键只有数量直方图，没有对应耗时分支。
- Fair Locking 四个条件不互斥，单次完成可同时增加四个 counter。
- duration 统一通过 `as_secs_f64()` 转为秒；MPP latency 等其他预绑定 Observer 的原始单位由其父级定义和调用者约定决定，本文件不做单位转换。

## 并发与资源生命周期

Prometheus 句柄自身可克隆并共享；map 中的 Observer clone 与独立静态句柄指向同一时序。指标没有显式释放阶段，生命周期与进程及 collector registry 一致。

并发风险来自 `static mut`：本文件用 `unsafe` 读写全局 `Option` 和 map，Rust 类型系统不能保证初始化与并发打点之间无数据竞争。父级指标创建受 `pkg/executor/metrics/lib.rs::metrics::PACKAGE_INIT_LOCK` 保护，但该锁并不包围本文件的 `InitMetricsVars` 或 `InitPhaseDurationObserverMap`。正确使用依赖启动期单线程、先初始化后服务请求且不再重新初始化的外部约束。新增并发重载/重注册功能时，不应继续依赖这一未编码的不变量，宜改为 `OnceLock`/`LazyLock` 或锁保护的整体状态。

## 与 Go 版本的对应关系

`pkg/executor/metrics/metrics.go` 是直接对照：阶段常量、包级句柄集合、固定标签值以及两张 15 项 phase map 均逐项保持。Go 的包级 `init()` 会自动先运行 `InitMetricsVars` 再运行 `InitPhaseDurationObserverMap`；Rust 将字段改成 `Option` 并提供显式 `init()`，这是最关键的生命周期差异。

五个 `Record*` 函数不是 Go 同路径文件中的 API，而是 Rust adapter 为承接 Go `pkg/executor/adapter.go` 分散的直接指标操作增加的局部桥接。它们保留了 Go 的条件分支：例如 fair-locking 在语句/事务完成时分别计数，phase 按 internal 标志选表，网络/阶段/锁统计仍由 adapter 的对应完成点触发。

迁移状态不是完全等价：Go 自动初始化两张 phase map；Rust 当前生产初始化链只调用 `InitMetricsVars`。此外，Go 生产文件会在 distsql、MPP manager 等处直接使用 IndexLookUp、MPP、network 句柄，而当前 Rust 直接引用主要集中在 adapter helper；未发现的 Rust 消费点应记录为尚未接线，而不能仅凭公开静态变量声称已使用。

## 扩展指南

- 新增父级指标时，先在所属的 `pkg/metrics/*.rs` 创建向量，再在 `InitMetricsVars` 以完全相同的标签顺序绑定；同时对照 `pkg/executor/metrics/metrics.go`，更新独立测试的 `assert_initialized!` 列表。
- 新增执行 phase 时，要同步阶段常量、general/internal 两组句柄、`InitMetricsVars`、两张 map、`RecordPhaseDuration` 的 adapter 名称转换，以及测试中“两张 map 的长度和成员”断言。还需决定 unknown/write-response 是否应由 helper 接受，避免只创建句柄却无法路由。
- 若修复生产 phase map 接线，应在 `pkg/util/metricsutil/common.rs::initMetrics` 调用完整 `executor_metrics::init()`，或在 `InitMetricsVars` 后明确调用 `InitPhaseDurationObserverMap`；必须增加独立集成级测试证明真实注册路径后 map 非空，而不只测试 crate 内手工 `subject::init()`。
- 调整指标标签、名称或单位具有监控兼容风险；尤其不能随意改变 `"0"/"1"`、general/internal、事务三维标签顺序和 Go 既有字符串。
- 修改全局初始化或支持动态重载时，需要先消除 `static mut` 的并发不安全设计。测试逻辑应继续放在 `pkg/executor/metrics/migration_aster_unit_test.rs` 等独立文件，不内嵌进生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/executor/metrics` 找到 `lib.rs`、`metrics.rs`、Go 对照和迁移测试；`node --file pkg/executor/metrics/metrics.rs` 完整读取 550 行并报告该文件被 `pkg/executor/adapter.rs` 与迁移测试使用；`query` 精确定位五个 `Record*`、`InitMetricsVars`、`InitPhaseDurationObserverMap`。精确 `callers/callees` 未产生函数边，已用直接引用搜索补证。
- Rust 源：`pkg/executor/metrics/metrics.rs`；调用入口 `pkg/executor/adapter.rs`；父级指标 `pkg/metrics/executor.rs`；生产初始化 `pkg/util/metricsutil/common.rs`；crate 入口 `pkg/executor/metrics/lib.rs`。
- crate 声明：`pkg/executor/metrics/Cargo.toml`；消费依赖：`pkg/executor/Cargo.toml`。
- Go 对照：`pkg/executor/metrics/metrics.go`；相关 Go 生产消费点通过 `rg` 核对了 `pkg/executor/adapter.go`、`pkg/executor/distsql.go` 与 `pkg/executor/mppcoordmanager/mpp_coordinator_manager.go`。
- 独立 Rust 测试：`pkg/executor/metrics/migration_aster_unit_test.rs::initializes_all_go_prebound_metrics_and_phase_maps` 验证父级标签、两张各 15 项 map、共享 Observer、五个 helper 的代表分支及全部预绑定句柄非空。同目录没有 Go `*_test.go` 专门测试此文件，相关 Go 行为由上述生产调用点提供直接证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含规定的 11 个二级标题；事实人工复核特别检查了初始化先后、当前生产 phase map 缺口、条件打点边界及 Go/Rust 生命周期差异。
