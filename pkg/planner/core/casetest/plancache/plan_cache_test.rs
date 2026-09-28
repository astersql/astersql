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

// 执行参数写入会话上下文与 PointGet 执行器复用安全性的用例。
//
// PointGet 是按主键/唯一键点查的物理计划；在计划缓存命中后，是否可复用已有
// PointGetExecutor 取决于事务状态、schema 版本等会话条件。
// `SetParameterValuesIntoSCtx` 把 EXECUTE 参数写入语句标记；
// `containUsePlanCacheHintInPreparedSQLOrBinding` 判断 SQL/binding 是否带强制用缓存 hint。

/// 对齐 Go `TestDropPrepare`：释放后复用同一 statement name 时，
/// 新 SQL 不得命中旧 SQL 的缓存计划，且后续可建立自己的缓存。
#[test]
fn drop_prepare_executes_and_releases_real_plan_cache_statement() {
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;
    use astersql_testkit::{Rows, TestKit};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("drop table if exists t", Vec::new());
    testkit.MustExec(
        "create table t(a int not null, b int not null, key(a), key(b))",
        Vec::new(),
    );
    testkit.MustExec("insert into t values (1, 1), (2, 2), (3, 3)", Vec::new());
    testkit.MustExec("set @@tidb_enable_prepared_plan_cache=1", Vec::new());
    testkit.MustExec(
        "prepare stmt from 'select * from t where a = ?'",
        Vec::new(),
    );
    testkit.MustExec("set @a = 1", Vec::new());

    testkit.MustQuery("execute stmt using @a", Vec::new());
    testkit
        .MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    testkit.MustExec("set @a = 2", Vec::new());
    testkit.MustQuery("execute stmt using @a", Vec::new());
    testkit
        .MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));

    testkit.MustQuery("execute stmt using @a", Vec::new());
    let plan_a = testkit.MustQuery(
        &format!("explain for connection {}", testkit.ConnectionID()),
        Vec::new(),
    );

    testkit.MustExec("deallocate prepare stmt", Vec::new());
    testkit.MustExec(
        "prepare stmt from 'select * from t where b = ?'",
        Vec::new(),
    );
    testkit.MustExec("set @b = 1", Vec::new());
    testkit.MustExec("execute stmt using @b", Vec::new());
    testkit
        .MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));

    testkit.MustExec("set @b = 3", Vec::new());
    testkit.MustExec("execute stmt using @b", Vec::new());
    let plan_b = testkit.MustQuery(
        &format!("explain for connection {}", testkit.ConnectionID()),
        Vec::new(),
    );

    assert_ne!(plan_a.Rows(), plan_b.Rows());
    plan_a.CheckContain("index:a(a)");
    plan_b.CheckContain("index:b(b)");

    testkit.MustExec("execute stmt using @b", Vec::new());
    testkit
        .MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
}

/// 验证：参数写入、PointGet 复用判定，以及 USE_PLAN_CACHE hint 探测逻辑。
#[test]
fn execution_parameters_and_point_get_reuse_follow_session_state() {
    use astersql_planner_core::{
        Datum, IsSafeToReusePointGetExecutor, PlanCacheParamMarker, PlanCacheStmt,
        SetParameterValuesIntoSCtx, ast, containUsePlanCacheHintInPreparedSQLOrBinding,
    };

    // 参数个数与 marker 不一致时应报错；一致时写入并标记 in_execute。
    let mut statement = PlanCacheStmt::<()>::new(
        ast::misc::Prepared::default(),
        "select * from t where id = ?",
    );
    statement.Params = vec![PlanCacheParamMarker {
        offset: 27,
        ..Default::default()
    }];
    assert!(SetParameterValuesIntoSCtx(&mut statement, Vec::new()).is_err());
    SetParameterValuesIntoSCtx(&mut statement, vec![Datum::Int(7)]).unwrap();
    assert!(statement.Params[0].in_execute);

    // IsSafeToReusePointGetExecutor(可缓存, 在事务中, schema变更, 旧版本, 新版本)。
    assert!(IsSafeToReusePointGetExecutor(true, false, false, 3, 3));
    assert!(!IsSafeToReusePointGetExecutor(true, true, false, 3, 3));
    assert!(!IsSafeToReusePointGetExecutor(true, false, true, 3, 3));
    assert!(!IsSafeToReusePointGetExecutor(true, false, false, 3, 4));

    // 语句未自带 hint 时，仅当 binding 侧允许才视为含 USE_PLAN_CACHE。
    statement.HasUsePlanCacheHint = false;
    assert!(containUsePlanCacheHintInPreparedSQLOrBinding(
        &statement, true, true
    ));
    assert!(!containUsePlanCacheHintInPreparedSQLOrBinding(
        &statement, true, false
    ));
}
