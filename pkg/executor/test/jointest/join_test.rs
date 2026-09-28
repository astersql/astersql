// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 这段逻辑用于记录 jointest join 回归测试，覆盖普通 join、outer/semi join、index join、hash join、泄漏关闭、OOM 与并发 failpoint 场景。

// Join 综合回归测试：以 CaseRecorder 保留完整 Go jointest 对照，并以
// 下方可执行契约测试验证核心连接和 NULL 半连接行为。
//
// 对应 Go `pkg/executor/test/jointest/join_test.go`。覆盖普通 join、
// outer/semi join、index join、hash join、泄漏关闭、OOM 与并发 failpoint。
// Go 中依赖尚未映射到 Rust 的 failpoint/资源注入步骤仍保留为逐行对照；
// 可由现有 Rust TestKit 承担的 SQL/断言路径使用真实执行器执行。
// Semi Join：只关心右表是否存在匹配，不展开右表列；Outer Join 保留未匹配侧并填 NULL。

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

use astersql_testkit::{NewTestKit, Rows, TestKit};

fn new_join_testkit() -> TestKit {
    let store = astersql_testkit::mockstore::CreateAnalyzeStatsStore();
    NewTestKit(store)
}

/// First executable slice of Go `TestJoin2`: exercise the same outer/inner
/// join result contract through the canonical SQL test harness.
#[test]
fn test_join2_executes_the_join_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table join_contract_left(a int, b int)", Vec::new());
    tk.MustExec("create table join_contract_right(a int, b int)", Vec::new());
    tk.MustExec(
        "insert into join_contract_left values (1, 10), (2, 20)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into join_contract_right values (2, 200)",
        Vec::new(),
    );

    let outer_rows = tk
        .MustQuery(
            "select l.a, r.b from join_contract_left l left join join_contract_right r on l.a = r.a order by l.a",
            Vec::new(),
        )
        .Rows();
    assert_eq!(outer_rows, Rows(&["1 <nil>", "2 200"]));
    let inner_rows = tk
        .MustQuery(
            "select l.a, r.b from join_contract_left l join join_contract_right r on l.a = r.a",
            Vec::new(),
        )
        .Rows();
    assert_eq!(inner_rows, Rows(&["2 200"]));
}

/// Go `TestNullEmptyAwareSemiJoin` relies on SQL three-valued NULL semantics;
/// keep both the `NOT IN` empty result and the correlated `NOT EXISTS` row.
#[test]
fn test_null_empty_aware_semijoin_executes_null_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table null_semijoin(a int, b int)", Vec::new());
    tk.MustExec(
        "insert into null_semijoin values (null, 1), (1, 2)",
        Vec::new(),
    );
    tk.MustQuery(
        "select a, b from null_semijoin t1 where a not in (select b from null_semijoin t2)",
        Vec::new(),
    )
    .Check(Rows(&[]));
    tk.MustQuery(
        "select a, b from null_semijoin t1 where not exists (select 1 from null_semijoin t2 where t1.a = t2.b)",
        Vec::new(),
    )
    .Check(Rows(&["<nil> 1"]));
}

/// Go `TestIssue11895`: `BIT(64)` containing all one bits must compare equal
/// to `BIGINT UNSIGNED::MAX` and preserve its hexadecimal representation.
fn execute_issue_11895_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t(c1 bigint unsigned)", Vec::new());
    tk.MustExec("create table t1(c1 bit(64))", Vec::new());
    tk.MustExec("insert into t value(18446744073709551615)", Vec::new());
    tk.MustExec("insert into t1 value(-1)", Vec::new());
    tk.MustQuery(
        "select t.c1, hex(t1.c1) from t, t1 where t.c1 = t1.c1",
        Vec::new(),
    )
    .Check(Rows(&["18446744073709551615 FFFFFFFFFFFFFFFF"]));
}

/// Go `TestIssue11896`: signed BIGINT and BIT compare by value without
/// treating `-1` as unsigned `BIT(64)::MAX`.
fn execute_issue_11896_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t(c1 bigint)", Vec::new());
    tk.MustExec("create table t1(c1 bit(64))", Vec::new());
    tk.MustExec("insert into t value(1)", Vec::new());
    tk.MustExec("insert into t1 value(1)", Vec::new());
    tk.MustQuery(
        "select t.c1, hex(t1.c1) from t, t1 where t.c1 = t1.c1",
        Vec::new(),
    )
    .Check(Rows(&["1 1"]));

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t(c1 bigint)", Vec::new());
    tk.MustExec("create table t1(c1 bit(64))", Vec::new());
    tk.MustExec("insert into t value(-1)", Vec::new());
    tk.MustExec("insert into t1 value(18446744073709551615)", Vec::new());
    tk.MustQuery("select * from t, t1 where t.c1 = t1.c1", Vec::new())
        .Check(Rows(&[]));
}

/// Go `TestSingleTaskIncrementalIndexHashJoin`: preserve the 9/9000-row
/// fixture and all inner/outer/anti join cardinalities.
fn execute_single_task_incremental_index_hash_join_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t1(a int primary key)", Vec::new());
    tk.MustExec(
        "create table t2(b int not null, c varchar(100), index idx_b(b))",
        Vec::new(),
    );

    let t1_values = (2..=10)
        .map(|value| format!("({value})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t1 values {t1_values}"), Vec::new());
    let t2_values = (1..=9000)
        .map(|value| format!("({}, 'abc')", value / 1000))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t2 values {t2_values}"), Vec::new());

    let cases = [
        (
            "select /*+ inl_hash_join(t1,t2) */ * from t1 inner join t2 on t1.a = t2.b",
            "select /*+ inl_hash_join(t1,t2) */ count(*) from t1 inner join t2 on t1.a = t2.b",
            "7001",
        ),
        (
            "select /*+ inl_hash_join(t1,t2) */ * from t1 left join t2 on t1.a = t2.b",
            "select /*+ inl_hash_join(t1,t2) */ count(*) from t1 left join t2 on t1.a = t2.b",
            "7002",
        ),
        (
            "select /*+ inl_hash_join(t2,t1) */ * from t2 right join t1 on t1.a = t2.b",
            "select /*+ inl_hash_join(t2,t1) */ count(*) from t2 right join t1 on t1.a = t2.b",
            "7002",
        ),
        (
            "select /*+ inl_hash_join(t2,t1) */ * from t1 where t1.a not in (select t2.b from t2 where t2.b = t1.a)",
            "select /*+ inl_hash_join(t2,t1) */ count(*) from t1 where t1.a not in (select t2.b from t2 where t2.b = t1.a)",
            "1",
        ),
    ];
    for (query, count_query, expected_count) in cases {
        let _ = tk.MustQuery(query, Vec::new());
        tk.MustQuery(count_query, Vec::new())
            .Check(Rows(&[expected_count]));
    }
}

fn check_semijoin_cases(tk: &TestKit, cases: &[(&str, &[&str])]) {
    const HINTS: [&str; 5] = [
        "/*+ HASH_JOIN(t1, t2) */",
        "/*+ MERGE_JOIN(t1, t2) */",
        "/*+ INL_JOIN(t1, t2) */",
        "/*+ INL_HASH_JOIN(t1, t2) */",
        "/*+ INL_MERGE_JOIN(t1, t2) */",
    ];
    for (fragment, expected) in cases {
        for hint in HINTS {
            let sql = format!("select {hint} {fragment}");
            let actual = tk.MustQuery(&sql, Vec::new()).Rows();
            assert_eq!(Rows(expected), actual, "sql={sql}");
        }
    }
}

