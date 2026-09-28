// Copyright 2026 AsterSQL.

// 表 Region 分裂（split table）测试 crate 入口。
//
// 对应 Go `pkg/executor/test/splittest` 包。Region 是 TiKV 数据分片单位；
// `SPLIT TABLE ... REGIONS` 按主键/索引范围预分裂以降低热点。本 crate 挂载
// 包级冒烟与 `split_table_test` 中对 regionsplit/executor::split
// 的真实算法与执行器校验。

#![allow(dead_code)]

/// 包级 TestMain 对应的 MinRegionStepValue 冒烟。
#[cfg(test)]
mod main_test;
/// SPLIT TABLE / SHOW TABLE REGIONS 相关的分裂点算法与执行器单元测试。
#[cfg(test)]
mod split_table_test;
