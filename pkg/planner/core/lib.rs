// Copyright 2026 AsterSQL.

// 规划器 core crate 的库根：模块声明、公开重导出与表达式工厂入口。
//
// 本包对应 TiDB `planner/core`，负责逻辑/物理计划构建、优化规则、代价估算、
// 计划缓存与各类谓词抽取器。对外暴露优化入口与子模块；测试通过 `#[path]`
// 挂载同目录测试文件。

#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

/// Go `context` 兼容层，用于规划器调用边界。
///
/// 两个根上下文永不超时、永不取消且不携带值，行为与
/// `context.Background()` / `context.TODO()` 一致。
///
/// Go `context` compatibility used at planner call boundaries.
///
/// The two root contexts never expire, are never cancelled and carry no
/// values, matching `context.Background()` and `context.TODO()` exactly.
pub mod context {
    pub use infoschema_dependency::{
        Background, BackgroundArc, ContextError, RequestContext as Context, TODO, TODOArc,
    };
}

/// 解析器 AST 类型再导出，便于 core 内统一引用。
pub mod ast {
    pub use parser_ast_dependency::*;
}

/// 穷举物理计划候选。
pub mod exhaust_physical_plans;
/// 在物理属性要求下为逻辑算子寻找最优任务。
pub mod find_best_task;
/// Index Join 路径构造。
pub mod index_join_path;
#[cfg(test)]
mod index_join_path_test;
/// IndexMerge 路径构造。
pub mod indexmerge_path;
/// 未完成的 IndexMerge 路径中间态。
pub mod indexmerge_unfinished_path;
/// 逻辑计划构建（简化/骨架实现）。
pub mod logical_plan_builder;
#[cfg(test)]
mod logical_plan_builder_test;
/// 代价模型版本 1。
pub mod plan_cost_ver1;
#[cfg(test)]
mod plan_cost_ver1_test;
/// 代价模型版本 2。
pub mod plan_cost_ver2;
/// 计划构建器主体与语句入口。
pub mod planbuilder;
/// 点查（Point Get）计划。
pub mod point_get_plan;
pub mod point_get_plan_runtime;
#[cfg(test)]
mod point_get_plan_runtime_test;
#[cfg(test)]
mod point_get_plan_test;
/// AST 预处理。
pub mod preprocess;
/// CTE 重入检查。
pub mod recheck_cte;
pub mod rule_aggregation_elimination;
#[cfg(test)]
mod rule_aggregation_elimination_test;
pub mod rule_aggregation_push_down;
#[cfg(test)]
mod rule_aggregation_push_down_test;
pub mod rule_aggregation_skew_rewrite;
#[cfg(test)]
mod rule_aggregation_skew_rewrite_test;
pub mod rule_correlate;
#[cfg(test)]
mod rule_correlate_test;
pub mod rule_decorrelate;
#[cfg(test)]
#[path = "rule_decorrelate_test.rs"]
mod rule_decorrelate_test;
pub mod rule_derive_topn_from_window;
#[cfg(test)]
mod rule_derive_topn_from_window_test;
pub mod rule_eliminate_empty_selection;
#[cfg(test)]
mod rule_eliminate_empty_selection_test;
pub mod rule_eliminate_projection;
#[cfg(test)]
mod rule_eliminate_projection_test;
pub mod rule_eliminate_unionall_dual_item;
#[cfg(test)]
mod rule_eliminate_unionall_dual_item_test;
pub mod rule_generate_column_substitute;
pub mod rule_inject_extra_projection;
#[cfg(test)]
mod rule_inject_extra_projection_test;
pub mod rule_join_elimination;
#[cfg(test)]
mod rule_join_elimination_test;
pub mod rule_join_reorder;
pub mod rule_join_reorder_dp;
pub mod rule_join_reorder_greedy;
#[cfg(test)]
mod rule_join_reorder_greedy_test;
pub mod rule_join_reorder_projection_inline;
#[cfg(test)]
mod rule_join_reorder_projection_inline_test;
#[cfg(test)]
mod rule_join_reorder_test;
pub mod rule_outer_to_inner_join;
#[cfg(test)]
mod rule_outer_to_inner_join_test;
pub mod rule_predicate_push_down;
#[cfg(test)]
mod rule_predicate_push_down_test;
pub mod rule_push_down_sequence;
#[cfg(test)]
mod rule_push_down_sequence_test;
pub mod rule_resolve_grouping_expand;
#[cfg(test)]
mod rule_resolve_grouping_expand_test;
pub mod rule_result_reorder;
#[cfg(test)]
mod rule_result_reorder_test;
pub mod rule_semi_join_rewrite;
#[cfg(test)]
#[path = "rule_semi_join_rewrite_test.rs"]
mod rule_semi_join_rewrite_test;
pub mod rule_topn_push_down;
#[cfg(test)]
mod rule_topn_push_down_test;
/// 子查询逻辑计划构建。
pub mod subquery_plan_builder;
/// 物理任务与属性传递。
pub mod task;

