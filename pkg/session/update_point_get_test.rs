// Copyright 2026 AsterSQL.

//! 聚簇主键点查 UPDATE 的回归测试。
//!
//! 通过失败注入禁止全表扫描，验证主键等值条件会走点查更新；同时覆盖唯一索引维护、
//! 主键与唯一键冲突、零影响行、事务回滚，以及非主键条件仍保留扫描回退路径。

use std::sync::Arc;

use astersql_domain::KvInfoSchemaLoader;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage, mockStorage};

use crate::runtime::{ConcreteSession, ConcreteTestRuntime, RuntimeDomain};
use crate::testutil::TestRuntime;
use crate::{SessionError, SessionResult};

const FULL_SCAN_FAILPOINT: &str = "session/updateFullTableScan";

/// 为每个用例构造独立的内存 KV 存储，避免状态在测试之间泄漏。
fn new_storage() -> SessionResult<mockStorage> {
    Arc::try_unwrap(
        NewMockStorage(KVStore::NewMemory(), None)
            .map_err(|error| SessionError::new(error.to_string()))?,
    )
    .map_err(|_| SessionError::new("mock storage retained an unexpected owner"))
}

fn concrete_session() -> ConcreteSession {
    let runtime = ConcreteTestRuntime::new(new_storage, Arc::new(KvInfoSchemaLoader::new()), false);
    let store = runtime.NewMockStore().expect("create UPDATE mock store");
    let domain = runtime
        .BootstrapSession(store)
        .expect("bootstrap UPDATE domain");
    let domain = domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("UPDATE runtime domain");
    ConcreteSession::new(Arc::clone(domain.domain()))
}

