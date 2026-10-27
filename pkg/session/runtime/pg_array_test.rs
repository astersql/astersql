// Copyright 2026 AsterSQL.
use super::*;
use astersql_parser_types::PostgreSQLArrayElement as Element;
fn parsed(sql: &str) -> Vec<Box<dyn ast::Node>> {
    parse_in_dialect(sql, ast::SqlDialect::PostgreSQL).unwrap()
}
#[test]
fn pg_dialect_09_array_analysis_and_values() {
    for (sql, element, value) in [
        ("SELECT ARRAY[1,2]", Element::Int4, "{1,2}"),
        ("SELECT ARRAY[NULL,1]", Element::Int4, "{NULL,1}"),
        (
            "SELECT ARRAY[1,2147483648]",
            Element::Int8,
            "{1,2147483648}",
        ),
        ("SELECT ARRAY[NULL,NULL]", Element::Text, "{NULL,NULL}"),
        (
            "SELECT ARRAY['中,文','NULL','']",
            Element::Text,
            "{\"中,文\",\"NULL\",\"\"}",
        ),
        (
            "SELECT ARRAY[CAST(NULL AS INT4),CAST(7 AS INT4)]",
            Element::Int4,
            "{NULL,7}",
        ),
    ] {
        let nodes = parsed(sql);
        let (types, _) = pg_array::analyze(sql, &nodes, None).unwrap();
        assert_eq!(types.values().copied().collect::<Vec<_>>(), [element]);
        let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert_eq!(
            pg_array::value(expr, &HashMap::new()).unwrap(),
            Some(value.into())
        );
    }
    let sql = "SELECT ARRAY[$2,$1,$2]";
    let nodes = parsed(sql);
    let (types, oids) = pg_array::analyze(sql, &nodes, Some(&[23, 23, 23])).unwrap();
    assert_eq!(oids, [23, 23, 23]);
    assert_eq!(types.values().next(), Some(&Element::Int4));
    assert_eq!(
        pg_array::analyze(sql, &nodes, Some(&[])).unwrap().1,
        [25, 25, 25]
    );
    assert_eq!(
        pg_array::analyze(sql, &nodes, None)
            .unwrap_err()
            .pg_array_error()
            .unwrap()
            .state,
        "42P02"
    );
    assert_eq!(
        pg_array::analyze("SELECT ARRAY[]", &parsed("SELECT ARRAY[]"), None)
            .unwrap_err()
            .pg_array_error()
            .unwrap()
            .state,
        "42P18"
    );
    assert_eq!(
        pg_array::analyze(
            "SELECT ARRAY[true,1]",
            &parsed("SELECT ARRAY[true,1]"),
            None
        )
        .unwrap_err()
        .pg_array_error()
        .unwrap()
        .state,
        "42804"
    );
}

#[test]
fn pg_dialect_10_nested_array_shape_and_common_type() {
    for (sql, element, value) in [
        (
            "SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]",
            Element::Int4,
            "{{1,2},{3,4}}",
        ),
        ("SELECT ARRAY[[1,2],[3,4]]", Element::Int4, "{{1,2},{3,4}}"),
        (
            "SELECT ARRAY[[NULL,1],[2,NULL]]",
            Element::Int4,
            "{{NULL,1},{2,NULL}}",
        ),
        ("SELECT ARRAY[1,2.5]", Element::Numeric, "{1,2.5}"),
    ] {
        let nodes = parsed(sql);
        let (types, _) = pg_array::analyze(sql, &nodes, None).unwrap();
        let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert_eq!(
            types.get(&(expr.OriginTextPosition as usize)),
            Some(&element)
        );
        assert_eq!(
            pg_array::value(expr, &HashMap::new()).unwrap(),
            Some(value.into())
        );
    }

    let ragged_sql = "SELECT ARRAY[[1,2],[3]]";
    let ragged = parsed(ragged_sql);
    assert!(pg_array::analyze(ragged_sql, &ragged, None).is_ok());
    let expr = ragged[0]
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .unwrap()
        .Fields
        .Fields[0]
        .Expr
        .as_ref()
        .unwrap();
    let error = pg_array::value(expr, &HashMap::new()).unwrap_err();
    let error = error.pg_array_error().unwrap();
    assert_eq!(error.state, "2202E");
    assert!(error.message.contains("matching dimensions"));

    let mixed_sql = "/*中*/ SELECT ARRAY[1,true]";
    let error = pg_array::analyze(mixed_sql, &parsed(mixed_sql), None).unwrap_err();
    let error = error.pg_array_error().unwrap();
    assert_eq!(error.state, "42804");
    assert_eq!(error.position, Some(mixed_sql.find("true").unwrap()));
    assert_eq!(
        error.message,
        "ARRAY types integer and boolean cannot be matched"
    );

    let parameter_sql = "SELECT ARRAY[ARRAY[$2,$1],ARRAY[$1,$2]]";
    let parameters = parsed(parameter_sql);
    let (_, oids) = pg_array::analyze(parameter_sql, &parameters, Some(&[23, 23])).unwrap();
    assert!(!oids.is_empty());
    assert!(
        oids.iter().all(|oid| *oid == 23),
        "inferred parameter OIDs: {oids:?}"
    );
}

