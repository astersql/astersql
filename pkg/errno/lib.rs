// Copyright 2026 AsterSQL.

// `errno` 包入口：MySQL / TiDB 错误码与错误消息、以及 INFORMATION_SCHEMA 风格的错误统计。
//
// 对应 Go 的 `pkg/errno`：
// - `errcode`：数值错误码常量（errno）；
// - `errname`：错误码 → 消息模板 / 日志脱敏位置映射；
// - `infoschema`：按全局 / 用户 / 主机维度累计 error / warning 次数。
//
// 本文件声明子模块、重导出 MySQL 消息构造辅助，并挂载相关单元测试。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// MySQL 错误消息构造（`mysql::Message` / `ErrMessage`），供 errname 填充模板。
pub use astersql_parser_mysql::errname as mysql;

/// 数值错误码常量定义。
pub mod errcode;
/// 错误码到消息模板的映射表。
pub mod errname;
/// 错误 / 警告发生次数的多维统计（供 infoschema 展示）。
pub mod infoschema;

/// 错误码数值区间与 Go 对齐的回归测试。
#[cfg(test)]
mod errcode_1_aster_unit_test;
/// 错误名表覆盖度与统计快照 / 并发增量测试。
#[cfg(test)]
mod errname_2_aster_unit_test;
/// 校验每个错误码均有消息，且避开预留区间。
#[cfg(test)]
mod errname_test;
/// infoschema 统计深拷贝安全性测试。
#[cfg(test)]
mod infoschema_test;
#[cfg(test)]
mod main_test;
