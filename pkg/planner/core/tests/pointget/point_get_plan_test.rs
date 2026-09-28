// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// PointGet 执行计划与 plan cache 行为测试。
//
// 对应 Go `point_get_plan_test.go`。这些测试使用真实 mock store、TestKit SQL
// 执行和 prepared statement，而不是用固定字符串模拟执行计划或手工递增指标。

#![allow(non_snake_case)]

use astersql_config_kerneltype as kerneltype;
use astersql_planner_core::{
    IsPointGetWithPKOrUniqueKeyByAutoCommit, IsSafeToReusePointGetExecutor, PlanKind, PlanNode,
    SessionVars,
};
use astersql_planner_core_metrics::planner_core_metrics::GetPlanCacheHitCounter;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use astersql_types::metadata::{ParseEnumValue, mysql};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

static POINTGET_TEST_MUTEX: Mutex<()> = Mutex::new(());

/// 创建 mock store/domain 与绑定其上的 `TestKit` 会话。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

/// 建 PointGet fixture 表（与 Go 两个 plan-cache/fix-control 用例一致）。
fn setup_point_get_table(tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t(a bigint unsigned primary key, b int, c int, key idx_bc(b,c))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values(1, 1, 1), (2, 2, 2), (3, 3, 3)",
        Vec::new(),
    );
}

fn check_plan(tk: &TestKit, sql: &str, expected: &[&str]) {
    tk.MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
        .Check(
            expected
                .iter()
                .map(|row| row.splitn(4, ' ').map(str::to_owned).collect())
                .collect(),
        );
}

/// PointGet plan-cache 场景的完整 SQL 流程；classic/next-gen 只有写计划的 lock 后缀不同。
fn point_get_plan_cache_common(next_gen: bool) {
    let _guard = POINTGET_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (domain, mut tk) = new_testkit();
    tk.MustExec("set @@tidb_opt_fix_control = '52592:OFF'", Vec::new());
    tk.MustExec("set @@tidb_enable_non_prepared_plan_cache=0", Vec::new());
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    setup_point_get_table(&mut tk);

    let table = domain
        .table_by_name("test", "t")
        .expect("pointget fixture table");
    assert!(table.Columns.iter().any(|column| {
        column.Name.L == "a" && mysql::HasUnsignedFlag(column.FieldType.GetFlag())
    }));

    check_plan(
        &tk,
        "select * from t where a = 1",
        &["Point_Get root table:t handle:1"],
    );
    check_plan(
        &tk,
        "select * from t where 1 = a",
        &["Point_Get root table:t handle:1"],
    );
    let lock = if next_gen { ", lock" } else { "" };
    check_plan(
        &tk,
        "update t set b=b+1, c=c+1 where a = 1",
        &[
            "Update root  N/A",
            &format!("└─Point_Get root table:t handle:1{lock}"),
        ],
    );
    check_plan(
        &tk,
        "delete from t where a = 1",
        &[
            "Delete root  N/A",
            &format!("└─Point_Get root table:t handle:1{lock}"),
        ],
    );
    check_plan(
        &tk,
        "select a from t where a = -1",
        &["TableDual root  rows:0"],
    );

    tk.MustExec(
        r#"prepare stmt0 from "select a from t where a = ?""#,
        Vec::new(),
    );
    tk.MustExec("set @p0 = -1", Vec::new());
    tk.MustQuery("execute stmt0 using @p0", Vec::new())
        .Check(Rows(&[]));

    let counter = GetPlanCacheHitCounter(false);
    let before = counter.get();
    tk.MustExec(
        r#"prepare stmt1 from "select * from t where a = ?""#,
        Vec::new(),
    );
    tk.MustExec(
        r#"prepare stmt2 from "select * from t where b = ? and c = ?""#,
        Vec::new(),
    );
    tk.MustExec("set @param=1", Vec::new());
    tk.MustQuery("execute stmt1 using @param", Vec::new())
        .Check(Rows(&["1 1 1"]));
    assert_eq!(counter.get(), before);
    tk.MustExec("set @param=2", Vec::new());
    tk.MustQuery("execute stmt1 using @param", Vec::new())
        .Check(Rows(&["2 2 2"]));
    assert_eq!(counter.get(), before + 1.0);
    tk.MustQuery("execute stmt2 using @param, @param", Vec::new())
        .Check(Rows(&["2 2 2"]));
    assert_eq!(counter.get(), before + 1.0);
    tk.MustExec("set @param=1", Vec::new());
    tk.MustQuery("execute stmt2 using @param, @param", Vec::new())
        .Check(Rows(&["1 1 1"]));
    assert_eq!(counter.get(), before + 2.0);

    tk.MustExec(
        r#"prepare stmt3 from "update t set b=b+1, c=c+1 where a = ?""#,
        Vec::new(),
    );
    tk.MustExec(
        r#"prepare stmt4 from "update t set a=a+1 where b = ? and c = ?""#,
        Vec::new(),
    );
    tk.MustExec("set @param=3", Vec::new());
    tk.MustExec("execute stmt3 using @param", Vec::new());
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1 1 1", "2 2 2", "3 4 4"]));
    assert_eq!(counter.get(), before + 2.0);
    tk.MustExec("set @param=4", Vec::new());
    tk.MustExec("execute stmt4 using @param, @param", Vec::new());
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1 1 1", "2 2 2", "4 4 4"]));
    assert_eq!(counter.get(), before + 2.0);

    tk.MustExec(
        r#"prepare stmt5 from "delete from t where a = ?""#,
        Vec::new(),
    );
    tk.MustExec(
        r#"prepare stmt6 from "delete from t where b = ? and c = ?""#,
        Vec::new(),
    );
    tk.MustExec("execute stmt5 using @param", Vec::new());
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1 1 1", "2 2 2"]));
    assert_eq!(counter.get(), before + 2.0);
    tk.MustExec("set @param=2", Vec::new());
    tk.MustExec("execute stmt6 using @param, @param", Vec::new());
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1 1 1"]));
    assert_eq!(counter.get(), before + 2.0);

    tk.MustExec(
        "insert into t (a, b, c) values (18446744073709551615, 4, 4)",
        Vec::new(),
    );
    tk.MustExec("set @p1=-1", Vec::new());
    tk.MustExec("set @p2=1", Vec::new());
    tk.MustExec(
        r#"prepare stmt7 from "select a from t where a = ?""#,
        Vec::new(),
    );
    tk.MustQuery("execute stmt7 using @p1", Vec::new())
        .Check(Rows(&[]));
    tk.MustQuery("execute stmt7 using @p2", Vec::new())
        .Check(Rows(&["1"]));
    assert_eq!(counter.get(), before + 2.0);
}

