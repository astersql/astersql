// Copyright 2026 AsterSQL.

// 优化器 Hint（提示）工具包入口。
//
// Hint 是 SQL 中 `/*+ ... */` 形式的优化器提示，用于指定连接算法、索引选择、
// 存储引擎等。本 crate 对应 Go `pkg/util/hint`：解析/还原 hint、按查询块（query
// block）绑定，并收集未匹配警告。再导出 AST、错误类型与 `hint_processor` /
// `hint_query_block` 及 `hint.rs` 中的核心数据结构。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

pub use parser::ast;
pub use parser::{New as NewParser, Parser as SQLParser};

/// 本 crate 统一错误：包装 parser 与 TiDB/dbterror 错误。
pub mod errors {
    pub use dbterror::errors::ErrorArg;
    /// Hint 相关可返回的错误变体。
    #[derive(Clone, Debug)]
    pub enum Error {
        /// SQL 解析器错误。
        Parser(parser::errors::SharedError),
        /// TiDB/dbterror 风格错误。
        TiDB(dbterror::errors::SharedError),
    }
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Parser(e) => e.fmt(f),
                Self::TiDB(e) => e.fmt(f),
            }
        }
    }
    impl std::error::Error for Error {}
    impl From<dbterror::errors::SharedError> for Error {
        fn from(e: dbterror::errors::SharedError) -> Self {
            Self::TiDB(e)
        }
    }
    /// 构造带消息的 TiDB 风格错误。
    pub fn New(message: impl Into<String>) -> Error {
        Error::TiDB(dbterror::errors::New(message))
    }
    /// 构造不带调用栈的 TiDB 风格错误。
    pub fn NewNoStackError(message: impl Into<String>) -> Error {
        Error::TiDB(dbterror::errors::NewNoStackError(message))
    }
}
/// 再导出 dbterror，供 hint 警告与错误生成使用。
pub mod dbterror {
    pub use ::dbterror::dbterror::*;
}
/// MySQL/errno 错误码再导出（冲突 hint 警告等）。
pub mod mysql {
    pub use errno::errcode::ErrWarnConflictingHint;
}
/// 元数据模型再导出（索引列/索引信息等）。
pub mod model {
    pub use meta_model::ast::IndexType;
    pub use meta_model::{IndexColumn, IndexInfo, StatePublic};
}
/// 类型常量再导出。
pub mod types {
    pub use ::types::scalar::UnspecifiedLength;
}

/// 查询块（query block）hint 处理：`QB_NAME`、视图 hint、offset 映射。
pub mod hint_query_block;
pub use hint_query_block::*;
/// Hint 收集、绑定、还原与 binding 完整性检查。
pub mod hint_processor;
pub use hint_processor::*;
// 内联 include：PlanHints、HintedTable 等核心类型与 ParsePlanHints 等逻辑。
include!("hint.rs");

#[cfg(test)]
#[path = "hint_2_aster_unit_test.rs"]
mod hint_2_aster_unit_test;
#[cfg(test)]
#[path = "hint_processor_1_aster_unit_test.rs"]
mod hint_processor_1_aster_unit_test;
