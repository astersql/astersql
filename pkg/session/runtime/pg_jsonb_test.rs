// Copyright 2026 AsterSQL.
use super::*;

#[test]
fn pg05_jsonb_sql_cast_roundtrip_and_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT ' {\"aa\":1,\"b\":2,\"aa\":3.00} '::text::jsonb",
        "SELECT ' {\"aa\":1,\"b\":2,\"aa\":3.00} '::json::jsonb::json::text",
        "SELECT CAST(' {\"aa\":1,\"b\":2,\"aa\":3.00} ' AS jsonb)::text",
        "SELECT ' {\"aa\":1,\"b\":2,\"aa\":3.00} '::jsonb::json",
    ] {
        assert_eq!(
            pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
            ["{\"b\": 2, \"aa\": 3.00}"]
        );
    }
    for sql in [
        "SELECT NULL::jsonb",
        "SELECT NULL::json::jsonb",
        "SELECT NULL::jsonb WHERE false",
    ] {
        let fields = pg.describe_result_fields(sql).unwrap();
        assert_eq!(
            astersql_parser_types::TypeStr(fields[0].column.FieldType.GetType()),
            "jsonb"
        );
        assert_eq!(fields[0].column.FieldType.GetDecimal(), -1);
        let mut rows = pg.execute(sql).unwrap();
        if sql.contains("WHERE") {
            assert!(rows[0].next_row().unwrap().is_none());
        } else {
            assert_eq!(rows[0].next_row().unwrap().unwrap(), ["<nil>"]);
        }
    }
}

