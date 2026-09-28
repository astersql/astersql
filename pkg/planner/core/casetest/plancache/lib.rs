// Copyright 2026 AsterSQL.

// `plancache` casetest crate 入口。
//
// 聚合会话级执行计划缓存（Plan Cache）相关用例：参数化、分区表、重建、
// 可缓存性检查等。Plan Cache 将 prepared / 可参数化语句的物理计划缓存起来，
// 命中时跳过重复优化以降低延迟。仅在 `cfg(test)` 下挂载子模块。

#![allow(dead_code)]

/// 对应 Go TestMain：prepared plan cache 内存上限读写往返。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// SQL 参数化、占位符还原与 Params2Expressions 语义测试。
#[cfg(test)]
#[path = "plan_cache_param_test.rs"]
mod plan_cache_param_test;
/// 分区表场景下 plan cache 行为测试。
#[cfg(test)]
#[path = "plan_cache_partition_table_test.rs"]
mod plan_cache_partition_table_test;
/// 分区裁剪与 plan cache 交互测试。
#[cfg(test)]
#[path = "plan_cache_partition_test.rs"]
mod plan_cache_partition_test;
/// 缓存计划失效后重建（rebuild）路径测试。
#[cfg(test)]
#[path = "plan_cache_rebuild_test.rs"]
mod plan_cache_rebuild_test;
/// plan cache suite 级编排与 golden/fixture 相关测试。
#[cfg(test)]
#[path = "plan_cache_suite_test.rs"]
mod plan_cache_suite_test;
/// plan cache 通用功能与命中/未命中用例。
#[cfg(test)]
#[path = "plan_cache_test.rs"]
mod plan_cache_test;
/// 语句是否可进入 plan cache 的 checker 测试。
#[cfg(test)]
#[path = "plan_cacheable_checker_test.rs"]
mod plan_cacheable_checker_test;