mod access_object;
mod columnar_index_utils;
mod common_plans;
mod core_init;
mod encode;
mod expression_codec_fn;
mod expression_rewriter;
mod flat_plan;
mod fts_plan_builder;
mod fts_resolve_index;
mod fulltext_to_like;
mod hint_utils;
mod initialize;
#[cfg(test)]
mod initialize_test;
mod logical_initialize;
mod logical_plan_builder_runtime;
mod memtable_infoschema_extractor;
mod memtable_predicate_extractor;
#[cfg(test)]
mod memtable_predicate_extractor_test;
mod optimizer;
mod optimizer_runtime;
mod pb_to_plan;
#[cfg(test)]
mod pb_to_plan_test;
mod plan;
mod plan_cache;
mod plan_cache_instance;
mod plan_cache_lru;
mod plan_cache_param;
mod plan_cache_rebuild;
#[cfg(test)]
mod plan_cache_test;
mod plan_cache_utils;
mod plan_cacheable_checker;
#[cfg(test)]
mod plan_cacheable_checker_test;
mod plan_clone_utils;
#[cfg(test)]
mod plan_clone_utils_test;
mod planbuilder_runtime;
#[cfg(test)]
mod planbuilder_runtime_test;
mod property_cols_prune;
#[cfg(test)]
mod property_cols_prune_test;
mod resolve_indices;
#[cfg(test)]
mod resolve_indices_test;
mod runtime_filter_generator;
mod scalar_subq_expression;
#[cfg(test)]
mod scalar_subq_expression_test;
mod schema_table_key;
#[cfg(test)]
mod schema_table_key_test;
mod show_predicate_extractor;
#[cfg(test)]
mod show_predicate_extractor_test;
mod stats;
mod stringer;
mod telemetry;
#[cfg(test)]
mod telemetry_test;
mod trace;
mod util;

pub use columnar_index_utils::*;
pub use common_plans::*;
pub use core_init::*;
pub use encode::*;
pub use expression_codec_fn::*;
pub(crate) use expression_rewriter::{subQueryCtx, unknowClause};
pub use flat_plan::*;
pub use fts_plan_builder::*;
pub use fts_resolve_index::*;
pub use fulltext_to_like::*;
pub use hint_utils::*;
pub use initialize::*;
pub use logical_plan_builder_runtime::{appendDynamicVisitInfo, resetCTECheckForSubQuery};
pub use memtable_infoschema_extractor::*;
pub use memtable_predicate_extractor::*;
pub use optimizer_runtime::{
    DoOptimize, InstallOptimizeAstNode, LogicalOptimizeForMpp, LogicalOptimizeForTest,
    MaxMemoryLimitForOverlongType, OptimizeAstNodeFn, PhysicalOptimizeForMpp,
    ShouldSkipReuseChunkForPhysicalPlan, ShouldSkipReuseChunkForPointGet,
};
pub use pb_to_plan::*;
pub use plan::*;
pub use plan_cache::*;
pub use plan_cache_instance::*;
pub use plan_cache_lru::*;
pub use plan_cache_param::*;
pub use plan_cache_rebuild::*;
pub use plan_cache_utils::*;
pub use plan_cacheable_checker::*;
pub use plan_clone_utils::*;
pub use planbuilder_runtime::*;
pub use property_cols_prune::*;
pub use resolve_indices::*;
pub use runtime_filter_generator::*;
pub use scalar_subq_expression::{
    EvalSubqueryFirstRowFn, InstallEvalSubqueryFirstRow, ScalarSubQueryExpr, ScalarSubqueryEvalCtx,
};
pub use schema_table_key::*;
pub use show_predicate_extractor::*;
pub use stats::*;
pub use stringer::*;
pub use telemetry::*;
pub use trace::*;
pub use util::*;

