// Copyright 2026 AsterSQL.

use crate::runtime::CreateAnalyzeSession;

#[test]
fn single_table_metadata_lookup_preserves_catalog_views() {
    let (domain, session) = CreateAnalyzeSession().expect("metadata lookup session");
    session
        .execute("create table metadata_lookup(a int, index(a))")
        .unwrap();
    let version = domain.stats_context().catalog_version();
    session
        .execute("alter table metadata_lookup add column b varchar(12)")
        .unwrap();
    let assert_same = |database: &str, name: &str| {
        let catalog = session.metadata_catalog().unwrap();
        let expected = catalog.get(&(database.to_ascii_lowercase(), name.to_ascii_lowercase()));
        let actual = session.metadata_table(database, name).unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    };
    for (database, table) in [
        ("test", "metadata_lookup"),
        ("information_schema", "tables"),
        ("sys", "session"),
        ("test", "missing"),
    ] {
        assert_same(database, table);
    }
    assert_eq!(
        session
            .metadata_table("test", "metadata_lookup")
            .unwrap()
            .unwrap()
            .Columns
            .len(),
        2
    );
    session.state.borrow_mut().snapshot_catalog_version = Some(version);
    assert_same("test", "metadata_lookup");
    assert_eq!(
        session
            .metadata_table("test", "metadata_lookup")
            .unwrap()
            .unwrap()
            .Columns
            .len(),
        1
    );
    session.state.borrow_mut().snapshot_catalog_version = None;
    session.state.borrow_mut().snapshot_read_ts = Some(1);
    assert_same("test", "metadata_lookup");
    assert_same("information_schema", "tables");
    session.state.borrow_mut().snapshot_read_ts = None;
    let mut physical = session
        .metadata_table("test", "metadata_lookup")
        .unwrap()
        .unwrap();
    physical.Name = astersql_parser_ast::NewCIStr("session");
    physical.ID = 0;
    domain.register_stats_table("sys", physical).unwrap();
    assert_same("sys", "session");
    assert_eq!(
        session
            .metadata_table("sys", "session")
            .unwrap()
            .unwrap()
            .Columns
            .len(),
        2
    );
    session.execute("drop table metadata_lookup").unwrap();
    assert_same("test", "metadata_lookup");
    assert!(
        session
            .metadata_table("test", "metadata_lookup")
            .unwrap()
            .is_none()
    );
}
