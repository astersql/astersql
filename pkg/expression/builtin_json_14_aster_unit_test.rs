// Copyright 2026 AsterSQL.

// JSON 标量内置函数的 Aster 单元测试（对照 Go 行为）。
//
// 覆盖 TYPE/QUOTE/EXTRACT/修改类/构造与合并/数组操作/存储与 SCHEMA_VALID 等契约。

use crate::expression_builtin_json::{
    BinaryJSON, BinaryJSONToSerde, JsonSumType, ParseBinaryJSONFromString, json_array,
    json_array_append, json_array_insert, json_contains, json_contains_path, json_depth,
    json_extract, json_insert, json_keys, json_length, json_member_of, json_merge_patch,
    json_merge_preserve, json_object, json_overlaps, json_pretty, json_quote, json_remove,
    json_replace, json_schema_valid, json_search, json_set, json_storage_free, json_storage_size,
    json_sum_crc32, json_type, json_unquote, json_valid_string,
};
use serde_json::{Value, json};

/// 解析 JSON 文本为 BinaryJSON。
fn document(text: &str) -> BinaryJSON {
    ParseBinaryJSONFromString(text).unwrap()
}

/// BinaryJSON → serde Value。
fn value(document: &BinaryJSON) -> Value {
    BinaryJSONToSerde(document).unwrap()
}

