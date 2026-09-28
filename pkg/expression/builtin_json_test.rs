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

// JSON 标量内置函数单元测试（对应 Go `builtin_json_test.go`）。
//
// 逐函数校验 TYPE/QUOTE/EXTRACT/修改类/合并/构造/CONTAINS/存储与 SCHEMA_VALID 等行为。

use crate::builtin_json_kernel::{
    BinaryJSON, BinaryJSONToSerde, JsonSumType, ParseBinaryJSONFromString, json_array,
    json_array_append, json_array_insert, json_contains, json_contains_path, json_depth,
    json_extract, json_insert, json_keys, json_length, json_member_of, json_merge,
    json_merge_patch, json_merge_preserve, json_object, json_overlaps, json_pretty, json_quote,
    json_remove, json_replace, json_schema_valid, json_search, json_set, json_storage_free,
    json_storage_size, json_sum_crc32, json_type, json_unquote, json_valid_string,
    sorted_object_entries,
};
use crc32fast::hash as crc32;
use serde_json::{Value, json};

/// 解析 JSON 文本为 BinaryJSON 测试夹具。
fn document(text: &str) -> BinaryJSON {
    ParseBinaryJSONFromString(text).unwrap()
}

/// BinaryJSON 转为 serde Value 便于深度比较。
fn value(document: &BinaryJSON) -> Value {
    BinaryJSONToSerde(document).unwrap()
}

#[test]
/// JSON_TYPE：标量/数组/对象类型名。
fn test_json_type() {
    let cases = [
        ("3", "INTEGER"),
        ("3.0", "DOUBLE"),
        ("null", "NULL"),
        ("true", "BOOLEAN"),
        ("[]", "ARRAY"),
        ("{}", "OBJECT"),
    ];
    for (input, expected) in cases {
        assert_eq!(json_type(&document(input)), expected, "input: {input}");
    }
}

