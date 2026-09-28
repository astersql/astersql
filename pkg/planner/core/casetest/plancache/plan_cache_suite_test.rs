// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 语句特性相对会话级计划缓存开关的可缓存性用例。
//
// `PlanCacheStmt` 收集到的子查询、参数化 LIMIT 等特征，会对照
// `PlanCacheKeyContext` 中的会话开关（如 `enable_plan_cache_for_subquery`、
// `enable_plan_cache_for_parameterized_limit`）决定是否可缓存；关闭对应开关时
// `NewPlanCacheKey` 应返回 `cacheable=false` 并给出原因字符串。

/// 真实 prepared EXECUTE 的 warning 生命周期：执行产生 warning，下一条成功执行
/// 后 warning 集合按语句边界重置，不会污染后续 plan-cache 断言。
#[test]
fn prepared_execution_warnings_are_statement_scoped() {
    use astersql_testkit::mockstore::CreateMockStoreAndDomain;
    use astersql_testkit::{Rows, TestKit};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "prepare warn_stmt from 'select tidb_decode_key(?)'",
        Vec::new(),
    );
    testkit.MustExec("set @key = 'not-a-key'", Vec::new());
    testkit.MustQuery("execute warn_stmt using @key", Vec::new());
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(vec![vec!["Warning", "1105", "invalid key: not-a-key"]]);

    testkit
        .MustQuery("select 1", Vec::new())
        .Check(Rows(&["1"]));
    testkit
        .MustQuery("show warnings", Vec::new())
        .Check(Rows(&[]));
}

/// 验证：子查询与参数化 LIMIT 开关逐级放开后，语句才变为可缓存。
#[test]
fn statement_features_obey_plan_cache_session_switches() {
    use astersql_planner_core::{
        NewPlanCacheKey, PlanCacheKeyContext, PlanCacheLimit, PlanCacheStmt, ast,
    };

    // 收集：带参数化 count 的 LIMIT，并标记含有子查询特征。
    let mut statement =
        PlanCacheStmt::<()>::new(ast::misc::Prepared::default(), "select * from t limit ?");
    statement.SchemaVersion = 9;
    statement.CollectPlanCacheStmtInfo(
        vec![PlanCacheLimit {
            count: 20,
            count_is_parameter: true,
            ..Default::default()
        }],
        true,
        Vec::new(),
    );

    // 默认上下文：子查询未启用 → 不可缓存。
    let disabled = NewPlanCacheKey(&PlanCacheKeyContext::default(), &statement).unwrap();
    assert!(!disabled.cacheable);
    assert!(disabled.reason.contains("subquery"));

    // 仅开启子查询缓存：仍因参数化 LIMIT 不可缓存。
    let subquery_enabled = PlanCacheKeyContext {
        enable_plan_cache_for_subquery: true,
        ..Default::default()
    };
    let limit_disabled = NewPlanCacheKey(&subquery_enabled, &statement).unwrap();
    assert!(!limit_disabled.cacheable);
    assert!(limit_disabled.reason.contains("param_limit"));

    // 两个开关都打开：语句可缓存。
    let all_enabled = PlanCacheKeyContext {
        enable_plan_cache_for_subquery: true,
        enable_plan_cache_for_parameterized_limit: true,
        ..Default::default()
    };
    assert!(NewPlanCacheKey(&all_enabled, &statement).unwrap().cacheable);
}
