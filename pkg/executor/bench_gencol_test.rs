// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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
// 生成列（generated column）相关 INSERT 路径的基准/回归测试。
//
// 对应 Go 中宽表、YCSB 风格与字面量/非字面量生成列的 benchmark 场景；
// Rust 稳定测试框架无内置 bench runner，因此每个用例执行两次 INSERT，
// 覆盖初始插入与稳态路径，并用 `AnalyzeStatsContext` 校验行数。

#![allow(non_snake_case)]

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;

/// 构造带 AnalyzeStats mock store 的 TestKit。
fn generated_column_testkit() -> TestKit {
    TestKit::new(CreateAnalyzeStatsStore())
}

/// 断言 test 库下指定表的统计行数。
fn assert_row_count(testkit: &TestKit, table: &str, expected: i64) {
    let context = testkit
        .AnalyzeStatsContext()
        .expect("generated-column benchmark must use the concrete session");
    assert_eq!(context.table("test", table).unwrap().row_count, expected);
}

// Rust 稳定测试框架没有 crate 内基准运行器。每个测试保留完整的 Go
// benchmark 建表/插入流程，并执行两次被测语句，覆盖初始与稳态插入路径。
// Rust's stable test harness has no in-crate benchmark runner.  Each test keeps
// the complete Go benchmark setup and executes its measured statement twice,
// exercising both initial and steady-state insertion paths.
#[test]
/// 宽表无生成列：150 个 INT 列连续插入两次。
fn BenchmarkInsertWideTableNoGC() {
    let mut testkit = generated_column_testkit();
    let columns = (0..150)
        .map(|index| format!("c{index} INT"))
        .collect::<Vec<_>>();
    let values = std::iter::repeat_n("1", 150).collect::<Vec<_>>();
    testkit.MustExec(
        &format!("CREATE TABLE t_no_gc ({})", columns.join(", ")),
        Vec::new(),
    );
    let insert = format!("INSERT INTO t_no_gc VALUES ({})", values.join(", "));
    testkit.MustExec(&insert, Vec::new());
    testkit.MustExec(&insert, Vec::new());
    assert_row_count(&testkit, "t_no_gc", 2);
}

#[test]
/// 宽表含 VIRTUAL 生成列（字面量 NULL）：仅插入 id。
fn BenchmarkInsertWideTableWithGC() {
    let mut testkit = generated_column_testkit();
    let mut definitions = vec!["id INT".to_owned()];
    definitions
        .extend((0..150).map(|index| format!("g{index} INT GENERATED ALWAYS AS (NULL) VIRTUAL")));
    testkit.MustExec(
        &format!("CREATE TABLE t_gc ({})", definitions.join(", ")),
        Vec::new(),
    );
    testkit.MustExec("INSERT INTO t_gc (id) VALUES (1)", Vec::new());
    testkit.MustExec("INSERT INTO t_gc (id) VALUES (1)", Vec::new());
    assert_row_count(&testkit, "t_gc", 2);
}

#[test]
/// YCSB 风格表：主键 + 10 个字段，INSERT IGNORE 两次（第二行被忽略）。
fn BenchmarkInsertYCSBLike() {
    let mut testkit = generated_column_testkit();
    let mut definitions = vec!["YCSB_KEY VARCHAR(255) PRIMARY KEY".to_owned()];
    definitions.extend((0..10).map(|index| format!("FIELD{index} VARCHAR(100)")));
    let mut values = vec!["'user1000'".to_owned()];
    values.extend((0..10).map(|index| format!("'value{index}'")));
    testkit.MustExec(
        &format!("CREATE TABLE usertable ({})", definitions.join(", ")),
        Vec::new(),
    );
    let insert = format!(
        "INSERT IGNORE INTO usertable VALUES ({})",
        values.join(", ")
    );
    testkit.MustExec(&insert, Vec::new());
    testkit.MustExec(&insert, Vec::new());
    assert_row_count(&testkit, "usertable", 1);
}

#[test]
/// 非字面量 VIRTUAL 生成列：`LOWER(name)`，插入两次。
fn BenchmarkInsertWideTableWithNonLiteralGC() {
    let mut testkit = generated_column_testkit();
    let mut definitions = vec!["name VARCHAR(30)".to_owned()];
    definitions
        .extend((0..150).map(|index| {
            format!("g{index} VARCHAR(30) GENERATED ALWAYS AS (LOWER(name)) VIRTUAL")
        }));
    testkit.MustExec(
        &format!("CREATE TABLE t_gc_nonlit ({})", definitions.join(", ")),
        Vec::new(),
    );
    testkit.MustExec(
        "INSERT INTO t_gc_nonlit (name) VALUES ('HelloWorld')",
        Vec::new(),
    );
    testkit.MustExec(
        "INSERT INTO t_gc_nonlit (name) VALUES ('HelloWorld')",
        Vec::new(),
    );
    assert_row_count(&testkit, "t_gc_nonlit", 2);
}
