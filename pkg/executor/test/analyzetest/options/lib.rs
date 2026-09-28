// Copyright 2026 AsterSQL.

// ANALYZE 选项测试 crate 入口。
//
// 对应 Go `pkg/executor/test/analyzetest/options` 包。
// 关注已保存 ANALYZE 选项的加载、复用，以及与 AutoAnalyze 生命周期的衔接。

#![allow(dead_code)]

/// 已保存 ANALYZE 选项在分区刷新等场景的复用语义测试。
#[cfg(test)]
mod analyze_saved_options_test;
/// 包级 TestMain / 运行时生命周期夹具。
#[cfg(test)]
mod main_test;
