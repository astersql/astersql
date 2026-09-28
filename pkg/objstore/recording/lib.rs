// Copyright 2026 AsterSQL.
// 对象存储访问统计（recording）crate 入口。
//
// 聚合 `recording` 模块并再导出请求次数与流量字节计数类型，供备份/对象 I/O
// 在读写对象时无锁累加观测指标。测试通过 path 属性挂载到本 crate。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 访问统计核心类型与记录 API。
pub mod recording;
pub use recording::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "recording_test.rs"]
mod recording_test;
