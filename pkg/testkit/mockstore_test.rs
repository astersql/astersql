// Copyright 2026 AsterSQL.

use crate::db_driver::{Database, DbValue};
use crate::mockstore::CreateMockStoreAndDomain;

#[test]
fn parameterized_execution_uses_and_releases_counted_prepared_statements() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let configurator = store
        .create_session()
        .expect("create session")
        .expect("canonical session");

    configurator
        .execute("set global max_prepared_stmt_count = 1", &[])
        .expect("limit prepared statements");
    configurator.close().expect("close configurator");

    let session = store
        .create_session()
        .expect("create limited session")
        .expect("canonical limited session");
    session
        .execute("create table t (id bigint primary key)", &[])
        .expect("create fixture table");
    session
        .execute("prepare blocker from 'select ?'", &[])
        .expect("occupy the single prepared-statement slot");

    let error = session
        .execute("insert into t values (?)", &[DbValue::I64(0)])
        .expect_err("transient prepare must share the canonical statement limit");
    assert!(
        error.to_string().contains("maxPreparedStmtCount"),
        "unexpected prepare-limit error: {error}"
    );
    session
        .execute("deallocate prepare blocker", &[])
        .expect("release blocker");

    session
        .execute("insert into t values (?)", &[DbValue::I64(1)])
        .expect("first parameterized execution");
    session
        .execute("insert into t(id) values (?)", &[DbValue::I64(2)])
        .expect("transient prepared statement was released");
    session
        .execute("set global max_prepared_stmt_count = -1", &[])
        .expect("restore unlimited prepared statements for the fixture domain");

    session.close().expect("close session");
    store.close().expect("close store");
}
