// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::planbuilder::{BuiltPlan, NewPlanBuilder as NewCorePlanBuilder, Privilege, Statement};
use crate::*;
use planctx_dependency::MLogCommitTSEstimation;

fn column(name: &str, not_null: bool) -> MViewColumn {
    MViewColumn {
        name: name.to_owned(),
        not_null,
    }
}

fn aggregate(kind: MViewAggKind, mv_offset: usize, argument: &str) -> MViewAggregate {
    MViewAggregate {
        kind,
        mv_offset,
        argument: argument.to_owned(),
    }
}

fn base_definition() -> MViewDefinition {
    MViewDefinition {
        database: "test".to_owned(),
        mv_table_id: 3,
        base_table_id: 1,
        mlog_table_id: 2,
        mv_columns: vec![
            column("a", true),
            column("cnt", true),
            column("cnt_b", true),
            column("s", false),
        ],
        base_columns: vec![column("a", true), column("b", false)],
        mlog_columns: vec![
            "a".to_owned(),
            "b".to_owned(),
            MLOG_DML_TYPE_COLUMN.to_owned(),
            MLOG_OLD_NEW_COLUMN.to_owned(),
        ],
        group_key_mv_offsets: vec![0],
        group_key_base_columns: vec!["a".to_owned()],
        aggregates: vec![
            aggregate(MViewAggKind::CountStar, 1, ""),
            aggregate(MViewAggKind::Count, 2, "b"),
            aggregate(MViewAggKind::Sum, 3, "b"),
        ],
        handle_kind: MViewHandleKind::ExtraRowId,
        common_handle_offsets: Vec::new(),
        base_indexes: vec![MViewIndex {
            name: "idx_a".to_owned(),
            columns: vec!["a".to_owned()],
            visible: true,
        }],
    }
}

#[test]
fn test_build_dry_run_refresh_materialized_view_plan() {
    let plan = BuildRefreshMaterializedView(&RefreshRequest {
        current_database: "test".to_owned(),
        database: String::new(),
        view: "mv".to_owned(),
        observe: RefreshObserveType::DryRun,
    })
    .unwrap();
    let RefreshPlan::DryRun(plan) = plan else {
        panic!("expected dry-run plan")
    };
    assert_eq!(plan.schema.columns, ["refresh steps"]);
}

#[test]
fn test_build_profile_refresh_materialized_view_plan() {
    let plan = BuildRefreshMaterializedView(&RefreshRequest {
        current_database: "test".to_owned(),
        database: String::new(),
        view: "mv".to_owned(),
        observe: RefreshObserveType::Profile,
    })
    .unwrap();
    let RefreshPlan::Profile(plan) = plan else {
        panic!("expected profile plan")
    };
    assert_eq!(plan.schema.columns, ["refresh steps"]);
}

#[test]
fn test_build_count_sum_and_count_expr_sum_expr() {
    let result = BuildMView(&base_definition(), MViewBuildOptions::default()).unwrap();
    assert_eq!(result.delta_column_count, 3);
    assert_eq!(result.source_column_count, 8);
    assert_eq!(result.agg_infos[0].dependencies, [0]);
    assert_eq!(result.agg_infos[1].dependencies, [1]);
    assert_eq!(result.agg_infos[2].dependencies, [2, 5]);
    assert_eq!(result.mv_table_pk_cols, [7]);
}

#[test]
fn test_build_mlog_delta_select_commit_ts_window_and_tiflash_hint() {
    let result = BuildMView(
        &base_definition(),
        MViewBuildOptions {
            from_ts: 11,
            to_ts: 29,
        },
    )
    .unwrap();
    assert_eq!(
        result.commit_ts_predicates,
        ["_tidb_commit_ts > 11", "_tidb_commit_ts <= 29"]
    );
    assert!(result.read_from_tiflash);
}

#[test]
fn test_build_merge_source_select_join_operator_by_mv_nullability() {
    let mut definition = base_definition();
    definition.mv_columns[0].not_null = false;
    let nullable = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    assert_eq!(nullable.merge_join_operators, ["<=>"]);
    definition.mv_columns[0].not_null = true;
    let not_null = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    assert_eq!(not_null.merge_join_operators, ["="]);
}

