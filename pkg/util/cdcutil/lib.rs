// Copyright 2026 AsterSQL.

// cdcutil 包入口：TiCDC changefeed 枚举与安全时间戳兼容性检查。
//
// 对外公开 `cdc` 模块 API；测试配置下挂载导出辅助、嵌入式 etcd 测试与迁移回归用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// CDC 巡检核心实现。
pub mod cdc;
/// 重新导出 `cdc` 中的公开类型与函数。
pub use cdc::*;

/// 测试专用：`TESTGetChangefeedNames` 等导出。
#[cfg(test)]
#[path = "export_for_test.rs"]
mod export_for_test;

/// 嵌入式内存 etcd 风格的行为测试。
#[cfg(test)]
#[path = "cdc_test.rs"]
mod cdc_test;

/// 迁移期回归：键版本枚举、checkpoint 过滤与错误上下文。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
