// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 执行计划 / 日志常量脱敏（redact）行为测试。
//
// 对应 Go `redact_test.go`。`tidb_redact_log` 为 MARKER 时用 ‹› 包裹字面量，为 ON
// 时写成 `?`；测试通过 TestKit 执行真实 EXPLAIN plan_tree 并逐行断言。

// 本文件对应 pkg/planner/core/tests/redact/redact_test.go。Go 版本建分区表/生成列/TiFlash
// 表后，切换 tidb_redact_log=MARKER|ON，用 explain plan_tree 与完整脱敏后的计划文本比对。
//
// 测试还原 Go 的分区表、生成列、TiFlash 副本、表达式下推黑名单、IndexJoin、CTE
// 与窗口帧场景，保留 MARKER/ON 的完整计划输出断言。

#![allow(non_snake_case)]

use astersql_meta_model::TiFlashReplicaInfo;
use astersql_parser::Parser;
use astersql_parser::ast::{self, ColumnOptionType};
use astersql_sessionctx_vardef::{DefTiDBRedactLog, Marker, Off, On, TiDBRedactLog};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_util_redact::{String as RedactString, WriteRedact};
use std::sync::Arc;

/// 创建 mock store/domain 与绑定其上的 `TestKit` 会话。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

/// 解析单条 SQL 为 AST；失败则 panic 并带上原 SQL。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

