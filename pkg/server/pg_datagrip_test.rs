// Copyright 2026 AsterSQL.
use crate::conn::{CancellationToken, SessionDriver, TiDBContext, Value};
use crate::pg_catalog::CatalogQuery;
use crate::runtime::{BootstrapAuthMode, ConcreteSessionDriver};

fn context() -> std::sync::Arc<dyn TiDBContext> {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(93001, 0, 45, "test", None).unwrap();
    context
}
fn execute(context: &dyn TiDBContext, sql: &str) -> Vec<Vec<Value>> {
    CatalogQuery::parse(sql)
        .unwrap_or_else(|error| panic!("{sql}\n{error:?}"))
        .unwrap()
        .execute(context, &CancellationToken::new())
        .unwrap()
        .rows
}
#[test]
fn pg_datagrip_tables() {
    let context = context();
    context
        .execute_query(
            "CREATE TABLE test.dg_tables (id INT PRIMARY KEY, note VARCHAR(30))",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let namespace = execute(
        context.as_ref(),
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    )[0][0]
        .clone();
    let Value::Signed(namespace) = namespace else {
        panic!("namespace OID")
    };
    let sql =
        include_str!("testdata/pg_datagrip/1869280142.sql").replace('?', &namespace.to_string());
    let rows = execute(context.as_ref(), &sql);
    let table = rows
        .iter()
        .find(|row| row[1] == Value::Text("dg_tables".into()))
        .expect("real table visible");
    assert_eq!(table.len(), 15);
    assert_eq!(table[0], Value::Text("r".into()));
    assert_eq!(table[8], Value::Null);
    assert_eq!(table[9], Value::Null);
    context
        .execute_query(
            "CREATE TABLE test.dg_tables_second (id INT)",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let aggregate = execute(
        context.as_ref(),
        "SELECT array_agg(oid::bigint ORDER BY relname DESC)::varchar FROM pg_class WHERE relname IN ('dg_tables','dg_tables_second')",
    );
    let ids = execute(
        context.as_ref(),
        "SELECT oid FROM pg_class WHERE relname IN ('dg_tables','dg_tables_second') ORDER BY relname DESC",
    );
    let expected = format!(
        "{{{},{}}}",
        match ids[0][0] {
            Value::Signed(n) => n,
            _ => panic!(),
        },
        match ids[1][0] {
            Value::Signed(n) => n,
            _ => panic!(),
        }
    );
    assert_eq!(aggregate, vec![vec![Value::Text(expected)]]);
    // Empty pg_inherits alone cannot prove the outer table identity is bound.
    // Exercise the same correlated scalar/aggregate shape with real pg_class rows.
    let correlated = execute(
        context.as_ref(),
        "SELECT T.oid, (SELECT C.oid FROM pg_class C WHERE C.oid=T.oid), (SELECT array_agg(C.oid::bigint ORDER BY C.oid)::varchar FROM pg_class C WHERE C.oid=T.oid) FROM pg_class T WHERE T.relname IN ('dg_tables','dg_tables_second') ORDER BY T.relname DESC",
    );
    assert_eq!(correlated.len(), 2);
    for (row, id) in correlated.iter().zip(&ids) {
        assert_eq!(row[0], id[0]);
        assert_eq!(row[1], id[0]);
        let Value::Signed(oid) = id[0] else {
            panic!("table OID")
        };
        assert_eq!(row[2], Value::Text(format!("{{{oid}}}")));
    }
    let empty = execute(
        context.as_ref(),
        "SELECT (SELECT C.oid FROM pg_class C WHERE C.oid=0), (SELECT array_agg(C.oid::bigint ORDER BY C.oid)::varchar FROM pg_class C WHERE C.oid=0) FROM pg_namespace WHERE nspname='public'",
    );
    assert_eq!(empty, vec![vec![Value::Null, Value::Null]]);
    context
        .execute_query(
            "RENAME TABLE test.dg_tables TO test.dg_tables_renamed",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let renamed = execute(context.as_ref(), &sql);
    assert!(
        !renamed
            .iter()
            .any(|row| row[1] == Value::Text("dg_tables".into()))
    );
    assert!(
        renamed
            .iter()
            .any(|row| row[1] == Value::Text("dg_tables_renamed".into()))
    );
    context
        .execute_query(
            "DROP TABLE test.dg_tables_renamed",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        !execute(context.as_ref(), &sql)
            .iter()
            .any(|row| row[1] == Value::Text("dg_tables_renamed".into()))
    );
    context.close().unwrap();
}

#[test]
fn pg_datagrip_column_lifecycle() {
    let context = context();
    context
        .execute_query(
            "CREATE TABLE test.dg_column_lifecycle (id INT PRIMARY KEY, retired VARCHAR(12), UNIQUE KEY retired_unique (retired))",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let Value::Signed(namespace) = execute(
        context.as_ref(),
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    )[0][0] else {
        panic!("namespace OID")
    };
    let tables =
        include_str!("testdata/pg_datagrip/1869280142.sql").replace('?', &namespace.to_string());
    let columns = include_str!("testdata/pg_datagrip/RetrieveColumns.sql")
        .replace('?', &namespace.to_string());
    let indices =
        include_str!("testdata/pg_datagrip/1869280153.sql").replace('?', &namespace.to_string());
    let constraints =
        include_str!("testdata/pg_datagrip/1869280154.sql").replace('?', &namespace.to_string());

    let table_id = execute(context.as_ref(), &tables)
        .into_iter()
        .find(|row| row[1] == Value::Text("dg_column_lifecycle".into()))
        .expect("created table visible to DataGrip")[2]
        .clone();
    let lifecycle_columns = || {
        execute(context.as_ref(), &columns)
            .into_iter()
            .filter(|row| row[0] == table_id)
            .collect::<Vec<_>>()
    };
    let assert_table_id_stable = || {
        let row = execute(context.as_ref(), &tables)
            .into_iter()
            .find(|row| row[1] == Value::Text("dg_column_lifecycle".into()))
            .expect("table remains visible to DataGrip");
        assert_eq!(row[2], table_id);
    };

    let created = lifecycle_columns();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0][1], Value::Signed(1));
    assert_eq!(created[0][2], Value::Text("id".into()));
    assert_eq!(created[1][1], Value::Signed(2));
    assert_eq!(created[1][2], Value::Text("retired".into()));
    assert_eq!(created[1][6], Value::Text("character varying(12)".into()));

    context
        .execute_query(
            "ALTER TABLE test.dg_column_lifecycle ADD COLUMN note VARCHAR(20) NULL DEFAULT 'draft'",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_table_id_stable();
    let added = lifecycle_columns();
    assert_eq!(added.len(), 3);
    assert_eq!(added[2][1], Value::Signed(3));
    assert_eq!(added[2][2], Value::Text("note".into()));
    assert_eq!(added[2][6], Value::Text("character varying(20)".into()));
    assert_eq!(added[2][8], Value::Text("false".into()));
    assert_eq!(
        added[2][9],
        Value::Text("'draft'::character varying".into())
    );

    context
        .execute_query(
            "ALTER TABLE test.dg_column_lifecycle RENAME COLUMN note TO summary",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_table_id_stable();
    let renamed = lifecycle_columns();
    assert_eq!(renamed.len(), 3);
    assert!(
        !renamed
            .iter()
            .any(|row| row[2] == Value::Text("note".into()))
    );
    assert_eq!(renamed[2][1], Value::Signed(3));
    assert_eq!(renamed[2][2], Value::Text("summary".into()));

    context
        .execute_query(
            "ALTER TABLE test.dg_column_lifecycle MODIFY COLUMN summary VARCHAR(64) NOT NULL DEFAULT 'ready'",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_table_id_stable();
    let modified = lifecycle_columns();
    assert_eq!(modified.len(), 3);
    assert_eq!(modified[2][1], Value::Signed(3));
    assert_eq!(modified[2][2], Value::Text("summary".into()));
    assert_eq!(modified[2][6], Value::Text("character varying(64)".into()));
    assert_eq!(modified[2][8], Value::Text("true".into()));
    assert_eq!(
        modified[2][9],
        Value::Text("'ready'::character varying".into())
    );

    context
        .execute_query(
            "ALTER TABLE test.dg_column_lifecycle DROP COLUMN retired",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_table_id_stable();
    let dropped = lifecycle_columns();
    assert_eq!(dropped.len(), 2);
    assert_eq!(dropped[0][1], Value::Signed(1));
    assert_eq!(dropped[0][2], Value::Text("id".into()));
    assert_eq!(dropped[1][1], Value::Signed(2));
    assert_eq!(dropped[1][2], Value::Text("summary".into()));
    assert!(
        !dropped
            .iter()
            .any(|row| row[2] == Value::Text("retired".into()))
    );

    let index_rows = execute(context.as_ref(), &indices);
    assert!(
        index_rows
            .iter()
            .all(|row| row[0] != table_id || row[2] != Value::Text("retired_unique".into()))
    );
    let constraint_rows = execute(context.as_ref(), &constraints);
    assert!(
        constraint_rows
            .iter()
            .all(|row| row[0] != table_id || row[4] != Value::Text("retired_unique".into()))
    );
    context.close().unwrap();
}

#[test]
fn pg_datagrip_structure() {
    let context = context();
    context.execute_query("CREATE TABLE test.dg_structure (id INT PRIMARY KEY, note VARCHAR(30), UNIQUE KEY note_unique (note, id))", false, &CancellationToken::new()).unwrap();
    let Value::Signed(namespace) = execute(
        context.as_ref(),
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    )[0][0] else {
        panic!()
    };
    let indices =
        include_str!("testdata/pg_datagrip/1869280153.sql").replace('?', &namespace.to_string());
    let constraints =
        include_str!("testdata/pg_datagrip/1869280154.sql").replace('?', &namespace.to_string());
    let index_rows = execute(context.as_ref(), &indices);
    assert_eq!(index_rows.len(), 2);
    let constraints_rows = execute(context.as_ref(), &constraints);
    assert_eq!(constraints_rows.len(), 2);
    let Value::Signed(table_id) = execute(
        context.as_ref(),
        "SELECT oid FROM pg_class WHERE relname='dg_structure'",
    )[0][0] else {
        panic!("table OID")
    };
    for row in &index_rows {
        assert_eq!(row[0], Value::Signed(table_id));
        assert_eq!(row[11], Value::Null); // Native indexes have no PG operator class.
    }
    let unique = constraints_rows
        .iter()
        .find(|row| row[4] == Value::Text("note_unique".into()))
        .unwrap();
    assert_eq!(unique[6], Value::Text("{2,1}".into()));
    let columns = execute(
        context.as_ref(),
        &format!(
            "SELECT indkey, 2 = ANY(indkey), 3 = ANY(indkey), NULL = ANY(indkey), array(select unnest::bigint from unnest(indkey)) FROM pg_index WHERE indexrelid={}",
            match unique[7] {
                Value::Signed(n) => n,
                _ => panic!(),
            }
        ),
    );
    assert_eq!(
        columns,
        vec![vec![
            Value::Text("2 1".into()),
            Value::Text("true".into()),
            Value::Text("false".into()),
            Value::Null,
            Value::Text("{2,1}".into())
        ]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT NULL = ANY(conexclop), conexclop::int[], array(select unnest::regoper::varchar from unnest(conexclop)) FROM pg_constraint WHERE conname='note_unique'"
        ),
        vec![vec![Value::Null, Value::Null, Value::Text("{}".into())]]
    );
    assert!(
        CatalogQuery::parse("SELECT array(select oid from unnest(conexclop)) FROM pg_constraint")
            .is_err()
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT 1 = ANY(confkey), array(select unnest::bigint from unnest(confkey)) FROM pg_constraint WHERE conname='note_unique'"
        ),
        vec![vec![Value::Null, Value::Text("{}".into())]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT 1 = ANY(NULL::int[]) FROM pg_constraint WHERE conname='note_unique'"
        ),
        vec![vec![Value::Null]]
    );
    assert!(CatalogQuery::parse("SELECT oid = ANY(conname) FROM pg_constraint").is_err());
    assert!(CatalogQuery::parse("SELECT conname::int[] FROM pg_constraint").is_err());
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT NULL = ANY(indclass) FROM pg_index WHERE indisprimary=true"
        ),
        vec![vec![Value::Text("false".into())]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT conkey::int[] FROM pg_constraint WHERE conname='note_unique'"
        ),
        vec![vec![Value::Text("{2,1}".into())]]
    );
    let result = CatalogQuery::parse("SELECT indclass FROM pg_index")
        .unwrap()
        .unwrap()
        .execute(context.as_ref(), &CancellationToken::new())
        .unwrap();
    assert!(
        result
            .rows
            .iter()
            .all(|row| row[0] == Value::Text(String::new()))
    );
    let wire = crate::pg_result::encode(&result, "SELECT").unwrap();
    let description = &wire.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
    // One column: count, nul-terminated name, table OID, attribute number, type OID.
    let type_offset = 2 + "indclass".len() + 1 + 4 + 2;
    assert_eq!(
        u32::from_be_bytes(
            description[type_offset..type_offset + 4]
                .try_into()
                .unwrap()
        ),
        30
    );
    for row in &constraints_rows {
        assert!(index_rows.iter().any(|index| index[3] == row[7]));
        assert_eq!(row[0], Value::Signed(table_id));
        assert_eq!(row[16], Value::Null);
        assert_eq!(row[17], Value::Text("{}".into()));
    }
    context.close().unwrap();
}

#[test]
fn pg_datagrip_templates_sets() {
    let context = context();
    context
        .execute_query(
            "CREATE TABLE test.dg_sets (id INT PRIMARY KEY)",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let types = include_str!("testdata/pg_datagrip/1869280144.sql").replace('?', "11");
    let rows = execute(context.as_ref(), &types);
    assert!(rows.iter().any(|r| r[0] == Value::Signed(26)));
    assert!(rows.iter().any(|r| r[0] == Value::Signed(25)));
    let routines = include_str!("testdata/pg_datagrip/1869280145.sql").replace('?', "11");
    let rows = execute(context.as_ref(), &routines);
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|r| r.last() == Some(&Value::Text("internal".into())))
    );
    assert_eq!(execute(context.as_ref(), "SELECT oid FROM pg_class WHERE relname='dg_sets' UNION SELECT oid FROM pg_class WHERE relname='dg_sets'").len(), 1);
    assert_eq!(execute(context.as_ref(), "SELECT oid FROM pg_class WHERE relname='dg_sets' UNION ALL SELECT oid FROM pg_class WHERE relname='dg_sets'").len(), 2);
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT DISTINCT relkind FROM pg_class WHERE relname='dg_sets'"
        ),
        vec![vec![Value::Text("r".into())]]
    );
    let query = CatalogQuery::parse(&routines).unwrap().unwrap();
    assert_eq!(query.metadata().columns[0].name, "lang_oid");
    assert_eq!(query.metadata().columns.len(), 23);
    assert!(execute(context.as_ref(), &types.replace("11", "0")).is_empty());
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT prorettype AS type_id FROM pg_proc UNION SELECT DISTINCT unnest(proargtypes) AS type_id FROM pg_proc ORDER BY type_id DESC LIMIT 1"
        ),
        vec![vec![Value::Signed(26)]]
    );
    assert_eq!(
        execute(context.as_ref(), "SELECT DISTINCT NULL FROM pg_proc"),
        vec![vec![Value::Null]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT NULL FROM pg_proc UNION SELECT NULL FROM pg_proc"
        ),
        vec![vec![Value::Null]]
    );
    let zipped = execute(
        context.as_ref(),
        "SELECT oid, unnest(proargtypes), unnest(proallargtypes) FROM pg_proc ORDER BY oid",
    );
    assert_eq!(zipped.len(), 3);
    assert!(
        zipped
            .iter()
            .all(|r| r[1] == Value::Signed(26) && r[2] == Value::Null)
    );
    assert!(
        execute(
            context.as_ref(),
            "SELECT unnest(proallargtypes) FROM pg_proc"
        )
        .is_empty()
    );
    let mismatched = "WITH a AS (SELECT oid AS id, proname AS name FROM pg_proc), b AS (SELECT oid AS id, 'different' AS name FROM pg_proc) SELECT * FROM a NATURAL JOIN b";
    assert!(execute(context.as_ref(), mismatched).is_empty());
    let matched = "WITH a AS (SELECT oid AS id, proname AS name FROM pg_proc), b AS (SELECT oid AS id, proname AS name FROM pg_proc) SELECT id, name FROM a NATURAL JOIN b";
    assert_eq!(execute(context.as_ref(), matched).len(), 3);
    assert_eq!(execute(context.as_ref(), "WITH a AS (SELECT oid AS id FROM pg_proc), b AS (SELECT oid AS other FROM pg_proc) SELECT * FROM a NATURAL JOIN b").len(), 9);
    assert!(execute(context.as_ref(), "SELECT aggtranstype FROM pg_aggregate").is_empty());
    assert!(execute(context.as_ref(), "SELECT oprresult FROM pg_operator").is_empty());
    for sql in [
        "SELECT oid FROM pg_proc WHERE unnest(proargtypes)=26",
        "SELECT unnest(proargtypes)::bigint FROM pg_proc",
        "SELECT oid FROM pg_proc UNION SELECT proname FROM pg_proc",
        "SELECT oid FROM pg_proc UNION SELECT oid, proname FROM pg_proc",
        "SELECT unnest(proname) FROM pg_proc",
        "SELECT oid FROM pg_proc UNION SELECT oid FROM pg_proc ORDER BY proname",
    ] {
        assert!(CatalogQuery::parse(sql).is_err(), "{sql}");
    }
    let arms = std::iter::repeat_n("SELECT oid FROM pg_proc", 18)
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    assert!(CatalogQuery::parse(&arms).is_err());
    let promoted =
        CatalogQuery::parse("SELECT oid FROM pg_proc UNION ALL SELECT oid::bigint FROM pg_proc")
            .unwrap()
            .unwrap();
    assert_eq!(promoted.metadata().native_types[0].code, 8);
    assert_eq!(
        promoted
            .execute(context.as_ref(), &CancellationToken::new())
            .unwrap()
            .rows
            .len(),
        6
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        CatalogQuery::parse(&types)
            .unwrap()
            .unwrap()
            .execute(context.as_ref(), &cancel)
            .is_err()
    );
    context.close().unwrap();
}

