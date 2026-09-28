// Copyright 2026 AsterSQL.

// Lightning 导入定义（import definition）crate 入口。
//
// 导出表/库元数据模型（`tidb`）及对 `model::TableInfo` 的再导出，
// 供导入流程描述当前与期望的 TiDB 表结构。

/// 再导出依赖中的表元数据模型类型。
pub mod model {
    pub use model_dependency::group_1::TableInfo;
}

#[path = "tidb.rs"]
mod tidb;
pub use tidb::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
