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

// `table_test.go` 的 Rust 对照测试。
//
// 各测试通过 [`crate::harness`] 复现同包 Go 测试的输入与预期，覆盖摘要按 digest
// 聚合、权限过滤、错误计数、敏感信息脱敏、容量淘汰、计划缓存统计与索引顾问消费。
// 这里由 harness 建模 SQL 执行及会话状态，不依赖完整的 mockstore/session 栈。

use crate::harness::{self, Env, RecordOpts};
use std::sync::MutexGuard;

/// 持有独占测试锁和隔离的语句摘要环境，避免全局配置在并行测试间相互污染。
struct Fixture {
    _guard: MutexGuard<'static, ()>,
    env: Env,
}

/// 获取串行化锁并创建恢复为默认配置的测试环境。
fn setup() -> Fixture {
    Fixture {
        _guard: harness::test_guard(),
        env: Env::setup(),
    }
}

#[test]
// 固定规范化 SQL 的 digest，防止摘要分组键偏离 TiDB 的标准算法。
fn test_stmt_summary_uses_canonical_tidb_digest() {
    let normalized = harness::normalize_digest_text("show databases;");
    assert_eq!(normalized, "show databases");
    assert_eq!(
        harness::digest_hex(&normalized),
        "0e247706bf6e791fbf4af8c8e7658af5ffc45c63179871202d8f91551ee03161"
    );
}

#[test]
// 验证索引顾问会累积消费语句摘要，并为不同过滤列分别生成建议。
fn test_stmt_summary_index_advisor() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("use test");
    env.must_exec("create table t (a int, b int, c int)");
    env.must_query_err("recommend index run");

    env.must_query("select a from t where a=1");
    let rows = env.must_query("recommend index run");
    assert_eq!(rows[0][2], "idx_a");

    env.must_query("select b from t where b=1");
    let rows = env.must_query("recommend index run");
    assert_eq!(rows[0][2], "idx_a");
    assert_eq!(rows[1][2], "idx_b");

    let rows = env.must_query(
        "select index_columns, index_details->'$.Reason' from mysql.index_advisor_results",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "a");
    assert_eq!(
        rows[0][1],
        "\"Column [a] appear in Equal or Range Predicate clause(s) in query: select `a` from `test` . `t` where `a` = ?\""
    );
    assert_eq!(rows[1][0], "b");
    assert_eq!(
        rows[1][1],
        "\"Column [b] appear in Equal or Range Predicate clause(s) in query: select `b` from `test` . `t` where `b` = ?\""
    );
    env.close();
}

#[test]
fn test_stmt_summary_index_advisor_null_schema() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("use test");
    env.must_exec("create table t (a int, b int, c int)");
    env.must_query_err("recommend index run");

    // 新会话未执行 USE 时，仍应从带库名的 SQL 中恢复目标表。
    env.auth("root", "%");
    env.must_query("select a from test.t where a=1");
    let rows = env.must_query("recommend index run");
    assert_eq!(rows[0][2], "idx_a");
    env.must_query("select b from test.t where b=1");
    let rows = env.must_query("recommend index run");
    assert_eq!(rows[0][2], "idx_a");
    assert_eq!(rows[1][2], "idx_b");
    env.close();
}

