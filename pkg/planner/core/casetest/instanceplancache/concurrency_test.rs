// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

//! 实例级执行计划缓存的并发回归测试。
//!
//! 测试让多个独立会话共享同一存储，在随机参数下反复执行预处理语句，覆盖点查、
//! 批量点查、分区表、索引扫描和索引连接等路径；部分用例还会对照普通 SQL 与
//! 预处理 SQL 的结果，验证跨会话复用缓存计划不会改变查询语义。

use std::sync::Arc;
use std::thread;

use astersql_testkit::TestKit;

use super::support::{Harness, exec, query, rand, rows};

#[derive(Debug, Clone, PartialEq, Eq)]
/// 同一条语句的普通 SQL 与预处理 SQL 表示，用于逐项对照执行结果。
pub struct TestStmt {
    /// 直接执行的 SQL，也是查询结果的基准。
    pub normal_stmt: String,
    /// 创建预处理语句的 SQL。
    pub prep_stmt: String,
    /// 为预处理语句绑定参数的 SQL。
    pub set_stmt: String,
    /// 使用已绑定参数执行预处理语句的 SQL。
    pub exec_stmt: String,
}

/// 判断语句是否为查询语句。
pub fn isDQL(stmt: &str) -> bool {
    stmt.starts_with("select")
}

/// 判断语句是否为数据修改语句。
pub fn isDML(stmt: &str) -> bool {
    ["insert", "update", "delete"]
        .iter()
        .any(|prefix| stmt.starts_with(prefix))
}

/// 判断语句是否为事务边界语句。
pub fn isTxn(stmt: &str) -> bool {
    ["begin", "commit", "rollback"]
        .iter()
        .any(|prefix| stmt.starts_with(prefix))
}

/// 在共享存储上创建十个独立会话，并发执行同一工作负载。
fn concurrent<F>(store: &Arc<astersql_testkit::mockstore::AnalyzeStatsStore>, worker: F)
where
    F: Fn(TestKit) + Send + Sync,
{
    thread::scope(|scope| {
        let worker = &worker;
        for _ in 0..10 {
            let store = Arc::clone(store);
            scope.spawn(move || worker(TestKit::new(store)));
        }
    });
}

/// 顺序执行一组对照语句，并核验普通 SQL 与预处理 SQL 的结果一致。
fn run_statements(mut tk: TestKit, statements: &[TestStmt]) {
    exec(&mut tk, "use test");
    for statement in statements {
        if isTxn(&statement.normal_stmt) {
            exec(&mut tk, &statement.normal_stmt);
        } else if isDQL(&statement.normal_stmt) {
            let mut stable_result = None;
            for _ in 0..32 {
                let mut expected = query(&tk, &statement.normal_stmt);
                exec(&mut tk, &statement.prep_stmt);
                exec(&mut tk, &statement.set_stmt);
                let mut actual = query(&tk, &statement.exec_stmt);
                let mut confirmed = query(&tk, &statement.normal_stmt);
                expected.Sort();
                actual.Sort();
                confirmed.Sort();
                if expected.Equal(confirmed.Rows()) {
                    stable_result = Some((expected, actual));
                    break;
                }
                thread::yield_now();
            }
            let (mut expected, actual) = stable_result
                .unwrap_or_else(|| panic!("normal query did not reach a stable visibility window"));
            expected.AddComment(&format!(
                "normal SQL: {}; prepared SQL: {}; bindings: {}",
                statement.normal_stmt, statement.prep_stmt, statement.set_stmt
            ));
            expected.Check(actual.Rows());
        } else if isDML(&statement.normal_stmt) {
            if let Err(error) = tk.Exec(&statement.normal_stmt, Vec::new()) {
                // 多个工作线程更新同一行时允许出现数据库层面的预期死锁。
                if error.to_string().contains("Deadlock") {
                    continue;
                }
                panic!("normal DML failed: {}: {error}", statement.normal_stmt);
            }
            exec(&mut tk, &statement.prep_stmt);
            exec(&mut tk, &statement.set_stmt);
            exec(&mut tk, &statement.exec_stmt);
        }
    }
}

