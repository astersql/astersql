// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// dbutil 索引辅助：解析 `SHOW INDEX`、按优先级选索引列、构造 ORDER BY 唯一键。
//
// 优先级一般为：主键 → 唯一索引 → 基数（Cardinality）最高的普通索引首列。

// dbutil 中读取 SHOW INDEX 结果、选择索引列和排序键的局部逻辑。

// ========== 可执行实现：基于 QueryExecutor 的 SHOW INDEX 与索引列选择 ==========
use crate::common::TableName;
use crate::interface::{DbError, QueryExecutor, Value};
use astersql_infoschema::infoschema::ColumnInfo;
use std::collections::HashSet;

/// `SHOW INDEX` 单行结果：表名、键名、列名、列序号、基数与是否非唯一。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    pub Table: String,
    pub KeyName: String,
    pub ColumnName: String,
    pub SeqInIndex: i64,
    pub Cardinality: i64,
    pub NoneUnique: bool,
}
/// 表内一条索引的轻量描述：列下标列表及 primary/unique 标记。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableIndexInfo {
    pub name: String,
    pub columns: Vec<usize>,
    pub primary: bool,
    pub unique: bool,
}
/// 带列与索引元数据的表视图，供索引选择算法使用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexedTable {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indices: Vec<TableIndexInfo>,
}

/// 将查询单元格转为字符串，供解析 SHOW INDEX 数值/布尔字段。
fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bytes(value)) => String::from_utf8_lossy(value).into_owned(),
        Some(Value::I64(value)) => value.to_string(),
        Some(Value::U64(value)) => value.to_string(),
        Some(Value::F64(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn required_text<'a>(
    result: &'a crate::interface::QueryResult,
    row: &'a [Value],
    name: &str,
) -> Result<String, DbError> {
    let index = result
        .columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case(name))
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: format!("SHOW INDEX result missing {name}"),
        })?;
    match row.get(index) {
        None | Some(Value::Null) => Err(DbError {
            code: 0,
            sql_state: None,
            message: format!("SHOW INDEX result has NULL {name}"),
        }),
        value => Ok(text(value)),
    }
}
/// 执行 `SHOW INDEX FROM schema.table` 并映射为 `IndexInfo` 列表。
pub fn ShowIndex(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
) -> Result<Vec<IndexInfo>, DbError> {
    let result = db.QueryContext(
        &format!("SHOW INDEX FROM {}", TableName(schema, table)),
        &[],
    )?;
    let mut indices = Vec::with_capacity(result.rows.len());
    for row in &result.rows {
        let seq_in_index = required_text(&result, row, "Seq_in_index")?
            .parse()
            .map_err(|error| DbError {
                code: 0,
                sql_state: None,
                message: format!("invalid Seq_in_index: {error}"),
            })?;
        let cardinality = required_text(&result, row, "Cardinality")?
            .parse()
            .map_err(|error| DbError {
                code: 0,
                sql_state: None,
                message: format!("invalid Cardinality: {error}"),
            })?;
        let non_unique = required_text(&result, row, "Non_unique")? == "1";
        indices.push(IndexInfo {
            Table: required_text(&result, row, "Table")?,
            KeyName: required_text(&result, row, "Key_name")?,
            ColumnName: required_text(&result, row, "Column_name")?,
            SeqInIndex: seq_in_index,
            Cardinality: cardinality,
            NoneUnique: non_unique,
        });
    }
    Ok(indices)
}
/// 按主键 → 唯一键 → SHOW INDEX 最高基数首列的优先级，选出适合扫描/排序的列。
pub fn FindSuitableColumnWithIndex<'a>(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &'a IndexedTable,
) -> Result<Option<&'a ColumnInfo>, DbError> {
    for index in &table.indices {
        if index.primary {
            return Ok(index
                .columns
                .first()
                .and_then(|position| table.columns.get(*position)));
        }
    }
    for index in &table.indices {
        if index.unique {
            return Ok(index
                .columns
                .first()
                .and_then(|position| table.columns.get(*position)));
        }
    }
    // Go 按 SHOW INDEX 返回顺序严格更新基数最大值；相同基数保留先出现的索引。
    let mut selected = None;
    let mut max_cardinality = 0;
    for index in ShowIndex(db, schema, &table.name)? {
        if index.SeqInIndex != 1 || index.Cardinality <= max_cardinality {
            continue;
        }
        let column = table
            .columns
            .iter()
            .find(|column| column.name.lower == index.ColumnName.to_lowercase())
            .ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: format!(
                    "column {} in {}.{} not found",
                    index.ColumnName, schema, table.name
                ),
            })?;
        max_cardinality = index.Cardinality;
        selected = Some(column);
    }
    Ok(selected)
}
/// 返回全部索引引用，排序为主键、唯一键、普通索引。
pub fn FindAllIndex(table: &IndexedTable) -> Vec<&TableIndexInfo> {
    let mut indices: Vec<_> = table.indices.iter().collect();
    indices.sort_by_key(|index| {
        if index.primary {
            0
        } else if index.unique {
            1
        } else {
            2
        }
    });
    indices
}
/// 按索引优先级收集带索引的列，列下标去重。
pub fn FindAllColumnWithIndex(table: &IndexedTable) -> Vec<&ColumnInfo> {
    let mut visited = HashSet::new();
    let mut columns = Vec::new();
    for index in FindAllIndex(table) {
        for position in &index.columns {
            if let Some(column) = table.columns.get(*position) {
                // Go 以列名去重，而不是以索引内的 offset 去重。
                if visited.insert(column.name.original.clone()) {
                    columns.push(column);
                }
            }
        }
    }
    columns
}
/// 选择 ORDER BY 可用的唯一键列名；优先主键，否则取最后一个唯一索引，再否则退回全列。
pub fn SelectUniqueOrderKey(table: &IndexedTable) -> (Vec<String>, Vec<&ColumnInfo>) {
    let selected = table
        .indices
        .iter()
        // 主键优先；否则从后往前找唯一索引（与 Go 后写覆盖语义对齐）。
        .find(|index| index.primary)
        .or_else(|| table.indices.iter().rev().find(|index| index.unique));
    let positions: Vec<usize> = selected
        .map(|index| index.columns.clone())
        .unwrap_or_else(|| (0..table.columns.len()).collect());
    let columns: Vec<&ColumnInfo> = positions
        .iter()
        .filter_map(|position| table.columns.get(*position))
        .collect();
    (
        columns
            .iter()
            .map(|column| column.name.original.clone())
            .collect(),
        columns,
    )
}
