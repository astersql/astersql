// Copyright 2026 AsterSQL.

//! BR 摘要日志包入口：对齐 Go `br/pkg/summary`。
//! `collector` 实现可注入 logger 的聚合器；`summary` 提供包级全局便捷函数。
//! 测试模块用 `#[path]` 挂入，与源文件分离，符合仓库约定。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 核心聚合实现（LogCollector、Field、单位常量等），对应 Go collector.go。
#[path = "collector.rs"]
pub mod collector;

// 包级全局入口（Collect*/Summary/Succeed 等），对应 Go summary.go 的包函数。
#[path = "summary.rs"]
pub mod summary;

// 对外 re-export：调用方 `use br_summary::...` 即可，无需深入子模块路径。
pub use collector::{
    BackupDataSize, BackupUnit, ContextCanceled, Field, FieldValue, InitCollector, LogCollector,
    NewLogCollector, RestoreDataSize, RestoreUnit, SetLogCollector, SkippedBytesByCheckpoint,
    SkippedKVCountByCheckpoint, SummaryError, SummaryValue, TotalBytes, TotalKV, log_key_for,
    units, zap,
};
// summary 侧多为薄包装，转发到全局 collector，保持与 Go 包函数签名对齐。
pub use summary::{
    AdjustStartTimeToEarlierTime, CollectDuration, CollectFailureUnit, CollectInt,
    CollectSuccessUnit, CollectUint, Log, NowDureTime, SetSuccessStatus, SetUnit, Succeed, Summary,
};

// 与 Go 行为对照的 parity 断言（字段累加、成功/失败模板等）。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

// 对应 Go collector_test.go 的单元场景。
#[cfg(test)]
#[path = "collector_test.rs"]
mod collector_test;

// 测试入口/共享初始化，对应 Go main_test.go。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