/// 与 Go `testWithWorkers` 一致：DML 只交给一个随机会话，查询和事务边界广播给全部会话。
fn test_with_workers(
    store: &Arc<astersql_testkit::mockstore::AnalyzeStatsStore>,
    statements: &[TestStmt],
) {
    let mut per_worker = vec![Vec::new(); 10];
    for statement in statements {
        if isDML(&statement.normal_stmt) {
            per_worker[rand::intn(10) as usize].push(statement.clone());
        } else {
            for worker in &mut per_worker {
                worker.push(statement.clone());
            }
        }
    }
    thread::scope(|scope| {
        for statements in per_worker {
            let store = Arc::clone(store);
            scope.spawn(move || run_statements(TestKit::new(store), &statements));
        }
    });
}

/// 构造单表点查数据，并在十个会话中用随机键反复验证缓存计划的查询结果。
fn run_point_case(
    ddl: &str,
    prepare: &str,
    set: &str,
    execute: &str,
    expected: fn(usize) -> Vec<String>,
) {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(&mut harness.tk, ddl);
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t values ({i}, {i})"));
    }
    concurrent(&harness.store, move |mut tk| {
        exec(&mut tk, "use test");
        exec(&mut tk, prepare);
        for _ in 0..100 {
            let value = rand::intn(100) as usize;
            exec(&mut tk, &set.replace("{v}", &value.to_string()));
            query(&tk, execute).Check(vec![expected(value)]);
        }
    });
}

#[test]
pub fn TestInstancePlanCacheConcurrencyPointNoTxn() {
    run_point_case(
        "create table t (a int, b int, primary key(a))",
        "prepare st from 'select * from t where a=?'",
        "set @v={v}",
        "execute st using @v",
        |value| vec![value.to_string(), value.to_string()],
    );
}

#[test]
pub fn TestInstancePlanCacheConcurrencyPointMultipleColPKNoTxn() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t (a int, b int, primary key(a,b))",
    );
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t values ({i},{i})"));
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        exec(
            &mut tk,
            "prepare st from 'select * from t where a=? and b=?'",
        );
        for _ in 0..100 {
            let value = rand::intn(100);
            exec(&mut tk, &format!("set @a={value}, @b={value}"));
            query(&tk, "execute st using @a,@b")
                .Check(vec![vec![value.to_string(), value.to_string()]]);
        }
    });
}

#[test]
pub fn TestInstancePlanCacheConcurrencyBatchPointNoTxn() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t (a int, b int, primary key(a))",
    );
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t values ({i},{i})"));
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        exec(
            &mut tk,
            "prepare st from 'select a from t where a in (?,?)'",
        );
        for _ in 0..100 {
            let a = rand::intn(50);
            let b = 50 + rand::intn(50);
            exec(&mut tk, &format!("set @a={a}, @b={b}"));
            let mut expected = vec![a.to_string(), b.to_string()];
            expected.sort();
            query(&tk, "execute st using @a,@b")
                .Sort()
                .Check(rows(&expected));
        }
    });
}

#[test]
pub fn TestInstancePlanCacheBatchPointMultiColIndex() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t (a int,b int,c int,d int,primary key(a,b),unique key(c,d))",
    );
    for i in 0..100 {
        exec(
            &mut harness.tk,
            &format!("insert into t values ({i},{i},{i},{i})"),
        );
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        // 每个会话随机选择复合主键或复合唯一索引，覆盖两类批量点查计划。
        let by_primary = rand::intn(2) == 0;
        let sql = if by_primary {
            "prepare st from 'select a from t where (a,b) in ((?,?),(?,?))'"
        } else {
            "prepare st from 'select a from t where (c,d) in ((?,?),(?,?))'"
        };
        exec(&mut tk, sql);
        for _ in 0..100 {
            let a = rand::intn(50);
            let b = 50 + rand::intn(50);
            exec(&mut tk, &format!("set @a={a},@b={b}"));
            let mut expected = vec![a.to_string(), b.to_string()];
            expected.sort();
            query(&tk, "execute st using @a,@a,@b,@b")
                .Sort()
                .Check(rows(&expected));
        }
    });
}

