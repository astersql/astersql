// Copyright 2026 AsterSQL.
// 跨 Keyspace（keyspace：多租户/多命名空间隔离单元）运行时管理模块入口。
// 聚合协调器、跨 KS 会话管理、DDL 提交与最小 start_ts 上报等子模块。

#![allow(dead_code)]
/// Schema 变更跨 KS 协调器子模块。
pub mod coordinator;
/// 跨 Keyspace SessionManager 生命周期与持有者管理。
pub mod cross_ks;
/// 跨 KS 场景下的 DDL（如 Alter Table Mode）提交客户端。
pub mod ddl_submit;
/// 最小事务 start_ts（时间戳序）上报占位实现。
pub mod reporter;
/// 再导出协调器公共 API。
pub use coordinator::*;
/// 再导出跨 KS 管理公共 API。
pub use cross_ks::*;
/// 再导出 DDL 提交公共 API。
pub use ddl_submit::*;
/// 再导出上报器公共 API。
pub use reporter::*;

#[cfg(test)]
/// Schema 协调器锁语义与 Go 实现的一致性测试。
#[path = "coordinator_test.rs"]
mod coordinator_test;
#[cfg(test)]
/// 跨 KS Manager 内部行为单测。
#[path = "cross_ks_internal_test.rs"]
mod cross_ks_internal_test;
#[cfg(test)]
/// 跨 KS 管理器与 Alter Table Mode 集成风格单测。
#[path = "cross_ks_test.rs"]
mod cross_ks_test;
#[cfg(test)]
#[path = "ddl_submit_test.rs"]
mod ddl_submit_test;
#[cfg(test)]
/// 对应 Go export_test 的 Get/CloseKS 辅助测试。
#[path = "export_test.rs"]
mod export_test;
