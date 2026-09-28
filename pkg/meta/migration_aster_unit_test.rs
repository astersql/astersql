// Copyright 2026 AsterSQL.

// `pkg/meta` 根包迁移对齐单元测试。
//
// 覆盖元数据键编解码、表信息快路径过滤、DDL 元素编解码与作业过滤、
// 整型区间切分，以及基于内存 KV 的库表读写、自增 ID 访问器与策略/历史作业往返。

use std::collections::HashSet;

use astersql_meta::*;

/// 各类元数据键的编码字节与解析结果应对齐 Go 前缀格式。
#[test]
fn metadata_key_layout_and_parsing_match_go() {
    type ParseFn = fn(&[u8]) -> Result<i64, astersql_meta::errors::Error>;
    let cases: [(Vec<u8>, &[u8], ParseFn); 6] = [
        (db_key(-7), b"DB:-7".as_slice(), parse_db_key),
        (
            auto_table_id_key(42),
            b"TID:42".as_slice(),
            parse_auto_table_id_key,
        ),
        (
            auto_increment_id_key(42),
            b"IID:42".as_slice(),
            parse_auto_increment_id_key,
        ),
        (
            auto_random_table_id_key(42),
            b"TARID:42".as_slice(),
            parse_auto_random_table_id_key,
        ),
        (table_key(42), b"Table:42".as_slice(), parse_table_key),
        (sequence_key(42), b"SID:42".as_slice(), parse_sequence_key),
    ];

    for (encoded, expected, parse) in cases {
        assert_eq!(encoded, expected);
        let expected_id = std::str::from_utf8(expected)
            .unwrap()
            .split_once(':')
            .unwrap()
            .1
            .parse::<i64>()
            .unwrap();
        assert_eq!(parse(&encoded).unwrap(), expected_id);
    }
    assert!(parse_db_key(b"database:1").is_err());
    assert!(parse_auto_table_id_key(b"TID").is_err());
    assert!(parse_table_key(b"Tabletop:1").is_err());
}

