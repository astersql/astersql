// Copyright 2026 AsterSQL.

// Planner 错误类（`plannererrors`）crate 入口。
//
// 对应 Go `pkg/util/dbterror/plannererrors`：聚合 terror/errno 依赖，
// 并导出优化器（planner）侧预构造的错误实例模块 `planner_terror`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as dbterror_plannererrors;

pub use astersql_errors as errors;
pub use astersql_parser_terror as terror_impl;
/// 再导出 parser terror 实现。
pub mod terror {
    pub use crate::terror_impl::*;
}
pub use astersql_errno::{errcode, errname};
/// MySQL 错误码与错误名表。
pub mod errno {
    pub use crate::errcode::*;
    pub use crate::errname::MySQLErrName;
}
/// Parser 侧 MySQL 辅助类型。
pub mod parser {
    pub mod mysql {
        pub use astersql_parser_mysql::*;
    }
}
/// 合并 errno 与错误消息构造的便捷命名空间。
pub mod mysql {
    pub use crate::errno::*;
    pub use crate::parser::mysql::errname::{ErrMessage, Message};
}
pub use astersql_util_dbterror as dbterror;

pub mod planner_terror;
pub use planner_terror::*;

// Go 在导入包时初始化全部包级错误变量。启动段构造器在 main/libtest 前完成同等注册，
// 避免服务调用 RegisterFinish 后首次解引用 LazyLock 才触发禁止注册的 panic。
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
static PLANNERERRORS_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        planner_terror::initialize_planner_errors();
    }
    initialize
};

#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
