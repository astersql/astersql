# `pkg/planner/mview/stats.rs`

## 文件定位

仅再导出 core 的 mlog commit-ts 窗口和选择率工具；实际算法位于 [`../core/stats.rs`](../core/stats.rs)，本文件不读取统计或解析表达式。

## 核心职责

仅再导出 core 的 mlog commit-ts 窗口和选择率工具；实际算法位于 [`../core/stats.rs`](../core/stats.rs)，本文件不读取统计或解析表达式。

## 主要符号

完整导出集合为 `DeriveMLogStatsByFilter`、`EstimateMLogCommitTSSelectivity`、`MLogCommitTSFilterWindow`、`MLogFilterBound`、`MLogFilterOp`、`NormalizeMLogCommitTSInclusiveLower`；没有导出 Go 的 `SplitMLogCommitTSFilterSelectivity`。

## 执行流程

调用方显式提供 normalized bound。core AddBound 对列在右边时反转比较符，对 >= 下界折算前一物理毫秒，取最紧上下界。Estimate 用 retained upper/lower 与过滤窗口求交；lower 缺失时回退 table_update_tso。以 TSO >>18 的物理毫秒计算比例并 clamp 到 [0,1]。Derive 将 table_rows 乘此比例，再乘剩余条件默认选择率 `0.8^remaining_conditions`。本文件没有额外阶段。

## 数据与状态

窗口包含 lower_tso/upper_tso 和 has_lower/has_upper。外部 `planctx::MLogCommitTSEstimation` 提供 MLogTableID、RetainedLowerTSO、RetainedUpperTSO；这些工具不据表 ID 自动筛选目标 mlog，调用者负责匹配。返回 StatsInfo 仅填 row_count，其余默认；本文件没有缓存或会话状态。

## 依赖与调用关系

由 mview.rs 装配并导出，planner Cargo 依赖 core。core 统计工具的独立直接调用证据在 `core/casetest/mview/mv_refresh_test.rs`；导出本身不证明真实 DataSource 过滤已调用这些函数。

## 错误处理与边界

Option::None 表示不能可靠估计：缺 upper/lower、物理时间跨度为零，或 fallback lower 不能证明空交集。显式 retained lower 能证明无交集时返回 Some(0.0)；fallback lower 的某些无交集情形仍返回 None。不会读取/删除 CNF 条件，也不会自行验证 schema、mlog identity 或常量可求值性。归一化 physical=0 时保留原值。

## 并发与资源生命周期

只含 pub use，无自身资源。core 工具是同步内存计算，窗口为 Copy 值，无线程、锁、存储读取或统计更新。

## 与 Go 版本的对应关系

Rust 导出的是手工 bound 输入的估计子集，不能声称 Go 的自动提取/接线已实现；物理毫秒窗口和 fallback 边界仍需按实现区分。

## 扩展指南

新增真实 CNF 接线时在 owning core 实现中处理表达式求值、目标 mlog 匹配、未识别条件保留和 fallback，保留 None 与 Some(0) 的差异。修改算法应更新独立 Rust 测试并对照 Go `stats_test.go`，本文件只同步导出清单。
