// Copyright 2026 AsterSQL.

// TiFlash 相关 DDL 测试包入口。
//
// 聚合 TiFlash 副本（replica）可用性、分区表副本、placement rule、
// 进度上报与批量限流等用例，对应 Go `pkg/ddl/tests/tiflash`。
// TiFlash 为列存加速引擎；DDL 通过 PD placement rule 调度副本，
// 并在 InfoSchema 中维护 Available / AvailablePartitionIDs 等状态。

#![allow(dead_code)]

/// TiFlash DDL 用例主体（步骤记录与部分可执行断言）。
#[cfg(test)]
mod ddl_tiflash_test;
/// TestMain：进程级初始化顺序、配置与 goleak 收尾契约。
#[cfg(test)]
mod main_test;
