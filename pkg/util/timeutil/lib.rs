// Copyright 2026 AsterSQL.

// 时间与时区工具包入口。
//
// 对应 Go `pkg/util/timeutil`。导出可取消睡眠、时区解析/推断与错误类型，
// 供会话时区、调度窗口等路径使用。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

/// 本 crate 自引用别名，供测试按包名风格引用。
extern crate self as astersql_util_timeutil;

/// 时区与时间相关错误类型。
pub mod errors;
/// 可取消睡眠等时间辅助。
pub mod time;
/// 时区加载、解析与系统时区推断。
pub mod time_zone;

/// 错误类型与 Go/MySQL 标准错误契约测试。
#[cfg(test)]
#[path = "errors_test.rs"]
mod errors_test;

/// AsterSQL 迁移补充单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// `Sleep` 行为单元测试。
#[cfg(test)]
#[path = "time_test.rs"]
mod time_test;

/// 时区相关单元测试。
#[cfg(test)]
#[path = "time_zone_test.rs"]
mod time_zone_test;
