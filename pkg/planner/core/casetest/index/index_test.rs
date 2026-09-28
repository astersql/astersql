// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 索引访问路径选择用例，对应 Go `index_test.go`。

use std::path::Path;
use std::sync::Arc;

use astersql_domain::Domain;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::LoadTestSuiteDataWithCascades;

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

fn new_testkit_with_domain() -> (Arc<Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

fn run_under_cascades(mut test: impl FnMut(&Arc<Domain>, &mut TestKit)) {
    for cascades in [false, true] {
        let (domain, mut tk) = new_testkit_with_domain();
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner = {}",
                if cascades { 1 } else { 0 }
            ),
            Vec::new(),
        );
        test(&domain, &mut tk);
    }
}

fn load_suite(name: &str) -> astersql_testkit::testdata::TestData {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("testdata path must be valid UTF-8"),
        name,
        true,
    )
    .unwrap_or_else(|error| panic!("load {name} testdata: {error}"))
}

fn assert_plan_rows(tk: &TestKit, sql: &str, expected: &[&str]) {
    let actual = tk
        .MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "sql={sql}");
}

fn assert_query_rows(tk: &TestKit, sql: &str, expected: &[&[&str]]) {
    let actual = tk.MustQuery(sql, Vec::new()).Rows();
    let expected = expected
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_owned()).collect())
        .collect::<Vec<Vec<String>>>();
    assert_eq!(actual, expected, "sql={sql}");
}

fn assert_sorted_query_rows(tk: &TestKit, sql: &str, expected: &[&[&str]]) {
    let mut actual = tk.MustQuery(sql, Vec::new());
    actual.Sort();
    let mut expected = expected
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_owned()).collect())
        .collect::<Vec<Vec<String>>>();
    expected.sort();
    assert_eq!(actual.Rows(), expected, "sql={sql}");
}

fn assert_explain_uses_index(tk: &TestKit, sql: &str, index: &str) {
    let rows = tk.MustQuery(&format!("explain {sql}"), Vec::new()).Rows();
    assert!(
        rows.iter().any(|row| row
            .iter()
            .any(|value| value.contains(&format!("index:{index}")))),
        "expected index {index} in EXPLAIN {sql}: {rows:?}"
    );
}

fn assert_explain_does_not_use_index(tk: &TestKit, sql: &str, index: &str) {
    let rows = tk.MustQuery(&format!("explain {sql}"), Vec::new()).Rows();
    assert!(
        rows.iter().all(|row| !row
            .iter()
            .any(|value| value.contains(&format!("index:{index}")))),
        "did not expect index {index} in EXPLAIN {sql}: {rows:?}"
    );
}

fn run_suite_cases(
    suite: &astersql_testkit::testdata::TestData,
    name: &str,
    integration: bool,
    setup: fn(&mut TestKit),
) {
    for cascades in [false, true] {
        let mut tk = new_testkit();
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner = {}",
                if cascades { 1 } else { 0 }
            ),
            Vec::new(),
        );
        setup(&mut tk);
        let (input, output) = suite
            .LoadTestCasesByName(name, cascades)
            .unwrap_or_else(|error| panic!("load {name} ({cascades}): {error}"));
        let input = input.as_array().expect("suite input must be an array");
        let output = output.as_array().expect("suite output must be an array");
        assert_eq!(input.len(), output.len(), "{name} input/output length");
        for (sql, expected) in input.iter().zip(output) {
            let sql = sql.as_str().expect("suite SQL must be text");
            let actual_plan = tk
                .MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
                .Rows()
                .into_iter()
                .map(|row| row.join(" "))
                .collect::<Vec<_>>();
            let expected_plan = expected
                .get("Plan")
                .and_then(|value| value.as_array())
                .unwrap_or_else(|| panic!("{name} golden case lacks Plan"))
                .iter()
                .map(|row| {
                    row.as_str()
                        .unwrap_or_else(|| panic!("{name} Plan row is not text"))
                        .to_owned()
                })
                .collect::<Vec<_>>();
            assert_eq!(actual_plan, expected_plan, "sql={sql}");
            if integration
                && expected
                    .get("Result")
                    .and_then(|value| value.as_array())
                    .is_some()
            {
                let mut actual = tk.MustQuery(sql, Vec::new());
                actual.Sort();
                let expected_result = expected
                    .get("Result")
                    .and_then(|value| value.as_array())
                    .unwrap_or_else(|| panic!("{name} golden case lacks Result"))
                    .iter()
                    .map(|row| {
                        row.as_str()
                            .unwrap_or_else(|| panic!("{name} Result row is not text"))
                            .to_owned()
                    })
                    .collect::<Vec<_>>();
                let actual_result = actual
                    .Rows()
                    .into_iter()
                    .map(|row| row.join(" "))
                    .collect::<Vec<_>>();
                assert_eq!(actual_result, expected_result, "sql={sql}");
            }
        }
    }
}

