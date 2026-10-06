// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Typed IndexJoin probe candidates and rows-after-access estimation.
use base::{PhysicalPlan, Plan};
use expression::{Column, ExprBox};
use logicalop::LogicalPlan as _;
use std::collections::{HashMap, HashSet};

pub(crate) struct ProbePathResult {
    pub scan: crate::PhysicalIndexScan,
    pub used_cols: usize,
    pub eq_ndv: f64,
    pub last_col_is_range: bool,
    pub last_col_manager: Option<crate::ColWithCmpFuncManager>,
    pub key_offsets: Vec<i32>,
}

fn fix_map(ctx: &dyn base::PlanContext) -> HashMap<u64, String> {
    let value = ctx
        .GetSessionVars()
        .GetSystemVar("tidb_opt_fix_control")
        .unwrap_or_default();
    fixcontrol::ParseToMap(&value)
        .map(|(map, _)| map)
        .unwrap_or_default()
}

pub(crate) fn access_rows_floor(
    ctx: &dyn base::PlanContext,
    stats: Option<&property::StatsInfo>,
    result: Option<&ProbePathResult>,
    join_keys: usize,
) -> f64 {
    let (Some(stats), Some(result)) = (stats, result) else {
        return 0.0;
    };
    if result.eq_ndv <= 0.0 || result.last_col_is_range || result.last_col_manager.is_some() {
        return 0.0;
    }
    let used: HashSet<_> = result
        .key_offsets
        .iter()
        .take(result.used_cols)
        .copied()
        .filter(|key| *key >= 0)
        .collect();
    if used.len() >= join_keys {
        return 0.0;
    }
    let enabled = fixcontrol::GetBoolWithDefault(Some(&fix_map(ctx)), fixcontrol::Fix44855, true);
    ctx.GetSessionVars()
        .RecordRelevantOptFix(fixcontrol::Fix44855);
    if enabled {
        stats.RowCount / result.eq_ndv
    } else {
        0.0
    }
}

pub(crate) fn apply_index_floor(access: f64, index: f64, floor: f64, unique: bool) -> (f64, f64) {
    if !unique && floor > access {
        (
            floor,
            floor * if access > 0.0 { index / access } else { 1.0 },
        )
    } else {
        (access, index)
    }
}

fn ndv_is_close(lhs: f64, rhs: f64) -> bool {
    if lhs == 0.0 || rhs == 0.0 {
        return lhs == rhs;
    }
    let min = lhs.min(rhs);
    let max = lhs.max(rhs);
    let diff = (lhs - rhs).abs();
    max <= 20.0 || (diff < 200.0 && min >= 20.0) || diff / max < 0.2
}

/// Preserve the existing Fix44855 upper bound: single-column statistics,
/// an exact column-set index, then initialized column NDVs as a lower bound.
pub(crate) fn ndv_lower_bound(
    mut columns: Vec<i64>,
    histograms: Option<&statistics::HistColl>,
) -> f64 {
    let Some(histograms) = histograms.filter(|_| !columns.is_empty()) else {
        return -1.0;
    };
    if columns.len() == 1 {
        if let Some(column) = histograms
            .GetCol(columns[0])
            .filter(|col| col.IsStatsInitialized())
        {
            return column.NDV as f64;
        }
    }
    columns.sort_unstable();
    for (id, index_columns) in &histograms.Idx2ColUniqueIDs {
        if index_columns.len() != columns.len() {
            continue;
        }
        let mut index_columns = index_columns.clone();
        index_columns.sort_unstable();
        if index_columns == columns {
            if let Some(index) = histograms
                .GetIdx(*id)
                .filter(|index| index.IsStatsInitialized())
            {
                return index.NDV as f64;
            }
        }
    }
    columns
        .into_iter()
        .filter_map(|id| histograms.GetCol(id))
        .filter(|column| column.IsStatsInitialized())
        .map(|column| column.NDV as f64)
        .fold(-1.0, f64::max)
}

