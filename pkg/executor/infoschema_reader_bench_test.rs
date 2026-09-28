// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Port of `BenchmarkInfoschemaTables` from
//! `infoschema_reader_bench_test.go`.

use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit::{Database, TestKit};

fn prepare_data(testkit: &mut TestKit) {
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t1 (a int, b int, c int)", Vec::new());
    testkit.MustExec("create table t2 (a int, b int, c int)", Vec::new());
    testkit.MustExec("create table t3 (a int, b int, c int)", Vec::new());
    testkit.MustExec("insert into t1 values (1, 2, 3)", Vec::new());
    testkit.MustExec("insert into t2 values (4, 5, 6)", Vec::new());
    testkit.MustExec("insert into t3 values (7, 8, 9)", Vec::new());
    testkit.MustExec("analyze table t1 all columns", Vec::new());
    testkit.MustExec("analyze table t2 all columns", Vec::new());
    testkit.MustExec("analyze table t3 all columns", Vec::new());
}

/// Exercise every information-schema projection used by the Go benchmark
/// repeatedly against the same analyzed catalog.
#[test]
fn benchmark_infoschema_tables_queries_remain_stable() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store.clone());
    prepare_data(&mut testkit);

    let cases = [
        (
            "select all",
            "select * from information_schema.tables where table_schema = 'test'",
        ),
        (
            "select basic",
            "select table_name from information_schema.tables where table_schema = 'test'",
        ),
        (
            "select without table rows",
            "select TABLE_NAME, CREATE_TIME from information_schema.tables where table_schema = 'test'",
        ),
        (
            "select with table rows",
            "select TABLE_NAME, TABLE_ROWS from information_schema.tables where table_schema = 'test'",
        ),
    ];

    for (case_name, sql) in cases {
        for iteration in 0..10 {
            let rows = testkit.MustQuery(sql, Vec::new()).Rows();
            assert_eq!(
                rows.len(),
                3,
                "{case_name} returned an unstable row count at iteration {iteration}"
            );
        }
    }

    Database::close(store.as_ref()).unwrap();
}
