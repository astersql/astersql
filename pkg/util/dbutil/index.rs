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
// 文件前半为迁移草稿块；可执行实现从草稿结束处的 `use` 开始。

// dbutil 中读取 SHOW INDEX 结果、选择索引列和排序键的局部逻辑。

/* Mechanical draft retained for migration history.
use std::cmp::Ordering;
use std::collections::HashSet;
use std::ptr;

// IndexInfo contains information of table index.
// IndexInfo 对应 Go 的同名结构体，字段顺序保持 SHOW INDEX 扫描后的赋值顺序。
pub struct IndexInfo {
    pub Table: String,
    pub KeyName: String,
    pub ColumnName: String,
    pub SeqInIndex: i32,
    pub Cardinality: i32,
    pub NoneUnique: bool,
}

// ShowIndex returns result of executing `show index`
// ShowIndex 对应 Go 中执行 SHOW INDEX 并逐行扫描为 IndexInfo 的流程。
pub fn ShowIndex(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
    table: String,
) -> Result<Vec<IndexInfo>, errors::Error> {
    /*
        show index example result:
        mysql> show index from test;
        +-------+------------+----------+--------------+-------------+-----------+-------------+----------+--------+------+------------+---------+---------------+
        | Table | Non_unique | Key_name | Seq_in_index | Column_name | Collation | Cardinality | Sub_part | Packed | Null | Index_type | Comment | Index_comment |
        +-------+------------+----------+--------------+-------------+-----------+-------------+----------+--------+------+------------+---------+---------------+
        | test  | 0          | PRIMARY  | 1            | id          | A         | 0           | NULL     | NULL   |      | BTREE      |         |               |
        | test  | 0          | aid      | 1            | aid         | A         | 0           | NULL     | NULL   | YES  | BTREE      |         |               |
        +-------+------------+----------+--------------+-------------+-----------+-------------+----------+--------+------+------------+---------+---------------+
    */
    let mut indices: Vec<IndexInfo> = Vec::with_capacity(3);
    let query = format!("SHOW INDEX FROM {}", TableName(schemaName, table));
    let mut rows = match db.QueryContext(ctx, query, &[]) {
        Ok(rows) => rows,
        Err(err) => return Err(errors::Trace(err)),
    };

    while rows.Next() {
        let fields = match ScanRow(&mut rows) {
            Ok(fields) => fields,
            Err(err) => {
                // Go 使用 defer rows.Close；这里在错误返回前显式标出资源收尾点。
                rows.Close();
                return Err(errors::Trace(err));
            }
        };

        let seqInIndex = match String::from_utf8_lossy(&fields["Seq_in_index"].Data).parse::<i32>() {
            Ok(v) => v,
            Err(err) => {
                rows.Close();
                return Err(errors::Trace(err));
            }
        };
        let cardinality = match String::from_utf8_lossy(&fields["Cardinality"].Data).parse::<i32>() {
            Ok(v) => v,
            Err(err) => {
                rows.Close();
                return Err(errors::Trace(err));
            }
        };
        let index = IndexInfo {
            Table: String::from_utf8_lossy(&fields["Table"].Data).to_string(),
            NoneUnique: String::from_utf8_lossy(&fields["Non_unique"].Data) == "1",
            KeyName: String::from_utf8_lossy(&fields["Key_name"].Data).to_string(),
            ColumnName: String::from_utf8_lossy(&fields["Column_name"].Data).to_string(),
            SeqInIndex: seqInIndex,
            Cardinality: cardinality,
        };
        indices.push(index);
    }

    // 对应 Go 的 defer rows.Close；正常路径也保留关闭 Rows 的意图。
    rows.Close();
    Ok(indices)
}

