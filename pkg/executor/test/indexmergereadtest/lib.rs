// Copyright 2026 AsterSQL.

// Index Merge Reader（索引合并读）测试 crate 入口。
//
// 对应 Go `pkg/executor/test/indexmergereadtest`：验证 IndexMerge 执行器
// 对多路索引扫描结果的去重与按分区/排序键归并。Index Merge 在优化器
// 选择多条索引路径时，把各路 handle 合并成最终行集。
// 聚合 `index_merge_reader_test` 与包级 `main_test`。

#![allow(dead_code)]

/// IndexMergeHandle 去重与分区序归并的单元用例。
#[cfg(test)]
mod index_merge_reader_test;
/// 包级 TestMain：autoid、全局配置与 failpoint 语义。
#[cfg(test)]
mod main_test;
