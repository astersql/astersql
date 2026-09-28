// Copyright 2026 AsterSQL.

// `sessionctx/variable` 集成测试 crate 入口。
//
// 在 `#[cfg(test)]` 下挂载 `main_test`、`session_test`、`variable_test`
// 三个测试模块，对应 Go 侧同目录测试包的拆分。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;
#[cfg(test)]
#[path = "variable_test.rs"]
mod variable_test;