pub(crate) struct ProbeCandidate {
    pub plan: Box<dyn PhysicalPlan>,
    pub result: ProbePathResult,
}

/// Only this datasource boundary supplies original table statistics and the
/// complete candidate set. Unary wrappers retain those same lookup paths.
pub(crate) fn best_probe(
    logical: &dyn logicalop::LogicalPlan,
    join: &crate::PhysicalIndexJoin,
    avg_rows: f64,
) -> Result<Option<ProbeCandidate>, expression::Error> {
    if let Some(source) = logical.as_any().downcast_ref::<logicalop::DataSource>() {
        return source_probe(source, join, avg_rows);
    }
    if let Some(scan) = logical
        .as_any()
        .downcast_ref::<logicalop::LogicalTableScan>()
    {
        if let Some(source) = &scan.Source {
            return source_probe(&source.borrow(), join, avg_rows);
        }
    }
    if let Some(gather) = logical
        .as_any()
        .downcast_ref::<logicalop::TiKVSingleGather>()
    {
        if let Some(source) = &gather.Source {
            return source_probe(&source.borrow(), join, avg_rows);
        }
    }
    let children = logical.Children();
    if children.len() == 1 {
        return best_probe(children[0].as_ref(), join, avg_rows);
    }
    Ok(None)
}