#[test]
fn TestPointGetPlanCache() {
    if kerneltype::IsNextGen() {
        return;
    }
    point_get_plan_cache_common(false);
}

#[test]
fn TestPointGetPlanCacheForNextGen() {
    if kerneltype::IsClassic() {
        return;
    }
    point_get_plan_cache_common(true);
}

/// Test that the plan id is reset before each optimization.
#[test]
fn TestPointGetId() {
    let _guard = POINTGET_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (c1 int primary key, c2 int)", Vec::new());

    for point_get_query in [
        "select c2 from t where c1 = 1",
        "select c2 as value, c1 from t where 1 = c1",
    ] {
        for _ in 0..2 {
            assert_eq!(
                tk.Session()
                    .OptimizeRootPlanIDForTest(point_get_query)
                    .expect("parse, preprocess and optimize point-get query"),
                1
            );
        }
    }
    tk.MustExec("set @@tidb_opt_fix_control = '52592:ON'", Vec::new());
    assert_ne!(
        tk.Session()
            .OptimizeRootPlanIDForTest("select c2 from t where c1 = 1")
            .expect("disabled fast path uses general optimization"),
        1
    );
}

#[test]
fn TestIssue20692() {
    let _guard = POINTGET_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t (id int primary key, v int, vv int, vvv int, unique key u0(id, v, vv))",
        Vec::new(),
    );
    tk.MustExec("insert into t values(1, 1, 1, 1)", Vec::new());
    drop(domain);

    let mut tk1 = TestKit::new(store.clone());
    let mut tk2 = TestKit::new(store.clone());
    let mut tk3 = TestKit::new(store);
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustExec("use test", Vec::new());
    tk2.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("use test", Vec::new());
    tk3.MustExec("begin pessimistic", Vec::new());
    tk3.MustExec("use test", Vec::new());
    tk1.MustExec(
        "delete from t where id = 1 and v = 1 and vv = 1",
        Vec::new(),
    );

    let vars = SessionVars {
        autocommit: true,
        in_txn: false,
        ..SessionVars::default()
    };
    let plan = PlanNode::New(1, PlanKind::Generic("PointGet".into()), Vec::new());
    assert!(IsPointGetWithPKOrUniqueKeyByAutoCommit(&vars, &plan));
    assert!(IsSafeToReusePointGetExecutor(true, false, false, 1, 1));
    assert!(!IsSafeToReusePointGetExecutor(true, true, false, 1, 1));

    let (start_tx, start_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut tk2 = tk2;
        start_rx.recv().expect("start tk2 after tk1 commit");
        tk2.MustExec("insert into t values(1, 2, 3, 4)", Vec::new());
        ready_tx.send(tk2).expect("send tk2 after insert");
        tk3.MustExec(
            "update t set id = 10, v = 20, vv = 30, vvv = 40 where id = 1 and v = 2 and vv = 3",
            Vec::new(),
        );
        finished_tx.send(tk3).expect("send tk3 after update");
    });

    tk1.MustExec("commit", Vec::new());
    start_tx.send(()).expect("start tk2 after tk1 commit");
    let mut tk2 = ready_rx.recv().expect("tk2 insert should finish");
    assert!(finished_rx.recv_timeout(Duration::from_millis(50)).is_err());
    tk2.MustExec("commit", Vec::new());
    let mut tk3 = finished_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("tk3 should proceed after tk2 commit");
    tk3.MustExec("commit", Vec::new());
    tk3.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["10 20 30 40"]));
    worker.join().expect("issue 20692 worker");
}

