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

#[test]
fn pg05_array_cast_shape_identity_null_and_text() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, element, expected) in [
        (
            "SELECT '[0:1][-2:-1]={{-2147483648,NULL},{3,2147483647}}'::int4[]::int8[]",
            Element::Int8,
            "[0:1][-2:-1]={{-2147483648,NULL},{3,2147483647}}",
        ),
        (
            "SELECT '[0:1]={1,NULL}'::int8[]::int4[]",
            Element::Int4,
            "[0:1]={1,NULL}",
        ),
        ("SELECT ARRAY[]::int8[]", Element::Int8, "{}"),
        (
            "SELECT ARRAY[NULL,NULL]::int8[]",
            Element::Int8,
            "{NULL,NULL}",
        ),
        ("SELECT NULL::int4[]::int8[]", Element::Int8, "<nil>"),
        ("SELECT NULL::int8[]::int4[]", Element::Int4, "<nil>"),
        (
            "SELECT ARRAY[1,2147483648]::int8[]",
            Element::Int8,
            "{1,2147483648}",
        ),
        (
            "SELECT '[-1:0]={-9223372036854775808,9223372036854775807}'::int8[]",
            Element::Int8,
            "[-1:0]={-9223372036854775808,9223372036854775807}",
        ),
    ] {
        let mut result = session
            .execute(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(element),
            "{sql}"
        );
        assert_eq!(result[0].next_row().unwrap().unwrap(), [expected], "{sql}");
        result[0].close().unwrap();
        let nodes = parsed(sql);
        let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert_eq!(
            relational_expression_value(expr, &HashMap::new())
                .unwrap()
                .as_deref(),
            (expected != "<nil>").then_some(expected)
        );
    }
    for sql in [
        "SELECT '[0:1]={1,NULL}'::text::int8[]::text",
        "SELECT '[0:1]={1,NULL}'::int4[]::int8[]::text",
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLTextOid(),
            Some(25)
        );
        assert_eq!(result[0].next_row().unwrap().unwrap(), ["[0:1]={1,NULL}"]);
        result[0].close().unwrap();
    }
}

#[test]
fn pg05_array_cast_atomic_failure_and_rebinding() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, _, fields) = session
        .prepare_protocol_statement_with_parameter_types(
            "SELECT ?::text::int8[]::int4[] AS v",
            &[25],
        )
        .unwrap();
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Int4)
    );
    for (input, state) in [
        ("{1,2147483648}", "22003"),
        ("{1,-2147483649}", "22003"),
        ("{1,9223372036854775808}", "22003"),
        ("{1,nope}", "22P02"),
        ("{{1},{2,3}}", "22P02"),
    ] {
        let failure = session
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Text(input.into())])
            .err()
            .expect("cast must fail atomically");
        assert_eq!(
            failure.pg_array_error().unwrap().state,
            state,
            "{input}: {failure}"
        );
        let mut result = session
            .execute_protocol_statement(
                id,
                &[ConcretePreparedArgument::Text("[0:1]={7,NULL}".into())],
            )
            .unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap(), ["[0:1]={7,NULL}"]);
        result[0].close().unwrap();
    }
    session.close_protocol_statement(id).unwrap();
    for sql in [
        "SELECT ARRAY[2147483648]::integer[]",
        "SELECT '{2147483648}'::int4[]::int8[]",
    ] {
        let failure = session
            .execute(sql)
            .err()
            .expect("source/target range must be enforced");
        assert_eq!(
            failure.pg_array_error().unwrap().state,
            "22003",
            "{sql}: {failure}"
        );
    }
    for sql in [
        "SELECT 1::int8[]",
        "SELECT ARRAY[true]::int8[]",
        "SELECT '{}'::jsonb::int8[] WHERE false",
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
}

#[test]
fn pg05_array_text_declared_parameter_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1009])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Text)
    );
    for (input, expected) in [
        (
            Some("[0:1][-2:-1]={{a,NULL},{b,c}}"),
            Some("[0:1][-2:-1]={{a,NULL},{b,c}}"),
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
            Some(Element::Text)
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
fn pg05_array_text_shared_casts_and_constructors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY['NULL','','é',NULL]::text[] AS v",
            r#"{"NULL","",é,NULL}"#,
        ),
        ("SELECT '[-2:-1]={a,NULL}'::text[] AS v", "[-2:-1]={a,NULL}"),
        ("SELECT ARRAY[]::text[] AS v", "{}"),
        ("SELECT NULL::text[] AS v", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Text)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
    }
}

#[test]
fn pg05_array_bool_declared_parameter_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1000])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Bool)
    );
    for (input, expected) in [
        (
            Some("[0:1][-2:-1]={{t,NULL},{f,t}}"),
            Some("[0:1][-2:-1]={{t,NULL},{f,t}}"),
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
            Some(Element::Bool)
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
fn pg05_array_bool_shared_casts_and_constructors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY[true,false,NULL]::boolean[] AS v",
            "{t,f,NULL}",
        ),
        (
            "SELECT '[-2:-1]={yes,NULL}'::boolean[] AS v",
            "[-2:-1]={t,NULL}",
        ),
        ("SELECT ARRAY[]::boolean[] AS v", "{}"),
        ("SELECT NULL::boolean[] AS v", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Bool)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
    }
}

