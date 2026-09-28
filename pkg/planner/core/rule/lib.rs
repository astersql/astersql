// Copyright 2026 AsterSQL.

// 逻辑优化规则（Logical Optimization Rules）crate 入口。
//
// 导出各类逻辑改写规则模块：列裁剪、常量传播、Join 重排、分区处理、
// 统计信息收集等。规则通过位掩码（见 `logical_rules`）控制启用集合。

#![allow(dead_code)]

/// 规则位标志常量与辅助函数。
mod logical_rules;
pub use logical_rules::*;

/// 收集列统计使用情况。
pub mod collect_column_stats_usage;
/// 构建唯一键 / 候选键信息。
pub mod rule_build_key_info;
/// 收集计划所需统计信息并触发加载。
pub mod rule_collect_plan_stats;
/// 列裁剪：删除计划树中未引用列。
pub mod rule_column_pruning;
/// 常量传播：用已知常量替换等价谓词。
pub mod rule_constant_propagation;
/// 规则初始化与共享计划桩类型。
pub mod rule_init;
/// Join 键类型强制转换对齐。
pub mod rule_join_key_type_cast;
/// MAX/MIN 聚合消除优化。
pub mod rule_max_min_eliminate;
/// 保序感知的 Join 重排。
pub mod rule_order_aware_join_reorder;
/// 外连接转半连接。
pub mod rule_outer_join_to_semi_join;
/// 分区表处理与裁剪相关规则。
pub mod rule_partition_processor;
/// 谓词简化。
pub mod rule_predicate_simplification;
/// 无用索引路径裁剪。
pub mod rule_prune_indexes;

#[cfg(test)]
mod collect_column_stats_usage_test;
#[cfg(test)]
mod rule_aster_unit_test;
#[cfg(test)]
mod rule_build_key_info_test;
#[cfg(test)]
mod rule_collect_plan_stats_test;
#[cfg(test)]
mod rule_column_pruning_test;
#[cfg(test)]
mod rule_constant_propagation_test;
#[cfg(test)]
mod rule_init_test;
#[cfg(test)]
mod rule_join_key_type_cast_test;
#[cfg(test)]
mod rule_max_min_eliminate_test;
#[cfg(test)]
mod rule_order_aware_join_reorder_test;
#[cfg(test)]
mod rule_outer_join_to_semi_join_test;
#[cfg(test)]
mod rule_partition_processor_test;
#[cfg(test)]
mod rule_partition_pruning_test;
#[cfg(test)]
mod rule_predicate_simplification_test;
#[cfg(test)]
mod rule_prune_indexes_test;
