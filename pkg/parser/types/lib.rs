// Copyright 2026 AsterSQL.

// parser/types 包入口：字段类型、求值类型与类型辅助工具的 crate 根。
//
// 对照 Go `pkg/parser/types`：通过 `include!` 挂载 `etc` / `eval_type` / `field_type`，
// 并再导出 mysql、字符集、格式化与 util 依赖，供类型串化与 AST 恢复使用。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

extern crate self as parser_types;

pub use astersql_errors as errors;

/// MySQL 类型码、标志与错误码等常量的再导出。
pub mod mysql {
    pub use ::mysql::*;
    pub use ::mysql::{charset::*, r#const::*, errcode::*, r#type::*, util::*};
}

/// terror 错误框架再导出。
pub mod terror {
    pub use ::terror::*;
}

/// 字符集与排序规则再导出。
pub mod charset {
    pub use parser_charset::charset::CollationBin;
    pub use parser_charset::charset::*;
    pub use parser_charset::*;
}

/// SQL 文本恢复（Restore）所用格式化工具再导出。
pub mod format {
    pub use ::format::*;
}

/// 解析期 util（如 IHasher）再导出。
pub mod util {
    pub use ::util::*;
}

/// 类型子系统：辅助函数、求值类型与 FieldType。
pub mod types {
    /// 类型名映射、BLOB/CHAR 判定与错误声明等辅助逻辑。
    pub mod etc {
        use crate::{mysql, terror};
        include!("etc.rs");
    }
    pub use etc::*;

    /// 表达式求值类型 EvalType。
    pub mod eval_type {
        include!("eval_type.rs");
    }
    pub use eval_type::*;

    /// 列字段类型 FieldType 及其串化/比较方法。
    pub mod field_type {
        use super::*;
        use crate::{charset, format, mysql, util};
        include!("field_type.rs");
    }
    pub use field_type::*;
}

pub use types::*;

// Go 在导入包时执行包级 var 初始化。沿用 dbterror 的跨平台启动段约定，
// 在 `main`/libtest 入口前按声明顺序注册 types 的四个标准错误。
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
static PARSER_TYPES_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        types::etc::initialize_standard_errors();
    }
    initialize
};

/// etc 辅助函数单元测试。
#[cfg(test)]
#[path = "etc_test.rs"]
mod etc_test;
/// FieldType 行为单元测试。
#[cfg(test)]
#[path = "field_type_test.rs"]
mod field_type_test;
/// 迁移对照用 Aster 单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
