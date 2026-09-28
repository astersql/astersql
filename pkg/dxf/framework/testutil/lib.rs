// Copyright 2026 AsterSQL.

// DXF framework 测试工具 crate 的模块入口。
//
// 聚合 context、分布式测试、执行器/调度器/任务表辅助函数，
// 供 DXF（Distributed eXecution Framework：分布式任务执行框架）集成测试复用。

#![allow(non_snake_case)]

/// 测试上下文与模拟集群节点状态。
pub mod context;
/// 分布式任务执行扩展与提交等待辅助。
pub mod disttest_util;
/// 任务执行器注册辅助。
pub mod executor_util;
/// Mock 调度器扩展构造。
pub mod scheduler_util;
/// 任务表（task table）测试夹具。
pub mod table_util;
/// 子任务写入与 keyspace 辅助。
pub mod task_util;

/// 再导出各子模块的公开 API，便于测试直接 `use astersql_dxf_framework_testutil::*`。
pub use context::*;
pub use disttest_util::*;
pub use executor_util::*;
pub use scheduler_util::*;
pub use table_util::*;
pub use task_util::*;

#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "scheduler_util_test.rs"]
mod scheduler_util_test;

#[cfg(test)]
#[path = "table_util_test.rs"]
mod table_util_test;
