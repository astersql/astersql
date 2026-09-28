// Copyright 2026 AsterSQL.

use super::physical_common_plans::{PhysicalExpr, is_default_expr_same_column};

#[test]
fn unqualified_default_is_valid_independent_of_name_slice() {
    let expression = PhysicalExpr::Default { column_name: None };

    assert!(is_default_expr_same_column(&[], &expression));
    assert!(is_default_expr_same_column(
        &["first".to_owned(), "second".to_owned()],
        &expression,
    ));
}
