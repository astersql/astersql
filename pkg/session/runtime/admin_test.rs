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
