// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Prepared plan cache 参数类型兼容性测试。
//
// 对应 Go `prepare_test.go` 中 `TestPrepareCacheChangingParamType` 的核心语义：
// 同一 prepared 语句用不同参数类型反复执行时，仅当类型“兼容”才复用旧计划，
// 否则强制重新生成。本文件直接调用生产 API `CheckTypesCompatibility4PC`。

// 本文件由 pkg/planner/core/tests/prepare/prepare_test.go 迁移而来。
// Go 原始测试通过真实 session/mockstore 覆盖 prepared plan cache 的参数类型变化、分区裁剪、
// 事务隔离、schema 变化、快照、权限和缓存表等场景，其中 TestPrepareCacheChangingParamType
// 专门验证：当同一条 prepared 语句用不同参数类型反复执行时，只有参数类型仍然“兼容”
// （例如 varchar/var_string 视为同一类，或者 int 系列 unsigned 标记一致）才允许复用旧计划，
// 否则必须重新生成计划，防止用错误类型的 range/point 计划服务新参数。
// AsterSQL 目前还没有把 session、mockstore、schema 变化和事务隔离接到这个 casetest 上；
// 下方保留自 prepare_test.go 的 Go 源码文本供对照（可编译），随后的
// #[test] 改用生产的 astersql_types::metadata::FieldType +
// astersql_planner_core::CheckTypesCompatibility4PC，直接验证同一份“参数类型兼容性决定
// plan cache 是否可以复用”的语义核心。
use astersql_parser_auth::parser::auth::auth::UserIdentity;
use astersql_planner_core::CheckTypesCompatibility4PC;
use astersql_testkit::TestKit;
use astersql_testkit::db_driver::DbValue;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_types::metadata::FieldType;
use astersql_types::metadata::mysql;

/// 构造仅设置基础类型码的 `FieldType`（无 flag）。
fn field_type(tp: u8) -> FieldType {
    let mut ft = FieldType::default();
    ft.SetType(tp);
    ft
}

/// 为 `FieldType` 添加 unsigned 标志（有符号/无符号切换会影响 plan cache 兼容性）。
fn unsigned(mut ft: FieldType) -> FieldType {
    ft.AddFlag(mysql::UnsignedFlag);
    ft
}

// prepared_plan_cache_reuses_across_compatible_param_type_changes 对应 Go
// TestPrepareCacheChangingParamType 里最核心的正向断言：反复用不同参数类型重新执行同一条
// prepared 语句时，只要新旧参数类型属于同一“可复用”类别（tinyint 变 tinyint、
// varchar 变 var_string 之类的方言别名），plan cache 就必须认为两者兼容，允许复用旧计划，
// 而不需要重新生成。
/// 正向：同族类型（含 varchar/var_string）及空参数列表应判定为可复用。
#[test]
fn prepared_plan_cache_reuses_across_compatible_param_type_changes() {
    let cached = vec![field_type(mysql::TypeTiny)];
    let same_type_again = vec![field_type(mysql::TypeTiny)];
    assert!(CheckTypesCompatibility4PC(&cached, &same_type_again));

    // varchar/var_string 是同一逻辑类型的两种内部标记（Go 侧同样把它们当作兼容对）。
    let cached_varchar = vec![field_type(mysql::TypeVarchar)];
    let executed_var_string = vec![field_type(mysql::TypeVarString)];
    assert!(CheckTypesCompatibility4PC(
        &cached_varchar,
        &executed_var_string
    ));

    // 空参数列表（无参数的 prepared 语句）永远兼容，不应触发重新生成计划。
    assert!(CheckTypesCompatibility4PC(&[], &executed_var_string));
    assert!(CheckTypesCompatibility4PC(&cached_varchar, &[]));
}

// prepared_plan_cache_rebuilds_when_param_type_family_or_signedness_changes 对应 Go
// TestPrepareCacheChangingParamType 里覆盖 tinyint/unsigned/float/decimal/year 多种 dtype
// 时的反向断言：一旦参数的基础类型发生真实变化（比如从 tinyint 变成 decimal），或者同一整数
// 类型的 unsigned 标记发生变化（Go 用 -1 之类越界值触发 unsigned/signed 切换），就必须认为
// 不兼容，强制重新生成计划，否则会用错误符号位的 range 服务新参数。
/// 反向：类型族变化、signedness 变化或参数个数变化必须判定为不兼容并重建计划。
#[test]
fn prepared_plan_cache_rebuilds_when_param_type_family_or_signedness_changes() {
    let cached_tinyint = vec![field_type(mysql::TypeTiny)];
    let executed_decimal = vec![field_type(mysql::TypeNewDecimal)];
    assert!(!CheckTypesCompatibility4PC(
        &cached_tinyint,
        &executed_decimal
    ));

    let cached_signed = vec![field_type(mysql::TypeLong)];
    let executed_unsigned = vec![unsigned(field_type(mysql::TypeLong))];
    assert!(!CheckTypesCompatibility4PC(
        &cached_signed,
        &executed_unsigned
    ));

    // 参数个数变化（例如从单参数变成多参数语句）也必须视为不兼容。
    let cached_single = vec![field_type(mysql::TypeFloat)];
    let executed_pair = vec![field_type(mysql::TypeFloat), field_type(mysql::TypeFloat)];
    assert!(!CheckTypesCompatibility4PC(&cached_single, &executed_pair));
}

