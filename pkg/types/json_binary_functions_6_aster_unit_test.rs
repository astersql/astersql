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

// 二进制 JSON 函数（Unquote/Quote/Extract/Modify/Merge 等）的 Aster 对照单元测试。
//
// 用固定用例验证与 Go `types` 包 JSON 函数行为一致，覆盖 Unicode 转义、
// 路径抽取/修改/删除、比较、合并、包含/重叠、深度、Walk 与 Search。

use crate::json_functions::*;
use serde_json::json;

/// 将 JSON 文本解析为 BinaryJSON，解析失败则 panic。
fn bj(text: &str) -> BinaryJSON {
    ParseBinaryJSONFromString(text).expect("valid JSON")
}

/// 校验 Unquote 与 Quote 对 Unicode / 代理对 / 非法转义的处理。
#[test]
fn unicode_unquote_and_quote_match_go_cases() {
    assert_eq!(UnquoteString(r#""0\u597d0""#.to_owned()).unwrap(), "0好0");
    assert_eq!(UnquoteString(r#""\ud83e\udd21""#.to_owned()).unwrap(), "🤡");
    assert!(UnquoteString(r#""\u59""#.to_owned()).is_err());
    for (input, expected) in [
        ("true", "true"),
        ("3", r#""3""#),
        ("你", r#""你""#),
        ("µ", "µ"),
        ("Ѡ", r#""Ѡ""#),
    ] {
        assert_eq!(QuoteJSONStringForTest(input.to_owned()), expected);
    }
}

/// 校验 Extract → Modify(Set) → Remove → ArrayInsert 链路与 Go 结果一致。
#[test]
fn extract_modify_remove_and_array_insert_match_go() {
    let source = bj(r#"{"a":[1,{"x":"y"},3],"b":true}"#);
    let extracted = source
        .Extract(&[ParseJSONPathExpr("$.a[1].x").unwrap()])
        .expect("extract");
    assert_eq!(BinaryJSONToSerde(&extracted.unwrap()).unwrap(), json!("y"));

    let set = source
        .Modify(
            &[ParseJSONPathExpr("$.a[1].x").unwrap()],
            &[bj(r#""z""#)],
            JSONModifySet,
        )
        .unwrap();
    let removed = set.Remove(&[ParseJSONPathExpr("$.b").unwrap()]).unwrap();
    assert_eq!(
        BinaryJSONToSerde(&removed).unwrap(),
        json!({"a": [1, {"x": "z"}, 3]})
    );

    let inserted = removed
        .ArrayInsert(ParseJSONPathExpr("$.a[1]").unwrap(), bj("2"))
        .unwrap();
    assert_eq!(
        BinaryJSONToSerde(&inserted).unwrap(),
        json!({"a": [1, 2, {"x": "z"}, 3]})
    );
}

/// 校验 Compare / Merge / Contains / Overlaps / Depth / PeekBytesAsJSON。
#[test]
fn compare_merge_contains_overlap_depth_and_peek_match_go() {
    assert_eq!(CompareBinaryJSON(&bj("9.0"), &bj("9")), 0);
    assert_eq!(
        CompareBinaryJSON(
            &bj("-1"),
            &CreateBinaryJSON(serde_json::json!(u64::MAX)).unwrap()
        ),
        -1
    );

    let merged = MergeBinaryJSON(&[bj(r#"{"a":1}"#), bj(r#"{"a":2,"b":3}"#)]).unwrap();
    assert_eq!(
        BinaryJSONToSerde(&merged).unwrap(),
        json!({"a": [1, 2], "b": 3})
    );
    assert!(ContainsBinaryJSON(
        &bj(r#"[1,2,[1,{"a":[2,3]}]]"#),
        &bj(r#"[1,{"a":[3]}]"#)
    ));
    assert!(OverlapsBinaryJSON(&bj(r#"[1,2]"#), &bj(r#"[4,2]"#)));
    assert_eq!(bj(r#"[10,{"a":20}]"#).GetElemDepth(), 3);

    let encoded = merged.Serialize();
    assert_eq!(PeekBytesAsJSON(&encoded).unwrap(), encoded.len());
}

/// 校验 MergePatch、Walk 路径枚举与 Search(LIKE) 命中路径。
#[test]
fn merge_patch_walk_and_search_match_go() {
    let target = bj(r#"{"title":"Goodbye","author":{"given":"John","family":"Doe"}}"#);
    let patch = bj(r#"{"title":"Hello","author":{"family":null}}"#);
    let merged = MergePatchBinaryJSON(&[Some(&target), Some(&patch)])
        .unwrap()
        .unwrap();
    assert_eq!(
        BinaryJSONToSerde(&merged).unwrap(),
        json!({"title": "Hello", "author": {"given": "John"}})
    );

    let source = bj(r#"["abc",{"x":"abc"},{"y":"bcd"}]"#);
    let mut paths = Vec::new();
    source
        .Walk(
            |path, _| {
                paths.push(path.to_string());
                Ok(false)
            },
            &[],
        )
        .unwrap();
    assert_eq!(paths, ["$", "$[0]", "$[1]", "$[1].x", "$[2]", "$[2].y"]);

    let found = source.Search("all", "a%", b'\\', &[]).unwrap().unwrap();
    assert_eq!(
        BinaryJSONToSerde(&found).unwrap(),
        json!(["$[0]", "$[1].x"])
    );
}
