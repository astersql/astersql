// Copyright 2026 AsterSQL.

// sqlkiller crate 根：查询/会话取消与超时杀进程相关能力。
//
// 对应 Go `pkg/util/sqlkiller`。挂载执行错误类型、intest 开关与核心 `sqlkiller` 模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_util_sqlkiller;

/// 执行层错误与错误码再导出。
pub use exeerrors_crate::{errors, exeerrors};
/// 测试环境探测开关（对应 Go `intest`）。
pub mod intest {
    /// 当前是否处于测试构建；`cfg!(test)` 为真时为 `true`。
    pub const InTest: bool = cfg!(test);
}
/// 日志工具再导出。
pub mod logutil {
    pub use logutil_crate::log;
}
/// SQL Killer 核心实现模块。
pub mod sqlkiller;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
