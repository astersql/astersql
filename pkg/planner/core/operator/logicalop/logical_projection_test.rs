// Copyright 2026 AsterSQL.

use crate::*;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

#[test]
fn projection_rewrites_pulled_predicate_to_output_column() {
    let input = column(1);
    let output = column(2);
    let predicates = vec![Box::new(input.clone()) as Expression];

    let rewritten = rewriteProjectionConstantPredicates(
        &[Box::new(input) as Expression],
        &expression::NewSchema(vec![output.clone()]),
        predicates,
    );

    assert_eq!(rewritten.len(), 1);
    let columns = expression::ExtractColumns(rewritten[0].as_ref());
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].UniqueID, output.UniqueID);
}
