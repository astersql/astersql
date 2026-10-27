// Copyright 2026 AsterSQL.
use super::*;

const JSON: &str = " {\"a\":1.00, \"a\":1e999999, \"s\":\"\\u0000\"} ";

#[test]
fn pg05_json_sql_cast_preserves_text_and_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    // The shared SQL lexer consumes backslash escapes; encode the SQL literal
    // separately from the JSON text being cast. Parameters below keep raw JSON.
    let literal = JSON.replace('\\', "\\\\");
    for sql in [
        format!("SELECT '{literal}'::text::json"),
        format!("SELECT CAST('{literal}' AS json)"),
        format!("SELECT '{literal}'::json::text::json"),
    ] {
        let mut rows = pg.execute(&sql).unwrap();
        assert_eq!(rows[0].next_row().unwrap().unwrap(), [JSON]);
        assert_eq!(
            pg.describe_result_fields(&sql).unwrap()[0]
                .column
                .FieldType
                .GetType(),
            astersql_parser_mysql::r#type::TypePgJson
        );
    }
    for value in [JSON, "null", "\"null\"", "[1,true,null]", "42", "true"] {
        let literal = value.replace('\\', "\\\\");
        let sql = format!("SELECT '{literal}'::json::text");
        assert_eq!(
            pg.execute(&sql).unwrap()[0].next_row().unwrap().unwrap(),
            [value]
        );
        assert_eq!(
            pg.describe_result_fields(&sql).unwrap()[0]
                .column
                .FieldType
                .PostgreSQLTextOid(),
            Some(25)
        );
    }
}

#[test]
fn pg05_json_sql_null_empty_and_parameter_context() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in ["SELECT NULL::json", "SELECT NULL::json WHERE false"] {
        let fields = pg.describe_result_fields(sql).unwrap();
        assert_eq!(
            fields[0].column.FieldType.GetType(),
            astersql_parser_mysql::r#type::TypePgJson
        );
        assert_eq!(fields[0].column.FieldType.GetDecimal(), -1);
        let mut rows = pg.execute(sql).unwrap();
        if sql.contains("WHERE") {
            assert!(rows[0].next_row().unwrap().is_none());
        } else {
            assert_eq!(rows[0].next_row().unwrap().unwrap(), ["<nil>"]);
        }
    }
    for oid in [0, 25, 17, 114] {
        assert_eq!(
            pg.protocol_parameter_types("SELECT ?::json", &[oid])
                .unwrap(),
            [if oid == 0 { 114 } else { oid }]
        );
    }
}

#[test]
fn pg05_json_sql_invalid_and_unsupported_recover() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 'bad'::json",
        "SELECT ''::text::json",
        "SELECT '[1,]'::json",
        "SELECT NULL::int4::json",
        "SELECT NULL::json::int4 WHERE false",
        "SELECT CAST(NULL AS BINARY)::json",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
    }
    assert_eq!(
        pg.execute("SELECT 'null'::text").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["null"]
    );
    assert!(pg.execute("SELECT NULL::json::text").is_ok());
}

#[test]
fn pg05_json_sql_bound_identity_errors_and_rebinding() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (oid, expected_type) in [
        (114, astersql_parser_mysql::r#type::TypePgJson),
        (25, astersql_parser_mysql::r#type::TypeVarString),
        (17, astersql_parser_mysql::r#type::TypeBlob),
    ] {
        let (id, _, fields) = pg
            .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[oid])
            .unwrap();
        assert_eq!(fields[0].column.FieldType.GetType(), expected_type);
        for argument in [
            ConcretePreparedArgument::Null,
            ConcretePreparedArgument::Text("null".into()),
        ] {
            let mut rows = pg
                .execute_protocol_statement(id, &[argument.clone()])
                .unwrap();
            let field = &rows[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType;
            if oid == 17 {
                assert_eq!(field.GetCharset(), "binary");
                assert!(matches!(
                    field.GetType(),
                    astersql_parser_mysql::r#type::TypeBlob
                        | astersql_parser_mysql::r#type::TypeString
                        | astersql_parser_mysql::r#type::TypeVarString
                ));
            } else {
                assert_eq!(field.GetType(), expected_type);
            }
            assert_eq!(
                rows[0].next_row().unwrap().unwrap(),
                [if matches!(argument, ConcretePreparedArgument::Null) {
                    "<nil>"
                } else {
                    "null"
                }]
            );
        }
        pg.close_protocol_statement(id).unwrap();
    }
    let (id, _, _) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ?::json", &[25])
        .unwrap();
    for (value, state) in [("bad", "22P02"), ("[1,]", "22P02")] {
        let error = pg
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Text(value.into())])
            .err()
            .unwrap();
        assert_eq!(error.pg_json_error().unwrap().sqlstate(), state);
        assert!(
            !error.to_string().contains(value),
            "input={value:?} error={error:?}"
        );
    }
    for value in [JSON, "null", "false"] {
        let mut rows = pg
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Text(value.into())])
            .unwrap();
        assert_eq!(rows[0].next_row().unwrap().unwrap(), [value]);
    }
    pg.close_protocol_statement(id).unwrap();
    for sql in [
        "SELECT NULL::int4::json WHERE false",
        "SELECT CAST(NULL::json AS BINARY)",
        "SELECT CAST(NULL AS BINARY)::json",
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
    let (id, _, _) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ?::json", &[17])
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
    let mysql = astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeJSON);
    let json = astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypePgJson);
    assert_eq!(
        super::pg_json::validate_cast(&mysql, &json)
            .unwrap_err()
            .pg_json_error()
            .unwrap()
            .sqlstate(),
        "0A000"
    );
    assert_eq!(
        super::pg_json::cast_value(Some("\0".into()))
            .unwrap_err()
            .pg_json_error()
            .unwrap()
            .sqlstate(),
        "22021"
    );
}
