// Copyright 2026 AsterSQL.

// 会话 `vars` 测试 crate 根模块。
//
// 聚合系统变量（system variable）相关集成测试：入口 harness 与具体用例分别在
// [`main_test`] / [`vars_test`] 中；本文件仅做模块声明，不包含业务逻辑。

#![allow(dead_code)]

/// 测试入口与全局环境准备（对应 Go `TestMain`）。
#[cfg(test)]
mod main_test;
/// 系统变量读写、升级规整、hint 与时区等用例。
#[cfg(test)]
mod vars_test;