#[test]
// 验证摘要表的列元数据、同 digest 聚合，以及全局开关关闭后的清空语义。
fn test_stmt_summary_table() {
    let mut fx = setup();
    let env = &mut fx.env;
    let comment = env.must_query(
        "select column_comment from information_schema.columns where table_name='STATEMENTS_SUMMARY' and column_name='STMT_TYPE'",
    );
    assert_eq!(comment, vec![vec!["Statement type".to_owned()]]);

    env.must_exec("drop table if exists t");
    env.must_exec("create table t(a int, b varchar(10), key k(a))");
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");

    env.must_exec("insert into t values(1, 'a')");
    env.must_exec("insert into t values(2, 'b')");
    env.must_exec("insert into t VALUES(3, 'c')");
    env.must_exec("/**/insert into t values(4, 'd')");

    let rows = env.must_query(
        "select stmt_type, schema_name, table_names, index_names, exec_count, sum_cop_task_num, avg_total_keys, max_total_keys, avg_processed_keys, max_processed_keys, avg_write_keys, max_write_keys, avg_prewrite_regions, max_prewrite_regions, avg_affected_rows, query_sample_text from information_schema.statements_summary where digest_text like 'insert into `t`%'",
    );
    assert!(!rows.is_empty());
    assert_eq!(rows[0][0], "Insert");
    assert_eq!(rows[0][1], "test");
    assert_eq!(rows[0][2], "test.t");
    assert_eq!(rows[0][4], "4");
    assert_eq!(rows[0][14], "1");
    assert_eq!(rows[0][15], "insert into t values(1, 'a')");

    env.must_exec("set global tidb_enable_stmt_summary = false");
    let empty = env.must_query(
        "select stmt_type, schema_name, table_names, index_names, exec_count from information_schema.statements_summary",
    );
    assert!(empty.is_empty() || empty.iter().all(|row| row.is_empty()));
    env.must_exec("set global tidb_enable_stmt_summary = on");
    env.close();
}

#[test]
// 普通用户只能看到自己的摘要；PROCESS 权限授予后才能查看其他用户记录。
fn test_stmt_summary_table_privilege() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("create table t(a int, b varchar(10), key k(a))");
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.create_user("test_user", "localhost");
    env.record_executed(
        "select * from t where a=1",
        "select * from `t` where `a` = ?",
        RecordOpts {
            user_override: Some("root@%"),
            ..RecordOpts::default()
        },
    );
    let root_rows = env.must_query(
        "select * from information_schema.statements_summary where digest_text like 'select * from `t`%'",
    );
    assert_eq!(root_rows.len(), 1);
    let root_history = env.must_query(
        "select * from information_schema.statements_summary_history where digest_text like 'select * from `t`%'",
    );
    assert_eq!(root_history.len(), 1);

    env.auth("test_user", "localhost");
    let hidden = env.must_query(
        "select * from information_schema.statements_summary where digest_text like 'select * from `t`%'",
    );
    assert_eq!(hidden.len(), 0);
    let hidden_history = env.must_query(
        "select * from information_schema.statements_summary_history where digest_text like 'select * from `t`%'",
    );
    assert_eq!(hidden_history.len(), 0);
    env.record_executed(
        "select * from t where b=1",
        "select * from `t` where `b` = ?",
        RecordOpts {
            user_override: Some("test_user@localhost"),
            ..RecordOpts::default()
        },
    );
    let own = env.must_query(
        "select * from information_schema.statements_summary where digest_text like 'select * from `t`%'",
    );
    assert_eq!(own.len(), 1);
    let own_history = env.must_query(
        "select * from information_schema.statements_summary_history where digest_text like 'select * from `t`%'",
    );
    assert_eq!(own_history.len(), 1);

    env.auth("root", "%");
    env.grant_process("test_user", "localhost");
    env.auth("test_user", "localhost");
    let all = env.must_query(
        "select * from information_schema.statements_summary where digest_text like 'select * from `t`%'",
    );
    assert_eq!(all.len(), 2);
    let all_history = env.must_query(
        "select * from information_schema.statements_summary_history where digest_text like 'select * from `t`%'",
    );
    assert_eq!(all_history.len(), 2);
    env.close();
}