#[test]
fn pg_datagrip_metadata() {
    let context = context();
    for sql in [
        "CREATE SEQUENCE test.dg_metadata_seq START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 5 CYCLE",
        "CREATE TABLE test.dg_metadata (id INT COMMENT 'identifier') COMMENT='table note'",
    ] {
        context
            .execute_query(sql, false, &CancellationToken::new())
            .unwrap();
    }
    let Value::Signed(namespace) = execute(
        context.as_ref(),
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    )[0][0] else {
        panic!()
    };
    let sql =
        include_str!("testdata/pg_datagrip/1869280140.sql").replace('?', &namespace.to_string());
    let rows = execute(context.as_ref(), &sql);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2], Value::Text("dg_metadata_seq".into()));
    assert_eq!(
        &rows[0][3..10],
        &[
            Value::Text("bigint".into()),
            Value::Signed(7),
            Value::Signed(3),
            Value::Signed(1),
            Value::Signed(100),
            Value::Signed(5),
            Value::Text("true".into())
        ]
    );
    let id = execute(
        context.as_ref(),
        "SELECT oid FROM pg_class WHERE relname='dg_metadata'",
    )[0][0]
        .clone();
    let comments = execute(
        context.as_ref(),
        "SELECT objoid, objsubid, description FROM pg_description ORDER BY objsubid",
    );
    assert_eq!(
        comments,
        vec![
            vec![
                id.clone(),
                Value::Signed(0),
                Value::Text("table note".into())
            ],
            vec![
                id.clone(),
                Value::Signed(1),
                Value::Text("identifier".into())
            ]
        ]
    );
    context
        .execute_query(
            "CREATE SEQUENCE test.dg_metadata_uncached NOCACHE",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let uncached = execute(context.as_ref(), &sql)
        .into_iter()
        .find(|r| r[2] == Value::Text("dg_metadata_uncached".into()))
        .unwrap();
    assert_eq!(uncached[8], Value::Signed(1));
    context
        .execute_query(
            "DROP TABLE test.dg_metadata",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    context
        .execute_query(
            "CREATE TABLE test.dg_metadata (id INT) COMMENT='updated note'",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let replaced_id = execute(
        context.as_ref(),
        "SELECT oid FROM pg_class WHERE relname='dg_metadata'",
    )[0][0]
        .clone();
    assert_ne!(replaced_id, id);
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT description FROM pg_description WHERE objsubid=0"
        ),
        vec![vec![Value::Text("updated note".into())]]
    );
    context
        .execute_query(
            "CREATE TABLE test.dg_metadata_partition (id INT) PARTITION BY HASH(id) PARTITIONS 2",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let tables =
        include_str!("testdata/pg_datagrip/1869280142.sql").replace('?', &namespace.to_string());
    let partition = execute(context.as_ref(), &tables)
        .into_iter()
        .find(|r| r[1] == Value::Text("dg_metadata_partition".into()))
        .unwrap();
    assert_eq!(partition[0], Value::Text("p".into()));
    assert_eq!(partition[10], Value::Text("false".into()));
    assert_eq!(partition[11], Value::Text("HASH (\"id\")".into()));
    assert_eq!(partition[12], Value::Null);
    assert_eq!(partition[13], Value::Signed(0)); // Native storage has no PG access method.
    context
        .execute_query(
            "DROP TABLE test.dg_metadata",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(execute(context.as_ref(), "SELECT objoid FROM pg_description").is_empty());
    context.close().unwrap();
}

#[test]
fn pg_datagrip_metadata_templates() {
    let context = context();
    context.execute_query("CREATE TABLE test.dg_metadata_templates (id INT COMMENT 'column text') COMMENT='table text'", false, &CancellationToken::new()).unwrap();
    let Value::Signed(namespace) = execute(
        context.as_ref(),
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    )[0][0] else {
        panic!()
    };
    let table_id = execute(
        context.as_ref(),
        "SELECT oid FROM pg_class WHERE relname='dg_metadata_templates'",
    )[0][0]
        .clone();
    for source in [
        include_str!("testdata/pg_datagrip/1869280141.sql"),
        include_str!("testdata/pg_datagrip/1869280143.sql"),
        include_str!("testdata/pg_datagrip/1869280146.sql"),
        include_str!("testdata/pg_datagrip/1869280147.sql"),
        include_str!("testdata/pg_datagrip/1869280148.sql"),
        include_str!("testdata/pg_datagrip/1869280149.sql"),
        include_str!("testdata/pg_datagrip/1869280150.sql"),
        include_str!("testdata/pg_datagrip/1869280151.sql"),
        include_str!("testdata/pg_datagrip/1869280152.sql"),
        include_str!("testdata/pg_datagrip/1869280155.sql"),
        include_str!("testdata/pg_datagrip/1869280156.sql"),
        include_str!("testdata/pg_datagrip/1869280157.sql"),
    ] {
        let sql = source.replace('?', &namespace.to_string());
        let rows = execute(context.as_ref(), &sql);
        if source.contains("D.description") {
            assert_eq!(rows.len(), 2);
            assert!(
                rows.iter()
                    .all(|r| r[0] == table_id && r[1] == Value::Text("r".into()))
            );
            assert!(
                rows.iter()
                    .any(|r| r[3] == Value::Text("table text".into()))
            );
            assert!(
                rows.iter()
                    .any(|r| r[3] == Value::Text("column text".into()))
            );
        } else if source.contains("relacl") {
            let query = CatalogQuery::parse(&sql).unwrap().unwrap();
            assert_eq!(
                query.metadata().native_types[1].code,
                crate::pg_result::CatalogColumnType::AclArray as u8
            );
            assert!(rows.iter().any(|r| r[1] == Value::Null));
        } else {
            assert!(rows.is_empty(), "{sql}");
        }
    }
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT pg_catalog.translate('pufc', 'pufc', 'kkxz'), translate('abc', 'ac', 'X'), translate(NULL, 'a', 'b') FROM pg_namespace WHERE nspname='public'"
        ),
        vec![vec![
            Value::Text("kkxz".into()),
            Value::Text("Xb".into()),
            Value::Null
        ]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT oid::regtype::varchar FROM pg_type WHERE oid=20"
        ),
        vec![vec![Value::Text("bigint".into())]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT oid::regprocedure::varchar FROM pg_proc WHERE oid=16000"
        ),
        vec![vec![Value::Text("pg_get_function_arguments(oid)".into())]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT oid::regproc::text FROM pg_proc WHERE oid=16000"
        ),
        vec![vec![Value::Text("pg_get_function_arguments".into())]]
    );
    for sql in [
        "SELECT oid::regtype FROM pg_type",
        "SELECT oid::regprocedure FROM pg_proc",
        "SELECT translate(oid, 'a', 'b') FROM pg_type",
        "SELECT 'text'::regtype::varchar FROM pg_type",
    ] {
        assert!(CatalogQuery::parse(sql).is_err(), "{sql}");
    }
    let chars = CatalogQuery::parse(
        "SELECT 'wide'::char, 'r'::\"char\" FROM pg_namespace WHERE nspname='public'",
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        chars.metadata().native_types[0].code,
        crate::pg_result::CatalogColumnType::Char as u8
    );
    assert_eq!(
        chars.metadata().native_types[1].code,
        crate::pg_result::CatalogColumnType::InternalChar as u8
    );
    assert_eq!(
        chars
            .execute(context.as_ref(), &CancellationToken::new())
            .unwrap()
            .rows,
        vec![vec![Value::Text("w".into()), Value::Text("r".into())]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT translate('aab', 'aa', 'XY'), translate('猫狗猫', '猫狗', '虎') FROM pg_namespace WHERE nspname='public'"
        ),
        vec![vec![Value::Text("XXb".into()), Value::Text("虎虎".into())]]
    );
    let bounded = format!(
        "SELECT translate('{}', 'x', 'y') FROM pg_class",
        "x".repeat(8000)
    );
    assert!(
        CatalogQuery::parse(&bounded)
            .unwrap()
            .unwrap()
            .execute(context.as_ref(), &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("work limit")
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT CASE WHEN oid=20 THEN 0 ELSE oid END FROM pg_type WHERE oid IN (20,25) ORDER BY oid"
        ),
        vec![vec![Value::Signed(0)], vec![Value::Signed(25)]]
    );
    assert!(
        CatalogQuery::parse("SELECT CASE WHEN oid=20 THEN true ELSE oid END FROM pg_type").is_err()
    );
    context.close().unwrap();
}

