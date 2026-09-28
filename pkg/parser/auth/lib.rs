// Copyright 2026 AsterSQL.

// `parser/auth` crate 根：MySQL/TiDB 认证插件相关解析与散列工具。
//
// 导出用户身份、caching_sha2 / mysql_native_password / tidb_sm3 等子模块，
// 并通过 `parser::{format,mysql,auth}` 命名空间兼容机械迁移后的导入路径。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as parser_auth;

/// 再导出错误类型 crate。
pub use astersql_errors as errors;
/// 再导出 terror 实现 crate。
pub use astersql_parser_terror as terror_impl;

/// 用户/角色身份数据结构与 SQL 还原。
pub mod auth;
/// caching_sha2_password / tidb_sm3_password 散列生成与校验。
pub mod caching_sha2;
/// mysql_native_password（SHA1 双重散列与 scramble）相关工具。
pub mod mysql_native_password;
/// SM3（国密哈希）摘要实现，供 tidb_sm3_password 使用。
pub mod tidb_sm3;

/// 兼容迁移导入路径的 `parser` 命名空间。
pub mod parser {
    /// 格式化/Restore 上下文，来自 format crate。
    pub mod format {
        pub use astersql_parser_format::*;
    }

    /// MySQL 常量与类型定义，来自 mysql crate。
    pub mod mysql {
        pub use astersql_parser_mysql::*;
    }

    /// 本 crate 认证子模块的聚合再导出。
    pub mod auth {
        pub use crate::{auth, caching_sha2, mysql_native_password, tidb_sm3};
    }
}

/// caching_sha2 单元测试。
#[cfg(test)]
#[path = "caching_sha2_test.rs"]
mod caching_sha2_test;
/// 跨子模块迁移对齐的综合单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
/// mysql_native_password 单元测试。
#[cfg(test)]
#[path = "mysql_native_password_test.rs"]
mod mysql_native_password_test;
/// tidb_sm3 单元测试。
#[cfg(test)]
#[path = "tidb_sm3_test.rs"]
mod tidb_sm3_test;