/// Complete executable SQL/NULL matrix from Go
/// `TestNullEmptyAwareSemiJoin`, including every join hint.
fn execute_null_empty_aware_semijoin_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t(a int, b int, c int)", Vec::new());
    tk.MustExec("insert into t values(null, 1, 0), (1, 2, 0)", Vec::new());
    let predicates = [
        "a, b from t t1 where a not in (select b from t t2)",
        "a, b from t t1 where a not in (select b from t t2 where t1.b = t2.a)",
        "a, b from t t1 where a not in (select a from t t2)",
        "a, b from t t1 where a not in (select a from t t2 where t1.b = t2.b)",
        "a, b from t t1 where a != all (select b from t t2)",
        "a, b from t t1 where a != all (select b from t t2 where t1.b = t2.a)",
        "a, b from t t1 where a != all (select a from t t2)",
        "a, b from t t1 where a != all (select a from t t2 where t1.b = t2.b)",
        "a, b from t t1 where not exists (select * from t t2 where t1.a = t2.b)",
        "a, b from t t1 where not exists (select * from t t2 where t1.a = t2.a)",
    ];
    let expected_1: [&[&str]; 10] = [
        &[],
        &["1 2"],
        &[],
        &[],
        &[],
        &["1 2"],
        &[],
        &[],
        &["<nil> 1"],
        &["<nil> 1"],
    ];
    let cases_1 = predicates
        .iter()
        .zip(expected_1)
        .map(|(sql, rows)| (*sql, rows))
        .collect::<Vec<_>>();
    check_semijoin_cases(&tk, &cases_1);

    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec("insert into t values(1, null, 0), (2, 1, 0)", Vec::new());
    let expected_2: [&[&str]; 10] = [
        &[],
        &["1 <nil>"],
        &[],
        &["1 <nil>"],
        &[],
        &["1 <nil>"],
        &[],
        &["1 <nil>"],
        &["2 1"],
        &[],
    ];
    let cases_2 = predicates
        .iter()
        .zip(expected_2)
        .map(|(sql, rows)| (*sql, rows))
        .collect::<Vec<_>>();
    check_semijoin_cases(&tk, &cases_2);

    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec(
        "insert into t values(1, null, 0), (2, 1, 0), (null, 2, 0)",
        Vec::new(),
    );
    let expected_3: [&[&str]; 10] = [
        &[],
        &["1 <nil>"],
        &[],
        &["1 <nil>"],
        &[],
        &["1 <nil>"],
        &[],
        &["1 <nil>"],
        &["<nil> 2"],
        &["<nil> 2"],
    ];
    let cases_3 = predicates
        .iter()
        .zip(expected_3)
        .map(|(sql, rows)| (*sql, rows))
        .collect::<Vec<_>>();
    check_semijoin_cases(&tk, &cases_3);

    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec("insert into t values(1, null, 0), (2, null, 0)", Vec::new());
    check_semijoin_cases(
        &tk,
        &[("a, b from t t1 where b not in (select a from t t2)", &[])],
    );

    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec(
        "insert into t values(null, 1, 1), (2, 2, 2), (3, null, 3), (4, 4, 3)",
        Vec::new(),
    );
    check_semijoin_cases(
        &tk,
        &[
            (
                "a, b, a not in (select b from t t2) from t t1 order by a",
                &["<nil> 1 <nil>", "2 2 0", "3 <nil> <nil>", "4 4 0"],
            ),
            (
                "a, c, a not in (select c from t t2) from t t1 order by a",
                &["<nil> 1 <nil>", "2 2 0", "3 3 0", "4 3 1"],
            ),
            (
                "a, b, a in (select b from t t2) from t t1 order by a",
                &["<nil> 1 <nil>", "2 2 1", "3 <nil> <nil>", "4 4 1"],
            ),
            (
                "a, c, a in (select c from t t2) from t t1 order by a",
                &["<nil> 1 <nil>", "2 2 1", "3 3 1", "4 3 0"],
            ),
        ],
    );

    tk.MustExec("create table s(a int, b int)", Vec::new());
    tk.MustExec("insert into s values(1, 2)", Vec::new());
    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec("insert into t values(null, null, 0)", Vec::new());
    check_semijoin_cases(
        &tk,
        &[
            (
                "a in (select b from t t2 where t2.a = t1.b) from s t1",
                &["0"],
            ),
            (
                "a in (select b from s t2 where t2.a = t1.b) from t t1",
                &["0"],
            ),
        ],
    );

    tk.MustExec("truncate table s", Vec::new());
    tk.MustExec("insert into s values(2, 2)", Vec::new());
    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec("insert into t values(null, 1, 0)", Vec::new());
    check_semijoin_cases(
        &tk,
        &[
            (
                "a in (select a from s t2 where t2.b = t1.b) from t t1",
                &["0"],
            ),
            (
                "a in (select a from s t2 where t2.b < t1.b) from t t1",
                &["0"],
            ),
        ],
    );

    tk.MustExec("truncate table s", Vec::new());
    tk.MustExec("insert into s values(null, 2)", Vec::new());
    tk.MustExec("truncate table t", Vec::new());
    tk.MustExec("insert into t values(1, 1, 0)", Vec::new());
    check_semijoin_cases(
        &tk,
        &[
            (
                "a in (select a from s t2 where t2.b = t1.b) from t t1",
                &["0"],
            ),
            ("b in (select a from s t2) from t t1", &["<nil>"]),
            (
                "* from t t1 where a not in (select a from s t2 where t2.b = t1.b)",
                &["1 1 0"],
            ),
            ("* from t t1 where a not in (select a from s t2)", &[]),
            ("* from s t1 where a not in (select a from t t2)", &[]),
        ],
    );

    tk.MustExec("create table t1(a int)", Vec::new());
    tk.MustExec("create table t2(a int)", Vec::new());
    tk.MustExec("insert into t1 values(1),(2)", Vec::new());
    tk.MustExec("insert into t2 values(1),(null)", Vec::new());
    for operator in ["not in", "!= all", "<> all"] {
        tk.MustQuery(
            &format!("select * from t1 where a {operator} (select a from t2 where t1.a = t2.a)"),
            Vec::new(),
        )
        .Check(Rows(&["2"]));
    }
}

/// Go `TestJoinLeak`: consume the join result after a large transactional
/// fixture and verify the statement tracker is detached afterwards.
fn execute_join_leak_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("set @@tidb_hash_join_concurrency=1", Vec::new());
    tk.MustExec("create table t(d int)", Vec::new());
    tk.MustExec("begin", Vec::new());
    for _ in 0..1002 {
        tk.MustExec("insert into t values(1)", Vec::new());
    }
    tk.MustExec("commit", Vec::new());

    let rows = tk
        .MustQuery(
            "select * from t t1 left join (select 1) t2 on 1",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 1002);
    assert!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .is_empty(),
        "join statement tracker must be detached after result consumption"
    );
    tk.MustExec("set @@tidb_hash_join_concurrency=5", Vec::new());
}

fn check_rows(tk: &TestKit, sql: &str, expected: &[&str]) {
    assert_eq!(
        Rows(expected),
        tk.MustQuery(sql, Vec::new()).Rows(),
        "sql={sql}"
    );
}

/// Executable coverage for every join family and regression branch in Go
/// `TestJoin2`. Hints are retained so planner selection and execution both
/// traverse the same public SQL boundary.
fn execute_join2_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("set @@tidb_index_lookup_join_concurrency = 200", Vec::new());
    check_rows(&tk, "select @@tidb_index_lookup_join_concurrency", &["200"]);
    tk.MustExec("set @@tidb_index_lookup_join_concurrency = 4", Vec::new());
    check_rows(&tk, "select @@tidb_index_lookup_join_concurrency", &["4"]);
    tk.MustExec("set @@tidb_index_lookup_size = 2", Vec::new());
    tk.MustExec("create table t(c int)", Vec::new());
    tk.MustExec("insert t values(1)", Vec::new());
    check_rows(&tk, "select 1 from t a left join t b on 0", &["1"]);
    check_rows(&tk, "select 1 from t a join t b on 1", &["1"]);

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("create table t(c1 int, c2 int)", Vec::new());
    tk.MustExec("create table t1(c1 int, c2 int)", Vec::new());
    tk.MustExec("insert into t values(1,1),(2,2)", Vec::new());
    tk.MustExec("insert into t1 values(2,3),(4,4)", Vec::new());
    let outer_cases = [
        (
            "select * from t left outer join t1 on t.c1=t1.c1 where t.c1=1 or t1.c2>20",
            &["1 1 <nil> <nil>"][..],
        ),
        (
            "select * from t1 right outer join t on t.c1=t1.c1 where t.c1=1 or t1.c2>20",
            &["<nil> <nil> 1 1"][..],
        ),
        (
            "select * from t right outer join t1 on t.c1=t1.c1 where t.c1=1 or t1.c2>20",
            &[][..],
        ),
        (
            "select * from t left outer join t1 on t.c1=t1.c1 where t1.c1=3 or false",
            &[][..],
        ),
        (
            "select * from t left outer join t1 on t.c1=t1.c1 and t.c1!=1 order by t1.c1",
            &["1 1 <nil> <nil>", "2 2 2 3"][..],
        ),
        (
            "select t.c1,t1.c1 from t left outer join t1 on t.c1=t1.c1 and t.c2+t1.c2<=5 order by t.c1",
            &["1 <nil>", "2 2"][..],
        ),
    ];
    for (sql, expected) in outer_cases {
        check_rows(&tk, sql, expected);
    }

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1(c1 int, c2 int)", Vec::new());
    tk.MustExec("create table t2(c1 int, c2 int)", Vec::new());
    tk.MustExec("create table t3(c1 int, c2 int)", Vec::new());
    tk.MustExec("insert into t1 values(1,1),(2,2),(3,3)", Vec::new());
    tk.MustExec("insert into t2 values(1,1),(3,3),(5,5)", Vec::new());
    tk.MustExec("insert into t3 values(1,1),(5,5),(9,9)", Vec::new());
    check_rows(
        &tk,
        "select * from t1 left join t2 on t1.c1=t2.c1 right join t3 on t2.c1=t3.c1 order by t1.c1,t1.c2,t2.c1,t2.c2,t3.c1,t3.c2",
        &[
            "<nil> <nil> <nil> <nil> 5 5",
            "<nil> <nil> <nil> <nil> 9 9",
            "1 1 1 1 1 1",
        ],
    );

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t1(c1 int)", Vec::new());
    tk.MustExec("insert into t1 values(1),(1),(1)", Vec::new());
    check_rows(
        &tk,
        "select * from t1 a join t1 b on a.c1=b.c1",
        &["1 1"; 9],
    );

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t(c1 int, index k(c1))", Vec::new());
    tk.MustExec("create table t1(c1 int)", Vec::new());
    tk.MustExec(
        "insert into t values(1),(2),(3),(4),(5),(6),(7)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values(1),(2),(3),(4),(5),(6),(7)",
        Vec::new(),
    );
    check_rows(
        &tk,
        "select a.c1 from t a,t1 b where a.c1=b.c1 order by a.c1",
        &["1", "2", "3", "4", "5", "6", "7"],
    );
    check_rows(
        &tk,
        "select a.c1 from t a,t1 b where a.c1=b.c1 and a.c1+b.c1>5 order by b.c1",
        &["3", "4", "5", "6", "7"],
    );
    check_rows(
        &tk,
        "select a.c1 from t a,(select * from t1 limit 3) b where a.c1=b.c1 order by b.c1",
        &["1", "2", "3"],
    );

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("create table t(c1 int)", Vec::new());
    tk.MustExec("create table t1(c1 int,c2 int)", Vec::new());
    tk.MustExec("create table t2(c1 int,c2 int)", Vec::new());
    tk.MustExec("insert into t values(1),(2),(3)", Vec::new());
    tk.MustExec("insert into t1 values(1,2),(2,3),(3,4)", Vec::new());
    tk.MustExec("insert into t2 values(1,0),(2,0),(3,0)", Vec::new());
    check_rows(
        &tk,
        "select * from t1,t2 where t2.c1=t1.c1 and t2.c2=0 and t1.c2 in (select * from t)",
        &["1 2 1 0", "2 3 2 0"],
    );
    check_rows(
        &tk,
        "select * from t1,t2 where t2.c1=t1.c1 and t2.c2=0 and t1.c1=1 order by t1.c2 limit 1",
        &["1 2 1 0"],
    );

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    tk.MustExec("create table t1(a int,b int,key s(b))", Vec::new());
    tk.MustExec("insert into t values(1,1),(2,2),(3,3)", Vec::new());
    tk.MustExec(
        "insert into t1 values(1,2),(1,3),(1,4),(3,4),(4,5)",
        Vec::new(),
    );
    for hint in ["INL_JOIN", "INL_HASH_JOIN", "INL_MERGE_JOIN"] {
        check_rows(
            &tk,
            &format!("select /*+ {hint}(t,t1) */ * from t join t1 on t.a=t1.a order by t.a,t1.b"),
            &["1 1 1 2", "1 1 1 3", "1 1 1 4", "3 3 3 4"],
        );
        check_rows(
            &tk,
            &format!(
                "select /*+ {hint}(t) */ * from t1 join t on t.a=t1.a and t.a<t1.b order by t1.a,t1.b"
            ),
            &["1 2 1 1", "1 3 1 1", "1 4 1 1", "3 4 3 3"],
        );
        check_rows(
            &tk,
            &format!(
                "select /*+ {hint}(t1) */ * from t right outer join t1 on t.a=t1.a order by t1.a,t1.b"
            ),
            &[
                "1 1 1 2",
                "1 1 1 3",
                "1 1 1 4",
                "3 3 3 4",
                "<nil> <nil> 4 5",
            ],
        );
        check_rows(
            &tk,
            &format!("select /*+ {hint}(t) */ avg(t.b) from t right outer join t1 on t.a=t1.a"),
            &["1.5000"],
        );
    }

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("create table t(a bigint)", Vec::new());
    tk.MustExec("insert into t values(1)", Vec::new());
    check_rows(
        &tk,
        "select t2.a,t1.a from t t1 inner join (select '1' as a) t2 on t2.a=t1.a",
        &["1 1"],
    );
    check_rows(
        &tk,
        "select t2.a,t1.a from t t1 inner join (select '2' as b,'1' as a) t2 on t2.a=t1.a",
        &["1 1"],
    );

    tk.MustExec("drop table t1", Vec::new());
    tk.MustExec("drop table t2", Vec::new());
    tk.MustExec("drop table t3", Vec::new());
    tk.MustExec("create table t1(a int,b int)", Vec::new());
    tk.MustExec("create table t2(a int,b int)", Vec::new());
    tk.MustExec("create table t3(a int,b int)", Vec::new());
    tk.MustExec("create table t4(a int,b int)", Vec::new());
    for table in ["t1", "t2", "t3", "t4"] {
        tk.MustExec(&format!("insert into {table} values(1,1)"), Vec::new());
    }
    check_rows(
        &tk,
        "select min(t2.b) from t1 right join t2 on t2.a=t1.a right join t3 on t2.a=t3.a left join t4 on t3.a=t4.a",
        &["1"],
    );
}

