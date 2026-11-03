// Copyright 2026 AsterSQL.

// Domain 中 ANALYZE TABLE SQL 识别逻辑的单元测试。
//
// `ANALYZE TABLE` 用于收集表的统计信息（statistics），供优化器估算代价。
// 本测试核对 `is_analyze_table_sql` 对语句前缀的判定是否与 TiDB 一致：
// 仅匹配以 analyze table 开头的语句，排除普通查询与 explain analyze 等变体。

/// 验证 analyze table 前缀识别与 TiDB 语句前缀规则一致。
#[test]
fn canonical_analyze_sql_detection_matches_tidb_statement_prefixes() {
    use crate::domain::is_analyze_table_sql;
    // 正例：大小写、空白与库名修饰均应识别为 ANALYZE TABLE。
    for sql in [
        "analyze table t",
        " ANALYZE TABLE db.t",
        "\nAnalyze Table `t`",
        "/* axxxx */ analyze table test.t",
        "/*\n\t\t/*> this is a\n\t\t/*> multiple-line comment\n\t\t/*> */ analyze table test.t",
        "/*+ hint */ analyze table test.t",
        "/*+ hint */analyze table test.t",
    ] {
        assert!(is_analyze_table_sql(sql), "{sql}");
    }
    // 反例：普通 SELECT、analyze index、explain analyze 均不应匹配。
    for sql in ["select 1", "analyze index t", "explain analyze table t"] {
        assert!(!is_analyze_table_sql(sql), "{sql}");
    }
}

#[test]
fn sequence_cache_one_uses_canonical_store() {
    use std::sync::Arc;
    let storage = Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            None,
        )
        .unwrap(),
    )
    .unwrap_or_else(|_| panic!("mock storage has another owner"));
    let handle = Arc::new(crate::StorageHandle::new(storage));
    let make_domain = || {
        crate::Domain::new_with_storage_handle(
            handle.clone(),
            Arc::new(crate::KvInfoSchemaLoader::default()),
            crate::DomainConfig::default(),
        )
    };
    let domain = Arc::new(make_domain());
    let mut table = astersql_meta_model::TableInfo {
        ID: 101,
        DBID: 100,
        Sequence: Some(astersql_meta_model::SequenceInfo {
            Start: 10,
            Increment: 1,
            MinValue: 1,
            MaxValue: 1000,
            Cache: false,
            CacheValue: 1,
            Cycle: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(domain.sequence_next_value(&table).unwrap(), 10);
    // A fresh Domain and allocator share the same canonical persisted water level.
    let second = make_domain();
    assert_eq!(second.sequence_next_value(&table).unwrap(), 11);
    let threads = (0..4)
        .map(|_| {
            let domain = domain.clone();
            let table = table.clone();
            std::thread::spawn(move || {
                (0..10)
                    .map(|_| domain.sequence_next_value(&table).unwrap())
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let mut values = threads
        .into_iter()
        .flat_map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    values.sort_unstable();
    assert_eq!(values, (12..52).collect::<Vec<_>>());
    table.ID = 102;
    table.Sequence.as_mut().unwrap().Start = 1000;
    assert_eq!(domain.sequence_next_value(&table).unwrap(), 1000);
    assert!(matches!(
        domain.sequence_next_value(&table),
        Err(astersql_meta_autoid::AutoIdError::AutoIncrementReadFailed(
            _
        ))
    ));
    table.ID = 103;
    let seq = table.Sequence.as_mut().unwrap();
    seq.Start = -10;
    seq.Increment = -2;
    seq.MinValue = -12;
    seq.MaxValue = -1;
    assert_eq!(domain.sequence_next_value(&table).unwrap(), -10);
    assert_eq!(domain.sequence_next_value(&table).unwrap(), -12);
    assert!(domain.sequence_next_value(&table).is_err());
    table.ID = 104;
    table.Sequence.as_mut().unwrap().Cache = true;
    table.Sequence.as_mut().unwrap().CacheValue = 4;
    assert!(matches!(
        domain.sequence_next_value(&table),
        Err(astersql_meta_autoid::AutoIdError::NotImplemented(_))
    ));
    table.ID = 105;
    let seq = table.Sequence.as_mut().unwrap();
    seq.Start = 10;
    seq.Increment = 1;
    seq.MinValue = 1;
    seq.MaxValue = 1000;
    seq.Cache = false;
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads = (0..4)
        .map(|_| {
            let domain = domain.clone();
            let table = table.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                domain.sequence_next_value(&table).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let mut first_values = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    first_values.sort_unstable();
    assert_eq!(first_values, vec![10, 11, 12, 13]);
    table.Sequence = None;
    assert!(domain.sequence_next_value(&table).is_err());
}