#[test]
pub fn TestInstancePlanCacheConcurrencyPointPartitioning() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t1 (a int, primary key(a)) partition by hash(a) partitions 10",
    );
    exec(
        &mut harness.tk,
        "create table t2 (a int, primary key(a)) partition by range(a) (partition p0 values less than (10), partition p1 values less than (20), partition p2 values less than (30), partition p3 values less than (40), partition p4 values less than (50), partition p5 values less than (60), partition p6 values less than (70), partition p7 values less than (80), partition p8 values less than (90), partition p9 values less than (100))",
    );
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t1 values ({i})"));
        exec(&mut harness.tk, &format!("insert into t2 values ({i})"));
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        for _ in 0..100 {
            let table = format!("t{}", rand::intn(2) + 1);
            exec(
                &mut tk,
                &format!("prepare st from 'select * from {table} where a=?'"),
            );
            let a = rand::intn(100);
            exec(&mut tk, &format!("set @a={a}"));
            query(&tk, "execute st using @a").Check(rows(&[a.to_string()]));
        }
    });
}

#[test]
pub fn TestInstancePlanCacheTableIndexScan() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t (a int primary key,b int,key(b))",
    );
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t values ({i},{i})"));
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        for _ in 0..100 {
            // 在同一并发工作负载中交替强制主键扫描和二级索引扫描。
            let (column, index) = if rand::intn(2) == 0 {
                ("a", "primary")
            } else {
                ("b", "b")
            };
            exec(
                &mut tk,
                &format!(
                    "prepare st from 'select {column} from t use index({index}) where {column}>=? and {column}<=?'"
                ),
            );
            let a = rand::intn(50);
            let b = 50 + rand::intn(50);
            exec(&mut tk, &format!("set @a={a},@b={b}"));
            let mut expected: Vec<String> = (a..=b).map(|v| v.to_string()).collect();
            expected.sort();
            query(&tk, "execute st using @a,@b")
                .Sort()
                .Check(rows(&expected));
        }
    });
}

#[test]
pub fn TestInstancePlanCacheIndexJoin() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(&mut harness.tk, "create table t1 (a int,b int)");
    exec(&mut harness.tk, "create table t2 (a int,key(a))");
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t1 values ({i},{i})"));
        exec(&mut harness.tk, &format!("insert into t2 values ({i})"));
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        exec(
            &mut tk,
            "prepare st from 'select /*+ tidb_inlj(t2) */ t2.a from t1,t2 where t1.a=t2.a and t1.b=?'",
        );
        for _ in 0..100 {
            let value = rand::intn(100);
            exec(&mut tk, &format!("set @v={value}"));
            query(&tk, "execute st using @v").Check(rows(&[value.to_string()]));
        }
    });
}

/// 在两个结构与数据相同的数据库中分别执行普通 SQL 和预处理 SQL，隔离写入状态
/// 并验证并发复用实例级缓存时两条执行路径保持等价。
fn point_workload() {
    let mut harness = Harness::new();
    query(&harness.tk, "select @@tidb_txn_mode").Check(rows(&["pessimistic".to_owned()]));
    exec(&mut harness.tk, "create database normal");
    exec(&mut harness.tk, "create database prepared");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for db in ["normal", "prepared"] {
        exec(&mut harness.tk, &format!("use {db}"));
        exec(
            &mut harness.tk,
            "create table t1 (col1 int, col2 int, primary key(col1), unique key(col2))",
        );
        for i in 0..100 {
            exec(&mut harness.tk, &format!("insert into t1 values ({i},{i})"));
        }
    }
    let mut statements = vec![txn("begin")];
    while statements.len() < 400 {
        if rand::intn(15) == 0 {
            statements.extend([txn("commit"), txn("begin")]);
            continue;
        }
        let v1 = rand::intn(100);
        if rand::intn(2) == 0 {
            statements.push(stmt(
                format!("select col1 from normal.t1 where col1={v1}"),
                "prepare st from 'select col1 from prepared.t1 where col1=?'",
                format!("set @v1 = {v1}"),
                "execute st using @v1",
            ));
        } else {
            let v2 = rand::intn(100);
            statements.push(stmt(
                format!("select col1 from normal.t1 where col1={v1} and col2={v2}"),
                "prepare st from 'select col1 from prepared.t1 where col1=? and col2=?'",
                format!("set @v1 = {v1}, @v2 = {v2}"),
                "execute st using @v1, @v2",
            ));
        }
    }
    statements.push(txn("commit"));
    test_with_workers(&harness.store, &statements);
}

