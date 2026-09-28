// Copyright 2026 AsterSQL.

// 每日基准结果采集与汇总工具 crate 入口。
//
// 对应 Go `util/benchdaily`：提供基准运行、JSON 读写与每日结果合并能力；
// 测试模块覆盖迁移期语义、TestMain 泄漏检查配置与汇总扫描逻辑。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 每日基准核心实现（运行、序列化、文件 IO）。
pub mod bench_daily;
pub use bench_daily::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期单元测试：JSON 往返、caller 名与 run_to_file。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "bench_daily_test.rs"]
/// 每日汇总扫描与合并逻辑的单元测试。
mod bench_daily_test;
