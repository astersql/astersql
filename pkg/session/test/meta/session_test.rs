// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Meta session 测试：DDL 系统表版本、保留表 ID、系统库判定与 InfoSchema 可见性。
//
// Rust 测试在 mutator 与 metadef 常量上校验版本序、保留 ID、系统库识别，
// 以及通过 Domain InfoSchema 读取建表元信息。
//
// DDLTableVersion：DDL 相关系统表演进阶段；InitDDLTables 按版本差补齐缺失系统表。
// Region：TiKV 数据分片单位；表级 split 可使系统表独占 region。

use astersql_meta::context::Context;
use astersql_meta::kv::Transaction;
use astersql_meta::{DDLTableVersion, new_mutator};
use astersql_meta_metadef::{
    CreateTiDBBackgroundSubtaskHistoryTable, CreateTiDBBackgroundSubtaskTable,
    CreateTiDBDDLHistoryTable, CreateTiDBDDLJobTable, CreateTiDBReorgTable, IsSystemRelatedDB,
    ReservedGlobalIDLowerBound, ReservedGlobalIDUpperBound, TiDBBackgroundSubtaskHistoryTableID,
    TiDBBackgroundSubtaskTableID, TiDBDDLHistoryTableID, TiDBDDLJobTableID, TiDBDDLReorgTableID,
    TiDBMDLInfoTableID,
};
use astersql_session::runtime::CreateAnalyzeSession;
use astersql_session::{
    BackfillTables, DDLJobTables, DDLNotifierTables, InitDDLTables, MDLTables, TableBasicInfo,
};
use astersql_tablecodec as tablecodec;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

/// 对应 Go TestInitDDLTables：从每个历史版本启动时只补齐后续系统表，
/// 并在全部创建成功后把版本推进到 DDLNotifier。
#[test]
fn init_ddl_tables_creates_only_tables_newer_than_the_stored_version() {
    let all_tables = DDLJobTables
        .into_iter()
        .chain(MDLTables)
        .chain(BackfillTables)
        .chain(DDLNotifierTables)
        .collect::<Vec<TableBasicInfo>>();
    let cases: [(fn() -> DDLTableVersion, i32, usize); 5] = [
        (|| DDLTableVersion::Init, DDLTableVersion::Init as i32, 0),
        (|| DDLTableVersion::Base, DDLTableVersion::Base as i32, 3),
        (|| DDLTableVersion::Mdl, DDLTableVersion::Mdl as i32, 4),
        (
            || DDLTableVersion::Backfill,
            DDLTableVersion::Backfill as i32,
            6,
        ),
        (
            || DDLTableVersion::DdlNotifier,
            DDLTableVersion::DdlNotifier as i32,
            7,
        ),
    ];

    for (initial_version, initial_ordinal, first_expected_table) in cases {
        let mut mutator = new_mutator(Transaction::default(), Vec::new());
        if initial_ordinal != DDLTableVersion::Init as i32 {
            mutator.set_ddl_table_version(initial_version()).unwrap();
        }

        InitDDLTables(&mut mutator).unwrap();
        let database_id = mutator.get_system_db_id().unwrap();
        let mut actual = mutator
            .list_tables(&Context::default(), database_id)
            .unwrap();
        actual.sort_by(|left, right| right.id.cmp(&left.id));
        let expected = &all_tables[first_expected_table..];
        assert_eq!(actual.len(), expected.len());
        assert!(actual.iter().zip(expected).all(|(actual, expected)| {
            actual.id == expected.id && actual.name.lower == expected.name
        }));
        assert_eq!(
            mutator.get_ddl_table_version().unwrap(),
            DDLTableVersion::DdlNotifier as i32
        );

        mutator
            .set_ddl_table_version(DDLTableVersion::Init)
            .unwrap();
        mutator.drop_database(database_id).unwrap();
    }
}

/// 校验 DDL job/reorg/backfill 等系统表保留 ID 互异且落在保留区间。
/// 对应 TestMetaTableRegion：DDL 相关系统表保留 ID 互异且落在 metadef 保留区间内。
// 对应 TestMetaTableRegion：DDL job/reorg/backfill/backfill-history 四张系统表的保留 ID
// 互不相同，且都落在 metadef 定义的保留区间内。
#[test]
fn ddl_system_table_reserved_ids_are_distinct_and_within_bounds() {
    let ids = [
        TiDBDDLJobTableID,
        TiDBDDLReorgTableID,
        TiDBDDLHistoryTableID,
        TiDBMDLInfoTableID,
        TiDBBackgroundSubtaskTableID,
        TiDBBackgroundSubtaskHistoryTableID,
    ];
    // 每个保留 ID 必须落在 ReservedGlobalID 上下界内。
    for id in ids {
        assert!(id > ReservedGlobalIDLowerBound);
        assert!(id <= ReservedGlobalIDUpperBound);
    }
    // 两两互异，对应 Go 中不同系统表落在不同 region 的前提。
    for (i, left) in ids.iter().enumerate() {
        for right in &ids[i + 1..] {
            assert_ne!(left, right);
        }
    }
}