#[test]
fn pg05_array_bool_unopened_cross_family_casts_even_for_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT NULL::boolean[]::int4[]",
        "SELECT (NULL::int4[])::boolean[]",
        "SELECT ARRAY[true]::int8[]",
    ] {
        let failure = session
            .prepare_protocol_statement_with_parameter_types(sql, &[])
            .unwrap_err();
        assert_eq!(
            failure.pg_array_error().map(|e| e.state),
            Some("0A000"),
            "{sql}: {failure}"
        );
    }
}
#[test]
fn pg05_array_int2_declared_parameter_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1005])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Int2)
    );
    for (input, expected) in [
        (
            Some("[0:1][-2:-1]={{-32768,NULL},{0,32767}}"),
            Some("[0:1][-2:-1]={{-32768,NULL},{0,32767}}"),
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
            Some(Element::Int2)
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
fn pg05_array_int2_shared_casts_and_constructors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY[-32768,32767,NULL]::int2[] AS v",
            "{-32768,32767,NULL}",
        ),
        (
            "SELECT '[-2:-1]={-32768,NULL}'::int2[] AS v",
            "[-2:-1]={-32768,NULL}",
        ),
        ("SELECT ARRAY[]::int2[] AS v", "{}"),
        ("SELECT NULL::int2[] AS v", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Int2)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
    }
}

#[test]
fn pg05_array_int2_unopened_cross_family_casts_even_for_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT NULL::int2[]::int4[]",
        "SELECT (NULL::int4[])::int2[]",
        "SELECT ARRAY[1::int2]::boolean[]",
    ] {
        let failure = session
            .prepare_protocol_statement_with_parameter_types(sql, &[])
            .unwrap_err();
        assert_eq!(
            failure.pg_array_error().map(|e| e.state),
            Some("0A000"),
            "{sql}: {failure}"
        );
    }
}

#[test]
fn pg05_array_int2_scalar_identity_and_common_integer_type() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, element, expected) in [
        (
            "SELECT ARRAY[(-32768)::int2,32767::int2,NULL]",
            Element::Int2,
            "{-32768,32767,NULL}",
        ),
        ("SELECT ARRAY[1::int2,2::int4]", Element::Int4, "{1,2}"),
        (
            "SELECT ARRAY[1::int2,2147483648::int8]",
            Element::Int8,
            "{1,2147483648}",
        ),
    ] {
        let (id, count, fields) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[])
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            fields[0].column.FieldType.PostgreSQLArrayElement(),
            Some(element)
        );
        let mut result = session.execute_protocol_statement(id, &[]).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(element)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
        session.close_protocol_statement(id).unwrap();
    }
}

#[test]
fn pg05_array_int8_declared_parameter_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1016])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Int8)
    );
    for (input, expected) in [
        (
            Some("[0:1][-2:-1]={{-9223372036854775808,NULL},{0,9223372036854775807}}"),
            Some("[0:1][-2:-1]={{-9223372036854775808,NULL},{0,9223372036854775807}}"),
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
            Some(Element::Int8)
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
fn pg05_array_int8_shared_casts_and_constructors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY[-9223372036854775808,9223372036854775807,NULL]::int8[] AS v",
            "{-9223372036854775808,9223372036854775807,NULL}",
        ),
        (
            "SELECT '[-2:-1]={-9223372036854775808,NULL}'::int8[] AS v",
            "[-2:-1]={-9223372036854775808,NULL}",
        ),
        ("SELECT ARRAY[]::int8[] AS v", "{}"),
        ("SELECT NULL::int8[] AS v", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Int8)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
    }
}

