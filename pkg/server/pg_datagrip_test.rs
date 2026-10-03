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
    context.execute_query("CREATE TABLE test.dg_structure (id INT PRIMARY KEY, note VARCHAR(30), UNIQUE KEY note_unique (note))", false, &CancellationToken::new()).unwrap();
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
    for row in &constraints_rows {
        assert!(index_rows.iter().any(|index| index[3] == row[7]));
        assert_eq!(row[17], Value::Text("{}".into()));
    }
    context.close().unwrap();
}
