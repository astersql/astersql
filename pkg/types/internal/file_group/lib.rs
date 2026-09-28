// Copyright 2026 AsterSQL.

// 字段相关辅助类型聚合门面：溢出处理、SET 类型与字符串工具。
//
// 通过 `#[path]` 挂接同目录上级的 overflow / set / string 实现，
// 供 types 子 crate 按文件组划分依赖时统一导出。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 数值溢出与截断相关逻辑。
#[path = "../../overflow.rs"]
pub mod overflow;
/// MySQL SET 类型解析与表示。
#[path = "../../set.rs"]
pub mod set;
/// 字符串类型辅助函数。
#[path = "../../string.rs"]
pub mod string;

#[cfg(test)]
#[path = "../../overflow_10_aster_unit_test.rs"]
mod overflow_10_aster_unit_test;

#[cfg(test)]
#[path = "../../string_test.rs"]
mod string_test;
