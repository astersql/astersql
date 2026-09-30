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

use astersql_meta_model::{IndexInfo, StatePublic, TableInfo};
use astersql_parser_ast as ast;
use astersql_parser_mysql as mysql;
use std::collections::HashSet;

/// Rejects SELECT clauses unsupported by materialized views before planning.
#[allow(non_snake_case)]
pub fn CheckMaterializedViewSelect(
    node: &dyn ast::Node,
) -> Result<(), astersql_errors::SharedError> {
    let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() else {
        return Ok(());
    };
    if select.With.is_some() {
        return Err(unsupported(
            "CREATE MATERIALIZED VIEW does not support common table expressions",
        ));
    }
    if select
        .lock_info
        .as_ref()
        .is_some_and(|lock| lock.LockType != ast::SelectLockType::None)
    {
        return Err(unsupported(
            "CREATE MATERIALIZED VIEW does not support locking clauses",
        ));
    }
    if select.SelectIntoOpt.is_some() {
        return Err(unsupported(
            "CREATE MATERIALIZED VIEW does not support SELECT INTO",
        ));
    }
    let Some(from) = &select.From else {
        return Ok(());
    };
    if from.TableRefs.Right.is_some() {
        return Ok(());
    }
    let Some(left) = from.TableRefs.Left.as_deref() else {
        return Ok(());
    };
    let ast::ResultSetNode::TableSource(source) = left else {
        return Ok(());
    };
    if source.QuerySource.is_some() {
        return Ok(());
    }
    if source.AsOf.is_some() {
        return Err(unsupported(
            "CREATE MATERIALIZED VIEW does not support AS OF",
        ));
    }
    if source.TableSample.is_some() {
        return Err(unsupported(
            "CREATE MATERIALIZED VIEW does not support TABLESAMPLE",
        ));
    }
    Ok(())
}

fn unsupported(message: &str) -> astersql_errors::SharedError {
    astersql_util_dbterror::ErrGeneralUnsupportedDDL.GenWithStack(message, &[])
}

/// Returns the first visible public index whose leading columns cover all group-by columns.
#[allow(non_snake_case)]
pub fn FindVisibleIndexWithPrefixCoveringColumns(
    table: Option<&TableInfo>,
    group_by: &[&str],
) -> (String, bool) {
    match FindVisibleIndexesWithPrefixCoveringColumns(table, group_by)
        .into_iter()
        .next()
    {
        Some(name) => (name, true),
        None => (String::new(), false),
    }
}

/// Returns every visible public index whose leading columns cover all group-by columns.
#[allow(non_snake_case)]
pub fn FindVisibleIndexesWithPrefixCoveringColumns(
    table: Option<&TableInfo>,
    group_by: &[&str],
) -> Vec<String> {
    find_indexes(table, group_by, "", true)
}

/// Reports whether any eligible index covers the group-by columns in its leading key positions.
#[allow(non_snake_case)]
pub fn HasIndexWithPrefixCoveringColumns(
    table: Option<&TableInfo>,
    group_by: &[&str],
    excluded: &str,
    require_visible_public: bool,
) -> bool {
    !find_indexes(table, group_by, excluded, require_visible_public).is_empty()
}

fn find_indexes(
    table: Option<&TableInfo>,
    group_by: &[&str],
    excluded: &str,
    require_visible_public: bool,
) -> Vec<String> {
    let Some(table) = table else {
        return Vec::new();
    };
    let prefix_len = group_by.len();
    if prefix_len == 0 {
        return Vec::new();
    }
    let group_set: HashSet<String> = group_by.iter().map(|name| name.to_lowercase()).collect();
    let excluded = excluded.to_lowercase();
    let mut names = Vec::new();
    if table.PKIsHandle
        && prefix_len == 1
        && excluded != mysql::r#const::PrimaryKeyName.to_lowercase()
    {
        if let Some(pk) = table.GetPkColInfo() {
            if group_set.contains(&pk.Name.L) {
                names.push(mysql::r#const::PrimaryKeyName.to_owned());
            }
        }
    }
    for index in &table.Indices {
        if index.Columns.len() < prefix_len {
            continue;
        }
        if require_visible_public && (index.State != StatePublic || index.Invisible) {
            continue;
        }
        if !excluded.is_empty() && index.Name.L == excluded {
            continue;
        }
        if index_prefix_covers(index, prefix_len, &group_set) {
            names.push(index.Name.O.clone());
        }
    }
    names
}

fn index_prefix_covers(index: &IndexInfo, prefix_len: usize, group_set: &HashSet<String>) -> bool {
    let mut matched = HashSet::with_capacity(prefix_len);
    for column in index.Columns.iter().take(prefix_len) {
        if column.Length > 0
            || !group_set.contains(&column.Name.L)
            || !matched.insert(&column.Name.L)
        {
            return false;
        }
    }
    matched.len() == prefix_len
}

#[cfg(test)]
#[path = "go_merge_30_test.rs"]
mod go_merge_30_test;
