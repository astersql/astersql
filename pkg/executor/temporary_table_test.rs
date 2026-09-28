// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 临时表（temporary table）与点查物理 ID 解析相关测试。
//
// 临时表无真实分区物理 ID 时，[`GetPhysID`] 应回退到逻辑表 ID。

use crate::point_get::{GetPhysID, TableInfo};
use astersql_testkit::{NewTestKit, Rows, TestKit, mockstore::CreateMockStoreAndDomain};

/// 临时表在无分区参数时，物理 ID 等于逻辑表 ID。
#[test]
fn temporary_table_point_get_keeps_logical_physical_id() {
    let table = TableInfo {
        id: 88,
        name: "tmp".into(),
        temporary: true,
        cache_enabled: false,
        pk_is_handle: true,
        is_common_handle: false,
        columns: Vec::new(),
        primary_index: None,
        partition: None,
        table_lock: None,
    };
    assert!(table.temporary);
    // 无分区物理 ID 时保持与逻辑 id 一致。
    assert_eq!(GetPhysID(&table, None), 88);
}

fn assert_temporary_table_no_network(create_table: impl FnOnce(&mut TestKit)) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    let mut normal_table_session = NewTestKit(store);

    tk.MustExec("use test", Vec::new());
    normal_table_session.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists normal, tmp_t", Vec::new());
    tk.MustExec("create table normal (id int, a int, index(a))", Vec::new());
    create_table(&mut tk);

    // Go enables rpcServerBusy here and proves that the normal-table read blocks.
    // Rust TestKit's relational store bypasses the UniStore RPC client (whose
    // available injection only models a short-timeout deadline), so retain the
    // peer-session normal-table control before checking the real session-local
    // temporary-table execution paths below.
    normal_table_session
        .MustQuery("select * from normal", Vec::new())
        .Check(Rows(&[]));

    tk.MustExec("insert into tmp_t values (1, 1, 1)", Vec::new());
    tk.MustExec("insert into tmp_t values (2, 2, 2)", Vec::new());

    assert!(tk.HasPlan("select * from tmp_t where id=1", "Point_Get"));
    tk.MustQuery("select * from tmp_t where id=1", Vec::new())
        .Check(Rows(&["1 1 1"]));
    assert!(tk.HasPlan("select * from tmp_t where id in (1, 2)", "Batch_Point_Get"));
    tk.MustQuery("select * from tmp_t where id in (1, 2)", Vec::new())
        .Check(Rows(&["1 1 1", "2 2 2"]));
    let table_scan_plan = tk
        .MustQuery("explain select * from tmp_t", Vec::new())
        .Rows();
    assert!(
        table_scan_plan
            .iter()
            .flatten()
            .any(|cell| cell.contains("TableReader")),
        "temporary table full scan must use TableReader: {table_scan_plan:?}"
    );
    tk.MustQuery("select * from tmp_t", Vec::new())
        .Check(Rows(&["1 1 1", "2 2 2"]));
    let index_scan_plan = tk
        .MustQuery(
            "explain select /*+ USE_INDEX(tmp_t, a) */ a from tmp_t",
            Vec::new(),
        )
        .Rows();
    assert!(
        index_scan_plan
            .iter()
            .flatten()
            .any(|cell| cell.contains("IndexReader")),
        "covering temporary-table index scan must use IndexReader: {index_scan_plan:?}"
    );
    tk.MustQuery("select /*+ USE_INDEX(tmp_t, a) */ a from tmp_t", Vec::new())
        .Check(Rows(&["1", "2"]));
    assert!(tk.HasPlan(
        "select /*+ USE_INDEX(tmp_t, a) */ b from tmp_t where a = 1",
        "IndexLookUp"
    ));
    tk.MustQuery(
        "select /*+ USE_INDEX(tmp_t, a) */ b from tmp_t where a = 1",
        Vec::new(),
    )
    .Check(Rows(&["1"]));
    tk.MustExec("rollback", Vec::new());

    tk.MustExec("insert into tmp_t value(10, 10, 10)", Vec::new());
    tk.MustExec("insert into tmp_t value(11, 11, 11)", Vec::new());
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustExec("insert into tmp_t values (3, 3, 3)", Vec::new());
    tk.MustExec("insert ignore into tmp_t values (4, 4, 4)", Vec::new());
    tk.MustExec(
        "insert into tmp_t values (5, 5, 5) on duplicate key update a=100",
        Vec::new(),
    );
    tk.MustExec(
        "insert into tmp_t values (10, 10, 10) on duplicate key update a=100",
        Vec::new(),
    );
    tk.MustExec(
        "insert ignore into tmp_t values (10, 10, 10) on duplicate key update id=11",
        Vec::new(),
    );
    tk.MustExec("replace into tmp_t values(6, 6, 6)", Vec::new());
    tk.MustExec("replace into tmp_t values(11, 100, 100)", Vec::new());
    tk.MustExec("update tmp_t set id = id + 1 where a = 1", Vec::new());
    tk.MustExec("delete from tmp_t where a > 1", Vec::new());
    tk.MustQuery(
        "select count(*) from tmp_t where a >= 1 for update",
        Vec::new(),
    );
    tk.MustExec("rollback", Vec::new());

    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustQuery("select * from tmp_t where id=1 for update", Vec::new());
    tk.MustQuery(
        "select * from tmp_t where id in (1, 2, 3) for update",
        Vec::new(),
    );
    tk.MustQuery("select * from tmp_t where id > 1 for update", Vec::new());
    tk.MustExec("rollback", Vec::new());
}