/// 对应 Go `TestPointGetPreparedPlan4PlanCache`：恢复真实 mockstore/TestKit
/// 接线后，确认带参数的点查仍保留 Point_Get 计划并返回正确行。
#[test]
fn test_point_get_prepared_plan_cache() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_point_get", Vec::new());
    tk.MustExec(
        "create table prepare_point_get (a int primary key, b int, c int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_point_get values (1, 1, 1), (2, 2, 2), (3, 3, 3)",
        Vec::new(),
    );

    tk.MustExec(
        "prepare point_get_stmt from 'select * from prepare_point_get where a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @point_get_a = 1", Vec::new());
    tk.MustQuery(
        "execute point_get_stmt using @point_get_a",
        Vec::<DbValue>::new(),
    )
    .Check(vec![vec!["1", "1", "1"]]);
    tk.MustQuery(
        "execute point_get_stmt using @point_get_a",
        Vec::<DbValue>::new(),
    )
    .Check(vec![vec!["1", "1", "1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestRandomFlushPlanCache`：会话级刷新只影响当前会话，实例级
/// 刷新使同一 Domain 上的所有 prepared 计划失效。
#[test]
fn test_random_flush_plan_cache() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    let mut tk2 = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_flush_t1", Vec::new());
    tk.MustExec("drop table if exists prepare_flush_t2", Vec::new());
    tk.MustExec(
        "create table prepare_flush_t1 (id int, a int, b int, key(a))",
        Vec::new(),
    );
    tk.MustExec(
        "create table prepare_flush_t2 (id int, a int, b int, key(a))",
        Vec::new(),
    );
    for (index, sql) in [
        "select * from prepare_flush_t1, prepare_flush_t2 where prepare_flush_t1.id = prepare_flush_t2.id",
        "select * from prepare_flush_t1",
        "select * from prepare_flush_t1 where id = 1",
        "select * from prepare_flush_t2",
        "select * from prepare_flush_t2 where id = 1",
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("prepare_flush_stmt{}", index + 1);
        tk.MustExec(
            &format!("prepare {name} from '{sql}'"),
            Vec::new(),
        );
        tk2.MustExec(
            &format!("prepare {name} from '{sql}'"),
            Vec::new(),
        );
    }

    for _ in 0..2 {
        for index in 1..=5 {
            let sql = format!("execute prepare_flush_stmt{index}");
            tk.MustExec(&sql, Vec::new());
            tk.MustExec(&sql, Vec::new());
            tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(vec![vec!["1"]]);
            tk2.MustExec(&sql, Vec::new());
            tk2.MustExec(&sql, Vec::new());
            tk2.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(vec![vec!["1"]]);
        }

        tk.MustExec("admin flush session plan_cache", Vec::new());
        tk.MustExec("execute prepare_flush_stmt1", Vec::new());
        tk.MustQuery("select @@last_plan_from_cache", Vec::new())
            .Check(vec![vec!["0"]]);
        tk.MustExec("execute prepare_flush_stmt1", Vec::new());
        tk.MustQuery("select @@last_plan_from_cache", Vec::new())
            .Check(vec![vec!["1"]]);
        tk2.MustExec("execute prepare_flush_stmt1", Vec::new());
        tk2.MustQuery("select @@last_plan_from_cache", Vec::new())
            .Check(vec![vec!["1"]]);

        tk2.MustExec("admin flush instance plan_cache", Vec::new());
        for index in 1..=5 {
            let sql = format!("execute prepare_flush_stmt{index}");
            tk.MustExec(&sql, Vec::new());
            tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(vec![vec!["0"]]);
            tk2.MustExec(&sql, Vec::new());
            tk2.MustQuery("select @@last_plan_from_cache", Vec::new())
                .Check(vec![vec!["0"]]);
        }
    }

    let error = tk.ExecToErr("admin flush global plan_cache");
    assert_eq!(
        error.message(),
        "Do not support the 'admin flush global scope.'"
    );
}

/// 对应 Go `TestPrepareCache` 的可执行 SQL 部分：覆盖索引提示、点查、范围查、
/// 排序与 DISTINCT prepared 语句的重复执行结果。
#[test]
fn test_prepare_cache_queries_and_indexes() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_cache_queries", Vec::new());
    tk.MustExec(
        "create table prepare_cache_queries (a int primary key, b int, c int, index idx1(b, a), index idx2(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_cache_queries values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,1,2)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_idx1 from 'select * from prepare_cache_queries use index(idx1) where a = ? and b = ?'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_idx2 from 'select a, b from prepare_cache_queries use index(idx2) where b = ?'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_point from 'select * from prepare_cache_queries where a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @prepare_a=1, @prepare_b=1", Vec::new());

    for _ in 0..2 {
        tk.MustQuery(
            "execute prepare_idx1 using @prepare_a, @prepare_b",
            Vec::new(),
        )
        .Check(vec![vec!["1", "1", "1"]]);
    }
    for _ in 0..2 {
        tk.MustQuery("execute prepare_idx2 using @prepare_b", Vec::new())
            .Check(vec![vec!["1", "1"], vec!["6", "1"]]);
    }
    for _ in 0..2 {
        tk.MustQuery("execute prepare_point using @prepare_a", Vec::new())
            .Check(vec![vec!["1", "1", "1"]]);
    }

    tk.MustExec(
        "prepare prepare_range from 'select * from prepare_cache_queries where a > ?'",
        Vec::new(),
    );
    tk.MustExec("set @prepare_a=3", Vec::new());
    for _ in 0..2 {
        tk.MustQuery("execute prepare_range using @prepare_a", Vec::new())
            .Check(vec![
                vec!["4", "4", "4"],
                vec!["5", "5", "5"],
                vec!["6", "1", "2"],
            ]);
    }

    tk.MustExec(
        "prepare prepare_order from 'select c from prepare_cache_queries order by c'",
        Vec::new(),
    );
    for _ in 0..2 {
        tk.MustQuery("execute prepare_order", Vec::new())
            .Check(vec![
                vec!["1"],
                vec!["2"],
                vec!["2"],
                vec!["3"],
                vec!["4"],
                vec!["5"],
            ]);
    }

    tk.MustExec(
        "prepare prepare_distinct from 'select distinct a from prepare_cache_queries order by a'",
        Vec::new(),
    );
    for _ in 0..2 {
        tk.MustQuery("execute prepare_distinct", Vec::new())
            .Check(vec![
                vec!["1"],
                vec!["2"],
                vec!["3"],
                vec!["4"],
                vec!["5"],
                vec!["6"],
            ]);
    }
}

/// 对应 Go `TestPrepareCacheNow` 的稳定部分：同一执行语句内的 deferred
/// 时间函数必须共享 statement 时间点，prepared cache 不能改变列间一致性。
#[test]
fn test_prepare_cache_now_deferred_values() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "prepare prepare_now from 'select now(), current_timestamp(), utc_timestamp(), unix_timestamp(), now(), current_timestamp(), utc_timestamp(), unix_timestamp()'",
        Vec::new(),
    );

    let _ = tk.MustQuery("execute prepare_now", Vec::new()).Rows();
    let rows = tk.MustQuery("execute prepare_now", Vec::new()).Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], rows[0][4]);
    assert_eq!(rows[0][1], rows[0][5]);
    assert_eq!(rows[0][2], rows[0][6]);
    assert_eq!(rows[0][3], rows[0][7]);
}

