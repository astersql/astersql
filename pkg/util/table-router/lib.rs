// Copyright 2026 AsterSQL.

// 表路由 crate：按模式把源库表映射到目标库表，并导出 selector。
//
// 用于数据迁移/同步时重写 schema.table 名称；测试模块挂接 router 与迁移用例。

#![allow(non_snake_case, non_upper_case_globals)]

/// 再导出 table-rule selector，供路由匹配选用。
pub mod selector {
    pub use astersql_util_table_rule_selector::*;
}

/// 表路由实现：规则增删改与 Route/FetchExtendColumn。
pub mod router;
pub use router::*;

#[cfg(test)]
/// 路由单元测试（include router_test.rs）。
mod router_test {
    use crate::router::*;
    use crate::selector;
    include!("router_test.rs");
}

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 与 Go 行为对齐的迁移对照测试。
mod migration_aster_unit_test;
