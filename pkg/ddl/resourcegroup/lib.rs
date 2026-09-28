// Copyright 2026 AsterSQL.

// 资源组（Resource Group）辅助库入口。
//
// 聚合 Resource Manager protobuf 绑定（`rmpb`）、元模型（`model`/`ast`）、
// 错误类型与组设置转换逻辑，供 DDL 创建/修改资源组时复用。

#![allow(
    dead_code,
    non_snake_case,
    non_upper_case_globals,
    renamed_and_removed_lints,
    static_mut_refs
)]

/// 由 build.rs 从 `resource_manager.proto` 生成的 protobuf 模块。
#[allow(warnings)]
pub mod rmpb {
    include!(concat!(env!("OUT_DIR"), "/resource_manager.rs"));
}
/// 复用 meta_model 中的资源组模型定义。
pub use meta_model::group_3 as model;
/// 复用 meta_model 中的 AST 枚举（Runaway 动作/Watch 类型等）。
pub use meta_model::group_3::ast;

pub mod errors;
pub use errors::*;
pub mod group;
pub use group::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
