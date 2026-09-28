// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Executable Rust implementation of the scalar behaviour from
// `pkg/expression/builtin_json.go`.
//
// Expression construction and row evaluation are supplied by the expression
// integration layer.  This file owns the JSON semantics used by those
// signatures and deliberately operates on TiDB's binary JSON representation.
//
// JSON 标量内置函数语义实现（对应 `builtin_json.go`）。
//
// 表达式构造与行级调度由上层完成；本文件实现 TYPE/EXTRACT/SET 等 JSON 函数语义，
// 操作对象为 TiDB 二进制 JSON（BinaryJSON：带类型码的紧凑编码，非纯文本）。
// SQL NULL（Rust `None`）与 JSON null 始终区分。

use crc32fast::hash as crc32;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;

pub use types_json_functions::{
    BinaryJSON, BinaryJSONToSerde, CompareBinaryJSON, ContainsBinaryJSON, CreateBinaryJSON,
    JSONContainsPathAll, JSONContainsPathOne, JSONModifyInsert, JSONModifyReplace, JSONModifySet,
    JSONPathExpression, JSONTypeCodeArray, JSONTypeCodeObject, MergeBinaryJSON,
    MergePatchBinaryJSON, OverlapsBinaryJSON, ParseBinaryJSONFromString, ParseJSONPathExpr,
    UnquoteString,
};

#[derive(Clone, Debug, Eq, PartialEq)]
/// JSON 内置函数求值错误（消息字符串，对齐 Go 错误文案）。
pub struct JsonBuiltinError(String);

impl JsonBuiltinError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for JsonBuiltinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for JsonBuiltinError {}

