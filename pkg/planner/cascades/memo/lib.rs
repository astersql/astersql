// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Cascades Memo 包入口。
//
// Memo 是 Cascades 优化器的核心数据结构：把逻辑等价的算子表达式
// 归入同一 Group，通过 GroupExpression 记录算子与其子 Group 输入，
// 并在探索/实现阶段复用已推导的逻辑属性，避免重复枚举等价计划。

#![allow(non_snake_case)]

/// 等价组（Group）及其逻辑属性。
mod group;
/// 组表达式（GroupExpression）：算子 + 子 Group 输入。
mod group_expr;
/// 组 ID 单调生成器。
mod group_id_generator;
/// Memo 图、插入/合并与计划枚举迭代器。
mod memo;

pub use group::*;
pub use group_expr::*;
pub use group_id_generator::*;
pub use memo::*;

#[cfg(test)]
mod group_and_expr_test;
#[cfg(test)]
mod group_expr_test;
#[cfg(test)]
mod group_id_generator_test;
#[cfg(test)]
mod logical_plan_route_aster_unit_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod memo_test;
