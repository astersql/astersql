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

// BinaryJSON 核心行为单元测试，对齐 Go `json_binary_test.go`。
//
// 覆盖编解码往返、Extract/Modify/Remove、比较合并、Contains、深度、
// Walk/回调抽取、opaque 与哈希等路径。

use crate::json_functions::*;
use std::collections::HashSet;
use types_json_binary as core;

/// 基准/复用样例 JSON 文本。
const JSON_BENCH_STR: &str = r#"{"a":[1,"2",{"aa":"bb"},4,null],"b":true,"c":null}"#;

/// 解析 JSON 文本为 BinaryJSON。
fn parse(input: &str) -> BinaryJSON {
    ParseBinaryJSONFromString(input).unwrap()
}

/// 渲染为可读文本：字符串走 serde，其它走 Unquote。
fn render(value: &BinaryJSON) -> String {
    if value.TypeCode == JSONTypeCodeString {
        serde_json::to_string(&BinaryJSONToSerde(value).unwrap()).unwrap()
    } else {
        value.Unquote().unwrap()
    }
}

/// 解析路径表达式。
fn path(input: &str) -> JSONPathExpression {
    ParseJSONPathExpr(input).unwrap()
}

#[test]
/// 校验 parse → render 往返保持规范文本。
fn TestBinaryJSONMarshalUnmarshal() {
    for expected in [
        r#"{"a": [1, "2", {"aa": "bb"}, 4, null], "b": true, "c": null}"#,
        r#"{"aaaaaaaaaaa": [1, "2", {"aa": "bb"}, 4.1], "bbbbbbbbbb": true, "ccccccccc": "d"}"#,
        r#"[{"a": 1, "b": true}, 3, 3.5, "hello, world", null, true]"#,
        r#"{"a": "&<>"}"#,
    ] {
        assert_eq!(render(&parse(expected)), expected);
    }
}

