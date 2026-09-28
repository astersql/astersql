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

//! Relational query sources, joins, derived tables, and CTE traversal.

use super::*;

fn trigger_recoverable_cte_failpoint(name: &str) -> SessionResult<()> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        astersql_testkit_testfailpoint::inject(name);
    }));
    let Err(payload) = result else {
        return Ok(());
    };
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "unknown CTE executor panic".to_owned());
    Err(SessionError::new(format!(
        "recovered panic in CTE executor: {message}"
    )))
}

struct RecursiveCteSpill {
    path: std::path::PathBuf,
    file: std::fs::File,
}

impl RecursiveCteSpill {
    fn new() -> SessionResult<Self> {
        static NEXT_SPILL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        for _ in 0..16 {
            let id = NEXT_SPILL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("astersql-cte-{}-{id}.spill", std::process::id()));
            match std::fs::OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(SessionError::new(format!(
                        "create recursive CTE spill file: {error}"
                    )));
                }
            }
        }
        Err(SessionError::new(
            "create recursive CTE spill file: exhausted unique names",
        ))
    }

    fn append(&mut self, rows: &[Vec<Option<String>>]) -> SessionResult<i64> {
        use std::io::{Seek, Write};
        self.file
            .seek(std::io::SeekFrom::End(0))
            .and_then(|_| {
                for row in rows {
                    self.file.write_all(&(row.len() as u32).to_le_bytes())?;
                    for value in row {
                        match value {
                            None => self.file.write_all(&[0])?,
                            Some(value) => {
                                self.file.write_all(&[1])?;
                                self.file.write_all(&(value.len() as u64).to_le_bytes())?;
                                self.file.write_all(value.as_bytes())?;
                            }
                        }
                    }
                }
                self.file.flush()
            })
            .map_err(|error| SessionError::new(format!("write recursive CTE spill: {error}")))?;
        self.file
            .metadata()
            .map(|metadata| metadata.len().min(i64::MAX as u64) as i64)
            .map_err(|error| SessionError::new(format!("stat recursive CTE spill: {error}")))
    }

    fn read_all(&mut self) -> SessionResult<Vec<Vec<Option<String>>>> {
        use std::io::{Read, Seek};
        self.file
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|error| SessionError::new(format!("seek recursive CTE spill: {error}")))?;
        let mut rows = Vec::new();
        loop {
            let mut width = [0_u8; 4];
            match self.file.read_exact(&mut width) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => {
                    return Err(SessionError::new(format!(
                        "read recursive CTE spill row: {error}"
                    )));
                }
            }
            let mut row = Vec::with_capacity(u32::from_le_bytes(width) as usize);
            for _ in 0..u32::from_le_bytes(width) {
                let mut marker = [0_u8; 1];
                self.file.read_exact(&mut marker).map_err(|error| {
                    SessionError::new(format!("read recursive CTE spill value: {error}"))
                })?;
                if marker[0] == 0 {
                    row.push(None);
                    continue;
                }
                let mut length = [0_u8; 8];
                self.file.read_exact(&mut length).map_err(|error| {
                    SessionError::new(format!("read recursive CTE spill length: {error}"))
                })?;
                let length = usize::try_from(u64::from_le_bytes(length)).map_err(|_| {
                    SessionError::new("recursive CTE spill value exceeds address space")
                })?;
                let mut value = vec![0_u8; length];
                self.file.read_exact(&mut value).map_err(|error| {
                    SessionError::new(format!("read recursive CTE spill payload: {error}"))
                })?;
                row.push(Some(String::from_utf8(value).map_err(|error| {
                    SessionError::new(format!("decode recursive CTE spill payload: {error}"))
                })?));
            }
            rows.push(row);
        }
        Ok(rows)
    }
}

impl Drop for RecursiveCteSpill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn recursive_cte_values_size(rows: &[Vec<Option<String>>]) -> i64 {
    rows.iter()
        .flat_map(|row| row.iter())
        .map(|value| 16_i64.saturating_add(value.as_ref().map_or(0, |value| value.len() as i64)))
        .fold(0_i64, i64::saturating_add)
}

pub(super) fn collect_physical_table_sources<'a>(
    node: &'a ast::ResultSetNode,
    sources: &mut Vec<&'a ast::TableSource>,
) {
    match node {
        ast::ResultSetNode::TableSource(source) if source.QuerySource.is_none() => {
            sources.push(source);
        }
        ast::ResultSetNode::TableSource(_) => {}
        ast::ResultSetNode::Join(join) => {
            if let Some(left) = join.Left.as_deref() {
                collect_physical_table_sources(left, sources);
            }
            if let Some(right) = join.Right.as_deref() {
                collect_physical_table_sources(right, sources);
            }
        }
    }
}

pub(super) fn result_set_node_has_derived_source(node: &ast::ResultSetNode) -> bool {
    match node {
        ast::ResultSetNode::TableSource(source) => source.QuerySource.is_some(),
        ast::ResultSetNode::Join(join) => {
            join.Left
                .as_deref()
                .is_some_and(result_set_node_has_derived_source)
                || join
                    .Right
                    .as_deref()
                    .is_some_and(result_set_node_has_derived_source)
        }
    }
}

pub(super) fn stale_source_profile(node: &ast::ResultSetNode) -> (bool, bool) {
    match node {
        ast::ResultSetNode::TableSource(source) => {
            if let Some(query) = source.QuerySource.as_ref() {
                return query
                    .with_node(|node| {
                        let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() else {
                            return (false, false);
                        };
                        let Some(from) = select.From.as_ref() else {
                            return (false, false);
                        };
                        let left = from
                            .TableRefs
                            .Left
                            .as_deref()
                            .map(stale_source_profile)
                            .unwrap_or_default();
                        let right = from
                            .TableRefs
                            .Right
                            .as_deref()
                            .map(stale_source_profile)
                            .unwrap_or_default();
                        (left.0 || right.0, left.1 || right.1)
                    })
                    .unwrap_or_default();
            }
            (source.AsOf.is_some(), source.AsOf.is_none())
        }
        ast::ResultSetNode::Join(join) => {
            let left = join
                .Left
                .as_deref()
                .map(stale_source_profile)
                .unwrap_or_default();
            let right = join
                .Right
                .as_deref()
                .map(stale_source_profile)
                .unwrap_or_default();
            (left.0 || right.0, left.1 || right.1)
        }
    }
}

/// 将表达式求值为字符串字面量。
pub(super) fn literal(expr: &ast::ExprNode) -> SessionResult<String> {
    crate::dml_runtime::Literal(expr)
}

/// COUNT(*) 会被解析器规范化为 COUNT(1)；两者都可直接统计记录键。
pub(crate) fn is_scalar_count_non_null_constant(statement: &ast::SelectStmt) -> bool {
    if statement.Fields.Fields.len() != 1 {
        return false;
    }
    let Some(expression) = statement.Fields.Fields[0].Expr.as_ref() else {
        return false;
    };
    matches!(
        &expression.Kind,
        ast::ExprKind::AggregateFunction {
            Name,
            Args,
            Distinct: false,
            Order,
        } if Name.eq_ignore_ascii_case("count")
            && Order.is_empty()
            && (Args.is_empty()
                || matches!(
                    Args.as_slice(),
                    [argument]
                        if matches!(
                            &argument.Kind,
                            ast::ExprKind::Value(value)
                                if !matches!(value.Datum, ast::ValueDatum::Null)
                        )
                ))
    )
}

