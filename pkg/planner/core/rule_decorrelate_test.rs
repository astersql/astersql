// Copyright 2026 AsterSQL.

use crate::rule_decorrelate::{
    ExtractOuterApplyCorrelatedCols, extractOuterApplyCorrelatedColsHelper,
    skipDecorrelateProjectionForLeftOuterApply,
};
use crate::rule_join_reorder::{JoinNode, JoinPlan};
use crate::task::{Expression, JoinType};

fn leaf(id: usize, schema: Vec<usize>, correlated_columns: Vec<usize>) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: format!("t{id}"),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns,
        },
        schema,
        row_count: 1.0,
    }
}

fn projection(expressions: Vec<Expression>) -> JoinPlan {
    JoinPlan {
        id: 4,
        node: JoinNode::Projection {
            expressions,
            child: Box::new(leaf(5, vec![40], Vec::new())),
        },
        schema: vec![40],
        row_count: 1.0,
    }
}

#[test]
fn outer_apply_columns_exclude_correlations_owned_by_an_inner_apply() {
    let plan = JoinPlan {
        id: 3,
        node: JoinNode::Apply {
            join_type: JoinType::Inner,
            left: Box::new(leaf(1, vec![30], Vec::new())),
            right: Box::new(leaf(2, vec![40], vec![20, 30])),
            correlated_columns: vec![20, 30],
            no_decorrelate: false,
        },
        schema: vec![30, 40],
        row_count: 1.0,
    };

    assert_eq!(ExtractOuterApplyCorrelatedCols(&plan), vec![20]);
    let (_, outer_schemas) = extractOuterApplyCorrelatedColsHelper(&plan);
    assert_eq!(outer_schemas, vec![[30].into_iter().collect()]);
}

#[test]
fn left_outer_apply_projection_guard_matches_go_column_rules() {
    let apply = JoinPlan {
        id: 3,
        node: JoinNode::Apply {
            join_type: JoinType::LeftOuter,
            left: Box::new(leaf(1, vec![10, 11], Vec::new())),
            right: Box::new(leaf(2, vec![20], Vec::new())),
            correlated_columns: vec![10],
            no_decorrelate: false,
        },
        schema: vec![10, 11, 20],
        row_count: 1.0,
    };

    assert!(skipDecorrelateProjectionForLeftOuterApply(
        &apply,
        &projection(vec![Expression::default()]),
    ));
    assert!(skipDecorrelateProjectionForLeftOuterApply(
        &apply,
        &projection(vec![Expression {
            column: Some(10),
            ..Expression::default()
        }]),
    ));
    assert!(!skipDecorrelateProjectionForLeftOuterApply(
        &apply,
        &projection(vec![Expression {
            column: Some(20),
            ..Expression::default()
        }]),
    ));
}