#[test]
fn pg_datagrip_ui_metadata_core() {
    let context = context();
    context.execute_query("CREATE TABLE test.dg_ui_structure (id INT PRIMARY KEY, note VARCHAR(30), UNIQUE KEY note_unique (note,id))",false,&CancellationToken::new()).unwrap();
    let namespace = match execute(
        context.as_ref(),
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    )[0][0]
    {
        Value::Signed(n) => n,
        _ => panic!("namespace"),
    };
    let sql = include_str!("testdata/pg_datagrip/RetrieveIndexColumns.sql")
        .replace('?', &namespace.to_string());
    let rows = execute(context.as_ref(), &sql);
    assert_eq!(rows.len(), 3);
    let index = execute(
        context.as_ref(),
        "SELECT indexrelid FROM pg_index WHERE indnatts=2",
    )[0][0]
        .clone();
    let composite = rows.iter().filter(|r| r[0] == index).collect::<Vec<_>>();
    assert_eq!(composite.len(), 2);
    assert_eq!(
        (composite[0][1].clone(), composite[0][3].clone()),
        (Value::Signed(1), Value::Signed(2))
    );
    assert_eq!(
        (composite[1][1].clone(), composite[1][3].clone()),
        (Value::Signed(2), Value::Signed(1))
    );
    for row in rows {
        assert_eq!(row[2], Value::Text("true".into()));
        assert_eq!(row[4], Value::Signed(0));
        assert_eq!(row[5], Value::Signed(0));
        assert_eq!(row[6], Value::Null);
        assert_eq!(row[8], Value::Null);
        assert_eq!(row[12], Value::Null);
        assert_eq!(row[11], Value::Null);
    }
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT indkey[0], indkey[1], indkey[-1], indkey[2], indclass[0] FROM pg_index WHERE indnatts=2"
        ),
        vec![vec![
            Value::Signed(2),
            Value::Signed(1),
            Value::Null,
            Value::Null,
            Value::Null
        ]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT conkey[1], conkey[0], conkey[2] FROM pg_constraint WHERE conname='note_unique'"
        ),
        vec![vec![Value::Signed(2), Value::Null, Value::Signed(1)]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            include_str!("testdata/pg_datagrip/1869280137.sql")
        ),
        vec![vec![Value::Signed(0)]]
    );
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT 7 - 5 % 2, CASE WHEN false THEN 1 WHEN true THEN 2 ELSE 3 END, pg_catalog.pg_is_in_recovery(), NULL::int FROM pg_catalog.pg_namespace WHERE nspname='public'"
        ),
        vec![vec![
            Value::Signed(6),
            Value::Signed(2),
            Value::Text("false".into()),
            Value::Null
        ]]
    );
    let native = context
        .execute_query(
            "SELECT Super_priv FROM mysql.user WHERE User='root'",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    let Value::Text(privilege) = &native[0].rows[0][0] else {
        panic!("native privilege");
    };
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT usesuper FROM pg_user WHERE usename='root'"
        ),
        vec![vec![Value::Text((privilege == "Y").to_string())]]
    );
    context
        .execute_query(
            "UPDATE mysql.user SET Super_priv='Y' WHERE User='root'",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        execute(
            context.as_ref(),
            "SELECT usesuper FROM pg_user WHERE usename='root'"
        ),
        vec![vec![Value::Text("true".into())]]
    );
    for sql in [
        "SELECT 1 % 0 FROM pg_namespace",
        "SELECT 2147483648::int FROM pg_namespace",
        "SELECT chr(0) FROM pg_namespace",
    ] {
        assert!(
            CatalogQuery::parse(sql)
                .unwrap()
                .unwrap()
                .execute(context.as_ref(), &CancellationToken::new())
                .is_err(),
            "{sql}"
        );
    }
    for sql in [
        "SELECT indkey[true] FROM pg_index",
        "SELECT oid FROM pg_namespace CROSS JOIN unnest(oid) u",
        "SELECT oid FROM pg_namespace CROSS JOIN unnest(nspname) u",
        "SELECT oid FROM pg_namespace CROSS JOIN unnest(oid) WITH ORDINALITY u(u,k,extra)",
        "SELECT oid FROM pg_namespace CROSS JOIN pg_catalog.pg_indexam_has_property(0,'bogus') p",
        "SELECT oid FROM pg_namespace JOIN pg_class c ON later.u=1 CROSS JOIN unnest(c.relacl) later",
    ] {
        assert!(CatalogQuery::parse(sql).is_err(), "{sql}");
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        CatalogQuery::parse(&sql)
            .unwrap()
            .unwrap()
            .execute(context.as_ref(), &cancel)
            .is_err()
    );
}

#[test]
fn pg_datagrip_function_acl_row_width() {
    let context = context();
    let acls = execute(context.as_ref(), "SELECT proacl FROM pg_proc");
    assert!(!acls.is_empty());
    assert!(acls.iter().all(|row| row == &[Value::Null]));
    let rows = execute(
        context.as_ref(),
        "SELECT p.oid, p.proacl, n.nspname FROM pg_proc p JOIN pg_namespace n ON p.pronamespace=n.oid ORDER BY p.oid",
    );
    assert!(!rows.is_empty(), "built-in function rows are real metadata");
    for row in rows {
        assert!(matches!(row[0], Value::Signed(_)));
        assert_eq!(row[1], Value::Null);
        assert_eq!(row[2], Value::Text("pg_catalog".into()));
    }
}
