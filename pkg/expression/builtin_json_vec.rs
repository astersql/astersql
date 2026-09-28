// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Vectorized JSON builtins corresponding to `builtin_json_vec.go`.
//
// A column is represented as `Vec<Option<T>>`: the outer vector preserves Go's
// row order and `None` is SQL NULL. JSON `null` remains a real `BinaryJSON`
// value, which is deliberately distinct from SQL NULL throughout this module.
//
// JSON 内置函数的向量化实现（对应 `builtin_json_vec.go`）。
//
// 列表示为 `Vec<Option<T>>`：外层顺序对齐 Go 行序，`None` 为 SQL NULL；
// JSON null 仍是合法的 `BinaryJSON`，与 SQL NULL 严格区分。

use crc32fast::hash as crc32;
use serde_json::{Map, Value};
use std::error::Error;
use std::fmt::{Display, Formatter};

pub use types_json_functions::{
    BinaryJSON, JSONModifyInsert, JSONModifyReplace, JSONModifySet, JSONModifyType,
    JSONPathExpression, JSONTypeCodeArray, JSONTypeCodeObject,
};

use types_json_functions::{
    BinaryJSONToSerde, CompareBinaryJSON, ContainsBinaryJSON, CreateBinaryJSON, MergeBinaryJSON,
    MergePatchBinaryJSON, OverlapsBinaryJSON, ParseBinaryJSONFromString, ParseJSONPathExpr,
    UnquoteString,
};

/// JSON 列：每行一个可选 BinaryJSON。
pub type JsonColumn = Vec<Option<BinaryJSON>>;
/// 字符串列。
pub type StringColumn = Vec<Option<String>>;
/// 整型列（布尔结果常以 0/1 存放）。
pub type IntColumn = Vec<Option<i64>>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 向量化 JSON 求值错误。
pub struct JsonVecError {
    message: String,
}

impl JsonVecError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for JsonVecError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for JsonVecError {}

/// 向量化 JSON 结果别名。
pub type JsonVecResult<T> = Result<T, JsonVecError>;

/// 将底层错误映射为 JsonVecError。
fn json_result<T, E: Display>(result: Result<T, E>) -> JsonVecResult<T> {
    result.map_err(|error| JsonVecError::new(error.to_string()))
}

/// 解析 JSON 文本为 BinaryJSON。
pub fn parse_json(text: &str) -> JsonVecResult<BinaryJSON> {
    json_result(ParseBinaryJSONFromString(text))
}

/// BinaryJSON → serde Value。
pub fn json_value(value: &BinaryJSON) -> JsonVecResult<Value> {
    json_result(BinaryJSONToSerde(value))
}

/// serde Value → BinaryJSON。
fn binary_json(value: Value) -> JsonVecResult<BinaryJSON> {
    json_result(CreateBinaryJSON(value))
}

/// JSON null 常量。
fn json_null() -> BinaryJSON {
    CreateBinaryJSON(Value::Null).expect("JSON null is always representable")
}

/// 解析 JSON Path；`allow_multiple=false` 时拒绝 `*`/范围。
fn parse_path(path: &str, allow_multiple: bool) -> JsonVecResult<JSONPathExpression> {
    let parsed = json_result(ParseJSONPathExpr(path))?;
    if !allow_multiple && parsed.CouldMatchMultipleValues() {
        return Err(JsonVecError::new("JSON path may not contain * or a range"));
    }
    Ok(parsed)
}

/// 断言单列行数与期望一致。
fn same_len<T>(expected: usize, column: &[Option<T>]) -> JsonVecResult<()> {
    if column.len() != expected {
        return Err(JsonVecError::new(format!(
            "column row count mismatch: expected {expected}, got {}",
            column.len()
        )));
    }
    Ok(())
}

/// 校验多列同行数并返回行数。
fn columns_len<T>(columns: &[Vec<Option<T>>]) -> JsonVecResult<usize> {
    let rows = columns.first().map_or(0, Vec::len);
    for column in columns {
        same_len(rows, column)?;
    }
    Ok(rows)
}