// FindSuitableColumnWithIndex returns first column of a suitable index.
// The priority is
// * primary key
// * unique key
// * normal index which has max cardinality
// FindSuitableColumnWithIndex 按 Go 的优先级选择适合做 order/scan 的索引首列。
pub fn FindSuitableColumnWithIndex(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
    tableInfo: &model::TableInfo,
) -> Result<*const model::ColumnInfo, errors::Error> {
    // find primary key
    // 第一优先级是主键，Go 直接取 index.Columns[0]；这里同样不额外补边界检查。
    for index in &tableInfo.Indices {
        if index.Primary {
            return Ok(FindColumnByName(&tableInfo.Columns, index.Columns[0].Name.O.clone()));
        }
    }

    // no primary key found, seek unique index
    // 第二优先级是唯一索引，返回唯一索引的首列。
    for index in &tableInfo.Indices {
        if index.Unique {
            return Ok(FindColumnByName(&tableInfo.Columns, index.Columns[0].Name.O.clone()));
        }
    }

    // no unique index found, seek index with max cardinality
    // 没有主键/唯一键时，需要查询 SHOW INDEX；这是 Go 里唯一依赖外部数据库返回值的分支。
    let indices = match ShowIndex(ctx, db, schemaName.clone(), tableInfo.Name.O.clone()) {
        Ok(indices) => indices,
        Err(err) => return Err(errors::Trace(err)),
    };
    let mut c: *const model::ColumnInfo = ptr::null();
    let mut maxCardinality = 0;
    for indexInfo in indices {
        // just use the first column in the index, otherwise can't hit the index when select
        // 多列索引只考虑第一列，保持 Go 中避免 SELECT 无法命中索引的判断。
        if indexInfo.SeqInIndex != 1 {
            continue;
        }

        if indexInfo.Cardinality > maxCardinality {
            let column = FindColumnByName(&tableInfo.Columns, indexInfo.ColumnName.clone());
            if column.is_null() {
                return Err(errors::NotFoundf(format!(
                    "column {} in {}.{}",
                    indexInfo.ColumnName, schemaName, tableInfo.Name.O
                )));
            }
            maxCardinality = indexInfo.Cardinality;
            c = column;
        }
    }

    Ok(c)
}

// FindAllIndex returns all index, order is pk, uk, and normal index.
// FindAllIndex 复制索引切片后稳定排序，优先级保持 pk、uk、普通索引。
pub fn FindAllIndex(tableInfo: &model::TableInfo) -> Vec<model::IndexInfo> {
    let mut indices = tableInfo.Indices.clone();
    indices.sort_by(|a, b| {
        // Go 的 sort.SliceStable 在优先级相同或 b 更高时返回 false；这里用 Equal 保留稳定顺序。
        if b.Primary {
            Ordering::Greater
        } else if a.Primary {
            Ordering::Less
        } else if b.Unique {
            Ordering::Greater
        } else if a.Unique {
            Ordering::Less
        } else {
            Ordering::Equal
        }
    });
    indices
}

// FindAllColumnWithIndex returns columns with index, order is pk, uk and normal index.
// FindAllColumnWithIndex 按排序后的索引收集列，并用列名去重。
pub fn FindAllColumnWithIndex(tableInfo: &model::TableInfo) -> Vec<*const model::ColumnInfo> {
    let mut colsMap: HashSet<String> = HashSet::new();
    let mut cols: Vec<*const model::ColumnInfo> = Vec::with_capacity(2);

    for index in FindAllIndex(tableInfo) {
        // index will be guaranteed to be visited in order PK -> UK -> IK
        // Go 假定 FindColumnByName 能找到列；用原始指针保留 nil 风险。
        for indexCol in index.Columns {
            let col = FindColumnByName(&tableInfo.Columns, indexCol.Name.O.clone());
            let colName = unsafe { (*col).Name.O.clone() };
            if colsMap.contains(&colName) {
                continue;
            }
            colsMap.insert(colName);
            cols.push(col);
        }
    }

    cols
}

// SelectUniqueOrderKey returns some columns for order by condition.
// SelectUniqueOrderKey 选择 ORDER BY 可用的唯一键列；主键会覆盖之前遇到的唯一索引。
pub fn SelectUniqueOrderKey(
    tbInfo: &model::TableInfo,
) -> (Vec<String>, Vec<*const model::ColumnInfo>) {
    let mut keys: Vec<String> = Vec::with_capacity(2);
    let mut keyCols: Vec<*const model::ColumnInfo> = Vec::with_capacity(2);

    for index in &tbInfo.Indices {
        if index.Primary {
            // 遇到主键时清空唯一索引候选，并立即用主键列结束搜索。
            keys.clear();
            keyCols.clear();
            for indexCol in &index.Columns {
                keys.push(indexCol.Name.O.clone());
                keyCols.push(tbInfo.Columns[indexCol.Offset]);
            }
            break;
        }
        if index.Unique {
            // Go 会让后续唯一索引覆盖前面的唯一索引；这里保持同样的清空再填充流程。
            keys.clear();
            keyCols.clear();
            for indexCol in &index.Columns {
                keys.push(indexCol.Name.O.clone());
                keyCols.push(tbInfo.Columns[indexCol.Offset]);
            }
        }
    }

    if !keys.is_empty() {
        return (keys, keyCols);
    }

    // no primary key or unique found, use all fields as order by key
    // 没有主键或唯一键时退回所有列，保持 Go 中稳定 order by key 的兜底策略。
    for col in &tbInfo.Columns {
        keys.push(unsafe { (**col).Name.O.clone() });
        keyCols.push(*col);
    }

    (keys, keyCols)
}
*/

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
