// Copyright 2026 AsterSQL.

// `pkg/executor/test/planreplayer` crate 根：挂接 Plan Replayer 测试模块。
//
// Plan Replayer 用于导出/导入执行计划现场（schema、统计信息、绑定、EXPLAIN 等），
// 便于在另一环境复现优化器决策；本 crate 仅在 `#[cfg(test)]` 下编译测试源。

#![allow(dead_code)]

/// 包级 TestMain：进程默认值与泄漏检查约定。
#[cfg(test)]
mod main_test;
/// Plan Replayer dump/load/capture 执行器契约测试。
#[cfg(test)]
mod plan_replayer_test;
