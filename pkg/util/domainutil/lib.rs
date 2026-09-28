// Copyright 2026 AsterSQL.

// Domain 修复模式工具 crate 入口。
//
// 对应 Go `pkg/util/domainutil`：在元数据损坏修复（admin repair table）场景下，
// 管理待修复库表列表与已缓存的 `DBInfo`/`TableInfo`。`model` 再导出元数据模型；
// `repair_vars` 实现修复状态机；测试通过 `#[path]` 挂载迁移回归用例。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 再导出元数据模型中的库/表信息类型，供修复逻辑与测试使用。
pub mod model {
    pub use meta_model::group_1::{DBInfo, TableInfo};
}
/// 修复模式状态、待修复表列表与 session 缓存键。
mod repair_vars;
/// 对外导出 `repair_vars` 中的公开类型与函数。
pub use repair_vars::*;

/// 迁移期单元测试：修复模式开关、大小写不敏感匹配与缓存行为。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
