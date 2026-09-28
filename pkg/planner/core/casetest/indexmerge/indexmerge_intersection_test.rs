// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

fn new_testkit() -> TestKit {
    TestKit::new(CreateMockStoreAndDomain().0)
}

fn plan_rows(tk: &TestKit, sql: &str) -> Vec<String> {
    tk.MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect()
}

macro_rules! string_array {
    ($value:expr, $field:literal, $sql:expr) => {{
        $value
            .get($field)
            .and_then(|value| value.as_array())
            .unwrap_or_else(|| panic!("missing {} for {}", $field, $sql))
            .iter()
            .map(|row| {
                row.as_str()
                    .unwrap_or_else(|| panic!("{} row must be a string for {}", $field, $sql))
                    .to_owned()
            })
            .collect::<Vec<_>>()
    }};
}

macro_rules! assert_golden_plan {
    ($tk:expr, $sql:expr, $expected:expr) => {{
        assert_eq!(
            $expected.get("SQL").and_then(|value| value.as_str()),
            Some($sql),
            "golden SQL"
        );
        assert_eq!(
            plan_rows($tk, $sql),
            string_array!($expected, "Plan", $sql),
            "sql={}",
            $sql
        );
    }};
}

macro_rules! assert_golden_result {
    ($tk:expr, $sql:expr, $expected:expr) => {{
        let mut actual = $tk.MustQuery($sql, Vec::new());
        actual.Sort();
        let actual = actual
            .Rows()
            .into_iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            string_array!($expected, "Result", $sql),
            "sql={}",
            $sql
        );
    }};
}

