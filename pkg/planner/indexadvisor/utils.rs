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

//! SQL utilities used by the index advisor.
//!
//! The production TiDB implementation uses the parser AST.  This crate is
//! deliberately dependency-light on non-Windows targets, so the same visitor
//! boundaries are implemented with a small SQL lexer.  The lexer is careful to
//! preserve the Go package's table/order/predicate rules rather than treating
//! every identifier in a statement as an index candidate.
//!
//! 索引顾问的 SQL 辅助层：负责语句校验与归一化、表名和候选列提取、查询过滤，
//! 以及候选索引集合的代价汇总。为保持非 Windows 目标的依赖轻量，本实现以小型词法器
//! 模拟 Go 版 AST 访问器的边界，只把谓词、排序和分组中的列视为索引候选。

use crate::model::{Column, Index, IndexSetCost, Query};
use crate::optimizer::{FieldType, Optimizer};
use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};

/// Parse one statement and reject empty, unsupported, or obviously malformed SQL.
/// 解析单条语句，并拒绝空语句、不支持的语句类型及明显不匹配的定界符。
pub fn parse_one_sql(sql: &str) -> Result<String, String> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("empty SQL".to_string());
    }
    let tokens = tokenize(sql);
    let head = tokens
        .iter()
        .find(|token| !token.is_empty())
        .map(String::as_str)
        .unwrap_or_default();
    if !matches!(
        head,
        "select"
            | "update"
            | "delete"
            | "insert"
            | "replace"
            | "with"
            | "create"
            | "drop"
            | "alter"
            | "truncate"
            | "rename"
            | "analyze"
            | "explain"
            | "show"
            | "set"
            | "use"
            | "begin"
            | "commit"
            | "rollback"
            | "admin"
    ) {
        return Err(format!("unsupported statement type {head}"));
    }
    if !balanced_delimiters(sql) {
        return Err("malformed SQL delimiters".to_string());
    }
    Ok(sql.to_string())
}

/// Normalize literals/comments similarly to parser.NormalizeDigest.
/// 按照 `parser.NormalizeDigest` 的用途移除注释、折叠空白并将字面量替换为占位符。
pub fn normalize_digest(sql: &str) -> (String, String) {
    let mut normalized = String::new();
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            normalized.push(' ');
        } else if ch == '-' && chars.peek() == Some(&'-') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '\n' {
                    break;
                }
            }
            normalized.push(' ');
        } else if ch == '\'' || ch == '"' {
            let quote = ch;
            while let Some(next) = chars.next() {
                if next == '\\' {
                    chars.next();
                } else if next == quote {
                    break;
                }
            }
            normalized.push('?');
        } else if ch.is_ascii_digit()
            && !normalized
                .chars()
                .last()
                .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_')
        {
            while chars
                .peek()
                .is_some_and(|next| next.is_ascii_digit() || *next == '.')
            {
                chars.next();
            }
            normalized.push('?');
        } else if ch.is_whitespace() {
            if !normalized.ends_with(' ') {
                normalized.push(' ');
            }
        } else {
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    let normalized = normalized.trim().to_string();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    (normalized, format!("{:016x}", hasher.finish()))
}

