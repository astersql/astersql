// Copyright 2026 AsterSQL.

// 表达式重写器（rewriter）相关集成测试的 crate 根模块。
//
// 通过 `#[path]` 挂载 `rewriter_test`（系统变量作用域重写）与
// `main_test`（对应 Go TestMain 的参考入口）。重写器负责将 AST 中的
// 系统变量引用等表达式改写为可执行形式，并校验 session/global/instance 作用域。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// 系统变量显式作用域重写与错误码对齐用例。
#[cfg(test)]
#[path = "rewriter_test.rs"]
mod rewriter_test;
