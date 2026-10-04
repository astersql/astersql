// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use astersql_parser::Parser;
use astersql_parser_ast::SelectStmt;

use super::{
    ConcreteRecordSet, ConcreteSession, empty_strict_integer_interval,
    should_reorder_hash_join_equal_conditions, should_swap_generated_hash_key,
    should_swap_hash_join_equality,
};

#[test]
fn explain_tree_indentation_stays_in_operator_column() {
    for prefix in ["", "└─", "  └─", "  │ ├─", "    │   └─"] {
        let operator = format!("{prefix}IndexReader");
        let line = format!("{operator} 1.00 root  index:IndexRangeScan");
        let result = ConcreteSession::explain_plan_tree_rows(vec![line.clone()]);
        let row = &result.rows[0];
        assert_eq!(
            row,
            &[
                operator,
                "1.00".into(),
                "root".into(),
                " index:IndexRangeScan".into()
            ]
        );
        assert_eq!(row.join(" "), line);
    }
}

#[test]
fn explain_tree_preserves_go_hash_join_key_order() {
    let line = "HashJoin root  inner join, equal:[eq(Column, test.t0.c0)]";
    let result = ConcreteSession::explain_plan_tree_rows(vec![line.to_owned()]);
    assert_eq!(result.rows[0].join(" "), line);
}

fn predicate(sql: &str) -> astersql_parser_ast::ExprNode {
    let statement = Parser::default()
        .ParseOneStmt(sql, "", "")
        .expect("parse SELECT");
    statement
        .as_any()
        .downcast_ref::<SelectStmt>()
        .and_then(|select| select.Where.clone())
        .expect("SELECT predicate")
}

#[test]
fn strict_integer_interval_handles_full_i64_domain_without_overflow() {
    let expression =
        predicate("select 1 where a > -9223372036854775808 and a < 9223372036854775807");

    assert!(!empty_strict_integer_interval(&expression));

    let expression = predicate("select 1 where a > 41 and a < 42");
    assert!(empty_strict_integer_interval(&expression));
}

#[test]
fn hash_join_explain_equality_order_is_rewritten_only_for_serialized_tasks() {
    assert!(should_reorder_hash_join_equal_conditions("root"));
    assert!(should_reorder_hash_join_equal_conditions("mpp[tiflash]"));
    assert!(!should_reorder_hash_join_equal_conditions("cop[tikv]"));
}

#[test]
fn hash_join_explain_swaps_only_when_operands_are_reversed_by_children() {
    assert!(should_swap_hash_join_equality(false, true, true, false));
    assert!(!should_swap_hash_join_equality(true, false, false, true));
    assert!(!should_swap_hash_join_equality(true, true, false, true));
}

#[test]
fn hash_join_explain_places_named_key_before_generated_projection_key() {
    assert!(should_swap_generated_hash_key("Column#267", "test.t3.a"));
    assert!(!should_swap_generated_hash_key("test.t3.a", "Column#267"));
    assert!(!should_swap_generated_hash_key("Column#267", "Column#268"));
}

#[test]
fn read_pool_explain_execution_info_preserves_the_complete_captured_aggregate() {
    use super::explain_analyze::append_read_pool_execution_info;
    let mut execution = "time:1ms, loops:1".to_owned();
    let unchanged = execution.clone();
    append_read_pool_execution_info(&mut execution, None);
    append_read_pool_execution_info(
        &mut execution,
        Some(&astersql_kv::PoolTaskDetails::default()),
    );
    assert_eq!(execution, unchanged);
    let pool = astersql_kv::PoolTaskDetails {
        TaskCount: 2,
        PollCount: 8,
        MaxPollCount: 4,
        MinPollCount: 4,
        PollWallTime: std::time::Duration::from_millis(24),
        ..Default::default()
    };
    append_read_pool_execution_info(&mut execution, Some(&pool));
    assert_eq!(
        execution,
        format!("{unchanged}, read_pool:{}", pool.String())
    );
}

#[test]
fn explain_ru_preserves_flat_operator_order_and_skips_empty_id() {
    let input = ConcreteRecordSet::new(
        ["id", "estRows", "actRows", "task", "operator info"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        vec![
            vec!["TableReader_1", "4", "4", "root", ""],
            vec!["└─TableFullScan_2", "4", "4", "cop[tikv]", ""],
            vec!["_0", "0", "0", "root", ""],
            vec!["CTE_3", "2", "2", "root", ""],
            vec!["└─TableFullScan_4", "2", "2", "cop[tikv]", ""],
            vec!["ScalarSubQuery_5", "1", "1", "root", ""],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_owned).collect())
        .collect(),
    );
    let mut output = ConcreteSession::explain_analyze_ru_rows(input).unwrap();
    let expected = [
        ("TableReader_1", "root", "4"),
        ("└─TableFullScan_2", "cop[tikv]", "4"),
        ("CTE_3", "root", "2"),
        ("└─TableFullScan_4", "cop[tikv]", "2"),
        ("ScalarSubQuery_5", "root", "1"),
    ];
    for (id, task, actual) in expected {
        let row = output.next_row().unwrap().unwrap();
        assert_eq!(&row[..3], [id, task, actual]);
        assert!(row[3..].iter().all(String::is_empty));
    }
    assert!(output.next_row().unwrap().is_none());
    let empty = ConcreteRecordSet::new(
        ["id", "task", "actRows"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Vec::new(),
    );
    assert!(
        ConcreteSession::explain_analyze_ru_rows(empty)
            .unwrap()
            .next_row()
            .unwrap()
            .is_none()
    );
}