fn assert_memory_error(error: astersql_testkit::TestError) {
    assert!(
        error.message().to_ascii_lowercase().contains("memory"),
        "expected memory quota error, got {error}"
    );
}

fn execute_issue_18070_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("set global tidb_mem_oom_action='CANCEL'", Vec::new());
    tk.MustExec("create table t1(a int, index(a))", Vec::new());
    tk.MustExec("create table t2(a int, index(a))", Vec::new());
    tk.MustExec("insert into t1 values(1),(2)", Vec::new());
    tk.MustExec("insert into t2 values(1),(1),(2),(2)", Vec::new());
    tk.MustExec("set @@tidb_mem_quota_query=1000", Vec::new());
    assert_memory_error(
        tk.QueryToErr("select /*+ inl_hash_join(t1) */ * from t1 join t2 on t1.a=t2.a"),
    );
    let _failpoint = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/join/mockIndexMergeJoinOOMPanic",
        "panic(ERROR 1105 (HY000): Out Of Memory Quota![conn=1])",
    );
    assert_memory_error(
        tk.QueryToErr("select /*+ inl_merge_join(t1) */ * from t1 join t2 on t1.a=t2.a"),
    );
    tk.MustExec("set global tidb_mem_oom_action=default", Vec::new());
}

fn execute_issue_20779_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t1(a int,b int,index idx(b))", Vec::new());
    tk.MustExec("insert into t1 values(1,1)", Vec::new());
    tk.MustExec("insert into t1 select * from t1", Vec::new());
    let _failpoint = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/join/testIssue20779",
        "return",
    );
    let error = tk.QueryToErr(
        "select /*+ inl_hash_join(t2) */ t1.b from t1 use index(idx) left join t1 t2 use index(idx) on t1.b=t2.b order by t1.b",
    );
    assert_eq!(error.message(), "testIssue20779");
    assert!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .is_empty()
    );
}

fn execute_issue_30211_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t1(a int, index(a))", Vec::new());
    tk.MustExec("create table t2(a int, index(a))", Vec::new());
    let _outer_failpoint = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/join/TestIssue49692",
        "return",
    );
    {
        let _panic_failpoint = astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/executor/join/TestIssue30211",
            "panic(TestIssue30211 IndexJoinPanic)",
        );
        for sql in [
            "select /*+ inl_join(t1) */ * from t1 join t2 on t1.a=t2.a",
            "select /*+ inl_hash_join(t1) */ * from t1 join t2 on t1.a=t2.a",
        ] {
            assert_eq!(
                tk.QueryToErr(sql).message(),
                "failpoint panic: TestIssue30211 IndexJoinPanic"
            );
        }
    }
    tk.MustExec("insert into t1 values(1),(2)", Vec::new());
    tk.MustExec("insert into t2 values(1),(1),(2),(2)", Vec::new());
    tk.MustExec(
        "prepare stmt1 from 'select /*+ inl_join(t1) */ * from t1 join t2 on t1.a=t2.a'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare stmt2 from 'select /*+ inl_hash_join(t1) */ * from t1 join t2 on t1.a=t2.a'",
        Vec::new(),
    );
    let expected = &["1 1", "1 1", "2 2", "2 2"];
    check_rows(&tk, "execute stmt1", expected);
    check_rows(&tk, "execute stmt2", expected);
    tk.MustExec("set @@tidb_mem_quota_query=1000", Vec::new());
    tk.MustExec("set tidb_index_join_batch_size=1", Vec::new());
    tk.MustExec("set global tidb_mem_oom_action='CANCEL'", Vec::new());
    assert_memory_error(tk.QueryToErr("execute stmt1"));
    assert_memory_error(tk.QueryToErr("execute stmt2"));
    tk.MustExec("set global tidb_mem_oom_action='LOG'", Vec::new());
}

fn execute_issue_37932_contract() {
    let store = astersql_testkit::mockstore::CreateAnalyzeStatsStore();
    let mut tk1 = NewTestKit(store.clone());
    let mut tk2 = NewTestKit(store);
    tk1.MustExec("create table tbl_1(a int primary key,b int)", Vec::new());
    tk1.MustExec(
        "create table tbl_3(a int primary key,b varchar(20))",
        Vec::new(),
    );
    tk1.MustExec("insert into tbl_1 values(1,10),(2,20)", Vec::new());
    tk1.MustExec(
        "insert into tbl_3 values(1,'keep'),(2,'delete')",
        Vec::new(),
    );
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustExec("update tbl_1 set b=b+1 where a=1", Vec::new());
    tk2.MustExec("delete from tbl_3 where b='delete'", Vec::new());
    tk1.MustExec(
        "update tbl_1 set b=b+1 where a in (select a from tbl_3)",
        Vec::new(),
    );
    tk1.MustExec("commit", Vec::new());
    check_rows(&tk1, "select a,b from tbl_1 order by a", &["1 12", "2 20"]);
    check_rows(&tk2, "select a,b from tbl_3 order by a", &["1 keep"]);
}