fn prepare_prefix_index_fixture(tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "CREATE TABLE t1 (id char(1) DEFAULT NULL, c1 varchar(255) DEFAULT NULL, c2 text DEFAULT NULL, KEY idx1 (c1), KEY idx2 (c1,c2(5))) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        Vec::new(),
    );
    tk.MustExec(
        "create table t2(a int, b varchar(10), index idx(b(5)))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t3(a int, b varchar(10), c int, primary key (a, b(5)) clustered)",
        Vec::new(),
    );
    tk.MustExec("set tidb_opt_prefix_index_single_scan = 1", Vec::new());
    tk.MustExec(
        "insert into t1 values ('a', '0xfff', '111111'), ('b', '0xfff', '22    '), ('c', '0xfff', ''), ('d', '0xfff', null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t2 values (1, 'aaaaaa'), (2, 'bb    '), (3, ''), (4, null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t3 values (1, 'aaaaaa', 2), (1, 'bb    ', 3), (1, '', 4)",
        Vec::new(),
    );
}

#[test]
fn test_null_condition_for_prefix_index() {
    let suite = load_suite("integration_suite");
    run_suite_cases(
        &suite,
        "TestNullConditionForPrefixIndex",
        true,
        prepare_prefix_index_fixture,
    );

    run_under_cascades(|_, tk| {
        prepare_prefix_index_fixture(tk);
        tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
        tk.MustExec("set @@tidb_enable_collect_execution_info=0", Vec::new());
        tk.MustExec(
            "prepare stmt from 'select count(1) from t1 where c1 = ? and c2 is not null'",
            Vec::new(),
        );
        tk.MustExec("set @a = '0xfff'", Vec::new());
        assert_query_rows(tk, "execute stmt using @a", &[&["3"]]);
        assert_query_rows(tk, "execute stmt using @a", &[&["3"]]);
        assert_query_rows(tk, "select @@last_plan_from_cache", &[&["1"]]);
        assert_query_rows(tk, "execute stmt using @a", &[&["3"]]);
        tk.MustUseIndex(
            "select count(1) from t1 where c1 = '0xfff' and c2 is not null",
            "idx2",
        );
    });
}

#[test]
fn index_range_quota_preserves_table_and_prefix_residuals() {
    run_under_cascades(|_, tk| {
        prepare_prefix_index_fixture(tk);
        for quota in [1, 1_000_000] {
            tk.MustExec(&format!("set tidb_opt_range_max_size={quota}"), Vec::new());
            assert_query_rows(
                tk,
                "select count(1) from t1 force index(idx1) where c1='0xfff' and c2 is not null",
                &[&["3"]],
            );
            assert_query_rows(
                tk,
                "select count(1) from t1 force index(idx2) where c1='0xfff' and c2='111111'",
                &[&["1"]],
            );
        }
    });
}

#[test]
fn test_invisible_index() {
    run_under_cascades(|_, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("CREATE TABLE t1 (a INT, KEY(a) INVISIBLE)", Vec::new());
        tk.MustExec(
            "INSERT INTO t1 VALUES (1),(2),(3),(4),(5),(6),(7),(8),(9),(10)",
            Vec::new(),
        );
        assert_plan_rows(
            tk,
            "SELECT a FROM t1",
            &[
                "TableReader root  data:TableFullScan",
                "└─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
            ],
        );
        tk.MustExec("set session tidb_opt_use_invisible_indexes=on", Vec::new());
        assert_plan_rows(
            tk,
            "SELECT a FROM t1",
            &[
                "IndexReader root  index:IndexFullScan",
                "└─IndexFullScan cop[tikv] table:t1, index:a(a) keep order:false, stats:pseudo",
            ],
        );
    });
}