/// 校验 path/value 参数列数量与行数对齐。
fn pair_columns_len(
    paths: &[StringColumn],
    values: &[JsonColumn],
    expected_rows: usize,
) -> JsonVecResult<()> {
    if paths.len() != values.len() {
        return Err(JsonVecError::new("JSON path/value argument count mismatch"));
    }
    for path in paths {
        same_len(expected_rows, path)?;
    }
    for value in values {
        same_len(expected_rows, value)?;
    }
    Ok(())
}

/// Go's `vecJSONModify`, including SQL NULL paths and JSON-null values.
/// 向量化修改（SET/INSERT/REPLACE 共用）：SQL NULL 路径使整行为 NULL；值侧 SQL NULL→JSON null。
pub fn vec_json_modify(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
    values: &[JsonColumn],
    modify_type: JSONModifyType,
) -> JsonVecResult<JsonColumn> {
    let rows = documents.len();
    pair_columns_len(paths, values, rows)?;
    let mut result = Vec::with_capacity(rows);

    for row in 0..rows {
        let Some(document) = &documents[row] else {
            result.push(None);
            continue;
        };
        // 先收集本行全部路径；任一 SQL NULL 则整行为 NULL。
        let mut parsed_paths = Vec::with_capacity(paths.len());
        let mut row_is_null = false;
        for path_column in paths {
            let Some(path) = &path_column[row] else {
                row_is_null = true;
                break;
            };
            parsed_paths.push(parse_path(path, true)?);
        }
        if row_is_null {
            result.push(None);
            continue;
        }
        let row_values = values
            .iter()
            .map(|column| column[row].clone().unwrap_or_else(json_null))
            .collect::<Vec<_>>();
        result.push(Some(json_result(document.Modify(
            &parsed_paths,
            &row_values,
            modify_type,
        ))?));
    }
    Ok(result)
}

/// JSON_INSERT 向量化。
pub fn vec_json_insert(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
    values: &[JsonColumn],
) -> JsonVecResult<JsonColumn> {
    vec_json_modify(documents, paths, values, JSONModifyInsert)
}

/// JSON_REPLACE 向量化。
pub fn vec_json_replace(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
    values: &[JsonColumn],
) -> JsonVecResult<JsonColumn> {
    vec_json_modify(documents, paths, values, JSONModifyReplace)
}

/// JSON_SET 向量化。
pub fn vec_json_set(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
    values: &[JsonColumn],
) -> JsonVecResult<JsonColumn> {
    vec_json_modify(documents, paths, values, JSONModifySet)
}

/// JSON_STORAGE_FREE：有文档则 0，SQL NULL 传播。
pub fn vec_json_storage_free(documents: &[Option<BinaryJSON>]) -> IntColumn {
    documents
        .iter()
        .map(|document| document.as_ref().map(|_| 0))
        .collect()
}

/// JSON_STORAGE_SIZE：载荷长 + 1。
pub fn vec_json_storage_size(documents: &[Option<BinaryJSON>]) -> IntColumn {
    documents
        .iter()
        .map(|document| {
            document
                .as_ref()
                .map(|json| json.Value.len().saturating_add(1) as i64)
        })
        .collect()
}

/// JSON_DEPTH 向量化。
pub fn vec_json_depth(documents: &[Option<BinaryJSON>]) -> IntColumn {
    documents
        .iter()
        .map(|document| document.as_ref().map(|json| json.GetElemDepth() as i64))
        .collect()
}

/// 对象键排序后组成 JSON 数组；非对象返回 None。
fn object_keys(document: &BinaryJSON) -> JsonVecResult<Option<BinaryJSON>> {
    if document.TypeCode != JSONTypeCodeObject {
        return Ok(None);
    }
    let value = json_value(document)?;
    let mut keys = value
        .as_object()
        .expect("object type code has object payload")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    binary_json(Value::Array(keys.into_iter().map(Value::String).collect())).map(Some)
}

