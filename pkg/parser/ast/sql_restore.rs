// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

//! Structured restoration for the canonical parser AST.

use crate as parser_ast;
use crate::*;

fn quote(name: &str) -> String {
    crate::dml::quote_name(name)
}

fn restore_column_name(col: &parser_ast::ColumnName) -> String {
    let mut parts = Vec::new();
    if !col.Schema.O.is_empty() {
        parts.push(quote(&col.Schema.O));
    }
    if !col.Table.O.is_empty() {
        parts.push(quote(&col.Table.O));
    }
    parts.push(quote(&col.Name.O));
    parts.join(".")
}

fn restore_value_text(text: &str) -> String {
    if text.eq_ignore_ascii_case("null") || text == "?" || text.parse::<f64>().is_ok() {
        text.to_owned()
    } else {
        format!("_UTF8MB4'{}'", text.replace('\'', "''"))
    }
}

fn time_unit_keyword(unit: parser_ast::TimeUnitType) -> &'static str {
    use parser_ast::TimeUnitType::*;
    match unit {
        Invalid => "",
        Microsecond => "MICROSECOND",
        Second => "SECOND",
        Minute => "MINUTE",
        Hour => "HOUR",
        Day => "DAY",
        Week => "WEEK",
        Month => "MONTH",
        Quarter => "QUARTER",
        Year => "YEAR",
        SecondMicrosecond => "SECOND_MICROSECOND",
        MinuteMicrosecond => "MINUTE_MICROSECOND",
        MinuteSecond => "MINUTE_SECOND",
        HourMicrosecond => "HOUR_MICROSECOND",
        HourSecond => "HOUR_SECOND",
        HourMinute => "HOUR_MINUTE",
        DayMicrosecond => "DAY_MICROSECOND",
        DaySecond => "DAY_SECOND",
        DayMinute => "DAY_MINUTE",
        DayHour => "DAY_HOUR",
        YearMonth => "YEAR_MONTH",
    }
}