/// Return referenced tables in AST traversal order, including nested queries.
/// 按遍历顺序收集嵌套查询中的表；无 schema 的表使用默认 schema，CTE 名不计入实体表。
pub fn collect_table_names(default_schema: &str, sql: &str) -> Result<Vec<String>, String> {
    parse_one_sql(sql)?;
    let tokens = tokenize(sql);
    let ctes = cte_names(&tokens, default_schema);
    let default_schema = default_schema.to_ascii_lowercase();
    let mut names = Vec::new();
    let mut expect_table = false;
    let mut in_from_list = false;
    for (index, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "from" => {
                expect_table = true;
                in_from_list = true;
            }
            "join" | "update" | "into" => expect_table = true,
            "," if in_from_list => expect_table = true,
            "on" | "where" | "group" | "order" | "having" | "limit" | "union" | "set"
            | "values" | "returning" | "for" => {
                expect_table = false;
                in_from_list = false;
            }
            "(" if expect_table => {}
            _ if expect_table && is_identifier(token) => {
                let name = normalize_identifier(token);
                let qualified = if name.contains('.') {
                    name
                } else {
                    format!("{default_schema}.{name}")
                };
                if !ctes.contains(&qualified) {
                    names.push(qualified);
                }
                expect_table = false;
            }
            _ => {}
        }
        // A comma in an ON/WHERE expression must not begin another table.
        if token == "on" || token == "where" {
            in_from_list = false;
        }
        let _ = index;
    }
    Ok(names)
}

/// Collect columns below SELECT fields for the single-table Go-supported case.
/// 收集单表查询的 SELECT 字段列；多表查询沿用 Go 版约束，返回空集合。
pub fn collect_select_columns(query: &Query) -> Result<BTreeSet<Column>, String> {
    let tables = collect_table_names(&query.schema_name, &query.text)?;
    if tables.len() != 1 {
        return Ok(BTreeSet::new());
    }
    let (schema, table) = split_table(&tables[0])?;
    let tokens = tokenize(&query.text);
    let Some(from) = tokens.iter().position(|token| token == "from") else {
        return Ok(BTreeSet::new());
    };
    let mut result = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate().take(from).skip(1) {
        if !is_identifier(token) || is_sql_keyword(token) || token == "*" {
            continue;
        }
        if index
            .checked_sub(1)
            .and_then(|previous| tokens.get(previous))
            .is_some_and(|previous| previous == "as")
            || (tokens
                .get(index + 1)
                .is_none_or(|next| next == "," || next == "from")
                && index
                    .checked_sub(1)
                    .and_then(|previous| tokens.get(previous))
                    .is_some_and(|previous| {
                        previous == ")" || is_identifier(previous) && previous != "select"
                    }))
        {
            continue;
        }
        if tokens.get(index + 1).is_some_and(|next| next == "(") {
            continue;
        }
        let column = normalize_identifier(token)
            .split('.')
            .next_back()
            .unwrap_or_default()
            .to_string();
        if !column.is_empty() {
            result.insert(Column::new(schema, table, &column));
        }
    }
    Ok(result)
}

/// Collect simple ORDER BY columns.  Any expression makes the result empty,
/// matching the Go visitor's unsupported-expression branch.
/// 仅收集由普通列组成的 ORDER BY；出现表达式时整体返回空结果，与 Go 访问器一致。
pub fn collect_order_by_columns(query: &Query) -> Result<Vec<Column>, String> {
    let tables = collect_table_names(&query.schema_name, &query.text)?;
    if tables.len() != 1 {
        return Ok(Vec::new());
    }
    let (schema, table) = split_table(&tables[0])?;
    let tokens = tokenize(&query.text);
    let Some(order) = tokens.iter().position(|token| token == "order") else {
        return Ok(Vec::new());
    };
    if tokens.get(order + 1).map(String::as_str) != Some("by") {
        return Ok(Vec::new());
    }
    let end = clause_end(&tokens, order + 2);
    let expression = &tokens[order + 2..end];
    let mut result = Vec::new();
    let mut expect_column = true;
    for token in expression {
        if token == "," {
            expect_column = true;
            continue;
        }
        if expect_column && is_identifier(token) {
            result.push(Column::new(
                schema,
                table,
                normalize_identifier(token)
                    .split('.')
                    .next_back()
                    .unwrap_or_default(),
            ));
            expect_column = false;
        } else if token != "asc" && token != "desc" {
            return Ok(Vec::new());
        }
    }
    Ok(result)
}

