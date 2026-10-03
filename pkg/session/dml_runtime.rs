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

// AST-driven DML plans used by the concrete session runtime.
//
// This module deliberately keeps SQL parsing outside the storage lifecycle:
// callers hand it canonical parser nodes, and it returns key/value mutations
// that are applied through a real `kv::Transaction` by `runtime`.
//
// 面向具体会话运行时的 AST 驱动 DML（数据操纵语言：INSERT/UPDATE/DELETE）计划。
//
// 本模块刻意将 SQL 解析置于存储生命周期之外：调用方传入规范解析节点，
// 本模块返回键值变更，由 `runtime` 通过真实的 `kv::Transaction`（事务）落盘。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use astersql_parser::Parser;
use astersql_parser_ast as ast;
use chrono::{Datelike, Months, NaiveDateTime, Timelike, Utc};
use regex::Regex;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::{SessionError, SessionResult};

/// 按解析器解析出的限定名读取列；限定列不回退到同名的其它表。
fn eval_column(
    column: &ast::ColumnName,
    row: &HashMap<String, Option<String>>,
) -> SessionResult<Option<String>> {
    if !column.Table.L.is_empty() {
        let qualified = format!("{}.{}", column.Table.L, column.Name.L);
        return row.get(&qualified).cloned().ok_or_else(|| {
            SessionError::new(format!(
                "unknown DML column {}.{}",
                column.Table.O, column.Name.O
            ))
        });
    }
    row.get(&column.Name.L)
        .cloned()
        .ok_or_else(|| SessionError::new(format!("unknown DML column {}", column.Name.O)))
}

/// MySQL 数值表达式在该会话运行时中的十进制表示。
fn eval_decimal(value: &str, context: &str) -> SessionResult<Decimal> {
    Decimal::from_str(value)
        .map_err(|error| SessionError::new(format!("{context} is not numeric: {error}")))
}

fn decimal_text(value: Decimal) -> String {
    value.normalize().to_string()
}

fn parse_datetime(value: &str) -> SessionResult<NaiveDateTime> {
    ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .ok_or_else(|| SessionError::new(format!("invalid TIMESTAMPADD datetime {value}")))
}

fn format_datetime(value: NaiveDateTime, source: &str) -> String {
    let precision = source
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len().min(6));
    if precision == 0 {
        value.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        let micros = value.and_utc().timestamp_subsec_micros();
        let fraction = format!("{micros:06}");
        format!(
            "{}.{}",
            value.format("%Y-%m-%d %H:%M:%S"),
            &fraction[..precision]
        )
    }
}

fn timestamp_add(unit: ast::TimeUnitType, interval: i64, value: &str) -> SessionResult<String> {
    let datetime = parse_datetime(value)?;
    let added = match unit {
        ast::TimeUnitType::Microsecond => {
            datetime.checked_add_signed(chrono::Duration::microseconds(interval))
        }
        ast::TimeUnitType::Second => {
            datetime.checked_add_signed(chrono::Duration::seconds(interval))
        }
        ast::TimeUnitType::Minute => {
            datetime.checked_add_signed(chrono::Duration::minutes(interval))
        }
        ast::TimeUnitType::Hour => datetime.checked_add_signed(chrono::Duration::hours(interval)),
        ast::TimeUnitType::Day => datetime.checked_add_signed(chrono::Duration::days(interval)),
        ast::TimeUnitType::Week => datetime.checked_add_signed(chrono::Duration::weeks(interval)),
        ast::TimeUnitType::Month | ast::TimeUnitType::Quarter | ast::TimeUnitType::Year => {
            let multiplier = match unit {
                ast::TimeUnitType::Month => 1_i64,
                ast::TimeUnitType::Quarter => 3_i64,
                ast::TimeUnitType::Year => 12_i64,
                _ => unreachable!(),
            };
            let months = interval
                .checked_mul(multiplier)
                .ok_or_else(|| SessionError::new("TIMESTAMPADD month interval overflow"))?;
            let magnitude = months.unsigned_abs();
            let months = u32::try_from(magnitude)
                .map_err(|_| SessionError::new("TIMESTAMPADD month interval is too large"))?;
            if interval >= 0 {
                datetime.checked_add_months(Months::new(months))
            } else {
                datetime.checked_sub_months(Months::new(months))
            }
        }
        _ => {
            return Err(SessionError::new(format!(
                "unsupported TIMESTAMPADD unit {unit:?}"
            )));
        }
    }
    .ok_or_else(|| {
        SessionError::new(format!(
            "TIMESTAMPADD overflow for {:04}-{:02}-{:02}",
            datetime.year(),
            datetime.month(),
            datetime.day()
        ))
    })?;
    Ok(format_datetime(added, value))
}

/// 会话 KV 演示表名：仅对该表展开具体行列计划，其余表走关系表路径。
const SESSION_KV_TABLE: &str = "aster_session_kv";

/// 关系表运行时状态：表元信息与下一可用自增 ID。
#[derive(Clone, Debug)]
pub struct RelationalTableState {
    /// 表的元数据（列、索引、自增位点等）。
    pub Info: astersql_meta_model::TableInfo,
    /// 下一次可分配的自增主键值。
    pub NextAutoID: u64,
}

impl RelationalTableState {
    /// 由 `TableInfo` 构造状态，NextAutoID 取现有自增位点与 1 的较大者。
    pub fn New(info: astersql_meta_model::TableInfo) -> Self {
        let next = u64::try_from(info.AutoIncID.max(info.AutoIncIDExtra).max(1)).unwrap_or(1);
        Self {
            Info: info,
            NextAutoID: next,
        }
    }

