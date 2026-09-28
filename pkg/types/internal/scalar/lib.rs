// Copyright 2026 AsterSQL.

// 标量类型转换与比较子系统门面。
//
// 聚合会话 Context、二进制字面量、比较与类型转换逻辑，
// 对应 Go `types` 包中面向单值（非向量化）的核心能力。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 警告追加器别名，用于类型转换时收集截断等告警。
pub use contextutil::WarnAppender as TypeWarnAppender;
pub use contextutil::errors;

/// MySQL 类型码与标志常量。
pub mod mysql {
    pub use parser_mysql::r#type::*;
}

#[cfg(feature = "types-integration")]
pub use types_decimal::mydecimal::MyDecimal;
#[cfg(feature = "types-integration")]
pub use types_file_group::set::Set;
#[cfg(feature = "types-integration")]
pub use types_json_binary::*;
#[cfg(feature = "types-integration")]
pub use types_metadata::Enum;
#[cfg(feature = "types-integration")]
pub use types_time::{
    Duration, MaxMySQLDuration, NewDuration, ParseDatetimeFromNum, ParseDuration, ParseTime, Time,
    TimeMaxHour, TimeMaxValue, ZeroDuration,
};

/// 语句级类型 Context（标志位与警告）。
#[path = "../../context.rs"]
mod context;
pub use context::*;

/// BIT/HEX 等二进制字面量解析。
#[path = "../../binary_literal.rs"]
mod binary_literal;
pub use binary_literal::*;

/// 整数/字符串比较语义。
#[path = "../../compare.rs"]
mod compare;
pub use compare::*;

/// 数值与字符串互转、截断规则。
#[path = "../../convert.rs"]
mod convert;
pub use convert::*;

#[cfg(test)]
#[path = "../../binary_literal_1_aster_unit_test.rs"]
mod binary_literal_1_aster_unit_test;

#[cfg(all(test, feature = "types-integration"))]
mod migration_aster_unit_test;
