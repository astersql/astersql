// Copyright 2026 AsterSQL.

// MySQL 文本协议（Text Protocol）结果行格式化包入口。
//
// 文本协议把查询结果以可读字符串字节写回客户端（相对二进制协议的定长编码）。
// 本 crate 聚合字符集、MySQL 类型常量、chunk 行表示等依赖，并导出：
// - [`result_encoder`]：按会话 `character_set_results` 与列 collation 做元数据/数据编码；
// - [`textrow`]：把单列 Datum 格式化为文本协议载荷。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 字符集与编码转换相关再导出（charset ID、EncodingRef、替换策略等）。
pub mod charset {
    pub use parser_charset::charset::GetCharsetInfoByID;
    pub use parser_charset::encoding::{EncodingRef, OpEncodeReplace};
    pub use parser_charset::{CharsetBin, CharsetUTF8MB4, FindEncodingTakeUTF8AsNoop};
}
/// MySQL 协议常量、字符集名与列类型码再导出。
pub mod mysql {
    pub use parser_mysql::charset::*;
    pub use parser_mysql::r#const::*;
    pub use parser_mysql::r#type::*;
}
/// 类型系统（Datum 等）集成再导出。
pub mod types {
    pub use types_integration::*;
}
/// 后台日志工具再导出。
pub mod logutil {
    pub use ::logutil::log::BgLogger;
}
/// 列式执行 chunk 中的行视图与从 Datum 构造可变行的工具。
pub mod chunk {
    pub use ::chunk::Row;
    pub use ::chunk::mutrow::MutRowFromDatums;
}

mod result_encoder;
pub use result_encoder::*;
mod textrow;
pub use textrow::*;

#[cfg(test)]
#[path = "result_encoder_test.rs"]
mod result_encoder_test;

#[cfg(test)]
#[path = "textrow_test.rs"]
mod textrow_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
