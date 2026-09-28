// Copyright 2026 AsterSQL.

// server/err crate 根模块：拼装 errno、terror、dbterror 与 Server 错误绑定。
//
// 将迁移测试所需的错误基础设施以 crate 形式聚合；`server_err` 对应 Go 的
// `pkg/server/err` 标准错误常量，测试通过 `migration_aster_unit_test` 对照。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_server_err;

/// 通用错误包装类型（astersql_errors）。
pub use astersql_errors as errors;
/// terror 实现 crate 的再导出别名。
pub use astersql_parser_terror as terror_impl;
/// terror：错误分类与 RFC 风格错误码基础设施。
pub mod terror {
    pub use crate::terror_impl::*;
}
pub use astersql_errno::{errcode, errname};
/// errno：MySQL / TiDB 数字错误码与标准错误名表。
pub mod errno {
    pub use crate::errcode::*;
    pub use crate::errname::MySQLErrName;
}
/// parser::mysql：解析器侧 MySQL 协议相关类型。
pub mod parser {
    pub mod mysql {
        pub use astersql_parser_mysql::*;
    }
}
/// mysql：错误消息模板类型别名，便于测试断言 MessageTemplate。
pub mod mysql {
    pub use crate::parser::mysql::errname::{ErrMessage, Message};
}
/// dbterror：按错误 Class（如 ClassServer）构造标准 Error。
pub mod dbterror {
    pub use astersql_util_dbterror::*;
    /// 与 Go `*terror.Error` 指针语义对齐的装箱错误类型。
    pub type Error = Box<crate::terror::Error>;
}
/// Server 层标准错误常量（对齐 Go `pkg/server/err`）。
#[path = "error.rs"]
pub mod server_err;

// Go 在导入包时执行包级 var 初始化。Rust 没有同等的语言级 crate 初始化钩子，
// 因此把构造函数指针放入各平台启动段，在 `main`/libtest 运行前按 Go 顺序注册错误。
#[cfg(any(target_family = "unix", target_os = "windows"))]
#[used]
#[cfg_attr(
    all(target_family = "unix", not(target_vendor = "apple")),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
static SERVER_ERR_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        server_err::initialize_server_errors();
    }
    initialize
};

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