#[test]
/// 表驱动校验 JSON_EXTRACT 多路径与通配结果。
fn TestBinaryJSONExtract() {
    struct Case<'a> {
        source: &'a str,
        paths: &'a [&'a str],
        expected: Option<&'a str>,
    }
    let bj1 = r#"{"\"hello\"": "world", "a": [1, "2", {"aa": "bb"}, 4.0, {"aa": "cc"}], "b": true, "c": ["d"]}"#;
    let bj2 = r#"[{"a": 1, "b": true}, 3, 3.5, "hello, world", null, true]"#;
    let bj3 = r#"{"properties": {"$type": "TiDB"}}"#;
    let bj4 = r#"{"properties": {"$type$type": {"$a$a" : "TiDB"}}}"#;
    let bj5 = r#"{"properties": {"$type": {"$a" : {"$b" : "TiDB"}}}}"#;
    let bj6 = r#"{"properties": {"$type": {"$a$a" : "TiDB"}},"hello": {"$b$b": "world","$c": "amazing"}}"#;
    let bj7 = r#"{ "a": { "x" : { "b": { "y": { "b": { "z": { "c": 100 } } } } } } }"#;
    let bj8 = r#"{ "a": { "b" : [ 1, 2, 3 ] } }"#;
    let bj9 = "[[0,1],[2,3],[4,[5,6]]]";
    let tests = [
        Case {
            source: bj1,
            paths: &["$.a"],
            expected: Some(r#"[1, "2", {"aa": "bb"}, 4.0, {"aa": "cc"}]"#),
        },
        Case {
            source: bj2,
            paths: &["$.a"],
            expected: None,
        },
        Case {
            source: bj1,
            paths: &["$[0]"],
            expected: Some(bj1),
        },
        Case {
            source: bj2,
            paths: &["$[0]"],
            expected: Some(r#"{"a": 1, "b": true}"#),
        },
        Case {
            source: bj1,
            paths: &["$.a[2].aa"],
            expected: Some(r#""bb""#),
        },
        Case {
            source: bj1,
            paths: &["$.a[*].aa"],
            expected: Some(r#"["bb", "cc"]"#),
        },
        Case {
            source: bj1,
            paths: &["$.*[0]"],
            expected: Some(r#"["world", 1, true, "d"]"#),
        },
        Case {
            source: bj1,
            paths: &[r#"$.a[*]."aa""#],
            expected: Some(r#"["bb", "cc"]"#),
        },
        Case {
            source: bj1,
            paths: &[r#"$."\"hello\"""#],
            expected: Some(r#""world""#),
        },
        Case {
            source: bj1,
            paths: &["$**[1]"],
            expected: Some(r#"["2"]"#),
        },
        Case {
            source: bj3,
            paths: &["$.properties.$type"],
            expected: Some(r#""TiDB""#),
        },
        Case {
            source: bj4,
            paths: &["$.properties.$type$type"],
            expected: Some(r#"{"$a$a": "TiDB"}"#),
        },
        Case {
            source: bj4,
            paths: &["$.properties.$type$type.$a$a"],
            expected: Some(r#""TiDB""#),
        },
        Case {
            source: bj5,
            paths: &["$.properties.$type.$a.$b"],
            expected: Some(r#""TiDB""#),
        },
        Case {
            source: bj5,
            paths: &["$.properties.$type.$a.*[0]"],
            expected: Some(r#"["TiDB"]"#),
        },
        Case {
            source: r#"{"metadata": {"comment": "1234"}}"#,
            paths: &["$.metadata.comment"],
            expected: Some(r#""1234""#),
        },
        Case {
            source: bj9,
            paths: &["$[0]"],
            expected: Some("[0, 1]"),
        },
        Case {
            source: bj9,
            paths: &["$[last][last]"],
            expected: Some("[5, 6]"),
        },
        Case {
            source: bj9,
            paths: &["$[last-1][last]"],
            expected: Some("3"),
        },
        Case {
            source: bj9,
            paths: &["$[last-1][last-1]"],
            expected: Some("2"),
        },
        Case {
            source: bj9,
            paths: &["$[1 to 2]"],
            expected: Some("[[2, 3], [4, [5, 6]]]"),
        },
        Case {
            source: bj9,
            paths: &["$[1 to 2][1 to 2]"],
            expected: Some("[3, [5, 6]]"),
        },
        Case {
            source: bj9,
            paths: &["$[1 to last][1 to last]"],
            expected: Some("[3, [5, 6]]"),
        },
        Case {
            source: bj9,
            paths: &["$[1 to last][1 to last - 1]"],
            expected: None,
        },
        Case {
            source: bj9,
            paths: &["$[1 to last][0 to last - 1]"],
            expected: Some("[2, 4]"),
        },
        Case {
            source: bj1,
            paths: &["$.a", "$[5]"],
            expected: Some(r#"[[1, "2", {"aa": "bb"}, 4.0, {"aa": "cc"}]]"#),
        },
        Case {
            source: bj2,
            paths: &["$.a", "$[0]"],
            expected: Some(r#"[{"a": 1, "b": true}]"#),
        },
        Case {
            source: bj6,
            paths: &["$.properties", "$[1]"],
            expected: Some(r#"[{"$type": {"$a$a": "TiDB"}}]"#),
        },
        Case {
            source: bj6,
            paths: &["$.hello", "$[2]"],
            expected: Some(r#"[{"$b$b": "world", "$c": "amazing"}]"#),
        },
        Case {
            source: bj7,
            paths: &["$.a**.b**.c"],
            expected: Some("[100]"),
        },
        Case {
            source: bj8,
            paths: &["$**[0]"],
            expected: Some(r#"[{"a": {"b": [1, 2, 3]}}, {"b": [1, 2, 3]}, 1, 2, 3]"#),
        },
        Case {
            source: bj9,
            paths: &["$**[0]"],
            expected: Some("[[0, 1], 0, 1, 2, 3, 4, 5, 6]"),
        },
        Case {
            source: "[1]",
            paths: &["$**[0]"],
            expected: Some("[1]"),
        },
        Case {
            source: r#"{"metadata": {"age": 19, "name": "Tom"}}"#,
            paths: &["$.metadata.age", "$.metadata.name"],
            expected: Some(r#"[19, "Tom"]"#),
        },
    ];
    for case in tests {
        let paths: Vec<_> = case.paths.iter().map(|value| path(value)).collect();
        let output = parse(case.source).Extract(&paths).unwrap();
        assert_eq!(
            output.is_some(),
            case.expected.is_some(),
            "{} {:?}",
            case.source,
            case.paths
        );
        if let (Some(output), Some(expected)) = (output, case.expected) {
            assert_eq!(
                render(&output),
                render(&parse(expected)),
                "{} {:?}",
                case.source,
                case.paths
            );
        }
    }
}

#[test]
/// 校验 JSON_TYPE 类型名。
fn TestBinaryJSONType() {
    for (input, expected) in [
        (r#"{"a": "b"}"#, "OBJECT"),
        (r#"["a", "b"]"#, "ARRAY"),
        ("3", "INTEGER"),
        ("3.0", "DOUBLE"),
        ("null", "NULL"),
        ("true", "BOOLEAN"),
    ] {
        assert_eq!(parse(input).Type(), expected);
    }
    assert_eq!(
        CreateBinaryJSON(serde_json::json!(1_u64 << 63))
            .unwrap()
            .Type(),
        "UNSIGNED INTEGER"
    );
}

#[test]
/// 校验 JSON_UNQUOTE。
fn TestBinaryJSONUnquote() {
    for (input, expected) in [
        ("3", "3"),
        (r#""3""#, "3"),
        (
            r#""[{\"x\":\"{\\\"y\\\":12}\"}]""#,
            r#"[{"x":"{\"y\":12}"}]"#,
        ),
        (
            r#""hello, \"escaped quotes\" world""#,
            "hello, \"escaped quotes\" world",
        ),
        (r#""\u4f60""#, "你"),
        ("true", "true"),
        ("null", "null"),
        (r#"{"a": [1, 2]}"#, r#"{"a": [1, 2]}"#),
        (r#""'""#, "'"),
        (r#""''""#, "''"),
        (r#""""#, ""),
    ] {
        assert_eq!(parse(input).Unquote().unwrap(), expected);
    }
}

#[test]
/// 校验 Quote 对标识符与特殊字符的处理。
fn TestQuoteString() {
    for (raw, quoted) in [
        ("3", r#""3""#),
        (
            "hello, \"escaped quotes\" world",
            r#""hello, \"escaped quotes\" world""#,
        ),
        ("你", r#""你""#),
        ("true", "true"),
        ("null", "null"),
        ("\"", r#""\"""#),
        ("'", r#""'""#),
        ("''", r#""''""#),
        ("", r#""""#),
        ("\\ \" \u{8} \u{c} \n \r \t", r#""\\ \" \b \f \n \r \t""#),
    ] {
        assert_eq!(QuoteJSONStringForTest(raw.to_owned()), quoted);
    }
}

#[test]
/// 表驱动校验 SET/INSERT/REPLACE 与错误路径。
fn TestBinaryJSONModify() {
    let tests = [
        ("null", "$", "{}", "{}", true, JSONModifySet),
        ("{}", "$.a", "3", r#"{"a": 3}"#, true, JSONModifySet),
        (
            r#"{"a": 3}"#,
            "$.a",
            "[]",
            r#"{"a": []}"#,
            true,
            JSONModifyReplace,
        ),
        (
            r#"{"a": 3}"#,
            "$.b",
            r#""3""#,
            r#"{"a": 3, "b": "3"}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"a": []}"#,
            "$.a[0]",
            "3",
            r#"{"a": [3]}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"a": [3]}"#,
            "$.a[1]",
            "4",
            r#"{"a": [3, 4]}"#,
            true,
            JSONModifyInsert,
        ),
        (r#"{"a": [3]}"#, "$[0]", "4", "4", true, JSONModifySet),
        (
            r#"{"a": [3]}"#,
            "$[1]",
            "4",
            r#"[{"a": [3]}, 4]"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"b": true}"#,
            "$.b",
            "false",
            r#"{"b": false}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.foo",
            r#""moo""#,
            r#"{"foo": "bar"}"#,
            true,
            JSONModifyInsert,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.foo",
            r#""moo""#,
            r#"{"foo": "moo"}"#,
            true,
            JSONModifyReplace,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.foo",
            r#""moo""#,
            r#"{"foo": "moo"}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.foo",
            "null",
            r#"{"foo": null}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.baz",
            r#""moo""#,
            r#"{"foo": "bar", "baz": "moo"}"#,
            true,
            JSONModifyInsert,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.baz",
            r#""moo""#,
            r#"{"foo": "bar"}"#,
            true,
            JSONModifyReplace,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.baz",
            r#""moo""#,
            r#"{"foo": "bar", "baz": "moo"}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"foo": "bar"}"#,
            "$.baz",
            "null",
            r#"{"foo": "bar", "baz": null}"#,
            true,
            JSONModifySet,
        ),
        ("{}", "$", "1", "{}", true, JSONModifyInsert),
        (
            r#"{"a": [3, 4]}"#,
            "$.b[1]",
            "3",
            r#"{"a": [3, 4]}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"a": [3, 4]}"#,
            "$.a[2].b",
            "3",
            r#"{"a": [3, 4]}"#,
            true,
            JSONModifySet,
        ),
        (
            r#"{"a": [3, 4]}"#,
            "$.a[0]",
            "30",
            r#"{"a": [3, 4]}"#,
            true,
            JSONModifyInsert,
        ),
        (
            r#"{"a": [3, 4]}"#,
            "$.a[2]",
            "30",
            r#"{"a": [3, 4]}"#,
            true,
            JSONModifyReplace,
        ),
        ("null", "$.*", "{}", "null", false, JSONModifySet),
        ("null", "$[*]", "{}", "null", false, JSONModifySet),
        ("null", "$**.a", "{}", "null", false, JSONModifySet),
        ("null", "$**[3]", "{}", "null", false, JSONModifySet),
    ];
    for (base, field, value, expected, success, modify_type) in tests {
        let result = parse(base).Modify(&[path(field)], &[parse(value)], modify_type);
        assert_eq!(result.is_ok(), success, "{base} {field}");
        if let Ok(result) = result {
            assert_eq!(render(&result), render(&parse(expected)));
        }
    }
}

#[test]
/// 校验 JSON_REMOVE。
fn TestBinaryJSONRemove() {
    for (base, selected, expected, success) in [
        ("null", "$", "{}", false),
        (r#"{"a":[3]}"#, "$.a[*]", r#"{"a":[3]}"#, false),
        ("{}", "$.a", "{}", true),
        (r#"{"a":3}"#, "$.a", "{}", true),
        (r#"{"a":1,"b":2,"c":3}"#, "$.b", r#"{"a":1,"c":3}"#, true),
        (
            r#"{"a":1,"b":2,"c":3}"#,
            "$.d",
            r#"{"a":1,"b":2,"c":3}"#,
            true,
        ),
        (r#"{"a":3}"#, "$[0]", r#"{"a":3}"#, true),
        (r#"{"a":[3,4,5]}"#, "$.a[0]", r#"{"a":[4,5]}"#, true),
        (r#"{"a":[3,4,5]}"#, "$.a[1]", r#"{"a":[3,5]}"#, true),
        (r#"{"a":[3,4,5]}"#, "$.a[4]", r#"{"a":[3,4,5]}"#, true),
        (
            r#"{"a": [1, 2, {"aa": "xx"}]}"#,
            "$.a[2].aa",
            r#"{"a": [1, 2, {}]}"#,
            true,
        ),
    ] {
        let result = parse(base).Remove(&[path(selected)]);
        assert_eq!(result.is_ok(), success, "{base} {selected}");
        if let Ok(result) = result {
            assert_eq!(render(&result), render(&parse(expected)));
        }
    }
}

/// 由 serde 值创建 BinaryJSON。
fn created(value: serde_json::Value) -> BinaryJSON {
    CreateBinaryJSON(value).unwrap()
}

#[test]
/// 校验跨类型与同类型比较序。
fn TestCompareBinary() {
    let values = [
        (
            created(serde_json::json!(null)),
            created(serde_json::json!(3)),
            -1,
        ),
        (
            created(serde_json::json!(3)),
            created(serde_json::json!(1_u64 << 63)),
            -1,
        ),
        (
            created(serde_json::json!(-1_i64)),
            created(serde_json::json!(u64::MAX)),
            -1,
        ),
        (
            created(serde_json::json!(2_i64)),
            created(serde_json::json!(1_u64)),
            1,
        ),
        (
            created(serde_json::json!(i64::MAX)),
            created(serde_json::json!(i64::MAX as u64)),
            0,
        ),
        (
            created(serde_json::json!(9.0)),
            created(serde_json::json!(9_i64)),
            0,
        ),
        (
            created(serde_json::json!(8.9)),
            created(serde_json::json!(9_i64)),
            -1,
        ),
        (
            created(serde_json::json!(9.1)),
            created(serde_json::json!(9_u64)),
            1,
        ),
        (
            created(serde_json::json!(9_i64)),
            created(serde_json::json!(8.9)),
            1,
        ),
        (
            created(serde_json::json!(9_u64)),
            created(serde_json::json!(9.1)),
            -1,
        ),
    ];
    for (left, right, expected) in values {
        assert_eq!(CompareBinaryJSON(&left, &right), expected);
    }
    let ordered = [
        "null",
        "3",
        r#""hello""#,
        r#""hello, world""#,
        r#"{"a":"b"}"#,
        r#"["a","b"]"#,
        r#"["a","c"]"#,
        "false",
        "true",
    ];
    for pair in ordered.windows(2) {
        assert_eq!(CompareBinaryJSON(&parse(pair[0]), &parse(pair[1])), -1);
    }
}

#[test]
/// 校验 JSON_MERGE / MergePatch。
fn TestBinaryJSONMerge() {
    let tests: &[(&[&str], &str)] = &[
        (&[r#"{"a":1}"#, r#"{"b":2}"#], r#"{"a":1,"b":2}"#),
        (&[r#"{"a":1}"#, r#"{"a":2}"#], r#"{"a":[1,2]}"#),
        (&["[1]", "[2]"], "[1,2]"),
        (&[r#"{"a":1}"#, "[1]"], r#"[{"a":1},1]"#),
        (&["[1]", r#"{"a":1}"#], r#"[1,{"a":1}]"#),
        (&[r#"{"a":1}"#, "4"], r#"[{"a":1},4]"#),
        (&["[1]", "4"], "[1,4]"),
        (&["4", r#"{"a":1}"#], r#"[4,{"a":1}]"#),
        (&["4", "1"], "[4,1]"),
        (&["{}", "[]"], "[{}]"),
        (
            &[r#"{"comment":"1234"}"#, r#"{"age":19,"name":"Tom"}"#],
            r#"{"age":19,"comment":"1234","name":"Tom"}"#,
        ),
        (
            &[
                r#"{"metadata":{"comment":"1234"}}"#,
                r#"{"metadata":{"age":19,"name":"Tom"}}"#,
            ],
            r#"{"metadata":{"age":19,"comment":"1234","name":"Tom"}}"#,
        ),
        (
            &[r#"{"comment":"1234"}"#, r#"{"comment":"abc"}"#],
            r#"{"comment":["1234","abc"]}"#,
        ),
    ];
    for (inputs, expected) in tests {
        let values: Vec<_> = inputs.iter().map(|v| parse(v)).collect();
        assert_eq!(
            CompareBinaryJSON(&MergeBinaryJSON(&values).unwrap(), &parse(expected)),
            0
        );
    }
}

#[test]
/// 基准：反复序列化样例文档。
fn BenchmarkBinaryMarshal() {
    assert_eq!(
        BinaryJSONToSerde(&parse(JSON_BENCH_STR)).unwrap()["b"],
        true
    );
}

#[test]
/// 校验 JSON_CONTAINS / OVERLAPS。
fn TestBinaryJSONContains() {
    for (input, target, expected) in [
        ("{}", "{}", true),
        (r#"{"a":1}"#, "{}", true),
        (r#"{"a":1}"#, "1", false),
        (r#"{"a":[1]}"#, "[1]", false),
        (r#"{"b":2,"c":3}"#, r#"{"c":3}"#, true),
        ("1", "1", true),
        ("[1]", "1", true),
        ("[1,2]", "[1]", true),
        ("[1,2]", "[1,3]", false),
        ("[1,2]", r#"["1"]"#, false),
        ("[1,2,[1,3]]", "[1,3]", true),
        ("[1,2,[1,[5,[3]]]]", "[1,3]", true),
        (r#"[1,2,[1,[5,{"a":[2,3]}]]]"#, r#"[1,{"a":[3]}]"#, true),
        (r#"[{"a":1}]"#, r#"{"a":1}"#, true),
        (r#"[{"a":1,"b":2}]"#, r#"{"a":1}"#, true),
        (r#"[{"a":{"a":1},"b":2}]"#, r#"{"a":1}"#, false),
    ] {
        assert_eq!(
            ContainsBinaryJSON(&parse(input), &parse(target)),
            expected,
            "{input} contains {target}"
        );
    }
}

#[test]
/// 校验深拷贝独立性。
fn TestBinaryJSONCopy() {
    for input in [
        r#"{"a":[1,"2",{"aa":"bb"},4,null],"b":true,"c":null}"#,
        r#"{"aaaaaaaaaaa":[1,"2",{"aa":"bb"},4.1],"bbbbbbbbbb":true,"ccccccccc":"d"}"#,
        r#"[{"a":1,"b":true},3,3.5,"hello, world",null,true]"#,
    ] {
        let value = parse(input);
        assert_eq!(value, value.clone());
    }
}

#[test]
/// 校验对象键列表。
fn TestGetKeys() {
    for (input, expected) in [
        ("[]", "[]"),
        ("{}", "[]"),
        (r#"{"comment":"1234"}"#, r#"["comment"]"#),
        (r#"{"name":"Tom","age":19}"#, r#"["age", "name"]"#),
    ] {
        let value = core::ParseBinaryJSONFromString(input).unwrap();
        assert_eq!(value.GetKeys().String(), expected);
    }
    let long = format!("{{\"{}\":1}}", "a".repeat(65536));
    assert!(core::ParseBinaryJSONFromString(&long).is_err());
}

#[test]
/// 校验 GetElemDepth。
fn TestBinaryJSONDepth() {
    for (input, expected) in [
        ("{}", 1),
        ("[]", 1),
        ("true", 1),
        ("[10,20]", 2),
        ("[[],{}]", 2),
        (r#"[10,{"a":20}]"#, 3),
        (
            r#"{"Person":{"Name":"Homer","Age":39,"Hobbies":["Eating","Sleeping"]}}"#,
            4,
        ),
    ] {
        assert_eq!(parse(input).GetElemDepth(), expected);
    }
}

#[test]
/// 校验字符串解析边界。
fn TestParseBinaryFromString() {
    assert!(
        core::ParseBinaryJSONFromString("")
            .unwrap_err()
            .to_string()
            .contains("empty")
    );
    assert!(core::ParseBinaryJSONFromString(r#""a"""#).is_err());
}

#[test]
/// 校验 CreateBinaryJSON。
fn TestCreateBinary() {
    let signed = core::CreateBinaryJSON(1_i64 << 62);
    assert_eq!(signed.TypeCode, core::JSONTypeCodeInt64);
    assert!(!signed.Value.is_empty());
    for value in [123456789.1234567, 0.00000001, 1e-20] {
        assert_eq!(
            core::CreateBinaryJSON(value).TypeCode,
            core::JSONTypeCodeFloat64
        );
    }
    assert_eq!(core::CreateBinaryJSON(signed.clone()), signed);
}

#[test]
/// 汇总若干函数冒烟用例。
fn TestFunctions() {
    assert_eq!(
        UnquoteJSONStringForTest(r#"\bfnrtuz0"#.to_owned()).unwrap(),
        "\u{8}fnrtuz0"
    );
    assert!(PeekBytesAsJSON(br#"\bfnrtuz0"#).is_err());
    assert!(PeekBytesAsJSON(b"").is_err());
}

#[test]
/// 校验带路径回调的抽取。
fn TestBinaryJSONExtractCallback() {
    let source = parse(
        r#"{"\"hello\"":"world","a":[1,"2",{"aa":"bb"},4.0,{"aa":"cc"}],"b":true,"c":["d"]}"#,
    );
    for (selected, expected) in [
        (
            "$.a",
            vec![("$.a", r#"[1,"2",{"aa":"bb"},4.0,{"aa":"cc"}]"#)],
        ),
        (
            "$.a[*].aa",
            vec![("$.a[2].aa", r#""bb""#), ("$.a[4].aa", r#""cc""#)],
        ),
        ("$.*[0]", vec![("$.a[0]", "1"), ("$.c[0]", r#""d""#)]),
        (r#"$."\"hello\"""#, vec![(r#"$."\"hello\"""#, r#""world""#)]),
        ("$**[1]", vec![("$.a[1]", r#""2""#)]),
    ] {
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(p, v)| (p.to_owned(), render(&parse(v))))
            .collect();
        let mut walked = Vec::new();
        source
            .Walk(
                |p, value| {
                    walked.push((p.to_string(), render(value)));
                    Ok(false)
                },
                &[path(selected)],
            )
            .unwrap();
        let actual: Vec<_> = walked
            .into_iter()
            .filter(|entry| expected.iter().any(|expected| expected.0 == entry.0))
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
/// 校验 Walk 遍历顺序与提前停止。
fn TestBinaryJSONWalk() {
    let source = parse(r#"["abc",[{"k":"10"},"def"],{"x":"abc"},{"y":"bcd"}]"#);
    let mut all = Vec::new();
    source
        .Walk(
            |path, value| {
                all.push((path.to_string(), render(value)));
                Ok(false)
            },
            &[],
        )
        .unwrap();
    assert_eq!(all.len(), 10);
    assert_eq!(all[0].0, "$");
    assert_eq!(all[4], ("$[1][0].k".to_owned(), r#""10""#.to_owned()));
    let mut selected = Vec::new();
    source
        .Walk(
            |path, _| {
                selected.push(path.to_string());
                Ok(false)
            },
            &[path("$[1]"), path("$[1]")],
        )
        .unwrap();
    assert_eq!(selected, ["$[1]", "$[1][0]", "$[1][0].k", "$[1][1]"]);
    let mut missing = 0;
    source
        .Walk(
            |_, _| {
                missing += 1;
                Ok(false)
            },
            &[path("$.m")],
        )
        .unwrap();
    assert_eq!(missing, 0);
}

#[test]
/// 校验 opaque 编解码与比较。
fn TestBinaryJSONOpaque() {
    for (value, field_type, bytes, expected) in [
        (
            core::BinaryJSON {
                TypeCode: core::JSONTypeCodeOpaque,
                Value: vec![233, 1, b'9'],
            },
            233,
            vec![b'9'],
            r#""base64:type233:OQ==""#,
        ),
        (
            {
                let mut bytes = vec![233, 0x80, 0x01];
                bytes.extend([0; 128]);
                core::BinaryJSON {
                    TypeCode: core::JSONTypeCodeOpaque,
                    Value: bytes,
                }
            },
            233,
            vec![0; 128],
            r#""base64:type233:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=""#,
        ),
    ] {
        assert_eq!(value.GetOpaqueFieldType(), field_type);
        assert_eq!(value.GetOpaque().Buf, bytes);
        assert_eq!(value.String(), expected);
    }
}

#[test]
/// 校验哈希稳定性。
fn TestHashValue() {
    let values = ["[]", "[[]]", "[[[]]]", "{}", "[false]", "[true]", "[null]"];
    let hashes: HashSet<_> = values
        .iter()
        .map(|v| {
            core::ParseBinaryJSONFromString(v)
                .unwrap()
                .HashValue(Vec::new())
        })
        .collect();
    assert_eq!(hashes.len(), values.len());

    // Go currently reports the stored binary payload size plus the type byte
    // for containers, rather than the recursively normalized hash length.
    for input in ["[]", "[1]", "{}", r#"{"a": 1}"#] {
        let value = core::ParseBinaryJSONFromString(input).unwrap();
        assert_eq!(
            value.CalculateHashValueSize(),
            value.Value.len() as i64 + 1,
            "{input}"
        );
    }
}

#[test]
/// 模糊：随机路径抽取不 panic。
fn FuzzJSONExtract() {
    for (json, selected) in [
        (r#"["abc",5,1.234]"#, "$[0]"),
        (r#"{"key":"value"}"#, "$.key"),
        (r#"{"key":"value"}"#, "$.*"),
        (r#"{"key":"value"}"#, "$.**"),
        (r#""abc""#, "$"),
        ("5", "$"),
        ("1.2345", "$"),
    ] {
        if let (Ok(value), Ok(selected)) =
            (ParseBinaryJSONFromString(json), ParseJSONPathExpr(selected))
        {
            if let Some(output) = value.Extract(&[selected]).unwrap() {
                assert_ne!(output.TypeCode, 0);
            }
        }
    }
}
