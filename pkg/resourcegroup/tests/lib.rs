// Copyright 2026 AsterSQL.

// 资源组（Resource Group）集成测试入口。
//
// 资源组用于按 RU（Request Unit，请求单元）配额隔离工作负载；
// 本 crate 的测试通过 `resource_group_test` 覆盖 DDL、information_schema、
// runaway（失控查询治理）、binding hint 与 burst limit 等行为。

#![allow(dead_code)]

/// 挂载资源组相关测试用例（仅在 `cfg(test)` 下编译）。
#[cfg(test)]
#[path = "resource_group_test.rs"]
mod resource_group_test;
