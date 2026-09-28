// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! 规范解析器 AST 的遍历与 SQL 恢复工具。
//!
//! 本模块遍历语句中的真实表节点以判断是否需要默认数据库，并将支持的查询及
//! DML 节点恢复为规范 SQL。恢复过程统一处理标识符、字面量、CTE 与库名注入，
//! 避免调用方直接拼接解析器内部结构。

use crate::parser_core::ast;
use std::collections::HashSet;

#[derive(Default)]
struct ImplicitDatabase {
    has_implicit: bool,
}

impl ast::Visitor for ImplicitDatabase {
    fn enter(&mut self, input: &dyn ast::Node) -> bool {
        if let Some(select) = input.as_any().downcast_ref::<ast::SelectStmt>() {
            if let Some(from) = &select.From {
                self.visit_join(&from.TableRefs);
            }
        } else if let Some(insert) = input.as_any().downcast_ref::<ast::InsertStmt>() {
            if let Some(table) = &insert.Table {
                self.visit_join(&table.TableRefs);
            }
        } else if let Some(update) = input.as_any().downcast_ref::<ast::UpdateStmt>() {
            if let Some(table) = &update.TableRefs {
                self.visit_join(&table.TableRefs);
            }
        } else if let Some(delete) = input.as_any().downcast_ref::<ast::DeleteStmt>() {
            if let Some(table) = &delete.TableRefs {
                self.visit_join(&table.TableRefs);
            }
            for table in &delete.Tables {
                self.visit_table(table);
            }
        } else if let Some(expr) = input.as_any().downcast_ref::<ast::ExprNode>()
            && let ast::ExprKind::TableName(table) = &expr.Kind
        {
            self.visit_table(table);
        }
        self.has_implicit
    }

    fn leave(&mut self, _input: &dyn ast::Node) -> bool {
        true
    }
}

impl ImplicitDatabase {
    fn visit_table(&mut self, table: &ast::TableName) {
        self.has_implicit |= table.Schema.L.is_empty();
    }

    fn visit_result_set(&mut self, result_set: &ast::ResultSetNode) {
        match result_set {
            ast::ResultSetNode::TableSource(source) if source.QuerySource.is_none() => {
                self.visit_table(&source.Source);
            }
            ast::ResultSetNode::TableSource(_) => {}
            ast::ResultSetNode::Join(join) => self.visit_join(join),
        }
    }

    fn visit_join(&mut self, join: &ast::Join) {
        if let Some(left) = join.Left.as_deref() {
            self.visit_result_set(left);
        }
        if let Some(right) = join.Right.as_deref() {
            self.visit_result_set(right);
        }
    }
}

/// 当语句中至少有一个真实表节点未指定库名时返回 `db_name`，否则返回空串。
///
/// CTE、派生表和表达式子查询会继续递归检查，列名中的限定符不会被误判为表。
#[allow(non_snake_case)]
pub fn GetDefaultDB(statement: &dyn ast::Node, db_name: &str) -> String {
    let mut implicit_database = ImplicitDatabase::default();
    statement.accept(&mut implicit_database);
    if implicit_database.has_implicit {
        db_name.to_owned()
    } else {
        String::new()
    }
}

// 只在表名之前的 SQL 前缀中识别由空格或逗号分隔的完整 token，避免把同名子串
// 或列名替换成带库名的表名。
fn find_table_pos(sql_prefix: &str, table_name: &str) -> Option<usize> {
    let bytes = sql_prefix.as_bytes();
    let mut token_start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(byte, b' ' | b',') {
            if table_name.len() == index - token_start
                && &sql_prefix[token_start..index] == table_name
            {
                return Some(token_start);
            }
            token_start = index + 1;
        }
    }
    (table_name.len() == sql_prefix.len() - token_start && &sql_prefix[token_start..] == table_name)
        .then_some(token_start)
}