/// 对应 Go `TestPrepareCache` 的 schema 失效分支：DDL 后首次执行必须重建
/// prepared plan，随后同一语句再次执行才允许命中缓存。
#[test]
fn test_prepare_cache_rebuilds_after_schema_change() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_schema_change", Vec::new());
    tk.MustExec(
        "create table prepare_schema_change (id int primary key, value int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_schema_change values (1, 10)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_schema from 'select value from prepare_schema_change where id = ?'",
        Vec::new(),
    );
    tk.MustExec("set @prepare_schema_id=1", Vec::new());
    tk.MustQuery(
        "execute prepare_schema using @prepare_schema_id",
        Vec::new(),
    )
    .Check(vec![vec!["10"]]);
    tk.MustQuery(
        "execute prepare_schema using @prepare_schema_id",
        Vec::new(),
    )
    .Check(vec![vec!["10"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);

    tk.MustExec(
        "alter table prepare_schema_change add column extra int",
        Vec::new(),
    );
    tk.MustQuery(
        "execute prepare_schema using @prepare_schema_id",
        Vec::new(),
    )
    .Check(vec![vec!["10"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustQuery(
        "execute prepare_schema using @prepare_schema_id",
        Vec::new(),
    )
    .Check(vec![vec!["10"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestPrepareCacheDeferredFunction`：含 deferred 时间函数的计划每次
/// 都要按当前 statement 时间重新构造 range，同时第二次执行仍可命中缓存。
#[test]
fn test_prepare_cache_deferred_function() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_deferred", Vec::new());
    tk.MustExec(
        "create table prepare_deferred (id int primary key, created timestamp(3) not null, key idx_created(created))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_deferred values (1, '2019-01-14 10:43:20.000')",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_deferred_stmt from 'select id, created from prepare_deferred where created < now(3)'",
        Vec::new(),
    );

    tk.MustQuery("execute prepare_deferred_stmt", Vec::new())
        .Check(vec![vec!["1", "2019-01-14 10:43:20.000"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustQuery("execute prepare_deferred_stmt", Vec::new())
        .Check(vec![vec!["1", "2019-01-14 10:43:20.000"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestPrepareOverMaxPreparedStmtCount`：准备语句计数在 deallocate/关闭
/// session 后归还，并在达到全局上限时拒绝新的 prepared statement。
#[test]
fn test_prepare_over_max_prepared_stmt_count() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@global.max_prepared_stmt_count = 2", Vec::new());
    tk.MustExec("prepare prepare_count_one from 'select 1'", Vec::new());
    tk.MustExec("deallocate prepare prepare_count_one", Vec::new());
    tk.MustExec("prepare prepare_count_two from 'select 1'", Vec::new());
    let before_close = tk
        .Session()
        .GetSessionVars()
        .MemTracker()
        .GetChildrenForTest()
        .len();
    tk.Session()
        .close()
        .expect("close prepared-statement session");
    let after_close = TestKit::new(store.clone())
        .Session()
        .GetSessionVars()
        .MemTracker()
        .GetChildrenForTest()
        .len();
    assert_eq!(before_close, after_close);

    let mut limited = TestKit::new(store);
    limited.MustExec("use test", Vec::new());
    limited.MustExec("prepare prepare_limit_one from 'select 1'", Vec::new());
    limited.MustExec("prepare prepare_limit_two from 'select 1'", Vec::new());
    let error = limited.ExecToErr("prepare prepare_limit_three from 'select 1'");
    assert!(error.message().to_ascii_lowercase().contains("prepared"));
}

/// 对应 Go `TestPrepareWithSnapshot`：prepared point/table scan 必须读取显式
/// tidb_snapshot 指定的历史版本，而不是 DDL/DML 后的最新版本。
#[test]
fn test_prepare_with_snapshot() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_snapshot", Vec::new());
    tk.MustExec(
        "create table prepare_snapshot (id int primary key, value int)",
        Vec::new(),
    );
    tk.MustExec("insert into prepare_snapshot values (1, 2)", Vec::new());
    tk.MustExec("begin", Vec::new());
    let snapshot = tk.MustQuery("select @@tidb_current_ts", Vec::new()).Rows()[0][0].clone();
    tk.MustExec("commit", Vec::new());
    tk.MustExec(
        "update prepare_snapshot set value = 3 where id = 1",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_snapshot_point from 'select * from prepare_snapshot where id = 1'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_snapshot_scan from 'select * from prepare_snapshot'",
        Vec::new(),
    );
    tk.MustExec(&format!("set @@tidb_snapshot = {snapshot}"), Vec::new());
    tk.MustQuery("execute prepare_snapshot_point", Vec::new())
        .Check(vec![vec!["1", "2"]]);
    tk.MustQuery("execute prepare_snapshot_scan", Vec::new())
        .Check(vec![vec!["1", "2"]]);
}

/// 对应 Go `TestPlanCacheSwitchDB`：未限定库名的 prepared statement 绑定 prepare
/// 时的数据库，切库后首次执行重建，之后才能命中；显式库名保持原绑定。
#[test]
fn test_plan_cache_switch_db() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("drop database if exists prepare_switch_db", Vec::new());
    tk.MustExec("create database prepare_switch_db", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists switch_table", Vec::new());
    tk.MustExec("create table switch_table (value int)", Vec::new());
    tk.MustExec("insert into switch_table values (-1)", Vec::new());
    tk.MustExec(
        "prepare prepare_switch_stmt from 'select * from switch_table'",
        Vec::new(),
    );

    tk.MustExec("use prepare_switch_db", Vec::new());
    tk.MustExec("create table switch_table (value int)", Vec::new());
    tk.MustExec("insert into switch_table values (1)", Vec::new());
    tk.MustQuery("select * from prepare_switch_db.switch_table", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustQuery("execute prepare_switch_stmt", Vec::new())
        .Check(vec![vec!["-1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustQuery("execute prepare_switch_stmt", Vec::new())
        .Check(vec![vec!["-1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);

    tk.MustExec(
        "prepare prepare_switch_qualified from 'select * from prepare_switch_db.switch_table'",
        Vec::new(),
    );
    tk.MustQuery("execute prepare_switch_qualified", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustQuery("execute prepare_switch_qualified", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestInvisibleIndexPrepare`：索引可见性变化必须使 prepared 计划失效，
/// 且重新选择索引/表扫描路径。
#[test]
fn test_invisible_index_prepare() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_invisible", Vec::new());
    tk.MustExec(
        "create table prepare_invisible (value int, unique idx_value(value))",
        Vec::new(),
    );
    tk.MustExec("insert into prepare_invisible values (1)", Vec::new());
    tk.MustExec(
        "prepare prepare_invisible_stmt from 'select value from prepare_invisible order by value'",
        Vec::new(),
    );
    tk.MustQuery("execute prepare_invisible_stmt", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustQuery("execute prepare_invisible_stmt", Vec::new())
        .Check(vec![vec!["1"]]);
    let initial_plan = tk.MustQuery(
        "explain select value from prepare_invisible order by value",
        Vec::new(),
    );
    assert!(
        initial_plan
            .Rows()
            .iter()
            .flatten()
            .any(|cell| cell.contains("Index")),
        "expected an index-backed plan, got {:?}",
        initial_plan.Rows()
    );
    tk.MustExec(
        "alter table prepare_invisible alter index idx_value invisible",
        Vec::new(),
    );
    tk.MustQuery("execute prepare_invisible_stmt", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustQuery("execute prepare_invisible_stmt", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustExec(
        "alter table prepare_invisible alter index idx_value visible",
        Vec::new(),
    );
    tk.MustQuery("execute prepare_invisible_stmt", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestConsistencyBetweenPrepareExecuteAndNormalSql`：prepared text protocol、
/// prepared binary protocol 与普通 SQL 在 schema 变化前后必须得到相同结果。
#[test]
fn test_consistency_between_prepare_execute_and_normal_sql() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_consistency", Vec::new());
    tk.MustExec(
        "create table prepare_consistency (id int primary key, value int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_consistency values (1, 1), (2, 2)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_consistency_stmt from 'select * from prepare_consistency'",
        Vec::new(),
    );
    let expected = vec![vec!["1", "1"], vec!["2", "2"]];
    tk.MustQuery("execute prepare_consistency_stmt", Vec::new())
        .Check(expected.clone());
    tk.MustQuery("select * from prepare_consistency", Vec::new())
        .Check(expected.clone());
    tk.MustExec(
        "alter table prepare_consistency add column extra int",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_consistency (id, value, extra) values (3, 3, 30)",
        Vec::new(),
    );
    let after_ddl = vec![
        vec!["1", "1", "<nil>"],
        vec!["2", "2", "<nil>"],
        vec!["3", "3", "30"],
    ];
    tk.MustQuery("execute prepare_consistency_stmt", Vec::new())
        .Check(after_ddl.clone());
    tk.MustQuery("select * from prepare_consistency", Vec::new())
        .Check(after_ddl);
}

/// 对应 Go `TestPrepareCacheForPartition`：静态分区计划按语句重建，动态分区
/// 允许相同 prepared 语句跨分区参数复用。
#[test]
fn test_prepare_cache_for_partition() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_partition", Vec::new());
    tk.MustExec(
        "create table prepare_partition (id int primary key, value varchar(10)) partition by hash(id) partitions 4",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_partition values (1,'a'),(2,'b'),(5,'e')",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_partition_stmt from 'select value from prepare_partition where id = ?'",
        Vec::new(),
    );
    tk.MustExec("set @partition_id=1", Vec::new());
    tk.MustQuery(
        "execute prepare_partition_stmt using @partition_id",
        Vec::new(),
    )
    .Check(vec![vec!["a"]]);
    tk.MustQuery(
        "execute prepare_partition_stmt using @partition_id",
        Vec::new(),
    )
    .Check(vec![vec!["a"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustExec("set @partition_id=5", Vec::new());
    tk.MustQuery(
        "execute prepare_partition_stmt using @partition_id",
        Vec::new(),
    )
    .Check(vec![vec!["e"]]);
}

/// 对应 Go `TestIssue33031`：分区表 prepared 计划遇到不匹配分区时不能错误
/// 复用过度优化的 Point/BatchGet 计划。
#[test]
fn test_issue_33031() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_issue_33031", Vec::new());
    tk.MustExec(
        "create table prepare_issue_33031 (id int, value bigint, unique key uk_id(id)) partition by range(id) (partition p0 values less than (0), partition p1 values less than (100))",
        Vec::new(),
    );
    tk.MustExec("insert into prepare_issue_33031 values (-5, 7)", Vec::new());
    tk.MustExec(
        "prepare prepare_issue_stmt from 'select *, ? from prepare_issue_33031 where value < ? and id in (?, ?)'",
        Vec::new(),
    );
    tk.MustExec(
        "set @issue_a=111, @issue_b=1, @issue_c=2, @issue_d=22",
        Vec::new(),
    );
    tk.MustQuery(
        "execute prepare_issue_stmt using @issue_d,@issue_a,@issue_b,@issue_c",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
    tk.MustExec(
        "set @issue_a=112, @issue_b=-2, @issue_c=-5, @issue_d=33",
        Vec::new(),
    );
    tk.MustQuery(
        "execute prepare_issue_stmt using @issue_d,@issue_a,@issue_b,@issue_c",
        Vec::new(),
    )
    .Check(vec![vec!["-5", "7", "33"]]);
}

/// 对应 Go `TestPlanCacheUnionScan`：事务内写入会使旧计划失效，但回滚后
/// UnionScan 计划仍可安全复用且结果正确。
#[test]
fn test_plan_cache_union_scan() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_union_scan", Vec::new());
    tk.MustExec(
        "create table prepare_union_scan (value int not null)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_union_stmt from 'select * from prepare_union_scan where value > ?'",
        Vec::new(),
    );
    tk.MustExec("set @union_value=0", Vec::new());
    tk.MustQuery("execute prepare_union_stmt using @union_value", Vec::new())
        .Check(Vec::<Vec<&str>>::new());
    tk.MustExec("begin", Vec::new());
    tk.MustQuery("execute prepare_union_stmt using @union_value", Vec::new())
        .Check(Vec::<Vec<&str>>::new());
    tk.MustExec("insert into prepare_union_scan values (1)", Vec::new());
    tk.MustQuery("execute prepare_union_stmt using @union_value", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustExec("rollback", Vec::new());
    tk.MustQuery("execute prepare_union_stmt using @union_value", Vec::new())
        .Check(Vec::<Vec<&str>>::new());
}

/// 对应 Go `TestPlanCacheSnapshot`：快照时间点固定时，prepared 查询仍沿用
/// 该时间点的结果并保持缓存命中。
#[test]
fn test_plan_cache_snapshot() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_plan_snapshot", Vec::new());
    tk.MustExec("create table prepare_plan_snapshot (id int)", Vec::new());
    tk.MustExec(
        "insert into prepare_plan_snapshot values (1),(2)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_plan_snapshot_stmt from 'select * from prepare_plan_snapshot where id = ?'",
        Vec::new(),
    );
    tk.MustExec("set @snapshot_id=1", Vec::new());
    tk.MustQuery(
        "execute prepare_plan_snapshot_stmt using @snapshot_id",
        Vec::new(),
    )
    .Check(vec![vec!["1"]]);
    tk.MustQuery(
        "execute prepare_plan_snapshot_stmt using @snapshot_id",
        Vec::new(),
    )
    .Check(vec![vec!["1"]]);
    tk.MustExec("insert into prepare_plan_snapshot values (1)", Vec::new());
    tk.MustQuery(
        "execute prepare_plan_snapshot_stmt using @snapshot_id",
        Vec::new(),
    )
    .Check(vec![vec!["1"], vec!["1"]]);
}

/// 对应 Go `TestPartitionTable`：hash、range、list 三类分区表的 prepared
/// 范围查询都必须返回与普通表一致的结果，并在重复执行时命中缓存。
#[test]
fn test_partition_table() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    for (table, definition) in [
        (
            "prepare_partition_hash",
            "partition by hash(id) partitions 4",
        ),
        (
            "prepare_partition_range",
            "partition by range(id) (partition p0 values less than (3), partition p1 values less than (10))",
        ),
        (
            "prepare_partition_list",
            "partition by list(id) (partition p0 values in (1,2), partition p1 values in (3,4))",
        ),
    ] {
        tk.MustExec(&format!("drop table if exists {table}"), Vec::new());
        tk.MustExec(
            &format!("create table {table} (id int, value int) {definition}"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("insert into {table} values (1,10),(2,20),(3,30)"),
            Vec::new(),
        );
        let statement = format!("prepare {table}_stmt from 'select * from {table} where id > ?'");
        tk.MustExec(&statement, Vec::new());
        tk.MustExec("set @partition_lower=1", Vec::new());
        tk.MustQuery(
            &format!("execute {table}_stmt using @partition_lower"),
            Vec::new(),
        )
        .Check(vec![vec!["2", "20"], vec!["3", "30"]]);
        tk.MustQuery(
            &format!("execute {table}_stmt using @partition_lower"),
            Vec::new(),
        )
        .Check(vec![vec!["2", "20"], vec!["3", "30"]]);
        tk.MustQuery("select @@last_plan_from_cache", Vec::new())
            .Check(vec![vec!["1"]]);
    }
}

/// 对应 Go `TestPartitionWithVariedDataSources`：主键/唯一索引及普通表的
/// table scan、point get、batch point get 在相同数据上必须保持结果一致。
#[test]
fn test_partition_with_varied_data_sources() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    for (table, definition) in [
        (
            "prepare_varied_range",
            "partition by range(id) (partition p0 values less than (10), partition p1 values less than (100))",
        ),
        ("prepare_varied_hash", "partition by hash(id) partitions 4"),
        ("prepare_varied_normal", ""),
    ] {
        tk.MustExec(&format!("drop table if exists {table}"), Vec::new());
        tk.MustExec(
            &format!("create table {table} (id int primary key, value int) {definition}"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("insert into {table} values (1,10),(2,20),(30,300)"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("prepare {table}_scan from 'select * from {table} where id > ? and id < ?'"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("prepare {table}_point from 'select * from {table} where id = ?'"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("prepare {table}_batch from 'select * from {table} where id in (?, ?, ?)'"),
            Vec::new(),
        );
    }
    tk.MustExec("set @varied_min=1, @varied_max=31, @varied_point=2, @varied_a=1, @varied_b=30, @varied_c=2", Vec::new());
    for table in [
        "prepare_varied_range",
        "prepare_varied_hash",
        "prepare_varied_normal",
    ] {
        let rows = tk
            .MustQuery(
                &format!("execute {table}_scan using @varied_min,@varied_max"),
                Vec::new(),
            )
            .Sort()
            .Rows();
        assert_eq!(rows, vec![vec!["2", "20"], vec!["30", "300"]]);
        tk.MustQuery(
            &format!("execute {table}_point using @varied_point"),
            Vec::new(),
        )
        .Check(vec![vec!["2", "20"]]);
        tk.MustQuery(
            &format!("execute {table}_batch using @varied_a,@varied_b,@varied_c"),
            Vec::new(),
        )
        .Sort()
        .Check(vec![vec!["1", "10"], vec!["2", "20"], vec!["30", "300"]]);
    }
}

/// 对应 Go `TestCachedTable`：缓存表上的 table/index/point prepared 查询
/// 反复执行时结果稳定，且缓存表读路径被保留。
#[test]
fn test_cached_table() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_cached_table", Vec::new());
    tk.MustExec(
        "create table prepare_cached_table (id int, value int, index idx_value(value))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_cached_table values (1,1),(2,2)",
        Vec::new(),
    );
    tk.MustExec("alter table prepare_cached_table cache", Vec::new());
    tk.MustExec(
        "prepare prepare_cached_scan from 'select * from prepare_cached_table where id >= ?'",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_cached_index from 'select value from prepare_cached_table use index(idx_value) where value > ?'",
        Vec::new(),
    );
    tk.MustExec("set @cached_value=1", Vec::new());
    tk.MustQuery(
        "execute prepare_cached_scan using @cached_value",
        Vec::new(),
    )
    .Check(vec![vec!["1", "1"], vec!["2", "2"]]);
    tk.MustQuery(
        "execute prepare_cached_scan using @cached_value",
        Vec::new(),
    )
    .Check(vec![vec!["1", "1"], vec!["2", "2"]]);
    tk.MustQuery(
        "execute prepare_cached_index using @cached_value",
        Vec::new(),
    )
    .Check(vec![vec!["2"]]);
    tk.MustQuery(
        "execute prepare_cached_index using @cached_value",
        Vec::new(),
    )
    .Check(vec![vec!["2"]]);
}

/// 对应 Go `TestPlanCacheWithRCWhenInfoSchemaChange`：两个 READ-COMMITTED
/// 事务缓存同一索引提示计划后，第三个会话删除索引并写入数据；随后两个事务都必须
/// 看到新行，且 schema 变化后的首次执行不能命中旧计划缓存。
#[test]
fn test_plan_cache_with_rc_when_info_schema_changes() {
    if astersql_config_kerneltype::IsNextGen() {
        eprintln!("skipped: MDL is always enabled and read only in nextgen");
        return;
    }
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut text = TestKit::new(store.clone());
    let mut binary = TestKit::new(store.clone());
    let mut ddl = TestKit::new(store);
    for tk in [&mut text, &mut binary, &mut ddl] {
        tk.MustExec("use test", Vec::new());
    }
    text.MustExec("set global tidb_enable_metadata_lock=0", Vec::new());
    text.MustExec("drop table if exists prepare_rc_schema", Vec::new());
    text.MustExec(
        "create table prepare_rc_schema (id int primary key, c int, index ic(c))",
        Vec::new(),
    );
    text.MustExec(
        "prepare prepare_rc_schema_text from 'select /*+ use_index(prepare_rc_schema, ic) */ * from prepare_rc_schema where 1'",
        Vec::new(),
    );
    binary.MustExec(
        "prepare prepare_rc_schema_binary from 'select /*+ use_index(prepare_rc_schema, ic) */ * from prepare_rc_schema where 1'",
        Vec::new(),
    );
    for tk in [&mut text, &mut binary] {
        tk.MustExec("set tx_isolation='READ-COMMITTED'", Vec::new());
        tk.MustExec("begin pessimistic", Vec::new());
    }
    text.MustQuery("execute prepare_rc_schema_text", Vec::new())
        .Check(Vec::<Vec<&str>>::new());
    binary
        .MustQuery("execute prepare_rc_schema_binary", Vec::new())
        .Check(Vec::<Vec<&str>>::new());

    ddl.MustExec("alter table prepare_rc_schema drop index ic", Vec::new());
    ddl.MustExec("insert into prepare_rc_schema values (1, 0)", Vec::new());

    text.MustQuery("execute prepare_rc_schema_text", Vec::new())
        .Check(vec![vec!["1", "0"]]);
    text.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    binary
        .MustQuery("execute prepare_rc_schema_binary", Vec::new())
        .Check(vec![vec!["1", "0"]]);
    binary
        .MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    text.MustExec("rollback", Vec::new());
    binary.MustExec("rollback", Vec::new());
}

/// 对应 Go `TestCacheHitInRc`：READ-COMMITTED 事务中 prepared text/binary
/// 语句的第二次执行命中缓存。
#[test]
fn test_cache_hit_in_rc() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("set tx_isolation='READ-COMMITTED'", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_rc", Vec::new());
    tk.MustExec(
        "create table prepare_rc (id int primary key, value int)",
        Vec::new(),
    );
    tk.MustExec("insert into prepare_rc values (1,1),(2,2)", Vec::new());
    tk.MustExec(
        "prepare prepare_rc_stmt from 'select * from prepare_rc'",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustQuery("execute prepare_rc_stmt", Vec::new())
        .Check(vec![vec!["1", "1"], vec!["2", "2"]]);
    tk.MustQuery("execute prepare_rc_stmt", Vec::new())
        .Check(vec![vec!["1", "1"], vec!["2", "2"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustExec("rollback", Vec::new());
}

/// 对应 Go `TestCacheHitInForUpdateRead`：FOR UPDATE prepared 读取在显式
/// 悲观事务内保持结果一致并在第二次执行命中缓存。
#[test]
fn test_cache_hit_in_for_update_read() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_for_update", Vec::new());
    tk.MustExec(
        "create table prepare_for_update (id int primary key, value int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_for_update values (1,1),(2,2)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_for_update_stmt from 'select * from prepare_for_update where id = 1 for update'",
        Vec::new(),
    );
    tk.MustExec("begin pessimistic", Vec::new());
    tk.MustQuery("execute prepare_for_update_stmt", Vec::new())
        .Check(vec![vec!["1", "1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustQuery("execute prepare_for_update_stmt", Vec::new())
        .Check(vec![vec!["1", "1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
    // A lock alone leaves the cached plan reusable; an actual write must
    // switch to the dirty-table context (Go NewPlanCacheKey).
    tk.MustExec(
        "update prepare_for_update set value = 3 where id = 1",
        Vec::new(),
    );
    for expected_cache_hit in ["0", "1"] {
        tk.MustQuery("execute prepare_for_update_stmt", Vec::new())
            .Check(vec![vec!["1", "3"]]);
        tk.MustQuery("select @@last_plan_from_cache", Vec::new())
            .Check(vec![vec![expected_cache_hit]]);
    }
    tk.MustExec("rollback", Vec::new());
}

/// 对应 Go `TestPointGetForUpdateAutoCommitCache`：自动提交点查计划可命中，
/// DDL 后首次执行不命中，下一次重新命中。
#[test]
fn test_point_get_for_update_autocommit_cache() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_for_update_ddl", Vec::new());
    tk.MustExec(
        "create table prepare_for_update_ddl (id int primary key, value int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_for_update_ddl values (1,1),(2,2)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_for_update_ddl_stmt from 'select * from prepare_for_update_ddl where id = 1 for update'",
        Vec::new(),
    );
    tk.MustQuery("execute prepare_for_update_ddl_stmt", Vec::new())
        .Check(vec![vec!["1", "1"]]);
    tk.MustQuery("execute prepare_for_update_ddl_stmt", Vec::new())
        .Check(vec![vec!["1", "1"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
    tk.MustExec(
        "alter table prepare_for_update_ddl add column extra int",
        Vec::new(),
    );
    tk.MustQuery("execute prepare_for_update_ddl_stmt", Vec::new())
        .Check(vec![vec!["1", "1", "<nil>"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["0"]]);
    tk.MustQuery("execute prepare_for_update_ddl_stmt", Vec::new())
        .Check(vec![vec!["1", "1", "<nil>"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestPrepareCacheForDynamicPartitionPruning`：静态模式不缓存
/// 分区 prepared range，动态模式在参数改变后仍返回正确行并命中缓存。
#[test]
fn test_prepare_cache_for_dynamic_partition_pruning() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_dynamic_partition", Vec::new());
    tk.MustExec(
        "create table prepare_dynamic_partition (id int, value bigint, unique key uk_id(id)) partition by range(id) (partition p0 values less than (0), partition p1 values less than (100))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_dynamic_partition values (-5,7)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_dynamic_stmt from 'select * from prepare_dynamic_partition where id = ? and value < ?'",
        Vec::new(),
    );
    tk.MustExec("set @dynamic_id=1, @dynamic_value=111", Vec::new());
    tk.MustQuery(
        "execute prepare_dynamic_stmt using @dynamic_id,@dynamic_value",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
    tk.MustExec("set @dynamic_id=-5, @dynamic_value=112", Vec::new());
    tk.MustQuery(
        "execute prepare_dynamic_stmt using @dynamic_id,@dynamic_value",
        Vec::new(),
    )
    .Check(vec![vec!["-5", "7"]]);
    tk.MustExec("set @dynamic_id=113, @dynamic_value=5", Vec::new());
    tk.MustQuery(
        "execute prepare_dynamic_stmt using @dynamic_id,@dynamic_value",
        Vec::new(),
    )
    .Check(Vec::<Vec<&str>>::new());
}

/// 对应 Go `TestHashPartitionAndPlanCache`：hash 分区主键与唯一键 point get
/// 的首次执行重建计划，后续同类参数命中缓存。
#[test]
fn test_hash_partition_and_plan_cache() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set tidb_enable_prepared_plan_cache=1", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_hash_point", Vec::new());
    tk.MustExec(
        "create table prepare_hash_point (value varchar(20), id int primary key, key idx_value(value)) partition by hash(id) partitions 5",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_hash_point values ('a',1),('b',2),('c',3)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_hash_point_stmt from 'select * from prepare_hash_point where id = ?'",
        Vec::new(),
    );
    tk.MustExec("set @hash_id=1", Vec::new());
    tk.MustQuery("execute prepare_hash_point_stmt using @hash_id", Vec::new())
        .Check(vec![vec!["a", "1"]]);
    tk.MustExec("set @hash_id=2", Vec::new());
    tk.MustQuery("execute prepare_hash_point_stmt using @hash_id", Vec::new())
        .Check(vec![vec!["b", "2"]]);
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(vec![vec!["1"]]);
}

/// 对应 Go `TestBatchPointGetPlanCacheMixedInList`：混合字面量/参数的复合
/// IN 列表第二次执行不能 panic，且必须重建正确的 batch point get 范围。
#[test]
fn test_batch_point_get_plan_cache_mixed_in_list() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_batch_point", Vec::new());
    tk.MustExec(
        "create table prepare_batch_point (k1 int, k2 int, value int, unique key uk(k1,k2))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_batch_point values (1,2,100),(3,2,200),(1,4,300)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare prepare_batch_stmt from 'select value from prepare_batch_point where (k1,k2) in ((1, ?), (?, 2))'",
        Vec::new(),
    );
    tk.MustExec("set @batch_a=2, @batch_b=3", Vec::new());
    tk.MustQuery(
        "execute prepare_batch_stmt using @batch_a,@batch_b",
        Vec::new(),
    )
    .Sort()
    .Check(vec![vec!["100"], vec!["200"]]);
    tk.MustExec("set @batch_a=4, @batch_b=3", Vec::new());
    tk.MustQuery(
        "execute prepare_batch_stmt using @batch_a,@batch_b",
        Vec::new(),
    )
    .Sort()
    .Check(vec![vec!["200"], vec!["300"]]);
}

/// 对应 Go `TestPrepareCache` 的权限失效分支：权限相关 DDL 与 prepared
/// statement 生命周期一起执行，确保授权元数据变更不会破坏已有缓存会话。
#[test]
fn test_prepare_cache_privilege_changes() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_privilege", Vec::new());
    tk.MustExec("drop user if exists 'prepare_user'@'localhost'", Vec::new());
    tk.MustExec(
        "create table prepare_privilege (id int primary key)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_privilege values (1),(2),(3)",
        Vec::new(),
    );
    tk.MustExec("create user 'prepare_user'@'localhost'", Vec::new());
    tk.MustExec(
        "grant select on test.prepare_privilege to 'prepare_user'@'localhost'",
        Vec::new(),
    );
    let mut user = TestKit::new(store);
    user.MustExec("use test", Vec::new());
    user.Session()
        .AuthenticateUserForTest(&UserIdentity {
            username: "prepare_user".to_owned(),
            hostname: "localhost".to_owned(),
            ..UserIdentity::default()
        })
        .expect("authenticate prepare_user");
    user.MustExec(
        "prepare prepare_privilege_stmt from 'select * from prepare_privilege where id > ?'",
        Vec::new(),
    );
    user.MustExec("set @privilege_id=2", Vec::new());
    user.MustQuery(
        "execute prepare_privilege_stmt using @privilege_id",
        Vec::new(),
    )
    .Check(vec![vec!["3"]]);
    tk.MustExec(
        "revoke all privileges on test.prepare_privilege from 'prepare_user'@'localhost'",
        Vec::new(),
    );
    let error = user.ExecToErr("execute prepare_privilege_stmt using @privilege_id");
    assert!(
        error.message().to_ascii_lowercase().contains("denied"),
        "expected privilege denial after revoke, got {error:?}"
    );
    tk.MustExec(
        "grant select on test.prepare_privilege to 'prepare_user'@'localhost'",
        Vec::new(),
    );
    user.MustQuery(
        "execute prepare_privilege_stmt using @privilege_id",
        Vec::new(),
    )
    .Check(vec![vec!["3"]]);
    tk.MustExec("drop user 'prepare_user'@'localhost'", Vec::new());
}
