// Copyright 2026 AsterSQL.

use crate::RemoveUnnecessaryFirstRow;

fn column(id: i64) -> expression::ExprBox {
    let mut column = expression::Column::default();
    column.UniqueID = id;
    Box::new(column)
}

fn first_row(argument: expression::ExprBox) -> aggregation::AggFuncDesc {
    aggregation::newAggFunc(parser_ast::AggFuncFirstRow, vec![argument], false).AggFuncDesc
}

#[test]
fn remove_first_row_only_for_matching_non_constant_group_key() {
    let functions = vec![first_row(column(1)), first_row(column(2))];
    let retained = RemoveUnnecessaryFirstRow(functions, &[column(1)]);

    assert_eq!(retained.len(), 1);
    assert_eq!(
        retained[0].Args[0]
            .as_any()
            .downcast_ref::<expression::Column>()
            .unwrap()
            .UniqueID,
        2
    );
}

#[test]
fn remove_first_row_keeps_constant_group_key() {
    let functions = vec![first_row(Box::new(expression::NewOne()))];
    let retained = RemoveUnnecessaryFirstRow(functions, &[Box::new(expression::NewOne())]);

    assert_eq!(retained.len(), 1);
}
