// Copyright 2026 AsterSQL.

// planner/core/tests/cte 测试 crate 入口。
//
// 在 `#[cfg(test)]` 下挂载 `main_test` 与 `cte_test`，覆盖 CTE 规划相关用例。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// CTE 规划行为与 plan_tree 形状测试。
#[cfg(test)]
#[path = "cte_test.rs"]
mod cte_test;
