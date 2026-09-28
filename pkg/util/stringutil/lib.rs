// Copyright 2026 AsterSQL.

// `stringutil` crate 入口：字符串反引号解析、LIKE 模式编译/匹配与相关工具。
//
// 对应 Go `pkg/util/stringutil`。对外暴露 `string_util` 子模块；
// 测试通过 `include!` / `#[path]` 挂到本 crate，与 Go 同目录测试一一对应。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 字符串工具实现：Unquote、LIKE、转义、UTF-8 位置换算等。
pub mod string_util;

/// `string_util` 单元测试（反引号、LIKE、标签格式化等）。
#[cfg(test)]
mod string_util_test {
    use super::string_util::*;
    include!("string_util_test.rs");
}

/// 迁移对齐回归：对照 Go 行为表验证关键 API。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
