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

// 实例级执行计划缓存的内建函数回归测试。
//
// 测试在共享存储上并发创建多个独立会话，反复预编译并执行带参数的表达式，
// 以验证不同列类型、索引访问路径和参数值复用同一实例缓存时不会串用执行结果。

use std::sync::Arc;
use std::thread;

use astersql_testkit::TestKit;
use astersql_testkit::db_driver::DbValue;

use super::support::{Harness, exec, query, rand, rows};

/// 基于同一存储启动十个独立会话，使实例级缓存承受真实的跨会话并发复用。
fn run_concurrently<F>(store: &Arc<astersql_testkit::mockstore::AnalyzeStatsStore>, worker: F)
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

/// 验证 `IN (?, ?)` 在指定列类型下可跨表访问路径和会话安全复用缓存计划。
///
/// 三个回调分别负责构造入库字面量、绑定参数语句和一对必定命中的随机值，
/// 从而让各类型共用同一套并发与结果校验流程。
fn run_in_case(
    column_type: &'static str,
    literal: fn(i32) -> String,
    parameter: fn(&str, &str) -> String,
    value: fn() -> (String, String),
) {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for (table, key) in [
        ("t1", ""),
        ("t2", ", key(a)"),
        ("t3", ", primary key(a)"),
        ("t4", ", unique key(a)"),
    ] {
        exec(
            &mut harness.tk,
            &format!("create table {table} (a {column_type}{key})"),
        );
        for i in 0..100 {
            exec(
                &mut harness.tk,
                &format!("insert into {table} values ({})", literal(i)),
            );
        }
    }
    run_concurrently(&harness.store, move |mut tk| {
        exec(&mut tk, "use test");
        for _ in 0..100 {
            let (v1, v2) = value();
            let table = format!("t{}", rand::intn(4) + 1);
            exec(
                &mut tk,
                &format!("prepare st from 'select a from {table} where a in (?, ?)'"),
            );
            exec(&mut tk, &parameter(&v1, &v2));
            // 查询未声明顺序；先规范化期望值，再与排序后的结果比较。
            let mut expected = vec![v1, v2];
            expected.sort();
            query(&tk, "execute st using @p1, @p2")
                .Sort()
                .Check(rows(&expected));
        }
    });
}

#[test]
fn test_builtin_in_int_sig() {
    run_in_case(
        "int",
        |i| i.to_string(),
        |a, b| format!("set @p1={a}, @p2={b}"),
        || {
            (
                rand::intn(50).to_string(),
                (50 + rand::intn(50)).to_string(),
            )
        },
    );
}

#[test]
fn test_builtin_in_string_sig() {
    run_in_case(
        "varchar(20)",
        |i| format!("'{i}'"),
        |a, b| format!("set @p1='{a}', @p2='{b}'"),
        || {
            (
                rand::intn(50).to_string(),
                (50 + rand::intn(50)).to_string(),
            )
        },
    );
}

#[test]
fn test_builtin_in_real_sig() {
    run_in_case(
        "real",
        |i| format!("'{i}.1'"),
        |a, b| format!("set @p1='{a}', @p2='{b}'"),
        || {
            (
                format!("{}.1", rand::intn(50)),
                format!("{}.1", 50 + rand::intn(50)),
            )
        },
    );
}

#[test]
fn test_builtin_in_decimal_sig() {
    run_in_case(
        "decimal(10, 2)",
        |i| format!("'{i}.10'"),
        |a, b| format!("set @p1='{a}', @p2='{b}'"),
        || {
            (
                format!("{}.10", rand::intn(50)),
                format!("{}.10", 50 + rand::intn(50)),
            )
        },
    );
}

