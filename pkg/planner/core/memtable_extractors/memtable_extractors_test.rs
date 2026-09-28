// Copyright 2026 AsterSQL.

// 内存表谓词提取器的跨版本语义回归测试。
//
// 覆盖 information_schema、集群诊断、热点历史、慢查询与语句摘要等提取器，
// 重点确保 Rust 实现沿用 Go 版本的大小写、谓词消费、集合求交及元数据筛选规则。

use super::*;
use model_dependency::{ColumnInfo, IndexInfo, TableInfo};
use parser_ast_dependency::NewCIStr;

// 构造字符串谓词值，避免各用例重复处理所有权转换。
fn text(value: &str) -> PredicateValue {
    PredicateValue::String(value.to_owned())
}

#[test]
// 每次抽取都必须清空旧状态；无谓词时则恢复“不过滤”的默认语义。
fn infoschema_extraction_resets_all_previous_state() {
    let mut extractor = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
    extractor.ExtractPredicates(&[
        Predicate::Eq("table_name".into(), text("orders")),
        Predicate::Like("table_schema".into(), "app%".into()),
    ]);

    assert!(extractor.HasTableName("orders"));
    extractor.ExtractPredicates(&[]);

    assert!(extractor.Base.ColPredicates.is_empty());
    assert!(extractor.Base.LikePatterns.is_empty());
    assert!(extractor.HasTableName("customers"));
    assert!(extractor.HasTableSchema("mysql"));
}

#[test]
// information_schema 的 LIKE 匹配忽略大小写，同时保留 SQL 通配符转义语义。
fn infoschema_like_patterns_filter_values_case_insensitively() {
    let mut extractor = InfoSchemaColumnsExtractor::NewInfoSchemaColumnsExtractor();
    assert!(
        extractor
            .ExtractPredicates(&[Predicate::Like("column_name".into(), "ID\\_%".into())])
            .is_empty()
    );

    assert!(extractor.Base.Has("column_name", "id_customer"));
    assert!(extractor.Base.Has("column_name", "ID_"));
    assert!(!extractor.Base.Has("column_name", "identity"));
}

#[test]
// 表 ID 等数值列应被统一转换为提取器内部使用的文本集合。
fn infoschema_integer_ids_are_extractable() {
    let mut tables = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
    let remaining = tables.ExtractPredicates(&[Predicate::Eq(
        "tidb_table_id".into(),
        PredicateValue::I64(42),
    )]);
    assert!(remaining.is_empty());
    assert!(tables.Base.Has("tidb_table_id", "42"));

    let mut constraints =
        InfoSchemaTiDBCheckConstraintsExtractor::NewInfoSchemaTiDBCheckConstraintsExtractor();
    let remaining = constraints.ExtractPredicates(&[
        Predicate::Eq("constraint_schema".into(), text("app")),
        Predicate::Eq("table_name".into(), text("orders")),
        Predicate::Eq("table_id".into(), PredicateValue::U64(7)),
    ]);
    assert!(remaining.is_empty());
    assert!(constraints.Base.Has("table_id", "7"));
}

#[test]
// 核对各 information_schema 提取器与 Go 版本一致的可提取列及空 IN 短路行为。
fn infoschema_extractors_use_the_go_column_lists() {
    let predicates = [
        Predicate::Eq("table_schema".into(), text("app")),
        Predicate::Eq("table_name".into(), text("orders")),
        Predicate::Eq("constraint_schema".into(), text("app")),
        Predicate::Eq("constraint_name".into(), text("primary")),
    ];

    let mut key_usage = InfoSchemaKeyColumnUsageExtractor::NewInfoSchemaKeyColumnUsageExtractor();
    assert!(key_usage.ExtractPredicates(&predicates).is_empty());

    let mut table_constraints =
        InfoSchemaTableConstraintsExtractor::NewInfoSchemaTableConstraintsExtractor();
    assert!(table_constraints.ExtractPredicates(&predicates).is_empty());

    let ddl_predicates = [Predicate::Eq("state".into(), text("synced"))];
    let mut ddl = InfoSchemaDDLExtractor::NewInfoSchemaDDLExtractor();
    // DDL 提取器记录 state 以供扫描端使用，但仍将原谓词留给上层继续求值。
    assert_eq!(ddl.ExtractPredicates(&ddl_predicates), ddl_predicates);
    assert!(ddl.Base.Has("state", "synced"));

    let mut partitions = InfoSchemaPartitionsExtractor::NewInfoSchemaPartitionsExtractor();
    partitions.ExtractPredicates(&[Predicate::In("partition_name".into(), Vec::new())]);
    assert!(partitions.Base.SkipRequest);
    assert!(!partitions.HasPartitionPred());
}

