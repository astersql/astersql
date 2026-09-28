// Copyright 2026 AsterSQL.

use super::CreateAnalyzeSession;

#[test]
fn insert_select_preserves_binary_bytes_across_history_copy() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("create table binary_source(v blob)")
        .unwrap();
    session
        .execute("create table binary_history(v blob)")
        .unwrap();
    session
        .execute("insert into binary_source values(x'00ff7b7d')")
        .unwrap();
    session
        .execute("insert into binary_history select * from binary_source")
        .unwrap();
    let source = session.execute("select v from binary_source").unwrap();
    let history = session.execute("select v from binary_history").unwrap();
    assert_eq!(source[0].rows, history[0].rows);
    assert_eq!(
        super::row_codec::binary_runtime_bytes(&history[0].rows[0][0]),
        Some(vec![0, 255, 123, 125])
    );
    domain.close();
}
