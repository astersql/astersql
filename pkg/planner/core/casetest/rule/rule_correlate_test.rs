// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Executable parity tests for Go `rule_correlate_test.go` and the correlate
//! rule support routines. SQL-level cases stay tied to the Go golden inventory;
//! the compact Rust planner model is exercised directly below.

use astersql_planner_core::rule_correlate::{
    CorrelateSolver, liftDataSourceConds, resetStatsForCorrelatedDS,
};
use astersql_planner_core::rule_join_reorder::{JoinNode, JoinPlan};
use astersql_planner_core::task::JoinType;

fn apply(
    left: JoinPlan,
    right: JoinPlan,
    correlated_columns: Vec<usize>,
    no_decorrelate: bool,
) -> JoinPlan {
    let mut schema = left.schema.clone();
    schema.extend(right.schema.iter().copied());
    JoinPlan {
        id: 10,
        node: JoinNode::Apply {
            join_type: JoinType::Semi,
            left: Box::new(left),
            right: Box::new(right),
            correlated_columns,
            no_decorrelate,
        },
        schema,
        row_count: 10.0,
    }
}

fn leaf_predicates(plan: &JoinPlan) -> &[astersql_planner_core::task::Expression] {
    match &plan.node {
        JoinNode::Leaf { predicates, .. } => predicates,
        other => panic!("expected leaf, got {other:?}"),
    }
}

#[test]
fn correlate_builds_inner_predicate_and_preserves_apply_contract() {
    let left = crate::support::leaf(1, "outer", vec![1], 3.0);
    let right = crate::support::leaf(2, "inner", vec![10], 4.0);
    let join = crate::support::join(10, JoinType::Semi, left, right, Some((1, 10)));
    let mut join = join;
    if let JoinNode::Join {
        preferred_method, ..
    } = &mut join.node
    {
        *preferred_method = Some("correlate".to_owned());
    }

    let (optimized, changed) = CorrelateSolver.Optimize(join).expect("correlate succeeds");

    assert!(changed);
    let JoinNode::Apply {
        join_type,
        right,
        correlated_columns,
        no_decorrelate,
        ..
    } = optimized.node
    else {
        panic!("correlate must preserve the Apply node");
    };
    assert_eq!(join_type, JoinType::Semi);
    assert_eq!(correlated_columns, vec![1]);
    assert!(!no_decorrelate);
    let predicates = leaf_predicates(&right);
    assert_eq!(predicates.len(), 1);
    assert!(
        predicates
            .iter()
            .all(|condition| condition.name == "correlated_eq")
    );
    assert_eq!(
        predicates
            .iter()
            .map(|condition| condition.column)
            .collect::<Vec<_>>(),
        vec![Some(10)]
    );
}

#[test]
fn correlated_condition_is_not_null_equal() {
    let condition = CorrelateSolver.buildCorrelatedCond(7, 9);
    assert_eq!(condition.left_column, 7);
    assert_eq!(condition.right_column, 9);
    // Go scalar equality must retain SQL three-valued NULL semantics; it must
    // not silently become null-safe equality.
    assert!(!condition.null_equal);
}

#[test]
fn no_decorrelate_apply_is_left_untouched() {
    let left = crate::support::leaf(1, "outer", vec![1], 3.0);
    let right = crate::support::leaf(2, "inner", vec![10], 4.0);
    let original = apply(left, right, vec![1], true);

    let (optimized, changed) = CorrelateSolver
        .Optimize(original)
        .expect("no-decorrelate apply succeeds");

    assert!(!changed);
    let JoinNode::Apply { right, .. } = optimized.node else {
        panic!("Apply must be preserved");
    };
    assert!(leaf_predicates(&right).is_empty());
}

#[test]
fn selection_conditions_are_lifted_without_losing_existing_predicates() {
    let mut leaf = crate::support::leaf(1, "inner", vec![2], 1.0);
    if let JoinNode::Leaf { predicates, .. } = &mut leaf.node {
        predicates.push(crate::support::expr("existing", Some(2)));
    }
    let plan = JoinPlan {
        id: 2,
        node: JoinNode::Selection {
            conditions: vec![crate::support::expr("lifted", Some(2))],
            child: Box::new(leaf),
        },
        schema: vec![2],
        row_count: 1.0,
    };

    let lifted = liftDataSourceConds(plan);
    let predicates = leaf_predicates(&lifted);
    assert_eq!(predicates.len(), 2);
    assert_eq!(predicates[0].name, "existing");
    assert_eq!(predicates[1].name, "lifted");
}

#[test]
fn selection_above_non_leaf_is_not_lifted() {
    let join = crate::support::join(
        3,
        JoinType::Inner,
        crate::support::leaf(1, "left", vec![1], 1.0),
        crate::support::leaf(2, "right", vec![2], 1.0),
        Some((1, 2)),
    );
    let plan = JoinPlan {
        id: 4,
        node: JoinNode::Selection {
            conditions: vec![crate::support::expr("keep", Some(1))],
            child: Box::new(join),
        },
        schema: vec![1, 2],
        row_count: 1.0,
    };

    assert!(matches!(
        liftDataSourceConds(plan).node,
        JoinNode::Selection { .. }
    ));
}

#[test]
fn reset_stats_only_marks_paths_with_correlated_data_sources() {
    let mut correlated = crate::support::leaf(1, "correlated", vec![1], 0.0);
    if let JoinNode::Leaf {
        correlated_columns, ..
    } = &mut correlated.node
    {
        correlated_columns.push(1);
    }
    let mut ordinary_alone = crate::support::leaf(2, "ordinary", vec![2], 0.0);
    let ordinary_in_join = ordinary_alone.clone();
    let mut root = crate::support::join(
        3,
        JoinType::Inner,
        correlated,
        ordinary_in_join,
        Some((1, 2)),
    );

    assert!(resetStatsForCorrelatedDS(&mut root));
    let JoinNode::Join { left, right, .. } = &root.node else {
        panic!("join must be preserved");
    };
    assert_eq!(left.row_count, 1.0);
    assert_eq!(right.row_count, 0.0);
    assert!(!resetStatsForCorrelatedDS(&mut ordinary_alone));
    assert_eq!(ordinary_alone.row_count, 0.0);
}

#[test]
fn correlate_fixture_inventory_matches_go() {
    let cases = crate::support::fixture_case_counts(
        "correlate_suite",
        &["TestCorrelate", "TestCorrelateWithCostFactors"],
    );
    assert_eq!(cases.len(), 2);
    assert!(
        cases
            .iter()
            .all(|(input, output)| input == output && *input > 0)
    );
}