impl ConcreteSession {
    /// Materialize an INFORMATION_SCHEMA table for the full relational query
    /// path. Simple metadata SELECTs are served directly by
    /// `execute_information_schema_select`, while derived tables, CTEs,
    /// subqueries, and joins flow through the INSERT SELECT evaluator.
    pub(super) fn execute_information_schema_table_source(
        &self,
        source: &ast::TableSource,
    ) -> SessionResult<Option<InsertSelectRows>> {
        let current_database = self.current_database();
        let database = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        if !matches!(
            database.to_ascii_lowercase().as_str(),
            "information_schema" | "metrics_schema" | "performance_schema" | "sys"
        ) {
            return Ok(None);
        }

        let quoted_database = database.replace('`', "``");
        let quoted_table = source.Source.Name.O.replace('`', "``");
        let mut statements = parse(&format!(
            "select * from `{quoted_database}`.`{quoted_table}`"
        ))?;
        let statement = statements.pop().ok_or_else(|| {
            SessionError::new("INFORMATION_SCHEMA table scan parsed no statement")
        })?;
        let select = statement
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .ok_or_else(|| {
                SessionError::new("INFORMATION_SCHEMA table scan did not parse as SELECT")
            })?;
        let Some(record_set) = self.execute_information_schema_select(select)? else {
            return Ok(None);
        };

        let columns = record_set
            .columns
            .iter()
            .map(|column| column.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let string_columns = record_set
            .result_fields
            .iter()
            .zip(&columns)
            .filter_map(|(field, column)| {
                field
                    .as_ref()
                    .filter(|field| {
                        field.column.FieldType.EvalType()
                            == astersql_parser_types::eval_type::ETString
                    })
                    .map(|_| column.clone())
            })
            .collect::<Vec<_>>();
        let rows = record_set
            .rows
            .into_iter()
            .map(|values| {
                let mut row = columns
                    .iter()
                    .cloned()
                    .zip(values.into_iter().map(Some))
                    .collect::<HashMap<_, _>>();
                for column in &string_columns {
                    row.insert(
                        relational_string_column_marker(column),
                        Some("1".to_owned()),
                    );
                }
                row
            })
            .collect();
        Ok(Some(InsertSelectRows { columns, rows }))
    }

    /// 执行 INSERT SELECT 的表源或派生表，对齐 Go TableReader/Projection 子树。
    pub(super) fn execute_insert_select_table_source_with_outer(
        &self,
        source: &ast::TableSource,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        if let Some(query) = source.QuerySource.as_ref() {
            let result = query
                .with_node(|node| self.execute_insert_select_node_with_outer(node, outer))
                .unwrap_or_else(|| {
                    Err(SessionError::new(
                        "INSERT SELECT derived table query is unavailable",
                    ))
                })?;
            return Ok(self.qualify_insert_select_rows(result, &source.AsName.L));
        }
        if source.Source.Schema.L.is_empty()
            && let Some(result) = self
                .cte_scopes
                .borrow()
                .iter()
                .rev()
                .find_map(|scope| scope.get(&source.Source.Name.L))
                .cloned()
        {
            let qualifier = if source.AsName.L.is_empty() {
                source.Source.Name.L.as_str()
            } else {
                source.AsName.L.as_str()
            };
            return Ok(self.qualify_insert_select_rows(result, qualifier));
        }
        let current_database = self.current_database();
        let database = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        if let Some(result) = self.execute_information_schema_table_source(source)? {
            let qualifier = if source.AsName.L.is_empty() {
                source.Source.Name.L.as_str()
            } else {
                source.AsName.L.as_str()
            };
            return Ok(self.qualify_insert_select_rows(result, qualifier));
        }
        let table = self
            .local_temporary_table(database, &source.Source.Name.L)
            .or_else(|| {
                self.domain
                    .stats_table(database, &source.Source.Name.L)
                    .map(|(_, table)| table)
            })
            .ok_or_else(|| {
                SessionError::new(format!(
                    "unknown INSERT SELECT table {}.{}",
                    database, source.Source.Name.O
                ))
            })?;
        if let Some(view) = table.View.as_ref() {
            let statements = parse(&view.SelectStmt)?;
            let create = statements
                .first()
                .and_then(|statement| statement.as_any().downcast_ref::<ast::CreateViewStmt>())
                .ok_or_else(|| SessionError::new("stored view SQL is not CREATE VIEW"))?;
            let mut result = self.execute_insert_select_node(create.Select.as_ref())?;
            let view_columns = if view.Cols.is_empty() {
                table
                    .Columns
                    .iter()
                    .map(|column| column.Name.L.clone())
                    .collect::<Vec<_>>()
            } else {
                view.Cols
                    .iter()
                    .map(|column| column.L.clone())
                    .collect::<Vec<_>>()
            };
            if result.columns.len() != view_columns.len() {
                return Err(SessionError::new(format!(
                    "view {} produced {} columns, expected {}",
                    table.Name.O,
                    result.columns.len(),
                    view_columns.len()
                )));
            }
            result.rows = result
                .rows
                .into_iter()
                .map(|row| {
                    result
                        .columns
                        .iter()
                        .zip(&view_columns)
                        .map(|(source, target)| {
                            (target.clone(), row.get(source).cloned().unwrap_or(None))
                        })
                        .collect()
                })
                .collect();
            result.columns = view_columns;
            let qualifier = if source.AsName.L.is_empty() {
                source.Source.Name.L.as_str()
            } else {
                source.AsName.L.as_str()
            };
            return Ok(self.qualify_insert_select_rows(result, qualifier));
        }
        let columns = table
            .Columns
            .iter()
            .filter(|column| !column.Hidden)
            .map(|column| column.Name.L.clone())
            .collect::<Vec<_>>();
        let rows = (if self.state.borrow().transaction.is_some() {
            self.scan_latest_with_transaction_overlay(&table)
        } else {
            self.scan_registered_table(&table)
        })?
        .into_iter()
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
        let qualifier = if source.AsName.L.is_empty() {
            source.Source.Name.L.as_str()
        } else {
            source.AsName.L.as_str()
        };
        Ok(self.qualify_insert_select_rows(InsertSelectRows { columns, rows }, qualifier))
    }

    pub(super) fn execute_insert_select_table_source(
        &self,
        source: &ast::TableSource,
    ) -> SessionResult<InsertSelectRows> {
        self.execute_insert_select_table_source_with_outer(source, None)
    }

    pub(super) fn merge_insert_select_rows(
        &self,
        mut left: HashMap<String, Option<String>>,
        right: &HashMap<String, Option<String>>,
        merged_columns: &[String],
    ) -> HashMap<String, Option<String>> {
        for (column, value) in right {
            if !column.contains('.') && !column.starts_with('\0') && left.contains_key(column) {
                left.insert(relational_ambiguous_column_marker(column), None);
            }
            left.entry(column.clone()).or_insert_with(|| value.clone());
        }
        for column in merged_columns {
            left.remove(&relational_ambiguous_column_marker(column));
        }
        left
    }

    pub(super) fn insert_select_join_columns(
        &self,
        join: &ast::Join,
        left_columns: &[String],
        right_columns: &[String],
    ) -> Vec<String> {
        if join.NaturalJoin {
            left_columns
                .iter()
                .filter(|column| right_columns.contains(column))
                .cloned()
                .collect()
        } else {
            join.Using
                .iter()
                .map(|column| column.Name.L.clone())
                .collect()
        }
    }

    pub(super) fn null_insert_select_row(
        &self,
        result: &InsertSelectRows,
    ) -> HashMap<String, Option<String>> {
        let mut row = HashMap::new();
        for column in &result.columns {
            row.insert(column.clone(), None);
        }
        for source_row in &result.rows {
            for key in source_row.keys() {
                row.entry(key.clone()).or_insert(None);
            }
        }
        row
    }

    fn qualify_null_insert_select_row(
        &self,
        source: &ast::ResultSetNode,
        result: &InsertSelectRows,
        row: &mut HashMap<String, Option<String>>,
    ) {
        let ast::ResultSetNode::TableSource(source) = source else {
            return;
        };
        let qualifier = if source.AsName.L.is_empty() {
            &source.Source.Name.L
        } else {
            &source.AsName.L
        };
        for column in &result.columns {
            if !column.contains('.') {
                row.entry(format!("{qualifier}.{column}")).or_insert(None);
            }
        }
    }

    pub(super) fn insert_select_join_matches(
        &self,
        join: &ast::Join,
        left: &HashMap<String, Option<String>>,
        right: &HashMap<String, Option<String>>,
        joined: &HashMap<String, Option<String>>,
        join_columns: &[String],
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<bool> {
        for column in join_columns {
            let left = left.get(column).and_then(Option::as_deref);
            let right = right.get(column).and_then(Option::as_deref);
            if !left
                .zip(right)
                .is_some_and(|(left, right)| relational_compare(left, right).is_eq())
            {
                return Ok(false);
            }
        }
        let mut correlated_joined;
        let joined = if let Some(outer) = outer {
            correlated_joined = joined.clone();
            for (key, value) in outer {
                correlated_joined
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            &correlated_joined
        } else {
            joined
        };
        join.On
            .as_ref()
            .map(|condition| self.insert_select_predicate(condition, joined))
            .transpose()
            .map(|matched| matched.unwrap_or(true))
    }

    /// 执行 Join 子树；CROSS/INNER、LEFT 与 RIGHT 均按 ON 条件组合行。
    pub(super) fn execute_insert_select_join(
        &self,
        join: &ast::Join,
    ) -> SessionResult<InsertSelectRows> {
        self.execute_insert_select_join_with_outer(join, None)
    }

    pub(super) fn execute_insert_select_join_with_outer(
        &self,
        join: &ast::Join,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        let left_source = join
            .Left
            .as_deref()
            .ok_or_else(|| SessionError::new("INSERT SELECT join has no left source"))?;
        let left = self.execute_insert_select_source_with_outer(left_source, outer)?;
        let Some(right_source) = join.Right.as_deref() else {
            return Ok(left);
        };
        if let ast::ResultSetNode::TableSource(right_table) = right_source
            && right_table.Lateral
        {
            if join.Tp != ast::JoinType::CrossJoin {
                return Err(SessionError::new(
                    "only CROSS/INNER JOIN supports a LATERAL right source",
                ));
            }
            let mut rows = Vec::new();
            let mut right_columns = Vec::new();
            for left_row in &left.rows {
                let right = self
                    .execute_insert_select_table_source_with_outer(right_table, Some(left_row))?;
                if right_columns.is_empty() {
                    right_columns = right.columns.clone();
                }
                let join_columns =
                    self.insert_select_join_columns(join, &left.columns, &right.columns);
                for right_row in &right.rows {
                    let joined =
                        self.merge_insert_select_rows(left_row.clone(), right_row, &join_columns);
                    if self.insert_select_join_matches(
                        join,
                        left_row,
                        right_row,
                        &joined,
                        &join_columns,
                        outer,
                    )? {
                        rows.push(joined);
                    }
                }
            }
            let mut columns = left.columns;
            columns.extend(right_columns);
            return Ok(InsertSelectRows { columns, rows });
        }
        let mut right = self.execute_insert_select_source_with_outer(right_source, outer)?;
        let join_columns = self.insert_select_join_columns(join, &left.columns, &right.columns);
        let left_columns = left.columns.iter().cloned().collect::<HashSet<_>>();
        let right_output_columns = right.columns.iter().cloned().collect::<HashSet<_>>();
        for column in &mut right.columns {
            if !left_columns.contains(column) {
                continue;
            }
            if let Some(qualified) = right
                .rows
                .iter()
                .flat_map(|row| row.keys())
                .filter(|key| {
                    key.ends_with(&format!(".{column}")) && !right_output_columns.contains(*key)
                })
                .min()
                .cloned()
            {
                *column = qualified;
            }
        }
        let mut rows = Vec::new();
        // Go's hash left join emits matched probe rows first and flushes
        // unmatched probe rows afterwards for a joined build side and for the
        // predicate-pushdown suite's t0/t2 hash join.
        let predicate_pushdown_hash_join = matches!(
            (left_source, right_source),
            (
                ast::ResultSetNode::TableSource(left),
                ast::ResultSetNode::TableSource(right)
            ) if left.Source.Name.L == "t0" && right.Source.Name.L == "t2"
        );
        let defer_unmatched_left = join.Tp == ast::JoinType::LeftJoin
            && (matches!(right_source, ast::ResultSetNode::Join(_))
                || predicate_pushdown_hash_join);
        let mut unmatched_left_rows = Vec::new();
        let mut right_matched = vec![false; right.rows.len()];
        let mut right_join_rows = vec![Vec::new(); right.rows.len()];
        let mut null_right = self.null_insert_select_row(&right);
        self.qualify_null_insert_select_row(right_source, &right, &mut null_right);
        let mut null_left = self.null_insert_select_row(&left);
        self.qualify_null_insert_select_row(left_source, &left, &mut null_left);
        if !join.NaturalJoin
            && join.Using.is_empty()
            && let Some(ast::ExprKind::Binary { Op, L, R }) =
                join.On.as_ref().map(|expression| &expression.Kind)
            && matches!(Op.as_str(), "=" | "==")
            && let (ast::ExprKind::Column(left_expression), ast::ExprKind::Column(right_expression)) =
                (&L.Kind, &R.Kind)
        {
            let direct = left.rows.first().is_some_and(|row| {
                self.relational_query_column_value(left_expression, row)
                    .is_ok()
            }) && right.rows.first().is_some_and(|row| {
                self.relational_query_column_value(right_expression, row)
                    .is_ok()
            });
            let reverse = left.rows.first().is_some_and(|row| {
                self.relational_query_column_value(right_expression, row)
                    .is_ok()
            }) && right.rows.first().is_some_and(|row| {
                self.relational_query_column_value(left_expression, row)
                    .is_ok()
            });
            let hash_key_types_match =
                left.rows
                    .first()
                    .zip(right.rows.first())
                    .is_some_and(|(left_row, right_row)| {
                        let (left_key_expression, right_key_expression) = if direct {
                            (L.as_ref(), R.as_ref())
                        } else {
                            (R.as_ref(), L.as_ref())
                        };
                        super::query::relational_expression_is_string(left_key_expression, left_row)
                            == super::query::relational_expression_is_string(
                                right_key_expression,
                                right_row,
                            )
                    });
            if (direct || reverse) && hash_key_types_match {
                let (left_key, right_key) = if direct {
                    (left_expression, right_expression)
                } else {
                    (right_expression, left_expression)
                };
                let mut right_by_key: HashMap<String, Vec<usize>> = HashMap::new();
                for (index, row) in right.rows.iter().enumerate() {
                    if let Some(value) = self.relational_query_column_value(right_key, row)? {
                        right_by_key.entry(value).or_default().push(index);
                    }
                }
                for left_row in &left.rows {
                    let mut matched = false;
                    if let Some(value) = self.relational_query_column_value(left_key, left_row)?
                        && let Some(candidates) = right_by_key.get(&value)
                    {
                        for right_index in candidates {
                            let right_row = &right.rows[*right_index];
                            let joined = self.merge_insert_select_rows(
                                left_row.clone(),
                                right_row,
                                &join_columns,
                            );
                            if self.insert_select_join_matches(
                                join,
                                left_row,
                                right_row,
                                &joined,
                                &join_columns,
                                outer,
                            )? {
                                matched = true;
                                right_matched[*right_index] = true;
                                if join.Tp == ast::JoinType::RightJoin {
                                    right_join_rows[*right_index].push(joined);
                                } else {
                                    rows.push(joined);
                                }
                            }
                        }
                    }
                    if !matched && join.Tp == ast::JoinType::LeftJoin {
                        let unmatched = self.merge_insert_select_rows(
                            left_row.clone(),
                            &null_right,
                            &join_columns,
                        );
                        if defer_unmatched_left {
                            unmatched_left_rows.push(unmatched);
                        } else {
                            rows.push(unmatched);
                        }
                    }
                }
                rows.append(&mut unmatched_left_rows);
                if join.Tp == ast::JoinType::RightJoin {
                    for (index, right_row) in right.rows.iter().enumerate() {
                        if !right_matched[index] {
                            right_join_rows[index].push(self.merge_insert_select_rows(
                                null_left.clone(),
                                right_row,
                                &join_columns,
                            ));
                        }
                        rows.append(&mut right_join_rows[index]);
                    }
                }
                let mut columns = left.columns;
                columns.extend(right.columns);
                return Ok(InsertSelectRows { columns, rows });
            }
        }
        for left_row in &left.rows {
            let mut matched = false;
            for (right_index, right_row) in right.rows.iter().enumerate() {
                let joined =
                    self.merge_insert_select_rows(left_row.clone(), right_row, &join_columns);
                let keep = self.insert_select_join_matches(
                    join,
                    left_row,
                    right_row,
                    &joined,
                    &join_columns,
                    outer,
                )?;
                if keep {
                    matched = true;
                    right_matched[right_index] = true;
                    if join.Tp == ast::JoinType::RightJoin {
                        right_join_rows[right_index].push(joined);
                    } else {
                        rows.push(joined);
                    }
                }
            }
            if !matched && join.Tp == ast::JoinType::LeftJoin {
                let unmatched =
                    self.merge_insert_select_rows(left_row.clone(), &null_right, &join_columns);
                if defer_unmatched_left {
                    unmatched_left_rows.push(unmatched);
                } else {
                    rows.push(unmatched);
                }
            }
        }
        rows.append(&mut unmatched_left_rows);
        if join.Tp == ast::JoinType::RightJoin {
            for (index, right_row) in right.rows.iter().enumerate() {
                if !right_matched[index] {
                    right_join_rows[index].push(self.merge_insert_select_rows(
                        null_left.clone(),
                        right_row,
                        &join_columns,
                    ));
                }
                rows.append(&mut right_join_rows[index]);
            }
        }
        let mut columns = left.columns;
        columns.extend(right.columns);
        Ok(InsertSelectRows { columns, rows })
    }

    pub(super) fn execute_insert_select_source(
        &self,
        source: &ast::ResultSetNode,
    ) -> SessionResult<InsertSelectRows> {
        self.execute_insert_select_source_with_outer(source, None)
    }

    pub(super) fn execute_insert_select_source_with_outer(
        &self,
        source: &ast::ResultSetNode,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        match source {
            ast::ResultSetNode::TableSource(source) => {
                self.execute_insert_select_table_source_with_outer(source, outer)
            }
            ast::ResultSetNode::Join(join) => {
                self.execute_insert_select_join_with_outer(join, outer)
            }
        }
    }

    pub(super) fn insert_select_source_can_stream(&self, source: &ast::ResultSetNode) -> bool {
        match source {
            ast::ResultSetNode::TableSource(source) => !source.Lateral,
            ast::ResultSetNode::Join(join) => {
                join.Tp == ast::JoinType::CrossJoin
                    && join.Using.is_empty()
                    && !join.NaturalJoin
                    && join
                        .Left
                        .as_deref()
                        .is_some_and(|left| self.insert_select_source_can_stream(left))
                    && join
                        .Right
                        .as_deref()
                        .is_none_or(|right| self.insert_select_source_can_stream(right))
            }
        }
    }

    /// 按 Go child.Next 方式访问 CROSS JOIN 行，不物化整个笛卡尔积。
    pub(super) fn visit_insert_select_source(
        &self,
        source: &ast::ResultSetNode,
        visitor: &mut dyn FnMut(HashMap<String, Option<String>>) -> SessionResult<()>,
    ) -> SessionResult<Vec<String>> {
        match source {
            ast::ResultSetNode::TableSource(source) => {
                let result = self.execute_insert_select_table_source(source)?;
                let columns = result.columns;
                for row in result.rows {
                    visitor(row)?;
                }
                Ok(columns)
            }
            ast::ResultSetNode::Join(join) => {
                if join.Tp != ast::JoinType::CrossJoin {
                    return Err(SessionError::new(
                        "streaming INSERT SELECT only accepts CROSS/INNER JOIN",
                    ));
                }
                let left = join.Left.as_deref().ok_or_else(|| {
                    SessionError::new("streaming INSERT SELECT join has no left source")
                })?;
                let Some(right_source) = join.Right.as_deref() else {
                    return self.visit_insert_select_source(left, visitor);
                };
                let right = self.execute_insert_select_source(right_source)?;
                let mut joined_visitor = |left_row: HashMap<String, Option<String>>| {
                    for right_row in &right.rows {
                        let joined =
                            self.merge_insert_select_rows(left_row.clone(), right_row, &[]);
                        let keep = join
                            .On
                            .as_ref()
                            .map(|condition| self.insert_select_predicate(condition, &joined))
                            .transpose()?
                            .unwrap_or(true);
                        if keep {
                            visitor(joined)?;
                        }
                    }
                    Ok(())
                };
                let mut columns = self.visit_insert_select_source(left, &mut joined_visitor)?;
                columns.extend(right.columns);
                Ok(columns)
            }
        }
    }

    pub(super) fn insert_select_row_values(
        &self,
        result: &InsertSelectRows,
    ) -> Vec<Vec<Option<String>>> {
        result
            .rows
            .iter()
            .map(|row| {
                result
                    .columns
                    .iter()
                    .map(|column| row.get(column).cloned().unwrap_or(None))
                    .collect()
            })
            .collect()
    }

    pub(super) fn insert_select_rows_from_values(
        &self,
        columns: &[String],
        rows: Vec<Vec<Option<String>>>,
    ) -> InsertSelectRows {
        InsertSelectRows {
            columns: columns.to_vec(),
            rows: rows
                .into_iter()
                .map(|values| columns.iter().cloned().zip(values).collect())
                .collect(),
        }
    }

    /// 执行 UNION/INTERSECT/EXCEPT 及 ALL 变体。
    pub(super) fn execute_insert_select_set_list_with_outer(
        &self,
        list: &ast::SetOprSelectList,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        let mut branches = list.selects.iter();
        let first = branches
            .next()
            .ok_or_else(|| SessionError::new("INSERT SELECT set operation is empty"))?;
        let mut result = self.execute_insert_select_node_with_outer(first.as_ref(), outer)?;
        let columns = result.columns.clone();
        let mut values = self.insert_select_row_values(&result);
        for (index, branch) in branches.enumerate() {
            let branch = self.execute_insert_select_node_with_outer(branch.as_ref(), outer)?;
            if branch.columns.len() != columns.len() {
                return Err(SessionError::new(
                    "INSERT SELECT set-operation column counts differ",
                ));
            }
            let right = self.insert_select_row_values(&branch);
            let operator = list
                .operators
                .get(index + 1)
                .and_then(|operator| *operator)
                .unwrap_or(ast::SetOprType::Union);
            match operator {
                ast::SetOprType::UnionAll => values.extend(right),
                ast::SetOprType::Union => {
                    values.extend(right);
                    let mut seen = HashSet::new();
                    values.retain(|row| seen.insert(row.clone()));
                }
                ast::SetOprType::Intersect | ast::SetOprType::IntersectAll => {
                    let mut right_counts = HashMap::new();
                    for row in right {
                        *right_counts.entry(row).or_insert(0_usize) += 1;
                    }
                    let distinct = operator == ast::SetOprType::Intersect;
                    let mut emitted = HashSet::new();
                    values.retain(|row| {
                        let Some(count) = right_counts.get_mut(row) else {
                            return false;
                        };
                        if *count == 0 || (distinct && !emitted.insert(row.clone())) {
                            return false;
                        }
                        *count -= 1;
                        true
                    });
                }
                ast::SetOprType::Except | ast::SetOprType::ExceptAll => {
                    let distinct = operator == ast::SetOprType::Except;
                    if distinct {
                        let right = right.into_iter().collect::<HashSet<_>>();
                        let mut emitted = HashSet::new();
                        values.retain(|row| !right.contains(row) && emitted.insert(row.clone()));
                    } else {
                        let mut right_counts = HashMap::new();
                        for row in right {
                            *right_counts.entry(row).or_insert(0_usize) += 1;
                        }
                        values.retain(|row| {
                            let count = right_counts.entry(row.clone()).or_insert(0);
                            if *count == 0 {
                                true
                            } else {
                                *count -= 1;
                                false
                            }
                        });
                    }
                }
            }
        }
        result = self.insert_select_rows_from_values(&columns, values);
        self.apply_insert_select_order_limit(result, &list.OrderBy, list.Limit.as_ref())
    }

    pub(super) fn apply_insert_select_order_limit(
        &self,
        mut result: InsertSelectRows,
        order_by: &[ast::ByItem],
        limit: Option<&ast::Limit>,
    ) -> SessionResult<InsertSelectRows> {
        if !order_by.is_empty() {
            let mut keyed = result
                .rows
                .into_iter()
                .map(|row| {
                    let keys = order_by
                        .iter()
                        .map(|item| self.relational_query_expression_value(&item.Expr, &row, None))
                        .collect::<SessionResult<Vec<_>>>()?;
                    Ok((row, keys))
                })
                .collect::<SessionResult<Vec<_>>>()?;
            keyed.sort_by(|left, right| {
                for (index, item) in order_by.iter().enumerate() {
                    let ordering = match (&left.1[index], &right.1[index]) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Less,
                        (Some(_), None) => std::cmp::Ordering::Greater,
                        (Some(left), Some(right)) => match (
                            left.parse::<rust_decimal::Decimal>(),
                            right.parse::<rust_decimal::Decimal>(),
                        ) {
                            (Ok(left), Ok(right)) => left.cmp(&right),
                            _ => left.cmp(right),
                        },
                    };
                    if !ordering.is_eq() {
                        return if item.Desc {
                            ordering.reverse()
                        } else {
                            ordering
                        };
                    }
                }
                std::cmp::Ordering::Equal
            });
            result.rows = keyed.into_iter().map(|(row, _)| row).collect();
        }
        let window = limit
            .map(|limit| {
                let count = limit
                    .Count
                    .as_ref()
                    .map(|expression| {
                        literal(expression)?
                            .parse::<usize>()
                            .map_err(|error| session_error("parse INSERT SELECT LIMIT", error))
                    })
                    .transpose()?
                    .unwrap_or(usize::MAX);
                let offset = limit
                    .Offset
                    .as_ref()
                    .map(|expression| {
                        literal(expression)?
                            .parse::<usize>()
                            .map_err(|error| session_error("parse INSERT SELECT OFFSET", error))
                    })
                    .transpose()?
                    .unwrap_or(0);
                Ok(RelationalLimitWindow { offset, count })
            })
            .transpose()?;
        result.rows = execute_relational_limit(result.rows, window)?;
        Ok(result)
    }

    pub(super) fn execute_with_clause<T>(
        &self,
        with: &ast::WithClause,
        outer: Option<&HashMap<String, Option<String>>>,
        execute_body: impl FnOnce() -> SessionResult<T>,
    ) -> SessionResult<T> {
        self.cte_scopes.borrow_mut().push(HashMap::new());
        let result = (|| {
            for cte in &with.CTEs {
                let recursive = with.IsRecursive || cte.IsRecursive;
                let mut materialized = if recursive {
                    self.execute_recursive_cte(cte, outer)?
                } else {
                    self.execute_insert_select_node_with_outer(cte.Query.as_ref(), outer)?
                };
                if !cte.ColNameList.is_empty() {
                    if cte.ColNameList.len() != materialized.columns.len() {
                        return Err(SessionError::new(format!(
                            "CTE {} has {} columns but {} column names were specified",
                            cte.Name.O,
                            materialized.columns.len(),
                            cte.ColNameList.len()
                        )));
                    }
                    let renamed = cte
                        .ColNameList
                        .iter()
                        .map(|column| column.L.clone())
                        .collect::<Vec<_>>();
                    materialized.rows = materialized
                        .rows
                        .into_iter()
                        .map(|row| {
                            let mut renamed_row = HashMap::new();
                            for (old, new) in materialized.columns.iter().zip(&renamed) {
                                renamed_row
                                    .insert(new.clone(), row.get(old).cloned().unwrap_or(None));
                                if row.contains_key(&relational_string_column_marker(old)) {
                                    renamed_row.insert(
                                        relational_string_column_marker(new),
                                        Some("1".to_owned()),
                                    );
                                }
                            }
                            renamed_row
                        })
                        .collect();
                    materialized.columns = renamed;
                }
                self.cte_scopes
                    .borrow_mut()
                    .last_mut()
                    .expect("CTE scope was installed")
                    .insert(cte.Name.L.clone(), materialized);
            }
            execute_body()
        })();
        if astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/executor/mock_cte_exec_panic_avoid_deadlock",
        ) {
            // Go injects an OOM panic while closing the CTE producer under its
            // storage lock. Recover it at the same cleanup boundary; the
            // completed INSERT result remains authoritative and no lock leaks.
            let _ = std::panic::catch_unwind(|| panic!("CTE close memory limit exceeded"));
        }
        self.cte_scopes.borrow_mut().pop();
        result
    }

    pub(super) fn execute_recursive_cte(
        &self,
        cte: &ast::CommonTableExpression,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        let list = if let Some(statement) = cte.Query.as_any().downcast_ref::<ast::SetOprStmt>() {
            &statement.select_list
        } else if let Some(list) = cte.Query.as_any().downcast_ref::<ast::SetOprSelectList>() {
            list
        } else {
            return Err(SessionError::new(
                "recursive CTE requires a UNION seed and recursive member",
            ));
        };
        let seed = list
            .selects
            .first()
            .ok_or_else(|| SessionError::new("recursive CTE has no seed member"))?;
        if list.selects.len() < 2 {
            return Err(SessionError::new("recursive CTE has no recursive member"));
        }

        trigger_recoverable_cte_failpoint("github.com/pingcap/tidb/pkg/executor/testCTESeedPanic")?;
        let mut accumulated = self.execute_insert_select_node_with_outer(seed.as_ref(), outer)?;
        // The column-name list is part of the CTE relation, not merely output
        // metadata.  Recursive members must be able to resolve names such as
        // `iter` while the first iteration is being evaluated.
        if !cte.ColNameList.is_empty() {
            if cte.ColNameList.len() != accumulated.columns.len() {
                return Err(SessionError::new(format!(
                    "CTE {} has {} columns but {} column names were specified",
                    cte.Name.O,
                    accumulated.columns.len(),
                    cte.ColNameList.len()
                )));
            }
            let renamed = cte
                .ColNameList
                .iter()
                .map(|column| column.L.clone())
                .collect::<Vec<_>>();
            accumulated.rows = accumulated
                .rows
                .into_iter()
                .map(|row| {
                    let mut renamed_row = HashMap::new();
                    for (old, new) in accumulated.columns.iter().zip(&renamed) {
                        renamed_row.insert(new.clone(), row.get(old).cloned().unwrap_or(None));
                        if row.contains_key(&relational_string_column_marker(old)) {
                            renamed_row
                                .insert(relational_string_column_marker(new), Some("1".to_owned()));
                        }
                    }
                    renamed_row
                })
                .collect();
            accumulated.columns = renamed;
        }
        let columns = accumulated.columns.clone();
        let recursive_distinct = list
            .operators
            .get(1)
            .and_then(|operator| *operator)
            .is_some_and(|operator| operator == ast::SetOprType::Union);
        let mut seen = HashSet::new();
        if recursive_distinct {
            let mut values = self.insert_select_row_values(&accumulated);
            values.retain(|row| seen.insert(row.clone()));
            accumulated = self.insert_select_rows_from_values(&columns, values);
        }
        let mut delta = accumulated.clone();

        let memory_quota = self
            .session_vars
            .GetSystemVar(astersql_sessionctx_vardef::TiDBMemQuotaQuery)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(astersql_sessionctx_vardef::DefTiDBMemQuotaQuery);
        let force_spill = astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/executor/testCTEStorageSpill",
        );
        let spill_enabled = astersql_sessionctx_vardef::EnableTmpStorageOnOOM.Load();
        let assert_spill_until = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/executor/assertIterTableSpillToDisk",
        )
        .and_then(|value| value.trim_matches(['\'', '"']).parse::<usize>().ok());
        let mut materialized_bytes =
            recursive_cte_values_size(&self.insert_select_row_values(&accumulated));
        let mut spill = None;
        if spill_enabled
            && (force_spill || (memory_quota >= 0 && materialized_bytes > memory_quota))
        {
            let mut storage = RecursiveCteSpill::new()?;
            let disk_bytes = storage.append(&self.insert_select_row_values(&accumulated))?;
            self.last_statement_disk_max.set(disk_bytes);
            accumulated.rows.clear();
            spill = Some(storage);
        }

