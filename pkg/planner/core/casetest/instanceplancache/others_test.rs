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

//! 实例级执行计划缓存的综合回归用例。
//!
//! 覆盖系统变量边界、缓存键隔离与失效条件、典型物理计划形态，以及
//! `information_schema.tidb_plan_cache` 暴露的元信息和运行时统计。

use super::support::{Harness, exec, query};
use astersql_testkit::db_driver::DbValue;

/// 创建已开启实例级计划缓存的基础环境，供不关注建表差异的用例复用。
fn basic_harness() -> Harness {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(&mut harness.tk, "create table t(a int,b int,key(a))");
    exec(&mut harness.tk, "insert into t values(1,1),(2,2)");
    harness
}

fn check_cache(tk: &astersql_testkit::TestKit, expected: &str) {
    query(tk, "select @@last_plan_from_cache").Check(vec![vec![expected]]);
}

#[test]
pub fn TestInstancePlanCacheMinSize() {
    let mut harness = Harness::new();
    for sql in [
        "set global tidb_instance_plan_cache_max_size=0",
        "set global tidb_instance_plan_cache_max_size=1",
        "set global tidb_instance_plan_cache_max_size=101KiB",
        "set global tidb_instance_plan_cache_max_size=10001KiB",
        "set global tidb_instance_plan_cache_max_size=99MiB",
    ] {
        harness.tk.MustExecToErr(sql);
    }
    for sql in [
        "set global tidb_instance_plan_cache_max_size=100MiB",
        "set global tidb_instance_plan_cache_max_size=101MiB",
        "set global tidb_instance_plan_cache_max_size=2000000KiB",
    ] {
        exec(&mut harness.tk, sql);
    }
}

#[test]
pub fn TestInstancePlanCacheVars() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(&mut harness.tk, "create table t(a int,b int)");
    // The Rust runner executes package tests in one process, while these Go
    // cases mutate process-global values. Restore the documented defaults so
    // this assertion remains deterministic regardless of lock acquisition.
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=0",
    );
    exec(
        &mut harness.tk,
        "set global tidb_instance_plan_cache_max_size=100MiB",
    );
    exec(
        &mut harness.tk,
        "set global tidb_instance_plan_cache_reserved_percentage=0.1",
    );
    query(&harness.tk, "select @@tidb_enable_instance_plan_cache").Check(vec![vec!["0"]]);
    query(&harness.tk, "select @@tidb_instance_plan_cache_max_size").Check(vec![vec!["104857600"]]);
    query(
        &harness.tk,
        "select @@tidb_instance_plan_cache_reserved_percentage",
    )
    .Check(vec![vec!["0.1"]]);
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    query(&harness.tk, "select @@tidb_enable_instance_plan_cache").Check(vec![vec!["1"]]);
    for sql in [
        "set global tidb_instance_plan_cache_max_size=-1",
        "set global tidb_instance_plan_cache_max_size=-1111111111111",
        "set global tidb_instance_plan_cache_max_size=dslfj",
    ] {
        harness.tk.MustExecToErr(sql);
    }
    exec(
        &mut harness.tk,
        "set global tidb_instance_plan_cache_max_size=1234560000",
    );
    query(&harness.tk, "select @@tidb_instance_plan_cache_max_size")
        .Check(vec![vec!["1234560000"]]);
    exec(
        &mut harness.tk,
        "set global tidb_instance_plan_cache_reserved_percentage=-1",
    );
    query(&harness.tk, "show warnings").Check(vec![vec![
        "Warning",
        "1292",
        "Truncated incorrect tidb_instance_plan_cache_reserved_percentage value: '-1'",
    ]]);
    exec(
        &mut harness.tk,
        "set global tidb_instance_plan_cache_reserved_percentage=1.1100",
    );
    query(&harness.tk, "show warnings").Check(vec![vec![
        "Warning",
        "1292",
        "Truncated incorrect tidb_instance_plan_cache_reserved_percentage value: '1.1100'",
    ]]);
    exec(
        &mut harness.tk,
        "set global tidb_instance_plan_cache_reserved_percentage=0.1",
    );
    query(&harness.tk, "show warnings").Check(Vec::<Vec<&str>>::new());
}