#[test]
// 同一 digest 的成功、失败与警告执行应分别累计到对应计数器。
fn test_stmt_summary_error_count() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("use test");
    env.must_exec("create table stmt_summary_test(id int primary key)");
    env.record_executed(
        "insert into stmt_summary_test values(1)",
        "insert into `stmt_summary_test` values ( ? )",
        RecordOpts {
            stmt_type: "Insert",
            error: false,
            ..RecordOpts::default()
        },
    );
    env.record_executed(
        "insert into stmt_summary_test values(1)",
        "insert into `stmt_summary_test` values ( ? )",
        RecordOpts {
            stmt_type: "Insert",
            error: true,
            ..RecordOpts::default()
        },
    );
    let rows = env.must_query(
        "select exec_count, sum_errors, sum_warnings from information_schema.statements_summary where digest_text like 'insert into `stmt_summary_test`%'",
    );
    assert_eq!(
        rows[0],
        vec!["2".to_owned(), "1".to_owned(), "0".to_owned()]
    );

    env.record_executed(
        "insert ignore into stmt_summary_test values(1)",
        "insert ignore into `stmt_summary_test` values ( ? )",
        RecordOpts {
            stmt_type: "Insert",
            warning: true,
            ..RecordOpts::default()
        },
    );
    let rows = env.must_query(
        "select exec_count, sum_errors, sum_warnings from information_schema.statements_summary where digest_text like 'insert ignore into `stmt_summary_test`%'",
    );
    assert_eq!(
        rows[0],
        vec!["1".to_owned(), "0".to_owned(), "1".to_owned()]
    );
    env.close();
}

#[test]
// PREPARE 本身不进入摘要；真正执行的参数化语句才产生统计记录。
fn test_stmt_summary_prepared_statements() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("prepare stmt from 'select ?'");
    env.record_executed(
        "select ?",
        "select ?",
        RecordOpts {
            prepared: true,
            ..RecordOpts::default()
        },
    );
    let prepare_rows = env.must_query(
        "select exec_count from information_schema.statements_summary where digest_text like \"prepare%\"",
    );
    assert!(prepare_rows.is_empty());
    let select_rows = env.must_query(
        "select exec_count from information_schema.statements_summary where digest_text like \"select ?\"",
    );
    assert_eq!(select_rows, vec![vec!["1".to_owned()]]);
    env.close();
}

#[test]
// 二进制字面量应完整保留在查询样例中，不能被文本归一化意外截断。
fn test_stmt_summary_binary_values() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("create table t1 (c1 binary(16) not null primary key)");
    env.record_executed(
        "select count(*) from t1 where c1 = 0xd2e4a6b8c1f3e5d7a9b2c4d6e8f1a3b5",
        "select count ( * ) from `t1` where `c1` = ?",
        RecordOpts {
            redacted_sample:
                "select count(*) from t1 where c1 = 0xd2e4a6b8c1f3e5d7a9b2c4d6e8f1a3b5".to_owned(),
            ..RecordOpts::default()
        },
    );
    let rows = env.must_query(
        "select query_sample_text from information_schema.statements_summary where digest_text like 'select count%from `t1` where%'",
    );
    assert_eq!(
        rows[0][0],
        "select count(*) from t1 where c1 = 0xd2e4a6b8c1f3e5d7a9b2c4d6e8f1a3b5"
    );
    env.close();
}

#[test]
// 涉及口令的账户语句只能暴露脱敏后的查询样例。
fn test_stmt_summary_sensitive_query() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("create user user_sensitive identified by '123456789';");
    env.must_exec("alter user 'user_sensitive'@'%' identified by 'abcdefg';");
    env.must_exec("set password for 'user_sensitive'@'%' = 'xyzuvw';");
    let rows = env.must_query(
        "select query_sample_text from `information_schema`.`STATEMENTS_SUMMARY` where query_sample_text like '%user_sensitive%' and (query_sample_text like 'set password%' or query_sample_text like 'create user%' or query_sample_text like 'alter user%') order by query_sample_text;",
    );
    assert_eq!(
        rows,
        vec![
            vec!["alter user {user_sensitive@% password = ***}".to_owned()],
            vec!["create user {user_sensitive@% password = ***}".to_owned()],
            vec!["set password for user user_sensitive@%".to_owned()],
        ]
    );
    env.close();
}

