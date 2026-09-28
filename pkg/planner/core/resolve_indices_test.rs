// Copyright 2026 AsterSQL.

use super::*;

fn column(unique_id: i64) -> IndexedColumn {
    IndexedColumn {
        unique_id,
        index: usize::MAX,
        virtual_expr: None,
    }
}

#[test]
fn selection_virtual_expression_fallback_recurses_through_scalar_arguments() {
    let mut virtual_reference = column(99);
    virtual_reference.virtual_expr = Some("lower(a)".into());
    let mut schema_column = column(7);
    schema_column.virtual_expr = Some("lower(a)".into());
    let mut plan = SelectionPlan {
        conditions: vec![IndexedExpr::Scalar {
            name: "eq".into(),
            args: vec![IndexedExpr::Column(virtual_reference)],
        }],
        child_schema: vec![column(1), schema_column],
    };

    assert_eq!(resolveIndices4PhysicalSelection(&mut plan), Ok(()));
    let IndexedExpr::Scalar { args, .. } = &plan.conditions[0] else {
        panic!("condition must remain a scalar expression");
    };
    let IndexedExpr::Column(resolved) = &args[0] else {
        panic!("argument must remain a column");
    };
    assert_eq!(resolved.index, 1);
}

#[test]
fn projection_keeps_earlier_resolution_when_a_later_expression_fails() {
    let mut plan = ProjectionPlan {
        exprs: vec![
            IndexedExpr::Column(column(2)),
            IndexedExpr::Column(column(404)),
        ],
        child_schema: vec![column(1), column(2)],
        ..ProjectionPlan::default()
    };

    assert!(resolveIndicesItself4PhysicalProjection(&mut plan).is_err());
    let IndexedExpr::Column(first) = &plan.exprs[0] else {
        panic!("first expression must remain a column");
    };
    assert_eq!(first.index, 1);
}

#[test]
fn union_scan_keeps_earlier_condition_resolution_on_error() {
    let mut plan = UnionScanPlan {
        conditions: vec![
            IndexedExpr::Column(column(2)),
            IndexedExpr::Column(column(404)),
        ],
        child_schema: vec![column(1), column(2)],
        ..UnionScanPlan::default()
    };

    assert!(resolveIndices4PhysicalUnionScan(&mut plan).is_err());
    let IndexedExpr::Column(first) = &plan.conditions[0] else {
        panic!("first condition must remain a column");
    };
    assert_eq!(first.index, 1);
}

#[test]
fn index_lookup_keeps_earlier_common_handle_resolution_on_error() {
    let mut plan = IndexLookUpReaderPlan {
        table_schema: vec![column(1), column(2)],
        common_handle_cols: vec![column(2), column(404)],
        ..IndexLookUpReaderPlan::default()
    };

    assert!(resolveIndices4PhysicalIndexLookUpReader(&mut plan).is_err());
    assert_eq!(plan.common_handle_cols[0].index, 1);
}