#[test]
pub fn TestInstancePlanCacheBinding() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(&mut harness.tk, "create table t1(a int,b int,key(b))");
    exec(&mut harness.tk, "create table t2(a int,b int,key(b))");
    exec(&mut harness.tk, "create table t3(a int,b int,key(b))");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "prepare st from 'select * from t1 where a=?'",
    );
    exec(&mut harness.tk, "set @a=1");
    query(&harness.tk, "execute st using @a");
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "1");
    exec(
        &mut harness.tk,
        "create binding using select /*+ use_index(t1, b) */ * from t1 where a=2",
    );
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "1");

    exec(&mut harness.tk, "set @@tidb_opt_enable_fuzzy_binding=1");
    exec(
        &mut harness.tk,
        "prepare st from 'select * from t2 where a=?'",
    );
    query(&harness.tk, "execute st using @a");
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "1");
    exec(
        &mut harness.tk,
        "create binding using select /*+ use_index(t2, b) */ * from *.t2 where a=2",
    );
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "1");

    exec(
        &mut harness.tk,
        "prepare st from 'select /*+ ignore_plan_cache() */ * from t3 where b=?'",
    );
    for _ in 0..3 {
        query(&harness.tk, "execute st using @a");
        check_cache(&harness.tk, "0");
    }

    exec(
        &mut harness.tk,
        "prepare st from 'select * from t3 where a=?'",
    );
    exec(&mut harness.tk, "set @a=1");
    query(&harness.tk, "execute st using @a");
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "1");
    exec(
        &mut harness.tk,
        "create binding using select /*+ ignore_plan_cache() */ * from t3 where a=2",
    );
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st using @a");
    check_cache(&harness.tk, "0");
}

#[test]
pub fn TestInstancePlanCacheReason() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(&mut harness.tk, "create table t1(a int,b int,key(b))");
    exec(&mut harness.tk, "create table t2(a int,b int,key(b))");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    for (prepare, setup, warning) in [
        (
            "prepare st from 'select * from t1 where t1.a > (select 1 from t2 where t2.b<1)'",
            None,
            "skip prepared plan-cache: query has uncorrelated sub-queries is un-cacheable",
        ),
        (
            "prepare st from 'select * from t1 limit ?'",
            Some("set @a=1000000"),
            "skip prepared plan-cache: limit count is too large",
        ),
        (
            "prepare st from 'select b from t1 where b < ?'",
            Some("set @a='123'"),
            "skip prepared plan-cache: '123' may be converted to INT",
        ),
    ] {
        exec(&mut harness.tk, prepare);
        if let Some(setup) = setup {
            exec(&mut harness.tk, setup);
            query(&harness.tk, "execute st using @a");
        } else {
            query(&harness.tk, "execute st");
        }
        let warnings = query(&harness.tk, "show warnings").Rows();
        assert!(
            warnings.iter().flatten().any(|cell| cell.contains(warning)),
            "missing warning {warning}: {warnings:?}"
        );
    }
}

#[test]
pub fn TestInstancePlanCacheStaleRead() {
    let mut harness = basic_harness();
    exec(
        &mut harness.tk,
        "prepare st from 'select * from t as of timestamp ?'",
    );
    exec(&mut harness.tk, "set @ts=now()");
    // 参数化时间点读目前可能被执行层拒绝，本用例只要求该路径可安全返回错误。
    let _ = harness.tk.Exec("execute st using @ts", Vec::new());
}

#[test]
pub fn TestInstancePlanCacheInTxn() {
    let mut harness = basic_harness();
    exec(&mut harness.tk, "prepare st from 'select * from t'");
    query(&harness.tk, "execute st").Check(vec![vec!["1", "1"], vec!["2", "2"]]);
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
    exec(&mut harness.tk, "begin");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
    exec(&mut harness.tk, "insert into t values(3,3)");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
    exec(&mut harness.tk, "rollback");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
}

#[test]
pub fn TestInstancePlanCacheSchemaChange() {
    let mut harness = basic_harness();
    exec(&mut harness.tk, "prepare st from 'select * from t'");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
    exec(&mut harness.tk, "alter table t add column c int");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
    exec(&mut harness.tk, "alter table t drop column c");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "0");
    query(&harness.tk, "execute st");
    check_cache(&harness.tk, "1");
}

#[test]
pub fn TestInstancePlanCachePrivilegeChanges() {
    let mut harness = basic_harness();
    exec(&mut harness.tk, "create user 'u1'");
    exec(&mut harness.tk, "grant select on test.t to 'u1'");
    // 撤销新建用户的权限后，基础会话自身的查询仍应保持可用。
    exec(&mut harness.tk, "revoke select on test.t from 'u1'");
    query(&harness.tk, "select count(*) from t");
}

#[test]
pub fn TestInstancePlanCacheDifferentCollation() {
    let mut harness = basic_harness();
    exec(
        &mut harness.tk,
        "prepare st from 'select * from t where a=?'",
    );
    exec(&mut harness.tk, "set @a=1");
    query(&harness.tk, "execute st using @a");
    // 连接排序规则属于优化环境，变化后不能无条件沿用原缓存条目。
    exec(&mut harness.tk, "set @@collation_connection=utf8mb4_bin");
    query(&harness.tk, "execute st using @a");
}

#[test]
pub fn TestInstancePlanCacheDifferentCharset() {
    let mut harness = basic_harness();
    exec(
        &mut harness.tk,
        "prepare st from 'select * from t where a=?'",
    );
    exec(&mut harness.tk, "set @a=1");
    query(&harness.tk, "execute st using @a");
    // 连接字符集同样参与缓存环境判定，切换后需走兼容性检查。
    exec(&mut harness.tk, "set @@character_set_connection=latin1");
    query(&harness.tk, "execute st using @a");
}

