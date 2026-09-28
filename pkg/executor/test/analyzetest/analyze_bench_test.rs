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

// ANALYZE 分区扫描基准的可执行验证用例。
//
// 对应 Go `pkg/executor/test/analyzetest/analyze_bench_test.go`。
// Go 基准反复执行包含 1000 个分区和 100000 行数据的
// `ANALYZE TABLE ... ALL COLUMNS`。Rust 稳定测试不使用 nightly benchmark
// harness，因此用显式忽略的重型测试保留完全相同的负载，并用快速测试锁定
// SQL 形状和迭代契约。

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

const PARTITION_COUNT: usize = 1000;
const PARTITION_INTERVAL: usize = 100;
const ROW_COUNT: usize = 100_000;

fn create_partition_table_sql() -> String {
    let mut sql = String::from(
        "create table t(a int,b varchar(100),c int,index idx_c(c)) partition by range (a) (",
    );
    for boundary in
        (PARTITION_INTERVAL..PARTITION_COUNT * PARTITION_INTERVAL).step_by(PARTITION_INTERVAL)
    {
        sql.push_str(&format!(
            "partition p{boundary} values less than ({boundary}),"
        ));
    }
    let max_boundary = PARTITION_COUNT * PARTITION_INTERVAL;
    sql.push_str(&format!(
        "partition p{max_boundary} values less than maxvalue)"
    ));
    sql
}

fn insert_rows_sql() -> String {
    let mut sql = String::from("insert into t (a,b,c) values(0, 'abc', 0)");
    for row in 1..ROW_COUNT {
        sql.push_str(&format!(" ,({row}, 'abc', {row})"));
    }
    sql.push(';');
    sql
}

fn repeat_analyze(iterations: usize, mut analyze: impl FnMut()) {
    for _ in 0..iterations {
        analyze();
    }
}

fn run_analyze_partition_benchmark(iterations: usize) {
    assert!(iterations > 0, "benchmark requires at least one iteration");
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec(&create_partition_table_sql(), Vec::new());
    tk.MustExec(&insert_rows_sql(), Vec::new());
    repeat_analyze(iterations, || {
        tk.MustExec("analyze table t all columns", Vec::new());
    });
}

/// Go 的 benchmark 不会被普通 `go test` 执行；Rust 对应入口同样只在显式请求时运行。
#[test]
#[ignore = "slow: exact Go benchmark workload (1000 partitions, 100000 rows)"]
fn analyze_partition_benchmark_executes_exact_go_workload() {
    run_analyze_partition_benchmark(1);
}

#[test]
fn benchmark_workload_shape_matches_go() {
    assert_eq!(PARTITION_COUNT, 1000);
    assert_eq!(PARTITION_INTERVAL, 100);
    assert_eq!(ROW_COUNT, 100_000);

    let create_sql = create_partition_table_sql();
    assert_eq!(create_sql.matches("partition p").count(), PARTITION_COUNT);
    assert!(create_sql.contains("partition p100 values less than (100),"));
    assert!(create_sql.contains("partition p99900 values less than (99900),"));
    assert!(create_sql.ends_with("partition p100000 values less than maxvalue)"));

    let insert_sql = insert_rows_sql();
    assert!(insert_sql.starts_with("insert into t (a,b,c) values(0, 'abc', 0)"));
    assert_eq!(insert_sql.matches("'abc'").count(), ROW_COUNT);
    assert!(insert_sql.ends_with(" ,(99999, 'abc', 99999);"));
}

#[test]
fn benchmark_iteration_count_matches_go_b_n_loop() {
    let mut executions = 0;
    repeat_analyze(7, || executions += 1);
    assert_eq!(executions, 7);
}
