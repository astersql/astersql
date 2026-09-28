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

// Go `pkg/statistics/integration_test.go` 需要 planner/executor/domain/handle。
// 这些测试放在上层 handletest crate，避免 statistics -> testkit -> statistics 循环依赖。

use crate::main_test::new_store_and_domain;
use std::time::Duration;

fn contains_pseudo(rows: &[Vec<String>]) -> bool {
    rows.iter()
        .flatten()
        .any(|cell| cell.contains("stats:pseudo"))
}

#[test]
fn go_test_null_on_full_sampling() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(a int, index idx(a))", Vec::new());
    testkit.MustExec(
        "insert into t values(1),(1),(1),(2),(2),(3),(4),(null),(null),(null)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t with 2 topn", Vec::new());
    let table = domain.table_by_name("test", "t").expect("table t");
    let stats = domain
        .stats_context()
        .physical_stats(table.ID)
        .expect("physical stats");
    assert_eq!(stats.columns.len(), 1);
    let column = stats.columns.values().next().expect("column stats");
    assert_eq!(column.null_count, 3);
    assert!(column.top_n.iter().all(|item| !item.0.is_empty()));
    assert!(column.buckets.iter().all(|bucket| !bucket.lower.is_empty()));
}

#[test]
fn go_test_analyze_snapshot() {
    let (_store, _domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(a int, index(a))", Vec::new());
    testkit.MustExec("insert into t values(1),(1),(1)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let first = testkit
        .MustQuery(
            "select count, snapshot, version from mysql.stats_meta",
            Vec::new(),
        )
        .Rows();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0][0], "3");
    let first_snapshot: u64 = first[0][1].parse().expect("snapshot");
    let version = first[0][2].clone();
    let histograms = testkit
        .MustQuery("select version from mysql.stats_histograms", Vec::new())
        .Rows();
    assert_eq!(histograms.len(), 2);
    assert!(histograms.iter().all(|row| row[0] == version));

    testkit.MustExec("insert into t values(1),(1),(1)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let second = testkit
        .MustQuery("select count, snapshot from mysql.stats_meta", Vec::new())
        .Rows();
    assert_eq!(second[0][0], "6");
    assert!(second[0][1].parse::<u64>().expect("snapshot") > first_snapshot);
}

#[test]
fn go_test_outdated_stats_check() {
    let (_store, _domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "set session tidb_enable_pseudo_for_outdated_stats=1",
        Vec::new(),
    );
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec(
        "insert into t values(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    testkit.MustExec(
        "insert into t values(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1),(1)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    let explain = testkit
        .MustQuery("explain select * from t where a = 1", Vec::new())
        .Rows();
    assert!(
        contains_pseudo(&explain),
        "expected outdated pseudo stats: {explain:?}"
    );
}

#[test]
fn go_test_show_histograms_load_status_and_single_column_index_ndv() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    domain
        .set_stats_lease(Duration::from_secs(1))
        .expect("set stats lease");
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t(a int, b int, c varchar(20), d varchar(20), index idx_a(a), index idx_b(b), index idx_c(c), index idx_d(d))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1,1,'xxx','zzz'),(2,2,'yyy','zzz'),(1,3,null,'zzz')",
        Vec::new(),
    );
    for _ in 0..5 {
        testkit.MustExec("insert into t select * from t", Vec::new());
    }
    testkit.MustExec("analyze table t", Vec::new());
    domain.update_stats().expect("refresh evicted statistics");
    let rows = testkit
        .MustQuery(
            "show stats_histograms where db_name = 'test' and table_name = 't'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 8);
    let mut results = rows
        .iter()
        .map(|row| {
            (
                row[3].clone(),
                row[6].clone(),
                row[7].clone(),
                row[10].clone(),
            )
        })
        .collect::<Vec<_>>();
    results.sort();
    assert_eq!(
        results
            .iter()
            .map(|row| (&row.0, &row.1, &row.2))
            .collect::<Vec<_>>(),
        vec![
            (&"a".into(), &"2".into(), &"0".into()),
            (&"b".into(), &"3".into(), &"0".into()),
            (&"c".into(), &"2".into(), &"32".into()),
            (&"d".into(), &"1".into(), &"0".into()),
            (&"idx_a".into(), &"2".into(), &"0".into()),
            (&"idx_b".into(), &"3".into(), &"0".into()),
            (&"idx_c".into(), &"2".into(), &"32".into()),
            (&"idx_d".into(), &"1".into(), &"0".into()),
        ]
    );
    assert!(
        results.iter().all(|row| row.3 == "allEvicted"),
        "unexpected load statuses: {results:?}"
    );
}

#[test]
fn go_test_issue_44369_and_table_last_analyze_version() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t(a int, b int, index iab(a,b))", Vec::new());
    let table = domain.table_by_name("test", "t").expect("table t");
    let before = domain
        .stats_context()
        .physical_stats(table.ID)
        .expect("initial stats");
    assert_eq!(before.last_analyze_version, 0);
    testkit.MustExec("insert into t values(1,1)", Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let analyzed = domain
        .stats_context()
        .physical_stats(table.ID)
        .expect("analyzed stats");
    assert!(analyzed.last_analyze_version > 0);
    testkit.MustExec("alter table t rename column b to bb", Vec::new());
    testkit.MustExec("select * from t where a = 10 and bb > 20", Vec::new());
}

#[test]
fn go_test_global_index_with_historical_stats() {
    let (_store, domain, mut testkit, _guard) = new_store_and_domain();
    testkit.MustExec("set tidb_analyze_version = 2", Vec::new());
    testkit.MustExec("set global tidb_enable_historical_stats = true", Vec::new());
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t(a int,b int,c int default 0) partition by range(a) (partition p0 values less than (10),partition p1 values less than (20),partition p2 values less than (30),partition p3 values less than (40))",
        Vec::new(),
    );
    testkit.MustExec("alter table t add unique index idx(b) global", Vec::new());
    testkit.MustExec(
        "insert into t(a,b) values(1,1),(2,2),(3,3),(15,15),(25,25),(35,35)",
        Vec::new(),
    );
    let table = domain.table_by_name("test", "t").expect("table t");
    for _ in 0..10 {
        testkit.MustExec("analyze table t", Vec::new());
    }
    testkit
        .MustQuery(
            &format!(
                "select count(*) from mysql.stats_history where table_id={}",
                table.ID
            ),
            Vec::new(),
        )
        .Check(vec![vec!["10"]]);
}