fn stmt(normal: String, prepare: &str, set: String, execute: &str) -> TestStmt {
    TestStmt {
        normal_stmt: normal,
        prep_stmt: prepare.into(),
        set_stmt: set,
        exec_stmt: execute.into(),
    }
}

fn txn(sql: &str) -> TestStmt {
    stmt(sql.into(), "", String::new(), "")
}

#[test]
pub fn TestInstancePlanCacheConcurrencyPoint() {
    point_workload();
}

#[test]
pub fn TestInstancePlanCacheConcurrencyPartitioning() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t (a int) partition by range(a) (partition p0 values less than (10), partition p1 values less than (20), partition p2 values less than (30), partition p3 values less than (40), partition p4 values less than (50), partition p5 values less than (60), partition p6 values less than (70), partition p7 values less than (80), partition p8 values less than (90), partition p9 values less than (100))",
    );
    for i in 0..100 {
        exec(&mut harness.tk, &format!("insert into t values ({i})"));
    }
    concurrent(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        for _ in 0..100 {
            match rand::intn(3) {
                0 => {
                    let v = rand::intn(100);
                    exec(&mut tk, "prepare st from 'select * from t where a=?'");
                    exec(&mut tk, &format!("set @v = {v}"));
                    query(&tk, "execute st using @v").Check(rows(&[v.to_string()]));
                }
                1 => {
                    let (v1, v2) = (rand::intn(50), 50 + rand::intn(50));
                    exec(
                        &mut tk,
                        "prepare st from 'select * from t where a in (?,?)'",
                    );
                    exec(&mut tk, &format!("set @v1={v1},@v2={v2}"));
                    let mut expected = vec![v1.to_string(), v2.to_string()];
                    expected.sort();
                    query(&tk, "execute st using @v1,@v2")
                        .Sort()
                        .Check(rows(&expected));
                }
                _ => {
                    let (v1, v2) = (rand::intn(50), 50 + rand::intn(50));
                    exec(
                        &mut tk,
                        "prepare st from 'select * from t where a between ? and ?'",
                    );
                    exec(&mut tk, &format!("set @v1={v1},@v2={v2}"));
                    let mut expected: Vec<_> = (v1..=v2).map(|v| v.to_string()).collect();
                    expected.sort();
                    query(&tk, "execute st using @v1,@v2")
                        .Sort()
                        .Check(rows(&expected));
                }
            }
        }
    });
}

#[test]
pub fn TestInstancePlanCacheConcurrencyComp() {
    mixed_read_workload();
}

#[test]
pub fn TestInstancePlanCacheConcurrencySysbench() {
    sysbench_workload();
}

