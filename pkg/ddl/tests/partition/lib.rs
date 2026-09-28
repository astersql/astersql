// Copyright 2026 AsterSQL.

// 分区 DDL 测试 crate 入口。
//
// 对应 Go `pkg/ddl/tests/partition`：覆盖分区表创建/修改列/交换分区、
// 全局索引版本、Placement Policy（放置策略）、多 Domain 并发可见性，
// 以及 Reorganize Partition（重组分区，重划分区边界并回填数据）等场景。
// 各子模块仅在 `#[cfg(test)]` 下编译。

#![allow(dead_code)]

#[cfg(test)]
mod db_partition_test;
#[cfg(test)]
mod error_injection_test;
#[cfg(test)]
mod exchange_partition_test;
#[cfg(test)]
mod global_index_version_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod modify_column_test;
#[cfg(test)]
mod multi_domain_part1_aster_unit_test;
#[cfg(test)]
mod multi_domain_part2_aster_unit_test;
#[cfg(test)]
mod multi_domain_part3_aster_unit_test;
#[cfg(test)]
mod placement_test;
#[cfg(test)]
mod reorg_partition_test;
