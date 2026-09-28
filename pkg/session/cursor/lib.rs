// Copyright 2026 AsterSQL.

// 会话游标（Cursor）子模块入口。
//
// 导出游标状态（`state`）与跟踪器（`tracker`），供服务端协议层管理
// 服务端游标的打开、取行与关闭；测试模块按需挂接。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 游标状态机与生命周期。
pub mod state;
/// 游标资源跟踪与统计。
pub mod tracker;
pub use state::*;
pub use tracker::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "tracker_test.rs"]
mod tracker_test;
