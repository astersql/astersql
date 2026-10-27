// Copyright 2026 AsterSQL.
use super::*;

const INPUT: &str = "{00112233445566778899AABBCCDDEEFF}";
const OUTPUT: &str = "00112233-4455-6677-8899-aabbccddeeff";

#[test]
fn pg05_uuid_sql_cast_roundtrip_and_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        format!("SELECT '{INPUT}'::text::uuid"),
        format!("SELECT CAST('{INPUT}' AS uuid)"),
        format!("SELECT ('{INPUT}'::uuid)::text::uuid"),
    ] {
        let mut records = pg.execute(&sql).unwrap();
        assert_eq!(records[0].next_row().unwrap().unwrap(), [OUTPUT]);
        let fields = pg.describe_result_fields(&sql).unwrap();
        assert_eq!(fields[0].column.FieldType.GetType(), 0xe4);
    }
    let sql = format!("SELECT '{INPUT}'::uuid::text");
    assert_eq!(
        pg.execute(&sql).unwrap()[0].next_row().unwrap().unwrap(),
        [OUTPUT]
    );
    assert_eq!(
        pg.describe_result_fields(&sql).unwrap()[0]
            .column
            .FieldType
            .PostgreSQLTextOid(),
        Some(25)
    );
    let sql = format!("SELECT '{INPUT}'::text");
    assert_eq!(
        pg.execute(&sql).unwrap()[0].next_row().unwrap().unwrap(),
        [INPUT]
    );
    assert_eq!(
        pg.describe_result_fields(&sql).unwrap()[0]
            .column
            .FieldType
            .PostgreSQLTextOid(),
        Some(25)
    );
}

#[test]
fn pg05_uuid_sql_null_and_empty_keep_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in ["SELECT NULL::uuid", "SELECT NULL::uuid WHERE false"] {
        let mut records = pg.execute(sql).unwrap();
        let fields = pg.describe_result_fields(sql).unwrap();
        assert_eq!(fields[0].column.FieldType.GetType(), 0xe4);
        assert_eq!(fields[0].column.FieldType.GetDecimal(), -1);
        if sql.contains("WHERE") {
            assert!(records[0].next_row().unwrap().is_none());
        } else {
            assert_eq!(records[0].next_row().unwrap().unwrap(), ["<nil>"]);
        }
    }
}

#[test]
fn pg05_uuid_sql_parameter_context_and_recovery() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    assert_eq!(
        pg.protocol_parameter_types("SELECT ?::uuid", &[0]).unwrap(),
        [2950]
    );
    assert_eq!(
        pg.protocol_parameter_types("SELECT ?::uuid", &[25])
            .unwrap(),
        [25]
    );
    assert_eq!(
        pg.protocol_parameter_types("SELECT ?::uuid", &[17])
            .unwrap(),
        [17]
    );
    let (id, _, _) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ?::uuid, ?::uuid::text", &[25, 25])
        .unwrap();
    for value in [
        INPUT,
        "00000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffff",
    ] {
        let expected = value
            .parse::<astersql_types::pg_uuid::PgUuid>()
            .unwrap()
            .to_string();
        let mut records = pg
            .execute_protocol_statement(
                id,
                &[
                    ConcretePreparedArgument::Text(value.into()),
                    ConcretePreparedArgument::Text(value.into()),
                ],
            )
            .unwrap();
        assert_eq!(
            records[0].next_row().unwrap().unwrap(),
            [expected.clone(), expected]
        );
    }
    assert!(
        pg.execute_protocol_statement(
            id,
            &[
                ConcretePreparedArgument::Text("bad".into()),
                ConcretePreparedArgument::Text(INPUT.into())
            ]
        )
        .is_err()
    );
    assert!(
        pg.execute_protocol_statement(
            id,
            &[
                ConcretePreparedArgument::Null,
                ConcretePreparedArgument::Null
            ]
        )
        .is_ok()
    );
    pg.close_protocol_statement(id).unwrap();
    assert_eq!(
        pg.execute(&format!("SELECT '{INPUT}'::uuid")).unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        [OUTPUT]
    );
}

#[test]
fn pg05_uuid_sql_errors_before_null_and_empty_evaluation() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 'bad'::uuid",
        "SELECT ''::text::uuid",
        "SELECT ' 00112233445566778899aabbccddeeff'::uuid",
    ] {
        let error = pg.execute(sql).err().unwrap();
        assert_eq!(error.pg_uuid_error().unwrap().sqlstate(), "22P02", "{sql}");
    }
    for sql in [
        "SELECT 1::uuid",
        "SELECT NULL::int4::uuid",
        "SELECT NULL::uuid::int4",
        "SELECT NULL::int4::uuid WHERE false",
    ] {
        let error = pg.execute(sql).err().unwrap();
        assert_eq!(error.pg_uuid_error().unwrap().sqlstate(), "0A000", "{sql}");
    }
    let (id, _, _) = pg
        .prepare_protocol_statement_with_parameter_types("SELECT ?::uuid", &[17])
        .unwrap();
    let error = pg
        .execute_protocol_statement(id, &[ConcretePreparedArgument::Null])
        .err()
        .unwrap();
    assert_eq!(error.pg_uuid_error().unwrap().sqlstate(), "0A000");
    pg.close_protocol_statement(id).unwrap();
    assert!(pg.execute("SELECT NULL::uuid::text").is_ok());
}

#[test]
fn pg05_uuid_sql_declared_parameter_identity_survives_null_and_results() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (oid, expected_type, expected_text) in [
        (2950, astersql_parser_mysql::r#type::TypePgUuid, OUTPUT),
        (25, astersql_parser_mysql::r#type::TypeVarString, INPUT),
    ] {
        let (id, _, fields) = pg
            .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[oid])
            .unwrap();
        assert_eq!(fields[0].column.FieldType.GetType(), expected_type);
        for (argument, expected) in [
            (ConcretePreparedArgument::Text(INPUT.into()), expected_text),
            (ConcretePreparedArgument::Null, "<nil>"),
        ] {
            let mut records = pg.execute_protocol_statement(id, &[argument]).unwrap();
            assert_eq!(records[0].next_row().unwrap().unwrap(), [expected]);
            assert_eq!(
                records[0].result_fields()[0]
                    .as_ref()
                    .unwrap()
                    .column
                    .FieldType
                    .GetType(),
                expected_type
            );
        }
        pg.close_protocol_statement(id).unwrap();
    }
}