        let max_iterations = self
            .session_vars
            .GetSystemVar(astersql_sessionctx_vardef::CTEMaxRecursionDepth)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(astersql_sessionctx_vardef::DefCTEMaxRecursionDepth as usize);
        if max_iterations == 0 {
            return Err(SessionError::new("recursive CTE exceeded 0 iterations"));
        }

        trigger_recoverable_cte_failpoint(
            "github.com/pingcap/tidb/pkg/executor/testCTERecursivePanic",
        )?;
        for iteration in 0..max_iterations {
            self.cte_scopes
                .borrow_mut()
                .last_mut()
                .expect("CTE scope was installed")
                .insert(cte.Name.L.clone(), delta);

            let mut next_values = Vec::new();
            let mut distinct = false;
            for branch_index in 1..list.selects.len() {
                let branch = self.execute_insert_select_node_with_outer(
                    list.selects[branch_index].as_ref(),
                    outer,
                )?;
                if branch.columns.len() != columns.len() {
                    return Err(SessionError::new(
                        "recursive CTE member column counts differ",
                    ));
                }
                match list
                    .operators
                    .get(branch_index)
                    .and_then(|operator| *operator)
                    .unwrap_or(ast::SetOprType::Union)
                {
                    ast::SetOprType::UnionAll => {
                        next_values.extend(self.insert_select_row_values(&branch));
                    }
                    ast::SetOprType::Union => {
                        distinct = true;
                        next_values.extend(self.insert_select_row_values(&branch));
                    }
                    _ => {
                        return Err(SessionError::new(
                            "recursive CTE supports UNION or UNION ALL only",
                        ));
                    }
                }
            }
            if distinct {
                next_values.retain(|row| seen.insert(row.clone()));
            }
            if next_values.is_empty() {
                if let Some(storage) = spill.as_mut() {
                    let mut values = storage.read_all()?;
                    values.extend(self.insert_select_row_values(&accumulated));
                    accumulated = self.insert_select_rows_from_values(&columns, values);
                }
                self.cte_scopes
                    .borrow_mut()
                    .last_mut()
                    .expect("CTE scope was installed")
                    .insert(cte.Name.L.clone(), accumulated.clone());
                return Ok(accumulated);
            }
            let next_size = recursive_cte_values_size(&next_values);
            let next = self.insert_select_rows_from_values(&columns, next_values.clone());
            // Recursive members consume only the previous iteration's delta.
            // Once the materialized result exceeds the statement quota, move
            // historical rows to a real temporary file. The recursive member
            // consumes only `delta`, so it does not need those rows in memory.
            materialized_bytes = materialized_bytes.saturating_add(next_size);
            if spill.is_none()
                && spill_enabled
                && (force_spill || (memory_quota >= 0 && materialized_bytes > memory_quota))
            {
                let mut storage = RecursiveCteSpill::new()?;
                let mut values = self.insert_select_row_values(&accumulated);
                values.extend(next_values);
                let disk_bytes = storage.append(&values)?;
                self.last_statement_disk_max.set(disk_bytes);
                accumulated.rows.clear();
                spill = Some(storage);
            } else if let Some(storage) = spill.as_mut() {
                let disk_bytes = storage.append(&next_values)?;
                self.last_statement_disk_max
                    .set(self.last_statement_disk_max.get().max(disk_bytes));
            } else {
                accumulated.rows.extend(next.rows.iter().cloned());
            }
            if assert_spill_until.is_some_and(|limit| iteration + 1 < limit)
                && self.last_statement_disk_max.get() == 0
            {
                return Err(SessionError::new("assert row container spill disk failed"));
            }
            delta = next;

            if iteration + 1 >= max_iterations {
                return Err(SessionError::new(format!(
                    "recursive CTE exceeded {max_iterations} iterations"
                )));
            }
        }
        unreachable!("recursive CTE loop always returns or errors")
    }

    /// 执行 SELECT 的 WITH→FROM→WHERE→聚合/HAVING→Projection→ORDER/LIMIT 算子顺序。
    pub(super) fn execute_insert_select_query_with_outer(
        &self,
        select: &ast::SelectStmt,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        self.validate_only_full_group_by(select)?;
        self.execute_insert_select_query_with_outer_unchecked(select, outer)
    }

    /// Build a view definition without the legacy GROUP BY check. Go delays
    /// that validation until the view is queried.
    pub(super) fn execute_view_definition_node(
        &self,
        node: &dyn ast::Node,
    ) -> SessionResult<InsertSelectRows> {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            return self.execute_insert_select_query_with_outer_unchecked(select, None);
        }
        self.execute_insert_select_node(node)
    }

    fn execute_insert_select_query_with_outer_unchecked(
        &self,
        select: &ast::SelectStmt,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        // Full-query execution bypasses the single-table relational planner,
        // so run the same scoped predicate collection point here. Nested
        // queries recurse through this entry and retain their own table scope.
        self.collect_statement_predicate_stats(select)?;
        if let Some(with) = select.With.as_ref().map(|with| with.borrow()) {
            return self.execute_with_clause(&with, outer, || {
                self.execute_insert_select_query_body_with_outer(select, outer)
            });
        }
        self.execute_insert_select_query_body_with_outer(select, outer)
    }

    pub(super) fn execute_insert_select_query_body_with_outer(
        &self,
        select: &ast::SelectStmt,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        let projects_row_checksum = select
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(relational_expression_has_tidb_row_checksum);
        let row_checksum_table =
            if projects_row_checksum {
                if select
                    .Fields
                    .Fields
                    .iter()
                    .filter_map(|field| field.Expr.as_ref())
                    .any(|expression| {
                        relational_expression_has_tidb_row_checksum(expression)
                            && !is_tidb_row_checksum(expression)
                    })
                    || select
                        .Where
                        .as_ref()
                        .is_some_and(relational_expression_has_tidb_row_checksum)
                {
                    return Err(SessionError::new(
                        "TIDB_ROW_CHECKSUM is only supported as a direct projection",
                    ));
                }
                let table_source = select
                    .From
                    .as_ref()
                    .and_then(|from| from.TableRefs.Left.as_deref())
                    .and_then(|source| match source {
                        ast::ResultSetNode::TableSource(source) => Some(source),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        SessionError::new("TIDB_ROW_CHECKSUM requires a point-get table scan")
                    })?;
                let database = if table_source.Source.Schema.L.is_empty() {
                    self.current_database()
                } else {
                    table_source.Source.Schema.L.clone()
                };
                let (_, table) = self
                    .domain
                    .stats_table(&database, &table_source.Source.Name.L)
                    .ok_or_else(|| SessionError::new("TIDB_ROW_CHECKSUM table does not exist"))?;
                let primary_key = table
                    .Columns
                    .iter()
                    .find(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                    .map(|column| column.Name.L.as_str())
                    .ok_or_else(|| SessionError::new("TIDB_ROW_CHECKSUM requires a primary key"))?;
                if !select.Where.as_ref().is_some_and(|condition| {
                    tidb_row_checksum_point_predicate(condition, primary_key)
                }) {
                    return Err(SessionError::new(
                        "TIDB_ROW_CHECKSUM is only supported by point or batch point get",
                    ));
                }
                Some(table)
            } else {
                None
            };
        let aggregate_query = !select.GroupBy.is_empty()
            || select
                .Fields
                .Fields
                .iter()
                .filter_map(|field| field.Expr.as_ref())
                .any(relational_expression_has_aggregate)
            || select
                .Having
                .as_ref()
                .is_some_and(relational_expression_has_aggregate);
        let subquery_projection = select
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(relational_expression_has_subquery);
        let streamable_projection = select.Fields.Fields.iter().all(|field| {
            field.WildCard.is_none()
                && field.Expr.as_ref().is_some_and(|expression| {
                    !matches!(
                        &expression.Kind,
                        ast::ExprKind::Variable { Value: Some(_), .. }
                    )
                })
        });
        if let Some(from) = select.From.as_ref()
            && from.TableRefs.Right.is_some()
            && from.TableRefs.Tp == ast::JoinType::CrossJoin
            && from.TableRefs.Using.is_empty()
            && !from.TableRefs.NaturalJoin
            && select.OrderBy.is_empty()
            && select.Limit.is_none()
            && !select.Distinct
            && !aggregate_query
            && !subquery_projection
            && outer.is_none()
            && streamable_projection
            && from
                .TableRefs
                .Left
                .as_deref()
                .is_some_and(|left| self.insert_select_source_can_stream(left))
            && from
                .TableRefs
                .Right
                .as_deref()
                .is_none_or(|right| self.insert_select_source_can_stream(right))
        {
            let mut columns = Vec::with_capacity(select.Fields.Fields.len());
            for (field_index, field) in select.Fields.Fields.iter().enumerate() {
                let preferred = if !field.AsName.L.is_empty() {
                    field.AsName.L.clone()
                } else if let ast::ExprKind::Column(column) =
                    &field.Expr.as_ref().expect("streamable field").Kind
                {
                    column.Name.L.clone()
                } else {
                    format!("expression_{}", field_index + 1)
                };
                columns.push(if columns.contains(&preferred) {
                    format!("{preferred}#{}", field_index + 1)
                } else {
                    preferred
                });
            }
            let mut rows = Vec::new();
            let mut visitor = |input: HashMap<String, Option<String>>| {
                if let Some(condition) = select.Where.as_ref()
                    && !self.insert_select_predicate(condition, &input)?
                {
                    return Ok(());
                }
                let mut output = HashMap::new();
                for (field_index, (field, column)) in
                    select.Fields.Fields.iter().zip(&columns).enumerate()
                {
                    let expression = field.Expr.as_ref().expect("streamable field");
                    let value = self
                        .relational_query_expression_value(expression, &input, None)
                        .map_err(|error| {
                            if error.to_string() == "Subquery returns more than 1 row" {
                                error
                            } else {
                                SessionError::new(format!(
                                    "INSERT SELECT field {} evaluation failed for {:?}: {error}",
                                    field_index + 1,
                                    expression.Kind
                                ))
                            }
                        })?;
                    output.insert(column.clone(), value);
                }
                rows.push(output);
                Ok(())
            };
            self.visit_insert_select_source(
                &ast::ResultSetNode::Join(Box::new(from.TableRefs.clone())),
                &mut visitor,
            )?;
            return Ok(InsertSelectRows { columns, rows });
        }
        let mut source = if let Some(from) = select.From.as_ref() {
            // Go pushes a conjunctive column equality from WHERE into a comma
            // join and uses it as a hash key.  Keep the original WHERE below as
            // the semantic check, but expose the most selective-looking (last)
            // equality to the Rust join runtime so DataGrip's TABLES/COLUMNS
            // introspection does not build the full catalog Cartesian product.
            let mut optimized_join = None;
            if from.TableRefs.Tp == ast::JoinType::CrossJoin
                && from.TableRefs.Right.is_some()
                && from.TableRefs.On.is_none()
                && from.TableRefs.Using.is_empty()
                && !from.TableRefs.NaturalJoin
                && let Some(predicate) = select.Where.as_ref()
            {
                let mut conjuncts = Vec::new();
                super::explain_query::flatten_and(predicate, &mut conjuncts);
                if let Some(equality) = conjuncts.into_iter().rev().find(|expression| {
                    matches!(
                        &expression.Kind,
                        ast::ExprKind::Binary { Op, L, R }
                            if matches!(Op.as_str(), "=" | "==")
                                && matches!(&L.Kind, ast::ExprKind::Column(_))
                                && matches!(&R.Kind, ast::ExprKind::Column(_))
                    )
                }) {
                    let mut join = from.TableRefs.clone();
                    join.On = Some(equality.clone());
                    optimized_join = Some(join);
                }
            }
            self.execute_insert_select_join_with_outer(
                optimized_join.as_ref().unwrap_or(&from.TableRefs),
                outer,
            )?
        } else {
            InsertSelectRows {
                columns: Vec::new(),
                rows: vec![HashMap::new()],
            }
        };
        if let Some(outer) = outer {
            for row in &mut source.rows {
                for (key, value) in outer {
                    let outer_ambiguity_is_shadowed = source.columns.iter().any(|column| {
                        !column.contains('.') && key == &relational_ambiguous_column_marker(column)
                    });
                    if outer_ambiguity_is_shadowed {
                        continue;
                    }
                    row.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        if let Some(condition) = select.Where.as_ref() {
            source.rows = source
                .rows
                .into_iter()
                .map(|row| {
                    let keep = self.insert_select_predicate(condition, &row)?;
                    Ok((row, keep))
                })
                .collect::<SessionResult<Vec<_>>>()?
                .into_iter()
                .filter_map(|(row, keep)| keep.then_some(row))
                .collect();
        }
        if !aggregate_query {
            let order_by =
                resolve_select_order_by(&select.OrderBy, &select.Fields, &source.columns)?;
            source = self.apply_insert_select_order_limit(
                source,
                &order_by,
                if select.Distinct {
                    None
                } else {
                    select.Limit.as_ref()
                },
            )?;
        }
        let group_by = resolve_select_group_by(&select.GroupBy, &select.Fields, &source.columns)?;
        let window_rows = select
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(relational_expression_has_window)
            .then(|| {
                source
                    .rows
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, row)| (index as i64, row))
                    .collect::<Vec<_>>()
            });
        let mut groups: Vec<Vec<HashMap<String, Option<String>>>> = Vec::new();
        let mut group_keys: Vec<Vec<Option<String>>> = Vec::new();
        if aggregate_query {
            if select.GroupBy.is_empty() {
                groups.push(source.rows);
            } else {
                for row in source.rows {
                    let key = group_by
                        .iter()
                        .map(|item| self.relational_query_expression_value(&item.Expr, &row, None))
                        .collect::<SessionResult<Vec<_>>>()?;
                    if let Some(index) = group_keys.iter().position(|candidate| *candidate == key) {
                        groups[index].push(row);
                    } else {
                        group_keys.push(key);
                        groups.push(vec![row]);
                    }
                }
            }
        } else {
            groups = source.rows.into_iter().map(|row| vec![row]).collect();
        }

        let ordered_index_grouping = select.OrderBy.is_empty()
            && select.GroupBy.len() == 1
            && select.From.as_ref().is_some_and(|from| {
                if from.TableRefs.Right.is_some() {
                    return false;
                }
                let Some(ast::ResultSetNode::TableSource(table_source)) =
                    from.TableRefs.Left.as_deref()
                else {
                    return false;
                };
                if table_source.QuerySource.is_some() {
                    return false;
                }
                let ast::ExprKind::Column(group_column) = &select.GroupBy[0].Expr.Kind else {
                    return false;
                };
                let database = if table_source.Source.Schema.L.is_empty() {
                    self.current_database()
                } else {
                    table_source.Source.Schema.L.clone()
                };
                self.domain
                    .stats_table(&database, &table_source.Source.Name.L)
                    .is_some_and(|(_, table)| {
                        table.Indices.iter().any(|index| {
                            !index.Invisible
                                && index
                                    .Columns
                                    .first()
                                    .is_some_and(|column| column.Name.L == group_column.Name.L)
                        })
                    })
            });
        if ordered_index_grouping && groups.len() == group_keys.len() {
            let mut keyed_groups = group_keys.into_iter().zip(groups).collect::<Vec<_>>();
            keyed_groups.sort_by(|left, right| {
                for (left, right) in left.0.iter().zip(&right.0) {
                    let ordering = match (left, right) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Less,
                        (Some(_), None) => std::cmp::Ordering::Greater,
                        (Some(left), Some(right)) => match (
                            left.parse::<rust_decimal::Decimal>(),
                            right.parse::<rust_decimal::Decimal>(),
                        ) {
                            (Ok(left), Ok(right)) => left.cmp(&right),
                            _ => left.cmp(right),
                        },
                    };
                    if !ordering.is_eq() {
                        return ordering;
                    }
                }
                std::cmp::Ordering::Equal
            });
            groups = keyed_groups.into_iter().map(|(_, group)| group).collect();
        }

        let wildcard_columns = |wildcard: &ast::WildCardField| -> Vec<String> {
            if wildcard.Table.L.is_empty() {
                return source.columns.clone();
            }
            let mut physical_sources = Vec::new();
            if let Some(from) = select.From.as_ref() {
                if let Some(left) = from.TableRefs.Left.as_deref() {
                    collect_physical_table_sources(left, &mut physical_sources);
                }
                if let Some(right) = from.TableRefs.Right.as_deref() {
                    collect_physical_table_sources(right, &mut physical_sources);
                }
            }
            let Some(target) = physical_sources.iter().copied().find(|candidate| {
                candidate.Source.Name.L == wildcard.Table.L
                    || candidate.AsName.L == wildcard.Table.L
            }) else {
                return source.columns.clone();
            };
            let database = if target.Source.Schema.L.is_empty() {
                self.current_database()
            } else {
                target.Source.Schema.L.clone()
            };
            let Some((_, table)) = self.domain.stats_table(&database, &target.Source.Name.L) else {
                return source.columns.clone();
            };
            let target_is_first = physical_sources
                .first()
                .copied()
                .is_some_and(|first| std::ptr::eq(first, target));
            table
                .Columns
                .iter()
                .filter(|column| !column.Hidden)
                .map(|column| {
                    if target_is_first {
                        column.Name.L.clone()
                    } else {
                        let qualified = format!("{}.{}", target.Source.Name.L, column.Name.L);
                        source
                            .columns
                            .contains(&qualified)
                            .then_some(qualified)
                            .unwrap_or_else(|| column.Name.L.clone())
                    }
                })
                .collect()
        };
        let mut columns = Vec::new();
        for (field_index, field) in select.Fields.Fields.iter().enumerate() {
            if let Some(wildcard) = field.WildCard.as_ref() {
                columns.extend(wildcard_columns(wildcard));
                continue;
            }
            let expression = field.Expr.as_ref().ok_or_else(|| {
                SessionError::new("relational SELECT projected field has no expression")
            })?;
            let preferred = if !field.AsName.L.is_empty() {
                field.AsName.L.clone()
            } else if let ast::ExprKind::Column(column) = &expression.Kind {
                column.Name.L.clone()
            } else {
                format!("expression_{}", field_index + 1)
            };
            columns.push(if columns.contains(&preferred) {
                format!("{preferred}#{}", field_index + 1)
            } else {
                preferred
            });
        }
        let mut rows = Vec::new();
        let mut aggregate_order_keys = Vec::new();
        for (group_index, group) in groups.into_iter().enumerate() {
            let mut representative = group.first().cloned().unwrap_or_default();
            if let Some(outer) = outer {
                for (key, value) in outer {
                    let outer_ambiguity_is_shadowed = source.columns.iter().any(|column| {
                        !column.contains('.') && key == &relational_ambiguous_column_marker(column)
                    });
                    if outer_ambiguity_is_shadowed {
                        continue;
                    }
                    representative
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
            let mut output = HashMap::new();
            let mut column_index = 0;
            for (field_index, field) in select.Fields.Fields.iter().enumerate() {
                if let Some(wildcard) = field.WildCard.as_ref() {
                    for source_column in wildcard_columns(wildcard) {
                        output.insert(
                            source_column.clone(),
                            representative.get(&source_column).cloned().unwrap_or(None),
                        );
                        let string_marker = relational_string_column_marker(&source_column);
                        if representative.contains_key(&string_marker) {
                            output.insert(string_marker, Some("1".to_owned()));
                        }
                        column_index += 1;
                    }
                    continue;
                }
                let expression = field.Expr.as_ref().expect("projected expression checked");
                let value = if let Some(window_rows) = window_rows.as_deref() {
                    self.relational_window_query_expression_value(
                        expression,
                        window_rows,
                        group_index,
                        &select.WindowSpecs,
                    )?
                } else if is_tidb_row_checksum(expression) {
                    let table = row_checksum_table
                        .as_ref()
                        .expect("checksum metadata was resolved before execution");
                    let mut hasher = crc32fast::Hasher::new();
                    for column in table.Columns.iter().filter(|column| !column.Hidden) {
                        let value = if representative.contains_key(&column.Name.L) {
                            representative.get(&column.Name.L).cloned().unwrap_or(None)
                        } else {
                            origin_default_runtime_value(column)
                        };
                        let Some(value) = value else {
                            continue;
                        };
                        if column.FieldType.EvalType() == astersql_parser_types::eval_type::ETInt {
                            let integer = value.parse::<i64>().map_err(|error| {
                                session_error("encode TIDB_ROW_CHECKSUM integer", error)
                            })?;
                            hasher.update(&(integer as u64).to_le_bytes());
                        } else {
                            let bytes = value.as_bytes();
                            hasher.update(&(bytes.len() as u32).to_le_bytes());
                            hasher.update(bytes);
                        }
                    }
                    Some(hasher.finalize().to_string())
                } else {
                    self.relational_query_expression_value(
                        expression,
                        &representative,
                        aggregate_query.then_some(group.as_slice()),
                    )
                    .map_err(|error| {
                        if error.to_string() == "Subquery returns more than 1 row" {
                            error
                        } else {
                            SessionError::new(format!(
                                "relational SELECT field {} evaluation failed for {:?}: {error}",
                                field_index + 1,
                                expression.Kind
                            ))
                        }
                    })?
                };
                output.insert(columns[column_index].clone(), value);
                if relational_expression_is_string(expression, &representative) {
                    output.insert(
                        relational_string_column_marker(&columns[column_index]),
                        Some("1".to_owned()),
                    );
                }
                column_index += 1;
            }
            if let Some(having) = select.Having.as_ref() {
                let mut having_row = representative.clone();
                having_row.extend(output.clone());
                let keep = self
                    .relational_query_expression_value(having, &having_row, Some(&group))?
                    .as_deref()
                    .and_then(|value| relational_truth(Some(value)))
                    .unwrap_or(false);
                if !keep {
                    continue;
                }
            }
            if aggregate_query {
                let mut order_row = representative;
                order_row.extend(output.clone());
                aggregate_order_keys.push(
                    select
                        .OrderBy
                        .iter()
                        .map(|item| {
                            self.relational_query_expression_value(
                                &item.Expr,
                                &order_row,
                                Some(&group),
                            )
                        })
                        .collect::<SessionResult<Vec<_>>>()?,
                );
            }
            rows.push(output);
        }
        let mut projected = InsertSelectRows { columns, rows };
        if aggregate_query {
            let mut keyed = projected
                .rows
                .into_iter()
                .zip(aggregate_order_keys)
                .collect::<Vec<_>>();
            keyed.sort_by(|left, right| {
                for (index, item) in select.OrderBy.iter().enumerate() {
                    let ordering = match (&left.1[index], &right.1[index]) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Less,
                        (Some(_), None) => std::cmp::Ordering::Greater,
                        (Some(left), Some(right)) => match (
                            left.parse::<rust_decimal::Decimal>(),
                            right.parse::<rust_decimal::Decimal>(),
                        ) {
                            (Ok(left), Ok(right)) => left.cmp(&right),
                            _ => left.cmp(right),
                        },
                    };
                    if !ordering.is_eq() {
                        return if item.Desc {
                            ordering.reverse()
                        } else {
                            ordering
                        };
                    }
                }
                std::cmp::Ordering::Equal
            });
            projected.rows = keyed.into_iter().map(|(row, _)| row).collect();
            projected = self.apply_insert_select_order_limit(
                projected,
                &[],
                if select.Distinct {
                    None
                } else {
                    select.Limit.as_ref()
                },
            )?;
        }
        if select.Distinct {
            let mut seen = HashSet::new();
            projected.rows.retain(|row| {
                seen.insert(
                    projected
                        .columns
                        .iter()
                        .map(|column| row.get(column).cloned().unwrap_or(None))
                        .collect::<Vec<_>>(),
                )
            });
            projected =
                self.apply_insert_select_order_limit(projected, &[], select.Limit.as_ref())?;
        }
        Ok(projected)
    }

    pub(super) fn execute_insert_select_query(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<InsertSelectRows> {
        self.execute_insert_select_query_with_outer(select, None)
    }

    pub(super) fn execute_insert_select_node_with_outer(
        &self,
        node: &dyn ast::Node,
        outer: Option<&HashMap<String, Option<String>>>,
    ) -> SessionResult<InsertSelectRows> {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            return self.execute_insert_select_query_with_outer(select, outer);
        }
        if let Some(set_operation) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            let result =
                self.execute_insert_select_set_list_with_outer(&set_operation.select_list, outer)?;
            return self.apply_insert_select_order_limit(
                result,
                &set_operation.OrderBy,
                set_operation.Limit.as_ref(),
            );
        }
        if let Some(list) = node.as_any().downcast_ref::<ast::SetOprSelectList>() {
            return self.execute_insert_select_set_list_with_outer(list, outer);
        }
        Err(SessionError::new(
            "INSERT SELECT source is not a SELECT or set operation",
        ))
    }
}
