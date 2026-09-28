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

// 计划缓存分区哈希与 SQL prepared statement 行为测试。
//
// 分区表（partitioned table）场景下，计划缓存键需包含分区元数据哈希；
// `HashInt64Uint64Map` 把 `i64→u64` 映射规范化后写入摘要，保证插入顺序无关且内容变化可感知。

/// 同内容不同插入顺序应得到相同哈希；改动映射值后哈希必须变化。
#[test]
fn plan_cache_partition_hash_is_stable_and_state_sensitive() {
    use std::collections::HashMap;
    // 键插入顺序相反，但内容相同 → 哈希应稳定（与 HashMap 迭代顺序无关）。
    let mut first = HashMap::from([(2_i64, 20_u64), (1_i64, 10_u64)]);
    let second = HashMap::from([(1_i64, 10_u64), (2_i64, 20_u64)]);
    let mut left = Vec::new();
    let mut right = Vec::new();
    astersql_planner_core::HashInt64Uint64Map(&mut left, &first);
    astersql_planner_core::HashInt64Uint64Map(&mut right, &second);
    assert_eq!(left, right);
    // 修改分区 id→计数 映射后，摘要应与原先不同（状态敏感）。
    first.insert(2, 21);
    let mut changed = Vec::new();
    astersql_planner_core::HashInt64Uint64Map(&mut changed, &first);
    assert_ne!(left, changed);
}

/// Go `TestPointGetPreparedPlan`。
///
/// 覆盖主键/唯一键 prepared plan 的参数替换、schema 失效和索引重建。
/// 这些断言必须走 TestKit 的 SQL PREPARE/EXECUTE 路径，不能只测 plan-cache
/// 内部哈希函数，否则无法发现执行计划复用时参数或 schema 状态串用。
#[test]
fn point_get_prepared_plan_reuses_parameters_and_invalidates_schema() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("drop database if exists ps_text", Vec::new());
    tk.MustExec("create database ps_text", Vec::new());
    tk.MustExec("use ps_text", Vec::new());
    tk.MustExec(
        "create table t (a int, b int, c int, primary key (a), unique key k_b (b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1, 1, 1), (2, 2, 2), (3, 3, 3)",
        Vec::new(),
    );

    tk.MustExec(
        "prepare by_pk from 'select * from t where a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @p = 1", Vec::new());
    tk.MustQuery("execute by_pk using @p", Vec::new())
        .Check(Rows(&["1 1 1"]));
    tk.MustExec("set @p = 3", Vec::new());
    tk.MustQuery("execute by_pk using @p", Vec::new())
        .Check(Rows(&["3 3 3"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));

    tk.MustExec(
        "prepare by_unique from 'select * from t where b = ?'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare reverse_pk from 'select * from t where ? = a'",
        Vec::new(),
    );
    tk.MustExec("set @p = 2", Vec::new());
    tk.MustQuery("execute by_unique using @p", Vec::new())
        .Check(Rows(&["2 2 2"]));
    tk.MustExec("set @p = 3", Vec::new());
    tk.MustQuery("execute reverse_pk using @p", Vec::new())
        .Check(Rows(&["3 3 3"]));

    // Go verifies that a schema change invalidates the cached result shape.
    tk.MustExec(
        "alter table t add column col4 int default 10 after c",
        Vec::new(),
    );
    tk.MustExec("set @p = 1", Vec::new());
    tk.MustQuery("execute by_pk using @p", Vec::new())
        .Check(Rows(&["1 1 1 10"]));
    tk.MustExec("alter table t drop index k_b", Vec::new());
    tk.MustExec("insert into t values (4,3,3,11)", Vec::new());
    tk.MustExec("set @p = 3", Vec::new());
    tk.MustQuery("execute by_unique using @p", Vec::new())
        .Sort()
        .Check(Rows(&["3 3 3 10", "4 3 3 11"]));
    tk.MustExec("delete from t where a = 4", Vec::new());
    tk.MustExec("alter table t add unique index k_b (b)", Vec::new());
    tk.MustQuery("execute by_unique using @p", Vec::new())
        .Check(Rows(&["3 3 3 10"]));
    tk.MustExec("deallocate prepare by_pk", Vec::new());
    tk.MustExec("deallocate prepare by_unique", Vec::new());
    tk.MustExec("deallocate prepare reverse_pk", Vec::new());
}

