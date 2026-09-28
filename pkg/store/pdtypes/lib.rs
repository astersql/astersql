// Copyright 2026 AsterSQL.

// pdtypes crate 入口：PD 相关类型定义。
//
// 聚合 PD API（Store/Region）、复制配置、placement rule、Region 树、
// 统计与类型工具等模块，供调度与元数据路径使用。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

/// 容量与时长等配置基础类型（再导出）。
pub mod configtypes {
    pub use astersql_config_configtypes::{ByteSize, Duration};
}

/// Store / Region 等 PD API 结构。
pub mod api;
/// 复制与 placement 开关等配置。
pub mod config;
/// Placement rule、标签约束与 Peer 角色。
pub mod placement;
/// Region 区间树（重叠替换与按键扫描）。
pub mod region_tree;
/// Region 统计信息。
pub mod statistics;
/// 字符串切片等序列化辅助。
pub mod typeutil;

#[cfg(test)]
#[path = "statistics_test.rs"]
mod statistics_test;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "placement_test.rs"]
mod placement_test;

/// AsterSQL 迁移补充单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