/// 覆盖 Go comp 用例中的写入、点查、范围、聚合与连接形状。
fn mixed_read_workload() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "create database normal");
    exec(&mut harness.tk, "create database prepared");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for db in ["normal", "prepared"] {
        exec(&mut harness.tk, &format!("use {db}"));
        exec(
            &mut harness.tk,
            "create table t1 (col1 int,col2 int,key(col1,col2))",
        );
    }
    let gen_insert = || {
        let (col1, col2) = (rand::intn(1000), rand::intn(1000));
        stmt(
            format!("insert into normal.t1 values ({col1},{col2})"),
            "prepare st from 'insert into prepared.t1 values (?,?)'",
            format!("set @col1={col1},@col2={col2}"),
            "execute st using @col1,@col2",
        )
    };
    let gen_basic_select = || match rand::intn(2) {
        0 => match rand::intn(3) {
            0 => one_param_stmt("select * from", "where col1", "=", rand::intn(1000)),
            1 => stmt(
                "select * from normal.t1 where col1 is null".into(),
                "prepare st from 'select * from prepared.t1 where col1 is null'",
                String::new(),
                "execute st",
            ),
            _ => {
                let (v1, v2, v3) = (rand::intn(1000), rand::intn(1000), rand::intn(1000));
                stmt(
                    format!("select * from normal.t1 where col1 in ({v1},{v2},{v3})"),
                    "prepare st from 'select * from prepared.t1 where col1 in (?,?,?)'",
                    format!("set @v1={v1},@v2={v2},@v3={v3}"),
                    "execute st using @v1,@v2,@v3",
                )
            }
        },
        _ => {
            let v1 = rand::intn(1000);
            match rand::intn(4) {
                0 => two_param_stmt("select * from", "where col1 between", v1, rand::intn(1000)),
                1 => one_param_stmt("select * from", "where col1", ">", v1),
                2 => one_param_stmt("select * from", "where col1", "<=", v1),
                _ => one_param_stmt("select * from", "where col1", "!=", v1),
            }
        }
    };
    let gen_agg_select = || {
        let v1 = rand::intn(1000);
        match rand::intn(5) {
            0 | 4 => one_param_stmt("select sum(col1),col2 from", "where col1", "=", v1),
            1 => two_param_stmt(
                "select sum(col1),col2 from",
                "where col1 between",
                v1,
                rand::intn(1000),
            ),
            2 => one_param_stmt("select sum(col1),col2 from", "where col1", ">", v1),
            _ => one_param_stmt("select sum(col1),col2 from", "where col1", "<=", v1),
        }
    };
    let gen_join_select = || {
        let v1 = rand::intn(1000);
        let (normal_join, prepared_join, predicate, two_params) = match rand::intn(4) {
            0 => (
                "normal.t1 t1 join normal.t1 t2 on t1.col1=t2.col1",
                "prepared.t1 t1 join prepared.t1 t2 on t1.col1=t2.col1",
                "t1.col1=",
                false,
            ),
            1 => (
                "normal.t1 t1 join normal.t1 t2 on t1.col1=t2.col1",
                "prepared.t1 t1 join prepared.t1 t2 on t1.col1=t2.col1",
                "t1.col1 between",
                true,
            ),
            2 => (
                "normal.t1 t1 left join normal.t1 t2 on t1.col1=t2.col1",
                "prepared.t1 t1 left join prepared.t1 t2 on t1.col1=t2.col1",
                "t1.col1 >",
                false,
            ),
            _ => (
                "normal.t1 t1 join normal.t1 t2 on t1.col1>t2.col1",
                "prepared.t1 t1 join prepared.t1 t2 on t1.col1>t2.col1",
                "t1.col1 <=",
                false,
            ),
        };
        if two_params {
            let v2 = rand::intn(1000);
            stmt(
                format!("select * from {normal_join} where {predicate} {v1} and {v2}"),
                &format!(
                    "prepare st from 'select * from {prepared_join} where {predicate} ? and ?'"
                ),
                format!("set @v1={v1},@v2={v2}"),
                "execute st using @v1,@v2",
            )
        } else {
            stmt(
                format!("select * from {normal_join} where {predicate}{v1}"),
                &format!("prepare st from 'select * from {prepared_join} where {predicate}?'"),
                format!("set @v1={v1}"),
                "execute st using @v1",
            )
        }
    };
    let gen_point_select = || {
        let values = [
            rand::intn(1000),
            rand::intn(1000),
            rand::intn(1000),
            rand::intn(1000),
        ];
        match rand::intn(5) {
            0 => stmt(
                format!("select col1 from normal.t1 where col1={}", values[0]),
                "prepare st from 'select col1 from prepared.t1 where col1=?'",
                format!("set @v1={}", values[0]),
                "execute st using @v1",
            ),
            1 => stmt(
                format!(
                    "select col1 from normal.t1 where col1={} and col2={}",
                    values[0], values[1]
                ),
                "prepare st from 'select col1 from prepared.t1 where col1=? and col2=?'",
                format!("set @v1={},@v2={}", values[0], values[1]),
                "execute st using @v1,@v2",
            ),
            2 => stmt(
                format!(
                    "select col1 from normal.t1 where col1={} or col2={}",
                    values[0], values[1]
                ),
                "prepare st from 'select col1 from prepared.t1 where col1=? or col2=?'",
                format!("set @v1={},@v2={}", values[0], values[1]),
                "execute st using @v1,@v2",
            ),
            choice => {
                let conjunction = if choice == 3 { "and" } else { "or" };
                stmt(
                    format!(
                        "select col1 from normal.t1 where col1 in ({},{},{}) {conjunction} col2={}",
                        values[0], values[1], values[2], values[3]
                    ),
                    &format!(
                        "prepare st from 'select col1 from prepared.t1 where col1 in (?,?,?) {conjunction} col2=?'"
                    ),
                    format!(
                        "set @v1={},@v2={},@v3={},@v4={}",
                        values[0], values[1], values[2], values[3]
                    ),
                    "execute st using @v1,@v2,@v3,@v4",
                )
            }
        }
    };
    let mut statements = vec![txn("begin")];
    while statements.len() < 2000 {
        if rand::intn(15) == 0 {
            statements.extend([txn("commit"), txn("begin")]);
            continue;
        }
        if statements.len() < 100 {
            statements.push(gen_insert());
            continue;
        }
        statements.push(match rand::intn(100) {
            0..=9 => gen_insert(),
            10..=49 => gen_basic_select(),
            50..=59 => gen_agg_select(),
            60..=69 => gen_join_select(),
            _ => gen_point_select(),
        });
    }
    statements.push(txn("commit"));
    test_with_workers(&harness.store, &statements);
}

