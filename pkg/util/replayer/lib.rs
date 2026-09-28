// Copyright 2026 AsterSQL.

// `util/replayer` crate 入口：Plan Replayer 文件命名与对象存储写入。
//
// 对应 Go `pkg/util/replayer`。Plan Replayer 用于导出执行计划、统计信息等，
// 便于异地复现优化器行为；本 crate 负责生成捕获文件名并写入相对 `replayer/` 目录。

/// Plan Replayer 核心类型与文件生成 API。
pub mod replayer;
/// 再导出 replayer 模块公开 API。
pub use replayer::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充：文件名分支与 WriteCloser 转发测试。
mod migration_aster_unit_test;
