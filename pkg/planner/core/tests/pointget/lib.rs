// Copyright 2026 AsterSQL.

// PointGet / BatchPointGet 计划相关集成测试的 crate 根模块。
//
// 通过 `#[path]` 挂载 `main_test`（对应 Go TestMain）与 `point_get_plan_test`
// （点查计划缓存、hint、复用安全判定等）。PointGet 是按主键或唯一键等值条件
// 直接定位单行的物理算子，常用于 autocommit 点查与 prepared plan cache 复用。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// PointGet 计划形状、缓存命中、hint 与 issue 回归用例。
#[cfg(test)]
#[path = "point_get_plan_test.rs"]
mod point_get_plan_test;
