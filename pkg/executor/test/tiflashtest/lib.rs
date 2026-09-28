// Copyright 2026 AsterSQL.

// TiFlash / MPP 相关执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/tiflashtest` 包。覆盖 TiFlash Compute 分发策略
// （DispatchPolicy：一致哈希 / 轮询）的名称往返解析，以及 MPP Coordinator
// Manager 在缺少协调器时按请求版本回报错误的路径。
// MPP（Massively Parallel Processing）是 TiFlash 侧的并行查询执行框架。
// 本 crate 仅在 `#[cfg(test)]` 下编译测试源，不导出生产 API。

#![allow(dead_code)]

/// 包级测试入口与 DispatchPolicy 名称往返冒烟用例。
#[cfg(test)]
mod main_test;
/// MPP Coordinator Manager 缺协调器时报错路径的单元用例。
#[cfg(test)]
mod tiflash_test;