/// 对应 Go TestPlanCacheForIntersectionIndexMerge：首次执行编译计划，后续
/// 执行命中缓存，且最终 hint 计划仍然是 IndexMerge。
#[test]
fn test_plan_cache_for_intersection_index_merge() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "create table t(a int, b int, c int, d int, e int, index ia(a), index ib(b), index ic(c), index id(d), index ie(e))",
        Vec::new(),
    );
    tk.MustExec(
        "prepare stmt from 'select /*+ use_index_merge(t, ia, ib, ic, id, ie) */ * from t where a = 10 and b = ? and c > ? and d is null and e in (0, 100)'",
        Vec::new(),
    );
    tk.MustExec("set @a=1, @b=3", Vec::new());
    tk.MustQuery("execute stmt using @a,@b", Vec::new())
        .Check(Rows(&[]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("execute stmt using @a,@b", Vec::new())
        .Check(Rows(&[]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("set @a=100, @b=500", Vec::new());
    tk.MustQuery("execute stmt using @a,@b", Vec::new())
        .Check(Rows(&[]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("execute stmt using @a,@b", Vec::new())
        .Check(Rows(&[]));
    assert!(tk.HasPlan(
        "select /*+ use_index_merge(t, ia, ib, ic, id, ie) */ * from t where a = 10 and b = 100 and c > 500 and d is null and e in (0, 100)",
        "IndexMerge"
    ));
}

/// 对应 Go TestIndexMergeWithOrderProperty：golden 中的排序/索引选择
/// 场景必须都能执行，且计划树逐行保持一致。
#[test]
fn test_index_merge_with_order_property() {
    let suite = crate::main_test::load_index_merge_suite();
    for cascades in [false, true] {
        let mut tk = new_testkit();
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner = {}",
                usize::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec("drop table if exists t2", Vec::new());
        tk.MustExec(
            "create table t (a int, b int, c int, d int, e int, key a(a), key b(b), key c(c), key ac(a, c), key bc(b, c), key ae(a, e), key be(b, e), key abd(a, b, d), key cd(c, d))",
            Vec::new(),
        );
        tk.MustExec(
            "create table t2 (a int, b int, c int, key a(a), key b(b), key ac(a, c))",
            Vec::new(),
        );
        let (input, output) = suite
            .LoadTestCasesByName("TestIndexMergeWithOrderProperty", cascades)
            .unwrap();
        for (sql, expected) in input
            .as_array()
            .unwrap()
            .iter()
            .zip(output.as_array().unwrap())
        {
            let sql = sql.as_str().unwrap();
            assert_golden_plan!(&tk, sql, expected);
            assert!(tk.MustQuery("show warnings", Vec::new()).is_empty());
        }
    }
}

/// 对应 Go TestHintForIntersectionIndexMerge：覆盖动态/静态分区裁剪、视图
/// hint、不同索引数据类型、相关子查询，以及 clustered primary key 回归。
#[test]
fn test_hint_for_intersection_index_merge() {
    let suite = crate::main_test::load_index_merge_suite();
    for cascades in [false, true] {
        let mut tk = new_testkit();
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner = {}",
                usize::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec(
            "create table t1(a int, b int, c int, d int, e int, index ia(a), index ibc(b, c),index ic(c), index id(d), index ie(e)) partition by range(c) (partition p0 values less than (10), partition p1 values less than (20), partition p2 values less than (30), partition p3 values less than (maxvalue))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t1 values (10, 20, 5, 5, 3), (20, 20, 50, 5, 200), (20, 20, 10, 5, 5), (10, 30, 5, 3, 1)",
            Vec::new(),
        );
        tk.MustExec(
            "create definer='root'@'localhost' view vh as select /*+ use_index_merge(t1, ia, ibc, id) */ * from t1 where a = 10 and b = 20 and c < 30 and d in (2,5)",
            Vec::new(),
        );
        tk.MustExec(
            "create definer='root'@'localhost' view v as select * from t1 where a = 10 and b = 20 and c < 30 and d in (2,5)",
            Vec::new(),
        );
        tk.MustExec(
            "create definer='root'@'localhost' view v1 as select * from t1 where a = 10 and b = 20",
            Vec::new(),
        );
        tk.MustExec(
            "create table t2(a int, b int, c int, d int, e int, index ia(a), index ibc(b, c), index id(d), index ie(e)) partition by range columns (c, d) (partition p0 values less than (10, 20), partition p1 values less than (30, 40), partition p2 values less than (50, 60), partition p3 values less than (maxvalue, maxvalue))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 values (10, 20, 5, 5, 3), (20, 20, 20, 5, 100), (100, 30, 5, 3, 100)",
            Vec::new(),
        );
        tk.MustExec(
            "create table t3(a int, b int, c int, d int, e int, index ia(a), index ibc(b, c), index id(d), index ie(e)) partition by hash (e) partitions 5",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t3 values (10, 20, 5, 5, 3), (20, 20, 20, 5, 100), (10, 30, 5, 3, 100)",
            Vec::new(),
        );
        tk.MustExec(
            "create table t4(a int, b int, c int, d int, e int, index ia(a), index ibc(b, c), index id(d), index ie(e)) partition by list (d) (partition p0 values in (1,2,3,4,5), partition p1 values in (6,7,8,9,10), partition p2 values in (11,12,13,14,15), partition p3 values in (16,17,18,19,20))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t4 values (30, 20, 5, 8, 100), (20, 20, 20, 3, 2), (10, 30, 5, 3, 100)",
            Vec::new(),
        );
        tk.MustExec(
            "create table t5(s1 varchar(20) collate utf8mb4_bin, s2 varchar(30) collate ascii_bin, s3 varchar(50) collate utf8_unicode_ci, s4 varchar(20) collate gbk_chinese_ci, index is1(s1), index is2(s2), index is3(s3), index is4(s4))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t5 values ('Abc', 'zzzz', 'aa', 'ccc'), ('abc', 'zzzz', 'CCC', 'ccc')",
            Vec::new(),
        );
        tk.MustExec(
            "create table t6(s1 varchar(20) collate utf8mb4_bin, s2 varchar(30) collate ascii_bin, s3 varchar(50) collate utf8_unicode_ci, s4 varchar(20) collate gbk_chinese_ci, primary key (s1, s2(10)) nonclustered, index is1(s1), index is2(s2), index is3(s3), index is4(s4))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t6 values ('Abc', 'zzzz', 'A啊A', 'Cdaa'), ('Abc', 'zczz', 'A啊', 'Cda')",
            Vec::new(),
        );
        tk.MustExec(
            "create table t7(a tinyint unsigned, b bit(3), c float, d decimal(10,3), e datetime, f timestamp(5), g year, primary key (d) nonclustered, index ia(a), unique index ib(b), index ic(c), index ie(e), index iff(f), index ig(g))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t7 values (100, 6, 12.2, 56, '2022-11-22 17:00', '2022-12-21 00:00', 2021), (20, 7, 12.4, 30, '2022-12-22 17:00', '2016-12-21 00:00', 2021)",
            Vec::new(),
        );
        tk.MustExec(
            "create table t8(s1 mediumtext collate utf8mb4_general_ci, s2 varbinary(20), s3 tinyblob, s4 enum('测试', 'aA', '??') collate gbk_chinese_ci, s5 set('^^^', 'tEsT', '2') collate utf8_general_ci, primary key (s1(10)) nonclustered, unique index is2(s2(20)), index is3(s3(20)), index is4(s4), index is5(s5))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t8 values('啊aabbccdd', 'abcc', 'cccc', 'aa', '2,test'), ('啊aabb', 'abcdc', 'aaaa', '??', '2')",
            Vec::new(),
        );
        tk.MustExec("analyze table t1,t2,t3,t4", Vec::new());

        let (input, output) = suite
            .LoadTestCasesByName("TestHintForIntersectionIndexMerge", cascades)
            .unwrap();
        for (sql, expected) in input
            .as_array()
            .unwrap()
            .iter()
            .zip(output.as_array().unwrap())
        {
            let sql = sql.as_str().unwrap();
            if sql.to_ascii_lowercase().starts_with("set") {
                assert_eq!(
                    expected.get("SQL").and_then(|value| value.as_str()),
                    Some(sql),
                    "golden SQL"
                );
                tk.MustExec(sql, Vec::new());
                continue;
            }
            assert_golden_plan!(&tk, sql, expected);
            assert_golden_result!(&tk, sql, expected);
            assert!(tk.MustQuery("show warnings", Vec::new()).is_empty());
        }

        tk.MustExec("drop table if exists t_issue_65791", Vec::new());
        tk.MustExec(
            "create table t_issue_65791 (id bigint not null, a bigint not null, b bigint not null, c varchar(32) not null, primary key (id) clustered, key ia(a))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t_issue_65791 values (1, 10, 100, 'x'), (2, 20, 200, 'y'), (3, 30, 300, 'z')",
            Vec::new(),
        );
        tk.MustExec("analyze table t_issue_65791 all columns", Vec::new());
        tk.MustExec("set tidb_enable_index_merge=1", Vec::new());
        let sql = "select /*+ use_index_merge(t_issue_65791, primary, ia) */ * from t_issue_65791 where id = 1 or a = 20";
        let mut result = tk.MustQuery(sql, Vec::new());
        result.Sort().Check(Rows(&["1 10 100 x", "2 20 200 y"]));
        assert!(tk.HasPlan(sql, "IndexMerge"));
        assert!(tk.MustQuery("show warnings", Vec::new()).is_empty());
    }
}
