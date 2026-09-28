// Copyright 2026 AsterSQL.

// 密码策略校验 crate 入口。
//
// 对应 Go `pkg/util/password-validation`：按全局系统变量（`validate_password_*`）
// 执行 LOW/MEDIUM/STRONG 策略，检查用户名、长度、字符类别与字典词。
// 依赖通过 `parser`/`sessionctx` 再导出，便于与迁移任务 crate 对齐。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 认证身份类型再导出（`UserIdentity` 等）。
pub mod parser {
    pub mod auth {
        pub use parser_auth::parser::auth::auth::*;
    }
}
/// 会话上下文：系统变量名定义与全局变量访问器。
pub mod sessionctx {
    /// `validate_password_*` 等变量名常量。
    pub mod vardef {
        pub use astersql_sessionctx_vardef::*;
    }
    /// 全局/会话变量读写与错误类型。
    pub mod variable {
        pub use astersql_sessionctx_variable::*;
    }
}

mod password_validation;
pub use password_validation::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "password_validation_test.rs"]
mod password_validation_test;