/// Collect columns from OR branches consisting only of `column = constant`.
/// 从纯 `列 = 常量` 的 OR 分支中收集 DNF 列；任一分支形状不符便放弃该结果。
pub fn collect_dnf_columns(query: &Query) -> Result<BTreeSet<Column>, String> {
    let tables = collect_table_names(&query.schema_name, &query.text)?;
    if tables.len() != 1 {
        return Ok(BTreeSet::new());
    }
    let (schema, table) = split_table(&tables[0])?;
    let tokens = tokenize(&query.text);
    let Some(where_index) = tokens.iter().position(|token| token == "where") else {
        return Ok(BTreeSet::new());
    };
    let end = clause_end(&tokens, where_index + 1);
    let predicate = &tokens[where_index + 1..end];
    if !predicate.iter().any(|token| token == "or") {
        return Ok(BTreeSet::new());
    }
    for cnf_term in split_boolean_terms(predicate, "and") {
        let dnf_terms = split_boolean_terms(trim_parentheses(cnf_term), "or");
        if dnf_terms.len() <= 1 {
            continue;
        }
        let mut result = BTreeSet::new();
        for dnf_term in dnf_terms {
            let term = trim_parentheses(dnf_term);
            if term.len() != 3 || term[1] != "=" {
                result.clear();
                break;
            }
            let column = if is_identifier(&term[0]) && is_constant(&term[2]) {
                &term[0]
            } else if is_constant(&term[0]) && is_identifier(&term[2]) {
                &term[2]
            } else {
                result.clear();
                break;
            };
            result.insert(Column::new(
                schema,
                table,
                normalize_identifier(column)
                    .split('.')
                    .next_back()
                    .unwrap_or_default(),
            ));
        }
        if !result.is_empty() {
            return Ok(result);
        }
    }
    Ok(BTreeSet::new())
}

fn trim_parentheses(mut tokens: &[String]) -> &[String] {
    while tokens.first().is_some_and(|token| token == "(")
        && tokens.last().is_some_and(|token| token == ")")
        && matching_outer_parentheses(tokens)
    {
        tokens = &tokens[1..tokens.len() - 1];
    }
    tokens
}

fn matching_outer_parentheses(tokens: &[String]) -> bool {
    let mut depth = 0_i32;
    for (index, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 && index + 1 != tokens.len() {
            return false;
        }
    }
    depth == 0
}

fn split_boolean_terms<'a>(tokens: &'a [String], operator: &str) -> Vec<&'a [String]> {
    let mut terms = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ if depth == 0 && token == operator => {
                terms.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    terms.push(&tokens[start..]);
    terms
}

/// Restore the default schema in the query metadata and unqualified table refs.
/// 为查询元数据及未限定表名补上默认 schema；可按调用方要求跳过无法解析的语句。
pub fn restore_schema_name(
    default_schema: &str,
    queries: BTreeSet<Query>,
    ignore_error: bool,
) -> Result<BTreeSet<Query>, String> {
    let mut result = BTreeSet::new();
    for mut query in queries {
        if query.schema_name.is_empty() {
            query.schema_name = default_schema.to_ascii_lowercase();
        }
        if !balanced_delimiters(&query.text) {
            if ignore_error {
                continue;
            }
            return Err(format!(
                "invalid query: {}, err: malformed SQL delimiters",
                query.text
            ));
        }
        if !parse_one_sql(&query.text).is_ok() {
            if ignore_error {
                continue;
            }
            return Err(format!("invalid query: {}", query.text));
        }
        query.text = qualify_table_references(&query.text, &query.schema_name);
        result.insert(query);
    }
    Ok(result)
}

/// Remove queries whose plan cannot be obtained; optionally propagate the first error.
/// 过滤无法取得执行计划的查询；未启用忽略错误时返回遇到的首个错误。
pub fn filter_invalid_queries(
    optimizer: &dyn Optimizer,
    queries: BTreeSet<Query>,
    ignore_error: bool,
) -> Result<BTreeSet<Query>, String> {
    let mut result = BTreeSet::new();
    for query in queries {
        match optimizer.query_plan_cost(&query.text, &[]) {
            Ok(_) => {
                result.insert(query);
            }
            Err(error) if ignore_error => {}
            Err(error) => return Err(error),
        }
    }
    Ok(result)
}

