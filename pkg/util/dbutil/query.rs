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
// 对应 Go `pkg/util/dbutil` 中 `ScanRowsToInterfaces` / `ScanRow`。
// 当前实现面向本 crate 的 `QueryResult`/`Value`，不依赖真实 `database/sql`。

// 把 sql.Rows 扫描为通用二维数组或列名映射的辅助逻辑。

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