/// JSON_KEYS（无路径）向量化。
pub fn vec_json_keys(documents: &[Option<BinaryJSON>]) -> JsonVecResult<JsonColumn> {
    documents
        .iter()
        .map(|document| match document {
            Some(document) => object_keys(document),
            None => Ok(None),
        })
        .collect()
}

/// JSON_ARRAY：按行从多列取元素构造数组。
pub fn vec_json_array(arguments: &[JsonColumn]) -> JsonVecResult<JsonColumn> {
    let rows = columns_len(arguments)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let values = arguments
            .iter()
            .map(|column| match &column[row] {
                Some(value) => json_value(value),
                None => Ok(Value::Null),
            })
            .collect::<JsonVecResult<Vec<_>>>()?;
        result.push(Some(binary_json(Value::Array(values))?));
    }
    Ok(result)
}

/// MEMBER OF 向量化，结果 0/1。
pub fn vec_json_member_of(
    targets: &[Option<BinaryJSON>],
    objects: &[Option<BinaryJSON>],
) -> JsonVecResult<IntColumn> {
    same_len(targets.len(), objects)?;
    let mut result = Vec::with_capacity(targets.len());
    for (target, object) in targets.iter().zip(objects) {
        let (Some(target), Some(object)) = (target, object) else {
            result.push(None);
            continue;
        };
        let matches = if object.TypeCode == JSONTypeCodeArray {
            json_value(object)?
                .as_array()
                .expect("array type code has array payload")
                .iter()
                .map(|value| binary_json(value.clone()))
                .collect::<JsonVecResult<Vec<_>>>()?
                .iter()
                .any(|item| CompareBinaryJSON(item, target) == 0)
        } else {
            CompareBinaryJSON(object, target) == 0
        };
        result.push(Some(i64::from(matches)));
    }
    Ok(result)
}

/// JSON_CONTAINS；可选路径列。
pub fn vec_json_contains(
    documents: &[Option<BinaryJSON>],
    targets: &[Option<BinaryJSON>],
    paths: Option<&StringColumn>,
) -> JsonVecResult<IntColumn> {
    same_len(documents.len(), targets)?;
    if let Some(paths) = paths {
        same_len(documents.len(), paths)?;
    }
    let mut result = Vec::with_capacity(documents.len());
    for row in 0..documents.len() {
        let (Some(document), Some(target)) = (&documents[row], &targets[row]) else {
            result.push(None);
            continue;
        };
        let selected = if let Some(paths) = paths {
            let Some(path) = &paths[row] else {
                result.push(None);
                continue;
            };
            let path = parse_path(path, false)?;
            match json_result(document.Extract(&[path]))? {
                Some(value) => value,
                None => {
                    result.push(None);
                    continue;
                }
            }
        } else {
            document.clone()
        };
        result.push(Some(i64::from(ContainsBinaryJSON(&selected, target))));
    }
    Ok(result)
}

/// JSON_OVERLAPS 向量化。
pub fn vec_json_overlaps(
    left: &[Option<BinaryJSON>],
    right: &[Option<BinaryJSON>],
) -> JsonVecResult<IntColumn> {
    same_len(left.len(), right)?;
    Ok(left
        .iter()
        .zip(right)
        .map(|(left, right)| match (left, right) {
            (Some(left), Some(right)) => Some(i64::from(OverlapsBinaryJSON(left, right))),
            _ => None,
        })
        .collect())
}

/// JSON_QUOTE 向量化。
pub fn vec_json_quote(strings: &[Option<String>]) -> JsonVecResult<StringColumn> {
    strings
        .iter()
        .map(|string| match string {
            Some(string) => serde_json::to_string(string)
                .map(Some)
                .map_err(|error| JsonVecError::new(error.to_string())),
            None => Ok(None),
        })
        .collect()
}