fn prepare_range_derivation_fixture(tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_opt_fix_control = '54337:ON'", Vec::new());
    tk.MustExec(
        "create table t1 (a1 int, b1 int, c1 int, primary key pkx (a1,b1))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t1char (a1 char(5), b1 char(5), c1 int, primary key pkx (a1,b1))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int, b int, c int, primary key(a,b))",
        Vec::new(),
    );
    tk.MustExec(
        "create table tuk (a int, b int, c int, unique key (a,b,c))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_regard_null_as_point=false", Vec::new());
}

#[test]
fn test_range_derivation() {
    let suite = load_suite("index_range");
    run_suite_cases(
        &suite,
        "TestRangeDerivation",
        false,
        prepare_range_derivation_fixture,
    );
}

fn prepare_range_intersection_fixture(tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_opt_fix_control = '54337:ON'", Vec::new());
    tk.MustExec(
        "create table t1 (a1 int, b1 int, c1 int, key pkx (a1,b1))",
        Vec::new(),
    );
    tk.MustExec("create table t_inlist_test(a1 int,b1 int,c1 varbinary(767) DEFAULT NULL, KEY twoColIndex (a1,b1))", Vec::new());
    for statement in [
        "insert into t1 values (1,1,1)",
        "insert into t1 values (null,1,1)",
        "insert into t1 values (1,null,1)",
        "insert into t1 values (1,1,null)",
        "insert into t1 values (1,10,1)",
        "insert into t1 values (10,20,1)",
        "insert into t1 select a1+1,b1,c1+1 from t1",
        "insert into t1 select a1,b1+1,c1+1 from t1",
        "insert into t1 select a1-1,b1+1,c1+1 from t1",
        "insert into t1 select a1+2,b1+2,c1+2 from t1",
        "insert into t1 select a1+2,b1-2,c1+2 from t1",
        "insert into t1 select a1+2,b1-1,c1+2 from t1",
        "insert into t1 select null,b1,c1+1 from t1",
        "insert into t1 select a1,null,c1+1 from t1",
    ] {
        tk.MustExec(statement, Vec::new());
    }
    tk.MustExec("create table t11 (a1 int, b1 int, c1 int)", Vec::new());
    tk.MustExec("insert into t11 select * from t1", Vec::new());
    tk.MustExec("create table tablename (primary_key varbinary(1024) NOT NULL, secondary_key varbinary(1024) NOT NULL, timestamp bigint NOT NULL, value mediumblob DEFAULT NULL, PRIMARY KEY PKK (primary_key,secondary_key,timestamp))", Vec::new());
    tk.MustExec(
        "create table t(a int, b int, c int, key PKK(a,b,c))",
        Vec::new(),
    );
    tk.MustExec(
        "create table tt(a int, b int, c int, primary key PKK(a,b,c))",
        Vec::new(),
    );
    tk.MustExec("insert into t select * from t1", Vec::new());
    tk.MustExec(
        "insert into tt select * from t1 where a1 is not null and b1 is not null and c1 is not null",
        Vec::new(),
    );
    tk.MustExec("create table tnull (a INT, KEY PK(a))", Vec::new());
    tk.MustExec("create table tkey_string(id1 CHAR(16) NOT NULL, id2 VARCHAR(16) NOT NULL, id3 BINARY(16) NOT NULL, id4 VARBINARY(16) NOT NULL, id5 BLOB NOT NULL, id6 TEXT NOT NULL, id7 ENUM('x-small','small','medium','large','x-large') NOT NULL, id8 SET('a','b','c','d') NOT NULL, name varchar(16), PRIMARY KEY(id1,id2,id3,id4,id7,id8)) PARTITION BY KEY(id7) PARTITIONS 4", Vec::new());
    for statement in [
        "insert into tkey_string values('huaian','huaian','huaian','huaian','huaian','huaian','x-small','a','linpin')",
        "insert into tkey_string values('nanjing','nanjing','nanjing','nanjing','nanjing','nanjing','small','b','linpin')",
        "insert into tkey_string values('zhenjiang','zhenjiang','zhenjiang','zhenjiang','zhenjiang','zhenjiang','medium','c','linpin')",
        "insert into tkey_string values('suzhou','suzhou','suzhou','suzhou','suzhou','suzhou','large','d','linpin')",
        "insert into tkey_string values('wuxi','wuxi','wuxi','wuxi','wuxi','wuxi','x-large','a','linpin')",
    ] {
        tk.MustExec(statement, Vec::new());
    }
    tk.MustExec("create table t_issue_60556(a int, b int, ac char(3), bc char(3), key ab(a,b), key acbc(ac,bc))", Vec::new());
    tk.MustExec(
        "insert into t_issue_60556 values (100, 500, '100', '500')",
        Vec::new(),
    );
}

