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

#[test]
fn pg05_record_consumer_layout_values_and_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected, oids) in [
        ("SELECT ROW(1)", "(1)", vec![23]),
        ("SELECT ROW(NULL)", "()", vec![705]),
        ("SELECT ROW(1,ROW(2,NULL))", "(1,\"(2,)\")", vec![23, 2249]),
        ("SELECT (1,2)", "(1,2)", vec![23, 23]),
        (
            "SELECT ROW(NULL::uuid,NULL::json,NULL::jsonb,NULL::int4[])",
            "(,,,)",
            vec![2950, 114, 3802, 1007],
        ),
        (
            "SELECT ROW('{1,NULL}'::int4[],NULL::jsonb[])",
            "(\"{1,NULL}\",)",
            vec![1007, 3807],
        ),
        (
            "SELECT ROW('00000000000000000000000000000001'::uuid,' [1, 2] '::json,'{\"aa\":1,\"b\":2}'::jsonb)",
            "(00000000-0000-0000-0000-000000000001,\" [1, 2] \",\"{\"\"b\"\": 2, \"\"aa\"\": 1}\")",
            vec![2950, 114, 3802],
        ),
    ] {
        let (id, _, fields) = session
            .prepare_protocol_statement(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        let layout = fields[0].column.FieldType.PostgreSQLRecordFields().unwrap();
        assert_eq!(
            layout.iter().map(|f| f.oid).collect::<Vec<_>>(),
            oids,
            "{sql}"
        );
        if sql.contains("ROW(2,NULL)") {
            assert_eq!(
                layout[1]
                    .fields
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|f| f.oid)
                    .collect::<Vec<_>>(),
                [23, 705]
            );
        }
        let result = session.execute_protocol_statement(id, &[]).unwrap();
        assert_eq!(result[0].rows, vec![vec![expected.to_owned()]], "{sql}");
        session.close_protocol_statement(id).unwrap();
    }
    let (_, _, fields) = session.prepare_protocol_statement("SELECT (1)").unwrap();
    assert!(
        fields[0]
            .column
            .FieldType
            .PostgreSQLRecordFields()
            .is_none()
    );
}

#[test]
fn pg05_record_consumer_declared_parameters_and_rebinding() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (oid, text, expected) in [
        (
            2950,
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000001",
        ),
        (114, " [null] ", "\" [null] \""),
        (3802, "1.00", "1.00"),
        (1007, "[0:1]={1,NULL}", "\"[0:1]={1,NULL}\""),
        (3807, "{\"null\",NULL}", "\"{\"\"null\"\",NULL}\""),
    ] {
        let sql = "SELECT ROW(?,?,?)";
        let (id, count, fields) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[oid, oid, oid])
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            fields[0]
                .column
                .FieldType
                .PostgreSQLRecordFields()
                .unwrap()
                .iter()
                .map(|f| f.oid)
                .collect::<Vec<_>>(),
            [oid, oid, oid]
        );
        for args in [
            vec![
                ConcretePreparedArgument::Null,
                ConcretePreparedArgument::Text(text.into()),
                ConcretePreparedArgument::Null,
            ],
            vec![
                ConcretePreparedArgument::Text(text.into()),
                ConcretePreparedArgument::Null,
                ConcretePreparedArgument::Text(text.into()),
            ],
        ] {
            let first_null = matches!(args[0], ConcretePreparedArgument::Null);
            let result = session.execute_protocol_statement(id, &args).unwrap();
            let expected = if first_null {
                format!("(,{expected},)")
            } else {
                format!("({expected},,{expected})")
            };
            assert_eq!(result[0].rows, vec![vec![expected]], "oid {oid}");
        }
        session.close_protocol_statement(id).unwrap();
    }
}

