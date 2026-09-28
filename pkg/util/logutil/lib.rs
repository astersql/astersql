// Copyright 2026 AsterSQL.

// `util/logutil` crate 入口：TiDB 风格日志工具集。
//
// 对应 Go `pkg/util/logutil`。提供全局/慢查询/General Log 初始化、
// protobuf 风格十六进制美化打印，以及采样与追踪字段辅助。

#![allow(non_snake_case)]

/// General Log 专用 logger 工厂。
pub mod general_logger;
/// protobuf 消息的十六进制/美化打印。
pub mod hex;
/// 核心日志配置、Logger、全局替换与追踪上下文。
pub mod log;
/// 慢查询日志专用 logger 工厂。
pub mod slow_query_logger;

// Preserve every adjacent test body while supplying Go's package lifecycle.
#[cfg(test)]
include!(concat!(env!("OUT_DIR"), "/registered_tests.rs"));

#[cfg(test)]
mod test_main;

#[cfg(test)]
fn main() {
    test_main::run(registered_tests());
}