fn execute_issue_49033_contract() {
    let mut tk = new_join_testkit();
    tk.MustExec("create table t(a int, index(a))", Vec::new());
    tk.MustExec("create table s(a int, index(a))", Vec::new());
    let values = (1..=128)
        .map(|value| format!("({value})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    tk.MustExec("insert into s values(1),(128)", Vec::new());
    tk.MustExec("set @@tidb_max_chunk_size=32", Vec::new());
    tk.MustExec("set @@tidb_index_lookup_join_concurrency=1", Vec::new());
    tk.MustExec("set @@tidb_index_join_batch_size=32", Vec::new());
    for suffix in ["", " order by t.a"] {
        check_rows(
            &tk,
            &format!("select /*+ inl_hash_join(s) */ * from t join s on t.a=s.a{suffix}"),
            &["1 1", "128 128"],
        );
    }
    let _failpoint = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/testIssue49033",
        "return",
    );
    for suffix in [" order by t.a", ""] {
        let error = tk.QueryToErr(&format!(
            "select /*+ inl_hash_join(s) */ * from t join s on t.a=s.a{suffix}"
        ));
        assert_eq!(error.message(), "testIssue49033");
    }
    assert!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .is_empty()
    );
}

// CaseStep records one action from the source test suite.
// Each step stores a source statement and a short semantic label.
/// 单步用例记录：保留一条 Go 源语句文本与语义标签。
pub struct CaseStep {
    pub go: &'static str,
    pub note: &'static str,
}

// CaseRecorder 对应 Go 的 testing.T、testkit.TestKit、failpoint 和 session harness 的组合占位。
// record appends a labeled source step without executing it.
/// 用例记录器：对应 Go testing.T / testkit / failpoint harness 的占位组合。
///
/// 只收集步骤，不真正执行 SQL；便于对照迁移进度。
pub struct CaseRecorder {
    pub name: &'static str,
    pub steps: Vec<CaseStep>,
}

impl CaseRecorder {
    /// 创建空步骤列表的记录器。
    pub fn new(name: &'static str) -> Self {
        CaseRecorder {
            name,
            steps: Vec::new(),
        }
    }

    /// 追加一条带 Go 原文与语义说明的步骤。
    pub fn record(&mut self, go: &'static str, note: &'static str) {
        // Steps are stored for review; they are not executed here.
        self.steps.push(CaseStep { go, note });
    }

    /// 追加仅含语义说明、不含 Go 原文的步骤。
    pub fn note(&mut self, note: &'static str) {
        self.record("", note);
    }
}

// record_line stores a source line with an optional semantic label.
/// 向记录器写入一行 Go 源码及可选标签（薄封装）。
fn record_line(draft: &mut CaseRecorder, go: &'static str, note: &'static str) {
    draft.record(go, note);
}

