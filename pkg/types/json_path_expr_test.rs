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

// JSON Path 表达式解析的单元测试，对齐 Go `json_path_expr_test.go`。
//
// 覆盖通配符检测、路径合法性、String 往返，以及 pushBack 数组/键 leg。

include!("json_path_expr.rs");

/// 校验路径是否含 `*` / `**` 通配（`containsAnyAsterisk`）。
#[test]
fn TestContainsAnyAsterisk() {
    for (expression, expected) in [
        ("$.a[1]", false),
        ("$.a[*]", true),
        ("$.*[1]", true),
        ("$**.a[1]", true),
    ] {
        let path = ParseJSONPathExpr(expression).unwrap();
        assert_eq!(path.flags.containsAnyAsterisk(), expected, "{expression}");
    }
}

/// 表驱动校验路径解析成败与 leg 数量。
#[test]
fn TestValidatePathExpr() {
    let tests = [
        ("   $  ", true, 0),
        ("   $ .   key1  [  3  ]\t[*].*.key3", true, 5),
        ("   $ .   key1  [  3  ]**[*].*.key3", true, 6),
        (r#"$."key1 string"[  3  ][*].*.key3"#, true, 5),
        (
            r#"$."hello \"escaped quotes\" world\\n"[3][*].*.key3"#,
            true,
            5,
        ),
        ("$[1 to 5]", true, 1),
        ("$[2 to 1]", false, 1),
        ("$[last]", true, 1),
        ("$[1 to last]", true, 1),
        ("$[1to3]", false, 1),
        ("$[last - 5 to last - 10]", false, 1),
        (r#"$.\"escaped quotes\"[3][*].*.key3"#, false, 0),
        (r#"$.hello \"escaped quotes\" world[3][*].*.key3"#, false, 0),
        ("$NoValidLegsHere", false, 0),
        ("$        No Valid Legs Here .a.b.c", false, 0),
        ("$.a[b]", false, 0),
        ("$.*[b]", false, 0),
        ("$**.a[b]", false, 0),
        ("$.b[ 1 ].", false, 0),
        ("$.performance.txn-entry-size-limit", false, 0),
        (r#"$."performance".txn-entry-size-limit"#, false, 0),
        (r#"$."performance."txn-entry-size-limit"#, false, 0),
        (r#"$."performance."txn-entry-size-limit""#, false, 0),
        ("$[", false, 0),
        ("$a.***[3]", false, 0),
        ("$1a", false, 0),
        ("$.ѿ", false, 0),
        (r#"$."ѿ""#, true, 1),
        ("$.\"\\0\\", false, 0),
        ("$.Ѡ", false, 0),
        (r#"$."Ѡ""#, true, 1),
        ("$.µ", true, 1),
    ];

    for (expression, success, legs) in tests {
        match ParseJSONPathExpr(expression) {
            Ok(path) => {
                assert!(success, "{expression:?} unexpectedly parsed");
                assert_eq!(path.legs.len(), legs, "{expression:?}");
            }
            Err(_) => assert!(!success, "{expression:?} should parse"),
        }
    }
}

/// 校验合法路径 String() 往返等于原文。
#[test]
fn TestPathExprToString() {
    for expression in [
        "$.a[1]",
        "$.a[*]",
        "$.*[2]",
        "$**.a[3]",
        r#"$."\"hello\"""#,
        r#"$."a b""#,
        r#"$."one potato""#,
    ] {
        assert_eq!(ParseJSONPathExpr(expression).unwrap().String(), expression);
    }
}

/// 校验追加数组下标 leg 后的路径串与多值匹配标志。
#[test]
fn TestPushBackOneIndexLeg() {
    let tests = [
        ("$", 1, "$[1]", false),
        ("$.a[1]", 1, "$.a[1][1]", false),
        ("$.a[*]", 10, "$.a[*][10]", true),
        ("$.*[2]", 2, "$.*[2][2]", true),
        ("$**.a[3]", 3, "$**.a[3][3]", true),
        ("$.a[1 to 3]", 3, "$.a[1 to 3][3]", true),
        ("$.a[last-3 to last-3]", 3, "$.a[last-3 to last-3][3]", true),
        ("$**.a[3]", -3, "$**.a[3][last-2]", true),
    ];
    for (expression, index, expected, multiple) in tests {
        let path = ParseJSONPathExpr(expression)
            .unwrap()
            .pushBackOneArraySelectionLeg(jsonPathArraySelectionIndex {
                index: jsonPathArrayIndexFromStart(index),
            });
        assert_eq!(path.String(), expected, "{expression}");
        assert_eq!(path.CouldMatchMultipleValues(), multiple, "{expression}");
    }
}

/// 校验追加对象键 leg（含 `*`）后的路径串与多值匹配标志。
#[test]
fn TestPushBackOneKeyLeg() {
    let tests = [
        ("$", "aa", "$.aa", false),
        ("$.a[1]", "aa", "$.a[1].aa", false),
        ("$.a[1]", "*", "$.a[1].*", true),
        ("$.a[*]", "k", "$.a[*].k", true),
        ("$.*[2]", "bb", "$.*[2].bb", true),
        ("$**.a[3]", "cc", "$**.a[3].cc", true),
    ];
    for (expression, key, expected, multiple) in tests {
        let path = ParseJSONPathExpr(expression)
            .unwrap()
            .pushBackOneKeyLeg(key.to_owned());
        assert_eq!(path.String(), expected, "{expression}");
        assert_eq!(path.CouldMatchMultipleValues(), multiple, "{expression}");
    }
}

#[test]
fn test_unquoted_json_escapes_match_go() {
    for (input, key) in [(r"$.\u0061", "a"), (r"$.a\u0030", "a0"), (r"$.\u00b5", "µ")] {
        let path = ParseJSONPathExpr(input).unwrap();
        assert_eq!(path.legs[0].dotKey, key);
        assert_eq!(path.String(), format!("$.{key}"));
    }
    for input in [r"$.\u0030", r"$.a\n", r"$.\q", r"$.\uD800"] {
        assert!(ParseJSONPathExpr(input).is_err(), "{input}");
    }
}

#[test]
fn test_path_boundaries_and_state() {
    for (input, position) in [
        ("", 1),
        ("$.", 2),
        ("$***.a", 3),
        ("$[4294967296]", 2),
        ("$.\"Ѡ\"!", 5),
    ] {
        assert_eq!(
            ParseJSONPathExpr(input).unwrap_err().position(),
            position,
            "{input}"
        );
    }
    for input in [
        "$[4294967295]",
        "$[last-4294967295]",
        "$[last-3 to last-1]",
        "\u{85}$\u{a0}[0]",
    ] {
        assert!(ParseJSONPathExpr(input).is_ok(), "{input}");
    }
    assert_eq!(jsonPathArraySelectionAsterisk.getIndexRange(0), (0, -1));
    assert_eq!(
        jsonPathArraySelectionIndex { index: 5 }.getIndexRange(3),
        (5, 2)
    );
    assert_eq!(
        jsonPathArraySelectionRange { start: -5, end: 8 }.getIndexRange(3),
        (-2, 2)
    );
    let path = ParseJSONPathExpr("$.*[1 to 3]**.a").unwrap();
    let (_, child) = path.popOneLeg();
    assert!(child.flags.containsAnyRange());
    assert!(child.flags.containsAnyAsterisk());
    let (_, child) = child.popOneLeg();
    assert!(!child.flags.containsAnyRange());
    let (_, child) = child.popOneLeg();
    assert!(!child.CouldMatchMultipleValues());
    let (parent, last) = child.popOneLastLeg();
    assert_eq!(parent.String(), "$");
    assert_eq!(last.dotKey, "a");
}

#[test]
fn test_cache_copy_eviction_and_concurrent_access() {
    let cache = JSONPathExpressionCache::default();
    for i in 0..PATH_CACHE_CAPACITY {
        cache.put(i.to_string(), ParseJSONPathExpr("$.a").unwrap());
    }
    let mut copy = cache.get("0").unwrap();
    copy.legs[0].dotKey = "changed".into();
    cache.put("new".into(), copy);
    assert!(cache.get("1").is_none());
    assert_eq!(cache.get("0").unwrap().String(), "$.a");
    assert_eq!(cache.lock().entries.len(), PATH_CACHE_CAPACITY);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..100 {
                    let mut path = ParseJSONPathExpr(r"$.\u0061[*]").unwrap();
                    assert_eq!(path.String(), "$.a[*]");
                    path.legs.clear();
                }
            });
        }
    });
}

#[test]
fn test_trailing_escape_error_position_matches_go() {
    assert_eq!(ParseJSONPathExpr("$.\"a\\").unwrap_err().position(), 6);
}

#[test]
fn test_surrogate_pair_go_replacement() {
    for input in [
        r#"$."\ud800\u0061""#,
        r#"$."\udc00\ud800""#,
        r#"$."\ud800\ud800""#,
    ] {
        assert_eq!(ParseJSONPathExpr(input).unwrap().legs[0].dotKey, "\u{fffd}");
    }
    assert_eq!(
        ParseJSONPathExpr(r#"$."\ud83d\ude00""#).unwrap().legs[0].dotKey,
        "😀"
    );
    assert!(ParseJSONPathExpr(r#"$."\ud800""#).is_err());
}

#[test]
fn test_surrogate_pair_followed_by_escape() {
    assert_eq!(
        ParseJSONPathExpr(r#"$."\ud83d\ude00\u0061""#).unwrap().legs[0].dotKey,
        "😀a"
    );
}