#[test]
fn test_range_intersection() {
    let suite = load_suite("index_range");
    run_suite_cases(
        &suite,
        "TestRangeIntersection",
        true,
        prepare_range_intersection_fixture,
    );
}

#[test]
fn test_row_function_match_the_index_range_scan() {
    let suite = load_suite("integration_suite");
    run_suite_cases(
        &suite,
        "TestRowFunctionMatchTheIndexRangeScan",
        true,
        |tk| {
            tk.MustExec("use test", Vec::new());
            tk.MustExec("set @@tidb_opt_fix_control = '54337:ON'", Vec::new());
            tk.MustExec(
                "CREATE TABLE t1 (k1 int, k2 int, k3 int, index pk1(k1,k2))",
                Vec::new(),
            );
            tk.MustExec("create table t2 (k1 int, k2 int)", Vec::new());
        },
    );
}

#[test]
fn test_ordered_index_with_is_null() {
    run_under_cascades(|_, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            "CREATE TABLE t1 (a int key, b int, c int, index (b,c))",
            Vec::new(),
        );
        assert_plan_rows(
            tk,
            "select a from t1 where b is null order by c",
            &[
                "Projection root  test.t1.a",
                "└─IndexReader root  index:IndexRangeScan",
                "  └─IndexRangeScan cop[tikv] table:t1, index:b(b, c) range:[NULL,NULL], keep order:true, stats:pseudo",
            ],
        );
        tk.MustExec(
            "create table t2(id bigint DEFAULT NULL, UNIQUE KEY index_on_id (id))",
            Vec::new(),
        );
        tk.MustExec("insert into t2 values (), (), ()", Vec::new());
        tk.MustExec("analyze table t2", Vec::new());
        assert_plan_rows(
            tk,
            "select count(*) from t2 where id is null",
            &[
                "StreamAgg root  funcs:count(Column)->Column",
                "└─IndexReader root  index:StreamAgg",
                "  └─StreamAgg cop[tikv]  funcs:count(1)->Column",
                "    └─IndexRangeScan cop[tikv] table:t2, index:index_on_id(id) range:[NULL,NULL], keep order:false",
            ],
        );
    });
}

#[test]
fn test_partial_index_with_plan_cache() {
    run_under_cascades(|_, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
        tk.MustExec("set tidb_enable_collect_execution_info=0", Vec::new());
        tk.MustExec("create table t(a int, b int, index idx1(a) where a is not null, index idx2(b) where b > 10)", Vec::new());
        tk.MustExec("insert into t values (1, 20), (2, 5)", Vec::new());

        tk.MustExec(
            "prepare stmt from 'select * from t where a = ?'",
            Vec::new(),
        );
        tk.MustExec("set @a = 123", Vec::new());
        tk.MustExec("execute stmt using @a", Vec::new());
        tk.MustExec("execute stmt using @a", Vec::new());
        assert_explain_uses_index(tk, "select * from t where a = 123", "idx1");
        tk.MustExec("execute stmt using @a", Vec::new());
        assert_query_rows(tk, "select @@last_plan_from_cache", &[&["1"]]);

        tk.MustExec(
            "prepare stmt from 'select * from t where b = ?'",
            Vec::new(),
        );
        tk.MustExec("set @a = 20", Vec::new());
        assert_query_rows(tk, "execute stmt using @a", &[&["1", "20"]]);
        assert_query_rows(tk, "execute stmt using @a", &[&["1", "20"]]);
        assert_explain_uses_index(tk, "select * from t where b = 20", "idx2");
        tk.MustExec("execute stmt using @a", Vec::new());
        assert_query_rows(tk, "select @@last_plan_from_cache", &[&["0"]]);
    });
}