/// JSON_SEARCH：one/all、转义与路径 NULL。
pub fn vec_json_search(
    documents: &[Option<BinaryJSON>],
    modes: &[Option<String>],
    searches: &[Option<String>],
    escapes: Option<&[Option<String>]>,
    paths: &[StringColumn],
) -> JsonVecResult<JsonColumn> {
    let rows = documents.len();
    same_len(rows, modes)?;
    same_len(rows, searches)?;
    if let Some(escapes) = escapes {
        same_len(rows, escapes)?;
    }
    for path in paths {
        same_len(rows, path)?;
    }

    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let (Some(document), Some(mode), Some(search)) =
            (&documents[row], &modes[row], &searches[row])
        else {
            result.push(None);
            continue;
        };
        let mode = mode.to_lowercase();
        if mode != "one" && mode != "all" {
            return Err(JsonVecError::new("json_search expects 'one' or 'all'"));
        }
        let escape = match escapes.and_then(|column| column[row].as_ref()) {
            None => b'\\',
            Some(value) if value.is_empty() => b'\\',
            Some(value) if value.len() == 1 => value.as_bytes()[0],
            Some(_) => return Err(JsonVecError::new("incorrect arguments to ESCAPE")),
        };
        let mut parsed_paths = Vec::with_capacity(paths.len());
        let mut null_path = false;
        for path in paths {
            let Some(path) = &path[row] else {
                null_path = true;
                break;
            };
            parsed_paths.push(parse_path(path, true)?);
        }
        if null_path {
            result.push(None);
            continue;
        }
        result.push(json_result(document.Search(
            &mode,
            search,
            escape,
            &parsed_paths,
        ))?);
    }
    Ok(result)
}

/// JSON_OBJECT：键为 SQL NULL 时报错。
pub fn vec_json_object(keys: &[StringColumn], values: &[JsonColumn]) -> JsonVecResult<JsonColumn> {
    if keys.len() != values.len() {
        return Err(JsonVecError::new(
            "JSON_OBJECT expects alternating key/value arguments",
        ));
    }
    let rows = keys
        .first()
        .map_or_else(|| values.first().map_or(0, Vec::len), Vec::len);
    pair_columns_len(keys, values, rows)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut object = Map::new();
        for (key_column, value_column) in keys.iter().zip(values) {
            let Some(key) = &key_column[row] else {
                return Err(JsonVecError::new(
                    "JSON documents may not contain NULL member names",
                ));
            };
            let value = match &value_column[row] {
                Some(value) => json_value(value)?,
                None => Value::Null,
            };
            object.insert(key.clone(), value);
        }
        result.push(Some(binary_json(Value::Object(object))?));
    }
    Ok(result)
}

/// JSON_ARRAY_INSERT 向量化。
pub fn vec_json_array_insert(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
    values: &[JsonColumn],
) -> JsonVecResult<JsonColumn> {
    let rows = documents.len();
    pair_columns_len(paths, values, rows)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let Some(mut document) = documents[row].clone() else {
            result.push(None);
            continue;
        };
        let mut row_is_null = false;
        for (path_column, value_column) in paths.iter().zip(values) {
            let Some(path) = &path_column[row] else {
                row_is_null = true;
                break;
            };
            let path = parse_path(path, false)?;
            let value = value_column[row].clone().unwrap_or_else(json_null);
            document = json_result(document.ArrayInsert(path, value))?;
        }
        result.push((!row_is_null).then_some(document));
    }
    Ok(result)
}

/// 在指定路径上取对象键（KEYS 带 path）。
pub fn vec_json_keys_at_path(
    documents: &[Option<BinaryJSON>],
    paths: &[Option<String>],
) -> JsonVecResult<JsonColumn> {
    same_len(documents.len(), paths)?;
    let mut result = Vec::with_capacity(documents.len());
    for (document, path) in documents.iter().zip(paths) {
        let (Some(document), Some(path)) = (document, path) else {
            result.push(None);
            continue;
        };
        let path = parse_path(path, false)?;
        result.push(match json_result(document.Extract(&[path]))? {
            Some(selected) => object_keys(&selected)?,
            None => None,
        });
    }
    Ok(result)
}