    /// 分配自增 ID：显式值会抬高水位；否则递增并回写 `Info.AutoIncID`。
    pub fn AllocateAutoID(&mut self, explicit: Option<u64>) -> SessionResult<u64> {
        // 显式主键：只推进水位，返回调用方给定值。
        if let Some(explicit) = explicit {
            self.NextAutoID = self.NextAutoID.max(explicit.saturating_add(1));
            self.Info.AutoIncID = i64::try_from(self.NextAutoID).unwrap_or(i64::MAX);
            return Ok(explicit);
        }
        let value = self.NextAutoID;
        self.NextAutoID = self
            .NextAutoID
            .checked_add(1)
            .ok_or_else(|| SessionError::new("auto-increment allocator overflow"))?;
        self.Info.AutoIncID = i64::try_from(self.NextAutoID).unwrap_or(i64::MAX);
        Ok(value)
    }
}

/// INSERT/REPLACE 计划：目标表、行集合及冲突处理选项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsertPlan {
    /// 目标表名。
    pub Table: String,
    /// 会话 KV 表的 (k, v) 行；关系表路径可为空。
    pub Rows: Vec<(String, String)>,
    /// 是否为 REPLACE INTO。
    pub Replace: bool,
    /// 是否带 IGNORE（忽略重复键错误）。
    pub Ignore: bool,
    /// ON DUPLICATE KEY UPDATE 的列赋值表达式列表。
    pub OnDuplicate: Vec<(String, ast::ExprNode)>,
}

/// UPDATE 计划：谓词列/运算符/键值与 SET 赋值列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdatePlan {
    /// Explicit target schema; empty means the session's current database.
    pub Schema: String,
    /// 目标表名。
    pub Table: String,
    /// WHERE 谓词左侧列名。
    pub PredicateColumn: String,
    /// 比较运算符（`=`/`</`>` 等）；`*` 表示无过滤。
    pub PredicateOp: String,
    /// 谓词右侧字面量键值。
    pub Key: String,
    /// WHERE 中由 AND 连接的全部比较谓词。
    pub Predicates: Vec<(String, String, String)>,
    /// Canonical WHERE AST used by relational execution (supports IN/BETWEEN).
    pub Predicate: Option<ast::ExprNode>,
    /// SET 子句的列与表达式。
    pub Assignments: Vec<(String, ast::ExprNode)>,
    /// MySQL single-table UPDATE ordering.
    pub Order: Vec<ast::ByItem>,
    /// MySQL single-table UPDATE limit.
    pub Limit: Option<ast::Limit>,
}

/// DELETE 计划：目标表与 WHERE 比较谓词。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeletePlan {
    /// 显式目标 schema；空字符串表示使用当前数据库。
    pub Schema: String,
    /// 目标表名。
    pub Table: String,
    /// WHERE 谓词左侧列名。
    pub PredicateColumn: String,
    /// 比较运算符。
    pub PredicateOp: String,
    /// 谓词右侧字面量键值。
    pub Key: String,
    /// WHERE 中由 AND 连接的全部比较谓词。
    pub Predicates: Vec<(String, String, String)>,
    /// Canonical WHERE AST used by relational execution (supports IN/BETWEEN).
    pub Predicate: Option<ast::ExprNode>,
    /// MySQL single-table DELETE ordering.
    pub Order: Vec<ast::ByItem>,
    /// MySQL single-table DELETE limit.
    pub Limit: Option<ast::Limit>,
}

/// DML 执行报告，供 `EXPLAIN ANALYZE` 展示写键、提交与自增分配统计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DmlExecutionReport {
    /// 算子名称（如 Replace_1）。
    pub Operator: String,
    /// 涉及表名。
    pub Table: String,
    /// 影响行数。
    pub AffectedRows: u64,
    /// MySQL protocol insert ID for the statement (generated or explicit).
    pub LastInsertID: u64,
    /// 写入键数量。
    pub WriteKeys: u64,
    /// 两阶段提交（2PC）预写阶段涉及的键数。
    pub PrewriteKeys: u64,
    /// 是否已成功提交。
    pub Committed: bool,
    /// 提交等待耗时。
    pub CommitWait: Duration,
    /// 自增分配器 alloc 次数。
    pub AllocCount: u64,
    /// 自增分配器 rebase（抬升水位）次数。
    pub RebaseCount: u64,
    /// INSERT 算子从打开到完成的总耗时；非 INSERT 报告保持为零。
    pub InsertTotalTime: Duration,
    /// INSERT 检查阶段之外的准备与提交耗时。
    pub InsertPrepareTime: Duration,
    /// 重复键、唯一键和外键检查阶段的总耗时。
    pub CheckInsertTime: Duration,
    /// 检查阶段中预取现有行/键的耗时。
    pub InsertPrefetchTime: Duration,
    /// 检查阶段中外键验证与加锁的耗时。
    pub ForeignKeyCheckTime: Duration,
    /// 是否携带 INSERT 专用运行时统计。
    pub HasInsertRuntimeStats: bool,
    /// INSERT 是否执行了外键检查；用于零耗时场景仍展示 `fk_check`。
    pub HasForeignKeyChecks: bool,
}

/// 从单表 `TableRefsClause` 取出物理表名；不支持 JOIN。
fn table_name(table: &ast::TableRefsClause) -> SessionResult<&str> {
    if table.TableRefs.Right.is_some() {
        return Err(SessionError::new(
            "joins are not supported by the session KV executor",
        ));
    }
    match table.TableRefs.Left.as_deref() {
        Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none() => {
            Ok(source.Source.Name.L.as_str())
        }
        _ => Err(SessionError::new("expected one physical table")),
    }
}