#[test]
// 容量仅允许一个 digest 时，普通摘要表仍应同时呈现淘汰汇总行。
fn test_stmt_summary_table_other() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_enable_stmt_summary=0");
    env.must_exec("set global tidb_enable_stmt_summary=1");
    env.must_exec("set global tidb_stmt_summary_max_stmt_count=1");
    env.must_exec("show databases;");
    let rows = env
        .must_query("SELECT DIGEST_TEXT, DIGEST FROM `INFORMATION_SCHEMA`.`STATEMENTS_SUMMARY`;");
    assert!(
        rows.iter().any(|row| row[0] == "show databases"
            && row[1] == "0e247706bf6e791fbf4af8c8e7658af5ffc45c63179871202d8f91551ee03161"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row[1] == "<nil>"), "{rows:?}");
    env.close();
}

#[test]
// 历史摘要表需保留规范 digest，并以 `<nil>` 行汇总被淘汰的语句。
fn test_stmt_summary_history_table_other() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_stmt_summary_max_stmt_count = 1");
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("set global tidb_stmt_summary_max_stmt_count=1");
    env.must_exec("show databases;");
    let rows = env.must_query(
        "SELECT DIGEST_TEXT, DIGEST FROM `INFORMATION_SCHEMA`.`STATEMENTS_SUMMARY_HISTORY`;",
    );
    assert!(
        rows.iter().any(|row| row[0] == "show databases"
            && row[1] == "0e247706bf6e791fbf4af8c8e7658af5ffc45c63179871202d8f91551ee03161"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row[1] == "<nil>"), "{rows:?}");
    env.close();
}

#[test]
// 非预处理计划缓存命中后，摘要中的缓存标志与命中次数必须同步更新。
fn test_performance_schema_for_non_prep_plan_cache() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("use test");
    env.must_exec("create table t (a int, key(a))");
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("set tidb_enable_non_prepared_plan_cache=1");

    env.must_query("select * from t where a=1");
    env.must_query("select * from t where a=1");
    assert_eq!(
        env.must_query("select @@last_plan_from_cache"),
        vec![vec!["1".to_owned()]]
    );
    let rows = env.must_query(
        "select exec_count, digest_text, prepared, plan_in_cache, plan_cache_hits, query_sample_text from information_schema.statements_summary where digest_text='select * from `t` where `a` = ?'",
    );
    assert_eq!(rows[0][0], "2");
    assert_eq!(rows[0][1], "select * from `t` where `a` = ?");
    assert_eq!(rows[0][3], "1");
    assert_eq!(rows[0][4], "1");
    assert_eq!(rows[0][5], "select * from t where a=1");

    env.must_query("select * from t where a=2");
    env.must_query("select * from t where a=3");
    let rows = env.must_query(
        "select exec_count, digest_text, prepared, plan_in_cache, plan_cache_hits, query_sample_text from information_schema.statements_summary where digest_text='select * from `t` where `a` = ?'",
    );
    assert_eq!(rows[0][0], "4");
    assert_eq!(rows[0][3], "1");
    assert_eq!(rows[0][4], "3");

    env.must_exec("set tidb_enable_non_prepared_plan_cache=0");
    env.must_query("select * from t where a=2");
    env.must_query("select * from t where a=3");
    let rows = env.must_query(
        "select exec_count, digest_text, prepared, plan_in_cache, plan_cache_hits, query_sample_text from information_schema.statements_summary where digest_text='select * from `t` where `a` = ?'",
    );
    assert_eq!(rows[0][0], "6");
    assert_eq!(rows[0][3], "0");
    assert_eq!(rows[0][4], "3");
    env.close();
}

