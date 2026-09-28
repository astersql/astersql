// Copyright 2026 AsterSQL.

// `astersql-infoschema-context` crate 入口。
//
// 将 meta_model / placement 依赖再导出为本地 `ast`、`model`、`placement` 模块，
// 并公开 InfoSchema 上下文侧的特殊属性过滤器与仅元数据接口（见 `infoschema` 模块）。
// 单元测试通过 `migration_aster_unit_test` 路径模块接入。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 再导出 AST 相关类型（如大小写不敏感标识符 `CIStr`）。
pub mod ast {
    pub use meta_model::group_4::ast::*;
}
/// 再导出正式表/库元模型；不在本 crate 维护平行的元数据类型。
pub mod model {
    pub use meta_model::group_3::{MaskingPolicyInfo, PolicyInfo, ResourceGroupInfo};
    pub use meta_model::group_4::*;
}
/// 再导出放置规则包（placement bundle）。
pub mod placement {
    pub use placement_dependency::Bundle;
}
mod infoschema;
pub use infoschema::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
