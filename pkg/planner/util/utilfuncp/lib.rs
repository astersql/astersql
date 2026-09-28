// Copyright 2026 AsterSQL.

// 优化器函数指针注入与 Plan Cache 克隆辅助 crate 根。
//
// 再导出 `func_pointer_misc`：回调槽注册表及表达式/列/常量的缓存安全克隆 API。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

/// 回调槽、错误类型与 Plan Cache 克隆实现。
mod func_pointer_misc;

/// 对外再导出全部回调槽与克隆辅助函数。
pub use func_pointer_misc::*;

#[cfg(test)]
#[path = "func_pointer_misc_test.rs"]
/// CloneConstantsForPlanCache 含 nil 的回归测试。
mod func_pointer_misc_test;
