// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 内存表（mem table）谓词抽取器单元测试。
// 将 WHERE 条件解析为对集群配置、日志、指标与 information_schema 等内存表的过滤参数。

use memtable_extractors::*;

/// 构造字符串谓词值。
fn text(value: &str) -> PredicateValue {
    PredicateValue::String(value.to_owned())
}

/// 校验集群配置表：节点类型与实例条件被抽取且无需再保留。
#[test]
fn TestClusterConfigTableExtractor() {
    let mut extractor = ClusterTableExtractor::default();
    let remained = extractor.ExtractPredicates(&[
        Predicate::In("type".into(), vec![text("TiKV"), text("TiDB")]),
        Predicate::Eq("instance".into(), text("127.0.0.1:20160")),
    ]);
    assert!(remained.is_empty());
    assert_eq!(
        extractor.NodeTypes,
        ["tidb".into(), "tikv".into()].into_iter().collect()
    );
    assert!(!extractor.SkipRequest);

    let remaining = extractor.ExtractPredicates(&[
        Predicate::In("type".into(), vec![text("TiKV"), text("PD")]),
        Predicate::In("type".into(), vec![text("TiKV"), text("TiDB")]),
        Predicate::Eq("instance".into(), text("TiKV-0:20160")),
        Predicate::Eq("unhandled".into(), text("keep")),
    ]);
    assert_eq!(extractor.NodeTypes, ["tikv".into()].into_iter().collect());
    assert_eq!(
        extractor.Instances,
        ["TiKV-0:20160".into()].into_iter().collect()
    );
    assert!(!extractor.SkipRequest);
    assert_eq!(remaining, [Predicate::Eq("unhandled".into(), text("keep"))]);

    extractor.ExtractPredicates(&[
        Predicate::Eq("type".into(), text("tikv")),
        Predicate::Eq("type".into(), text("tidb")),
    ]);
    assert!(extractor.SkipRequest);
}

/// 校验集群日志表：时间窗口、消息模式与日志级别抽取。
#[test]
fn TestClusterLogTableExtractor() {
    let mut extractor = ClusterLogTableExtractor::default();
    let remained = extractor.ExtractPredicates(&[
        Predicate::Ge("time".into(), PredicateValue::I64(100)),
        Predicate::Le("time".into(), PredicateValue::I64(200)),
        Predicate::Like("message".into(), "%raft%".into()),
        Predicate::In("level".into(), vec![text("ERROR"), text("WARN")]),
    ]);
    assert!(remained.is_empty());
    assert_eq!((extractor.StartTime, extractor.EndTime), (100, 200));
    assert_eq!(extractor.Patterns, vec!["%raft%"]);
    assert!(extractor.LogLevels.contains("error"));

    let remaining = extractor.ExtractPredicates(&[
        Predicate::Gt("time".into(), PredicateValue::I64(100)),
        Predicate::Lt("time".into(), PredicateValue::I64(200)),
        Predicate::In("level".into(), vec![text("ERROR"), text("WARN")]),
        Predicate::Eq("other".into(), text("keep")),
    ]);
    assert_eq!((extractor.StartTime, extractor.EndTime), (101, 199));
    assert_eq!(
        extractor.LogLevels,
        ["error".into(), "warn".into()].into_iter().collect()
    );
    assert_eq!(remaining, [Predicate::Eq("other".into(), text("keep"))]);

    extractor.ExtractPredicates(&[
        Predicate::Eq("level".into(), text("error")),
        Predicate::Eq("level".into(), text("warn")),
    ]);
    assert!(extractor.SkipRequest);
}

