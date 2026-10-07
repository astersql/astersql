// Copyright 2026 AsterSQL.

// Cascades 旧版优化器 crate 入口。
//
// 对应 Go `pkg/planner/cascades/old`：包含变换规则、实现规则、强制规则、
// 优化主流程与计划字符串化；与新版 memo/cascades 并存，供对照与渐进迁移。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 物理属性强制规则（如 OrderEnforcer）。
mod enforcer_rules;
/// 逻辑算子 → 物理实现候选规则表。
mod implementation_rules;
/// Cascades 优化主循环（探索/实现阶段）。
mod optimize;
/// 计划/Group 调试字符串化。
mod stringer;
/// 逻辑等价变换规则。
mod transformation_rules;

pub use enforcer_rules::*;
pub use implementation_rules::*;
pub use optimize::*;
pub use stringer::*;
pub use transformation_rules::*;

#[cfg(test)]
mod enforcer_rules_test;
#[cfg(test)]
mod implementation_rules_test;
#[cfg(test)]
mod main_test;

// optimize_test.rs、stringer_test.rs 和 transformation_rules_test.rs 分别在
// optimize.rs / transformation_rules.rs 内部以 `#[path = "..."]` 声明为子模块（而不是在这里
// 声明为 crate 根的兄弟模块），因为它们需要调用 Optimizer 的私有阶段方法
// （onPhasePreprocessing/onPhaseExploration/fillGroupStats/implGroup）以及
// transformation_rules.rs 内部私有 `mod memo` 里的 ExprIterExt/PlanHandle 帮助类型。
// Rust 的可见性规则只允许"定义模块及其后代模块"访问私有 item，兄弟模块访问不到，
// 所以用 `#[path]` 把测试文件挂到正确的父模块下，同时仍然保持测试代码在独立文件中
// （符合 AGENTS.md 要求），文件本身不变。
