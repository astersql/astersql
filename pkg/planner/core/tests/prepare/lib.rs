// Copyright 2026 AsterSQL.

// Prepared Statement / prepared plan cache 相关集成测试的 crate 根模块。
//
// 挂载 `main_test`（对应 Go TestMain 参考）与 `prepare_test`（参数类型兼容性
// 决定计划是否可复用）。Prepare 将 SQL 预编译为可反复执行的语句；plan cache
// 在参数类型兼容时复用旧执行计划，否则强制重新生成以防 range/point 计划错配。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
/// 参数类型兼容性与 prepared plan cache 复用语义用例。
#[cfg(test)]
mod prepare_test;
