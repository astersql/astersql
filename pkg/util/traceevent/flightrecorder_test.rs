// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// flightrecorder 单元测试：JSON 配置编译、类别解析与 and/or 真值表。
//
// 对齐 Go `flightrecorder_test.go`；用例通过 `test_support` 串行化。

use astersql_util_traceevent::flightrecorder::{
    FlightRecorderConfig, check_truth_table, parse_categories, truth_table_for_and,
    truth_table_for_or,
};
use astersql_util_traceevent::traceevent::{ALL_CATEGORIES, GENERAL, STMT_PLAN, TXN_2PC};
use std::collections::HashMap;

/// 将 JSON 文本解码为 FlightRecorderConfig。
fn decode_config(input: &str) -> serde_json::Result<FlightRecorderConfig> {
    serde_json::from_str(input)
}

/// 合法配置应产出期望的 name_mapping；非法 JSON/重复触发器应失败。
#[test]
fn test_flight_recorder_config() {
    let _guard = super::test_support::test_guard();
    let good = [
        (
            r#"{"enabled_categories":["txn_2pc","stmt_plan"],"dump_trigger":{"type":"sampling","sampling":100}}"#,
            vec![("dump_trigger.sampling", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"sampling","sampling":1}}"#,
            vec![("dump_trigger.sampling", 0)],
        ),
        (
            r#"{"enabled_categories":["general"],"dump_trigger":{"type":"user_command","user_command":{"type":"sql_regexp","sql_regexp":"^select"}}}"#,
            vec![("dump_trigger.user_command.sql_regexp", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"user_command","user_command":{"type":"stmt_label","stmt_label":"CreateTable"}}}"#,
            vec![("dump_trigger.user_command.stmt_label", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"user_command","user_command":{"type":"sql_regexp","sql_regexp":"^select"}}}"#,
            vec![("dump_trigger.user_command.sql_regexp", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"suspicious_event","suspicious_event":{"type":"slow_query"}}}"#,
            vec![("dump_trigger.suspicious_event", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"suspicious_event","suspicious_event":{"type":"region_error"}}}"#,
            vec![("dump_trigger.suspicious_event", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"and","and":[{"type":"user_command","user_command":{"type":"stmt_label","stmt_label":"Select"}},{"type":"suspicious_event","suspicious_event":{"type":"resolve_lock"}}]}}"#,
            vec![
                ("dump_trigger.user_command.stmt_label", 0),
                ("dump_trigger.suspicious_event", 1),
            ],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"or","or":[{"type":"and","and":[{"type":"user_command","user_command":{"type":"stmt_label","stmt_label":"Insert"}},{"type":"suspicious_event","suspicious_event":{"type":"query_fail"}}]},{"type":"sampling","sampling":10}]}}"#,
            vec![
                ("dump_trigger.user_command.stmt_label", 0),
                ("dump_trigger.suspicious_event", 1),
                ("dump_trigger.sampling", 2),
            ],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"suspicious_event","suspicious_event":{"type":"is_internal","is_internal":true}}}"#,
            vec![("dump_trigger.suspicious_event.is_internal", 0)],
        ),
        (
            r#"{"enabled_categories":["*"],"dump_trigger":{"type":"suspicious_event","suspicious_event":{"type":"dev_debug","dev_debug":{"type":"execute_internal_trace_missing"}}}}"#,
            vec![("dump_trigger.suspicious_event.dev_debug", 0)],
        ),
    ];
    for (idx, (input, expected)) in good.iter().enumerate() {
        let compiled = decode_config(input)
            .unwrap_or_else(|error| panic!("case {idx}: {error}"))
            .compile()
            .unwrap_or_else(|error| panic!("case {idx}: {error}"));
        let expected: HashMap<String, usize> = expected
            .iter()
            .map(|(name, bit)| ((*name).to_owned(), *bit))
            .collect();
        assert_eq!(compiled.name_mapping, expected, "case {idx}");
    }

    assert!(
        decode_config(
            r#""enabled_categories":["*"],"dump_trigger":{"type":"sampling","sampling":5,}"#
        )
        .is_err()
    );
    for input in [
        r#"{"enabled_categories":["txn_2pc","stmt_plan"],"dump_trigger":{"type":"user_command","sampling":5}}"#,
        r#"{"enabled_categories":["sdaf"],"dump_trigger":{"type":"user_command","user_command":{"type":"non_exist","sql_regexp":"^select"}}}"#,
        r#"{"enabled_categories":["*"],"dump_trigger":{"type":"and","and":[{"type":"suspicious_event","suspicious_event":{"type":"slow_query"}},{"type":"suspicious_event","suspicious_event":{"type":"query_fail"}}]}}"#,
    ] {
        assert!(decode_config(input).expect("valid JSON").compile().is_err());
    }
}

/// `*` / `-` 减法与未知名解析与 Go 一致。
#[test]
fn test_parse_trace_category() {
    let _guard = super::test_support::test_guard();
    let cases = [
        (vec!["*"], ALL_CATEGORIES),
        (vec!["-", "general"], ALL_CATEGORIES & !GENERAL),
        (vec!["txn_2pc"], TXN_2PC),
        (
            vec!["txn_2pc", "stmt_plan", "non_exist"],
            TXN_2PC | STMT_PLAN,
        ),
        (vec!["non_exist"], Default::default()),
    ];
    for (input, expected) in cases {
        let input: Vec<String> = input.into_iter().map(str::to_owned).collect();
        assert_eq!(parse_categories(&input), expected);
    }
}

/// and/or 真值表组合与 check_truth_table 命中语义。
#[test]
fn test_and_or_combination() {
    let _guard = super::test_support::test_guard();
    let (a, b, c, d) = (1_u64, 2_u64, 4_u64, 8_u64);
    let table = truth_table_for_and(vec![a], vec![b, c]);
    assert!(!check_truth_table(a, &table));
    assert!(!check_truth_table(d, &table));
    assert!(check_truth_table(a | b, &table));
    assert!(check_truth_table(a | c, &table));
    assert!(!check_truth_table(b | c, &table));

    let table = truth_table_for_and(vec![a | b], vec![c]);
    for value in [a, d, a | b, a | c, b | c] {
        assert!(!check_truth_table(value, &table));
    }
    for value in [a | b | c, a | b | c | d] {
        assert!(check_truth_table(value, &table));
    }

    let table = truth_table_for_and(vec![a, b], vec![c, d]);
    for value in [a, b, c, d, a | b, c | d] {
        assert!(!check_truth_table(value, &table));
    }
    for value in [a | c, b | c, b | d, b | c | d, a | b | c | d] {
        assert!(check_truth_table(value, &table));
    }

    let table = truth_table_for_or(vec![a, b], vec![c | d]);
    for value in [a, b, a | d, a | b] {
        assert!(check_truth_table(value, &table));
    }
    for value in [c, d] {
        assert!(!check_truth_table(value, &table));
    }

    let table = truth_table_for_or(vec![a | c], vec![b | d]);
    for value in [a, b, c, d, a | d, b | c, c | d] {
        assert!(!check_truth_table(value, &table));
    }
    for value in [a | c, b | d, a | c | d] {
        assert!(check_truth_table(value, &table));
    }
}