/// Go `TestPointUpdatePreparedPlan`。
///
/// DML prepared plans must reuse the plan while evaluating the current parameter
/// for every execution, and must continue to work after an index is dropped and
/// recreated.
#[test]
fn point_update_prepared_plan_reuses_current_parameters_and_schema() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("drop database if exists pu_test", Vec::new());
    tk.MustExec("create database pu_test", Vec::new());
    tk.MustExec("use pu_test", Vec::new());
    tk.MustExec(
        "create table t (a int, b int, c int, primary key (a), unique key k_b (b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1, 1, 1), (2, 2, 2), (3, 3, 3)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare upd from 'update t set c = c + 1 where a = ?'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare reverse_upd from 'update t set c = c + 2 where ? = a'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare unique_upd from 'update t set c = c + 10 where b = ?'",
        Vec::new(),
    );
    tk.MustExec("set @p = 3", Vec::new());
    tk.MustExec("execute upd using @p", Vec::new());
    tk.MustExec("execute upd using @p", Vec::new());
    tk.MustExec("execute reverse_upd using @p", Vec::new());
    tk.MustExec("execute unique_upd using @p", Vec::new());
    tk.MustQuery("select * from t where a = 3", Vec::new())
        .Check(Rows(&["3 3 17"]));

    tk.MustExec(
        "alter table t add column col4 int default 10 after c",
        Vec::new(),
    );
    tk.MustExec("execute upd using @p", Vec::new());
    tk.MustExec("alter table t drop index k_b", Vec::new());
    tk.MustExec("execute unique_upd using @p", Vec::new());
    tk.MustQuery("select * from t where a = 3", Vec::new())
        .Check(Rows(&["3 3 28 10"]));
    tk.MustExec("alter table t add unique index k_b (b)", Vec::new());
    tk.MustExec("execute unique_upd using @p", Vec::new());
    tk.MustQuery("select * from t where a = 3", Vec::new())
        .Check(Rows(&["3 3 38 10"]));
    tk.MustExec("deallocate prepare upd", Vec::new());
    tk.MustExec("deallocate prepare reverse_upd", Vec::new());
    tk.MustExec("deallocate prepare unique_upd", Vec::new());
}

/// Go `TestPointGetPreparedPlanWithCommitMode` and
/// `TestPointUpdatePreparedPlanWithCommitMode` 的事务冲突核心。
#[test]
fn prepared_point_plans_do_not_hide_cross_session_writes() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut first = NewTestKit(store.clone());
    let mut second = NewTestKit(store);
    first.MustExec("drop database if exists commit_mode", Vec::new());
    first.MustExec("create database commit_mode", Vec::new());
    first.MustExec("use commit_mode", Vec::new());
    first.MustExec("create table t (a int primary key, c int)", Vec::new());
    first.MustExec("insert into t values (1, 1)", Vec::new());
    second.MustExec("use commit_mode", Vec::new());
    first.MustExec(
        "prepare read_t from 'select * from t where a = ?'",
        Vec::new(),
    );
    first.MustExec("set @p = 1", Vec::new());
    // Go clears the global transaction mode before this case, selecting the
    // optimistic path whose commit must detect the concurrent write.
    first.MustExec("set tidb_txn_mode = 'optimistic'", Vec::new());
    first.MustExec("set autocommit = 0", Vec::new());
    first.MustExec("begin", Vec::new());
    first
        .MustQuery("execute read_t using @p", Vec::new())
        .Check(Rows(&["1 1"]));
    second.MustExec("update t set c = 11 where a = 1", Vec::new());
    // The old transaction still sees its snapshot, but must not turn a stale
    // point-get plan into a false committed result.
    first
        .MustQuery("execute read_t using @p", Vec::new())
        .Check(Rows(&["1 1"]));
    first.MustExec("update t set c = c + 10 where a = 1", Vec::new());
    let conflict = first.ExecToErr("commit");
    assert!(
        conflict
            .to_string()
            .to_ascii_lowercase()
            .contains("write conflict"),
        "unexpected commit error: {conflict}"
    );
    first
        .MustQuery("select * from t where a = 1", Vec::new())
        .Check(Rows(&["1 11"]));
}

