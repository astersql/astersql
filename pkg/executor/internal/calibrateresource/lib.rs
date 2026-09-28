// Copyright 2026 AsterSQL.

// CALIBRATE RESOURCE 相关逻辑的 crate 入口。
//
// 资源校准（Resource Calibration）根据集群 CPU/工作负载估算 RU（Request Unit，
// 统一资源消耗单位）容量，供资源组（Resource Group）配额配置参考。
// 实现见 `calibrate_resource`；测试模块仅在 `cfg(test)` 下编译。

#![allow(dead_code)]

/// 资源校准核心：静态/动态 RU 估算与执行器。
pub mod calibrate_resource;

#[cfg(test)]
/// 对照 Go 的校准行为回归。
mod calibrate_resource_test;
#[cfg(test)]
/// 包级测试入口契约（对应 Go `TestMain` 的初始化顺序与参数）。
mod main_test;
