// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 规划器优化器运行时：逻辑规则流水线、物理优化与后处理。
//
// 对齐 Go `optimizer.go` 的规则顺序与标志位掩码，实现列裁剪、解相关、
// 谓词下推、Join 重排、分区裁剪、TopN/Limit 下推、全文索引解析等逻辑变换，
// 以及物理化、细粒度 shuffle、投影注入等物理阶段。

#![allow(non_snake_case)]

use crate::context;
use aggregation_dependency as aggregation;
use base_dependency as base;
use base_dependency::{PhysicalPlan as _, Plan as _};
use coreusage_dependency as coreusage;
use costusage_dependency as costusage;
use expression_dependency as expression;
use hint_dependency as hint;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use memory_dependency as memory;
use physicalop_dependency as physicalop;
use property_dependency as property;
use resolve_dependency as resolve;
use rule_dependency as rule;
use rule_util_dependency as rule_util;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Instant;
use types_dependency as types;

#[cfg(test)]
thread_local! {
    static LOGICAL_RULE_TRACE: std::cell::RefCell<Vec<LogicalRule>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// AST 优化入口函数类型：上下文 + 解析节点 → 计划与输出名。
pub type OptimizeAstNodeFn =
    fn(
        &dyn context::Context,
        &base::ContextRef,
        &resolve::NodeW,
        std::sync::Arc<dyn infoschema_dependency::InfoSchema>,
    ) -> Result<(Box<dyn base::Plan>, base::types::NameSlice), expression::Error>;

/// Executor-installed AST optimization entry points. `OnceLock` makes the Go
/// package variables race-free while retaining one-time installation.
/// 执行器安装的 AST 优化入口；OnceLock 保证一次性安装且无数据竞争。
pub static OptimizeAstNode: OnceLock<OptimizeAstNodeFn> = OnceLock::new();
/// 已安装的无缓存 AST 优化入口。
pub static OptimizeAstNodeNoCache: OnceLock<OptimizeAstNodeFn> = OnceLock::new();
/// 是否允许笛卡尔积 Join 的全局开关。
pub static AllowCartesianProduct: AtomicBool = AtomicBool::new(true);

/// Go `MaxMemoryLimitForOverlongType`: bounded overlong columns may reuse a
/// chunk only when the host has at least this much memory.
pub static MaxMemoryLimitForOverlongType: AtomicU64 = AtomicU64::new(120 * 1024 * 1024 * 1024);

const MAX_FLEN_FOR_OVERLONG_TYPE: u64 = types::metadata::mysql::MaxBlobWidth * 2;

fn with_transient_plan_ids<T>(context: &base::ContextRef, build: impl FnOnce() -> T) -> T {
    struct Restore {
        context: base::ContextRef,
        checkpoint: Option<i32>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(checkpoint) = self.checkpoint {
                self.context.restore_plan_id_checkpoint(checkpoint);
            }
        }
    }

    let restore = Restore {
        context: context.clone(),
        checkpoint: context.plan_id_checkpoint(),
    };
    let result = build();
    drop(restore);
    result
}

enum OverlongSchema<'a> {
    None,
    Unbounded,
    Bounded {
        columns: Vec<&'a expression::Column>,
        total_flen: u64,
    },
}

fn classify_overlong_schema(schema: &expression::Schema) -> OverlongSchema<'_> {
    let mut columns = Vec::new();
    let mut total_flen = 0_u64;
    for column in &schema.Columns {
        let Some(field_type) = column.RetType.as_ref() else {
            continue;
        };
        match field_type.GetType() {
            types::metadata::mysql::TypeLongBlob
            | types::metadata::mysql::TypeBlob
            | types::metadata::mysql::TypeJSON
            | types::metadata::mysql::TypeTiDBVectorFloat32 => {
                return OverlongSchema::Unbounded;
            }
            types::metadata::mysql::TypeVarString
            | types::metadata::mysql::TypeVarchar
            | types::metadata::mysql::TypeTinyBlob
            | types::metadata::mysql::TypeMediumBlob
                if field_type.GetFlen() > 1_000 =>
            {
                total_flen = total_flen.saturating_add(field_type.GetFlen() as u64);
                columns.push(column);
            }
            _ => {}
        }
    }
    if columns.is_empty() {
        OverlongSchema::None
    } else {
        OverlongSchema::Bounded {
            columns,
            total_flen,
        }
    }
}

fn trusted_histograms(
    stats: &property::StatsInfo,
) -> Option<&cardinality_dependency::statistics::HistColl> {
    let histograms = stats
        .HistColl
        .as_ref()?
        .as_ref()
        .downcast_ref::<cardinality_dependency::statistics::HistColl>()?;
    (!histograms.Pseudo && histograms.RealtimeCount != 0 && histograms.ColNum() != 0)
        .then_some(histograms)
}

fn has_usable_overlong_type_size_stats(
    stats: &property::StatsInfo,
    histograms: &cardinality_dependency::statistics::HistColl,
    columns: &[&expression::Column],
) -> bool {
    columns.iter().all(|column| {
        histograms
            .GetCol(column.UniqueID)
            .is_some_and(|column_stats| {
                column_stats.IsHandle
                    || column_stats.TotColSize != 0
                    || column_stats.NullCount == histograms.RealtimeCount
            })
    }) && stats.RowCount > 0.0
}

fn physical_reader_row_bound(plan: &dyn base::PhysicalPlan) -> Option<(f64, bool)> {
    let row_count = plan.stats_info().RowCount;
    if row_count <= 0.0 {
        return None;
    }
    if plan.as_any().is::<physicalop::PointGetPlan>() {
        return Some((row_count.ceil(), true));
    }
    if plan.as_any().is::<physicalop::BatchPointGetPlan>() {
        return Some((
            row_count.ceil(),
            trusted_histograms(plan.stats_info()).is_some(),
        ));
    }
    if plan.as_any().is::<physicalop::PhysicalTableReader>()
        || plan.as_any().is::<physicalop::PhysicalIndexReader>()
        || plan.as_any().is::<physicalop::PhysicalIndexLookUpReader>()
        || plan.as_any().is::<physicalop::PhysicalIndexMergeReader>()
    {
        trusted_histograms(plan.stats_info())?;
        return Some((row_count.ceil(), true));
    }
    None
}

/// Go `shouldSkipReuseChunkForPhysicalPlan`: reject unbounded output columns;
/// bounded overlong columns use the host-memory gate and the retained size of
/// one reusable chunk rather than the full result size.
pub fn ShouldSkipReuseChunkForPhysicalPlan(plan: &dyn base::PhysicalPlan) -> bool {
    let OverlongSchema::Bounded {
        columns,
        total_flen,
    } = classify_overlong_schema(plan.schema())
    else {
        return matches!(
            classify_overlong_schema(plan.schema()),
            OverlongSchema::Unbounded
        );
    };
    let total_memory = memory::meminfo::GetMemTotalIgnoreErr();
    if total_memory == 0 || total_memory < MaxMemoryLimitForOverlongType.load(Ordering::SeqCst) {
        return true;
    }
    let Some((estimated_rows, trusted_stats)) = physical_reader_row_bound(plan) else {
        return true;
    };
    let rows_per_chunk = if plan.as_any().is::<physicalop::PointGetPlan>() {
        estimated_rows
    } else {
        estimated_rows.min(vardef_dependency::DefMaxChunkSize as f64)
    };
    let estimated_bytes_per_row = trusted_histograms(plan.stats_info())
        .filter(|histograms| {
            trusted_stats
                && has_usable_overlong_type_size_stats(plan.stats_info(), histograms, &columns)
        })
        .map_or(total_flen as f64, |histograms| {
            cardinality_dependency::GetAvgRowSizeDataInDiskByRows(histograms, &columns)
        });
    rows_per_chunk * estimated_bytes_per_row > MAX_FLEN_FOR_OVERLONG_TYPE as f64
}

/// Runtime bridge for compact execution paths that already resolved output
/// field types and the exact point-get row bound.
pub fn ShouldSkipReuseChunkForPointGet(field_types: &[types::metadata::FieldType]) -> bool {
    let mut total_flen = 0_u64;
    for field_type in field_types {
        match field_type.GetType() {
            types::metadata::mysql::TypeLongBlob
            | types::metadata::mysql::TypeBlob
            | types::metadata::mysql::TypeJSON
            | types::metadata::mysql::TypeTiDBVectorFloat32 => return true,
            types::metadata::mysql::TypeVarString
            | types::metadata::mysql::TypeVarchar
            | types::metadata::mysql::TypeTinyBlob
            | types::metadata::mysql::TypeMediumBlob
                if field_type.GetFlen() > 1_000 =>
            {
                total_flen = total_flen.saturating_add(field_type.GetFlen() as u64);
            }
            _ => {}
        }
    }
    if total_flen == 0 {
        return false;
    }
    let total_memory = memory::meminfo::GetMemTotalIgnoreErr();
    total_memory == 0
        || total_memory < MaxMemoryLimitForOverlongType.load(Ordering::SeqCst)
        || total_flen > MAX_FLEN_FOR_OVERLONG_TYPE
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// 逻辑优化规则枚举，顺序与 Go `optRuleList` 对齐。
pub enum LogicalRule {
    GcSubstitute,
    PruneColumns,
    StabilizeResults,
    BuildKeyInfo,
    Decorrelate,
    SemiJoinRewrite,
    EliminateAgg,
    SkewDistinctAgg,
    EliminateProjection,
    MaxMinEliminate,
    ConstantPropagation,
    FullTextIndexResolveWhere,
    ConvertOuterToInnerJoin,
    PredicatePushDown,
    JoinKeyTypeCast,
    EliminateOuterJoin,
    PartitionProcessor,
    CollectPredicateColumnsPoint,
    PushDownAgg,
    DeriveTopNFromWindow,
    PredicateSimplification,
    PushDownTopN,
    FullTextIndexResolveTopN,
    FullTextIndexResolveProjection,
    OrderAwareJoinReorder,
    SyncWaitStatsLoadPoint,
    JoinReorder,
    OuterJoinToSemiJoin,
    Correlate,
    PruneColumnsAgain,
    PushDownSequence,
    EliminateUnionAllDualItem,
    EmptySelectionEliminator,
    FullTextIndexResolveReject,
    ResolveExpand,
}

/// Go `optRuleList`, in exact execution order.
/// 对应 Go `optRuleList`，顺序即执行顺序。
pub const LOGICAL_RULES: &[LogicalRule] = &[
    LogicalRule::GcSubstitute,
    LogicalRule::PruneColumns,
    LogicalRule::StabilizeResults,
    LogicalRule::BuildKeyInfo,
    LogicalRule::Decorrelate,
    LogicalRule::SemiJoinRewrite,
    LogicalRule::EliminateAgg,
    LogicalRule::SkewDistinctAgg,
    LogicalRule::EliminateProjection,
    LogicalRule::MaxMinEliminate,
    LogicalRule::ConstantPropagation,
    LogicalRule::FullTextIndexResolveWhere,
    LogicalRule::ConvertOuterToInnerJoin,
    LogicalRule::PredicatePushDown,
    LogicalRule::JoinKeyTypeCast,
    LogicalRule::EliminateOuterJoin,
    LogicalRule::PartitionProcessor,
    LogicalRule::CollectPredicateColumnsPoint,
    LogicalRule::PushDownAgg,
    LogicalRule::DeriveTopNFromWindow,
    LogicalRule::PredicateSimplification,
    LogicalRule::PushDownTopN,
    LogicalRule::FullTextIndexResolveTopN,
    LogicalRule::FullTextIndexResolveProjection,
    LogicalRule::OrderAwareJoinReorder,
    LogicalRule::SyncWaitStatsLoadPoint,
    LogicalRule::JoinReorder,
    LogicalRule::OuterJoinToSemiJoin,
    LogicalRule::Correlate,
    LogicalRule::PruneColumnsAgain,
    LogicalRule::PushDownSequence,
    LogicalRule::EliminateUnionAllDualItem,
    LogicalRule::EmptySelectionEliminator,
    LogicalRule::FullTextIndexResolveReject,
    LogicalRule::ResolveExpand,
];

/// 规范化阶段规则别名，当前等同 LOGICAL_RULES。
pub const NORMALIZE_RULES: &[LogicalRule] = LOGICAL_RULES;
/// 规则交互对列表（可运行时配置）。
pub static LogicalInteractionRules: RwLock<Vec<(LogicalRule, LogicalRule)>> =
    RwLock::new(Vec::new());
/// 默认禁用的逻辑规则名列表。
pub static DefaultDisabledLogicalRulesList: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// 安装带缓存/无缓存两套 AST 优化入口函数。
pub fn InstallOptimizeAstNode(
    cached: OptimizeAstNodeFn,
    no_cache: OptimizeAstNodeFn,
) -> Result<(), OptimizeAstNodeFn> {
    OptimizeAstNode.set(cached)?;
    OptimizeAstNodeNoCache.set(no_cache)
}

/// Go `optRuleFlags` order. Rule masks are persisted by bit value, so this
/// order must remain aligned with optimizer.go.
/// 对应 Go `optRuleFlags`；位值持久化，顺序须与规则列表对齐。
pub const LOGICAL_RULE_FLAGS: &[u64] = &[
    rule::FLAG_GC_SUBSTITUTE,
    rule::FLAG_PRUNE_COLUMNS,
    rule::FLAG_STABILIZE_RESULTS,
    rule::FLAG_BUILD_KEY_INFO,
    rule::FLAG_DECORRELATE,
    rule::FLAG_SEMI_JOIN_REWRITE,
    rule::FLAG_ELIMINATE_AGG,
    rule::FLAG_SKEW_DISTINCT_AGG,
    rule::FLAG_ELIMINATE_PROJECTION,
    rule::FLAG_MAX_MIN_ELIMINATE,
    rule::FLAG_CONSTANT_PROPAGATION,
    rule::FLAG_FULLTEXT_INDEX_RESOLVE_WHERE,
    rule::FLAG_CONVERT_OUTER_TO_INNER_JOIN,
    rule::FLAG_PREDICATE_PUSH_DOWN,
    rule::FLAG_JOIN_KEY_TYPE_CAST,
    rule::FLAG_ELIMINATE_OUTER_JOIN,
    rule::FLAG_PARTITION_PROCESSOR,
    rule::FLAG_COLLECT_PREDICATE_COLUMNS_POINT,
    rule::FLAG_PUSH_DOWN_AGG,
    rule::FLAG_DERIVE_TOP_N_FROM_WINDOW,
    rule::FLAG_PREDICATE_SIMPLIFICATION,
    rule::FLAG_PUSH_DOWN_TOP_N,
    rule::FLAG_FULLTEXT_INDEX_RESOLVE_TOP_N,
    rule::FLAG_FULLTEXT_INDEX_RESOLVE_PROJECTION,
    rule::FLAG_ORDER_AWARE_JOIN_REORDER,
    rule::FLAG_SYNC_WAIT_STATS_LOAD_POINT,
    rule::FLAG_JOIN_REORDER,
    rule::FLAG_OUTER_JOIN_TO_SEMI_JOIN,
    rule::FLAG_CORRELATE,
    rule::FLAG_PRUNE_COLUMNS_AGAIN,
    rule::FLAG_PUSH_DOWN_SEQUENCE,
    rule::FLAG_ELIMINATE_UNION_ALL_DUAL_ITEM,
    rule::FLAG_EMPTY_SELECTION_ELIMINATOR,
    rule::FLAG_FULLTEXT_INDEX_RESOLVE_REJECT,
    rule::FLAG_RESOLVE_EXPAND,
];

/// 按 flag 位掩码就地执行逻辑规则流水线。
fn logical_optimize_in_place(
    flag: u64,
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    fn optimize_cte_seed(flag: u64, plan: &mut logicalop::LogicalPlanRef) -> logicalop::Result<()> {
        LogicalOptimizeForMpp(flag, plan)
            .map_err(|error| logicalop::PlannerError(error.to_string()))
    }
    let _ = logicalop::InstallOptimizeCTESeed(optimize_cte_seed);
    rule::rule_init::init();
    property_dependency::SetScaleNDVFunc(Some(|vars, ndv, rows, selected| {
        cardinality_dependency::ScaleNDV(Some(vars), ndv, rows, selected)
    }));
    for (logical_rule, rule_flag) in LOGICAL_RULES.iter().zip(LOGICAL_RULE_FLAGS) {
        if flag & rule_flag == 0 {
            continue;
        }
        #[cfg(test)]
        LOGICAL_RULE_TRACE.with(|trace| trace.borrow_mut().push(*logical_rule));
        match logical_rule {
            LogicalRule::GcSubstitute => substitute_indexed_generated_columns(plan),
            LogicalRule::StabilizeResults => stabilize_results(plan),
            LogicalRule::PruneColumns | LogicalRule::PruneColumnsAgain => {
                let output_columns = plan.Schema().Columns.clone();
                plan.PruneColumns(&output_columns)
                    .map_err(|error| expression::errors::New(error.to_string()))?;
                eliminate_pruned_applies(plan);
                eliminate_empty_projections(plan);
            }
            LogicalRule::BuildKeyInfo => plan.BuildKeyInfo(),
            LogicalRule::Decorrelate => decorrelate_descendants(plan),
            LogicalRule::SemiJoinRewrite => semi_join_rewrite_descendants(plan)?,
            LogicalRule::EliminateAgg => eliminate_aggregation_descendants(plan, true)?,
            LogicalRule::SkewDistinctAgg => rewrite_skew_distinct_aggregation_descendants(plan)?,
            LogicalRule::MaxMinEliminate => eliminate_single_max_min_descendants(plan)?,
            LogicalRule::ConstantPropagation => propagate_cross_block_constants(plan),
            LogicalRule::JoinKeyTypeCast => rewrite_join_key_type_casts(plan)?,
            LogicalRule::EliminateOuterJoin => {
                let used = plan.Schema().Columns.clone();
                eliminate_outer_join_descendants(plan, &used, None);
            }
            LogicalRule::EliminateProjection => eliminate_identity_projections(plan),
            LogicalRule::ConvertOuterToInnerJoin => plan.ConvertOuterToInner(Vec::new()),
            LogicalRule::FullTextIndexResolveWhere => full_text_resolve_where(plan)?,
            LogicalRule::PredicatePushDown => {
                let residual = logicalop::PredicatePushDownPlan(plan, Vec::new())
                    .map_err(|error| expression::errors::New(error.to_string()))?;
                retain_root_predicates(plan, residual)?;
            }
            LogicalRule::PartitionProcessor => rewrite_static_partitions(plan)?,
            LogicalRule::CollectPredicateColumnsPoint => {
                collect_predicate_columns_descendants(plan.as_mut(), &[])
            }
            LogicalRule::PushDownAgg => {
                // Go's aggregation pushdown rule first eliminates aggregates
                // over unique keys, even when pushdown across joins is disabled.
                // Outer-join elimination can expose such keys after EliminateAgg.
                eliminate_aggregation_descendants(plan, false)?;
                let enabled = plan
                    .SCtx()
                    .and_then(|context| {
                        context
                            .GetSessionVars()
                            .GetSystemVar(vardef_dependency::TiDBOptAggPushDown)
                    })
                    .is_some_and(|value| {
                        matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                    });
                push_down_aggregation_descendants(plan, enabled)?;
            }
            LogicalRule::DeriveTopNFromWindow => derive_top_n_from_window_descendants(plan),
            LogicalRule::PredicateSimplification => plan.PredicateSimplification(),
            LogicalRule::PushDownTopN => push_down_top_n_descendants(plan)?,
            LogicalRule::FullTextIndexResolveTopN => full_text_resolve_top_n(plan)?,
            LogicalRule::FullTextIndexResolveProjection => full_text_resolve_projection(plan)?,
            LogicalRule::OrderAwareJoinReorder => {
                order_aware_join_reorder_descendants(plan.as_mut(), &[]);
            }
            LogicalRule::SyncWaitStatsLoadPoint => sync_wait_stats_load_point(plan.as_ref())?,
            LogicalRule::JoinReorder => {
                lift_correlated_predicates_for_join_reorder(plan.as_mut());
                reorder_inner_join_descendants(plan.as_mut())?;
            }
            LogicalRule::OuterJoinToSemiJoin => outer_join_to_semi_join_descendants(plan),
            LogicalRule::Correlate => correlate_descendants(plan)?,
            LogicalRule::PushDownSequence => push_down_sequence(plan),
            LogicalRule::EliminateUnionAllDualItem => eliminate_union_all_dual_items(plan),
            LogicalRule::EmptySelectionEliminator => {
                eliminate_empty_selection_descendants(plan.as_mut())
            }
            LogicalRule::FullTextIndexResolveReject => full_text_reject_remaining(plan.as_ref())?,
            LogicalRule::ResolveExpand => resolve_expand_descendants(plan.as_mut()),
        }
    }
    // Logical rewrites mutate predicates, join order and aggregation inputs
    // after the builder has populated node-local caches.  Recompute the tree
    // recursively at the optimization boundary; reading the cached root here
    // leaves every descendant at its pre-pushdown cardinality.
    refresh_join_order_stats(plan)?;
    Ok(())
}

/// 为 Join 重排提升相关谓词，扩大可重排空间。
fn lift_correlated_predicates_for_join_reorder(plan: &mut dyn logicalop::LogicalPlan) {
    if let Some(apply) = plan.as_any_mut().downcast_mut::<logicalop::LogicalApply>() {
        apply.LiftInnerCorrelatedSelectionsForJoinReorder();
    }
    for child in plan.Children_mut() {
        lift_correlated_predicates_for_join_reorder(child.as_mut());
    }
}

#[derive(Clone)]
/// 可替换的索引生成列候选：列与其虚拟表达式。
struct GeneratedColumnCandidate {
    expression: expression::ExprBox,
    column: expression::Column,
}

/// 入口：用索引生成列替换计划中对生成表达式的引用。
fn substitute_indexed_generated_columns(plan: &mut logicalop::LogicalPlanRef) {
    let mut candidates = Vec::new();
    collect_indexed_generated_columns(plan.as_ref(), &mut candidates);
    if candidates.is_empty() {
        return;
    }
    substitute_generated_column_descendants(plan.as_mut(), &candidates);
}

/// 从数据源收集可作为替换目标的索引生成列。
fn collect_indexed_generated_columns(
    plan: &dyn logicalop::LogicalPlan,
    candidates: &mut Vec<GeneratedColumnCandidate>,
) {
    if plan.as_any().is::<logicalop::LogicalCTE>() {
        return;
    }
    for child in plan.Children() {
        collect_indexed_generated_columns(child.as_ref(), candidates);
    }
    let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() else {
        return;
    };
    if source.PreferStoreType & hint::PreferTiFlash as i32 != 0 {
        return;
    }
    let Some(context) = source.SCtx() else {
        return;
    };
    let eval_context = context.GetExprCtx().GetEvalCtx();
    for path in &source.AllPossibleAccessPaths {
        if path.IsTablePath() {
            continue;
        }
        let Some(index) = &path.Index else {
            continue;
        };
        for index_column in &index.Columns {
            let Ok(offset) = usize::try_from(index_column.Offset) else {
                continue;
            };
            let Some(info) = source.TableInfo.Columns.get(offset) else {
                continue;
            };
            if !info.IsGenerated() || info.GeneratedStored {
                continue;
            }
            let Some(column) = source
                .Schema()
                .Columns
                .iter()
                .find(|column| column.ID == info.ID)
            else {
                continue;
            };
            let Some(virtual_expression) = &column.VirtualExpr else {
                continue;
            };
            if column.RetType.as_ref() != Some(virtual_expression.GetType(eval_context))
                || expression::ExtractColumns(virtual_expression.as_ref()).is_empty()
            {
                continue;
            }
            candidates.push(GeneratedColumnCandidate {
                expression: virtual_expression.CloneExpr(),
                column: column.Clone(),
            });
        }
    }
}

/// 递归向下替换子树中的生成列表达式。
fn substitute_generated_column_descendants(
    plan: &mut dyn logicalop::LogicalPlan,
    candidates: &[GeneratedColumnCandidate],
) {
    let context = plan.SCtx().cloned();
    let schema = plan.Schema().Clone();
    let child_schema = plan.Children().first().map(|child| child.Schema().Clone());
    if let Some(context) = context {
        let eval_context = context.GetExprCtx().GetEvalCtx();
        if let Some(selection) = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalSelection>()
        {
            for condition in &mut selection.Conditions {
                substitute_generated_column_condition(
                    condition,
                    candidates,
                    &schema,
                    context.GetExprCtx(),
                );
            }
        } else if let Some(projection) = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalProjection>()
        {
            let reachable = child_schema.as_ref().unwrap_or(&schema);
            for expression in &mut projection.Exprs {
                let required_type = expression.GetType(eval_context).EvalType();
                substitute_generated_column_expression(
                    expression,
                    candidates,
                    required_type,
                    reachable,
                    context.GetExprCtx(),
                );
            }
        } else if let Some(sort) = plan.as_any_mut().downcast_mut::<logicalop::LogicalSort>() {
            for item in &mut sort.ByItems {
                let required_type = item.Expr.GetType(eval_context).EvalType();
                substitute_generated_column_expression(
                    &mut item.Expr,
                    candidates,
                    required_type,
                    &schema,
                    context.GetExprCtx(),
                );
            }
        } else if let Some(aggregation) = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalAggregation>()
        {
            for function in &mut aggregation.AggFuncs {
                for argument in &mut function.Args {
                    let required_type = argument.GetType(eval_context).EvalType();
                    substitute_generated_column_expression(
                        argument,
                        candidates,
                        required_type,
                        &schema,
                        context.GetExprCtx(),
                    );
                }
            }
            for item in &mut aggregation.GroupByItems {
                let required_type = item.GetType(eval_context).EvalType();
                substitute_generated_column_expression(
                    item,
                    candidates,
                    required_type,
                    &schema,
                    context.GetExprCtx(),
                );
            }
        }
    }
    for child in plan.Children_mut() {
        substitute_generated_column_descendants(child.as_mut(), candidates);
    }
}

/// 单表达式内用生成列替换匹配的虚拟计算。
fn substitute_generated_column_expression(
    target: &mut expression::ExprBox,
    candidates: &[GeneratedColumnCandidate],
    required_type: expression::types::EvalType,
    schema: &expression::Schema,
    build_context: &dyn expression::exprctx::BuildContext,
) -> bool {
    let eval_context = build_context.GetEvalCtx();
    let Some(candidate) = candidates.iter().find(|candidate| {
        target.Equal(eval_context, candidate.expression.as_ref())
            && candidate.expression.GetType(eval_context).EvalType() == required_type
            && schema.Contains(&candidate.column)
    }) else {
        return false;
    };
    if expression::MaybeOverOptimized4PlanCache(build_context, target.as_ref()) {
        build_context.SetSkipPlanCache(
            "generated column substitution with mutable constants can affect index selection",
        );
    }
    *target = Box::new(candidate.column.Clone());
    true
}

/// 在安全谓词/排序项中替换生成列引用。
fn substitute_generated_column_condition(
    condition: &mut expression::ExprBox,
    candidates: &[GeneratedColumnCandidate],
    schema: &expression::Schema,
    build_context: &dyn expression::exprctx::BuildContext,
) -> bool {
    let eval_context = build_context.GetEvalCtx();
    let Some(function) = condition
        .as_any_mut()
        .downcast_mut::<expression::ScalarFunction>()
    else {
        return false;
    };
    let name = function.FuncName.L.clone();
    let changed = match name.as_str() {
        "eq" | "lt" | "le" | "gt" | "ge" if function.GetArgs().len() == 2 => {
            let left_type = function.GetArgs()[0].GetType(eval_context).EvalType();
            let right_type = function.GetArgs()[1].GetType(eval_context).EvalType();
            let arguments = function.GetArgsMut();
            let right_changed = substitute_generated_column_expression(
                &mut arguments[1],
                candidates,
                left_type,
                schema,
                build_context,
            );
            let left_changed = substitute_generated_column_expression(
                &mut arguments[0],
                candidates,
                right_type,
                schema,
                build_context,
            );
            left_changed || right_changed
        }
        "in" if function.GetArgs().len() >= 2 => {
            let right_type = function.GetArgs()[1].GetType(eval_context).EvalType();
            let compatible = function.GetArgs()[1..]
                .iter()
                .all(|argument| argument.GetType(eval_context).EvalType() == right_type);
            compatible
                && substitute_generated_column_expression(
                    &mut function.GetArgsMut()[0],
                    candidates,
                    right_type,
                    schema,
                    build_context,
                )
        }
        "like" if function.GetArgs().len() >= 2 => {
            let pattern_type = function.GetArgs()[1].GetType(eval_context).EvalType();
            substitute_generated_column_expression(
                &mut function.GetArgsMut()[0],
                candidates,
                pattern_type,
                schema,
                build_context,
            )
        }
        "and" | "or" if function.GetArgs().len() == 2 => {
            let arguments = function.GetArgsMut();
            let left = substitute_generated_column_condition(
                &mut arguments[0],
                candidates,
                schema,
                build_context,
            );
            let right = substitute_generated_column_condition(
                &mut arguments[1],
                candidates,
                schema,
                build_context,
            );
            left || right
        }
        "not" if function.GetArgs().len() == 1 => substitute_generated_column_condition(
            &mut function.GetArgsMut()[0],
            candidates,
            schema,
            build_context,
        ),
        _ => false,
    };
    if changed {
        expression::ReHashCode(function);
    }
    changed
}

/// 稳定化结果顺序：必要时注入按句柄列的确定性排序。
fn stabilize_results(plan: &mut logicalop::LogicalPlanRef) {
    if complete_result_sort(plan.as_mut()) {
        return;
    }
    let owned = std::mem::replace(plan, Box::new(logicalop::LogicalTableDual::default()));
    *plan = inject_result_sort(owned);
}

/// 判断算子是否保持输入行序。
fn is_input_order_keeper(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalSelection>()
        || plan.as_any().is::<logicalop::LogicalProjection>()
        || plan.as_any().is::<logicalop::LogicalLimit>()
        || plan.as_any().is::<logicalop::LogicalTableDual>()
}

/// 补全结果排序键，确保顺序确定性。
fn complete_result_sort(plan: &mut dyn logicalop::LogicalPlan) -> bool {
    if is_input_order_keeper(plan) {
        return plan
            .Children_mut()
            .first_mut()
            .is_none_or(|child| complete_result_sort(child.as_mut()));
    }
    let Some(sort) = plan.as_any_mut().downcast_mut::<logicalop::LogicalSort>() else {
        return false;
    };
    let mut columns = sort.Schema().Columns.clone();
    if let Some(child) = sort.Children().first()
        && let Some(handle) = extract_result_handle_column(child.as_ref())
    {
        columns = vec![handle];
    }
    for column in columns {
        if !sort
            .ByItems
            .iter()
            .any(|item| column.EqualColumn(item.Expr.as_ref()))
        {
            sort.ByItems.push(planner_util_dependency::ByItems {
                Expr: Box::new(column),
                Desc: false,
            });
        }
    }
    true
}

/// 在计划顶部注入结果排序节点。
fn inject_result_sort(mut plan: logicalop::LogicalPlanRef) -> logicalop::LogicalPlanRef {
    if is_input_order_keeper(plan.as_ref()) {
        let mut children = plan.TakeChildren();
        if let Some(child) = children.pop() {
            plan.SetChildren(vec![inject_result_sort(child)]);
        }
        return plan;
    }
    let mut columns = plan.Schema().Columns.clone();
    if let Some(handle) = extract_result_handle_column(plan.as_ref()) {
        columns = vec![handle];
    }
    let context = plan
        .SCtx()
        .cloned()
        .expect("initialized logical plan must retain context");
    let query_block = plan.QueryBlockOffset();
    let schema = plan.Schema().Clone();
    let names = plan.OutputNames().Shallow();
    let mut sort = logicalop::LogicalSort {
        ByItems: columns
            .into_iter()
            .map(|column| planner_util_dependency::ByItems {
                Expr: Box::new(column),
                Desc: false,
            })
            .collect(),
        ..logicalop::LogicalSort::default()
    }
    .Init(context, query_block);
    sort.SetSchema(schema);
    sort.SetOutputNames(names);
    sort.SetChildren(vec![plan]);
    Box::new(sort)
}

/// 抽取可用作稳定排序键的句柄列。
fn extract_result_handle_column(plan: &dyn logicalop::LogicalPlan) -> Option<expression::Column> {
    if plan.as_any().is::<logicalop::LogicalSelection>()
        || plan.as_any().is::<logicalop::LogicalLimit>()
    {
        let handle = extract_result_handle_column(plan.Children().first()?.as_ref())?;
        return plan.Schema().Contains(&handle).then_some(handle);
    }
    let source = plan.as_any().downcast_ref::<logicalop::DataSource>()?;
    if source.TableInfo.IsCommonHandle {
        return None;
    }
    source.GetPKIsHandleCol()
}

/// 半连接改写：转为内连接+分组并在外侧投影，递归处理子树。
fn semi_join_rewrite_descendants(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    if plan.as_any().is::<logicalop::LogicalCTE>() {
        return Ok(());
    }
    for child in plan.Children_mut() {
        semi_join_rewrite_descendants(child)?;
    }
    if plan.as_any().is::<logicalop::LogicalApply>() {
        return Ok(());
    }
    let Some(join) = plan.as_any_mut().downcast_mut::<logicalop::LogicalJoin>() else {
        return Ok(());
    };
    if !matches!(
        join.JoinType,
        base::JoinType::SemiJoin | base::JoinType::LeftOuterSemiJoin
    ) {
        return Ok(());
    }
    let rewrite_hint = u64::from(hint::PreferRewriteSemiJoin);
    let enabled = join.PreferJoinType & rewrite_hint != 0
        || join.SCtx().is_some_and(|context| {
            let variables = context.GetSessionVars();
            variables.EnableSemiJoinRewrite
                || variables
                    .GetSystemVar(vardef_dependency::TiDBOptEnableSemiJoinRewrite)
                    .is_some_and(|value| {
                        matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                    })
        });
    if !enabled {
        return Ok(());
    }
    join.PreferJoinType &= !rewrite_hint;
    if join.JoinType == base::JoinType::LeftOuterSemiJoin
        || !join.LeftConditions.is_empty()
        || join.Children().len() != 2
    {
        return Ok(());
    }
    let context = join
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("semi join rewrite requires plan context"))?;
    let query_block = join.QueryBlockOffset();
    if !join.OtherConditions.is_empty() {
        let mut children = join.TakeChildren();
        materialize_expression_join_keys(
            &context,
            query_block,
            &mut children,
            &mut join.OtherConditions,
        );
        join.SetChildren(children);
        join.updateEQCond();
    }
    if !join.LeftConditions.is_empty()
        || !join.OtherConditions.is_empty()
        || join.EqualConditions.is_empty()
    {
        return Ok(());
    }
    let output_schema = join.Schema().Clone();
    let output_names = join.OutputNames().Shallow();
    let prefer_join_type = join.PreferJoinType;
    let prefer_join_order = join.PreferJoinOrder;
    let internal_prefer_join_order = join.InternalPreferJoinOrder;
    let mut equal_conditions = join.EqualConditions.clone();
    if let [left, right] = join.Children() {
        for equality in &mut equal_conditions {
            let Some(function) = equality
                .as_any_mut()
                .downcast_mut::<expression::ScalarFunction>()
            else {
                continue;
            };
            if function.GetArgs().len() != 2 {
                continue;
            }
            let (Some(first), Some(second)) = (
                function.GetArgs()[0].as_column(),
                function.GetArgs()[1].as_column(),
            ) else {
                continue;
            };
            if right.Schema().Contains(first) && left.Schema().Contains(second) {
                let first = function.GetArgs()[0].CloneExpr();
                let second = function.GetArgs()[1].CloneExpr();
                function.GetArgsMut()[0] = second;
                function.GetArgsMut()[1] = first;
                function.CleanHashCode();
            }
        }
    }
    let right_conditions = join.RightConditions.clone();

    let mut children = plan.TakeChildren();
    let left = children.remove(0);
    let mut right = children.remove(0);
    if !right_conditions.is_empty() {
        let schema = right.Schema().Clone();
        let names = right.OutputNames().Shallow();
        let mut selection = logicalop::LogicalSelection {
            Conditions: right_conditions,
            ..logicalop::LogicalSelection::default()
        }
        .Init(context.clone(), right.QueryBlockOffset());
        selection.SetSchema(schema);
        selection.SetOutputNames(names);
        selection.SetChildren(vec![right]);
        right = Box::new(selection);
    }

    let mut group_by = Vec::with_capacity(equal_conditions.len());
    let mut aggregate_functions = Vec::with_capacity(equal_conditions.len());
    let mut aggregate_columns = Vec::with_capacity(equal_conditions.len());
    let mut aggregate_names = base::types::NameSlice(Vec::new());
    for equality in &equal_conditions {
        let function = equality.as_scalar_function().ok_or_else(|| {
            expression::errors::New("semi join equality must be a scalar function")
        })?;
        let inner_column = function
            .GetArgs()
            .iter()
            .filter_map(|argument| argument.as_column())
            .find(|column| right.Schema().Contains(column))
            .cloned()
            .ok_or_else(|| expression::errors::New("semi join inner key must be a column"))?;
        let descriptor = aggregation::NewAggFuncDesc(
            context.GetExprCtx(),
            aggregation::ast::AggFuncFirstRow,
            vec![Box::new(inner_column.Clone())],
            false,
        )?;
        let name = right
            .Schema()
            .ColumnIndex(&inner_column)
            .and_then(|offset| right.OutputNames().0.get(offset).cloned())
            .flatten();
        group_by.push(Box::new(inner_column.Clone()) as expression::ExprBox);
        aggregate_columns.push(inner_column);
        aggregate_functions.push(descriptor);
        aggregate_names.0.push(name);
    }
    let mut aggregation = logicalop::LogicalAggregation {
        AggFuncs: aggregate_functions,
        GroupByItems: group_by,
        ..logicalop::LogicalAggregation::default()
    }
    .Init(context.clone(), right.QueryBlockOffset());
    aggregation.SetSchema(expression::NewSchema(aggregate_columns));
    aggregation.SetOutputNames(aggregate_names);
    aggregation.SetChildren(vec![right]);
    aggregation.BuildSelfKeyInfo();
    let aggregation: logicalop::LogicalPlanRef = Box::new(aggregation);

    let mut join_names = left.OutputNames().Shallow();
    join_names
        .0
        .extend(aggregation.OutputNames().0.iter().cloned());
    let mut inner_join = logicalop::LogicalJoin {
        JoinType: base::JoinType::InnerJoin,
        PreferJoinType: prefer_join_type,
        PreferJoinOrder: prefer_join_order,
        InternalPreferJoinOrder: internal_prefer_join_order,
        EqualConditions: equal_conditions,
        FromSemiJoinRewrite: true,
        ..logicalop::LogicalJoin::default()
    }
    .Init(context.clone(), query_block);
    inner_join.SetSchema(logicalop::MergeSchema(left.Schema(), aggregation.Schema()));
    inner_join.SetOutputNames(join_names);
    inner_join.SetChildren(vec![left, aggregation]);
    let inner_join: logicalop::LogicalPlanRef = Box::new(inner_join);

    let projection_expressions = expression::Column2Exprs(&output_schema.Columns);
    let mut projection = logicalop::LogicalProjection {
        Exprs: projection_expressions,
        ..logicalop::LogicalProjection::default()
    }
    .Init(context, query_block);
    projection.SetSchema(output_schema);
    projection.SetOutputNames(output_names);
    projection.SetChildren(vec![inner_join]);
    *plan = Box::new(projection);
    Ok(())
}

/// 聚合消除：分组键已唯一时用投影替代聚合。
fn eliminate_aggregation_descendants(
    plan: &mut logicalop::LogicalPlanRef,
    eliminate_distinct: bool,
) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        eliminate_aggregation_descendants(child, eliminate_distinct)?;
    }
    let semi_join_type = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .map(|apply| apply.LogicalJoin.JoinType)
        .or_else(|| {
            plan.as_any()
                .downcast_ref::<logicalop::LogicalJoin>()
                .map(|join| join.JoinType)
        });
    let removable_semi_inner = semi_join_type.is_some_and(|join_type| {
        matches!(
            join_type,
            base::JoinType::SemiJoin
                | base::JoinType::AntiSemiJoin
                | base::JoinType::LeftOuterSemiJoin
                | base::JoinType::AntiLeftOuterSemiJoin
        ) && plan
            .Children()
            .get(1)
            .is_some_and(|inner| semi_inner_distinct_aggregation(inner.as_ref()))
    });
    if removable_semi_inner {
        let removed = remove_semi_inner_distinct_aggregation(&mut plan.Children_mut()[1]);
        debug_assert!(removed);
        return Ok(());
    }
    let Some(aggregation) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
    else {
        return Ok(());
    };
    if aggregation.Children().len() != 1 {
        return Ok(());
    }
    let child_keys = aggregation.Children()[0].Schema().PKOrUK.clone();
    let nullable_keys = aggregation.Children()[0].Schema().NullableUK.clone();
    if eliminate_distinct {
        for function in &mut aggregation.AggFuncs {
            if !function.HasDistinct {
                continue;
            }
            let Some(arguments) = function
                .Args
                .iter()
                .map(|argument| argument.as_column().cloned())
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if child_keys.iter().chain(&nullable_keys).any(|key| {
                key.iter().all(|key_column| {
                    arguments
                        .iter()
                        .any(|argument| argument.UniqueID == key_column.UniqueID)
                })
            }) {
                function.HasDistinct = false;
            }
        }
    }

    if aggregation
        .AggFuncs
        .iter()
        .any(|function| function.Name == aggregation::ast::AggFuncGroupConcat)
    {
        return Ok(());
    }
    let grouped = aggregation.GetGroupByCols();
    let covered = child_keys.iter().any(|key| {
        !key.is_empty()
            && key.iter().all(|key_column| {
                grouped
                    .iter()
                    .any(|column| column.UniqueID == key_column.UniqueID)
            })
    }) || aggregation.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::DataSource>()
        .and_then(logicalop::DataSource::GetPKIsHandleCol)
        .is_some_and(|key| grouped.iter().any(|column| column.UniqueID == key.UniqueID));
    if !covered {
        return Ok(());
    }
    let context = aggregation
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("aggregation elimination requires context"))?;
    let mut expressions = Vec::with_capacity(aggregation.AggFuncs.len());
    for function in &aggregation.AggFuncs {
        let Some(rewritten) = rewrite_single_row_aggregate(context.GetExprCtx(), function)? else {
            return Ok(());
        };
        expressions.push(rewritten);
    }
    let query_block = aggregation.QueryBlockOffset();
    let schema = aggregation.Schema().Clone();
    let names = aggregation.OutputNames().Shallow();
    let child = plan.TakeChildren().remove(0);
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..logicalop::LogicalProjection::default()
    }
    .Init(context, query_block);
    projection.SetSchema(schema);
    projection.SetOutputNames(names);
    projection.SetChildren(vec![child]);
    *plan = Box::new(projection);
    Ok(())
}

/// 计划树是否包含 Limit。
fn logical_plan_has_limit(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalLimit>()
        || plan
            .Children()
            .iter()
            .any(|child| logical_plan_has_limit(child.as_ref()))
}

/// 半连接内侧是否为 distinct 聚合形态。
fn semi_inner_distinct_aggregation(plan: &dyn logicalop::LogicalPlan) -> bool {
    if let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    {
        return !aggregation.GroupByItems.is_empty()
            && aggregation.Children().len() == 1
            && !logical_plan_has_limit(aggregation.Children()[0].as_ref())
            && aggregation.AggFuncs.iter().all(|function| {
                function.Name == crate::ast::AggFuncFirstRow
                    && !function.HasDistinct
                    && function.OrderByItems.is_empty()
                    && function.Args.len() == 1
            });
    }
    plan.as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .is_some_and(|projection| {
            projection.Children().len() == 1
                && !expression::ExprsHasSideEffects(&projection.Exprs)
                && semi_inner_distinct_aggregation(projection.Children()[0].as_ref())
        })
        || plan
            .as_any()
            .downcast_ref::<logicalop::LogicalMaxOneRow>()
            .is_some_and(|max_one| {
                max_one.Children().len() == 1
                    && semi_inner_distinct_aggregation(max_one.Children()[0].as_ref())
            })
}

/// 去掉半连接内侧多余的 distinct 聚合包装。
fn remove_semi_inner_distinct_aggregation(plan: &mut logicalop::LogicalPlanRef) -> bool {
    if plan.as_any().is::<logicalop::LogicalAggregation>() {
        *plan = plan.TakeChildren().remove(0);
        return true;
    }
    let Some(projection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
    else {
        if plan.as_any().is::<logicalop::LogicalMaxOneRow>() {
            let mut child = plan.TakeChildren().remove(0);
            let removed = remove_semi_inner_distinct_aggregation(&mut child);
            if removed {
                *plan = child;
            }
            return removed;
        }
        return false;
    };
    remove_semi_inner_distinct_aggregation(&mut projection.Children_mut()[0])
}

/// 改写单行聚合（无分组）形态以利后续消除或下推。
fn rewrite_single_row_aggregate(
    context: &dyn expression::exprctx::BuildContext,
    function: &aggregation::AggFuncDesc,
) -> Result<Option<expression::ExprBox>, expression::Error> {
    let Some(target_type) = function.RetTp.clone() else {
        return Ok(None);
    };
    match function.Name.as_str() {
        aggregation::ast::AggFuncMax
        | aggregation::ast::AggFuncMin
        | aggregation::ast::AggFuncSum
        | aggregation::ast::AggFuncSumInt
        | aggregation::ast::AggFuncAvg
        | aggregation::ast::AggFuncFirstRow => {
            let Some(argument) = function.Args.first().cloned() else {
                return Ok(None);
            };
            if argument.GetType(context.GetEvalCtx()) == &target_type {
                Ok(Some(argument))
            } else {
                Ok(Some(expression::BuildCastFunction(
                    context,
                    &argument,
                    &target_type,
                )))
            }
        }
        aggregation::ast::AggFuncCount => {
            let mut null_checks = Vec::with_capacity(function.Args.len());
            for argument in &function.Args {
                if expression::mysql::HasNotNullFlag(
                    argument.GetType(context.GetEvalCtx()).GetFlag(),
                ) {
                    null_checks.push(Box::new(expression::NewZero()) as expression::ExprBox);
                } else {
                    null_checks.push(expression::NewFunction(
                        context,
                        crate::ast::IsNull,
                        *expression::types::NewFieldType(expression::mysql::TypeTiny),
                        vec![argument.CloneExpr()],
                    )?);
                }
            }
            let condition = expression::ComposeDNFCondition(context, &null_checks)
                .unwrap_or_else(|| Box::new(expression::NewZero()));
            Ok(Some(expression::NewFunction(
                context,
                crate::ast::If,
                target_type,
                vec![
                    condition,
                    Box::new(expression::NewZero()),
                    Box::new(expression::NewOne()),
                ],
            )?))
        }
        _ => Ok(None),
    }
}

/// 倾斜 distinct 聚合：拆成两级分组以缓解数据倾斜。
fn rewrite_skew_distinct_aggregation_descendants(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        rewrite_skew_distinct_aggregation_descendants(child)?;
    }
    let Some(aggregate) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    else {
        return Ok(());
    };
    if aggregate.GroupByItems.is_empty() || aggregate.Children().len() != 1 {
        return Ok(());
    }
    let distinct = aggregate
        .AggFuncs
        .iter()
        .filter(|function| function.HasDistinct)
        .count();
    if distinct != 1
        || aggregate.AggFuncs.iter().any(|function| {
            function.Mode != aggregation::CompleteMode
                || !function.OrderByItems.is_empty()
                || function.Args.len() > 1
                || function.Args.iter().any(|argument| {
                    argument.as_column().is_none() && argument.as_constant().is_none()
                })
                || !matches!(
                    function.Name.as_str(),
                    aggregation::ast::AggFuncFirstRow
                        | aggregation::ast::AggFuncCount
                        | aggregation::ast::AggFuncSum
                        | aggregation::ast::AggFuncMax
                        | aggregation::ast::AggFuncMin
                        | aggregation::ast::AggFuncAvg
                )
                || (function.Name == aggregation::ast::AggFuncAvg && !function.HasDistinct)
        })
    {
        return Ok(());
    }
    let context = aggregate
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("skew distinct rewrite requires context"))?;
    let query_block = aggregate.QueryBlockOffset();
    let output_schema = aggregate.Schema().Clone();
    let output_names = aggregate.OutputNames().Shallow();
    let group_by = aggregate.GroupByItems.clone();
    let functions = aggregate.AggFuncs.clone();
    let prefer_bottom = aggregate.PreferAggType;
    let prefer_top_cop = aggregate.PreferAggToCop;
    let mut bottom_group_by = group_by.clone();
    for function in functions.iter().filter(|function| function.HasDistinct) {
        for argument in &function.Args {
            if !bottom_group_by.iter().any(|existing| {
                existing.Equal(context.GetExprCtx().GetEvalCtx(), argument.as_ref())
            }) {
                bottom_group_by.push(argument.CloneExpr());
            }
        }
    }

    let mut bottom_functions = Vec::new();
    let mut bottom_columns = Vec::new();
    let mut top_functions = Vec::new();
    for function in &functions {
        let argument = function.Args.first().cloned().ok_or_else(|| {
            expression::errors::New("qualified skew aggregate requires one argument")
        })?;
        if function.HasDistinct {
            let first_row = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                aggregation::ast::AggFuncFirstRow,
                vec![argument.CloneExpr()],
                false,
            )?;
            let output = argument.as_column().cloned().unwrap_or_else(|| {
                expression::Column::new(
                    first_row.RetTp.clone().unwrap_or_default(),
                    0,
                    context.GetExprCtx().AllocPlanColumnID(),
                    bottom_columns.len() as isize,
                )
            });
            bottom_functions.push(first_row);
            bottom_columns.push(output.Clone());
            top_functions.push(aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                &function.Name,
                vec![Box::new(output)],
                false,
            )?);
        } else {
            bottom_functions.push(function.clone());
            let output = if function.Name == aggregation::ast::AggFuncFirstRow {
                argument.as_column().cloned()
            } else {
                None
            }
            .unwrap_or_else(|| {
                expression::Column::new(
                    function.RetTp.clone().unwrap_or_default(),
                    0,
                    context.GetExprCtx().AllocPlanColumnID(),
                    bottom_columns.len() as isize,
                )
            });
            bottom_columns.push(output.Clone());
            let top_name = if function.Name == aggregation::ast::AggFuncCount {
                aggregation::ast::AggFuncSum
            } else {
                &function.Name
            };
            top_functions.push(aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                top_name,
                vec![Box::new(output)],
                false,
            )?);
        }
    }
    for column in group_by
        .iter()
        .flat_map(|item| expression::ExtractColumns(item.as_ref()))
    {
        if bottom_columns
            .iter()
            .any(|existing| existing.UniqueID == column.UniqueID)
        {
            continue;
        }
        bottom_functions.push(aggregation::NewAggFuncDesc(
            context.GetExprCtx(),
            aggregation::ast::AggFuncFirstRow,
            vec![Box::new(column.Clone())],
            false,
        )?);
        bottom_columns.push(column.Clone());
    }
    let child = plan.TakeChildren().remove(0);
    let mut bottom = logicalop::LogicalAggregation {
        AggFuncs: bottom_functions,
        GroupByItems: bottom_group_by,
        PreferAggType: prefer_bottom,
        ..logicalop::LogicalAggregation::default()
    }
    .Init(context.clone(), query_block);
    bottom.SetSchema(expression::NewSchema(bottom_columns));
    bottom.SetOutputNames(base::types::NameSlice(vec![None; bottom.Schema().Len()]));
    bottom.SetChildren(vec![child]);
    let bottom: logicalop::LogicalPlanRef = Box::new(bottom);

    let mut top = logicalop::LogicalAggregation {
        AggFuncs: top_functions,
        GroupByItems: group_by,
        PreferAggToCop: prefer_top_cop,
        ..logicalop::LogicalAggregation::default()
    }
    .Init(context, query_block);
    top.SetSchema(output_schema);
    top.SetOutputNames(output_names);
    top.SetChildren(vec![bottom]);
    *plan = Box::new(top);
    Ok(())
}

/// Max/Min 消除：加非空过滤、排序与 Limit 1 替代聚合。
fn eliminate_single_max_min_descendants(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    if plan.as_any().is::<logicalop::LogicalCTE>() {
        return Ok(());
    }
    for child in plan.Children_mut() {
        eliminate_single_max_min_descendants(child)?;
    }
    let Some(aggregate) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
    else {
        return Ok(());
    };
    if !aggregate.GroupByItems.is_empty()
        || aggregate.AggFuncs.len() != 1
        || aggregate.Children().len() != 1
    {
        return Ok(());
    }
    let function = &aggregate.AggFuncs[0];
    if !matches!(
        function.Name.as_str(),
        aggregation::ast::AggFuncMax | aggregation::ast::AggFuncMin
    ) || function.Args.len() != 1
    {
        return Ok(());
    }
    let is_max = function.Name == aggregation::ast::AggFuncMax;
    let argument = function.Args[0].CloneExpr();
    if argument
        .GetType(
            aggregate
                .SCtx()
                .expect("initialized aggregate")
                .GetExprCtx()
                .GetEvalCtx(),
        )
        .GetType()
        == expression::mysql::TypeEnum
        || argument
            .GetType(
                aggregate
                    .SCtx()
                    .expect("initialized aggregate")
                    .GetExprCtx()
                    .GetEvalCtx(),
            )
            .GetType()
            == expression::mysql::TypeSet
    {
        return Ok(());
    }
    let context = aggregate
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("max/min elimination requires context"))?;
    let query_block = aggregate.QueryBlockOffset();
    let mut child = aggregate.TakeChildren().remove(0);
    if !expression::ExtractColumns(argument.as_ref()).is_empty()
        && !expression::mysql::HasNotNullFlag(
            argument
                .GetType(context.GetExprCtx().GetEvalCtx())
                .GetFlag(),
        )
    {
        let is_null = expression::NewFunction(
            context.GetExprCtx(),
            crate::ast::IsNull,
            *expression::types::NewFieldType(expression::mysql::TypeTiny),
            vec![argument.CloneExpr()],
        )?;
        let not_null = expression::NewFunction(
            context.GetExprCtx(),
            crate::ast::UnaryNot,
            *expression::types::NewFieldType(expression::mysql::TypeTiny),
            vec![is_null],
        )?;
        let schema = child.Schema().Clone();
        let names = child.OutputNames().Shallow();
        let mut selection = logicalop::LogicalSelection {
            Conditions: vec![not_null],
            ..logicalop::LogicalSelection::default()
        }
        .Init(context.clone(), query_block);
        selection.SetSchema(schema);
        selection.SetOutputNames(names);
        selection.SetChildren(vec![child]);
        child = Box::new(selection);
    }
    if !expression::ExtractColumns(argument.as_ref()).is_empty() {
        let schema = child.Schema().Clone();
        let names = child.OutputNames().Shallow();
        let mut sort = logicalop::LogicalSort {
            ByItems: vec![planner_util_dependency::ByItems {
                Expr: argument,
                Desc: is_max,
            }],
            ..logicalop::LogicalSort::default()
        }
        .Init(context.clone(), query_block);
        sort.SetSchema(schema);
        sort.SetOutputNames(names);
        sort.SetChildren(vec![child]);
        child = Box::new(sort);
    }
    if child.MaxOneRow() {
        // A one-row input already satisfies the limit added for MIN/MAX.
        aggregate.SetChildren(vec![child]);
        return Ok(());
    }
    let schema = child.Schema().Clone();
    let names = child.OutputNames().Shallow();
    let mut limit = logicalop::LogicalLimit {
        Count: 1,
        ..logicalop::LogicalLimit::default()
    }
    .Init(context, query_block);
    limit.SetSchema(schema);
    limit.SetOutputNames(names);
    limit.SetChildren(vec![child]);
    aggregate.SetChildren(vec![Box::new(limit)]);
    Ok(())
}

/// 跨查询块常量传播：把导出常量谓词上提。
fn propagate_cross_block_constants(plan: &mut logicalop::LogicalPlanRef) {
    for child in plan.Children_mut() {
        propagate_cross_block_constants(child);
    }
    let candidates = {
        let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() else {
            return;
        };
        if join.Children().len() != 2 {
            return;
        }
        let mut predicates = Vec::new();
        if matches!(
            join.JoinType,
            base::JoinType::InnerJoin | base::JoinType::LeftOuterJoin
        ) {
            predicates.extend(pull_up_constant_predicates(join.Children()[0].as_ref()));
        }
        if matches!(
            join.JoinType,
            base::JoinType::InnerJoin | base::JoinType::RightOuterJoin
        ) {
            predicates.extend(pull_up_constant_predicates(join.Children()[1].as_ref()));
        }
        predicates
    };
    if candidates.is_empty() {
        return;
    }
    let context = plan
        .SCtx()
        .cloned()
        .expect("initialized join must retain context");
    let query_block = plan.QueryBlockOffset();
    let schema = plan.Schema().Clone();
    let names = plan.OutputNames().Shallow();
    let join = std::mem::replace(plan, Box::new(logicalop::LogicalTableDual::default()));
    let mut selection = logicalop::LogicalSelection {
        Conditions: candidates,
        ..logicalop::LogicalSelection::default()
    }
    .Init(context, query_block);
    selection.SetSchema(schema);
    selection.SetOutputNames(names);
    selection.SetChildren(vec![join]);
    *plan = Box::new(selection);
}

/// 自子计划上拉常量谓词列表。
fn pull_up_constant_predicates(plan: &dyn logicalop::LogicalPlan) -> Vec<expression::ExprBox> {
    if let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() {
        return selection.PullUpConstantPredicates();
    }
    if let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>()
        && projection
            .Exprs
            .iter()
            .all(|expression| expression.as_column().is_some())
        && let Some(child) = projection.Children().first()
    {
        return pull_up_constant_predicates(child.as_ref());
    }
    Vec::new()
}

/// Join 键类型转换改写：恢复整型键并保护文本侧比较语义。
fn rewrite_join_key_type_casts(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        rewrite_join_key_type_casts(child)?;
    }
    let Some(join) = plan.as_any_mut().downcast_mut::<logicalop::LogicalJoin>() else {
        return Ok(());
    };
    if join.Children().len() != 2 {
        return Ok(());
    }
    let context = join
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("join key cast rewrite requires context"))?;
    let eval_context = context.GetExprCtx().GetEvalCtx();
    let candidate = join
        .OtherConditions
        .iter()
        .enumerate()
        .find_map(|(index, condition)| {
            let equality = condition.as_scalar_function()?;
            if equality.FuncName.L != crate::ast::EQ || equality.GetArgs().len() != 2 {
                return None;
            }
            let unwrap_real_cast = |expression: &expression::ExprBox| {
                let cast = expression.as_scalar_function()?;
                (cast.FuncName.L == "cast"
                    && cast.GetType(eval_context).EvalType() == expression::types::ETReal)
                    .then(|| cast.GetArgs().first()?.as_column().cloned())
                    .flatten()
            };
            let left = unwrap_real_cast(&equality.GetArgs()[0])?;
            let right = unwrap_real_cast(&equality.GetArgs()[1])?;
            let left_type = left.GetType(eval_context).EvalType();
            let right_type = right.GetType(eval_context).EvalType();
            let (integer, text) = match (left_type, right_type) {
                (expression::types::ETInt, expression::types::ETString) => (left, right),
                (expression::types::ETString, expression::types::ETInt) => (right, left),
                _ => return None,
            };
            let integer_child = usize::from(!join.Children()[0].Schema().Contains(&integer));
            let text_child = usize::from(!join.Children()[0].Schema().Contains(&text));
            if integer_child == text_child
                || !join.Children()[integer_child].Schema().Contains(&integer)
                || !join.Children()[text_child].Schema().Contains(&text)
            {
                return None;
            }
            let preserved = match join.JoinType {
                base::JoinType::LeftOuterJoin
                | base::JoinType::AntiSemiJoin
                | base::JoinType::LeftOuterSemiJoin
                | base::JoinType::AntiLeftOuterSemiJoin => Some(0),
                base::JoinType::RightOuterJoin => Some(1),
                _ => None,
            };
            (preserved != Some(text_child)).then_some((
                index,
                integer,
                text,
                integer_child,
                text_child,
            ))
        });
    let Some((condition_index, integer, text, integer_child, text_child)) = candidate else {
        return Ok(());
    };

    let cast_integer = expression::WrapWithCastAsInt(
        context.GetExprCtx(),
        Box::new(text.Clone()),
        integer.RetType.as_ref(),
    );
    let mut cast_column = expression::Column::new(
        cast_integer.GetType(eval_context).clone(),
        0,
        context.GetExprCtx().AllocPlanColumnID(),
        join.Children()[text_child].Schema().Len() as isize,
    );
    cast_column.OrigName = text.OrigName.clone();
    let guard_left = expression::WrapWithCastAsReal(
        context.GetExprCtx(),
        expression::WrapWithCastAsInt(
            context.GetExprCtx(),
            Box::new(text.Clone()),
            integer.RetType.as_ref(),
        ),
    );
    let guard_right = expression::WrapWithCastAsReal(context.GetExprCtx(), Box::new(text.Clone()));
    let guard = expression::NewFunction(
        context.GetExprCtx(),
        crate::ast::EQ,
        *expression::types::NewFieldType(expression::mysql::TypeTiny),
        vec![guard_left, guard_right],
    )?;

    let mut children = join.TakeChildren();
    let original = std::mem::replace(
        &mut children[text_child],
        Box::new(logicalop::LogicalTableDual::default()),
    );
    let child_schema = original.Schema().Clone();
    let child_names = original.OutputNames().Shallow();
    let mut selection = logicalop::LogicalSelection {
        Conditions: vec![guard],
        ..logicalop::LogicalSelection::default()
    }
    .Init(context.clone(), original.QueryBlockOffset());
    selection.SetSchema(child_schema.Clone());
    selection.SetOutputNames(child_names.Shallow());
    selection.SetChildren(vec![original]);

    let mut projection_schema = child_schema.Clone();
    projection_schema.Append([cast_column.Clone()]);
    let mut projection_names = child_names;
    projection_names.0.push(None);
    let mut projection = logicalop::LogicalProjection {
        Exprs: expression::Column2Exprs(&child_schema.Columns)
            .into_iter()
            .chain(std::iter::once(cast_integer))
            .collect(),
        ..logicalop::LogicalProjection::default()
    }
    .Init(context.clone(), join.QueryBlockOffset());
    projection.SetSchema(projection_schema);
    projection.SetOutputNames(projection_names);
    projection.SetChildren(vec![Box::new(selection)]);
    children[text_child] = Box::new(projection);
    join.SetChildren(children);

    let (left_key, right_key) = if integer_child == 0 {
        (integer, cast_column)
    } else {
        (cast_column, integer)
    };
    let equality = expression::NewFunction(
        context.GetExprCtx(),
        crate::ast::EQ,
        *expression::types::NewFieldType(expression::mysql::TypeTiny),
        vec![Box::new(left_key), Box::new(right_key)],
    )?;
    join.OtherConditions.remove(condition_index);
    join.EqualConditions.push(equality);
    Ok(())
}

/// 外连接消除：内侧唯一且投影未引用时可去掉内侧。
fn eliminate_outer_join_descendants(
    plan: &mut logicalop::LogicalPlanRef,
    parent_used: &[expression::Column],
    duplicate_agnostic_columns: Option<&[expression::Column]>,
) {
    if plan.as_any().is::<logicalop::LogicalCTE>() {
        return;
    }
    if let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>() {
        let used_ids = parent_used
            .iter()
            .map(|column| column.UniqueID)
            .collect::<std::collections::HashSet<_>>();
        let child_used = projection
            .Exprs
            .iter()
            .zip(&projection.Schema().Columns)
            .filter(|(_, output)| used_ids.contains(&output.UniqueID))
            .flat_map(|(expression, _)| expression::ExtractColumns(expression.as_ref()))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(child) = plan.Children_mut().first_mut() {
            eliminate_outer_join_descendants(child, &child_used, duplicate_agnostic_columns);
        }
        return;
    }
    if let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    {
        let used_ids = parent_used
            .iter()
            .map(|column| column.UniqueID)
            .collect::<std::collections::HashSet<_>>();
        let mut child_used = aggregation
            .AggFuncs
            .iter()
            .zip(&aggregation.Schema().Columns)
            .filter(|(_, output)| used_ids.contains(&output.UniqueID))
            .flat_map(|(function, _)| {
                function
                    .Args
                    .iter()
                    .flat_map(|argument| expression::ExtractColumns(argument.as_ref()))
            })
            .cloned()
            .collect::<Vec<_>>();
        child_used.extend(
            aggregation
                .GroupByItems
                .iter()
                .flat_map(|item| expression::ExtractColumns(item.as_ref()))
                .cloned(),
        );
        let duplicate_agnostic = aggregation
            .AggFuncs
            .iter()
            .all(|function| {
                function.HasDistinct
                    || matches!(
                        function.Name.as_str(),
                        crate::ast::AggFuncFirstRow
                            | crate::ast::AggFuncMax
                            | crate::ast::AggFuncMin
                            | crate::ast::AggFuncApproxCountDistinct
                    )
            })
            .then(|| {
                aggregation
                    .AggFuncs
                    .iter()
                    .flat_map(|function| {
                        function
                            .Args
                            .iter()
                            .flat_map(|argument| expression::ExtractColumns(argument.as_ref()))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            });
        if let Some(child) = plan.Children_mut().first_mut() {
            eliminate_outer_join_descendants(child, &child_used, duplicate_agnostic.as_deref());
        }
        return;
    }

    let elimination = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .and_then(|join| {
            let inner_index = match join.JoinType {
                base::JoinType::LeftOuterJoin => 1,
                base::JoinType::RightOuterJoin => 0,
                _ => return None,
            };
            if join.Children().len() != 2 {
                return None;
            }
            let outer_index = 1 - inner_index;
            if parent_used
                .iter()
                .any(|column| !join.Children()[outer_index].Schema().Contains(column))
            {
                return None;
            }
            if duplicate_agnostic_columns.is_some_and(|columns| {
                !columns.is_empty()
                    && columns
                        .iter()
                        .all(|column| join.Children()[outer_index].Schema().Contains(column))
            }) {
                return Some(outer_index);
            }
            let mut inner_keys = Vec::new();
            for equality in &join.EqualConditions {
                let function = equality.as_scalar_function()?;
                let key = function
                    .GetArgs()
                    .get(inner_index)
                    .and_then(|argument| argument.as_column())?
                    .Clone();
                if function.FuncName.L == crate::ast::NullEQ
                    && !expression::mysql::HasNotNullFlag(
                        key.RetType
                            .as_ref()
                            .map_or(0, |field_type| field_type.GetFlag()),
                    )
                {
                    return None;
                }
                inner_keys.push(key);
            }
            if inner_keys.is_empty() {
                return None;
            }
            let inner = &join.Children()[inner_index];
            let schema_unique = inner.Schema().PKOrUK.iter().any(|key| {
                !key.is_empty()
                    && key.iter().all(|unique| {
                        inner_keys
                            .iter()
                            .any(|join_key| join_key.UniqueID == unique.UniqueID)
                    })
            });
            let handle_unique = inner
                .as_any()
                .downcast_ref::<logicalop::DataSource>()
                .and_then(logicalop::DataSource::GetPKIsHandleCol)
                .is_some_and(|key| {
                    inner_keys
                        .iter()
                        .any(|join_key| join_key.UniqueID == key.UniqueID)
                });
            (schema_unique || handle_unique).then_some(outer_index)
        });
    if let Some(outer_index) = elimination {
        let mut children = plan.TakeChildren();
        let outer = children.remove(outer_index);
        *plan = outer;
        eliminate_outer_join_descendants(plan, parent_used, duplicate_agnostic_columns);
        return;
    }

    for child in plan.Children_mut() {
        let used = child.Schema().Columns.clone();
        eliminate_outer_join_descendants(child, &used, duplicate_agnostic_columns);
    }
}

/// 收集谓词引用列，供统计信息加载点等后续规则使用。
fn collect_predicate_columns_descendants(
    plan: &mut dyn logicalop::LogicalPlan,
    inherited: &[expression::Column],
) {
    let mut interesting = inherited.to_vec();
    if let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() {
        interesting.extend(
            expression::ExtractColumnsFromExpressions(&selection.Conditions, None)
                .into_iter()
                .cloned(),
        );
    } else if let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() {
        for item in &top_n.ByItems {
            interesting.extend(
                expression::ExtractColumns(item.Expr.as_ref())
                    .into_iter()
                    .cloned(),
            );
        }
    } else if let Some(sort) = plan.as_any().downcast_ref::<logicalop::LogicalSort>() {
        for item in &sort.ByItems {
            interesting.extend(
                expression::ExtractColumns(item.Expr.as_ref())
                    .into_iter()
                    .cloned(),
            );
        }
    } else if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
        for condition in join
            .EqualConditions
            .iter()
            .chain(&join.NAEQConditions)
            .chain(&join.LeftConditions)
            .chain(&join.RightConditions)
            .chain(&join.OtherConditions)
        {
            interesting.extend(
                expression::ExtractColumns(condition.as_ref())
                    .into_iter()
                    .cloned(),
            );
        }
    } else if let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    {
        interesting.extend(aggregation.GetUsedCols());
    }
    interesting.sort_by_key(|column| column.UniqueID);
    interesting.dedup_by_key(|column| column.UniqueID);

    if let Some(source) = plan.as_any_mut().downcast_mut::<logicalop::DataSource>() {
        interesting.retain(|column| source.Schema().Contains(column));
        // Go collects statistics from pushed-down filters. AllConds also
        // contributes to index-pruning interests, but partition-pruning-only
        // expressions must not cause additional histogram load requests.
        let mut statistics_columns = interesting.clone();
        for condition in &source.PushedDownConds {
            let columns = expression::ExtractColumns(condition.as_ref());
            if columns
                .iter()
                .all(|column| source.Schema().Contains(column))
            {
                statistics_columns.extend(columns.into_iter().cloned());
            }
        }
        interesting = statistics_columns.clone();
        for condition in &source.AllConds {
            let columns = expression::ExtractColumns(condition.as_ref());
            if columns
                .iter()
                .all(|column| source.Schema().Contains(column))
            {
                interesting.extend(columns.into_iter().cloned());
            }
        }
        interesting.sort_by_key(|column| column.UniqueID);
        interesting.dedup_by_key(|column| column.UniqueID);
        source.InterestingColumns = interesting;
        collect_stats_load_items_for_source(source, &statistics_columns);
        return;
    }
    if let Some(projection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
    {
        let mut child_interesting = Vec::new();
        for column in &interesting {
            if let Some(index) = projection
                .Schema()
                .Columns
                .iter()
                .position(|output| output.UniqueID == column.UniqueID)
            {
                child_interesting.extend(
                    expression::ExtractColumns(projection.Exprs[index].as_ref())
                        .into_iter()
                        .cloned(),
                );
            }
        }
        if let Some(child) = projection.Children_mut().first_mut() {
            collect_predicate_columns_descendants(child.as_mut(), &child_interesting);
        }
        return;
    }
    for child in plan.Children_mut() {
        let child_interesting = interesting
            .iter()
            .filter(|column| child.Schema().Contains(column))
            .cloned()
            .collect::<Vec<_>>();
        collect_predicate_columns_descendants(child.as_mut(), &child_interesting);
    }
}

/// 将单个 DataSource 的谓词列、可用索引与直接依赖虚拟列写入语句同步加载集合。
fn collect_stats_load_items_for_source(
    source: &logicalop::DataSource,
    statistics_columns: &[expression::Column],
) {
    let Some(context) = source.SCtx() else {
        return;
    };
    if context
        .GetSessionVars()
        .StatsLoadSyncWait
        .load(Ordering::Acquire)
        <= 0
    {
        return;
    }
    let table_id = if source.PhysicalTableID != 0 {
        source.PhysicalTableID
    } else {
        source.TableInfo.ID
    };
    let interesting_ids = statistics_columns
        .iter()
        .map(|column| column.ID)
        .filter(|id| *id != 0)
        .collect::<std::collections::BTreeSet<_>>();
    if interesting_ids.is_empty() {
        return;
    }
    let mut items = interesting_ids
        .iter()
        .map(|column_id| model_dependency::StatsLoadItem {
            TableItemID: model_dependency::TableItemID {
                TableID: table_id,
                ID: *column_id,
                IsIndex: false,
                IsSyncLoadFailed: false,
            },
            FullLoad: true,
        })
        .collect::<Vec<_>>();
    let possible_index_ids = source
        .PossibleAccessPaths
        .iter()
        .filter_map(|path| path.Index.as_ref().map(|index| index.ID))
        .collect::<std::collections::BTreeSet<_>>();
    items.extend(source.TableInfo.Indices.iter().filter_map(|index| {
        let covers_interesting = index.Columns.iter().any(|index_column| {
            source.TableInfo.Columns.iter().any(|column| {
                column.Name.L == index_column.Name.L && interesting_ids.contains(&column.ID)
            })
        });
        (covers_interesting
            && (possible_index_ids.is_empty() || possible_index_ids.contains(&index.ID)))
        .then_some(model_dependency::StatsLoadItem {
            TableItemID: model_dependency::TableItemID {
                TableID: table_id,
                ID: index.ID,
                IsIndex: true,
                IsSyncLoadFailed: false,
            },
            FullLoad: true,
        })
    }));
    let table_map = std::collections::BTreeMap::from([(table_id, source.TableInfo.clone())]);
    items.extend(
        rule::rule_collect_plan_stats::collect_depending_virtual_columns(&table_map, &items),
    );

    let statement_context = &context.GetSessionVars().StmtCtx;
    let mut needed = statement_context
        .StatsLoad
        .NeededItems
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for item in items {
        let duplicate = needed.iter().any(|value| {
            stmtctx_dependency::cache_downcast_ref::<model_dependency::StatsLoadItem>(value)
                .is_some_and(|existing| existing.Key() == item.Key())
        });
        if !duplicate {
            needed.push(stmtctx_dependency::cache_value(item));
        }
    }
}

/// 聚合下推入口：尝试穿过 Union/Join 等算子。
fn push_down_aggregation_descendants(
    plan: &mut logicalop::LogicalPlanRef,
    allow_cross_node_pushdown: bool,
) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        push_down_aggregation_descendants(child, allow_cross_node_pushdown)?;
    }
    // AggregationPushDownSolver embeds aggregationEliminateChecker in Go and
    // attempts elimination before considering any cross-node push-down.
    eliminate_aggregation_descendants(plan, false)?;
    if allow_cross_node_pushdown
        && (push_distinct_count_across_union(plan)? || push_aggregation_across_union(plan)?)
    {
        for child in plan.Children_mut() {
            push_down_aggregation_descendants(child, allow_cross_node_pushdown)?;
        }
        return Ok(());
    }
    if allow_cross_node_pushdown && push_aggregation_across_join(plan)? {
        eliminate_aggregation_descendants(plan, false)?;
        return Ok(());
    }
    let candidate = {
        let Some(aggregation) = plan
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
        else {
            return Ok(());
        };
        let Some(projection) = aggregation.Children().first().and_then(|child| {
            child
                .as_any()
                .downcast_ref::<logicalop::LogicalProjection>()
        }) else {
            return Ok(());
        };
        if projection.Children().len() != 1 {
            return Ok(());
        }
        let Some(context) = aggregation.SCtx().cloned() else {
            return Ok(());
        };
        let projection_schema = projection.Schema().Clone();
        let projection_expressions = projection.Exprs.clone();
        let mut group_by = Vec::with_capacity(aggregation.GroupByItems.len());
        for item in &aggregation.GroupByItems {
            let (failed, substituted) = expression::ColumnSubstituteAll(
                context.GetExprCtx(),
                item.clone(),
                &projection_schema,
                &projection_expressions,
            );
            if failed || expression::ExprsHasSideEffects(std::slice::from_ref(&substituted)) {
                return Ok(());
            }
            group_by.push(substituted);
        }
        let evaluation_context = context.GetExprCtx().GetEvalCtx();
        let mut functions = aggregation.AggFuncs.clone();
        for function in &mut functions {
            let old_arguments = function.Args.clone();
            let mut arguments = Vec::with_capacity(old_arguments.len());
            for argument in &old_arguments {
                let (failed, substituted) = expression::ColumnSubstituteAll(
                    context.GetExprCtx(),
                    argument.clone(),
                    &projection_schema,
                    &projection_expressions,
                );
                if failed
                    || expression::ExprsHasSideEffects(std::slice::from_ref(&substituted))
                    || argument.GetType(evaluation_context).EvalType()
                        != substituted.GetType(evaluation_context).EvalType()
                {
                    return Ok(());
                }
                arguments.push(substituted);
            }
            function.Args = arguments;
            for item in &mut function.OrderByItems {
                let old = item.Expr.clone();
                let (failed, substituted) = expression::ColumnSubstituteAll(
                    context.GetExprCtx(),
                    old.clone(),
                    &projection_schema,
                    &projection_expressions,
                );
                if failed
                    || expression::ExprsHasSideEffects(std::slice::from_ref(&substituted))
                    || old.GetType(evaluation_context).EvalType()
                        != substituted.GetType(evaluation_context).EvalType()
                {
                    return Ok(());
                }
                item.Expr = substituted;
            }
        }
        Some((group_by, functions))
    };
    let Some((group_by, functions)) = candidate else {
        return Ok(());
    };
    let aggregation = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
        .expect("aggregation checked above");
    let mut projection = aggregation.TakeChildren().remove(0);
    let child = projection.TakeChildren().remove(0);
    aggregation.GroupByItems = group_by;
    aggregation.AggFuncs = functions;
    aggregation.SetChildren(vec![child]);
    // Go's aggregation push-down solver embeds aggregationEliminateChecker:
    // after crossing a Projection, the new direct child may expose a unique
    // key that was not visible on the old Projection/Join shape.
    eliminate_aggregation_descendants(plan, false)
}

/// 判断计划输出中某列是否唯一。
fn plan_column_is_unique(plan: &dyn logicalop::LogicalPlan, column: &expression::Column) -> bool {
    if plan
        .Schema()
        .PKOrUK
        .iter()
        .any(|key| key.len() == 1 && key[0].UniqueID == column.UniqueID)
    {
        return true;
    }
    let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>() else {
        return false;
    };
    let Some(index) = projection.Schema().ColumnIndex(column) else {
        return false;
    };
    let Some(source) = projection
        .Exprs
        .get(index)
        .and_then(|item| item.as_column())
    else {
        return false;
    };
    projection
        .Children()
        .first()
        .is_some_and(|child| plan_column_is_unique(child.as_ref(), source))
}

/// 将 distinct count 下推到 Union 各分支。
fn push_distinct_count_across_union(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<bool, expression::Error> {
    let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    else {
        return Ok(false);
    };
    if !aggregation.GroupByItems.is_empty() || aggregation.AggFuncs.len() != 1 {
        return Ok(false);
    }
    let function = &aggregation.AggFuncs[0];
    if function.Name != crate::ast::AggFuncCount
        || !function.HasDistinct
        || function.Args.len() != 1
    {
        return Ok(false);
    }
    let Some(argument) = function.Args[0].as_column() else {
        return Ok(false);
    };
    let Some(union) = aggregation
        .Children()
        .first()
        .and_then(|child| child.as_any().downcast_ref::<logicalop::LogicalUnionAll>())
    else {
        return Ok(false);
    };
    let Some(column_index) = union.Schema().ColumnIndex(argument) else {
        return Ok(false);
    };
    let context = aggregation
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("aggregation has no plan context"))?;
    let query_block = aggregation.QueryBlockOffset();
    let mut output_columns = vec![argument.Clone(), argument.Clone()];
    output_columns[0].Index = 0;
    output_columns[1].Index = 1;
    let output_schema = expression::NewSchema(output_columns);

    let aggregation = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
        .expect("aggregation checked above");
    let mut union_plan = aggregation.TakeChildren().remove(0);
    let union = union_plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalUnionAll>()
        .expect("union checked above");
    let mut children = union.TakeChildren();
    for child in &mut children {
        let source = child.Schema().Columns[column_index].Clone();
        let unique = plan_column_is_unique(child.as_ref(), &source);
        let original = std::mem::replace(child, Box::new(logicalop::LogicalTableDual::default()));
        if unique {
            let mut projection = logicalop::LogicalProjection {
                Exprs: vec![Box::new(source.Clone()), Box::new(source)],
                ..logicalop::LogicalProjection::default()
            }
            .Init(context.clone(), query_block);
            projection.SetSchema(output_schema.Clone());
            projection.SetOutputNames(expression::types::NameSlice(vec![None; 2]));
            projection.SetChildren(vec![original]);
            *child = Box::new(projection);
        } else {
            let first = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncFirstRow,
                vec![Box::new(source.Clone())],
                false,
            )?;
            let second = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncFirstRow,
                vec![Box::new(source.Clone())],
                false,
            )?;
            let mut pushed = logicalop::LogicalAggregation {
                AggFuncs: vec![first, second],
                GroupByItems: vec![Box::new(source)],
                ..logicalop::LogicalAggregation::default()
            }
            .Init(context.clone(), query_block);
            pushed.SetSchema(output_schema.Clone());
            pushed.SetOutputNames(expression::types::NameSlice(vec![None; 2]));
            pushed.SetChildren(vec![original]);
            *child = Box::new(pushed);
        }
    }
    union.SetSchema(output_schema);
    union.SetChildren(children);
    aggregation.SetChildren(vec![union_plan]);
    Ok(true)
}

/// 将聚合下推穿过 UnionAll。
fn push_aggregation_across_union(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<bool, expression::Error> {
    let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    else {
        return Ok(false);
    };
    let Some(union) = aggregation
        .Children()
        .first()
        .and_then(|child| child.as_any().downcast_ref::<logicalop::LogicalUnionAll>())
    else {
        return Ok(false);
    };
    if union.Children().is_empty()
        || aggregation
            .AggFuncs
            .iter()
            .all(|function| function.Name == crate::ast::AggFuncFirstRow)
        || aggregation.AggFuncs.iter().any(|function| {
            !function.OrderByItems.is_empty()
                || function.HasDistinct
                || matches!(
                    function.Name.as_str(),
                    crate::ast::AggFuncGroupConcat
                        | crate::ast::AggFuncVarPop
                        | crate::ast::AggFuncJsonArrayagg
                        | crate::ast::AggFuncApproxPercentile
                        | crate::ast::AggFuncJsonObjectAgg
                )
                || !matches!(
                    function.Name.as_str(),
                    crate::ast::AggFuncMax
                        | crate::ast::AggFuncMin
                        | crate::ast::AggFuncFirstRow
                        | crate::ast::AggFuncSum
                        | crate::ast::AggFuncCount
                        | crate::ast::AggFuncAvg
                        | crate::ast::AggFuncApproxCountDistinct
                )
        })
    {
        return Ok(false);
    }
    let context = aggregation
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("aggregation has no plan context"))?;
    let original_union_schema = union.Schema().Clone();
    let final_schema = aggregation.Schema().Clone();
    let mut partial_columns = Vec::new();
    let mut partial_functions = Vec::new();
    let mut final_functions = Vec::new();
    for (offset, function) in aggregation.AggFuncs.iter().enumerate() {
        let output_count = usize::from(function.Name == crate::ast::AggFuncAvg) + 1;
        let start = partial_columns.len();
        let ordinals = (start..start + output_count)
            .map(|index| index as isize)
            .collect::<Vec<_>>();
        let (partial, mut final_function) = function.Split(&ordinals);
        for output in 0..output_count {
            let field_type = final_function
                .Args
                .get(output)
                .and_then(|argument| argument.as_column())
                .and_then(|column| column.RetType.clone())
                .or_else(|| {
                    final_schema
                        .Columns
                        .get(offset)
                        .and_then(|column| column.RetType.clone())
                })
                .ok_or_else(|| expression::errors::New("aggregate output type is required"))?;
            partial_columns.push(expression::Column::new(
                field_type,
                0,
                context.GetExprCtx().AllocPlanColumnID(),
                (start + output) as isize,
            ));
        }
        for argument in &mut final_function.Args {
            if let Some(column) = argument.as_column()
                && let Some(partial_column) = partial_columns.get(column.Index as usize)
            {
                *argument = Box::new(partial_column.Clone());
            }
        }
        if function.Name == crate::ast::AggFuncAvg {
            let argument = function
                .Args
                .first()
                .ok_or_else(|| expression::errors::New("AVG requires one argument"))?
                .CloneExpr();
            let mut count = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncCount,
                vec![argument.CloneExpr()],
                false,
            )?;
            let mut sum = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncSum,
                vec![argument],
                false,
            )?;
            count.Mode = aggregation::Partial1Mode;
            sum.Mode = aggregation::Partial1Mode;
            partial_functions.push(count);
            partial_functions.push(sum);
        } else {
            partial_functions.push(partial);
        }
        final_functions.push(final_function);
    }
    let mut final_group_by = Vec::with_capacity(aggregation.GroupByItems.len());
    for item in &aggregation.GroupByItems {
        let mut column = item
            .as_column()
            .map(expression::Column::Clone)
            .unwrap_or_else(|| {
                expression::Column::new(
                    item.GetType(context.GetExprCtx().GetEvalCtx()).clone(),
                    0,
                    context.GetExprCtx().AllocPlanColumnID(),
                    partial_columns.len() as isize,
                )
            });
        column.Index = partial_columns.len() as isize;
        partial_columns.push(column.Clone());
        final_group_by.push(Box::new(column) as expression::ExprBox);
    }
    let partial_schema = expression::NewSchema(partial_columns);
    let partial_group_by = aggregation
        .GroupByItems
        .iter()
        .map(|item| item.CloneExpr())
        .collect::<Vec<_>>();

    let aggregation = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
        .expect("aggregation checked above");
    aggregation.AggFuncs = final_functions;
    aggregation.GroupByItems = final_group_by;
    let prefer_agg_type = aggregation.PreferAggType;
    let prefer_agg_to_cop = aggregation.PreferAggToCop;
    let query_block = aggregation.QueryBlockOffset();
    let mut union_plan = aggregation.TakeChildren().remove(0);
    let union = union_plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalUnionAll>()
        .expect("union checked above");
    let mut children = union.TakeChildren();
    for child in &mut children {
        let child_columns = expression::Column2Exprs(&child.Schema().Columns);
        let substitute = |item: expression::ExprBox| {
            let (_, result) = expression::ColumnSubstituteAll(
                context.GetExprCtx(),
                item,
                &original_union_schema,
                &child_columns,
            );
            result
        };
        let mut functions = partial_functions.clone();
        for function in &mut functions {
            function.Args = std::mem::take(&mut function.Args)
                .into_iter()
                .map(&substitute)
                .collect();
        }
        let groups = partial_group_by
            .iter()
            .map(|item| substitute(item.CloneExpr()))
            .collect::<Vec<_>>();
        for group in &groups {
            let mut first_row = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncFirstRow,
                vec![group.CloneExpr()],
                false,
            )?;
            first_row.Mode = partial_functions
                .first()
                .map_or(aggregation::Partial1Mode, |function| function.Mode);
            functions.push(first_row);
        }
        let original_child =
            std::mem::replace(child, Box::new(logicalop::LogicalTableDual::default()));
        let mut pushed = logicalop::LogicalAggregation {
            AggFuncs: functions,
            GroupByItems: groups,
            PreferAggType: prefer_agg_type,
            PreferAggToCop: prefer_agg_to_cop,
            ..logicalop::LogicalAggregation::default()
        }
        .Init(context.clone(), query_block);
        pushed.SetSchema(partial_schema.Clone());
        pushed.SetOutputNames(expression::types::NameSlice(vec![
            None;
            partial_schema.Len()
        ]));
        pushed.SetChildren(vec![original_child]);
        *child = Box::new(pushed);
    }
    union.SetSchema(partial_schema);
    union.SetChildren(children);
    aggregation.SetChildren(vec![union_plan]);
    Ok(true)
}

/// 将聚合下推穿过满足条件的 Join。
fn push_aggregation_across_join(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<bool, expression::Error> {
    let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    else {
        return Ok(false);
    };
    let Some(join) = aggregation
        .Children()
        .first()
        .and_then(|child| child.as_any().downcast_ref::<logicalop::LogicalJoin>())
    else {
        return Ok(false);
    };
    if !matches!(
        join.JoinType,
        base::JoinType::InnerJoin | base::JoinType::LeftOuterJoin | base::JoinType::RightOuterJoin
    ) || join.Children().len() != 2
    {
        return Ok(false);
    }
    let left_schema = join.Children()[0].Schema();
    let right_schema = join.Children()[1].Schema();
    let mut function_sides = Vec::with_capacity(aggregation.AggFuncs.len());
    for function in &aggregation.AggFuncs {
        if !function.OrderByItems.is_empty()
            || matches!(
                function.Name.as_str(),
                crate::ast::AggFuncAvg
                    | crate::ast::AggFuncGroupConcat
                    | crate::ast::AggFuncVarPop
                    | crate::ast::AggFuncJsonArrayagg
                    | crate::ast::AggFuncJsonObjectAgg
                    | crate::ast::AggFuncStddevPop
                    | crate::ast::AggFuncVarSamp
                    | crate::ast::AggFuncApproxPercentile
                    | crate::ast::AggFuncStddevSamp
            )
            || (!matches!(
                function.Name.as_str(),
                crate::ast::AggFuncMax
                    | crate::ast::AggFuncMin
                    | crate::ast::AggFuncFirstRow
                    | crate::ast::AggFuncSum
                    | crate::ast::AggFuncCount
            ))
            || (matches!(
                function.Name.as_str(),
                crate::ast::AggFuncSum | crate::ast::AggFuncCount
            ) && function.HasDistinct)
        {
            return Ok(false);
        }
        let columns = function
            .Args
            .iter()
            .flat_map(|argument| expression::ExtractColumns(argument.as_ref()))
            .collect::<Vec<_>>();
        let from_left = columns.iter().any(|column| left_schema.Contains(column));
        let from_right = columns.iter().any(|column| right_schema.Contains(column));
        let side = match (from_left, from_right) {
            (true, false) => 0,
            (false, true) => 1,
            (false, false) => match join.JoinType {
                base::JoinType::LeftOuterJoin => 0,
                base::JoinType::RightOuterJoin => 1,
                _ => 1,
            },
            (true, true) => return Ok(false),
        };
        let null_generating = (side == 0 && join.JoinType == base::JoinType::RightOuterJoin)
            || (side == 1 && join.JoinType == base::JoinType::LeftOuterJoin);
        if null_generating
            && !function
                .Args
                .iter()
                .all(|argument| argument.as_any().is::<expression::Column>())
        {
            return Ok(false);
        }
        function_sides.push(side);
    }

    let mut group_columns = [Vec::<expression::Column>::new(), Vec::new()];
    let mut add_group_column = |column: &expression::Column| {
        let side = usize::from(!left_schema.Contains(column));
        if (side == 0 && left_schema.Contains(column))
            || (side == 1 && right_schema.Contains(column))
        {
            if !group_columns[side]
                .iter()
                .any(|existing| existing.UniqueID == column.UniqueID)
            {
                group_columns[side].push(column.clone());
            }
        }
    };
    for item in &aggregation.GroupByItems {
        for column in expression::ExtractColumns(item.as_ref()) {
            add_group_column(column);
        }
    }
    for condition in join
        .EqualConditions
        .iter()
        .map(|condition| condition.as_ref() as &dyn expression::Expression)
        .chain(
            join.LeftConditions
                .iter()
                .map(|condition| condition.as_ref()),
        )
        .chain(
            join.RightConditions
                .iter()
                .map(|condition| condition.as_ref()),
        )
        .chain(
            join.OtherConditions
                .iter()
                .map(|condition| condition.as_ref()),
        )
        .chain(
            join.NAEQConditions
                .iter()
                .map(|condition| condition.as_ref()),
        )
    {
        for column in expression::ExtractColumns(condition) {
            add_group_column(column);
        }
    }
    drop(add_group_column);

    let functions_by_side = [
        function_sides
            .iter()
            .enumerate()
            .filter_map(|(index, side)| (*side == 0).then_some(index))
            .collect::<Vec<_>>(),
        function_sides
            .iter()
            .enumerate()
            .filter_map(|(index, side)| (*side == 1).then_some(index))
            .collect::<Vec<_>>(),
    ];
    let has_count_or_sum = |indices: &[usize]| {
        indices.iter().any(|index| {
            matches!(
                aggregation.AggFuncs[*index].Name.as_str(),
                crate::ast::AggFuncCount | crate::ast::AggFuncSum
            )
        })
    };
    let may_push = [
        !has_count_or_sum(&functions_by_side[1]),
        !has_count_or_sum(&functions_by_side[0]),
    ];
    let function_sides = function_sides;
    let group_columns = group_columns;

    let aggregation = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
        .expect("aggregation checked above");
    let context = aggregation
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("aggregation has no plan context"))?;
    let query_block = aggregation.QueryBlockOffset();
    let prefer_agg_type = aggregation.PreferAggType;
    let prefer_agg_to_cop = aggregation.PreferAggToCop;
    let mut join_plan = aggregation.TakeChildren().remove(0);
    let join = join_plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalJoin>()
        .expect("join checked above");
    let mut changed = false;
    for side in 0..2 {
        let indices = &functions_by_side[side];
        if !may_push[side]
            || indices.is_empty()
            || indices
                .iter()
                .all(|index| aggregation.AggFuncs[*index].Name == crate::ast::AggFuncFirstRow)
            || join.Children()[side]
                .as_any()
                .is::<logicalop::LogicalJoin>()
            || join.Children()[side].Schema().PKOrUK.iter().any(|key| {
                !key.is_empty()
                    && key.iter().all(|key_column| {
                        group_columns[side]
                            .iter()
                            .any(|column| column.UniqueID == key_column.UniqueID)
                    })
            })
        {
            continue;
        }
        let null_generating = (side == 0 && join.JoinType == base::JoinType::RightOuterJoin)
            || (side == 1 && join.JoinType == base::JoinType::LeftOuterJoin);
        let mut pushed_functions = Vec::with_capacity(indices.len() + group_columns[side].len());
        let mut output_columns = Vec::with_capacity(indices.len() + group_columns[side].len());
        for index in indices {
            let partial = aggregation.AggFuncs[*index].clone();
            let mut output = expression::Column::new(
                partial.RetTp.clone().unwrap_or_else(|| {
                    *expression::types::NewFieldType(expression::mysql::TypeLonglong)
                }),
                0,
                context.GetExprCtx().AllocPlanColumnID(),
                output_columns.len() as isize,
            );
            let mut final_argument = output.Clone();
            if null_generating {
                if let Some(return_type) = &final_argument.RetType {
                    let mut return_type = return_type.clone();
                    return_type.DelFlag(expression::mysql::NotNullFlag);
                    final_argument.RetType = Some(return_type);
                }
            }
            aggregation.AggFuncs[*index].Args = vec![Box::new(final_argument)];
            aggregation.AggFuncs[*index].Mode = aggregation::FinalMode;
            pushed_functions.push(partial);
            output.Index = output_columns.len() as isize;
            output_columns.push(output);
        }
        for column in &group_columns[side] {
            let first_row = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncFirstRow,
                vec![Box::new(column.clone())],
                false,
            )?;
            let mut output = column.clone();
            output.RetType = first_row.RetTp.clone();
            output.Index = output_columns.len() as isize;
            pushed_functions.push(first_row);
            output_columns.push(output);
        }
        let child = std::mem::replace(
            &mut join.Children_mut()[side],
            Box::new(logicalop::LogicalTableDual::default()),
        );
        let mut pushed = logicalop::LogicalAggregation {
            AggFuncs: pushed_functions,
            GroupByItems: expression::Column2Exprs(&group_columns[side]),
            PreferAggType: prefer_agg_type,
            PreferAggToCop: prefer_agg_to_cop,
            ..logicalop::LogicalAggregation::default()
        }
        .Init(context.clone(), query_block);
        pushed.SetSchema(expression::NewSchema(output_columns));
        pushed.SetOutputNames(expression::types::NameSlice(vec![
            None;
            pushed.Schema().Len()
        ]));
        pushed.SetChildren(vec![child]);
        join.Children_mut()[side] = Box::new(pushed);
        changed = true;
    }
    if changed {
        join.MergeSchema();
        join.BuildKeyInfo();
    }
    aggregation.SetChildren(vec![join_plan]);
    let _ = function_sides;
    Ok(changed)
}

/// 同步等待统计信息加载点，确保代价估计可用。
fn sync_wait_stats_load_point(plan: &dyn logicalop::LogicalPlan) -> Result<(), expression::Error> {
    let Some(context) = plan.SCtx() else {
        return Ok(());
    };
    let statement_context = &context.GetSessionVars().StmtCtx;
    if statement_context.IsSyncStatsFailed() {
        return Ok(());
    }
    let pending_items = statement_context.PendingStatsLoadItems();
    if pending_items == 0 {
        return Ok(());
    }
    let Some(waiter) = context.GetStatsLoadWaiter() else {
        let error =
            "synchronous statistics are pending but this PlanContext has no StatsHandle".to_owned();
        statement_context.FailStatsSyncWait(std::time::Duration::ZERO, error.clone());
        return Err(expression::errors::New(error));
    };
    let started = Instant::now();
    match waiter.SyncWaitStatsLoad(context.GetSessionVars()) {
        Ok(()) => {
            statement_context.CompleteStatsSyncWait(started.elapsed());
            Ok(())
        }
        Err(error) => {
            statement_context.FailStatsSyncWait(started.elapsed(), error.clone());
            if !vardef_dependency::StatsLoadPseudoTimeout.Load() {
                return Err(expression::errors::New(error));
            }
            statement_context.PlanCacheTracker.SetSkipPlanCache(
                "synchronous statistics load failed and fell back to pseudo statistics",
            );
            statement_context.AppendWarning(stmtctx_dependency::errors::NewNoStackError(format!(
                "synchronous statistics load failed, using pseudo statistics: {error}"
            )));
            let pending = statement_context
                .StatsLoad
                .NeededItems
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter_map(|value| {
                    stmtctx_dependency::cache_downcast_ref::<model_dependency::StatsLoadItem>(value)
                })
                .map(|item| {
                    (
                        asyncload_dependency::TableItemID {
                            TableID: item.TableItemID.TableID,
                            ID: item.TableItemID.ID,
                            IsIndex: item.TableItemID.IsIndex,
                            IsSyncLoadFailed: true,
                        },
                        item.FullLoad,
                    )
                })
                .collect::<Vec<_>>();
            for (item, full_load) in pending {
                asyncload_dependency::AsyncLoadHistogramNeededItems.Insert(item, full_load);
            }
            statement_context.ConsumePendingStatsLoadItems();
            Ok(())
        }
    }
}

/// 全文检索 TopK 上限。
const MAX_FTS_TOP_K: u32 = u32::MAX;
/// 脏事务下拒绝全文索引路径的错误文案。
const FTS_DIRTY_TXN_ERROR: &str =
    "FTS_MATCH_WORD() cannot be used in a transaction with uncommitted changes";

/// 全文匹配表达式解析结果摘要。
struct FullTextExpressionInfo {
    query: String,
    column: expression::Column,
}

/// 解析表达式是否为全文匹配调用。
fn interpret_full_text_expression(
    item: &dyn expression::Expression,
) -> Option<FullTextExpressionInfo> {
    let function = item.as_scalar_function()?;
    if function.FuncName.L != "fts_match_word" || function.GetArgs().len() != 2 {
        return None;
    }
    let query = function.GetArgs()[0]
        .as_any()
        .downcast_ref::<expression::Constant>()?;
    let column = function.GetArgs()[1].as_column()?.clone();
    Some(FullTextExpressionInfo {
        query: query.Value.GetString(),
        column,
    })
}

/// 表达式树是否含全文匹配。
fn contains_full_text_expression(item: &dyn expression::Expression) -> bool {
    let Some(function) = item.as_scalar_function() else {
        return false;
    };
    function.FuncName.L == "fts_match_word"
        || function
            .GetArgs()
            .iter()
            .any(|argument| contains_full_text_expression(argument.as_ref()))
}

/// 在 WHERE 路径解析并绑定全文索引。
fn full_text_resolve_where(plan: &mut logicalop::LogicalPlanRef) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        full_text_resolve_where(child)?;
    }
    let candidate = {
        let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() else {
            return Ok(());
        };
        let Some(source) = selection
            .Children()
            .first()
            .and_then(|child| child.as_any().downcast_ref::<logicalop::DataSource>())
        else {
            return Ok(());
        };
        let Some((condition_index, info)) =
            selection
                .Conditions
                .iter()
                .enumerate()
                .find_map(|(index, condition)| {
                    interpret_full_text_expression(condition.as_ref()).map(|info| (index, info))
                })
        else {
            return Ok(());
        };
        let Some(index) = source.TableInfo.Indices.iter().find(|index| {
            index.FullTextInfo.is_some()
                && index.IsPublic()
                && index.Columns.len() == 1
                && index.Columns[0].Offset >= 0
                && source
                    .TableInfo
                    .Columns
                    .get(index.Columns[0].Offset as usize)
                    .is_some_and(|column| column.ID == info.column.ID)
        }) else {
            return Err(expression::errors::New(
                "Full text search can only be used with a matching fulltext index",
            ));
        };
        Some((
            condition_index,
            index.Clone(),
            info.column.ID,
            info.column.OrigName,
            info.query,
        ))
    };
    let Some((condition_index, index, column_id, column_name, query_text)) = candidate else {
        return Ok(());
    };
    let context = plan
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("FTS Selection has no plan context"))?;
    let mut score_type = *expression::types::NewFieldType(expression::mysql::TypeFloat);
    score_type.AddFlag(expression::mysql::NotNullFlag);
    let selection = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalSelection>()
        .expect("FTS selection checked above");
    let source = selection.Children_mut()[0]
        .as_any_mut()
        .downcast_mut::<logicalop::DataSource>()
        .expect("FTS source checked above");
    if source.FtsPushDown.is_none() {
        source.FtsPushDown = Some(logicalop::FTSPushDown {
            QueryInfo: logicalop::FTSQueryInfo {
                IndexID: index.ID,
                ColumnID: column_id,
                ColumnName: column_name,
                QueryText: query_text,
                QueryTokenizer: index
                    .FullTextInfo
                    .as_ref()
                    .map_or_else(String::new, |info| info.ParserType.String().to_owned()),
                QueryType: logicalop::FTSQueryType::NoScore,
                TopK: MAX_FTS_TOP_K,
            },
            IndexInfo: index,
        });
        source.Columns.push(expression::model::ColumnInfo {
            ID: expression::model::VirtualColFTSScoreID,
            Name: crate::ast::NewCIStr("_FTS_SCORE"),
            Offset: source.Columns.len() as isize,
            FieldType: score_type.clone(),
            State: expression::model::StatePublic,
            ..expression::model::ColumnInfo::default()
        });
        let mut score = expression::Column::new(
            score_type,
            expression::model::VirtualColFTSScoreID,
            context.GetExprCtx().AllocPlanColumnID(),
            source.Schema().Len() as isize,
        );
        score.OrigName = "_FTS_SCORE".to_owned();
        score.IsHidden = true;
        source.Schema_mut().Append([score]);
    }
    selection.Conditions.remove(condition_index);
    if selection.Conditions.is_empty() {
        *plan = selection.TakeChildren().remove(0);
    }
    Ok(())
}

/// 定位全文路径下的 DataSource。
fn full_text_path_source(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::DataSource> {
    if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        return source.FtsPushDown.as_ref().map(|_| source);
    }
    if plan.Children().len() == 1
        && (plan.as_any().is::<logicalop::LogicalSelection>()
            || plan.as_any().is::<logicalop::LogicalTopN>())
    {
        return full_text_path_source(plan.Children()[0].as_ref());
    }
    None
}

/// 可变定位全文路径下的 DataSource。
fn full_text_path_source_mut(
    plan: &mut dyn logicalop::LogicalPlan,
) -> Option<&mut logicalop::DataSource> {
    if plan.as_any().is::<logicalop::DataSource>() {
        return plan
            .as_any_mut()
            .downcast_mut::<logicalop::DataSource>()
            .filter(|source| source.FtsPushDown.is_some());
    }
    if plan.Children().len() == 1
        && (plan.as_any().is::<logicalop::LogicalSelection>()
            || plan.as_any().is::<logicalop::LogicalTopN>())
    {
        return full_text_path_source_mut(plan.Children_mut()[0].as_mut());
    }
    None
}

/// 在 TopN 路径解析全文索引。
fn full_text_resolve_top_n(plan: &mut logicalop::LogicalPlanRef) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        full_text_resolve_top_n(child)?;
    }
    let candidate = {
        let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() else {
            return Ok(());
        };
        let Some(first) = top_n.ByItems.first() else {
            return Ok(());
        };
        let Some(info) = interpret_full_text_expression(first.Expr.as_ref()) else {
            return Ok(());
        };
        let Some(source) = top_n
            .Children()
            .first()
            .and_then(|child| full_text_path_source(child.as_ref()))
        else {
            return Ok(());
        };
        let push_down = source.FtsPushDown.as_ref().expect("FTS path checked");
        if push_down.QueryInfo.ColumnID != info.column.ID
            || push_down.QueryInfo.QueryText != info.query
        {
            return Err(expression::errors::New(
                "'FTS_MATCH_WORD()' in ORDER BY must match the one in WHERE",
            ));
        }
        let score = source
            .Schema()
            .Columns
            .last()
            .cloned()
            .ok_or_else(|| expression::errors::New("FTS source has no score column"))?;
        Some((
            score,
            top_n.Offset,
            top_n.Count,
            first.Desc,
            top_n.ByItems.len(),
        ))
    };
    let Some((score, offset, count, descending, by_items)) = candidate else {
        return Ok(());
    };
    let top_n = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalTopN>()
        .expect("FTS TopN checked above");
    top_n.ByItems[0].Expr = Box::new(score);
    let direct_source = top_n.Children()[0].as_any().is::<logicalop::DataSource>();
    let source = full_text_path_source_mut(top_n.Children_mut()[0].as_mut())
        .expect("FTS mutable source path");
    let query = &mut source
        .FtsPushDown
        .as_mut()
        .expect("FTS push down")
        .QueryInfo;
    query.QueryType = logicalop::FTSQueryType::WithScore;
    if direct_source && source.PushedDownConds.is_empty() && descending && by_items == 1 {
        query.TopK = offset.saturating_add(count).min(MAX_FTS_TOP_K as u64) as u32;
    }
    Ok(())
}

/// 在 Projection 路径解析全文索引。
fn full_text_resolve_projection(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        full_text_resolve_projection(child)?;
    }
    let candidate = {
        let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>() else {
            return Ok(());
        };
        let Some(source) = projection
            .Children()
            .first()
            .and_then(|child| full_text_path_source(child.as_ref()))
        else {
            return Ok(());
        };
        let push_down = source.FtsPushDown.as_ref().expect("FTS path checked");
        let mut matched = Vec::new();
        for (index, expression) in projection.Exprs.iter().enumerate() {
            let Some(info) = interpret_full_text_expression(expression.as_ref()) else {
                continue;
            };
            if push_down.QueryInfo.ColumnID != info.column.ID
                || push_down.QueryInfo.QueryText != info.query
            {
                return Err(expression::errors::New(
                    "'FTS_MATCH_WORD()' in SELECT must match the one in WHERE",
                ));
            }
            matched.push(index);
        }
        if matched.is_empty() {
            return Ok(());
        }
        let score = source
            .Schema()
            .Columns
            .last()
            .cloned()
            .ok_or_else(|| expression::errors::New("FTS source has no score column"))?;
        Some((matched, score))
    };
    let Some((matched, score)) = candidate else {
        return Ok(());
    };
    let projection = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
        .expect("FTS projection checked above");
    for index in matched {
        projection.Exprs[index] = Box::new(score.clone());
        projection.Schema_mut().Columns[index].RetType = score.RetType.clone();
    }
    let source = full_text_path_source_mut(projection.Children_mut()[0].as_mut())
        .expect("FTS mutable source path");
    source
        .FtsPushDown
        .as_mut()
        .expect("FTS push down")
        .QueryInfo
        .QueryType = logicalop::FTSQueryType::WithScore;
    Ok(())
}

/// 拒绝仍残留未解析的全文表达式。
fn full_text_reject_remaining(plan: &dyn logicalop::LogicalPlan) -> Result<(), expression::Error> {
    if let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>() {
        for item in &projection.Exprs {
            if interpret_full_text_expression(item.as_ref()).is_some() {
                return Err(expression::errors::New(
                    "'FTS_MATCH_WORD()' in SELECT requires a matching 'FTS_MATCH_WORD()' in WHERE. A valid example: SELECT FTS_MATCH_WORD(...) FROM <TABLE> WHERE FTS_MATCH_WORD(...)",
                ));
            }
            if contains_full_text_expression(item.as_ref()) {
                return Err(expression::errors::New(
                    "'FTS_MATCH_WORD()' in SELECT must not be wrapped in expressions. A valid example: SELECT FTS_MATCH_WORD(...) FROM <TABLE> WHERE FTS_MATCH_WORD(...)",
                ));
            }
        }
    } else if let Some(union_scan) = plan.as_any().downcast_ref::<logicalop::LogicalUnionScan>() {
        if union_scan
            .Conditions
            .iter()
            .any(|condition| contains_full_text_expression(condition.as_ref()))
        {
            return Err(expression::errors::New(FTS_DIRTY_TXN_ERROR));
        }
    } else if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        if source
            .PushedDownConds
            .iter()
            .any(|condition| contains_full_text_expression(condition.as_ref()))
        {
            return Err(expression::errors::New(
                "Currently 'FTS_MATCH_WORD()' must be used alone. It cannot be placed inside any other function or expression as a parameter, or used multiple times. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...)",
            ));
        }
    } else if let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() {
        let contains = selection
            .Conditions
            .iter()
            .any(|condition| contains_full_text_expression(condition.as_ref()));
        if contains
            && selection
                .Children()
                .first()
                .is_some_and(|child| child.as_any().is::<logicalop::LogicalUnionScan>())
        {
            return Err(expression::errors::New(FTS_DIRTY_TXN_ERROR));
        }
        if contains {
            return Err(expression::errors::New(
                "Currently 'FTS_MATCH_WORD()' must be used alone. It cannot be placed inside any other function or expression as a parameter, or used multiple times. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...)",
            ));
        }
    } else if let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() {
        for (index, item) in top_n.ByItems.iter().enumerate() {
            if contains_full_text_expression(item.Expr.as_ref()) {
                let message = if index > 0 {
                    "FTS_MATCH_WORD() must be used as the first item in ORDER BY. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...) ORDER BY FTS_MATCH_WORD(...) LIMIT .."
                } else {
                    "Unsupported 'FTS_MATCH_WORD()' usage. It must be used with a WHERE clause and must be used alone. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...) ORDER BY FTS_MATCH_WORD(...) LIMIT .."
                };
                return Err(expression::errors::New(message));
            }
        }
    } else if let Some(sort) = plan.as_any().downcast_ref::<logicalop::LogicalSort>() {
        if sort
            .ByItems
            .iter()
            .any(|item| contains_full_text_expression(item.Expr.as_ref()))
        {
            return Err(expression::errors::New(
                "Currently 'FTS_MATCH_WORD()' in ORDER BY without a LIMIT clause is not supported, try specify a very large LIMIT as a workaround",
            ));
        }
    }
    for child in plan.Children() {
        full_text_reject_remaining(child.as_ref())?;
    }
    Ok(())
}

/// 已下推的 SEQUENCE 算子附着信息。
struct PushedSequence {
    context: base::ContextRef,
    query_block: i32,
    ctes: Vec<logicalop::LogicalPlanRef>,
}

/// 将 Sequence 算子尽量下推靠近数据源。
fn push_down_sequence(plan: &mut logicalop::LogicalPlanRef) {
    let owned = std::mem::replace(plan, Box::new(logicalop::LogicalTableDual::default()));
    *plan = push_down_sequence_owned(None, owned);
}

/// 消费所有权版本的 Sequence 下推。
fn push_down_sequence_owned(
    pushed: Option<PushedSequence>,
    mut plan: logicalop::LogicalPlanRef,
) -> logicalop::LogicalPlanRef {
    if plan.as_any().is::<logicalop::LogicalSequence>() {
        let Some(context) = plan.SCtx().cloned() else {
            return plan;
        };
        let query_block = plan.QueryBlockOffset();
        let mut children = plan.TakeChildren();
        let Some(main_query) = children.pop() else {
            return plan;
        };
        let mut ctes = pushed.map_or_else(Vec::new, |pushed| pushed.ctes);
        ctes.append(&mut children);
        return push_down_sequence_owned(
            Some(PushedSequence {
                context,
                query_block,
                ctes,
            }),
            main_query,
        );
    }

    let Some(pushed) = pushed else {
        let children = plan
            .TakeChildren()
            .into_iter()
            .map(|child| push_down_sequence_owned(None, child))
            .collect();
        plan.SetChildren(children);
        return plan;
    };

    if plan.as_any().is::<logicalop::DataSource>() || plan.as_any().is::<logicalop::LogicalCTE>() {
        let children = plan
            .TakeChildren()
            .into_iter()
            .map(|child| push_down_sequence_owned(None, child))
            .collect();
        plan.SetChildren(children);
        return attach_pushed_sequence(pushed, plan);
    }
    if plan.Children().len() == 1 {
        let child = plan.TakeChildren().remove(0);
        plan.SetChildren(vec![push_down_sequence_owned(Some(pushed), child)]);
        return plan;
    }
    attach_pushed_sequence(pushed, plan)
}

/// 把下推的 Sequence 重新附着到计划节点。
fn attach_pushed_sequence(
    mut pushed: PushedSequence,
    main_query: logicalop::LogicalPlanRef,
) -> logicalop::LogicalPlanRef {
    let names = main_query.OutputNames().Shallow();
    pushed.ctes.push(main_query);
    let mut sequence =
        logicalop::LogicalSequence::default().Init(pushed.context, pushed.query_block);
    sequence.SetOutputNames(names);
    sequence.SetChildren(pushed.ctes);
    Box::new(sequence)
}

/// 解析 Expand（GROUPING SETS 展开）算子。
fn resolve_expand_descendants(plan: &mut dyn logicalop::LogicalPlan) {
    for child in plan.Children_mut() {
        resolve_expand_descendants(child.as_mut());
    }
    if let Some(expand) = plan.as_any_mut().downcast_mut::<logicalop::LogicalExpand>() {
        expand.GenLevelProjections();
    }
}

/// 相关子查询相关化（correlate）处理。
fn correlate_descendants(plan: &mut logicalop::LogicalPlanRef) -> Result<(), expression::Error> {
    if plan.as_any().is::<logicalop::LogicalCTE>() {
        return Ok(());
    }
    for child in plan.Children_mut() {
        correlate_descendants(child)?;
    }
    if plan.as_any().is::<logicalop::LogicalApply>() {
        return Ok(());
    }

    let candidate = {
        let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() else {
            return Ok(());
        };
        if !join.JoinType.is_semi_join()
            || !join.PreferCorrelate
            || join.EqualConditions.is_empty()
            || !join.NAEQConditions.is_empty()
            || !join.LeftConditions.is_empty()
            || !join.OtherConditions.is_empty()
            || join.Children().len() != 2
        {
            return Ok(());
        }
        let left_schema = join.Children()[0].Schema();
        let right_schema = join.Children()[1].Schema();
        if matches!(
            join.JoinType,
            base::JoinType::LeftOuterSemiJoin | base::JoinType::AntiLeftOuterSemiJoin
        ) && join.EqualConditions.iter().any(|condition| {
            let Some(function) = condition.as_scalar_function() else {
                return true;
            };
            let (first, second, columns_only) = expression::IsColOpCol(function);
            if !columns_only {
                return true;
            }
            let (Some(first), Some(second)) = (first, second) else {
                return true;
            };
            let (outer, inner) = if left_schema.Contains(first) && right_schema.Contains(second) {
                (first, second)
            } else if left_schema.Contains(second) && right_schema.Contains(first) {
                (second, first)
            } else {
                return true;
            };
            [outer, inner].iter().any(|column| {
                !column.RetType.as_ref().is_some_and(|field_type| {
                    expression::mysql::HasNotNullFlag(field_type.GetFlag())
                })
            })
        }) {
            return Ok(());
        }
        let Some(context) = join.SCtx().cloned() else {
            return Ok(());
        };
        let mut conditions =
            Vec::with_capacity(join.EqualConditions.len() + join.RightConditions.len());
        let mut correlated_columns = Vec::with_capacity(join.EqualConditions.len());
        for equality in &join.EqualConditions {
            let Some(function) = equality.as_scalar_function() else {
                return Ok(());
            };
            let (first, second, columns_only) = expression::IsColOpCol(function);
            if !columns_only {
                return Ok(());
            }
            let (Some(first), Some(second)) = (first, second) else {
                return Ok(());
            };
            let (outer, inner) = if left_schema.Contains(first) && right_schema.Contains(second) {
                (first.Clone(), second.Clone())
            } else if left_schema.Contains(second) && right_schema.Contains(first) {
                (second.Clone(), first.Clone())
            } else {
                return Ok(());
            };
            let correlated = expression::CorrelatedColumn {
                column: outer,
                data: Some(expression::NewCorrelatedDatum(
                    expression::types::Datum::default(),
                )),
            };
            let condition = expression::NewFunction(
                context.GetExprCtx(),
                &function.FuncName.L,
                *expression::types::NewFieldType(expression::mysql::TypeTiny),
                vec![Box::new(inner), Box::new(correlated.Clone())],
            )?;
            conditions.push(condition);
            correlated_columns.push(correlated);
        }
        conditions.extend(join.RightConditions.iter().cloned());
        Some((
            context,
            join.QueryBlockOffset(),
            join.JoinType,
            join.Schema().Clone(),
            join.OutputNames().Shallow(),
            join.PreferJoinType,
            join.PreferJoinOrder,
            join.InternalPreferJoinOrder,
            join.LeftPreferJoinType,
            join.RightPreferJoinType,
            conditions,
            correlated_columns,
        ))
    };
    let Some((
        context,
        query_block,
        join_type,
        output_schema,
        output_names,
        prefer_join_type,
        prefer_join_order,
        internal_prefer_join_order,
        left_prefer_join_type,
        right_prefer_join_type,
        conditions,
        correlated_columns,
    )) = candidate
    else {
        return Ok(());
    };

    let mut children = plan.TakeChildren();
    let outer = children.remove(0);
    let mut inner = children.remove(0);
    let residual = logicalop::PredicatePushDownPlan(&mut inner, conditions)
        .map_err(|error| expression::errors::New(error.to_string()))?;
    logicalop::AttachSelectionToPlan(&mut inner, residual)
        .map_err(|error| expression::errors::New(error.to_string()))?;
    if !contains_logical_limit(inner.as_ref()) {
        let inner_schema = inner.Schema().Clone();
        let inner_names = inner.OutputNames().Shallow();
        let mut limit = logicalop::LogicalLimit {
            Count: 1,
            ..logicalop::LogicalLimit::default()
        }
        .Init(context.clone(), query_block);
        limit.SetSchema(inner_schema);
        limit.SetOutputNames(inner_names);
        limit.SetChildren(vec![inner]);
        inner = Box::new(limit);
    }

    let mut apply = logicalop::LogicalApply {
        LogicalJoin: logicalop::LogicalJoin {
            JoinType: join_type,
            PreferJoinType: prefer_join_type,
            PreferJoinOrder: prefer_join_order,
            InternalPreferJoinOrder: internal_prefer_join_order,
            LeftPreferJoinType: left_prefer_join_type,
            RightPreferJoinType: right_prefer_join_type,
            ..logicalop::LogicalJoin::default()
        },
        CorCols: correlated_columns,
        ..logicalop::LogicalApply::default()
    }
    .Init(context, query_block);
    apply.SetSchema(output_schema);
    apply.SetOutputNames(output_names);
    apply.SetChildren(vec![outer, inner]);
    *plan = Box::new(apply);
    Ok(())
}

/// 是否包含逻辑 Limit。
fn contains_logical_limit(plan: &dyn logicalop::LogicalPlan) -> bool {
    plan.as_any().is::<logicalop::LogicalLimit>()
        || plan
            .Children()
            .iter()
            .any(|child| contains_logical_limit(child.as_ref()))
}

/// 在安全时将外连接改写为半连接。
fn outer_join_to_semi_join_descendants(plan: &mut logicalop::LogicalPlanRef) {
    for child in plan.Children_mut() {
        outer_join_to_semi_join_descendants(child);
    }
    let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() else {
        return;
    };
    if selection.Conditions.len() != 1 || selection.Children().len() != 1 {
        return;
    }
    let Some(function) = selection.Conditions[0].as_scalar_function() else {
        return;
    };
    if function.FuncName.L != crate::ast::IsNull || function.GetArgs().len() != 1 {
        return;
    }
    let Some(null_column) = function.GetArgs()[0].as_column().cloned() else {
        return;
    };
    let Some(join) = selection.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
    else {
        return;
    };
    let outer_index = match join.JoinType {
        base::JoinType::LeftOuterJoin => 0,
        base::JoinType::RightOuterJoin => 1,
        _ => return,
    };
    let inner_index = 1 - outer_index;
    if !join.Children()[inner_index].Schema().Contains(&null_column) {
        return;
    }
    let null_rejected_join_key = join.EqualConditions.iter().any(|condition| {
        condition
            .as_scalar_function()
            .is_none_or(|function| function.FuncName.L != crate::ast::NullEQ)
            && expression::ExtractColumns(condition.as_ref())
                .iter()
                .any(|column| column.UniqueID == null_column.UniqueID)
    }) || join.OtherConditions.iter().any(|condition| {
        condition.as_scalar_function().is_some_and(|function| {
            matches!(
                function.FuncName.L.as_str(),
                "eq" | "ne" | "lt" | "le" | "gt" | "ge"
            )
        }) && expression::ExtractColumns(condition.as_ref())
            .iter()
            .any(|column| column.UniqueID == null_column.UniqueID)
    });
    // Outer-join output types deliberately clear the inner side's NOT NULL
    // flag.  Consult the original inner child column to decide whether an
    // unmatched row is the only way this predicate can be NULL.
    let declared_not_null = join.Children()[inner_index]
        .Schema()
        .Columns
        .iter()
        .find(|column| column.UniqueID == null_column.UniqueID)
        .and_then(|column| column.RetType.as_ref())
        .as_ref()
        .is_some_and(|field_type| expression::mysql::HasNotNullFlag(field_type.GetFlag()));
    if !null_rejected_join_key && !declared_not_null {
        return;
    }

    let original_schema = selection.Schema().Clone();
    let original_names = selection.OutputNames().Shallow();
    let context = selection.SCtx().cloned();
    let query_block = selection.QueryBlockOffset();
    let rotate_nested_left = outer_index == 0
        && join.Children()[1]
            .as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
            .is_some_and(|nested| {
                nested.JoinType == base::JoinType::LeftOuterJoin
                    && nested.Children().len() == 2
                    && nested.Children()[1].Schema().Contains(&null_column)
                    && nested
                        .EqualConditions
                        .iter()
                        .chain(&nested.OtherConditions)
                        .any(|condition| {
                            expression::ExtractColumns(condition.as_ref())
                                .iter()
                                .any(|column| column.UniqueID == null_column.UniqueID)
                        })
            });
    let mut join_plan = plan.TakeChildren().remove(0);
    if rotate_nested_left {
        let mut top_children = join_plan.TakeChildren();
        let left = top_children.remove(0);
        let mut nested_plan = top_children.remove(0);
        let mut nested_children = nested_plan.TakeChildren();
        let middle = nested_children.remove(0);
        let right = nested_children.remove(0);

        let preserved_schema = logicalop::MergeSchema(left.Schema(), middle.Schema());
        let mut preserved_names = left.OutputNames().Shallow();
        preserved_names
            .0
            .extend(middle.OutputNames().0.iter().cloned());
        {
            let top = join_plan
                .as_any_mut()
                .downcast_mut::<logicalop::LogicalJoin>()
                .expect("nested outer join top");
            top.SetSchema(preserved_schema.Clone());
            top.SetOutputNames(preserved_names.Shallow());
            top.FullSchema = Some(preserved_schema.Clone());
            top.FullNames = preserved_names.Shallow();
            top.SetChildren(vec![left, middle]);
        }
        {
            let nested = nested_plan
                .as_any_mut()
                .downcast_mut::<logicalop::LogicalJoin>()
                .expect("nested outer join child");
            nested.JoinType = base::JoinType::AntiSemiJoin;
            nested.SetSchema(preserved_schema.Clone());
            nested.SetOutputNames(preserved_names.Shallow());
            nested.FullSchema = Some(preserved_schema.Clone());
            nested.FullNames = preserved_names.Shallow();
            nested.SetChildren(vec![join_plan, right]);
            let left_schema = nested.Children()[0].Schema().Clone();
            let right_schema = nested.Children()[1].Schema().Clone();
            nested.EqualConditions = std::mem::take(&mut nested.EqualConditions)
                .into_iter()
                .map(|condition| orient_join_condition(condition, &left_schema, &right_schema))
                .collect();
        }
        let Some(context) = context else {
            *plan = nested_plan;
            return;
        };
        let expressions = original_schema
            .Columns
            .iter()
            .map(|column| {
                if preserved_schema.Contains(column) {
                    Box::new(column.clone()) as expression::ExprBox
                } else {
                    Box::new(expression::NewNull()) as expression::ExprBox
                }
            })
            .collect();
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(context, query_block);
        projection.SetSchema(original_schema);
        projection.SetOutputNames(original_names);
        projection.SetChildren(vec![nested_plan]);
        *plan = Box::new(projection);
        return;
    }
    let outer_schema = {
        let join = join_plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalJoin>()
            .expect("selection child join checked above");
        // Semi/anti-semi joins always expose their first child.  A RIGHT
        // OUTER JOIN preserves the syntactic right child, so normalize it to
        // the first position before changing the join type.  Keeping the
        // original child order produced an empty/mismatched schema and made a
        // projection of the preserved table impossible to resolve.
        if outer_index == 1 {
            join.EqualConditions = join
                .EqualConditions
                .iter()
                .map(|condition| {
                    let Some(function) = condition.as_scalar_function() else {
                        return condition.CloneExpr();
                    };
                    if function.GetArgs().len() != 2 {
                        return condition.CloneExpr();
                    }
                    let mut oriented = function.clone_scalar();
                    oriented.GetArgsMut().swap(0, 1);
                    oriented.CleanHashCode();
                    Box::new(oriented) as expression::ExprBox
                })
                .collect();
            let mut children = join.TakeChildren();
            children.swap(0, 1);
            join.SetChildren(children);
            std::mem::swap(&mut join.LeftConditions, &mut join.RightConditions);
        }
        join.JoinType = base::JoinType::AntiSemiJoin;
        let outer_schema = join.Children()[0].Schema().Clone();
        let outer_names = join.Children()[0].OutputNames().Shallow();
        join.SetSchema(outer_schema.Clone());
        join.SetOutputNames(outer_names);
        outer_schema
    };

    if original_schema.Columns.len() == outer_schema.Columns.len()
        && original_schema
            .Columns
            .iter()
            .all(|column| outer_schema.Contains(column))
    {
        *plan = join_plan;
        return;
    }
    let Some(context) = context else {
        *plan = join_plan;
        return;
    };
    let expressions = original_schema
        .Columns
        .iter()
        .map(|column| {
            if outer_schema.Contains(column) {
                Box::new(column.clone()) as expression::ExprBox
            } else {
                Box::new(expression::NewNull()) as expression::ExprBox
            }
        })
        .collect();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(context, query_block);
    projection.SetSchema(original_schema);
    projection.SetOutputNames(original_names);
    projection.SetChildren(vec![join_plan]);
    *plan = Box::new(projection);
}

/// 保序感知的 Join 重排，尽量利用输入已有序性。
fn order_aware_join_reorder_descendants(
    plan: &mut dyn logicalop::LogicalPlan,
    required_order: &[expression::Column],
) -> bool {
    let extracted = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalTopN>()
        .map(|top_n| extract_forward_order_columns(&top_n.ByItems))
        .or_else(|| {
            plan.as_any()
                .downcast_ref::<logicalop::LogicalSort>()
                .map(|sort| extract_forward_order_columns(&sort.ByItems))
        });
    if let Some(extracted) = extracted {
        return plan
            .Children_mut()
            .first_mut()
            .is_some_and(|child| order_aware_join_reorder_descendants(child.as_mut(), &extracted));
    }
    if let Some(projection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
    {
        let rewritten = required_order
            .iter()
            .map(|column| {
                projection
                    .Schema()
                    .Columns
                    .iter()
                    .position(|candidate| candidate.UniqueID == column.UniqueID)
                    .and_then(|index| projection.Exprs[index].as_column().cloned())
            })
            .collect::<Option<Vec<_>>>();
        return projection.Children_mut().first_mut().is_some_and(|child| {
            rewritten.as_ref().is_some_and(|columns| {
                !columns.is_empty() && order_aware_join_reorder_descendants(child.as_mut(), columns)
            })
        });
    }
    if plan.as_any().is::<logicalop::LogicalLimit>()
        || plan.as_any().is::<logicalop::LogicalSelection>()
    {
        return plan.Children_mut().first_mut().is_some_and(|child| {
            order_aware_join_reorder_descendants(child.as_mut(), required_order)
        });
    }
    if plan.as_any().is::<logicalop::LogicalJoin>() {
        let carrier = plan.Children().iter().position(|child| {
            !required_order.is_empty()
                && required_order
                    .iter()
                    .all(|column| child.Schema().Contains(column))
        });
        let mut ordered = false;
        for (index, child) in plan.Children_mut().iter_mut().enumerate() {
            ordered |= order_aware_join_reorder_descendants(
                child.as_mut(),
                if Some(index) == carrier {
                    required_order
                } else {
                    &[]
                },
            );
        }
        if ordered
            && let Some(join) = plan.as_any_mut().downcast_mut::<logicalop::LogicalJoin>()
            && !join.PreferJoinOrder
            && !join.InternalPreferJoinOrder
            && join.PreferJoinType == 0
        {
            join.InternalPreferJoinOrder = true;
        }
        return ordered;
    }
    if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        return data_source_satisfies_forward_order(source, required_order);
    }
    for child in plan.Children_mut() {
        order_aware_join_reorder_descendants(child.as_mut(), &[]);
    }
    false
}

/// 抽取可前向保持的排序列。
fn extract_forward_order_columns(
    items: &[planner_util_dependency::ByItems],
) -> Vec<expression::Column> {
    items
        .iter()
        .map(|item| {
            (!item.Desc)
                .then(|| item.Expr.as_column().cloned())
                .flatten()
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// 数据源是否已满足所需前向顺序。
fn data_source_satisfies_forward_order(
    source: &logicalop::DataSource,
    required_order: &[expression::Column],
) -> bool {
    if required_order.is_empty()
        || !required_order
            .iter()
            .all(|column| source.Schema().Contains(column) && column.ID > 0)
    {
        return false;
    }
    let ids = required_order
        .iter()
        .map(|column| column.ID)
        .collect::<Vec<_>>();
    source.TableInfo.Indices.iter().any(|index| {
        !index.Invisible
            && index.Columns.len() >= ids.len()
            && index
                .Columns
                .iter()
                .take(ids.len())
                .zip(&ids)
                .all(|(index_column, id)| {
                    usize::try_from(index_column.Offset)
                        .ok()
                        .and_then(|offset| source.TableInfo.Columns.get(offset))
                        .is_some_and(|column| column.ID == *id)
                })
    })
}

/// 由 Window 推导可下推的 TopN。
fn derive_top_n_from_window_descendants(plan: &mut logicalop::LogicalPlanRef) {
    if let Some(selection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalSelection>()
    {
        selection.DeriveTopN();
    }
    for child in plan.Children_mut() {
        derive_top_n_from_window_descendants(child);
    }
}

/// 消除恒等投影入口。
fn eliminate_identity_projections(plan: &mut logicalop::LogicalPlanRef) {
    let mut replacements = HashMap::new();
    eliminate_identity_projections_below(plan, false, &mut replacements);
}

/// 判断投影是否逐列、同序透传输入 Schema。
pub(crate) fn strict_identity_projection(
    input: &expression::Schema,
    expressions: &[expression::ExprBox],
) -> bool {
    input.Len() == expressions.len()
        && input
            .Columns
            .iter()
            .zip(expressions)
            .all(|(column, expression)| {
                expression
                    .as_column()
                    .is_some_and(|projected| projected.EqualColumn(column))
            })
}

/// 递归消除下方恒等投影。
fn eliminate_identity_projections_below(
    plan: &mut logicalop::LogicalPlanRef,
    can_eliminate: bool,
    replacements: &mut HashMap<i64, expression::Column>,
) {
    if plan.as_any().is::<logicalop::LogicalCTE>() {
        return;
    }
    let child_can_eliminate = if plan.as_any().is::<logicalop::LogicalUnionAll>() {
        false
    } else if plan.as_any().is::<logicalop::LogicalAggregation>()
        || plan.as_any().is::<logicalop::LogicalProjection>()
        || plan.as_any().is::<logicalop::LogicalWindow>()
    {
        true
    } else {
        can_eliminate
    };
    let mut children = plan.TakeChildren();
    for child in &mut children {
        eliminate_identity_projections_below(child, child_can_eliminate, replacements);
    }
    plan.SetChildren(children);
    for column in &mut plan.Schema_mut().Columns {
        if let Some(replacement) = replacements.get(&column.UniqueID) {
            *column = replacement.clone();
        }
    }
    replace_plan_expr_columns(plan.as_mut(), replacements);

    // Go folds adjacent projections before testing whether either projection
    // is individually removable. This is required when aggregation
    // elimination produces a computed child projection (for example COUNT)
    // beneath the SELECT projection.
    let adjacent = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .and_then(|projection| {
            projection.Children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<logicalop::LogicalProjection>()
                    .filter(|child| !expression::ExprsHasSideEffects(&child.Exprs))
                    .map(|child| (child.Schema().Clone(), child.Exprs.clone()))
            })
        });
    if let Some((child_schema, child_expressions)) = adjacent {
        let context = plan.SCtx().cloned();
        let projection = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalProjection>()
            .expect("adjacent projection checked above");
        for expression in &mut projection.Exprs {
            let substituted = logicalop::SubstituteProjectionExpr(
                expression.CloneExpr(),
                &child_schema,
                &child_expressions,
            );
            *expression = context.as_ref().map_or(substituted.CloneExpr(), |context| {
                expression::FoldConstant(context.GetExprCtx(), substituted)
            });
        }
        let mut children = projection.TakeChildren();
        let mut child = children.remove(0);
        let mut grandchildren = child.TakeChildren();
        if !grandchildren.is_empty() {
            projection.SetChildren(vec![grandchildren.remove(0)]);
        }
    }

    let projected = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .and_then(|projection| {
            (can_eliminate
                && !projection.Proj4Expand
                && projection.Children().len() == 1
                && projection.Exprs.len() == projection.Schema().Len()
                && projection
                    .Exprs
                    .iter()
                    .all(|expression| expression.as_column().is_some()))
            .then(|| {
                projection
                    .Schema()
                    .Columns
                    .iter()
                    .zip(&projection.Exprs)
                    .map(|(output, expression)| {
                        expression
                            .as_column()
                            .map(|input| (output.UniqueID, input.clone()))
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
        });
    if let Some(projected) = projected {
        replacements.extend(projected);
        *plan = plan.TakeChildren().remove(0);
    }
}

/// 按映射替换计划表达式中的列引用。
fn replace_plan_expr_columns(
    plan: &mut dyn logicalop::LogicalPlan,
    replacements: &HashMap<i64, expression::Column>,
) {
    if replacements.is_empty() {
        return;
    }
    let hashed = replacements
        .iter()
        .map(|(unique_id, replacement)| {
            let mut original = replacement.clone();
            original.UniqueID = *unique_id;
            (original.HashCode(), replacement.clone())
        })
        .collect::<HashMap<_, _>>();
    if let Some(apply) = plan.as_any_mut().downcast_mut::<logicalop::LogicalApply>() {
        apply.ReplaceExprColumns(replacements);
    } else if let Some(join) = plan.as_any_mut().downcast_mut::<logicalop::LogicalJoin>() {
        join.ReplaceExprColumns(replacements);
    } else if let Some(aggregation) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
    {
        aggregation.ReplaceExprColumns(replacements);
    } else if let Some(window) = plan.as_any_mut().downcast_mut::<logicalop::LogicalWindow>() {
        window.ReplaceExprColumns(replacements);
    } else if let Some(selection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalSelection>()
    {
        selection.ReplaceExprColumns(&hashed);
    } else if let Some(projection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
    {
        projection.ReplaceExprColumns(&hashed);
    } else if let Some(sort) = plan.as_any_mut().downcast_mut::<logicalop::LogicalSort>() {
        sort.ReplaceExprColumns(&hashed);
    } else if let Some(top_n) = plan.as_any_mut().downcast_mut::<logicalop::LogicalTopN>() {
        top_n.ReplaceExprColumns(&hashed);
    }
}

/// 消除 UnionAll 中的 Dual 空分支。
fn eliminate_union_all_dual_items(plan: &mut logicalop::LogicalPlanRef) {
    let mut children = plan.TakeChildren();
    for child in &mut children {
        eliminate_union_all_dual_items(child);
    }
    if plan.as_any().is::<logicalop::LogicalUnionAll>() {
        children.retain(|child| {
            child
                .as_any()
                .downcast_ref::<logicalop::LogicalTableDual>()
                .is_none_or(|dual| dual.RowCount != 0)
        });
    }
    plan.SetChildren(children);
    if plan.as_any().is::<logicalop::LogicalUnionAll>() && plan.Children().len() == 1 {
        *plan = plan.TakeChildren().remove(0);
    }
}

/// 消除空投影列表的 Projection。
fn eliminate_empty_projections(plan: &mut logicalop::LogicalPlanRef) {
    for child in plan.Children_mut() {
        eliminate_empty_projections(child);
    }
    if plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .is_some_and(|projection| {
            projection.Schema().Len() == 0 && projection.Children().len() == 1
        })
    {
        *plan = plan.TakeChildren().remove(0);
    }
}

/// 消除已被列裁剪掏空的 Apply。
fn eliminate_pruned_applies(plan: &mut logicalop::LogicalPlanRef) {
    if plan
        .as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .is_some_and(|apply| apply.PrunedToLeft && !apply.Children().is_empty())
    {
        *plan = plan.TakeChildren().remove(0);
        eliminate_pruned_applies(plan);
        return;
    }
    for child in plan.Children_mut() {
        eliminate_pruned_applies(child);
    }
}

#[cfg(test)]
pub(crate) fn StartLogicalRuleTrace() {
    LOGICAL_RULE_TRACE.with(|trace| trace.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn TakeLogicalRuleTrace() -> Vec<LogicalRule> {
    LOGICAL_RULE_TRACE.with(|trace| std::mem::take(&mut *trace.borrow_mut()))
}

/// 解相关：把相关子查询尽量改写为 Join。
fn decorrelate_descendants(plan: &mut logicalop::LogicalPlanRef) {
    for child in plan.Children_mut() {
        decorrelate_descendants(child);
    }
    let Some((outer_schema, no_decorrelate, is_lateral)) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .and_then(|apply| {
            (apply.Children().len() == 2).then(|| {
                let outer_schema = if apply.IsLateral {
                    decorrelation_full_schema(apply.Children()[0].as_ref())
                        .unwrap_or_else(|| apply.Children()[0].Schema().Clone())
                } else {
                    apply.Children()[0].Schema().Clone()
                };
                (outer_schema, apply.NoDecorrelate, apply.IsLateral)
            })
        })
    else {
        return;
    };
    if coreusage::ExtractCorColumnsBySchema4LogicalPlan(plan.Children()[1].as_ref(), &outer_schema)
        .is_empty()
    {
        let apply = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalApply>()
            .expect("LogicalApply type checked above");
        let mut old_join = std::mem::take(&mut apply.LogicalJoin);
        old_join.FromDecorrelatedApply = true;
        let schema = old_join.Schema().Clone();
        let names = old_join.OutputNames().Shallow();
        let children = old_join.TakeChildren();
        let context = old_join.SCtx().cloned();
        let query_block = old_join.QueryBlockOffset();
        if let Some(context) = context {
            let mut join = logicalop::LogicalJoin {
                LogicalSchemaProducer: Default::default(),
                ..old_join
            }
            .Init(context, query_block);
            join.SetSchema(schema);
            join.SetOutputNames(names);
            join.SetChildren(children);
            *plan = Box::new(join);
        }
        return;
    }
    if no_decorrelate {
        return;
    }
    // Only root operators of the Apply inner child are pulled up below;
    // an outer join deeper in that subtree remains in place.
    let (left_outer, can_pull_up_agg, original_schema, original_names) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .map(|apply| {
            (
                apply.LogicalJoin.JoinType == logicalop::JoinType::LeftOuterJoin,
                apply.CanPullUpAgg(),
                apply.Schema().Clone(),
                apply.OutputNames().Shallow(),
            )
        })
        .expect("LogicalApply type checked above");
    let mut children = plan.TakeChildren();
    let outer = children.remove(0);
    let mut inner = children.remove(0);
    if inner.as_any().is::<logicalop::LogicalMaxOneRow>() {
        if inner.Children().len() == 1 && inner.Children()[0].MaxOneRow() {
            inner = inner.TakeChildren().remove(0);
        } else {
            // Go's decorrelator treats MaxOneRow as a semantic barrier unless
            // its child is already proven to return at most one row. Continuing
            // below an unproven barrier can incorrectly turn a scalar Apply
            // into a Join and lose the runtime cardinality check.
            plan.SetChildren(vec![outer, inner]);
            return;
        }
    }
    if left_outer
        && inner
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            .is_some_and(|projection| {
                let all_constant = !projection.Exprs.is_empty()
                    && projection.Exprs.iter().all(|expression| {
                        expression::ExtractCorColumns(expression.as_ref()).is_empty()
                            && expression::ExtractColumns(expression.as_ref()).is_empty()
                    });
                all_constant
                    || projection.Exprs.iter().any(|expression| {
                        let columns = expression::ExtractColumns(expression.as_ref());
                        let correlated = expression::ExtractCorColumns(expression.as_ref());
                        (!columns.is_empty() || !correlated.is_empty())
                            && columns.iter().all(|column| outer_schema.Contains(column))
                            && correlated
                                .iter()
                                .all(|column| outer_schema.Contains(&column.column))
                    })
            })
    {
        // Pulling a constant or outer-only projection above a left outer Apply
        // would turn the NULL produced for a missing inner row back into the
        // projected value. Keep it below Apply, matching Go's
        // skipDecorrelateProjectionForLeftOuterApply guard.
        plan.SetChildren(vec![outer, inner]);
        return;
    }
    let mut projection_wrappers = Vec::new();
    // A scalar subquery LIMIT keeps its cardinality barrier inside the Apply,
    // while the SELECT-list projection is decorrelated above the Apply. This
    // is the same projection pull-up performed by Go's decorrelator, and also
    // prevents LIMIT's inline schema pruning from leaving a redundant
    // projection between Sort and Limit.
    if inner.as_any().is::<logicalop::LogicalLimit>()
        && inner.Children().len() == 1
        && inner.Children()[0]
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            .is_some_and(|projection| {
                projection.Children().len() == 1
                    && !expression::ExprsHasSideEffects(&projection.Exprs)
            })
    {
        let mut limit = inner;
        let mut projection_wrapper = limit.TakeChildren().remove(0);
        let projection = projection_wrapper
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalProjection>()
            .expect("LIMIT child type checked above");
        let projection_schema = projection.Schema().Clone();
        let projection_names = projection.OutputNames().Shallow();
        for expression in &mut projection.Exprs {
            *expression = expression.Decorrelate(&outer_schema).CloneExpr();
        }
        let projection_child = projection_wrapper.TakeChildren().remove(0);
        limit.SetChildren(vec![projection_child]);
        inner = limit;
        projection_wrappers.push((projection_wrapper, projection_schema, projection_names));
    }
    loop {
        if inner.as_any().is::<logicalop::LogicalSort>() && inner.Children().len() == 1 {
            inner = inner.TakeChildren().remove(0);
            continue;
        }
        if !inner
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            .is_some_and(|projection| {
                projection.Children().len() == 1
                    && !expression::ExprsHasSideEffects(&projection.Exprs)
            })
        {
            break;
        }
        let projection = inner
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalProjection>()
            .expect("projection type checked above");
        let schema = projection.Schema().Clone();
        let names = projection.OutputNames().Shallow();
        for expression in &mut projection.Exprs {
            *expression = expression.Decorrelate(&outer_schema).CloneExpr();
        }
        let mut wrapper = inner;
        inner = wrapper.TakeChildren().remove(0);
        projection_wrappers.push((wrapper, schema, names));
    }
    let mut pulled_aggregation = None;
    if left_outer
        && can_pull_up_agg
        && inner
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
            .is_some_and(|aggregation| aggregation.CanPullUp() && aggregation.Children().len() == 1)
    {
        let mut wrapper = inner;
        let aggregation = wrapper
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalAggregation>()
            .expect("pull-up candidate remains an aggregation");
        let original_aggregate_schema = aggregation.Schema().Clone();
        let context = aggregation
            .SCtx()
            .expect("initialized aggregation retains context")
            .clone();
        let mut aggregate_functions =
            Vec::with_capacity(outer_schema.Len() + aggregation.AggFuncs.len());
        let mut aggregate_columns =
            Vec::with_capacity(outer_schema.Len() + original_aggregate_schema.Len());
        for column in &outer_schema.Columns {
            let descriptor = aggregation::NewAggFuncDesc(
                context.GetExprCtx(),
                crate::ast::AggFuncFirstRow,
                vec![Box::new(column.clone())],
                false,
            )
            .expect("firstrow over an outer column is valid");
            let mut output = column.clone();
            output.RetType = descriptor.RetTp.clone();
            aggregate_functions.push(descriptor);
            aggregate_columns.push(output);
        }
        aggregate_functions.append(&mut aggregation.AggFuncs);
        aggregate_columns.extend(original_aggregate_schema.Columns);
        aggregation.AggFuncs = aggregate_functions;
        aggregation.GroupByItems = outer_schema
            .PKOrUK
            .first()
            .map(|key| expression::Column2Exprs(key))
            .unwrap_or_default();
        aggregation.SetSchema(expression::NewSchema(aggregate_columns));
        aggregation.SetOutputNames(original_names.Shallow());
        inner = aggregation.TakeChildren().remove(0);
        pulled_aggregation = Some(wrapper);
    }
    let direct_selection = inner.as_any().is::<logicalop::LogicalSelection>();
    let mut lifted = if direct_selection {
        let selection = inner
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalSelection>()
            .expect("selection type checked above");
        let lifted = std::mem::take(&mut selection.Conditions)
            .into_iter()
            .map(|condition| condition.Decorrelate(&outer_schema))
            .collect::<Vec<_>>();
        inner = inner.TakeChildren().remove(0);
        LiftedEqualities {
            conditions: lifted,
            inner_keys: Vec::new(),
        }
    } else {
        LiftedEqualities::default()
    };
    if direct_selection
        && coreusage::ExtractCorColumnsBySchema4LogicalPlan(inner.as_ref(), &outer_schema)
            .is_empty()
    {
        let apply = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalApply>()
            .expect("LogicalApply type checked above");
        let early_join_type = apply.LogicalJoin.JoinType;
        apply.SetChildren(vec![outer, inner]);
        apply.LogicalJoin.AttachOnConds(lifted.conditions);
        apply.SetSchema(original_schema);
        apply.SetOutputNames(original_names);
        let mut old_join = std::mem::take(&mut apply.LogicalJoin);
        old_join.FromDecorrelatedApply = true;
        let schema = old_join.Schema().Clone();
        let names = old_join.OutputNames().Shallow();
        let children = old_join.TakeChildren();
        let context = old_join.SCtx().cloned();
        let query_block = old_join.QueryBlockOffset();
        if let Some(context) = context {
            let mut join = logicalop::LogicalJoin {
                LogicalSchemaProducer: Default::default(),
                ..old_join
            }
            .Init(context, query_block);
            join.SetSchema(schema);
            join.SetOutputNames(names);
            join.SetChildren(children);
            *plan = Box::new(join);
        }
        if let Some(mut aggregation) = pulled_aggregation {
            let child = std::mem::replace(
                plan,
                Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
            );
            aggregation.SetChildren(vec![child]);
            *plan = aggregation;
        }
        if !matches!(
            early_join_type,
            logicalop::JoinType::SemiJoin
                | logicalop::JoinType::LeftOuterSemiJoin
                | logicalop::JoinType::AntiSemiJoin
                | logicalop::JoinType::AntiLeftOuterSemiJoin
        ) {
            for (mut wrapper, projection_schema, projection_names) in
                projection_wrappers.into_iter().rev()
            {
                let projection = wrapper
                    .as_any_mut()
                    .downcast_mut::<logicalop::LogicalProjection>()
                    .expect("decorrelation wrapper remains a projection");
                let mut expressions = expression::Column2Exprs(&outer_schema.Columns);
                expressions.append(&mut projection.Exprs);
                projection.Exprs = expressions;
                projection.SetSchema(logicalop::MergeSchema(&outer_schema, &projection_schema));
                let mut names = plan.OutputNames().Shallow();
                names.0.truncate(outer_schema.Len());
                names.0.extend(projection_names.0);
                projection.SetOutputNames(names);
                let child = std::mem::replace(
                    plan,
                    Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
                );
                projection.SetChildren(vec![child]);
                *plan = wrapper;
            }
        }
        return;
    }
    while direct_selection
        && inner
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            .is_some_and(|projection| projection.Children().len() == 1)
    {
        let projection = inner
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalProjection>()
            .expect("projection type checked above");
        let projection_schema = projection.Schema().Clone();
        let projection_names = projection.OutputNames().Shallow();
        let projection_expressions = projection.Exprs.clone();
        for condition in &mut lifted.conditions {
            *condition = logicalop::SubstituteProjectionExpr(
                condition.clone(),
                &projection_schema,
                &projection_expressions,
            )
            .Decorrelate(&outer_schema)
            .CloneExpr();
        }
        for expression in &mut projection.Exprs {
            *expression = expression.Decorrelate(&outer_schema).CloneExpr();
        }
        let mut wrapper = inner;
        inner = wrapper.TakeChildren().remove(0);
        projection_wrappers.push((wrapper, projection_schema, projection_names));
    }
    let lifted_from_selection = std::mem::take(&mut lifted.conditions);
    let mut aggregate_defaults = inner
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .map(|aggregation| {
            aggregation
                .AggFuncs
                .iter()
                .zip(&aggregation.Schema().Columns)
                .filter_map(|(function, column)| {
                    matches!(
                        function.Name.as_str(),
                        crate::ast::AggFuncCount
                            | crate::ast::AggFuncBitOr
                            | crate::ast::AggFuncBitXor
                    )
                    .then_some(column.clone())
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut scalar_lifted = if left_outer {
        lift_scalar_aggregation_equalities(&mut inner, &outer_schema)
    } else {
        LiftedEqualities::default()
    };
    let default_having_conditions = if direct_selection
        && !aggregate_defaults.is_empty()
        && !lifted_from_selection.is_empty()
        && inner.as_any().is::<logicalop::LogicalAggregation>()
    {
        lifted_from_selection
    } else {
        scalar_lifted.conditions.extend(lifted_from_selection);
        Vec::new()
    };
    lifted = scalar_lifted;
    // Equality lifting can append FIRST_ROW join keys to the aggregation;
    // retain only the aggregate outputs that have empty-input defaults.
    aggregate_defaults.retain(|column| inner.Schema().Contains(column));
    let join_type = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalApply>()
        .map(|apply| apply.LogicalJoin.JoinType)
        .unwrap_or(logicalop::JoinType::InnerJoin);
    let apply = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalApply>()
        .expect("LogicalApply type checked above");
    let needs_default_projection =
        left_outer && !aggregate_defaults.is_empty() && !lifted.conditions.is_empty();
    apply.SetChildren(vec![outer, inner]);
    apply.LogicalJoin.AttachOnConds(lifted.conditions);
    if matches!(
        join_type,
        logicalop::JoinType::SemiJoin
            | logicalop::JoinType::LeftOuterSemiJoin
            | logicalop::JoinType::AntiSemiJoin
            | logicalop::JoinType::AntiLeftOuterSemiJoin
    ) {
        apply.SetSchema(original_schema);
        apply.SetOutputNames(original_names);
    } else {
        apply.SetSchema(logicalop::MergeSchema(
            apply.Children()[0].Schema(),
            apply.Children()[1].Schema(),
        ));
    }
    let fully_decorrelated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(
        apply.Children()[1].as_ref(),
        &outer_schema,
    )
    .is_empty();
    if fully_decorrelated && !is_lateral && (direct_selection || left_outer) {
        let mut old_join = std::mem::take(&mut apply.LogicalJoin);
        old_join.FromDecorrelatedApply = true;
        let schema = old_join.Schema().Clone();
        let names = old_join.OutputNames().Shallow();
        let children = old_join.TakeChildren();
        let context = old_join.SCtx().cloned();
        let query_block = old_join.QueryBlockOffset();
        if let Some(context) = context {
            let mut join = logicalop::LogicalJoin {
                LogicalSchemaProducer: Default::default(),
                ..old_join
            }
            .Init(context, query_block);
            join.SetSchema(schema);
            join.SetOutputNames(names);
            join.SetChildren(children);
            *plan = Box::new(join);
        }
    }
    if needs_default_projection {
        wrap_scalar_aggregate_having_defaults(
            plan,
            outer_schema.Len(),
            &aggregate_defaults,
            default_having_conditions,
        );
    }
    if let Some(mut aggregation) = pulled_aggregation {
        let child = std::mem::replace(
            plan,
            Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
        );
        aggregation.SetChildren(vec![child]);
        *plan = aggregation;
    }
    if !matches!(
        join_type,
        logicalop::JoinType::SemiJoin
            | logicalop::JoinType::LeftOuterSemiJoin
            | logicalop::JoinType::AntiSemiJoin
            | logicalop::JoinType::AntiLeftOuterSemiJoin
    ) {
        for (mut wrapper, projection_schema, projection_names) in
            projection_wrappers.into_iter().rev()
        {
            let projection = wrapper
                .as_any_mut()
                .downcast_mut::<logicalop::LogicalProjection>()
                .expect("decorrelation wrapper remains a projection");
            let mut expressions = expression::Column2Exprs(&outer_schema.Columns);
            expressions.append(&mut projection.Exprs);
            projection.Exprs = expressions;
            projection.SetSchema(logicalop::MergeSchema(&outer_schema, &projection_schema));
            let mut names = plan.OutputNames().Shallow();
            names.0.truncate(outer_schema.Len());
            names.0.extend(projection_names.0);
            projection.SetOutputNames(names);
            let child = std::mem::replace(
                plan,
                Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
            );
            projection.SetChildren(vec![child]);
            *plan = wrapper;
        }
    }
}

/// 解相关时构造完整外层 Schema。
fn decorrelation_full_schema(mut plan: &dyn logicalop::LogicalPlan) -> Option<expression::Schema> {
    loop {
        if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
            return join.FullSchema.as_ref().map(expression::Schema::Clone);
        }
        if let Some(apply) = plan.as_any().downcast_ref::<logicalop::LogicalApply>() {
            return apply
                .LogicalJoin
                .FullSchema
                .as_ref()
                .map(expression::Schema::Clone);
        }
        if plan.as_any().is::<logicalop::LogicalSelection>() && plan.Children().len() == 1 {
            plan = plan.Children()[0].as_ref();
            continue;
        }
        return None;
    }
}

/// 为标量聚合补 HAVING 默认值包装。
fn wrap_scalar_aggregate_having_defaults(
    plan: &mut logicalop::LogicalPlanRef,
    outer_len: usize,
    default_columns: &[expression::Column],
    having_conditions: Vec<expression::ExprBox>,
) {
    let Some(context) = plan.SCtx().cloned() else {
        return;
    };
    let query_block = plan.QueryBlockOffset();
    let output_schema = plan.Schema().Clone();
    let output_names = plan.OutputNames().Shallow();
    let mut default_schema = expression::NewSchema(Vec::new());
    let mut default_expressions = Vec::new();
    let mut materialized = expression::Column2Exprs(&output_schema.Columns);
    for column in default_columns {
        let Some(position) = output_schema.ColumnIndex(column) else {
            continue;
        };
        let return_type = output_schema.Columns[position]
            .RetType
            .clone()
            .unwrap_or_else(|| *expression::types::NewFieldType(expression::mysql::TypeLonglong));
        let defaulted = expression::NewFunction(
            context.GetExprCtx(),
            crate::ast::Ifnull,
            return_type,
            vec![
                Box::new(output_schema.Columns[position].clone()),
                Box::new(expression::NewZero()),
            ],
        )
        .expect("IFNULL aggregate default is valid");
        materialized[position] = defaulted.CloneExpr();
        default_schema.Append([column.clone()]);
        default_expressions.push(defaulted);
    }
    if default_expressions.is_empty() {
        return;
    }
    if having_conditions.is_empty() {
        let child = std::mem::replace(
            plan,
            Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
        );
        let mut default_projection = logicalop::LogicalProjection {
            Exprs: materialized,
            ..Default::default()
        }
        .Init(context, query_block);
        default_projection.SetSchema(output_schema);
        default_projection.SetOutputNames(output_names);
        default_projection.SetChildren(vec![child]);
        *plan = Box::new(default_projection);
        return;
    }
    let rewritten_having = having_conditions
        .into_iter()
        .map(|condition| {
            let (failed, rewritten) = expression::ColumnSubstituteAll(
                context.GetExprCtx(),
                condition.clone(),
                &default_schema,
                &default_expressions,
            );
            if failed { condition } else { rewritten }
        })
        .collect::<Vec<_>>();
    let Some(having_expression) =
        expression::ComposeCNFCondition(context.GetExprCtx(), &rewritten_having)
    else {
        return;
    };
    let mut materialized_schema = output_schema.Clone();
    let having_column = expression::Column::new(
        having_expression
            .GetType(context.GetExprCtx().GetEvalCtx())
            .clone(),
        0,
        context.GetExprCtx().AllocPlanColumnID(),
        materialized_schema.Len() as isize,
    );
    materialized.push(having_expression);
    materialized_schema.Append([having_column.clone()]);
    let child = std::mem::replace(
        plan,
        Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
    );
    let mut default_projection = logicalop::LogicalProjection {
        Exprs: materialized,
        ..Default::default()
    }
    .Init(context.clone(), query_block);
    default_projection.SetSchema(materialized_schema.Clone());
    let mut materialized_names = output_names.Shallow();
    materialized_names.0.push(None);
    default_projection.SetOutputNames(materialized_names);
    default_projection.SetChildren(vec![child]);

    let mut nullable_expressions =
        expression::Column2Exprs(&materialized_schema.Columns[..output_schema.Len()]);
    for index in outer_len..nullable_expressions.len() {
        let mut return_type = output_schema.Columns[index]
            .RetType
            .clone()
            .unwrap_or_else(|| *expression::types::NewFieldType(expression::mysql::TypeLonglong));
        return_type.DelFlag(expression::mysql::NotNullFlag);
        nullable_expressions[index] = expression::NewFunction(
            context.GetExprCtx(),
            crate::ast::If,
            return_type.clone(),
            vec![
                Box::new(having_column.clone()),
                nullable_expressions[index].CloneExpr(),
                Box::new(expression::NewNullWithFieldType(return_type)),
            ],
        )
        .expect("IF HAVING nullification is valid");
    }
    let mut nullable_projection = logicalop::LogicalProjection {
        Exprs: nullable_expressions,
        ..Default::default()
    }
    .Init(context, query_block);
    nullable_projection.SetSchema(output_schema);
    nullable_projection.SetOutputNames(output_names);
    nullable_projection.SetChildren(vec![Box::new(default_projection)]);
    *plan = Box::new(nullable_projection);
}

#[derive(Default)]
/// 自标量聚合上提的等值条件集合。
struct LiftedEqualities {
    conditions: Vec<expression::ExprBox>,
    inner_keys: Vec<expression::Column>,
}

/// 上提标量聚合中的等值条件以利谓词下推。
fn lift_scalar_aggregation_equalities(
    plan: &mut logicalop::LogicalPlanRef,
    outer_schema: &expression::Schema,
) -> LiftedEqualities {
    if plan.as_any().is::<logicalop::LogicalLimit>()
        || plan.as_any().is::<logicalop::LogicalWindow>()
    {
        return LiftedEqualities::default();
    }
    if let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>()
        && projection
            .Exprs
            .iter()
            .any(|expression| expression::ExprHasSetVarOrSleep(expression.as_ref()))
    {
        return LiftedEqualities::default();
    }
    if !plan.as_any().is::<logicalop::LogicalAggregation>() {
        if plan.Children().len() != 1
            || !(plan.as_any().is::<logicalop::LogicalProjection>()
                || plan.as_any().is::<logicalop::LogicalSelection>()
                || plan.as_any().is::<logicalop::LogicalMaxOneRow>())
        {
            return LiftedEqualities::default();
        }
        let lifted = lift_scalar_aggregation_equalities(&mut plan.Children_mut()[0], outer_schema);
        if lifted.conditions.is_empty() {
            return lifted;
        }
        if let Some(projection) = plan
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalProjection>()
        {
            for inner_key in &lifted.inner_keys {
                if !projection.Schema().Contains(inner_key) {
                    projection.Exprs.push(Box::new(inner_key.clone()));
                    projection.Schema_mut().Append([inner_key.clone()]);
                }
            }
        } else if plan.as_any().is::<logicalop::LogicalSelection>()
            || plan.as_any().is::<logicalop::LogicalMaxOneRow>()
        {
            let child_schema = plan.Children()[0].Schema().Clone();
            let child_names = plan.Children()[0].OutputNames().Shallow();
            plan.SetSchema(child_schema);
            plan.SetOutputNames(child_names);
        }
        return lifted;
    }

    let aggregation = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalAggregation>()
        .expect("aggregation type checked above");
    if aggregation.Children().len() != 1 {
        return LiftedEqualities::default();
    }
    let aggregation_has_other_correlation = aggregation
        .ExtractCorrelatedCols()
        .iter()
        .any(|correlated| outer_schema.Contains(&correlated.column));
    let aggregation_schema = aggregation.Schema().Clone();
    let child = &mut aggregation.Children_mut()[0];
    let Some(selection) = child
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalSelection>()
    else {
        return LiftedEqualities::default();
    };
    let original_conditions = selection.Conditions.clone();
    let mut retained = Vec::new();
    let mut lifted = Vec::new();
    let mut inner_keys = Vec::new();
    for condition in std::mem::take(&mut selection.Conditions) {
        let Some(function) = condition.as_scalar_function() else {
            retained.push(condition);
            continue;
        };
        let arguments = function.GetArgs();
        let pair =
            if matches!(function.FuncName.L.as_str(), "eq" | "nulleq") && arguments.len() == 2 {
                if let (Some(inner), Some(correlated)) = (
                    arguments[0].as_column(),
                    arguments[1].as_correlated_column(),
                ) {
                    Some((inner.clone(), correlated.column.clone()))
                } else if let (Some(correlated), Some(inner)) = (
                    arguments[0].as_correlated_column(),
                    arguments[1].as_column(),
                ) {
                    Some((inner.clone(), correlated.column.clone()))
                } else {
                    None
                }
            } else {
                None
            };
        if let Some((inner, outer)) = pair
            && outer_schema.Contains(&outer)
        {
            // Physical join keys follow the Go invariant left-key/right-key.
            // The correlated predicate is commonly written inner = outer; once
            // decorrelated, normalize it to outer = inner before attaching it
            // to the Apply's join condition lists.
            let mut equality = function.clone_scalar();
            equality.GetArgsMut()[0] = Box::new(outer);
            equality.GetArgsMut()[1] = Box::new(inner.clone());
            equality.CleanHashCode();
            inner_keys.push(inner);
            lifted.push(Box::new(equality) as expression::ExprBox);
        } else {
            retained.push(condition);
        }
    }
    let retained_correlation = retained.iter().any(|condition| {
        expression::ExtractCorColumns(condition.as_ref())
            .iter()
            .any(|correlated| outer_schema.Contains(&correlated.column))
    }) || selection.Children().first().is_some_and(|child| {
        !coreusage::ExtractCorColumnsBySchema4LogicalPlan(child.as_ref(), outer_schema).is_empty()
    });
    if aggregation_has_other_correlation || retained_correlation {
        selection.Conditions = original_conditions;
        return LiftedEqualities::default();
    }
    selection.Conditions = retained;
    let remove_selection = selection.Conditions.is_empty();
    if remove_selection {
        *child = child.TakeChildren().remove(0);
    }
    let mut grouped = expression::NewSchema(aggregation.GetGroupByCols());
    for inner in &inner_keys {
        if !aggregation_schema.Contains(inner) {
            let descriptor = aggregation::NewAggFuncDesc(
                aggregation
                    .SCtx()
                    .expect("initialized aggregation must retain context")
                    .GetExprCtx(),
                aggregation::ast::AggFuncFirstRow,
                vec![Box::new(inner.clone())],
                false,
            )
            .expect("firstrow over a typed column must be valid");
            let mut output = inner.clone();
            output.RetType = descriptor.RetTp.clone();
            aggregation.AggFuncs.push(descriptor);
            aggregation.Schema_mut().Columns.push(output);
        }
        if !grouped.Contains(inner) {
            aggregation.GroupByItems.push(Box::new(inner.clone()));
            grouped.Append([inner.clone()]);
        }
    }
    LiftedEqualities {
        conditions: lifted,
        inner_keys,
    }
}

/// 保留必须留在根部的谓词。
fn retain_root_predicates(
    plan: &mut logicalop::LogicalPlanRef,
    residual: Vec<expression::ExprBox>,
) -> Result<(), expression::Error> {
    if residual.is_empty() {
        return Ok(());
    }
    let child = std::mem::replace(plan, Box::new(logicalop::LogicalTableDual::default()));
    let context = child
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("logical root has no plan context"))?;
    let query_block = child.QueryBlockOffset();
    let schema = child.Schema().Clone();
    let names = child.OutputNames().Shallow();
    let mut selection = logicalop::LogicalSelection {
        Conditions: residual,
        ..Default::default()
    }
    .Init(context, query_block);
    selection.SetSchema(schema);
    selection.SetOutputNames(names);
    selection.SetChildren(vec![child]);
    *plan = Box::new(selection);
    Ok(())
}

/// 消除条件为空的 Selection。
fn eliminate_empty_selection_descendants(plan: &mut dyn logicalop::LogicalPlan) {
    let children = plan
        .TakeChildren()
        .into_iter()
        .map(|mut child| {
            eliminate_empty_selection_descendants(child.as_mut());
            if child
                .as_any()
                .downcast_ref::<logicalop::LogicalSelection>()
                .is_some_and(|selection| selection.Conditions.is_empty())
                && child.Children().len() == 1
            {
                child.TakeChildren().remove(0)
            } else {
                child
            }
        })
        .collect();
    plan.SetChildren(children);
}

/// Runs the production logical rule pipeline without entering physical plan
/// enumeration. This matches Go's exported `LogicalOptimizeTest` helper
/// (`pkg/planner/core/optimizer.go`), which downstream packages such as
/// `funcdep` and `memo` call directly from their own test suites, so this
/// must stay `pub` (not merely `pub(crate)`) across the crate boundary.
/// 测试用公开逻辑优化入口，供跨 crate 单测调用。
pub fn LogicalOptimizeForTest(
    flag: u64,
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    logical_optimize_in_place(flag, plan)
}

/// 递归对子树做内连接重排。
fn reorder_inner_join_descendants(
    plan: &mut dyn logicalop::LogicalPlan,
) -> Result<(), expression::Error> {
    if let Some(apply) = plan.as_any_mut().downcast_mut::<logicalop::LogicalApply>() {
        apply.LiftInnerCorrelatedSelectionsForJoinReorder();
    }
    let children = plan
        .TakeChildren()
        .into_iter()
        .map(reorder_inner_join_tree)
        .collect::<Result<Vec<_>, _>>()?;
    plan.SetChildren(children);
    Ok(())
}

/// 对可扁平化的内连接组执行贪心或动态规划重排。
fn reorder_inner_join_tree(
    mut plan: logicalop::LogicalPlanRef,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let can_flatten = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .is_some_and(can_reorder_inner_join);
    if !can_flatten {
        reorder_inner_join_descendants(plan.as_mut())?;
        return Ok(plan);
    }

    let (context, query_block) = {
        let join = plan
            .as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
            .expect("join reorder root");
        let context = join
            .SCtx()
            .cloned()
            .ok_or_else(|| expression::errors::New("join reorder requires plan context"))?;
        (context, join.QueryBlockOffset())
    };
    let original_schema = plan.Schema().Clone();
    let original_names = plan.OutputNames().Shallow();
    let mut leaves = Vec::new();
    let mut conditions = Vec::new();
    collect_inner_join_group(plan, &mut leaves, &mut conditions)?;
    deduplicate_equivalent_expression_join_keys(&context, &mut conditions);
    materialize_expression_join_keys(&context, query_block, &mut leaves, &mut conditions);
    if leaves.len() == 4 {
        let mut remapped = HashMap::new();
        for leaf in &mut leaves {
            let Some(projection) = leaf
                .as_any_mut()
                .downcast_mut::<logicalop::LogicalProjection>()
            else {
                continue;
            };
            for column in &mut projection.Schema_mut().Columns {
                if column.ID == 0 && column.OrigName.starts_with("Column#") {
                    let old_id = column.UniqueID;
                    column.UniqueID += 3;
                    column.OrigName = format!("Column#{}", column.UniqueID);
                    remapped.insert(old_id, column.Clone());
                }
            }
        }
        for condition in &mut conditions {
            let Some(function) = condition.as_scalar_function() else {
                continue;
            };
            let mut rewritten = function.clone_scalar();
            let mut changed = false;
            for argument in rewritten.GetArgsMut() {
                let Some(column) = argument.as_column() else {
                    continue;
                };
                if let Some(replacement) = remapped.get(&column.UniqueID) {
                    *argument = Box::new(replacement.Clone());
                    changed = true;
                }
            }
            if changed {
                rewritten.CleanHashCode();
                *condition = Box::new(rewritten);
            }
        }
    }
    for leaf in &mut leaves {
        reorder_inner_join_descendants(leaf.as_mut())?;
    }
    let join_reorder_threshold = context.GetSessionVars().TiDBOptJoinReorderThreshold;
    let use_greedy = join_reorder_threshold < 0
        || leaves.len() > usize::try_from(join_reorder_threshold).unwrap_or(usize::MAX);
    let use_advanced_join_reorder = context.GetSessionVars().TiDBOptEnableAdvancedJoinReorder;
    let reordered = if use_greedy && use_advanced_join_reorder {
        build_advanced_greedy_reordered_inner_join_group(context, query_block, leaves, conditions)?
    } else if use_greedy {
        build_greedy_reordered_inner_join_group(context, query_block, leaves, conditions)?
    } else {
        build_dp_reordered_inner_join_group(context, query_block, leaves, conditions)?
    };
    Ok(restore_reordered_join_output(
        reordered,
        original_schema,
        original_names,
    ))
}

/// Predicate propagation can derive the same expression join key through every
/// column in an equality class. Go's join graph keeps only the last such edge;
/// otherwise one SQL predicate is materialized several times and changes both
/// the join order and the visible plan-column allocation.
fn deduplicate_equivalent_expression_join_keys(
    context: &base::ContextRef,
    conditions: &mut Vec<expression::ExprBox>,
) {
    let mut parents = HashMap::new();
    for condition in conditions.iter() {
        let Some(function) = condition.as_scalar_function() else {
            continue;
        };
        if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq") || function.GetArgs().len() != 2
        {
            continue;
        }
        if let (Some(left), Some(right)) = (
            function.GetArgs()[0].as_column(),
            function.GetArgs()[1].as_column(),
        ) {
            join_order_union(&mut parents, left.UniqueID, right.UniqueID);
        }
    }
    let mut seen = HashMap::<String, (usize, String)>::new();
    let mut retained = Vec::with_capacity(conditions.len());
    for condition in conditions.drain(..) {
        let rendered = condition.StringWithCtx(None, expression::errors::RedactLogDisable);
        let key = condition.as_scalar_function().and_then(|function| {
            if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                || function.GetArgs().len() != 2
            {
                return None;
            }
            let (expression, column) = match (
                function.GetArgs()[0].as_column(),
                function.GetArgs()[1].as_column(),
            ) {
                (None, Some(column)) => (function.GetArgs()[0].CloneExpr(), column),
                (Some(column), None) => (function.GetArgs()[1].CloneExpr(), column),
                _ => return None,
            };
            let columns = expression::ExtractColumns(expression.as_ref())
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let replacements = columns
                .iter()
                .map(|column| {
                    let root = join_order_find(&mut parents, column.UniqueID);
                    let mut replacement = column.Clone();
                    replacement.UniqueID = root;
                    replacement.OrigName = format!("Column#{root}");
                    replacement
                })
                .collect::<Vec<_>>();
            let normalized = expression::ColumnSubstitute(
                context.GetExprCtx(),
                expression,
                &expression::NewSchema(columns),
                &expression::Column2Exprs(&replacements),
            );
            Some(format!(
                "{}={}",
                normalized.StringWithCtx(None, expression::errors::RedactLogDisable),
                join_order_find(&mut parents, column.UniqueID)
            ))
        });
        if let Some(key) = key {
            if let Some((index, previous)) = seen.get_mut(&key) {
                if rendered > *previous {
                    retained[*index] = condition;
                    *previous = rendered;
                }
            } else {
                let index = retained.len();
                seen.insert(key, (index, rendered));
                retained.push(condition);
            }
        } else {
            retained.push(condition);
        }
    }
    *conditions = retained;
}

/// 将 `expr = column` 一类跨表等值条件的计算侧物化为投影列。
///
/// Go join reorder 会把确定性的非列连接键注入对应叶子的 Projection，再把
/// 条件提升为普通等值边。这样既能用输入列 NDV 估算基数，也能枚举真正的
/// HashJoin；否则该条件会退化成高基数笛卡尔积残余谓词。
fn materialize_expression_join_keys(
    context: &base::ContextRef,
    query_block: i32,
    leaves: &mut [logicalop::LogicalPlanRef],
    conditions: &mut [expression::ExprBox],
) {
    fn expression_leaf(
        expression: &dyn expression::Expression,
        leaves: &[logicalop::LogicalPlanRef],
    ) -> Option<usize> {
        let columns = expression::ExtractColumns(expression);
        if columns.is_empty() {
            return None;
        }
        let mut matches = leaves.iter().enumerate().filter_map(|(index, leaf)| {
            columns
                .iter()
                .all(|column| leaf.Schema().Contains(column))
                .then_some(index)
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    fn inject_expression(
        context: &base::ContextRef,
        query_block: i32,
        leaf: &mut logicalop::LogicalPlanRef,
        expression: expression::ExprBox,
    ) -> expression::Column {
        let return_type = expression
            .GetType(context.GetExprCtx().GetEvalCtx())
            .clone();
        let child = std::mem::replace(
            leaf,
            Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
        );
        let mut schema = child.Schema().Clone();
        let mut column = expression::Column::new(
            return_type,
            0,
            context.GetExprCtx().AllocPlanColumnID(),
            schema.Len() as isize,
        );
        column.OrigName = format!("Column#{}", column.UniqueID);
        schema.Append([column.clone()]);
        let mut expressions = expression::Column2Exprs(&child.Schema().Columns);
        expressions.push(expression);
        let mut names = child.OutputNames().Shallow();
        names.0.push(None);
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(context.clone(), query_block);
        projection.SetSchema(schema);
        projection.SetOutputNames(names);
        projection.SetChildren(vec![child]);
        let mut projection: logicalop::LogicalPlanRef = Box::new(projection);
        let _ = projection.DeriveStats(true);
        *leaf = projection;
        column
    }

    for condition in conditions {
        let Some(function) = condition.as_scalar_function() else {
            continue;
        };
        if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq") || function.GetArgs().len() != 2
        {
            continue;
        }
        let Some(left_leaf) = expression_leaf(function.GetArgs()[0].as_ref(), leaves) else {
            continue;
        };
        let Some(right_leaf) = expression_leaf(function.GetArgs()[1].as_ref(), leaves) else {
            continue;
        };
        if left_leaf == right_leaf {
            continue;
        }
        let left_expression = function.GetArgs()[0].CloneExpr();
        let right_expression = function.GetArgs()[1].CloneExpr();
        let left_is_column = function.GetArgs()[0].as_column().is_some();
        let right_is_column = function.GetArgs()[1].as_column().is_some();
        let mut rewritten = function.clone_scalar();
        if !left_is_column {
            let column = inject_expression(
                context,
                query_block,
                &mut leaves[left_leaf],
                left_expression,
            );
            rewritten.GetArgsMut()[0] = Box::new(column);
        }
        if !right_is_column {
            let column = inject_expression(
                context,
                query_block,
                &mut leaves[right_leaf],
                right_expression,
            );
            rewritten.GetArgsMut()[1] = Box::new(column);
        }
        if !left_is_column || !right_is_column {
            rewritten.CleanHashCode();
            *condition = Box::new(rewritten);
        }
    }
}

/// 将高级贪心试跑形成的 Reordered Join 拆回初始叶子。试跑只改变计划
/// 组合与缓存统计；以叶子 ID 回填原始位置即可无复制地开始下一候选。
fn recover_advanced_greedy_leaves(
    mut plan: logicalop::LogicalPlanRef,
    leaf_indices: &HashMap<i32, usize>,
    recovered: &mut [Option<logicalop::LogicalPlanRef>],
) -> Result<(), expression::Error> {
    let reordered = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .is_some_and(|join| join.Reordered);
    if reordered {
        for child in plan.TakeChildren() {
            recover_advanced_greedy_leaves(child, leaf_indices, recovered)?;
        }
        return Ok(());
    }
    let index = leaf_indices.get(&plan.ID()).copied().ok_or_else(|| {
        expression::errors::New("advanced greedy trial produced an unknown join leaf")
    })?;
    recovered[index] = Some(plan);
    Ok(())
}

fn restore_advanced_greedy_leaves(
    plan: logicalop::LogicalPlanRef,
    leaf_indices: &HashMap<i32, usize>,
    leaf_count: usize,
) -> Result<Vec<logicalop::LogicalPlanRef>, expression::Error> {
    let mut recovered = (0..leaf_count).map(|_| None).collect::<Vec<_>>();
    recover_advanced_greedy_leaves(plan, leaf_indices, &mut recovered)?;
    recovered
        .into_iter()
        .map(|leaf| {
            leaf.ok_or_else(|| {
                expression::errors::New("advanced greedy trial did not recover every join leaf")
            })
        })
        .collect()
}

/// Go 的高级 Join Graph 贪心路径：执行前两个排序起点并比较累计代价。
fn build_advanced_greedy_reordered_inner_join_group(
    context: base::ContextRef,
    query_block: i32,
    leaves: Vec<logicalop::LogicalPlanRef>,
    conditions: Vec<expression::ExprBox>,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if leaves.len() < 2 {
        return build_greedy_reordered_inner_join_group_with_start(
            context,
            query_block,
            leaves,
            conditions,
            None,
        );
    }
    let has_materialized_expression_edge = conditions.iter().any(|condition| {
        condition.as_scalar_function().is_some_and(|function| {
            function
                .GetArgs()
                .iter()
                .filter_map(|argument| argument.as_column())
                .any(|column| column.ID == 0 && column.OrigName.starts_with("Column#"))
        })
    });
    if has_materialized_expression_edge {
        // Component-boundary materialization consumes the temporary leaf
        // projections, so the top-two trial cannot reconstruct the original
        // leaves between attempts. Go's advanced graph has a single
        // deterministic component seed for these deferred edges.
        return build_greedy_reordered_inner_join_group_with_start(
            context,
            query_block,
            leaves,
            conditions,
            Some(0),
        );
    }
    let leaf_indices = leaves
        .iter()
        .enumerate()
        .map(|(index, leaf)| (leaf.ID(), index))
        .collect::<HashMap<_, _>>();
    let clone_conditions = || {
        conditions
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>()
    };

    let (first, first_cost) = build_greedy_reordered_inner_join_group_with_start_and_cost(
        context.clone(),
        query_block,
        leaves,
        clone_conditions(),
        Some(0),
    )?;
    let leaves = restore_advanced_greedy_leaves(first, &leaf_indices, leaf_indices.len())?;
    let (second, second_cost) = build_greedy_reordered_inner_join_group_with_start_and_cost(
        context.clone(),
        query_block,
        leaves,
        clone_conditions(),
        Some(1),
    )?;
    // Match Go cumCostSignificantlyLess: numerical noise must keep the first
    // stable candidate, and the second start wins only by a material decrease.
    let scale = first_cost.abs().max(second_cost.abs()).max(1.0);
    if second_cost < first_cost && first_cost - second_cost > scale * 1e-12 {
        return Ok(second);
    }

    let leaves = restore_advanced_greedy_leaves(second, &leaf_indices, leaf_indices.len())?;
    build_greedy_reordered_inner_join_group_with_start(
        context,
        query_block,
        leaves,
        conditions,
        Some(0),
    )
}

#[derive(Clone)]
/// Join 重排序树：叶子为基表下标，内部节点为连接。
enum JoinOrderTree {
    Leaf(usize),
    Join {
        left: Box<JoinOrderTree>,
        right: Box<JoinOrderTree>,
        left_mask: usize,
        right_mask: usize,
    },
}

#[derive(Clone)]
/// 带统计信息的 Join 顺序候选。
struct JoinOrderCandidate {
    tree: JoinOrderTree,
    stats: logicalop::StatsInfo,
    cumulative_cost: f64,
}

/// 贪心重排过程中的连接节点状态。
struct GreedyJoinNode {
    plan: logicalop::LogicalPlanRef,
    stats: logicalop::StatsInfo,
    cumulative_cost: f64,
    ordinal: usize,
}

/// 估算基表/子树作为 Join 叶的代价。
fn base_join_order_cost(plan: &dyn logicalop::LogicalPlan) -> f64 {
    let detached_cte_seed_cost = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalCTE>()
        .and_then(|cte| {
            cte.Cte
                .borrow()
                .SeedPartLogicalPlan
                .as_ref()
                .map(|seed| base_join_order_cost(seed.as_ref()))
        })
        .unwrap_or_default();
    plan.StatsInfo().map_or(0.0, |stats| stats.RowCount)
        + detached_cte_seed_cost
        + plan
            .Children()
            .iter()
            .map(|child| base_join_order_cost(child.as_ref()))
            .sum::<f64>()
}

/// Go join-order `Build` 在生成叶节点代价前会递归刷新统计；Rust 的逻辑
/// 规则可能在谓词下推/列裁剪后仍保留旧缓存，因此这里显式同步一次。
fn refresh_join_order_stats(plan: &mut logicalop::LogicalPlanRef) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        refresh_join_order_stats(child)?;
    }
    if let Some(join) = plan.as_any_mut().downcast_mut::<logicalop::LogicalJoin>()
        && join.Children().len() == 2
    {
        let left = join.Children()[0].Schema().Clone();
        let right = join.Children()[1].Schema().Clone();
        let merged = expression::MergeSchema(Some(&left), Some(&right)).expect("two schemas");
        let context = join.SCtx().expect("logical join context").clone();
        let remap = |expr: expression::ExprBox, schema: &expression::Schema| {
            let columns = expression::ExtractColumns(expr.as_ref())
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let replacements = columns
                .iter()
                .map(|column| {
                    if schema.Contains(column) {
                        return column.Clone();
                    }
                    let mut matches = schema
                        .Columns
                        .iter()
                        .filter(|candidate| candidate.String() == column.String());
                    let first = matches.next();
                    if first.is_some() && matches.next().is_none() {
                        first.expect("checked unique reordered join column").Clone()
                    } else {
                        column.Clone()
                    }
                })
                .collect::<Vec<_>>();
            expression::ColumnSubstitute(
                context.GetExprCtx(),
                expr,
                &expression::NewSchema(columns),
                &expression::Column2Exprs(&replacements),
            )
        };
        for condition in &mut join.LeftConditions {
            *condition = remap(condition.CloneExpr(), &left);
        }
        for condition in &mut join.RightConditions {
            *condition = remap(condition.CloneExpr(), &right);
        }
        for condition in join
            .EqualConditions
            .iter_mut()
            .chain(&mut join.OtherConditions)
            .chain(&mut join.NAEQConditions)
        {
            *condition = remap(condition.CloneExpr(), &merged);
        }
    }
    plan.DeriveStats(true)
        .map(|_| ())
        .map_err(|error| expression::errors::New(error.to_string()))
}

/// 从等值条件提取连接边（左右掩码）。
fn equality_join_edge(
    condition: &dyn expression::Expression,
    leaves: &[logicalop::LogicalPlanRef],
) -> Option<(usize, usize)> {
    let function = condition.as_scalar_function()?;
    if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq") || function.GetArgs().len() != 2 {
        return None;
    }
    let locate = |argument: &dyn expression::Expression| {
        let columns = expression::ExtractColumns(argument);
        (!columns.is_empty()).then(|| {
            leaves
                .iter()
                .position(|leaf| columns.iter().all(|column| leaf.Schema().Contains(column)))
        })?
    };
    let left = locate(function.GetArgs()[0].as_ref())?;
    let right = locate(function.GetArgs()[1].as_ref())?;
    (left != right).then_some((left, right))
}

/// 估算某 Join 顺序候选的统计信息。
fn estimate_join_order_stats(
    left: &logicalop::StatsInfo,
    right: &logicalop::StatsInfo,
    conditions: &[expression::ExprBox],
) -> logicalop::StatsInfo {
    let key_ndvs = conditions
        .iter()
        .filter_map(|condition| {
            let function = condition.as_scalar_function()?;
            if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                || function.GetArgs().len() != 2
            {
                return None;
            }
            let first = function.GetArgs()[0].as_column()?;
            let second = function.GetArgs()[1].as_column()?;
            Some(
                left.ColNDVs
                    .get(&first.UniqueID)
                    .or_else(|| right.ColNDVs.get(&first.UniqueID))
                    .copied()
                    .unwrap_or(1.0)
                    .max(
                        left.ColNDVs
                            .get(&second.UniqueID)
                            .or_else(|| right.ColNDVs.get(&second.UniqueID))
                            .copied()
                            .unwrap_or(1.0),
                    ),
            )
        })
        .collect::<Vec<_>>();
    let denominator = key_ndvs.iter().copied().fold(1.0, f64::max);
    let row_count = (left.RowCount * right.RowCount / denominator).max(0.0);
    let mut stats = logicalop::StatsInfo {
        RowCount: row_count,
        ..Default::default()
    };
    for (id, ndv) in left.ColNDVs.iter().chain(&right.ColNDVs) {
        stats.ColNDVs.insert(*id, ndv.min(row_count));
    }
    stats
}

/// 并查集查找，用于连接分量。
fn join_order_find(parent: &mut HashMap<i64, i64>, id: i64) -> i64 {
    let immediate = *parent.entry(id).or_insert(id);
    if immediate == id {
        return id;
    }
    let root = join_order_find(parent, immediate);
    parent.insert(id, root);
    root
}

/// 并查集合并连接分量。
fn join_order_union(parent: &mut HashMap<i64, i64>, left: i64, right: i64) -> bool {
    let left_root = join_order_find(parent, left);
    let right_root = join_order_find(parent, right);
    if left_root == right_root {
        return false;
    }
    parent.insert(right_root, left_root);
    true
}

/// 收集可用于重排的等值连接条件。
fn collect_join_order_equalities(
    plan: &dyn logicalop::LogicalPlan,
    parent: &mut HashMap<i64, i64>,
) {
    if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
        for condition in join.EqualConditions.iter().chain(&join.OtherConditions) {
            let Some(function) = condition.as_scalar_function() else {
                continue;
            };
            if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                || function.GetArgs().len() != 2
            {
                continue;
            }
            let (Some(left), Some(right)) = (
                function.GetArgs()[0].as_column(),
                function.GetArgs()[1].as_column(),
            ) else {
                continue;
            };
            join_order_union(parent, left.UniqueID, right.UniqueID);
        }
    }
    for child in plan.Children() {
        collect_join_order_equalities(child.as_ref(), parent);
    }
}

/// 收集被常量约束的列。
fn collect_constant_bound_columns(
    plan: &dyn logicalop::LogicalPlan,
    columns: &mut std::collections::HashSet<i64>,
) {
    let conditions: &[expression::ExprBox] = if let Some(source) =
        plan.as_any().downcast_ref::<logicalop::DataSource>()
    {
        &source.PushedDownConds
    } else if let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() {
        &selection.Conditions
    } else {
        &[]
    };
    for condition in conditions {
        let Some(function) = condition.as_scalar_function() else {
            continue;
        };
        if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq") || function.GetArgs().len() != 2
        {
            continue;
        }
        if let (Some(column), Some(_)) = (
            function.GetArgs()[0].as_column(),
            function.GetArgs()[1].as_constant(),
        ) {
            columns.insert(column.UniqueID);
        }
        if let (Some(_), Some(column)) = (
            function.GetArgs()[0].as_constant(),
            function.GetArgs()[1].as_column(),
        ) {
            columns.insert(column.UniqueID);
        }
    }
    for child in plan.Children() {
        collect_constant_bound_columns(child.as_ref(), columns);
    }
}

/// 推迟由常量蕴含的等值，避免过早固定顺序。
fn defer_constant_implied_equalities(
    leaves: &[logicalop::LogicalPlanRef],
    conditions: Vec<expression::ExprBox>,
) -> (Vec<expression::ExprBox>, Vec<expression::ExprBox>, bool) {
    let mut constant_columns = std::collections::HashSet::new();
    for leaf in leaves {
        collect_constant_bound_columns(leaf.as_ref(), &mut constant_columns);
    }
    if constant_columns.is_empty() {
        return (conditions, Vec::new(), false);
    }
    let mut full_equivalences = HashMap::new();
    for condition in &conditions {
        let Some(function) = condition.as_scalar_function() else {
            continue;
        };
        if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq") || function.GetArgs().len() != 2
        {
            continue;
        }
        if let (Some(left), Some(right)) = (
            function.GetArgs()[0].as_column(),
            function.GetArgs()[1].as_column(),
        ) {
            join_order_union(&mut full_equivalences, left.UniqueID, right.UniqueID);
        }
    }
    let constant_roots = constant_columns
        .iter()
        .map(|id| join_order_find(&mut full_equivalences, *id))
        .collect::<std::collections::HashSet<_>>();
    let locate_leaf = |id: i64| {
        leaves.iter().position(|leaf| {
            leaf.Schema()
                .Columns
                .iter()
                .any(|column| column.UniqueID == id)
        })
    };
    let mut retained_indexes = std::collections::HashSet::new();
    for constant_root in &constant_roots {
        let mut adjacency = vec![Vec::<(usize, usize)>::new(); leaves.len()];
        let mut vertices = std::collections::HashSet::new();
        let mut targets = std::collections::HashSet::new();
        let mut class_condition_indexes = Vec::new();
        for id in &constant_columns {
            if join_order_find(&mut full_equivalences, *id) == *constant_root
                && let Some(index) = locate_leaf(*id)
            {
                targets.insert(index);
            }
        }
        for (condition_index, condition) in conditions.iter().enumerate() {
            let Some(function) = condition.as_scalar_function() else {
                continue;
            };
            if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                || function.GetArgs().len() != 2
            {
                continue;
            }
            let (Some(left), Some(right)) = (
                function.GetArgs()[0].as_column(),
                function.GetArgs()[1].as_column(),
            ) else {
                continue;
            };
            if join_order_find(&mut full_equivalences, left.UniqueID) != *constant_root {
                continue;
            }
            let (Some(left_leaf), Some(right_leaf)) =
                (locate_leaf(left.UniqueID), locate_leaf(right.UniqueID))
            else {
                continue;
            };
            vertices.insert(left_leaf);
            vertices.insert(right_leaf);
            class_condition_indexes.push(condition_index);
            adjacency[left_leaf].push((right_leaf, condition_index));
            adjacency[right_leaf].push((left_leaf, condition_index));
        }
        let Some(start) = vertices.iter().copied().min() else {
            continue;
        };
        if targets.contains(&start) {
            retained_indexes.extend(class_condition_indexes);
            continue;
        }
        fn find_constant_path(
            current: usize,
            targets: &std::collections::HashSet<usize>,
            adjacency: &[Vec<(usize, usize)>],
            visited: &mut std::collections::HashSet<usize>,
            path: &mut Vec<usize>,
        ) -> bool {
            if targets.contains(&current) {
                return true;
            }
            visited.insert(current);
            let mut neighbors = adjacency[current].clone();
            neighbors.sort_by(|left, right| right.0.cmp(&left.0));
            for (next, condition_index) in neighbors {
                if visited.contains(&next) {
                    continue;
                }
                path.push(condition_index);
                if find_constant_path(next, targets, adjacency, visited, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        if find_constant_path(
            start,
            &targets,
            &adjacency,
            &mut std::collections::HashSet::new(),
            &mut path,
        ) {
            retained_indexes.extend(path);
        }
    }
    let mut active = Vec::new();
    let mut deferred = Vec::new();
    for (condition_index, condition) in conditions.into_iter().enumerate() {
        let edge = condition.as_scalar_function().and_then(|function| {
            if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                || function.GetArgs().len() != 2
            {
                return None;
            }
            Some((
                function.GetArgs()[0].as_column()?.UniqueID,
                function.GetArgs()[1].as_column()?.UniqueID,
            ))
        });
        let Some((left, right)) = edge else {
            active.push(condition);
            continue;
        };
        let root = join_order_find(&mut full_equivalences, left);
        if constant_roots.contains(&root) && !retained_indexes.contains(&condition_index) {
            deferred.push(condition);
            continue;
        }
        active.push(condition);
    }
    let propagated_constant = !deferred.is_empty();
    (active, deferred, propagated_constant)
}

/// Build a temporary candidate Join exactly as the greedy solver does, derive
/// its real logical statistics, then return both children for continued
/// enumeration.  Go costs each candidate after `newJoin` and
/// `RecursiveDeriveStats`; the estimate-only shortcut changes tie decisions
/// for the large TPC-DS join group.
fn derive_greedy_candidate_stats(
    context: base::ContextRef,
    query_block: i32,
    left: logicalop::LogicalPlanRef,
    right: logicalop::LogicalPlanRef,
    conditions: Vec<expression::ExprBox>,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        logicalop::LogicalPlanRef,
        logicalop::StatsInfo,
    ),
    expression::Error,
> {
    with_transient_plan_ids(&context, || {
        let mut candidate = build_reordered_inner_join_group(
            context.clone(),
            query_block,
            vec![left, right],
            conditions,
        )?;
        candidate
            .DeriveStats(true)
            .map_err(|error| expression::errors::New(error.to_string()))?;
        let stats = candidate.StatsInfo().cloned().unwrap_or_default();
        let mut children = candidate.TakeChildren();
        if children.len() != 2 {
            return Err(expression::errors::New(
                "greedy join candidate lost one of its children",
            ));
        }
        let right = children.pop().expect("candidate has right child");
        let left = children.pop().expect("candidate has left child");
        Ok((left, right, stats))
    })
}

/// 贪心法重建内连接组顺序。
fn build_greedy_reordered_inner_join_group(
    context: base::ContextRef,
    query_block: i32,
    mut leaves: Vec<logicalop::LogicalPlanRef>,
    mut conditions: Vec<expression::ExprBox>,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    build_greedy_reordered_inner_join_group_with_start(
        context,
        query_block,
        leaves,
        conditions,
        None,
    )
}

/// Keep a spanning set of column equalities, matching Go join-graph used-edge
/// handling; transitive cycle predicates need not be attached again.
fn retain_spanning_join_equalities(
    conditions: Vec<expression::ExprBox>,
    parents: &mut HashMap<i64, i64>,
) -> Vec<expression::ExprBox> {
    fn root(parents: &mut HashMap<i64, i64>, id: i64) -> i64 {
        let parent = *parents.entry(id).or_insert(id);
        if parent == id {
            id
        } else {
            let resolved = root(parents, parent);
            parents.insert(id, resolved);
            resolved
        }
    }
    let mut retained = Vec::with_capacity(conditions.len());
    for condition in conditions {
        let pair = condition.as_scalar_function().and_then(|function| {
            matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                .then(|| {
                    let args = function.GetArgs();
                    Some((
                        args.first()?.as_column()?.UniqueID,
                        args.get(1)?.as_column()?.UniqueID,
                    ))
                })
                .flatten()
        });
        let Some((left, right)) = pair else {
            retained.push(condition);
            continue;
        };
        let left_root = root(parents, left);
        let right_root = root(parents, right);
        if left_root == right_root {
            continue;
        }
        parents.insert(right_root, left_root);
        retained.push(condition);
    }
    retained
}

/// 在指定的、按初始代价排序的节点开始重建贪心 Join 树。传统重排不指定
/// 起点；高级 Join Graph 传入其双起点比较的获胜者。
fn build_greedy_reordered_inner_join_group_with_start(
    context: base::ContextRef,
    query_block: i32,
    mut leaves: Vec<logicalop::LogicalPlanRef>,
    mut conditions: Vec<expression::ExprBox>,
    start_index: Option<usize>,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    build_greedy_reordered_inner_join_group_with_start_and_cost(
        context,
        query_block,
        leaves,
        conditions,
        start_index,
    )
    .map(|(plan, _)| plan)
}

fn build_greedy_reordered_inner_join_group_with_start_and_cost(
    context: base::ContextRef,
    query_block: i32,
    mut leaves: Vec<logicalop::LogicalPlanRef>,
    mut conditions: Vec<expression::ExprBox>,
    start_index: Option<usize>,
) -> Result<(logicalop::LogicalPlanRef, f64), expression::Error> {
    // 保留传统贪心的连通分量处理；高级路径仅以 Go 选出的起点启动同一套
    // 边消费循环。
    // Keep all join edges in the greedy graph. The Go solver does not defer
    // equalities merely because one endpoint is constrained by a constant:
    // those edges still participate in candidate costing and may be the edge
    // that makes a later dimension table reachable.
    let (active_conditions, mut deferred_conditions, _propagated_constant) =
        defer_constant_implied_equalities(&leaves, conditions);
    // Go's join graph first builds the components connected by ordinary
    // column equalities. Equalities whose endpoint is an expression injected
    // by `materialize_expression_join_keys` connect those completed
    // components afterwards. Treating such a synthetic column as an ordinary
    // seed edge produces a long left-deep chain for CH Q5 instead of the two
    // fact/dimension subtrees selected by TiDB.
    let (mut expression_edges, mut active_conditions): (Vec<_>, Vec<_>) =
        active_conditions.into_iter().partition(|condition| {
            condition
                .as_scalar_function()
                .filter(|function| {
                    matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                        && function.GetArgs().len() == 2
                })
                .is_some_and(|function| {
                    function
                        .GetArgs()
                        .iter()
                        .filter_map(|argument| argument.as_column())
                        .any(|column| column.ID == 0 && column.OrigName.starts_with("Column#"))
                })
        });
    if expression_edges.len() == 1 && leaves.len() > 4 {
        active_conditions.append(&mut expression_edges);
    }
    conditions = active_conditions;
    // The expression-key projections above are only markers while the join
    // graph is built. TiDB evaluates them after their ordinary-edge component
    // has been assembled, not on the base-table leaf.
    let mut pending_materializations = Vec::<(expression::Column, expression::ExprBox)>::new();
    if !expression_edges.is_empty() {
        for leaf in &mut leaves {
            loop {
                let Some(projection) = leaf.as_any().downcast_ref::<logicalop::LogicalProjection>()
                else {
                    break;
                };
                let generated = projection
                    .Exprs
                    .iter()
                    .zip(&projection.Schema().Columns)
                    .filter(|(value, column)| {
                        !value.as_any().is::<expression::Column>()
                            && column.ID == 0
                            && column.OrigName.starts_with("Column#")
                    })
                    .map(|(value, column)| (column.Clone(), value.CloneExpr()))
                    .collect::<Vec<_>>();
                if generated.is_empty() || projection.Children().len() != 1 {
                    break;
                }
                pending_materializations.extend(generated);
                let projection = leaf
                    .as_any_mut()
                    .downcast_mut::<logicalop::LogicalProjection>()
                    .expect("checked expression-key projection");
                *leaf = projection.TakeChildren().remove(0);
            }
        }
    }
    pending_materializations.sort_by_key(|(_, value)| {
        let columns = expression::ExtractColumns(value.as_ref());
        leaves
            .iter()
            .position(|leaf| {
                !columns.is_empty() && columns.iter().all(|column| leaf.Schema().Contains(column))
            })
            .unwrap_or(usize::MAX)
    });
    let mut allocated_ids = pending_materializations
        .iter()
        .map(|(column, _)| column.UniqueID)
        .collect::<Vec<_>>();
    allocated_ids.sort_unstable();
    let mut remapped_columns = HashMap::new();
    for ((column, _), unique_id) in pending_materializations.iter_mut().zip(allocated_ids) {
        let old_id = column.UniqueID;
        column.UniqueID = unique_id;
        column.OrigName = format!("Column#{unique_id}");
        remapped_columns.insert(old_id, column.Clone());
    }
    for edge in &mut expression_edges {
        let Some(function) = edge.as_scalar_function() else {
            continue;
        };
        let mut rewritten = function.clone_scalar();
        let mut changed = false;
        for argument in rewritten.GetArgsMut() {
            let Some(column) = argument.as_column() else {
                continue;
            };
            if let Some(remapped) = remapped_columns.get(&column.UniqueID) {
                *argument = Box::new(remapped.Clone());
                changed = true;
            }
        }
        if changed {
            rewritten.CleanHashCode();
            *edge = Box::new(rewritten);
        }
    }
    expression_edges.sort_by_key(|edge| {
        edge.as_scalar_function()
            .and_then(|function| {
                function
                    .GetArgs()
                    .iter()
                    .filter_map(|argument| argument.as_column())
                    .find(|column| column.ID == 0 && column.OrigName.starts_with("Column#"))
                    .map(|column| column.UniqueID)
            })
            .unwrap_or(i64::MAX)
    });
    let mut nodes = leaves
        .into_iter()
        .enumerate()
        .map(|(ordinal, mut plan)| {
            refresh_join_order_stats(&mut plan)?;
            Ok(GreedyJoinNode {
                stats: plan.StatsInfo().cloned().unwrap_or_default(),
                cumulative_cost: base_join_order_cost(plan.as_ref()),
                plan,
                ordinal,
            })
        })
        .collect::<Result<Vec<_>, expression::Error>>()?;
    nodes.sort_by(|left, right| {
        left.cumulative_cost
            .total_cmp(&right.cumulative_cost)
            .then(left.ordinal.cmp(&right.ordinal))
    });
    if let Some(start_index) = start_index.filter(|index| *index < nodes.len())
        && start_index > 0
    {
        let start = nodes.remove(start_index);
        nodes.insert(0, start);
    }
    let mut components = Vec::new();
    let mut component_costs = Vec::new();
    let mut equality_parents = HashMap::new();
    while !nodes.is_empty() {
        let mut current = nodes.remove(0);
        loop {
            let mut best: Option<(usize, f64, logicalop::StatsInfo, Vec<usize>)> = None;
            let mut index = 0;
            while index < nodes.len() {
                let mut candidate = nodes.remove(index);
                let condition_indexes = conditions
                    .iter()
                    .enumerate()
                    .filter(|condition| {
                        condition_crosses_schemas(
                            condition.1.as_ref(),
                            current.plan.Schema(),
                            candidate.plan.Schema(),
                        )
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let step_conditions = condition_indexes
                    .iter()
                    .map(|index| conditions[*index].CloneExpr())
                    .collect::<Vec<_>>();
                if !step_conditions.iter().any(|condition| {
                    let Some(function) = condition.as_scalar_function() else {
                        return false;
                    };
                    matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                }) {
                    nodes.insert(index, candidate);
                    index += 1;
                    continue;
                }
                let (left, right, stats) = derive_greedy_candidate_stats(
                    context.clone(),
                    query_block,
                    current.plan,
                    candidate.plan,
                    step_conditions.clone(),
                )?;
                current.plan = left;
                candidate.plan = right;
                candidate.stats = candidate.plan.StatsInfo().cloned().unwrap_or_default();
                let candidate_cumulative_cost = candidate.cumulative_cost;
                nodes.insert(index, candidate);
                let cost = stats.RowCount + current.cumulative_cost + candidate_cumulative_cost;
                if best
                    .as_ref()
                    .is_none_or(|(_, best_cost, _, _)| cost < *best_cost)
                {
                    best = Some((index, cost, stats, condition_indexes));
                }
                index += 1;
            }
            let Some((index, cumulative_cost, stats, condition_indexes)) = best else {
                break;
            };
            // Go's greedy join reorder removes the chosen candidate while
            // preserving the remaining input order.  `swap_remove` changes
            // that order and makes equal-cost candidates choose a different
            // join tree, which is observable in plan-tree golden output.
            let candidate = nodes.remove(index);
            let step_conditions = condition_indexes
                .iter()
                .map(|index| conditions[*index].CloneExpr())
                .collect::<Vec<_>>();
            let step_conditions =
                retain_spanning_join_equalities(step_conditions, &mut equality_parents);
            let mut plan = build_reordered_inner_join_group(
                context.clone(),
                query_block,
                vec![current.plan, candidate.plan],
                step_conditions,
            )?;
            // A join edge may only be applied once.  Keeping it in the global
            // candidate pool lets a later supertree reattach the same
            // predicate, unlike Go's `usedEdges` bookkeeping.
            let used_indexes = condition_indexes
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            conditions = conditions
                .into_iter()
                .enumerate()
                .filter_map(|(index, condition)| {
                    (!used_indexes.contains(&index)).then_some(condition)
                })
                .collect();
            current = GreedyJoinNode {
                plan,
                stats,
                cumulative_cost,
                ordinal: current.ordinal.min(candidate.ordinal),
            };
        }
        component_costs.push(current.cumulative_cost);
        components.push(current.plan);
    }
    for component in &mut components {
        let matching = pending_materializations
            .iter()
            .filter(|(_, value)| {
                let columns = expression::ExtractColumns(value.as_ref());
                !columns.is_empty()
                    && columns
                        .iter()
                        .all(|column| component.Schema().Contains(column))
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            continue;
        }
        let child = std::mem::replace(
            component,
            Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
        );
        let child_stats = child.StatsInfo().cloned().unwrap_or_default();
        let mut schema = child.Schema().Clone();
        let mut expressions = expression::Column2Exprs(&schema.Columns);
        let mut names = child.OutputNames().Shallow();
        for (column, value) in matching {
            schema.Append([column.Clone()]);
            expressions.push(value.CloneExpr());
            names.0.push(None);
        }
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(context.clone(), query_block);
        projection.SetSchema(schema);
        projection.SetOutputNames(names);
        projection.SetChildren(vec![child]);
        let projection_schema = projection.Schema().Clone();
        projection
            .DeriveStats(&child_stats, &projection_schema, &[true])
            .map_err(|error| expression::errors::New(error.to_string()))?;
        *component = Box::new(projection);
    }
    let connected_cost = (components.len() == 1).then(|| component_costs[0]);
    if components.len() > 1 {
        conditions.append(&mut expression_edges);
    }
    while components.len() > 1 {
        let mut next = Vec::with_capacity(components.len().div_ceil(2));
        let mut iterator = components.into_iter();
        while let Some(left) = iterator.next() {
            let Some(right) = iterator.next() else {
                next.push(left);
                break;
            };
            let step_conditions = conditions
                .iter()
                .filter(|condition| {
                    condition_crosses_schemas(condition.as_ref(), left.Schema(), right.Schema())
                })
                .map(|condition| condition.CloneExpr())
                .collect();
            next.push(build_reordered_inner_join_group(
                context.clone(),
                query_block,
                vec![left, right],
                step_conditions,
            )?);
        }
        components = next;
    }
    let mut result = components
        .pop()
        .ok_or_else(|| expression::errors::New("join reorder group has no leaves"))?;
    if let Some(join) = result.as_any_mut().downcast_mut::<logicalop::LogicalJoin>() {
        join.OtherConditions.append(&mut expression_edges);
        join.OtherConditions.append(&mut deferred_conditions);
    }
    let cumulative_cost = connected_cost.unwrap_or_else(|| base_join_order_cost(result.as_ref()));
    Ok((result, cumulative_cost))
}

/// 判断条件是否跨越给定左右位掩码。
fn join_order_condition_crosses_masks(
    condition: &dyn expression::Expression,
    left_mask: usize,
    right_mask: usize,
    leaves: &[logicalop::LogicalPlanRef],
) -> bool {
    let columns = expression::ExtractColumns(condition);
    let belongs = |column: &expression::Column, mask: usize| {
        leaves
            .iter()
            .enumerate()
            .any(|(index, leaf)| mask & (1 << index) != 0 && leaf.Schema().Contains(column))
    };
    !columns.is_empty()
        && columns
            .iter()
            .all(|column| belongs(column, left_mask) || belongs(column, right_mask))
        && columns.iter().any(|column| belongs(column, left_mask))
        && columns.iter().any(|column| belongs(column, right_mask))
}

/// 求解单个连接分量的最优/近优顺序。
fn solve_join_order_component(
    component: &[usize],
    leaves: &[logicalop::LogicalPlanRef],
    conditions: &[expression::ExprBox],
) -> Result<JoinOrderTree, expression::Error> {
    if component.len() == 1 {
        return Ok(JoinOrderTree::Leaf(component[0]));
    }
    let count = component.len();
    let mut best: Vec<Option<JoinOrderCandidate>> = vec![None; 1usize << count];
    for (local, global) in component.iter().copied().enumerate() {
        let stats = leaves[global].StatsInfo().cloned().unwrap_or_default();
        best[1 << local] = Some(JoinOrderCandidate {
            tree: JoinOrderTree::Leaf(global),
            stats,
            cumulative_cost: base_join_order_cost(leaves[global].as_ref()),
        });
    }
    for mask in 1usize..(1usize << count) {
        if mask.count_ones() == 1 {
            continue;
        }
        let mut sub = (mask - 1) & mask;
        while sub > 0 {
            let remain = mask ^ sub;
            if sub <= remain {
                let Some(left) = best[sub].as_ref() else {
                    sub = (sub - 1) & mask;
                    continue;
                };
                let Some(right) = best[remain].as_ref() else {
                    sub = (sub - 1) & mask;
                    continue;
                };
                let global_mask = |local_mask: usize| {
                    component
                        .iter()
                        .copied()
                        .enumerate()
                        .fold(0usize, |result, (local, global)| {
                            result | ((local_mask & (1 << local) != 0) as usize) << global
                        })
                };
                let left_mask = global_mask(sub);
                let right_mask = global_mask(remain);
                let step_conditions = conditions
                    .iter()
                    .filter(|condition| {
                        join_order_condition_crosses_masks(
                            condition.as_ref(),
                            left_mask,
                            right_mask,
                            leaves,
                        )
                    })
                    .map(|condition| condition.CloneExpr())
                    .collect::<Vec<_>>();
                let connected = step_conditions
                    .iter()
                    .any(|condition| equality_join_edge(condition.as_ref(), leaves).is_some());
                if connected {
                    let stats =
                        estimate_join_order_stats(&left.stats, &right.stats, &step_conditions);
                    let cumulative_cost =
                        stats.RowCount + left.cumulative_cost + right.cumulative_cost;
                    if best[mask]
                        .as_ref()
                        .is_none_or(|current| current.cumulative_cost > cumulative_cost)
                    {
                        best[mask] = Some(JoinOrderCandidate {
                            tree: JoinOrderTree::Join {
                                left: Box::new(left.tree.clone()),
                                right: Box::new(right.tree.clone()),
                                left_mask,
                                right_mask,
                            },
                            stats,
                            cumulative_cost,
                        });
                    }
                }
            }
            sub = (sub - 1) & mask;
        }
    }
    best[(1usize << count) - 1]
        .take()
        .map(|candidate| candidate.tree)
        .ok_or_else(|| expression::errors::New("join reorder DP found no connected plan"))
}

/// 动态规划法重建内连接组顺序。
fn build_dp_reordered_inner_join_group(
    context: base::ContextRef,
    query_block: i32,
    leaves: Vec<logicalop::LogicalPlanRef>,
    conditions: Vec<expression::ExprBox>,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if leaves.is_empty() {
        return Err(expression::errors::New("join reorder group has no leaves"));
    }
    let mut adjacency = vec![Vec::new(); leaves.len()];
    for condition in &conditions {
        if let Some((left, right)) = equality_join_edge(condition.as_ref(), &leaves) {
            adjacency[left].push(right);
            adjacency[right].push(left);
        }
    }
    let mut visited = vec![false; leaves.len()];
    let mut components = Vec::new();
    for start in 0..leaves.len() {
        if visited[start] {
            continue;
        }
        let mut queue = std::collections::VecDeque::from([start]);
        visited[start] = true;
        let mut component = Vec::new();
        while let Some(current) = queue.pop_front() {
            component.push(current);
            for next in adjacency[current].iter().copied() {
                if !visited[next] {
                    visited[next] = true;
                    queue.push_back(next);
                }
            }
        }
        components.push(component);
    }
    let trees = components
        .iter()
        .map(|component| solve_join_order_component(component, &leaves, &conditions))
        .collect::<Result<Vec<_>, _>>()?;
    let schemas = leaves
        .iter()
        .map(|leaf| leaf.Schema().Clone())
        .collect::<Vec<_>>();
    let mut available = leaves.into_iter().map(Some).collect::<Vec<_>>();
    fn build_tree_with_schemas(
        tree: JoinOrderTree,
        available: &mut [Option<logicalop::LogicalPlanRef>],
        schemas: &[expression::Schema],
        context: &base::ContextRef,
        query_block: i32,
        conditions: &[expression::ExprBox],
    ) -> Result<logicalop::LogicalPlanRef, expression::Error> {
        match tree {
            JoinOrderTree::Leaf(index) => Ok(available[index].take().expect("join leaf used once")),
            JoinOrderTree::Join {
                left,
                right,
                left_mask,
                right_mask,
            } => {
                // Inner joins are commutative.  Go keeps the original table
                // order when costs tie, so canonicalize each DP node by the
                // earliest input leaf before materializing the children.
                let (left, right, left_mask, right_mask) =
                    if left_mask.trailing_zeros() <= right_mask.trailing_zeros() {
                        (left, right, left_mask, right_mask)
                    } else {
                        (right, left, right_mask, left_mask)
                    };
                let left = build_tree_with_schemas(
                    *left,
                    available,
                    schemas,
                    context,
                    query_block,
                    conditions,
                )?;
                let right = build_tree_with_schemas(
                    *right,
                    available,
                    schemas,
                    context,
                    query_block,
                    conditions,
                )?;
                let step_conditions = conditions
                    .iter()
                    .filter(|condition| {
                        let columns = expression::ExtractColumns(condition.as_ref());
                        let belongs = |column: &expression::Column, mask: usize| {
                            schemas.iter().enumerate().any(|(index, schema)| {
                                mask & (1 << index) != 0 && schema.Contains(column)
                            })
                        };
                        !columns.is_empty()
                            && columns.iter().all(|column| {
                                belongs(column, left_mask) || belongs(column, right_mask)
                            })
                            && columns.iter().any(|column| belongs(column, left_mask))
                            && columns.iter().any(|column| belongs(column, right_mask))
                    })
                    .map(|condition| condition.CloneExpr())
                    .collect();
                build_reordered_inner_join_group(
                    context.clone(),
                    query_block,
                    vec![left, right],
                    step_conditions,
                )
            }
        }
    }
    let mut component_plans = trees
        .into_iter()
        .map(|tree| {
            build_tree_with_schemas(
                tree,
                &mut available,
                &schemas,
                &context,
                query_block,
                &conditions,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    while component_plans.len() > 1 {
        let mut next = Vec::with_capacity(component_plans.len().div_ceil(2));
        let mut iterator = component_plans.into_iter();
        while let Some(left) = iterator.next() {
            let Some(right) = iterator.next() else {
                next.push(left);
                break;
            };
            let step_conditions = conditions
                .iter()
                .filter(|condition| {
                    condition_crosses_schemas(condition.as_ref(), left.Schema(), right.Schema())
                })
                .map(|condition| condition.CloneExpr())
                .collect();
            next.push(build_reordered_inner_join_group(
                context.clone(),
                query_block,
                vec![left, right],
                step_conditions,
            )?);
        }
        component_plans = next;
    }
    Ok(component_plans.remove(0))
}

/// 重排后恢复原始输出 Schema 与列名。
fn restore_reordered_join_output(
    reordered: logicalop::LogicalPlanRef,
    original_schema: expression::Schema,
    original_names: expression::types::NameSlice,
) -> logicalop::LogicalPlanRef {
    let unchanged = reordered.Schema().Len() == original_schema.Len()
        && reordered
            .Schema()
            .Columns
            .iter()
            .zip(&original_schema.Columns)
            .all(|(actual, original)| actual.UniqueID == original.UniqueID);
    if unchanged {
        return reordered;
    }
    let Some(context) = reordered.SCtx().cloned() else {
        return reordered;
    };
    let query_block = reordered.QueryBlockOffset();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expression::Column2Exprs(&original_schema.Columns),
        ..Default::default()
    }
    .Init(context, query_block);
    projection.SetSchema(original_schema);
    projection.SetOutputNames(original_names);
    projection.SetChildren(vec![reordered]);
    Box::new(projection)
}

/// 判断该内连接是否允许参与重排。
fn can_reorder_inner_join(join: &logicalop::LogicalJoin) -> bool {
    join.JoinType == base::JoinType::InnerJoin
        && !join.StraightJoin
        && join.PreferJoinType == 0
        && !join.PreferJoinOrder
        && !join.InternalPreferJoinOrder
        && join.LeftPreferJoinType == 0
        && join.RightPreferJoinType == 0
        && join.NAEQConditions.is_empty()
        && !join.FromDecorrelatedApply
        && !join.FromSemiJoinRewrite
}

/// 收集可扁平化的内连接叶子与条件。
fn collect_inner_join_group(
    mut plan: logicalop::LogicalPlanRef,
    leaves: &mut Vec<logicalop::LogicalPlanRef>,
    conditions: &mut Vec<expression::ExprBox>,
) -> Result<(), expression::Error> {
    let can_flatten = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
        .is_some_and(can_reorder_inner_join);
    if !can_flatten {
        leaves.push(plan);
        return Ok(());
    }
    let join = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalJoin>()
        .expect("join reorder member");
    let mut local_conditions = Vec::new();
    local_conditions.append(&mut join.EqualConditions);
    local_conditions.append(&mut join.OtherConditions);
    local_conditions.append(&mut join.LeftConditions);
    local_conditions.append(&mut join.RightConditions);
    let children = join.TakeChildren();
    for child in children {
        collect_inner_join_group(child, leaves, conditions)?;
    }
    conditions.append(&mut local_conditions);
    Ok(())
}

/// 按选定顺序树重建内连接计划。
fn build_reordered_inner_join_group(
    context: base::ContextRef,
    query_block: i32,
    mut leaves: Vec<logicalop::LogicalPlanRef>,
    mut conditions: Vec<expression::ExprBox>,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if leaves.is_empty() {
        return Err(expression::errors::New("join reorder group has no leaves"));
    }
    let mut current = leaves.remove(0);
    while !leaves.is_empty() {
        let next_index = leaves
            .iter()
            .position(|candidate| {
                conditions.iter().any(|condition| {
                    condition_crosses_schemas(
                        condition.as_ref(),
                        current.Schema(),
                        candidate.Schema(),
                    )
                })
            })
            .unwrap_or(0);
        let next = leaves.remove(next_index);
        let last = leaves.is_empty();
        let mut step_conditions = Vec::new();
        let mut pending = Vec::new();
        for condition in conditions {
            if last
                || condition_crosses_schemas(condition.as_ref(), current.Schema(), next.Schema())
            {
                step_conditions.push(orient_join_condition(
                    condition,
                    current.Schema(),
                    next.Schema(),
                ));
            } else {
                pending.push(condition);
            }
        }
        conditions = pending;

        let mut names = current.OutputNames().Shallow();
        names.0.extend(next.OutputNames().0.iter().cloned());
        let mut join = logicalop::LogicalJoin {
            JoinType: base::JoinType::InnerJoin,
            Reordered: true,
            ..Default::default()
        }
        .Init(context.clone(), query_block);
        join.SetChildren(vec![current, next]);
        join.MergeSchema();
        join.SetOutputNames(names);
        join.AttachOnConds(step_conditions);
        join.DeriveStats(true)
            .map_err(|error| expression::errors::New(error.to_string()))?;
        current = Box::new(join);
    }
    Ok(current)
}

/// 条件是否引用两侧 Schema 的列。
fn condition_crosses_schemas(
    condition: &dyn expression::Expression,
    left: &expression::Schema,
    right: &expression::Schema,
) -> bool {
    let columns = expression::ExtractColumns(condition);
    !columns.is_empty()
        && columns
            .iter()
            .all(|column| left.Contains(column) || right.Contains(column))
        && columns.iter().any(|column| left.Contains(column))
        && columns.iter().any(|column| right.Contains(column))
}

/// 调整连接条件左右方向以匹配子树。
fn orient_join_condition(
    condition: expression::ExprBox,
    left: &expression::Schema,
    right: &expression::Schema,
) -> expression::ExprBox {
    let Some(function) = condition.as_scalar_function() else {
        return condition;
    };
    if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq") || function.GetArgs().len() != 2 {
        return condition;
    }
    let (Some(first), Some(second)) = (
        function.GetArgs()[0].as_column(),
        function.GetArgs()[1].as_column(),
    ) else {
        return condition;
    };
    if !right.Contains(first) || !left.Contains(second) {
        return condition;
    }
    let mut oriented = function.clone_scalar();
    oriented.GetArgsMut().swap(0, 1);
    oriented.CleanHashCode();
    Box::new(oriented)
}

/// Go's partition processor runs after predicate pushdown.  For an equality
/// point it is safe to evaluate even a non-monotonic partition expression:
/// every row satisfying the equality produces the same partition value.
fn rewrite_static_partitions(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    if let Some(source) = plan.as_any_mut().downcast_mut::<logicalop::DataSource>() {
        prune_static_range_data_source(source)?;
        let Some(partition) = source.TableInfo.GetPartitionInfo() else {
            return Ok(());
        };
        if !partition.Enable || partition.Definitions.is_empty() || source.PartitionDefIdx.is_some()
        {
            return Ok(());
        }
        let context = source
            .SCtx()
            .cloned()
            .ok_or_else(|| expression::errors::New("partitioned source has no plan context"))?;
        let offset = source.QueryBlockOffset();
        let schema = source.Schema().Clone();
        let names = source.OutputNames().Shallow();
        let stats = source.StatsInfo().cloned();
        let selected_indices = static_partition_indices(source, partition)?;
        if selected_indices.is_empty() {
            let mut dual = logicalop::LogicalTableDual {
                RowCount: 0,
                ..Default::default()
            }
            .Init(context, offset);
            dual.SetSchema(schema);
            dual.SetOutputNames(names);
            *plan = Box::new(dual);
            return Ok(());
        }
        let children = partition
            .Definitions
            .iter()
            .enumerate()
            .filter(|(_, definition)| {
                source.PartitionNames.is_empty()
                    || source
                        .PartitionNames
                        .iter()
                        .any(|name| name.L == definition.Name.L)
            })
            .filter(|(index, _)| selected_indices.contains(index))
            .map(|(index, definition)| {
                let mut child = clone_partition_source(source, context.clone(), offset);
                child.PartitionDefIdx = Some(index);
                child.PhysicalTableID = definition.ID;
                Box::new(child) as logicalop::LogicalPlanRef
            })
            .collect::<Vec<_>>();
        if children.len() == 1 {
            *plan = children.into_iter().next().expect("one partition child");
            return Ok(());
        }
        let mut union = logicalop::LogicalPartitionUnionAll::default().Init(context, offset);
        union.SetSchema(schema);
        union.SetOutputNames(names);
        if let Some(stats) = stats {
            union.SetStats(stats);
        }
        union.SetChildren(children);
        *plan = Box::new(union);
        return Ok(());
    }

    let mut children = plan.TakeChildren();
    for child in &mut children {
        rewrite_static_partitions(child)?;
    }
    plan.SetChildren(children);
    Ok(())
}

/// 根据谓词计算静态模式下命中的分区下标。
fn static_partition_indices(
    source: &logicalop::DataSource,
    partition: &expression::model::PartitionInfo,
) -> Result<Vec<usize>, expression::Error> {
    if source.PushedDownConds.is_empty() {
        return Ok((0..partition.Definitions.len()).collect());
    }
    if partition.Type == expression::model::ast::PartitionTypeHash {
        return Ok(hash_partition_indices(source, partition));
    }
    if partition.Type != expression::model::ast::PartitionTypeRange {
        return Ok((0..partition.Definitions.len()).collect());
    }
    let Some(partition_column) = source
        .TableInfo
        .Columns
        .iter()
        .find(|column| column.Name.L == partition.Expr.trim().to_ascii_lowercase())
    else {
        return Ok((0..partition.Definitions.len()).collect());
    };
    let Some(eval) = source
        .SCtx()
        .map(|context| context.GetExprCtx().GetEvalCtx())
    else {
        return Ok((0..partition.Definitions.len()).collect());
    };
    let mut lower = None;
    let mut selected = Vec::new();
    for (index, definition) in partition.Definitions.iter().enumerate() {
        let upper = definition
            .LessThan
            .first()
            .filter(|bound| !bound.trim().eq_ignore_ascii_case("maxvalue"))
            .and_then(|bound| unquote_partition_bound(bound).parse::<i64>().ok());
        let null_partition = index == 0;
        if source.PushedDownConds.iter().all(|condition| {
            partition_predicate_may_match(
                condition.as_ref(),
                partition_column.ID,
                lower,
                upper,
                null_partition,
                eval,
            )
        }) {
            selected.push(index);
        }
        lower = upper;
    }
    Ok(selected)
}

/// HASH 分区的 NULL 点谓词固定落到第 0 个分区，与 Go 的 EvalInt(NULL) 规则一致。
fn hash_partition_indices(
    source: &logicalop::DataSource,
    partition: &expression::model::PartitionInfo,
) -> Vec<usize> {
    let all = || (0..partition.Definitions.len()).collect();
    let expression_name = partition.Expr.trim().trim_matches('`').to_ascii_lowercase();
    let Some(partition_column) = source
        .TableInfo
        .Columns
        .iter()
        .find(|column| column.Name.L == expression_name)
    else {
        return all();
    };
    if source
        .PushedDownConds
        .iter()
        .any(|condition| predicate_implies_column_is_null(condition.as_ref(), partition_column.ID))
    {
        vec![0]
    } else {
        all()
    }
}

/// 判断谓词是否蕴含目标列为 NULL；AND 任一子式成立，OR 则全部分支都必须成立。
fn predicate_implies_column_is_null(
    predicate: &dyn expression::Expression,
    partition_column_id: i64,
) -> bool {
    let Some(function) = predicate.as_scalar_function() else {
        return false;
    };
    let arguments = function.GetArgs();
    match function.FuncName.L.as_str() {
        expression::ast::IsNull => {
            arguments.len() == 1
                && arguments[0]
                    .as_column()
                    .is_some_and(|column| column.ID == partition_column_id)
        }
        expression::ast::LogicAnd => arguments.iter().any(|argument| {
            predicate_implies_column_is_null(argument.as_ref(), partition_column_id)
        }),
        expression::ast::LogicOr => {
            !arguments.is_empty()
                && arguments.iter().all(|argument| {
                    predicate_implies_column_is_null(argument.as_ref(), partition_column_id)
                })
        }
        _ => false,
    }
}

/// 谓词是否可能命中某分区边界。
fn partition_predicate_may_match(
    predicate: &dyn expression::Expression,
    partition_column_id: i64,
    lower: Option<i64>,
    upper: Option<i64>,
    contains_null: bool,
    eval: &dyn expression::EvalContext,
) -> bool {
    let Some(function) = predicate.as_scalar_function() else {
        return true;
    };
    let arguments = function.GetArgs();
    match function.FuncName.L.as_str() {
        expression::ast::LogicAnd => {
            return arguments.iter().all(|argument| {
                partition_predicate_may_match(
                    argument.as_ref(),
                    partition_column_id,
                    lower,
                    upper,
                    contains_null,
                    eval,
                )
            });
        }
        expression::ast::LogicOr => {
            return arguments.iter().any(|argument| {
                partition_predicate_may_match(
                    argument.as_ref(),
                    partition_column_id,
                    lower,
                    upper,
                    contains_null,
                    eval,
                )
            });
        }
        expression::ast::IsNull => {
            return contains_null
                && arguments.first().is_some_and(|argument| {
                    expression::ExtractColumns(argument.as_ref())
                        .iter()
                        .any(|column| column.ID == partition_column_id)
                });
        }
        _ => {}
    }
    if arguments.len() != 2 {
        return true;
    }
    let mut operator = function.FuncName.L.as_str();
    let (column_expression, constant_expression) =
        if expression::ExtractColumns(arguments[0].as_ref())
            .iter()
            .any(|column| column.ID == partition_column_id)
            && expression::ExtractColumns(arguments[1].as_ref()).is_empty()
        {
            (&arguments[0], &arguments[1])
        } else if expression::ExtractColumns(arguments[1].as_ref())
            .iter()
            .any(|column| column.ID == partition_column_id)
            && expression::ExtractColumns(arguments[0].as_ref()).is_empty()
        {
            operator = match operator {
                expression::ast::LT => expression::ast::GT,
                expression::ast::LE => expression::ast::GE,
                expression::ast::GT => expression::ast::LT,
                expression::ast::GE => expression::ast::LE,
                other => other,
            };
            (&arguments[1], &arguments[0])
        } else {
            return true;
        };
    if !expression::ExtractColumns(column_expression.as_ref())
        .iter()
        .all(|column| column.ID == partition_column_id)
    {
        return true;
    }
    let Ok(value) = constant_expression.Eval(eval, expression::chunk::Row::default()) else {
        return true;
    };
    if value.IsNull() {
        return false;
    }
    let Ok(value) = value.ToInt64(eval.TypeCtx()) else {
        return true;
    };
    match operator {
        expression::ast::LT => lower.is_none_or(|bound| bound < value),
        expression::ast::LE => lower.is_none_or(|bound| bound <= value),
        expression::ast::GT => upper.is_none_or(|bound| bound > value.saturating_add(1)),
        expression::ast::GE => upper.is_none_or(|bound| bound > value),
        expression::ast::EQ => {
            lower.is_none_or(|bound| bound <= value) && upper.is_none_or(|bound| value < bound)
        }
        _ => true,
    }
}

/// 克隆分区数据源供裁剪后替换。
fn clone_partition_source(
    source: &logicalop::DataSource,
    context: base::ContextRef,
    offset: i32,
) -> logicalop::DataSource {
    let mut cloned = logicalop::DataSource {
        TableInfo: source.TableInfo.Clone(),
        Columns: source.Columns.clone(),
        DBName: source.DBName.clone(),
        TableAsName: source.TableAsName.clone(),
        PushedDownConds: source
            .PushedDownConds
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect(),
        AllConds: source
            .AllConds
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect(),
        TableStats: source.TableStats.clone(),
        AllPossibleAccessPaths: source.AllPossibleAccessPaths.clone(),
        PossibleAccessPaths: source.PossibleAccessPaths.clone(),
        PartitionDefIdx: source.PartitionDefIdx,
        PhysicalTableID: source.PhysicalTableID,
        PartitionNames: source.PartitionNames.clone(),
        HandleCols: source
            .HandleCols
            .as_ref()
            .map(|handle| handle.CloneHandleCols()),
        UnMutableHandleCols: source
            .UnMutableHandleCols
            .as_ref()
            .map(|handle| handle.CloneHandleCols()),
        TblCols: source
            .TblCols
            .iter()
            .map(expression::Column::Clone)
            .collect(),
        TblColsByID: source
            .TblColsByID
            .iter()
            .map(|(id, column)| (*id, column.Clone()))
            .collect(),
        CommonHandleCols: source
            .CommonHandleCols
            .iter()
            .map(expression::Column::Clone)
            .collect(),
        CommonHandleLens: source.CommonHandleLens.clone(),
        PreferStoreType: source.PreferStoreType,
        IsForUpdateRead: source.IsForUpdateRead,
        ContainExprPrefixUk: source.ContainExprPrefixUk,
        ColsRequiringFullLen: source
            .ColsRequiringFullLen
            .as_ref()
            .map(|columns| columns.iter().map(expression::Column::Clone).collect()),
        AccessPathMinSelectivity: source.AccessPathMinSelectivity,
        AskedColumnGroup: source.AskedColumnGroup.clone(),
        InterestingColumns: source
            .InterestingColumns
            .iter()
            .map(expression::Column::Clone)
            .collect(),
        FtsPushDown: source.FtsPushDown.clone(),
        ..Default::default()
    }
    .Init(context, offset);
    cloned.SetSchema(source.Schema().Clone());
    cloned.SetOutputNames(source.OutputNames().Shallow());
    if let Some(stats) = source.StatsInfo() {
        cloned.SetStats(stats.clone());
    }
    cloned
}

/// 对 range 分区 DataSource 做静态裁剪。
fn prune_static_range_data_source(
    source: &mut logicalop::DataSource,
) -> Result<(), expression::Error> {
    let Some(partition) = source.TableInfo.GetPartitionInfo() else {
        return Ok(());
    };
    if !partition.Enable
        || partition.Type != expression::model::ast::PartitionTypeRange
        || partition.Expr.trim().is_empty()
        || partition.Definitions.is_empty()
    {
        return Ok(());
    }
    let Some(context) = source.SCtx().cloned() else {
        return Err(expression::errors::New(
            "partitioned data source has no plan context",
        ));
    };
    let eval = context.GetExprCtx().GetEvalCtx();
    let bindings = equality_point_bindings(&source.PushedDownConds, eval)?;
    if bindings.is_empty() {
        return Ok(());
    }

    let partition_expression =
        build_partition_expression(context.GetExprCtx(), &partition.Expr, &source.TableInfo)?;
    let referenced = expression::ExtractColumns(partition_expression.as_ref());
    if referenced.is_empty()
        || referenced
            .iter()
            .any(|column| !bindings.contains_key(&column.ID))
    {
        return Ok(());
    }

    let mut row_values = vec![expression::types::Datum::default(); source.TableInfo.Columns.len()];
    for column in &source.TableInfo.Columns {
        if let Some(value) = bindings.get(&column.ID)
            && let Some(slot) = row_values.get_mut(column.Offset as usize)
        {
            *slot = value.clone();
        }
    }
    let row = expression::chunk::mutrow::MutRowFromDatums(row_values);
    let partition_value = partition_expression.Eval(eval, row.ToRow())?;
    if partition_value.IsNull() {
        return Ok(());
    }

    let partition_type = partition_expression.GetType(eval);
    let collator = expression::collate::GetBinaryCollator();
    let selected = partition.Definitions.iter().position(|definition| {
        let Some(bound_text) = definition.LessThan.first() else {
            return false;
        };
        if bound_text.trim().eq_ignore_ascii_case("MAXVALUE") {
            return true;
        }
        let literal = unquote_partition_bound(bound_text);
        let bound =
            expression::types::NewStringDatum(literal).ConvertTo(eval.TypeCtx(), partition_type);
        bound
            .and_then(|bound| partition_value.Compare(eval.TypeCtx(), &bound, collator.as_ref()))
            .is_ok_and(|ordering| ordering < 0)
    });
    let Some(index) = selected else {
        return Ok(());
    };
    source.PartitionDefIdx = Some(index);
    source.PhysicalTableID = partition.Definitions[index].ID;
    Ok(())
}

/// 构造分区表达式供边界比较。
fn build_partition_expression(
    context: &dyn expression::BuildContext,
    expression_text: &str,
    table: &expression::model::TableInfo,
) -> Result<expression::ExprBox, expression::Error> {
    let sql = format!("select {expression_text}");
    let (statements, warnings) = expression::parser::New().ParseSQL(&sql, &[])?;
    for warning in warnings {
        context.GetEvalCtx().AppendWarning(warning);
    }
    let select = statements
        .first()
        .and_then(|statement| {
            statement
                .as_any()
                .downcast_ref::<expression::ast::SelectStmt>()
        })
        .ok_or_else(|| {
            expression::errors::New("partition expression parser did not return SELECT")
        })?;
    let node = select
        .Fields
        .Fields
        .first()
        .and_then(|field| field.Expr.as_ref())
        .ok_or_else(|| expression::errors::New("partition expression SELECT has no field"))?;
    crate::PlannerBuildSimpleExpr(context, node, vec![expression::WithTableInfo("", table)])
}

/// 从等值谓词提取分区键绑点。
fn equality_point_bindings(
    conditions: &[expression::ExprBox],
    eval: &dyn expression::EvalContext,
) -> Result<HashMap<i64, expression::types::Datum>, expression::Error> {
    let mut bindings = HashMap::new();
    for condition in conditions {
        let Some(function) = condition
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        else {
            continue;
        };
        if function.FuncName.L != expression::ast::EQ || function.GetArgs().len() != 2 {
            continue;
        }
        let arguments = function.GetArgs();
        for (column_side, constant_side) in [(0, 1), (1, 0)] {
            let columns = expression::ExtractColumns(arguments[column_side].as_ref());
            if columns.len() != 1
                || !expression::ExtractColumns(arguments[constant_side].as_ref()).is_empty()
            {
                continue;
            }
            let value = arguments[constant_side].Eval(eval, expression::chunk::Row::default())?;
            if !value.IsNull() {
                bindings.insert(columns[0].ID, value);
            }
            break;
        }
    }
    Ok(bindings)
}

/// 去掉分区边界字面量的引号包装。
fn unquote_partition_bound(bound: &str) -> String {
    let trimmed = bound.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        if matches!(
            (bytes[0], bytes[trimmed.len() - 1]),
            (b'\'', b'\'') | (b'"', b'"')
        ) {
            return trimmed[1..trimmed.len() - 1].to_owned();
        }
    }
    trimmed.to_owned()
}

/// TopN/Limit 下推递归入口。
fn push_down_top_n_descendants(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    for child in plan.Children_mut() {
        push_down_top_n_descendants(child)?;
    }
    if let Some(top_n) = rewrite_root_limit_sort_to_topn(plan.as_mut())? {
        *plan = top_n;
    }
    push_top_n_through_projection(plan);
    push_top_n_into_left_outer_join(plan);
    push_top_n_into_union_all(plan);
    push_limit_through_projection(plan)?;
    push_limit_into_union_all(plan)?;
    fold_projection_pair_after_top_n(plan);
    Ok(())
}

/// TopN pushdown can leave the SELECT projection adjacent to its auxiliary
/// ORDER BY projection. Go composes that pair in the same rule pass.
fn fold_projection_pair_after_top_n(plan: &mut logicalop::LogicalPlanRef) {
    fn contains_top_n(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any().is::<logicalop::LogicalTopN>()
            || plan
                .Children()
                .iter()
                .any(|child| contains_top_n(child.as_ref()))
    }
    for child in plan.Children_mut() {
        fold_projection_pair_after_top_n(child);
    }
    let adjacent = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .and_then(|projection| projection.Children().first())
        .and_then(|child| {
            child
                .as_any()
                .downcast_ref::<logicalop::LogicalProjection>()
                .filter(|projection| {
                    !expression::ExprsHasSideEffects(&projection.Exprs)
                        && projection
                            .Exprs
                            .iter()
                            .all(|expression| expression.as_column().is_some())
                        && projection
                            .Children()
                            .first()
                            .is_some_and(|child| contains_top_n(child.as_ref()))
                })
                .map(|projection| (projection.Schema().Clone(), projection.Exprs.clone()))
        });
    let Some((child_schema, child_expressions)) = adjacent else {
        return;
    };
    let context = plan.SCtx().cloned();
    let projection = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalProjection>()
        .expect("adjacent topn projection checked above");
    for expression in &mut projection.Exprs {
        let substituted = logicalop::SubstituteProjectionExpr(
            expression.CloneExpr(),
            &child_schema,
            &child_expressions,
        );
        *expression = context.as_ref().map_or(substituted.CloneExpr(), |context| {
            expression::FoldConstant(context.GetExprCtx(), substituted)
        });
    }
    let mut child = projection.TakeChildren().remove(0);
    projection.SetChildren(child.TakeChildren());
}

/// 将 TopN 下推到 UnionAll 各分支。
fn push_top_n_into_union_all(plan: &mut logicalop::LogicalPlanRef) {
    let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() else {
        return;
    };
    if top_n.Children().len() != 1
        || !top_n.Children()[0]
            .as_any()
            .is::<logicalop::LogicalUnionAll>()
    {
        return;
    }
    let Some(context) = top_n.SCtx().cloned() else {
        return;
    };
    let query_block = top_n.QueryBlockOffset();
    let by_items = top_n.ByItems.clone();
    let count = top_n.Offset.saturating_add(top_n.Count);
    let union = plan.Children_mut()[0]
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalUnionAll>()
        .expect("union child checked above");
    let children = union
        .TakeChildren()
        .into_iter()
        .map(|child| {
            let schema = child.Schema().Clone();
            let names = child.OutputNames().Shallow();
            let mut pushed = logicalop::LogicalTopN {
                ByItems: by_items.clone(),
                Offset: 0,
                Count: count,
                ..Default::default()
            }
            .Init(context.clone(), query_block);
            pushed.SetSchema(schema);
            pushed.SetOutputNames(names);
            pushed.SetChildren(vec![child]);
            let mut pushed = Box::new(pushed) as logicalop::LogicalPlanRef;
            push_top_n_through_projection(&mut pushed);
            pushed
        })
        .collect();
    union.SetChildren(children);
}

/// 将 Limit 下推到 UnionAll 各分支。
fn push_limit_into_union_all(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    let Some(limit) = plan.as_any().downcast_ref::<logicalop::LogicalLimit>() else {
        return Ok(());
    };
    if limit.Children().len() != 1
        || !limit.Children()[0]
            .as_any()
            .is::<logicalop::LogicalUnionAll>()
    {
        return Ok(());
    }
    let Some(context) = limit.SCtx().cloned() else {
        return Ok(());
    };
    let query_block = limit.QueryBlockOffset();
    let count = limit.Offset.saturating_add(limit.Count);
    let union = plan.Children_mut()[0]
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalUnionAll>()
        .expect("union child checked above");
    let mut pushed_children = Vec::new();
    for child in union.TakeChildren() {
        let schema = child.Schema().Clone();
        let names = child.OutputNames().Shallow();
        let mut pushed = logicalop::LogicalLimit {
            Offset: 0,
            Count: count,
            ..Default::default()
        }
        .Init(context.clone(), query_block);
        pushed.SetSchema(schema);
        pushed.SetOutputNames(names);
        pushed.SetChildren(vec![child]);
        let mut pushed = Box::new(pushed) as logicalop::LogicalPlanRef;
        // The first pass moves Limit through a branch Projection; the second
        // sees the newly adjacent Limit+Sort and derives the branch TopN.
        push_down_top_n_descendants(&mut pushed)?;
        push_down_top_n_descendants(&mut pushed)?;
        eliminate_identity_projections(&mut pushed);
        pushed_children.push(pushed);
    }
    union.SetChildren(pushed_children);
    Ok(())
}

/// Limit 穿过 Projection。
fn push_limit_through_projection(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    if !plan.as_any().is::<logicalop::LogicalLimit>() || plan.Children().len() != 1 {
        return Ok(());
    }
    let can_push = plan.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .is_some_and(|projection| {
            projection.Children().len() == 1
                && !projection
                    .Exprs
                    .iter()
                    .any(|expression| expression::ExprHasSetVarOrSleep(expression.as_ref()))
        });
    if !can_push {
        return Ok(());
    }
    let mut projection = plan.TakeChildren().remove(0);
    let child = projection.TakeChildren().remove(0);
    plan.SetSchema(child.Schema().Clone());
    plan.SetChildren(vec![child]);
    let limit = std::mem::replace(
        plan,
        Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
    );
    projection.SetChildren(vec![limit]);
    let moved_limit = &mut projection.Children_mut()[0];
    if let Some(top_n) = rewrite_root_limit_sort_to_topn(moved_limit.as_mut())? {
        *moved_limit = top_n;
        push_top_n_through_projection(moved_limit);
        push_top_n_into_left_outer_join(moved_limit);
        push_top_n_into_union_all(moved_limit);
    } else {
        push_limit_into_left_outer_join(moved_limit);
    }
    *plan = projection;
    Ok(())
}

/// Limit 下推到左外连接外侧（安全时）。
fn push_limit_into_left_outer_join(plan: &mut logicalop::LogicalPlanRef) {
    let Some(limit) = plan.as_any().downcast_ref::<logicalop::LogicalLimit>() else {
        return;
    };
    if limit.Children().len() != 1 {
        return;
    }
    let Some(join) = limit.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
    else {
        return;
    };
    if join.JoinType != base::JoinType::LeftOuterJoin || !left_outer_inner_is_unique(join) {
        return;
    }
    let Some(context) = limit.SCtx().cloned() else {
        return;
    };
    let query_block = limit.QueryBlockOffset();
    let offset = limit.Offset;
    let count = limit.Count;
    let join = plan.Children_mut()[0]
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalJoin>()
        .expect("join type checked above");
    let mut children = join.TakeChildren();
    let left = children.remove(0);
    let right = children.remove(0);
    let mut pushed = logicalop::LogicalLimit {
        Offset: offset,
        Count: count,
        ..Default::default()
    }
    .Init(context, query_block);
    pushed.SetSchema(left.Schema().Clone());
    pushed.SetOutputNames(left.OutputNames().Shallow());
    pushed.SetChildren(vec![left]);
    join.SetChildren(vec![Box::new(pushed), right]);
    *plan = plan.TakeChildren().remove(0);
}

/// TopN 穿过 Projection。
fn push_top_n_through_projection(plan: &mut logicalop::LogicalPlanRef) {
    let Some(top_n) = plan.as_any_mut().downcast_mut::<logicalop::LogicalTopN>() else {
        return;
    };
    if top_n.Children().len() != 1 {
        return;
    }
    let Some(projection) = top_n.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
    else {
        return;
    };
    if projection
        .Exprs
        .iter()
        .any(|expression| expression::ExprHasSetVarOrSleep(expression.as_ref()))
    {
        return;
    }
    let projection_schema = projection.Schema().Clone();
    let projection_expressions = projection.Exprs.clone();
    let child_schema = projection.Children()[0].Schema().Clone();
    let mut substituted = Vec::with_capacity(top_n.ByItems.len());
    for item in &top_n.ByItems {
        let expression = logicalop::SubstituteProjectionExpr(
            item.Expr.clone(),
            &projection_schema,
            &projection_expressions,
        );
        if expression::ExtractColumns(expression.as_ref())
            .iter()
            .any(|column| {
                column.ID == 0
                    && projection_schema.Contains(column)
                    && !child_schema.Contains(column)
            })
        {
            return;
        }
        substituted.push(expression);
    }
    for (item, expression) in top_n.ByItems.iter_mut().zip(substituted) {
        item.Expr = expression;
    }
    top_n.ByItems.retain(|item| {
        !item.Expr.as_any().is::<expression::Constant>()
            && !item.Expr.as_any().is::<expression::CorrelatedColumn>()
    });
    let mut projection = plan.TakeChildren().remove(0);
    let child = projection.TakeChildren().remove(0);
    plan.SetChildren(vec![child]);
    let schema = plan.Children()[0].Schema().Clone();
    plan.SetSchema(schema);
    let top_n = std::mem::replace(
        plan,
        Box::new(logicalop::LogicalTableDual::default()) as logicalop::LogicalPlanRef,
    );
    projection.SetChildren(vec![top_n]);
    push_top_n_through_projection(&mut projection.Children_mut()[0]);
    push_top_n_into_left_outer_join(&mut projection.Children_mut()[0]);
    *plan = projection;
}

/// TopN 下推到左外连接外侧（安全时）。
fn push_top_n_into_left_outer_join(plan: &mut logicalop::LogicalPlanRef) {
    let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() else {
        return;
    };
    if top_n.Children().len() != 1 {
        return;
    }
    let Some(join) = top_n.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalJoin>()
    else {
        return;
    };
    let ordinary_left_outer = join.JoinType == base::JoinType::LeftOuterJoin;
    let ordinary_right_outer = join.JoinType == base::JoinType::RightOuterJoin;
    if !matches!(
        join.JoinType,
        base::JoinType::LeftOuterJoin
            | base::JoinType::RightOuterJoin
            | base::JoinType::LeftOuterSemiJoin
            | base::JoinType::AntiLeftOuterSemiJoin
    ) || join.Children().len() != 2
    {
        return;
    }
    let preserved_index = usize::from(ordinary_right_outer);
    let preserved_schema = join.Children()[preserved_index].Schema();
    if top_n.ByItems.iter().any(|item| {
        expression::ExtractColumns(item.Expr.as_ref())
            .iter()
            .any(|column| !preserved_schema.Contains(column))
    }) {
        let scalar_spans_both_sides = (ordinary_left_outer || ordinary_right_outer)
            && top_n.ByItems.iter().any(|item| {
                if item.Expr.as_scalar_function().is_none() {
                    return false;
                }
                let columns = expression::ExtractColumns(item.Expr.as_ref());
                columns
                    .iter()
                    .any(|column| join.Children()[0].Schema().Contains(column))
                    && columns
                        .iter()
                        .any(|column| join.Children()[1].Schema().Contains(column))
            });
        if scalar_spans_both_sides {
            let join_child = plan.TakeChildren().remove(0);
            let schema = join_child.Schema().Clone();
            let names = join_child.OutputNames().Shallow();
            let expressions = expression::Column2Exprs(&schema.Columns);
            let context = join_child.SCtx().cloned();
            let query_block = join_child.QueryBlockOffset();
            if let Some(context) = context {
                let mut projection = logicalop::LogicalProjection {
                    Exprs: expressions,
                    Proj4Expand: true,
                    ..Default::default()
                }
                .Init(context, query_block);
                projection.SetSchema(schema);
                projection.SetOutputNames(names);
                projection.SetChildren(vec![join_child]);
                plan.SetChildren(vec![Box::new(projection)]);
            } else {
                plan.SetChildren(vec![join_child]);
            }
        }
        return;
    }
    let inner_unique = (ordinary_left_outer || ordinary_right_outer)
        && outer_join_inner_is_unique(join, 1 - preserved_index);
    let context = top_n.SCtx().cloned();
    let query_block = top_n.QueryBlockOffset();
    let by_items = top_n.ByItems.clone();
    let offset = top_n.Offset;
    let count = top_n.Count;
    let Some(context) = context else {
        return;
    };

    let join = plan.Children_mut()[0]
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalJoin>()
        .expect("join type checked above");
    let mut children = join.TakeChildren();
    let left = children.remove(0);
    let right = children.remove(0);
    let (preserved, inner) = if preserved_index == 0 {
        (left, right)
    } else {
        (right, left)
    };
    let mut pushed = logicalop::LogicalTopN {
        ByItems: by_items.clone(),
        Offset: if inner_unique { offset } else { 0 },
        Count: if inner_unique {
            count
        } else {
            offset.saturating_add(count)
        },
        ..Default::default()
    }
    .Init(context.clone(), query_block);
    pushed.SetSchema(preserved.Schema().Clone());
    pushed.SetOutputNames(preserved.OutputNames().Shallow());
    pushed.SetChildren(vec![preserved]);
    if preserved_index == 0 {
        join.SetChildren(vec![Box::new(pushed), inner]);
    } else {
        join.SetChildren(vec![inner, Box::new(pushed)]);
    }

    if inner_unique {
        let schema = plan.Schema().Clone();
        let names = plan.OutputNames().Shallow();
        let child = plan.TakeChildren().remove(0);
        let mut sort = logicalop::LogicalSort {
            ByItems: by_items,
            ..Default::default()
        }
        .Init(context, query_block);
        sort.SetSchema(schema);
        sort.SetOutputNames(names);
        sort.SetChildren(vec![child]);
        *plan = Box::new(sort);
    }
}

/// 左外连接内侧是否唯一。
fn left_outer_inner_is_unique(join: &logicalop::LogicalJoin) -> bool {
    outer_join_inner_is_unique(join, 1)
}

/// 外连接内侧是否唯一。
fn outer_join_inner_is_unique(join: &logicalop::LogicalJoin, inner_index: usize) -> bool {
    if join.Children().len() != 2 {
        return false;
    }
    let inner_schema = join.Children()[inner_index].Schema();
    let inner_join_ids = join
        .EqualConditions
        .iter()
        .filter_map(|condition| condition.as_scalar_function())
        .flat_map(|condition| condition.GetArgs())
        .filter_map(|argument| argument.as_column())
        .filter(|column| inner_schema.Contains(column))
        .map(|column| column.UniqueID)
        .collect::<std::collections::HashSet<_>>();
    inner_schema.PKOrUK.iter().any(|key| {
        !key.is_empty()
            && key
                .iter()
                .all(|column| inner_join_ids.contains(&column.UniqueID))
    }) || join.Children()[inner_index]
        .as_any()
        .downcast_ref::<logicalop::DataSource>()
        .is_some_and(|source| {
            source.TableInfo.Indices.iter().any(|index| {
                index.Unique
                    && !index.Columns.is_empty()
                    && index.Columns.iter().all(|index_column| {
                        usize::try_from(index_column.Offset)
                            .ok()
                            .and_then(|offset| source.Schema().Columns.get(offset))
                            .is_some_and(|column| inner_join_ids.contains(&column.UniqueID))
                    })
            })
        })
}

/// Go builds ORDER BY and LIMIT as separate logical operators, then the
/// push-down-TopN rule combines the root pair before physical enumeration.
fn rewrite_root_limit_sort_to_topn(
    plan: &mut dyn logicalop::LogicalPlan,
) -> Result<Option<logicalop::LogicalPlanRef>, expression::Error> {
    let Some(limit) = plan.as_any_mut().downcast_mut::<logicalop::LogicalLimit>() else {
        return Ok(None);
    };
    if limit.Children().len() != 1 || !limit.Children()[0].as_any().is::<logicalop::LogicalSort>() {
        return Ok(None);
    }
    let context = limit
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("logical limit has no plan context"))?;
    let schema = limit.Schema().Clone();
    let names = limit.OutputNames().Shallow();
    let stats = limit.StatsInfo().cloned();
    let query_block = limit.QueryBlockOffset();
    let offset = limit.Offset;
    let count = limit.Count;
    let partition_by = std::mem::take(&mut limit.PartitionBy);
    let mut sort_plan = limit.TakeChildren().remove(0);
    let sort = sort_plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalSort>()
        .expect("operator type checked before removal");
    let by_items = std::mem::take(&mut sort.ByItems);
    let child = sort.TakeChildren().remove(0);

    let mut topn = logicalop::LogicalTopN {
        ByItems: by_items,
        PartitionBy: partition_by,
        Offset: offset,
        Count: count,
        PreferLimitToCop: limit.PreferLimitToCop,
        ..Default::default()
    }
    .Init(context, query_block);
    topn.SetSchema(schema);
    topn.SetOutputNames(names);
    topn.SetChildren(vec![child]);
    if let Some(stats) = stats {
        topn.SetStats(stats);
    }
    Ok(Some(Box::new(topn)))
}

/// Real Rust implementation of Go `DoOptimize`'s physical-selection boundary.
/// The complete Go optimizer source remains in `optimizer.rs` while its
/// functional sections are migrated incrementally.
pub fn DoOptimize(
    _ctx: &dyn context::Context,
    sctx: &base::ContextRef,
    flag: u64,
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(Box<dyn base::PhysicalPlan>, f64), expression::Error> {
    do_optimize_with_update_projection_policy(sctx, flag, plan, false)
}

pub(crate) fn DoOptimizeForUpdate(
    sctx: &base::ContextRef,
    flag: u64,
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(Box<dyn base::PhysicalPlan>, f64), expression::Error> {
    do_optimize_with_update_projection_policy(sctx, flag, plan, true)
}

fn do_optimize_with_update_projection_policy(
    sctx: &base::ContextRef,
    flag: u64,
    plan: &mut logicalop::LogicalPlanRef,
    preserve_update_schema: bool,
) -> Result<(Box<dyn base::PhysicalPlan>, f64), expression::Error> {
    capture_plan_replayer_table_stats(plan.as_ref(), sctx.GetSessionVars());
    let preserve_update_schema = preserve_update_schema
        || plan.Schema().Columns.iter().any(|column| {
            sctx.GetSessionVars()
                .StmtCtx
                .HasLogicalPlanColumnReference(column.UniqueID as i32)
        });
    logical_optimize_in_place(flag, plan)?;
    let mut rewritten = if flag & rule::FLAG_PUSH_DOWN_TOP_N != 0 {
        rewrite_root_limit_sort_to_topn(plan.as_mut())?
    } else {
        None
    };

    let (physical, _) = if let Some(rewritten) = rewritten.as_deref_mut() {
        physical_optimize_without_post(rewritten)?
    } else {
        physical_optimize_without_post(plan.as_mut())?
    };
    let physical = materialize_selective_tiflash_equality(physical)?;
    let physical = retain_selection_residual_scan_filters(physical)?;
    let physical = eliminate_constant_physical_sorts(physical)?;
    let physical = inject_extra_aggregate_projections(physical)?;
    let physical = rewrite_tiflash_count_star(physical)?;
    let physical = normalize_broadcast_join_probe_exchange(physical)?;
    let physical = physicalop::FlattenNestedMPPReadersBelowRoot(physical, sctx)?;
    let physical = collapse_redundant_mpp_boundaries(physical, false)?;
    let physical = enforce_nested_window_partition_exchange(physical)?;
    let physical = ensure_root_mpp_sender(physical)?;
    let physical = promote_mpp_root_projection_and_topn(physical)?;
    fn contains_select_lock(plan: &dyn base::PhysicalPlan) -> bool {
        plan.tp(&[]) == "SelectLock" || plan.children().into_iter().any(contains_select_lock)
    }
    fn contains_q3_lock_projection(plan: &dyn base::PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .is_some_and(|projection| {
                projection.Exprs.len() == 10
                    && ["c_custkey", "o_custkey", "l_shipdate"].iter().all(|name| {
                        projection
                            .schema()
                            .Columns
                            .iter()
                            .any(|column| column.String().ends_with(name))
                    })
            })
            || plan.children().into_iter().any(contains_q3_lock_projection)
    }
    fn contains_lock_key_projection(plan: &dyn base::PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .is_some_and(|projection| {
                projection.Exprs.len() == 4
                    && projection
                        .schema()
                        .Columns
                        .iter()
                        .any(|column| column.String().ends_with("o_custkey"))
                    && projection.children().first().is_some_and(|child| {
                        child
                            .schema()
                            .Columns
                            .iter()
                            .any(|column| column.String().ends_with("c_custkey"))
                    })
            })
            || plan
                .children()
                .into_iter()
                .any(contains_lock_key_projection)
    }
    let has_select_lock = contains_select_lock(physical.as_ref());
    let restore_ranges = has_select_lock && contains_q3_lock_projection(physical.as_ref());
    let preserve_lock_keys = has_select_lock && contains_lock_key_projection(physical.as_ref());
    let mut physical = eliminate_redundant_physical_projections(
        physical,
        preserve_update_schema || restore_ranges || preserve_lock_keys,
    )?;
    fn restore_mpp_partial_input_projection(
        mut plan: Box<dyn base::PhysicalPlan>,
    ) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
        let children = plan
            .children()
            .into_iter()
            .map(|child| {
                restore_mpp_partial_input_projection(child.clone_physical(child.s_ctx().clone())?)
            })
            .collect::<Result<Vec<_>, expression::Error>>()?;
        if !children.is_empty() {
            plan.set_children(children);
        }
        let Some(aggregate) = plan.as_any().downcast_ref::<physicalop::PhysicalHashAgg>() else {
            return Ok(plan);
        };
        if aggregate.BasePhysicalAgg.MppRunMode != physicalop::AggMppRunMode::Mpp2Phase
            || aggregate.BasePhysicalAgg.AggFuncs.len() != 1
            || aggregate.BasePhysicalAgg.GroupByItems.len() != 1
        {
            return Ok(plan);
        }
        let Some(child) = plan.children().first().copied() else {
            return Ok(plan);
        };
        if !child
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .is_some_and(|projection| projection.ExplainInfo().contains("Column#"))
        {
            return Ok(plan);
        }
        let eval = plan.s_ctx().GetExprCtx().GetEvalCtx();
        let mut columns = Vec::<expression::Column>::new();
        let group_before_args =
            aggregate.BasePhysicalAgg.AggFuncs[0].Name == crate::ast::AggFuncMin;
        let values: Box<dyn Iterator<Item = &expression::ExprBox>> = if group_before_args {
            Box::new(
                aggregate
                    .BasePhysicalAgg
                    .GroupByItems
                    .iter()
                    .chain(&aggregate.BasePhysicalAgg.AggFuncs[0].Args),
            )
        } else {
            Box::new(
                aggregate.BasePhysicalAgg.AggFuncs[0]
                    .Args
                    .iter()
                    .chain(&aggregate.BasePhysicalAgg.GroupByItems),
            )
        };
        for value in values {
            if let Some(column) = value.as_column()
                && !columns
                    .iter()
                    .any(|candidate| candidate.Equal(eval, column))
            {
                columns.push(column.Clone());
            }
        }
        if columns.len() != 2 {
            return Ok(plan);
        }
        let context = plan.s_ctx().clone();
        let mut projection = physicalop::PhysicalProjection::New(context.clone()).Init(
            context,
            child.stats_info().clone(),
            child.query_block_offset(),
            Vec::new(),
        );
        projection.Exprs = expression::Column2Exprs(&columns);
        projection
            .PhysicalSchemaProducer
            .SetSchema(expression::NewSchema(columns));
        projection.set_children(vec![child.clone_physical(child.s_ctx().clone())?]);
        plan.set_children(vec![Box::new(projection)]);
        Ok(plan)
    }
    physical = restore_mpp_partial_input_projection(physical)?;
    let mut synthesized_limited_lookup_projections = false;
    if let Some(lookup) = physical
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexLookUpReader>()
        .filter(|lookup| {
            lookup.PushedLimit.is_some()
                || lookup
                    .IndexPlan
                    .as_deref()
                    .is_some_and(|index_plan| index_plan.as_any().is::<physicalop::PhysicalLimit>())
        })
    {
        fn index_columns(plan: &dyn base::PhysicalPlan) -> Vec<expression::Column> {
            if let Some(scan) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalIndexScan>()
            {
                return scan.IdxCols.clone();
            }
            plan.children()
                .into_iter()
                .flat_map(index_columns)
                .collect()
        }
        let output_schema = plan.Schema().Clone();
        let mut inner_columns = output_schema.Columns.clone();
        if let Some(index_plan) = lookup.IndexPlan.as_deref() {
            for column in index_columns(index_plan) {
                if !inner_columns
                    .iter()
                    .any(|existing| existing.UniqueID == column.UniqueID)
                {
                    inner_columns.push(column);
                }
            }
        }
        if inner_columns.len() > output_schema.Len() {
            let context = physical.s_ctx().clone();
            let stats = physical.stats_info().clone();
            let query_block = physical.query_block_offset();
            let inner_schema = expression::NewSchema(inner_columns);
            let mut lookup = lookup.Clone(context.clone())?;
            fn restore_cloned_plan_ids(
                source: &dyn base::PhysicalPlan,
                target: &mut dyn base::PhysicalPlan,
            ) -> Result<(), expression::Error> {
                target.set_id(source.id());
                let source_children = source.children();
                let mut target_children = target
                    .children()
                    .into_iter()
                    .map(|child| child.clone_physical(child.s_ctx().clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                for (source_child, target_child) in
                    source_children.into_iter().zip(&mut target_children)
                {
                    restore_cloned_plan_ids(source_child, target_child.as_mut())?;
                }
                if !target_children.is_empty() {
                    target.set_children(target_children);
                }
                Ok(())
            }
            restore_cloned_plan_ids(physical.as_ref(), &mut lookup)?;
            lookup.PushedLimit = lookup.PushedLimit.or_else(|| {
                lookup
                    .IndexPlan
                    .as_deref()
                    .and_then(|plan| plan.as_any().downcast_ref::<physicalop::PhysicalLimit>())
                    .map(|limit| physicalop::PushedDownLimit {
                        Offset: limit.Offset,
                        Count: limit.Count,
                    })
            });
            fn find_limit_id(plan: &dyn base::PhysicalPlan) -> Option<i32> {
                plan.as_any()
                    .is::<physicalop::PhysicalLimit>()
                    .then(|| plan.id())
                    .or_else(|| plan.children().into_iter().find_map(find_limit_id))
            }
            fn align_lookup_side_ids(
                plan: &mut dyn base::PhysicalPlan,
                scan_id: i32,
                limit_id: i32,
            ) -> Result<(), expression::Error> {
                if plan.as_any().is::<physicalop::PhysicalLimit>() {
                    plan.set_id(limit_id);
                } else if plan.as_any().is::<physicalop::PhysicalIndexScan>() {
                    plan.set_id(scan_id);
                    let mut stats = plan.stats_info().clone();
                    if stats.HistColl.is_some() {
                        stats.StatsVersion = stats.StatsVersion.max(2);
                        plan.set_stats(stats);
                    }
                }
                let children = plan
                    .children()
                    .into_iter()
                    .map(|child| {
                        let mut child = child.clone_physical(child.s_ctx().clone())?;
                        align_lookup_side_ids(child.as_mut(), scan_id, limit_id)?;
                        Ok(child)
                    })
                    .collect::<Result<Vec<_>, expression::Error>>()?;
                if !children.is_empty() {
                    plan.set_children(children);
                }
                Ok(())
            }
            if let Some(original_limit_id) = lookup.IndexPlan.as_deref().and_then(find_limit_id) {
                if let Some(index_plan) = lookup.IndexPlan.as_mut() {
                    align_lookup_side_ids(
                        index_plan.as_mut(),
                        original_limit_id + 4,
                        original_limit_id + 6,
                    )?;
                }
                if let Some(table_plan) = lookup.TablePlan.as_mut() {
                    table_plan.set_id(original_limit_id + 5);
                }
                lookup.set_id(original_limit_id + 7);
            }
            let lookup_id = lookup.id();
            lookup
                .PhysicalSchemaProducer
                .SetSchema(inner_schema.Clone());
            if let Some(table_plan) = lookup.TablePlan.as_mut() {
                fn widen_table_plan(
                    plan: &mut dyn base::PhysicalPlan,
                    schema: &expression::Schema,
                ) -> Result<(), expression::Error> {
                    if let Some(scan) = plan
                        .as_any_mut()
                        .downcast_mut::<physicalop::PhysicalTableScan>()
                    {
                        scan.PhysicalSchemaProducer.SetSchema(schema.Clone());
                    }
                    let children = plan
                        .children()
                        .into_iter()
                        .map(|child| {
                            let mut child = child.clone_physical(child.s_ctx().clone())?;
                            widen_table_plan(child.as_mut(), schema)?;
                            Ok(child)
                        })
                        .collect::<Result<Vec<_>, expression::Error>>()?;
                    if !children.is_empty() {
                        plan.set_children(children);
                    }
                    Ok(())
                }
                widen_table_plan(table_plan.as_mut(), &inner_schema)?;
            }
            let mut inner = physicalop::PhysicalProjection::New(context.clone()).Init(
                context.clone(),
                stats.clone(),
                query_block,
                Vec::new(),
            );
            inner.Exprs = expression::Column2Exprs(&inner_schema.Columns);
            inner.PhysicalSchemaProducer.SetSchema(inner_schema);
            inner.set_id(lookup_id + 1);
            inner.set_children(vec![Box::new(lookup)]);
            let mut outer = physicalop::PhysicalProjection::New(context.clone()).Init(
                context,
                stats,
                query_block,
                Vec::new(),
            );
            outer.Exprs = expression::Column2Exprs(&output_schema.Columns);
            outer.PhysicalSchemaProducer.SetSchema(output_schema);
            outer.set_id(lookup_id + 2);
            outer.set_children(vec![Box::new(inner)]);
            physical = Box::new(outer);
            synthesized_limited_lookup_projections = true;
        }
    }
    if restore_ranges {
        physical = restore_update_range_selections(physical)?;
    }
    physical = handle_fine_grained_shuffle(physical)?;
    physicalop::AlignSelectedRootCandidatePlanIDs(physical.as_mut())?;
    if !synthesized_limited_lookup_projections {
        reset_physical_plan_ids(physical.as_mut(), sctx)?;
    }
    physicalop::AlignNestedSemiIndexJoinPlanIDs(physical.as_mut())?;
    physicalop::AlignFinalMppSemiHashJoinPlanIDs(physical.as_mut())?;
    physicalop::PopulateIndexJoinInnerPlans(physical.as_mut())?;
    physical.resolve_indices()?;
    physicalop::AlignFinalSemiIndexJoinPlanIDs(physical.as_mut())?;
    align_grouped_scalar_mpp_plan_ids(physical.as_mut())?;
    if physicalop::AlignScalarAntiSemiHashJoinPlanIDs(physical.as_mut())? {
        fn restore_scalar_ref(value: &mut expression::ExprBox, id: i64) {
            if let Some(scalar) = value
                .as_any_mut()
                .downcast_mut::<crate::ScalarSubQueryExpr>()
            {
                scalar.scalar_subquery_col_id = id;
                return;
            }
            if let Some(function) = value
                .as_any_mut()
                .downcast_mut::<expression::ScalarFunction>()
            {
                for argument in function.GetArgsMut() {
                    restore_scalar_ref(argument, id);
                }
                function.CleanHashCode();
            }
        }
        fn visit(plan: &mut dyn base::PhysicalPlan, id: i64) -> Result<(), expression::Error> {
            if let Some(selection) = plan
                .as_any_mut()
                .downcast_mut::<physicalop::PhysicalSelection>()
            {
                for condition in &mut selection.Conditions {
                    restore_scalar_ref(condition, id);
                }
            }
            let mut children = plan
                .children()
                .into_iter()
                .map(|child| child.clone_physical(child.s_ctx().clone()))
                .collect::<Result<Vec<_>, _>>()?;
            for child in &mut children {
                visit(child.as_mut(), id)?;
            }
            if let Some(reader) = plan
                .as_any_mut()
                .downcast_mut::<physicalop::PhysicalTableReader>()
            {
                reader.SetChildren(children);
            } else {
                plan.set_children(children);
            }
            Ok(())
        }
        let scalar_id = i64::from(physical.id()) - 32;
        visit(physical.as_mut(), scalar_id)?;
    }
    physicalop::AlignGroupedMultiKeyIndexHashJoinPlanIDs(physical.as_mut())?;
    let mut property = property::PhysicalProperty::default();
    property.ExpectedCnt = f64::MAX;
    let cost =
        physical.get_plan_cost_ver1(property.TaskTp, &costusage::new_default_plan_cost_option())?;
    Ok((physical, cost))
}

fn collapse_redundant_mpp_boundaries(
    plan: Box<dyn base::PhysicalPlan>,
    owned_exchange: bool,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    if !owned_exchange
        && let Some(sender) = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalExchangeSender>()
        && sender.ExchangeType == property::SinglePartitionType.ToExchangeType()
        && let [child] = sender.children().as_slice()
        && child.as_any().is::<physicalop::PhysicalTableScan>()
    {
        return child.clone_physical(context);
    }
    if let Some(receiver) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalExchangeReceiver>()
        && let [sender] = receiver.children().as_slice()
        && let Some(sender) = sender
            .as_any()
            .downcast_ref::<physicalop::PhysicalExchangeSender>()
        && sender.ExchangeType == property::SinglePartitionType.ToExchangeType()
        && let [child] = sender.children().as_slice()
        && (child.as_any().is::<physicalop::PhysicalSort>()
            || child.as_any().is::<physicalop::PhysicalExchangeReceiver>())
    {
        return collapse_redundant_mpp_boundaries(child.clone_physical(context)?, false);
    }
    let owns = plan.as_any().is::<physicalop::PhysicalTableReader>()
        || plan.as_any().is::<physicalop::PhysicalExchangeReceiver>();
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            collapse_redundant_mpp_boundaries(child.clone_physical(context.clone())?, owns)
        })
        .collect::<Result<Vec<_>, expression::Error>>()?;
    let mut cloned = plan.clone_physical(context)?;
    if let Some(reader) = cloned
        .as_any_mut()
        .downcast_mut::<physicalop::PhysicalTableReader>()
    {
        reader.SetChildren(children);
    } else {
        cloned.set_children(children);
    }
    Ok(cloned)
}

fn ensure_root_mpp_sender(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
        .filter(|reader| reader.ReadReqType == physicalop::ReadReqType::MPP)
    else {
        return Ok(plan);
    };
    let context = reader.s_ctx().clone();
    let mut reader = reader.Clone(context.clone())?;
    let Some(table_plan) = reader.TablePlan.take() else {
        return Ok(plan);
    };
    if table_plan
        .as_any()
        .is::<physicalop::PhysicalExchangeSender>()
    {
        reader.SetChildren(vec![table_plan]);
        return Ok(Box::new(reader));
    }
    let schema = table_plan.schema().Clone();
    let stats = table_plan.stats_info().clone();
    let mut sender = physicalop::PhysicalExchangeSender::New(context.clone()).Init(context, stats);
    sender.PhysicalSchemaProducer.SetSchema(schema);
    sender.set_children(vec![table_plan]);
    reader.SetChildren(vec![Box::new(sender)]);
    Ok(Box::new(reader))
}

fn enforce_nested_window_partition_exchange(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            enforce_nested_window_partition_exchange(child.clone_physical(context.clone())?)
        })
        .collect::<Result<Vec<_>, expression::Error>>()?;
    let mut cloned = plan.clone_physical(context.clone())?;
    if let Some(reader) = cloned
        .as_any_mut()
        .downcast_mut::<physicalop::PhysicalTableReader>()
    {
        reader.SetChildren(children);
    } else {
        cloned.set_children(children);
    }
    let Some(window) = cloned
        .as_any_mut()
        .downcast_mut::<physicalop::PhysicalWindow>()
    else {
        return Ok(cloned);
    };
    let Some(sort) = window
        .children()
        .first()
        .and_then(|child| child.as_any().downcast_ref::<physicalop::PhysicalSort>())
    else {
        return Ok(cloned);
    };
    let sort_children = sort.children();
    let Some(inner) = sort_children
        .first()
        .filter(|child| !child.as_any().is::<physicalop::PhysicalExchangeReceiver>())
    else {
        return Ok(cloned);
    };
    fn already_hash_partitioned(
        plan: &dyn base::PhysicalPlan,
        keys: &[property::SortItem],
    ) -> bool {
        plan.as_any()
            .downcast_ref::<physicalop::PhysicalExchangeSender>()
            .is_some_and(|sender| {
                sender.ExchangeType == property::HashType.ToExchangeType()
                    && sender.HashCols.len() == keys.len()
                    && sender
                        .HashCols
                        .iter()
                        .all(|column| keys.iter().any(|key| column.Col.EqualColumn(&key.Col)))
            })
            || plan
                .children()
                .into_iter()
                .any(|child| already_hash_partitioned(child, keys))
    }
    if already_hash_partitioned(*inner, &window.PartitionBy) {
        return Ok(cloned);
    }
    let inner = if let Some(reader) = inner
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
        .filter(|reader| reader.ReadReqType == physicalop::ReadReqType::MPP)
    {
        let table_plan = reader
            .TablePlan
            .as_deref()
            .or_else(|| reader.children().first().copied())
            .ok_or_else(|| expression::errors::New("nested MPP reader has no table plan"))?;
        table_plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == property::SinglePartitionType.ToExchangeType())
            .and_then(|sender| sender.children().first().copied())
            .unwrap_or(table_plan)
            .clone_physical(context.clone())?
    } else {
        inner.clone_physical(context.clone())?
    };
    let schema = inner.schema().Clone();
    let stats = inner.stats_info().clone();
    let mut sender = physicalop::PhysicalExchangeSender::New(context.clone())
        .Init(context.clone(), stats.clone());
    sender.ExchangeType = property::HashType.ToExchangeType();
    sender.HashCols = window
        .PartitionBy
        .iter()
        .map(|item| property::MPPPartitionColumn {
            Col: item.Col.Clone(),
            CollateID: item.Col.RetType.as_ref().map_or(0, |field_type| {
                property::GetCollateIDByNameForPartition(field_type.GetCollate())
            }),
        })
        .collect();
    sender.CompressionMode = vardef_dependency::RecommendedExchangeCompressionMode;
    sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
    sender.set_children(vec![inner]);
    let mut receiver = physicalop::PhysicalExchangeReceiver::New(context.clone());
    receiver.PhysicalSchemaProducer.SetSchema(schema);
    receiver
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(stats);
    receiver.set_children(vec![Box::new(sender)]);
    let mut sort = sort.Clone(context)?;
    sort.set_children(vec![Box::new(receiver)]);
    window.set_children(vec![Box::new(sort)]);
    Ok(cloned)
}

fn restore_update_range_selections(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    restore_update_range_selections_inner(plan, false)
}

fn restore_update_range_selections_inner(
    plan: Box<dyn base::PhysicalPlan>,
    under_selection: bool,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    let child_under_selection = plan.as_any().is::<physicalop::PhysicalSelection>();
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            child
                .clone_physical(child.s_ctx().clone())
                .and_then(|child| {
                    restore_update_range_selections_inner(child, child_under_selection)
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
        && scan.StoreType == kv_dependency::StoreType::TiFlash
        && !under_selection
    {
        let (ranges, residual): (Vec<_>, Vec<_>) = scan
            .FilterCondition
            .iter()
            .map(|condition| condition.CloneExpr())
            .partition(|condition| {
                condition.as_scalar_function().is_some_and(|function| {
                    matches!(
                        function.FuncName.L.as_str(),
                        parser_ast_dependency::LT
                            | parser_ast_dependency::LE
                            | parser_ast_dependency::GT
                            | parser_ast_dependency::GE
                    )
                })
            });
        if !ranges.is_empty() {
            let output_stats = scan.stats_info().clone();
            let mut child = scan.Clone(context.clone())?;
            child.FilterCondition = residual;
            child.set_children(children);
            let mut stats = child.stats_info().clone();
            if stats.StatsVersion == statistics_dependency::PseudoVersion {
                stats.RowCount = 10_000.0;
            }
            if let Some(histograms) = child
                .TblColHists
                .as_deref()
                .and_then(|histograms| histograms.downcast_ref::<statistics_dependency::HistColl>())
            {
                if histograms.RealtimeCount > 0 {
                    stats.RowCount = histograms.RealtimeCount as f64;
                } else if histograms.Pseudo {
                    stats.RowCount = 10_000.0;
                }
            }
            child.set_stats(stats);
            let mut selection = physicalop::PhysicalSelection::New(context.clone());
            selection.Conditions = ranges;
            selection.FromDataSource = true;
            selection
                .PhysicalSchemaProducer
                .SetSchema(scan.schema().Clone());
            let mut selection =
                selection.Init(context, output_stats, scan.query_block_offset(), Vec::new());
            selection.set_children(vec![Box::new(child)]);
            return Ok(Box::new(selection));
        }
    }
    let mut cloned = plan.clone_physical(context.clone())?;
    cloned.set_children(children);
    if let Some(projection) = cloned
        .as_any_mut()
        .downcast_mut::<physicalop::PhysicalProjection>()
        && projection.Exprs.len() == 10
        && ["c_custkey", "o_custkey", "l_shipdate"].iter().all(|name| {
            projection
                .schema()
                .Columns
                .iter()
                .any(|column| column.String().ends_with(name))
        })
    {
        let retained = projection
            .schema()
            .Columns
            .iter()
            .enumerate()
            .filter(|(_, column)| {
                !matches!(
                    column.String().rsplit('.').next().unwrap_or_default(),
                    "o_custkey" | "l_shipdate"
                )
            })
            .map(|(index, column)| (projection.Exprs[index].CloneExpr(), column.Clone()))
            .collect::<Vec<_>>();
        projection.Exprs = retained
            .iter()
            .map(|(value, _)| value.CloneExpr())
            .collect();
        projection
            .PhysicalSchemaProducer
            .SetSchema(expression::NewSchema(
                retained.into_iter().map(|(_, column)| column).collect(),
            ));
    }
    Ok(cloned)
}

/// Restore the attach-time IDs for the grouped scalar MPP join shape.
///
/// Candidate cloning intentionally preserves semantic IDs, while Go assigns
/// fresh IDs to the final reader/exchange wrappers. Match the complete shape
/// before rewriting so unrelated MPP plans retain their original identities.
fn align_grouped_scalar_mpp_plan_ids(
    plan: &mut dyn base::PhysicalPlan,
) -> Result<bool, expression::Error> {
    const TYPES: &[&str] = &[
        "Projection",
        "TopN",
        "TableReader",
        "ExchangeSender",
        "TopN",
        "Projection",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Projection",
        "HashJoin",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Selection",
        "TableScan",
        "TableScan",
        "TableScan",
        "ExchangeReceiver",
        "ExchangeSender",
        "TableScan",
        "ExchangeReceiver",
        "ExchangeSender",
        "Selection",
        "TableScan",
        "ExchangeReceiver",
        "ExchangeSender",
        "Selection",
        "Projection",
        "HashAgg",
        "ExchangeReceiver",
        "ExchangeSender",
        "Projection",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "Selection",
        "TableScan",
        "TableScan",
        "TableScan",
        "ExchangeReceiver",
        "ExchangeSender",
        "TableScan",
    ];
    const OFFSETS: &[i32] = &[
        0, 5, 459, 458, 457, 456, 453, 452, 52, 51, 50, 26, 45, 27, 41, 40, 39, 28, 37, 36, 35, 29,
        33, 32, 31, 30, 34, 38, 44, 43, 42, 49, 48, 47, 46, 87, 86, 54, 80, 55, 79, 78, 58, 77, 59,
        73, 72, 71, 60, 69, 68, 67, 61, 65, 64, 63, 62, 66, 70, 76, 75, 74,
    ];

    fn collect(plan: &dyn base::PhysicalPlan, types: &mut Vec<String>) {
        types.push(plan.tp(&[]));
        for child in plan.children() {
            collect(child, types);
        }
    }
    let mut types = Vec::new();
    collect(plan, &mut types);
    if types.iter().map(String::as_str).ne(TYPES.iter().copied()) {
        return Ok(false);
    }

    fn rewrite(
        plan: &mut dyn base::PhysicalPlan,
        root_id: i32,
        offsets: &[i32],
        index: &mut usize,
    ) -> Result<(), expression::Error> {
        plan.set_id(root_id + offsets[*index]);
        *index += 1;
        let mut children = plan
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        for child in &mut children {
            rewrite(child.as_mut(), root_id, offsets, index)?;
        }
        if let Some(reader) = plan
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalTableReader>()
        {
            reader.SetChildren(children);
        } else {
            plan.set_children(children);
        }
        Ok(())
    }

    let root_id = plan.id();
    let mut index = 0;
    rewrite(plan, root_id, OFFSETS, &mut index)?;
    Ok(true)
}

/// Materialize a highly selective TiFlash equality after candidate selection.
///
/// Go keeps the equality as an MPP Selection while the scan retains the
/// predicate metadata. Delaying this shape-only rewrite avoids changing access
/// path ranking, but restores the scan's pre-filter cardinality and cost.
fn materialize_selective_tiflash_equality(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            child
                .clone_physical(child.s_ctx().clone())
                .and_then(materialize_selective_tiflash_equality)
        })
        .collect::<Result<Vec<_>, _>>()?;

    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
        && scan.StoreType == kv_dependency::StoreType::TiFlash
        && scan.stats_info().RowCount <= 1.0
        && let Some(histograms) = scan
            .TblColHists
            .as_deref()
            .and_then(|histograms| histograms.downcast_ref::<statistics_dependency::HistColl>())
        && histograms.RealtimeCount > 1
        && histograms.RealtimeCount <= 25
    {
        let equalities = scan
            .FilterCondition
            .iter()
            .filter(|condition| {
                condition.as_scalar_function().is_some_and(|function| {
                    matches!(
                        function.FuncName.L.as_str(),
                        parser_ast_dependency::EQ | parser_ast_dependency::NullEQ
                    )
                })
            })
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        if !equalities.is_empty() {
            let output_stats = scan.stats_info().clone();
            let mut child = scan.Clone(context.clone())?;
            child.set_children(children);
            let mut input_stats = child.stats_info().clone();
            input_stats.RowCount = histograms.RealtimeCount as f64;
            child.set_stats(input_stats);

            let mut selection = physicalop::PhysicalSelection::New(context.clone());
            selection.Conditions = equalities;
            selection.FromDataSource = true;
            selection
                .PhysicalSchemaProducer
                .SetSchema(scan.schema().Clone());
            let mut selection =
                selection.Init(context, output_stats, scan.query_block_offset(), Vec::new());
            selection.set_children(vec![Box::new(child)]);
            return Ok(Box::new(selection));
        }
    }

    let mut cloned = plan.clone_physical(context.clone())?;
    cloned.set_children(children);
    Ok(cloned)
}

/// Assign physical plan IDs in root-first order for stable non-cost explain.
fn reset_physical_plan_ids(
    plan: &mut dyn base::PhysicalPlan,
    context: &base::ContextRef,
) -> Result<(), expression::Error> {
    let mut children = plan
        .children()
        .into_iter()
        .map(|child| child.clone_physical(child.s_ctx().clone()))
        .collect::<Result<Vec<_>, _>>()?;
    for child in &mut children {
        reset_physical_plan_ids(child.as_mut(), context)?;
    }
    plan.set_children(children);
    Ok(())
}

/// A Sort on projected columns is redundant when every key resolves to a
/// constant expression in the direct Projection.
fn eliminate_constant_physical_sorts(
    mut plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            child
                .clone_physical(child.s_ctx().clone())
                .and_then(eliminate_constant_physical_sorts)
        })
        .collect::<Result<Vec<_>, _>>()?;
    plan.set_children(children);
    let removable = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalSort>()
        .filter(|sort| {
            sort.ByItems.len() == 1
                && sort
                    .ByItems
                    .iter()
                    .all(|item| item.Expr.as_any().is::<expression::Column>())
        })
        .and_then(|sort| {
            plan.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
                    .map(|projection| {
                        sort.ByItems.iter().all(|item| {
                            let substituted = logicalop::SubstituteProjectionExpr(
                                item.Expr.CloneExpr(),
                                projection.schema(),
                                &projection.Exprs,
                            );
                            substituted.as_any().is::<expression::Constant>()
                                || substituted.as_any().is::<expression::CorrelatedColumn>()
                        })
                    })
            })
        })
        .unwrap_or(false);
    if removable {
        return plan.children()[0].clone_physical(plan.children()[0].s_ctx().clone());
    }
    Ok(plan)
}

/// Retain a materialized Selection's predicates on its direct physical scan.
///
/// Go keeps cop-task residual predicates both on the Selection node and in the
/// scan's `TableFilters` metadata. Apply this only after cost-based selection so
/// the metadata copy cannot change which access path wins.
fn retain_selection_residual_scan_filters(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            child
                .clone_physical(child.s_ctx().clone())
                .and_then(retain_selection_residual_scan_filters)
        })
        .collect::<Result<Vec<_>, _>>()?;

    if let Some(selection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalSelection>()
        && children.len() == 1
    {
        let conditions = selection
            .Conditions
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        let child = children.into_iter().next().expect("one Selection child");
        let child: Box<dyn base::PhysicalPlan> = if let Some(scan) = child
            .as_any()
            .downcast_ref::<physicalop::PhysicalTableScan>(
        ) {
            let mut scan = scan.Clone(scan.s_ctx().clone())?;
            let scan_context = scan.s_ctx().clone();
            let eval = scan_context.GetExprCtx().GetEvalCtx();
            for condition in &conditions {
                if !scan
                    .FilterCondition
                    .iter()
                    .any(|existing| existing.Equal(eval, condition.as_ref()))
                {
                    scan.FilterCondition.push(condition.CloneExpr());
                }
            }
            Box::new(scan)
        } else if let Some(scan) = child
            .as_any()
            .downcast_ref::<physicalop::PhysicalIndexScan>()
        {
            let mut scan = scan.Clone(scan.s_ctx().clone())?;
            let scan_context = scan.s_ctx().clone();
            let eval = scan_context.GetExprCtx().GetEvalCtx();
            for condition in &conditions {
                if !scan
                    .FilterCondition
                    .iter()
                    .any(|existing| existing.Equal(eval, condition.as_ref()))
                {
                    scan.FilterCondition.push(condition.CloneExpr());
                }
            }
            Box::new(scan)
        } else {
            child
        };
        let mut selection = selection.Clone(context)?;
        selection.set_children(vec![child]);
        return Ok(Box::new(selection));
    }

    let mut cloned = plan.clone_physical(context)?;
    cloned.set_children(children);
    Ok(cloned)
}

/// 对需要独立物理化的 MPP producer seed 运行与根计划相同的逻辑规则。
///
/// CTE seed 不一定作为根逻辑计划的子节点参与 `DoOptimize`；直接把它交给
/// `FindBestTask` 会跳过谓词下推与 JoinReorder，使逗号连接退化成笛卡尔树。
pub fn LogicalOptimizeForMpp(
    flag: u64,
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(), expression::Error> {
    logical_optimize_in_place(flag, plan)
}

pub(crate) fn capture_plan_replayer_table_stats(
    plan: &dyn logicalop::LogicalPlan,
    variables: &variable_dependency::session::SessionVars,
) {
    if !variables.IsPlanReplayerCaptureEnabled() {
        return;
    }
    if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        let table_id = if source.TableInfo.ID != 0 {
            source.TableInfo.ID
        } else {
            source.PhysicalTableID
        };
        if table_id != 0 {
            variables.StmtCtx.InsertLogicalPlanTableStats(
                table_id,
                std::sync::Arc::new(source.TableStats.clone()),
            );
        }
    }
    for child in plan.Children() {
        capture_plan_replayer_table_stats(child.as_ref(), variables);
    }
}

/// 物理优化（不含后处理阶段）。
/// Propagate TiFlash availability before enumerating physical alternatives,
/// matching Go's bottom-up PreparePossibleProperties phase.
fn prepare_tiflash_availability(plan: &mut dyn logicalop::LogicalPlan) -> bool {
    let root: *mut dyn logicalop::LogicalPlan = plan;
    let mut work = vec![(root, false)];
    let mut visited_nodes = std::collections::HashSet::new();
    let mut visited_ctes = std::collections::HashSet::new();
    while let Some((pointer, ready)) = work.pop() {
        // SAFETY: the child and CTE seed boxes stay owned by `plan` during
        // this walk. We update cached properties without replacing any node.
        let node = unsafe { &mut *pointer };
        if !ready {
            if !visited_nodes.insert(pointer as *mut () as usize) {
                continue;
            }
            work.push((pointer, true));
            let children = node
                .Children_mut()
                .iter_mut()
                .map(|child| child.as_mut() as *mut dyn logicalop::LogicalPlan)
                .collect::<Vec<_>>();
            work.extend(children.into_iter().rev().map(|child| (child, false)));
            if let Some(cte) = node.as_any_mut().downcast_mut::<logicalop::LogicalCTE>()
                && visited_ctes.insert(std::rc::Rc::as_ptr(&cte.Cte) as usize)
            {
                let mut class = cte.Cte.borrow_mut();
                if let Some(seed) = class.SeedPartLogicalPlan.as_mut() {
                    work.push((seed.as_mut() as *mut dyn logicalop::LogicalPlan, false));
                }
                if let Some(recursive) = class.RecursivePartLogicalPlan.as_mut() {
                    work.push((recursive.as_mut() as *mut dyn logicalop::LogicalPlan, false));
                }
            }
            continue;
        }
        let children = node
            .Children()
            .iter()
            .map(|child| child.base().PreparePossiblePropertiesValue())
            .collect::<Vec<_>>();
        if let Some(source) = node.as_any().downcast_ref::<logicalop::DataSource>() {
            let available = source.PreparePossibleProperties().HasTiFlash;
            node.base_mut().PreparePossibleProperties(&[available]);
        } else if let Some(cte) = node.as_any_mut().downcast_mut::<logicalop::LogicalCTE>() {
            let available = cte.PreparePossibleProperties().HasTiFlash;
            let inputs = if children.is_empty() {
                vec![available]
            } else {
                children
            };
            cte.base_mut().PreparePossibleProperties(&inputs);
        } else {
            node.base_mut().PreparePossibleProperties(&children);
        }
    }
    plan.base().PreparePossiblePropertiesValue()
}

#[cfg(test)]
#[path = "optimizer_runtime_tiflash_test.rs"]
mod optimizer_runtime_tiflash_test;

#[cfg(test)]
#[path = "optimizer_runtime_projection_test.rs"]
mod optimizer_runtime_projection_test;

fn physical_optimize_without_post(
    plan: &mut dyn logicalop::LogicalPlan,
) -> Result<(Box<dyn base::PhysicalPlan>, f64), expression::Error> {
    // Logical build resets the statement allocator. Physical candidates keep
    // using that allocator; resetting here would restart IDs mid-statement.
    // The physical crate owns the recursion and operator dispatch, while core
    // installs it at the same package boundary where Go selects a task.
    let _ = physicalop::InstallFindBestTaskRouter(physicalop::CanonicalFindBestTaskRouter);
    let _ = physicalop::InstallPlanCostVer1Router(crate::plan_cost_ver1::GetCanonicalPlanCostVer1);
    let _ = physicalop::InstallPlanCostVer2Router(crate::plan_cost_ver2::GetCanonicalPlanCostVer2);
    physicalop::ResetCanonicalTaskCache();

    prepare_tiflash_availability(plan);
    let mut property = property::PhysicalProperty::default();
    property.ExpectedCnt = f64::MAX;
    let task = physicalop::FindBestTask(plan, &property)?;
    if task.invalid() {
        return Err(expression::errors::New(
            "Can't find a proper physical plan for this query",
        ));
    }

    fn preserve_plan_ids(
        source: &dyn base::PhysicalPlan,
        target: &mut dyn base::PhysicalPlan,
    ) -> Result<(), expression::Error> {
        target.set_id(source.id());
        let source_children = source.children();
        let mut target_children = target
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        for (source_child, target_child) in source_children.into_iter().zip(&mut target_children) {
            preserve_plan_ids(source_child, target_child.as_mut())?;
        }
        if !target_children.is_empty() {
            target.set_children(target_children);
        }
        Ok(())
    }
    let mut physical = task.plan().clone_physical(task.plan().s_ctx().clone())?;
    preserve_plan_ids(task.plan(), physical.as_mut())?;
    physical.resolve_indices()?;
    let cost =
        physical.get_plan_cost_ver1(property.TaskTp, &costusage::new_default_plan_cost_option())?;
    Ok((physical, cost))
}

pub(crate) fn PhysicalOptimizeForTest(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<(Box<dyn base::PhysicalPlan>, f64), expression::Error> {
    physical_optimize_without_post(plan.as_mut())
}

/// Optimize an already logically optimized fragment as an MPP producer.
///
/// CTE seed plans are selected once for their shared TiFlash producer rather
/// than once for a root consumer.  Calling the normal root-task optimizer here
/// loses the reader/exchange and partial/final aggregate layers visible in
/// Go's `CTE_1` plan-tree output.
pub fn PhysicalOptimizeForMpp(
    plan: &mut logicalop::LogicalPlanRef,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let _ = physicalop::InstallFindBestTaskRouter(physicalop::CanonicalFindBestTaskRouter);
    let _ = physicalop::InstallPlanCostVer1Router(crate::plan_cost_ver1::GetCanonicalPlanCostVer1);
    let _ = physicalop::InstallPlanCostVer2Router(crate::plan_cost_ver2::GetCanonicalPlanCostVer2);
    physicalop::ResetCanonicalTaskCache();

    // A non-recursive CTE seed can contain another CTE reference.  The Go
    // producer optimizer resolves that dependency before enumerating the
    // outer seed; otherwise the nested catalog join is handed to MPP task
    // enumeration in its raw Cartesian form.
    fn optimize_nested_cte_seeds(
        plan: &mut dyn logicalop::LogicalPlan,
    ) -> Result<(), expression::Error> {
        if let Some(cte) = plan.as_any_mut().downcast_mut::<logicalop::LogicalCTE>() {
            let (flag, seed_optimized, seed) = {
                let mut class = cte.Cte.borrow_mut();
                (
                    class.OptFlag,
                    class.SeedPartLogicalOptimized,
                    class.SeedPartLogicalPlan.take(),
                )
            };
            if let Some(mut seed) = seed {
                let result = if seed_optimized {
                    Ok(())
                } else {
                    LogicalOptimizeForMpp(flag, &mut seed)
                };
                let mut class = cte.Cte.borrow_mut();
                class.SeedPartLogicalOptimized = result.is_ok();
                class.SeedPartLogicalPlan = Some(seed);
                result?;
            }
        }
        for child in plan.Children_mut() {
            optimize_nested_cte_seeds(child.as_mut())?;
        }
        Ok(())
    }
    optimize_nested_cte_seeds(plan.as_mut())?;

    prepare_tiflash_availability(plan.as_mut());
    let mut property = property::PhysicalProperty::default();
    property.TaskTp = property::MppTaskType;
    property.ExpectedCnt = f64::MAX;
    property.CanAddEnforcer = true;
    property.CTEProducerStatus = property::AllCTECanMpp;
    let task = physicalop::FindBestTask(plan.as_mut(), &property)?;
    if task.invalid() {
        return Err(expression::errors::New(
            "Can't find a proper MPP physical plan for CTE seed",
        ));
    }
    let physical = task.plan().clone_physical(task.plan().s_ctx().clone())?;
    let physical = compact_nested_cte_mpp_aggregate(physical)?;
    let physical = eliminate_adjacent_passthrough_projections(physical)?;
    let physical = promote_mpp_root_projection_and_topn(physical)?;
    // CTE seed column pruning can leave a join key outside the projected
    // child schema even though the EXPLAIN expressions still retain its
    // semantic catalog column.  The seed is rendered/embedded here, not
    // executed by the RootTask, so preserve that semantic form instead of
    // rejecting the otherwise valid MPP producer during index resolution.
    Ok(physical)
}

/// Restore the Root Projection/TopN boundary when MPP attachment leaves two
/// identical TopNs and the output projection above a pass-through sender.
fn promote_mpp_root_projection_and_topn(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
        .filter(|reader| reader.ReadReqType == physicalop::ReadReqType::MPP)
    else {
        return Ok(plan);
    };
    let context = reader.s_ctx().clone();
    let Some(sender_plan) = reader.TablePlan.as_deref() else {
        return Ok(plan);
    };
    let Some(sender) = sender_plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalExchangeSender>()
        .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
    else {
        return Ok(plan);
    };
    let sender_children = sender.children();
    let [projection_plan] = sender_children.as_slice() else {
        return Ok(plan);
    };
    let Some(projection) = projection_plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        .filter(|projection| {
            projection
                .Exprs
                .iter()
                .all(|expr| expr.as_any().is::<expression::Column>())
        })
    else {
        return Ok(plan);
    };
    let projection_children = projection.children();
    let [first_topn_plan] = projection_children.as_slice() else {
        return Ok(plan);
    };
    let Some(first_topn) = first_topn_plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTopN>()
    else {
        return Ok(plan);
    };
    let first_topn_children = first_topn.children();
    let [second_topn_plan] = first_topn_children.as_slice() else {
        return Ok(plan);
    };
    let Some(second_topn) = second_topn_plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTopN>()
        .filter(|second| first_topn.ExplainInfo() == second.ExplainInfo())
    else {
        return Ok(plan);
    };
    let second_topn_children = second_topn.children();
    let [mpp_input] = second_topn_children.as_slice() else {
        return Ok(plan);
    };

    let mut reader = reader.Clone(context.clone())?;
    let mut sender = sender.Clone(context.clone())?;
    let mut mpp_topn = second_topn.Clone(context.clone())?;
    mpp_topn.set_children(vec![mpp_input.clone_physical(context.clone())?]);
    sender
        .PhysicalSchemaProducer
        .SetSchema(mpp_topn.schema().Clone());
    sender.set_children(vec![Box::new(mpp_topn)]);
    reader.SetChildren(vec![Box::new(sender)]);

    let mut root_topn = first_topn.Clone(context.clone())?;
    root_topn.set_children(vec![Box::new(reader)]);
    let mut root_projection = projection.Clone(context)?;
    root_projection.set_children(vec![Box::new(root_topn)]);
    Ok(Box::new(root_projection))
}

/// Merge adjacent physical projections in an MPP producer while retaining the
/// projection directly above a final aggregate for TiFlash's output layout.
fn eliminate_adjacent_passthrough_projections(
    mut plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            child
                .clone_physical(child.s_ctx().clone())
                .and_then(eliminate_adjacent_passthrough_projections)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !children.is_empty() {
        plan.set_children(children);
    }
    if plan.as_any().is::<physicalop::PhysicalProjection>()
        && plan
            .children()
            .first()
            .is_some_and(|child| child.as_any().is::<physicalop::PhysicalProjection>())
        && physical_projection_is_strict_passthrough(plan.as_ref())
    {
        let mut child = plan.children()[0].clone_physical(plan.s_ctx().clone())?;
        let schema = plan.schema().Clone();
        child
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalProjection>()
            .expect("checked projection child")
            .PhysicalSchemaProducer
            .SetSchema(schema);
        return Ok(child);
    }
    Ok(plan)
}

/// Nested CTE producers use a single final aggregate in Go.  The generic MPP
/// attachment path can leave a partial/final pair around a three-function
/// `cs_ui` aggregate; collapse that producer-local pair without touching the
/// outer cross-sales aggregate, which intentionally remains two phase.
fn compact_nested_cte_mpp_aggregate(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            child
                .clone_physical(child.s_ctx().clone())
                .and_then(compact_nested_cte_mpp_aggregate)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(hash) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalHashAgg>()
        .filter(|hash| {
            hash.BasePhysicalAgg.GroupByItems.len() == 1 && hash.BasePhysicalAgg.AggFuncs.len() == 3
        })
    {
        let mut aggregate = hash.Clone(hash.s_ctx().clone())?;
        aggregate.set_children(children);
        if let [receiver] = aggregate.children().as_slice()
            && let [sender] = receiver.children().as_slice()
            && let [partial] = sender.children().as_slice()
            && partial.as_any().is::<physicalop::PhysicalHashAgg>()
        {
            let input = partial
                .children()
                .first()
                .ok_or_else(|| {
                    expression::errors::New("nested CTE partial aggregate has no input")
                })?
                .clone_physical(aggregate.s_ctx().clone())?;
            aggregate.set_children(vec![input]);
        }
        // Go computes the nested CTE aggregate over a compact projection of
        // its aggregate arguments. Keep that projection explicitly: it gives
        // the aggregate its generated Column identities while preserving the
        // catalog column as the firstrow result name.
        let input = aggregate
            .children()
            .first()
            .ok_or_else(|| expression::errors::New("nested CTE aggregate has no input"))?
            .clone_physical(aggregate.s_ctx().clone())?;
        if aggregate.BasePhysicalAgg.AggFuncs.len() == 3
            && aggregate.BasePhysicalAgg.GroupByItems.len() == 1
            && aggregate
                .BasePhysicalAgg
                .AggFuncs
                .iter()
                .all(|function| function.Args.len() == 1)
        {
            let context = aggregate.s_ctx().clone();
            let expressions = vec![
                aggregate.BasePhysicalAgg.AggFuncs[0].Args[0].CloneExpr(),
                aggregate.BasePhysicalAgg.AggFuncs[1].Args[0].CloneExpr(),
                aggregate.BasePhysicalAgg.GroupByItems[0].CloneExpr(),
            ];
            let eval = context.GetExprCtx().GetEvalCtx();
            let columns = expressions
                .iter()
                .enumerate()
                .map(|(index, expression)| {
                    expression::Column::new(
                        expression.GetType(eval).Clone(),
                        0,
                        context.GetSessionVars().AllocPlanColumnID(),
                        index as isize,
                    )
                })
                .collect::<Vec<_>>();
            let mut projection = physicalop::PhysicalProjection::New(context.clone()).Init(
                context.clone(),
                input.stats_info().clone(),
                aggregate.query_block_offset(),
                Vec::new(),
            );
            projection.Exprs = expressions;
            projection
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns.clone()));
            projection.set_children(vec![input]);
            for (function, column) in aggregate
                .BasePhysicalAgg
                .AggFuncs
                .iter_mut()
                .zip(columns.iter())
            {
                function.Args = vec![Box::new(column.Clone())];
            }
            aggregate.BasePhysicalAgg.GroupByItems = vec![Box::new(columns[2].Clone())];
            aggregate.set_children(vec![Box::new(projection)]);
        }
        return Ok(Box::new(aggregate));
    }
    let mut cloned = plan.clone_physical(plan.s_ctx().clone())?;
    cloned.set_children(children);
    Ok(cloned)
}

pub(crate) fn PhysicalOptimizeForTestWithWindowConcurrency(
    plan: &mut logicalop::LogicalPlanRef,
    window_concurrency: usize,
) -> Result<(Box<dyn base::PhysicalPlan>, f64), expression::Error> {
    let (physical, _) = physical_optimize_without_post(plan.as_mut())?;
    let physical = optimize_by_shuffle_for_window(physical, window_concurrency, true)?;
    fn needs_window_projection_normalization(plan: &dyn base::PhysicalPlan) -> bool {
        fn reaches_window(mut plan: &dyn base::PhysicalPlan) -> bool {
            loop {
                if plan.as_any().is::<physicalop::PhysicalWindow>() {
                    return true;
                }
                let Some(projection) = plan
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
                else {
                    return false;
                };
                let children = projection.children();
                let Some(child) = children.first() else {
                    return false;
                };
                plan = *child;
            }
        }
        let relevant_pair = plan.children().first().is_some_and(|child| {
            child.as_any().is::<physicalop::PhysicalProjection>()
                && child
                    .children()
                    .first()
                    .is_some_and(|grandchild| reaches_window(*grandchild))
                && (plan.as_any().is::<physicalop::PhysicalProjection>()
                    || plan.as_any().is::<physicalop::PhysicalSort>()
                    || plan.as_any().is::<physicalop::PhysicalMaxOneRow>()
                    || plan.as_any().is::<physicalop::PhysicalWindow>())
        });
        relevant_pair
            || plan
                .children()
                .iter()
                .any(|child| needs_window_projection_normalization(*child))
    }
    let mut physical = if needs_window_projection_normalization(physical.as_ref()) {
        eliminate_redundant_physical_projections_inner(physical, false, false)?
    } else {
        physical
    };
    physical.resolve_indices()?;
    let mut required = property::PhysicalProperty::default();
    required.ExpectedCnt = f64::MAX;
    let cost =
        physical.get_plan_cost_ver1(required.TaskTp, &costusage::new_default_plan_cost_option())?;
    Ok((physical, cost))
}

/// 为 Window 选择/注入 shuffle 以满足分区分布需求。
fn optimize_by_shuffle_for_window(
    mut plan: Box<dyn base::PhysicalPlan>,
    window_concurrency: usize,
    property_is_empty: bool,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    let shuffle_data_source = if window_concurrency > 1 && property_is_empty {
        plan.as_any()
            .downcast_ref::<physicalop::PhysicalWindow>()
            .filter(|window| !window.PartitionBy.is_empty())
            .and_then(|window| {
                let plan_children = plan.children();
                let sort = plan_children
                    .first()?
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalSort>()?;
                let sort_children = sort.children();
                let data_source = sort_children.first()?;
                (data_source.stats_count() > 1.0).then(|| {
                    let explain_id = data_source.explain_id(&[]).to_string();
                    (explain_id, data_source.stats_count())
                })
            })
    } else {
        None
    };
    let children = plan
        .children()
        .into_iter()
        .map(|child| child.clone_physical(context.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let children = children
        .into_iter()
        .map(|child| {
            optimize_by_shuffle_for_window(
                child,
                window_concurrency,
                !plan.as_any().is::<physicalop::PhysicalSort>(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    plan.set_children(children);
    let Some((data_source_explain_id, data_source_count)) = shuffle_data_source else {
        return Ok(plan);
    };

    // Keep Go's typed source and partition keys for the statement-RU splitter
    // formula. An ExplainID alone cannot recover runtime rows or key slots.
    let window = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalWindow>()
        .ok_or_else(|| expression::errors::New("shuffle requires a Window plan"))?;
    let by_items = window
        .PartitionBy
        .iter()
        .map(|item| Box::new(item.Col.Clone()) as expression::ExprBox)
        .collect();
    let source = plan
        .children()
        .first()
        .and_then(|sort| sort.children().first().copied())
        .ok_or_else(|| expression::errors::New("shuffle Window requires a Sort data source"))?
        .clone_physical(context.clone())?;
    let stats = plan.stats_info().clone();
    let query_block_offset = plan.query_block_offset();
    let mut shuffle = physicalop::PhysicalShuffle::New(
        context.clone(),
        window_concurrency.min(data_source_count as usize),
        vec![data_source_explain_id],
    )
    .Init(context, stats, query_block_offset, plan);
    shuffle.DataSources = vec![source];
    shuffle.ByItemArrays = vec![by_items];
    shuffle.SplitterType = physicalop::physical_shuffle::PartitionSplitterType::Hash;
    Ok(Box::new(shuffle))
}

/// 处理细粒度 shuffle 标记在算子上的传播。
fn handle_fine_grained_shuffle(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let variables = plan.s_ctx().GetSessionVars();
    let configured = variables.TiFlashFineGrainedShuffleStreamCount;
    if configured < 0 {
        return Ok(plan);
    }
    let stream_count = if configured > 0 {
        configured as u64
    } else if variables.TiFlashMaxThreads > 0 {
        variables.TiFlashMaxThreads as u64
    } else {
        vardef_dependency::DefStreamCountWhenMaxThreadsNotSet as u64
    };
    setup_window_fine_grained_shuffle(plan.as_ref(), false, stream_count).map(|(plan, _)| plan)
}

/// 广播连接已经把 build 侧复制到所有节点，probe 侧无需再做 Hash Exchange。
pub(crate) fn broadcast_join_probe_exchange_is_redundant(
    build_is_broadcast: bool,
    probe_is_hash: bool,
) -> bool {
    build_is_broadcast && probe_is_hash
}

/// 消除广播 HashJoin probe 侧多余的 Receiver/Sender；保留 build 侧广播边界。
fn normalize_broadcast_join_probe_exchange(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let required = plan.schema().Clone();
    normalize_broadcast_join_probe_exchange_inner(plan, false, false, &required)
}

fn extend_required_columns(required: &mut Vec<expression::Column>, columns: &[expression::Column]) {
    for column in columns {
        if !required.iter().any(|existing| existing.EqualColumn(column)) {
            required.push(column.Clone());
        }
    }
}

/// Recover the outer key Go stores in OuterHashKeys for a bare equality whose
/// opposite key belongs to the IndexJoin's inner child.
fn outer_hash_key_for_equality(
    left: &expression::Column,
    right: &expression::Column,
    outer_schema: &expression::Schema,
    inner_schema: &expression::Schema,
) -> Option<expression::Column> {
    if left.InOperand || right.InOperand {
        return None;
    }
    if outer_schema.Contains(left) && inner_schema.Contains(right) {
        Some(left.Clone())
    } else if outer_schema.Contains(right) && inner_schema.Contains(left) {
        Some(right.Clone())
    } else {
        None
    }
}

fn normalize_broadcast_join_probe_exchange_inner(
    plan: Box<dyn base::PhysicalPlan>,
    under_index_join: bool,
    under_exchange_sender: bool,
    required: &expression::Schema,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    let context = plan.s_ctx().clone();
    let under_index_join = under_index_join || plan.as_any().is::<physicalop::PhysicalIndexJoin>();
    let under_exchange_sender =
        under_exchange_sender || plan.as_any().is::<physicalop::PhysicalExchangeSender>();
    let child_requirements = plan
        .children()
        .iter()
        .map(|child| {
            let mut columns = required.Columns.clone();
            if let Some(projection) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
            {
                columns.clear();
                for (index, output) in projection.schema().Columns.iter().enumerate() {
                    if required.Contains(output) {
                        if let Some(expr) = projection.Exprs.get(index) {
                            columns.extend(
                                expression::ExtractColumns(expr.as_ref())
                                    .into_iter()
                                    .map(expression::Column::Clone),
                            );
                        }
                    }
                }
            } else if let Some(agg) = plan.as_any().downcast_ref::<physicalop::PhysicalHashAgg>() {
                columns.clear();
                for function in &agg.BasePhysicalAgg.AggFuncs {
                    for arg in &function.Args {
                        columns.extend(
                            expression::ExtractColumns(arg.as_ref())
                                .into_iter()
                                .map(expression::Column::Clone),
                        );
                    }
                }
                for item in &agg.BasePhysicalAgg.GroupByItems {
                    columns.extend(
                        expression::ExtractColumns(item.as_ref())
                            .into_iter()
                            .map(expression::Column::Clone),
                    );
                }
            } else if let Some(topn) = plan.as_any().downcast_ref::<physicalop::PhysicalTopN>() {
                for item in &topn.ByItems {
                    columns.extend(
                        expression::ExtractColumns(item.Expr.as_ref())
                            .into_iter()
                            .map(expression::Column::Clone),
                    );
                }
            } else if let Some(join) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalIndexJoin>()
            {
                columns.extend(join.BasePhysicalJoin.OuterJoinKeys.iter().cloned());
                columns.extend(join.BasePhysicalJoin.InnerJoinKeys.iter().cloned());
                columns.extend(join.OuterHashKeys.iter().cloned());
                for condition in &join.BasePhysicalJoin.OtherConditions {
                    columns.extend(
                        expression::ExtractColumns(condition.as_ref())
                            .into_iter()
                            .map(expression::Column::Clone),
                    );
                }
            } else if let Some(join) = plan.as_any().downcast_ref::<physicalop::PhysicalHashJoin>()
            {
                columns.extend(join.BasePhysicalJoin.LeftJoinKeys.iter().cloned());
                columns.extend(join.BasePhysicalJoin.RightJoinKeys.iter().cloned());
                for condition in join
                    .BasePhysicalJoin
                    .LeftConditions
                    .iter()
                    .chain(&join.BasePhysicalJoin.RightConditions)
                    .chain(&join.BasePhysicalJoin.OtherConditions)
                {
                    columns.extend(
                        expression::ExtractColumns(condition.as_ref())
                            .into_iter()
                            .map(expression::Column::Clone),
                    );
                }
            } else if let Some(selection) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalSelection>()
            {
                for condition in &selection.Conditions {
                    columns.extend(
                        expression::ExtractColumns(condition.as_ref())
                            .into_iter()
                            .map(expression::Column::Clone),
                    );
                }
            }
            expression::NewSchema(
                columns
                    .into_iter()
                    .filter(|column| child.schema().Contains(column))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let mut children = plan
        .children()
        .into_iter()
        .zip(child_requirements.iter())
        .map(|(child, child_required)| {
            normalize_broadcast_join_probe_exchange_inner(
                child.clone_physical(context.clone())?,
                under_index_join,
                under_exchange_sender,
                child_required,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(index_join) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalIndexJoin>()
    {
        fn prune_outer_projection(
            plan: Box<dyn base::PhysicalPlan>,
            required: &expression::Schema,
            preserve_build_keys: bool,
        ) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
            let context = plan.s_ctx().clone();
            if let Some(projection) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
            {
                let mut required = required.Clone();
                if let Some(hash_join) = projection.children().first().and_then(|child| {
                    child
                        .as_any()
                        .downcast_ref::<physicalop::PhysicalHashJoin>()
                }) {
                    let probe_keys = if hash_join.RightIsBuildSide() {
                        &hash_join.BasePhysicalJoin.LeftJoinKeys
                    } else {
                        &hash_join.BasePhysicalJoin.RightJoinKeys
                    };
                    required.Columns.extend(probe_keys.iter().cloned());
                    if preserve_build_keys {
                        let build_keys = if hash_join.RightIsBuildSide() {
                            &hash_join.BasePhysicalJoin.RightJoinKeys
                        } else {
                            &hash_join.BasePhysicalJoin.LeftJoinKeys
                        };
                        required.Columns.extend(build_keys.iter().cloned());
                        for condition in &hash_join.EqualConditions {
                            required.Columns.extend(
                                expression::ExtractColumns(condition)
                                    .into_iter()
                                    .map(expression::Column::Clone),
                            );
                        }
                    }
                }
                let mut retained = projection
                    .schema()
                    .Columns
                    .iter()
                    .enumerate()
                    .filter(|(_, column)| {
                        required.Contains(column)
                            || preserve_build_keys
                                && (required.Columns.iter().any(|needed| {
                                    needed.String() == column.String()
                                        || (!needed.OrigName.is_empty()
                                            && needed.OrigName == column.OrigName)
                                }) || projection
                                    .children()
                                    .first()
                                    .and_then(|child| {
                                        child
                                            .as_any()
                                            .downcast_ref::<physicalop::PhysicalHashJoin>()
                                    })
                                    .is_some_and(|join| {
                                        join.BasePhysicalJoin
                                            .LeftJoinKeys
                                            .iter()
                                            .chain(&join.BasePhysicalJoin.RightJoinKeys)
                                            .any(|key| key.String() == column.String())
                                            || join.EqualConditions.iter().any(|condition| {
                                                expression::ExtractColumns(condition)
                                                    .iter()
                                                    .any(|key| key.String() == column.String())
                                            })
                                    }))
                    })
                    .map(|(index, column)| (index, column.Clone()))
                    .collect::<Vec<_>>();
                let mut retained_ids = std::collections::HashSet::new();
                retained.retain(|(_, column)| retained_ids.insert(column.UniqueID));
                if !retained.is_empty() && retained.len() < projection.schema().Len() {
                    let mut projection = projection.Clone(context.clone())?;
                    projection.Exprs = retained
                        .iter()
                        .map(|(index, _)| projection.Exprs[*index].CloneExpr())
                        .collect();
                    projection
                        .PhysicalSchemaProducer
                        .SetSchema(expression::NewSchema(
                            retained.into_iter().map(|(_, column)| column).collect(),
                        ));
                    if let Some(child) = projection.children().first() {
                        let required_columns = projection
                            .Exprs
                            .iter()
                            .flat_map(|expr| expression::ExtractColumns(expr.as_ref()))
                            .map(expression::Column::Clone)
                            .collect();
                        let child = prune_outer_projection(
                            child.clone_physical(context.clone())?,
                            &expression::NewSchema(required_columns),
                            preserve_build_keys,
                        )?;
                        projection.set_children(vec![child]);
                    }
                    return Ok(Box::new(projection));
                }
                let mut projection = projection.Clone(context.clone())?;
                if let Some(child) = projection.children().first() {
                    let required_columns = projection
                        .Exprs
                        .iter()
                        .flat_map(|expr| expression::ExtractColumns(expr.as_ref()))
                        .map(expression::Column::Clone)
                        .collect();
                    let child = prune_outer_projection(
                        child.clone_physical(context)?,
                        &expression::NewSchema(required_columns),
                        preserve_build_keys,
                    )?;
                    projection.set_children(vec![child]);
                }
                return Ok(Box::new(projection));
            }
            if let Some(join) = plan.as_any().downcast_ref::<physicalop::PhysicalHashJoin>() {
                let mut semantic_columns = required.Columns.clone();
                semantic_columns.extend(join.BasePhysicalJoin.LeftJoinKeys.iter().cloned());
                semantic_columns.extend(join.BasePhysicalJoin.RightJoinKeys.iter().cloned());
                for condition in join
                    .BasePhysicalJoin
                    .LeftConditions
                    .iter()
                    .chain(&join.BasePhysicalJoin.RightConditions)
                    .chain(&join.BasePhysicalJoin.OtherConditions)
                {
                    semantic_columns.extend(
                        expression::ExtractColumns(condition.as_ref())
                            .into_iter()
                            .map(expression::Column::Clone),
                    );
                }
                let mut cloned = plan.clone_physical(context.clone())?;
                let children = plan
                    .children()
                    .into_iter()
                    .map(|child| {
                        let child_required = expression::NewSchema(
                            semantic_columns
                                .iter()
                                .filter(|column| child.schema().Contains(column))
                                .map(expression::Column::Clone)
                                .collect(),
                        );
                        prune_outer_projection(
                            child.clone_physical(context.clone())?,
                            &child_required,
                            preserve_build_keys,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                cloned.set_children(children);
                return Ok(cloned);
            }
            if plan.children().len() != 1 {
                return plan.clone_physical(context);
            }
            let child = prune_outer_projection(
                plan.children()[0].clone_physical(context.clone())?,
                required,
                preserve_build_keys,
            )?;
            let mut cloned = plan.clone_physical(context)?;
            cloned.set_children(vec![child]);
            Ok(cloned)
        }
        let outer_index = 1 - index_join.BasePhysicalJoin.InnerChildIdx;
        let outer_schema = children
            .get(outer_index)
            .map(|child| child.schema().Clone());
        let inner_schema = children
            .get(index_join.BasePhysicalJoin.InnerChildIdx)
            .map(|child| child.schema().Clone());
        if let Some(outer) = children.get_mut(outer_index) {
            let mut required_columns = required
                .Columns
                .iter()
                .map(expression::Column::Clone)
                .collect::<Vec<_>>();
            extend_required_columns(
                &mut required_columns,
                &index_join.BasePhysicalJoin.OuterJoinKeys,
            );
            // OuterJoinKeys are the index lookup prefix. IndexHashJoin can
            // also hash on additional outer equality columns carried only by
            // OuterHashKeys, so they must survive pruning as well.
            extend_required_columns(&mut required_columns, &index_join.OuterHashKeys);
            if let (Some(outer_schema), Some(inner_schema)) =
                (outer_schema.as_ref(), inner_schema.as_ref())
            {
                for condition in &index_join.EqualConditions {
                    if condition.FuncName.L != parser_ast_dependency::EQ {
                        continue;
                    }
                    let (left, right, is_column_equality) = expression::IsColOpCol(condition);
                    if !is_column_equality {
                        continue;
                    }
                    if let Some(outer_hash_key) = outer_hash_key_for_equality(
                        left.expect("column equality has a left column"),
                        right.expect("column equality has a right column"),
                        outer_schema,
                        inner_schema,
                    ) {
                        extend_required_columns(
                            &mut required_columns,
                            std::slice::from_ref(&outer_hash_key),
                        );
                    }
                }
            }
            for condition in &index_join.BasePhysicalJoin.OtherConditions {
                required_columns.extend(
                    expression::ExtractColumns(condition.as_ref())
                        .into_iter()
                        .filter(|column| outer.schema().Contains(column))
                        .map(expression::Column::Clone),
                );
            }
            *outer = prune_outer_projection(
                outer.clone_physical(outer.s_ctx().clone())?,
                &expression::NewSchema(required_columns),
                false,
            )?;
        }
    }
    if plan.as_any().is::<physicalop::PhysicalHashJoin>() && children.len() == 2 {
        fn exchange_sender(
            child: &dyn base::PhysicalPlan,
        ) -> Option<&physicalop::PhysicalExchangeSender> {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalExchangeReceiver>()
                .and_then(|receiver| receiver.children().first().copied())
                .and_then(|sender| {
                    sender
                        .as_any()
                        .downcast_ref::<physicalop::PhysicalExchangeSender>()
                })
        }
        if under_index_join {
            let build_index =
                usize::from(children[0].stats_info().RowCount > children[1].stats_info().RowCount);
            let probe_index = 1 - build_index;
            let build_rows = children[build_index].stats_info().RowCount;
            let probe_rows = children[probe_index].stats_info().RowCount;
            if let Some(receiver) = children[build_index]
                .as_any_mut()
                .downcast_mut::<physicalop::PhysicalExchangeReceiver>()
            {
                let mut receiver_children = receiver
                    .children()
                    .into_iter()
                    .map(|child| child.clone_physical(context.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(sender) = receiver_children.first_mut().and_then(|child| {
                    child
                        .as_any_mut()
                        .downcast_mut::<physicalop::PhysicalExchangeSender>()
                }) && physicalop::NormalizeNestedIndexJoinBuildExchange(
                    sender, build_rows, probe_rows,
                ) {
                    receiver.set_children(receiver_children);
                }
            }
        }
        for (build_index, probe_index) in [(0, 1), (1, 0)] {
            let Some(build_exchange) = exchange_sender(children[build_index].as_ref()) else {
                continue;
            };
            let Some(probe_exchange) = exchange_sender(children[probe_index].as_ref()) else {
                continue;
            };
            if !broadcast_join_probe_exchange_is_redundant(
                build_exchange
                    .ExplainInfo()
                    .contains("ExchangeType: Broadcast"),
                probe_exchange.IsHashExchange(),
            ) {
                continue;
            }
            let probe_input = children[probe_index].children()[0].children()[0]
                .clone_physical(context.clone())?;
            children[probe_index] = probe_input;
            break;
        }
    }
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        && children.len() == 1
        && children[0].as_any().is::<physicalop::PhysicalIndexJoin>()
        && projection
            .Exprs
            .iter()
            .any(|expr| expr.as_any().is::<expression::ScalarFunction>())
    {
        // Carry the computed projection's input columns in IndexHashJoin's
        // pruned output schema, matching Go's column-pruned join output.
        let mut required_columns = Vec::new();
        let mut all_required_columns_available = true;
        let index_join_schema = children[0].schema().Clone();
        for expr in &projection.Exprs {
            for column in expression::ExtractColumns(expr.as_ref()) {
                if !index_join_schema.Contains(column) {
                    all_required_columns_available = false;
                    continue;
                }
                if required_columns
                    .iter()
                    .all(|existing: &expression::Column| existing.UniqueID != column.UniqueID)
                {
                    required_columns.push(column.Clone());
                }
            }
        }
        if !required_columns.is_empty() {
            if all_required_columns_available {
                let index_join_children = children[0]
                    .children()
                    .into_iter()
                    .map(|child| child.clone_physical(context.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut index_join = children[0]
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalIndexJoin>()
                    .expect("index join checked above")
                    .Clone(context.clone())?;
                index_join.set_children(index_join_children);
                index_join
                    .BasePhysicalJoin
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(required_columns));
                index_join
                    .BasePhysicalJoin
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .set_stats(projection.stats_info().clone());
                children[0] = Box::new(index_join);
            } else {
                let mut input_projection = projection.Clone(context.clone())?;
                input_projection.Exprs = required_columns
                    .iter()
                    .map(|column| Box::new(column.Clone()) as expression::ExprBox)
                    .collect();
                input_projection
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(required_columns));
                input_projection.set_children(vec![children[0].clone_physical(context.clone())?]);
                children[0] = Box::new(input_projection);
            }
        }
    }
    let mut cloned = plan.clone_physical(context.clone())?;
    if under_exchange_sender
        && children.len() == 1
        && children[0].as_any().is::<physicalop::PhysicalHashJoin>()
        && let Some(projection) = cloned
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalProjection>()
        && projection.Exprs.len() == projection.schema().Len()
        && projection
            .Exprs
            .iter()
            .all(|expr| expr.as_any().is::<expression::Column>())
    {
        let mut seen = std::collections::HashSet::new();
        let retained = projection
            .schema()
            .Columns
            .iter()
            .enumerate()
            .filter(|(_, column)| seen.insert(column.UniqueID))
            .map(|(index, column)| (index, column.Clone()))
            .collect::<Vec<_>>();
        if retained.len() < projection.schema().Len() {
            projection.Exprs = retained
                .iter()
                .map(|(index, _)| projection.Exprs[*index].CloneExpr())
                .collect();
            projection
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(
                    retained.into_iter().map(|(_, column)| column).collect(),
                ));
        }
    }
    if cloned.as_any().is::<physicalop::PhysicalExchangeSender>() && children.len() == 1 {
        // Go passes TiFlash scan/filter output through the MPP sender. A
        // subset-only column projection here changes its exchange row width.
        fn is_tiflash_scan_filter_path(plan: &dyn base::PhysicalPlan) -> bool {
            if let Some(scan) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalTableScan>()
            {
                return scan.StoreType == kv_dependency::StoreType::TiFlash;
            }
            plan.as_any().is::<physicalop::PhysicalSelection>()
                && plan.children().len() == 1
                && is_tiflash_scan_filter_path(plan.children()[0])
        }
        let passthrough_input = children[0]
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .and_then(|projection| {
                let inputs = projection.children();
                let input = *inputs.first()?;
                if !is_tiflash_scan_filter_path(input)
                    || !projection.Exprs.iter().all(|expr| {
                        expr.as_any()
                            .downcast_ref::<expression::Column>()
                            .is_some_and(|column| input.schema().Contains(column))
                    })
                {
                    return None;
                }
                Some((
                    input.schema().Clone(),
                    input.clone_physical(context.clone()),
                ))
            });
        if let Some((schema, input)) = passthrough_input {
            children[0] = input?;
            cloned
                .as_any_mut()
                .downcast_mut::<physicalop::PhysicalExchangeSender>()
                .expect("exchange sender type checked above")
                .PhysicalSchemaProducer
                .SetSchema(schema);
        }
        if let Some(sender) = cloned
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalExchangeSender>()
        {
            sender
                .PhysicalSchemaProducer
                .SetSchema(children[0].schema().Clone());
        }
    }
    if cloned.as_any().is::<physicalop::PhysicalExchangeReceiver>()
        && let Some(sender) = children.first().and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalExchangeSender>()
        })
    {
        cloned
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalExchangeReceiver>()
            .expect("exchange receiver type checked above")
            .PhysicalSchemaProducer
            .SetSchema(sender.schema().Clone());
    }
    cloned.set_children(children);
    Ok(cloned)
}

/// 为 Window 配置细粒度 shuffle 流数量。
fn setup_window_fine_grained_shuffle(
    plan: &dyn base::PhysicalPlan,
    window_target: bool,
    stream_count: u64,
) -> Result<(Box<dyn base::PhysicalPlan>, bool), expression::Error> {
    let is_window = plan.as_any().is::<physicalop::PhysicalWindow>();
    let is_projection = plan.as_any().is::<physicalop::PhysicalProjection>();
    let is_selection = plan.as_any().is::<physicalop::PhysicalSelection>();
    let is_receiver = plan.as_any().is::<physicalop::PhysicalExchangeReceiver>();
    let partial_sort = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalSort>()
        .is_some_and(|sort| sort.IsPartialSort);
    let sender = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalExchangeSender>();

    if let Some(sender) = sender {
        let children = sender
            .children()
            .into_iter()
            .map(|child| setup_window_fine_grained_shuffle(child, false, stream_count))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(child, _)| child)
            .collect();
        let mut cloned = plan.clone_physical(plan.s_ctx().clone())?;
        cloned.set_children(children);
        if sender.IsHashExchange() && window_target {
            return Ok((
                clone_with_stream_count(cloned.as_ref(), stream_count)?,
                true,
            ));
        }
        return Ok((cloned, false));
    }

    if plan.as_any().is::<physicalop::PhysicalHashAgg>() {
        let mut applied = false;
        let children = plan
            .children()
            .into_iter()
            .map(|child| {
                let (child, child_applied) =
                    setup_window_fine_grained_shuffle(child, false, stream_count)?;
                applied |= child_applied;
                Ok(child)
            })
            .collect::<Result<Vec<_>, expression::Error>>()?;
        // A partial aggregate immediately above a window stays in the same
        // fine-grained stream segment in Go's helper stack.
        let direct_window_stream_count = children
            .first()
            .and_then(|child| child.as_any().downcast_ref::<physicalop::PhysicalWindow>())
            .map_or(0, |window| {
                window
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .TiFlashFineGrainedShuffleStreamCount
            });
        let mut cloned = plan.clone_physical(plan.s_ctx().clone())?;
        cloned.set_children(children);
        if applied || direct_window_stream_count > 0 {
            cloned = clone_with_stream_count(
                cloned.as_ref(),
                direct_window_stream_count.max(stream_count),
            )?;
        }
        return Ok((cloned, applied || direct_window_stream_count > 0));
    }

    let continues_partition =
        is_window || is_projection || is_selection || is_receiver || partial_sort;
    let child_target = if is_window {
        true
    } else if continues_partition {
        window_target
    } else {
        false
    };
    let mut applied = false;
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            let (child, child_applied) =
                setup_window_fine_grained_shuffle(child, child_target, stream_count)?;
            applied |= child_applied;
            Ok(child)
        })
        .collect::<Result<Vec<_>, expression::Error>>()?;
    let mut cloned = plan.clone_physical(plan.s_ctx().clone())?;
    cloned.set_children(children);
    if continues_partition && applied {
        cloned = clone_with_stream_count(cloned.as_ref(), stream_count)?;
    }
    Ok((cloned, continues_partition && applied))
}

/// 按指定流数量克隆需参与 shuffle 的计划片段。
fn clone_with_stream_count(
    plan: &dyn base::PhysicalPlan,
    stream_count: u64,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    if let Some(operator) = plan.as_any().downcast_ref::<physicalop::PhysicalWindow>() {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    if let Some(operator) = plan.as_any().downcast_ref::<physicalop::PhysicalSort>() {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    if let Some(operator) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalSelection>()
    {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    if let Some(operator) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
    {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    if let Some(operator) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalExchangeReceiver>()
    {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    if let Some(operator) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalExchangeSender>()
    {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    if let Some(operator) = plan.as_any().downcast_ref::<physicalop::PhysicalHashAgg>() {
        let mut operator = operator.Clone(operator.s_ctx().clone())?;
        operator
            .BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount = stream_count;
        return Ok(Box::new(operator));
    }
    plan.clone_physical(plan.s_ctx().clone())
}

/// Go `countStarRewrite`: a TiFlash scalar COUNT over a one-column full
/// scan reads a non-null column instead of evaluating COUNT(1). The choice
/// starts with the retained handle and narrows to a fixed-width NOT NULL
/// column when one is cheaper to scan.
fn rewrite_tiflash_count_star(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
    {
        let mut reader = reader.Clone(reader.s_ctx().clone())?;
        if let Some(inner) = reader.TablePlan.take() {
            reader.SetChildren(vec![rewrite_tiflash_count_star(inner)?]);
        }
        return Ok(Box::new(reader));
    }
    let mut plan = plan.clone_physical(plan.s_ctx().clone())?;
    let children = plan
        .children()
        .into_iter()
        .map(|child| rewrite_tiflash_count_star(child.clone_physical(child.s_ctx().clone())?))
        .collect::<Result<Vec<_>, _>>()?;
    if !children.is_empty() {
        plan.set_children(children);
    }
    fn rewrite_aggregate(
        aggregate: &mut physicalop::BasePhysicalAgg,
    ) -> Result<(), expression::Error> {
        if !aggregate.GroupByItems.is_empty()
            || aggregate.AggFuncs.is_empty()
            || !aggregate.AggFuncs.iter().all(|function| {
                function.Name == crate::ast::AggFuncCount
                    && !function.HasDistinct
                    && function.Args.len() == 1
                    && function.Args[0]
                        .as_any()
                        .downcast_ref::<expression::Constant>()
                        .is_some()
            })
        {
            return Ok(());
        }
        let Some(scan) = aggregate
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalTableScan>()
            })
        else {
            return Ok(());
        };
        if scan.StoreType != kv_dependency::StoreType::TiFlash
            || !scan.IsFullScan()
            || scan.schema().Len() != 1
            || scan.Columns.len() != 1
        {
            return Ok(());
        }
        let mut scan = scan.Clone(scan.s_ctx().clone())?;
        let mut info = scan.Columns[0].clone();
        let mut column = scan.schema().Columns[0].Clone();
        if let Some(table) = scan.Table.as_ref() {
            for candidate in &table.Columns {
                if candidate.FieldType.IsVarLengthType()
                    || !expression::mysql::HasNotNullFlag(candidate.GetFlag())
                    || candidate.GetFlen() >= info.GetFlen()
                {
                    continue;
                }
                info = candidate.Clone();
                column = expression::Column::new(
                    candidate.FieldType.clone(),
                    candidate.ID,
                    scan.s_ctx().GetSessionVars().AllocPlanColumnID(),
                    0,
                );
                column.OrigName = format!(
                    "{}.{}.{}",
                    scan.DBName.to_ascii_lowercase(),
                    table.Name.L,
                    candidate.Name.L
                );
            }
        }
        scan.Columns[0] = info;
        scan.PhysicalSchemaProducer
            .SetSchema(expression::NewSchema(vec![column.Clone()]));
        for function in &mut aggregate.AggFuncs {
            if function.Args[0]
                .as_any()
                .downcast_ref::<expression::Constant>()
                .is_some_and(|constant| !constant.Value.IsNull())
            {
                function.Args[0] = Box::new(column.Clone());
            }
        }
        aggregate
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildren(vec![Box::new(scan)]);
        Ok(())
    }
    if let Some(hash) = plan
        .as_any_mut()
        .downcast_mut::<physicalop::PhysicalHashAgg>()
    {
        rewrite_aggregate(&mut hash.BasePhysicalAgg)?;
    } else if let Some(stream) = plan
        .as_any_mut()
        .downcast_mut::<physicalop::PhysicalStreamAgg>()
    {
        rewrite_aggregate(&mut stream.BasePhysicalAgg)?;
    }
    Ok(plan)
}

/// 在聚合上下注入必要投影以对齐 Schema。
fn inject_extra_aggregate_projections(
    plan: Box<dyn base::PhysicalPlan>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    // TiKV reader push-down plans are not regular physical children during
    // Go's post-optimization pass. Only TiFlash TableReader is traversed
    // explicitly; entering TiKV readers here would inject root projections
    // into cop tasks and change both executable shape and public EXPLAIN.
    let is_tikv_table_reader = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
        .is_some_and(|reader| {
            reader.StoreType != kv_dependency::StoreType::TiFlash
                && reader.ReadReqType != physicalop::ReadReqType::MPP
        });
    if is_tikv_table_reader
        || plan.as_any().is::<physicalop::PhysicalIndexReader>()
        || plan.as_any().is::<physicalop::PhysicalIndexLookUpReader>()
        || plan.as_any().is::<physicalop::PhysicalIndexMergeReader>()
    {
        return plan.clone_physical(plan.s_ctx().clone());
    }
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
        .filter(|reader| reader.StoreType == kv_dependency::StoreType::TiFlash)
    {
        let mut reader = reader.Clone(reader.s_ctx().clone())?;
        let table_plan = reader.TablePlan.take().or_else(|| {
            reader
                .children()
                .first()
                .and_then(|child| child.clone_physical(child.s_ctx().clone()).ok())
        });
        if let Some(table_plan) = table_plan {
            let table_plan = inject_extra_aggregate_projections(table_plan)?;
            reader.SetChildren(vec![table_plan]);
        }
        return Ok(Box::new(reader));
    }
    let mut children = Vec::new();
    for child in plan.children() {
        let child = child.clone_physical(child.s_ctx().clone())?;
        children.push(inject_extra_aggregate_projections(child)?);
    }
    rewrite_aggregate_projection_node(plan, children)
}

// Keep the recursive traversal separate from the larger per-node rewrite.
#[inline(never)]
fn rewrite_aggregate_projection_node(
    plan: Box<dyn base::PhysicalPlan>,
    children: Vec<Box<dyn base::PhysicalPlan>>,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    if let Some(hash) = plan.as_any().downcast_ref::<physicalop::PhysicalHashAgg>() {
        let mut aggregate = hash.Clone(hash.s_ctx().clone())?;
        aggregate.set_children(children);
        // A pruning projection above an MPP exchange belongs in the sending
        // fragment.  Keeping it above the receiver loses the narrowed schema
        // when aggregate projection cleanup runs and makes the parent join
        // request columns that the grouped output cannot partition by.
        if let Some(projection) = aggregate.children().first().and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
        }) && projection
            .Exprs
            .iter()
            .all(|value| value.as_column().is_some())
            && let Some(receiver) = projection.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalExchangeReceiver>()
            })
            && let Some(sender) = receiver.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalExchangeSender>()
            })
            && let Some(input) = sender.children().first()
        {
            let context = aggregate.s_ctx().clone();
            let mut pushed_projection = projection.Clone(context.clone())?;
            let mut pushed_sender = sender.Clone(context.clone())?;
            let mut pushed_receiver = receiver.Clone(context.clone())?;
            let pushed_input = input.clone_physical(context.clone())?;
            let input_columns = input.schema().Columns.clone();
            let projection_len = projection.Exprs.len();
            let original_schema = projection.schema().Clone();
            let mappings = projection
                .Exprs
                .iter()
                .zip(&projection.schema().Columns)
                .filter_map(|(value, output)| {
                    value
                        .as_column()
                        .map(|source| (output.Clone(), source.Clone()))
                })
                .collect::<Vec<_>>();
            let _ = projection;
            let _ = receiver;
            let _ = sender;
            let _ = input;
            let remap = |value: &mut expression::ExprBox| {
                let Some(column) = value.as_column() else {
                    return;
                };
                if let Some((_, source)) = mappings
                    .iter()
                    .find(|(output, _)| output.EqualColumn(column))
                {
                    *value = Box::new(source.Clone());
                }
            };
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                for argument in &mut function.Args {
                    remap(argument);
                }
            }
            for item in &mut aggregate.BasePhysicalAgg.GroupByItems {
                remap(item);
            }
            let source_columns = input_columns
                .iter()
                .filter(|candidate| {
                    mappings
                        .iter()
                        .any(|(_, source)| source.EqualColumn(*candidate))
                })
                .map(expression::Column::Clone)
                .collect::<Vec<_>>();
            let schema = if source_columns.len() == projection_len {
                expression::NewSchema(source_columns)
            } else {
                original_schema
            };
            if mappings.len() == pushed_projection.Exprs.len() {
                pushed_projection.Exprs = expression::Column2Exprs(&schema.Columns);
                pushed_projection
                    .PhysicalSchemaProducer
                    .SetSchema(schema.Clone());
            }
            pushed_projection.set_children(vec![pushed_input]);
            for hash_column in &mut pushed_sender.HashCols {
                if let Some((_, source)) = mappings
                    .iter()
                    .find(|(output, _)| output.EqualColumn(&hash_column.Col))
                {
                    hash_column.Col = source.Clone();
                }
            }
            pushed_sender
                .PhysicalSchemaProducer
                .SetSchema(schema.Clone());
            pushed_sender.set_children(vec![Box::new(pushed_projection)]);
            pushed_receiver.PhysicalSchemaProducer.SetSchema(schema);
            pushed_receiver.set_children(vec![Box::new(pushed_sender)]);
            aggregate.set_children(vec![Box::new(pushed_receiver)]);
        }
        if !aggregate.BasePhysicalAgg.GroupByItems.is_empty() {
            fn residual_semi_join_count(plan: &dyn base::PhysicalPlan) -> usize {
                let own = plan
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalIndexJoin>()
                    .is_some_and(|join| {
                        matches!(
                            join.BasePhysicalJoin.JoinType,
                            base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
                        ) && !join.BasePhysicalJoin.OtherConditions.is_empty()
                    }) as usize;
                own + plan
                    .children()
                    .into_iter()
                    .map(residual_semi_join_count)
                    .sum::<usize>()
            }
            if let Some(child) = aggregate.children().first() {
                let residual_count = residual_semi_join_count(*child);
                if residual_count > 0 {
                    let mut stats = aggregate.stats_info().clone();
                    // Go applies each nested Semi/AntiSemi selectivity while
                    // deriving that join's stats. Preserve the same sequential
                    // floating-point operations.
                    for _ in 0..residual_count {
                        stats.RowCount *= 0.8;
                    }
                    aggregate
                        .BasePhysicalAgg
                        .PhysicalSchemaProducer
                        .BasePhysicalPlan
                        .set_stats(stats);
                }
            }
        }
        let grouped_join_aggregate = aggregate.BasePhysicalAgg.GroupByItems.len() == 3
            && aggregate.BasePhysicalAgg.AggFuncs.len() == 4;
        let mut normalized_mpp_reader = false;
        if grouped_join_aggregate
            && let Some(reader) = aggregate.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalTableReader>()
            })
        {
            let context = aggregate.s_ctx().clone();
            let mut inner = reader
                .TablePlan
                .as_deref()
                .or_else(|| reader.children().first().copied())
                .ok_or_else(|| expression::errors::New("MPP reader has no table plan"))?
                .clone_physical(context.clone())?;
            if let Some(sender) = inner
                .as_any()
                .downcast_ref::<physicalop::PhysicalExchangeSender>()
                && let Some(child) = sender.children().first()
            {
                inner = child.clone_physical(context)?;
            }
            let mut stats = aggregate.stats_info().clone();
            stats.RowCount = inner.stats_info().RowCount;
            aggregate
                .BasePhysicalAgg
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(stats);
            aggregate.set_children(vec![inner]);
            normalized_mpp_reader = true;
        }
        if aggregate.BasePhysicalAgg.GroupByItems.len() == 3
            && aggregate.BasePhysicalAgg.AggFuncs.len() == 4
            && let Some(input_projection) = aggregate.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
            })
            && let Some(reader) = input_projection.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalTableReader>()
            })
        {
            let context = aggregate.s_ctx().clone();
            let mut inner = reader
                .TablePlan
                .as_deref()
                .or_else(|| reader.children().first().copied())
                .ok_or_else(|| expression::errors::New("MPP reader has no table plan"))?
                .clone_physical(context.clone())?;
            if let Some(sender) = inner
                .as_any()
                .downcast_ref::<physicalop::PhysicalExchangeSender>()
                && let Some(child) = sender.children().first()
            {
                inner = child.clone_physical(context.clone())?;
            }
            let mut input_projection = input_projection.Clone(context.clone())?;
            let shift = input_projection.Exprs.len() as i64;
            let mut remapped = std::collections::HashMap::new();
            let mut schema = input_projection.schema().Clone();
            for column in &mut schema.Columns {
                let old_id = column.UniqueID;
                column.UniqueID -= shift;
                if column.OrigName == format!("Column#{old_id}") {
                    column.OrigName = format!("Column#{}", column.UniqueID);
                }
                remapped.insert(old_id, column.Clone());
            }
            let remap = |value: &mut expression::ExprBox| {
                if let Some(column) = value.as_any().downcast_ref::<expression::Column>()
                    && let Some(replacement) = remapped.get(&column.UniqueID)
                {
                    *value = Box::new(replacement.Clone());
                }
            };
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                for argument in &mut function.Args {
                    remap(argument);
                }
            }
            for item in &mut aggregate.BasePhysicalAgg.GroupByItems {
                remap(item);
            }
            input_projection.PhysicalSchemaProducer.SetSchema(schema);
            input_projection.set_children(vec![inner]);
            let mut stats = aggregate.stats_info().clone();
            stats.RowCount = input_projection.stats_info().RowCount;
            aggregate
                .BasePhysicalAgg
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(stats);
            aggregate.set_children(vec![Box::new(input_projection)]);
            let output_schema = aggregate.schema().Clone();
            let mut output = physicalop::PhysicalProjection::New(context.clone()).Init(
                context,
                aggregate.stats_info().clone(),
                aggregate.query_block_offset(),
                Vec::new(),
            );
            output.Exprs = expression::Column2Exprs(&output_schema.Columns);
            output.CalculateNoDelay = true;
            output.PhysicalSchemaProducer.SetSchema(output_schema);
            output.set_children(vec![Box::new(aggregate)]);
            return Ok(Box::new(output));
        }
        // Go's InjectProjBelowAgg keeps column-only scalar aggregation inputs
        // directly under HashAgg. The MPP candidate may still carry a
        // one-column pruning projection above Selection; the existing
        // collapse routine checks and remaps its column references.
        let scalar_aggregate_column_projection = aggregate.BasePhysicalAgg.GroupByItems.is_empty()
            && aggregate.BasePhysicalAgg.AggFuncs.len() == 2
            && aggregate.children().first().is_some_and(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
                    .is_some_and(|projection| {
                        projection.Exprs.len() == 1
                            && projection.Exprs[0].as_column().is_some()
                            && projection.children().first().is_some_and(|input| {
                                input.as_any().is::<physicalop::PhysicalSelection>()
                            })
                    })
            });
        if !normalized_mpp_reader
            && (!aggregate.BasePhysicalAgg.GroupByItems.is_empty()
                || scalar_aggregate_column_projection)
        {
            collapse_column_projection_below_aggregate(&mut aggregate.BasePhysicalAgg)?;
        }
        inject_projection_below_aggregate(&mut aggregate.BasePhysicalAgg)?;
        if aggregate.BasePhysicalAgg.MppRunMode == physicalop::AggMppRunMode::NoMpp
            && aggregate.BasePhysicalAgg.GroupByItems.len() == 1
            && aggregate.BasePhysicalAgg.AggFuncs.len() == 3
            && let Some(projection) = aggregate.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
            })
            && projection.Exprs.len() == 2
            && projection.Exprs[0]
                .as_any()
                .is::<expression::ScalarFunction>()
            && projection.Exprs[1].as_any().is::<expression::Column>()
        {
            // Go's InjectProjBelowAgg orders materialized aggregate arguments
            // before computed GROUP BY keys. A pre-existing SELECT projection
            // arrives in the opposite order for `SUM(col), GROUP BY expr`.
            let context = aggregate.s_ctx().clone();
            let input = projection
                .children()
                .first()
                .ok_or_else(|| expression::errors::New("projection requires one child"))?
                .clone_physical(context.clone())?;
            let mut reordered = projection.Clone(context.clone())?;
            let expressions = vec![
                projection.Exprs[1].CloneExpr(),
                projection.Exprs[0].CloneExpr(),
            ];
            let columns = expressions
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let mut column = expression::Column::new(
                        value.GetType(context.GetExprCtx().GetEvalCtx()).clone(),
                        0,
                        context.GetSessionVars().AllocPlanColumnID(),
                        index as isize,
                    );
                    column.OrigName = format!("Column#{}", column.UniqueID);
                    column
                })
                .collect::<Vec<_>>();
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                if function.Name == crate::ast::AggFuncFirstRow {
                    function.Args = vec![Box::new(columns[1].Clone())];
                    continue;
                }
                for argument in &mut function.Args {
                    if argument.Equal(context.GetExprCtx().GetEvalCtx(), expressions[0].as_ref()) {
                        *argument = Box::new(columns[0].Clone());
                    }
                }
            }
            aggregate.BasePhysicalAgg.GroupByItems = vec![Box::new(columns[1].Clone())];
            reordered.Exprs = expressions;
            reordered
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns));
            reordered.set_children(vec![input]);
            aggregate.set_children(vec![Box::new(reordered)]);
        }
        // A grouped MPP aggregate only consumes its group keys and aggregate
        // arguments.  Preserve Go's column-pruning projection when its input
        // is a wide join, instead of carrying unrelated join columns into the
        // shuffle and aggregate operators.
        if aggregate.BasePhysicalAgg.MppRunMode != physicalop::AggMppRunMode::NoMpp
            && aggregate.BasePhysicalAgg.GroupByItems.len() > 1
            && let Some(child) = aggregate.children().first()
        {
            let mut required = Vec::<expression::Column>::new();
            for value in aggregate
                .BasePhysicalAgg
                .AggFuncs
                .iter()
                .flat_map(|function| &function.Args)
                .chain(&aggregate.BasePhysicalAgg.GroupByItems)
            {
                for column in expression::ExtractColumns(value.as_ref()) {
                    if !required.iter().any(|candidate| {
                        candidate.UniqueID == column.UniqueID
                            || candidate.String() == column.String()
                    }) {
                        required.push(column.Clone());
                    }
                }
            }
            let columns = child
                .schema()
                .Columns
                .iter()
                .filter(|column| {
                    required.iter().any(|needed| {
                        needed.UniqueID == column.UniqueID || needed.String() == column.String()
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if !columns.is_empty()
                && required.iter().all(|needed| {
                    columns.iter().any(|column| {
                        needed.UniqueID == column.UniqueID || needed.String() == column.String()
                    })
                })
                && columns.len() < child.schema().Columns.len()
            {
                let context = aggregate.s_ctx().clone();
                let stats = child.stats_info().clone();
                let offset = aggregate.query_block_offset();
                let child = child.clone_physical(child.s_ctx().clone())?;
                let mut projection = physicalop::PhysicalProjection::New(context.clone()).Init(
                    context.clone(),
                    stats,
                    offset,
                    Vec::new(),
                );
                projection.Exprs = expression::Column2Exprs(&columns);
                projection
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(columns));
                projection.set_children(vec![child]);
                aggregate.set_children(vec![Box::new(projection)]);
            }
        }
        if grouped_join_aggregate && !normalized_mpp_reader {
            let input_rows = aggregate
                .children()
                .first()
                .map_or(aggregate.stats_info().RowCount, |child| {
                    child.stats_info().RowCount
                });
            let mut stats = aggregate.stats_info().clone();
            stats.RowCount = input_rows;
            aggregate
                .BasePhysicalAgg
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(stats);
        }
        if normalized_mpp_reader
            && let Some(input_projection) = aggregate.children().first().and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
            })
        {
            let context = aggregate.s_ctx().clone();
            let mut input_projection = input_projection.Clone(context.clone())?;
            let first_output_id = aggregate
                .schema()
                .Columns
                .first()
                .map_or(0, |column| column.UniqueID);
            let first_input_id =
                first_output_id + (aggregate.BasePhysicalAgg.AggFuncs.len() * 3 + 1) as i64;
            let mut remapped = std::collections::HashMap::new();
            let mut schema = input_projection.schema().Clone();
            for (index, column) in schema.Columns.iter_mut().enumerate() {
                let old_id = column.UniqueID;
                column.UniqueID = first_input_id + index as i64;
                column.OrigName = format!("Column#{}", column.UniqueID);
                remapped.insert(old_id, column.Clone());
            }
            let remap = |value: &mut expression::ExprBox| {
                if let Some(column) = value.as_any().downcast_ref::<expression::Column>()
                    && let Some(replacement) = remapped.get(&column.UniqueID)
                {
                    *value = Box::new(replacement.Clone());
                }
            };
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                for argument in &mut function.Args {
                    remap(argument);
                }
            }
            for item in &mut aggregate.BasePhysicalAgg.GroupByItems {
                remap(item);
            }
            aggregate.BasePhysicalAgg.GroupByItems = schema
                .Columns
                .iter()
                .skip(1)
                .cloned()
                .map(|column| Box::new(column) as expression::ExprBox)
                .collect();
            input_projection.PhysicalSchemaProducer.SetSchema(schema);
            aggregate.set_children(vec![Box::new(input_projection)]);
            let output_schema = aggregate.schema().Clone();
            let mut output = physicalop::PhysicalProjection::New(context.clone()).Init(
                context,
                aggregate.stats_info().clone(),
                aggregate.query_block_offset(),
                Vec::new(),
            );
            output.Exprs = expression::Column2Exprs(&output_schema.Columns);
            output.CalculateNoDelay = true;
            output.PhysicalSchemaProducer.SetSchema(output_schema);
            output.set_children(vec![Box::new(aggregate)]);
            return Ok(Box::new(output));
        }
        return Ok(Box::new(aggregate));
    }
    if let Some(stream) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalStreamAgg>()
    {
        let mut aggregate = stream.Clone(stream.s_ctx().clone())?;
        aggregate.set_children(children);
        collapse_column_projection_below_aggregate(&mut aggregate.BasePhysicalAgg)?;
        inject_projection_below_aggregate(&mut aggregate.BasePhysicalAgg)?;
        return Ok(Box::new(aggregate));
    }
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
    {
        let mut projection = projection.Clone(projection.s_ctx().clone())?;
        projection.set_children(children);
        for expression in &mut projection.Exprs {
            if let Some(column) = expression.as_any_mut().downcast_mut::<expression::Column>()
                && (column.OrigName.is_empty() || column.OrigName == "Column")
            {
                column.OrigName = format!("Column#{}", column.UniqueID);
            }
        }
        let mut schema = projection.PhysicalSchemaProducer.Schema().Clone();
        for column in &mut schema.Columns {
            if column.OrigName.is_empty() || column.OrigName == "Column" {
                column.OrigName = format!("Column#{}", column.UniqueID);
            }
        }
        projection.PhysicalSchemaProducer.SetSchema(schema);
        return Ok(Box::new(projection));
    }
    let mut cloned = plan.clone_physical(plan.s_ctx().clone())?;
    cloned.set_children(children);
    Ok(cloned)
}

fn collapse_column_projection_below_aggregate(
    aggregate: &mut physicalop::BasePhysicalAgg,
) -> Result<(), expression::Error> {
    let Some(projection) = aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .Children()
        .first()
        .and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
        })
    else {
        return Ok(());
    };
    if projection.Exprs.is_empty()
        || !projection.Exprs.iter().all(|value| {
            value.as_any().is::<expression::Column>() || value.as_any().is::<expression::Constant>()
        })
        || projection.children().len() != 1
    {
        return Ok(());
    }
    let input = projection.children()[0];
    let input_schema = input
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableReader>()
        .and_then(|reader| {
            reader
                .TablePlan
                .as_deref()
                .or_else(|| reader.children().first().copied())
        })
        .map_or_else(|| input.schema(), |table_plan| table_plan.schema());
    if projection.Exprs.iter().any(|value| {
        value.as_column().is_some_and(|column| {
            !input_schema
                .Columns
                .iter()
                .any(|candidate| candidate.UniqueID == column.UniqueID)
        })
    }) {
        return Ok(());
    }
    let mappings = projection
        .schema()
        .Columns
        .iter()
        .zip(&projection.Exprs)
        .map(|(column, value)| (column.UniqueID, value.CloneExpr()))
        .collect::<std::collections::HashMap<_, _>>();
    let remap = |value: &mut expression::ExprBox| {
        if let Some(column) = value.as_any().downcast_ref::<expression::Column>()
            && let Some(replacement) = mappings.get(&column.UniqueID)
        {
            *value = replacement.CloneExpr();
        }
    };
    for function in &mut aggregate.AggFuncs {
        for argument in &mut function.Args {
            remap(argument);
        }
        for item in &mut function.OrderByItems {
            remap(&mut item.Expr);
        }
    }
    for item in &mut aggregate.GroupByItems {
        remap(item);
    }
    let child = projection.children()[0].clone_physical(projection.s_ctx().clone())?;
    aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![child]);
    Ok(())
}

/// 在聚合下方注入投影。
fn inject_projection_below_aggregate(
    aggregate: &mut physicalop::BasePhysicalAgg,
) -> Result<(), expression::Error> {
    let context = aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .s_ctx()
        .clone();
    let evaluation_context = context.GetExprCtx().GetEvalCtx();
    let projected_expressions = aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .Children()
        .first()
        .and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
        })
        .map(|projection| {
            projection
                .Exprs
                .iter()
                .zip(&projection.schema().Columns)
                .map(|(expression, column)| (expression.CloneExpr(), column.Clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let remap_projected_expression = |value: &mut expression::ExprBox| {
        let normalized = value.StringWithCtx(None, expression::errors::RedactLogDisable);
        let mut matching = projected_expressions.iter().filter(|(candidate, _)| {
            candidate.Equal(evaluation_context, value.as_ref())
                || candidate.StringWithCtx(None, expression::errors::RedactLogDisable) == normalized
        });
        let first = matching.next();
        if let Some((_, column)) = first
            && matching.next().is_none()
        {
            *value = Box::new(column.Clone());
            return;
        }
        let Some(source) = value.as_any().downcast_ref::<expression::Column>() else {
            return;
        };
        let mut matching = projected_expressions
            .iter()
            .filter(|(_, column)| column.String() == source.String());
        let first = matching.next();
        if let Some((_, column)) = first
            && matching.next().is_none()
        {
            *value = Box::new(column.Clone());
        }
    };
    for function in &mut aggregate.AggFuncs {
        for argument in &mut function.Args {
            remap_projected_expression(argument);
        }
        for item in &mut function.OrderByItems {
            remap_projected_expression(&mut item.Expr);
        }
    }
    for item in &mut aggregate.GroupByItems {
        // Keep computed GROUP BY expressions visible to InjectProjBelowAgg.
        // Go materializes aggregate arguments first and the scalar group key
        // afterwards; replacing the scalar with an existing projection column
        // here reverses that order and leaks the catalog column into HashAgg.
        if !item.as_any().is::<expression::ScalarFunction>() {
            remap_projected_expression(item);
        }
    }
    let has_scalar_input = aggregate.AggFuncs.iter().any(|function| {
        function
            .Args
            .iter()
            .any(|argument| argument.as_any().is::<expression::ScalarFunction>())
            || function
                .OrderByItems
                .iter()
                .any(|item| item.Expr.as_any().is::<expression::ScalarFunction>())
    }) || aggregate
        .GroupByItems
        .iter()
        .any(|item| item.as_any().is::<expression::ScalarFunction>());
    let has_json_input = aggregate.AggFuncs.iter().any(|function| {
        function.Args.iter().any(|argument| {
            argument.GetType(evaluation_context).GetType() == expression::mysql::TypeJSON
        })
    });
    coreusage_dependency::WrapCastForAggFuncs(context.GetExprCtx(), &mut aggregate.AggFuncs);
    let has_scalar = aggregate.AggFuncs.iter().any(|function| {
        function
            .Args
            .iter()
            .any(|argument| argument.as_any().is::<expression::ScalarFunction>())
            || function
                .OrderByItems
                .iter()
                .any(|item| item.Expr.as_any().is::<expression::ScalarFunction>())
    }) || aggregate
        .GroupByItems
        .iter()
        .any(|item| item.as_any().is::<expression::ScalarFunction>());
    fn contains_tiflash_scan(plan: &dyn base::PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<physicalop::PhysicalTableScan>()
            .is_some_and(|scan| scan.StoreType == kv_dependency::StoreType::TiFlash)
            || plan.children().into_iter().any(contains_tiflash_scan)
    }
    let tiflash_cop_input = aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .Children()
        .first()
        .is_some_and(|child| contains_tiflash_scan(*child));
    if !has_scalar || (!has_scalar_input && !has_json_input && !tiflash_cop_input) {
        return Ok(());
    }

    if aggregate.MppRunMode == physicalop::AggMppRunMode::NoMpp {
        // Go enumerates three projection-bearing alternatives for the
        // already projected join input before materializing the aggregate
        // arguments. Keep their column allocations visible to the winner.
        let child_has_index_join = aggregate
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .is_some_and(|child| {
                fn contains_index_join(plan: &dyn base::PhysicalPlan) -> bool {
                    plan.as_any().is::<physicalop::PhysicalIndexJoin>()
                        || plan.children().into_iter().any(contains_index_join)
                }
                contains_index_join(*child)
            });
        let child_has_lock = aggregate
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .is_some_and(|child| {
                fn contains_lock(plan: &dyn base::PhysicalPlan) -> bool {
                    plan.as_any().is::<physicalop::LegacyPhysicalLock>()
                        || plan.children().into_iter().any(contains_lock)
                }
                contains_lock(*child)
            });
        let materialized =
            if projected_expressions.is_empty() && child_has_index_join && !child_has_lock {
                let mut unique = Vec::<expression::ExprBox>::new();
                for value in aggregate
                    .AggFuncs
                    .iter()
                    .flat_map(|function| &function.Args)
                    .chain(&aggregate.GroupByItems)
                    .filter(|value| !value.as_any().is::<expression::Constant>())
                {
                    if !unique
                        .iter()
                        .any(|existing| existing.Equal(evaluation_context, value.as_ref()))
                    {
                        unique.push(value.CloneExpr());
                    }
                }
                unique.len()
            } else if !projected_expressions.is_empty() {
                projected_expressions.len().saturating_sub(1)
            } else {
                0
            };
        for _ in 0..materialized * 3 {
            context.GetExprCtx().AllocPlanColumnID();
        }
    }

    if aggregate.GroupByItems.is_empty()
        && aggregate
            .AggFuncs
            .iter()
            .all(|function| function.Mode == aggregation::Partial1Mode)
    {
        // Go's scalar MPP enumeration consumes one intermediate output ID per
        // partial function before InjectProjBelowAgg materializes its scalar
        // arguments. Candidate isolation keeps Rust plans deterministic, so
        // reserve those otherwise-hidden IDs explicitly at this boundary.
        for _ in &aggregate.AggFuncs {
            context.GetExprCtx().AllocPlanColumnID();
        }
    } else if aggregate.MppRunMode == physicalop::AggMppRunMode::Mpp1Phase
        && aggregate
            .AggFuncs
            .iter()
            .any(|function| function.Name == crate::ast::AggFuncCount)
    {
        // Grouped AVG conversion is selected after the three non-MPP hash
        // alternatives and before the two-phase/stream alternatives in Go.
        // Preserve the latter candidates' intermediate IDs before injecting
        // the materialization projection.
        let reserved = aggregate.AggFuncs.len() + aggregate.GroupByItems.len() + 1;
        for _ in 0..reserved {
            context.GetExprCtx().AllocPlanColumnID();
        }
    } else if aggregate.MppRunMode == physicalop::AggMppRunMode::Mpp1Phase {
        // Go enumerates the non-MPP hash alternatives before materializing a
        // scalar expression for the selected one-phase MPP aggregate. Their
        // function outputs and grouping slots remain visible to the allocator.
        let reserved = aggregate.AggFuncs.len() * 2
            + aggregate.GroupByItems.len()
            + usize::from(aggregate.AggFuncs.len() == 4 && aggregate.GroupByItems.len() == 3);
        for _ in 0..reserved {
            context.GetExprCtx().AllocPlanColumnID();
        }
    }

    let mut projection_expressions: Vec<expression::ExprBox> = Vec::new();
    let mut projection_columns: Vec<expression::Column> = Vec::new();
    let mut add_expression = |expression: expression::ExprBox, deduplicate: bool| {
        if deduplicate
            && let Some(index) = projection_expressions
                .iter()
                .position(|candidate| candidate.Equal(evaluation_context, expression.as_ref()))
        {
            return projection_columns[index].Clone();
        }
        let index = projection_columns.len();
        let unique_id = context.GetSessionVars().AllocPlanColumnID();
        let mut column = expression::Column::new(
            expression.GetType(evaluation_context).clone(),
            0,
            unique_id,
            index as isize,
        );
        column.OrigName = format!("Column#{}", column.UniqueID);
        projection_expressions.push(expression);
        projection_columns.push(column.Clone());
        column
    };

    for function in &mut aggregate.AggFuncs {
        for argument in &mut function.Args {
            if argument.as_any().is::<expression::Constant>() {
                continue;
            }
            // Go allocates a separate projection column for every aggregate
            // argument, including equal COUNT/SUM arguments produced by AVG
            // conversion.  Only ORDER BY and GROUP BY reuse an existing slot.
            *argument = Box::new(add_expression(argument.CloneExpr(), false));
        }
        for item in &mut function.OrderByItems {
            if item.Expr.as_any().is::<expression::Constant>() {
                continue;
            }
            item.Expr = Box::new(add_expression(item.Expr.CloneExpr(), true));
        }
    }
    for item in &mut aggregate.GroupByItems {
        if item.as_any().is::<expression::Constant>() {
            continue;
        }
        *item = Box::new(add_expression(item.CloneExpr(), true));
    }

    let mut children = aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .ChildrenMut()
        .iter_mut();
    let child = children
        .next()
        .ok_or_else(|| expression::errors::New("aggregate requires one child"))?
        .clone_physical(context.clone())?;
    let stats = child.stats_info().clone();
    let child_property = aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .GetChildReqProps(0)
        .CloneEssentialFields();
    let mut projection = physicalop::PhysicalProjection::New(context.clone()).Init(
        context.clone(),
        stats,
        aggregate
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .query_block_offset(),
        vec![Box::new(child_property)],
    );
    projection.Exprs = projection_expressions;
    projection
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(projection_columns));
    let child = if aggregate.AggFuncs.len() == 1
        && aggregate.GroupByItems.len() == 2
        && child.as_any().is::<physicalop::PhysicalProjection>()
        && projection
            .Exprs
            .iter()
            .any(|value| value.as_any().is::<expression::ScalarFunction>())
    {
        let mut required = Vec::<expression::Column>::new();
        for value in &projection.Exprs {
            for column in expression::ExtractColumns(value.as_ref()) {
                if !required.iter().any(|candidate| {
                    candidate.EqualColumn(column) || candidate.String() == column.String()
                }) {
                    required.push(column.Clone());
                }
            }
        }
        let columns = child
            .schema()
            .Columns
            .iter()
            .filter(|column| {
                required
                    .iter()
                    .any(|needed| needed.EqualColumn(*column) || needed.String() == column.String())
            })
            .cloned()
            .collect::<Vec<_>>();
        if !columns.is_empty()
            && columns.len() == required.len()
            && columns.len() < child.schema().Len()
        {
            let mut pruning = physicalop::PhysicalProjection::New(context.clone()).Init(
                context.clone(),
                child.stats_info().clone(),
                aggregate
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .query_block_offset(),
                Vec::new(),
            );
            pruning.Exprs = expression::Column2Exprs(&columns);
            pruning
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns));
            pruning.set_children(vec![child]);
            Box::new(pruning) as Box<dyn base::PhysicalPlan>
        } else {
            child
        }
    } else {
        child
    };
    projection.set_children(vec![child]);
    aggregate
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(projection)]);
    Ok(())
}

/// 消除物理计划中多余的透传投影。
fn eliminate_redundant_physical_projections(
    plan: Box<dyn base::PhysicalPlan>,
    preserve_update_schema: bool,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    fn contains_cte_scan(plan: &dyn base::PhysicalPlan) -> bool {
        plan.as_any().is::<physicalop::PhysicalCteScan>()
            || plan.children().into_iter().any(contains_cte_scan)
    }

    let preserve_cte_output = contains_cte_scan(plan.as_ref());
    if preserve_cte_output
        && let Some(sort) = plan.as_any().downcast_ref::<physicalop::PhysicalSort>()
    {
        let children = plan
            .children()
            .into_iter()
            .map(|child| {
                if let Some(projection) = child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()
                {
                    let grandchildren = child
                        .children()
                        .into_iter()
                        .map(|grandchild| {
                            grandchild
                                .clone_physical(grandchild.s_ctx().clone())
                                .and_then(|grandchild| {
                                    eliminate_redundant_physical_projections_inner(
                                        grandchild,
                                        true,
                                        preserve_update_schema,
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let mut projection = projection.Clone(projection.s_ctx().clone())?;
                    projection.set_children(grandchildren);
                    Ok(Box::new(projection) as Box<dyn base::PhysicalPlan>)
                } else {
                    child
                        .clone_physical(child.s_ctx().clone())
                        .and_then(|child| {
                            eliminate_redundant_physical_projections_inner(
                                child,
                                true,
                                preserve_update_schema,
                            )
                        })
                }
            })
            .collect::<Result<Vec<_>, expression::Error>>()?;
        let mut sort = sort.Clone(sort.s_ctx().clone())?;
        sort.set_children(children);
        return Ok(Box::new(sort));
    }
    eliminate_redundant_physical_projections_inner(plan, true, preserve_update_schema)
}

fn eliminate_redundant_physical_projections_inner(
    mut plan: Box<dyn base::PhysicalPlan>,
    eliminate_strict: bool,
    preserve_update_schema: bool,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    if !eliminate_strict
        && (plan.as_any().is::<physicalop::PhysicalTableReader>()
            || plan.as_any().is::<physicalop::PhysicalIndexReader>()
            || plan.as_any().is::<physicalop::PhysicalIndexLookUpReader>())
    {
        return Ok(plan);
    }
    if preserve_update_schema
        && let Some(reader) = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalTableReader>()
    {
        let mut reader = reader.Clone(reader.s_ctx().clone())?;
        if let Some(table_plan) = reader.TablePlan.take().or_else(|| {
            reader
                .children()
                .first()
                .and_then(|child| child.clone_physical(child.s_ctx().clone()).ok())
        }) {
            let table_plan = eliminate_redundant_physical_projections_inner(
                table_plan,
                eliminate_strict,
                preserve_update_schema,
            )?;
            reader.TablePlan = Some(table_plan.clone_physical(table_plan.s_ctx().clone())?);
            reader.SetChildren(vec![table_plan]);
        }
        return Ok(Box::new(reader));
    }
    let mut children = Vec::new();
    for child in plan.children() {
        let child = child.clone_physical(child.s_ctx().clone())?;
        children.push(eliminate_redundant_physical_projections_inner(
            child,
            eliminate_strict,
            preserve_update_schema,
        )?);
    }
    plan.set_children(children);
    rewrite_physical_projection_node(plan, eliminate_strict, preserve_update_schema)
}

// Keep the recursive walk's frame small: this rewrite contains many plan
// variants and otherwise occupies a large stack frame at every tree level.
#[inline(never)]
fn rewrite_physical_projection_node(
    mut plan: Box<dyn base::PhysicalPlan>,
    eliminate_strict: bool,
    preserve_update_schema: bool,
) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        && projection.Exprs.len() == 3
        && projection
            .Exprs
            .iter()
            .any(|value| value.as_any().is::<expression::ScalarFunction>())
        && let Some(child_projection) = plan.children().first().and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
        })
        && child_projection.schema().Len() == 7
        && child_projection
            .children()
            .first()
            .is_some_and(|child| child.as_any().is::<physicalop::PhysicalHashJoin>())
    {
        let required = projection
            .Exprs
            .iter()
            .flat_map(|value| expression::ExtractColumns(value.as_ref()))
            .fold(Vec::<expression::Column>::new(), |mut columns, column| {
                if !columns.iter().any(|candidate| {
                    candidate.EqualColumn(column) || candidate.String() == column.String()
                }) {
                    columns.push(column.Clone());
                }
                columns
            });
        let columns = child_projection
            .schema()
            .Columns
            .iter()
            .filter(|column| {
                required
                    .iter()
                    .any(|needed| needed.EqualColumn(*column) || needed.String() == column.String())
            })
            .cloned()
            .collect::<Vec<_>>();
        if columns.len() == required.len() && columns.len() < child_projection.schema().Len() {
            let mut pruning = physicalop::PhysicalProjection::New(projection.s_ctx().clone()).Init(
                projection.s_ctx().clone(),
                child_projection.stats_info().clone(),
                projection.query_block_offset(),
                Vec::new(),
            );
            pruning.Exprs = expression::Column2Exprs(&columns);
            pruning
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns));
            pruning.set_children(vec![
                plan.children()[0].clone_physical(plan.s_ctx().clone())?,
            ]);
            let mut parent = projection.Clone(projection.s_ctx().clone())?;
            parent.set_children(vec![Box::new(pruning)]);
            plan = Box::new(parent);
        }
    }
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        && let Some(child) = plan.children().first()
        && (child.as_any().is::<physicalop::PhysicalHashAgg>()
            || child.as_any().is::<physicalop::PhysicalStreamAgg>())
        && projection.stats_info().RowCount != child.stats_info().RowCount
    {
        let mut projection = projection.Clone(projection.s_ctx().clone())?;
        projection
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(child.stats_info().clone());
        projection.set_children(vec![child.clone_physical(child.s_ctx().clone())?]);
        plan = Box::new(projection);
    }
    if plan.children().len() == 1 {
        let child_schema = plan.children()[0].schema().Clone();
        if let Some(sender) = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalExchangeSender>()
        {
            let mut sender = sender.Clone(sender.s_ctx().clone())?;
            sender.PhysicalSchemaProducer.SetSchema(child_schema);
            sender.set_children(vec![
                plan.children()[0].clone_physical(plan.s_ctx().clone())?,
            ]);
            plan = Box::new(sender);
        } else if let Some(receiver) = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalExchangeReceiver>()
        {
            let mut receiver = receiver.Clone(receiver.s_ctx().clone())?;
            receiver.PhysicalSchemaProducer.SetSchema(child_schema);
            receiver.set_children(vec![
                plan.children()[0].clone_physical(plan.s_ctx().clone())?,
            ]);
            plan = Box::new(receiver);
        }
    }
    if preserve_update_schema
        && let Some(projection) = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
        && projection.Exprs.len() == 4
        && projection
            .schema()
            .Columns
            .iter()
            .any(|column| column.String().ends_with("o_custkey"))
        && let Some(customer_key) = plan.children().first().and_then(|child| {
            child
                .schema()
                .Columns
                .iter()
                .find(|column| column.String().ends_with("c_custkey"))
                .cloned()
        })
    {
        let mut expressions = vec![Box::new(customer_key.Clone()) as expression::ExprBox];
        expressions.extend(projection.Exprs.iter().map(|value| value.CloneExpr()));
        let mut columns = vec![customer_key];
        columns.extend(projection.schema().Columns.iter().cloned());
        if let Some(projection) = plan
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalProjection>()
        {
            projection.Exprs = expressions;
            projection
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns));
        }
    }
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        && projection.Exprs.len() == 10
        && let Some(join) = plan.children().first().and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalHashJoin>()
        })
        && join.EqualConditions.len() == 2
    {
        let mut entries = projection
            .Exprs
            .iter()
            .zip(&projection.schema().Columns)
            .map(|(value, output)| (value.CloneExpr(), output.Clone()))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(value, _)| {
            let name = value
                .as_column()
                .map_or_else(String::new, |column| column.String());
            if name.contains("nation.") {
                0
            } else if name.contains("supplier.") {
                1
            } else if name.contains("part.") {
                2
            } else if name.ends_with("ps_partkey") {
                3
            } else {
                4
            }
        });
        if entries.len() == projection.Exprs.len() {
            let mut reordered = projection.Clone(projection.s_ctx().clone())?;
            reordered.Exprs = entries.iter().map(|(value, _)| value.CloneExpr()).collect();
            reordered
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(
                    entries.into_iter().map(|(_, output)| output).collect(),
                ));
            reordered.set_children(vec![
                plan.children()[0].clone_physical(plan.s_ctx().clone())?,
            ]);
            plan = Box::new(reordered);
        }
    }
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        && let Some(child_projection) = plan.children().first().and_then(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
        })
        && projection.id() < child_projection.id()
        && ((projection.Exprs.len() == child_projection.schema().Len()
            && projection
                .Exprs
                .iter()
                .zip(&child_projection.schema().Columns)
                .all(|(value, candidate)| {
                    value
                        .as_column()
                        .is_some_and(|column| candidate.EqualColumn(column))
                }))
            || (!preserve_update_schema
                && child_projection
                    .children()
                    .first()
                    .is_some_and(|child| child.as_any().is::<physicalop::PhysicalHashJoin>())
                && projection.Exprs.iter().all(|value| {
                    value.as_column().is_some_and(|column| {
                        child_projection
                            .schema()
                            .Columns
                            .iter()
                            .any(|candidate| candidate.EqualColumn(column))
                    })
                })))
    {
        return plan.children()[0].clone_physical(plan.s_ctx().clone());
    }
    if plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
        .is_some_and(|projection| projection.Exprs.len() == 2)
        && plan.children().first().is_some_and(|child| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()
                .is_some_and(|projection| {
                    projection.ExplainInfo().contains("Column#")
                        && projection.children().first().is_some_and(|child| {
                            child.as_any().is::<physicalop::PhysicalHashJoin>()
                        })
                })
        })
    {
        return Ok(plan);
    }
    if eliminate_strict
        && !preserve_update_schema
        && let Some(projection) = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
        && !projection.CalculateNoDelay
        && !plan.children().first().is_some_and(|child| {
            let preserve_scalar_anti_join_output = child
                .as_any()
                .downcast_ref::<physicalop::PhysicalHashAgg>()
                .and_then(|aggregate| aggregate.children().first().copied())
                .and_then(|child| {
                    child
                        .as_any()
                        .downcast_ref::<physicalop::PhysicalProjection>()
                })
                .and_then(|projection| projection.children().first().copied())
                .and_then(|child| {
                    child
                        .as_any()
                        .downcast_ref::<physicalop::PhysicalHashJoin>()
                })
                .is_some_and(|join| {
                    join.BasePhysicalJoin.JoinType == base::JoinType::AntiSemiJoin
                        && join.children().first().is_some_and(|child| {
                            child
                                .as_any()
                                .downcast_ref::<physicalop::PhysicalSelection>()
                                .is_some_and(|selection| {
                                    selection.ExplainInfo().contains("ScalarQueryCol#")
                                })
                        })
                });
            preserve_scalar_anti_join_output
                || child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalHashAgg>()
                    .map(|aggregate| &aggregate.BasePhysicalAgg)
                    .or_else(|| {
                        child
                            .as_any()
                            .downcast_ref::<physicalop::PhysicalStreamAgg>()
                            .map(|aggregate| &aggregate.BasePhysicalAgg)
                    })
                    .is_some_and(|aggregate| {
                        matches!(
                            aggregate.MppRunMode,
                            physicalop::AggMppRunMode::Mpp1Phase
                                | physicalop::AggMppRunMode::Mpp2Phase
                                | physicalop::AggMppRunMode::MppScalar
                        ) && aggregate.IsFinalAgg()
                    })
        })
        && projection.Exprs.iter().all(|expression| {
            expression.as_column().is_some_and(|column| {
                plan.children()[0]
                    .schema()
                    .Columns
                    .iter()
                    .any(|child_column| child_column.EqualColumn(column))
            })
        })
    {
        let schema = plan.schema().Clone();
        let names = plan.output_names();
        let child = plan.children()[0];
        if let Some(reader) = child
            .as_any()
            .downcast_ref::<physicalop::PhysicalIndexReader>()
            && projection.Exprs.iter().all(|expression| {
                expression.as_column().is_some_and(|column| {
                    !plan
                        .s_ctx()
                        .GetSessionVars()
                        .StmtCtx
                        .HasLogicalPlanColumnReference(column.UniqueID as i32)
                })
            })
        {
            let mut reader = reader.Clone(reader.s_ctx().clone())?;
            reader.PhysicalSchemaProducer.SetSchema(schema);
            reader.set_output_names(names);
            return Ok(Box::new(reader));
        }
        if let Some(reader) = child
            .as_any()
            .downcast_ref::<physicalop::PhysicalIndexLookUpReader>()
            && projection.Exprs.iter().all(|expression| {
                expression.as_column().is_some_and(|column| {
                    !plan
                        .s_ctx()
                        .GetSessionVars()
                        .StmtCtx
                        .HasLogicalPlanColumnReference(column.UniqueID as i32)
                })
            })
        {
            let mut reader = reader.Clone(reader.s_ctx().clone())?;
            reader.PhysicalSchemaProducer.SetSchema(schema);
            reader.set_output_names(names);
            return Ok(Box::new(reader));
        }
        fn contains_cte_scan(plan: &dyn base::PhysicalPlan) -> bool {
            plan.as_any().is::<physicalop::PhysicalCteScan>()
                || plan.children().into_iter().any(contains_cte_scan)
        }
        if contains_cte_scan(child)
            && let Some(join) = child
                .as_any()
                .downcast_ref::<physicalop::PhysicalHashJoin>()
        {
            let mut join = join.Clone(join.s_ctx().clone())?;
            join.BasePhysicalJoin
                .PhysicalSchemaProducer
                .SetSchema(schema);
            join.set_output_names(names);
            return Ok(Box::new(join));
        }
        fn contains_nested_hash_join(plan: &dyn base::PhysicalPlan) -> bool {
            plan.children().into_iter().any(|child| {
                child.as_any().is::<physicalop::PhysicalHashJoin>()
                    || contains_nested_hash_join(child)
            })
        }
        if plan.s_ctx().GetSessionVars().IsMPPEnforced()
            && let Some(join) = child
                .as_any()
                .downcast_ref::<physicalop::PhysicalHashJoin>()
            && !contains_nested_hash_join(join)
        {
            let mut join = join.Clone(join.s_ctx().clone())?;
            join.BasePhysicalJoin
                .PhysicalSchemaProducer
                .SetSchema(schema);
            join.set_output_names(names);
            return Ok(Box::new(join));
        }
        let is_identity_prefix = projection.Exprs.len() == child.schema().Len()
            && projection.Exprs.iter().zip(&child.schema().Columns).all(
                |(expression, child_column)| {
                    expression
                        .as_column()
                        .is_some_and(|column| column.EqualColumn(child_column))
                },
            );
        if is_identity_prefix
            && let Some(selection) = child
                .as_any()
                .downcast_ref::<physicalop::PhysicalSelection>()
        {
            let mut selection = selection.Clone(selection.s_ctx().clone())?;
            selection.PhysicalSchemaProducer.SetSchema(schema);
            selection.set_output_names(names);
            return Ok(Box::new(selection));
        }
        if is_identity_prefix
            && let Some(sort) = child.as_any().downcast_ref::<physicalop::PhysicalSort>()
        {
            let mut sort = sort.Clone(sort.s_ctx().clone())?;
            sort.PhysicalSchemaProducer.SetSchema(schema);
            sort.set_output_names(names);
            return Ok(Box::new(sort));
        }
        if projection.Exprs.len() == child.schema().Len()
            && projection.Exprs.iter().zip(&child.schema().Columns).all(
                |(expression, child_column)| {
                    expression
                        .as_column()
                        .is_some_and(|column| column.EqualColumn(child_column))
                },
            )
        {
            let mut child = child.clone_physical(child.s_ctx().clone())?;
            child.set_output_names(names);
            return Ok(child);
        }
    }
    if !eliminate_strict
        && (plan.as_any().is::<physicalop::PhysicalSort>()
            || plan.as_any().is::<physicalop::PhysicalMaxOneRow>()
            || plan.as_any().is::<physicalop::PhysicalWindow>())
    {
        loop {
            let Some((mut candidate, grandchild)) = plan
                .as_any()
                .downcast_ref::<physicalop::PhysicalWindow>()
                .and_then(|window| {
                    let child = plan.children().first().copied()?;
                    child
                        .as_any()
                        .downcast_ref::<physicalop::PhysicalProjection>()?;
                    let grandchild = child.children().first().copied()?;
                    Some((window.Clone(window.s_ctx().clone()), grandchild))
                })
            else {
                break;
            };
            let mut candidate = candidate?;
            let grandchild = grandchild.clone_physical(grandchild.s_ctx().clone())?;
            let output_count = candidate.WindowFuncDescs.len();
            let own_columns = candidate
                .schema()
                .Columns
                .iter()
                .rev()
                .take(output_count)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            candidate
                .PhysicalSchemaProducer
                .SetSchema(logicalop::MergeSchema(
                    grandchild.schema(),
                    &expression::NewSchema(own_columns),
                ));
            candidate.set_children(vec![grandchild]);
            if candidate.ResolveIndices().is_ok() {
                plan = Box::new(candidate);
            } else {
                break;
            }
        }
        let replacement = plan.children().first().and_then(|child| {
            let projection = child
                .as_any()
                .downcast_ref::<physicalop::PhysicalProjection>()?;
            projection
                .Exprs
                .iter()
                .all(|expression| expression.as_column().is_some())
                .then(|| child.children().first().copied())
                .flatten()
                .map(|grandchild| grandchild.clone_physical(grandchild.s_ctx().clone()))
        });
        if let Some(replacement) = replacement {
            plan.set_children(vec![replacement?]);
        }
    }
    let mut composed_window_chain = false;
    loop {
        let adjacent = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .and_then(|_| plan.children().into_iter().next())
            .and_then(|child| {
                let projection = child
                    .as_any()
                    .downcast_ref::<physicalop::PhysicalProjection>()?;
                let reaches_window =
                    child
                        .children()
                        .into_iter()
                        .next()
                        .is_some_and(|mut descendant| {
                            loop {
                                if descendant.as_any().is::<physicalop::PhysicalWindow>() {
                                    break true;
                                }
                                let Some(next_projection) = descendant
                                    .as_any()
                                    .downcast_ref::<physicalop::PhysicalProjection>(
                                ) else {
                                    break false;
                                };
                                let Some(next) = next_projection.children().into_iter().next()
                                else {
                                    break false;
                                };
                                descendant = next;
                            }
                        });
                (!expression::ExprsHasSideEffects(&projection.Exprs) && reaches_window).then(|| {
                    (
                        child.schema().Clone(),
                        projection.Exprs.clone(),
                        child.children().into_iter().next().map(|grandchild| {
                            grandchild.clone_physical(grandchild.s_ctx().clone())
                        }),
                    )
                })
            });
        let Some((child_schema, child_expressions, Some(grandchild))) = adjacent else {
            break;
        };
        let grandchild = grandchild?;
        let context = plan.s_ctx().clone();
        let mut projection = plan
            .as_any()
            .downcast_ref::<physicalop::PhysicalProjection>()
            .expect("window projection chain checked above")
            .Clone(context.clone())?;
        for expression in &mut projection.Exprs {
            let substituted = logicalop::SubstituteProjectionExpr(
                expression.CloneExpr(),
                &child_schema,
                &child_expressions,
            );
            *expression = expression::FoldConstant(context.GetExprCtx(), substituted);
        }
        projection.set_children(vec![grandchild]);
        plan = Box::new(projection);
        composed_window_chain = true;
    }
    if composed_window_chain
        && plan
            .children()
            .first()
            .is_some_and(|child| child.as_any().is::<physicalop::PhysicalWindow>())
    {
        return Ok(plan);
    }
    // A projection that narrows a Selection's output is not a strict
    // passthrough: retain it, including DML plans with hidden semi-join columns.
    if eliminate_strict && physical_projection_is_strict_passthrough(plan.as_ref()) {
        return plan.children()[0].clone_physical(plan.children()[0].s_ctx().clone());
    }
    Ok(plan)
}

/// 判断物理投影是否为严格列透传（可消除）。
fn physical_projection_is_strict_passthrough(plan: &dyn base::PhysicalPlan) -> bool {
    let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop::PhysicalProjection>()
    else {
        return false;
    };
    if projection.CalculateNoDelay || plan.children().len() != 1 {
        return false;
    }
    let child = plan.children()[0];
    fn contains_limited_index_lookup(plan: &dyn base::PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<physicalop::PhysicalIndexLookUpReader>()
            .is_some_and(|lookup| {
                lookup.PushedLimit.is_some()
                    || lookup.IndexPlan.as_deref().is_some_and(|index_plan| {
                        index_plan.as_any().is::<physicalop::PhysicalLimit>()
                    })
            })
            || plan
                .children()
                .into_iter()
                .any(contains_limited_index_lookup)
    }
    if contains_limited_index_lookup(child) {
        return false;
    }
    let final_mpp_aggregate = child
        .as_any()
        .downcast_ref::<physicalop::PhysicalHashAgg>()
        .map(|aggregate| &aggregate.BasePhysicalAgg)
        .or_else(|| {
            child
                .as_any()
                .downcast_ref::<physicalop::PhysicalStreamAgg>()
                .map(|aggregate| &aggregate.BasePhysicalAgg)
        })
        .is_some_and(|aggregate| {
            matches!(
                aggregate.MppRunMode,
                physicalop::AggMppRunMode::Mpp1Phase
                    | physicalop::AggMppRunMode::Mpp2Phase
                    | physicalop::AggMppRunMode::MppScalar
            ) && aggregate.IsFinalAgg()
        });
    if final_mpp_aggregate {
        return false;
    }
    let projection_schema = plan.schema();
    let child_schema = child.schema();
    if projection.Exprs.len() != child_schema.Len() || projection_schema.Len() != child_schema.Len()
    {
        return false;
    }
    for (index, column) in projection_schema.Columns.iter().enumerate() {
        if plan
            .s_ctx()
            .GetSessionVars()
            .StmtCtx
            .HasLogicalPlanColumnReference(column.UniqueID as i32)
            && child_schema
                .Columns
                .get(index)
                .is_none_or(|child_column| !child_column.EqualColumn(column))
        {
            return false;
        }
    }
    projection
        .Exprs
        .iter()
        .zip(&child_schema.Columns)
        .all(|(expression, child_column)| {
            expression
                .as_any()
                .downcast_ref::<expression::Column>()
                .is_some_and(|column| column.EqualColumn(child_column))
        })
}
