// Copyright 2026 AsterSQL.

// `astersql_plugin` crate 入口：TiDB 插件框架的 Rust 迁移基线。
//
// 聚合审计事件、常量、错误、辅助函数、核心加载逻辑与 SPI（服务提供接口）
// 子模块，并统一重导出；测试文件通过 `#[path]` 挂载以对齐 Go 包布局。

#![allow(dead_code)]

/// 审计相关事件与清单类型。
pub mod audit;
/// 插件种类与生命周期状态常量。
pub mod r#const;
/// 插件错误类型。
pub mod errors;
/// 清单声明、ID 解析与测试加载辅助。
pub mod helper;
/// 插件加载、初始化与遍历核心逻辑。
pub mod plugin;
/// 插件 SPI（对外回调接口）定义。
pub mod spi;

pub use audit::*;
pub use r#const::*;
pub use errors::*;
pub use helper::*;
pub use plugin::*;
pub use spi::*;

#[cfg(test)]
#[path = "audit_test.rs"]
mod audit_test;
#[cfg(test)]
#[path = "const_test.rs"]
mod const_test;
#[cfg(test)]
#[path = "helper_test.rs"]
mod helper_test;
#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;
#[cfg(test)]
#[path = "plugin_test.rs"]
mod plugin_test;
#[cfg(test)]
#[path = "spi_test.rs"]
mod spi_test;
