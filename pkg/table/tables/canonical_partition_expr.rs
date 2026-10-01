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

//! Go partition.go expression construction over the complete catalog model.
use crate::partition_expr::*;
use expression::{BuildContext, ExprBox, Expression};
use model_dependency as model;
use std::{collections::BTreeMap, sync::Arc};
fn error(e: impl ToString) -> String {
    e.to_string()
}
fn quote(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}
pub fn context() -> exprstatic_dependency::ExprContext {
    let flags = expression::types::StrictFlags
        .WithIgnoreTruncateErr(true)
        .WithIgnoreZeroDateErr(true)
        .WithIgnoreZeroInDate(true)
        .WithIgnoreInvalidDateErr(true);
    let mut levels = [expression::errctx::Level::LevelError; expression::errctx::errGroupCount];
    levels[expression::errctx::ErrGroup::ErrGroupTruncate as usize] =
        expression::errctx::Level::LevelIgnore;
    let eval = exprstatic_dependency::NewEvalContext(vec![
        exprstatic_dependency::WithSQLMode(expression::mysql::ModeAllowInvalidDates),
        exprstatic_dependency::WithTypeFlags(flags),
        exprstatic_dependency::WithErrLevelMap(levels),
    ]);
    exprstatic_dependency::NewExprContext(vec![
        exprstatic_dependency::WithEvalCtx(Arc::new(eval)),
        exprstatic_dependency::WithNewCollationEnabled(collate_dependency::NewCollationEnabled()),
    ])
}
pub fn build(
    table: &model::TableInfo,
    tp: model::ast::PartitionType,
    text: &str,
    names: &[model::ast::CIStr],
    defs: &[model::PartitionDefinition],
) -> Result<Option<PartitionExpr>, String> {
    use model::ast::model::*;
    if tp == PartitionTypeNone {
        return Ok(None);
    }
    let ctx = context();
    let (columns, field_names) = expression::ColumnInfos2ColumnsAndNamesWithCollate(
        &ctx,
        model::ast::NewCIStr(""),
        table.Name.clone(),
        &table.Columns,
        table,
        ctx.NewCollationEnabled(),
    )
    .map_err(error)?;
    let schema = expression::NewSchema(columns.clone());
    let parse = |s: &str| {
        expression::ParseSimpleExpr(
            &ctx,
            s,
            vec![expression::WithInputSchemaAndNames(
                &schema,
                field_names.Shallow(),
                Some(table),
            )],
        )
        .map_err(error)
    };
    let mut ret = PartitionExpr::default();
    let part_columns = if names.is_empty() {
        let expr = parse(text)?;
        let extracted = expression::ExtractColumns(expr.as_ref())
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        ret.ColumnOffset = extracted
            .iter()
            .map(|c| {
                columns
                    .iter()
                    .position(|v| v.UniqueID == c.UniqueID)
                    .ok_or_else(|| format!("unknown partition column {}", c.OrigName))
            })
            .collect::<Result<_, _>>()?;
        ret.Expr = Some(expr);
        extracted
    } else {
        let mut result = Vec::new();
        for n in names {
            let i = table
                .Columns
                .iter()
                .position(|c| c.Name.L == n.L)
                .ok_or_else(|| format!("[table:1054]Unknown column '{}'", n.O))?;
            ret.ColumnOffset.push(i);
            if !result
                .iter()
                .any(|c: &expression::Column| c.UniqueID == columns[i].UniqueID)
            {
                result.push(columns[i].clone());
            }
        }
        result
    };
    match tp {
        PartitionTypeHash => {
            ret.OrigExpr = Some(generatedexpr::ParseExpression(text).map_err(error)?);
            ret.Expr
                .as_ref()
                .ok_or("HASH partition expression is missing")?
                .HashCode();
        }
        PartitionTypeKey => {
            ret.ForKeyPruning = Some(ForKeyPruning {
                KeyPartCols: part_columns,
                UseNewCollate: ctx.NewCollationEnabled(),
            });
        }
        PartitionTypeRange => {
            let parts = if names.is_empty() {
                vec![text.to_owned()]
            } else {
                names.iter().map(|n| quote(&n.O)).collect()
            };
            for def in defs {
                let first = def
                    .LessThan
                    .first()
                    .ok_or("missing RANGE partition bound")?;
                let locate = if first.eq_ignore_ascii_case("MAXVALUE") {
                    "true".to_owned()
                } else {
                    if def.LessThan.len() != parts.len() {
                        return Err("RANGE column count mismatch".into());
                    }
                    match def
                        .LessThan
                        .iter()
                        .position(|v| v.eq_ignore_ascii_case("MAXVALUE"))
                    {
                        Some(i) => format!(
                            "(({}) <= ({}))",
                            parts[..i].join(","),
                            def.LessThan[..i].join(",")
                        ),
                        None => format!("(({}) < ({}))", parts.join(","), def.LessThan.join(",")),
                    }
                };
                ret.UpperBounds.push(parse(&locate)?);
            }
            if names.is_empty() {
                let mut prune = ForRangePruning::default();
                for def in defs {
                    let bound = &def.LessThan[0];
                    let value = if bound.eq_ignore_ascii_case("MAXVALUE") {
                        prune.MaxValue = true;
                        0
                    } else {
                        match bound.parse::<i64>() {
                            Ok(v) => v,
                            Err(e) => {
                                if matches!(
                                    e.kind(),
                                    std::num::IntErrorKind::PosOverflow
                                        | std::num::IntErrorKind::NegOverflow
                                ) {
                                    if let Ok(v) = bound.parse::<u64>() {
                                        prune.Unsigned = true;
                                        v as i64
                                    } else {
                                        let (v, null) = parse(bound)?
                                            .EvalInt(
                                                ctx.GetEvalCtx(),
                                                expression::chunk::Row::default(),
                                            )
                                            .map_err(error)?;
                                        if null {
                                            return Err(error(e));
                                        }
                                        v
                                    }
                                } else {
                                    let (v, null) = parse(bound)?
                                        .EvalInt(
                                            ctx.GetEvalCtx(),
                                            expression::chunk::Row::default(),
                                        )
                                        .map_err(error)?;
                                    if null {
                                        return Err(error(e));
                                    }
                                    v
                                }
                            }
                        }
                    };
                    prune.LessThan.push(value);
                }
                ret.ForRangePruning = Some(prune);
            } else {
                let mut prune = ForRangeColumnsPruning::default();
                for def in defs {
                    let mut bounds = Vec::new();
                    for (i, bound) in def.LessThan.iter().enumerate() {
                        if bound.eq_ignore_ascii_case("MAXVALUE") {
                            bounds.push(None);
                            break;
                        }
                        let mut expr = parse(bound)?;
                        if !expr.as_any().is::<expression::Constant>() {
                            return Err(
                                "[ddl:1654]Partition column values of incorrect type".into()
                            );
                        }
                        let tp = columns[ret.ColumnOffset[i]]
                            .RetType
                            .as_ref()
                            .ok_or("missing partition column type")?;
                        if matches!(
                            tp.GetType(),
                            expression::mysql::TypeDatetime | expression::mysql::TypeDate
                        ) {
                            expr = expression::formal_registry::BuildCastFunction(&ctx, &expr, tp);
                        }
                        bounds.push(Some(expr));
                    }
                    prune.LessThan.push(bounds);
                }
                ret.ForRangeColumnsPruning = Some(prune);
            }
        }
        PartitionTypeList => {
            let mut prune = ForListPruning {
                NullPartitionIdx: -1,
                DefaultPartitionIdx: -1,
                ..Default::default()
            };
            if names.is_empty() {
                let expr = ret.Expr.as_ref().ok_or("missing LIST expression")?;
                prune.LocateExpr = Some(expr.clone());
                prune.PruneExpr = Some(expr.clone());
                prune.PruneExprCols = part_columns;
                // Clone and reindex the pruning expression against its compact column row.
                if let Some(e) = prune.PruneExpr.as_mut() {
                    reindex(e, &prune.PruneExprCols)?;
                }
                let unsigned =
                    expression::mysql::HasUnsignedFlag(expr.GetType(ctx.GetEvalCtx()).GetFlag());
                let mut map = BTreeMap::new();
                for (p, def) in defs.iter().enumerate() {
                    for values in &def.InValues {
                        let text = values.first().ok_or("missing LIST value")?;
                        if text.eq_ignore_ascii_case("DEFAULT") {
                            prune.DefaultPartitionIdx = p as isize;
                            continue;
                        }
                        let (v, null) = parse(text)?
                            .EvalInt(ctx.GetEvalCtx(), expression::chunk::Row::default())
                            .map_err(error)?;
                        if null {
                            prune.NullPartitionIdx = p as isize;
                        } else {
                            map.insert(
                                if unsigned {
                                    v as u64
                                } else {
                                    expression::codec::EncodeIntToCmpUint(v)
                                },
                                p,
                            );
                        }
                    }
                }
                prune.ValueToPartitionIdx = Arc::new(map);
            } else {
                for (i, name) in names.iter().enumerate() {
                    let position = ret.ColumnOffset[i];
                    let column = columns[position].clone();
                    let tp = column.RetType.clone().ok_or("missing LIST column type")?;
                    let mut cp = ForListColumnPruning {
                        ExprCol: Some(column),
                        ValueType: Some(tp),
                        UseNewCollate: ctx.NewCollationEnabled(),
                        ..Default::default()
                    };
                    let mut map: BTreeMap<String, ListPartitionLocation> = BTreeMap::new();
                    for (p, def) in defs.iter().enumerate() {
                        for (g, values) in def.InValues.iter().enumerate() {
                            if values.len() == 1 && values[0] == "DEFAULT" {
                                cp.DefaultPartID = def.ID;
                                prune.DefaultPartitionIdx = p as isize;
                                break;
                            }
                            let value = parse(values.get(i).ok_or_else(|| {
                                format!("LIST column '{}' value missing", name.O)
                            })?)?
                            .Eval(ctx.GetEvalCtx(), expression::chunk::Row::default())
                            .map_err(error)?;
                            let key = cp
                                .GenKey(
                                    ctx.GetEvalCtx().TypeCtx(),
                                    &ctx.GetEvalCtx().ErrCtx(),
                                    value,
                                )
                                .map_err(error)?;
                            let key: String = key.into_iter().map(char::from).collect();
                            let location = map.entry(key).or_default();
                            if let Some(group) = location.0.iter_mut().find(|v| v.PartIdx == p) {
                                group.GroupIdxs.push(g);
                            } else {
                                location.0.push(ListPartitionGroup {
                                    PartIdx: p,
                                    GroupIdxs: vec![g],
                                });
                            }
                        }
                    }
                    cp.Sorted = Arc::new(map.clone());
                    cp.ValueMap = Arc::new(map);
                    prune.ColPrunes.push(cp);
                }
            }
            ret.ForListPruning = Some(prune);
        }
        _ => return Err(format!("unknown partition type {tp:?}")),
    }
    Ok(Some(ret))
}
fn reindex(expr: &mut ExprBox, cols: &[expression::Column]) -> Result<(), String> {
    if let Some(c) = expr.as_any_mut().downcast_mut::<expression::Column>() {
        c.Index =
            cols.iter()
                .position(|v| v.UniqueID == c.UniqueID)
                .ok_or_else(|| format!("unknown LIST column {}", c.OrigName))? as isize;
    } else if let Some(f) = expr
        .as_any_mut()
        .downcast_mut::<expression::ScalarFunction>()
    {
        for arg in f.GetArgsMut() {
            reindex(arg, cols)?;
        }
    }
    Ok(())
}
