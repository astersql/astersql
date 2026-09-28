// Copyright 2026 AsterSQL.

#[test]
fn common_handle_routes_encode_errors_through_statement_context() {
    let source = include_str!("handle_cols.rs");
    let function = source
        .split("fn buildHandleByDatumsBuffer")
        .nth(1)
        .and_then(|tail| tail.split("pub fn GetColumns").next())
        .expect("common-handle encoding helper must remain present");

    assert!(
        function.contains("statement_context.HandleError"),
        "Go routes EncodeKey errors through StatementContext.HandleError"
    );
}
