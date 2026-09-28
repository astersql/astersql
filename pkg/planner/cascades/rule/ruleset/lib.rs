// Copyright 2026 AsterSQL.

// Cascades 规则集（rule set）子模块入口。
//
// 规则集按 Operand（算子种类）组织可用的变换规则列表，并支持按
// 规则掩码过滤、以及解相关 Apply 中间态的短路规则子集。本包再导出
// `rule_set` 中的公开类型与工厂函数。

#![allow(non_snake_case, non_upper_case_globals)]

/// 规则集、规则掩码与 Operand→规则映射实现。
mod rule_set;
pub use rule_set::*;

#[cfg(test)]
mod rule_set_aster_unit_test;
