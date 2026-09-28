// Copyright 2026 AsterSQL.

// `util/paging` crate 入口：分布式 SQL 分页请求大小增长与 seek 次数估算。
//
// 对应 Go `pkg/util/paging`。分页（paging）把大范围扫描拆成多批请求并逐批放大
// page size，使调用方可尽早拿到数据；本 crate 导出纯数值计算 API 与相关测试。

/// 分页常量与 GrowPagingSize / CalculateSeekCnt 等函数。
pub mod paging;
/// 再导出 paging 模块公开 API。
pub use paging::*;

#[cfg(test)]
#[path = "paging_test.rs"]
/// 分页算法单元测试。
mod paging_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移回归：增长边界与 seek 次数对照 Go。
mod migration_aster_unit_test;