fn min_max_definition() -> MViewDefinition {
    let mut definition = base_definition();
    definition
        .mv_columns
        .extend([column("mx", false), column("mn", false)]);
    definition
        .aggregates
        .push(aggregate(MViewAggKind::Max, 4, "b"));
    definition
        .aggregates
        .push(aggregate(MViewAggKind::Min, 5, "b"));
    definition
}

#[test]
fn test_build_min_max_has_removed_gate_and_nullable_dependency_order() {
    let result = BuildMView(&min_max_definition(), MViewBuildOptions::default()).unwrap();
    assert_eq!(result.agg_infos[3].dependencies, [3, 4, 5, 6, 13]);
    assert_eq!(result.agg_infos[4].dependencies, [7, 8, 9, 10, 13]);
    assert_eq!(result.full_update_lookup_mv_offsets, [0, 4, 5]);
}

#[test]
fn test_build_min_max_nullable_without_count_expr() {
    let mut definition = min_max_definition();
    definition
        .aggregates
        .retain(|aggregate| aggregate.kind != MViewAggKind::Count);
    definition.base_columns[1].not_null = true;
    definition.mv_columns.remove(2);
    for aggregate in &mut definition.aggregates {
        if aggregate.mv_offset > 2 {
            aggregate.mv_offset -= 1;
        }
    }
    let result = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    let max = result
        .agg_infos
        .iter()
        .find(|info| info.kind == MViewAggKind::Max)
        .unwrap();
    assert_eq!(max.dependencies.len(), 4);
}

#[test]
fn test_build_sum_nullable_with_duplicate_count_expr_uses_first() {
    let mut definition = base_definition();
    definition.mv_columns.insert(3, column("cnt_b_2", true));
    definition
        .aggregates
        .insert(2, aggregate(MViewAggKind::Count, 3, " ( `b` ) "));
    definition.aggregates[3].mv_offset = 4;
    let result = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    let sum = result
        .agg_infos
        .iter()
        .find(|info| info.kind == MViewAggKind::Sum)
        .unwrap();
    assert_eq!(sum.dependencies[1], result.delta_column_count + 2);
}

#[test]
fn test_build_min_max_nullable_with_duplicate_count_expr_uses_first() {
    let mut definition = min_max_definition();
    definition.mv_columns.insert(3, column("cnt_b_2", true));
    definition
        .aggregates
        .insert(2, aggregate(MViewAggKind::Count, 3, "(b)"));
    for aggregate in definition.aggregates.iter_mut().skip(3) {
        aggregate.mv_offset += 1;
    }
    let result = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    let max = result
        .agg_infos
        .iter()
        .find(|info| info.kind == MViewAggKind::Max)
        .unwrap();
    assert_eq!(
        *max.dependencies.last().unwrap(),
        result.delta_column_count + 2
    );
}

#[test]
fn test_build_sum_without_count_expr_errors_but_not_null_sum_does_not() {
    let mut definition = base_definition();
    definition
        .aggregates
        .retain(|aggregate| aggregate.kind != MViewAggKind::Count);
    assert!(
        BuildMView(&definition, MViewBuildOptions::default())
            .unwrap_err()
            .contains("requires matching COUNT(expr)")
    );
    definition.base_columns[1].not_null = true;
    let result = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    let sum = result
        .agg_infos
        .iter()
        .find(|info| info.kind == MViewAggKind::Sum)
        .unwrap();
    assert_eq!(sum.dependencies.len(), 1);
}

#[test]
fn test_build_missing_count_star_and_old_new_errors() {
    let mut definition = base_definition();
    definition
        .aggregates
        .retain(|aggregate| aggregate.kind != MViewAggKind::CountStar);
    assert!(
        BuildMView(&definition, MViewBuildOptions::default())
            .unwrap_err()
            .contains("requires COUNT(*)")
    );
    let mut definition = base_definition();
    definition
        .mlog_columns
        .retain(|column| column != MLOG_OLD_NEW_COLUMN);
    assert!(
        BuildMView(&definition, MViewBuildOptions::default())
            .unwrap_err()
            .contains(MLOG_OLD_NEW_COLUMN)
    );
}