/// 规划器侧的简单表达式构建入口；安装工厂后再委托 expression 包。
///
/// Go 在 planner 包 init 中安装该回调；Rust 无包级 init，故在公开入口
/// 幂等地建立同一边界，避免嵌套生成/默认表达式递归时工厂尚未就绪。
pub fn PlannerBuildSimpleExpr<'a>(
    ctx: &dyn expression_dependency::BuildContext,
    node: &ast::ExprNode,
    options: Vec<expression_dependency::BuildOption<'a>>,
) -> Result<expression_dependency::ExprBox, expression_dependency::errors::Error> {
    // Go installs this callback from planner package init. Rust has no package
    // init hook, so the public planner entry point establishes the same
    // idempotent boundary before nested generated/default expressions recurse
    // through expression::BuildSimpleExpr.
    // Go 从 planner 包 init 安装回调；Rust 无包级 init，故在公开入口幂等建立同一边界。
    InstallPlannerExpressionFactory()?;
    expression_rewriter::buildSimpleExpr(ctx, Some(node), options)
}

/// 将 `PlannerBuildSimpleExpr` 注册为 expression 包的默认简单表达式工厂。
pub fn InstallPlannerExpressionFactory() -> Result<(), expression_dependency::errors::Error> {
    expression_dependency::InstallBuildSimpleExpr(PlannerBuildSimpleExpr)
}

#[cfg(test)]
#[path = "fulltext_to_like_test.rs"]
mod fulltext_to_like_test;

#[cfg(test)]
#[path = "columnar_index_utils_test.rs"]
mod columnar_index_utils_test;

#[cfg(test)]
#[path = "core_init_test.rs"]
mod core_init_test;

#[cfg(test)]
#[path = "exhaust_physical_plans_test.rs"]
mod exhaust_physical_plans_test;

#[cfg(test)]
#[path = "encode_test.rs"]
mod encode_test;

#[cfg(test)]
#[path = "common_plans_test.rs"]
mod common_plans_test;

#[cfg(test)]
#[path = "fts_resolve_index_test.rs"]
mod fts_resolve_index_test;

#[cfg(test)]
#[path = "find_best_task_test.rs"]
mod find_best_task_test;

#[cfg(test)]
#[path = "expression_test.rs"]
mod expression_test;

#[cfg(test)]
#[path = "real_result_set_builder_aster_unit_test.rs"]
mod real_result_set_builder_aster_unit_test;

#[cfg(test)]
#[path = "logical_plan_builder_subquery_join_aster_unit_test.rs"]
mod logical_plan_builder_subquery_join_aster_unit_test;

#[cfg(test)]
#[path = "enforce_mpp_test.rs"]
mod enforce_mpp_test;

#[cfg(test)]
#[path = "cbo_test.rs"]
mod cbo_test;

#[cfg(test)]
#[path = "hint_test.rs"]
mod hint_test;

#[cfg(test)]
#[path = "hint_utils_test.rs"]
mod hint_utils_test;