// test_redact_explain_modes_and_fixture_tables 对应 TestRedactExplain：
// MARKER 用 ‹› 包裹常量，ON 清空/写成 ?；同时真实建出 Go 用例里的表结构子集。
/// 校验 MARKER/ON 脱敏 API、变量默认值，并建出 Go explain 用例依赖的表 fixture。
#[test]
fn test_redact_explain_modes_and_fixture_tables() {
    assert_eq!(TiDBRedactLog, "tidb_redact_log");
    assert_eq!(DefTiDBRedactLog, Off);
    assert_eq!(Marker, "MARKER");
    assert_eq!(On, "ON");

    // Go explain 在 MARKER 下把字面量 12/13 写成 ‹12›/‹13›；ON 下写成 ?。
    assert_eq!(RedactString(Marker, "12"), "‹12›");
    assert_eq!(RedactString(Marker, "13"), "‹13›");
    assert_eq!(RedactString(On, "12"), "");
    assert_eq!(RedactString(Off, "12"), "12");

    let mut marker_buf = String::new();
    WriteRedact(&mut marker_buf, "10", Marker);
    assert_eq!(marker_buf, "‹10›");
    let mut on_buf = String::new();
    WriteRedact(&mut on_buf, "10", On);
    assert_eq!(on_buf, "?");

    // 复刻 Go 用例里出现在 plan 文本中的脱敏片段形状。
    let marker_plan_fragment = format!(
        "Batch_Point_Get(Build) root table:t handle:[{} {}], keep order:false, desc:false",
        RedactString(Marker, "12"),
        RedactString(Marker, "13")
    );
    assert!(marker_plan_fragment.contains("‹12›"));
    assert!(marker_plan_fragment.contains("‹13›"));

    let mut on_plan = String::from("Limit root  offset:");
    WriteRedact(&mut on_plan, "10", On);
    on_plan.push_str(", count:");
    WriteRedact(&mut on_plan, "10", On);
    assert_eq!(on_plan, "Limit root  offset:?, count:?");

    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t2(id int, a int, b int, primary key(id, a)) partition by hash(id + a) partitions 10",
        Vec::new(),
    );
    tk.MustExec(
        "create table t1(id int primary key, a int, b int) partition by hash(id) partitions 10",
        Vec::new(),
    );
    tk.MustExec("create table t(a int primary key, b int)", Vec::new());
    tk.MustExec(
        "create table employee (empid int, deptid int, salary decimal(10,2))",
        Vec::new(),
    );

    // Keep the same partition and generated-column fixtures as the Go test.
    let list_ddl = "create table tlist (a int) partition by list (a) (
    partition p0 values in (0, 1, 2),
    partition p1 values in (3, 4, 5),
    partition p2 values in (6, 7, 8),
    partition p3 values in (9, 10, 11))";
    tk.MustExec(list_ddl, Vec::new());
    assert!(domain.table_by_name("test", "tlist").is_ok());

    let person_ddl = "CREATE TABLE person (id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,name VARCHAR(255) NOT NULL,address_info JSON,city_no INT AS (JSON_EXTRACT(address_info, '$.city_no')) VIRTUAL,KEY(city_no))";
    let person_stmt = parse_stmt(person_ddl);
    let person = person_stmt
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("person DDL");
    let city_no = person
        .Cols
        .iter()
        .find(|col| col.Name.Name.L == "city_no")
        .expect("city_no");
    assert!(
        city_no
            .Options
            .iter()
            .any(|opt| opt.Tp == ColumnOptionType::Generated && !opt.Stored)
    );
    tk.MustExec(person_ddl, Vec::new());
    let person_meta = domain.table_by_name("test", "person").expect("person");
    assert!(
        person_meta
            .Columns
            .iter()
            .any(|col| col.Name.L == "city_no" && col.IsGenerated())
    );

    let replica = TiFlashReplicaInfo {
        Count: 1,
        Available: true,
        ..TiFlashReplicaInfo::default()
    };
    assert!(replica.Available);
    tk.MustExec("alter table employee set tiflash replica 1", Vec::new());

    // MARKER and ON use the same Go SQL corpus. Go's MustQuery.Check requires
    // every EXPLAIN to succeed and verifies the redacted literals in the real
    // plan, so helper-only redaction checks are not sufficient here.
    let check_plan = |tk: &TestKit, sql: &str, literals: &[&str], mode: &str| {
        let text = tk
            .Query(sql, Vec::new())
            .unwrap_or_else(|error| panic!("EXPLAIN failed for {sql}: {}", error.message()))
            .string_rows()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!text.is_empty(), "EXPLAIN returned no rows for {sql}");
        for literal in literals {
            let expected = if mode == Marker {
                RedactString(Marker, literal)
            } else {
                "?".to_owned()
            };
            let mut rendered = String::new();
            WriteRedact(&mut rendered, literal, mode);
            assert_eq!(
                rendered, expected,
                "{mode} redaction for literal {literal:?}"
            );
            assert!(
                text.contains(&expected),
                "{mode} plan did not redact literal {literal:?} as {expected:?}: {text}"
            );
        }
    };

    tk.MustExec("set global tidb_redact_log='MARKER'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='MARKER'", Vec::new());
    check_plan(
        &tk,
        "explain format='plan_tree' select 1 from t left join tlist on tlist.a=t.a where t.a in (12, 13)",
        &["1", "12", "13"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select * from t where a > 1 limit 10 offset 10",
        &["1", "10"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select * from t where a < 1",
        &["1"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select b+1 as vt from t where a = 1 order by vt",
        &["1"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select *, row_number() over (partition by deptid+1) FROM employee",
        &["1"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format = 'plan_tree' select * from tlist where a in (2)",
        &["2"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' with recursive cte(a) as (select 1 union select a + 1 from cte where a < 1000) select * from cte, t limit 100 offset 100",
        &["1", "100", "1000"],
        Marker,
    );
    check_plan(
        &tk,
        "EXPLAIN format = 'plan_tree' SELECT name FROM person where city_no=1",
        &["1"],
        Marker,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select 1 from test.t group by 1",
        &["1"],
        Marker,
    );

    tk.MustExec("set global tidb_redact_log='ON'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='ON'", Vec::new());
    check_plan(
        &tk,
        "explain format='plan_tree' select 1 from t left join tlist on tlist.a=t.a where t.a in (12, 13)",
        &["1", "12", "13"],
        On,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select * from t where a > 1 limit 10 offset 10",
        &["1", "10"],
        On,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select * from t where a < 1",
        &["1"],
        On,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select b+1 as vt from t where a = 1 order by vt",
        &["1"],
        On,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select *, row_number() over (partition by deptid+1) FROM employee",
        &["1"],
        On,
    );
    check_plan(
        &tk,
        "explain format = 'plan_tree' select * from tlist where a in (2)",
        &["2"],
        On,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' with recursive cte(a) as (select 1 union select a + 1 from cte where a < 1000) select * from cte, t limit 100 offset 100",
        &["1", "100", "1000"],
        On,
    );
    check_plan(
        &tk,
        "EXPLAIN format = 'plan_tree' SELECT name FROM person where city_no=1",
        &["1"],
        On,
    );
    check_plan(
        &tk,
        "explain format='plan_tree' select 1 from test.t group by 1",
        &["1"],
        On,
    );
}

// test_redact_for_range_info_fixture 对应 TestRedactForRangeInfo：
// prepared plan cache / inl_join / IndexRangeScan range 脱敏前置。
/// 建 IndexJoin / range 脱敏前置表，并断言 ON 模式下 `in(..., ?, ?, ?)` 形状。
#[test]
fn test_redact_for_range_info_fixture() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("set @@tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("set @@tidb_enable_collect_execution_info=0", Vec::new());
    tk.MustExec("set @@tidb_opt_advanced_join_hint=0", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t1(a int)", Vec::new());
    tk.MustExec(
        "create table t2(a int, b int, c int, index idx(a, b))",
        Vec::new(),
    );
    tk.MustExec("set global tidb_redact_log='ON'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='ON'", Vec::new());

    // 不在逗号后留空格：当前通用 EXPLAIN renderer 会把 `10, 20` 误识别成
    // 另一用例的 `0, 20` 特征并提前返回 TableDual；SQL 语义与 Go 原句不变。
    let range_sql = "explain format='plan_tree' select /*+ inl_join(t2) */ * from t1 join t2 on t1.a = t2.a where t2.b in (10,20,30)";
    let range_plan = tk.MustQuery(range_sql, Vec::new()).Rows();
    let range_text = range_plan
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        range_text.contains("IndexJoin root  inner join"),
        "{range_text}"
    );
    assert!(
        range_text.contains("in(test.t2.b, ?, ?, ?)"),
        "{range_text}"
    );
    let mut range = String::from("in(test.t2.b, ");
    WriteRedact(&mut range, "10", On);
    range.push_str(", ");
    WriteRedact(&mut range, "20", On);
    range.push_str(", ");
    WriteRedact(&mut range, "30", On);
    range.push(')');
    assert_eq!(range, "in(test.t2.b, ?, ?, ?)");
}

// test_join_not_supported_by_tiflash_redact_modes 对应 TestJoinNotSupportedByTiFlash。
/// TiFlash 不支持的 join 谓词场景下，ON/MARKER 对 dayofmonth 比较常量的脱敏。
#[test]
fn test_join_not_supported_by_tiflash_redact_modes() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists table_1", Vec::new());
    tk.MustExec(
        "create table table_1(id int not null, bit_col bit(2) not null, datetime_col datetime not null, index idx(id, bit_col, datetime_col))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into table_1 values(1,b'1','2020-01-01 00:00:00'),(2,b'0','2020-01-01 00:00:00')",
        Vec::new(),
    );
    tk.MustExec("analyze table table_1", Vec::new());
    assert!(domain.table_by_name("test", "table_1").is_ok());

    tk.MustExec(
        "insert into mysql.expr_pushdown_blacklist values('dayofmonth', 'tiflash', '')",
        Vec::new(),
    );
    tk.MustExec("admin reload expr_pushdown_blacklist", Vec::new());
    tk.MustExec("set global tidb_redact_log='ON'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='ON'", Vec::new());
    let sql = "explain format = 'plan_tree' select * from table_1 a left join table_1 b on a.id = b.id and dayofmonth(a.datetime_col) > 100";
    let on_plan = tk.MustQuery(sql, Vec::new()).String();
    assert!(
        on_plan.contains("MergeJoin root  left outer join"),
        "{on_plan}"
    );
    assert!(
        on_plan.contains("gt(dayofmonth(test.table_1.datetime_col), ?)"),
        "{on_plan}"
    );
    let mut on_condition = String::new();
    on_condition.push_str("gt(dayofmonth(test.table_1.datetime_col), ");
    WriteRedact(&mut on_condition, "100", On);
    on_condition.push(')');
    assert_eq!(
        on_condition, "gt(dayofmonth(test.table_1.datetime_col), ?)",
        "ON mode must redact the unsupported TiFlash join condition"
    );
    tk.MustExec("set global tidb_redact_log='MARKER'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='MARKER'", Vec::new());
    let marker_plan = tk.MustQuery(sql, Vec::new()).String();
    assert!(marker_plan.contains("MergeJoin root  left outer join"));
    assert!(marker_plan.contains("gt(dayofmonth(test.table_1.datetime_col), ‹100›)"));
    assert_eq!(
        format!(
            "gt(dayofmonth(test.table_1.datetime_col), {})",
            RedactString(Marker, "100")
        ),
        "gt(dayofmonth(test.table_1.datetime_col), ‹100›)"
    );
}

// test_redact_tiflash_window_frame_literals 对应 TestRedactTiFlash：
// window range between 3 preceding and 0 following 在 ON/MARKER 下的字面量脱敏。
/// TiFlash 窗口帧 `range between N preceding and M following` 的字面量脱敏。
#[test]
fn test_redact_tiflash_window_frame_literals() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table first_range(p int not null, o int not null, v int not null, o_datetime datetime not null, o_time time not null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into first_range (p, o, v, o_datetime, o_time) values (0, 0, 0, '2023-9-20 11:17:10', '11:17:10')",
        Vec::new(),
    );
    tk.MustExec(
        "create table first_range_d64(p int not null, o decimal(17,1) not null, v int not null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into first_range_d64 (p, o, v) values (0, 0.1, 0), (1, 1.0, 1), (1, 2.1, 2), (1, 4.1, 4), (1, 8.1, 8), (2, 0.0, 0), (2, 3.1, 3), (2, 10.0, 10), (2, 13.1, 13), (2, 15.1, 15), (3, 1.1, 1), (3, 2.9, 3), (3, 5.1, 5), (3, 9.1, 9), (3, 15.0, 15), (3, 20.1, 20), (3, 31.1, 31)",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_allow_mpp=1", Vec::new());
    tk.MustExec("set @@tidb_enforce_mpp=1", Vec::new());
    tk.MustExec("set @@tidb_isolation_read_engines = 'tiflash'", Vec::new());
    tk.MustExec("set @@tidb_max_tiflash_threads=20", Vec::new());
    tk.MustExec("alter table first_range set tiflash replica 1", Vec::new());
    tk.MustExec(
        "alter table first_range_d64 set tiflash replica 1",
        Vec::new(),
    );

    let mut on_frame = String::from("range between ");
    WriteRedact(&mut on_frame, "3", On);
    on_frame.push_str(" preceding and ");
    WriteRedact(&mut on_frame, "0", On);
    on_frame.push_str(" following");
    assert_eq!(on_frame, "range between ? preceding and ? following");

    let marker_frame = format!(
        "range between {} preceding and {} following",
        RedactString(Marker, "3"),
        RedactString(Marker, "0")
    );
    assert_eq!(
        marker_frame,
        "range between ‹3› preceding and ‹0› following"
    );

    let sql = "explain format='plan_tree' select *, first_value(v) over (partition by p order by o range between 3 preceding and 0 following) as a from first_range";
    tk.MustExec("set global tidb_redact_log='ON'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='ON'", Vec::new());
    let on_plan = tk.MustQuery(sql, Vec::new()).Rows();
    let on_plan_text = on_plan
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(on_plan_text.contains("Window"), "{on_plan_text}");
    assert!(on_plan_text.contains("Exchange"), "{on_plan_text}");
    assert!(on_plan_text.contains("TableFullScan"), "{on_plan_text}");
    assert!(
        on_plan_text.contains("range between ? preceding and ? following"),
        "{on_plan_text}"
    );
    assert!(!on_plan_text.contains("IndexReader root  window input"));
    tk.MustExec("set global tidb_redact_log='MARKER'", Vec::new());
    tk.MustExec("set @@session.tidb_redact_log='MARKER'", Vec::new());
    let marker_plan = tk.MustQuery(sql, Vec::new()).Rows();
    let marker_plan_text = marker_plan
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(marker_plan_text.contains("Window"));
    assert!(marker_plan_text.contains("Exchange"));
    assert!(marker_plan_text.contains("TableFullScan"));
    assert!(marker_plan_text.contains("range between ‹3› preceding and ‹0› following"));
    assert!(!marker_plan_text.contains("IndexReader root  window input"));
}

// Go TestRedactExplain checks whole plan rows, including the database and
// partition names. Also exercise LIST COLUMNS and static pruning, whose
// predicate constants use the same Constant.ExplainInfo contract.
#[test]
fn test_list_partition_redaction_preserves_plan_identifiers() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("create database redact_partition", Vec::new());
    tk.MustExec("use redact_partition", Vec::new());
    for (table, partition_key) in [("tlist", "(a)"), ("tcollist", "columns(a)")] {
        tk.MustExec(
            &format!(
                "create table {table} (a int, b int) partition by list {partition_key} (
                 partition p0 values in (0, 1, 2), partition p1 values in (3, 4, 5),
                 partition p2 values in (6, 7, 8), partition p3 values in (9, 10, 11),
                 partition p4 values in (-1))"
            ),
            Vec::new(),
        );
        for (mode, two, seven) in [(Off, "2", "7"), (Marker, "‹2›", "‹7›"), (On, "?", "?")]
        {
            tk.MustExec(
                &format!("set @@session.tidb_redact_log='{mode}'"),
                Vec::new(),
            );
            for prune in ["dynamic", "static"] {
                tk.MustExec(
                    &format!("set @@tidb_partition_prune_mode='{prune}'"),
                    Vec::new(),
                );
                for (value, literal, partition) in [(2, two, "p0"), (7, seven, "p2")] {
                    let plan = tk.MustQuery(
                        &format!("explain format='plan_tree' select * from {table} where a in ({value})"),
                        Vec::new(),
                    ).Rows().into_iter().map(|row| row.join(" ")).collect::<Vec<_>>();
                    let (root, access) = if prune == "dynamic" {
                        (
                            format!("TableReader root partition:{partition} data:Selection"),
                            format!("table:{table}"),
                        )
                    } else {
                        (
                            "TableReader root  data:Selection".to_owned(),
                            format!("table:{table}, partition:{partition}"),
                        )
                    };
                    assert_eq!(
                        plan,
                        vec![
                            root,
                            format!(
                                "└─Selection cop[tikv]  eq(redact_partition.{table}.a, {literal})"
                            ),
                            format!(
                                "  └─TableFullScan cop[tikv] {access} keep order:false, stats:pseudo"
                            ),
                        ],
                        "{table}, {mode}, {prune}, {value}"
                    );
                }
            }
        }
    }
}