fn one_param_stmt(select: &str, predicate: &str, operator: &str, value: i32) -> TestStmt {
    let suffix = if select.contains("sum(") {
        " group by col2"
    } else {
        ""
    };
    stmt(
        format!("{select} normal.t1 {predicate}{operator}{value}{suffix}"),
        &format!("prepare st from '{select} prepared.t1 {predicate}{operator}?{suffix}'"),
        format!("set @v1={value}"),
        "execute st using @v1",
    )
}

fn two_param_stmt(select: &str, predicate: &str, v1: i32, v2: i32) -> TestStmt {
    let suffix = if select.contains("sum(") {
        " group by col2"
    } else {
        ""
    };
    stmt(
        format!("{select} normal.t1 {predicate} {v1} and {v2}{suffix}"),
        &format!("prepare st from '{select} prepared.t1 {predicate} ? and ?{suffix}'"),
        format!("set @v1={v1},@v2={v2}"),
        "execute st using @v1,@v2",
    )
}

/// 复现 Sysbench 的事务、读写比例和普通/预处理双库对照。
fn sysbench_workload() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "create database normal");
    exec(&mut harness.tk, "create database prepared");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for db in ["normal", "prepared"] {
        exec(&mut harness.tk, &format!("use {db}"));
        exec(
            &mut harness.tk,
            "create table sbtest (id int unsigned not null auto_increment,k int unsigned not null default 0,c char(120) not null default '',primary key(id),key k(k))",
        );
    }
    let mut max_id = 1;
    let mut txn_least_id = 1;
    let mut statements = vec![txn("begin")];
    let mut generated_slots = statements.len();
    while generated_slots < 2000 {
        if rand::intn(15) == 0 {
            statements.extend([txn("commit"), txn("begin")]);
            generated_slots += 2;
            txn_least_id = 1;
            continue;
        }
        if generated_slots < 100 {
            statements.push(sysbench_insert(&mut max_id));
            generated_slots += 1;
            continue;
        }
        let generated = match rand::intn(100) {
            0..=49 => Some(sysbench_select(max_id)),
            50..=74 => sysbench_update(max_id, &mut txn_least_id),
            75..=89 => Some(sysbench_insert(&mut max_id)),
            _ => sysbench_delete(max_id, &mut txn_least_id),
        };
        if let Some(statement) = generated {
            statements.push(statement);
        }
        // Go 把 nil 更新/删除也追加进原始切片，分发时再跳过；因此它仍占一个生成槽位。
        generated_slots += 1;
    }
    statements.push(txn("commit"));
    test_with_workers(&harness.store, &statements);
}

