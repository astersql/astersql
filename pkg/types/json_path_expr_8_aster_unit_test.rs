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

// JSON Path 解析的 Aster 对照单元测试（内联 `json_path_expr.rs`）。
//
// 覆盖 Go 校验表、通配/范围往返、push/pop leg，以及数组下标边界计算。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

include!("json_path_expr.rs");

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 Go 校验表：合法路径 leg 数与非法路径拒绝。
    #[test]
    fn parses_the_go_validation_table() {
        let valid = [
            ("   $  ", 0),
            ("   $ .   key1  [  3  ]\t[*].*.key3", 5),
            ("   $ .   key1  [  3  ]**[*].*.key3", 6),
            (r#"$."key1 string"[  3  ][*].*.key3"#, 5),
            (r#"$."hello \"escaped quotes\" world\\n"[3][*].*.key3"#, 5),
            ("$[1 to 5]", 1),
            ("$[last]", 1),
            ("$[1 to last]", 1),
            (r#"$."ѿ""#, 1),
            (r#"$."Ѡ""#, 1),
            ("$.µ", 1),
        ];
        for (expression, leg_count) in valid {
            let parsed = ParseJSONPathExpr(expression)
                .unwrap_or_else(|error| panic!("{expression:?} should parse: {error}"));
            assert_eq!(parsed.legs.len(), leg_count, "{expression:?}");
        }

        let invalid = [
            "$[2 to 1]",
            "$[1to3]",
            "$[last - 5 to last - 10]",
            r#"$.\"escaped quotes\"[3][*].*.key3"#,
            r#"$.hello \"escaped quotes\" world[3][*].*.key3"#,
            "$NoValidLegsHere",
            "$        No Valid Legs Here .a.b.c",
            "$.a[b]",
            "$.*[b]",
            "$**.a[b]",
            "$.b[ 1 ].",
            "$.performance.txn-entry-size-limit",
            r#"$."performance".txn-entry-size-limit"#,
            r#"$."performance."txn-entry-size-limit"#,
            r#"$."performance."txn-entry-size-limit""#,
            "$[",
            "$a.***[3]",
            "$1a",
            "$.ѿ",
            "$.Ѡ",
            "$.\"\\0\\",
            "$**",
        ];
        for expression in invalid {
            assert!(
                ParseJSONPathExpr(expression).is_err(),
                "{expression:?} should be rejected"
            );
        }
    }

    /// 校验通配/范围标志、多值匹配与 String 往返。
    #[test]
    fn tracks_wildcards_ranges_and_round_trips_like_go() {
        let cases = [
            ("$.a[1]", false, false),
            ("$.a[*]", true, true),
            ("$.*[1]", true, true),
            ("$.*[2]", true, true),
            ("$**.a[1]", true, true),
            ("$**.a[3]", true, true),
            ("$.a[1 to 3]", false, true),
            (r#"$."\"hello\"""#, false, false),
            (r#"$."a b""#, false, false),
            (r#"$."one potato""#, false, false),
        ];
        for (expression, has_asterisk, could_match_multiple) in cases {
            let parsed = ParseJSONPathExpr(expression).unwrap();
            assert_eq!(parsed.flags.containsAnyAsterisk(), has_asterisk);
            assert_eq!(parsed.CouldMatchMultipleValues(), could_match_multiple);
            assert_eq!(parsed.String(), expression);
        }
    }

    /// 对照 Go 表：追加数组下标 leg。
    #[test]
    fn push_array_selection_matches_the_go_table() {
        let cases = [
            ("$", 1, "$[1]", false),
            ("$.a[1]", 1, "$.a[1][1]", false),
            ("$.a[*]", 10, "$.a[*][10]", true),
            ("$.*[2]", 2, "$.*[2][2]", true),
            ("$**.a[3]", 3, "$**.a[3][3]", true),
            ("$.a[1 to 3]", 3, "$.a[1 to 3][3]", true),
            ("$.a[last-3 to last-3]", 3, "$.a[last-3 to last-3][3]", true),
            ("$**.a[3]", -3, "$**.a[3][last-2]", true),
        ];
        for (expression, index, expected, could_match_multiple) in cases {
            let parsed = ParseJSONPathExpr(expression).unwrap();
            let appended = parsed.pushBackOneArraySelectionLeg(jsonPathArraySelectionIndex {
                index: jsonPathArrayIndexFromStart(index),
            });
            assert_eq!(appended.String(), expected);
            assert_eq!(
                appended.CouldMatchMultipleValues(),
                could_match_multiple,
                "{expression:?}"
            );
        }
    }

    /// 对照 Go 表：追加对象键 leg。
    #[test]
    fn push_key_matches_the_go_table() {
        let cases = [
            ("$", "aa", "$.aa", false),
            ("$.a[1]", "aa", "$.a[1].aa", false),
            ("$.a[1]", "*", "$.a[1].*", true),
            ("$.a[*]", "k", "$.a[*].k", true),
            ("$.*[2]", "bb", "$.*[2].bb", true),
            ("$**.a[3]", "cc", "$**.a[3].cc", true),
        ];
        for (expression, key, expected, could_match_multiple) in cases {
            let parsed = ParseJSONPathExpr(expression).unwrap();
            let appended = parsed.pushBackOneKeyLeg(key.to_owned());
            assert_eq!(appended.String(), expected);
            assert_eq!(
                appended.CouldMatchMultipleValues(),
                could_match_multiple,
                "{expression:?}"
            );
        }
    }

    /// pop 后重算 flags，且不污染路径缓存中的原值。
    #[test]
    fn pop_recomputes_flags_without_mutating_cached_values() {
        let base = ParseJSONPathExpr("$.a[*].b").unwrap();
        let appended = base
            .pushBackOneArraySelectionLeg(jsonPathArraySelectionIndex {
                index: jsonPathArrayIndexFromStart(3),
            })
            .pushBackOneKeyLeg("*".to_owned());
        assert_eq!(appended.String(), "$.a[*].b[3].*");
        assert!(appended.CouldMatchMultipleValues());

        let (first, child) = appended.popOneLeg();
        assert_eq!(first.typ, jsonPathLegKey);
        assert_eq!(first.dotKey, "a");
        assert_eq!(child.String(), "$[*].b[3].*");
        assert!(child.CouldMatchMultipleValues());

        // Go popOneLastLeg is used only after rejecting wildcard/range paths.
        let single = ParseJSONPathExpr("$.a[3].b").unwrap();
        let (parent, last) = single.popOneLastLeg();
        assert_eq!(last.typ, jsonPathLegKey);
        assert_eq!(last.dotKey, "b");
        assert_eq!(parent.String(), "$.a[3]");
        assert!(!parent.CouldMatchMultipleValues());

        for expression in ["$.*.b", "$[*].b", "$**.b", "$[1 to 3].b"] {
            let parsed = ParseJSONPathExpr(expression).unwrap();
            assert!(parsed.CouldMatchMultipleValues());
            let (_, child) = parsed.popOneLeg();
            assert_eq!(child.String(), "$.b");
            assert!(!child.CouldMatchMultipleValues(), "{expression}");
            assert_eq!(parsed.String(), expression);
        }

        assert_eq!(ParseJSONPathExpr("$.a[*].b").unwrap().String(), "$.a[*].b");
    }

    /// 校验 `last-n` 编码、范围合法性与 getIndexRange 边界。
    #[test]
    fn array_indexes_and_ranges_match_go_boundaries() {
        assert_eq!(jsonPathArrayIndexFromStart(5).String(), "5");
        assert_eq!(jsonPathArrayIndexFromLast(0).String(), "last-0");
        assert_eq!(jsonPathArrayIndexFromLast(5).String(), "last-5");
        assert!(validateIndexRange(1, jsonPathArrayIndexFromLast(0)));
        assert!(!validateIndexRange(5, 1));
        assert!(!validateIndexRange(
            jsonPathArrayIndexFromLast(5),
            jsonPathArrayIndexFromLast(10)
        ));

        let index = jsonPathArraySelectionIndex {
            index: jsonPathArrayIndexFromLast(1),
        };
        assert_eq!(index.getIndexRange(5), (3, 3));
        let range = jsonPathArraySelectionRange {
            start: jsonPathArrayIndexFromStart(2),
            end: jsonPathArrayIndexFromStart(10),
        };
        assert_eq!(range.getIndexRange(5), (2, 4));
        assert_eq!(jsonPathArraySelectionAsterisk.getIndexRange(0), (0, -1));
    }
}