/// 单值 LENGTH：数组/对象元素数，标量为 1。
fn json_length(value: &BinaryJSON) -> JsonVecResult<i64> {
    match json_value(value)? {
        Value::Array(values) => Ok(values.len() as i64),
        Value::Object(values) => Ok(values.len() as i64),
        _ => Ok(1),
    }
}

/// JSON_LENGTH 向量化；可选路径。
pub fn vec_json_length(
    documents: &[Option<BinaryJSON>],
    paths: Option<&StringColumn>,
) -> JsonVecResult<IntColumn> {
    if let Some(paths) = paths {
        same_len(documents.len(), paths)?;
    }
    let mut result = Vec::with_capacity(documents.len());
    for row in 0..documents.len() {
        let Some(document) = &documents[row] else {
            result.push(None);
            continue;
        };
        let selected = if let Some(paths) = paths {
            let Some(path) = &paths[row] else {
                result.push(None);
                continue;
            };
            let path = parse_path(path, false)?;
            match json_result(document.Extract(&[path]))? {
                Some(value) => value,
                None => {
                    result.push(None);
                    continue;
                }
            }
        } else {
            document.clone()
        };
        result.push(Some(json_length(&selected)?));
    }
    Ok(result)
}

/// JSON_TYPE 向量化。
pub fn vec_json_type(documents: &[Option<BinaryJSON>]) -> StringColumn {
    documents
        .iter()
        .map(|document| document.as_ref().map(BinaryJSON::Type))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SUM_CRC32 元素目标类型。
pub enum JsonCrc32Type {
    Signed,
    Unsigned,
    Real,
    String,
}

/// 将 JSON 值 CAST 为目标类型字符串。
fn crc32_cast(value: &Value, target: JsonCrc32Type) -> JsonVecResult<String> {
    match target {
        JsonCrc32Type::Signed => value
            .as_i64()
            .map(|value| value.to_string())
            .ok_or_else(|| JsonVecError::new("invalid JSON value for CAST to SIGNED")),
        JsonCrc32Type::Unsigned => value
            .as_u64()
            .map(|value| value.to_string())
            .ok_or_else(|| JsonVecError::new("invalid JSON value for CAST to UNSIGNED")),
        JsonCrc32Type::Real => value
            .as_f64()
            .map(|value| value.to_string())
            .ok_or_else(|| JsonVecError::new("invalid JSON value for CAST to DOUBLE")),
        JsonCrc32Type::String => match value {
            Value::String(value) => Ok(value.clone()),
            value => {
                serde_json::to_string(value).map_err(|error| JsonVecError::new(error.to_string()))
            }
        },
    }
}

/// JSON_SUM_CRC32：对数组元素 CRC 累加。
pub fn vec_json_sum_crc32(
    documents: &[Option<BinaryJSON>],
    target: JsonCrc32Type,
) -> JsonVecResult<IntColumn> {
    let mut result = Vec::with_capacity(documents.len());
    for document in documents {
        let Some(document) = document else {
            result.push(None);
            continue;
        };
        let value = json_value(document)?;
        let Some(array) = value.as_array() else {
            return Err(JsonVecError::new(
                "invalid argument type for JSON_SUM_CRC32",
            ));
        };
        let mut sum = 0_i64;
        for item in array {
            let cast = crc32_cast(item, target)?;
            sum = sum.wrapping_add(i64::from(crc32(cast.as_bytes())));
        }
        result.push(Some(sum));
    }
    Ok(result)
}

/// JSON_EXTRACT 向量化；任一路径 SQL NULL 则行结果 NULL。
pub fn vec_json_extract(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
) -> JsonVecResult<JsonColumn> {
    let rows = documents.len();
    for path in paths {
        same_len(rows, path)?;
    }
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let Some(document) = &documents[row] else {
            result.push(None);
            continue;
        };
        let mut parsed_paths = Vec::with_capacity(paths.len());
        let mut null_path = false;
        for path in paths {
            let Some(path) = &path[row] else {
                null_path = true;
                break;
            };
            parsed_paths.push(parse_path(path, true)?);
        }
        if null_path {
            result.push(None);
            continue;
        }
        result.push(json_result(document.Extract(&parsed_paths))?);
    }
    Ok(result)
}

