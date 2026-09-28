// Copyright 2026 AsterSQL.

// Cascades 优化器（基于 Cascades/Volcano 框架的代价枚举式查询优化器）crate 入口。
//
// 再导出 `cascades` 模块中的公开 API，并在测试配置下挂载 `cascades_test`。

#![allow(non_snake_case)]

mod cascades;

pub use cascades::*;

#[cfg(test)]
mod cascades_test;
