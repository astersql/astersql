// Copyright 2026 AsterSQL.

// server 游标相关测试 crate 根模块。
//
// 在 test 配置下挂载 `cursor_test` 与 `main_test` 子模块。

#![allow(dead_code)]

/// 游标 fetch 包解码与行数上限用例。
#[cfg(test)]
mod cursor_test;
/// 游标测试运行时入口与非法包拒绝用例。
#[cfg(test)]
mod main_test;
