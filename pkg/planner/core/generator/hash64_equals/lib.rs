// Copyright 2026 AsterSQL.

// Hash64/Equals 代码生成器包入口。
//
// 导出 `hash64_equals_generator`，并在测试配置下挂载 `hash64_equals_test`。
// Hash64/Equals 用于逻辑算子在 Cascades 优化器中的结构哈希与相等比较。

#![allow(dead_code, non_snake_case)]

/// 生成逻辑算子 Hash64/Equals 的 Go 源码文本。
pub mod hash64_equals_generator;

/// 再导出生成器公共 API。
pub use hash64_equals_generator::*;

#[cfg(test)]
// 测试模块通过 path 属性挂到同目录测试文件。
#[path = "hash64_equals_test.rs"]
mod hash64_equals_test;

#[cfg(test)]
#[path = "hash64_equals_generator_test.rs"]
mod hash64_equals_generator_test;