/// JSON_REMOVE 向量化。
pub fn vec_json_remove(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
) -> JsonVecResult<JsonColumn> {
    let rows = documents.len();
    for path in paths {
        same_len(rows, path)?;
    }
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let Some(document) = &documents[row] else {
            result.push(None);
            continue;
        };
        let mut parsed_paths = Vec::with_capacity(paths.len());
        let mut null_path = false;
        for path in paths {
            let Some(path) = &path[row] else {
                null_path = true;
                break;
            };
            parsed_paths.push(parse_path(path, true)?);
        }
        if null_path {
            result.push(None);
            continue;
        }
        result.push(Some(json_result(document.Remove(&parsed_paths))?));
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// MERGE 结果：值列 + 可选弃用警告列表。
pub struct JsonMergeOutcome {
    pub values: JsonColumn,
    pub warnings: Vec<String>,
}

/// Implements both JSON_MERGE_PRESERVE and the deprecated JSON_MERGE alias.
/// 同时实现 MERGE_PRESERVE 与弃用别名；别名对每个非 NULL 行追加一条警告。
/// Go appends one deprecation warning for every non-NULL row of the alias.
/// Go 对别名的每个非 NULL 行追加一条弃用警告。
pub fn vec_json_merge_with_warnings(
    arguments: &[JsonColumn],
    deprecated_json_merge_alias: bool,
) -> JsonVecResult<JsonMergeOutcome> {
    let rows = columns_len(arguments)?;
    let mut values = Vec::with_capacity(rows);
    let mut warnings = Vec::new();
    for row in 0..rows {
        if arguments.iter().any(|column| column[row].is_none()) {
            values.push(None);
            continue;
        }
        let row_values = arguments
            .iter()
            .map(|column| column[row].clone().expect("checked non-null"))
            .collect::<Vec<_>>();
        values.push(Some(json_result(MergeBinaryJSON(&row_values))?));
        if deprecated_json_merge_alias {
            warnings.push("JSON_MERGE is deprecated; use JSON_MERGE_PRESERVE instead".to_owned());
        }
    }
    Ok(JsonMergeOutcome { values, warnings })
}

/// JSON_MERGE_PRESERVE 向量化（无弃用警告）。
pub fn vec_json_merge(arguments: &[JsonColumn]) -> JsonVecResult<JsonColumn> {
    Ok(vec_json_merge_with_warnings(arguments, false)?.values)
}

/// JSON_CONTAINS_PATH 向量化。
pub fn vec_json_contains_path(
    documents: &[Option<BinaryJSON>],
    modes: &[Option<String>],
    paths: &[StringColumn],
) -> JsonVecResult<IntColumn> {
    let rows = documents.len();
    same_len(rows, modes)?;
    for path in paths {
        same_len(rows, path)?;
    }
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let (Some(document), Some(mode)) = (&documents[row], &modes[row]) else {
            result.push(None);
            continue;
        };
        let mode = mode.to_lowercase();
        if mode != "one" && mode != "all" {
            return Err(JsonVecError::new(
                "json_contains_path expects 'one' or 'all'",
            ));
        }
        // one：遇存在即真；all：遇缺失即假；路径 SQL NULL 则行结果 NULL。
        let mut contains = mode == "all";
        let mut row_is_null = false;
        for path in paths {
            let Some(path) = &path[row] else {
                row_is_null = true;
                break;
            };
            let path = parse_path(path, true)?;
            let exists = json_result(document.Extract(&[path]))?.is_some();
            if mode == "one" && exists {
                contains = true;
                break;
            }
            if mode == "all" && !exists {
                contains = false;
                break;
            }
        }
        result.push((!row_is_null).then_some(i64::from(contains)));
    }
    Ok(result)
}

