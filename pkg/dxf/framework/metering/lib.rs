// Copyright 2026 AsterSQL.

// DXF 计量子系统入口：数据模型、Meter 状态机与 Recorder。
//
// 负责在任务执行期间累计资源用量，并按间隔 flush 到计量写入器（对象存储等）。

#![allow(non_snake_case, non_upper_case_globals)]

/// 计量数据模型与增量计算。
#[path = "data.rs"]
pub mod data;

/// Meter 状态机：注册 recorder、定时 flush、失败重试。
pub mod metering {
    include!("metering.rs");
}
/// 单任务用量累计器（原子计数）。
#[path = "recorder.rs"]
pub mod recorder;

/// 再导出 data 公共 API。
pub use data::*;
/// 再导出 metering 公共 API。
pub use metering::*;
/// 再导出 recorder 公共 API。
pub use recorder::*;

#[cfg(test)]
/// 迁移对照用综合单元测试。
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
/// data 模块单元测试。
#[path = "data_test.rs"]
mod data_test;

#[cfg(test)]
/// metering 模块单元测试（include 源文件）。
mod metering_test {
    include!("metering_test.rs");
}

#[cfg(test)]
/// recorder 模块单元测试。
#[path = "recorder_test.rs"]
mod recorder_test;
