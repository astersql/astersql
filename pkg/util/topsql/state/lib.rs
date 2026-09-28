// Copyright 2026 AsterSQL.

// TopSQL 状态 crate 入口。
//
// 导出全局 TopSQL / TopRU（按 RU 聚合的 Top SQL）开关与配置；
// 测试配置下挂载迁移回归与 Go 对应测试。

extern crate self as topsql_state;

#[path = "state.rs"]
/// TopSQL / TopRU 全局状态实现。
mod state;
pub use state::*;

#[cfg(test)]
#[path = "test_util.rs"]
/// 全局状态测试的共享串行化工具。
mod test_util;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "state_test.rs"]
/// 对应 Go 的状态单元测试。
mod state_test;