/// 处理与 Go `SimpleCases` 一致的保守 `InsertStmt` 快路径。
///
/// 仅对没有查询源、`SET`、冲突更新和 Hint 的单表 INSERT 修改原始 SQL；任何结构
/// 不确定的情况都返回 `false`，交由完整 AST 恢复器处理。
#[allow(non_snake_case)]
pub fn SimpleCases(statement: &dyn ast::Node, default_db: &str, origin: &str) -> (String, bool) {
    if origin.is_empty() {
        return (String::new(), false);
    }
    let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() else {
        return (String::new(), false);
    };
    if insert.Select.is_some()
        || insert.Setlist
        || !insert.OnDuplicate.is_empty()
        || !insert.TableHints.is_empty()
    {
        return (String::new(), false);
    }
    let Some(join) = insert.Table.as_ref().map(|table| &table.TableRefs) else {
        return (String::new(), false);
    };
    if join.Tp != ast::JoinType::CrossJoin || join.Right.is_some() {
        return (String::new(), false);
    }
    let Some(ast::ResultSetNode::TableSource(source)) = join.Left.as_deref() else {
        return (String::new(), false);
    };
    if source.QuerySource.is_some() {
        return (String::new(), false);
    }
    let table = &source.Source;
    let Some(paren_pos) = origin.find('(') else {
        return (String::new(), false);
    };
    if origin[..paren_pos].contains('.') {
        return (origin.to_owned(), true);
    }
    let lower = origin[..paren_pos].to_lowercase();
    let Some(position) = find_table_pos(&lower, &table.Name.L) else {
        return (String::new(), false);
    };
    let schema = if table.Schema.O.is_empty() {
        default_db
    } else {
        &table.Schema.O
    };
    let mut result = String::with_capacity(origin.len() + schema.len() + 1);
    result.push_str(&origin[..position]);
    result.push_str(schema);
    result.push('.');
    result.push_str(&origin[position..]);
    (result, true)
}

const RESTORE_STRING_SINGLE_QUOTES: u64 = 1 << 0;
const RESTORE_NAME_BACK_QUOTES: u64 = 1 << 8;
const RESTORE_SPACES_AROUND_BINARY_OPERATION: u64 = 1 << 9;
const RESTORE_STRING_WITHOUT_CHARSET: u64 = 1 << 11;
const RESTORE_WITHOUT_SCHEMA_NAME: u64 = 1 << 16;
const RESTORE_SKIP_REDUNDANT_PARENTHESES: u64 = 1 << 20;

/// 默认 SQL 恢复选项：统一字符串、标识符与二元运算符的输出格式。
#[allow(non_upper_case_globals)]
pub const defaultRestoreFlag: u64 = RESTORE_STRING_SINGLE_QUOTES
    | RESTORE_SPACES_AROUND_BINARY_OPERATION
    | RESTORE_STRING_WITHOUT_CHARSET
    | RESTORE_NAME_BACK_QUOTES;

/// 绑定 SQL 的恢复选项；额外去除不影响语义的冗余括号。
#[allow(non_upper_case_globals)]
pub const bindingRestoreFlag: u64 = defaultRestoreFlag | RESTORE_SKIP_REDUNDANT_PARENTHESES;

fn time_unit_keyword(unit: ast::TimeUnitType) -> &'static str {
    use ast::TimeUnitType;
    match unit {
        TimeUnitType::Invalid => "",
        TimeUnitType::Microsecond => "MICROSECOND",
        TimeUnitType::Second => "SECOND",
        TimeUnitType::Minute => "MINUTE",
        TimeUnitType::Hour => "HOUR",
        TimeUnitType::Day => "DAY",
        TimeUnitType::Week => "WEEK",
        TimeUnitType::Month => "MONTH",
        TimeUnitType::Quarter => "QUARTER",
        TimeUnitType::Year => "YEAR",
        TimeUnitType::SecondMicrosecond => "SECOND_MICROSECOND",
        TimeUnitType::MinuteMicrosecond => "MINUTE_MICROSECOND",
        TimeUnitType::MinuteSecond => "MINUTE_SECOND",
        TimeUnitType::HourMicrosecond => "HOUR_MICROSECOND",
        TimeUnitType::HourSecond => "HOUR_SECOND",
        TimeUnitType::HourMinute => "HOUR_MINUTE",
        TimeUnitType::DayMicrosecond => "DAY_MICROSECOND",
        TimeUnitType::DaySecond => "DAY_SECOND",
        TimeUnitType::DayMinute => "DAY_MINUTE",
        TimeUnitType::DayHour => "DAY_HOUR",
        TimeUnitType::YearMonth => "YEAR_MONTH",
    }
}

