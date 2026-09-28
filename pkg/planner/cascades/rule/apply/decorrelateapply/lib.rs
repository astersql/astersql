// Copyright 2026 AsterSQL.

// Apply 解相关（decorrelate）变换规则子 crate 入口。
//
// 解相关：将相关子查询的 Apply（外层每行驱动内层）尽量改写为普通 Join，
// 以便 Cascades 后续做更通用的连接优化。本模块再导出基类与简单 Apply
// 解相关实现，并在测试配置下挂载对应单测。

#![allow(dead_code)]

/// 解相关规则共享基类与轻量逻辑计划桩类型。
pub mod xf_decorrelate_apply_base;
/// 简单无相关 Apply → Join 的变换规则实现。
pub mod xf_decorrelate_simple_apply;

pub use xf_decorrelate_apply_base::*;
pub use xf_decorrelate_simple_apply::*;

#[cfg(test)]
#[path = "xf_decorrelate_apply_test.rs"]
mod xf_decorrelate_apply_test;
