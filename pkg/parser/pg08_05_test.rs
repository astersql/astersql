// Copyright 2026 AsterSQL.
#[test]
fn pg08_05_fields() {
    use parser_ast::{ExprKind, SelectStmt, SqlDialect, ValueDatum};
    for field in [
        "SECOND",
        "MICROSECONDS",
        "ISOYEAR",
        "TIMEZONE",
        "aster_bad_unit",
        "'second'",
    ] {
        let sql = format!("SELECT EXTRACT({field} FROM $1::timestamp) AS v");
        let mut parser = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
        let stmt = parser
            .ParseOneStmt(&sql, "", "")
            .unwrap_or_else(|e| panic!("{field}: {e}"));
        let select = stmt.as_any().downcast_ref::<SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let ExprKind::Function { FnName, Args, .. } = &expr.Kind else {
            panic!("expected extract")
        };
        assert_eq!(FnName.L, "extract");
        assert_eq!(Args.len(), 2);
        {
            let ExprKind::Value(value) = &Args[0].Kind else {
                panic!("field text lost")
            };
            assert_eq!(
                value.Datum,
                ValueDatum::String(field.trim_matches('\'').to_owned())
            );
            assert_eq!(
                Args[0].OriginTextPosition as usize,
                sql.find(field).unwrap()
            );
        }
        let ExprKind::Cast { Expr: source, .. } = &Args[1].Kind else {
            panic!("source cast lost")
        };
        assert!(matches!(source.Kind, ExprKind::ParamMarker { .. }));
        assert_eq!(source.OriginTextPosition as usize, sql.find("$1").unwrap());
        let mut mysql = crate::Parser::with_dialect(SqlDialect::MySQL);
        if field != "SECOND" {
            assert!(
                mysql
                    .ParseOneStmt(
                        &format!("SELECT EXTRACT({field} FROM '2024-01-02 00:00:40')"),
                        "",
                        ""
                    )
                    .is_err()
            );
        }
    }
    let mut mysql = crate::Parser::with_dialect(SqlDialect::MySQL);
    for field in ["SECOND", "MICROSECOND", "DAY_SECOND"] {
        assert!(
            mysql
                .ParseOneStmt(
                    &format!("SELECT EXTRACT({field} FROM '2024-01-02 00:00:40.123456')"),
                    "",
                    ""
                )
                .is_ok()
        );
    }
    let mut pg = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
    assert!(
        pg.ParseOneStmt("SELECT EXTRACT($2 FROM $1::timestamp)", "", "")
            .is_err()
    );
    println!("pg08_05_fields: 10 cases passed");
}