/// 将受支持的 AST 节点递归恢复为规范 SQL，并集中维护库名与 CTE 作用域。
struct Restorer<'a> {
    /// 表节点未显式指定库名时使用的默认数据库。
    default_db: &'a str,
    /// 为绑定等场景恢复 SQL 时，是否抑制所有库名前缀。
    without_schema: bool,
    /// 当前语句声明的 CTE 名；同名引用不能被误加默认数据库。
    cte_names: HashSet<String>,
}

fn binary_precedence(operator: &str) -> u8 {
    match operator.to_ascii_uppercase().as_str() {
        "OR" => 1,
        "XOR" => 2,
        "AND" => 3,
        "=" | "==" | "<=>" | "!=" | "<>" | "<" | "<=" | ">" | ">=" => 4,
        "|" => 5,
        "^" => 6,
        "&" => 7,
        "<<" | ">>" => 8,
        "+" | "-" => 9,
        "*" | "/" | "DIV" | "%" | "MOD" => 10,
        _ => 0,
    }
}

fn parentheses_are_redundant(expr: &ast::ExprNode, parent: Option<(&str, bool)>) -> bool {
    let ast::ExprKind::Binary { Op: child, .. } = &expr.Kind else {
        return matches!(expr.Kind, ast::ExprKind::Parentheses(_));
    };
    let Some((parent, right_child)) = parent else {
        return true;
    };
    if parent == "<unary>" {
        return false;
    }
    let child_precedence = binary_precedence(child);
    let parent_precedence = binary_precedence(parent);
    if child_precedence != parent_precedence {
        return child_precedence > parent_precedence;
    }
    if !right_child {
        return true;
    }
    matches!(
        parent.to_ascii_uppercase().as_str(),
        "AND" | "OR" | "+" | "*"
    ) && child.eq_ignore_ascii_case(parent)
}

impl<'a> Restorer<'a> {
    fn new(default_db: &'a str, without_schema: bool) -> Self {
        Self {
            default_db,
            without_schema,
            cte_names: HashSet::new(),
        }
    }

    fn name(&self, value: &str) -> String {
        format!("`{}`", value.replace('`', "``"))
    }