/// Remove system-schema and no-table statements, as FilterSQLAccessingSystemTables does.
/// 排除访问系统 schema 的查询和不引用表的语句，与 Go 的系统表过滤规则保持一致。
pub fn filter_system_queries(
    queries: BTreeSet<Query>,
    ignore_error: bool,
) -> Result<BTreeSet<Query>, String> {
    let mut result = BTreeSet::new();
    for query in queries {
        match collect_table_names(&query.schema_name, &query.text) {
            Ok(tables) if tables.is_empty() => {}
            Ok(tables)
                if tables.iter().all(|table| {
                    !is_system_schema(table.split('.').next().unwrap_or_default())
                }) =>
            {
                result.insert(query);
            }
            Ok(_) => {}
            Err(_) if ignore_error => {}
            Err(error) => return Err(error),
        }
    }
    Ok(result)
}

/// Collect all indexable columns across a workload.
/// 汇总整个查询负载中所有可建立索引的列。
pub fn collect_indexable_columns_for_query_set(
    optimizer: &dyn Optimizer,
    queries: &BTreeSet<Query>,
) -> Result<BTreeSet<Column>, String> {
    let mut result = BTreeSet::new();
    for query in queries {
        result.extend(collect_indexable_columns(query, optimizer)?);
    }
    Ok(result)
}

/// Collect columns used in predicates, IN/BETWEEN, ORDER BY, or GROUP BY.
/// 收集范围谓词、IN/BETWEEN、ORDER BY 与 GROUP BY 中的候选列。
/// 无 schema 限定的列会在查询涉及的各 schema 中通过优化器消歧，并排除不支持索引的类型。
pub fn collect_indexable_columns(
    query: &Query,
    optimizer: &dyn Optimizer,
) -> Result<BTreeSet<Column>, String> {
    let tables = collect_table_names(&query.schema_name, &query.text)?;
    let tokens = tokenize(&query.text);
    let relevant = relevant_column_names(&tokens);
    let mut columns = BTreeSet::new();
    let mut schemas = BTreeSet::from([query.schema_name.to_ascii_lowercase()]);
    schemas.extend(
        tables
            .iter()
            .filter_map(|table| table.split_once('.').map(|(schema, _)| schema.to_string())),
    );
    for name in relevant {
        let name = normalize_identifier(&name);
        let (qualified_schema, column_name) = name
            .split_once('.')
            .map_or((None, name.as_str()), |(schema, column)| {
                (Some(schema), column)
            });
        let schema_names = qualified_schema
            .map(|schema| vec![schema.to_string()])
            .unwrap_or_else(|| schemas.iter().cloned().collect());
        for schema in schema_names {
            let Ok(possible) = optimizer.possible_columns(&schema, column_name) else {
                continue;
            };
            for candidate in possible {
                if let Ok(field_type) = optimizer.column_type(&candidate) {
                    if is_indexable_column_type(&field_type) {
                        columns.insert(candidate);
                    }
                }
            }
        }
    }
    Ok(columns)
}

/// Calculate the frequency-weighted workload cost and deterministic tie-break key.
/// 计算按查询频次加权的负载代价，并生成列数与稳定索引键供并列时确定性比较。
pub fn evaluate_index_set_cost(
    queries: &BTreeSet<Query>,
    optimizer: &dyn Optimizer,
    indexes: &BTreeSet<Index>,
) -> Result<IndexSetCost, String> {
    let index_list = indexes.iter().cloned().collect::<Vec<_>>();
    let total = queries.iter().try_fold(0.0, |sum, query| {
        optimizer
            .query_plan_cost(&query.text, &index_list)
            .map(|cost| sum + cost * query.frequency as f64)
    })?;
    let mut keys = index_list.iter().map(Index::key).collect::<Vec<_>>();
    keys.sort();
    Ok(IndexSetCost {
        total_workload_query_cost: total,
        total_number_of_index_columns: index_list.iter().map(|index| index.columns.len()).sum(),
        index_keys: keys.join(","),
    })
}

