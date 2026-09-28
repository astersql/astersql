// Copyright 2026 AsterSQL.

// DDL 测试桩与 Go 版本的语义对齐回归测试。
//
// 覆盖随机辅助函数的非法参数边界，以及内存测试套件执行删表和错误查询时的行为，
// 防止 Rust 测试支撑代码悄然偏离 `cmd/ddltest` 中的 Go 基线。

use astersql_cmd_ddltest::stubs::{create_ddl_suite, random_intn, random_num, random_string};

/// 与 Go 的 `rand.Intn` 保持一致：非正上界属于非法输入，必须触发 panic。
#[test]
#[should_panic]
fn random_intn_rejects_non_positive_bound_like_go() {
    let _ = random_intn(0);
}

/// 两端相等会形成空区间，并经由零上界调用触发与 Go 相同的 panic。
#[test]
#[should_panic]
fn random_num_rejects_empty_range_like_go() {
    let _ = random_num(&[7, 7]);
}

/// Go 版本无法创建负长度字节切片；Rust 实现也必须拒绝负长度。
#[test]
#[should_panic]
fn random_string_rejects_negative_length_like_go() {
    let _ = random_string(-1);
}

/// `DROP TABLE IF EXISTS` 成功后，后续查询应证明表状态已从测试套件中移除。
#[test]
fn drop_table_if_exists_removes_an_existing_table() {
    let suite = create_ddl_suite();
    suite
        .run_ddl("create table if not exists parity_drop (c1 int, primary key(c1))")
        .recv()
        .expect("create result")
        .expect("create table");
    suite
        .run_ddl("drop table if exists parity_drop")
        .recv()
        .expect("drop result")
        .expect("drop table");

    assert!(suite.query("select c1 from parity_drop").is_err());
    suite.teardown();
}

/// 查询不存在的投影列应作为查询错误返回，不能被测试桩误判为成功。
#[test]
fn unknown_projection_is_reported_as_query_error() {
    let suite = create_ddl_suite();
    let result = suite.query("select missing_column from test_insert");
    assert!(result.is_err());
    suite.teardown();
}

/// TiDB/MySQL 的 `IF NOT EXISTS` 不能重建已存在的表，更不能清空其中数据。
#[test]
fn create_table_if_not_exists_preserves_existing_table() {
    let suite = create_ddl_suite();
    suite
        .exec("insert into test_insert values (1, 2)")
        .expect("seed existing table");

    suite
        .exec("create table if not exists test_insert (replacement int)")
        .expect("IF NOT EXISTS is a no-op");

    let mut rows = suite
        .query("select c1, c2 from test_insert")
        .expect("original schema remains available");
    assert!(rows.next());
    assert_eq!(rows.current()[0].get_int64(), 1);
    assert_eq!(rows.current()[1].get_int64(), 2);
    suite.teardown();
}

/// 不带 `IF NOT EXISTS` 重复建表时，TiDB/MySQL 必须报告表已存在。
#[test]
fn duplicate_create_table_is_rejected() {
    let suite = create_ddl_suite();
    let err = suite
        .exec("create table test_insert (replacement int)")
        .expect_err("duplicate CREATE TABLE must fail");
    assert!(err.contains("already exists"), "unexpected error: {err}");
    suite.teardown();
}
