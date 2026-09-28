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

// JSON 向量化内置函数的 Aster 单元测试。
//
// 对照 Go `builtin_json_vec`：多路径修改、元数据、构造器、MEMBER OF/CONTAINS、
// SEARCH、MERGE 弃用警告、CRC32 求和及列形状/路径错误。

use crate::expression_json_vec::*;
use serde_json::json;

/// 解析合法 JSON 文本为 BinaryJSON。
fn bj(text: &str) -> BinaryJSON {
    parse_json(text).expect("valid JSON")
}

/// BinaryJSON → serde Value。
fn value(json: &BinaryJSON) -> serde_json::Value {
    json_value(json).expect("valid binary JSON")
}

#[test]
/// SET/INSERT/REPLACE：SQL NULL 路径/文档与多 path-value 对语义。
fn modify_preserves_go_null_and_multi_pair_semantics() {
    let docs = vec![Some(bj(r#"{"a":1}"#)), Some(bj(r#"{"a":1}"#)), None];
    let paths = vec![
        vec![Some("$.a".into()), None, Some("$.a".into())],
        vec![Some("$.b".into()), Some("$.b".into()), Some("$.b".into())],
    ];
    let values = vec![
        vec![Some(bj("2")), Some(bj("2")), Some(bj("2"))],
        vec![None, Some(bj("3")), Some(bj("3"))],
    ];

    let set = vec_json_set(&docs, &paths, &values).unwrap();
    assert_eq!(value(set[0].as_ref().unwrap()), json!({"a": 2, "b": null}));
    assert!(set[1].is_none());
    assert!(set[2].is_none());

    let inserted = vec_json_insert(
        &[Some(bj(r#"{"a":1}"#))],
        &[vec![Some("$.a".into())], vec![Some("$.b".into())]],
        &[vec![Some(bj("9"))], vec![Some(bj("2"))]],
    )
    .unwrap();
    assert_eq!(
        value(inserted[0].as_ref().unwrap()),
        json!({"a": 1, "b": 2})
    );

    let replaced = vec_json_replace(
        &[Some(bj(r#"{"a":1}"#))],
        &[vec![Some("$.a".into())], vec![Some("$.b".into())]],
        &[vec![Some(bj("9"))], vec![Some(bj("2"))]],
    )
    .unwrap();
    assert_eq!(value(replaced[0].as_ref().unwrap()), json!({"a": 9}));
}

#[test]
/// STORAGE/DEPTH/TYPE/LENGTH/KEYS 列级结果。
fn scalar_metadata_matches_json_storage_depth_keys_length_and_type() {
    let docs = vec![Some(bj(r#"{"b":[1,{"x":2}],"a":3}"#)), Some(bj("7")), None];
    assert_eq!(vec_json_storage_free(&docs), vec![Some(0), Some(0), None]);
    assert_eq!(vec_json_depth(&docs), vec![Some(4), Some(1), None]);
    assert_eq!(
        vec_json_type(&docs),
        vec![Some("OBJECT".into()), Some("INTEGER".into()), None]
    );
    assert_eq!(
        vec_json_length(&docs, None).unwrap(),
        vec![Some(2), Some(1), None]
    );
    assert_eq!(
        vec_json_length(
            &docs,
            Some(&vec![Some("$.b".into()), Some("$".into()), None])
        )
        .unwrap(),
        vec![Some(2), Some(1), None]
    );
    let keys = vec_json_keys(&docs).unwrap();
    assert_eq!(value(keys[0].as_ref().unwrap()), json!(["a", "b"]));
    assert!(keys[1].is_none());
    assert!(keys[2].is_none());
    assert_eq!(vec_json_storage_size(&docs)[2], None);
    assert!(vec_json_storage_size(&docs)[0].unwrap() > 1);
}

#[test]
/// ARRAY/OBJECT：SQL NULL→JSON null；NULL 键报错。
fn array_and_object_builders_preserve_json_null_and_reject_null_keys() {
    let array = vec_json_array(&[
        vec![Some(bj("1")), None],
        vec![None, Some(bj(r#"{"x":2}"#))],
    ])
    .unwrap();
    assert_eq!(value(array[0].as_ref().unwrap()), json!([1, null]));
    assert_eq!(value(array[1].as_ref().unwrap()), json!([null, {"x": 2}]));

    let object = vec_json_object(
        &[vec![Some("a".into()), Some("x".into())]],
        &[vec![None, Some(bj("2"))]],
    )
    .unwrap();
    assert_eq!(value(object[0].as_ref().unwrap()), json!({"a": null}));
    assert_eq!(value(object[1].as_ref().unwrap()), json!({"x": 2}));
    assert!(vec_json_object(&[vec![None]], &[vec![Some(bj("1"))]]).is_err());
}

#[test]
/// MEMBER OF、CONTAINS、OVERLAPS。
fn member_contains_and_overlaps_follow_mysql_json_rules() {
    let target = vec![Some(bj("2")), Some(bj(r#"{"a":1}"#)), None];
    let candidate = vec![Some(bj("[1,2,3]")), Some(bj(r#"{"a":1}"#)), Some(bj("[]"))];
    assert_eq!(
        vec_json_member_of(&target, &candidate).unwrap(),
        vec![Some(1), Some(1), None]
    );

    let docs = vec![Some(bj(r#"{"a":[1,2],"b":3}"#))];
    let needles = vec![Some(bj("[2]"))];
    assert_eq!(
        vec_json_contains(&docs, &needles, Some(&vec![Some("$.a".into())])).unwrap(),
        vec![Some(1)]
    );
    assert_eq!(
        vec_json_overlaps(&[Some(bj("[1,2]"))], &[Some(bj("[4,2]"))]).unwrap(),
        vec![Some(1)]
    );
}

#[test]
/// QUOTE/UNQUOTE/PRETTY 边界（含非法转义）。
fn quote_unquote_and_pretty_match_go_edges() {
    let quoted = vec_json_quote(&[Some("<tag>\n\"x\"".into()), None]).unwrap();
    assert_eq!(quoted, vec![Some(r#""<tag>\n\"x\"""#.into()), None]);
    assert_eq!(
        vec_json_unquote(&[Some(r#""line\nvalue""#.into()), Some("plain".into()), None]).unwrap(),
        vec![Some("line\nvalue".into()), Some("plain".into()), None]
    );
    assert!(vec_json_unquote(&[Some(r#""broken\q""#.into())]).is_err());
    assert_eq!(
        vec_json_pretty(&[Some(bj(r#"{"a":[1,2]}"#)), None]).unwrap(),
        vec![Some("{\n  \"a\": [\n    1,\n    2\n  ]\n}".into()), None]
    );
}

#[test]
/// SEARCH：one/all、转义、路径限制与文档 NULL。
fn search_supports_one_all_escape_paths_and_nulls() {
    let docs = vec![Some(bj(r#"["abc",[{"k":"10"},"def"],{"x":"abc"}]"#)), None];
    let modes = vec![Some("all".into()), Some("one".into())];
    let patterns = vec![Some("abc".into()), Some("abc".into())];
    let found = vec_json_search(&docs, &modes, &patterns, None, &[]).unwrap();
    assert_eq!(value(found[0].as_ref().unwrap()), json!(["$[0]", "$[2].x"]));
    assert!(found[1].is_none());

    let restricted = vec_json_search(
        &docs[..1],
        &modes[..1],
        &[Some("10".into())],
        Some(&[Some("".into())]),
        &[vec![Some("$[1][0]".into())]],
    )
    .unwrap();
    assert_eq!(value(restricted[0].as_ref().unwrap()), json!("$[1][0].k"));
    assert!(vec_json_search(&docs[..1], &[Some("bad".into())], &patterns[..1], None, &[]).is_err());
}

#[test]
/// ARRAY_INSERT、EXTRACT、KEYS_AT_PATH、REMOVE。
fn insert_extract_remove_and_keys_at_path_match_go() {
    let inserted = vec_json_array_insert(
        &[Some(bj(r#"["a",{"b":[1,2]},[3,4]]"#))],
        &[vec![Some("$[2][1]".into())]],
        &[vec![Some(bj(r#"{"x":3}"#))]],
    )
    .unwrap();
    assert_eq!(
        value(inserted[0].as_ref().unwrap()),
        json!(["a", {"b":[1,2]}, [3, {"x":3}, 4]])
    );

    let docs = vec![Some(bj(r#"{"a":{"b":1,"c":2},"x":3}"#))];
    assert_eq!(
        value(
            vec_json_extract(&docs, &[vec![Some("$.a.b".into())]]).unwrap()[0]
                .as_ref()
                .unwrap()
        ),
        json!(1)
    );
    assert_eq!(
        value(
            vec_json_keys_at_path(&docs, &[Some("$.a".into())]).unwrap()[0]
                .as_ref()
                .unwrap()
        ),
        json!(["b", "c"])
    );
    assert_eq!(
        value(
            vec_json_remove(&docs, &[vec![Some("$.x".into())]]).unwrap()[0]
                .as_ref()
                .unwrap()
        ),
        json!({"a":{"b":1,"c":2}})
    );
}

#[test]
/// CONTAINS_PATH：one/all 大小写与路径 NULL。
fn contains_path_handles_one_all_case_and_missing_paths() {
    let docs = vec![Some(bj(r#"{"a":{"c":{"d":4}},"b":2}"#)); 3];
    let mode = vec![Some("one".into()), Some("aLl".into()), Some("all".into())];
    let paths = vec![
        vec![Some("$.a".into()), Some("$.a".into()), Some("$.a".into())],
        vec![Some("$.x".into()), Some("$.x".into()), None],
    ];
    assert_eq!(
        vec_json_contains_path(&docs, &mode, &paths).unwrap(),
        vec![Some(1), Some(0), None]
    );
}

#[test]
/// ARRAY_APPEND：标量装箱、数组整体追加、缺失路径忽略。
fn array_append_wraps_scalars_keeps_array_values_nested_and_ignores_missing_paths() {
    let docs = vec![
        Some(bj(r#"{"a":1}"#)),
        Some(bj("[1]")),
        Some(bj(r#"{"a":1}"#)),
    ];
    let appended = vec_json_array_append(
        &docs,
        &[vec![
            Some("$.a".into()),
            Some("$".into()),
            Some("$.missing".into()),
        ]],
        &[vec![Some(bj("2")), Some(bj("[2,3]")), Some(bj("9"))]],
    )
    .unwrap();
    assert_eq!(value(appended[0].as_ref().unwrap()), json!({"a":[1,2]}));
    assert_eq!(value(appended[1].as_ref().unwrap()), json!([1, [2, 3]]));
    assert_eq!(value(appended[2].as_ref().unwrap()), json!({"a":1}));
}

#[test]
/// MERGE/MERGE_PATCH 行级 NULL；弃用别名产生警告。
fn merge_and_merge_patch_preserve_row_null_rules() {
    let arguments = [
        vec![Some(bj(r#"{"a":1}"#)), Some(bj("1")), None],
        vec![
            Some(bj(r#"{"a":2,"b":3}"#)),
            Some(bj("[2,3]")),
            Some(bj("4")),
        ],
    ];
    let merged = vec_json_merge(&arguments).unwrap();
    assert_eq!(value(merged[0].as_ref().unwrap()), json!({"a":[1,2],"b":3}));
    assert_eq!(value(merged[1].as_ref().unwrap()), json!([1, 2, 3]));
    assert!(merged[2].is_none());
    let deprecated = vec_json_merge_with_warnings(&arguments, true).unwrap();
    assert_eq!(deprecated.values, merged);
    assert_eq!(deprecated.warnings.len(), 2);
    assert!(
        deprecated
            .warnings
            .iter()
            .all(|warning| warning.contains("JSON_MERGE"))
    );

    let patched = vec_json_merge_patch(&[
        vec![Some(bj(r#"{"a":1,"b":2}"#)), Some(bj(r#"{"a":1}"#))],
        vec![Some(bj(r#"{"a":null,"c":3}"#)), None],
    ])
    .unwrap();
    assert_eq!(value(patched[0].as_ref().unwrap()), json!({"b":2,"c":3}));
    assert!(patched[1].is_none());
}

#[test]
/// SUM_CRC32：逐元素 CRC 累加；非数组报错。
fn crc32_sum_uses_each_array_element_and_rejects_non_arrays() {
    let sums = vec_json_sum_crc32(&[Some(bj("[1,2,3]")), None], JsonCrc32Type::Signed).unwrap();
    let expected = ["1", "2", "3"]
        .into_iter()
        .map(|text| i64::from(crc32fast::hash(text.as_bytes())))
        .sum();
    assert_eq!(sums, vec![Some(expected), None]);
    assert!(vec_json_sum_crc32(&[Some(bj("1"))], JsonCrc32Type::Signed).is_err());
}

#[test]
/// 列长度不一致与非法路径不得被静默忽略。
fn column_shape_and_path_errors_are_not_silently_ignored() {
    assert!(vec_json_array(&[vec![Some(bj("1"))], vec![]]).is_err());
    assert!(vec_json_extract(&[Some(bj("{}"))], &[vec![Some("not-a-path".into())]]).is_err());
    assert!(vec_json_length(&[Some(bj("[]"))], Some(&vec![Some("$[*]".into())])).is_err());
}