#[test]
fn test_partial_index_with_index_prune() {
    run_under_cascades(|_, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("set tidb_enable_collect_execution_info=0", Vec::new());
        tk.MustExec("create table t(a int, b int, index idx1(a) where a is not null, index idx2(b) where b > 10)", Vec::new());

        assert_explain_uses_index(tk, "select * from t use index(idx1) where a > 1", "idx1");
        tk.MustExec("set tidb_opt_index_prune_threshold=0", Vec::new());
        assert_explain_does_not_use_index(tk, "select * from t", "idx1");
        assert_explain_does_not_use_index(tk, "select * from t", "idx2");
        assert_explain_does_not_use_index(tk, "select * from t order by b", "idx2");
        assert_explain_does_not_use_index(tk, "select * from t where a is null", "idx1");
    });
}

#[test]
fn test_force_index_limit() {
    run_under_cascades(|_, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("CREATE TABLE tb (object_id bigint, a bigint, b bigint, c bigint, PRIMARY KEY(object_id), KEY ab(a,b))", Vec::new());
        assert_plan_rows(
            tk,
            "select count(1) from (select /* issue:54213 */ /*+ force_index(tb, ab) */ 1 from tb where a=1 and b=1 limit 100) a",
            &[
                "StreamAgg root  funcs:count(1)->Column",
                "└─Limit root  offset:0, count:100",
                "  └─IndexReader root  index:Limit",
                "    └─Limit cop[tikv]  offset:0, count:100",
                "      └─IndexRangeScan cop[tikv] table:tb, index:ab(a, b) range:[1 1,1 1], keep order:false, stats:pseudo",
            ],
        );
    });
}

#[test]
fn test_vector_index() {
    run_under_cascades(|domain, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            "create table t(a int, b vector, c vector(3), d vector(4))",
            Vec::new(),
        );
        tk.MustExec("alter table t set tiflash replica 1", Vec::new());
        tk.MustExec(
            "alter table t add vector index vecIdx1((vec_cosine_distance(d))) using hnsw",
            Vec::new(),
        );
        domain
            .set_tiflash_replica_for_test("test", "t", 1, true)
            .expect("set test.t TiFlash replica");
        tk.MustUseIndex(
            "select * from t use index(vecIdx1) order by vec_cosine_distance(d, '[1,1,1,1]') limit 1",
            "vecidx1",
        );
        tk.MustUseIndex(
            "select * from t use index(vecIdx1) order by vec_cosine_distance('[1,1,1,1]', d) limit 1",
            "vecidx1",
        );
        let wrong_distance = tk.Exec(
            "select * from t use index(vecIdx1) order by vec_l2_distance(d, '[1,1,1,1]') limit 1",
            Vec::new(),
        );
        let filtered = tk.Exec(
            "select * from t use index(vecIdx1) where a = 5 order by vec_cosine_distance(d, '[1,1,1,1]') limit 1",
            Vec::new(),
        );
        assert!(
            wrong_distance.is_err() && filtered.is_err(),
            "vector-index incompatibilities must fail: wrong_distance={wrong_distance:?}, filtered={filtered:?}"
        );
    });
}

