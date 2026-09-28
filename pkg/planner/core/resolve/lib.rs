// Copyright 2026 AsterSQL.

// 名称解析（name resolve）模块入口。
//
// 将解析器 AST、元数据模型与解析上下文/结果字段类型重新导出，
// 供计划构建阶段把表名、列名解析为带元数据的包装节点。

pub use parser_ast as ast;
/// 元数据模型子集：库、表、列描述。
pub mod model {
    pub use meta_model::group_1::{ColumnInfo, DBInfo, TableInfo};
}

/// 名称解析核心逻辑（上下文、节点包装等）。
pub mod resolve;
/// 解析结果字段类型。
pub mod result;
pub use resolve::{Context, NodeW, TableNameW};
pub use result::ResultField;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
