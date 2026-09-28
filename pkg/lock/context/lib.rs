// Copyright 2026 AsterSQL.

// 会话表锁上下文（table lock context）子 crate 入口。
//
// 表锁（table lock）是 MySQL/TiDB 在会话级记录的表级锁映射，用于
// `LOCK TABLES` / 元数据操作权限检查；本 crate 只定义读写接口与锁类型再导出，
// 不负责真正加锁或访问存储。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 再导出 AST 侧表锁类型常量（Read/Write/ReadOnly 等）。
pub mod ast {
    pub use parser_ast::model::{
        TableLockNone, TableLockRead, TableLockReadLocal, TableLockReadOnly, TableLockType,
        TableLockWrite, TableLockWriteLocal,
    };
}
/// 再导出元模型中的 `TableLockTpInfo` 与模型层锁类型。
pub mod model {
    pub use meta_model::group_4::TableLockTpInfo;
    pub use meta_model::group_4::ast::model::TableLockType as ModelTableLockType;
}
/// 会话表锁读写 trait：`TableLockReadContext` / `TableLockContext`。
pub mod lockcontext;
pub use lockcontext::{TableLockContext, TableLockReadContext};

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
