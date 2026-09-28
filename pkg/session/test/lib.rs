// Copyright 2026 AsterSQL.

// `session/test` 顶层测试包入口。
//
// 挂接 session 主 harness、通用 session 行为与 TiDB 兼容性相关测试模块。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod session_test;
#[cfg(test)]
mod tidb_test;
