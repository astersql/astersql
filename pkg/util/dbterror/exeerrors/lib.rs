// Copyright 2026 AsterSQL.

// Executor 错误类（`exeerrors`）crate 入口。
//
// 对应 Go `pkg/util/dbterror/exeerrors`：聚合 terror/errno/parser 依赖，
// 并通过 `include!("errors.rs")` 暴露执行器侧预构造的错误实例。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as dbterror_exeerrors;

pub use astersql_errors as errors;
pub use astersql_parser_terror as terror_impl;
/// 再导出 parser terror 实现，供本 crate 内以 `terror::` 路径访问。
pub mod terror {
    pub use crate::terror_impl::*;
}
pub use astersql_errno::{errcode, errname};
/// MySQL 错误码与错误名表。
pub mod errno {
    pub use crate::errcode::*;
    pub use crate::errname::MySQLErrName;
}
/// Parser 侧 MySQL 错误消息辅助模块。
pub mod parser {
    pub mod mysql {
        pub use astersql_parser_mysql::*;
    }
}
/// 错误消息构造（`ErrMessage` / `Message`）便捷再导出。
pub mod mysql {
    pub use crate::parser::mysql::errname::{ErrMessage, Message};
}
/// 上层 dbterror 错误类（ClassDDL / ClassExecutor 等）。
pub mod dbterror {
    pub use astersql_util_dbterror::*;
}

/// 执行器错误实例表，内容来自同目录 `errors.rs`。
pub mod exeerrors {
    include!("errors.rs");
}

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
