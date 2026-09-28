// Copyright 2026 AsterSQL.

// parser/terror crate 入口。
//
// 将共享 `errors` 基座与 MySQL 错误码表接线到本 crate 的 `terror` 模块。
// terror（分类错误）按 ErrClass/ErrCode 组织错误，并可转换为 MySQL 协议层的 SQLError。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 复用共享 errors 基座，提供 Error / SharedError / Normalize 等能力。
pub use astersql_errors as errors;

/// 嵌套命名空间，对齐 Go `parser.mysql` / `parser.terror` 的引用路径。
pub mod parser {
    /// 引入 MySQL 错误码与错误消息表，供 terror 构造标准错误。
    pub use astersql_parser_mysql as mysql;

    /// 将本 crate 的 terror API 暴露在 `parser::terror` 路径下。
    pub mod terror {
        pub use crate::terror::*;
    }
}

/// 实现文件：错误类别、错误码注册与 SQLError 转换。
mod terror;
/// 对外再导出 terror 中的全部公开类型与函数。
pub use terror::*;

#[cfg(test)]
#[path = "terror_test.rs"]
/// 对齐 Go terror_test.go 的行为测试。
mod parser_terror_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期补充的单元测试，覆盖注册/合成/相等性等边界。
mod migration_aster_unit_test;
