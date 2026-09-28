// Copyright 2026 AsterSQL.

// 计划缓存（Plan Cache）克隆代码生成器包入口。
//
// 导出 `plan_clone_generator`：为物理算子生成 `CloneForPlanCache`，使缓存命中时
// 能安全克隆计划树并替换会话上下文，避免跨会话共享可变状态。

#![allow(dead_code)]
#![allow(non_snake_case)]

/// 物理计划 CloneForPlanCache 的 Go 源码生成器。
pub mod plan_clone_generator;

/// 再导出生成器公共 API。
pub use plan_clone_generator::*;

#[cfg(test)]
// 测试模块通过 path 属性挂到同目录测试文件。
#[path = "plan_clone_test.rs"]
mod plan_clone_test;
