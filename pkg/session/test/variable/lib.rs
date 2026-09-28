// Copyright 2026 AsterSQL.

// `variable` 会话系统变量测试包入口。
//
// 挂接系统变量（system variable：会话/全局作用域配置项）相关 harness 与功能测试模块。

#![allow(dead_code)]

/// 测试入口与全局环境准备（对应 Go `TestMain`）。
#[cfg(test)]
mod main_test;
/// 系统变量读写、校验与作用域行为用例。
#[cfg(test)]
mod variable_test;