#[cfg(test)]
#[path = "integration_test.rs"]
mod integration_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "lateral_join_test.rs"]
mod lateral_join_test;

#[cfg(test)]
#[path = "logical_plans_test.rs"]
mod logical_plans_test;

#[cfg(test)]
#[path = "logical_plan_builder_lateral_with_aster_unit_test.rs"]
mod logical_plan_builder_lateral_with_aster_unit_test;
#[cfg(test)]
mod optimizer_logical_entry_aster_unit_test;

#[cfg(test)]
#[path = "optimizer_identity_projection_aster_unit_test.rs"]
mod optimizer_identity_projection_aster_unit_test;

#[cfg(test)]
#[path = "panicrisk_regression_test.rs"]
mod panicrisk_regression_test;

#[cfg(test)]
#[path = "expression_codec_fn_test.rs"]
mod expression_codec_fn_test;

#[cfg(test)]
#[path = "plan_cache_lru_test.rs"]
mod plan_cache_lru_test;

#[cfg(test)]
#[path = "plan_cache_utils_test.rs"]
mod plan_cache_utils_test;

#[cfg(test)]
#[path = "plan_cache_rebuild_test.rs"]
mod plan_cache_rebuild_test;

#[cfg(test)]
#[path = "plan_cache_instance_test.rs"]
mod plan_cache_instance_test;

#[cfg(test)]
#[path = "plan_to_pb_test.rs"]
mod plan_to_pb_test;

#[cfg(test)]
#[path = "optimizer_test.rs"]
mod optimizer_test;

#[cfg(test)]
#[path = "plan_cost_ver2_test.rs"]
mod plan_cost_ver2_test;

#[cfg(test)]
#[path = "plan_replayer_capture_test.rs"]
mod plan_replayer_capture_test;

#[cfg(test)]
#[path = "plan_test.rs"]
mod plan_test;

#[cfg(test)]
#[path = "physical_plan_test.rs"]
mod physical_plan_test;

#[cfg(test)]
#[path = "planbuilder_test.rs"]
mod planbuilder_test;

#[cfg(test)]
#[path = "preprocess_test.rs"]
mod preprocess_test;

#[cfg(test)]
#[path = "rule_generate_column_substitute_test.rs"]
mod rule_generate_column_substitute_test;

#[cfg(test)]
#[path = "rule_join_reorder_dp_test.rs"]
mod rule_join_reorder_dp_test;

#[cfg(test)]
#[path = "runtime_filter_generator_test.rs"]
mod runtime_filter_generator_test;

#[cfg(test)]
#[path = "stats_test.rs"]
mod stats_test;

#[cfg(test)]
#[path = "stringer_test.rs"]
mod stringer_test;

#[cfg(test)]
#[path = "task_heavy_function_optimize_test.rs"]
mod task_heavy_function_optimize_test;

#[cfg(test)]
#[path = "task_test.rs"]
mod task_test;

#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;

#[cfg(test)]
#[path = "expression_rewriter_test.rs"]
mod expression_rewriter_test;

#[cfg(test)]
#[path = "flat_plan_test.rs"]
mod flat_plan_test;

#[cfg(test)]
#[path = "logical_plan_builder_runtime_aster_unit_test.rs"]
mod logical_plan_builder_runtime_aster_unit_test;
#[cfg(test)]
#[path = "logical_plan_builder_runtime_test.rs"]
mod logical_plan_builder_runtime_test;

#[cfg(test)]
#[path = "plan_cache_utils_aster_unit_test.rs"]
mod plan_cache_utils_aster_unit_test;

#[cfg(test)]
#[path = "scalar_subq_expression_aster_unit_test.rs"]
mod scalar_subq_expression_aster_unit_test;

#[cfg(test)]
#[path = "indexmerge_path_test.rs"]
mod indexmerge_path_test;

#[cfg(test)]
#[path = "indexmerge_unfinished_path_test.rs"]
mod indexmerge_unfinished_path_test;
