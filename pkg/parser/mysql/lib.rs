// Copyright 2026 AsterSQL.

// `astersql_parser_mysql` crate 入口。
//
// 聚合 MySQL 协议侧常量与元数据：字符集、SQL mode、错误码/消息、SQLSTATE、
// 权限位、字段类型与 locale 数字格式等。各子模块只提供进程内查表与构造，
// 不发起网络 IO，也不访问系统表。

#![allow(non_snake_case, non_upper_case_globals)]

/// 转发共享错误/脱敏基础设施，供本 crate 的 error 格式化路径使用。
pub use astersql_errors as errors;

/// 字符集与排序规则 ID、名称映射。
pub mod charset;
/// MySQL/TiDB 版本串、SQL mode 等位标志常量。
pub mod r#const;
/// MySQL/MariaDB/TiDB 数字错误码常量。
pub mod errcode;
/// 错误码到默认消息模板与脱敏参数位置的映射。
pub mod errname;
/// SQLError 构造与 printf 风格消息格式化。
pub mod error;
/// FORMAT() 函数使用的 locale 数字分组与小数点规则。
pub mod locale_format;
/// GRANT/REVOKE 相关权限位与列名/集合枚举映射。
pub mod privs;
/// 错误码到 SQLSTATE（五字符状态码）映射。
pub mod state;
/// MySQL 协议字段类型编号与列标志位。
pub mod r#type;
/// 类型默认长度/精度、鉴权插件分类等工具函数。
pub mod util;

#[cfg(test)]
mod charset_1_aster_unit_test;
#[cfg(test)]
mod const_test;
#[cfg(test)]
mod errcode_2_aster_unit_test;
#[cfg(test)]
mod error_3_aster_unit_test;
#[cfg(test)]
mod error_test;
#[cfg(test)]
mod locale_format_test;
#[cfg(test)]
mod privs_test;
#[cfg(test)]
mod type_test;
#[cfg(test)]
mod unit_test;
