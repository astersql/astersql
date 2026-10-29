// Copyright 2026 AsterSQL.

use super::parser_contract_cases::{parser_contract_cases, parser_contract_strings};

#[test]
fn go_contract_tables_are_not_reduced_to_smoke_cases() {
    assert_eq!(parser_contract_cases("TestRecommendIndex").len(), 22);
    assert_eq!(parser_contract_cases("TestTrafficStmt").len(), 25);
    assert_eq!(
        parser_contract_strings("TestSignedInt64OutOfRange", "cases"),
        [
            "recover table by job 18446744073709551612",
            "recover table t 18446744073709551612",
            "admin check index t idx (0, 18446744073709551612)",
            "create user abc@def with max_queries_per_hour 18446744073709551612",
        ]
    );
}

#[test]
fn pg_function_23_function_source_spans() {
    for (sql, start) in [
        ("SELECT abs(1)", 7),
        ("SELECT 'é', abs(1)", 13),
        ("SELECT pg_catalog.abs(1)", 7),
    ] {
        let node = crate::Parser::default().ParseOneStmt(sql, "", "").unwrap();
        let select = node
            .as_any()
            .downcast_ref::<crate::ast::SelectStmt>()
            .unwrap();
        let function = select.Fields.Fields.last().unwrap().Expr.as_ref().unwrap();
        assert_eq!(function.OriginTextPosition, start);
        assert!(matches!(
            function.Kind,
            crate::ast::ExprKind::Function { .. }
        ));
    }
}