/// 校验指标表：实例、分位数与时间范围，并生成 PromQL。
#[test]
fn TestMetricTableExtractor() {
    let mut extractor = MetricTableExtractor::default();
    let remained = extractor.ExtractPredicates(&[
        Predicate::Eq("instance".into(), text("tidb-0")),
        Predicate::In(
            "quantile".into(),
            vec![PredicateValue::F64(0.9), PredicateValue::F64(0.99)],
        ),
        Predicate::Ge("time".into(), PredicateValue::I64(10)),
        Predicate::Le("time".into(), PredicateValue::I64(20)),
    ]);
    assert_eq!(remained, [Predicate::Eq("instance".into(), text("tidb-0"))]);
    assert_eq!(extractor.StartTime, 10);
    assert_eq!(extractor.EndTime, 20);
    assert_eq!(
        extractor.GetMetricTablePromQL("tidb_query_duration"),
        r#"tidb_query_duration{instance=\"tidb-0\"}"#
    );

    let remaining = extractor.ExtractPredicates(&[
        Predicate::Eq("instance".into(), text("tidb-0")),
        Predicate::In("instance".into(), vec![text("tidb-0"), text("tidb-1")]),
        Predicate::Eq("quantile".into(), PredicateValue::F64(0.9)),
        Predicate::Ge("time".into(), PredicateValue::I64(10)),
        Predicate::Le("time".into(), PredicateValue::I64(20)),
        Predicate::Eq("unhandled".into(), text("keep")),
    ]);
    assert_eq!(
        extractor.LabelConditions["instance"],
        ["tidb-0".into()].into_iter().collect()
    );
    assert_eq!(extractor.Quantiles, ["0.9".into()].into_iter().collect());
    assert_eq!((extractor.StartTime, extractor.EndTime), (10, 20));
    assert_eq!(remaining, [
        Predicate::Eq("instance".into(), text("tidb-0")),
        Predicate::In("instance".into(), vec![text("tidb-0"), text("tidb-1")]),
        Predicate::Eq("unhandled".into(), text("keep")),
    ]);

    extractor.ExtractPredicates(&[Predicate::Eq("quantile".into(), PredicateValue::F64(1.1))]);
    assert!(!extractor.SkipRequest);

    let remained = extractor.ExtractPredicates(&[
        Predicate::In(
            "quantile".into(),
            vec![PredicateValue::F64(0.999), PredicateValue::F64(0.95)],
        ),
        Predicate::In(
            "quantile".into(),
            vec![PredicateValue::F64(0.99), PredicateValue::F64(0.95)],
        ),
    ]);
    assert!(remained.is_empty());
    assert_eq!(extractor.Quantiles, ["0.95".into()].into_iter().collect());

    extractor.ExtractPredicates(&[
        Predicate::Ge("time".into(), PredicateValue::I64(20)),
        Predicate::Le("time".into(), PredicateValue::I64(10)),
    ]);
    assert!(extractor.SkipRequest);
}

/// 校验指标摘要表：指标名与分位数集合。
#[test]
fn TestMetricsSummaryTableExtractor() {
    let mut extractor = MetricSummaryTableExtractor::default();
    let remained = extractor.ExtractPredicates(&[
        Predicate::In("metrics_name".into(), vec![text("cpu"), text("memory")]),
        Predicate::Eq("quantile".into(), PredicateValue::F64(0.99)),
    ]);
    assert_eq!(remained, [Predicate::Eq("quantile".into(), PredicateValue::F64(0.99))]);
    assert_eq!(extractor.MetricsNames.len(), 2);
    assert!(extractor.Quantiles.contains("0.99"));

    extractor.ExtractPredicates(&[
        Predicate::In("metrics_name".into(), vec![text("cpu"), text("disk")]),
        Predicate::In("metrics_name".into(), vec![text("cpu"), text("memory")]),
    ]);
    assert_eq!(extractor.MetricsNames, ["cpu".into()].into_iter().collect());
    extractor.ExtractPredicates(&[
        Predicate::Eq("metrics_name".into(), text("cpu")),
        Predicate::Eq("metrics_name".into(), text("memory")),
    ]);
    assert!(extractor.SkipRequest);

    let remained = extractor.ExtractPredicates(&[
        Predicate::In(
            "quantile".into(),
            vec![PredicateValue::F64(0.999), PredicateValue::F64(0.95)],
        ),
        Predicate::In(
            "quantile".into(),
            vec![PredicateValue::F64(0.99), PredicateValue::F64(0.95)],
        ),
        Predicate::Eq("other".into(), text("keep")),
    ]);
    assert_eq!(extractor.Quantiles, ["0.95".into()].into_iter().collect());
    assert_eq!(remained, [
        Predicate::In("quantile".into(), vec![PredicateValue::F64(0.999), PredicateValue::F64(0.95)]),
        Predicate::In("quantile".into(), vec![PredicateValue::F64(0.99), PredicateValue::F64(0.95)]),
        Predicate::Eq("other".into(), text("keep")),
    ]);
}