/// 将 AST 常量表达式求值为会话 KV 可用的字符串字面量。
pub fn Literal(expr: &ast::ExprNode) -> SessionResult<String> {
    match &expr.Kind {
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::Null => Err(SessionError::new("NULL is not a session KV value")),
            ast::ValueDatum::Bool(value) => Ok(if *value { "1" } else { "0" }.to_owned()),
            ast::ValueDatum::Int64(value) => Ok(value.to_string()),
            ast::ValueDatum::Uint64(value) => Ok(value.to_string()),
            ast::ValueDatum::Float32(bits) => Ok(f32::from_bits(*bits).to_string()),
            ast::ValueDatum::Float64(bits) => Ok(f64::from_bits(*bits).to_string()),
            ast::ValueDatum::Decimal(value) | ast::ValueDatum::String(value) => Ok(value.clone()),
            ast::ValueDatum::Bytes(value)
            | ast::ValueDatum::BitLiteral(value)
            | ast::ValueDatum::HexLiteral(value) => String::from_utf8(value.clone())
                .map_err(|error| SessionError::new(format!("literal is not UTF-8: {error}"))),
        },
        ast::ExprKind::Parentheses(value) => Literal(value),
        // 一元正负号：先求内层字面量再加符号。
        ast::ExprKind::Unary { Op, V } if Op == "+" || Op == "-" => {
            let value = Literal(V)?;
            Ok(if Op == "-" {
                format!("-{value}")
            } else {
                value
            })
        }
        ast::ExprKind::ParamMarker { .. } => {
            Err(SessionError::new("parameter marker was not bound"))
        }
        _ => Err(SessionError::new(
            "session KV executor requires literal values",
        )),
    }
}

/// 从 WHERE 抽出比较谓词 `(列名, 运算符, 字面量)`；无 WHERE 时运算符为 `*`。
fn comparison_key(condition: Option<&ast::ExprNode>) -> SessionResult<(String, String, String)> {
    let Some(condition) = condition else {
        return Ok((String::new(), "*".to_owned(), String::new()));
    };
    let ast::ExprKind::Binary { Op, L, R } = &condition.Kind else {
        return Err(SessionError::new("DML requires WHERE k = literal"));
    };
    if !matches!(Op.as_str(), "=" | "<" | "<=" | ">" | ">=") {
        return Err(SessionError::new("DML requires a comparison predicate"));
    }
    // 允许 `col op lit`；等号还允许字面量在左侧。
    match (&L.Kind, &R.Kind) {
        (ast::ExprKind::Column(column), _) => Ok((column.Name.L.clone(), Op.clone(), Literal(R)?)),
        (_, ast::ExprKind::Column(column)) if Op == "=" => {
            Ok((column.Name.L.clone(), Op.clone(), Literal(L)?))
        }
        _ => Err(SessionError::new("DML requires WHERE column = literal")),
    }
}

/// 从 WHERE 递归抽出由 AND 连接的比较谓词。
fn comparison_keys(
    condition: Option<&ast::ExprNode>,
) -> SessionResult<Vec<(String, String, String)>> {
    let Some(condition) = condition else {
        return Ok(vec![(String::new(), "*".to_owned(), String::new())]);
    };
    if let ast::ExprKind::Binary { Op, L, R } = &condition.Kind
        && Op.eq_ignore_ascii_case("and")
    {
        let mut predicates = comparison_keys(Some(L))?;
        predicates.extend(comparison_keys(Some(R))?);
        return Ok(predicates);
    }
    comparison_key(Some(condition)).map(|predicate| vec![predicate])
}

/// 将 `InsertStmt` 规划为 `InsertPlan`；会话 KV 表校验 (k,v) 列与 VALUES 行。
pub fn PlanInsert(statement: &ast::InsertStmt) -> SessionResult<InsertPlan> {
    let table = statement
        .Table
        .as_ref()
        .ok_or_else(|| SessionError::new("INSERT has no table"))?;
    let Table = table_name(table)?.to_owned();
    if statement.Select.is_some() {
        return Err(SessionError::new(
            "INSERT SELECT requires the full executor",
        ));
    }
    // 非会话 KV 表：只携带冲突选项，行列由完整执行器处理。
    if Table != SESSION_KV_TABLE {
        return Ok(InsertPlan {
            Table,
            Rows: Vec::new(),
            Replace: statement.IsReplace,
            Ignore: statement.IgnoreErr,
            OnDuplicate: statement
                .OnDuplicate
                .iter()
                .map(|assignment| (assignment.Column.Name.L.clone(), assignment.Expr.clone()))
                .collect(),
        });
    }
    if !statement.Columns.is_empty()
        && (statement.Columns.len() != 2
            || statement.Columns[0].Name.L != "k"
            || statement.Columns[1].Name.L != "v")
    {
        return Err(SessionError::new("expected columns (k, v)"));
    }
    if statement.Lists.is_empty() {
        return Err(SessionError::new("INSERT VALUES requires at least one row"));
    }
    // 将会话 KV 的 VALUES 列表求值为 (k, v) 字符串对。
    let Rows = statement
        .Lists
        .iter()
        .map(|row| {
            if row.len() != 2 {
                return Err(SessionError::new("each session KV row needs key and value"));
            }
            Ok((Literal(&row[0])?, Literal(&row[1])?))
        })
        .collect::<SessionResult<Vec<_>>>()?;
    let OnDuplicate = statement
        .OnDuplicate
        .iter()
        .map(|assignment| (assignment.Column.Name.L.clone(), assignment.Expr.clone()))
        .collect();
    Ok(InsertPlan {
        Table,
        Rows,
        Replace: statement.IsReplace,
        Ignore: statement.IgnoreErr,
        OnDuplicate,
    })
}

