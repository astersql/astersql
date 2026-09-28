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

//! LIST 与 LIST COLUMNS 分区的 TestKit SQL 集成测试。
//!
//! 测试以写入相同数据的普通表作为结果基准，对比分区表在静态、动态裁剪模式下的
//! 排序限量、聚合、视图与事务行为；输入数据保持确定性，便于稳定复现失败。

use astersql_testkit::{Rows, TestKit};

fn new_testkit() -> TestKit {
    crate::support::new_testkit()
}

/// 分别执行基准查询与待测查询，排序后比较结果以排除扫描顺序差异。
fn compare_sorted(tk: &TestKit, control_sql: &str, candidate_sql: &str) {
    let mut control = tk.MustQuery(control_sql, Vec::new());
    control.Sort();
    let mut candidate = tk.MustQuery(candidate_sql, Vec::new());
    candidate.Sort();
    assert_eq!(control.Rows(), candidate.Rows(), "{candidate_sql}");
}

/// 创建使用同一分区范围的 LIST、LIST COLUMNS 分区表及普通对照表。
fn prepare_three_list_tables(tk: &mut TestKit, database: &str) {
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    tk.MustExec("drop table if exists tlist", Vec::new());
    let parts = format!(
        "partition p0 values in {}, partition p1 values in {}, partition p2 values in {}, partition p3 values in {}, partition p4 values in {}",
        gen_list_partition(0, 20),
        gen_list_partition(20, 40),
        gen_list_partition(40, 60),
        gen_list_partition(60, 80),
        gen_list_partition(80, 100),
    );
    tk.MustExec(
        &format!("create table tlist (a int, b int) partition by list(a) ({parts})"),
        Vec::new(),
    );
    tk.MustExec(
        &format!("create table tcollist (a int, b int) partition by list columns(a) ({parts})"),
        Vec::new(),
    );
    tk.MustExec("create table tnormal (a int, b int)", Vec::new());
}

#[test]
/// 验证分区裁剪模式不影响不同排序列和 LIMIT 组合的查询结果。
pub fn test_list_partition_order_limit() {
    let mut tk = new_testkit();
    prepare_three_list_tables(&mut tk, "list_partition_order_limit");
    let values = deterministic_pairs_for_order_limit();
    for table in ["tlist", "tcollist", "tnormal"] {
        tk.MustExec(&format!("insert into {table} values {values}"), Vec::new());
    }
    let conditions = [
        "where a > 0",
        "where b > 17",
        "where a > 50",
        "where b > 99",
        "where b > 0",
        "where a > 17",
        "where b > 50",
        "where a > 99",
    ];
    for (case_index, (order_col, limit_num)) in ["a", "b"]
        .into_iter()
        .flat_map(|order_col| {
            ["1", "5", "20", "100"]
                .into_iter()
                .map(move |limit_num| (order_col, limit_num))
        })
        .enumerate()
    {
        let condition = conditions[case_index];
        let control =
            format!("select * from tnormal {condition} order by {order_col} limit {limit_num}");
        for (mode, table) in [
            ("dynamic", "tlist"),
            ("static", "tlist"),
            ("dynamic", "tcollist"),
            ("static", "tcollist"),
        ] {
            tk.MustExec(
                &format!("set @@tidb_partition_prune_mode = '{mode}'"),
                Vec::new(),
            );
            compare_sorted(
                &tk,
                &control,
                &format!(
                    "select * from {table} {condition} order by {order_col} limit {limit_num}"
                ),
            );
        }
    }
}

#[test]
/// 验证两类列表分区在静态、动态裁剪下的分组聚合结果均与普通表一致。
pub fn test_list_partition_agg() {
    let mut tk = new_testkit();
    prepare_three_list_tables(&mut tk, "list_partition_agg");
    let values = deterministic_pairs_for_agg();
    for table in ["tlist", "tcollist", "tnormal"] {
        tk.MustExec(&format!("insert into {table} values {values}"), Vec::new());
    }
    for aggregate in ["min", "max", "sum", "count"] {
        for _ in 0..2 {
            let control = format!("select a, {aggregate}(b) from tnormal group by a");
            for (mode, table) in [
                ("dynamic", "tlist"),
                ("static", "tlist"),
                ("dynamic", "tcollist"),
                ("static", "tcollist"),
            ] {
                tk.MustExec(
                    &format!("set @@tidb_partition_prune_mode = '{mode}'"),
                    Vec::new(),
                );
                compare_sorted(
                    &tk,
                    &control,
                    &format!("select a, {aggregate}(b) from {table} group by a"),
                );
            }
        }
    }
}

