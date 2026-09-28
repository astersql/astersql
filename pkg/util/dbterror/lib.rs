// Copyright 2026 AsterSQL.

// 数据库 terror 错误类包装与 DDL 错误清单 crate 入口。
//
// 对应 Go `pkg/util/dbterror`：对 `parser/terror` 的 ErrClass 做轻量包装，
// 并挂接 `ddl_terror` 中的 DDL 错误变量与 reorg 可重试边界。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

#[path = "terror.rs"]
mod terror_impl;
pub use terror_impl::*;

/// 自引用命名空间，便于依赖方以 `dbterror::` 路径使用本 crate 导出项。
pub mod dbterror {
    pub use crate::*;
}

/// MySQL 错误码常量与 `MySQLErrName` 消息表。
pub mod errno {
    pub use astersql_errno::errcode::*;
    pub use astersql_errno::errname::MySQLErrName;
}

/// Parser terror 的通用错误类型再导出。
pub mod errors {
    pub use astersql_parser_terror::errors::*;
}

/// 完整 parser terror API（含 ErrClass、ToSQLError 等）。
pub mod terror {
    pub use astersql_parser_terror::*;
}

mod ddl_terror;
pub use ddl_terror::*;

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
static DBTERROR_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        ddl_terror::initialize_ddl_errors();
    }
    initialize
};

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod migration_aster_unit_test;
#[cfg(test)]
mod terror_test;