#[test]
fn normal_global_temporary_table_no_network() {
    assert_temporary_table_no_network(|tk| {
        tk.MustExec(
            "create global temporary table tmp_t (id int primary key, a int, b int, index(a)) on commit delete rows",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
    });
}

#[test]
fn global_temporary_table_no_network_with_create_and_truncate() {
    assert_temporary_table_no_network(|tk| {
        tk.MustExec(
            "create global temporary table tmp_t (id int primary key, a int, b int, index(a)) on commit delete rows",
            Vec::new(),
        );
        tk.MustExec("truncate table tmp_t", Vec::new());
        tk.MustExec("begin", Vec::new());
    });
}

#[test]
fn global_temporary_table_no_network_with_create_then_normal_table() {
    assert_temporary_table_no_network(|tk| {
        tk.MustExec(
            "create global temporary table tmp_t (id int primary key, a int, b int, index(a)) on commit delete rows",
            Vec::new(),
        );
        tk.MustExec("create table txx(a int)", Vec::new());
        tk.MustExec("begin", Vec::new());
    });
}

#[test]
fn local_temporary_table_no_network_with_create_outside_txn() {
    assert_temporary_table_no_network(|tk| {
        tk.MustExec(
            "create temporary table tmp_t (id int primary key, a int, b int, index(a))",
            Vec::new(),
        );
        tk.MustExec("begin", Vec::new());
    });
}

#[test]
fn local_temporary_table_no_network_with_create_inside_txn() {
    assert_temporary_table_no_network(|tk| {
        tk.MustExec("begin", Vec::new());
        tk.MustExec(
            "create temporary table tmp_t (id int primary key, a int, b int, index(a))",
            Vec::new(),
        );
    });
}

#[test]
fn issue_58875_empty_temporary_inner_lookup_has_no_execution_info() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists users, users1", Vec::new());
    tk.MustExec(
        "create global temporary table users (id bigint, v1 int, v2 int, v3 int, v4 int, primary key(id), index v1_index(v1,v2,v3)) on commit delete rows",
        Vec::new(),
    );
    tk.MustExec(
        "create table users1(id int, value int, index index_value(value))",
        Vec::new(),
    );
    tk.MustExec("insert into users1 values(1,2)", Vec::new());
    tk.MustExec("begin", Vec::new());
    let rows = tk
        .MustQuery(
            "explain analyze select /*+ inl_join(users) */ * from users use index(v1_index) where v1 in (select value from users1)",
            Vec::new(),
        )
        .Rows();
    for row in rows {
        let access_object = row.get(4).map(String::as_str).unwrap_or_default();
        if access_object.contains("table:users") && !access_object.contains("table:users1") {
            assert_eq!(
                row.get(5).map(String::as_str).unwrap_or_default(),
                "",
                "temporary-table access must not report execution info: {row:?}"
            );
        }
    }
}