fn source_probe(
    source: &logicalop::DataSource,
    join: &crate::PhysicalIndexJoin,
    avg_rows: f64,
) -> Result<Option<ProbeCandidate>, expression::Error> {
    let Some(ctx) = source.SCtx().cloned() else {
        return Ok(None);
    };
    let inner_keys = &join.BasePhysicalJoin.InnerJoinKeys;
    let outer_keys = &join.BasePhysicalJoin.OuterJoinKeys;
    let bool_type = *expression::types::NewFieldType(expression::mysql::TypeTiny);
    let mut paths = source.PossibleAccessPaths.iter().collect::<Vec<_>>();
    if paths.is_empty() {
        paths.extend(&source.AllPossibleAccessPaths);
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert((path.Index.as_ref().map(|i| i.ID), path.IsTablePath())));
    let mut best: Option<(f64, ProbeCandidate)> = None;
    for path in paths {
        if path.IsIndexJoinUnapplicable() || path.StoreType != kv::StoreType::TiKV {
            continue;
        }
        let primary = source.TableInfo.Indices.iter().find(|index| index.Primary);
        let index = path.Index.as_ref().or_else(|| {
            if source.TableInfo.IsCommonHandle {
                primary
            } else {
                None
            }
        });
        let Some(index) = index else {
            continue;
        };
        let (columns, lengths) =
            planner_util::IndexInfo2PrefixCols(&source.Columns, &source.Schema().Columns, index);
        if columns.is_empty() {
            continue;
        }
        let mut placeholders = source
            .AllConds
            .iter()
            .map(|c| c.CloneExpr())
            .collect::<Vec<_>>();
        // Keep static conditions on dynamic key columns as residual filters:
        // a template value must never intersect them as if it were real data.
        let mut key_filters = Vec::new();
        placeholders.retain(|cond| {
            if expression::ExtractColumns(cond.as_ref())
                .iter()
                .any(|col| inner_keys.iter().any(|key| key.EqualColumn(*col)))
            {
                key_filters.push(cond.CloneExpr());
                false
            } else {
                true
            }
        });
        let mut template_conditions = Vec::new();
        let mut original_join_conditions = Vec::new();
        let mut mapping = Vec::new();
        for column in &columns {
            let key = inner_keys.iter().position(|key| key.EqualColumn(column));
            mapping.push(key.map_or(-1, |key| key as i32));
            if let Some(key) = key {
                // The ranger builds a template; the physical access expression
                // keeps its correlated outer value for every real probe.
                let template = expression::NewFunction(
                    ctx.GetExprCtx(),
                    parser_ast::EQ,
                    bool_type.clone(),
                    vec![
                        Box::new(column.Clone()),
                        Box::new(expression::NewInt64Const(0)),
                    ],
                )?;
                placeholders.push(template.CloneExpr());
                template_conditions.push(template);
                original_join_conditions.push((
                    key,
                    expression::NewFunction(
                        ctx.GetExprCtx(),
                        parser_ast::EQ,
                        bool_type.clone(),
                        vec![
                            Box::new(column.Clone()),
                            Box::new(expression::CorrelatedColumn {
                                column: outer_keys[key].Clone(),
                                data: None,
                            }),
                        ],
                    )?,
                ));
            }
        }
        let mut detached = ranger::DetachCondAndBuildRangeForIndex(
            ctx.GetRangerCtx(),
            placeholders,
            columns.iter().map(Column::Clone).collect(),
            lengths.iter().map(|l| *l as i32).collect(),
            ctx.GetSessionVars().RangeMaxSize,
        )
        .map_err(|e| expression::errors::New(e.to_string()))?;
        let mut used = detached
            .Ranges
            .0
            .first()
            .map_or(0, |range| range.LowVal.len());
        if used == 0 {
            continue;
        }
        for key in mapping.iter_mut().skip(used) {
            *key = -1;
        }
        if !mapping.iter().any(|key| *key >= 0) {
            continue;
        }
        let mut last_range = used > detached.EqOrInCount;
        let mut manager = None;
        if !last_range && used < columns.len() {
            let target = &columns[used];
            let mut candidate =
                crate::ColWithCmpFuncManager::New(Some(target.Clone()), lengths[used] as i32);
            for cond in &join.BasePhysicalJoin.OtherConditions {
                let Some(function) = cond.as_any().downcast_ref::<expression::ScalarFunction>()
                else {
                    continue;
                };
                let args = function.GetArgs();
                if args.len() != 2 {
                    continue;
                }
                let op = function.FuncName.L.as_str();
                if !matches!(op, "lt" | "le" | "gt" | "ge") {
                    continue;
                }
                let target_left = args[0]
                    .as_any()
                    .downcast_ref::<Column>()
                    .is_some_and(|col| target.EqualColumn(col));
                let target_right = args[1]
                    .as_any()
                    .downcast_ref::<Column>()
                    .is_some_and(|col| target.EqualColumn(col));
                let (arg, op) = if target_left {
                    (&args[1], op)
                } else if target_right {
                    (
                        &args[0],
                        match op {
                            "lt" => "gt",
                            "le" => "ge",
                            "gt" => "lt",
                            _ => "le",
                        },
                    )
                } else {
                    continue;
                };
                let affected = expression::ExtractColumns(arg.as_ref());
                if affected.is_empty() || affected.iter().any(|col| source.Schema().Contains(col)) {
                    continue;
                }
                candidate.AppendNewExpr(
                    op.to_owned(),
                    arg.CloneExpr(),
                    &affected.into_iter().cloned().collect::<Vec<_>>(),
                );
            }
            if !candidate.OpType.is_empty() {
                let (ranges, fallback) = ranger::AppendRanges2PointRanges(
                    detached.Ranges.clone(),
                    ranger::Ranges(vec![ranger::Range {
                        LowVal: vec![expression::types::Datum::default()],
                        HighVal: vec![expression::types::Datum::default()],
                        Collators: vec![expression::collate::GetCollator("binary")],
                        ..Default::default()
                    }]),
                    ctx.GetSessionVars().RangeMaxSize,
                );
                if !fallback {
                    detached.Ranges = ranges;
                    used += 1;
                    last_range = true;
                    manager = Some(candidate);
                }
            }
        }
        let eq_len = used - usize::from(last_range);
        let ndv = if source.TableStats.StatsVersion != statistics::PseudoVersion {
            cardinality::EstimateColsNDVWithMatchedLen(
                Some(&super::base_physical_plan::ScanCardinalityContext(
                    ctx.as_ref(),
                )),
                &columns[..eq_len],
                source.Schema(),
                &source.TableStats,
            )
            .0
        } else {
            0.0
        };
        let mut scan =
            crate::PhysicalIndexScan::New(ctx.clone()).Init(ctx.clone(), source.QueryBlockOffset());
        scan.PhysicalSchemaProducer
            .SetSchema(source.Schema().Clone());
        scan.Table = Some(source.TableInfo.Clone());
        scan.Index = Some(index.Clone());
        scan.IdxCols = columns;
        scan.IdxColLens = lengths.iter().map(|l| *l as i32).collect();
        scan.Columns = source.Columns.clone();
        scan.PhysicalTableID = source.PhysicalTableID;
        scan.DBName = source.DBName.O.clone();
        scan.TableAsName = source
            .TableAsName
            .as_ref()
            .unwrap_or(&source.TableInfo.Name)
            .O
            .clone();
        scan.TblColHists = source.TableStats.HistColl.clone();
        scan.Ranges = detached.Ranges;
        let is_template = |cond: &dyn expression::Expression| {
            template_conditions
                .iter()
                .any(|template| template.Equal(ctx.GetExprCtx().GetEvalCtx(), cond))
        };
        scan.AccessCondition = detached
            .AccessConds
            .into_iter()
            .filter(|cond| !is_template(cond.as_ref()))
            .collect();
        for range in &mut scan.Ranges.0 {
            for (offset, key) in mapping.iter().take(used).enumerate() {
                if *key >= 0 {
                    range.LowVal[offset] = expression::types::Datum::default();
                    range.HighVal[offset] = expression::types::Datum::default();
                }
            }
        }
        for (key, cond) in original_join_conditions {
            if mapping
                .iter()
                .take(used)
                .any(|mapped| *mapped == key as i32)
            {
                scan.AccessCondition.push(cond);
            }
        }
        scan.RangeInfo = format!(
            "[{}]",
            scan.AccessCondition
                .iter()
                .map(|c| c.StringWithCtx(
                    Some(ctx.GetExprCtx().GetEvalCtx()),
                    expression::errors::RedactLogDisable
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
        // Ranger residual predicates are real datasource filters, never the
        // unusable equality keys, which remain evaluated at the join.
        scan.FilterCondition = detached
            .RemainedConds
            .into_iter()
            .filter(|cond| !is_template(cond.as_ref()))
            .collect();
        scan.FilterCondition.extend(key_filters);
        let result = ProbePathResult {
            scan,
            used_cols: used,
            eq_ndv: ndv,
            last_col_is_range: last_range,
            last_col_manager: manager,
            key_offsets: mapping,
        };
        let unique = index.Unique
            && !last_range
            && used == index.Columns.len()
            && detached.EqCondCount == used;
        let floor = access_rows_floor(
            ctx.as_ref(),
            Some(&source.TableStats),
            Some(&result),
            inner_keys.len(),
        );
        // Preserve the independently default-OFF NDV upper bound. The new
        // default-ON floor is applied after residual filter restoration.
        let upper = if !index.Primary {
            ctx.GetSessionVars()
                .RecordRelevantOptFix(fixcontrol::Fix44855);
            if fixcontrol::GetBoolWithDefault(
                Some(&fix_map(ctx.as_ref())),
                fixcontrol::Fix44855,
                false,
            ) {
                let columns = result
                    .key_offsets
                    .iter()
                    .enumerate()
                    .filter(|(offset, key)| **key >= 0 && lengths[*offset] < 0)
                    .map(|(offset, _)| result.scan.IdxCols[offset].UniqueID)
                    .collect();
                ndv_lower_bound(
                    columns,
                    source
                        .TableStats
                        .HistColl
                        .as_deref()
                        .and_then(|histograms| histograms.downcast_ref::<statistics::HistColl>()),
                )
            } else {
                0.0
            }
        } else {
            0.0
        };
        let upper_rows = if upper > 0.0 {
            source.TableStats.RowCount / upper
        } else {
            f64::INFINITY
        };
        let output_rows = (if index.Primary && avg_rows <= 0.0 {
            1.0
        } else {
            avg_rows
        })
        .min(upper_rows)
        .min(if unique { 1.0 } else { f64::INFINITY });
        let mut access = output_rows;
        let mut after_index = output_rows;
        let mut index_filters = Vec::new();
        let mut table_filters = Vec::new();
        for cond in &result.scan.FilterCondition {
            if expression::ExtractColumns(cond.as_ref()).iter().all(|col| {
                result
                    .scan
                    .IdxCols
                    .iter()
                    .any(|indexed| indexed.EqualColumn(*col))
            }) {
                index_filters.push(cond.CloneExpr());
            } else {
                table_filters.push(cond.CloneExpr());
            }
        }
        let selectivity = |filters: &[ExprBox]| {
            source
                .TableStats
                .HistColl
                .as_deref()
                .and_then(|h| h.downcast_ref::<statistics::HistColl>())
                .and_then(|h| {
                    cardinality::Selectivity(
                        &super::base_physical_plan::ScanCardinalityContext(ctx.as_ref()),
                        h,
                        filters,
                        &[],
                    )
                    .ok()
                })
                .filter(|s| *s > 0.0)
                .unwrap_or(0.8)
        };
        if !table_filters.is_empty() {
            after_index = (after_index / selectivity(&table_filters))
                .min(upper_rows)
                .min(if unique { 1.0 } else { f64::INFINITY });
            access = after_index;
        }
        if !index_filters.is_empty() {
            access = (access / selectivity(&index_filters))
                .min(upper_rows)
                .min(if unique { 1.0 } else { f64::INFINITY });
        }
        if unique {
            access = access.min(1.0);
            after_index = after_index.min(1.0);
        }
        let primary_selectivity = if result.scan.FilterCondition.is_empty() {
            1.0
        } else {
            selectivity(&result.scan.FilterCondition)
        };
        let (access, after_index) = if index.Primary {
            let access = (output_rows / primary_selectivity)
                .max(floor)
                .min(if unique { 1.0 } else { f64::INFINITY });
            (access, access * primary_selectivity)
        } else {
            apply_index_floor(access, after_index, floor, unique)
        };
        let mut result = result;
        let mut scan_stats = source.TableStats.clone();
        scan_stats.RowCount = access;
        let single_scan = source.IsSingleScan(&result.scan.IdxCols, &lengths);
        result.scan.DataSourceSchema = Some(source.Schema().Clone());
        if !index.Primary {
            let mut index_columns = path
                .FullIdxCols
                .iter()
                .map(|col| col.as_ref().map(Column::Clone))
                .collect::<Vec<_>>();
            index_columns.extend(source.CommonHandleCols.iter().map(|col| Some(col.Clone())));
            result.scan.InitSchema(&index_columns, !single_scan);
        }
        result
            .scan
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(scan_stats.clone());
        let mut table_scan =
            crate::PhysicalTableScan::New(ctx.clone()).Init(ctx.clone(), source.QueryBlockOffset());
        table_scan
            .PhysicalSchemaProducer
            .SetSchema(source.Schema().Clone());
        table_scan.Table = Some(source.TableInfo.Clone());
        table_scan.Columns = source.Columns.clone();
        table_scan.IsCommonHandle = source.TableInfo.IsCommonHandle;
        table_scan.TblColHists = result.scan.TblColHists.clone();
        table_scan.DBName = source.DBName.O.clone();
        table_scan.TableAsName = result.scan.TableAsName.clone();
        table_scan.PhysicalTableID = source.PhysicalTableID;
        table_scan.Ranges = result.scan.Ranges.clone();
        table_scan.RangeInfo = result.scan.RangeInfo.clone();
        table_scan.AccessCondition = result
            .scan
            .AccessCondition
            .iter()
            .map(|c| c.CloneExpr())
            .collect();
        let mut table_stats = source.TableStats.clone();
        table_stats.RowCount = if index.Primary { access } else { after_index };
        table_scan
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(table_stats);
        let wrap_filters = |child: Box<dyn PhysicalPlan>,
                            filters: Vec<ExprBox>,
                            rows: f64|
         -> Box<dyn PhysicalPlan> {
            if filters.is_empty() {
                return child;
            }
            let mut stats = source.TableStats.clone();
            stats.RowCount = rows;
            let mut selection = crate::PhysicalSelection::New(ctx.clone()).Init(
                ctx.clone(),
                stats,
                source.QueryBlockOffset(),
                Vec::new(),
            );
            selection
                .PhysicalSchemaProducer
                .SetSchema(child.schema().Clone());
            selection.Conditions = filters;
            selection.FromDataSource = true;
            selection.set_children(vec![child]);
            Box::new(selection)
        };
        let mut plan: Box<dyn PhysicalPlan> = if index.Primary {
            let mut reader = crate::PhysicalTableReader::New(ctx.clone())
                .Init(ctx.clone(), source.QueryBlockOffset());
            reader
                .PhysicalSchemaProducer
                .SetSchema(source.Schema().Clone());
            reader.TablePlan = Some(wrap_filters(
                Box::new(table_scan),
                result
                    .scan
                    .FilterCondition
                    .iter()
                    .map(|c| c.CloneExpr())
                    .collect(),
                after_index,
            ));
            let mut reader_stats = scan_stats.clone();
            reader_stats.RowCount = after_index;
            reader
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(reader_stats);
            Box::new(reader)
        } else if single_scan {
            let mut reader = crate::PhysicalIndexReader::New(ctx.clone())
                .Init(ctx.clone(), source.QueryBlockOffset());
            reader
                .PhysicalSchemaProducer
                .SetSchema(source.Schema().Clone());
            reader.IndexPlan = Some(wrap_filters(
                Box::new(result.scan.Clone(ctx.clone())?),
                index_filters,
                after_index,
            ));
            reader.PhysicalSchemaProducer.BasePhysicalPlan.set_stats({
                let mut stats = scan_stats.clone();
                stats.RowCount = output_rows;
                stats
            });
            Box::new(reader)
        } else {
            let mut reader = crate::PhysicalIndexLookUpReader::New(ctx.clone());
            reader
                .PhysicalSchemaProducer
                .SetSchema(source.Schema().Clone());
            reader.IndexPlan = Some(wrap_filters(
                Box::new(result.scan.Clone(ctx.clone())?),
                index_filters,
                after_index,
            ));
            reader.TablePlan = Some(wrap_filters(
                Box::new(table_scan),
                if index.Primary {
                    result
                        .scan
                        .FilterCondition
                        .iter()
                        .map(|c| c.CloneExpr())
                        .collect()
                } else {
                    table_filters
                },
                output_rows,
            ));
            reader.PhysicalSchemaProducer.BasePhysicalPlan.set_stats({
                let mut stats = scan_stats.clone();
                stats.RowCount = output_rows;
                stats
            });
            Box::new(reader)
        };
        let cost = plan
            .get_plan_cost_ver2(
                property::RootTaskType,
                &costusage::new_default_plan_cost_option(),
                &[true],
            )?
            .get_cost();
        let current_is_better = best.as_ref().is_none_or(|(old_cost, old_candidate)| {
            let current_key_count = result
                .key_offsets
                .iter()
                .filter(|offset| **offset >= 0)
                .count();
            let old_key_count = old_candidate
                .result
                .key_offsets
                .iter()
                .filter(|offset| **offset >= 0)
                .count();
            if current_key_count == old_key_count
                && !ndv_is_close(result.eq_ndv, old_candidate.result.eq_ndv)
            {
                result.eq_ndv > old_candidate.result.eq_ndv
            } else {
                cost < *old_cost
            }
        });
        if current_is_better {
            best = Some((cost, ProbeCandidate { plan, result }));
        }
    }
    Ok(best.map(|(_, best)| best))
}
