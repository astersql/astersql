// Copyright 2026 AsterSQL.

// `lockstats` 测试子包入口。
//
// 覆盖 LOCK/UNLOCK STATS（锁定/解锁表或分区统计，阻止 ANALYZE 覆盖）相关用例：
// 表级锁定、分区级锁定，以及包级 harness。

#![allow(dead_code)]

#[cfg(test)]
#[path = "lock_table_stats_test.rs"]
/// 表级 LOCK/UNLOCK STATS 及辅助断言、setup。
mod lock_table_stats_test;

#[cfg(test)]
#[path = "lock_partition_stats_test.rs"]
/// 分区级 LOCK/UNLOCK STATS，以及分区 DDL 后锁信息清理。
mod lock_partition_stats_test;

#[cfg(test)]
#[path = "main_test.rs"]
/// 包级测试入口与公共 setup。
mod main_test;
