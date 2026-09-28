// Copyright 2026 AsterSQL.

// `physicalplantest` casetest crate 入口。
//
// 物理计划（Physical Plan）相关用例：验证 SQL 归一化、hint 保留等与物理计划
// 输入形状相关的语义。物理计划是逻辑计划经代价优化后选出的可执行算子树。
// 仅在 `cfg(test)` 下挂载子模块。

#![allow(dead_code)]

/// 对应 Go TestMain / suite：稳定 SQL 归一化（NormalizeDigest）断言。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 物理计划 hint 对 NormalizeKeepHint 输入形状的影响测试。
#[cfg(test)]
#[path = "physical_plan_test.rs"]
mod physical_plan_test;