impl From<types_json_functions::JsonBinaryError> for JsonBuiltinError {
    fn from(error: types_json_functions::JsonBinaryError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for JsonBuiltinError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// JSON 内置函数结果类型别名。
pub type JsonBuiltinResult<T> = Result<T, JsonBuiltinError>;

/// 构造 JSON null 字面量（不是 SQL NULL）。
fn json_null() -> JsonBuiltinResult<BinaryJSON> {
    Ok(CreateBinaryJSON(Value::Null)?)
}

/// 将 SQL NULL 参数映射为 JSON null。
fn argument_value(value: Option<BinaryJSON>) -> JsonBuiltinResult<BinaryJSON> {
    value.map_or_else(json_null, Ok)
}

/// 批量解析 JSON Path 表达式（如 `$.a[0]`）。
fn parsed_paths(paths: &[&str]) -> JsonBuiltinResult<Vec<JSONPathExpression>> {
    paths
        .iter()
        .map(|path| ParseJSONPathExpr(path).map_err(Into::into))
        .collect()
}

/// JSON_TYPE.
/// 返回 JSON 值的类型名字符串（OBJECT/ARRAY/INTEGER 等）。
pub fn json_type(value: &BinaryJSON) -> String {
    value.Type()
}

/// JSON_EXTRACT. Multiple paths and multiple-selection paths return an array,
/// exactly as `BinaryJSON.Extract` does in the Go implementation.
/// 按路径抽取；多路径或多选路径时返回数组，行为与 Go `BinaryJSON.Extract` 一致。
pub fn json_extract(value: &BinaryJSON, paths: &[&str]) -> JsonBuiltinResult<Option<BinaryJSON>> {
    Ok(value.Extract(&parsed_paths(paths)?)?)
}

/// JSON_UNQUOTE. The extra validity check preserves the Go error for a quoted
/// document followed by another JSON value.
/// 去掉 JSON 字符串引号；额外校验「带引号文档后不得跟其它值」。
pub fn json_unquote(value: &str) -> JsonBuiltinResult<String> {
    if value.len() >= 2
        && value.as_bytes().first() == Some(&b'"')
        && value.as_bytes().last() == Some(&b'"')
        && serde_json::from_str::<Value>(value).is_err()
    {
        return Err(JsonBuiltinError::new(
            "The document root must not be followed by other values.",
        ));
    }
    Ok(UnquoteString(value.to_owned())?)
}

/// JSON_QUOTE. `serde_json` does not HTML-escape `<`, `>` or `&`, matching the
/// Go encoder after `SetEscapeHTML(false)`.
/// 将字符串编码为 JSON 字符串字面量；不对 `<>&` 做 HTML 转义。
pub fn json_quote(value: &str) -> JsonBuiltinResult<String> {
    Ok(serde_json::to_string(value)?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// JSON_SET / INSERT / REPLACE 三种修改模式。
enum ModifyMode {
    Set,
    Insert,
    Replace,
}

/// 统一修改入口：先解析全部路径再求值，使非法路径优先于非法值报错。
fn json_modify(
    document: &BinaryJSON,
    pairs: &[(&str, Option<BinaryJSON>)],
    mode: ModifyMode,
) -> JsonBuiltinResult<BinaryJSON> {
    // Go evaluates and parses all paths before it evaluates modification
    // values. Keep that ordering so a bad path wins over a bad value.
    // 先解析全部路径再求修改值，保证坏路径优先于坏值报错。
    let path_text: Vec<_> = pairs.iter().map(|(path, _)| *path).collect();
    let paths = parsed_paths(&path_text)?;
    let values = pairs
        .iter()
        .map(|(_, value)| argument_value(value.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let mode = match mode {
        ModifyMode::Set => JSONModifySet,
        ModifyMode::Insert => JSONModifyInsert,
        ModifyMode::Replace => JSONModifyReplace,
    };
    Ok(document.Modify(&paths, &values, mode)?)
}

/// JSON_SET.
/// 存在则替换，不存在则插入。
pub fn json_set(
    document: &BinaryJSON,
    pairs: &[(&str, Option<BinaryJSON>)],
) -> JsonBuiltinResult<BinaryJSON> {
    json_modify(document, pairs, ModifyMode::Set)
}

/// JSON_INSERT.
/// 仅在路径不存在时插入，已有路径不变。
pub fn json_insert(
    document: &BinaryJSON,
    pairs: &[(&str, Option<BinaryJSON>)],
) -> JsonBuiltinResult<BinaryJSON> {
    json_modify(document, pairs, ModifyMode::Insert)
}

/// JSON_REPLACE.
/// 仅替换已存在路径，不存在则忽略。
pub fn json_replace(
    document: &BinaryJSON,
    pairs: &[(&str, Option<BinaryJSON>)],
) -> JsonBuiltinResult<BinaryJSON> {
    json_modify(document, pairs, ModifyMode::Replace)
}

/// JSON_REMOVE.
/// 按路径删除文档中的元素。
pub fn json_remove(document: &BinaryJSON, paths: &[&str]) -> JsonBuiltinResult<BinaryJSON> {
    Ok(document.Remove(&parsed_paths(paths)?)?)
}

/// JSON_MERGE and JSON_MERGE_PRESERVE share the same Go evaluator.
/// JSON_MERGE_PRESERVE：合并对象/数组并保留冲突键为数组。
pub fn json_merge_preserve(values: &[BinaryJSON]) -> JsonBuiltinResult<BinaryJSON> {
    Ok(MergeBinaryJSON(values)?)
}

/// Deprecated JSON_MERGE synonym. Warning emission remains the responsibility
/// of the expression evaluation context.
/// 已弃用的 JSON_MERGE 同义词；弃用警告由求值上下文负责。
pub fn json_merge(values: &[BinaryJSON]) -> JsonBuiltinResult<BinaryJSON> {
    json_merge_preserve(values)
}

/// JSON_MERGE_PATCH. `None` is SQL NULL, which differs from JSON null.
/// RFC 7396 风格合并补丁；`None` 为 SQL NULL，不同于 JSON null。
pub fn json_merge_patch(values: &[Option<&BinaryJSON>]) -> JsonBuiltinResult<Option<BinaryJSON>> {
    Ok(MergePatchBinaryJSON(values)?)
}

/// JSON_OBJECT. A SQL NULL value becomes JSON null, a SQL NULL key is an error,
/// and later duplicate keys replace earlier keys.
/// 构造对象：SQL NULL 值→JSON null；NULL 键报错；重复键后者覆盖前者。
pub fn json_object(
    entries: &[(Option<&str>, Option<BinaryJSON>)],
) -> JsonBuiltinResult<BinaryJSON> {
    let mut object = Map::new();
    for (key, value) in entries {
        let key = key.ok_or_else(|| {
            JsonBuiltinError::new("JSON documents may not contain NULL member names")
        })?;
        object.insert(
            key.to_owned(),
            BinaryJSONToSerde(&argument_value(value.clone())?)?,
        );
    }
    Ok(CreateBinaryJSON(Value::Object(object))?)
}

/// JSON_ARRAY. SQL NULL arguments are represented by JSON null.
/// 构造数组；SQL NULL 参数表示为 JSON null。
pub fn json_array(values: &[Option<BinaryJSON>]) -> JsonBuiltinResult<BinaryJSON> {
    let values = values
        .iter()
        .cloned()
        .map(argument_value)
        .map(|value| value.and_then(|value| BinaryJSONToSerde(&value).map_err(Into::into)))
        .collect::<Result<Vec<_>, JsonBuiltinError>>()?;
    Ok(CreateBinaryJSON(Value::Array(values))?)
}

/// 规范化 one/all 参数（大小写不敏感），非法则报错。
fn normalized_one_or_all(value: &str, function: &str) -> JsonBuiltinResult<&'static str> {
    if value.eq_ignore_ascii_case(JSONContainsPathOne) {
        Ok(JSONContainsPathOne)
    } else if value.eq_ignore_ascii_case(JSONContainsPathAll) {
        Ok(JSONContainsPathAll)
    } else {
        Err(JsonBuiltinError::new(format!(
            "{function} expects 'one' or 'all'"
        )))
    }
}

/// JSON_CONTAINS_PATH.
/// 检查文档是否包含给定路径；`one` 任一命中即可，`all` 须全部存在。
pub fn json_contains_path(
    document: &BinaryJSON,
    one_or_all: &str,
    paths: &[&str],
) -> JsonBuiltinResult<bool> {
    let mode = normalized_one_or_all(one_or_all, "json_contains_path")?;
    let mut contains = true;
    for path in paths {
        let exists = document.Extract(&[ParseJSONPathExpr(path)?])?.is_some();
        if mode == JSONContainsPathOne {
            if exists {
                return Ok(true);
            }
            contains = false;
        } else if !exists {
            return Ok(false);
        }
    }
    Ok(contains)
}

/// MEMBER OF. A non-array right operand is compared as a single value.
/// `MEMBER OF`：右侧非数组时按单值比较；数组则逐元素比较。
pub fn json_member_of(target: &BinaryJSON, document: &BinaryJSON) -> bool {
    if document.TypeCode != JSONTypeCodeArray {
        return CompareBinaryJSON(document, target) == 0;
    }
    BinaryJSONToSerde(document)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .is_some_and(|values| {
            values.into_iter().any(|value| {
                CreateBinaryJSON(value).is_ok_and(|value| CompareBinaryJSON(&value, target) == 0)
            })
        })
}

/// JSON_CONTAINS. `None` means SQL NULL because the requested path is absent.
/// 判断 target 是否包含于（可选路径下的）document；路径缺失返回 SQL NULL。
pub fn json_contains(
    document: &BinaryJSON,
    target: &BinaryJSON,
    path: Option<&str>,
) -> JsonBuiltinResult<Option<bool>> {
    let selected = if let Some(path) = path {
        let path = ParseJSONPathExpr(path)?;
        if path.CouldMatchMultipleValues() {
            return Err(JsonBuiltinError::new(
                "JSON path may not contain * or a range",
            ));
        }
        let Some(selected) = document.Extract(&[path])? else {
            return Ok(None);
        };
        selected
    } else {
        document.clone()
    };
    Ok(Some(ContainsBinaryJSON(&selected, target)))
}

/// JSON_OVERLAPS.
/// 两 JSON 值是否有交集（数组元素或对象键值重叠）。
pub fn json_overlaps(left: &BinaryJSON, right: &BinaryJSON) -> bool {
    OverlapsBinaryJSON(left, right)
}

/// JSON_VALID for string input. JSON-typed input is valid by construction;
/// non-string scalar inputs are handled by the expression signature as false.
/// 对字符串做 JSON 合法性检查；JSON 类型输入在签名层视为恒真。
pub fn json_valid_string(value: &str) -> bool {
    serde_json::from_str::<Value>(value).is_ok()
}

/// JSON_ARRAY_APPEND. Missing paths are ignored, scalar selected values are
/// first wrapped as an array, and an array argument is appended as one element.
/// 在路径处追加：缺失路径忽略；标量先包成数组；数组参数作为单个元素追加。
pub fn json_array_append(
    document: &BinaryJSON,
    pairs: &[(&str, Option<BinaryJSON>)],
) -> JsonBuiltinResult<BinaryJSON> {
    let mut result = document.clone();
    for (path, value) in pairs {
        let path = ParseJSONPathExpr(path)?;
        if path.CouldMatchMultipleValues() {
            return Err(JsonBuiltinError::new(
                "JSON path may not contain * or a range",
            ));
        }
        let Some(selected) = result.Extract(std::slice::from_ref(&path))? else {
            continue;
        };
        // 选中值若非数组则先包装为单元素数组，再 push 新值。
        let new_value = BinaryJSONToSerde(&argument_value(value.clone())?)?;
        let selected = BinaryJSONToSerde(&selected)?;
        let mut array = match selected {
            Value::Array(array) => array,
            scalar => vec![scalar],
        };
        array.push(new_value);
        let replacement = CreateBinaryJSON(Value::Array(array))?;
        result = result.Modify(&[path], &[replacement], JSONModifySet)?;
    }
    Ok(result)
}

/// JSON_ARRAY_INSERT. Missing or non-array parents are unchanged.
/// 在数组指定下标插入；父路径缺失或非数组则不变。
pub fn json_array_insert(
    document: &BinaryJSON,
    pairs: &[(&str, Option<BinaryJSON>)],
) -> JsonBuiltinResult<BinaryJSON> {
    let mut result = document.clone();
    for (path, value) in pairs {
        let path = ParseJSONPathExpr(path)?;
        if path.CouldMatchMultipleValues() {
            return Err(JsonBuiltinError::new(
                "JSON path may not contain * or a range",
            ));
        }
        result = result.ArrayInsert(path, argument_value(value.clone())?)?;
    }
    Ok(result)
}

/// JSON_PRETTY.
/// 美化打印 JSON 文本。
pub fn json_pretty(document: &BinaryJSON) -> JsonBuiltinResult<String> {
    Ok(serde_json::to_string_pretty(&BinaryJSONToSerde(document)?)?)
}

/// JSON_SEARCH. Empty or omitted escape strings use `\\`; any other escape
/// must contain exactly one byte, matching the Go evaluator.
/// 按 LIKE 风格在 JSON 中搜索字符串；空转义默认 `\\`，其它转义须恰好一字节。
pub fn json_search(
    document: &BinaryJSON,
    one_or_all: &str,
    search: &str,
    escape: Option<&str>,
    paths: &[&str],
) -> JsonBuiltinResult<Option<BinaryJSON>> {
    let mode = normalized_one_or_all(one_or_all, "json_search")?;
    let escape = match escape {
        None | Some("") => b'\\',
        Some(value) if value.len() == 1 => value.as_bytes()[0],
        Some(_) => return Err(JsonBuiltinError::new("Incorrect arguments to ESCAPE")),
    };
    Ok(document.Search(mode, search, escape, &parsed_paths(paths)?)?)
}

/// JSON_STORAGE_FREE. TiDB does not support partial JSON updates, so Go always
/// returns zero after validating the document.
/// TiDB 不支持部分更新 JSON，校验文档后恒返回 0。
pub fn json_storage_free(_document: &BinaryJSON) -> usize {
    0
}

/// JSON_STORAGE_SIZE is the binary payload plus its one-byte type code.
/// 存储大小：二进制载荷长度 + 1 字节类型码。
pub fn json_storage_size(document: &BinaryJSON) -> usize {
    document.Value.len() + 1
}

/// JSON_DEPTH.
/// 文档最大嵌套深度。
pub fn json_depth(document: &BinaryJSON) -> usize {
    document.GetElemDepth()
}

/// JSON_KEYS. A missing path or a selected non-object returns SQL NULL.
/// 返回对象键数组；路径缺失或选中非对象时为 SQL NULL。
pub fn json_keys(
    document: &BinaryJSON,
    path: Option<&str>,
) -> JsonBuiltinResult<Option<BinaryJSON>> {
    let selected = if let Some(path) = path {
        let path = ParseJSONPathExpr(path)?;
        if path.CouldMatchMultipleValues() {
            return Err(JsonBuiltinError::new(
                "JSON path may not contain * or a range",
            ));
        }
        let Some(selected) = document.Extract(&[path])? else {
            return Ok(None);
        };
        selected
    } else {
        document.clone()
    };
    let Value::Object(object) = BinaryJSONToSerde(&selected)? else {
        return Ok(None);
    };
    let keys = object.keys().cloned().map(Value::String).collect();
    Ok(Some(CreateBinaryJSON(Value::Array(keys))?))
}

/// JSON_LENGTH. Scalars have length one; an absent path returns SQL NULL.
/// 数组/对象元素个数；标量为 1；路径不存在为 SQL NULL。
pub fn json_length(document: &BinaryJSON, path: Option<&str>) -> JsonBuiltinResult<Option<usize>> {
    let selected = if let Some(path) = path {
        let path = ParseJSONPathExpr(path)?;
        if path.CouldMatchMultipleValues() {
            return Err(JsonBuiltinError::new(
                "JSON path may not contain * or a range",
            ));
        }
        let Some(selected) = document.Extract(&[path])? else {
            return Ok(None);
        };
        selected
    } else {
        document.clone()
    };
    Ok(Some(match BinaryJSONToSerde(&selected)? {
        Value::Array(values) => values.len(),
        Value::Object(values) => values.len(),
        _ => 1,
    }))
}

/// JSON_SCHEMA_VALID. The Go qri schema decoder requires the schema argument
/// to be an object; preserve that restriction before using the mature Rust
/// JSON Schema validator.
/// 用 schema（须为对象）校验 document；对齐 Go 对 schema 类型的限制。
pub fn json_schema_valid(schema: &BinaryJSON, document: &BinaryJSON) -> JsonBuiltinResult<bool> {
    let schema = BinaryJSONToSerde(schema)?;
    if !schema.is_object() {
        return Err(JsonBuiltinError::new(
            "Invalid data type for JSON data in argument 1 to function json_schema_valid; a JSON object is required",
        ));
    }
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| JsonBuiltinError::new(error.to_string()))?;
    Ok(validator.is_valid(&BinaryJSONToSerde(document)?))
}

/// Target element types accepted by JSON_SUM_CRC32. Go rejects JSON, YEAR,
/// FLOAT and DECIMAL arrays during signature construction.
/// JSON_SUM_CRC32 接受的元素目标类型；JSON/YEAR/FLOAT/DECIMAL 在签名构造时拒绝。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonSumType {
    Signed,
    Unsigned,
    Double,
    String,
    Boolean,
}

/// 将数组元素按目标类型格式化为 Go `%v` 风格字符串，供 CRC32 哈希。
fn display_sum_item(value: &Value, target: JsonSumType) -> JsonBuiltinResult<String> {
    let invalid =
        || JsonBuiltinError::new(format!("Invalid JSON value for CAST to type {target:?}"));
    match target {
        JsonSumType::Signed => match value {
            Value::Number(value) => value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
                .map(|value| value.to_string())
                .ok_or_else(invalid),
            Value::String(value) => value
                .parse::<i64>()
                .map(|value| value.to_string())
                .map_err(|_| invalid()),
            Value::Bool(value) => Ok(i64::from(*value).to_string()),
            _ => Err(invalid()),
        },
        JsonSumType::Unsigned => match value {
            Value::Number(value) => value
                .as_u64()
                .map(|value| value.to_string())
                .ok_or_else(invalid),
            Value::String(value) => value
                .parse::<u64>()
                .map(|value| value.to_string())
                .map_err(|_| invalid()),
            Value::Bool(value) => Ok(u64::from(*value).to_string()),
            _ => Err(invalid()),
        },
        JsonSumType::Double => match value {
            Value::Number(value) => value
                .as_f64()
                .map(|value| value.to_string())
                .ok_or_else(invalid),
            Value::String(value) => value
                .parse::<f64>()
                .map(|value| value.to_string())
                .map_err(|_| invalid()),
            Value::Bool(value) => Ok(if *value { "1" } else { "0" }.to_owned()),
            _ => Err(invalid()),
        },
        JsonSumType::String => match value {
            Value::String(value) => Ok(value.clone()),
            Value::Null | Value::Array(_) | Value::Object(_) => Err(invalid()),
            value => Ok(value.to_string()),
        },
        JsonSumType::Boolean => match value {
            Value::Bool(value) => Ok(value.to_string()),
            Value::Number(value) => Ok((value.as_f64().unwrap_or_default() != 0.0).to_string()),
            Value::String(value) if value == "0" || value.eq_ignore_ascii_case("false") => {
                Ok("false".to_owned())
            }
            Value::String(_) => Ok("true".to_owned()),
            _ => Err(invalid()),
        },
    }
}

/// JSON_SUM_CRC32. Each converted element is formatted as Go `%v`, hashed by
/// IEEE CRC32, then accumulated into a signed 64-bit result.
/// 对数组各元素 CAST 后做 IEEE CRC32 并累加为有符号 64 位。
pub fn json_sum_crc32(document: &BinaryJSON, target: JsonSumType) -> JsonBuiltinResult<i64> {
    let Value::Array(values) = BinaryJSONToSerde(document)? else {
        return Err(JsonBuiltinError::new(
            "Invalid data type for JSON data in argument 1 to function JSON_SUM_CRC32",
        ));
    };
    values.into_iter().try_fold(0_i64, |sum, value| {
        let item = display_sum_item(&value, target)?;
        Ok(sum + i64::from(crc32(item.as_bytes())))
    })
}

/// Deterministic helper used by callers that need object-key order identical
/// to TiDB binary JSON encoding.
/// 按键排序返回对象条目，便于与 TiDB 二进制 JSON 编码顺序对齐。
pub fn sorted_object_entries(document: &BinaryJSON) -> JsonBuiltinResult<Vec<(String, Value)>> {
    let Value::Object(object) = BinaryJSONToSerde(document)? else {
        return Err(JsonBuiltinError::new("JSON value is not an object"));
    };
    let sorted: BTreeMap<_, _> = object.into_iter().collect();
    Ok(sorted.into_iter().collect())
}
