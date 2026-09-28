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

// JSON 内置函数向量化路径的单元测试。
//
// 验证列级 JSON_SET/INSERT、元数据函数、构造器、CONTAINS 等与 Go 一致的
// SQL NULL 传播及路径错误行为。`None` 表示 SQL NULL，与 JSON null 不同。

use serde_json::json;

use crate::expression_json_vec::*;

/// 将 JSON 文本解析为 TiDB 二进制 JSON（BinaryJSON）。
fn binary(text: &str) -> BinaryJSON {
    parse_json(text).expect("valid JSON fixture")
}

/// BinaryJSON 转 serde_json::Value，便于断言。
fn value(json: &BinaryJSON) -> serde_json::Value {
    json_value(json).expect("valid binary JSON")
}

#[test]
/// JSON_SET/INSERT：路径或文档为 SQL NULL 时整行结果为 NULL。
fn test_vectorized_json_modify_and_null_propagation() {
    let documents = vec![Some(binary(r#"{"a":1}"#)), Some(binary(r#"{"a":1}"#)), None];
    let paths = vec![vec![Some("$.a".into()), None, Some("$.a".into())]];
    let values = vec![vec![
        Some(binary("2")),
        Some(binary("2")),
        Some(binary("2")),
    ]];

    let set = vec_json_set(&documents, &paths, &values).unwrap();
    assert_eq!(value(set[0].as_ref().unwrap()), json!({"a": 2}));
    assert!(set[1].is_none(), "a SQL NULL path makes the row NULL");
    assert!(set[2].is_none(), "a SQL NULL document stays NULL");

    let inserted = vec_json_insert(
        &[Some(binary(r#"{"a":1}"#))],
        &[vec![Some("$.a".into())], vec![Some("$.b".into())]],
        &[vec![Some(binary("9"))], vec![None]],
    )
    .unwrap();
    assert_eq!(
        value(inserted[0].as_ref().unwrap()),
        json!({"a": 1, "b": null})
    );
}

#[test]
/// DEPTH/TYPE/LENGTH/KEYS/EXTRACT 的列级语义与 NULL。
fn test_vectorized_json_metadata_and_paths() {
    let documents = vec![Some(binary(r#"{"b":[1,{"x":2}],"a":3}"#)), None];
    assert_eq!(vec_json_depth(&documents), vec![Some(4), None]);
    assert_eq!(vec_json_type(&documents), vec![Some("OBJECT".into()), None]);
    assert_eq!(
        vec_json_length(&documents, None).unwrap(),
        vec![Some(2), None]
    );

    let keys = vec_json_keys(&documents).unwrap();
    assert_eq!(value(keys[0].as_ref().unwrap()), json!(["a", "b"]));
    assert!(keys[1].is_none());

    let extracted = vec_json_extract(
        &documents,
        &[vec![Some("$.b[1].x".into()), Some("$.b".into())]],
    )
    .unwrap();
    assert_eq!(value(extracted[0].as_ref().unwrap()), json!(2));
    assert!(extracted[1].is_none());
}

#[test]
/// ARRAY/OBJECT 构造、CONTAINS，以及 NULL 键/列长不一致/非法路径错误。
fn test_vectorized_json_array_object_contains_and_errors() {
    let array = vec_json_array(&[
        vec![Some(binary("1")), None],
        vec![None, Some(binary(r#"{"x":2}"#))],
    ])
    .unwrap();
    assert_eq!(value(array[0].as_ref().unwrap()), json!([1, null]));
    assert_eq!(value(array[1].as_ref().unwrap()), json!([null, {"x": 2}]));

    let object = vec_json_object(
        &[vec![Some("a".into()), Some("x".into())]],
        &[vec![None, Some(binary("2"))]],
    )
    .unwrap();
    assert_eq!(value(object[0].as_ref().unwrap()), json!({"a": null}));
    assert_eq!(value(object[1].as_ref().unwrap()), json!({"x": 2}));

    assert_eq!(
        vec_json_contains(
            &[Some(binary(r#"{"a":[1,2]}"#))],
            &[Some(binary("[2]"))],
            Some(&vec![Some("$.a".into())]),
        )
        .unwrap(),
        vec![Some(1)],
    );
    assert!(vec_json_object(&[vec![None]], &[vec![Some(binary("1"))]]).is_err());
    assert!(vec_json_array(&[vec![Some(binary("1"))], vec![]]).is_err());
    assert!(vec_json_extract(&[Some(binary("{}"))], &[vec![Some("bad-path".into())]]).is_err());
}