#[test]
fn pg05_record_consumer_scalar_subquery_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (cast, oid) in [
        ("uuid", 2950),
        ("json", 114),
        ("jsonb", 3802),
        ("jsonb[]", 3807),
    ] {
        let sql = format!("SELECT ROW((SELECT NULL::{cast}))");
        let (id, _, fields) = session.prepare_protocol_statement(&sql).unwrap();
        assert_eq!(
            fields[0].column.FieldType.PostgreSQLRecordFields().unwrap()[0].oid,
            oid,
            "{sql}"
        );
        assert_eq!(
            session.execute_protocol_statement(id, &[]).unwrap()[0].rows,
            vec![vec!["()".to_owned()]]
        );
        session.close_protocol_statement(id).unwrap();
    }
}

#[test]
fn pg05_record_consumer_unsupported_and_invalid_recover() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT ROW(1)=ROW(1)",
        "SELECT ARRAY[ROW(1)]",
        "SELECT ROW(ARRAY[1,NULL])",
    ] {
        let e = session.prepare_protocol_statement(sql).unwrap_err();
        assert_eq!(
            e.pg_array_error().map(|e| e.state),
            Some("0A000"),
            "{sql}: {e}"
        );
    }
    let e = session
        .prepare_protocol_statement_with_parameter_types("SELECT ROW($1)", &[2249])
        .unwrap_err();
    assert_eq!(e.pg_array_error().map(|e| e.state), Some("0A000"));
    let (id, _, _) = session
        .prepare_protocol_statement("SELECT ROW('bad'::uuid)")
        .unwrap();
    assert_eq!(
        session
            .execute_protocol_statement(id, &[])
            .err()
            .unwrap()
            .pg_uuid_error()
            .map(|e| e.sqlstate()),
        Some("22P02")
    );
    session.close_protocol_statement(id).unwrap();
    assert_eq!(
        session.execute("SELECT ROW(1)").unwrap()[0].rows,
        vec![vec!["(1)".to_owned()]]
    );
}

#[test]
fn pg05_record_consumer_all_array_null_layouts_and_empty_results() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (name, oid) in [
        ("bool", 1000),
        ("bytea", 1001),
        ("int2", 1005),
        ("int4", 1007),
        ("text", 1009),
        ("bpchar", 1014),
        ("varchar", 1015),
        ("int8", 1016),
        ("float4", 1021),
        ("float8", 1022),
        ("numeric", 1231),
        ("date", 1182),
        ("time", 1183),
        ("timestamp", 1115),
        ("timestamptz", 1185),
        ("interval", 1187),
        ("uuid", 2951),
        ("json", 199),
        ("jsonb", 3807),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT ROW(NULL::{name}[]){}",
                if empty { " WHERE false" } else { "" }
            );
            let (id, _, fields) = session.prepare_protocol_statement(&sql).unwrap();
            let layout = fields[0].column.FieldType.PostgreSQLRecordFields().unwrap();
            assert_eq!(layout[0].oid, oid, "{sql}");
            let results = session.execute_protocol_statement(id, &[]).unwrap();
            if empty {
                assert!(results[0].rows.is_empty());
            } else {
                assert_eq!(results[0].rows, vec![vec!["()".to_owned()]]);
            }
            assert_eq!(
                results[0].result_fields[0]
                    .as_ref()
                    .unwrap()
                    .column
                    .FieldType
                    .PostgreSQLRecordFields(),
                Some(layout)
            );
            session.close_protocol_statement(id).unwrap();
        }
    }
}

#[test]
fn pg05_record_consumer_nested_subquery_layout() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, _, fields) = session
        .prepare_protocol_statement("SELECT ROW((SELECT ROW(1,NULL)))")
        .unwrap();
    let layout = fields[0].column.FieldType.PostgreSQLRecordFields().unwrap();
    assert_eq!(layout[0].oid, 2249);
    assert_eq!(
        layout[0]
            .fields
            .as_ref()
            .unwrap()
            .iter()
            .map(|f| f.oid)
            .collect::<Vec<_>>(),
        [23, 705]
    );
    assert_eq!(
        session.execute_protocol_statement(id, &[]).unwrap()[0].rows,
        vec![vec!["(\"(1,)\")".to_owned()]]
    );
    session.close_protocol_statement(id).unwrap();
}
