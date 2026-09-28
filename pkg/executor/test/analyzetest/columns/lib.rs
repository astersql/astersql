// Copyright 2026 AsterSQL.

// ANALYZE 指定列（COLUMNS / PREDICATE COLUMNS）测试 crate 入口。
//
// 对应 Go `pkg/executor/test/analyzetest/columns` 包。该路径验证
// `ANALYZE TABLE ... COLUMNS ...` / `PREDICATE COLUMNS` 在主键、索引、
// 分区表与虚拟列场景下的列选择与缺失列告警语义。

#![allow(dead_code)]

/// `ANALYZE ... COLUMNS` / `PREDICATE COLUMNS` 用例与 fixture。
#[cfg(test)]
mod analyze_columns_with_test;
/// 包级 TestMain：公共测试初始化与统计缓存内存配额开关。
#[cfg(test)]
mod main_test;