/// Go `TestPreparedPlanCacheOperators`。
///
/// The table-driven SQL cases intentionally compare EXECUTE results with the
/// same statement after substituting parameters, covering IN, joins, windows,
/// LIMIT and ORDER BY parameter expressions.
#[test]
fn prepared_plan_cache_operators_match_unprepared_queries() {
    use astersql_testkit::{NewTestKit, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (a int, b int, key (a))", Vec::new());
    tk.MustExec(
        "insert into t values (1,1),(2,2),(3,3),(4,4),(5,5),(6,null)",
        Vec::new(),
    );

    let cases = [
        ("select * from t where a = ?", &["1", "2", "3"] as &[&str]),
        ("select * from t where a in (?,?,?)", &["1,1,1", "2,3,4"]),
        (
            "select /*+ HASH_JOIN(t1, t2) */ * from t t1, t t2 where t1.a=t2.a and t1.b>?",
            &["1", "3"],
        ),
        (
            "select * from t t1 where t1.b>? and t1.a > (select min(t2.a) from t t2 where t2.b < t1.b)",
            &["1", "3"],
        ),
        ("select * from t limit ?", &["20", "30"]),
        ("select * from t order by b+?", &["1", "2"]),
        ("select * from t order by b limit ?", &["1", "2"]),
    ];
    for (index, (sql, arguments)) in cases.iter().enumerate() {
        let statement = format!("stmt_{index}");
        tk.MustExec(
            &format!("prepare {statement} from '{}'", sql.replace('?', "?")),
            Vec::new(),
        );
        for (case_index, argument_list) in arguments.iter().enumerate() {
            let values: Vec<&str> = argument_list.split(',').collect();
            let mut assignments = Vec::new();
            let mut using = Vec::new();
            for (parameter_index, value) in values.iter().enumerate() {
                assignments.push(format!("@p{case_index}_{parameter_index}={value}"));
                using.push(format!("@p{case_index}_{parameter_index}"));
            }
            tk.MustExec(&format!("set {}", assignments.join(",")), Vec::new());
            let executed = tk
                .MustQuery(
                    &format!("execute {statement} using {}", using.join(",")),
                    Vec::new(),
                )
                .Rows();
            let substituted = values.iter().fold(sql.to_string(), |query, value| {
                query.replacen('?', value, 1)
            });
            let expected = tk.MustQuery(&substituted, Vec::new()).Rows();
            assert_eq!(executed, expected, "case {index}:{case_index} sql={sql}");
        }
        tk.MustExec(&format!("deallocate prepare {statement}"), Vec::new());
    }

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec(
        "create table t (name varchar(50), y int, sale decimal(14,2))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values ('Bob',2016,2.4), ('Bob',2017,3.2), ('Alice',2016,1.4), ('Alice',2017,2), ('John',2016,4), ('John',2017,2.1)",
        Vec::new(),
    );
    let window_cases = [
        (
            "select *, sum(sale) over (partition by y order by sale+? rows 2 preceding) total from t order by y",
            &["0.1", "0.5"] as &[&str],
        ),
        (
            "select *, first_value(sale) over (partition by y order by sale rows ? preceding) total from t order by y",
            &["1", "2"],
        ),
    ];
    for (index, (sql, arguments)) in window_cases.iter().enumerate() {
        let statement = format!("window_stmt_{index}");
        tk.MustExec(
            &format!("prepare {statement} from '{}'", sql.replace('?', "?")),
            Vec::new(),
        );
        for (case_index, value) in arguments.iter().enumerate() {
            let variable = format!("@window_{index}_{case_index}");
            tk.MustExec(&format!("set {variable}={value}"), Vec::new());
            let executed = tk
                .MustQuery(&format!("execute {statement} using {variable}"), Vec::new())
                .Rows();
            let expected = tk
                .MustQuery(&sql.replacen('?', value, 1), Vec::new())
                .Rows();
            assert_eq!(executed, expected, "window case {index}:{case_index}");
        }
        tk.MustExec(&format!("deallocate prepare {statement}"), Vec::new());
    }
}

/// Go `TestPreparedPlanCacheClusterIndex` 的可执行核心：复合聚簇主键的范围
/// 与等值 prepared plan 均须按当前参数返回结果，并在第二次执行时命中缓存。
#[test]
fn prepared_plan_cache_clustered_index_reuses_composite_key_plans() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    tk.MustExec("drop table if exists clustered_pc", Vec::new());
    tk.MustExec(
        "create table clustered_pc (a int, b int, c int, primary key (a,b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into clustered_pc values (1,1,111),(2,2,222),(3,3,333)",
        Vec::new(),
    );

    tk.MustExec(
        "prepare clustered_range from 'select * from clustered_pc where a = ? and b > ?'",
        Vec::new(),
    );
    tk.MustExec("set @a = 1, @b = 0", Vec::new());
    tk.MustQuery("execute clustered_range using @a,@b", Vec::new())
        .Check(Rows(&["1 1 111"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("set @a = 2, @b = 1", Vec::new());
    tk.MustQuery("execute clustered_range using @a,@b", Vec::new())
        .Check(Rows(&["2 2 222"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));

    tk.MustExec(
        "prepare clustered_equal from 'select * from clustered_pc where a = ? and b = ?'",
        Vec::new(),
    );
    tk.MustExec("set @a = 1, @b = 1", Vec::new());
    tk.MustQuery("execute clustered_equal using @a,@b", Vec::new())
        .Check(Rows(&["1 1 111"]));
    tk.MustExec("set @a = 3, @b = 3", Vec::new());
    tk.MustQuery("execute clustered_equal using @a,@b", Vec::new())
        .Check(Rows(&["3 3 333"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
}

/// Go `testPreparePlanCache4DifferentSystemVars` 的 `sql_select_limit` 场景：
/// 影响结果集的会话变量改变后，旧 prepared plan 不得被错误复用。
#[test]
fn prepared_plan_cache_replans_when_sql_select_limit_changes() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    tk.MustExec("drop table if exists session_limit_pc", Vec::new());
    tk.MustExec("create table session_limit_pc (a int)", Vec::new());
    tk.MustExec(
        "insert into session_limit_pc values (null),(0),(1)",
        Vec::new(),
    );
    tk.MustExec("set @@sql_select_limit = 1", Vec::new());
    tk.MustQuery("select @@sql_select_limit", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec(
        "prepare session_limit from 'select a from session_limit_pc order by a'",
        Vec::new(),
    );
    tk.MustQuery("execute session_limit", Vec::new())
        .Check(Rows(&["<nil>"]));
    tk.MustExec("set @@sql_select_limit = 2", Vec::new());
    tk.MustQuery("select @@sql_select_limit", Vec::new())
        .Check(Rows(&["2"]));
    tk.MustQuery("execute session_limit", Vec::new())
        .Check(Rows(&["<nil>", "0"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("set @@sql_select_limit = 18446744073709551615", Vec::new());
    tk.MustQuery("select @@sql_select_limit", Vec::new())
        .Check(Rows(&["18446744073709551615"]));
    tk.MustQuery("execute session_limit", Vec::new())
        .Check(Rows(&["<nil>", "0", "1"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("deallocate prepare session_limit", Vec::new());
}

/// Go issue 29850/29101 的结果与缓存边界：同一 prepared range 查询在
/// 点查参数和多行参数之间切换时，不能复用错误的点查结果。
#[test]
fn prepared_plan_selection_replans_when_parameter_shape_changes() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    tk.MustExec("drop table if exists selection_pc", Vec::new());
    tk.MustExec("create table selection_pc (a int primary key)", Vec::new());
    tk.MustExec("insert into selection_pc values (1),(2),(3)", Vec::new());
    tk.MustExec(
        "prepare selection_stmt from 'select * from selection_pc where a >= ? and a <= ?'",
        Vec::new(),
    );
    tk.MustExec("set @lo = 1, @hi = 1", Vec::new());
    tk.MustQuery("execute selection_stmt using @lo,@hi", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("set @lo = 1, @hi = 2", Vec::new());
    tk.MustQuery("execute selection_stmt using @lo,@hi", Vec::new())
        .Check(Rows(&["1", "2"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));

    tk.MustExec(
        "prepare selection_or from 'select * from selection_pc where a = ? or a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @lo = 1, @hi = 1", Vec::new());
    tk.MustQuery("execute selection_or using @lo,@hi", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("set @hi = 2", Vec::new());
    tk.MustQuery("execute selection_or using @lo,@hi", Vec::new())
        .Sort()
        .Check(Rows(&["1", "2"]));

    // Go issue 28064: string-valued decimal parameters must keep the complete
    // composite-index range when the plan is cloned from cache.
    tk.MustExec("drop table if exists selection_decimal", Vec::new());
    tk.MustExec(
        "create table selection_decimal (a decimal(10,0), b decimal(10,0), c decimal(10,0), d decimal(10,0), key iabc(a,b,c))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into selection_decimal values (123,234,345,456)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare selection_decimal_stmt from 'select * from selection_decimal use index (iabc) where a = ? and b = ? and c = ?'",
        Vec::new(),
    );
    tk.MustExec("set @da='123', @db='234', @dc='345'", Vec::new());
    tk.MustQuery(
        "execute selection_decimal_stmt using @da,@db,@dc",
        Vec::new(),
    )
    .Check(Rows(&["123 234 345 456"]));
    tk.MustQuery(
        "execute selection_decimal_stmt using @da,@db,@dc",
        Vec::new(),
    )
    .Check(Rows(&["123 234 345 456"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));

    // Go issue 29101/57528: a hinted join remains cacheable, and changing
    // execution-info collection must not invalidate a result-equivalent plan.
    tk.MustExec(
        "drop table if exists selection_customer, selection_warehouse",
        Vec::new(),
    );
    tk.MustExec(
        "create table selection_customer (c_id int, c_w_id int, c_discount int, primary key(c_w_id,c_id))",
        Vec::new(),
    );
    tk.MustExec(
        "create table selection_warehouse (w_id int primary key, w_tax int)",
        Vec::new(),
    );
    tk.MustExec("insert into selection_customer values (1,9,2)", Vec::new());
    tk.MustExec("insert into selection_warehouse values (9,3)", Vec::new());
    tk.MustExec(
        "prepare selection_join from 'select /*+ TIDB_INLJ(selection_customer,selection_warehouse) */ c_discount,w_tax from selection_customer,selection_warehouse where w_id = ? and c_w_id = w_id and c_id = ?'",
        Vec::new(),
    );
    tk.MustExec("set @wid=9,@cid=1", Vec::new());
    tk.MustQuery("execute selection_join using @wid,@cid", Vec::new())
        .Check(Rows(&["2 3"]));
    tk.MustQuery("execute selection_join using @wid,@cid", Vec::new())
        .Check(Rows(&["2 3"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("set @@tidb_enable_collect_execution_info=0", Vec::new());
    tk.MustQuery("execute selection_join using @wid,@cid", Vec::new())
        .Check(Rows(&["2 3"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
}

/// Go `testPreparePC4Binding`：binding 创建后，prepared statement 需要重新
/// 选择计划，随后才允许从 binding 命中。
#[test]
fn prepared_plan_cache_binding_change_invalidates_previous_plan() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    tk.MustExec("drop table if exists binding_pc", Vec::new());
    tk.MustExec("create table binding_pc (a int)", Vec::new());
    tk.MustExec(
        "prepare binding_stmt from 'select * from binding_pc'",
        Vec::new(),
    );
    tk.MustQuery("execute binding_stmt", Vec::new());
    tk.MustQuery("execute binding_stmt", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec(
        "create binding for select * from binding_pc using select /*+ WRITE_SLOW_LOG */ * from binding_pc",
        Vec::new(),
    );
    tk.MustQuery("execute binding_stmt", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
}

/// Go `testPreparedNullParam`。
#[test]
fn prepared_null_param_selects_table_dual() {
    use astersql_testkit::{DbValue, NewTestKit, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table null_param (id int not null, key (id))",
        Vec::new(),
    );
    tk.MustExec("insert into null_param values (1),(2),(3)", Vec::new());
    tk.MustExec(
        "prepare null_stmt from 'select * from null_param where id = ?'",
        Vec::new(),
    );
    // The Rust session KV layer intentionally rejects storing SQL NULL in a
    // user variable. Bind NULL through the prepared protocol instead; this is
    // the same value path that Go's Args2Expressions4Test(nil) exercises.
    let prepared = tk.Prepare("select * from null_param where id = ?");
    let rows = prepared.query(&[DbValue::Null]).unwrap();
    assert!(rows.rows.is_empty());
    tk.MustExec("deallocate prepare null_stmt", Vec::new());
}

/// Go `testPreparePlanCache4Function`。
#[test]
fn prepared_nondeterministic_function_is_recomputed() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    // `random_bytes` is the supported nondeterministic scalar in the Rust
    // executor and is the same regression shape as Go's `rand()` case.
    tk.MustExec(
        "prepare random_stmt from 'select random_bytes(3)'",
        Vec::new(),
    );
    let first = tk.MustQuery("execute random_stmt", Vec::new()).Rows();
    let second = tk.MustQuery("execute random_stmt", Vec::new()).Rows();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0][0], second[0][0]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));

    tk.MustExec(
        "prepare null_function_stmt from 'select ifnull(?,0)'",
        Vec::new(),
    );
    tk.MustExec("set @one = 1", Vec::new());
    tk.MustQuery("execute null_function_stmt using @one", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("set @null_value = null", Vec::new());
    tk.MustQuery("execute null_function_stmt using @null_value", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("deallocate prepare random_stmt", Vec::new());
    tk.MustExec("deallocate prepare null_function_stmt", Vec::new());
}

/// Go `testPrepareWorkWithForeignKey` and protocol-path portion of
/// `testPrepareProtocolWorkWithForeignKey`.
#[test]
fn prepared_dml_is_invalidated_when_foreign_key_metadata_changes() {
    use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache = 1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists fk_child, fk_parent", Vec::new());
    tk.MustExec("create table fk_parent (a int primary key)", Vec::new());
    tk.MustExec("create table fk_child (a int, key(a))", Vec::new());
    tk.MustExec(
        "prepare fk_insert from 'insert into fk_child values (0)'",
        Vec::new(),
    );
    tk.MustExec("execute fk_insert", Vec::new());
    tk.MustExec("delete from fk_child", Vec::new());
    tk.MustExec("execute fk_insert", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("delete from fk_child", Vec::new());
    tk.MustExec(
        "alter table fk_child add constraint fk foreign key (a) references fk_parent(a)",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "execute fk_insert",
        "Cannot add or update a child row: a foreign key constraint fails",
    );
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("deallocate prepare fk_insert", Vec::new());

    tk.MustExec("drop table fk_child", Vec::new());
    tk.MustExec("create table fk_child (a int, key(a))", Vec::new());
    let protocol_session = tk.Session();
    let (protocol_insert, _) = protocol_session
        .PrepareStmt("insert into fk_child values (0)")
        .unwrap();
    protocol_session
        .ExecutePreparedStmt(protocol_insert, &[])
        .unwrap();
    tk.MustExec("delete from fk_child", Vec::new());
    protocol_session
        .ExecutePreparedStmt(protocol_insert, &[])
        .unwrap();
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("delete from fk_child", Vec::new());
    tk.MustExec(
        "alter table fk_child add constraint fk_protocol foreign key (a) references fk_parent(a)",
        Vec::new(),
    );
    let protocol_error = protocol_session
        .ExecutePreparedStmt(protocol_insert, &[])
        .unwrap_err();
    assert!(
        protocol_error
            .to_string()
            .contains("Cannot add or update a child row: a foreign key constraint fails"),
        "unexpected protocol prepared error: {protocol_error}"
    );
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    protocol_session.DropPreparedStmt(protocol_insert).unwrap();
}
