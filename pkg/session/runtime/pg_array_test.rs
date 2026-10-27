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

#[test]
fn pg05_array_float4_private_binding_and_public_text_isolation() {
    use astersql_types::pg_array::{PgArray, PgArrayElement};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in ["SELECT ARRAY[?,NULL] AS v", "SELECT ARRAY[(?),NULL] AS v"] {
        let (id, _, fields) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[700])
            .unwrap();
        assert_eq!(
            fields[0].column.FieldType.PostgreSQLArrayElement(),
            Some(Element::Float4)
        );
        for bits in [0, 0x80000000, 1, 0x7fc12345, 0xffa12345] {
            let mut result = session
                .execute_protocol_statement(id, &[ConcretePreparedArgument::Float4Bits(bits)])
                .unwrap();
            let row = result[0].next_row().unwrap().unwrap();
            let bytes = super::row_codec::binary_runtime_bytes(&row[0]).unwrap();
            let array = PgArray::float4_from_binary(&bytes).unwrap();
            assert_eq!(array.elements(), [Some(PgArrayElement::Float4(bits)), None]);
            result[0].close().unwrap();
        }
        session.close_protocol_statement(id).unwrap();
    }
    for sql in [
        "SELECT '__astersql_binary_hex__:7fc12345'::float4",
        "SELECT '__astersql_binary_hex__:00000000'::float4[]",
        "SELECT '{1,bad}'::float4[]",
        "SELECT NULL::float4[]::float8[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
    }
    assert!(session.state.borrow().pg_bound_float4_constants.is_empty());
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_float4_sql_constructors_and_failure_restore() {
    use astersql_types::pg_array::PgArray;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={1.5,NULL}'::float4[]",
            "[-2:-1]={1.5,NULL}",
        ),
        ("SELECT ARRAY[]::real[]", "{}"),
        ("SELECT ARRAY[NULL,NULL]::float4[]", "{NULL,NULL}"),
        (
            "SELECT ARRAY[['-0'::float4,NULL],['NaN'::float4,'1.5'::float4]]",
            "{{-0,NULL},{NaN,1.5}}",
        ),
        ("SELECT '{NaN,-0}'::float4[]::float4[]", "{NaN,-0}"),
    ] {
        let mut results = session.execute(sql).unwrap();
        let row = results[0].next_row().unwrap().unwrap();
        let bytes = super::row_codec::binary_runtime_bytes(&row[0]).unwrap();
        assert_eq!(
            PgArray::float4_from_binary(&bytes)
                .unwrap()
                .to_text(b',')
                .unwrap(),
            expected,
            "{sql}"
        );
        results[0].close().unwrap();
    }
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1021])
        .unwrap();
    assert!(
        session
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Float4Array(vec![0])])
            .is_err()
    );
    assert!(session.state.borrow().pg_bound_float4_constants.is_empty());
    assert!(session.execute("SELECT 1").is_ok());
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg05_array_float4_scalar_cast_preserves_typed_bits() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, raw) in [
        ("SELECT ?::float4 AS v", true),
        ("SELECT ?::text AS v", false),
        ("SELECT ?::float8 AS v", false),
    ] {
        let (id, _, _) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[700])
            .unwrap();
        let mut result = session
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Float4Bits(0xffa12345)])
            .unwrap();
        let row = result[0].next_row().unwrap().unwrap();
        if raw {
            assert_eq!(
                super::row_codec::binary_runtime_bytes(&row[0]).unwrap(),
                0xffa12345u32.to_be_bytes()
            );
        } else {
            assert_eq!(row[0], "NaN");
        }
        result[0].close().unwrap();
        session.close_protocol_statement(id).unwrap();
    }

    use astersql_types::pg_array::{PgArray, PgArrayElement};
    let array = PgArray::new(
        700,
        -1,
        vec![2],
        vec![-1],
        vec![Some(PgArrayElement::Float4(0xffa12345)), None],
    )
    .unwrap();
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ?::text AS v", &[1021])
        .unwrap();
    let mut result = session
        .execute_protocol_statement(
            id,
            &[ConcretePreparedArgument::Float4Array(
                array.to_binary().unwrap(),
            )],
        )
        .unwrap();
    assert_eq!(
        result[0].next_row().unwrap().unwrap(),
        vec!["[-1:0]={NaN,NULL}".to_owned()]
    );
    result[0].close().unwrap();
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg05_array_float8_private_binding_and_public_text_isolation() {
    use astersql_types::pg_array::{PgArray, PgArrayElement};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in ["SELECT ARRAY[?,NULL] AS v", "SELECT ARRAY[(?),NULL] AS v"] {
        let (id, _, fields) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[701])
            .unwrap();
        assert_eq!(
            fields[0].column.FieldType.PostgreSQLArrayElement(),
            Some(Element::Float8)
        );
        for bits in [
            0,
            0x8000000000000000,
            1,
            0x7ff812345678abcd,
            0xfff012345678abcd,
        ] {
            let mut result = session
                .execute_protocol_statement(id, &[ConcretePreparedArgument::Float8Bits(bits)])
                .unwrap();
            let row = result[0].next_row().unwrap().unwrap();
            let bytes = super::row_codec::binary_runtime_bytes(&row[0]).unwrap();
            let array = PgArray::float8_from_binary(&bytes).unwrap();
            assert_eq!(array.elements(), [Some(PgArrayElement::Float8(bits)), None]);
            result[0].close().unwrap();
        }
        session.close_protocol_statement(id).unwrap();
    }
    for sql in [
        "SELECT '__astersql_binary_hex__:7fc12345'::float8",
        "SELECT '__astersql_binary_hex__:00000000'::float8[]",
        "SELECT '{1,bad}'::float8[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
    }
    assert!(session.execute("SELECT NULL::float8[]::float8[]").is_ok());
    assert!(session.state.borrow().pg_bound_float4_constants.is_empty());
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_float8_sql_constructors_and_failure_restore() {
    use astersql_types::pg_array::PgArray;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={1.5,NULL}'::float8[]",
            "[-2:-1]={1.5,NULL}",
        ),
        ("SELECT ARRAY[]::double precision[]", "{}"),
        ("SELECT ARRAY[NULL,NULL]::float8[]", "{NULL,NULL}"),
        (
            "SELECT ARRAY[['-0'::float8,NULL],['NaN'::float8,'1.5'::float8]]",
            "{{-0,NULL},{NaN,1.5}}",
        ),
        ("SELECT '{NaN,-0}'::float8[]::float8[]", "{NaN,-0}"),
    ] {
        let mut results = session.execute(sql).unwrap();
        let row = results[0].next_row().unwrap().unwrap();
        let bytes = super::row_codec::binary_runtime_bytes(&row[0]).unwrap();
        assert_eq!(
            PgArray::float8_from_binary(&bytes)
                .unwrap()
                .to_text(b',')
                .unwrap(),
            expected,
            "{sql}"
        );
        results[0].close().unwrap();
    }
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ? AS v", &[1022])
        .unwrap();
    assert!(
        session
            .execute_protocol_statement(id, &[ConcretePreparedArgument::Float8Array(vec![0])])
            .is_err()
    );
    assert!(session.state.borrow().pg_bound_float4_constants.is_empty());
    assert!(session.execute("SELECT 1").is_ok());
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg05_array_float8_scalar_cast_preserves_typed_bits() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, raw) in [
        ("SELECT ?::float8 AS v", true),
        ("SELECT ?::text AS v", false),
        ("SELECT ?::float4 AS v", false),
    ] {
        let (id, _, _) = session
            .prepare_protocol_statement_with_parameter_types(sql, &[701])
            .unwrap();
        let mut result = session
            .execute_protocol_statement(
                id,
                &[ConcretePreparedArgument::Float8Bits(0xfff012345678abcd)],
            )
            .unwrap();
        let row = result[0].next_row().unwrap().unwrap();
        if raw {
            assert_eq!(
                super::row_codec::binary_runtime_bytes(&row[0]).unwrap(),
                0xfff012345678abcdu64.to_be_bytes()
            );
        } else {
            assert_eq!(row[0], "NaN");
        }
        result[0].close().unwrap();
        session.close_protocol_statement(id).unwrap();
    }

    use astersql_types::pg_array::{PgArray, PgArrayElement};
    let array = PgArray::new(
        701,
        -1,
        vec![2],
        vec![-1],
        vec![Some(PgArrayElement::Float8(0xfff012345678abcd)), None],
    )
    .unwrap();
    let (id, _, _) = session
        .prepare_protocol_statement_with_parameter_types("SELECT ?::text AS v", &[1022])
        .unwrap();
    let mut result = session
        .execute_protocol_statement(
            id,
            &[ConcretePreparedArgument::Float8Array(
                array.to_binary().unwrap(),
            )],
        )
        .unwrap();
    assert_eq!(
        result[0].next_row().unwrap().unwrap(),
        vec!["[-1:0]={NaN,NULL}".to_owned()]
    );
    result[0].close().unwrap();
    session.close_protocol_statement(id).unwrap();
}

