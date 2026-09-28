// Copyright 2026 AsterSQL.

// JSON 二进制编码子系统门面。
//
// 导出类型码常量与 `BinaryJSON` 实现，对齐 MySQL/TiDB 的
// binary JSON 布局（对象/数组条目偏移相对容器起始位置）。

/// JSON 类型码、字面量与布局常量。
#[path = "../../json_constants.rs"]
pub mod json_constants;
pub use json_constants::*;

/// BinaryJSON 编解码、序列化与取值访问。
#[path = "../../json_binary.rs"]
pub mod json_binary;
pub use json_binary::*;