#[test]
fn test_full_update_lookup_index_hint_uses_all_supporting_indexes() {
    let mut definition = min_max_definition();
    definition.base_indexes.push(MViewIndex {
        name: "idx_a_b".to_owned(),
        columns: vec!["a".to_owned(), "b".to_owned()],
        visible: true,
    });
    definition.base_indexes.push(MViewIndex {
        name: "idx_hidden".to_owned(),
        columns: vec!["a".to_owned()],
        visible: false,
    });
    let result = BuildMView(&definition, MViewBuildOptions::default()).unwrap();
    assert_eq!(result.full_update_supporting_indexes, ["idx_a", "idx_a_b"]);
}

#[test]
fn test_build_refresh_mv_fast_plan_with_min_max_and_snapshot() {
    let plan = BuildRefreshMaterializedViewImplement(
        RefreshImplementMode::Fast,
        &min_max_definition(),
        MViewBuildOptions {
            from_ts: 7,
            to_ts: 19,
        },
        19,
    )
    .unwrap();
    let RefreshImplementPlan::Fast(plan) = plan else {
        panic!("expected fast plan")
    };
    assert!(plan.full_update_inner_source.is_some());
    assert_eq!(
        plan.full_update_snapshot,
        Some(DataReaderSnapshot { ts: 19 })
    );
    assert_eq!(plan.full_update_inner_column_count, 3);
    assert_eq!(plan.full_update_output_mv_offsets, [0, 4, 5]);
}

#[test]
fn test_complete_diff_source_layout_and_nullable_group_key() {
    let mut definition = base_definition();
    definition.mv_columns[0].not_null = false;
    let result = BuildCompleteDiffSource(&definition).unwrap();
    assert_eq!(result.source_column_count, 10);
    assert_eq!(result.op_col_offset, 0);
    assert_eq!(result.current_handle_cols, [1]);
    assert_eq!(result.current_row_offsets, [2, 3, 4, 5]);
    assert_eq!(result.recomputed_row_offsets, [6, 7, 8, 9]);
    assert_eq!(result.group_key_join_operators, ["<=>"]);
    result.ValidateSourceLayout(10).unwrap();
}

#[test]
fn test_complete_diff_common_handle_reuses_old_row_image() {
    let mut definition = base_definition();
    definition.handle_kind = MViewHandleKind::CommonPrimaryKey;
    definition.common_handle_offsets = vec![0, 1];
    let result = BuildCompleteDiffSource(&definition).unwrap();
    assert_eq!(result.source_column_count, 9);
    assert_eq!(result.current_handle_cols, [1, 2]);
}

#[test]
fn test_complete_diff_extra_handle_keeps_rowid_projection() {
    let result = BuildCompleteDiffSource(&base_definition()).unwrap();
    assert_eq!(result.current_handle_cols, [1]);
    assert_eq!(result.current_row_offsets[0], 2);
}

#[test]
fn test_build_refresh_mview_complete_delta_apply_plan() {
    let plan = BuildRefreshMaterializedViewImplement(
        RefreshImplementMode::CompleteDeltaApply,
        &base_definition(),
        MViewBuildOptions::default(),
        0,
    )
    .unwrap();
    let RefreshImplementPlan::CompleteDeltaApply(plan) = plan else {
        panic!("expected complete delta apply")
    };
    assert_eq!(plan.mv_table_id, 3);
    assert_eq!(plan.current_row_input_col_ids, [2, 3, 4, 5]);
    assert!(plan.ExplainInfo().contains("op_offset:0"));
}

#[test]
fn test_mview_physical_plan_explain_info_and_tree() {
    let plan = BuildRefreshMaterializedViewImplement(
        RefreshImplementMode::Fast,
        &base_definition(),
        MViewBuildOptions::default(),
        0,
    )
    .unwrap();
    let RefreshImplementPlan::Fast(plan) = plan else {
        unreachable!()
    };
    assert_eq!(
        plan.ExplainInfo(),
        "agg_deps:[count(*)@1->[0], count(b)@2->[1], sum(b)@3->[2,5]]"
    );
    let node = plan.IntoPlanNode(10);
    assert_eq!(node.kind, PlanKind::MViewDeltaMerge);
    assert_eq!(node.children.len(), 1);
}