#[test]
fn pg05_array_float8_cross_family_is_explicitly_unsupported() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT '{1}'::float8[]::float4[]",
        "SELECT '{1}'::float4[]::float8[]",
        "SELECT '{1}'::int4[]::float8[]",
    ] {
        let error = session.execute(sql).err().unwrap();
        assert_eq!(
            error.pg_array_error().map(|e| e.state),
            Some("0A000"),
            "{sql}: {error}"
        );
    }
}

#[test]
fn pg05_array_varchar_sql_modifiers_constructors_and_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={é😀x,NULL}'::varchar(2)[]",
            Some("[-2:-1]={é😀,NULL}"),
        ),
        (
            "SELECT ARRAY[['é😀x',NULL],['a  ','éx']]::varchar(2)[]",
            Some("{{é😀,NULL},{\"a \",é}}"),
        ),
        ("SELECT ARRAY[]::varchar(2)[]", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::varchar(2)[]", Some("{NULL,NULL}")),
        ("SELECT ARRAY['é'::varchar,'😀'::varchar]", Some("{é,😀}")),
        ("SELECT NULL::varchar(2)[]", None),
        (
            "SELECT '{abcdef}'::varchar(3)[]::varchar(2)[]",
            Some("{ab}"),
        ),
    ] {
        let mut results = session.execute(sql).unwrap();
        let row = results[0].next_row().unwrap().unwrap();
        assert_eq!(row[0], expected.unwrap_or("<nil>"), "{sql}");
        results[0].close().unwrap();
    }
    for n in [0, -2, 10485761] {
        for suffix in ["", " WHERE false"] {
            let sql = format!("SELECT NULL::varchar({n})[]{suffix}");
            assert!(session.execute(&sql).is_err(), "{sql}");
        }
    }
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_bpchar_sql_modifiers_constructors_and_null() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY['','a','é','😀']::char(2)[]",
            Some(r#"{"  ","a ","é ","😀 "}"#),
        ),
        ("SELECT '{\"\",abc}'::char[]", Some(r#"{" ",a}"#)),
        ("SELECT '{a}'::char(3)[]::char(2)[]", Some(r#"{"a "}"#)),
        (
            "SELECT '[-2:-1]={é😀x,NULL}'::char(2)[]",
            Some("[-2:-1]={é😀,NULL}"),
        ),
        (
            "SELECT ARRAY[['é😀x',NULL],['a  ','éx']]::char(2)[]",
            Some("{{é😀,NULL},{\"a \",é}}"),
        ),
        ("SELECT ARRAY[]::char(2)[]", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::char(2)[]", Some("{NULL,NULL}")),
        ("SELECT ARRAY['é'::bpchar,'😀'::bpchar]", Some("{é,😀}")),
        ("SELECT NULL::char(2)[]", None),
        ("SELECT '{abcdef}'::char(3)[]::char(2)[]", Some("{ab}")),
    ] {
        let mut results = session.execute(sql).unwrap();
        let row = results[0].next_row().unwrap().unwrap();
        assert_eq!(row[0], expected.unwrap_or("<nil>"), "{sql}");
        results[0].close().unwrap();
    }
    for n in [0, -2, 10485761] {
        for suffix in ["", " WHERE false"] {
            let sql = format!("SELECT NULL::char({n})[]{suffix}");
            assert!(session.execute(&sql).is_err(), "{sql}");
        }
    }
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_date_sql_constructors_null_and_boundaries() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={\"4714-11-24 BC\",5874897-12-31}'::date[]",
            Some("[-2:-1]={\"4714-11-24 BC\",5874897-12-31}"),
        ),
        (
            "SELECT ARRAY['2000-01-01'::date,NULL::date]",
            Some("{2000-01-01,NULL}"),
        ),
        (
            "SELECT ARRAY[['infinity',NULL],['-infinity','2000-01-01']]::date[]",
            Some("{{infinity,NULL},{-infinity,2000-01-01}}"),
        ),
        ("SELECT ARRAY[]::date[]", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::date[]", Some("{NULL,NULL}")),
        ("SELECT NULL::date[]", None),
    ] {
        let mut results = session.execute(sql).unwrap();
        let row = results[0].next_row().unwrap().unwrap();
        assert_eq!(row[0], expected.unwrap_or("<nil>"), "{sql}");
        results[0].close().unwrap();
    }
    for sql in [
        "SELECT '{2000-01-01,2023-02-29}'::date[]",
        "SELECT NULL::int4[]::date[]",
        "SELECT NULL::date[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
    }
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_time_sql_constructors_null_and_boundaries() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={24:00:00,00:00:00.000001}'::time[]",
            Some("[-2:-1]={24:00:00,00:00:00.000001}"),
        ),
        (
            "SELECT ARRAY['12:34:56.123456'::time,NULL::time]",
            Some("{12:34:56.123456,NULL}"),
        ),
        (
            "SELECT ARRAY[['23:59:59.999999',NULL],['00:00:00','12:34:56.123456']]::time[]",
            Some("{{23:59:59.999999,NULL},{00:00:00,12:34:56.123456}}"),
        ),
        (
            "SELECT '[-2:-1]={23:59:59.999999,NULL}'::time(0)[]",
            Some("[-2:-1]={24:00:00,NULL}"),
        ),
        (
            "SELECT ARRAY['23:59:59.999999',NULL]::time(3)[]",
            Some("{24:00:00,NULL}"),
        ),
        ("SELECT ARRAY[]::time[]", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::time[]", Some("{NULL,NULL}")),
        ("SELECT NULL::time[]", None),
    ] {
        let mut results = session.execute(sql).unwrap();
        let row = results[0].next_row().unwrap().unwrap();
        assert_eq!(row[0], expected.unwrap_or("<nil>"), "{sql}");
        results[0].close().unwrap();
    }
    for sql in [
        "SELECT '{12:34:56.123456,25:00:00}'::time[]",
        "SELECT NULL::int4[]::time[]",
        "SELECT NULL::time[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
    }
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_timestamp_sql_constructors_null_and_boundaries() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={infinity,-infinity}'::timestamp[]",
            Some("[-2:-1]={infinity,-infinity}"),
        ),
        (
            "SELECT ARRAY['0001-01-01 00:00:00 BC'::timestamp,NULL::timestamp]",
            Some(r#"{"0001-01-01 00:00:00 BC",NULL}"#),
        ),
        (
            "SELECT ARRAY['12000-01-01 00:00:00'::timestamp]",
            Some(r#"{"12000-01-01 00:00:00"}"#),
        ),
        (
            "SELECT '[-2:-1]={2000-01-01 23:59:59.999999,NULL}'::timestamp(0)[]",
            Some(r#"[-2:-1]={"2000-01-02 00:00:00",NULL}"#),
        ),
        (
            "SELECT ARRAY['1999-12-31 23:59:59.999999',NULL]::timestamp(3)[]",
            Some(r#"{"2000-01-01 00:00:00",NULL}"#),
        ),
        ("SELECT ARRAY[]::timestamp[]", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::timestamp[]", Some("{NULL,NULL}")),
        ("SELECT NULL::timestamp[]", None),
    ] {
        let mut results = session.execute(sql).unwrap();
        assert_eq!(
            results[0].next_row().unwrap().unwrap()[0],
            expected.unwrap_or("<nil>"),
            "{sql}"
        );
        results[0].close().unwrap();
    }
    for sql in [
        "SELECT '{2000-01-01 00:00:00,garbage}'::timestamp[]",
        "SELECT NULL::int4[]::timestamp[]",
        "SELECT NULL::timestamp[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
    }
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_timestamptz_sql_constructors_null_and_boundaries() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session.execute("SET time_zone = 'UTC'").unwrap();
    for (sql, expected) in [
        (
            "SELECT '[-2:-1]={infinity,-infinity}'::timestamptz[]",
            Some("[-2:-1]={infinity,-infinity}"),
        ),
        (
            "SELECT ARRAY['0001-01-01 00:00:00+00 BC'::timestamptz,NULL::timestamptz]",
            Some(r#"{"0001-01-01 00:00:00+00 BC",NULL}"#),
        ),
        (
            "SELECT ARRAY['12000-01-01 00:00:00+00'::timestamptz]",
            Some(r#"{"12000-01-01 00:00:00+00"}"#),
        ),
        (
            "SELECT '[-2:-1]={2000-01-01 23:59:59.999999+00,NULL}'::timestamptz(0)[]",
            Some(r#"[-2:-1]={"2000-01-02 00:00:00+00",NULL}"#),
        ),
        (
            "SELECT ARRAY['1999-12-31 23:59:59.999999+00',NULL]::timestamptz(3)[]",
            Some(r#"{"2000-01-01 00:00:00+00",NULL}"#),
        ),
        ("SELECT ARRAY[]::timestamptz[]", Some("{}")),
        (
            "SELECT ARRAY[NULL,NULL]::timestamptz[]",
            Some("{NULL,NULL}"),
        ),
        ("SELECT NULL::timestamptz[]", None),
    ] {
        let mut results = session.execute(sql).unwrap();
        assert_eq!(
            results[0].next_row().unwrap().unwrap()[0],
            expected.unwrap_or("<nil>"),
            "{sql}"
        );
        results[0].close().unwrap();
    }
    for sql in [
        "SELECT '{2000-01-01 00:00:00+00,garbage}'::timestamptz[]",
        "SELECT NULL::int4[]::timestamptz[]",
        "SELECT NULL::timestamptz[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
    }
    assert!(session.execute("SELECT 1").is_ok());
}

#[test]
fn pg05_array_timestamptz_session_timezone_and_constant_union() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    session
        .execute("SET time_zone = 'America/New_York'")
        .unwrap();
    for sql in [
        "SELECT ARRAY['2024-03-10 02:30:00'::timestamptz,NULL::timestamptz]",
        "SELECT '{2024-03-10 02:30:00,NULL}'::timestamptz[]",
        "SELECT '{2024-03-10 02:30:00,NULL}'::timestamptz[] UNION ALL SELECT '{2024-03-10 02:30:00,NULL}'::timestamptz[]",
    ] {
        let mut results = session.execute(sql).unwrap();
        let expected = r#"{"2024-03-10 03:30:00-04",NULL}"#;
        assert_eq!(
            results[0].next_row().unwrap().unwrap()[0],
            expected,
            "{sql}"
        );
        if sql.contains("UNION") {
            assert_eq!(results[0].next_row().unwrap().unwrap()[0], expected);
        }
        results[0].close().unwrap();
    }
}

#[test]
fn pg05_array_interval_runtime_constructors_typmod_null_and_atomic_errors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY['1 mon'::interval,'24 hours'::interval,NULL::interval]",
            r#"{"1 mon",24:00:00,NULL}"#,
        ),
        (
            "SELECT '[-2:-1]={1.2345 seconds,NULL}'::interval(3)[]",
            "[-2:-1]={00:00:01.235,NULL}",
        ),
        (
            "SELECT '{1 year 2 months}'::interval year[]",
            r#"{"1 year"}"#,
        ),
        ("SELECT ARRAY[]::interval[]", "{}"),
        ("SELECT ARRAY[NULL,NULL]::interval[]", "{NULL,NULL}"),
        ("SELECT NULL::interval(3)[]", "<nil>"),
        (
            "SELECT '{infinity,-infinity}'::interval[]",
            "{infinity,-infinity}",
        ),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap()[0], expected, "{sql}");
        result[0].close().unwrap();
    }
    for sql in [
        "SELECT '{1 month,garbage}'::interval[]",
        "SELECT NULL::int4[]::interval[]",
        "SELECT NULL::interval[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
        assert!(session.execute("SELECT 1").is_ok());
    }
}

#[test]
fn pg05_array_uuid_runtime_identity_null_and_atomic_errors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        (
            "SELECT ARRAY['00112233-4455-6677-8899-aabbccddeeff'::uuid,NULL::uuid]",
            "{00112233-4455-6677-8899-aabbccddeeff,NULL}",
        ),
        (
            "SELECT '[-2:-1]={00112233445566778899AABBCCDDEEFF,NULL}'::uuid[]",
            "[-2:-1]={00112233-4455-6677-8899-aabbccddeeff,NULL}",
        ),
        ("SELECT ARRAY[]::uuid[]", "{}"),
        ("SELECT ARRAY[NULL,NULL]::uuid[]", "{NULL,NULL}"),
        ("SELECT NULL::uuid[]", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap()[0], expected, "{sql}");
        result[0].close().unwrap();
    }
    for sql in [
        "SELECT '{00000000-0000-0000-0000-000000000000,bad}'::uuid[]",
        "SELECT NULL::int4[]::uuid[]",
        "SELECT NULL::uuid[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
        assert!(session.execute("SELECT 1").is_ok());
    }
}

#[test]
fn pg05_array_json_runtime_identity_null_and_atomic_errors() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session = ConcreteSession::new_with_dialect(domain, ast::SqlDialect::PostgreSQL);
    for (sql, expected) in [
        ("SELECT ARRAY['null'::json,NULL::json]", r#"{"null",NULL}"#),
        (
            "SELECT '[-2:-1]={1e100000,NULL}'::json[]",
            "[-2:-1]={1e100000,NULL}",
        ),
        ("SELECT ARRAY[]::json[]", "{}"),
        ("SELECT ARRAY[NULL,NULL]::json[]", "{NULL,NULL}"),
        ("SELECT NULL::json[]", "<nil>"),
    ] {
        let mut result = session.execute(sql).unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap()[0], expected, "{sql}");
        result[0].close().unwrap();
    }
    for sql in [
        "SELECT '{null,bad}'::json[]",
        "SELECT NULL::int4[]::json[]",
        "SELECT NULL::json[]::text[]",
    ] {
        assert!(session.execute(sql).is_err(), "{sql}");
        assert!(session.execute("SELECT 1").is_ok());
    }
}
