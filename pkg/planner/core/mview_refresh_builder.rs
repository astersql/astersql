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

use std::collections::{HashMap, HashSet};

use crate::mview_refresh_lookup::FindVisibleIndexesWithPrefixCoveringColumns;
use crate::{
    DataReaderSnapshot, DryRunRefreshMaterializedView, MViewAggInfo, MViewAggKind,
    MViewCompleteDeltaApply, MViewDeltaMerge, PlanKind, PlanNode, ProfileRefreshMaterializedView,
    RefreshMaterializedView, SchemaProducer,
};

pub const MLOG_OLD_NEW_COLUMN: &str = "_tidb_old_new";
pub const MLOG_DML_TYPE_COLUMN: &str = "_tidb_dml_type";
pub const MLOG_COMMIT_TS_COLUMN: &str = "_tidb_commit_ts";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RefreshObserveType {
    #[default]
    None,
    DryRun,
    Profile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshRequest {
    pub current_database: String,
    pub database: String,
    pub view: String,
    pub observe: RefreshObserveType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshPlan {
    Refresh(RefreshMaterializedView),
    DryRun(DryRunRefreshMaterializedView),
    Profile(ProfileRefreshMaterializedView),
}

/// Build the public REFRESH plan, including the one-column observe schema.
pub fn BuildRefreshMaterializedView(request: &RefreshRequest) -> Result<RefreshPlan, String> {
    if request.view.is_empty() {
        return Err("REFRESH MATERIALIZED VIEW: missing view name".to_owned());
    }
    let database = if request.database.is_empty() {
        &request.current_database
    } else {
        &request.database
    };
    if database.is_empty() {
        return Err("No database selected".to_owned());
    }
    let statement = RefreshMaterializedView {
        database: database.clone(),
        view: request.view.clone(),
    };
    let observe_schema = || SchemaProducer {
        columns: vec!["refresh steps".to_owned()],
    };
    Ok(match request.observe {
        RefreshObserveType::None => RefreshPlan::Refresh(statement),
        RefreshObserveType::DryRun => RefreshPlan::DryRun(DryRunRefreshMaterializedView {
            statement,
            schema: observe_schema(),
        }),
        RefreshObserveType::Profile => RefreshPlan::Profile(ProfileRefreshMaterializedView {
            statement,
            schema: observe_schema(),
        }),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MViewColumn {
    pub name: String,
    pub not_null: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MViewHandleKind {
    IntPrimaryKey {
        column_offset: usize,
    },
    CommonPrimaryKey,
    #[default]
    ExtraRowId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MViewAggregate {
    pub kind: MViewAggKind,
    pub mv_offset: usize,
    pub argument: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MViewDefinition {
    pub database: String,
    pub mv_table_id: i64,
    pub base_table_id: i64,
    pub mlog_table_id: i64,
    pub mv_columns: Vec<MViewColumn>,
    pub base_columns: Vec<MViewColumn>,
    pub mlog_columns: Vec<String>,
    pub group_key_mv_offsets: Vec<usize>,
    pub group_key_base_columns: Vec<String>,
    pub aggregates: Vec<MViewAggregate>,
    pub handle_kind: MViewHandleKind,
    pub common_handle_offsets: Vec<usize>,
    pub base_indexes: Vec<crate::mview_refresh_lookup::MViewIndex>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MViewBuildOptions {
    pub from_ts: u64,
    pub to_ts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MViewBuildResult {
    pub source_columns: Vec<String>,
    pub source_column_count: usize,
    pub full_update_lookup_column_count: usize,
    pub full_update_lookup_mv_offsets: Vec<usize>,
    pub mv_table_id: i64,
    pub base_table_id: i64,
    pub mlog_table_id: i64,
    pub mv_column_count: usize,
    pub delta_column_count: usize,
    pub mv_table_pk_cols: Vec<usize>,
    pub group_key_mv_offsets: Vec<usize>,
    pub group_key_base_columns: Vec<String>,
    pub count_star_mv_offset: usize,
    pub agg_infos: Vec<MViewAggInfo>,
    pub commit_ts_predicates: Vec<String>,
    pub read_from_tiflash: bool,
    pub merge_join_operators: Vec<&'static str>,
    pub full_update_supporting_indexes: Vec<String>,
}

fn normalized_expression(expression: &str) -> String {
    expression
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '`')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn validate_definition(definition: &MViewDefinition) -> Result<(), String> {
    if definition.mv_columns.is_empty() {
        return Err("materialized view has no columns".to_owned());
    }
    if definition.group_key_mv_offsets.is_empty() {
        return Err("materialized view has empty group key offsets".to_owned());
    }
    if definition.group_key_mv_offsets.len() != definition.group_key_base_columns.len() {
        return Err("materialized view group-key metadata length mismatch".to_owned());
    }
    if definition
        .group_key_mv_offsets
        .iter()
        .any(|offset| *offset >= definition.mv_columns.len())
    {
        return Err("materialized view has invalid group key offset".to_owned());
    }
    let mlog_columns = definition
        .mlog_columns
        .iter()
        .map(|column| column.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    for required in [MLOG_OLD_NEW_COLUMN, MLOG_DML_TYPE_COLUMN] {
        if !mlog_columns.contains(required) {
            return Err(format!(
                "materialized view log missing required column {required}"
            ));
        }
    }
    Ok(())
}

/// Build the two-stage mlog delta source and executor-facing aggregate layout.
pub fn BuildMView(
    definition: &MViewDefinition,
    options: MViewBuildOptions,
) -> Result<MViewBuildResult, String> {
    validate_definition(definition)?;
    let count_star = definition
        .aggregates
        .iter()
        .find(|aggregate| aggregate.kind == MViewAggKind::CountStar)
        .ok_or_else(|| "materialized view requires COUNT(*) in SELECT list".to_owned())?;

    let base_not_null = definition
        .base_columns
        .iter()
        .map(|column| (column.name.to_ascii_lowercase(), column.not_null))
        .collect::<HashMap<_, _>>();

    let mut delta_columns = vec!["__mview_delta_cnt_star".to_owned()];
    let mut delta_offsets = HashMap::from([("__mview_delta_cnt_star".to_owned(), 0usize)]);
    for aggregate in &definition.aggregates {
        let names = match aggregate.kind {
            MViewAggKind::CountStar => Vec::new(),
            MViewAggKind::Count => vec![format!("__mview_delta_cnt_{}", aggregate.mv_offset)],
            MViewAggKind::Sum => vec![format!("__mview_delta_sum_{}", aggregate.mv_offset)],
            MViewAggKind::Max => vec![
                format!("__mview_max_in_added_{}", aggregate.mv_offset),
                format!("__mview_max_cnt_in_added_{}", aggregate.mv_offset),
                format!("__mview_max_in_removed_{}", aggregate.mv_offset),
                format!("__mview_max_cnt_in_removed_{}", aggregate.mv_offset),
            ],
            MViewAggKind::Min => vec![
                format!("__mview_min_in_added_{}", aggregate.mv_offset),
                format!("__mview_min_cnt_in_added_{}", aggregate.mv_offset),
                format!("__mview_min_in_removed_{}", aggregate.mv_offset),
                format!("__mview_min_cnt_in_removed_{}", aggregate.mv_offset),
            ],
        };
        for name in names {
            let offset = delta_columns.len();
            delta_offsets.insert(name.clone(), offset);
            delta_columns.push(name);
        }
    }
    let delta_column_count = delta_columns.len();
    let mv_column_base = delta_column_count;

    let matching_count = |argument: &str| {
        let normalized = normalized_expression(argument);
        definition.aggregates.iter().find(|candidate| {
            candidate.kind == MViewAggKind::Count
                && normalized_expression(&candidate.argument) == normalized
        })
    };

    let mut aggregate_infos = Vec::with_capacity(definition.aggregates.len());
    for aggregate in &definition.aggregates {
        if aggregate.mv_offset >= definition.mv_columns.len() {
            return Err(format!(
                "invalid mv offset {} for aggregate",
                aggregate.mv_offset
            ));
        }
        let argument_key = aggregate.argument.to_ascii_lowercase();
        let arg_not_null = base_not_null.get(&argument_key).copied().unwrap_or(false);
        let mut dependencies = match aggregate.kind {
            MViewAggKind::CountStar => vec![0],
            MViewAggKind::Count => {
                vec![delta_offsets[&format!("__mview_delta_cnt_{}", aggregate.mv_offset)]]
            }
            MViewAggKind::Sum => {
                vec![delta_offsets[&format!("__mview_delta_sum_{}", aggregate.mv_offset)]]
            }
            MViewAggKind::Max => vec![
                delta_offsets[&format!("__mview_max_in_added_{}", aggregate.mv_offset)],
                delta_offsets[&format!("__mview_max_cnt_in_added_{}", aggregate.mv_offset)],
                delta_offsets[&format!("__mview_max_in_removed_{}", aggregate.mv_offset)],
                delta_offsets[&format!("__mview_max_cnt_in_removed_{}", aggregate.mv_offset)],
            ],
            MViewAggKind::Min => vec![
                delta_offsets[&format!("__mview_min_in_added_{}", aggregate.mv_offset)],
                delta_offsets[&format!("__mview_min_cnt_in_added_{}", aggregate.mv_offset)],
                delta_offsets[&format!("__mview_min_in_removed_{}", aggregate.mv_offset)],
                delta_offsets[&format!("__mview_min_cnt_in_removed_{}", aggregate.mv_offset)],
            ],
        };
        match aggregate.kind {
            MViewAggKind::Sum if !arg_not_null => {
                let count = matching_count(&aggregate.argument).ok_or_else(|| {
                    format!(
                        "SUM expression {} at mv offset {} requires matching COUNT(expr) in SELECT list",
                        aggregate.argument, aggregate.mv_offset
                    )
                })?;
                dependencies.push(mv_column_base + count.mv_offset);
            }
            MViewAggKind::Min | MViewAggKind::Max if !arg_not_null => {
                if let Some(count) = matching_count(&aggregate.argument) {
                    dependencies.push(mv_column_base + count.mv_offset);
                }
            }
            _ => {}
        }
        aggregate_infos.push(MViewAggInfo {
            kind: aggregate.kind,
            mv_offset: aggregate.mv_offset,
            arg_col_name: aggregate.argument.clone(),
            arg_not_null,
            dependencies,
        });
    }

    let group_keys = definition
        .group_key_mv_offsets
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let merge_join_operators = definition
        .group_key_mv_offsets
        .iter()
        .map(|offset| {
            if definition.mv_columns[*offset].not_null {
                "="
            } else {
                "<=>"
            }
        })
        .collect::<Vec<_>>();
    let mut source_columns = delta_columns;
    source_columns.extend(
        definition
            .mv_columns
            .iter()
            .map(|column| column.name.clone()),
    );
    let mv_table_pk_cols = match &definition.handle_kind {
        MViewHandleKind::IntPrimaryKey { column_offset } => {
            if *column_offset >= definition.mv_columns.len() {
                return Err("materialized view primary-key offset is invalid".to_owned());
            }
            vec![mv_column_base + column_offset]
        }
        MViewHandleKind::CommonPrimaryKey => {
            if definition.common_handle_offsets.is_empty()
                || definition
                    .common_handle_offsets
                    .iter()
                    .any(|offset| *offset >= definition.mv_columns.len())
            {
                return Err("materialized view common-handle offsets are invalid".to_owned());
            }
            definition
                .common_handle_offsets
                .iter()
                .map(|offset| mv_column_base + offset)
                .collect()
        }
        MViewHandleKind::ExtraRowId => {
            source_columns.push("__mview_mv_rowid".to_owned());
            vec![source_columns.len() - 1]
        }
    };

    let has_min_max = definition
        .aggregates
        .iter()
        .any(|aggregate| matches!(aggregate.kind, MViewAggKind::Min | MViewAggKind::Max));
    let full_update_lookup_mv_offsets = if has_min_max {
        definition
            .mv_columns
            .iter()
            .enumerate()
            .filter_map(|(offset, _)| {
                (group_keys.contains(&offset)
                    || definition.aggregates.iter().any(|aggregate| {
                        aggregate.mv_offset == offset
                            && matches!(aggregate.kind, MViewAggKind::Min | MViewAggKind::Max)
                    }))
                .then_some(offset)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let full_update_supporting_indexes = if has_min_max {
        FindVisibleIndexesWithPrefixCoveringColumns(
            &definition.base_indexes,
            &definition.group_key_base_columns,
        )
    } else {
        Vec::new()
    };

    let mut commit_ts_predicates = vec![format!("{MLOG_COMMIT_TS_COLUMN} > {}", options.from_ts)];
    if options.to_ts > 0 {
        commit_ts_predicates.push(format!("{MLOG_COMMIT_TS_COLUMN} <= {}", options.to_ts));
    }
    Ok(MViewBuildResult {
        source_column_count: source_columns.len(),
        source_columns,
        full_update_lookup_column_count: full_update_lookup_mv_offsets.len(),
        full_update_lookup_mv_offsets,
        mv_table_id: definition.mv_table_id,
        base_table_id: definition.base_table_id,
        mlog_table_id: definition.mlog_table_id,
        mv_column_count: definition.mv_columns.len(),
        delta_column_count,
        mv_table_pk_cols,
        group_key_mv_offsets: definition.group_key_mv_offsets.clone(),
        group_key_base_columns: definition.group_key_base_columns.clone(),
        count_star_mv_offset: count_star.mv_offset,
        agg_infos: aggregate_infos,
        commit_ts_predicates,
        read_from_tiflash: true,
        merge_join_operators,
        full_update_supporting_indexes,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteDiffBuildResult {
    pub source_column_count: usize,
    pub mv_table_id: i64,
    pub mv_column_count: usize,
    pub op_col_offset: usize,
    pub marker_mv_offset: usize,
    pub group_key_mv_offsets: Vec<usize>,
    pub current_handle_cols: Vec<usize>,
    pub current_row_offsets: Vec<usize>,
    pub recomputed_row_offsets: Vec<usize>,
    pub group_key_join_operators: Vec<&'static str>,
    pub payload_comparison_operators: Vec<&'static str>,
}

impl CompleteDiffBuildResult {
    pub fn ValidateSourceLayout(&self, schema_len: usize) -> Result<(), String> {
        if self.mv_column_count == 0 || self.source_column_count == 0 {
            return Err("complete diff build result has invalid column count".to_owned());
        }
        if schema_len > 0 && schema_len != self.source_column_count {
            return Err(format!(
                "complete diff build result: schema length mismatch: got {schema_len}, expected {}",
                self.source_column_count
            ));
        }
        let has_extra_row_id = self.current_row_offsets.first().copied() == Some(2);
        let expected = 1 + self.mv_column_count * 2 + usize::from(has_extra_row_id);
        if self.source_column_count != expected {
            return Err(format!(
                "complete diff build result: unexpected column count: got {}, expected {expected}",
                self.source_column_count
            ));
        }
        if self.marker_mv_offset >= self.mv_column_count
            || self.group_key_mv_offsets.is_empty()
            || self
                .group_key_mv_offsets
                .iter()
                .any(|offset| *offset >= self.mv_column_count)
            || self.current_row_offsets.len() != self.mv_column_count
            || self.recomputed_row_offsets.len() != self.mv_column_count
            || self
                .current_handle_cols
                .iter()
                .any(|offset| *offset >= self.source_column_count)
        {
            return Err("complete diff build result has invalid layout metadata".to_owned());
        }
        Ok(())
    }
}

/// Build the full-outer-join row-image layout for COMPLETE DELTA APPLY.
pub fn BuildCompleteDiffSource(
    definition: &MViewDefinition,
) -> Result<CompleteDiffBuildResult, String> {
    validate_definition(definition)?;
    let marker_mv_offset = definition
        .mv_columns
        .iter()
        .position(|column| column.not_null)
        .ok_or_else(|| {
            "materialized view has no visible NOT NULL column for diff marker".to_owned()
        })?;
    let use_row_id = definition.handle_kind == MViewHandleKind::ExtraRowId;
    let current_start = 1 + usize::from(use_row_id);
    let current_row_offsets =
        (current_start..current_start + definition.mv_columns.len()).collect::<Vec<_>>();
    let recomputed_start = current_start + definition.mv_columns.len();
    let recomputed_row_offsets =
        (recomputed_start..recomputed_start + definition.mv_columns.len()).collect::<Vec<_>>();
    let current_handle_cols = match &definition.handle_kind {
        MViewHandleKind::IntPrimaryKey { column_offset } => {
            if *column_offset >= definition.mv_columns.len() {
                return Err("materialized view primary-key offset is invalid".to_owned());
            }
            vec![current_row_offsets[*column_offset]]
        }
        MViewHandleKind::CommonPrimaryKey => definition
            .common_handle_offsets
            .iter()
            .map(|offset| {
                current_row_offsets
                    .get(*offset)
                    .copied()
                    .ok_or_else(|| "materialized view common-handle offset is invalid".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
        MViewHandleKind::ExtraRowId => vec![1],
    };
    let group_keys = definition
        .group_key_mv_offsets
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let result = CompleteDiffBuildResult {
        source_column_count: recomputed_start + definition.mv_columns.len(),
        mv_table_id: definition.mv_table_id,
        mv_column_count: definition.mv_columns.len(),
        op_col_offset: 0,
        marker_mv_offset,
        group_key_mv_offsets: definition.group_key_mv_offsets.clone(),
        current_handle_cols,
        current_row_offsets,
        recomputed_row_offsets,
        group_key_join_operators: definition
            .group_key_mv_offsets
            .iter()
            .map(|offset| {
                if definition.mv_columns[*offset].not_null {
                    "="
                } else {
                    "<=>"
                }
            })
            .collect(),
        payload_comparison_operators: definition
            .mv_columns
            .iter()
            .enumerate()
            .filter_map(|(offset, column)| {
                (!group_keys.contains(&offset)).then_some(if column.not_null { "=" } else { "<=>" })
            })
            .collect(),
    };
    result.ValidateSourceLayout(0)?;
    Ok(result)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshImplementMode {
    Fast,
    CompleteDeltaApply,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RefreshImplementPlan {
    Fast(MViewDeltaMerge),
    CompleteDeltaApply(MViewCompleteDeltaApply),
}

/// Wire the pure mview build metadata into the two executor-facing physical plans.
pub fn BuildRefreshMaterializedViewImplement(
    mode: RefreshImplementMode,
    definition: &MViewDefinition,
    options: MViewBuildOptions,
    target_refresh_read_ts: u64,
) -> Result<RefreshImplementPlan, String> {
    match mode {
        RefreshImplementMode::Fast => {
            let result = BuildMView(definition, options)?;
            let source = PlanNode::New(
                1,
                PlanKind::Selection {
                    conditions: result.commit_ts_predicates.clone(),
                },
                vec![PlanNode::New(
                    2,
                    PlanKind::TableScan {
                        table: format!("$mlog#{}", result.mlog_table_id),
                    },
                    Vec::new(),
                )],
            );
            let has_min_max = result
                .agg_infos
                .iter()
                .any(|info| matches!(info.kind, MViewAggKind::Min | MViewAggKind::Max));
            if has_min_max && result.full_update_supporting_indexes.is_empty() {
                return Err("refresh materialized view fast with MIN/MAX requires base table index whose leading columns cover all GROUP BY columns".to_owned());
            }
            let full_update_inner_source = has_min_max.then(|| {
                PlanNode::New(
                    3,
                    PlanKind::IndexJoin {
                        keys: definition
                            .group_key_base_columns
                            .iter()
                            .map(|key| (key.clone(), key.clone()))
                            .collect(),
                    },
                    Vec::new(),
                )
            });
            let full_update_inner_column_count = result.full_update_lookup_column_count;
            let full_update_key_off_to_index_off =
                (0..definition.group_key_mv_offsets.len()).collect::<Vec<_>>();
            let full_update_key_result_col_indexes =
                (0..definition.group_key_mv_offsets.len()).collect::<Vec<_>>();
            Ok(RefreshImplementPlan::Fast(MViewDeltaMerge {
                source,
                full_update_inner_source,
                full_update_inner_column_count,
                full_update_index_ranges: has_min_max
                    .then(|| {
                        definition
                            .group_key_base_columns
                            .iter()
                            .map(|key| format!("{key}=?"))
                            .collect::<Vec<_>>()
                    })
                    .into_iter()
                    .collect(),
                full_update_key_off_to_index_off,
                full_update_key_result_col_indexes,
                full_update_output_mv_offsets: result.full_update_lookup_mv_offsets,
                full_update_snapshot: (target_refresh_read_ts > 0).then_some(DataReaderSnapshot {
                    ts: target_refresh_read_ts,
                }),
                mv_table_id: result.mv_table_id,
                base_table_id: result.base_table_id,
                mlog_table_id: result.mlog_table_id,
                mv_column_count: result.mv_column_count,
                delta_column_count: result.delta_column_count,
                mv_table_pk_cols: result.mv_table_pk_cols,
                group_key_mv_offsets: result.group_key_mv_offsets,
                count_star_mv_offset: result.count_star_mv_offset,
                agg_infos: result.agg_infos,
            }))
        }
        RefreshImplementMode::CompleteDeltaApply => {
            let result = BuildCompleteDiffSource(definition)?;
            let source = PlanNode::New(
                1,
                PlanKind::HashJoin {
                    inner_child: 1,
                    equal_conditions: definition
                        .group_key_base_columns
                        .iter()
                        .map(|key| (key.clone(), key.clone()))
                        .collect(),
                },
                Vec::new(),
            );
            Ok(RefreshImplementPlan::CompleteDeltaApply(
                MViewCompleteDeltaApply {
                    source,
                    mv_table_id: result.mv_table_id,
                    mv_column_count: result.mv_column_count,
                    op_col_offset: result.op_col_offset,
                    marker_mv_offset: result.marker_mv_offset,
                    group_key_mv_offsets: result.group_key_mv_offsets,
                    current_handle_cols: result.current_handle_cols,
                    current_row_input_col_ids: result.current_row_offsets,
                    recomputed_row_input_col_ids: result.recomputed_row_offsets,
                },
            ))
        }
    }
}
