// Copyright 2026 AsterSQL.

// server/internal/column crate 入口。
//
// 聚合列定义编码、结果字段转换及其依赖的 charset / chunk / mysql / textrow
// 等子系统 re-export，并在测试配置下挂接对照单测。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 字符集常量与查询接口。
pub mod charset {
    pub use parser_charset::CharsetUTF8MB4;
    pub use parser_charset::charset::GetCharsetInfo;
}
/// 列式执行 chunk / Row 类型。
pub mod chunk {
    pub use chunk_crate::*;
}
/// 服务端错误类型与无效类型错误码。
pub mod err {
    pub use server_err::errors::{ErrorArg, SharedError};
    pub use server_err::server_err::ErrInvalidType;
    pub type Error = SharedError;
}
/// 元数据模型（含 DefaultValue）。
pub mod model {
    pub use meta_model::group_1::*;
}
/// MySQL 类型、字符集与工具函数。
pub mod mysql {
    pub use parser_mysql::charset::*;
    pub use parser_mysql::r#const::*;
    pub use parser_mysql::r#type::*;
    pub use parser_mysql::util::*;
}
/// 规划器 resolve 结果字段。
pub mod resolve {
    pub use planner_resolve::*;
}
/// 文本行编码辅助。
pub mod textrow {
    pub use textrow_crate::*;
}
/// 长度编码字节解析。
pub mod util {
    pub use server_util::ParseLengthEncodedBytes;
}
/// 类型系统与 IsString 等元数据判断。
pub mod types {
    pub use types_integration::*;
    pub use types_metadata::IsString;
}
/// 列定义与结果行 dump 实现。
mod column;
pub use column::*;
/// ResultField → Info 转换。
mod convert;
pub use convert::*;

#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
