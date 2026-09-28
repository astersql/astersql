// Copyright 2026 AsterSQL.

// `indexmerge` casetest crate 入口。
//
// Index Merge（索引合并）指优化器对 OR/AND 等复合谓词分别走多条索引访问路径，
// 再将行集做并集或交集合并。本 crate 仅在 `cfg(test)` 下挂载交叉、路径与主测试。

#![allow(dead_code)]

/// Index Merge 交集谓词归一化（NormalizeDigest）用例。
#[cfg(test)]
#[path = "indexmerge_intersection_test.rs"]
mod indexmerge_intersection_test;
/// Index Merge IN 列表常量无关归一化用例。
#[cfg(test)]
#[path = "indexmerge_path_test.rs"]
mod indexmerge_path_test;
/// Index Merge AND/OR 谓词结构在 digest 中的区分用例。
#[cfg(test)]
#[path = "indexmerge_test.rs"]
mod indexmerge_test;
/// 对应 Go TestMain / hint 保留语义的占位测试。
#[cfg(test)]
#[path = "main_test.rs"]
pub(crate) mod main_test;