#[test]
fn pg05_array_bytea_declared_parameter_identity() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    let (id, count, fields) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1001])
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields[0].column.FieldType.PostgreSQLArrayElement(),
        Some(Element::Bytea)
    );
    for (input, expected) in [
        (
            Some(r#"[0:1][-2:-1]={{"\\x00ff",NULL},{"\\x","\\xc328"}}"#),
            Some(r#"[0:1][-2:-1]={{"\\x00ff",NULL},{"\\x","\\xc328"}}"#),
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
            Some(Element::Bytea)
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
fn pg05_array_bytea_shared_casts_and_constructors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY['abc','',NULL]::bytea[] AS v",
            r#"{"\\x616263","\\x",NULL}"#,
        ),
        (
            "SELECT ARRAY[['a',NULL],['b','']]::bytea[] AS v",
            r#"{{"\\x61",NULL},{"\\x62","\\x"}}"#,
        ),
        (
            "SELECT '[-2:-1]={abc,NULL}'::bytea[] AS v",
            r#"[-2:-1]={"\\x616263",NULL}"#,
        ),
        ("SELECT ARRAY[]::bytea[] AS v", "{}"),
        (
            "SELECT ARRAY['__astersql_binary_hex__:00ff']::bytea[] AS v",
            r#"{"\\x5f5f617374657273716c5f62696e6172795f6865785f5f3a30306666"}"#,
        ),
        ("SELECT NULL::bytea[] AS v", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(
            result[0].result_fields()[0]
                .as_ref()
                .unwrap()
                .column
                .FieldType
                .PostgreSQLArrayElement(),
            Some(Element::Bytea)
        );
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
    }
    // The other shared evaluator must delegate the same cast, not MySQL text.
    let node = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseOneStmt("SELECT ARRAY['abc',NULL]::bytea[]", "", "")
        .unwrap();
    let select = node.as_any().downcast_ref::<ast::SelectStmt>().unwrap();
    assert_eq!(
        super::relational_value::relational_expression_value(
            select.Fields.Fields[0].Expr.as_ref().unwrap(),
            &HashMap::new()
        )
        .unwrap(),
        Some(r#"{"\\x616263",NULL}"#.into())
    );
    for sql in [
        "SELECT ARRAY[1]::bytea[]",
        "SELECT ARRAY[true]::bytea[]",
        "SELECT '{}'::int4[]::bytea[]",
        "SELECT '{}'::bytea[]::text[]",
    ] {
        let err = session.execute(sql).err().expect(sql);
        assert!(
            err.to_string()
                .contains("array cast source is not supported"),
            "{sql}: {err}"
        );
    }
}

#[test]
fn pg05_array_bytea_scalar_parameter_constructor() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in ["SELECT ARRAY[?,NULL] AS v", "SELECT ARRAY[(?),NULL] AS v"] {
        let (id, _, fields) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[17])
            .unwrap();
        assert_eq!(
            fields[0].column.FieldType.PostgreSQLArrayElement(),
            Some(Element::Bytea)
        );
        for (argument, expected) in [
            (
                ConcretePreparedArgument::Bytes(vec![0, 255, 195, 40, 92]),
                r#"{"\\x00ffc3285c",NULL}"#,
            ),
            (ConcretePreparedArgument::Bytes(vec![]), r#"{"\\x",NULL}"#),
            (ConcretePreparedArgument::Null, "{NULL,NULL}"),
        ] {
            let mut result = session.execute_protocol_statement(id, &[argument]).unwrap();
            assert_eq!(
                result[0].next_row().unwrap().unwrap(),
                vec![expected.to_owned()]
            );
            assert_eq!(
                result[0].result_fields()[0]
                    .as_ref()
                    .unwrap()
                    .column
                    .FieldType
                    .PostgreSQLArrayElement(),
                Some(Element::Bytea)
            );
            result[0].close().unwrap();
        }
        session.close_protocol_statement(id).unwrap();
    }
}

#[test]
fn pg05_array_numeric_runtime_typmod_constructor_and_errors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={1.25,NULL}'::numeric(3,1)[]",
            "[-2:-1]={1.3,NULL}",
        ),
        ("SELECT '{1250,-1250}'::numeric(2,-2)[]", "{1300,-1300}"),
        ("SELECT ARRAY[1.25,NULL]::numeric(3,1)[]", "{1.3,NULL}"),
        (
            "SELECT ARRAY['NaN'::numeric,'Infinity'::numeric,NULL]",
            "{NaN,Infinity,NULL}",
        ),
    ] {
        let (id, _, fields) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[])
            .unwrap();
        assert_eq!(
            fields[0].column.FieldType.PostgreSQLArrayElement(),
            Some(Element::Numeric)
        );
        let mut result = session.execute_protocol_statement(id, &[]).unwrap();
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()],
            "{sql}"
        );
        result[0].close().unwrap();
        session.close_protocol_statement(id).unwrap();
    }
    for sql in [
        "SELECT '{1,bad}'::numeric[]",
        "SELECT '{1,Infinity}'::numeric(3,1)[]",
        "SELECT '{1,99.95}'::numeric(3,1)[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
        let mut result = session.execute("SELECT 1").unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["1".to_owned()]);
        result[0].close().unwrap();
    }
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ARRAY[?,NULL] AS v", &[1700])
        .unwrap();
    for (argument, expected) in [
        (
            ConcretePreparedArgument::Text("12.340".into()),
            "{12.340,NULL}",
        ),
        (ConcretePreparedArgument::Null, "{NULL,NULL}"),
    ] {
        let mut result = session.execute_protocol_statement(id, &[argument]).unwrap();
        assert_eq!(
            result[0].next_row().unwrap().unwrap(),
            vec![expected.to_owned()]
        );
        result[0].close().unwrap();
    }
    session.close_protocol_statement(id).unwrap();
}