#[test]
// 不可缓存原因按 digest 聚合；大量重复执行后计数仍须准确累加。
fn test_plan_cache_unqualified() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("use test");
    env.must_exec("create table t1 (a int, b int)");
    env.must_exec("create table t2 (a int, b int)");
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");

    for _ in 0..4 {
        env.record_executed(
            "select * from t1 where a<='123'",
            "select * from `t1` where `a` <= ?",
            RecordOpts {
                unqualified_reason: Some("'123' may be converted to INT"),
                ..RecordOpts::default()
            },
        );
    }
    for _ in 0..3 {
        env.record_executed(
            "select * from t1 where t1.a > (select 1 from t2 where t2.b<1)",
            "select * from `t1` where `t1` . `a` > ( select ? from `t2` where `t2` . `b` < ? )",
            RecordOpts {
                unqualified_reason: Some("query has uncorrelated sub-queries is un-cacheable"),
                ..RecordOpts::default()
            },
        );
    }
    for _ in 0..2 {
        env.record_executed(
            "select /*+ ignore_plan_cache() */ * from t1",
            "select * from `t1`",
            RecordOpts {
                unqualified_reason: Some("ignore_plan_cache hint used in SQL query"),
                ..RecordOpts::default()
            },
        );
    }
    for _ in 0..2 {
        env.record_executed(
            "select database() from t1",
            "select database ( ) from `t1`",
            RecordOpts {
                unqualified_reason: Some("query has 'database' is un-cacheable"),
                ..RecordOpts::default()
            },
        );
    }
    let rows = env.must_query(
        "select digest_text, exec_count, plan_cache_unqualified, plan_cache_unqualified_last_reason from information_schema.statements_summary where plan_cache_unqualified > 0",
    );
    assert_eq!(
        rows,
        vec![
            vec![
                "select * from `t1`",
                "2",
                "2",
                "ignore_plan_cache hint used in SQL query"
            ],
            vec![
                "select * from `t1` where `a` <= ?",
                "4",
                "4",
                "'123' may be converted to INT"
            ],
            vec![
                "select * from `t1` where `t1` . `a` > ( select ? from `t2` where `t2` . `b` < ? )",
                "3",
                "3",
                "query has uncorrelated sub-queries is un-cacheable"
            ],
            vec![
                "select database ( ) from `t1`",
                "2",
                "2",
                "query has 'database' is un-cacheable"
            ],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_owned).collect())
        .collect::<Vec<Vec<String>>>()
    );

    for _ in 0..100 {
        env.record_executed(
            "select /*+ ignore_plan_cache() */ * from t1",
            "select * from `t1`",
            RecordOpts {
                unqualified_reason: Some("ignore_plan_cache hint used in SQL query"),
                ..RecordOpts::default()
            },
        );
        env.record_executed(
            "select database() from t1",
            "select database ( ) from `t1`",
            RecordOpts {
                unqualified_reason: Some("query has 'database' is un-cacheable"),
                ..RecordOpts::default()
            },
        );
    }
    let rows = env.must_query(
        "select digest_text, exec_count, plan_cache_unqualified, plan_cache_unqualified_last_reason from information_schema.statements_summary where plan_cache_unqualified > 0",
    );
    assert!(
        rows.iter()
            .any(|row| row[0] == "select * from `t1`" && row[1] == "102")
    );
    assert!(
        rows.iter()
            .any(|row| row[0] == "select database ( ) from `t1`" && row[1] == "102")
    );
    for _ in 0..20 {
        env.record_executed(
            "select * from t1 where a<='123'",
            "select * from `t1` where `a` <= ?",
            RecordOpts {
                unqualified_reason: Some("'123' may be converted to INT"),
                ..RecordOpts::default()
            },
        );
        env.record_executed(
            "select * from t1 where a<=123",
            "select * from `t1` where `a` <= ?",
            RecordOpts::default(),
        );
    }
    let rows = env.must_query(
        "select digest_text, exec_count, plan_cache_unqualified, plan_cache_unqualified_last_reason from information_schema.statements_summary where digest_text like '%<= ?%'",
    );
    assert_eq!(
        rows[0],
        vec![
            "select * from `t1` where `a` <= ?".to_owned(),
            "44".to_owned(),
            "24".to_owned(),
            "'123' may be converted to INT".to_owned(),
        ]
    );
    env.close();
}