/// 将 `UpdateStmt` 规划为 `UpdatePlan`；ORDER/LIMIT 交给完整执行器处理。
pub fn PlanUpdate(statement: &ast::UpdateStmt) -> SessionResult<UpdatePlan> {
    let table = statement
        .TableRefs
        .as_ref()
        .ok_or_else(|| SessionError::new("UPDATE has no table"))?;
    let Table = table_name(table)?.to_owned();
    let Schema = match table.TableRefs.Left.as_deref() {
        Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none() => {
            source.Source.Schema.L.clone()
        }
        _ => String::new(),
    };
    if statement.MultipleTable {
        return Err(SessionError::new(
            "multi-table UPDATE requires the full executor",
        ));
    }
    if statement.List.is_empty() {
        return Err(SessionError::new("UPDATE has no assignments"));
    }
    let fixed_table = Table == SESSION_KV_TABLE;
    if fixed_table && (!statement.Order.is_empty() || statement.Limit.is_some()) {
        return Err(SessionError::new(
            "ordered/limited UPDATE requires a relational table",
        ));
    }
    let Assignments = statement
        .List
        .iter()
        .map(|assignment| {
            let name = assignment.Column.Name.L.clone();
            if fixed_table && name != "k" && name != "v" {
                return Err(SessionError::new(format!(
                    "unknown session KV column {name}"
                )));
            }
            Ok((name, assignment.Expr.clone()))
        })
        .collect::<SessionResult<Vec<_>>>()?;
    let Predicates = if fixed_table {
        comparison_keys(statement.Where.as_ref())?
    } else {
        comparison_keys(statement.Where.as_ref()).unwrap_or_default()
    };
    if fixed_table && Predicates.len() != 1 {
        return Err(SessionError::new(
            "session KV executor requires one comparison predicate",
        ));
    }
    let (PredicateColumn, PredicateOp, Key) = Predicates
        .first()
        .cloned()
        .unwrap_or_else(|| (String::new(), "*".to_owned(), String::new()));
    Ok(UpdatePlan {
        Schema,
        Table,
        PredicateColumn,
        PredicateOp,
        Key,
        Predicates,
        Predicate: statement.Where.clone(),
        Assignments,
        Order: statement.Order.clone(),
        Limit: statement.Limit.clone(),
    })
}

/// 将 `DeleteStmt` 规划为 `DeletePlan`；关系表保留单表 ORDER/LIMIT。
pub fn PlanDelete(statement: &ast::DeleteStmt) -> SessionResult<DeletePlan> {
    let table = statement
        .TableRefs
        .as_ref()
        .ok_or_else(|| SessionError::new("DELETE has no table"))?;
    let Table = table_name(table)?.to_owned();
    let Schema = match table.TableRefs.Left.as_deref() {
        Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none() => {
            source.Source.Schema.L.clone()
        }
        _ => String::new(),
    };
    if statement.IsMultiTable {
        return Err(SessionError::new(
            "multi-table DELETE requires the full executor",
        ));
    }
    if Table == SESSION_KV_TABLE && (!statement.Order.is_empty() || statement.Limit.is_some()) {
        return Err(SessionError::new(
            "ordered/limited DELETE requires a relational table",
        ));
    }
    let Predicates = if Table == SESSION_KV_TABLE {
        comparison_keys(statement.Where.as_ref())?
    } else {
        comparison_keys(statement.Where.as_ref()).unwrap_or_default()
    };
    if Table == SESSION_KV_TABLE && Predicates.len() != 1 {
        return Err(SessionError::new(
            "session KV executor requires one comparison predicate",
        ));
    }
    let (PredicateColumn, PredicateOp, Key) = Predicates
        .first()
        .cloned()
        .unwrap_or_else(|| (String::new(), "*".to_owned(), String::new()));
    Ok(DeletePlan {
        Schema,
        Table,
        PredicateColumn,
        PredicateOp,
        Key,
        Predicates,
        Predicate: statement.Where.clone(),
        Order: statement.Order.clone(),
        Limit: statement.Limit.clone(),
    })
}

/// 在会话 KV 行上下文中求值 SET 赋值表达式，NULL 结果视为错误。
pub fn EvaluateAssignment(
    expression: &ast::ExprNode,
    key: &str,
    value: &str,
) -> SessionResult<String> {
    let row = HashMap::from([
        ("k".to_owned(), Some(key.to_owned())),
        ("v".to_owned(), Some(value.to_owned())),
    ]);
    EvalExpr(expression, &row, None)?
        .ok_or_else(|| SessionError::new("assignment evaluated to NULL"))
}

/// 在当前行（及可选的 INSERT 入边行）上求值表达式，供生成列与 ON DUPLICATE 使用。
pub fn EvalExpr(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    incoming: Option<&HashMap<String, Option<String>>>,
) -> SessionResult<Option<String>> {
    EvalExprWithBitColumns(expression, row, incoming, &[])
}

fn eval_arithmetic_operand(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    incoming: Option<&HashMap<String, Option<String>>>,
    bit_columns: &[String],
) -> SessionResult<Option<String>> {
    if let ast::ExprKind::Parentheses(inner) = &expression.Kind {
        return eval_arithmetic_operand(inner, row, incoming, bit_columns);
    }
    let value = EvalExprWithBitColumns(expression, row, incoming, bit_columns)?;
    // row_codec preserves BIT bytes as hex for lossless storage. Only a column
    // known to be BIT has this numeric interpretation: VARCHAR '0xFF' does not.
    if let ast::ExprKind::Column(column) = &expression.Kind
        && bit_columns.contains(&column.Name.L)
        && let Some(hex) = value.as_deref().and_then(|value| value.strip_prefix("0x"))
    {
        // Go BinaryLiteral.ToInt interprets bytes as an unsigned big-endian u64.
        let hex = hex.trim_start_matches('0');
        let numeric = if hex.is_empty() {
            0
        } else {
            u64::from_str_radix(hex, 16).map_err(|error| {
                SessionError::new(format!("invalid BIT arithmetic value: {error}"))
            })?
        };
        return Ok(Some(numeric.to_string()));
    }
    Ok(value)
}

