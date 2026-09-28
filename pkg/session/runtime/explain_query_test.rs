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
    ConcreteSession, empty_strict_integer_interval, should_reorder_hash_join_equal_conditions,
    should_swap_generated_hash_key, should_swap_hash_join_equality,
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
