// Copyright 2026 AsterSQL.

// `util/errmsg` crate 入口：SQL 错误消息正则扩展。
//
// 对应 Go `pkg/util/errmsg`。桥接 config / parser-mysql 依赖，导出 `Extend`；
// 测试配置下用互斥锁串行化全局配置，并挂载单元测试与迁移回归用例。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as util_errmsg;

/// 重新导出全局配置相关类型与函数。
pub use astersql_config::*;
/// 错误类型别名，对齐 Go 侧 errors 包引用习惯。
pub use astersql_errors as errors;

/// 配置子模块：错误消息扩展条目与全局配置读写。
pub mod config {
    pub use astersql_config::{
        Config, ErrorMessageExtension, get_error_message_extensions, get_global_config,
        store_global_config,
    };
}

/// parser 命名空间，便于按 Go 路径 `parser/mysql` 引用 `SQLError`。
pub mod parser {
    pub mod mysql {
        pub use astersql_parser_mysql::error::SQLError;
        pub use astersql_parser_mysql::*;
    }
}

/// 错误消息扩展核心实现。
mod errmsg;
/// 对外导出 `Extend`。
pub use errmsg::Extend;

/// 测试间串行化全局 errmsg 配置的互斥锁。
#[cfg(test)]
static ERRMSG_CONFIG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// 表驱动与并发单元测试。
#[cfg(test)]
#[path = "errmsg_test.rs"]
mod errmsg_test;
/// 迁移期回归用例。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