/// Select the first CASE result without converting its typed literal to text.
/// DXF batch metadata updates must preserve bytes and exact integer task IDs.
pub(crate) fn CaseResult<'a>(
    expression: &'a ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    incoming: Option<&HashMap<String, Option<String>>>,
    bit_columns: &[String],
) -> SessionResult<Option<&'a ast::ExprNode>> {
    let ast::ExprKind::Case {
        Value,
        WhenClauses,
        ElseClause,
    } = &expression.Kind
    else {
        return Ok(Some(expression));
    };
    let case_value = Value
        .as_ref()
        .map(|value| EvalExprWithBitColumns(value, row, incoming, bit_columns))
        .transpose()?;
    for clause in WhenClauses {
        let when_value = EvalExprWithBitColumns(&clause.Expr, row, incoming, bit_columns)?;
        let matched = match case_value.as_ref() {
            Some(case_value) => case_value
                .as_deref()
                .zip(when_value.as_deref())
                .is_some_and(|(left, right)| {
                    match (Decimal::from_str(left), Decimal::from_str(right)) {
                        (Ok(left), Ok(right)) => left == right,
                        _ => left == right,
                    }
                }),
            None => when_value
                .as_deref()
                .map(|value| eval_decimal(value, "CASE condition").map(|value| !value.is_zero()))
                .transpose()?
                .unwrap_or(false),
        };
        if matched {
            return Ok(Some(&clause.Result));
        }
    }
    Ok(ElseClause.as_deref())
}

