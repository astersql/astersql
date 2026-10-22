// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::testutil::TestRecordSet;

#[test]
fn expr_pushdown_blacklist_delete_respects_where_predicate() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database if not exists test")
        .expect("create test database");
    session.execute("use test").expect("select test database");
    session
        .execute("create table t(a decimal(10,2), b date)")
        .expect("create expression test table");
    session
        .execute(
            "insert into mysql.expr_pushdown_blacklist values \
             ('date_format', 'tikv'), ('cast', 'tikv')",
        )
        .expect("seed two blacklist rows");
    session
        .execute("delete from mysql.expr_pushdown_blacklist where name = 'cast'")
        .expect("delete one blacklist row");
    session
        .execute("admin reload expr_pushdown_blacklist")
        .expect("reload remaining blacklist row");

    session
        .execute(
            "explain select * from t where \
             date_format(b, '%m') = '11' and cast(a as decimal(10,2)) > 10.10",
        )
        .expect("the unmatched date_format blacklist row must remain");
}

#[test]
fn show_regions_deduplicates_unsplit_index_ranges_like_go_mockstore() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database if not exists test")
        .expect("create test database");
    session.execute("use test").expect("select test database");
    session
        .execute("create table t(a int, index(a)) partition by hash(a) partitions 3")
        .expect("create indexed partition table");

    let mut result = session
        .execute("show table t regions")
        .expect("show table regions")
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read region row") {
        rows.push(row);
    }

    assert_eq!(rows.len(), 3);
}

#[test]
fn mysql_tidb_insert_uses_session_transaction_and_preserves_duplicate_semantics() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    let check = crate::runtime::ConcreteSession::new(domain.clone());
    let rows = |session: &crate::runtime::ConcreteSession| {
        let mut record = session.execute("SELECT VARIABLE_VALUE, COMMENT FROM mysql.tidb WHERE VARIABLE_NAME='starter_txn_boundary'").unwrap().remove(0);
        let row = record.next_row().unwrap();
        record.close().unwrap();
        row
    };
    session.execute("BEGIN").unwrap();
    session
        .execute(
            "INSERT INTO mysql.tidb VALUES ('starter_txn_boundary','first','original comment')",
        )
        .unwrap();
    assert_eq!(
        rows(&session),
        Some(vec!["first".into(), "original comment".into()])
    );
    assert_eq!(rows(&check), None);
    assert!(
        session
            .execute("INSERT INTO mysql.tidb VALUES ('starter_txn_boundary','duplicate','test')")
            .err()
            .expect("duplicate insert must fail")
            .to_string()
            .contains("1062")
    );
    session.execute("ROLLBACK").unwrap();
    assert_eq!(rows(&check), None);
    session
        .execute(
            "INSERT INTO mysql.tidb VALUES ('starter_txn_boundary','first','original comment')",
        )
        .unwrap();
    session.execute("INSERT IGNORE INTO mysql.tidb VALUES ('starter_txn_boundary','ignored','changed comment')").unwrap();
    assert_eq!(
        rows(&check),
        Some(vec!["first".into(), "original comment".into()])
    );
    session.execute("INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('starter_txn_boundary','second','changed comment') ON DUPLICATE KEY UPDATE VARIABLE_VALUE='second'").unwrap();
    assert_eq!(
        rows(&check),
        Some(vec!["second".into(), "original comment".into()])
    );
    session.execute("BEGIN").unwrap();
    session.execute("UPDATE mysql.tidb SET VARIABLE_VALUE='temporary' WHERE VARIABLE_NAME='starter_txn_boundary'").unwrap();
    assert_eq!(
        rows(&session),
        Some(vec!["temporary".into(), "original comment".into()])
    );
    assert_eq!(
        rows(&check),
        Some(vec!["second".into(), "original comment".into()])
    );
    session.execute("ROLLBACK").unwrap();
    assert_eq!(
        rows(&check),
        Some(vec!["second".into(), "original comment".into()])
    );
    session.execute("BEGIN").unwrap();
    session
        .execute("DELETE FROM mysql.tidb WHERE VARIABLE_NAME='starter_txn_boundary'")
        .unwrap();
    assert_eq!(rows(&session), None);
    assert!(rows(&check).is_some());
    session.execute("ROLLBACK").unwrap();
    assert!(rows(&check).is_some());
    domain.close();
}

#[test]
fn reorg_global_admin_check_rejects_missing_scalar_index_entries() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    session
        .execute("create database if not exists test")
        .unwrap();
    session.execute("use test").unwrap();
    session.execute("create table global_check(a int, b int, key idx(b) global) partition by range(a) (partition p0 values less than (10), partition pmax values less than (maxvalue))").unwrap();
    session
        .execute("insert into global_check values(1,10),(11,20)")
        .unwrap();
    session.execute("admin check table global_check").unwrap();
    let table = domain.table_by_name("test", "global_check").unwrap();
    domain.storage_handle().with_storage(|storage| {
        let (start, end) =
            astersql_tablecodec::GetTableIndexKeyRange(table.ID, table.Indices[0].ID);
        let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
        let mut it = snapshot
            .Iter(astersql_kv::Key(start), Some(astersql_kv::Key(end)))
            .unwrap();
        assert!(it.Valid());
        let key = it.Key();
        it.Close();
        let mut tx = storage.Begin(&[]).unwrap();
        tx.Delete(key).unwrap();
        tx.Commit(&astersql_kv::Context::default()).unwrap();
    });
    let error = session
        .execute("admin check table global_check")
        .err()
        .expect("missing global entry must be detected");
    assert!(
        error.to_string().contains("index entry is missing"),
        "{error}"
    );
}
