// Copyright 2026 AsterSQL.

// 逻辑算子（Logical Operator）crate 入口。
//
// 聚合各类逻辑计划节点（聚合、连接、投影、CTE、扫描等），对外 re-export 公共类型与
// 表达式/属性相关别名。逻辑计划是优化器在物理化之前的代数树形态。
//
#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

// 各逻辑算子与生成代码（Hash64/Equals、浅引用）子模块。
mod base_logical_plan;
mod expression_util;
mod hash64_equals_generated;
mod logical_aggregation;
mod logical_apply;
mod logical_cte;
mod logical_cte_table;
mod logical_datasource;
mod logical_expand;
mod logical_index_scan;
mod logical_join;
mod logical_limit;
mod logical_lock;
mod logical_max_one_row;
mod logical_mem_table;
mod logical_mock;
mod logical_partition_union_all;
mod logical_plans_misc;
mod logical_projection;
mod logical_schema_producer;
mod logical_selection;
mod logical_sequence;
mod logical_show;
mod logical_show_ddl_jobs;
mod logical_sort;
mod logical_table_dual;
mod logical_table_scan;
mod logical_tikv_single_gather;
mod logical_top_n;
mod logical_union_all;
mod logical_union_scan;
mod logical_window;
mod shallow_ref_generated;

// 对外导出全部逻辑算子实现，供 planner/core 与规则层使用。
pub use base_logical_plan::*;
pub use expression_util::*;
pub use logical_aggregation::*;
pub use logical_apply::*;
pub use logical_cte::*;
pub use logical_cte_table::*;
pub use logical_datasource::*;
pub use logical_expand::*;
pub use logical_index_scan::*;
pub use logical_join::*;
pub use logical_limit::*;
pub use logical_lock::*;
pub use logical_max_one_row::*;
pub use logical_mem_table::*;
pub use logical_mock::*;
pub use logical_partition_union_all::*;
pub use logical_plans_misc::*;
pub use logical_projection::*;
pub use logical_schema_producer::*;
pub use logical_selection::*;
pub use logical_sequence::*;
pub use logical_show::*;
pub use logical_show_ddl_jobs::*;
pub use logical_sort::*;
pub use logical_table_dual::*;
pub use logical_table_scan::*;
pub use logical_tikv_single_gather::*;
pub use logical_top_n::*;
pub use logical_union_all::*;
pub use logical_union_scan::*;
pub use logical_window::*;

// 与表达式、KV、AST、属性层的常用类型 re-export，减少调用方路径深度。
pub use expression::{Column, CorrelatedColumn, ExprBox as Expression, Schema};
pub use kv::StoreType;
pub use parser_ast::{SelectLockInfo, SelectLockType};
pub use plancodec::{TypeLimit, TypeStringToPhysicalID};
pub use planner_util::{ByItems, HandleCols};
pub use property::{SortItem, StatsInfo};
pub use types::metadata::{FieldName, NameSlice};

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 逻辑优化阶段错误包装，对应 Go 侧 planner 错误字符串。
pub struct PlannerError(pub String);

impl fmt::Display for PlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlannerError {}

/// 本 crate 统一 Result 别名。
pub type Result<T> = std::result::Result<T, PlannerError>;

// 单元测试模块（Aster 侧迁移测试）。
#[cfg(test)]
mod logical_datasource_aster_unit_test;
#[cfg(test)]
mod logical_datasource_test;
#[cfg(test)]
mod logical_index_scan_test;

#[cfg(test)]
#[path = "logical_relational_aster_unit_test.rs"]
mod logical_relational_aster_unit_test;

#[cfg(test)]
#[path = "logical_aggregation_descriptor_aster_unit_test.rs"]
mod logical_aggregation_descriptor_aster_unit_test;

#[cfg(test)]
mod logical_aggregation_test;

#[cfg(test)]
mod logical_d_aster_unit_test;

#[cfg(test)]
mod expression_util_test;

#[cfg(test)]
mod hash64_equals_generated_test;

#[cfg(test)]
mod logical_generated_aster_unit_test;

#[cfg(test)]
mod logical_apply_test;

#[cfg(test)]
mod logical_cte_test;

#[cfg(test)]
mod logical_expand_test;

#[cfg(test)]
mod logical_join_test;

#[cfg(test)]
mod logical_limit_test;

#[cfg(test)]
mod logical_lock_test;

#[cfg(test)]
mod logical_max_one_row_test;

#[cfg(test)]
mod logical_mem_table_test;

#[cfg(test)]
mod logical_partition_union_all_test;

#[cfg(test)]
mod logical_plans_misc_test;

#[cfg(test)]
mod logical_projection_test;

#[cfg(test)]
mod logical_schema_producer_test;

#[cfg(test)]
mod logical_show_test;

#[cfg(test)]
mod logical_sort_test;

#[cfg(test)]
mod logical_top_n_test;

#[cfg(test)]
mod logical_union_all_test;

#[cfg(test)]
mod logical_union_scan_test;

#[cfg(test)]
mod logical_table_dual_test;

#[cfg(test)]
mod logical_table_scan_test;

#[cfg(test)]
mod logical_tikv_single_gather_test;

#[cfg(test)]
mod logical_window_test;