#[test]
// 枚举临时表、生成列、子查询等不可缓存场景，核对最后一次拒绝原因。
fn test_plan_cache_unqualified2() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("use test");
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");

    let cases = [
        (
            "select * from `t1` , `t_temp_unqualified_test` where `t1` . `a` > ?",
            "query accesses temporary tables is un-cacheable",
        ),
        (
            "select * from `t1` , `t_gen_unqualified_test` where `t1` . `a` > ?",
            "query accesses generated columns is un-cacheable",
        ),
        (
            "select * from `t1` where `t1` . `a` > ( select max ( `a` ) from `t_subquery_unqualified_test` )",
            "query has uncorrelated sub-queries is un-cacheable",
        ),
        (
            "select * from `t1` where `t1` . `a` > ( select `a` from `t_apply_unqualified_test` where `t1` . `b` > `t_apply_unqualified_test` . `b` )",
            "PhysicalApply plan is un-cacheable",
        ),
        (
            "select * from `t_ignore_unqualified_test`",
            "ignore_plan_cache hint used in SQL query",
        ),
        (
            "select user ( ) from `t_non_deterministic_1_unqualified_test`",
            "query has 'user' is un-cacheable",
        ),
        (
            "select `version` ( ) from `t_non_deterministic_2_unqualified_test`",
            "query has 'version' is un-cacheable",
        ),
        (
            "select * from `t_limit_unqualified_test` limit ?",
            "limit count is too large",
        ),
        (
            "select ? from `t_system_unqualified_test` , `information_schema` . `tables`",
            "PhysicalMemTable plan is un-cacheable",
        ),
    ];
    for (digest, reason) in cases {
        env.record_executed(
            digest,
            digest,
            RecordOpts {
                unqualified_reason: Some(reason),
                ..RecordOpts::default()
            },
        );
        let rows = env.must_query(
            "select digest_text, exec_count, plan_cache_unqualified, plan_cache_unqualified_last_reason from information_schema.statements_summary where plan_cache_unqualified > 0",
        );
        assert!(
            rows.iter().any(|row| row[0] == digest && row[3] == reason),
            "missing {digest} / {reason} in {rows:?}"
        );
    }
    env.close();
}

#[test]
// 预处理语句首次生成计划不算命中，后续复用应累加命中并标记仍在缓存中。
fn test_performance_schema_for_plan_cache() {
    let mut fx = setup();
    let env = &mut fx.env;
    env.must_exec("set global tidb_enable_stmt_summary = 0");
    env.must_exec("set global tidb_enable_stmt_summary = 1");
    env.must_exec("use test");
    env.must_exec("create table t(a int)");
    env.record_executed(
        "select * from t",
        "select * from `t`",
        RecordOpts {
            prepared: true,
            from_cache: false,
            can_cache: true,
            ..RecordOpts::default()
        },
    );
    let rows = env.must_query(
        "select plan_cache_hits, plan_in_cache from information_schema.statements_summary where digest_text='select * from `t`'",
    );
    assert_eq!(rows[0], vec!["0".to_owned(), "0".to_owned()]);
    for _ in 0..3 {
        env.record_executed(
            "select * from t",
            "select * from `t`",
            RecordOpts {
                prepared: true,
                from_cache: true,
                can_cache: true,
                ..RecordOpts::default()
            },
        );
    }
    let rows = env.must_query(
        "select plan_cache_hits, plan_in_cache from information_schema.statements_summary where digest_text='select * from `t`'",
    );
    assert_eq!(rows[0], vec!["3".to_owned(), "1".to_owned()]);
    env.close();
}
