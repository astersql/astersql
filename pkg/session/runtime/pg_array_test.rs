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
        "0A000"
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
