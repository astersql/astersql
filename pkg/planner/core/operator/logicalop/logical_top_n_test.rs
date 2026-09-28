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
    let mut top_n = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(CorrelatedColumn {
                column: origin.clone(),
                data: None,
            }),
            Desc: false,
        }],
        ..LogicalTopN::default()
    };

    top_n.ReplaceExprColumns(&HashMap::from([(origin.HashCode(), replacement)]));

    let correlated = top_n.ByItems[0]
        .Expr
        .as_any()
        .downcast_ref::<CorrelatedColumn>()
        .expect("TopN expression should remain a correlated column");
    assert_eq!(correlated.column.UniqueID, 9);
}

#[test]
fn explain_info_marks_descending_sort_items_like_go() {
    let top_n = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(column(3)),
            Desc: true,
        }],
        Count: 5,
        ..LogicalTopN::default()
    };

    assert!(top_n.ExplainInfo().contains(":desc"));
    assert!(top_n.ExplainInfo().ends_with(", offset:0, count:5"));
}
