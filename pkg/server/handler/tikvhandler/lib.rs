// Copyright 2026 AsterSQL.

// TiKV/Status HTTP handler 子包入口。
//
// 聚合 DXF（分布式框架调度）与通用 TiKV status 接口实现，
// 供 status HTTP 服务注册路由时引用。测试模块 `dxf_test` 仅在 test 配置下编译。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

/// DXF（Distributed eXecution Framework）调度与任务管理 HTTP 接口。
pub mod dxf;
/// 通用 TiKV status handler：schema、Region、MVCC、DDL 等运维接口。
pub mod tikv_handler;

pub use dxf::*;
pub use tikv_handler::*;

#[cfg(test)]
/// DXF 解析与 TTL 相关单元测试。
mod dxf_test;

#[cfg(test)]
/// TiKV handler 与 Go 可观察响应形状的回归测试。
mod tikv_handler_test;
