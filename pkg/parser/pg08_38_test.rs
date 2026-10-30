// Copyright 2026 AsterSQL.
#[test]
fn pg08_38_predicate_ast() {
    use parser_ast::{ExprKind, SelectStmt, SqlDialect};
    for (sql, operator) in [
        (
            "SELECT $1::int4 IS DISTINCT FROM $2::int4 AS v",
            "is distinct from",
        ),
        (
            "SELECT $1::int4 IS NOT DISTINCT FROM $2::int4 AS v",
            "is not distinct from",
        ),
    ] {
        let statement = crate::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseOneStmt(sql, "", "")
            .unwrap();
        let select = statement.as_any().downcast_ref::<SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let ExprKind::Binary { Op, L, R } = &expr.Kind else {
            panic!("predicate must retain both operands")
        };
        assert_eq!(Op, operator);
        assert!(matches!(L.Kind, ExprKind::Cast { .. }));
        assert!(matches!(R.Kind, ExprKind::Cast { .. }));
        assert!(
            crate::Parser::with_dialect(SqlDialect::MySQL)
                .ParseOneStmt(&format!("SELECT 1 {operator} 1 AS v"), "", "")
                .is_err()
        );
    }
    let statement = crate::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt("SELECT 1::int4 <=> 2::int4 AS v", "", "")
        .unwrap();
    let select = statement.as_any().downcast_ref::<SelectStmt>().unwrap();
    assert_eq!(
        select.Fields.Fields[0]
            .Expr
            .as_ref()
            .unwrap()
            .node_text
            .OriginTextPosition(),
        15
    );
}
