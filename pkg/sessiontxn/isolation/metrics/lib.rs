// Copyright 2026 AsterSQL.

// 隔离级别指标（isolation/metrics）子 crate 入口。
//
// 声明 Read Committed（RC，读已提交）检查时间戳相关的 Prometheus 标签与源计数器，
// 再挂载本目录 `metrics.rs` 中的具体计数器句柄初始化逻辑。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

extern crate self as astersql_sessiontxn_isolation_metrics;

/// 指标标签与上游 `CounterVec` 占位，供 `isolation_metrics` 绑定固定标签。
pub mod metrics {
    /// RC 读路径检查时间戳（RCCheckTS）冲突计数的标签值。
    pub const LblRCReadCheckTS: &str = "read_check";
    /// RC 写路径检查时间戳冲突计数的标签值。
    pub const LblRCWriteCheckTS: &str = "write_check";

    /// 上游按 `type` 标签区分的写冲突计数器向量；须在隔离指标初始化前由全局 metrics 注入。
    pub static mut RCCheckTSWriteConfilictCounter: Option<prometheus::CounterVec> = None;
}

/// 对应 Go `sessiontxn/isolation/metrics`：从源 `CounterVec` 抽出带固定标签的 Counter。
#[path = "../../../../pkg/sessiontxn/isolation/metrics/metrics.rs"]
pub mod isolation_metrics;

/// 迁移期单元测试：验证标签绑定与源计数器更换后的句柄重绑。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
