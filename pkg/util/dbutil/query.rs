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

// 查询结果扫描辅助：将行数据转为二维 `Value` 或按列名的 `ColumnData` 映射。
//
// 对应 Go `pkg/util/dbutil` 中 `ScanRowsToInterfaces` / `ScanRow`。迁移稿保留在块注释中；
// 当前实现面向本 crate 的 `QueryResult`/`Value`，不依赖真实 `database/sql`。

// 把 sql.Rows 扫描为通用二维数组或列名映射的辅助逻辑。

/* Mechanical draft retained for migration history.
use std::collections::HashMap;
// ScanRowsToInterfaces scans rows to interface array.
// ScanRowsToInterfaces 对应 Go 中把每行扫描进 []any，并累积为 [][]any 的辅助函数。
pub fn ScanRowsToInterfaces(
    rows: &mut sql::Rows,
) -> Result<Vec<Vec<Box<dyn std::any::Any>>>, errors::Error> {
    let mut rowsData: Vec<Vec<Box<dyn std::any::Any>>> = Vec::new();
    let cols = match rows.Columns() {
        Ok(cols) => cols,
        Err(err) => return Err(errors::Trace(err)),
    };

    while rows.Next() {
        let mut colVals: Vec<Box<dyn std::any::Any>> = Vec::with_capacity(cols.len());
        for _ in &cols {
            // Go 的 make([]any, len(cols)) 会得到 nil interface 槽位；
            // Rust 用 unit 占位，只表达“扫描目标数量等于列数”的结构。
            colVals.push(Box::new(()));
        }

        if let Err(err) = rows.Scan(&mut colVals) {
            return Err(errors::Trace(err));
        }
        rowsData.push(colVals);
    }

    Ok(rowsData)
}

// ColumnData saves column's data.
// ColumnData 保存单列原始字节和 NULL 标记，对应 Go 中 map[string]*ColumnData 的值类型。
pub struct ColumnData {
    pub Data: Vec<u8>,
    pub IsNull: bool,
}

// ScanRow scans rows into a map.
// ScanRow 对应 Go 中把当前行按列名扫描为 map 的逻辑。
pub fn ScanRow(rows: &mut sql::Rows) -> Result<HashMap<String, ColumnData>, errors::Error> {
    let cols = match rows.Columns() {
        Ok(cols) => cols,
        Err(err) => return Err(errors::Trace(err)),
    };

    let mut colVals: Vec<Option<Vec<u8>>> = vec![None; cols.len()];
    let mut colValsI: Vec<&mut Option<Vec<u8>>> = Vec::with_capacity(colVals.len());
    for colVal in &mut colVals {
        // Go 把每个 []byte 的地址放进 []any，Rows.Scan 再填充这些地址。
        // Rust 用 Option<Vec<u8>> 表达数据库 NULL 与非 NULL 字节值的差异。
        colValsI.push(colVal);
    }

    if let Err(err) = rows.Scan(&mut colValsI) {
        return Err(errors::Trace(err));
    }

    let mut result: HashMap<String, ColumnData> = HashMap::new();
    for i in 0..colVals.len() {
        let data = ColumnData {
            Data: colVals[i].clone().unwrap_or_default(),
            IsNull: colVals[i].is_none(),
        };
        result.insert(cols[i].clone(), data);
    }

    Ok(result)
}
*/

use std::collections::HashMap;

use crate::interface::{QueryResult, Value};

fn format_scanned_float(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "+Inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        value.to_string()
    }
}

/// 取出结果集全部行，对应 Go 将 `sql.Rows` 扫成 `[][]any` 的简化版。
pub fn ScanRowsToInterfaces(rows: QueryResult) -> Vec<Vec<Value>> {
    rows.rows
}

/// 单列原始字节与 NULL 标记，对应 Go `map[string]*ColumnData` 的值类型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnData {
    /// 列值的原始字节；NULL 时通常为空切片。
    pub Data: Vec<u8>,
    /// 是否为 SQL NULL。
    pub IsNull: bool,
}

/// 将一行按列名映射为 `ColumnData`；列数与值数不一致时返回错误。
pub fn ScanRow(columns: &[String], row: &[Value]) -> Result<HashMap<String, ColumnData>, String> {
    // 列名与单元格一一对应，长度不符则无法建 map。
    if columns.len() != row.len() {
        return Err(format!(
            "column count {} does not match value count {}",
            columns.len(),
            row.len()
        ));
    }
    // 将各 Value 变体统一为字节 + IsNull，便于下游按列名取原始数据。
    Ok(columns
        .iter()
        .cloned()
        .zip(row.iter())
        .map(|(name, value)| {
            let (data, is_null) = match value {
                Value::Null => (Vec::new(), true),
                Value::Bytes(value) => (value.clone(), false),
                Value::String(value) => (value.as_bytes().to_vec(), false),
                Value::Bool(value) => (value.to_string().into_bytes(), false),
                Value::I64(value) => (value.to_string().into_bytes(), false),
                Value::U64(value) => (value.to_string().into_bytes(), false),
                Value::F64(value) => (format_scanned_float(*value).into_bytes(), false),
            };
            (
                name,
                ColumnData {
                    Data: data,
                    IsNull: is_null,
                },
            )
        })
        .collect())
}