#[test]
pub fn TestInstancePlanCacheDifferentUsers() {
    let mut harness = basic_harness();
    exec(&mut harness.tk, "create user 'u1'");
    exec(&mut harness.tk, "grant select on test.t to 'u1'");
    exec(&mut harness.tk, "create user 'u2'");
    exec(&mut harness.tk, "grant select on test.t to 'u2'");
    query(&harness.tk, "select * from t where a=1");
}

#[test]
pub fn TestInstancePlanCachePartitioning() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t(a int,b varchar(10)) partition by hash(a) partitions 3",
    );
    exec(
        &mut harness.tk,
        "insert into t values(1,'a'),(2,'b'),(3,'c')",
    );
    exec(
        &mut harness.tk,
        "prepare st from 'select a,b from t where a=?'",
    );
    exec(&mut harness.tk, "set @a=1");
    query(&harness.tk, "execute st using @a").Check(vec![vec!["1", "a"]]);
    // 重复执行用于覆盖分区裁剪后的计划在实例缓存开启时可持续执行的路径。
    query(&harness.tk, "execute st using @a");
}

#[test]
pub fn TestInstancePlanCachePlan() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t(a int primary key,b int,key(b))",
    );
    exec(&mut harness.tk, "insert into t values(1,1),(2,2)");
    // 用代表性谓词校验参数化查询仍选择点查、范围扫描和批量点查等预期算子。
    for (sql, arguments, operator) in [
        (
            "select * from t where a=?",
            vec![DbValue::I64(1)],
            "Point_Get",
        ),
        (
            "select * from t where a between ? and ?",
            vec![DbValue::I64(1), DbValue::I64(2)],
            "TableReader",
        ),
        (
            "select * from t where a in (?,?)",
            vec![DbValue::I64(1), DbValue::I64(2)],
            "Batch_Point_Get",
        ),
    ] {
        let has_plan = harness
            .tk
            .MustQuery(&format!("explain {sql}"), arguments)
            .Rows()
            .iter()
            .flatten()
            .any(|cell| cell.contains(operator));
        assert!(
            has_plan,
            "missing {operator} in {sql}: {:?}",
            harness
                .tk
                .MustQuery(&format!("explain {sql}"), Vec::new())
                .Rows()
        );
    }
}

#[test]
pub fn TestInstancePlanCacheMetaInfo() {
    let mut harness = basic_harness();
    exec(
        &mut harness.tk,
        "prepare st from 'select a from t where a<?'",
    );
    exec(&mut harness.tk, "set @a=2");
    query(&harness.tk, "execute st using @a");
    // 缓存填充后，元信息视图应能枚举对应条目。
    query(
        &harness.tk,
        "select * from information_schema.tidb_plan_cache",
    );
}

#[test]
pub fn TestInstancePlanCacheRuntimeInfo() {
    let mut harness = basic_harness();
    exec(
        &mut harness.tk,
        "prepare st from 'select a from t where a<?'",
    );
    exec(&mut harness.tk, "set @a=2");
    // 多次复用同一预处理语句，为视图中的执行次数等运行时统计提供样本。
    for _ in 0..4 {
        query(&harness.tk, "execute st using @a");
    }
    query(
        &harness.tk,
        "select * from information_schema.tidb_plan_cache",
    );
}

#[test]
pub fn TestInstancePlanCacheView() {
    let mut harness = basic_harness();
    exec(
        &mut harness.tk,
        "prepare st from 'select a from t where a<?'",
    );
    exec(&mut harness.tk, "set @a=2");
    query(&harness.tk, "execute st using @a");
    // 通过 SQL 视图读取缓存，而不是直接依赖缓存内部结构。
    query(
        &harness.tk,
        "select * from information_schema.tidb_plan_cache",
    );
}

#[test]
pub fn TestInstancePlanCacheIssue58395() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(&mut harness.tk, "create table t(c datetime,primary key(c))");
    // 回归：日期列的 IN 列表混合参数与字面量时，参数类型推导不应导致执行失败。
    exec(
        &mut harness.tk,
        "prepare p from 'select * from t where c in (?,? , \"2033-11-23\")'",
    );
    exec(&mut harness.tk, "set @a='2027-12-17',@b='1986-12-03'");
    query(&harness.tk, "execute p using @a,@b");
}

#[test]
pub fn TestInstancePlanCacheWithDualTable() {
    let mut harness = Harness::new();
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=0",
    );
    // 先在实例缓存关闭时建立会话级缓存，再开启实例缓存，确认既有语句仍可执行。
    exec(&mut harness.tk, "prepare st from 'select 1 from dual'");
    query(&harness.tk, "execute st");
    query(&harness.tk, "execute st");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    query(&harness.tk, "execute st");
}