#[test]
/// 验证经由视图计算表达式时，LIST 与 LIST COLUMNS 分区表仍保持普通表语义。
pub fn test_list_partition_view() {
    let mut tk = new_testkit();
    tk.MustExec("create database list_partition_view", Vec::new());
    tk.MustExec("use list_partition_view", Vec::new());
    tk.MustExec("create table tlist (a int, b int) partition by list (a) (partition p0 values in (0, 1, 2, 3, 4), partition p1 values in (5, 6, 7, 8, 9), partition p2 values in (10, 11, 12, 13, 14))", Vec::new());
    tk.MustExec(
        "create definer='root'@'localhost' view vlist as select a*2 as a2, a+b as ab from tlist",
        Vec::new(),
    );
    tk.MustExec("create table tnormal (a int, b int)", Vec::new());
    tk.MustExec("create definer='root'@'localhost' view vnormal as select a*2 as a2, a+b as ab from tnormal", Vec::new());
    for (a, b) in deterministic_view_rows() {
        tk.MustExec(&format!("insert into tlist values ({a}, {b})"), Vec::new());
        tk.MustExec(
            &format!("insert into tnormal values ({a}, {b})"),
            Vec::new(),
        );
    }
    compare_sorted(&tk, "select * from vnormal", "select * from vlist");

    tk.MustExec("create table tcollist (a int, b int) partition by list columns (a) (partition p0 values in (0, 1, 2, 3, 4), partition p1 values in (5, 6, 7, 8, 9), partition p2 values in (10, 11, 12, 13, 14))", Vec::new());
    tk.MustExec("create definer='root'@'localhost' view vcollist as select a*2 as a2, a+b as ab from tcollist", Vec::new());
    tk.MustExec("truncate tnormal", Vec::new());
    for (a, b) in deterministic_view_rows() {
        tk.MustExec(
            &format!("insert into tcollist values ({a}, {b})"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("insert into tnormal values ({a}, {b})"),
            Vec::new(),
        );
    }
    compare_sorted(&tk, "select * from vnormal", "select * from vcollist");
}

#[test]
/// 在可复现的混合事务序列中交替查询、插入、提交与回滚，持续校验三张表一致。
pub fn test_list_partition_random_transaction() {
    let mut tk = new_testkit();
    prepare_three_list_tables(&mut tk, "list_partition_random_tran");
    let mut seed = 0x1234_5678_u32;
    let mut in_transaction = false;
    let mut commits = 0;
    let mut rollbacks = 0;
    for _ in 0..50 {
        match next_deterministic(&mut seed, 4) {
            0 if !in_transaction => {
                tk.MustExec("begin", Vec::new());
                in_transaction = true;
            }
            1 => {
                let low = next_deterministic(&mut seed, 50);
                let high = 50 + next_deterministic(&mut seed, 50);
                let condition = format!("where a >= {low} and a <= {high}");
                compare_sorted(
                    &tk,
                    &format!("select * from tnormal {condition}"),
                    &format!("select * from tlist {condition}"),
                );
                compare_sorted(
                    &tk,
                    &format!("select * from tnormal {condition}"),
                    &format!("select * from tcollist {condition}"),
                );
            }
            2 => {
                let a = next_deterministic(&mut seed, 100);
                let b = next_deterministic(&mut seed, 100);
                for table in ["tnormal", "tlist", "tcollist"] {
                    tk.MustExec(
                        &format!("insert into {table} values ({a}, {b})"),
                        Vec::new(),
                    );
                }
            }
            3 if in_transaction => {
                let statement = if next_deterministic(&mut seed, 2) == 0 {
                    "commit"
                } else {
                    "rollback"
                };
                tk.MustExec(statement, Vec::new());
                if statement == "commit" {
                    commits += 1;
                } else {
                    rollbacks += 1;
                }
                in_transaction = false;
            }
            _ => {}
        }
    }
    if in_transaction {
        // 避免最后一次 BEGIN 将未结束事务泄漏到测试清理阶段。
        tk.MustExec("rollback", Vec::new());
    }
    assert!(commits > 0, "transaction workload must exercise commit");
    assert!(rollbacks > 0, "transaction workload must exercise rollback");
}

#[test]
/// 覆盖日常基准中的 RANGE COLUMNS 动态裁剪查询路径。
pub fn test_bench_daily() {
    let mut tk = new_testkit();
    tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk.MustExec("create schema rcb", Vec::new());
    tk.MustExec("use rcb", Vec::new());
    tk.MustExec("create table t (c1 int primary key clustered,c2 varchar(255)) partition by range columns (c1) interval (10000) first partition less than (10000) last partition less than (5120000)", Vec::new());
    for value in ["0", "7919", "15838"] {
        tk.MustExec(&format!("select * from t where c1 = {value}"), Vec::new());
    }
}

/// 生成半开区间 `[begin, end)` 对应的 LIST 分区值列表。
pub fn gen_list_partition(begin: i32, end: i32) -> String {
    let values = (begin..end)
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    format!("({})", values.join(", "))
}

/// 生成排序与 LIMIT 用例共享的确定性批量插入值。
pub fn deterministic_pairs_for_order_limit() -> String {
    (0..50)
        .map(|i| format!("({}, {})", i * 2 + (i % 2), i * 2 + ((i + 1) % 2)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 生成聚合用例共享的确定性批量插入值，并让分组键出现非顺序分布。
pub fn deterministic_pairs_for_agg() -> String {
    (0..50)
        .map(|i| format!("({}, {})", (i * 37) % 100, (i * 53) % 100))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 生成视图用例共享的确定性行，且保证分区键落在已定义的取值范围内。
pub fn deterministic_view_rows() -> Vec<(i32, i32)> {
    (0..10).map(|i| ((i * 7) % 15, (i * 31) % 100)).collect()
}

/// 固定种子的伪随机序列；每次调用独立取值，复现 Go 测试为操作参数再次调用 rand 的语义。
fn next_deterministic(seed: &mut u32, upper: i32) -> i32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    ((*seed >> 16) % upper as u32) as i32
}

#[test]
fn list_partition_helpers_match_go_shapes() {
    assert_eq!(gen_list_partition(0, 4), "(0, 1, 2, 3)");
    let result = Rows(&["1 2", "3 4"]);
    assert_eq!(result, Rows(&["1 2", "3 4"]));
}
