// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use crate::*;
use expression::Expression as _;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.ID = id;
    column.UniqueID = id;
    column
}

#[test]
fn replace_expr_columns_rewrites_correlated_columns_like_go() {
    let origin = column(1);
    let replacement = column(9);
    let mut sort = LogicalSort {
        ByItems: vec![ByItems {
            Expr: Box::new(CorrelatedColumn {
                column: origin.clone(),
                data: None,
            }),
            Desc: false,
        }],
        ..LogicalSort::default()
    };

    sort.ReplaceExprColumns(&HashMap::from([(origin.HashCode(), replacement)]));

    let correlated = sort.ByItems[0]
        .Expr
        .as_any()
        .downcast_ref::<CorrelatedColumn>()
        .expect("sort expression should remain a correlated column");
    assert_eq!(correlated.column.UniqueID, 9);
}

#[test]
fn explain_info_marks_descending_sort_items_like_go() {
    let sort = LogicalSort {
        ByItems: vec![ByItems {
            Expr: Box::new(column(3)),
            Desc: true,
        }],
        ..LogicalSort::default()
    };

    assert!(sort.ExplainInfo().ends_with(":desc"));
}

#[test]
fn prune_sort_items_drops_null_typed_column_expressions_like_go() {
    let mut null_column = column(4);
    null_column.RetType = Some(*expression::types::NewFieldType(
        expression::mysql::TypeNull,
    ));

    let (kept, used) = pruneSortByItems(vec![ByItems {
        Expr: Box::new(null_column),
        Desc: false,
    }]);

    assert!(kept.is_empty());
    assert!(used.is_empty());
}