#[test]
fn test_builtin_in_time_sig() {
    // 时间值需要固定合法的分钟区间，因此单独构造数据，但保留与通用 IN 用例相同的
    // 无索引、普通索引、主键和唯一索引四种访问路径。
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for (table, key) in [
        ("t1", ""),
        ("t2", ", key(a)"),
        ("t3", ", primary key(a)"),
        ("t4", ", unique key(a)"),
    ] {
        exec(
            &mut harness.tk,
            &format!("create table {table} (a datetime{key})"),
        );
        for i in 10..50 {
            exec(
                &mut harness.tk,
                &format!("insert into {table} values ('2000-01-01 00:{i:02}:00')"),
            );
        }
    }
    run_concurrently(&harness.store, |mut tk| {
        exec(&mut tk, "use test");
        for _ in 0..100 {
            let a = format!("2000-01-01 00:{:02}:00", 10 + rand::intn(20));
            let b = format!("2000-01-01 00:{:02}:00", 30 + rand::intn(20));
            let table = format!("t{}", rand::intn(4) + 1);
            exec(
                &mut tk,
                &format!("prepare st from 'select a from {table} where a in (?, ?)'"),
            );
            exec(&mut tk, &format!("set @p1='{a}', @p2='{b}'"));
            let mut expected = vec![a, b];
            expected.sort();
            query(&tk, "execute st using @p1, @p2")
                .Sort()
                .Check(rows(&expected));
        }
    });
}

/// 验证数值表达式的 `IS TRUE`/`IS FALSE` 计划不会在跨会话缓存后混淆参数语义。
fn run_is_true_false(column_type: &str, values: [&str; 2]) {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        &format!("create table t (a {column_type})"),
    );
    for value in values {
        exec(&mut harness.tk, &format!("insert into t values ({value})"));
    }
    run_concurrently(&harness.store, move |mut tk| {
        exec(&mut tk, "use test");
        for _ in 0..100 {
            let index = rand::intn(2) as usize;
            let predicate = if rand::intn(2) == 0 {
                "is true"
            } else {
                "is false"
            };
            exec(
                &mut tk,
                &format!("prepare st from 'select a from t where (a-?) {predicate}'"),
            );
            exec(&mut tk, &format!("set @p1={}", values[index]));
            // `a - ?` 对被绑定的值为零、对另一个值为非零，所以真假分支的期望行相反。
            let expected = if predicate == "is true" {
                values[1 - index]
            } else {
                values[index]
            };
            query(&tk, "execute st using @p1").Check(rows(&[expected.to_owned()]));
        }
    });
}

#[test]
fn test_builtin_real_is_true_false() {
    run_is_true_false("real", ["1.1", "2.2"]);
}

#[test]
fn test_builtin_decimal_is_true_false() {
    run_is_true_false("decimal(10, 2)", ["1.10", "2.20"]);
}

#[test]
fn test_builtin_int_is_true_false() {
    run_is_true_false("int", ["1", "2"]);
}

/// 每个会话中的预处理语句都必须拒绝参数个数不匹配的调用。
#[test]
fn prepared_builtin_range_query_is_safe_across_sessions() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(&mut harness.tk, "set global tidb_enable_instance_plan_cache=1");
    exec(&mut harness.tk, "create table t (id int, created_at datetime)");
    exec(
        &mut harness.tk,
        "insert into t values (7, '2026-07-23 00:30:00')",
    );
    let store = harness.store;
    let sql = "select id from t where created_at between ? and ?";
    thread::scope(|scope| {
        for worker in 0..8 {
            let store = Arc::clone(&store);
            scope.spawn(move || {
                let mut kit = TestKit::new(store);
                exec(&mut kit, "use test");
                let prepared = kit.Prepare(sql);
                let result = prepared
                    .query(&[
                        DbValue::String(format!("2026-07-23 00:{worker:02}:00")),
                        DbValue::String(format!("2026-07-23 01:{worker:02}:00")),
                    ])
                    .unwrap();
                assert_eq!(result.rows, vec![vec![DbValue::String("7".to_owned())]]);
                assert!(prepared.query(&[DbValue::I64(1)]).is_err());
            });
        }
    });
}