#[test]
fn TestIssue18042() {
    let _guard = POINTGET_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t(a int, b int, c int, primary key(a), index ab(a, b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1, 1, 1), (2, 2, 2), (3, 3, 3), (4, 4, 4)",
        Vec::new(),
    );

    let sql = "SELECT /*+ MAX_EXECUTION_TIME(100), MEMORY_QUOTA(1 MB) */ * FROM t where a = 1";
    tk.MustExec(sql, Vec::new());
    let hints = tk.Session().LastStatementHintsForTest();
    assert_eq!(hints.MaxExecutionTime, 100);
    assert_eq!(hints.MemQuotaQuery, 1_i64 << 20);
    tk.MustQuery("select * from t where a = 1", Vec::new())
        .Check(Rows(&["1 1 1"]));
}

fn issue_52592_common(next_gen: bool) {
    let _guard = POINTGET_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("set @@tidb_opt_fix_control = '52592:OFF'", Vec::new());
    setup_point_get_table(&mut tk);

    let point_update = if next_gen {
        "└─Point_Get root table:t handle:1, lock"
    } else {
        "└─Point_Get root table:t handle:1"
    };
    check_plan(
        &tk,
        "select * from t where a = 1",
        &["Point_Get root table:t handle:1"],
    );
    check_plan(
        &tk,
        "select * from t where 1 = a",
        &["Point_Get root table:t handle:1"],
    );
    check_plan(
        &tk,
        "update t set b=b+1, c=c+1 where a = 1",
        &["Update root  N/A", point_update],
    );
    check_plan(
        &tk,
        "delete from t where a = 1",
        &[
            "Delete root  N/A",
            if next_gen {
                "└─Point_Get root table:t handle:1, lock"
            } else {
                "└─Point_Get root table:t handle:1"
            },
        ],
    );
    check_plan(
        &tk,
        "select a from t where a = -1",
        &["TableDual root  rows:0"],
    );

    tk.MustExec("set @@tidb_opt_fix_control = '52592:ON'", Vec::new());
    check_plan(
        &tk,
        "select * from t where a = 1",
        &[
            "TableReader root  data:TableRangeScan",
            "└─TableRangeScan cop[tikv] table:t range:[1,1], keep order:false, stats:pseudo",
        ],
    );
    check_plan(
        &tk,
        "select * from t where 1 = a",
        &[
            "TableReader root  data:TableRangeScan",
            "└─TableRangeScan cop[tikv] table:t range:[1,1], keep order:false, stats:pseudo",
        ],
    );
    let write_prefix = if next_gen {
        "    └─TableReader root  data:TableRangeScan"
    } else {
        "└─TableReader root  data:TableRangeScan"
    };
    let scan_prefix = if next_gen {
        "      └─TableRangeScan cop[tikv] table:t range:[1,1], keep order:false, stats:pseudo"
    } else {
        "  └─TableRangeScan cop[tikv] table:t range:[1,1], keep order:false, stats:pseudo"
    };
    let mut update_plan = vec!["Update root  N/A"];
    if next_gen {
        update_plan.push("└─SelectLock root  for update 0");
    }
    update_plan.extend([write_prefix, scan_prefix]);
    check_plan(&tk, "update t set b=b+1, c=c+1 where a = 1", &update_plan);

    let mut delete_plan = vec!["Delete root  N/A"];
    if next_gen {
        delete_plan.push("└─SelectLock root  for update 0");
    }
    delete_plan.extend([write_prefix, scan_prefix]);
    check_plan(&tk, "delete from t where a = 1", &delete_plan);
    check_plan(
        &tk,
        "select a from t where a = -1",
        &["TableDual root  rows:0"],
    );
}

#[test]
fn TestIssue52592() {
    if kerneltype::IsNextGen() {
        return;
    }
    issue_52592_common(false);
}

#[test]
fn TestIssue52592ForNextGen() {
    if kerneltype::IsClassic() {
        return;
    }
    issue_52592_common(true);
}

#[test]
fn TestIssue56832() {
    let _guard = POINTGET_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t (id int primary key, c enum('0', '1', '2'))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (0,'0'), (1,'1'), (2,'2')", Vec::new());
    tk.MustExec("update t set c = 2 where id = 0", Vec::new());
    tk.MustQuery("select c from t where id = 0", Vec::new())
        .Check(Rows(&["1"]));

    // Keep the direct metadata assertion from Go's enum conversion regression alongside SQL.
    let elems = vec!["0".into(), "1".into(), "2".into()];
    let updated = ParseEnumValue(&elems, 2).expect("enum index 2");
    assert_eq!(updated.Name, "1");
    assert_eq!(updated.Value, 2);
}