#[test]
/// TYPE / QUOTE / UNQUOTE / VALID / PRETTY。
fn type_quote_unquote_valid_and_pretty_match_go() {
    assert_eq!(json_type(&document(r#"{"a": 1}"#)), "OBJECT");
    assert_eq!(json_type(&document("42")), "INTEGER");
    assert_eq!(json_quote("a<b\n").unwrap(), r#""a<b\n""#);
    assert_eq!(json_unquote(r#""a\tb\u4f60""#).unwrap(), "a\tb你");
    assert!(json_valid_string(r#"[1,true,null]"#));
    assert!(!json_valid_string("[1,"));
    assert_eq!(
        json_pretty(&document(r#"{"a":[1,true]}"#)).unwrap(),
        "{\n  \"a\": [\n    1,\n    true\n  ]\n}"
    );
}

#[test]
/// EXTRACT、SET/INSERT/REPLACE/REMOVE 与 CONTAINS_PATH one/all。
fn extract_modify_remove_and_path_modes_match_go() {
    let source = document(r#"{"a":[1,{"b":2}],"keep":true}"#);
    assert_eq!(
        value(&json_extract(&source, &["$.a[1].b"]).unwrap().unwrap()),
        json!(2)
    );
    assert!(json_extract(&source, &["$.missing"]).unwrap().is_none());
    assert_eq!(
        value(&json_extract(&source, &["$.a[*]"]).unwrap().unwrap()),
        json!([1, {"b": 2}])
    );

    let set = json_set(&source, &[("$.a[0]", Some(document("9"))), ("$.new", None)]).unwrap();
    assert_eq!(value(&set), json!({"a":[9,{"b":2}],"keep":true,"new":null}));
    let inserted = json_insert(
        &set,
        &[
            ("$.a[0]", Some(document("8"))),
            ("$.tail", Some(document("3"))),
        ],
    )
    .unwrap();
    assert_eq!(value(&inserted)["a"][0], json!(9));
    assert_eq!(value(&inserted)["tail"], json!(3));
    let replaced = json_replace(
        &inserted,
        &[
            ("$.tail", Some(document("4"))),
            ("$.absent", Some(document("5"))),
        ],
    )
    .unwrap();
    assert_eq!(value(&replaced)["tail"], json!(4));
    assert!(value(&replaced).get("absent").is_none());
    let removed = json_remove(&replaced, &["$.a[1].b", "$.keep"]).unwrap();
    assert_eq!(value(&removed), json!({"a":[9,{}],"new":null,"tail":4}));

    assert!(json_contains_path(&source, "one", &["$.missing", "$.a[0]"]).unwrap());
    assert!(!json_contains_path(&source, "all", &["$.a[0]", "$.missing"]).unwrap());
    assert!(json_contains_path(&source, "ALL", &["$.a", "$.keep"]).unwrap());
}

#[test]
/// OBJECT/ARRAY、MERGE_PRESERVE/PATCH、MEMBER OF、CONTAINS、OVERLAPS。
fn constructors_merge_contains_and_overlap_match_go() {
    let object = json_object(&[
        (Some("a"), Some(document("1"))),
        (Some("b"), None),
        (Some("a"), Some(document("2"))),
    ])
    .unwrap();
    assert_eq!(value(&object), json!({"a":2,"b":null}));
    assert!(json_object(&[(None, Some(document("1")))]).is_err());
    assert_eq!(
        value(&json_array(&[Some(document("1")), None, Some(document(r#"{"x":2}"#))]).unwrap()),
        json!([1, null, {"x": 2}])
    );

    let preserved =
        json_merge_preserve(&[document(r#"{"a":1}"#), document(r#"{"a":2,"b":3}"#)]).unwrap();
    assert_eq!(value(&preserved), json!({"a":[1,2],"b":3}));
    let patched = json_merge_patch(&[
        Some(&document(r#"{"a":1,"b":{"c":2}}"#)),
        Some(&document(r#"{"a":null,"b":{"d":3}}"#)),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(value(&patched), json!({"b":{"c":2,"d":3}}));

    assert!(json_member_of(&document("2"), &document("[1,2,3]")));
    assert!(json_member_of(&document("2"), &document("2")));
    assert_eq!(
        json_contains(
            &document(r#"{"a":[1,2,3]}"#),
            &document("[2,3]"),
            Some("$.a")
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        json_contains(&document(r#"{"a":1}"#), &document("1"), Some("$.missing")).unwrap(),
        None
    );
    assert!(json_overlaps(
        &document(r#"{"a":1,"b":2}"#),
        &document(r#"{"b":2,"c":3}"#)
    ));
    assert!(!json_overlaps(&document("[1,2]"), &document("[3,4]")));
}

#[test]
/// ARRAY_APPEND/INSERT、DEPTH/LENGTH/KEYS、SEARCH one/all。
fn array_append_insert_search_keys_length_and_depth_match_go() {
    let source = document(r#"{"a":[1],"scalar":"abc","nested":{"x":"axb"}}"#);
    let appended = json_array_append(
        &source,
        &[
            ("$.a", Some(document("[2,3]"))),
            ("$.scalar", Some(document("4"))),
        ],
    )
    .unwrap();
    assert_eq!(value(&appended)["a"], json!([1, [2, 3]]));
    assert_eq!(value(&appended)["scalar"], json!(["abc", 4]));
    let unchanged = json_array_append(&appended, &[("$.missing", Some(document("5")))]).unwrap();
    assert_eq!(value(&unchanged), value(&appended));

    let inserted = json_array_insert(
        &appended,
        &[
            ("$.a[1]", Some(document("9"))),
            ("$.missing[0]", Some(document("8"))),
        ],
    )
    .unwrap();
    assert_eq!(value(&inserted)["a"], json!([1, 9, [2, 3]]));
    assert_eq!(json_depth(&inserted), 4);
    assert_eq!(json_length(&inserted, Some("$.a")).unwrap(), Some(3));
    assert_eq!(
        json_length(&inserted, Some("$.scalar[0]")).unwrap(),
        Some(1)
    );
    assert_eq!(json_length(&inserted, Some("$.absent")).unwrap(), None);
    assert_eq!(
        value(&json_keys(&inserted, None).unwrap().unwrap()),
        json!(["a", "nested", "scalar"])
    );
    assert_eq!(
        value(&json_keys(&inserted, Some("$.nested")).unwrap().unwrap()),
        json!(["x"])
    );

    assert_eq!(
        value(
            &json_search(&source, "one", "a%", None, &[])
                .unwrap()
                .unwrap()
        ),
        json!("$.nested.x")
    );
    assert_eq!(
        value(
            &json_search(&source, "all", "a%", None, &[])
                .unwrap()
                .unwrap()
        ),
        json!(["$.nested.x", "$.scalar"])
    );
}

#[test]
/// STORAGE_FREE/SIZE、SUM_CRC32、SCHEMA_VALID（schema 须为对象）。
fn storage_crc_and_schema_validation_match_go_contract() {
    let object = document(r#"{"a":[1,true]}"#);
    assert_eq!(json_storage_free(&object), 0);
    assert_eq!(json_storage_size(&object), object.Value.len() + 1);
    assert_eq!(
        json_sum_crc32(&document("[1,2,3]"), JsonSumType::Signed).unwrap(),
        4_505_025_631
    );

    let schema = document(
        r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}"#,
    );
    assert!(json_schema_valid(&schema, &document(r#"{"name":"TiDB"}"#)).unwrap());
    assert!(!json_schema_valid(&schema, &document(r#"{"name":42}"#)).unwrap());
    assert!(json_schema_valid(&document("true"), &document("42")).is_err());
}

#[test]
/// Go 测试覆盖的非法路径、模式与转义参数必须返回错误，不能静默退化为未命中。
fn invalid_paths_modes_and_escape_arguments_match_go_errors() {
    let source = document(r#"{"a":[1,2],"text":"abc"}"#);

    assert!(json_extract(&source, &["$InvalidPath"]).is_err());
    assert!(json_set(&source, &[("$InvalidPath", Some(document("3")))]).is_err());
    assert!(json_remove(&source, &["$"]).is_err());
    assert!(json_remove(&source, &["$.*"]).is_err());
    assert!(json_contains(&source, &document("1"), Some("$[*]")).is_err());
    assert!(json_contains_path(&source, "wrong", &["$.a"]).is_err());
    assert!(json_search(&source, "wrong", "abc", None, &[]).is_err());
    assert!(json_search(&source, "all", "abc", Some("??"), &[]).is_err());
    assert!(json_array_insert(&source, &[("$.a", Some(document("3")))]).is_err());
}