/// 校验 IsSystemRelatedDB 仅识别系统库，不误判业务库。
/// 对应 TestNextgenBootstrap：仅 mysql 等系统库被识别为系统相关 schema。
// 对应 TestNextgenBootstrap 中系统库判定的前置条件：mysql/sys 等系统库应被识别，
// 普通业务库不应被误判为系统库。
#[test]
fn is_system_related_db_recognizes_system_schemas_only() {
    assert!(IsSystemRelatedDB("mysql"));
    assert!(!IsSystemRelatedDB("test"));
    assert!(!IsSystemRelatedDB("my_app_database"));
}

/// Go TestNextgenBootstrap skips classic builds and, on nextgen, validates the
/// bootstrapped InfoSchema rather than the constants in isolation.
#[test]
fn nextgen_bootstrap_uses_reserved_ids_for_every_base_system_table() {
    if astersql_config_kerneltype::IsClassic() {
        return;
    }

    let (domain, _session) = CreateAnalyzeSession().expect("bootstrap canonical domain");
    let info_schema = domain.info_schema();
    let mut reserved_schema_count = 0;
    let mut reserved_table_count = 0;
    for schema in info_schema.AllSchemas() {
        if !IsSystemRelatedDB(&schema.name.lower) {
            continue;
        }
        reserved_schema_count += 1;
        assert!(
            schema.id > ReservedGlobalIDLowerBound,
            "{} has non-reserved ID {}",
            schema.name.lower,
            schema.id
        );
        assert!(
            schema.id <= ReservedGlobalIDUpperBound,
            "{} has out-of-range ID {}",
            schema.name.lower,
            schema.id
        );
        for table in info_schema
            .SchemaTableInfos(&schema.name)
            .expect("list system tables")
        {
            // AsterSQL keeps this compatibility table in `sys`; Go has no
            // corresponding bootstrap table, so exclude it from Go's 60.
            if schema.name.lower == "sys" && table.name.lower == "sys_config" {
                continue;
            }
            if table.is_view || table.is_sequence {
                continue;
            }
            reserved_table_count += 1;
            assert!(
                table.id > ReservedGlobalIDLowerBound,
                "{}",
                table.name.lower
            );
            assert!(
                table.id <= ReservedGlobalIDUpperBound,
                "{}",
                table.name.lower
            );
        }
    }
    assert_eq!(reserved_schema_count, 2);
    assert_eq!(reserved_table_count, 60);
}

/// 校验 DDL 系统表在 mysql/test 两个库中的元信息一致。
/// 对应 TestInitMetaTable：改写 Go 侧系统表 SQL 到 test 库后，抹平 ID、UpdateTS、DBID，
/// 再对两个 TableInfo 做完整序列化比较，而不是只检查一两列。
#[test]
fn init_meta_tables_preserves_table_metadata_across_databases() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);

    let tables = [
        ("tidb_ddl_job", CreateTiDBDDLJobTable),
        ("tidb_ddl_reorg", CreateTiDBReorgTable),
        ("tidb_ddl_history", CreateTiDBDDLHistoryTable),
        ("tidb_background_subtask", CreateTiDBBackgroundSubtaskTable),
        (
            "tidb_background_subtask_history",
            CreateTiDBBackgroundSubtaskHistoryTable,
        ),
    ];
    // The canonical Rust bootstrap may already have created a source table;
    // only backfill definitions absent from that bootstrap.
    testkit.MustExec("use mysql", Vec::new());
    for (name, create_sql) in tables {
        if domain.table_by_name("mysql", name).is_err() {
            testkit.MustExec(create_sql, Vec::new());
        }
    }

    testkit.MustExec("use test", Vec::new());
    for (_, create_sql) in tables {
        testkit.MustExec(&create_sql.replacen("mysql.", "", 1), Vec::new());
    }

    for (name, _) in tables {
        let mysql_table = domain
            .table_by_name("mysql", name)
            .unwrap_or_else(|error| panic!("lookup mysql.{name}: {error}"));
        let test_table = domain
            .table_by_name("test", name)
            .unwrap_or_else(|error| panic!("lookup test.{name}: {error}"));
        assert!(mysql_table.ID > 0);
        assert!(mysql_table.UpdateTS > 0);

        let mut normalized_mysql = mysql_table.as_ref().clone();
        let mut normalized_test = test_table.as_ref().clone();
        normalized_test.ID = normalized_mysql.ID;
        normalized_mysql.UpdateTS = normalized_test.UpdateTS;
        normalized_test.DBID = 0;
        normalized_mysql.DBID = 0;
        assert_eq!(
            astersql_meta::json::marshal(&normalized_mysql).unwrap(),
            astersql_meta::json::marshal(&normalized_test).unwrap(),
            "metadata mismatch for {name}"
        );
    }
}