fn orders_table_info() -> astersql_meta_model::TableInfo {
    let mut id_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    id_type.SetFlag(
        astersql_parser_mysql::r#type::PriKeyFlag | astersql_parser_mysql::r#type::NotNullFlag,
    );
    let mut varchar_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeVarchar);
    varchar_type.SetCharset("utf8mb4".to_owned());
    varchar_type.SetCollate("utf8mb4_bin".to_owned());
    let column = |id, name: &str, field_type| astersql_meta_model::ColumnInfo {
        ID: id,
        Name: astersql_parser_ast::NewCIStr(name),
        Offset: (id - 1) as isize,
        State: astersql_meta_model::StatePublic,
        FieldType: field_type,
        ..Default::default()
    };
    astersql_meta_model::TableInfo {
        ID: 9_301,
        Name: astersql_parser_ast::NewCIStr("orders"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        PKIsHandle: true,
        Columns: vec![
            column(1, "id", id_type),
            column(2, "order_no", varchar_type.clone()),
            column(3, "note", varchar_type),
        ],
        Indices: vec![astersql_meta_model::IndexInfo {
            ID: 9_302,
            Name: astersql_parser_ast::NewCIStr("uk_order_no"),
            State: astersql_meta_model::StatePublic,
            Unique: true,
            Columns: vec![astersql_meta_model::IndexColumn {
                Name: astersql_parser_ast::NewCIStr("order_no"),
                Offset: 1,
                Length: astersql_parser_types::UnspecifiedLength,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// 同时核对唯一索引列和普通列，确保失败路径没有留下部分更新。
fn assert_order(session: &ConcreteSession, id: &str, expected_order_no: &str, expected_note: &str) {
    let rows = session.ReadDmlRows("orders").expect("read orders rows");
    let row = rows
        .iter()
        .find(|row| row["id"].as_deref() == Some(id))
        .unwrap_or_else(|| panic!("missing orders row id={id}"));
    assert_eq!(row["order_no"].as_deref(), Some(expected_order_no));
    assert_eq!(row["note"].as_deref(), Some(expected_note));
}

/// 在全表扫描失败注入生效期间执行 SQL，用以证明目标语句确实走点查路径。
fn execute_without_full_scan(session: &ConcreteSession, sql: &str) -> SessionResult<()> {
    let guard = astersql_testkit_testfailpoint::enable(FULL_SCAN_FAILPOINT, "return(true)");
    let result = session.execute(sql).map(|_| ());
    drop(guard);
    result
}

#[test]
fn primary_key_update_avoids_full_scan_and_preserves_unique_index_semantics() {
    let session = concrete_session();
    session
        .RegisterDmlTable(orders_table_info())
        .expect("register orders table");
    session
        .execute(
            "insert into orders values \
             (1, 'ORD-1', 'unrelated'), \
             (2, 'ORD-2', 'target'), \
             (3, 'ORD-3', 'conflict')",
        )
        .expect("seed orders rows");

    let changed = execute_without_full_scan(
        &session,
        "update orders set order_no = 'ORD-2-NEW' where id = 2",
    );
    if let Err(error) = changed {
        assert_order(&session, "2", "ORD-2", "target");
        panic!("primary-key UPDATE used a full scan: {error}");
    }
    assert_eq!(
        session
            .LastDmlReport()
            .expect("changed UPDATE report")
            .AffectedRows,
        1
    );
    assert_order(&session, "2", "ORD-2-NEW", "target");

    // 旧唯一键应在更新后释放，而新唯一键必须立即参与重复键检查。
    session
        .execute("insert into orders values (4, 'ORD-2', 'old-key-reused')")
        .expect("old unique key was removed");
    let duplicate = session
        .execute("insert into orders values (5, 'ORD-2-NEW', 'duplicate')")
        .map(|_| ())
        .expect_err("new unique key must reject another row");
    assert!(duplicate.to_string().contains("[kv:1062]"), "{duplicate}");

    execute_without_full_scan(
        &session,
        "update orders set order_no = 'ORD-2-NEW' where id = 2",
    )
    .expect("same-value point UPDATE");
    assert_eq!(
        session
            .LastDmlReport()
            .expect("same-value UPDATE report")
            .AffectedRows,
        0
    );

    execute_without_full_scan(&session, "update orders set note = 'none' where id = 999")
        .expect("missing point UPDATE");
    assert_eq!(
        session
            .LastDmlReport()
            .expect("missing UPDATE report")
            .AffectedRows,
        0
    );

    let unique_conflict = execute_without_full_scan(
        &session,
        "update orders set order_no = 'ORD-3' where id = 2",
    )
    .expect_err("point UPDATE must probe the committed unique-index KV");
    assert!(
        unique_conflict.to_string().contains("[kv:1062]")
            && unique_conflict.to_string().contains("uk_order_no"),
        "{unique_conflict}"
    );
    assert_order(&session, "2", "ORD-2-NEW", "target");

    let primary_conflict =
        execute_without_full_scan(&session, "update orders set id = 3 where id = 2")
            .expect_err("point UPDATE must detect a primary-key conflict");
    assert!(
        primary_conflict.to_string().contains("[kv:1062]"),
        "{primary_conflict}"
    );
    assert_order(&session, "2", "ORD-2-NEW", "target");

    // 主键点查更新仍须遵守事务隔离：事务内可见，回滚后恢复原值。
    session
        .execute("begin")
        .expect("begin point UPDATE transaction");
    execute_without_full_scan(
        &session,
        "update orders set note = 'inside-transaction' where id = 2",
    )
    .expect("transactional point UPDATE");
    assert_order(&session, "2", "ORD-2-NEW", "inside-transaction");
    session
        .execute("rollback")
        .expect("roll back point UPDATE transaction");
    assert_order(&session, "2", "ORD-2-NEW", "target");

    // 非主键谓词不属于本优化范围，应继续进入扫描路径并触发失败注入。
    let guard = astersql_testkit_testfailpoint::enable(FULL_SCAN_FAILPOINT, "return(true)");
    let fallback = session
        .execute("update orders set note = 'fallback' where order_no = 'ORD-1'")
        .map(|_| ())
        .expect_err("non-primary predicate must retain the full-scan fallback");
    drop(guard);
    assert!(
        fallback
            .to_string()
            .contains("UPDATE reached a full relational table scan"),
        "{fallback}"
    );
    assert_order(&session, "1", "ORD-1", "unrelated");
}

#[test]
fn composite_integer_primary_key_update_avoids_full_scan() {
    let session = concrete_session();
    let mut table = orders_table_info();
    table.PKIsHandle = false;
    table.IsCommonHandle = true;
    table.Columns[1].FieldType = table.Columns[0].FieldType.clone();
    table.Indices[0].Name = astersql_parser_ast::NewCIStr("uk_note");
    table.Indices[0].Columns[0].Name = astersql_parser_ast::NewCIStr("note");
    table.Indices[0].Columns[0].Offset = 2;
    session
        .RegisterDmlTable(table)
        .expect("register common handle table");
    session
        .execute("insert into orders values (1,1,'one'),(1,2,'two'),(2,1,'three')")
        .expect("seed composite keys");
    execute_without_full_scan(
        &session,
        "update orders set note='changed' where (order_no=2 and 1=id)",
    )
    .expect("composite key UPDATE must avoid scanning unrelated rows");
    let assert_rows = || {
        let mut rows = session
            .ReadDmlRows("orders")
            .unwrap()
            .into_iter()
            .map(|row| {
                (
                    row["id"].clone(),
                    row["order_no"].clone(),
                    row["note"].clone(),
                )
            })
            .collect::<Vec<_>>();
        rows.sort();
        let expected = [
            ("1", "1", "one"),
            ("1", "2", "changed"),
            ("2", "1", "three"),
        ]
        .map(|(id, order, note)| {
            (
                Some(id.to_owned()),
                Some(order.to_owned()),
                Some(note.to_owned()),
            )
        });
        assert_eq!(rows, expected);
    };
    assert_rows();
    assert_eq!(session.LastDmlReport().unwrap().AffectedRows, 1);
    execute_without_full_scan(
        &session,
        "update orders set note='changed' where id=1 and order_no=2",
    )
    .expect("unchanged composite row");
    assert_eq!(session.LastDmlReport().unwrap().AffectedRows, 0);
    execute_without_full_scan(
        &session,
        "update orders set note='missing' where id=9 and order_no=2",
    )
    .expect("missing composite row");
    assert_eq!(session.LastDmlReport().unwrap().AffectedRows, 0);
    let duplicate = execute_without_full_scan(
        &session,
        "update orders set note='three' where id=1 and order_no=2",
    )
    .unwrap_err();
    assert!(duplicate.to_string().contains("[kv:1062]"), "{duplicate}");
    let duplicate = execute_without_full_scan(
        &session,
        "update orders set order_no=1 where id=1 and order_no=2",
    )
    .unwrap_err();
    assert!(duplicate.to_string().contains("[kv:1062]"), "{duplicate}");
    session.execute("begin").unwrap();
    execute_without_full_scan(
        &session,
        "update orders set note='inside' where id=1 and order_no=2",
    )
    .unwrap();
    session.execute("rollback").unwrap();
    assert_rows();
    execute_without_full_scan(
        &session,
        "update orders set note='residual' where id=1 and order_no=2 and note='absent'",
    )
    .expect("retain residual filtering inside a point range");
    assert_eq!(session.LastDmlReport().unwrap().AffectedRows, 0);
    assert_rows();
    let disjunction = execute_without_full_scan(
        &session,
        "update orders set note='disjunction' where id=1 or order_no=2",
    )
    .unwrap_err();
    assert!(
        disjunction
            .to_string()
            .contains("UPDATE reached a full relational table scan")
    );
    let fallback =
        execute_without_full_scan(&session, "update orders set note='partial' where id=1")
            .unwrap_err();
    assert!(
        fallback
            .to_string()
            .contains("UPDATE reached a full relational table scan")
    );
}
