// Copyright 2026 AsterSQL.

#![allow(dead_code)]

// 常量传播集成测试 crate 入口。
//
// 挂载 main_test 与 constant_propagation_test，验证常量传播求解器与 Go 回归场景对齐。

/// 常量传播回归与源码契约测试。
#[cfg(test)]
#[path = "constant_propagation_test.rs"]
mod constant_propagation_test;
/// 测试环境引导。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