#[test]
/// JSON_QUOTE：字符串转义与宽字符。
fn test_json_quote() {
    let cases = [
        ("", r#""""#),
        (r#""""#, r#""\"\"""#),
        ("a", r#""a""#),
        ("3", r#""3""#),
        (r#"{"a": "b"}"#, r#""{\"a\": \"b\"}""#),
        ("hello,\"宽字符\",world", r#""hello,\"宽字符\",world""#),
        (
            "Invalid Json string\tis OK",
            r#""Invalid Json string\tis OK""#,
        ),
        (r"1\u2232\u22322", r#""1\\u2232\\u22322""#),
    ];
    for (input, expected) in cases {
        assert_eq!(json_quote(input).unwrap(), expected, "input: {input}");
    }
}

#[test]
/// JSON_UNQUOTE：去引号与「根后跟其它值」错误。
fn test_json_unquote() {
    let cases = [
        ("", ""),
        (r#""""#, ""),
        ("''", "''"),
        ("3", "3"),
        (r#"{"a": "b"}"#, r#"{"a": "b"}"#),
        (
            r#""hello,\"quoted string\",world""#,
            "hello,\"quoted string\",world",
        ),
        (r#""hello,\"宽字符\",world""#, "hello,\"宽字符\",world"),
        (r#""1\\u2232\\u22322""#, "1\\u2232\\u22322"),
        (r#""a""#, "a"),
    ];
    for (input, expected) in cases {
        assert_eq!(json_unquote(input).unwrap(), expected, "input: {input}");
    }
    for invalid in [r#"""a"""#, r#""""a""""#] {
        let error = json_unquote(invalid).unwrap_err().to_string();
        assert!(
            error.contains("The document root must not be followed by other values"),
            "{error}"
        );
    }
}

#[test]
/// JSON_SUM_CRC32：元素 CRC 累加与类型拒绝。
fn test_json_sum_crc32() {
    let expected = ["1", "2", "3"]
        .into_iter()
        .map(|item| i64::from(crc32(item.as_bytes())))
        .sum::<i64>();
    assert_eq!(
        json_sum_crc32(&document("[1,2,3]"), JsonSumType::Signed).unwrap(),
        expected
    );
    assert_eq!(
        json_sum_crc32(&document(r#"["a","b","c"]"#), JsonSumType::String).unwrap(),
        ["a", "b", "c"]
            .into_iter()
            .map(|item| i64::from(crc32(item.as_bytes())))
            .sum::<i64>()
    );
    assert!(json_sum_crc32(&document("[-1,1]"), JsonSumType::Unsigned).is_err());
    assert!(json_sum_crc32(&document(r#"[1.1,"x"]"#), JsonSumType::Double).is_err());
    assert!(json_sum_crc32(&document(r#"{"a":1}"#), JsonSumType::Signed).is_err());
}

#[test]
/// JSON_EXTRACT：多路径、缺失与非法路径。
fn test_json_extract() {
    let source = document(r#"{"a":[{"aa":[{"aaa":1}]}],"aaa":2}"#);
    assert_eq!(
        value(
            &json_extract(&source, &["$.a[0].aa[0].aaa", "$.aaa"])
                .unwrap()
                .unwrap()
        ),
        json!([1, 2])
    );
    assert!(json_extract(&source, &["$.missing"]).unwrap().is_none());
    assert!(json_extract(&source, &["$InvalidPath"]).is_err());
}

#[test]
/// SET/INSERT/REPLACE 语义差异。
fn test_json_set_insert_replace() {
    let source = document(r#"{"a":1,"nested":{"x":2}}"#);
    let set = json_set(
        &source,
        &[
            ("$.a", Some(document("3"))),
            ("$.new", None),
            ("$.nested.y", Some(document("4"))),
        ],
    )
    .unwrap();
    assert_eq!(
        value(&set),
        json!({"a":3,"nested":{"x":2,"y":4},"new":null})
    );

    let inserted = json_insert(
        &source,
        &[("$.a", Some(document("9"))), ("$.b", Some(document("2")))],
    )
    .unwrap();
    assert_eq!(value(&inserted), json!({"a":1,"b":2,"nested":{"x":2}}));

    let replaced = json_replace(
        &source,
        &[("$.a", Some(document("9"))), ("$.b", Some(document("2")))],
    )
    .unwrap();
    assert_eq!(value(&replaced), json!({"a":9,"nested":{"x":2}}));
    assert!(json_set(&source, &[("$invalid", Some(document("1")))]).is_err());
}

#[test]
/// 弃用 JSON_MERGE（与 MERGE_PRESERVE 同语义）。
fn test_json_merge() {
    let merged = json_merge(&[document(r#"{"a":1}"#), document(r#"{"a":2,"b":3}"#)]).unwrap();
    assert_eq!(value(&merged), json!({"a":[1,2],"b":3}));
}

#[test]
/// MERGE_PRESERVE：数组与对象混合合并。
fn test_json_merge_preserve() {
    let merged =
        json_merge_preserve(&[document("[1,2]"), document(r#"{"a":3}"#), document("4")]).unwrap();
    assert_eq!(value(&merged), json!([1, 2, {"a": 3}, 4]));
}

#[test]
/// JSON_ARRAY：SQL NULL → JSON null。
fn test_json_array() {
    let result = json_array(&[
        Some(document("1")),
        None,
        Some(document(r#"{"x":2}"#)),
        Some(document("[3]")),
    ])
    .unwrap();
    assert_eq!(value(&result), json!([1, null, {"x":2}, [3]]));
}

#[test]
/// JSON_OBJECT：重复键覆盖、NULL 键报错。
fn test_json_object() {
    let object = json_object(&[
        (Some("a"), Some(document("1"))),
        (Some("b"), None),
        (Some("a"), Some(document("2"))),
    ])
    .unwrap();
    assert_eq!(value(&object), json!({"a":2,"b":null}));
    assert!(json_object(&[(None, Some(document("1")))]).is_err());
}

#[test]
/// JSON_REMOVE。
fn test_json_remove() {
    let source = document(r#"{"a":[1,{"b":2}],"keep":true}"#);
    assert_eq!(
        value(&json_remove(&source, &["$.a[1].b", "$.keep"]).unwrap()),
        json!({"a":[1,{}]})
    );
    assert!(json_remove(&source, &["$invalid"]).is_err());
}

#[test]
/// MEMBER OF：数组与单值右侧。
fn test_json_member_of() {
    assert!(json_member_of(&document("2"), &document("[1,2,3]")));
    assert!(!json_member_of(&document("4"), &document("[1,2,3]")));
    assert!(json_member_of(
        &document(r#"{"a":1}"#),
        &document(r#"{"a":1}"#)
    ));
    assert!(!json_member_of(&document("2"), &document("3")));
}

#[test]
/// JSON_CONTAINS：路径选择与通配路径拒绝。
fn test_json_contains() {
    let source = document(r#"{"a":[1,2,3],"object":{"x":1,"y":2}}"#);
    assert_eq!(
        json_contains(&source, &document("[2,3]"), Some("$.a")).unwrap(),
        Some(true)
    );
    assert_eq!(
        json_contains(&source, &document(r#"{"x":1}"#), Some("$.object")).unwrap(),
        Some(true)
    );
    assert_eq!(
        json_contains(&source, &document("1"), Some("$.missing")).unwrap(),
        None
    );
    assert!(json_contains(&source, &document("1"), Some("$.a[*]")).is_err());
}

#[test]
/// JSON_OVERLAPS。
fn test_json_overlaps() {
    assert!(json_overlaps(&document("[1,2]"), &document("[2,3]")));
    assert!(!json_overlaps(&document("[1,2]"), &document("[3,4]")));
    assert!(json_overlaps(
        &document(r#"{"a":1,"b":2}"#),
        &document(r#"{"b":2,"c":3}"#)
    ));
}

#[test]
/// JSON_CONTAINS_PATH：one/all 与非法模式。
fn test_json_contains_path() {
    let source = document(r#"{"a":[1],"b":2}"#);
    assert!(json_contains_path(&source, "one", &["$.missing", "$.a[0]"]).unwrap());
    assert!(!json_contains_path(&source, "all", &["$.a", "$.missing"]).unwrap());
    assert!(json_contains_path(&source, "ALL", &["$.a", "$.b"]).unwrap());
    assert!(json_contains_path(&source, "invalid", &["$.a"]).is_err());
}

#[test]
/// JSON_LENGTH：对象/数组/标量与缺失路径。
fn test_json_length() {
    let source = document(r#"{"array":[1,2],"object":{"x":1},"scalar":3}"#);
    assert_eq!(json_length(&source, None).unwrap(), Some(3));
    assert_eq!(json_length(&source, Some("$.array")).unwrap(), Some(2));
    assert_eq!(json_length(&source, Some("$.object")).unwrap(), Some(1));
    assert_eq!(json_length(&source, Some("$.scalar")).unwrap(), Some(1));
    assert_eq!(json_length(&source, Some("$.missing")).unwrap(), None);
    assert!(json_length(&source, Some("$.array[*]")).is_err());
}

#[test]
/// JSON_KEYS 与 sorted_object_entries 键序。
fn test_json_keys() {
    let source = document(r#"{"z":1,"a":{"y":2,"x":3},"scalar":4}"#);
    assert_eq!(
        value(&json_keys(&source, None).unwrap().unwrap()),
        json!(["a", "scalar", "z"])
    );
    assert_eq!(
        value(&json_keys(&source, Some("$.a")).unwrap().unwrap()),
        json!(["x", "y"])
    );
    assert!(json_keys(&source, Some("$.scalar")).unwrap().is_none());
    assert!(json_keys(&source, Some("$.missing")).unwrap().is_none());
    assert!(json_keys(&source, Some("$.*")).is_err());

    let entries = sorted_object_entries(&source).unwrap();
    assert_eq!(
        entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>(),
        ["a", "scalar", "z"]
    );
}

#[test]
/// JSON_DEPTH。
fn test_json_depth() {
    let cases = [
        ("null", 1),
        ("1", 1),
        ("[]", 1),
        ("[1]", 2),
        (r#"{"a":[1,{"b":2}]}"#, 4),
    ];
    for (input, expected) in cases {
        assert_eq!(json_depth(&document(input)), expected, "input: {input}");
    }
}

#[test]
/// JSON_ARRAY_APPEND。
fn test_json_array_append() {
    let source = document(r#"{"a":[1],"scalar":2}"#);
    let result = json_array_append(
        &source,
        &[
            ("$.a", Some(document("[2,3]"))),
            ("$.scalar", Some(document("4"))),
            ("$.missing", Some(document("5"))),
        ],
    )
    .unwrap();
    assert_eq!(value(&result), json!({"a":[1,[2,3]],"scalar":[2,4]}));
    assert!(json_array_append(&source, &[("$.*", Some(document("1")))]).is_err());
}

#[test]
/// JSON_SEARCH：one/all 与转义校验。
fn test_json_search() {
    let source = document(r#"{"a":"abc","nested":{"b":"axb"},"other":"no"}"#);
    assert_eq!(
        value(
            &json_search(&source, "one", "a%", None, &[])
                .unwrap()
                .unwrap()
        ),
        json!("$.a")
    );
    assert_eq!(
        value(
            &json_search(&source, "all", "a%", None, &[])
                .unwrap()
                .unwrap()
        ),
        json!(["$.a", "$.nested.b"])
    );
    assert!(json_search(&source, "one", "a%", Some("xx"), &[]).is_err());
    assert!(json_search(&source, "invalid", "a%", None, &[]).is_err());
}

#[test]
/// JSON_ARRAY_INSERT。
fn test_json_array_insert() {
    let source = document(r#"{"a":[1,2],"scalar":3}"#);
    let result = json_array_insert(
        &source,
        &[
            ("$.a[1]", Some(document("9"))),
            ("$.a[99]", None),
            ("$.missing[0]", Some(document("8"))),
        ],
    )
    .unwrap();
    assert_eq!(value(&result), json!({"a":[1,9,2,null],"scalar":3}));
    assert!(json_array_insert(&source, &[("$.a[*]", Some(document("1")))]).is_err());
}

#[test]
/// JSON_VALID（字符串输入）。
fn test_json_valid() {
    for valid in ["null", "1", r#""a""#, "[]", r#"{"a":true}"#] {
        assert!(json_valid_string(valid), "input: {valid}");
    }
    for invalid in ["", "[1,", r#"{"a":}"#, "ordinary string"] {
        assert!(!json_valid_string(invalid), "input: {invalid}");
    }
}

#[test]
/// JSON_STORAGE_FREE 恒为 0。
fn test_json_storage_free() {
    for input in ["null", "1", "[]", r#"{"a":[1,true]}"#] {
        assert_eq!(json_storage_free(&document(input)), 0, "input: {input}");
    }
}

#[test]
/// JSON_STORAGE_SIZE = 载荷 + 类型码。
fn test_json_storage_size() {
    for input in ["null", "1", "[]", r#"{"a":[1,true]}"#] {
        let document = document(input);
        assert_eq!(json_storage_size(&document), document.Value.len() + 1);
    }
}

#[test]
/// JSON_PRETTY。
fn test_json_pretty() {
    assert_eq!(
        json_pretty(&document(r#"{"a":[1,true]}"#)).unwrap(),
        "{\n  \"a\": [\n    1,\n    true\n  ]\n}"
    );
    assert_eq!(json_pretty(&document("1")).unwrap(), "1");
}

#[test]
/// JSON_MERGE_PATCH：补丁与 SQL NULL 短路。
fn test_json_merge_patch() {
    let first = document(r#"{"a":1,"b":{"c":2}}"#);
    let second = document(r#"{"a":null,"b":{"d":3}}"#);
    let patched = json_merge_patch(&[Some(&first), Some(&second)])
        .unwrap()
        .unwrap();
    assert_eq!(value(&patched), json!({"b":{"c":2,"d":3}}));
    assert!(json_merge_patch(&[Some(&first), None]).unwrap().is_none());
}

#[test]
/// JSON_SCHEMA_VALID：schema 须为对象。
fn test_json_schema_valid() {
    let schema = document(
        r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}"#,
    );
    assert!(json_schema_valid(&schema, &document(r#"{"name":"TiDB"}"#)).unwrap());
    assert!(!json_schema_valid(&schema, &document(r#"{"name":42}"#)).unwrap());
    assert!(json_schema_valid(&document("true"), &document("42")).is_err());
}

#[test]
/// 重复调用 SCHEMA_VALID（缓存场景）。
fn test_json_schema_valid_cache() {
    let schema = document(r#"{"type":"integer","minimum":1}"#);
    for (input, expected) in [("1", true), ("2", true), ("0", false), (r#""1""#, false)] {
        assert_eq!(
            json_schema_valid(&schema, &document(input)).unwrap(),
            expected,
            "input: {input}"
        );
    }
}