/// Types excluded by TiDB's index-advisor type filter.
/// 判断字段类型是否通过索引顾问的类型过滤；复杂或大对象类型不作为候选。
pub fn is_indexable_column_type(field_type: &FieldType) -> bool {
    !matches!(
        field_type,
        FieldType::Json | FieldType::Blob | FieldType::Geometry | FieldType::Vector
    )
}

fn relevant_column_names(tokens: &[String]) -> HashSet<String> {
    // 只扫描 Go 版访问器关心的谓词与排序/分组子句，避免把所有标识符误判为候选列。
    let mut result = HashSet::new();
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token.as_str(), "=" | "<" | "<=" | ">" | ">=") {
            if let Some(left) = index.checked_sub(1).and_then(|i| tokens.get(i)) {
                if is_column_token(tokens, index.saturating_sub(1)) {
                    result.insert(left.clone());
                }
            }
            if let Some(right) = tokens.get(index + 1) {
                if is_column_token(tokens, index + 1) {
                    result.insert(right.clone());
                }
            }
        } else if token == "between" || token == "in" {
            if let Some(left) = index.checked_sub(1).and_then(|i| tokens.get(i)) {
                if is_identifier(left) {
                    result.insert(left.clone());
                }
            }
        } else if token == "order" || token == "group" {
            if tokens.get(index + 1).map(String::as_str) == Some("by") {
                let end = clause_end(tokens, index + 2);
                for name in &tokens[index + 2..end] {
                    if is_identifier(name) && name != "asc" && name != "desc" {
                        result.insert(name.clone());
                    }
                }
            }
        }
    }
    result
}

fn is_system_schema(schema: &str) -> bool {
    matches!(
        schema.to_ascii_lowercase().as_str(),
        "mysql" | "information_schema" | "performance_schema" | "metrics_schema" | "sys"
    )
}

fn is_column_token(tokens: &[String], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    is_identifier(token)
        && !is_sql_keyword(token)
        && tokens.get(index + 1).map(String::as_str) != Some("(")
}

fn split_table(table: &str) -> Result<(&str, &str), String> {
    table
        .split_once('.')
        .ok_or_else(|| format!("invalid table {table}"))
}

fn cte_names(tokens: &[String], default_schema: &str) -> HashSet<String> {
    // 预先登记 CTE，后续表名收集时据此排除查询内的临时关系。
    let mut result = HashSet::new();
    for index in 0..tokens.len().saturating_sub(1) {
        let is_cte_header = index == 0
            || tokens.get(index - 1).is_some_and(|previous| {
                previous == "with" || previous == "recursive" || previous == ","
            });
        if tokens[index + 1] == "as" && is_identifier(&tokens[index]) && is_cte_header {
            result.insert(format!(
                "{}.{}",
                default_schema.to_ascii_lowercase(),
                normalize_identifier(&tokens[index])
            ));
        }
    }
    result
}

fn clause_end(tokens: &[String], start: usize) -> usize {
    tokens[start..]
        .iter()
        .position(|token| {
            matches!(
                token.as_str(),
                "where" | "group" | "order" | "having" | "limit" | "union" | "for"
            )
        })
        .map_or(tokens.len(), |offset| start + offset)
}