#[test]
fn pg_dialect_09_array_normal_prepared_consumer() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ARRAY[?,?,?]", &[23, 23, 23])
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Int4)
    );
    for (args, value) in [
        (
            vec![
                ConcretePreparedArgument::Signed(9),
                ConcretePreparedArgument::Signed(7),
                ConcretePreparedArgument::Signed(9),
            ],
            "{9,7,9}",
        ),
        (vec![ConcretePreparedArgument::Null; 3], "{NULL,NULL,NULL}"),
    ] {
        let mut records = session.execute_protocol_statement(id, &args).unwrap();
        assert_eq!(
            records[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Int4)
        );
        assert_eq!(
            records[0].next_row().unwrap().unwrap(),
            vec![value.to_owned()]
        );
        records[0].close().unwrap();
    }
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg_dialect_09_array_rejects_unbound_contexts() {
    let sql = "WITH c AS (SELECT ARRAY[$1]) SELECT * FROM c";
    assert_eq!(
        pg_array::analyze(sql, &parsed(sql), Some(&[23]))
            .unwrap_err()
            .pg_array_error()
            .unwrap()
            .state,
        "0A000"
    );
}

#[test]
fn pg05_int4_array_required_integer_context() {
    for sql in [
        "SELECT $1::integer[] AS v",
        "SELECT NULL::integer[] AS v",
        "SELECT ARRAY[]::integer[] AS v",
        "SELECT CAST($1 AS integer[]) AS v",
    ] {
        let parsed = parse_in_dialect(sql, ast::SqlDialect::PostgreSQL);
        assert!(
            parsed.is_ok(),
            "required int4 array context {sql}: {:?}",
            parsed.err()
        );
    }
}
#[test]
fn pg05_int4_array_declared_parameter_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1007])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Int4)
    );
    for (input, expected) in [
        (
            Some("[0:1][-2:-1]={{1,NULL},{3,4}}"),
            Some("[0:1][-2:-1]={{1,NULL},{3,4}}"),
        ),
        (Some("{}"), Some("{}")),
        (None, None),
    ] {
        let argument = input
            .map(|s| ConcretePreparedArgument::Text(s.to_owned()))
            .unwrap_or(ConcretePreparedArgument::Null);
        let mut result = session.execute_protocol_statement(id, &[argument]).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Int4)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.unwrap_or("<nil>").to_owned()]
        );
        result[0].close().unwrap();
    }
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg05_int4_array_constructors_and_rejected_bridges() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        ("SELECT ARRAY[]::integer[]", "{}"),
        ("SELECT ARRAY[NULL,NULL]::integer[]", "{NULL,NULL}"),
        (
            "SELECT ARRAY[[1,NULL],[3,4]]::integer[][]",
            "{{1,NULL},{3,4}}",
        ),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Int4)
        );
        assert_eq!(result[0].next_row().unwrap().unwrap(), [expected]);
        result[0].close().unwrap();
    }
    for sql in [
        "SELECT 1::integer[]",
        "SELECT true::integer[]",
        "SELECT ARRAY[true]::integer[]",
        "SELECT ARRAY[2147483648]::integer[]",
        "SELECT '{}'::jsonb::integer[] WHERE false",
    ] {
        let failure = session
            .prepare_protocol_statement_with_parameter_types(sql, &[])
            .unwrap_err();
        assert_eq!(
            failure
                .pg_array_error()
                .map(|e| e.state)
                .or_else(|| failure.pg_json_error().map(|e| e.sqlstate())),
            Some("0A000"),
            "{sql}: {failure}"
        );
    }
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ARRAY[?]::integer[]", &[])
        .unwrap();
    for (arg, expected) in [
        (ConcretePreparedArgument::Signed(7), "{7}"),
        (ConcretePreparedArgument::Null, "{NULL}"),
    ] {
        let mut result = session.execute_protocol_statement(id, &[arg]).unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap(), [expected]);
        result[0].close().unwrap();
    }
    session.close_protocol_statement(id).unwrap();
}