/// 校验巡检结果表：规则与检查项过滤。
#[test]
fn TestInspectionResultTableExtractor() {
    let mut extractor = InspectionResultTableExtractor::default();
    extractor.ExtractPredicates(&[
        Predicate::Eq("rule".into(), text("config")),
        Predicate::In(
            "item".into(),
            vec![text("ddl.lease"), text("raftstore.sync-log")],
        ),
    ]);
    assert!(extractor.Rules.contains("config"));
    assert_eq!(extractor.Items.len(), 2);

    extractor.ExtractPredicates(&[
        Predicate::In("rule".into(), vec![text("ddl"), text("config")]),
        Predicate::In("rule".into(), vec![text("config"), text("slow_query")]),
        Predicate::Eq("item".into(), text("ddl.lease")),
    ]);
    assert_eq!(extractor.Rules, ["config".into()].into_iter().collect());
    extractor.ExtractPredicates(&[
        Predicate::Eq("item".into(), text("ddl.lease")),
        Predicate::Eq("item".into(), text("other")),
    ]);
    assert!(extractor.SkipInspection);
}

/// 校验巡检摘要表：规则与指标名。
#[test]
fn TestInspectionSummaryTableExtractor() {
    let mut extractor = InspectionSummaryTableExtractor::default();
    extractor.ExtractPredicates(&[
        Predicate::Eq("rule".into(), text("query-summary")),
        Predicate::Eq("metrics_name".into(), text("qps")),
    ]);
    assert!(extractor.Rules.contains("query-summary"));
    assert!(extractor.MetricNames.contains("qps"));

    extractor.ExtractPredicates(&[
        Predicate::In("rule".into(), vec![text("ddl"), text("config")]),
        Predicate::In("rule".into(), vec![text("config"), text("slow_query")]),
        Predicate::In("metrics_name".into(), vec![text("qps"), text("latency")]),
        Predicate::In("metrics_name".into(), vec![text("qps"), text("errors")]),
    ]);
    assert_eq!(extractor.Rules, ["config".into()].into_iter().collect());
    assert_eq!(extractor.MetricNames, ["qps".into()].into_iter().collect());
}

/// 校验巡检规则表：规则类型集合。
#[test]
fn TestInspectionRuleTableExtractor() {
    let mut extractor = InspectionRuleTableExtractor::default();
    extractor.ExtractPredicates(&[Predicate::In(
        "type".into(),
        vec![text("inspection"), text("summary")],
    )]);
    assert_eq!(extractor.Types.len(), 2);

    extractor.ExtractPredicates(&[
        Predicate::Eq("type".into(), text("inspection")),
        Predicate::Eq("type".into(), text("summary")),
    ]);
    assert!(extractor.SkipRequest);
}

/// 校验热点 Region 历史表：时间、Region ID 与 Leader 标记。
/// Region 是 TiKV 的数据分片单位。
#[test]
fn TestTiDBHotRegionsHistoryTableExtractor() {
    let mut extractor = HotRegionsHistoryTableExtractor::default();
    extractor.ExtractPredicates(&[
        Predicate::Ge("update_time".into(), PredicateValue::I64(100)),
        Predicate::Le("update_time".into(), PredicateValue::I64(200)),
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(1), PredicateValue::I64(2)],
        ),
        Predicate::Eq("is_leader".into(), PredicateValue::Bool(true)),
    ]);
    assert_eq!(extractor.RegionIDs, [1, 2].into_iter().collect());
    assert!(extractor.IsLeaders.contains(&true));
    assert!(!extractor.SkipRequest);

    let remaining = extractor.ExtractPredicates(&[
        Predicate::Gt("update_time".into(), PredicateValue::I64(100)),
        Predicate::Lt("update_time".into(), PredicateValue::I64(200)),
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(1), PredicateValue::I64(2)],
        ),
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(2), PredicateValue::I64(3)],
        ),
        Predicate::Eq("is_learner".into(), PredicateValue::Bool(false)),
        Predicate::Eq("type".into(), text("read")),
        Predicate::Eq("other".into(), text("keep")),
    ]);
    assert_eq!((extractor.StartTime, extractor.EndTime), (101, 199));
    assert_eq!(extractor.RegionIDs, [2].into_iter().collect());
    assert_eq!(extractor.IsLearners, [false].into_iter().collect());
    assert_eq!(
        extractor.HotRegionTypes,
        ["read".into()].into_iter().collect()
    );
    assert_eq!(remaining, [Predicate::Eq("other".into(), text("keep"))]);

    extractor.ExtractPredicates(&[
        Predicate::Eq("region_id".into(), PredicateValue::I64(1)),
        Predicate::Eq("region_id".into(), PredicateValue::I64(2)),
    ]);
    assert!(extractor.SkipRequest);
}

