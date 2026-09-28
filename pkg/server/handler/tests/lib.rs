// Copyright 2026 AsterSQL.

// server/handler 集成测试 crate 入口。
//
// 聚合 DXF、HTTP handler（含串行用例）与 TestMain 等价初始化模块。

#![allow(dead_code)]

#[cfg(test)]
/// DXF（分布式执行框架）相关 HTTP API 测试。
mod dxf_test;
#[cfg(test)]
/// 需串行执行的 HTTP handler 测试，避免全局状态互相干扰。
mod http_handler_serial_test;
#[cfg(test)]
/// HTTP status handler 主测试集（Region/MVCC/schema 等）。
mod http_handler_test;
#[cfg(test)]
/// 对应 Go TestMain 的进程级测试环境初始化。
mod main_test;