// TestJoin2 对应 Go 的同名测试，来源行 32。
// Scope:通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestJoin2 对应 Go 的同名测试，来源行 32。
///
/// 通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_join2() {
    execute_join2_contract();

    let mut draft = CaseRecorder::new(r#"TestJoin2"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestJoin2(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_lookup_join_concurrency = 200")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, 200, tk.Session().GetSessionVars().IndexLookupJoinConcurrency())"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_lookup_join_concurrency = 4")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Equal(t, 4, tk.Session().GetSessionVars().IndexLookupJoinConcurrency())"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_lookup_size = 2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t (c int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert t values (1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests := []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql    string"#, r#"source line"#);

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"select 1 from t as a left join t as b on 0","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"			testkit.Rows("1"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"select 1 from t as a join t as b on 1","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"			testkit.Rows("1"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for _, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"		result := tk.MustQuery(tt.sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"		result.Check(tt.result)"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1,1),(2,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(2,3),(4,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result := tk.MustQuery("select * from t left outer join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1 1 <nil> <nil>"))"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t1 right outer join t on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("<nil> <nil> 1 1"))"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t right outer join t1 on t.c1 = t1.c1 where t.c1 = 1 or t1.c2 > 20")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows())"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t left outer join t1 on t.c1 = t1.c1 where t1.c1 = 3 or false")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows())"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t left outer join t1 on t.c1 = t1.c1 and t.c1 != 1 order by t1.c1")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1 1 <nil> <nil>", "2 2 2 3"))"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select t.c1, t1.c1 from t left outer join t1 on t.c1 = t1.c1 and t.c2 + t1.c2 <= 5 order by t.c1")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1 <nil>", "2 2"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t3")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2 (c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t3 (c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1,1), (2,2), (3,3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values (1,1), (3,3), (5,5)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t3 values (1,1), (5,5), (9,9)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t1 left join t2 on t1.c1 = t2.c1 right join t3 on t2.c1 = t3.c1 order by t1.c1, t1.c2, t2.c1, t2.c2, t3.c1, t3.c2;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("<nil> <nil> <nil> <nil> 5 5", "<nil> <nil> <nil> <nil> 9 9", "1 1 1 1 1 1"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1 (c1 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1), (1), (1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t1 a join t1 b on a.c1 = b.c1;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1 1", "1 1", "1 1", "1 1", "1 1", "1 1", "1 1", "1 1", "1 1"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 int, index k(c1))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values (1),(2),(3),(4),(5),(6),(7)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values (1),(2),(3),(4),(5),(6),(7)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select a.c1 from t a , t1 b where a.c1 = b.c1 order by a.c1;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1", "2", "3", "4", "5", "6", "7"))"#,
        r#"source line"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test race."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select a.c1 from t a , t1 b where a.c1 = b.c1 and a.c1 + b.c1 > 5 order by b.c1")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("3", "4", "5", "6", "7"))"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select a.c1 from t a , (select * from t1 limit 3) b where a.c1 = b.c1 order by b.c1;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1", "2", "3"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t,t2,t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(c1 int, c2 int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1,2),(2,3),(3,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1,0),(2,0),(3,0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1),(2),(3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t1 , t2 where t2.c1 = t1.c1 and t2.c2 = 0 and t1.c2 in (select * from t)")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Sort().Check(testkit.Rows("1 2 1 0", "2 3 2 0"))"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select * from t1 , t2 where t2.c1 = t1.c1 and t2.c2 = 0 and t1.c1 = 1 order by t1.c2 limit 1")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Sort().Check(testkit.Rows("1 2 1 0"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int primary key, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int, key s(b))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1, 1), (2, 2), (3, 3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1, 2), (1, 3), (1, 4), (3, 4), (4, 5)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// The physical plans of the two sql are tested at physical_plan_test.go"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t, t1) */ * from t join t1 on t.a=t1.a").Check(testkit.Rows("1 1 1 2", "1 1 1 3", "1 1 1 4", "3 3 3 4"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t, t1) */ * from t join t1 on t.a=t1.a").Sort().Check(testkit.Rows("1 1 1 2", "1 1 1 3", "1 1 1 4", "3 3 3 4"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t, t1) */ * from t join t1 on t.a=t1.a").Check(testkit.Rows("1 1 1 4", "1 1 1 3", "1 1 1 2", "3 3 3 4"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t) */ * from t1 join t on t.a=t1.a and t.a < t1.b").Check(testkit.Rows("1 2 1 1", "1 3 1 1", "1 4 1 1", "3 4 3 3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t) */ * from t1 join t on t.a=t1.a and t.a < t1.b").Sort().Check(testkit.Rows("1 2 1 1", "1 3 1 1", "1 4 1 1", "3 4 3 3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t) */ * from t1 join t on t.a=t1.a and t.a < t1.b").Check(testkit.Rows("1 4 1 1", "1 3 1 1", "1 2 1 1", "3 4 3 3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test single index reader."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t, t1) */ t1.b from t1 join t on t.b=t1.b").Check(testkit.Rows("2", "3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t, t1) */ t1.b from t1 join t on t.b=t1.b").Sort().Check(testkit.Rows("2", "3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t, t1) */ t1.b from t1 join t on t.b=t1.b").Check(testkit.Rows("2", "3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t1) */ * from t right outer join t1 on t.a=t1.a").Sort().Check(testkit.Rows("1 1 1 2", "1 1 1 3", "1 1 1 4", "3 3 3 4", "<nil> <nil> 4 5"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t1) */ * from t right outer join t1 on t.a=t1.a").Sort().Check(testkit.Rows("1 1 1 2", "1 1 1 3", "1 1 1 4", "3 3 3 4", "<nil> <nil> 4 5"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t1) */ * from t right outer join t1 on t.a=t1.a").Sort().Check(testkit.Rows("1 1 1 2", "1 1 1 3", "1 1 1 4", "3 3 3 4", "<nil> <nil> 4 5"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t) */ avg(t.b) from t right outer join t1 on t.a=t1.a").Check(testkit.Rows("1.5000"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t) */ avg(t.b) from t right outer join t1 on t.a=t1.a").Check(testkit.Rows("1.5000"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t) */ avg(t.b) from t right outer join t1 on t.a=t1.a").Check(testkit.Rows("1.5000"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// Test that two conflict hints will return warning."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("select /*+ TIDB_INLJ(t) TIDB_SMJ(t) */ * from t join t1 on t.a=t1.a")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("select /*+ TIDB_INLJ(t) TIDB_HJ(t) */ * from t join t1 on t.a=t1.a")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("select /*+ TIDB_SMJ(t) TIDB_HJ(t) */ * from t join t1 on t.a=t1.a")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1),(2), (3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select @a := @a + 1 from t, (select @a := 0) b;").Check(testkit.Rows("1", "2", "3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int primary key, b int, key s(b))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1, 3), (2, 2), (3, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(0, 0), (1, 2), (1, 3), (3, 4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t1) */ * from t join t1 on t.a=t1.a order by t.b").Sort().Check(testkit.Rows("1 3 1 2", "1 3 1 3", "3 1 3 4"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t1) */ * from t join t1 on t.a=t1.a order by t.b").Sort().Check(testkit.Rows("1 3 1 2", "1 3 1 3", "3 1 3 4"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t1) */ * from t join t1 on t.a=t1.a order by t.b").Sort().Check(testkit.Rows("1 3 1 2", "1 3 1 3", "3 1 3 4"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t) */ t.a, t.b from t join t1 on t.a=t1.a where t1.b = 4 limit 1").Check(testkit.Rows("3 1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t) */ t.a, t.b from t join t1 on t.a=t1.a where t1.b = 4 limit 1").Check(testkit.Rows("3 1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t) */ t.a, t.b from t join t1 on t.a=t1.a where t1.b = 4 limit 1").Check(testkit.Rows("3 1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t, t1) */ * from t right join t1 on t.a=t1.a order by t.b").Sort().Check(testkit.Rows("1 3 1 2", "1 3 1 3", "3 1 3 4", "<nil> <nil> 0 0"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t, t1) */ * from t right join t1 on t.a=t1.a order by t.b").Sort().Check(testkit.Rows("1 3 1 2", "1 3 1 3", "3 1 3 4", "<nil> <nil> 0 0"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t, t1) */ * from t right join t1 on t.a=t1.a order by t.b").Sort().Check(testkit.Rows("1 3 1 2", "1 3 1 3", "3 1 3 4", "<nil> <nil> 0 0"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// join reorder will disorganize the resulting schema"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1,2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(3,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select (select t1.a from t1 , t where t.a = s.a limit 2) from t as s").Check(testkit.Rows("3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// test index join bug"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int, b int, key s1(a,b), key s2(b))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1,2), (5,3), (6,4)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1), (2), (3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_JOIN(t) */ t1.a from t1, t where t.a = 5 and t.b = t1.a").Check(testkit.Rows("3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(t) */ t1.a from t1, t where t.a = 5 and t.b = t1.a").Check(testkit.Rows("3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_MERGE_JOIN(t) */ t1.a from t1, t where t.a = 5 and t.b = t1.a").Check(testkit.Rows("3"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// test issue#4997"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	CREATE TABLE t1 ("#, r#"source line"#);

    record_line(
        &mut draft,
        r#"  		pk int(11) NOT NULL AUTO_INCREMENT primary key,"#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"  		a int(11) DEFAULT NULL,"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"  		b date DEFAULT NULL,"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"  		c varchar(1) DEFAULT NULL,"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"  		KEY a (a),"#, r#"source line"#);

    record_line(&mut draft, r#"  		KEY b (b),"#, r#"source line"#);

    record_line(&mut draft, r#"  		KEY c (c,a)"#, r#"source line"#);

    record_line(&mut draft, r#"	)`)"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	CREATE TABLE t2 ("#, r#"source line"#);

    record_line(
        &mut draft,
        r#"  		pk int(11) NOT NULL AUTO_INCREMENT primary key,"#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"  		a int(11) DEFAULT NULL,"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"  		b date DEFAULT NULL,"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"  		c varchar(1) DEFAULT NULL,"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"  		KEY a (a),"#, r#"source line"#);

    record_line(&mut draft, r#"  		KEY b (b),"#, r#"source line"#);

    record_line(&mut draft, r#"  		KEY c (c,a)"#, r#"source line"#);

    record_line(&mut draft, r#"	)`)"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t1 value(1,1,"2000-11-11", null);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery(`"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	SELECT table2.b AS field2 FROM"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"	("#, r#"source line"#);

    record_line(
        &mut draft,
        r#"	  t1 AS table1  LEFT OUTER JOIN"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"		(SELECT tmp_t2.* FROM ( t2 AS tmp_t1 RIGHT JOIN t1 AS tmp_t2 ON (tmp_t2.a = tmp_t1.a))) AS table2"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	  ON (table2.c = table1.c)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"	) `)"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("<nil>"))"#,
        r#"source line"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// test virtual rows are included (issue#5771)"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery(`SELECT 1 FROM (SELECT 1) t1, (SELECT 1) t2`)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1"))"#,
        r#"source line"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery(`"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"		SELECT @NUM := @NUM + 1 as NUM FROM"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		( SELECT 1 UNION ALL"#, r#"source line"#);

    record_line(&mut draft, r#"			SELECT 2 UNION ALL"#, r#"source line"#);

    record_line(&mut draft, r#"			SELECT 3"#, r#"source line"#);

    record_line(&mut draft, r#"		) a"#, r#"source line"#);

    record_line(&mut draft, r#"		INNER JOIN"#, r#"source line"#);

    record_line(&mut draft, r#"		( SELECT 1 UNION ALL"#, r#"source line"#);

    record_line(&mut draft, r#"			SELECT 2 UNION ALL"#, r#"source line"#);

    record_line(&mut draft, r#"			SELECT 3"#, r#"source line"#);

    record_line(&mut draft, r#"		) b,"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"		(SELECT @NUM := 0) d;"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"	`)"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1", "2", "3", "4", "5", "6", "7", "8", "9"))"#,
        r#"source line"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	// This case is for testing:"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	// when the main thread calls Executor.Close() while the out data fetch worker and join workers are still working,"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// we need to stop the goroutines as soon as possible to avoid unexpected error."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_hash_join_concurrency=5")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for range 100 {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec("insert into t value(1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	result = tk.MustQuery("select /*+ TIDB_HJ(s, r) */ * from t as s join t as r on s.a = r.a limit 1;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"	result.Check(testkit.Rows("1 1"))"#,
        r#"source line"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists user, aa, bb")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table aa(id int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into aa values(1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table bb(id int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into bb values(1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table user(id int, name varchar(20))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into user values(1, 'a'), (2, 'b')")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select user.id,user.name from user left join aa on aa.id = user.id left join bb on aa.id = bb.id where bb.id < 10;").Check(testkit.Rows("1 a"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`drop table if exists t;`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`create table t (a bigint);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(`insert into t values (1);`)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery(`select t2.a, t1.a from t t1 inner join (select "1" as a) t2 on t2.a = t1.a;`).Check(testkit.Rows("1 1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery(`select t2.a, t1.a from t t1 inner join (select "2" as b, "1" as a) t2 on t2.a = t1.a;`).Check(testkit.Rows("1 1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2, t3, t4")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t3(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t4(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t3 values(1, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t4 values(1, 1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select min(t2.b) from t1 right join t2 on t2.a=t1.a right join t3 on t2.a=t3.a left join t4 on t3.a=t4.a").Check(testkit.Rows("1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestJoinLeak 对应 Go 的同名测试，来源行 290。
// Scope:通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestJoinLeak 对应 Go 的同名测试，来源行 290。
///
/// 通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_join_leak() {
    execute_join_leak_contract();

    let mut draft = CaseRecorder::new(r#"TestJoinLeak"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestJoinLeak(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_hash_join_concurrency=1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t (d int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("begin")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for range 1002 {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"		tk.MustExec("insert t values (1)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("commit")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	result, err := tk.Exec("select * from t t1 left join (select 1) t2 on 1")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	req := result.NewChunk(nil)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	err = result.Next(context.Background(), req)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 时间等待点：Go 测试依赖短暂等待触发后台巡检或 kill 逻辑。
    record_line(
        &mut draft,
        r#"	time.Sleep(time.Millisecond)"#,
        r#"时间等待点：Go 测试依赖短暂等待触发后台巡检或 kill 逻辑。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, result.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_hash_join_concurrency=5")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestNullEmptyAwareSemiJoin 对应 Go 的同名测试，来源行 313。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestNullEmptyAwareSemiJoin 对应 Go 的同名测试，来源行 313。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_null_empty_aware_semi_join() {
    execute_null_empty_aware_semijoin_contract();

    let mut draft = CaseRecorder::new(r#"TestNullEmptyAwareSemiJoin"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestNullEmptyAwareSemiJoin(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int, b int, c int, index idx_a(a), index idb_b(b), index idx_c(c))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(null, 1, 0), (1, 2, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests := []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql string"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a not in (select b from t t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a not in (select b from t t2 where t1.b = t2.a)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a not in (select a from t t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a not in (select a from t t2 where t1.b = t2.b)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a != all (select b from t t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a != all (select b from t t2 where t1.b = t2.a)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a != all (select a from t t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a, b from t t1 where a != all (select a from t t2 where t1.b = t2.b)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a, b from t t1 where not exists (select * from t t2 where t1.a = t2.b)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a, b from t t1 where not exists (select * from t t2 where t1.a = t2.a)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results := []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("1 2"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("1 2"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("<nil> 1"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("<nil> 1"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	hints := [5]string{"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(
        &mut draft,
        r#"		"/*+ HASH_JOIN(t1, t2) */","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"/*+ MERGE_JOIN(t1, t2) */","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"/*+ INL_JOIN(t1, t2) */","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"/*+ INL_HASH_JOIN(t1, t2) */","#,
        r#"source line"#,
    );

    record_line(
        &mut draft,
        r#"		"/*+ INL_MERGE_JOIN(t1, t2) */","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1, null, 0), (2, 1, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("2 1"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1, null, 0), (2, 1, 0), (null, 2, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("1 <nil>"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("<nil> 2"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			testkit.Rows("<nil> 2"),"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1, null, 0), (2, null, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql string"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b from t t1 where b not in (select a from t t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(null, 1, 1), (2, 2, 2), (3, null, 3), (4, 4, 3)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql string"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b, a not in (select b from t t2) from t t1 order by a","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, c, a not in (select c from t t2) from t t1 order by a","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, b, a in (select b from t t2) from t t1 order by a","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a, c, a in (select c from t t2) from t t1 order by a","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("#, r#"source line"#);

    record_line(&mut draft, r#"				"<nil> 1 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"				"2 2 0","#, r#"source line"#);

    record_line(&mut draft, r#"				"3 <nil> <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"				"4 4 0","#, r#"source line"#);

    record_line(&mut draft, r#"			),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("#, r#"source line"#);

    record_line(&mut draft, r#"				"<nil> 1 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"				"2 2 0","#, r#"source line"#);

    record_line(&mut draft, r#"				"3 3 0","#, r#"source line"#);

    record_line(&mut draft, r#"				"4 3 1","#, r#"source line"#);

    record_line(&mut draft, r#"			),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("#, r#"source line"#);

    record_line(&mut draft, r#"				"<nil> 1 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"				"2 2 1","#, r#"source line"#);

    record_line(&mut draft, r#"				"3 <nil> <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"				"4 4 1","#, r#"source line"#);

    record_line(&mut draft, r#"			),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("#, r#"source line"#);

    record_line(&mut draft, r#"				"<nil> 1 <nil>","#, r#"source line"#);

    record_line(&mut draft, r#"				"2 2 1","#, r#"source line"#);

    record_line(&mut draft, r#"				"3 3 1","#, r#"source line"#);

    record_line(&mut draft, r#"				"4 3 0","#, r#"source line"#);

    record_line(&mut draft, r#"			),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table s(a int, b int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into s values(1, 2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(null, null, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql string"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a in (select b from t t2 where t2.a = t1.b) from s t1","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a in (select b from s t2 where t2.a = t1.b) from t t1","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("0"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("0"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into s values(2, 2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(null, 1, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql string"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a in (select a from s t2 where t2.b = t1.b) from t t1","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"a in (select a from s t2 where t2.b < t1.b) from t t1","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("0"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("0"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table s")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into s values(null, 2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("truncate table t")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1, 1, 0)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tests = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		sql string"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"a in (select a from s t2 where t2.b = t1.b) from t t1","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"b in (select a from s t2) from t t1","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			"* from t t1 where a not in (select a from s t2 where t2.b = t1.b)","#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"* from t t1 where a not in (select a from s t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"			"* from s t1 where a not in (select a from t t2)","#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	results = []struct {"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );

    record_line(&mut draft, r#"		result [][]any"#, r#"source line"#);

    record_line(&mut draft, r#"	}{"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("0"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("<nil>"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows("1 1 0"),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"		{"#, r#"source line"#);

    record_line(&mut draft, r#"			testkit.Rows(),"#, r#"source line"#);

    record_line(&mut draft, r#"		},"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i, tt := range tests {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"		for _, hint := range hints {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"			sql := fmt.Sprintf("select %s %s", hint, tt.sql)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"			result := tk.MustQuery(sql)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(
        &mut draft,
        r#"			result.Check(results[i].result)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(a int)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1),(2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1),(null)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select * from t1 where a not in (select a from t2 where t1.a = t2.a)").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"		"2","#, r#"source line"#);

    record_line(&mut draft, r#"	))"#, r#"source line"#);
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select * from t1 where a != all (select a from t2 where t1.a = t2.a)").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"		"2","#, r#"source line"#);

    record_line(&mut draft, r#"	))"#, r#"source line"#);
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select * from t1 where a <> all (select a from t2 where t1.a = t2.a)").Check(testkit.Rows("#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"		"2","#, r#"source line"#);

    record_line(&mut draft, r#"	))"#, r#"source line"#);

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue18070 对应 Go 的同名测试，来源行 708。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。
/// TestIssue18070 对应 Go 的同名测试，来源行 708。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。
#[test]
pub fn test_issue18070() {
    execute_issue_18070_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue18070"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。"#);

    record_line(
        &mut draft,
        r#"func TestIssue18070(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	defer tk.MustExec("SET GLOBAL tidb_mem_oom_action = DEFAULT")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("SET GLOBAL tidb_mem_oom_action='CANCEL'")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, index(a))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(a int, index(a))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1),(2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1),(1),(2),(2)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_mem_quota_query=1000")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 错误路径断言：Go 测试期望执行语句返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.ExecToErr("select /*+ inl_hash_join(t1)*/ * from t1 join t2 on t1.a = t2.a;")"#,
        r#"错误路径断言：Go 测试期望执行语句返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, exeerrors.ErrMemoryExceedForQuery.Equal(err))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName := "github.com/pingcap/tidb/pkg/executor/join/mockIndexMergeJoinOOMPanic""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName, `panic("ERROR 1105 (HY000): Out Of Memory Quota![conn=1]")`))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 错误路径断言：Go 测试期望执行语句返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.ExecToErr("select /*+ inl_merge_join(t1)*/ * from t1 join t2 on t1.a = t2.a;")"#,
        r#"错误路径断言：Go 测试期望执行语句返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, exeerrors.ErrMemoryExceedForQuery.Equal(err))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue20779 对应 Go 的同名测试，来源行 732。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestIssue20779 对应 Go 的同名测试，来源行 732。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_issue20779() {
    execute_issue_20779_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue20779"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestIssue20779(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, b int, index idx(b));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1, 1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 select * from t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/join/testIssue20779", "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/join/testIssue20779"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	rs, err := tk.Exec("select /*+ inl_hash_join(t2) */ t1.b from t1 use index(idx) left join t1 t2 use index(idx) on t1.b=t2.b order by t1.b;")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。
    record_line(
        &mut draft,
        r#"	_, err = session.GetRows4Test(context.Background(), nil, rs)"#,
        r#"结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "testIssue20779")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, rs.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue30211 对应 Go 的同名测试，来源行 753。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。
/// TestIssue30211 对应 Go 的同名测试，来源行 753。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。
#[test]
pub fn test_issue30211() {
    execute_issue_30211_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue30211"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。"#);

    record_line(
        &mut draft,
        r#"func TestIssue30211(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int, index(a));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(a int, index(a));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	fpName2 := "github.com/pingcap/tidb/pkg/executor/join/TestIssue49692""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable(fpName2, `return`))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable(fpName2))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);

    record_line(&mut draft, r#"	func() {"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"		fpName := "github.com/pingcap/tidb/pkg/executor/join/TestIssue30211""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Enable(fpName, `panic("TestIssue30211 IndexJoinPanic")`))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"		defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"			require.NoError(t, failpoint.Disable(fpName))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"		}()"#, r#"source line"#);
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"		err := tk.QueryToErr("select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.a;")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"		require.EqualError(t, err, "failpoint panic: TestIssue30211 IndexJoinPanic")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"		err = tk.QueryToErr("select /*+ inl_hash_join(t1) */ * from t1 join t2 on t1.a = t2.a;")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"		require.EqualError(t, err, "failpoint panic: TestIssue30211 IndexJoinPanic")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 values(1),(2);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t2 values(1),(1),(2),(2);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// the memory used in planner stage is less than the memory used in executor stage, so we have to use"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// the Plan Cache so that the query will not be canceled during compilation."#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("prepare stmt1 from 'select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.a';")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("prepare stmt2 from 'select /*+ inl_hash_join(t1) */ * from t1 join t2 on t1.a = t2.a';")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("execute stmt1;").Check(testkit.Rows("1 1", "1 1", "2 2", "2 2"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("execute stmt2;").Check(testkit.Rows("1 1", "1 1", "2 2", "2 2"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_mem_quota_query=1000;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set tidb_index_join_batch_size = 1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("SET GLOBAL tidb_mem_oom_action = 'CANCEL'")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	defer tk.MustExec("SET GLOBAL tidb_mem_oom_action='LOG'")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk.QueryToErr("execute stmt1")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, exeerrors.ErrMemoryExceedForQuery.Equal(err))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 错误路径断言：Go 测试期望查询返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err = tk.QueryToErr("execute stmt2")"#,
        r#"错误路径断言：Go 测试期望查询返回特定错误或错误类型。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.True(t, exeerrors.ErrMemoryExceedForQuery.Equal(err))"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue37932 对应 Go 的同名测试，来源行 798。
// Scope:通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。
/// TestIssue37932 对应 Go 的同名测试，来源行 798。
///
/// 通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。
#[test]
pub fn test_issue37932() {
    execute_issue_37932_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue37932"#);
    draft.note(
        r#"通过 testkit 执行 SQL fixture 和结果断言；覆盖错误返回路径；涉及内存配额或 OOM 行为。"#,
    );

    record_line(
        &mut draft,
        r#"func TestIssue37932(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk1 := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk2 := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk2.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("create table tbl_1 ( col_1 set ( 'Alice','Bob','Charlie','David' )   not null default 'Alice' ,col_2 tinyint  unsigned ,col_3 decimal ( 34 , 3 )   not null default 79 ,col_4 bigint  unsigned not null ,col_5 bit ( 12 )   not null , unique key idx_1 ( col_2 ) ,unique key idx_2 ( col_2 ) ) charset utf8mb4 collate utf8mb4_bin ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("create table tbl_2 ( col_6 text ( 52 ) collate utf8_unicode_ci  not null ,col_7 int  unsigned not null ,col_8 blob ( 369 ) ,col_9 bit ( 51 ) ,col_10 decimal ( 38 , 16 ) , unique key idx_3 ( col_7 ) ,unique key idx_4 ( col_7 ) ) charset utf8 collate utf8_unicode_ci ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("create table tbl_3 ( col_11 set ( 'Alice','Bob','Charlie','David' )   not null ,col_12 bigint  unsigned not null default 1678891638492596595 ,col_13 text ( 18 ) ,col_14 set ( 'Alice','Bob','Charlie','David' )   not null default 'Alice' ,col_15 mediumint , key idx_5 ( col_12 ) ,unique key idx_6 ( col_12 ) ) charset utf8mb4 collate utf8mb4_general_ci ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("create table tbl_4 ( col_16 set ( 'Alice','Bob','Charlie','David' )   not null ,col_17 tinyint  unsigned ,col_18 int  unsigned not null default 4279145838 ,col_19 varbinary ( 210 )   not null ,col_20 timestamp , primary key  ( col_18 ) /*T![clustered_index] nonclustered */ ,key idx_8 ( col_19 ) ) charset utf8mb4 collate utf8mb4_unicode_ci ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("create table tbl_5 ( col_21 bigint ,col_22 set ( 'Alice','Bob','Charlie','David' ) ,col_23 blob ( 311 ) ,col_24 bigint  unsigned not null default 3415443099312152509 ,col_25 time , unique key idx_9 ( col_21 ) ,unique key idx_10 ( col_21 ) ) charset gbk collate gbk_bin ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Bob',null,0.04,2650749963804575036,4044 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Alice',171,1838.2,6452757231340518222,1190 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Bob',202,2.962,4304284252076747481,2112 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'David',155,32610.05,5899651588546531414,104 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Charlie',52,4219.7,6151233689319516187,1246 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Bob',55,3963.11,3614977408465893392,1188 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Alice',203,72.01,1553550133494908281,1658 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Bob',40,871.569,8114062926218465773,1397 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'Alice',165,7765,4481202107781982005,2089 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_1 values ( 'David',79,7.02,993594504887208796,514 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( 'iB_%7c&q!6-gY4bkvg',2064909882,'dLN52t1YZSdJ',2251679806445488,32 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( 'h_',1478443689,'EqP+iN=',180492371752598,0.1 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( 'U@U&*WKfPzil=6YaDxp',4271201457,'QWuo24qkSSo',823931105457505,88514 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( 'FR4GA=',505128825,'RpEmV6ph5Z7',568030123046798,609381 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( '3GsU',166660047,'',1061132816887762,6.4605 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( 'BA4hPRD0lm*pbg#NE',3440634757,'7gUPe2',288001159469205,6664.9 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( '+z',2117152318,'WTkD(N',215697667226264,7.88 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( 'x@SPhy9lOomPa4LF',2881759652,'ETUXQQ0b4HnBSKgTWIU',153379720424625,null );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( '',2075177391,'MPae!9%ufd',115899580476733,341.23 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_2 values ( '~udi',1839363347,'iQj$$YsZc5ULTxG)yH',111454353417190,6.6 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Alice',7032411265967085555,'P7*KBZ159','Alice',7516989 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'David',486417871670147038,'','Charlie',-2135446 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Charlie',5784081664185069254,'7V_&YzKM~Q','Charlie',5583839 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'David',6346366522897598558,')Lp&$2)SC@','Bob',2522913 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Charlie',224922711063053272,'gY','David',6624398 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Alice',4678579167560495958,'fPIXY%R8WyY(=u&O','David',-3267160 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'David',8817108026311573677,'Cs0dZW*SPnKhV1','Alice',2359718 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Bob',3177426155683033662,'o2=@zv2qQDhKUs)4y','Bob',-8091802 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Bob',2543586640437235142,'hDa*CsOUzxmjf2m','Charlie',-8091935 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_3 values ( 'Charlie',6204182067887668945,'DX-!=)dbGPQO','David',-1954600 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'David',167,576262750,'lX&x04W','2035-09-28' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Charlie',236,2637776757,'92OhsL!w%7','2036-02-08' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Bob',68,1077999933,'M0l','1997-09-16' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Charlie',184,1280264753,'FhjkfeXsK1Q(','2030-03-16' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Alice',10,2150711295,'Eqip)^tr*MoL','2032-07-02' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Bob',108,2421602476,'Eul~~Df_Q8s&I3Y-7','2019-06-10' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Alice',36,2811198561,'%XgRou0#iKtn*','2022-06-13' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Charlie',115,330972286,'hKeJS','2000-11-15' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Alice',6,2958326555,'c6+=1','2001-02-11' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_4 values ( 'Alice',99,387404826,'figc(@9R*k3!QM_Vve','2036-02-17' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( -401358236474313609,'Charlie','4J$',701059766304691317,'08:19:10.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( 2759837898825557143,'Bob','E',5158554038674310466,'11:04:03.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( 273910054423832204,'Alice',null,8944547065167499612,'08:02:30.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( 2875669873527090798,'Alice','4^SpR84',4072881341903432150,'18:24:55.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( -8446590100588981557,'David','yBj8',8760380566452862549,'09:01:10.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( -1075861460175889441,'Charlie','ti11Pl0lJ',9139997565676405627,'08:30:14.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( 95663565223131772,'Alice','6$',8467839300407531400,'23:31:42.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( -5661709703968335255,'Charlie','',8122758569495329946,'19:36:24.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( 3338588216091909518,'Bob','',6558557574025196860,'15:22:56.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert into tbl_5 values ( 8918630521194612922,'David','I$w',5981981639362947650,'22:03:24.00' );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("begin pessimistic;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert ignore into tbl_1 set col_1 = 'David', col_2 = 110, col_3 = 37065, col_4 = 8164500960513474805, col_5 = 1264 on duplicate key update col_3 = 22151.5, col_4 = 6266058887081523571, col_5 = 3254, col_2 = 59, col_1 = 'Bob';")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("insert  into tbl_4 (col_16,col_17,col_18,col_19,col_20) values ( 'Charlie',34,2499970462,'Z','1978-10-27' ) ,( 'David',217,1732485689,'*)~@@Q8ryi','2004-12-01' ) ,( 'Charlie',40,1360558255,'H(Y','1998-06-25' ) ,( 'Alice',108,2973455447,'%CcP4$','1979-03-28' ) ,( 'David',9,3835209932,'tdKXUzLmAzwFf$','2009-03-03' ) ,( 'David',68,163270003,'uimsclz@FQJN','1988-09-11' ) ,( 'Alice',76,297067264,'BzFF','1989-01-05' ) on duplicate key update col_16 = 'Charlie', col_17 = 14, col_18 = 4062155275, col_20 = '2002-03-07', col_19 = 'tmvchLzp*o8';")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk2.MustExec("delete from tbl_3 where tbl_3.col_13 in ( null ,'' ,'g8EEzUU7LQ' ,'~fC3&B*cnOOx_' ,'%RF~AFto&x' ,'NlWkMWG^00' ,'e^4o2Ji^q_*Fa52Z' ) ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk2.MustExec("delete from tbl_5 where not( tbl_5.col_21 between -1075861460175889441 and 3338588216091909518 ) ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("replace into tbl_1 (col_1,col_2,col_3,col_4,col_5) values ( 'Alice',83,8.33,4070808626051569664,455 ) ,( 'Alice',53,2.8,2763362085715461014,1912 ) ,( 'David',178,4242.8,962727993466011464,1844 ) ,( 'Alice',16,650054,5638988670318229867,565 ) ,( 'Alice',76,89783.1,3968605744540056024,2563 ) ,( 'Bob',120,0.89,1003144931151245839,2670 );")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("delete from tbl_5 where col_24 is null ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk1.MustExec("delete from tbl_3 where tbl_3.col_11 in ( 'Alice' ,'Bob' ,'Alice' ) ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk2.MustExec("insert  into tbl_3 set col_11 = 'Bob', col_12 = 5701982550256146475, col_13 = 'Hhl)yCsQ2K3cfc^', col_14 = 'Alice', col_15 = -3718868 on duplicate key update col_15 = 7210750, col_12 = 6133680876296985245, col_14 = 'Alice', col_11 = 'David', col_13 = 'F+RMGE!_2^Cfr3Fw';")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk2.MustExec("insert ignore into tbl_5 set col_21 = 2439343116426563397, col_22 = 'Charlie', col_23 = '~Spa2YzRFFom16XD', col_24 = 5571575017340582365, col_25 = '13:24:38.00' ;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 错误路径断言：Go 测试期望执行语句返回特定错误或错误类型。
    record_line(
        &mut draft,
        r#"	err := tk1.ExecToErr("update tbl_4 set tbl_4.col_20 = '2006-01-24' where tbl_4.col_18 in ( select col_11 from tbl_3 where IsNull( tbl_4.col_16 ) or not( tbl_4.col_19 in ( select col_3 from tbl_1 where tbl_4.col_16 between 'Alice' and 'David' and tbl_4.col_19 <= '%XgRou0#iKtn*' ) ) ) ;")"#,
        r#"错误路径断言：Go 测试期望执行语句返回特定错误或错误类型。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"	if err != nil {"#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );

    record_line(&mut draft, r#"		print(err.Error())"#, r#"source line"#);
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"		if strings.Contains(err.Error(), "Truncated incorrect DOUBLE value") {"#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );

    record_line(
        &mut draft,
        r#"			t.Log("Truncated incorrect DOUBLE value is within expectations, skipping")"#,
        r#"source line"#,
    );
    // 返回值语义：保留 Go 辅助函数交还 testkit、错误或 fixture 的位置。
    record_line(
        &mut draft,
        r#"			return"#,
        r#"返回值语义：保留 Go 辅助函数交还 testkit、错误或 fixture 的位置。"#,
    );

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue49033 对应 Go 的同名测试，来源行 880。
// Scope:包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
/// TestIssue49033 对应 Go 的同名测试，来源行 880。
///
/// 包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。
#[test]
pub fn test_issue49033() {
    execute_issue_49033_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue49033"#);
    draft.note(r#"包含 failpoint 注入和清理；通过 testkit 执行 SQL fixture 和结果断言；显式检查结果集拉取与资源收尾。"#);

    record_line(
        &mut draft,
        r#"func TestIssue49033(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	val := runtime.GOMAXPROCS(1)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );

    record_line(&mut draft, r#"		runtime.GOMAXPROCS(val)"#, r#"source line"#);

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t, s;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(a int, index(a));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table s(a int, index(a));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t values(1), (2), (3), (4), (5), (6), (7), (8), (9), (10), (11), (12), (13), (14), (15), (16), (17), (18), (19), (20), (21), (22), (23), (24), (25), (26), (27), (28), (29), (30), (31), (32), (33), (34), (35), (36), (37), (38), (39), (40), (41), (42), (43), (44), (45), (46), (47), (48), (49), (50), (51), (52), (53), (54), (55), (56), (57), (58), (59), (60), (61), (62), (63), (64), (65), (66), (67), (68), (69), (70), (71), (72), (73), (74), (75), (76), (77), (78), (79), (80), (81), (82), (83), (84), (85), (86), (87), (88), (89), (90), (91), (92), (93), (94), (95), (96), (97), (98), (99), (100), (101), (102), (103), (104), (105), (106), (107), (108), (109), (110), (111), (112), (113), (114), (115), (116), (117), (118), (119), (120), (121), (122), (123), (124), (125), (126), (127), (128);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into s values(1), (128);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_max_chunk_size=32;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_lookup_join_concurrency=1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("set @@tidb_index_join_batch_size=32;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(s) */ * from t join s on t.a=s.a;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ INL_HASH_JOIN(s) */ * from t join s on t.a=s.a order by t.a;")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。
    record_line(
        &mut draft,
        r#"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/executor/testIssue49033", "return"))"#,
        r#"Failpoint 开启点：Go 测试通过外部注入模拟 executor、planner 或 worker 的异常路径。"#,
    );
    // defer 语义：Rust 这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。
    record_line(
        &mut draft,
        r#"	defer func() {"#,
        r#"defer 语义：这里只记录 Go 的延迟恢复/清理动作，不实现 RAII 接线。"#,
    );
    // Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。
    record_line(
        &mut draft,
        r#"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/executor/testIssue49033"))"#,
        r#"Failpoint 收尾点：Go 测试在 defer 或显式分支中关闭注入，避免污染后续用例。"#,
    );

    record_line(&mut draft, r#"	}()"#, r#"source line"#);
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	rs, err := tk.Exec("select /*+ INL_HASH_JOIN(s) */ * from t join s on t.a=s.a order by t.a;")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。
    record_line(
        &mut draft,
        r#"	_, err = session.GetRows4Test(context.Background(), nil, rs)"#,
        r#"结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "testIssue49033")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, rs.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	rs, err = tk.Exec("select /*+ INL_HASH_JOIN(s) */ * from t join s on t.a=s.a")"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.NoError(t, err)"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。
    record_line(
        &mut draft,
        r#"	_, err = session.GetRows4Test(context.Background(), nil, rs)"#,
        r#"结果集拉取路径：Go 测试显式消费 rows，用来触发异步 worker 或 Close 错误。"#,
    );
    // require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。
    record_line(
        &mut draft,
        r#"	require.EqualError(t, err, "testIssue49033")"#,
        r#"require 断言：保留错误、长度、正则、相等性或布尔条件的检查语义。"#,
    );
    // 资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。
    record_line(
        &mut draft,
        r#"	require.NoError(t, rs.Close())"#,
        r#"资源收尾：Go 测试显式关闭 result set，验证 worker/row container 能正确释放。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue11895 对应 Go 的同名测试，来源行 917。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue11895 对应 Go 的同名测试，来源行 917。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue11895() {
    execute_issue_11895_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue11895"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue11895(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 bigint unsigned);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 bit(64));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t value(18446744073709551615);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 value(-1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select t.c1, hex(t1.c1) from t, t1 where t.c1 = t1.c1;").Check(testkit.Rows("18446744073709551615 FFFFFFFFFFFFFFFF"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestIssue11896 对应 Go 的同名测试，来源行 931。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestIssue11896 对应 Go 的同名测试，来源行 931。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_issue11896() {
    execute_issue_11896_contract();

    let mut draft = CaseRecorder::new(r#"TestIssue11896"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestIssue11896(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 bigint);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 bit(64));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t value(1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 value(1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select t.c1, hex(t1.c1) from t, t1 where t.c1 = t1.c1;").Check(testkit.Rows("1 1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1;")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t(c1 bigint);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(c1 bit(64));")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t value(-1);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("insert into t1 value(18446744073709551615);")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select * from t, t1 where t.c1 = t1.c1;").Check(nil)"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}

// TestSingleTaskIncrementalIndexHashJoin 对应 Go 的同名测试，来源行 954。
// Scope:通过 testkit 执行 SQL fixture 和结果断言。
/// TestSingleTaskIncrementalIndexHashJoin 对应 Go 的同名测试，来源行 954。
///
/// 通过 testkit 执行 SQL fixture 和结果断言。
#[test]
pub fn test_single_task_incremental_index_hash_join() {
    execute_single_task_incremental_index_hash_join_contract();

    let mut draft = CaseRecorder::new(r#"TestSingleTaskIncrementalIndexHashJoin"#);
    draft.note(r#"通过 testkit 执行 SQL fixture 和结果断言。"#);

    record_line(
        &mut draft,
        r#"func TestSingleTaskIncrementalIndexHashJoin(t *testing.T) {"#,
        r#"source line"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	store := testkit.CreateMockStore(t)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	tk := testkit.NewTestKit(t, store)"#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("use test")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("drop table if exists t1, t2")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t1(a int primary key)")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec("create table t2(b int not null, c varchar(100), index idx_b(b))")"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	sql1 := "insert into t1 values ""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// insert 9 rows to t1, a is 2-10"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := 2; i <= 10; i++ {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"		if i > 2 {"#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );

    record_line(&mut draft, r#"			sql1 += ",""#, r#"source line"#);

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"		sql1 += fmt.Sprintf("(%d)", i)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(sql1)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。
    record_line(
        &mut draft,
        r#"	sql2 := "insert into t2 values ""#,
        r#"状态或参数准备：记录 Go 测试中的变量、fixture 或会话对象构造。"#,
    );
    // 保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。
    record_line(
        &mut draft,
        r#"	// insert 9000 rows to t2, b is 1-9"#,
        r#"保留 Go 原注释：该说明解释后续 SQL、计划或回归问题背景。"#,
    );
    // 循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。
    record_line(
        &mut draft,
        r#"	for i := 1; i <= 9000; i++ {"#,
        r#"循环结构：保留批量插入、重复执行或遍历 join 开关的 Go 控制流。"#,
    );
    // 条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。
    record_line(
        &mut draft,
        r#"		if i > 1 {"#,
        r#"条件分支：保留 Go 测试中跳过、错误容忍或缓存命中的判断。"#,
    );

    record_line(&mut draft, r#"			sql2 += ",""#, r#"source line"#);

    record_line(&mut draft, r#"		}"#, r#"source line"#);

    record_line(
        &mut draft,
        r#"		sql2 += fmt.Sprintf("(%d, 'abc')", i/1000)"#,
        r#"source line"#,
    );

    record_line(&mut draft, r#"	}"#, r#"source line"#);
    // SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。
    record_line(
        &mut draft,
        r#"	tk.MustExec(sql2)"#,
        r#"SQL 执行步骤：保留建表、变量设置、事务、DML 或分析语句的原始顺序。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t1,t2) */ * from t1 inner join t2 on t1.a = t2.b")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t1,t2) */ count(*) from t1 inner join t2 on t1.a = t2.b").Check(testkit.Rows("7001"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t1,t2) */ * from t1 left join t2 on t1.a = t2.b")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t1,t2) */ count(*) from t1 left join t2 on t1.a = t2.b").Check(testkit.Rows("7002"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t2,t1) */ * from t2 right join t1 on t1.a = t2.b")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t2,t1) */ count(*) from t2 right join t1 on t1.a = t2.b").Check(testkit.Rows("7002"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t2,t1) */ * from t1 where t1.a not in (select t2.b from t2 where t2.b = t1.a)")"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );
    // 查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。
    record_line(
        &mut draft,
        r#"	tk.MustQuery("select /*+ inl_hash_join(t2,t1) */ count(*) from t1 where t1.a not in (select t2.b from t2 where t2.b = t1.a)").Check(testkit.Rows("1"))"#,
        r#"查询断言步骤：保留 SQL、排序和期望行集，真实比较仍由 Go testkit 完成。"#,
    );

    record_line(&mut draft, r#"}"#, r#"source line"#);
}