/// 校验 TiKV Region Peer 表：Region 与 Store 过滤。
#[test]
fn TestTikvRegionPeersExtractor() {
    let mut extractor = TikvRegionPeersExtractor::default();
    extractor.ExtractPredicates(&[
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(1), PredicateValue::I64(2)],
        ),
        Predicate::Eq("store_id".into(), PredicateValue::I64(3)),
    ]);
    assert_eq!(extractor.RegionIDs, [1, 2].into_iter().collect());
    assert!(extractor.StoreIDs.contains(&3));

    extractor.ExtractPredicates(&[
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(1), PredicateValue::I64(2)],
        ),
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(2), PredicateValue::I64(3)],
        ),
    ]);
    assert_eq!(extractor.RegionIDs, [2].into_iter().collect());
    extractor.ExtractPredicates(&[
        Predicate::Eq("store_id".into(), PredicateValue::I64(1)),
        Predicate::Eq("store_id".into(), PredicateValue::I64(2)),
    ]);
    assert!(extractor.SkipRequest);
}

/// 校验 information_schema.COLUMNS：库名、表名与列名模式。
#[test]
fn TestColumns() {
    let mut extractor = InfoSchemaColumnsExtractor::NewInfoSchemaColumnsExtractor();
    extractor.ExtractPredicates(&[
        Predicate::Eq("table_schema".into(), text("Test")),
        Predicate::In("table_name".into(), vec![text("t1"), text("t2")]),
        Predicate::Like("column_name".into(), "id%".into()),
    ]);
    assert!(extractor.Base.Has("table_schema", "test"));
    assert_eq!(extractor.Base.ColPredicates["table_name"].len(), 2);
    assert_eq!(extractor.Base.LikePatterns["column_name"], vec!["id%"]);

    extractor.ExtractPredicates(&[Predicate::Like("column_name".into(), "ID\\_%".into())]);
    assert!(extractor.Base.Has("column_name", "id_customer"));
    assert!(!extractor.Base.Has("column_name", "identity"));
}

/// 校验 TiKV Region 状态表：按 table_id 过滤并有序返回。
#[test]
fn TestTikvRegionStatusExtractor() {
    let mut extractor = TiKVRegionStatusExtractor::default();
    extractor.ExtractPredicates(&[Predicate::In(
        "table_id".into(),
        vec![PredicateValue::I64(2), PredicateValue::I64(1)],
    )]);
    assert_eq!(extractor.GetTablesID(), vec![1, 2]);

    extractor.ExtractPredicates(&[
        Predicate::In(
            "table_id".into(),
            vec![PredicateValue::I64(1), PredicateValue::I64(2)],
        ),
        Predicate::In(
            "table_id".into(),
            vec![PredicateValue::I64(2), PredicateValue::I64(3)],
        ),
    ]);
    assert_eq!(extractor.GetTablesID(), vec![2]);
}

/// 校验预处理语句场景下抽取器状态相互独立。
#[test]
fn TestExtractorInPreparedStmt() {
    let template = |value| vec![Predicate::Eq("table_name".into(), text(value))];
    let mut first = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
    let mut second = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
    first.ExtractPredicates(&template("t1"));
    second.ExtractPredicates(&template("t2"));
    assert!(first.HasTableName("t1"));
    assert!(!first.HasTableName("t2"));
    assert!(second.HasTableName("t2"));
}

/// 校验表约束 information_schema 抽取：库名与主键约束。
#[test]
fn TestInfoSchemaTableExtract() {
    let mut extractor =
        InfoSchemaTableConstraintsExtractor::NewInfoSchemaTableConstraintsExtractor();
    extractor.ExtractPredicates(&[
        Predicate::Eq("constraint_schema".into(), text("test")),
        Predicate::Eq("constraint_name".into(), text("PRIMARY")),
    ]);
    assert!(extractor.HasConstraintSchema("test"));
    assert!(extractor.HasPrimaryKey());
    assert!(!extractor.Base.SkipRequest);

    extractor.ExtractPredicates(&[
        Predicate::Eq("constraint_name".into(), text("primary")),
        Predicate::Eq("constraint_name".into(), text("foreign")),
    ]);
    assert!(extractor.Base.SkipRequest);
}