/// Runtime regression for Go TestInformationSchemaCreateTime.
/// UpdateTS must be visible through both metadata surfaces and converted using
/// the session timezone after ALTER TABLE publishes a new schema version.
#[test]
fn runtime_metadata_timestamps_follow_ddl_and_session_timezone() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table timestamp_probe (c int)", Vec::new());
    testkit.MustExec("set @@time_zone = 'Asia/Shanghai'", Vec::new());
    let first = testkit
        .MustQuery(
            "select create_time from information_schema.tables where table_name='timestamp_probe'",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();

    // Metadata timestamps are rendered with one-second precision. Wait only for
    // the first representable instant after CREATE instead of an unconditional
    // 1.1 seconds, while keeping the real DDL clock and publication path.
    let create_millis = domain
        .table_by_name("test", "timestamp_probe")
        .expect("timestamp_probe metadata")
        .UpdateTS
        >> 18;
    let next_display_second = (create_millis / 1_000 + 1) * 1_000;
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    std::thread::sleep(std::time::Duration::from_millis(
        next_display_second.saturating_sub(now_millis) + 5,
    ));
    testkit.MustExec(
        "alter table timestamp_probe modify c int default 11",
        Vec::new(),
    );
    let second = testkit
        .MustQuery(
            "select create_time from information_schema.tables where table_name='timestamp_probe'",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    let status = testkit
        .MustQuery("show table status like 'timestamp_probe'", Vec::new())
        .Rows();
    assert_eq!(status[0][11], second);
    assert!(second > first, "ALTER TABLE must advance create_time");

    testkit.MustExec("set @@time_zone = 'Europe/Amsterdam'", Vec::new());
    let amsterdam = testkit
        .MustQuery(
            "select create_time from information_schema.tables where table_name='timestamp_probe'",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    let amsterdam_status = testkit
        .MustQuery("show table status like 'timestamp_probe'", Vec::new())
        .Rows();
    assert_eq!(amsterdam_status[0][11], amsterdam);
    assert!(
        second > amsterdam,
        "timezone conversion must change wall clock"
    );
}

/// Runtime regression for Go TestMetaTableRegion: the first region boundary
/// uses the canonical table-prefix form `t_<table_id>_`.
#[test]
fn runtime_meta_table_regions_use_canonical_start_keys() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use mysql", Vec::new());
    let tables = [
        ("tidb_ddl_reorg", CreateTiDBReorgTable, TiDBDDLReorgTableID),
        ("tidb_ddl_job", CreateTiDBDDLJobTable, TiDBDDLJobTableID),
        (
            "tidb_background_subtask",
            CreateTiDBBackgroundSubtaskTable,
            TiDBBackgroundSubtaskTableID,
        ),
        (
            "tidb_background_subtask_history",
            CreateTiDBBackgroundSubtaskHistoryTable,
            TiDBBackgroundSubtaskHistoryTableID,
        ),
    ];
    for (name, create_sql, _) in tables {
        if domain.table_by_name("mysql", name).is_err() {
            testkit.MustExec(create_sql, Vec::new());
        }
    }

    let mut region_ids = std::collections::BTreeMap::new();
    for (table, _, expected_table_id) in tables {
        let rows = testkit
            .MustQuery(&format!("show table mysql.{table} regions"), Vec::new())
            .Rows();
        assert!(!rows.is_empty(), "{table} must expose at least one region");
        assert_eq!(
            rows[0][1],
            format!(
                "{}_{expected_table_id}_",
                String::from_utf8_lossy(tablecodec::TablePrefix()),
            )
        );
        region_ids.insert(table, rows[0][0].clone());
    }
    assert_ne!(region_ids["tidb_ddl_job"], region_ids["tidb_ddl_reorg"]);
    assert_ne!(
        region_ids["tidb_background_subtask"],
        region_ids["tidb_background_subtask_history"]
    );
}

/// Runtime regression for Go TestRecordTTLRows: a successful INSERT into a
/// TTL table records its inserted row count in the package counter.
#[test]
fn runtime_ttl_insert_rows_counter_tracks_inserted_rows() {
    unsafe {
        astersql_metrics::metrics::InitMetrics().expect("initialize package metrics");
    }
    let read_counter = || unsafe {
        let counter = std::ptr::read(std::ptr::addr_of!(
            astersql_metrics::ttl::TTLInsertRowsCount
        ));
        let value = counter.as_ref().map_or(0.0, |counter| counter.get());
        std::mem::forget(counter);
        value
    };

    let before = read_counter();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table ttl_probe (created_at datetime) TTL = created_at + INTERVAL 1 DAY",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    assert_eq!(read_counter(), before + 1.0);

    testkit.MustExec("begin", Vec::new());
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec("commit", Vec::new());
    assert_eq!(read_counter(), before + 2.0);

    testkit.MustExec("begin", Vec::new());
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec("commit", Vec::new());
    assert_eq!(read_counter(), before + 4.0);

    testkit.MustExec("begin", Vec::new());
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec("rollback", Vec::new());
    assert_eq!(read_counter(), before + 6.0);

    testkit.MustExec("begin", Vec::new());
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec("savepoint insert1", Vec::new());
    testkit.MustExec(
        "insert into ttl_probe values ('2026-08-10 00:00:00')",
        Vec::new(),
    );
    testkit.MustExec("rollback to insert1", Vec::new());
    testkit.MustExec("commit", Vec::new());
    assert_eq!(read_counter(), before + 7.0);
}
