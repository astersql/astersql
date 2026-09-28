// Copyright 2026 AsterSQL.

// 正则/通配表路由（regexpr-router）库入口。
//
// 通过 `include!` 嵌入 `regexpr_router.rs` 实现，并重新导出其公共 API；
// 测试模块分别覆盖迁移对齐与 Go 对照用例。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as regexpr_router;
mod regexpr_router_impl {
    include!("regexpr_router.rs");
}
pub use regexpr_router_impl::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "regexpr_router_test.rs"]
mod regexpr_router_test;
