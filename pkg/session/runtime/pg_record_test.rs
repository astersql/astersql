// Copyright 2026 AsterSQL.
use super::*;
#[test]
fn pg_dialect_11_normal_record_consumer() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        ("SELECT ROW(1,2)", "(1,2)"),
        ("SELECT ROW(1)", "(1)"),
        ("SELECT ROW(1,ROW(2,3))", "(1,\"(2,3)\")"),
        ("SELECT ROW(NULL)", "()"),
    ] {
        let (id, count, fields) = session.prepare_protocol_statement(sql).unwrap();
        assert_eq!(count, 0);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].column_as_name.O, "row");
        let results = session.execute_protocol_statement(id, &[]).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].rows, vec![vec![expected.to_owned()]]);
        session.close_protocol_statement(id).unwrap();
    }
}

#[test]
fn pg_dialect_11_record_type_layout() {
    let sql = "SELECT ROW(NULL,1,ROW(2,NULL))";
    let nodes = parse_in_dialect(sql, ast::SqlDialect::PostgreSQL).unwrap();
    let (types, _) = pg_record::analyze(&nodes, None).unwrap();
    let fields = types.values().find(|fields| fields.len() == 3).unwrap();
    assert_eq!(
        fields.iter().map(|field| field.oid).collect::<Vec<_>>(),
        [705, 23, 2249]
    );
    assert_eq!(
        fields[2]
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .map(|field| field.oid)
            .collect::<Vec<_>>(),
        [23, 705]
    );
    let tp = pg_record::field_type(fields.clone());
    let restored: astersql_parser_types::FieldType =
        serde_json::from_str(&serde_json::to_string(&tp).unwrap()).unwrap();
    assert_eq!(tp, restored);
    assert_eq!(tp.String(), "record");
    assert!(!tp.Equal(&astersql_parser_types::NewFieldType(
        astersql_parser_mysql::r#type::TypeVarString
    )));
    let mut changed = tp.clone();
    changed.SetType(astersql_parser_mysql::r#type::TypeLong);
    assert!(changed.PostgreSQLRecordFields().is_none());
}

#[test]
fn pg_dialect_12_grouping_and_record_fields() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg_row12_unit").unwrap();
    native
        .execute("CREATE TABLE pg_row12_unit.t (id INT)")
        .unwrap();
    native
        .execute("INSERT INTO pg_row12_unit.t VALUES(31)")
        .unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("USE pg_row12_unit").unwrap();
    for (sql, expected, name) in [
        ("SELECT (1,2)", "(1,2)", "row"),
        ("SELECT (1)", "1", "?column?"),
        ("SELECT (SELECT 1)", "1", "?column?"),
        ("SELECT ROW(t.id,(SELECT 2)) FROM t t", "(31,2)", "row"),
    ] {
        let (id, _, fields) = session.prepare_protocol_statement(sql).unwrap();
        assert_eq!(fields[0].column_as_name.O, name, "{sql}");
        let rows = session.execute_protocol_statement(id, &[]).unwrap();
        assert_eq!(rows[0].rows, vec![vec![expected.to_owned()]], "{sql}");
        session.close_protocol_statement(id).unwrap();
    }
    native.execute("DROP DATABASE pg_row12_unit").unwrap();
}

#[test]
fn pg_dialect_26_escape_result_type() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT ? ILIKE 'a!_b' ESCAPE ? AS v",
            &[25, 25],
        )
        .unwrap();
    assert_eq!(count, 2);
    assert_ne!(
        fields[0].column.FieldType.GetFlag() & astersql_parser_mysql::r#type::IsBooleanFlag,
        0
    );
    let records = session
        .execute_protocol_statement(
            id,
            &[
                ConcretePreparedArgument::Text("A_b".into()),
                ConcretePreparedArgument::Text("!".into()),
            ],
        )
        .unwrap();
    assert_eq!(records[0].rows, vec![vec!["1".to_owned()]]);
    assert_eq!(records[0].result_fields.iter().flatten().count(), 1);
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg_dialect_26_zero_row_constructor() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT ROW(?::int4,?::int4,?::int4) AS v WHERE false",
            &[23, 23, 23],
        )
        .unwrap();
    assert_eq!(count, 3);
    assert!(
        fields[0]
            .column
            .FieldType
            .PostgreSQLRecordFields()
            .is_some()
    );
    let records = session
        .execute_protocol_statement(
            id,
            &[
                ConcretePreparedArgument::Signed(9),
                ConcretePreparedArgument::Signed(7),
                ConcretePreparedArgument::Signed(9),
            ],
        )
        .unwrap();
    assert!(records[0].rows.is_empty());
    assert_eq!(records[0].result_fields.iter().flatten().count(), 1);
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg_dialect_26_assignment_occurrence_constraints() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.parameter26_constraints(id INT,v TEXT)")
        .unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("USE test").unwrap();
    assert_eq!(
        session
            .protocol_parameter_types(
                "INSERT INTO parameter26_constraints(id,v) VALUES(?,?) RETURNING id",
                &[]
            )
            .unwrap(),
        vec![23, 25]
    );
    native
        .execute("DROP TABLE test.parameter26_constraints")
        .unwrap();
}