#[test]
// 实例地址保持原始大小写；同一列的多个 CNF 条件按交集收窄候选集合。
fn cluster_extractors_keep_case_sensitive_instances_and_intersect_cnf_sets() {
    let mut cluster = ClusterTableExtractor::default();
    cluster.ExtractPredicates(&[Predicate::Eq(
        "instance".into(),
        text("TiDB-0.EXAMPLE:4000"),
    )]);
    assert!(cluster.Instances.contains("TiDB-0.EXAMPLE:4000"));

    let mut peers = TikvRegionPeersExtractor::default();
    peers.ExtractPredicates(&[
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(1), PredicateValue::I64(2)],
        ),
        Predicate::In(
            "region_id".into(),
            vec![PredicateValue::I64(2), PredicateValue::I64(3)],
        ),
    ]);
    assert_eq!(peers.RegionIDs, [2].into_iter().collect());
    assert!(!peers.SkipRequest);
}

#[test]
// 严格时间不等式需换算为闭区间，未指定的枚举维度则采用 Go 版本的全集默认值。
fn hot_regions_apply_go_defaults_and_strict_time_bounds() {
    let mut extractor = HotRegionsHistoryTableExtractor::default();
    extractor.ExtractPredicates(&[
        Predicate::Gt("update_time".into(), PredicateValue::I64(100)),
        Predicate::Lt("update_time".into(), PredicateValue::I64(200)),
    ]);

    assert_eq!((extractor.StartTime, extractor.EndTime), (101, 199));
    assert_eq!(
        extractor.HotRegionTypes,
        ["read".into(), "write".into()].into_iter().collect()
    );
    assert_eq!(extractor.IsLearners, [false, true].into_iter().collect());
    assert_eq!(extractor.IsLeaders, [false, true].into_iter().collect());
}

#[test]
// 组合验证后补齐的提取器具有真实过滤语义，而非仅满足类型接口的占位实现。
fn missing_go_extractors_have_real_predicate_behavior() {
    let mut storage = TableStorageStatsExtractor::default();
    assert!(
        storage
            .ExtractPredicates(&[
                Predicate::Eq("table_schema".into(), text("APP")),
                Predicate::Eq("table_name".into(), text("Orders")),
            ])
            .is_empty()
    );
    assert_eq!(storage.TableSchema, ["app".into()].into_iter().collect());
    assert_eq!(storage.TableName, ["orders".into()].into_iter().collect());

    let mut tiflash = TiFlashSystemTableExtractor::default();
    assert!(
        tiflash
            .ExtractPredicates(&[
                Predicate::Eq("tiflash_instance".into(), text("TiFlash-0:3930")),
                Predicate::In("tidb_database".into(), vec![text("App"), text("Archive")],),
                Predicate::Eq("tidb_table".into(), text("Orders")),
            ])
            .is_empty()
    );
    assert!(tiflash.TiFlashInstances.contains("TiFlash-0:3930"));
    assert_eq!(tiflash.TiDBDatabases, "app,archive");
    assert_eq!(tiflash.TiDBTables, "orders");

    let mut slow = SlowQueryExtractor::default();
    // 多个行数提示取最小非零值，避免下层读取超过最严格的上层限制。
    slow.SetRowLimitHint(20);
    slow.SetRowLimitHint(5);
    slow.SetDesc(true);
    assert!(
        slow.ExtractPredicates(&[
            Predicate::Ge("time".into(), PredicateValue::I64(10)),
            Predicate::Le("time".into(), PredicateValue::I64(20)),
        ])
        .is_empty()
    );
    assert_eq!(slow.Limit, 5);
    assert!(slow.Desc);
    assert!(slow.Enable);
    assert_eq!(slow.TimeRanges, vec![TimeRange::new(10, 20)]);

    let mut inspection = InspectionSummaryTableExtractor::default();
    let remaining = inspection.ExtractPredicates(&[
        Predicate::Eq("rule".into(), text("CONFIG")),
        Predicate::Eq("metrics_name".into(), text("QPS")),
        Predicate::In(
            "quantile".into(),
            vec![PredicateValue::F64(0.9), PredicateValue::F64(0.99)],
        ),
    ]);
    assert_eq!(remaining.len(), 2);
    assert_eq!(inspection.Rules, ["config".into()].into_iter().collect());
    assert_eq!(inspection.MetricNames, ["qps".into()].into_iter().collect());
    assert_eq!(
        inspection.Quantiles,
        ["0.9".into(), "0.99".into()].into_iter().collect()
    );
}

