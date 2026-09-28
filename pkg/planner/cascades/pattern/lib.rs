// Copyright 2026 AsterSQL.

// Cascades pattern 匹配子 crate：引擎类型位集与算子模式树。
//
// `engine` 描述规则可落在的执行引擎；`pattern` 描述待匹配的逻辑算子树形状，
// 供旧版 Cascades 变换规则在 memo 上绑定表达式。

#![allow(non_snake_case, non_upper_case_globals)]

/// 引擎类型与引擎集合定义。
mod engine;
/// 算子 Operand 与 Pattern 树。
mod pattern;

pub use engine::*;
pub use pattern::*;

#[cfg(test)]
mod engine_test;
#[cfg(test)]
mod pattern_test;
