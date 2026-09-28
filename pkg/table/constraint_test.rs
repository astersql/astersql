// Copyright 2026 AsterSQL.

// CHECK constraint Go/Rust parity regression tests.

use model_dependency::group_4 as constraint_model;

use crate::BuildConstraintExprWithCtx;

/// Go returns an executable expression from BuildConstraintExprWithCtx, not a
/// parser AST node. Keep the public Rust boundary aligned with that contract.
#[test]
fn build_constraint_expression_returns_executable_expression() {
    let _builder: for<'a> fn(
        &'a dyn expression_dependency::BuildContext,
        &'a constraint_model::ConstraintInfo,
        &'a constraint_model::TableInfo,
        &'a str,
    ) -> Result<
        Box<dyn expression_dependency::Expression>,
        errors_dependency::SharedError,
    > = BuildConstraintExprWithCtx;
}