pub fn restore_expr(expr: &parser_ast::ExprNode) -> Result<String, String> {
    match &expr.Kind {
        ExprKind::Value(value) => {
            if matches!(&value.Datum, ValueDatum::String(text) if text.is_empty()) {
                let source = expr.Text();
                if !source.is_empty() {
                    return Ok(source);
                }
            }
            Ok(restore_value_text(&value.text()))
        }
        ExprKind::IntroducedValue { Value, Charset, .. } => Ok(format!(
            "_{}{}",
            Charset.to_ascii_uppercase(),
            restore_value_text(Value)
        )),
        ExprKind::Column(col) => Ok(restore_column_name(col)),
        ExprKind::Variable { Name, .. } => Ok(format!("@{}", quote(Name))),
        ExprKind::Binary { Op, L, R } => {
            if Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or") {
                Ok(format!(
                    "{} {} {}",
                    restore_expr(L)?,
                    Op.to_ascii_uppercase(),
                    restore_expr(R)?
                ))
            } else {
                Ok(format!("{}{}{}", restore_expr(L)?, Op, restore_expr(R)?))
            }
        }
        ExprKind::Unary { Op, V } => Ok(format!("{Op}{}", restore_expr(V)?)),
        ExprKind::Parentheses(inner) => Ok(format!("({})", restore_expr(inner)?)),
        ExprKind::IsTruth { Expr, Not, True } => Ok(format!(
            "{} IS{} {}",
            restore_expr(Expr)?,
            if *Not { " NOT" } else { "" },
            if *True { "TRUE" } else { "FALSE" }
        )),
        ExprKind::IsNull { Expr, Not } => Ok(format!(
            "{} IS{} NULL",
            restore_expr(Expr)?,
            if *Not { " NOT" } else { "" }
        )),
        ExprKind::InList {
            Expr, List, Not, ..
        } => Ok(format!(
            "{}{} IN ({})",
            restore_expr(Expr)?,
            if *Not { " NOT" } else { "" },
            List.iter()
                .map(restore_expr)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        ExprKind::Between {
            Expr,
            Left,
            Right,
            Not,
        } => Ok(format!(
            "{}{} BETWEEN {} AND {}",
            restore_expr(Expr)?,
            if *Not { " NOT" } else { "" },
            restore_expr(Left)?,
            restore_expr(Right)?
        )),
        ExprKind::Like {
            Expr,
            Pattern,
            Not,
            Escape,
            Explicit,
            IsLike,
            ..
        } => {
            let mut sql = format!(
                "{}{} {} {}",
                restore_expr(Expr)?,
                if *Not { " NOT" } else { "" },
                if *IsLike { "LIKE" } else { "ILIKE" },
                restore_expr(Pattern)?
            );
            if *Explicit && Escape != "\\" {
                sql.push_str(&format!(" ESCAPE '{}'", Escape.replace('\'', "''")));
            }
            Ok(sql)
        }
        ExprKind::Regexp {
            Expr, Pattern, Not, ..
        } => Ok(format!(
            "{}{} REGEXP {}",
            restore_expr(Expr)?,
            if *Not { " NOT" } else { "" },
            restore_expr(Pattern)?
        )),
        ExprKind::Row(values) => Ok(format!(
            "ROW({})",
            values
                .iter()
                .map(restore_expr)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        ExprKind::Collate { Expr, Collation } => {
            Ok(format!("{} COLLATE {}", restore_expr(Expr)?, Collation))
        }
        ExprKind::NamedDefault(column) => Ok(format!("DEFAULT({})", restore_column_name(column))),
        ExprKind::MaxValue => Ok("MAXVALUE".into()),
        ExprKind::DefaultValue => Ok("DEFAULT".into()),
        ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            let mut sql = "CASE".to_owned();
            if let Some(value) = Value {
                sql.push(' ');
                sql.push_str(&restore_expr(value)?);
            }
            for clause in WhenClauses {
                sql.push_str(" WHEN ");
                sql.push_str(&restore_expr(&clause.Expr)?);
                sql.push_str(" THEN ");
                sql.push_str(&restore_expr(&clause.Result)?);
            }
            if let Some(otherwise) = ElseClause {
                sql.push_str(" ELSE ");
                sql.push_str(&restore_expr(otherwise)?);
            }
            sql.push_str(" END");
            Ok(sql)
        }
        ExprKind::Function {
            Schema,
            FnName,
            Args,
        } => {
            let args = Args
                .iter()
                .map(restore_expr)
                .collect::<Result<Vec<_>, _>>()?
                .join(",");
            let function = FnName.O.to_ascii_uppercase();
            if Schema.O.is_empty() {
                Ok(format!("{function}({args})"))
            } else {
                Ok(format!("{}.{}({args})", quote(&Schema.O), function))
            }
        }
        ExprKind::AggregateFunction {
            Name,
            Args,
            Distinct,
            ..
        } => {
            let args = Args
                .iter()
                .map(restore_expr)
                .collect::<Result<Vec<_>, _>>()?
                .join(",");
            let distinct = if *Distinct { "DISTINCT " } else { "" };
            Ok(format!("{Name}({distinct}{args})"))
        }
        ExprKind::WindowFunction {
            Name, Args, Spec, ..
        } => {
            let args = Args
                .iter()
                .map(restore_expr)
                .collect::<Result<Vec<_>, _>>()?
                .join(",");
            Ok(format!(
                "{Name}({args}) OVER {}",
                restore_window_spec(Spec)?
            ))
        }
        ExprKind::MatchAgainst {
            ColumnNames,
            Against,
            Modifier,
        } => {
            let mut sql = format!(
                "MATCH ({}) AGAINST ({})",
                ColumnNames
                    .iter()
                    .map(restore_column_name)
                    .collect::<Vec<_>>()
                    .join(","),
                restore_expr(Against)?
            );
            let boolean = Modifier & 0x0f == 1;
            let expansion = Modifier & 0x10 != 0;
            if boolean && expansion {
                return Err("BOOLEAN MODE doesn't support QUERY EXPANSION".into());
            }
            if boolean {
                sql.insert_str(sql.len() - 1, " IN BOOLEAN MODE");
            } else if expansion {
                sql.insert_str(sql.len() - 1, " WITH QUERY EXPANSION");
            }
            Ok(sql)
        }
        ExprKind::TimeUnit(unit) => Ok(time_unit_keyword(*unit).to_owned()),
        ExprKind::GetFormatSelector(selector) => Ok(match selector {
            GetFormatSelectorType::Date => "DATE",
            GetFormatSelectorType::Datetime => "DATETIME",
            GetFormatSelectorType::Time => "TIME",
        }
        .to_owned()),
        ExprKind::TrimDirection(direction) => Ok(match direction {
            TrimDirectionType::Both => "BOTH",
            TrimDirectionType::Leading => "LEADING",
            TrimDirectionType::Trailing => "TRAILING",
        }
        .to_owned()),
        ExprKind::TableName(name) => Ok(restore_table_name(name)),
        ExprKind::ParamMarker { .. } => Ok("?".into()),
        ExprKind::Subquery { Query, .. } => {
            let query = Query
                .with_node(restore_node)
                .ok_or("subquery is missing query")??;
            Ok(format!("({query})"))
        }
        ExprKind::CompareSubquery { Op, L, R, All } => Ok(format!(
            "{} {} {} {}",
            restore_expr(L)?,
            Op.to_ascii_uppercase(),
            if *All { "ALL" } else { "ANY" },
            restore_expr(R)?
        )),
        ExprKind::InSubquery { Expr, Sel, Not } => Ok(format!(
            "{}{} IN {}",
            restore_expr(Expr)?,
            if *Not { " NOT" } else { "" },
            restore_expr(Sel)?
        )),
        ExprKind::ExistsSubquery { Sel, Not } => Ok(format!(
            "{}EXISTS {}",
            if *Not { "NOT " } else { "" },
            restore_expr(Sel)?
        )),
        ExprKind::Cast {
            Expr,
            Tp,
            FunctionType,
            ExplicitCharSet,
        } => {
            let value = restore_expr(Expr)?;
            if *FunctionType == CastFunctionType::Binary {
                return Ok(format!("BINARY {value}"));
            }
            let mut encoded = Vec::new();
            Tp.FormatAsCastType(&mut encoded, *ExplicitCharSet)
                .map_err(|e| e.to_string())?;
            let target = String::from_utf8(encoded).map_err(|e| e.to_string())?;
            Ok(match FunctionType {
                CastFunctionType::Cast => format!("CAST({value} AS {target})"),
                CastFunctionType::Convert => format!("CONVERT({value}, {target})"),
                CastFunctionType::Binary => unreachable!(),
            })
        }
        ExprKind::JSONSumCrc32 {
            Expr,
            Tp,
            ExplicitCharSet,
        } => {
            let mut encoded = Vec::new();
            Tp.FormatAsCastType(&mut encoded, *ExplicitCharSet)
                .map_err(|e| e.to_string())?;
            let target = String::from_utf8(encoded).map_err(|e| e.to_string())?;
            Ok(format!(
                "JSON_SUM_CRC32({} AS {target})",
                restore_expr(Expr)?
            ))
        }
    }
}

fn restore_by_item(item: &parser_ast::ByItem) -> Result<String, String> {
    let mut out = restore_expr(&item.Expr)?;
    if item.Desc {
        out.push_str(" DESC");
    }
    Ok(out)
}

fn restore_frame_bound(bound: &parser_ast::FrameBound) -> Result<String, String> {
    let mut out = String::new();
    if bound.UnBounded {
        out.push_str("UNBOUNDED");
    }
    match bound.Type {
        parser_ast::BoundType::CurrentRow => out.push_str("CURRENT ROW"),
        parser_ast::BoundType::Preceding | parser_ast::BoundType::Following => {
            let has_unit = bound.Unit != parser_ast::TimeUnitType::Invalid;
            if has_unit {
                out.push_str("INTERVAL ");
            }
            if let Some(expr) = &bound.Expr {
                out.push_str(&restore_expr(expr)?);
            }
            if has_unit {
                out.push(' ');
                out.push_str(time_unit_keyword(bound.Unit));
            }
            out.push_str(if bound.Type == parser_ast::BoundType::Preceding {
                " PRECEDING"
            } else {
                " FOLLOWING"
            });
        }
    }
    Ok(out)
}

fn restore_frame_clause(frame: &parser_ast::FrameClause) -> Result<String, String> {
    let kind = match frame.Type {
        parser_ast::FrameType::Rows => "ROWS",
        parser_ast::FrameType::Ranges => "RANGE",
        parser_ast::FrameType::Groups => "GROUPS",
    };
    Ok(format!(
        "{kind} BETWEEN {} AND {}",
        restore_frame_bound(&frame.Extent.Start)?,
        restore_frame_bound(&frame.Extent.End)?
    ))
}

fn restore_window_spec(spec: &parser_ast::WindowSpec) -> Result<String, String> {
    let mut out = String::new();
    if !spec.Name.O.is_empty() {
        out.push_str(&quote(&spec.Name.O));
        if spec.OnlyAlias {
            return Ok(out);
        }
        out.push_str(" AS ");
    }
    out.push('(');
    let mut sep = "";
    if !spec.Ref.O.is_empty() {
        out.push_str(&quote(&spec.Ref.O));
        sep = " ";
    }
    if !spec.PartitionBy.is_empty() {
        out.push_str(sep);
        out.push_str("PARTITION BY ");
        out.push_str(
            &spec
                .PartitionBy
                .iter()
                .map(restore_by_item)
                .collect::<Result<Vec<_>, _>>()?
                .join(", "),
        );
        sep = " ";
    }
    if !spec.OrderBy.is_empty() {
        out.push_str(sep);
        out.push_str("ORDER BY ");
        out.push_str(
            &spec
                .OrderBy
                .iter()
                .map(restore_by_item)
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        );
        sep = " ";
    }
    if let Some(frame) = &spec.Frame {
        out.push_str(sep);
        out.push_str(&restore_frame_clause(frame)?);
    }
    out.push(')');
    Ok(out)
}

// ---------------------------------------------------------------------------
// Table names, index hints, joins, and table sources.
// ---------------------------------------------------------------------------

fn restore_index_hint(hint: &parser_ast::IndexHint) -> String {
    let kind = match hint.HintType {
        parser_ast::IndexHintType::Use => "USE INDEX",
        parser_ast::IndexHintType::Ignore => "IGNORE INDEX",
        parser_ast::IndexHintType::Force => "FORCE INDEX",
        parser_ast::IndexHintType::OrderIndex => "ORDER INDEX",
        parser_ast::IndexHintType::NoOrderIndex => "NO ORDER INDEX",
    };
    let scope = match hint.HintScope {
        parser_ast::IndexHintScope::Scan => "",
        parser_ast::IndexHintScope::Join => " FOR JOIN",
        parser_ast::IndexHintScope::OrderBy => " FOR ORDER BY",
        parser_ast::IndexHintScope::GroupBy => " FOR GROUP BY",
    };
    let names = hint
        .IndexNames
        .iter()
        .map(|name| quote(&name.O))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{kind}{scope} ({names})")
}

fn restore_index_hints(hints: &[parser_ast::IndexHint]) -> String {
    hints
        .iter()
        .map(|hint| format!(" {}", restore_index_hint(hint)))
        .collect::<String>()
}

fn restore_table_name_core(table: &parser_ast::TableName) -> String {
    let mut out = if table.Schema.O.is_empty() {
        quote(&table.Name.O)
    } else {
        format!("{}.{}", quote(&table.Schema.O), quote(&table.Name.O))
    };
    if !table.PartitionNames.is_empty() {
        out.push_str(" PARTITION(");
        out.push_str(
            &table
                .PartitionNames
                .iter()
                .map(|name| quote(&name.O))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push(')');
    }
    out
}

fn restore_table_name(table: &parser_ast::TableName) -> String {
    format!(
        "{}{}",
        restore_table_name_core(table),
        restore_index_hints(&table.IndexHints)
    )
}

fn restore_grant_level(level: &parser_ast::GrantLevel) -> String {
    match level.Level {
        parser_ast::GrantLevelType::Global => "*.*".to_owned(),
        parser_ast::GrantLevelType::DB => format!("{}.*", quote(&level.DBName)),
        parser_ast::GrantLevelType::Table => format!(
            "{}.{}",
            if level.DBName.is_empty() {
                "*".to_owned()
            } else {
                quote(&level.DBName)
            },
            quote(&level.TableName)
        ),
    }
}

fn restore_grant_users(users: &[parser_ast::UserSpec]) -> String {
    users
        .iter()
        .map(|user| {
            format!(
                "{}@{}",
                quote(&user.User.username),
                quote(&user.User.hostname)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn restore_node(node: &dyn parser_ast::Node) -> Result<String, String> {
    if let Some(stmt) = node.as_any().downcast_ref::<parser_ast::SetRoleStmt>() {
        let mut sql = "SET ROLE".to_owned();
        sql.push_str(match stmt.SetRoleOpt {
            parser_ast::SetRoleOpt::None => " NONE",
            parser_ast::SetRoleOpt::All => " ALL",
            parser_ast::SetRoleOpt::Default => " DEFAULT",
            parser_ast::SetRoleOpt::AllExcept => " ALL EXCEPT",
            parser_ast::SetRoleOpt::Regular => "",
        });
        if !stmt.RoleList.is_empty() {
            sql.push(' ');
            sql.push_str(
                &stmt
                    .RoleList
                    .iter()
                    .map(|role| format!("{}@{}", quote(&role.username), quote(&role.hostname)))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        return Ok(sql);
    }
    if let Some(stmt) = node.as_any().downcast_ref::<parser_ast::GrantStmt>() {
        if stmt.Privs.len() == 1
            && stmt.Privs[0].Priv == parser_mysql::privs::OperateViewPriv
            && stmt.Privs[0].Cols.is_empty()
            && stmt.AuthTokenOrTLSOptions.is_empty()
            && !stmt.WithGrant
        {
            return Ok(format!(
                "GRANT OPERATE VIEW ON {}{} TO {}",
                if stmt.ObjectType == parser_ast::ObjectTypeType::Table {
                    "TABLE "
                } else {
                    ""
                },
                restore_grant_level(&stmt.Level),
                restore_grant_users(&stmt.Users)
            ));
        }
    }
    if let Some(stmt) = node.as_any().downcast_ref::<parser_ast::RevokeStmt>() {
        if stmt.Privs.len() == 1
            && stmt.Privs[0].Priv == parser_mysql::privs::OperateViewPriv
            && stmt.Privs[0].Cols.is_empty()
        {
            return Ok(format!(
                "REVOKE OPERATE VIEW ON {}{} FROM {}",
                if stmt.ObjectType == parser_ast::ObjectTypeType::Table {
                    "TABLE "
                } else {
                    ""
                },
                restore_grant_level(&stmt.Level),
                restore_grant_users(&stmt.Users)
            ));
        }
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::CreateMaterializedViewStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::CreateMaterializedViewLogStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::AlterMaterializedViewStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::AlterMaterializedViewLogStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::DropMaterializedViewStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::DropMaterializedViewLogStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::PurgeMaterializedViewLogStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::CancelMaterializedViewJobStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::RefreshMaterializedViewStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node
        .as_any()
        .downcast_ref::<parser_ast::RefreshMaterializedViewImplementStmt>()
    {
        return stmt.restore();
    }
    if let Some(stmt) = node.as_any().downcast_ref::<parser_ast::AnalyzeTableStmt>() {
        return stmt.restore();
    }
    if let Some(show) = node.as_any().downcast_ref::<parser_ast::ShowStmt>() {
        if show.Tp == parser_ast::ShowStmtType::StorageClassTransitions {
            let mut sql = "SHOW STORAGE_CLASS TRANSITIONS".to_owned();
            if let Some(pattern) = &show.Pattern {
                sql.push_str(" LIKE ");
                sql.push_str(&restore_expr(pattern)?);
            }
            if let Some(where_clause) = &show.Where {
                sql.push_str(" WHERE ");
                sql.push_str(&restore_expr(where_clause)?);
            }
            return Ok(sql);
        }
    }
    if let Some(select) = node.as_any().downcast_ref::<parser_ast::SelectStmt>() {
        if select.Fields.Fields.is_empty() {
            let text = node.Text();
            if !text.is_empty() {
                return Ok(text);
            }
        }
        return restore_select_stmt(select);
    }
    if let Some(set_opr) = node.as_any().downcast_ref::<parser_ast::SetOprStmt>() {
        return restore_setopr_stmt(set_opr);
    }
    let text = node.Text();
    if text.is_empty() {
        Err("restore_node: unsupported node without SQL text".into())
    } else {
        Ok(text)
    }
}

fn restore_with_clause(with: &parser_ast::WithClause) -> Result<String, String> {
    let mut sql = if with.IsRecursive {
        "WITH RECURSIVE"
    } else {
        "WITH"
    }
    .to_owned();
    let ctes = with
        .CTEs
        .iter()
        .map(|cte| {
            let mut part = quote(&cte.Name.O);
            if !cte.ColNameList.is_empty() {
                part.push('(');
                part.push_str(
                    &cte.ColNameList
                        .iter()
                        .map(|name| quote(&name.O))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                part.push(')');
            }
            part.push_str(" AS (");
            part.push_str(&restore_node(cte.Query.as_ref())?);
            part.push(')');
            Ok(part)
        })
        .collect::<Result<Vec<_>, String>>()?;
    sql.push(' ');
    sql.push_str(&ctes.join(", "));
    Ok(sql)
}

fn restore_table_source(source: &parser_ast::TableSource) -> Result<String, String> {
    if let Some(query) = &source.QuerySource {
        let inner = query
            .with_node(restore_node)
            .ok_or_else(|| "missing query source".to_string())??;
        let mut out = format!("({inner})");
        if !source.AsName.O.is_empty() {
            out.push_str(" AS ");
            out.push_str(&quote(&source.AsName.O));
        }
        return Ok(out);
    }
    let mut out = restore_table_name_core(&source.Source);
    if !source.AsName.O.is_empty() {
        out.push_str(" AS ");
        out.push_str(&quote(&source.AsName.O));
    }
    out.push_str(&restore_index_hints(&source.Source.IndexHints));
    Ok(out)
}

fn restore_result_set_node(node: &parser_ast::ResultSetNode) -> Result<String, String> {
    match node {
        parser_ast::ResultSetNode::TableSource(source) => restore_table_source(source),
        parser_ast::ResultSetNode::Join(join) => restore_join(join),
    }
}

/// 还原 Join 树文本。
fn restore_join(join: &parser_ast::Join) -> Result<String, String> {
    let left = join
        .Left
        .as_deref()
        .ok_or_else(|| "join is missing Left".to_string())?;
    let left_is_join = matches!(left, parser_ast::ResultSetNode::Join(_));
    let left_text = restore_result_set_node(left)?;
    let mut out = if left_is_join {
        format!("({left_text})")
    } else {
        left_text
    };
    let Some(right) = join.Right.as_deref() else {
        return Ok(out);
    };
    if join.NaturalJoin {
        out.push_str(" NATURAL");
    }
    match join.Tp {
        parser_ast::JoinType::LeftJoin => out.push_str(" LEFT"),
        parser_ast::JoinType::RightJoin => out.push_str(" RIGHT"),
        parser_ast::JoinType::FullJoin => out.push_str(" FULL OUTER"),
        parser_ast::JoinType::CrossJoin => {}
    }
    out.push_str(if join.StraightJoin {
        " STRAIGHT_JOIN "
    } else {
        " JOIN "
    });
    let right_is_join = matches!(right, parser_ast::ResultSetNode::Join(_));
    let right_text = restore_result_set_node(right)?;
    if right_is_join {
        out.push('(');
        out.push_str(&right_text);
        out.push(')');
    } else {
        out.push_str(&right_text);
    }
    if let Some(on) = &join.On {
        out.push_str(" ON ");
        out.push_str(&restore_expr(on)?);
    }
    if !join.Using.is_empty() {
        out.push_str(" USING (");
        out.push_str(
            &join
                .Using
                .iter()
                .map(restore_column_name)
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push(')');
    }
    Ok(out)
}

// 对齐 Go：无显式括号时把新交叉连接插入右子树最左叶

fn restore_select_field(field: &parser_ast::SelectField) -> Result<String, String> {
    let mut out = if let Some(wildcard) = &field.WildCard {
        let mut parts = Vec::new();
        if !wildcard.Schema.O.is_empty() {
            parts.push(quote(&wildcard.Schema.O));
        }
        if !wildcard.Table.O.is_empty() {
            parts.push(quote(&wildcard.Table.O));
        }
        parts.push("*".to_owned());
        parts.join(".")
    } else if let Some(expr) = &field.Expr {
        restore_expr(expr)?
    } else {
        String::new()
    };
    if !field.AsName.O.is_empty() {
        out.push_str(" AS ");
        out.push_str(&quote(&field.AsName.O));
    }
    Ok(out)
}

fn restore_field_list(fields: &parser_ast::FieldList) -> Result<String, String> {
    Ok(fields
        .Fields
        .iter()
        .map(restore_select_field)
        .collect::<Result<Vec<_>, _>>()?
        .join(", "))
}

fn restore_limit(limit: &parser_ast::Limit) -> Result<String, String> {
    let count = limit
        .Count
        .as_ref()
        .map(restore_expr)
        .transpose()?
        .unwrap_or_default();
    match &limit.Offset {
        Some(offset) => Ok(format!("LIMIT {},{count}", restore_expr(offset)?)),
        None => Ok(format!("LIMIT {count}")),
    }
}

pub fn restore_select_stmt(stmt: &parser_ast::SelectStmt) -> Result<String, String> {
    let opts = &stmt.SelectStmtOpts;
    let unsupported_options = opts.ExplicitAll
        || !opts.SQLCache
        || !opts.TableHints.is_empty()
        || opts.Priority != 0
        || opts.SQLSmallResult
        || opts.SQLBigResult
        || opts.SQLBufferResult
        || opts.CalcFoundRows
        || opts.StraightJoin;
    if stmt.Kind != parser_ast::SelectStmtKind::Select
        || unsupported_options
        || !stmt.TableHints.is_empty()
        || stmt.GroupByRollup
        || stmt.lock_info.is_some()
        || !stmt.Lists.is_empty()
        || stmt.SelectIntoOpt.is_some()
        || !stmt.children.is_empty()
    {
        let source = stmt.Text();
        return if source.is_empty() {
            Err("restore_select_stmt: unsupported SELECT feature without SQL text".into())
        } else {
            Ok(source)
        };
    }
    let mut out = String::new();
    if stmt.WithBeforeBraces {
        if let Some(with) = &stmt.With {
            out.push_str(&restore_with_clause(&with.borrow())?);
            out.push(' ');
        }
    }
    if stmt.IsInBraces {
        out.push('(');
    }
    if !stmt.WithBeforeBraces {
        if let Some(with) = &stmt.With {
            out.push_str(&restore_with_clause(&with.borrow())?);
            out.push(' ');
        }
    }
    out.push_str("SELECT ");
    if stmt.Distinct || opts.Distinct {
        out.push_str("DISTINCT ");
    }
    out.push_str(&restore_field_list(&stmt.Fields)?);
    if let Some(from) = &stmt.From {
        out.push_str(" FROM ");
        out.push_str(&restore_join(&from.TableRefs)?);
    }
    if let Some(where_expr) = &stmt.Where {
        out.push_str(" WHERE ");
        out.push_str(&restore_expr(where_expr)?);
    }
    if !stmt.GroupBy.is_empty() {
        out.push_str(" GROUP BY ");
        out.push_str(
            &stmt
                .GroupBy
                .iter()
                .map(restore_by_item)
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        );
    }
    if let Some(having) = &stmt.Having {
        out.push_str(" HAVING ");
        out.push_str(&restore_expr(having)?);
    }
    if !stmt.WindowSpecs.is_empty() {
        out.push_str(" WINDOW ");
        out.push_str(
            &stmt
                .WindowSpecs
                .iter()
                .map(restore_window_spec)
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        );
    }
    if !stmt.OrderBy.is_empty() {
        out.push_str(" ORDER BY ");
        out.push_str(
            &stmt
                .OrderBy
                .iter()
                .map(restore_by_item)
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        );
    }
    if let Some(limit) = &stmt.Limit {
        out.push(' ');
        out.push_str(&restore_limit(limit)?);
    }
    if stmt.IsInBraces {
        out.push(')');
    }
    Ok(out)
}

fn restore_setopr_stmt(stmt: &parser_ast::SetOprStmt) -> Result<String, String> {
    let mut out = String::new();
    if let Some(with) = stmt.With.as_ref().or(stmt.select_list.With.as_ref()) {
        out.push_str(&restore_with_clause(&with.borrow())?);
        out.push(' ');
    }
    if stmt.IsInBraces {
        out.push('(');
    }
    let mut parts = Vec::new();
    for (index, select) in stmt.select_list.selects.iter().enumerate() {
        let restored = restore_node(select.as_ref())?;
        if index == 0 {
            parts.push(restored);
            continue;
        }
        let operator = stmt.select_list.operators.get(index).and_then(|op| *op);
        let keyword = match operator {
            Some(parser_ast::SetOprType::Union) | None => "UNION",
            Some(parser_ast::SetOprType::UnionAll) => "UNION ALL",
            Some(parser_ast::SetOprType::Except) => "EXCEPT",
            Some(parser_ast::SetOprType::ExceptAll) => "EXCEPT ALL",
            Some(parser_ast::SetOprType::Intersect) => "INTERSECT",
            Some(parser_ast::SetOprType::IntersectAll) => "INTERSECT ALL",
        };
        parts.push(format!("{keyword} {restored}"));
    }
    out.push_str(&parts.join(" "));
    if !stmt.OrderBy.is_empty() {
        out.push_str(" ORDER BY ");
        out.push_str(
            &stmt
                .OrderBy
                .iter()
                .map(restore_by_item)
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        );
    }
    if let Some(limit) = &stmt.Limit {
        out.push(' ');
        out.push_str(&restore_limit(limit)?);
    }
    if stmt.IsInBraces {
        out.push(')');
    }
    Ok(out)
}