/// 在单路径处追加：缺失则原样返回；标量装箱；数组合并。
fn append_json_array(
    document: BinaryJSON,
    path: &str,
    value: BinaryJSON,
) -> JsonVecResult<BinaryJSON> {
    let path = parse_path(path, false)?;
    let Some(selected) = json_result(document.Extract(&[path.clone()]))? else {
        return Ok(document);
    };
    let selected = if selected.TypeCode == JSONTypeCodeArray {
        selected
    } else {
        binary_json(Value::Array(vec![json_value(&selected)?]))?
    };
    // The extra wrapper is intentional: appending [2,3] to [1] yields
    // [1,[2,3]], not [1,2,3].
    // 故意再包一层：向 [1] 追加 [2,3] 得到 [1,[2,3]] 而非摊平。
    let wrapped_value = binary_json(Value::Array(vec![json_value(&value)?]))?;
    let appended = json_result(MergeBinaryJSON(&[selected, wrapped_value]))?;
    json_result(document.Modify(&[path], &[appended], JSONModifySet))
}

/// JSON_ARRAY_APPEND 向量化。
pub fn vec_json_array_append(
    documents: &[Option<BinaryJSON>],
    paths: &[StringColumn],
    values: &[JsonColumn],
) -> JsonVecResult<JsonColumn> {
    let rows = documents.len();
    pair_columns_len(paths, values, rows)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let Some(mut document) = documents[row].clone() else {
            result.push(None);
            continue;
        };
        let mut row_is_null = false;
        for (path_column, value_column) in paths.iter().zip(values) {
            let Some(path) = &path_column[row] else {
                row_is_null = true;
                break;
            };
            let value = value_column[row].clone().unwrap_or_else(json_null);
            document = append_json_array(document, path, value)?;
        }
        result.push((!row_is_null).then_some(document));
    }
    Ok(result)
}

/// JSON_UNQUOTE 向量化。
pub fn vec_json_unquote(strings: &[Option<String>]) -> JsonVecResult<StringColumn> {
    strings
        .iter()
        .map(|string| match string {
            None => Ok(None),
            Some(string) => {
                let bytes = string.as_bytes();
                if bytes.len() >= 2
                    && bytes.first() == Some(&b'"')
                    && bytes.last() == Some(&b'"')
                    && serde_json::from_str::<Value>(string).is_err()
                {
                    return Err(JsonVecError::new(
                        "invalid JSON text: document root must not be followed by other values",
                    ));
                }
                json_result(UnquoteString(string.clone())).map(Some)
            }
        })
        .collect()
}

/// JSON_PRETTY 向量化。
pub fn vec_json_pretty(documents: &[Option<BinaryJSON>]) -> JsonVecResult<StringColumn> {
    documents
        .iter()
        .map(|document| match document {
            Some(document) => serde_json::to_string_pretty(&json_value(document)?)
                .map(Some)
                .map_err(|error| JsonVecError::new(error.to_string())),
            None => Ok(None),
        })
        .collect()
}

/// JSON_MERGE_PATCH 向量化；任一侧 SQL NULL 则行结果 NULL。
pub fn vec_json_merge_patch(arguments: &[JsonColumn]) -> JsonVecResult<JsonColumn> {
    let rows = columns_len(arguments)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        let values = arguments
            .iter()
            .map(|column| column[row].as_ref())
            .collect::<Vec<_>>();
        result.push(json_result(MergePatchBinaryJSON(&values))?);
    }
    Ok(result)
}

/// Every signature in the Go source reports that it is vectorized.
/// Go 侧所有 JSON 向量化签名均声明 vectorized=true。
pub const fn vectorized() -> bool {
    true
}