#[test]
fn test_derive_full_update_output_offsets_uses_inner_schema_order() {
    let result =
        DeriveMViewFullUpdateOutputMVOffsetsByInnerSchema(&[0, 4, 5], &[0], &[2], 3).unwrap();
    assert_eq!(result, [4, 5, 0]);
}

#[test]
fn test_normalize_mlog_commit_ts_inclusive_lower() {
    let value = (1000_u64 << 18) | 123;
    assert_eq!(NormalizeMLogCommitTSInclusiveLower(value), 999_u64 << 18);
    assert_eq!(NormalizeMLogCommitTSInclusiveLower(123), 123);
}

#[test]
fn test_extract_mlog_commit_ts_filter_bound_and_selectivity() {
    let mut window = MLogCommitTSFilterWindow::default();
    window.AddBound(MLogFilterBound {
        column_on_left: true,
        op: MLogFilterOp::Greater,
        value: 12_u64 << 18,
    });
    window.AddBound(MLogFilterBound {
        column_on_left: false,
        op: MLogFilterOp::GreaterOrEqual,
        value: 18_u64 << 18,
    });
    let estimation = MLogCommitTSEstimation {
        MLogTableID: 2,
        RetainedLowerTSO: 10_u64 << 18,
        RetainedUpperTSO: 20_u64 << 18,
    };
    assert_eq!(
        EstimateMLogCommitTSSelectivity(&estimation, 0, window),
        Some(0.6)
    );
}

#[test]
fn test_estimate_mlog_commit_ts_selectivity_disjoint_lower() {
    let filter = MLogCommitTSFilterWindow {
        upper_tso: 14_u64 << 18,
        has_upper: true,
        ..Default::default()
    };
    let fallback = MLogCommitTSEstimation {
        MLogTableID: 2,
        RetainedLowerTSO: 0,
        RetainedUpperTSO: 20_u64 << 18,
    };
    assert_eq!(
        EstimateMLogCommitTSSelectivity(&fallback, 15_u64 << 18, filter),
        None
    );
    let explicit = MLogCommitTSEstimation {
        RetainedLowerTSO: 15_u64 << 18,
        ..fallback
    };
    assert_eq!(
        EstimateMLogCommitTSSelectivity(&explicit, 0, filter),
        Some(0.0)
    );
}

#[test]
fn test_derive_stats_by_filter_uses_mlog_commit_ts_selectivity() {
    let estimation = MLogCommitTSEstimation {
        MLogTableID: 2,
        RetainedLowerTSO: 10_u64 << 18,
        RetainedUpperTSO: 20_u64 << 18,
    };
    let filter = MLogCommitTSFilterWindow {
        lower_tso: 15_u64 << 18,
        has_lower: true,
        ..Default::default()
    };
    let stats = DeriveMLogStatsByFilter(100.0, 1, &estimation, 0, filter).unwrap();
    assert_eq!(stats.row_count, 40.0);
}

#[test]
fn test_planbuilder_dispatches_mview_refresh() {
    let mut builder = NewCorePlanBuilder(&[]);
    let plan = builder
        .Build(&Statement::RefreshMaterializedView(RefreshRequest {
            current_database: "test".to_owned(),
            database: String::new(),
            view: "mv".to_owned(),
            observe: RefreshObserveType::None,
        }))
        .unwrap();
    assert!(matches!(plan, BuiltPlan::RefreshMaterializedView(_)));
    assert_eq!(builder.visitInfo.len(), 1);
    assert_eq!(builder.visitInfo[0].privilege, Privilege::OperateView);

    let mut observe_builder = NewCorePlanBuilder(&[]);
    observe_builder
        .Build(&Statement::RefreshMaterializedView(RefreshRequest {
            current_database: "test".to_owned(),
            database: String::new(),
            view: "mv".to_owned(),
            observe: RefreshObserveType::DryRun,
        }))
        .unwrap();
    assert_eq!(observe_builder.visitInfo.len(), 2);
    assert_eq!(observe_builder.visitInfo[1].privilege, Privilege::ShowView);
}
