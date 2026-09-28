// Copyright 2026 AsterSQL.

// EXPLAIN 语句相关集成/单元测试。
//
// - 活 catalog + 刷写后的 stats_delta 行数估计出现在计划树中；
// - TiDB_JSON 编码保留嵌套 subOperators，并对 operatorInfo 中的引号/反斜杠转义。

use astersql_planner_core::{ExplainInfoForEncode, JSONToString};

/// EXPLAIN SELECT 使用真实统计信息，IndexReader/Selection 估计行数与 Go 一致。
#[test]
fn explain_select_uses_live_catalog_and_flushed_row_count() {
    let store = astersql_testkit::mockstore::CreateAnalyzeStatsStore();
    let mut testkit = astersql_testkit::TestKit::new(store.clone());
    testkit.MustExec("create table explain_t(a int, index idx_a(a))", Vec::new());
    testkit.MustExec(
        "insert into explain_t values (1), (2), (3), (4)",
        Vec::new(),
    );
    // flush stats_delta：把增量统计刷入，供优化器估计 EstRows。
    testkit.MustExec("flush stats_delta explain_t", Vec::new());

    testkit
        .MustQuery("explain select * from explain_t where a > 1", Vec::new())
        .Check(vec![
            vec![
                "IndexReader".to_owned(),
                "3.20".to_owned(),
                "root".to_owned(),
                "index:Selection".to_owned(),
            ],
            vec![
                "└─Selection".to_owned(),
                "3.20".to_owned(),
                "cop[tikv]".to_owned(),
                "gt(test.explain_t.a, 1)".to_owned(),
            ],
            vec![
                "  └─IndexFullScan".to_owned(),
                "4.00".to_owned(),
                "cop[tikv]".to_owned(),
                "table:explain_t, index:idx_a(a) keep order:false".to_owned(),
            ],
        ]);
    astersql_testkit::Database::close(store.as_ref()).unwrap();
}

/// TiDB_JSON 格式：嵌套计划字段完整，SQL 文本中的引号与反斜杠正确转义。
#[test]
fn tidb_json_explain_keeps_nested_plan_fields_and_escapes_sql_text() {
    let rows = vec![ExplainInfoForEncode {
        ID: "Projection_1".to_owned(),
        EstRows: "1.00".to_owned(),
        TaskType: "root".to_owned(),
        OperatorInfo: "select \"a\\b\"".to_owned(),
        Children: vec![ExplainInfoForEncode {
            ID: "TableFullScan_2".to_owned(),
            EstRows: "4.00".to_owned(),
            TaskType: "cop[tikv]".to_owned(),
            AccessObject: "table:t".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    }];
    let encoded = JSONToString(&rows);
    assert!(encoded.contains("\"id\": \"Projection_1\""));
    assert!(encoded.contains("\"operatorInfo\": \"select \\\"a\\\\b\\\"\""));
    assert!(encoded.contains(
        "\"subOperators\": [\n            {\n                \"id\": \"TableFullScan_2\""
    ));
    assert!(encoded.contains("\"accessObject\": \"table:t\""));
}