/// Evaluate a typed table-row expression without changing its string-valued uses.
/// Callers provide BIT column names from table metadata, never inferred from text.
pub fn EvalExprWithBitColumns(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    incoming: Option<&HashMap<String, Option<String>>>,
    bit_columns: &[String],
) -> SessionResult<Option<String>> {
    match &expression.Kind {
        ast::ExprKind::Value(value) if matches!(value.Datum, ast::ValueDatum::Null) => Ok(None),
        ast::ExprKind::Value(_) => Literal(expression).map(Some),
        ast::ExprKind::IntroducedValue { Value, .. } => Ok(Some(Value.clone())),
        // Generated columns may use a unary sign over an input column. Evaluate
        // the operand in the row context first so `-a` and `-NULL` follow SQL
        // arithmetic rather than the session-KV literal-only path.
        ast::ExprKind::Unary { Op, V } if Op == "+" || Op == "-" => {
            let Some(value) = eval_arithmetic_operand(V, row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let value = eval_decimal(&value, "unary arithmetic operand")?;
            let value = if Op == "-" {
                Decimal::ZERO
                    .checked_sub(value)
                    .ok_or_else(|| SessionError::new("DML unary arithmetic overflow"))?
            } else {
                value
            };
            Ok(Some(decimal_text(value)))
        }
        ast::ExprKind::Case { .. } => CaseResult(expression, row, incoming, bit_columns)?
            .map(|result| EvalExprWithBitColumns(result, row, incoming, bit_columns))
            .unwrap_or(Ok(None)),
        ast::ExprKind::Column(column) => eval_column(column, row),
        ast::ExprKind::Parentheses(inner) => {
            EvalExprWithBitColumns(inner, row, incoming, bit_columns)
        }
        // MySQL 数值算术：任一操作数为 NULL 则整体为 NULL。
        ast::ExprKind::Binary { Op, L, R }
            if matches!(Op.as_str(), "+" | "-" | "*" | "/" | "%") =>
        {
            let left = eval_arithmetic_operand(L, row, incoming, bit_columns)?;
            let right = eval_arithmetic_operand(R, row, incoming, bit_columns)?;
            let (Some(left), Some(right)) = (left, right) else {
                return Ok(None);
            };
            if Op != "/"
                && let (Ok(left_integer), Ok(right_integer)) =
                    (left.parse::<i128>(), right.parse::<i128>())
            {
                let integer_value = match Op.as_str() {
                    "+" => left_integer.checked_add(right_integer),
                    "-" => left_integer.checked_sub(right_integer),
                    "*" => left_integer.checked_mul(right_integer),
                    "%" if right_integer == 0 => return Ok(None),
                    "%" => left_integer.checked_rem(right_integer),
                    _ => unreachable!(),
                };
                if let Some(integer_value) = integer_value {
                    return Ok(Some(integer_value.to_string()));
                }
            }
            let left = eval_decimal(&left, "left arithmetic operand")?;
            let right = eval_decimal(&right, "right arithmetic operand")?;
            let value = match Op.as_str() {
                "+" => left.checked_add(right),
                "-" => left.checked_sub(right),
                "*" => left.checked_mul(right),
                "/" if right.is_zero() => {
                    return Err(SessionError::new("division by zero in DML expression"));
                }
                "/" => left.checked_div(right),
                "%" if right.is_zero() => return Ok(None),
                "%" => left.checked_rem(right),
                _ => unreachable!(),
            }
            .ok_or_else(|| SessionError::new("DML arithmetic overflow"))?;
            Ok(Some(decimal_text(value)))
        }
        ast::ExprKind::Binary { Op, L, R }
            if matches!(
                Op.as_str(),
                "=" | "==" | "!=" | "<>" | "<" | "<=" | ">" | ">="
            ) =>
        {
            let left = EvalExprWithBitColumns(L, row, incoming, bit_columns)?;
            let right = EvalExprWithBitColumns(R, row, incoming, bit_columns)?;
            let (Some(left), Some(right)) = (left, right) else {
                return Ok(None);
            };
            let ordering = match (Decimal::from_str(&left), Decimal::from_str(&right)) {
                (Ok(left), Ok(right)) => left.cmp(&right),
                _ => left.cmp(&right),
            };
            let matched = match Op.as_str() {
                "=" | "==" => ordering.is_eq(),
                "!=" | "<>" => !ordering.is_eq(),
                "<" => ordering.is_lt(),
                "<=" => ordering.is_le(),
                ">" => ordering.is_gt(),
                ">=" => ordering.is_ge(),
                _ => unreachable!(),
            };
            Ok(Some(i32::from(matched).to_string()))
        }
        ast::ExprKind::Binary { Op, L, R }
            if Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or") =>
        {
            let left = EvalExprWithBitColumns(L, row, incoming, bit_columns)?;
            let right = EvalExprWithBitColumns(R, row, incoming, bit_columns)?;
            let truth = |value: Option<String>| -> SessionResult<Option<bool>> {
                value
                    .map(|value| {
                        eval_decimal(&value, "logical operand").map(|value| !value.is_zero())
                    })
                    .transpose()
            };
            let left = truth(left)?;
            let right = truth(right)?;
            let result = if Op.eq_ignore_ascii_case("and") {
                match (left, right) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                }
            } else {
                match (left, right) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                }
            };
            Ok(result.map(|value| i32::from(value).to_string()))
        }
        // VALUES(col)：取 INSERT 入边行中对应列，仅 ON DUPLICATE KEY UPDATE 合法。
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "values" => {
            let incoming = incoming.ok_or_else(|| {
                SessionError::new("VALUES() is only valid in ON DUPLICATE KEY UPDATE")
            })?;
            let column = Args
                .first()
                .and_then(|arg| match &arg.Kind {
                    ast::ExprKind::Column(column) => Some(column.Name.L.as_str()),
                    _ => None,
                })
                .ok_or_else(|| SessionError::new("VALUES() requires a column argument"))?;
            incoming
                .get(column)
                .cloned()
                .ok_or_else(|| SessionError::new(format!("unknown VALUES column {column}")))
        }
        ast::ExprKind::Function { FnName, Args, .. }
            if matches!(
                FnName.L.as_str(),
                "now" | "current_timestamp" | "localtimestamp" | "utc_timestamp"
            ) && Args.len() <= 1 =>
        {
            let precision = Args
                .first()
                .map(|argument| {
                    EvalExprWithBitColumns(argument, row, incoming, bit_columns)?
                        .unwrap_or_default()
                        .parse::<usize>()
                        .map_err(|error| {
                            SessionError::new(format!("parse {} precision: {error}", FnName.O))
                        })
                })
                .transpose()?
                .unwrap_or(0)
                .min(6);
            let now = Utc::now();
            let mut value = now.format("%Y-%m-%d %H:%M:%S").to_string();
            if precision > 0 {
                let fractional = format!("{:06}", now.timestamp_subsec_micros());
                value.push('.');
                value.push_str(&fractional[..precision]);
            }
            Ok(Some(value))
        }
        // JSON_EXTRACT(doc, path)：解析二进制 JSON 并按路径抽取后 Unquote。
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "json_extract" => {
            if Args.len() != 2 {
                return Err(SessionError::new("JSON_EXTRACT requires document and path"));
            }
            let Some(document) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)?
            else {
                return Ok(None);
            };
            let Some(path) = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let document = astersql_types::json_functions::ParseBinaryJSONFromString(&document)
                .map_err(|error| SessionError::new(format!("invalid JSON document: {error}")))?;
            let path = astersql_types::json_functions::ParseJSONPathExpr(&path)
                .map_err(|error| SessionError::new(format!("invalid JSON path: {error}")))?;
            document
                .Extract(&[path])
                .and_then(|value| value.map(|value| value.Unquote()).transpose())
                .map_err(|error| SessionError::new(format!("extract JSON path: {error}")))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "ifnull" => {
            if Args.len() != 2 {
                return Err(SessionError::new("IFNULL requires two arguments"));
            }
            let first = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)?;
            if first.is_some() {
                Ok(first)
            } else {
                EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)
            }
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "json_merge_patch" => {
            let values = Args
                .iter()
                .map(|argument| {
                    EvalExprWithBitColumns(argument, row, incoming, bit_columns)?
                        .map(|value| {
                            astersql_types::json_functions::ParseBinaryJSONFromString(&value)
                                .map_err(|error| {
                                    SessionError::new(format!(
                                        "invalid JSON merge operand: {error}"
                                    ))
                                })
                        })
                        .transpose()
                })
                .collect::<SessionResult<Vec<_>>>()?;
            let references = values.iter().map(Option::as_ref).collect::<Vec<_>>();
            astersql_types::json_functions::MergePatchBinaryJSON(&references)
                .and_then(|value| value.map(|value| value.Unquote()).transpose())
                .map_err(|error| SessionError::new(format!("JSON_MERGE_PATCH: {error}")))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "json_unquote" => {
            if Args.len() != 1 {
                return Err(SessionError::new("JSON_UNQUOTE requires one argument"));
            }
            EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "left" => {
            if Args.len() != 2 {
                return Err(SessionError::new("LEFT requires two arguments"));
            }
            let Some(value) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let count = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)?
                .and_then(|count| count.parse::<usize>().ok())
                .ok_or_else(|| SessionError::new("LEFT length must be a nonnegative integer"))?;
            Ok(Some(value.chars().take(count).collect()))
        }
        ast::ExprKind::Function { FnName, Args, .. }
            if matches!(FnName.L.as_str(), "lower" | "upper") =>
        {
            if Args.len() != 1 {
                return Err(SessionError::new(format!(
                    "{} requires one argument",
                    FnName.O
                )));
            }
            Ok(
                EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)?.map(|value| {
                    if FnName.L == "lower" {
                        value.to_lowercase()
                    } else {
                        value.to_uppercase()
                    }
                }),
            )
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "concat" => {
            let mut result = String::new();
            for argument in Args {
                let Some(value) = EvalExprWithBitColumns(argument, row, incoming, bit_columns)?
                else {
                    return Ok(None);
                };
                result.push_str(&value);
            }
            Ok(Some(result))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "mod" => {
            if Args.len() != 2 {
                return Err(SessionError::new("MOD requires two arguments"));
            }
            let Some(left) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let Some(right) = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let left = left
                .parse::<i128>()
                .map_err(|error| SessionError::new(format!("MOD dividend: {error}")))?;
            let right = right
                .parse::<i128>()
                .map_err(|error| SessionError::new(format!("MOD divisor: {error}")))?;
            if right == 0 {
                return Ok(None);
            }
            Ok(Some((left % right).to_string()))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "lpad" => {
            if Args.len() != 3 {
                return Err(SessionError::new("LPAD requires three arguments"));
            }
            let Some(value) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let Some(length) = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let Some(padding) = EvalExprWithBitColumns(&Args[2], row, incoming, bit_columns)?
            else {
                return Ok(None);
            };
            let length = length
                .parse::<i64>()
                .map_err(|error| SessionError::new(format!("LPAD length: {error}")))?;
            if length < 0 {
                return Ok(None);
            }
            let length = usize::try_from(length)
                .map_err(|_| SessionError::new("LPAD length exceeds platform limit"))?;
            let value_chars = value.chars().collect::<Vec<_>>();
            if value_chars.len() >= length {
                return Ok(Some(value_chars.into_iter().take(length).collect()));
            }
            if padding.is_empty() {
                return Ok(Some(String::new()));
            }
            let pad_chars = padding.chars().collect::<Vec<_>>();
            let needed = length - value_chars.len();
            let mut result = String::new();
            for index in 0..needed {
                result.push(pad_chars[index % pad_chars.len()]);
            }
            result.push_str(&value);
            Ok(Some(result))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "elt" => {
            if Args.len() < 2 {
                return Err(SessionError::new("ELT requires an index and values"));
            }
            let Some(index) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let index = index
                .parse::<i64>()
                .map_err(|error| SessionError::new(format!("ELT index: {error}")))?;
            if index <= 0 {
                return Ok(None);
            }
            let index = usize::try_from(index - 1)
                .map_err(|_| SessionError::new("ELT index exceeds platform limit"))?;
            let Some(argument) = Args.get(index + 1) else {
                return Ok(None);
            };
            EvalExprWithBitColumns(argument, row, incoming, bit_columns)
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "nullif" => {
            if Args.len() != 2 {
                return Err(SessionError::new("NULLIF requires two arguments"));
            }
            let left = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)?;
            let right = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)?;
            if left
                .as_deref()
                .zip(right.as_deref())
                .is_some_and(|(left, right)| {
                    crate::runtime::relational_compare(left, right).is_eq()
                })
            {
                Ok(None)
            } else {
                Ok(left)
            }
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "timestampadd" => {
            if Args.len() != 3 {
                return Err(SessionError::new("TIMESTAMPADD requires three arguments"));
            }
            let ast::ExprKind::TimeUnit(unit) = Args[0].Kind else {
                return Err(SessionError::new(
                    "TIMESTAMPADD first argument must be a time unit",
                ));
            };
            let Some(interval) = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)?
            else {
                return Ok(None);
            };
            let Some(value) = EvalExprWithBitColumns(&Args[2], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let interval = interval
                .parse::<i64>()
                .map_err(|error| SessionError::new(format!("TIMESTAMPADD interval: {error}")))?;
            timestamp_add(unit, interval, &value).map(Some)
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "repeat" => {
            if Args.len() != 2 {
                return Err(SessionError::new("REPEAT requires two arguments"));
            }
            let Some(value) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let Some(count) = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let count = count
                .parse::<i64>()
                .map_err(|error| SessionError::new(format!("REPEAT count: {error}")))?;
            if count <= 0 {
                return Ok(Some(String::new()));
            }
            let count = usize::try_from(count)
                .map_err(|_| SessionError::new("REPEAT count exceeds platform limit"))?;
            Ok(Some(value.repeat(count)))
        }
        ast::ExprKind::Function { FnName, Args, .. }
            if matches!(FnName.L.as_str(), "length" | "octet_length") =>
        {
            if Args.len() != 1 {
                return Err(SessionError::new("LENGTH requires one argument"));
            }
            Ok(
                EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)?
                    .map(|value| value.len().to_string()),
            )
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "abs" => {
            if Args.len() != 1 {
                return Err(SessionError::new("ABS requires one argument"));
            }
            let Some(value) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let value = value
                .parse::<i64>()
                .map_err(|error| SessionError::new(format!("ABS value: {error}")))?;
            Ok(Some(value.saturating_abs().to_string()))
        }
        ast::ExprKind::Function { FnName, Args, .. }
            if matches!(FnName.L.as_str(), "mid" | "substr" | "substring") =>
        {
            if !(2..=3).contains(&Args.len()) {
                return Err(SessionError::new(
                    "MID/SUBSTR/SUBSTRING requires two or three arguments",
                ));
            }
            let Some(value) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let start = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)?
                .and_then(|start| start.parse::<i64>().ok())
                .ok_or_else(|| SessionError::new("substring start must be an integer"))?;
            let chars = value.chars().collect::<Vec<_>>();
            let offset = if start > 0 {
                usize::try_from(start - 1).unwrap_or(usize::MAX)
            } else if start < 0 {
                let distance = usize::try_from(start.unsigned_abs()).unwrap_or(usize::MAX);
                let Some(offset) = chars.len().checked_sub(distance) else {
                    return Ok(Some(String::new()));
                };
                offset
            } else {
                return Ok(Some(String::new()));
            };
            let length = if Args.len() == 3 {
                let length = EvalExprWithBitColumns(&Args[2], row, incoming, bit_columns)?
                    .and_then(|length| length.parse::<i64>().ok())
                    .ok_or_else(|| SessionError::new("substring length must be an integer"))?;
                if length <= 0 {
                    return Ok(Some(String::new()));
                }
                usize::try_from(length).unwrap_or(usize::MAX)
            } else {
                chars.len().saturating_sub(offset)
            };
            Ok(Some(
                chars
                    .into_iter()
                    .skip(offset)
                    .take(length)
                    .collect::<String>(),
            ))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "sleep" => {
            if Args.len() != 1 {
                return Err(SessionError::new("SLEEP requires one argument"));
            }
            let Some(seconds) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)?
            else {
                return Ok(None);
            };
            let seconds = seconds
                .parse::<f64>()
                .map_err(|error| SessionError::new(format!("SLEEP duration: {error}")))?;
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(SessionError::new("SLEEP duration must be nonnegative"));
            }
            std::thread::sleep(Duration::from_secs_f64(seconds));
            Ok(Some("0".to_owned()))
        }
        ast::ExprKind::Function { FnName, Args, .. } if FnName.L == "regexp_replace" => {
            if !(3..=6).contains(&Args.len()) {
                return Err(SessionError::new(
                    "REGEXP_REPLACE requires three to six arguments",
                ));
            }
            let Some(value) = EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)? else {
                return Ok(None);
            };
            let Some(pattern) = EvalExprWithBitColumns(&Args[1], row, incoming, bit_columns)?
            else {
                return Ok(None);
            };
            let Some(replacement) = EvalExprWithBitColumns(&Args[2], row, incoming, bit_columns)?
            else {
                return Ok(None);
            };
            let regex = Regex::new(&pattern)
                .map_err(|error| SessionError::new(format!("REGEXP_REPLACE pattern: {error}")))?;
            Ok(Some(
                regex.replace_all(&value, replacement.as_str()).into_owned(),
            ))
        }
        ast::ExprKind::Cast { Expr, Tp, .. } => {
            let Some(value) = EvalExprWithBitColumns(Expr, row, incoming, bit_columns)? else {
                return Ok(None);
            };
            if Tp.GetType() == astersql_parser_mysql::r#type::TypeNewDecimal {
                let scale = u32::try_from(Tp.GetDecimal().max(0)).unwrap_or(0);
                let decimal = eval_decimal(&value, "CAST AS DECIMAL")?
                    .round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero);
                Ok(Some(format!("{decimal:.scale$}", scale = scale as usize)))
            } else {
                Ok(Some(value))
            }
        }
        ast::ExprKind::Function { FnName, Args, .. }
            if matches!(FnName.L.as_str(), "vec_cosine_distance" | "vec_l2_distance")
                && Args.len() == 1 =>
        {
            // VECTOR INDEX stores a hidden generated column whose expression
            // is the one-argument distance marker. Go preserves the vector
            // payload here; the two-argument form is the scalar distance.
            EvalExprWithBitColumns(&Args[0], row, incoming, bit_columns)
        }
        ast::ExprKind::DefaultValue | ast::ExprKind::NamedDefault(_) => Ok(None),
        _ => Err(SessionError::new("unsupported relational DML expression")),
    }
}