#[test]
fn pg05_jsonb_sql_parameters_revalidate_and_recover() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for oid in [0, 25, 114, 3802] {
        assert_eq!(
            pg.protocol_parameter_types("SELECT ?::jsonb", &[oid])
                .unwrap(),
            [if oid == 0 { 3802 } else { oid }]
        );
    }
    for oid in [25, 114, 3802] {
        let (id, _, fields) = pg
            .prepare_protocol_statement_with_parameter_types("SELECT ?::jsonb AS v", &[oid])
            .unwrap();
        assert_eq!(
            astersql_parser_types::TypeStr(fields[0].column.FieldType.GetType()),
            "jsonb"
        );
        for (value, state) in [
            (r#""\u0000""#, "22P05"),
            (r#""\ud800""#, "22P05"),
            ("1e200000", "22003"),
            ("[1,]", "22P02"),
            ("bad", "22P02"),
        ] {
            // Raw JSON can retain the first three inputs. JSONB must validate
            // them anew at conversion, even when the source already is JSON.
            if oid == 114 && state != "22P02" {
                let (raw_id, _, _) = pg
                    .prepare_protocol_statement_with_parameter_types("SELECT ?::json", &[114])
                    .unwrap();
                let mut rows = pg
                    .execute_protocol_statement(
                        raw_id,
                        &[ConcretePreparedArgument::Text(value.into())],
                    )
                    .unwrap();
                assert_eq!(rows[0].next_row().unwrap().unwrap(), [value]);
                pg.close_protocol_statement(raw_id).unwrap();
            }
            let error = pg
                .execute_protocol_statement(id, &[ConcretePreparedArgument::Text(value.into())])
                .err()
                .unwrap();
            assert_eq!(
                error.pg_json_error().unwrap().sqlstate(),
                state,
                "oid={oid}, value={value}"
            );
            assert!(!error.to_string().contains(value));
        }
        for (value, expected) in [
            (ConcretePreparedArgument::Null, "<nil>"),
            (ConcretePreparedArgument::Text("null".into()), "null"),
            (
                ConcretePreparedArgument::Text(r#""null""#.into()),
                r#""null""#,
            ),
            (
                ConcretePreparedArgument::Text("1.230e-5".into()),
                "0.00001230",
            ),
            (
                ConcretePreparedArgument::Text("18446744073709551617".into()),
                "18446744073709551617",
            ),
            (
                ConcretePreparedArgument::Text(r#"[2,1,2,{"aa":1,"b":2,"aa":3}]"#.into()),
                r#"[2, 1, 2, {"b": 2, "aa": 3}]"#,
            ),
        ] {
            let mut rows = pg.execute_protocol_statement(id, &[value]).unwrap();
            assert_eq!(
                astersql_parser_types::TypeStr(
                    rows[0].result_fields()[0]
                        .as_ref()
                        .unwrap()
                        .column
                        .FieldType
                        .GetType()
                ),
                "jsonb"
            );
            assert_eq!(rows[0].next_row().unwrap().unwrap(), [expected]);
        }
        pg.close_protocol_statement(id).unwrap();
    }
    for (target, expected_type) in [("json", "json"), ("text", "var_string")] {
        let sql = format!("SELECT ?::{target}");
        assert_eq!(pg.protocol_parameter_types(&sql, &[3802]).unwrap(), [3802]);
        let (id, _, fields) = pg
            .prepare_protocol_statement_with_parameter_types(&sql, &[3802])
            .unwrap();
        if target == "text" {
            assert_eq!(fields[0].column.FieldType.PostgreSQLTextOid(), Some(25));
        } else {
            assert_eq!(
                astersql_parser_types::TypeStr(fields[0].column.FieldType.GetType()),
                expected_type
            );
        }
        for (argument, expected) in [
            (ConcretePreparedArgument::Null, "<nil>"),
            (
                ConcretePreparedArgument::Text(r#" {"a":1,"a":2} "#.into()),
                r#"{"a": 2}"#,
            ),
        ] {
            let mut rows = pg.execute_protocol_statement(id, &[argument]).unwrap();
            assert_eq!(rows[0].next_row().unwrap().unwrap(), [expected]);
        }
        pg.close_protocol_statement(id).unwrap();
    }
    assert_eq!(
        super::pg_jsonb::cast_value(Some("\0".into()))
            .unwrap_err()
            .pg_json_error()
            .unwrap()
            .sqlstate(),
        "22021"
    );
    let (id, _, fields) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[3802])
        .unwrap();
    assert_eq!(
        astersql_parser_types::TypeStr(fields[0].column.FieldType.GetType()),
        "jsonb"
    );
    for argument in [
        ConcretePreparedArgument::Null,
        ConcretePreparedArgument::Text("false".into()),
    ] {
        let mut rows = pg.execute_protocol_statement(id, &[argument]).unwrap();
        assert_eq!(
            astersql_parser_types::TypeStr(
                rows[0].result_fields()[0]
                    .as_ref()
                    .unwrap()
                    .column
                    .FieldType
                    .GetType()
            ),
            "jsonb"
        );
        assert!(rows[0].next_row().unwrap().is_some());
    }
    pg.close_protocol_statement(id).unwrap();
}

#[test]
fn pg05_jsonb_sql_unsupported_bridges_and_shared_evaluator() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT NULL::int4::jsonb WHERE false",
        "SELECT NULL::jsonb::int4 WHERE false",
        "SELECT CAST(NULL AS BINARY)::jsonb",
        "SELECT CAST(NULL::jsonb AS BINARY)",
    ] {
        assert_eq!(
            pg.execute(sql)
                .err()
                .unwrap()
                .pg_json_error()
                .unwrap()
                .sqlstate(),
            "0A000",
            "{sql}"
        );
    }
    let mysql = astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeJSON);
    let jsonb = astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypePgJsonb);
    assert_eq!(
        super::pg_json::validate_cast(&mysql, &jsonb)
            .unwrap_err()
            .pg_json_error()
            .unwrap()
            .sqlstate(),
        "0A000"
    );
    let (id, _, _) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ?::jsonb", &[17])
        .unwrap();
    assert_eq!(
        pg.execute_protocol_statement(id, &[ConcretePreparedArgument::Null])
            .err()
            .unwrap()
            .pg_json_error()
            .unwrap()
            .sqlstate(),
        "0A000"
    );
    pg.close_protocol_statement(id).unwrap();
    let statements = parse_in_dialect(
        r#"SELECT '{"a":1,"a":2}'::jsonb::json::text"#,
        ast::SqlDialect::PostgreSQL,
    )
    .unwrap();
    let select = statements[0]
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .unwrap();
    let expression = select.Fields.Fields[0].Expr.as_ref().unwrap();
    assert_eq!(
        super::relational_value::relational_expression_value(expression, &HashMap::new()).unwrap(),
        Some(r#"{"a": 2}"#.into())
    );
    assert_eq!(
        pg.execute("SELECT 'true'::jsonb::text").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["true"]
    );
}
