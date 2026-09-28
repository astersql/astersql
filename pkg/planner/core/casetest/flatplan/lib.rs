// Copyright 2026 AsterSQL.

// Flat Plan（扁平化物理计划）用例测试 crate 入口。
//
// Flat Plan 将树形物理执行计划按深度优先展开为 `FlatOperator` 列表，
// 便于 explain 树形缩进与 Label（Build/Probe、Seed/Recursive 等）展示。
// 本 crate 在 `cfg(test)` 下挂载 `flat_plan_test` 与 `main_test`。

#![allow(dead_code)]

/// FlattenPhysicalPlan / ExplainFlatPlanInRowFormat 等直连用例。
#[cfg(test)]
#[path = "flat_plan_test.rs"]
mod flat_plan_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