/// JSON 转义还原与“必须加载公共表信息”的快路径过滤应对齐 Go。
#[test]
fn table_fast_path_filters_and_name_unescape_match_go() {
    assert_eq!(unescape(r#"a\"b\\c"#), "a\"b\\c");

    let ordinary = br#"{"fk_info":null,"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null}"#;
    assert!(!is_table_info_must_load_public(ordinary));
    let with_affinity = br#"{"fk_info":[],"partition":null,"Lock":null,"tiflash_replica":null,"temp_table_type":0,"policy_ref_info":null,"ttl_info":null,"affinity":{"level":"table"}}"#;
    assert!(is_table_info_must_load_public(with_affinity));
    assert!(is_table_info_must_load_public(br#"{"partition":null}"#));
}

/// DDL 元素（列/索引）编解码长度与历史作业 schema/table 过滤应对齐 Go。
#[test]
fn ddl_element_and_history_filter_match_go() {
    for type_key in [COLUMN_ELEMENT_KEY.as_slice(), INDEX_ELEMENT_KEY.as_slice()] {
        let element = Element {
            id: -9,
            type_key: type_key.to_vec(),
        };
        let encoded = element.encode_element();
        assert_eq!(encoded.len(), 13);
        let decoded = decode_element(&encoded).unwrap();
        assert_eq!(decoded.id, -9);
        assert_eq!(decoded.type_key, type_key);
    }
    assert!(decode_element(b"_bad_12345678").is_err());
    assert!(decode_element(b"_col_short").is_err());
    assert_eq!(
        Element {
            id: 1,
            type_key: b"x".to_vec()
        }
        .encode_element(),
        b"x\0\0\0\0\0\0\0\0\0\0\0\x01"
    );

    let job = br#"{"schema_name":"app","table_name":"orders"}"#;
    let schemas = HashSet::from(["app".to_owned()]);
    let tables = HashSet::from(["orders".to_owned()]);
    assert!(is_job_match(job, &schemas, &tables).unwrap());
    assert!(!is_job_match(job, &HashSet::from(["other".to_owned()]), &tables).unwrap());
}

/// `split_range_int64_max` 应将 `[0, 10^19)` 切成相邻衔接的三段。
#[test]
fn range_partitioning_matches_go_boundaries() {
    let ranges = split_range_int64_max(3);
    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[0].0, "0");
    assert_eq!(ranges[0].1.len(), 19);
    assert_eq!(ranges[0].1, ranges[1].0);
    assert_eq!(ranges[1].1, ranges[2].0);
    assert_eq!(ranges[2].1, "9999999999999999999");
}

/// 通过 Mutator/Reader 创建库表、更新表名与 revision，并核对简单列表与 must-load 集合。
#[test]
fn database_table_and_reader_round_trip_match_go() {
    let transaction = kv::Transaction::default();
    let snapshot = transaction.snapshot();
    let mut meta = new_mutator(transaction, Vec::new());
    let database = model::DbInfo::public_system(7, "app", "utf8mb4", "utf8mb4_bin");
    meta.create_database(&database).unwrap();
    assert!(meta.create_database(&database).is_err());

    let mut table = model::TableInfo {
        id: 11,
        name: ast::CiString::new("Orders"),
        version: model::TABLE_INFO_VERSION_5,
        has_auto_increment_column: true,
        auto_random_bits: 5,
        ..Default::default()
    };
    meta.create_table_and_set_auto_id(
        7,
        &table,
        &model::AutoIdGroup {
            row_id: 10,
            increment_id: 20,
            random_id: 30,
        },
    )
    .unwrap();
    table.name = ast::CiString::new("OrdersV2");
    meta.update_table(7, &mut table).unwrap();
    assert_eq!(table.revision, 1);

    let reader = new_reader(snapshot);
    assert_eq!(reader.get_database(7).unwrap().unwrap().name.lower, "app");
    let loaded = reader.get_table(7, 11).unwrap().unwrap();
    assert_eq!(loaded.db_id, 7);
    assert_eq!(loaded.name.original, "OrdersV2");
    assert_eq!(loaded.revision, 1);
    assert_eq!(
        reader.list_simple_tables(7).unwrap()[0].name.original,
        "OrdersV2"
    );
    let (names, must_load) = reader
        .get_all_name_to_id_and_the_must_loaded_table_info(7)
        .unwrap();
    assert_eq!(names.get("OrdersV2"), Some(&11));
    assert_eq!(must_load.len(), 1);
}

/// 自增/自随机 ID 访问器的 put/get/inc/copy/del 顺序与跨表复制应对齐 Go。
#[test]
fn auto_id_picker_copy_and_group_order_match_go() {
    let mut meta = new_mutator(kv::Transaction::default(), Vec::new());
    let database = model::DbInfo::public_system(1, "app", "utf8mb4", "utf8mb4_bin");
    meta.create_database(&database).unwrap();

    let mut accessors = meta.get_auto_id_accessors(1, 10);
    accessors
        .put(&model::AutoIdGroup {
            row_id: 4,
            increment_id: 8,
            random_id: 15,
        })
        .unwrap();
    assert_eq!(
        accessors.get().unwrap(),
        model::AutoIdGroup {
            row_id: 4,
            increment_id: 8,
            random_id: 15
        }
    );
    assert_eq!(accessors.row_id().inc(3).unwrap(), 7);
    assert_eq!(accessors.increment_id(4).get().unwrap(), 7);
    assert_eq!(accessors.increment_id(5).get().unwrap(), 8);
    accessors.increment_id(5).copy_to(2, 20).unwrap();
    drop(accessors);

    let mut copied = meta.get_auto_id_accessors(2, 20);
    assert_eq!(copied.increment_id(5).get().unwrap(), 8);
    copied.row_id().put(99).unwrap();
    drop(copied);

    let mut zero_source = meta.get_auto_id_accessors(1, 99);
    zero_source.row_id().copy_to(2, 20).unwrap();
    drop(zero_source);
    assert_eq!(
        meta.get_auto_id_accessors(2, 20).row_id().get().unwrap(),
        99
    );

    let mut original = meta.get_auto_id_accessors(1, 10);
    original.del().unwrap();
    assert_eq!(original.get().unwrap(), model::AutoIdGroup::default());
}

/// 元数据锁、schema 缓存、BDR 角色、放置策略、资源组与 DDL 历史迭代应对齐 Go。
#[test]
fn scalar_policy_history_and_schema_state_match_go() {
    let mut meta = new_mutator(kv::Transaction::default(), Vec::new());
    assert_eq!(meta.get_metadata_lock().unwrap(), (false, true));
    meta.set_metadata_lock(true).unwrap();
    assert_eq!(meta.get_metadata_lock().unwrap(), (true, false));
    assert_eq!(meta.get_schema_cache_size().unwrap(), (0, true));
    meta.set_schema_cache_size(4096).unwrap();
    assert_eq!(meta.get_schema_cache_size().unwrap(), (4096, false));
    meta.set_bdr_role("primary").unwrap();
    assert_eq!(meta.get_bdr_role().unwrap(), "primary");
    meta.clear_bdr_role().unwrap();
    assert_eq!(meta.get_bdr_role().unwrap(), "");

    let policy = model::PolicyInfo {
        id: 5,
        name: ast::CiString::new("p"),
    };
    meta.create_policy(&policy).unwrap();
    assert!(meta.create_policy(&policy).is_err());
    assert_eq!(meta.get_policy(5).unwrap(), policy);
    assert_eq!(meta.list_policies().unwrap().len(), 1);
    meta.drop_policy(5).unwrap();
    assert!(meta.get_policy(5).is_err());

    assert_eq!(meta.get_resource_group(1).unwrap().name.lower, "default");
    assert!(meta.get_resource_group(99).is_err());

    assert_eq!(meta.gen_schema_versions(2).unwrap(), 2);
    assert_eq!(meta.get_schema_version_with_non_empty_diff().unwrap(), 1);
    meta.set_schema_diff(&model::SchemaDiff { version: 2 })
        .unwrap();
    assert_eq!(meta.get_schema_version_with_non_empty_diff().unwrap(), 2);

    let first = model::Job {
        id: 1,
        schema_name: "app".into(),
        table_name: "t1".into(),
        raw_args: Vec::new(),
    };
    let second = model::Job {
        id: 2,
        schema_name: "app".into(),
        table_name: "t2".into(),
        raw_args: Vec::new(),
    };
    meta.add_history_ddl_job_public(&first, true).unwrap();
    meta.add_history_ddl_job_public(&second, true).unwrap();
    assert_eq!(meta.get_history_ddl_count().unwrap(), 2);
    assert_eq!(meta.get_history_ddl_job(1).unwrap().unwrap(), first);
    let mut iterator = meta
        .get_last_history_ddl_jobs_iterator_with_filter(
            HashSet::from(["app".to_owned()]),
            HashSet::from(["t1".to_owned()]),
        )
        .unwrap();
    assert_eq!(iterator.get_last_jobs(10).unwrap(), vec![first]);
}