fn qualify_table_references(sql: &str, schema: &str) -> String {
    let tokens = tokenize(sql);
    let mut output = Vec::new();
    let mut expect_table = false;
    let mut in_from_list = false;
    for token in tokens {
        let mut value = token.clone();
        if expect_table && is_identifier(&token) && !token.contains('.') {
            value = format!("{}.{}", schema.to_ascii_lowercase(), token);
            expect_table = false;
        } else if token == "(" && expect_table {
            // Derived tables are qualified by their nested FROM clause instead.
            // 派生表由其内部 FROM 子句补全，外层左括号不应被当作表名。
        } else {
            match token.as_str() {
                "from" => {
                    expect_table = true;
                    in_from_list = true;
                }
                "join" | "update" | "into" => expect_table = true,
                "," if in_from_list => expect_table = true,
                "on" | "where" | "group" | "order" | "having" | "limit" | "union" | "set"
                | "values" | "returning" | "for" => {
                    expect_table = false;
                    in_from_list = false;
                }
                _ => {}
            }
        }
        output.push(value);
    }
    output.join(" ")
}

fn is_constant(token: &str) -> bool {
    token.starts_with('\'')
        || token.starts_with('"')
        || token.parse::<f64>().is_ok()
        || token == "?"
}

fn is_sql_keyword(token: &str) -> bool {
    matches!(
        token,
        "select" | "from" | "as" | "distinct" | "all" | "case" | "when" | "then" | "else"
    )
}

fn is_identifier(token: &str) -> bool {
    !token.is_empty()
        && token != "*"
        && token != ","
        && token != "("
        && token != ")"
        && !matches!(
            token,
            "=" | "<" | ">" | "<=" | ">=" | "<>" | "+" | "-" | "/" | "*"
        )
        && !token.starts_with('\'')
        && !token.starts_with('"')
        && !token.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && token
            .chars()
            .any(|ch| ch.is_ascii_alphabetic() || ch == '_')
}

fn normalize_identifier(token: &str) -> String {
    token.trim_matches('`').to_ascii_lowercase()
}

fn balanced_delimiters(sql: &str) -> bool {
    // 跳过引号内的括号，并处理反斜杠转义，避免把字面量内容计入嵌套深度。
    let mut depth = 0_i32;
    let mut quote = None;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(expected) = quote {
            if ch == '\\' {
                chars.next();
            } else if ch == expected {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth < 0 {
                return false;
            }
        }
    }
    quote.is_none() && depth == 0
}

fn tokenize(sql: &str) -> Vec<String> {
    // 词法器保留字符串字面量和比较运算符，同时丢弃注释并统一普通标识符的大小写。
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let flush = |tokens: &mut Vec<String>, current: &mut String| {
        if !current.is_empty() {
            tokens.push(std::mem::take(current).to_ascii_lowercase());
        }
    };
    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'*') {
            flush(&mut tokens, &mut current);
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
        } else if ch == '-' && chars.peek() == Some(&'-') {
            flush(&mut tokens, &mut current);
            chars.next();
            while let Some(next) = chars.next() {
                if next == '\n' {
                    break;
                }
            }
        } else if ch == '\'' || ch == '"' {
            flush(&mut tokens, &mut current);
            let quote = ch;
            let mut value = String::new();
            value.push(quote);
            while let Some(next) = chars.next() {
                value.push(next);
                if next == '\\' {
                    if let Some(escaped) = chars.next() {
                        value.push(escaped);
                    }
                } else if next == quote {
                    break;
                }
            }
            tokens.push(value.to_ascii_lowercase());
        } else if ch.is_whitespace() {
            flush(&mut tokens, &mut current);
        } else if ",();".contains(ch) {
            flush(&mut tokens, &mut current);
            tokens.push(ch.to_string());
        } else if "=<>+-/".contains(ch) {
            flush(&mut tokens, &mut current);
            let mut operator = ch.to_string();
            if chars.peek().is_some_and(|next| *next == '=') {
                operator.push(chars.next().unwrap());
            }
            tokens.push(operator);
        } else {
            current.push(ch);
        }
    }
    flush(&mut tokens, &mut current);
    tokens
}