    fn table_name(&self, table: &ast::TableName) -> String {
        let mut output = String::new();
        // 显式库名优先；仅普通表可回退到默认库，CTE 引用必须保持无库名前缀。
        if !self.without_schema {
            let schema = if !table.Schema.O.is_empty() {
                Some(table.Schema.O.as_str())
            } else if !self.default_db.is_empty() && !self.cte_names.contains(&table.Name.L) {
                Some(self.default_db)
            } else {
                None
            };
            if let Some(schema) = schema {
                output.push_str(&self.name(schema));
                output.push('.');
            }
        }
        output.push_str(&self.name(&table.Name.O));
        if !table.PartitionNames.is_empty() {
            output.push_str(" PARTITION(");
            output.push_str(
                &table
                    .PartitionNames
                    .iter()
                    .map(|name| self.name(&name.O))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(')');
        }
        output
    }

    fn column_name(&self, column: &ast::ColumnName) -> String {
        let mut parts = Vec::new();
        if !self.without_schema && !column.Schema.O.is_empty() {
            parts.push(self.name(&column.Schema.O));
        }
        if !column.Table.O.is_empty() {
            parts.push(self.name(&column.Table.O));
        }
        parts.push(self.name(&column.Name.O));
        parts.join(".")
    }

    fn value(&self, value: &ast::ValueExpr) -> String {
        // 按 SQL 字面量规则转义字符串；浮点值由解析阶段保留的位模式精确还原。
        use ast::ValueDatum;
        match &value.Datum {
            ValueDatum::Null => "NULL".to_owned(),
            ValueDatum::Bool(value) => {
                if *value {
                    "TRUE".to_owned()
                } else {
                    "FALSE".to_owned()
                }
            }
            ValueDatum::Int64(value) => value.to_string(),
            ValueDatum::Uint64(value) => value.to_string(),
            ValueDatum::Float32(bits) => f32::from_bits(*bits).to_string(),
            ValueDatum::Float64(bits) => f64::from_bits(*bits).to_string(),
            ValueDatum::Decimal(value) => value.clone(),
            ValueDatum::String(value) => format!("'{}'", value.replace('\'', "''")),
            ValueDatum::Bytes(value) => {
                format!("'{}'", String::from_utf8_lossy(value).replace('\'', "''"))
            }
            ValueDatum::BitLiteral(value) => format!(
                "0b{}",
                value
                    .iter()
                    .map(|byte| format!("{byte:08b}"))
                    .collect::<String>()
            ),
            ValueDatum::HexLiteral(value) => format!(
                "0x{}",
                value
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ),
        }
    }

    fn expr(&mut self, expr: &ast::ExprNode) -> Result<String, String> {
        self.expr_with_parent(expr, None)
    }

    fn expr_with_parent(
        &mut self,
        expr: &ast::ExprNode,
        parent: Option<(&str, bool)>,
    ) -> Result<String, String> {
        use ast::ExprKind;
        match &expr.Kind {
            ExprKind::Value(value) => Ok(self.value(value)),
            ExprKind::IntroducedValue { Value, Charset, .. } => {
                Ok(format!("_{Charset}'{}'", Value.replace('\'', "''")))
            }
            ExprKind::Column(column) => Ok(self.column_name(column)),
            ExprKind::Variable {
                Name,
                IsSystem,
                IsGlobal,
                IsInstance,
                ..
            } => {
                if *IsSystem {
                    let scope = if *IsGlobal {
                        "GLOBAL."
                    } else if *IsInstance {
                        "INSTANCE."
                    } else {
                        ""
                    };
                    Ok(format!("@@{scope}{}", self.name(Name)))
                } else {
                    Ok(format!("@{}", self.name(Name)))
                }
            }
            ExprKind::Function { FnName, Args, .. } => Ok(format!(
                "{}({})",
                FnName.O,
                Args.iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            )),
            ExprKind::AggregateFunction {
                Name,
                Args,
                Distinct,
                ..
            } => Ok(format!(
                "{Name}({}{})",
                if *Distinct { "DISTINCT " } else { "" },
                Args.iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            )),
            ExprKind::Binary { Op, L, R } => Ok(format!(
                "{} {} {}",
                self.expr_with_parent(L, Some((Op, false)))?,
                Op,
                self.expr_with_parent(R, Some((Op, true)))?
            )),
            ExprKind::Unary { Op, V } => Ok(format!(
                "{Op}{}",
                self.expr_with_parent(V, Some(("<unary>", true)))?
            )),
            ExprKind::IsTruth { Expr, Not, True } => Ok(format!(
                "{} IS {}{}",
                self.expr(Expr)?,
                if *Not { "NOT " } else { "" },
                if *True { "TRUE" } else { "FALSE" }
            )),
            ExprKind::IsNull { Expr, Not } => Ok(format!(
                "{} IS {}NULL",
                self.expr(Expr)?,
                if *Not { "NOT " } else { "" }
            )),
            ExprKind::InList {
                Expr, List, Not, ..
            } => Ok(format!(
                "{} {}IN ({})",
                self.expr(Expr)?,
                if *Not { "NOT " } else { "" },
                List.iter()
                    .map(|item| self.expr(item))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            )),
            ExprKind::Between {
                Expr,
                Left,
                Right,
                Not,
            } => Ok(format!(
                "{} {}BETWEEN {} AND {}",
                self.expr(Expr)?,
                if *Not { "NOT " } else { "" },
                self.expr(Left)?,
                self.expr(Right)?
            )),
            ExprKind::Like {
                Expr,
                Pattern,
                Not,
                Escape,
                ..
            } => {
                let mut output = format!(
                    "{} {}LIKE {}",
                    self.expr(Expr)?,
                    if *Not { "NOT " } else { "" },
                    self.expr(Pattern)?
                );
                if !Escape.is_empty() {
                    output.push_str(&format!(" ESCAPE '{}'", Escape.replace('\'', "''")));
                }
                Ok(output)
            }
            ExprKind::Regexp {
                Expr, Pattern, Not, ..
            } => Ok(format!(
                "{} {}REGEXP {}",
                self.expr(Expr)?,
                if *Not { "NOT " } else { "" },
                self.expr(Pattern)?
            )),
            ExprKind::Row(values) => Ok(format!(
                "ROW({})",
                values
                    .iter()
                    .map(|value| self.expr(value))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            )),
            ExprKind::Collate { Expr, Collation } => {
                Ok(format!("{} COLLATE {Collation}", self.expr(Expr)?))
            }
            ExprKind::NamedDefault(column) => Ok(format!("DEFAULT({})", self.column_name(column))),
            ExprKind::MaxValue => Ok("MAXVALUE".to_owned()),
            ExprKind::MatchAgainst {
                ColumnNames,
                Against,
                Modifier,
            } => {
                let modifier = match Modifier {
                    1 => " IN BOOLEAN MODE",
                    2 => " IN NATURAL LANGUAGE MODE",
                    3 => " IN NATURAL LANGUAGE MODE WITH QUERY EXPANSION",
                    4 => " WITH QUERY EXPANSION",
                    _ => "",
                };
                Ok(format!(
                    "MATCH ({}) AGAINST ({}{modifier})",
                    ColumnNames
                        .iter()
                        .map(|column| self.column_name(column))
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.expr(Against)?
                ))
            }
            ExprKind::Case {
                Value,
                WhenClauses,
                ElseClause,
            } => {
                let mut output = "CASE".to_owned();
                if let Some(value) = Value {
                    output.push(' ');
                    output.push_str(&self.expr(value)?);
                }
                for clause in WhenClauses {
                    output.push_str(" WHEN ");
                    output.push_str(&self.expr(&clause.Expr)?);
                    output.push_str(" THEN ");
                    output.push_str(&self.expr(&clause.Result)?);
                }
                if let Some(value) = ElseClause {
                    output.push_str(" ELSE ");
                    output.push_str(&self.expr(value)?);
                }
                output.push_str(" END");
                Ok(output)
            }
            ExprKind::WindowFunction {
                Name, Args, Spec, ..
            } => {
                let mut specification = String::new();
                if !Spec.Name.O.is_empty() {
                    specification.push_str(&self.name(&Spec.Name.O));
                } else {
                    specification.push('(');
                    if !Spec.PartitionBy.is_empty() {
                        specification.push_str("PARTITION BY ");
                        specification.push_str(&self.by_items(&Spec.PartitionBy)?);
                    }
                    if !Spec.OrderBy.is_empty() {
                        if !Spec.PartitionBy.is_empty() {
                            specification.push(' ');
                        }
                        specification.push_str("ORDER BY ");
                        specification.push_str(&self.by_items(&Spec.OrderBy)?);
                    }
                    specification.push(')');
                }
                Ok(format!(
                    "{Name}({}) OVER {specification}",
                    Args.iter()
                        .map(|arg| self.expr(arg))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ")
                ))
            }
            ExprKind::TimeUnit(unit) => Ok(time_unit_keyword(*unit).to_owned()),
            ExprKind::GetFormatSelector(selector) => Ok(match selector {
                ast::GetFormatSelectorType::Date => "DATE",
                ast::GetFormatSelectorType::Datetime => "DATETIME",
                ast::GetFormatSelectorType::Time => "TIME",
            }
            .to_owned()),
            ExprKind::TrimDirection(direction) => Ok(match direction {
                ast::TrimDirectionType::Both => "BOTH",
                ast::TrimDirectionType::Leading => "LEADING",
                ast::TrimDirectionType::Trailing => "TRAILING",
            }
            .to_owned()),
            ExprKind::TableName(table) => Ok(self.table_name(table)),
            ExprKind::Parentheses(inner) => {
                if parentheses_are_redundant(inner, parent) {
                    self.expr_with_parent(inner, parent)
                } else {
                    Ok(format!("({})", self.expr(inner)?))
                }
            }
            ExprKind::ParamMarker { .. } => Ok("?".to_owned()),
            ExprKind::DefaultValue => Ok("DEFAULT".to_owned()),
            ExprKind::Subquery { Query, .. } => Query
                .with_node(|node| self.node(node))
                .ok_or_else(|| "subquery node is missing".to_owned())?
                .map(|sql| format!("({sql})")),
            ExprKind::CompareSubquery { Op, L, R, All } => Ok(format!(
                "{} {Op} {} {}",
                self.expr(L)?,
                if *All { "ALL" } else { "ANY" },
                self.expr(R)?
            )),
            ExprKind::InSubquery { Expr, Sel, Not } => Ok(format!(
                "{} {}IN {}",
                self.expr(Expr)?,
                if *Not { "NOT " } else { "" },
                self.expr(Sel)?
            )),
            ExprKind::ExistsSubquery { Sel, Not } => Ok(format!(
                "{}EXISTS {}",
                if *Not { "NOT " } else { "" },
                self.expr(Sel)?
            )),
            ExprKind::Cast {
                Expr,
                Tp,
                FunctionType,
                ..
            } => match FunctionType {
                ast::CastFunctionType::Cast => {
                    Ok(format!("CAST({} AS {})", self.expr(Expr)?, Tp.String()))
                }
                ast::CastFunctionType::Convert => {
                    Ok(format!("CONVERT({}, {})", self.expr(Expr)?, Tp.String()))
                }
                ast::CastFunctionType::Binary => Ok(format!("BINARY {}", self.expr(Expr)?)),
            },
            ExprKind::JSONSumCrc32 { Expr, .. } => {
                Ok(format!("JSON_SUM_CRC32({})", self.expr(Expr)?))
            }
        }
    }

    fn by_items(&mut self, items: &[ast::ByItem]) -> Result<String, String> {
        items
            .iter()
            .map(|item| {
                let mut output = self.expr(&item.Expr)?;
                if item.Desc {
                    output.push_str(" DESC");
                }
                Ok(output)
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|items| items.join(", "))
    }

    fn result_set(&mut self, node: &ast::ResultSetNode) -> Result<String, String> {
        match node {
            ast::ResultSetNode::TableSource(source) => {
                let mut output = if let Some(query) = &source.QuerySource {
                    let query = query
                        .with_node(|node| self.node(node))
                        .ok_or_else(|| "table query source is missing".to_owned())??;
                    format!("({query})")
                } else {
                    self.table_name(&source.Source)
                };
                if !source.AsName.O.is_empty() {
                    output.push_str(" AS ");
                    output.push_str(&self.name(&source.AsName.O));
                }
                Ok(output)
            }
            ast::ResultSetNode::Join(join) => self.join(join),
        }
    }

    fn join(&mut self, join: &ast::Join) -> Result<String, String> {
        let left = join
            .Left
            .as_deref()
            .ok_or_else(|| "join has no left source".to_owned())?;
        let mut output = self.result_set(left)?;
        let Some(right) = join.Right.as_deref() else {
            return Ok(output);
        };
        if join.NaturalJoin {
            output.push_str(" NATURAL");
        }
        match join.Tp {
            ast::JoinType::CrossJoin => {}
            ast::JoinType::LeftJoin => output.push_str(" LEFT"),
            ast::JoinType::RightJoin => output.push_str(" RIGHT"),
        }
        output.push_str(if join.StraightJoin {
            " STRAIGHT_JOIN "
        } else {
            " JOIN "
        });
        output.push_str(&self.result_set(right)?);
        if let Some(on) = &join.On {
            output.push_str(" ON ");
            output.push_str(&self.expr(on)?);
        }
        if !join.Using.is_empty() {
            output.push_str(" USING (");
            output.push_str(
                &join
                    .Using
                    .iter()
                    .map(|column| self.name(&column.Name.O))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(')');
        }
        Ok(output)
    }

    fn select(&mut self, select: &ast::SelectStmt) -> Result<String, String> {
        let mut output = String::new();
        if let Some(with) = select.With.as_ref().map(|with| with.borrow()) {
            output.push_str("WITH ");
            if with.IsRecursive {
                output.push_str("RECURSIVE ");
            }
            // 先登记全部名称，使 CTE 之间的前向/相互引用都不会注入默认数据库。
            for cte in &with.CTEs {
                self.cte_names.insert(cte.Name.L.clone());
            }
            output.push_str(
                &with
                    .CTEs
                    .iter()
                    .map(|cte| {
                        let query = self.node(cte.Query.as_ref())?;
                        Ok(format!("{} AS ({query})", self.name(&cte.Name.O)))
                    })
                    .collect::<Result<Vec<_>, String>>()?
                    .join(", "),
            );
            output.push(' ');
        }
        output.push_str("SELECT ");
        if select.Distinct {
            output.push_str("DISTINCT ");
        }
        output.push_str(
            &select
                .Fields
                .Fields
                .iter()
                .map(|field| {
                    let mut rendered = if let Some(wildcard) = &field.WildCard {
                        let mut parts = Vec::new();
                        if !self.without_schema && !wildcard.Schema.O.is_empty() {
                            parts.push(self.name(&wildcard.Schema.O));
                        }
                        if !wildcard.Table.O.is_empty() {
                            parts.push(self.name(&wildcard.Table.O));
                        }
                        parts.push("*".to_owned());
                        parts.join(".")
                    } else if let Some(expr) = &field.Expr {
                        self.expr(expr)?
                    } else {
                        String::new()
                    };
                    if !field.AsName.O.is_empty() {
                        rendered.push_str(" AS ");
                        rendered.push_str(&self.name(&field.AsName.O));
                    }
                    Ok(rendered)
                })
                .collect::<Result<Vec<_>, String>>()?
                .join(", "),
        );
        if let Some(from) = &select.From {
            output.push_str(" FROM ");
            output.push_str(&self.join(&from.TableRefs)?);
        }
        if let Some(expr) = &select.Where {
            output.push_str(" WHERE ");
            output.push_str(&self.expr(expr)?);
        }
        if !select.GroupBy.is_empty() {
            output.push_str(" GROUP BY ");
            output.push_str(&self.by_items(&select.GroupBy)?);
        }
        if let Some(expr) = &select.Having {
            output.push_str(" HAVING ");
            output.push_str(&self.expr(expr)?);
        }
        if !select.OrderBy.is_empty() {
            output.push_str(" ORDER BY ");
            output.push_str(&self.by_items(&select.OrderBy)?);
        }
        if let Some(limit) = &select.Limit {
            output.push_str(" LIMIT ");
            if let Some(offset) = &limit.Offset {
                output.push_str(&self.expr(offset)?);
                output.push(',');
            }
            if let Some(count) = &limit.Count {
                output.push_str(&self.expr(count)?);
            }
        }
        Ok(output)
    }

    fn set_operation(&mut self, set: &ast::SetOprStmt) -> Result<String, String> {
        let mut parts = Vec::new();
        for (index, select) in set.select_list.selects.iter().enumerate() {
            let sql = self.node(select.as_ref())?;
            if index == 0 {
                parts.push(sql);
                continue;
            }
            let keyword = match set
                .select_list
                .operators
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(ast::SetOprType::Union)
            {
                ast::SetOprType::Union => "UNION",
                ast::SetOprType::UnionAll => "UNION ALL",
                ast::SetOprType::Except => "EXCEPT",
                ast::SetOprType::ExceptAll => "EXCEPT ALL",
                ast::SetOprType::Intersect => "INTERSECT",
                ast::SetOprType::IntersectAll => "INTERSECT ALL",
            };
            parts.push(format!("{keyword} {sql}"));
        }
        Ok(parts.join(" "))
    }

    fn assignments(&mut self, assignments: &[ast::Assignment]) -> Result<String, String> {
        assignments
            .iter()
            .map(|assignment| {
                Ok(format!(
                    "{} = {}",
                    self.column_name(&assignment.Column),
                    self.expr(&assignment.Expr)?
                ))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|items| items.join(", "))
    }

    fn insert(&mut self, insert: &ast::InsertStmt) -> Result<String, String> {
        let mut output = if insert.IsReplace {
            "REPLACE".to_owned()
        } else {
            "INSERT".to_owned()
        };
        if insert.IgnoreErr {
            output.push_str(" IGNORE");
        }
        output.push_str(" INTO ");
        let table = insert
            .Table
            .as_ref()
            .ok_or_else(|| "insert has no target table".to_owned())?;
        output.push_str(&self.join(&table.TableRefs)?);
        if !insert.Columns.is_empty() {
            output.push_str(" (");
            output.push_str(
                &insert
                    .Columns
                    .iter()
                    .map(|column| self.name(&column.Name.O))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(')');
        }
        if insert.Setlist {
            let Some(values) = insert.Lists.first() else {
                return Err("INSERT SET has no values".to_owned());
            };
            output.push_str(" SET ");
            output.push_str(
                &insert
                    .Columns
                    .iter()
                    .zip(values)
                    .map(|(column, value)| {
                        Ok(format!(
                            "{} = {}",
                            self.name(&column.Name.O),
                            self.expr(value)?
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?
                    .join(", "),
            );
        } else if !insert.Lists.is_empty() {
            output.push_str(" VALUES ");
            output.push_str(
                &insert
                    .Lists
                    .iter()
                    .map(|row| {
                        Ok(format!(
                            "({})",
                            row.iter()
                                .map(|value| self.expr(value))
                                .collect::<Result<Vec<_>, _>>()?
                                .join(", ")
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?
                    .join(", "),
            );
        } else if let Some(select) = &insert.Select {
            output.push(' ');
            output.push_str(&self.node(select.as_ref())?);
        }
        if !insert.OnDuplicate.is_empty() {
            output.push_str(" ON DUPLICATE KEY UPDATE ");
            output.push_str(&self.assignments(&insert.OnDuplicate)?);
        }
        Ok(output)
    }

    fn update(&mut self, update: &ast::UpdateStmt) -> Result<String, String> {
        let mut output = "UPDATE ".to_owned();
        if update.IgnoreErr {
            output.push_str("IGNORE ");
        }
        let table = update
            .TableRefs
            .as_ref()
            .ok_or_else(|| "update has no table".to_owned())?;
        output.push_str(&self.join(&table.TableRefs)?);
        output.push_str(" SET ");
        output.push_str(&self.assignments(&update.List)?);
        if let Some(where_expr) = &update.Where {
            output.push_str(" WHERE ");
            output.push_str(&self.expr(where_expr)?);
        }
        if !update.Order.is_empty() {
            output.push_str(" ORDER BY ");
            output.push_str(&self.by_items(&update.Order)?);
        }
        if let Some(limit) = &update.Limit {
            output.push_str(" LIMIT ");
            if let Some(count) = &limit.Count {
                output.push_str(&self.expr(count)?);
            }
        }
        Ok(output)
    }

    fn delete(&mut self, delete: &ast::DeleteStmt) -> Result<String, String> {
        let mut output = "DELETE".to_owned();
        if delete.IgnoreErr {
            output.push_str(" IGNORE");
        }
        if delete.IsMultiTable && !delete.Tables.is_empty() {
            output.push(' ');
            output.push_str(
                &delete
                    .Tables
                    .iter()
                    .map(|table| self.table_name(table))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        output.push_str(" FROM ");
        let table = delete
            .TableRefs
            .as_ref()
            .ok_or_else(|| "delete has no table".to_owned())?;
        output.push_str(&self.join(&table.TableRefs)?);
        if let Some(where_expr) = &delete.Where {
            output.push_str(" WHERE ");
            output.push_str(&self.expr(where_expr)?);
        }
        if !delete.Order.is_empty() {
            output.push_str(" ORDER BY ");
            output.push_str(&self.by_items(&delete.Order)?);
        }
        if let Some(limit) = &delete.Limit {
            output.push_str(" LIMIT ");
            if let Some(count) = &limit.Count {
                output.push_str(&self.expr(count)?);
            }
        }
        Ok(output)
    }

    fn node(&mut self, node: &dyn ast::Node) -> Result<String, String> {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            self.select(select)
        } else if let Some(set) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            self.set_operation(set)
        } else if let Some(insert) = node.as_any().downcast_ref::<ast::InsertStmt>() {
            self.insert(insert)
        } else if let Some(update) = node.as_any().downcast_ref::<ast::UpdateStmt>() {
            self.update(update)
        } else if let Some(delete) = node.as_any().downcast_ref::<ast::DeleteStmt>() {
            self.delete(delete)
        } else {
            Err("statement type does not expose a canonical restore yet".to_owned())
        }
    }
}

/// 将真实解析器语句恢复为规范 SQL，并为未限定的普通表注入默认数据库。
///
/// 简单 INSERT 优先保留原始 SQL 的写法；其余语句使用结构化恢复。不支持或缺失
/// 必要节点时返回空串，保持与现有调用约定一致。
#[allow(non_snake_case)]
pub fn RestoreWithDefaultDB(statement: &dyn ast::Node, default_db: &str, origin: &str) -> String {
    let (simple, ok) = SimpleCases(statement, default_db, origin);
    if ok {
        return simple;
    }
    Restorer::new(default_db, false)
        .node(statement)
        .unwrap_or_default()
}

/// 将真实解析器语句恢复为规范 SQL，同时抑制所有库名前缀。
///
/// 不支持或缺失必要节点时返回空串。
#[allow(non_snake_case)]
pub fn RestoreWithoutDB(statement: &dyn ast::Node) -> String {
    Restorer::new("", true).node(statement).unwrap_or_default()
}
