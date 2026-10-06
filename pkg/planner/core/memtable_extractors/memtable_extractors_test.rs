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
        !extractor
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
// 单边时间谓词必须保持开放端点，不能再隐式收窄为一小时窗口。
fn statements_summary_preserves_open_ended_time_ranges() {
    const MIN_DATETIME_MS: i64 = -62_135_596_800_000;
    const MAX_DATETIME_MS: i64 = 253_402_300_799_999;

    let mut lower_bounded = StatementsSummaryExtractor::default();
    let lower_predicate = Predicate::Ge("summary_end_time".into(), PredicateValue::I64(100));
    assert_eq!(
        lower_bounded.ExtractPredicates(std::slice::from_ref(&lower_predicate)),
        [lower_predicate]
    );
    assert_eq!(
        lower_bounded.CoarseTimeRange,
        Some(TimeRange::new(100, MAX_DATETIME_MS))
    );
    assert!(
        lower_bounded
            .ExplainInfo()
            .contains("end_time: 9999-12-31 23:59:59.999999")
    );

    let mut upper_bounded = StatementsSummaryExtractor::default();
    let upper_predicate = Predicate::Le("summary_begin_time".into(), PredicateValue::I64(200));
    assert_eq!(
        upper_bounded.ExtractPredicates(std::slice::from_ref(&upper_predicate)),
        [upper_predicate]
    );
    assert_eq!(
        upper_bounded.CoarseTimeRange,
        Some(TimeRange::new(MIN_DATETIME_MS, 200))
    );
    assert!(
        upper_bounded
            .ExplainInfo()
            .contains("start_time: 0001-01-01 00:00:00.000000")
    );
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

#[test]
fn cluster_log_like_compiles_regex_for_remote_search() {
    let predicate = Predicate::Like("message".into(), "%a\\%b%".into());
    let mut extractor = ClusterLogTableExtractor::default();
    assert!(extractor.ExtractPredicates(&[predicate]).is_empty());
    assert_eq!(extractor.Patterns, ["^.*a%b.*$"]);
}

#[test]
fn cluster_log_escape_and_ilike_keep_scalar_rechecks() {
    for (predicate, pattern, recheck) in [
        (
            Predicate::LikeWithEscape(
                "message".into(),
                "%a#%b%".into(),
                LikeEscape::Constant(b'#'),
            ),
            "^.*a%b.*$",
            false,
        ),
        (
            Predicate::Ilike(
                "message".into(),
                "%error%".into(),
                LikeEscape::Constant(b'\\'),
            ),
            "(?i:^.*error.*$)",
            true,
        ),
        (
            Predicate::Ilike(
                "message".into(),
                "%error#%%".into(),
                LikeEscape::Constant(b'#'),
            ),
            "(?i:^.*error%.*$)",
            true,
        ),
        (
            Predicate::Or(vec![
                Predicate::Ilike("message".into(), "%pd%".into(), LikeEscape::Constant(b'\\')),
                Predicate::Like("message".into(), "%tikv%".into()),
            ]),
            "(?i:^.*pd.*$)|^.*tikv.*$",
            true,
        ),
        (
            Predicate::LikeWithEscape("message".into(), r"%a\_%".into(), LikeEscape::Constant(0)),
            r"^.*a\\..*$",
            false,
        ),
    ] {
        let mut extractor = ClusterLogTableExtractor::default();
        let remaining = extractor.ExtractPredicates(std::slice::from_ref(&predicate));
        assert_eq!(extractor.Patterns, [pattern], "{predicate:?}");
        assert_eq!(remaining, if recheck { vec![predicate] } else { vec![] });
    }
    for escape in [
        LikeEscape::Missing,
        LikeEscape::Dynamic,
        LikeEscape::Deferred,
        LikeEscape::Parameter,
    ] {
        let predicate = Predicate::Ilike("message".into(), "%FOO%".into(), escape);
        let mut extractor = ClusterLogTableExtractor::default();
        assert_eq!(
            extractor.ExtractPredicates(std::slice::from_ref(&predicate)),
            [predicate]
        );
        assert!(extractor.Patterns.is_empty());
    }
    let predicate = Predicate::Or(vec![
        Predicate::Like("message".into(), "%foo%".into()),
        Predicate::Like("other".into(), "%bar%".into()),
    ]);
    let mut extractor = ClusterLogTableExtractor::default();
    assert_eq!(
        extractor.ExtractPredicates(std::slice::from_ref(&predicate)),
        [predicate]
    );
    assert!(extractor.Patterns.is_empty());
}

#[test]
fn infoschema_custom_escape_filters_nonempty_metadata_and_retains_like() {
    let mut extractor = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
    let predicate = Predicate::LikeWithEscape(
        "table_name".into(),
        "%#_%".into(),
        LikeEscape::Constant(b'#'),
    );
    assert_eq!(
        extractor.ExtractPredicates(std::slice::from_ref(&predicate)),
        [predicate]
    );
    let tables = ["abc_def", "abc#x"].map(|name| TableInfo {
        Name: NewCIStr(name),
        ..Default::default()
    });
    let matching = tables
        .iter()
        .filter(|table| extractor.HasTableName(&table.Name.O))
        .map(|table| table.Name.O.as_str())
        .collect::<Vec<_>>();
    assert_eq!(matching, ["abc_def"]);
    assert_eq!(extractor.Base.LikePatterns["table_name"], ["%#_%"]);
    extractor.ExtractPredicates(&[Predicate::Like("table_name".into(), "abc%".into())]);
    assert!(
        tables
            .iter()
            .all(|table| extractor.HasTableName(&table.Name.O))
    );
    extractor.ExtractPredicates(&[Predicate::Ilike(
        "table_name".into(),
        "%A_%".into(),
        LikeEscape::Constant(b'A'),
    )]);
    assert!(extractor.HasTableName("abc_def"));
    assert!(!extractor.HasTableName("abc#x"));
    let unresolved =
        Predicate::LikeWithEscape("table_name".into(), "%#_%".into(), LikeEscape::Deferred);
    assert_eq!(
        extractor.ExtractPredicates(std::slice::from_ref(&unresolved)),
        [unresolved]
    );
    assert!(extractor.Base.LikePatterns.is_empty());
}

#[test]
fn infoschema_folded_like_retains_original_filters() {
    let table = TableInfo {
        Name: NewCIStr("test70825"),
        ..Default::default()
    };
    for pattern in ["T%", "t%"] {
        let predicate = Predicate::Like("table_name".into(), pattern.into());
        let mut extractor = InfoSchemaTablesExtractor::NewInfoSchemaTablesExtractor();
        assert_eq!(
            extractor.ExtractPredicates(&[predicate.clone()]),
            [predicate]
        );
        assert_eq!(extractor.Base.LikePatterns["table_name"], ["t%"]);
        assert!(extractor.HasTableName(&table.Name.O));
    }
    let mut columns = InfoSchemaColumnsExtractor::NewInfoSchemaColumnsExtractor();
    for field in ["table_name", "column_name"] {
        let predicate = Predicate::Like(field.into(), "T%".into());
        assert_eq!(columns.ExtractPredicates(&[predicate.clone()]), [predicate]);
        assert_eq!(columns.Base.LikePatterns[field], ["t%"]);
    }
    let left = Predicate::Like("column_name".into(), "abc%".into());
    let right = Predicate::Like("column_name".into(), "%def".into());
    let conjunction = [left.clone(), right.clone()];
    assert_eq!(columns.ExtractPredicates(&conjunction), conjunction);
    assert_eq!(columns.Base.LikePatterns["column_name"], ["abc%", "%def"]);
    let disjunction = Predicate::Or(vec![left, right]);
    assert_eq!(
        columns.ExtractPredicates(&[disjunction.clone()]),
        [disjunction]
    );
    assert!(columns.Base.LikePatterns.is_empty());
    let ilike = Predicate::Ilike(
        "column_name".into(),
        "T%".into(),
        LikeEscape::Constant(b'\\'),
    );
    assert!(columns.ExtractPredicates(&[ilike]).is_empty());
    assert_eq!(columns.Base.LikePatterns["column_name"], ["t%"]);
    assert!(columns.Base.Has("column_name", "Test"));
    assert!(columns.Base.Has("column_name", "test"));
}
