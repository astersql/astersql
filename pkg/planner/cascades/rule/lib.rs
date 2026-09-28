// Copyright 2026 AsterSQL.

// Cascades 变换规则（transformation rule）包入口。
//
// Cascades/Volcano 风格优化器通过规则改写逻辑计划：规则用 Pattern 匹配
// Memo 中的表达式，再经 XForm 生成等价逻辑算子。本包聚合规则绑定器
// （binder）、规则 trait、规则类型枚举，并再导出公开 API。

#![allow(non_snake_case, non_upper_case_globals)]

/// 将 Pattern 绑定到 Memo 组表达式，产出 BoundPlan。
mod binder;
/// 规则 trait、错误类型与 BaseRule 骨架。
mod rule;
/// 变换规则类型（Type）枚举与字符串名。
mod rule_type;

pub use binder::*;
pub use rule::*;
pub use rule_type::*;

#[cfg(test)]
mod binder_test;
