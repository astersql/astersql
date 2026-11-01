// Copyright 2026 AsterSQL.

use super::row_codec::insert_default_runtime_value;

#[test]
fn timestamp_default_accepts_ast_restored_fractional_precision() {
    for expression in [
        "CURRENT_TIMESTAMP(6)",
        "CURRENT_TIMESTAMP('6')",
        "CURRENT_TIMESTAMP('6').000000",
    ] {
        let mut column = astersql_meta_model::ColumnInfo::default();
        column.SetType(astersql_parser_mysql::r#type::TypeTimestamp);
        column.SetDecimal(6);
        column
            .SetDefaultValue(Some(astersql_meta_model::DefaultValue::String(
                expression.as_bytes().to_vec(),
            )))
            .unwrap();
        let value = insert_default_runtime_value(&column).unwrap();
        assert_eq!(value.len(), 26);
        assert!(chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.6f").is_ok());
    }
}

#[test]
fn uuid_expression_default_is_evaluated_per_row() {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column
        .SetDefaultValue(Some(astersql_meta_model::DefaultValue::String(
            b"UUID()".to_vec(),
        )))
        .unwrap();
    column.DefaultIsExpr = true;

    let first = insert_default_runtime_value(&column).unwrap();
    let second = insert_default_runtime_value(&column).unwrap();

    assert_eq!(uuid::Uuid::parse_str(&first).unwrap().get_version_num(), 4);
    assert_eq!(uuid::Uuid::parse_str(&second).unwrap().get_version_num(), 4);
    assert_ne!(first, second);
}

#[test]
fn decimal_index_cursor_preserves_declared_precision_and_scale() {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column.SetType(astersql_parser_mysql::r#type::TypeNewDecimal);
    column.SetDecimal(2);
    for (precision, expected) in [
        (10, vec![6, 10, 2, 128, 0, 0, 12, 30]),
        (5, vec![6, 5, 2, 128, 12, 30]),
    ] {
        column.SetFlen(precision);
        for input in ["12.30", "12.3"] {
            let datum = super::row_codec::runtime_value_to_datum(
                Some(&input.to_owned()),
                &column,
                astersql_types::Flags::default(),
            )
            .unwrap();
            let key = astersql_tablecodec::codec::NewEncoder(false)
                .EncodeKey(astersql_tablecodec::time::UTC, Vec::new(), vec![datum])
                .unwrap();
            // Match the declared DECIMAL field metadata used by Go's row
            // decoder and MODIFY COLUMN backfill, independently of spelling.
            assert_eq!(key, expected, "DECIMAL({precision},2): {input}");
        }
    }
}

#[test]
fn reorg_global_index_copy_mutations_stop_at_delete_only() {
    use astersql_meta_model::{
        ColumnInfo, IndexColumn, IndexInfo, PartitionInfo, SchemaState as S, TableInfo,
    };
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("b");
    column.SetType(astersql_parser_mysql::r#type::TypeLonglong);
    let row = std::collections::HashMap::from([
        ("b".to_owned(), Some("10".to_owned())),
        ("_tidb_rowid".to_owned(), Some("1".to_owned())),
    ]);
    for unique in [false, true] {
        for version in [0, astersql_meta_model::GlobalIndexVersionV1] {
            for state in [S::WriteOnly, S::DeleteOnly] {
                let mut pi = PartitionInfo::default();
                pi.DDLChangedIndex.insert(7, true);
                let table = TableInfo {
                    ID: 100,
                    Columns: vec![column.clone()],
                    Partition: Some(pi),
                    Indices: vec![IndexInfo {
                        ID: 7,
                        Name: astersql_parser_ast::NewCIStr("same_name"),
                        Global: true,
                        Unique: unique,
                        GlobalIndexVersion: version,
                        State: state,
                        Columns: vec![IndexColumn {
                            Name: column.Name.clone(),
                            Length: -1,
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                let inserted = super::row_codec::relational_index_mutations(
                    &table,
                    None,
                    Some(&row),
                    astersql_types::Flags::default(),
                )
                .unwrap();
                assert_eq!(inserted.len(), usize::from(state == S::WriteOnly));
                let deleted = super::row_codec::relational_index_mutations(
                    &table,
                    Some(&row),
                    None,
                    astersql_types::Flags::default(),
                )
                .unwrap();
                assert_eq!(deleted.len(), 1);
                assert!(deleted[0].1.is_none());
            }
        }
    }
}

#[test]
fn pg_scalar_15_numeric_row_formats_and_legacy_metadata() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    for pg_numeric in [false, true] {
        for new_format in [false, true] {
            let mut column = ColumnInfo::default();
            column.ID = 1;
            column.Name = astersql_parser_ast::NewCIStr("n");
            column.State = SchemaState::Public;
            column.SetType(astersql_parser_mysql::r#type::TypeNewDecimal);
            let mut table = TableInfo {
                ID: 9815,
                ExactName: true,
                PostgreSQLNumericText: pg_numeric,
                Columns: vec![column],
                ..Default::default()
            };
            let encoded_metadata = serde_json::to_vec(&table).unwrap();
            table = serde_json::from_slice(&encoded_metadata).unwrap();
            assert_eq!(table.PostgreSQLNumericText, pg_numeric);
            if !pg_numeric {
                assert!(
                    !String::from_utf8(encoded_metadata)
                        .unwrap()
                        .contains("postgresql_numeric_text")
                );
            }
            for (precision, scale, input, expected) in [
                (5, 2, Some("12.345".to_owned()), Some("12.35".to_owned())),
                (2, -3, Some("12500".to_owned()), Some("13000".to_owned())),
                (
                    3,
                    5,
                    Some("0.001234".to_owned()),
                    Some("0.00123".to_owned()),
                ),
                (-1, -1, Some("9".repeat(1000)), Some("9".repeat(1000))),
                (-1, -1, Some("NaN".to_owned()), Some("NaN".to_owned())),
                (
                    -1,
                    -1,
                    Some("Infinity".to_owned()),
                    Some("Infinity".to_owned()),
                ),
                (5, 2, None, None),
            ] {
                if !pg_numeric && (precision != 5 || scale != 2) {
                    continue;
                }
                table.Columns[0].SetFlen(precision);
                table.Columns[0].SetDecimal(scale);
                let row = HashMap::from([
                    ("n".to_owned(), input),
                    ("_tidb_rowid".to_owned(), Some("1".to_owned())),
                ]);
                let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                    &table,
                    &row,
                    astersql_types::Flags::default(),
                    new_format,
                )
                .unwrap();
                let fields = table
                    .Columns
                    .iter()
                    .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                    .collect();
                let handle = astersql_tablecodec::kv::IntHandle(1);
                let (_, decoded) = super::relational_scan::decode_relational_row_value(
                    &table, &fields, &handle, &bytes,
                )
                .unwrap_or_else(|error| panic!("PG={pg_numeric} new_format={new_format} precision={precision} scale={scale}: {error}"));
                assert_eq!(
                    decoded.get("n").cloned().flatten(),
                    expected,
                    "PG={pg_numeric} new_format={new_format} precision={precision} scale={scale}"
                );
            }
        }
    }
}

#[test]
fn pg_scalar_20_float_signed_zero_row_formats() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    for ty in [4, 5] {
        for new_format in [false, true] {
            let mut column = ColumnInfo::default();
            column.ID = 1;
            column.Name = astersql_parser_ast::NewCIStr("f");
            column.State = SchemaState::Public;
            column.SetType(ty);
            let table = TableInfo {
                ID: 9820,
                PostgreSQLFloatText: true,
                Columns: vec![column],
                ..Default::default()
            };
            let table: TableInfo =
                serde_json::from_slice(&serde_json::to_vec(&table).unwrap()).unwrap();
            assert!(table.PostgreSQLFloatText);
            assert!(!TableInfo::default().PostgreSQLFloatText);
            for input in [
                Some("-0".to_owned()),
                Some("1.5".to_owned()),
                Some("NaN".to_owned()),
                None,
            ] {
                let row = HashMap::from([
                    ("f".to_owned(), input.clone()),
                    ("_tidb_rowid".to_owned(), Some("1".to_owned())),
                ]);
                let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                    &table,
                    &row,
                    astersql_types::Flags::default(),
                    new_format,
                )
                .unwrap();
                let fields = table
                    .Columns
                    .iter()
                    .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                    .collect();
                let handle = astersql_tablecodec::kv::IntHandle(1);
                let (_, decoded) = super::relational_scan::decode_relational_row_value(
                    &table, &fields, &handle, &bytes,
                )
                .unwrap();
                assert_eq!(
                    decoded.get("f").cloned().flatten(),
                    input,
                    "type={ty} new_format={new_format}"
                );
            }
        }
    }
}

#[test]
fn pg06_date_timestamp_row_formats() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("v");
    column.State = SchemaState::Public;
    column.SetType(astersql_parser_mysql::r#type::TypeDatetime);
    column.SetDecimal(-1);
    let table = TableInfo {
        ID: 9870,
        ExactName: true,
        Columns: vec![column],
        ..Default::default()
    };
    for new_format in [false, true] {
        for input in [
            Some("2000-01-01 00:00:00"),
            Some("4713-11-24 00:00:00 BC"),
            Some("294276-12-31 00:00:00"),
            Some("infinity"),
            Some("-infinity"),
            None,
        ] {
            let row = HashMap::from([
                ("v".to_owned(), input.map(str::to_owned)),
                ("_tidb_rowid".to_owned(), Some("1".to_owned())),
            ]);
            let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                &table,
                &row,
                astersql_types::Flags::default(),
                new_format,
            )
            .unwrap();
            let fields = table
                .Columns
                .iter()
                .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                .collect();
            let handle = astersql_tablecodec::kv::IntHandle(1);
            let (_, decoded) = super::relational_scan::decode_relational_row_value(
                &table, &fields, &handle, &bytes,
            )
            .unwrap();
            assert_eq!(
                decoded.get("v").cloned().flatten(),
                input.map(str::to_owned),
                "format={new_format}"
            );
        }
    }
}

#[test]
fn pg06_date_timestamptz_row_formats() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("v");
    column.State = SchemaState::Public;
    column.SetType(astersql_parser_mysql::r#type::TypePgTimestampTz);
    column.SetDecimal(-1);
    let table = TableInfo {
        ID: 9871,
        ExactName: false,
        Columns: vec![column],
        ..Default::default()
    };
    for new_format in [false, true] {
        for input in [
            Some("2000-01-01 00:00:00+00"),
            Some("4713-11-24 00:00:00+00 BC"),
            Some("294276-12-31 00:00:00+00"),
            Some("infinity"),
            Some("-infinity"),
            None,
        ] {
            let row = HashMap::from([
                ("v".to_owned(), input.map(str::to_owned)),
                ("_tidb_rowid".to_owned(), Some("1".to_owned())),
            ]);
            let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                &table,
                &row,
                astersql_types::Flags::default(),
                new_format,
            )
            .unwrap();
            let fields = table
                .Columns
                .iter()
                .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                .collect();
            let handle = astersql_tablecodec::kv::IntHandle(1);
            let (_, decoded) = super::relational_scan::decode_relational_row_value(
                &table, &fields, &handle, &bytes,
            )
            .unwrap();
            assert_eq!(
                decoded.get("v").cloned().flatten(),
                input.map(str::to_owned),
                "format={new_format}"
            );
        }
    }
}

#[test]
fn pg06_task_078_interval_row_formats() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("v");
    column.State = SchemaState::Public;
    column.SetType(astersql_parser_mysql::r#type::TypePgInterval);
    column.SetDecimal(-1);
    column.SetFlen(-1);
    let table = TableInfo {
        ID: 9871,
        ExactName: false,
        Columns: vec![column],
        ..Default::default()
    };
    for new_format in [false, true] {
        for input in [
            Some("00:00:00"),
            Some("12:34:56.123456"),
            Some("24:00:00"),
            Some("1 mon 2 days 24:00:00"),
            None,
        ] {
            let row = HashMap::from([
                ("v".to_owned(), input.map(str::to_owned)),
                ("_tidb_rowid".to_owned(), Some("1".to_owned())),
            ]);
            let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                &table,
                &row,
                astersql_types::Flags::default(),
                new_format,
            )
            .unwrap();
            let fields = table
                .Columns
                .iter()
                .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                .collect();
            let handle = astersql_tablecodec::kv::IntHandle(1);
            let (_, decoded) = super::relational_scan::decode_relational_row_value(
                &table, &fields, &handle, &bytes,
            )
            .unwrap();
            assert_eq!(
                decoded.get("v").cloned().flatten(),
                input.map(str::to_owned),
                "format={new_format}"
            );
        }
    }
}

#[test]
fn pg06_task_080_jsonb_row_formats() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("v");
    column.State = SchemaState::Public;
    column.SetType(astersql_parser_mysql::r#type::TypePgJsonb);
    column.SetDecimal(-1);
    column.SetFlen(-1);
    let table = TableInfo {
        ID: 9871,
        ExactName: false,
        Columns: vec![column],
        ..Default::default()
    };
    for input in [r#""\u0000""#, r#""\ud800""#, "1e200000"] {
        let value = input.to_owned();
        assert!(
            super::row_codec::runtime_value_to_datum(
                Some(&value),
                &table.Columns[0],
                astersql_types::Flags::default()
            )
            .is_err()
        );
    }
    for new_format in [false, true] {
        for (input, expected) in [
            (
                Some(r#" {"aa":1,"b":2,"aa":3.00} "#),
                Some(r#"{"b": 2, "aa": 3.00}"#),
            ),
            (Some("null"), Some("null")),
            (None, None),
        ] {
            let row = HashMap::from([
                ("v".to_owned(), input.map(str::to_owned)),
                ("_tidb_rowid".to_owned(), Some("1".to_owned())),
            ]);
            let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                &table,
                &row,
                astersql_types::Flags::default(),
                new_format,
            )
            .unwrap();
            let fields = table
                .Columns
                .iter()
                .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                .collect();
            let handle = astersql_tablecodec::kv::IntHandle(1);
            let (_, decoded) = super::relational_scan::decode_relational_row_value(
                &table, &fields, &handle, &bytes,
            )
            .unwrap();
            assert_eq!(
                decoded.get("v").cloned().flatten(),
                expected.map(str::to_owned),
                "format={new_format}"
            );
        }
    }
}

#[test]
fn pg06_task_081_json_row_formats() {
    use astersql_meta_model::{ColumnInfo, SchemaState, TableInfo};
    use std::collections::HashMap;
    let mut column = ColumnInfo::default();
    column.ID = 1;
    column.Name = astersql_parser_ast::NewCIStr("v");
    column.State = SchemaState::Public;
    column.SetType(astersql_parser_mysql::r#type::TypePgJson);
    column.SetDecimal(-1);
    column.SetFlen(-1);
    let table = TableInfo {
        ID: 9871,
        ExactName: false,
        Columns: vec![column],
        ..Default::default()
    };
    for input in ["garbage", "{", "null trailing"] {
        let value = input.to_owned();
        assert!(
            super::row_codec::runtime_value_to_datum(
                Some(&value),
                &table.Columns[0],
                astersql_types::Flags::default()
            )
            .is_err()
        );
    }
    for new_format in [false, true] {
        for (input, expected) in [
            (
                Some(r#" {"aa":1,"b":2,"aa":3.00} "#),
                Some(r#" {"aa":1,"b":2,"aa":3.00} "#),
            ),
            (Some("null"), Some("null")),
            (None, None),
        ] {
            let row = HashMap::from([
                ("v".to_owned(), input.map(str::to_owned)),
                ("_tidb_rowid".to_owned(), Some("1".to_owned())),
            ]);
            let (_, bytes) = super::row_codec::encode_relational_row_with_format(
                &table,
                &row,
                astersql_types::Flags::default(),
                new_format,
            )
            .unwrap();
            let fields = table
                .Columns
                .iter()
                .map(|column| (column.ID, Box::new(column.FieldType.clone())))
                .collect();
            let handle = astersql_tablecodec::kv::IntHandle(1);
            let (_, decoded) = super::relational_scan::decode_relational_row_value(
                &table, &fields, &handle, &bytes,
            )
            .unwrap();
            assert_eq!(
                decoded.get("v").cloned().flatten(),
                expected.map(str::to_owned),
                "format={new_format}"
            );
        }
    }
}

#[test]
fn port_4150cef7a2_binary_expression_defaults_sql() {
    use super::mview_refresh_test::query;
    let f = super::normal_ddl_fixture::Fixture::new();
    super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
    super::EnableMockDDLForTest(&f.domain).unwrap();
    let session = super::ConcreteSession::new(f.domain.clone());
    query(&session, "use test");
    query(
        &session,
        "create table binary_defaults (id int primary key, u binary(50) default (uuid()), b binary(50) default (uuid_to_bin(uuid())), v varbinary(50) default (uuid()), literal binary(4) default 'a', nullable binary(4) default null)",
    );
    query(&session, "insert into binary_defaults(id) values (1), (2)");
    assert_eq!(
        query(
            &session,
            "select length(u), length(b), length(v), hex(literal), nullable is null, hex(substr(u,37)) = repeat('00',14), hex(substr(b,17)) = repeat('00',34) from binary_defaults order by id"
        ),
        vec![vec!["50", "50", "36", "61000000", "1", "1", "1"]; 2]
    );
    assert_eq!(
        query(
            &session,
            "select count(distinct u), count(distinct b) from binary_defaults"
        ),
        vec![vec!["2", "2"]]
    );
    let before = f.domain.table_by_name("test", "binary_defaults").unwrap();
    query(
        &session,
        "alter table binary_defaults alter column u set default (uuid())",
    );
    let after = f.domain.table_by_name("test", "binary_defaults").unwrap();
    // SET DEFAULT must preserve all existing column attributes and row defaults.
    assert_eq!(
        serde_json::to_value(&before.Columns[1]).unwrap(),
        serde_json::to_value(&after.Columns[1]).unwrap()
    );

    query(&session, "insert into binary_defaults(id) values (3)");
    assert_eq!(
        query(
            &session,
            "select length(u), hex(substr(u,37)) = repeat('00',14) from binary_defaults where id = 3"
        ),
        vec![vec!["50", "1"]]
    );
    query(
        &session,
        "alter table binary_defaults modify column u binary(60) default (uuid())",
    );
    query(&session, "insert into binary_defaults(id) values (4)");
    assert_eq!(
        query(
            &session,
            "select length(u), hex(substr(u,37)) = repeat('00',24) from binary_defaults where id = 4"
        ),
        vec![vec!["60", "1"]]
    );
    query(&session, "drop table binary_defaults");
}

#[test]
fn port_87ffb1fda6_reverse_expression_sort_nullable_column() {
    use super::mview_refresh_test::query;
    let f = super::normal_ddl_fixture::Fixture::new();
    super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
    super::EnableMockDDLForTest(&f.domain).unwrap();
    let session = super::ConcreteSession::new(f.domain.clone());
    query(&session, "use test");
    query(
        &session,
        "create table reverse_sort (id int primary key, c double, key i(c))",
    );
    query(
        &session,
        "insert into reverse_sort values (1,null),(2,0.3),(3,0.7),(4,0.8)",
    );
    for expr in ["-c", "0-c", "-(c+1)"] {
        assert_eq!(
            query(
                &session,
                &format!("select id from reverse_sort use index(i) order by {expr} desc,id")
            ),
            vec![vec!["2"], vec!["3"], vec!["4"], vec!["1"]]
        );
        assert_eq!(
            query(
                &session,
                &format!("select id from reverse_sort use index(i) order by {expr},id")
            ),
            vec![vec!["1"], vec!["4"], vec!["3"], vec!["2"]]
        );
    }
    assert_eq!(
        query(
            &session,
            "select id from reverse_sort order by -(-c) desc,id"
        ),
        vec![vec!["4"], vec!["3"], vec!["2"], vec!["1"]]
    );
    query(&session, "delete from reverse_sort where c is null");
    query(
        &session,
        "alter table reverse_sort modify c double not null",
    );
    assert_eq!(
        query(&session, "select id from reverse_sort order by -c desc,id"),
        vec![vec!["2"], vec!["3"], vec!["4"]]
    );
    let plan = query(
        &session,
        "explain format='brief' select id from reverse_sort use index(i) order by -c desc,id",
    );
    assert!(!format!("{plan:?}").contains("Sort"), "{plan:?}");
}

#[test]
fn port_87ffb1fda6_reverse_join_golden_results() {
    use super::mview_refresh_test::query;
    let f = super::normal_ddl_fixture::Fixture::new();
    super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
    super::EnableMockDDLForTest(&f.domain).unwrap();
    let session = super::ConcreteSession::new(f.domain.clone());
    query(&session, "use test");
    query(
        &session,
        "create table t (a int, b int, index idx_a(a), index idx_b(b))",
    );
    query(&session, "insert into t values (1,1),(1,2),(2,4),(3,5)");
    for expr in ["-t1.a", "-t1.a+3", "1-t1.a", "1-t1.a+3"] {
        let sql = format!(
            "select /*+ TIDB_SMJ(t1,t2) */ t1.a from t t1, t t2 where t1.a=t2.b order by {expr}"
        );
        assert_eq!(
            query(&session, &sql),
            vec![vec!["2"], vec!["1"], vec!["1"]],
            "{sql}"
        );
    }
}
