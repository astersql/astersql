// Copyright 2026 AsterSQL.
#[test]
fn pg08_20_cardinality_syntax() {
    use parser_ast::{ExprKind, SelectStmt, SqlDialect};
    for dialect in [SqlDialect::PostgreSQL, SqlDialect::MySQL] {
        for sql in [
            "SELECT cardinality(NULL) AS v",
            "SELECT cardinality (NULL) AS v",
            "SELECT pg_catalog.cardinality(NULL) AS v",
        ] {
            let mut parser = crate::Parser::with_dialect(dialect);
            let statement = parser.ParseOneStmt(sql, "", "").unwrap();
            let select = statement.as_any().downcast_ref::<SelectStmt>().unwrap();
            let ExprKind::Function { FnName, Args, .. } =
                &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
            else {
                panic!("expected function");
            };
            assert_eq!(FnName.L, "cardinality");
            assert_eq!(Args.len(), 1);
        }
    }
    let mut mysql = crate::Parser::with_dialect(SqlDialect::MySQL);
    assert!(
        mysql
            .ParseOneStmt("CREATE STATISTICS stats (CARDINALITY) ON t(a)", "", "")
            .is_ok()
    );
    println!("pg08_20_cardinality_syntax: 7 scenarios passed");
}
