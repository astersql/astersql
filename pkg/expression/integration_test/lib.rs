// Copyright 2026 AsterSQL.

// expression 集成测试 crate 入口。
//
// 挂载 part1–part3 Aster 单元测试与 `main_test` 配置桩。
// 对应 Go `pkg/expression/integration_test` 包，验证向量/JSON/时间等跨模块语义。

#![allow(dead_code)]

/// VECTOR 类型与距离函数相关用例。
#[cfg(test)]
mod integration_part1_aster_unit_test;
/// JSON / 时间 / 系统变量相关用例。
#[cfg(test)]
mod integration_part2_aster_unit_test;
/// 时间内置、行校验和、计划缓存相关用例。
#[cfg(test)]
mod integration_part3_aster_unit_test;
/// 测试主入口配置（忽略后台 worker 列表）。
#[cfg(test)]
mod main_test;
