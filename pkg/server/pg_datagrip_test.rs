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
            vec![id, Value::Signed(1), Value::Text("identifier".into())]
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
            "ALTER TABLE test.dg_metadata COMMENT='updated note'",
            false,
            &CancellationToken::new(),
        )
        .unwrap();
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
