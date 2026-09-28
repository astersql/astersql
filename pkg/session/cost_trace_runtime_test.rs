// Copyright 2026 AsterSQL.

// cost_trace 的真实会话回归：固定格式列契约，并覆盖扫描、连接和聚合算子。

use crate::runtime::{ConcreteRecordSet, ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("cost_trace setup failed: {sql}: {error}"));
}

fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("cost_trace query failed: {sql}: {error}"))
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result
        .Next()
        .unwrap_or_else(|error| panic!("read cost_trace row: {sql}: {error}"))
    {
        rows.push(row);
    }
    rows
}

fn assert_cost_trace(session: &ConcreteSession, sql: &str, operator: &str) {
    let cost = rows(session, &format!("explain format='cost_trace' {sql}"));
    let verbose = rows(session, &format!("explain format='verbose' {sql}"));
    assert!(!cost.is_empty(), "empty cost_trace plan: {sql}");
    assert_eq!(cost.len(), verbose.len(), "plan row count: {sql}");
    assert!(
        cost.iter().any(|row| row.join(" ").contains(operator)),
        "missing {operator} in cost_trace plan: {cost:?}"
    );
    for (index, (cost_row, verbose_row)) in cost.iter().zip(&verbose).enumerate() {
        let cost_line = cost_row.join(" ");
        let verbose_line = verbose_row.join(" ");
        let cost_fields = cost_line.split_whitespace().collect::<Vec<_>>();
        let verbose_fields = verbose_line.split_whitespace().collect::<Vec<_>>();
        assert!(
            cost_fields.len() >= 5,
            "cost_trace row {index}: {cost_line}"
        );
        assert!(
            verbose_fields.len() >= 3,
            "verbose row {index}: {verbose_line}"
        );
        assert_eq!(
            &cost_fields[..3],
            &verbose_fields[..3],
            "row {index}: {sql}"
        );
        let task_index = cost_fields
            .iter()
            .position(|field| matches!(*field, "root" | "cop[tikv]" | "mpp[tiflash]"))
            .unwrap_or_else(|| panic!("missing task at row {index}: {cost_line}"));
        assert!(task_index > 3, "empty formula at row {index}: {sql}");
        assert!(
            !cost_fields[3..task_index].contains(&"N/A"),
            "empty formula at row {index}: {sql}"
        );
    }
}

#[test]
fn cost_trace_renders_scan_join_and_aggregate_with_verbose_prefix() {
    let (_domain, session) = CreateAnalyzeSession().expect("cost_trace session");
    execute(&session, "create database cost_trace_test");
    execute(&session, "use cost_trace_test");
    execute(
        &session,
        "create table parent (id int primary key, value int)",
    );
    execute(
        &session,
        "create table child (id int primary key, parent_id int, value int)",
    );
    execute(&session, "insert into parent values (1, 10), (2, 20)");
    execute(
        &session,
        "insert into child values (1, 1, 3), (2, 1, 4), (3, 2, 5)",
    );

    assert_cost_trace(&session, "select * from parent where value > 10", "Scan");
    assert_cost_trace(
        &session,
        "select p.id, c.value from parent p join child c on p.id = c.parent_id",
        "Join",
    );
    assert_cost_trace(
        &session,
        "select parent_id, sum(value) from child group by parent_id",
        "Agg",
    );
}

#[test]
fn semi_index_join_batch_lookup_scales_by_inner_matches() {
    let (_domain, session) = CreateAnalyzeSession().expect("cost_trace session");
    execute(&session, "create database cost_trace_index_join_test");
    execute(&session, "use cost_trace_index_join_test");
    execute(&session, "create table parent (id int primary key)");
    execute(
        &session,
        "create table child (id int primary key, parent_id int, index parent_idx(parent_id))",
    );
    execute(&session, "insert into parent values (1), (2)");
    execute(&session, "insert into child values (1, 1), (2, 1), (3, 2)");

    let plan = rows(
        &session,
        "explain format='cost_trace' select /*+ INL_JOIN(child) */ * from parent where exists (select 1 from child where child.parent_id = parent.id)",
    );
    let join = plan
        .iter()
        .map(|row| row.join(" "))
        .find(|row| row.contains("IndexJoin"))
        .unwrap_or_else(|| panic!("missing IndexJoin in plan: {plan:?}"));
    assert!(join.contains("/6.00"), "missing batch ratio: {join}");
    assert!(
        !join.contains("/6.00)*0.8"),
        "semi lookup must not scale below one match per outer row: {join}"
    );
}