#[test]
fn test_inverted_index() {
    run_under_cascades(|domain, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("create table t (a int, b bigint, c tinyint, d smallint unsigned, columnar index idx_a (a) using inverted, columnar index idx_b (b) using inverted)", Vec::new());
        tk.MustExec(
            "alter table t add columnar index idx_c (c) using inverted",
            Vec::new(),
        );
        tk.MustExec(
            "alter table t add columnar index idx_d (d) using inverted",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t values (1,1,1,1),(2,2,2,2),(3,3,3,3),(4,4,4,4)",
            Vec::new(),
        );
        domain
            .set_tiflash_replica_for_test("test", "t", 1, true)
            .expect("set test.t TiFlash replica");
        for (sql, index) in [
            ("select * from t force index(idx_a) where a > 0", "idx_a"),
            ("select * from t force index(idx_b) where b < 0", "idx_b"),
            ("select * from t force index(idx_c) where c = 0", "idx_c"),
            ("select * from t force index(idx_d) where d != 0", "idx_d"),
        ] {
            tk.MustUseIndex(sql, index);
        }
        for sql in [
            "select * from t ignore index(idx_a) where a = 1",
            "select * from t ignore index(idx_b) where b = 2",
            "select * from t ignore index(idx_c) where c = 3",
            "select * from t ignore index(idx_d) where d < 1",
        ] {
            tk.MustNoIndexUsed(sql);
        }
    });
}

#[test]
fn test_analyze_columnar_index() {
    run_under_cascades(|domain, tk| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            "create table t(a int, b vector(2), c datetime, j json, index(a))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t values(1, '[1, 0]', '2022-01-01 12:00:00', '{\"a\": 1}')",
            Vec::new(),
        );
        tk.MustExec(
            "alter table t set tiflash replica 2 location labels 'a','b'",
            Vec::new(),
        );
        domain
            .set_tiflash_replica_for_test("test", "t", 2, true)
            .expect("set test.t TiFlash replicas");
        tk.MustExec(
            "alter table t add vector index idx((vec_cosine_distance(b))) using hnsw",
            Vec::new(),
        );
        tk.MustExec(
            "alter table t add columnar index idx2(c) using inverted",
            Vec::new(),
        );

        tk.MustUseIndex(
            "select * from t use index(idx) order by vec_cosine_distance(b, '[1, 0]') limit 1",
            "idx",
        );
        tk.MustUseIndex(
            "select * from t order by vec_cosine_distance(b, '[1, 0]') limit 1",
            "idx",
        );
        tk.MustNoIndexUsed(
            "select * from t ignore index(idx) order by vec_cosine_distance(b, '[1, 0]') limit 1",
        );

        tk.MustExec("set tidb_analyze_version=2", Vec::new());
        tk.MustExec("analyze table t", Vec::new());
        assert_sorted_query_rows(
            tk,
            "show warnings",
            &[
                &[
                    "Note",
                    "1105",
                    "Analyze use auto adjusted sample rate 1.000000 for table test.t, reason to use this rate is \"use min(1, 110000/10000) as the sample-rate=1\"",
                ],
                &[
                    "Warning",
                    "1105",
                    "analyzing columnar index is not supported, skip idx",
                ],
                &[
                    "Warning",
                    "1105",
                    "analyzing columnar index is not supported, skip idx2",
                ],
            ],
        );
        tk.MustExec("analyze table t index idx", Vec::new());
        assert_sorted_query_rows(
            tk,
            "show warnings",
            &[
                &[
                    "Note",
                    "1105",
                    "Analyze use auto adjusted sample rate 1.000000 for table test.t, reason to use this rate is \"use min(1, 110000/1) as the sample-rate=1\"",
                ],
                &[
                    "Warning",
                    "1105",
                    "The version 2 would collect all statistics not only the selected indexes",
                ],
                &[
                    "Warning",
                    "1105",
                    "analyzing columnar index is not supported, skip idx",
                ],
                &[
                    "Warning",
                    "1105",
                    "analyzing columnar index is not supported, skip idx2",
                ],
            ],
        );

        let context = tk.AnalyzeStatsContext().expect("stats context");
        let table_id = context.table("test", "t").expect("test.t stats").table_id;
        let stats = context
            .physical_stats(table_id)
            .expect("test.t physical stats");
        assert!(stats.last_analyze_version > 0);
        let int_column = stats.columns.get(&1).expect("int column stats");
        assert!(!int_column.buckets.is_empty() || !int_column.top_n.is_empty());
        let vector_column = stats.columns.get(&2).expect("vector column stats");
        assert!(vector_column.buckets.is_empty() && vector_column.top_n.is_empty());
    });
}
