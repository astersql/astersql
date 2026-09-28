// Copyright 2026 AsterSQL.

// telemetry 包：集群遥测数据采集、上报与 TTL（生存时间）用量统计。
//
// 聚合特征用量、数据窗口计数、上报调度与 TTL 作业直方图等子模块，
// 并向外 re-export；测试模块按文件路径挂载。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]
/// 遥测载荷数据结构。
mod data;
/// 特征用量采集。
mod data_feature_usage;
/// 时间窗口内的用量计数。
mod data_window;
/// 上报开关、会话上下文与上报入口。
mod telemetry;
/// TTL 作业用量与直方图。
mod ttl;
pub use data::*;
pub use data_feature_usage::*;
pub use data_window::*;
pub use telemetry::*;
pub use ttl::*;

/// 特征用量测试。
#[cfg(test)]
#[path = "data_feature_usage_test.rs"]
mod data_feature_usage_test;
/// 数据窗口测试。
#[cfg(test)]
#[path = "data_window_test.rs"]
mod data_window_test;
/// 测试入口与导出辅助绑定。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 遥测开关与上报入口测试。
#[cfg(test)]
#[path = "telemetry_test.rs"]
mod telemetry_test;
