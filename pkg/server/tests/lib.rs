// Copyright 2026 AsterSQL.

// server/tests 根模块：挂载通用与串行 TiDB 兼容回归测试。
//
// 通过 `#[path]` 引入同目录下的 `main_test` 与 `tidb_serial_test`。

#![allow(dead_code)]

/// 全局配置默认值与临时覆盖用例。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 需串行执行的 TiDB 协议/鉴权相关用例。
#[cfg(test)]
#[path = "tidb_serial_test.rs"]
mod tidb_serial_test;
