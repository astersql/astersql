// Copyright 2026 AsterSQL.

// Schema Tracker（模式跟踪器）crate 入口。
//
// 本 crate 在内存中跟踪 DDL（数据定义语言）变更对库、表、列、索引、
// 分区等元数据的影响，用于校验 DDL 作业的正确性或做 dry-run 推演。
// 主要子模块：
// - [`info_store`]：内存信息模式仓库，按库/表名索引元数据；
// - [`dm_tracker`]：数据模型（data model）变更跟踪器；
// - [`checker`]：对跟踪结果做一致性校验。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 再导出 meta 层的数据模型类型（库/表/列等元信息）。
pub mod model {
    pub use meta_model::*;
}
/// 再导出解析器 AST（抽象语法树）类型，供跟踪器解析 DDL 语句。
pub mod ast {
    pub use parser_ast::*;
}

/// Schema Tracker 操作过程中可能产生的错误。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    /// 目标库（database/schema）不存在。
    #[error("database does not exist: {0}")]
    DatabaseNotExists(String),
    /// 目标库已存在。
    #[error("database already exists: {0}")]
    DatabaseExists(String),
    /// 目标表不存在（参数为库名与表名）。
    #[error("table does not exist: {0}.{1}")]
    TableNotExists(String, String),
    /// 目标表已存在（参数为库名与表名）。
    #[error("table already exists: {0}.{1}")]
    TableExists(String, String),
    /// DROP TABLE/VIEW 中一个或多个目标不存在或对象类型不匹配。
    #[error("unknown table(s): {0}")]
    TableDropExists(String),
    /// 对象类型与 DDL 要求不符。
    #[error("{0}.{1} is not a {2}")]
    WrongObject(String, String, &'static str),
    /// 目标列已存在。
    #[error("column already exists: {0}")]
    ColumnExists(String),
    /// 目标列不存在。
    #[error("column does not exist: {0}")]
    ColumnNotExists(String),
    /// 目标索引已存在。
    #[error("index already exists: {0}")]
    IndexExists(String),
    /// 目标索引不存在。
    #[error("index does not exist: {0}")]
    IndexNotExists(String),
    /// 目标分区（partition）不存在。
    #[error("partition does not exist: {0}")]
    PartitionNotExists(String),
    /// 禁止删除表的最后一列。
    #[error("cannot remove all columns from table")]
    CannotRemoveAllColumns,
    /// 非分区表不允许执行分区管理。
    #[error("partition management on a nonpartitioned table")]
    PartitionManagementOnNonpartitionedTable,
    /// 当前不支持的 DDL 操作。
    #[error("unsupported DDL: {0}")]
    Unsupported(&'static str),
    /// 跟踪结果与预期元数据不一致。
    #[error("schema tracker mismatch: {0}")]
    Mismatch(String),
}

mod info_store;
pub use info_store::*;
mod dm_tracker;
pub use dm_tracker::*;
mod checker;
pub use checker::*;

#[cfg(test)]
#[path = "checker_test.rs"]
mod checker_test;
#[cfg(test)]
#[path = "dm_tracker_test.rs"]
mod dm_tracker_test;
#[cfg(test)]
#[path = "info_store_test.rs"]
mod info_store_test;
