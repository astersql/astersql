// Copyright 2026 AsterSQL.

// `param` crate 入口：导出 MySQL 二进制协议参数类型与错误依赖。
//
// 将 `dbterror` / `errno` / `terror` 以子模块形式再导出，供 `binary_params`
// 构造标准服务端错误；测试通过 `migration_aster_unit_test` 挂载。
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_param;

/// 数据库错误类工厂（再导出）。
pub mod dbterror {
    pub use dbterror_crate::*;
}
/// 错误码常量（再导出）。
pub mod errno {
    pub use astersql_errno::errcode::*;
}
/// terror 错误类型（再导出）。
pub mod terror {
    pub use astersql_parser_terror::*;
}

/// 二进制协议参数结构定义。
mod binary_params;
/// 对外导出 BinaryParam 与相关错误静态量。
pub use binary_params::*;

/// 迁移相关单元测试（仅测试构建）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
