// Copyright 2026 AsterSQL.

// Store Helper（存储层辅助工具）crate 入口。
//
// 提供通过 TiKV / PD（Placement Driver，集群元数据与调度服务）查询 Region
// （键空间分片）、Store 信息、表与索引区域分布等辅助能力，供诊断与管理接口使用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// Helper 主实现：Region/Store 查询、表区域扫描等。
pub mod helper;
pub use helper::*;

/// Aster 迁移对照的 Helper 单元测试。
#[cfg(test)]
mod helper_1_aster_unit_test {
    include!("helper_1_aster_unit_test.rs");
}

/// Helper 行为与边界条件的集成风格测试。
#[cfg(test)]
#[path = "helper_test.rs"]
mod helper_test;

/// 对应 Go TestMain 的测试环境初始化校验。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