fn sysbench_insert(max_id: &mut i32) -> TestStmt {
    let id = *max_id;
    *max_id += 1;
    let (k, c) = (rand::intn(10000), rand::intn(10000));
    stmt(
        format!("insert into normal.sbtest values ({id},{k},'{c}')"),
        "prepare st from 'insert into prepared.sbtest values (?,?,?)'",
        format!("set @id={id},@k={k},@c='{c}'"),
        "execute st using @id,@k,@c",
    )
}

fn sysbench_select(max_id: i32) -> TestStmt {
    let id1 = rand::intn(max_id);
    let id2 = rand::intn(max_id);
    let (low, high) = if id1 <= id2 { (id1, id2) } else { (id2, id1) };
    match rand::intn(5) {
        0 => stmt(
            format!("select c from normal.sbtest where id={id1}"),
            "prepare st from 'select c from prepared.sbtest where id=?'",
            format!("set @id={id1}"),
            "execute st using @id",
        ),
        1 => stmt(
            format!("select c from normal.sbtest where id between {low} and {high}"),
            "prepare st from 'select c from prepared.sbtest where id between ? and ?'",
            format!("set @id1={low},@id2={high}"),
            "execute st using @id1,@id2",
        ),
        2 => stmt(
            format!("select sum(k) from normal.sbtest where id between {low} and {high}"),
            "prepare st from 'select sum(k) from prepared.sbtest where id between ? and ?'",
            format!("set @id1={low},@id2={high}"),
            "execute st using @id1,@id2",
        ),
        3 => stmt(
            format!("select c from normal.sbtest where id between {low} and {high} order by c"),
            "prepare st from 'select c from prepared.sbtest where id between ? and ? order by c'",
            format!("set @id1={low},@id2={high}"),
            "execute st using @id1,@id2",
        ),
        _ => stmt(
            format!(
                "select distinct c from normal.sbtest where id between {low} and {high} order by c"
            ),
            "prepare st from 'select distinct c from prepared.sbtest where id between ? and ? order by c'",
            format!("set @id1={low},@id2={high}"),
            "execute st using @id1,@id2",
        ),
    }
}

fn next_sysbench_mutation_id(max_id: i32, txn_least_id: &mut i32) -> Option<i32> {
    let id = *txn_least_id + rand::intn(max_id - *txn_least_id + 1);
    if id == *txn_least_id {
        return None;
    }
    *txn_least_id = id;
    Some(id)
}

fn sysbench_update(max_id: i32, txn_least_id: &mut i32) -> Option<TestStmt> {
    let id = next_sysbench_mutation_id(max_id, txn_least_id)?;
    Some(if rand::intn(2) == 0 {
        stmt(
            format!("update normal.sbtest set k=k+1 where id={id}"),
            "prepare st from 'update prepared.sbtest set k=k+1 where id=?'",
            format!("set @id={id}"),
            "execute st using @id",
        )
    } else {
        let c = rand::intn(10000);
        stmt(
            format!("update normal.sbtest set c='{c}' where id={id}"),
            "prepare st from 'update prepared.sbtest set c=? where id=?'",
            format!("set @c='{c}',@id={id}"),
            "execute st using @c,@id",
        )
    })
}

fn sysbench_delete(max_id: i32, txn_least_id: &mut i32) -> Option<TestStmt> {
    let id = next_sysbench_mutation_id(max_id, txn_least_id)?;
    Some(stmt(
        format!("delete from normal.sbtest where id={id}"),
        "prepare st from 'delete from prepared.sbtest where id=?'",
        format!("set @id={id}"),
        "execute st using @id",
    ))
}
