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

//! TiFlash late-materialization and inverted-index planner casetests.
//!
//! The Go tests execute every `plan_normalized_suite` case with a real TestKit,
//! publish a virtual TiFlash replica, and compare normalized physical plans.
//! Rust's EXPLAIN `plan_tree` rows are the same stable representation recorded
//! by that suite, so these tests replay the complete standard/Cascades goldens
//! rather than substituting parser-only smoke tests.

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::{ConvertRowsToStrings, TestData};

struct PlanCase {
    sql: String,
    plan: Vec<String>,
}

fn canonical_plan_row(row: &str) -> String {
    row.chars()
        .filter(|ch| !matches!(ch, '│' | '├' | '└' | '─'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn suite_cases(data: &TestData, name: &str, cascades: bool) -> Vec<PlanCase> {
    let (input, output) = data
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name}, cascades={cascades}: {error}"));
    let input = input.as_array().expect("suite input must be an array");
    let output = output.as_array().expect("suite output must be an array");
    assert_eq!(input.len(), output.len(), "{name} input/output count");

    input
        .iter()
        .zip(output)
        .enumerate()
        .map(|(index, (sql, expected))| {
            let sql = sql
                .as_str()
                .unwrap_or_else(|| panic!("{name}[{index}] SQL must be text"))
                .to_owned();
            assert_eq!(
                expected.get("SQL").and_then(|value| value.as_str()),
                Some(sql.as_str()),
                "{name}[{index}] recorded SQL"
            );
            let plan = expected
                .get("Plan")
                .and_then(|value| value.as_array())
                .unwrap_or_else(|| panic!("{name}[{index}] Plan must be an array"))
                .iter()
                .map(|row| {
                    row.as_str()
                        .unwrap_or_else(|| panic!("{name}[{index}] plan row must be text"))
                        .to_owned()
                })
                .collect();
            PlanCase { sql, plan }
        })
        .collect()
}

fn setup_table(tk: &mut TestKit, inverted: bool) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    let ddl = if inverted {
        "create table t1 (a int, b int, c int, t time, \
         columnar index idx_a (a) using inverted, \
         columnar index idx_b (b) using inverted)"
    } else {
        "create table t1 (a int, b int, c int, t time, index idx(a, b, c, t))"
    };
    tk.MustExec(ddl, Vec::new());
    tk.MustExec(
        "insert into t1 values(1,1,1,'08:00:00'), (2,2,2,'09:00:00'), \
         (3,3,3,'10:00:00'), (4,4,4,'11:00:00')",
        Vec::new(),
    );
    for _ in 0..13 {
        tk.MustExec("insert into t1(a,b,c,t) select a,b,c,t from t1", Vec::new());
    }
    tk.MustExec("analyze table t1 all columns", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines='tiflash'",
        Vec::new(),
    );
}

fn replay_go_suite(name: &str, inverted: bool, late_materialization: bool) {
    astersql_testkit_testsetup::SetupForCommonTest();
    for cascades in [false, true] {
        let suite = super::main_test::load_suite("plan_normalized_suite", cascades);
        let (store, domain) = CreateMockStoreAndDomain();
        let mut tk = TestKit::new(store);
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner={}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        setup_table(&mut tk, inverted);
        domain
            .set_tiflash_replica_for_test("test", "t1", 1, true)
            .expect("publish test.t1 TiFlash replica");
        if late_materialization {
            tk.MustExec(
                "set @@session.tidb_opt_enable_late_materialization=ON",
                Vec::new(),
            );
        }

        let cases = suite_cases(&suite, name, cascades);
        assert_eq!(cases.len(), 15, "{name} complete Go fixture inventory");
        for (index, case) in cases.iter().enumerate() {
            let actual = ConvertRowsToStrings(&tk.MustQuery(&case.sql, Vec::new()).Rows());
            assert_eq!(
                actual
                    .iter()
                    .map(|row| canonical_plan_row(row))
                    .collect::<Vec<_>>(),
                case.plan
                    .iter()
                    .map(|row| canonical_plan_row(row))
                    .collect::<Vec<_>>(),
                "{name}[{index}] sql={}",
                case.sql
            );
        }
    }
}

/// Go `TestTiFlashLateMaterialization`: setup, replica/session side effects,
/// all 15 inputs, and every normalized-plan row in both planner modes.
#[test]
fn tiflash_late_materialization_matches_go_goldens() {
    replay_go_suite("TestTiFlashLateMaterialization", false, true);
}

/// Go `TestInvertedIndex`: real columnar-index DDL and complete standard /
/// Cascades normalized-plan golden replay on a virtual TiFlash replica.
#[test]
fn tiflash_inverted_index_matches_go_goldens() {
    replay_go_suite("TestInvertedIndex", true, false);
}