#[test]
// V1 仅用时间谓词计算粗粒度扫描窗口，原谓词仍须保留以便后续精确过滤。
fn statements_summary_keeps_time_predicates_for_v1() {
    let predicates = [
        Predicate::Eq("digest".into(), text("ABC")),
        Predicate::Le("summary_begin_time".into(), PredicateValue::I64(200)),
        Predicate::Ge("summary_end_time".into(), PredicateValue::I64(100)),
    ];
    let mut extractor = StatementsSummaryExtractor::default();
    let remaining = extractor.ExtractPredicates(&predicates);

    assert_eq!(extractor.Digests, ["ABC".into()].into_iter().collect());
    assert_eq!(extractor.CoarseTimeRange, Some(TimeRange::new(100, 200)));
    assert_eq!(remaining, predicates[1..]);
}

#[test]
// 元数据枚举需同时保证稳定排序、隐藏列过滤、原始序号和索引条件筛选。
fn infoschema_listing_preserves_sorting_visibility_and_ordinals() {
    let table = |id, name: &str| TableInfo {
        ID: id,
        Name: NewCIStr(name),
        ..TableInfo::default()
    };
    let mut tables = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
    tables.ExtractPredicates(&[Predicate::In(
        "tidb_table_id".into(),
        vec![PredicateValue::I64(2), PredicateValue::I64(3)],
    )]);
    let (schemas, filtered) = tables.ListSchemasAndTables(&[
        (NewCIStr("z"), table(3, "b")),
        (NewCIStr("a"), table(1, "x")),
        (NewCIStr("a"), table(2, "a")),
    ]);
    assert_eq!(
        schemas
            .iter()
            .map(|schema| schema.L.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(
        filtered
            .iter()
            .map(|table| table.Name.L.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );

    let mut column_table = table(4, "columns");
    column_table.Columns = vec![
        ColumnInfo {
            Name: NewCIStr("_hidden"),
            Hidden: true,
            ..ColumnInfo::default()
        },
        ColumnInfo {
            Name: NewCIStr("id"),
            ..ColumnInfo::default()
        },
        ColumnInfo {
            Name: NewCIStr("ignored"),
            ..ColumnInfo::default()
        },
        ColumnInfo {
            Name: NewCIStr("id_suffix"),
            ..ColumnInfo::default()
        },
    ];
    let mut columns = InfoSchemaColumnsExtractor::NewInfoSchemaColumnsExtractor();
    columns.ExtractPredicates(&[Predicate::Like("column_name".into(), "id%".into())]);
    let (selected, ordinals) = columns.ListColumns(&column_table);
    assert_eq!(
        selected
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect::<Vec<_>>(),
        ["id", "id_suffix"]
    );
    assert_eq!(ordinals, [1, 3]);

    column_table.PKIsHandle = true;
    column_table.Indices = vec![
        IndexInfo {
            ID: 7,
            Name: NewCIStr("idx_orders"),
            ..IndexInfo::default()
        },
        IndexInfo {
            ID: 8,
            Name: NewCIStr("other"),
            ..IndexInfo::default()
        },
    ];
    let mut indexes = InfoSchemaTiDBIndexUsageExtractor::NewInfoSchemaTiDBIndexUsageExtractor();
    indexes.ExtractPredicates(&[Predicate::Like("index_name".into(), "idx%".into())]);
    assert_eq!(
        indexes.ListIndexes(&column_table),
        vec![IndexUsageIndexInfo {
            Name: "idx_orders".into(),
            ID: 7,
        }]
    );
}