/// 将生成列表达式字符串包装为 `SELECT` 再解析，取出首个字段表达式。
pub fn ParseGeneratedExpr(sql: &str) -> SessionResult<ast::ExprNode> {
    let mut parser = Parser::default();
    parser.SetSQLMode(astersql_parser_mysql::r#const::ModeNoBackslashEscapes);
    let (mut statements, _) = parser
        .ParseSQL(&format!("select {sql}"), &[])
        .map_err(|error| SessionError::new(format!("parse generated expression: {error}")))?;
    let statement = statements
        .pop()
        .ok_or_else(|| SessionError::new("generated expression parser returned no statement"))?;
    let select = statement
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .ok_or_else(|| SessionError::new("generated expression did not parse as SELECT"))?;
    select
        .Fields
        .Fields
        .first()
        .and_then(|field| field.Expr.clone())
        .ok_or_else(|| SessionError::new("generated expression SELECT has no field"))
}

/// 按列比较谓词判断行是否匹配；两侧均可解析为整数时走数值序，否则走字典序。
pub fn MatchesPredicate(
    row: &HashMap<String, Option<String>>,
    column: &str,
    op: &str,
    expected: &str,
) -> SessionResult<bool> {
    let Some(actual) = row.get(column).and_then(Option::as_ref) else {
        // `*` 表示无过滤：缺列也视为匹配。
        if op == "*" {
            return Ok(true);
        }
        return Ok(false);
    };
    // 两侧都能解析为整数则用数值比较，否则回退字符串比较。
    let ordering = match (actual.parse::<i128>(), expected.parse::<i128>()) {
        (Ok(actual), Ok(expected)) => actual.cmp(&expected),
        _ => actual.as_str().cmp(expected),
    };
    Ok(match op {
        "=" => ordering.is_eq(),
        "<" => ordering.is_lt(),
        "<=" => ordering.is_le(),
        ">" => ordering.is_gt(),
        ">=" => ordering.is_ge(),
        _ => {
            return Err(SessionError::new(format!(
                "unsupported predicate operator {op}"
            )));
        }
    })
}
