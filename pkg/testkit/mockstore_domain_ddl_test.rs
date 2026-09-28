// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// MockStore + Domain 上的 DDL 与 InfoSchema 集成测试。
//
// 验证跨会话元数据可见性、TRUNCATE/分区 DDL 的物理 ID 变更、
// EXCHANGE PARTITION、元数据编解码保真、Domain 重启水合以及
// DDL 发布失败后的恢复等路径。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use astersql_domain::{Domain, DomainConfig, KvInfoSchemaLoader};
use astersql_meta_model::{
    ColumnInfo, DBInfo, DecodeDBInfo, EncodeDBInfo, FKInfo, IndexColumn, IndexInfo,
    PartitionDefinition, PartitionInfo, PolicyRefInfo, StatePublic, TableInfo,
};
use astersql_parser_ast as ast;

use crate::mockstore::{CreateCrossKeyspaceTestCluster, CreateMockStoreAndDomain};
use crate::{DbValue, TestKit};

/// 按库 `test` 与表名从 Domain InfoSchema 取表元数据。
fn table_by_name(domain: &Domain, table: &str) -> Arc<TableInfo> {
    domain
        .table_by_name("test", table)
        .unwrap_or_else(|error| panic!("typed InfoSchema lookup for test.{table}: {error}"))
}

/// 从分区表元数据中按分区名取物理分区 ID。
fn partition_id(table: &TableInfo, name: &str) -> i64 {
    table
        .GetPartitionInfo()
        .and_then(|partition| {
            partition
                .Definitions
                .iter()
                .find(|definition| definition.Name.L == name)
        })
        .unwrap_or_else(|| panic!("partition {name} in {}", table.Name.O))
        .ID
}

/// DDL 经版本化 InfoSchema 跨会话可见；DROP 后查找失败。
#[test]
fn ddl_is_visible_across_sessions_through_versioned_infoschema() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut writer = TestKit::new(store.clone());
    let mut reader = TestKit::new(store);
    let initial_version = domain.info_schema().SchemaMetaVersion();

    // 写会话建表后，读会话应看到新版本 InfoSchema。
    writer.MustExec("create table shared_ddl(a int)", Vec::new());

    let created = table_by_name(&domain, "shared_ddl");
    assert!(created.ID > 0);
    assert!(domain.info_schema().SchemaMetaVersion() > initial_version);
    reader.MustExec("analyze table shared_ddl", Vec::new());

    writer.MustExec("drop table shared_ddl", Vec::new());
    assert!(domain.table_by_name("test", "shared_ddl").is_err());
}

/// 跨 keyspace 工厂必须创建彼此隔离的真实 Store/Domain，而不是共享一份
/// 表元数据的命名视图；SYSTEM 缺省运行时也应稳定存在。
#[test]
fn cross_keyspace_cluster_uses_isolated_real_stores() {
    let cluster = CreateCrossKeyspaceTestCluster(&[("keyspace2", true), ("keyspace1", false)]);
    assert_eq!(cluster.keyspaces(), ["SYSTEM", "keyspace1", "keyspace2"]);

    let system_store = cluster.store("SYSTEM");
    let first_store = cluster.store("keyspace1");
    let second_store = cluster.store("keyspace2");
    assert!(!Arc::ptr_eq(&system_store, &first_store));
    assert!(!Arc::ptr_eq(&first_store, &second_store));

    let mut first = TestKit::new(first_store.clone());
    first.MustExec("create database isolated", Vec::new());
    first.MustExec("use isolated", Vec::new());
    first.MustExec("create table only_in_first(a int)", Vec::new());
    first.MustExec("insert into only_in_first values (1)", Vec::new());
    first
        .MustQuery("select count(*) from only_in_first", Vec::new())
        .Check(crate::Rows(&["1"]));

    assert!(
        second_store
            .domain()
            .stats_table("isolated", "only_in_first")
            .is_none()
    );
    assert!(
        system_store
            .domain()
            .stats_table("isolated", "only_in_first")
            .is_none()
    );

    let mut second = TestKit::new(second_store);
    second.MustExec("create database isolated", Vec::new());
    second.MustExec("use isolated", Vec::new());
    second.MustExec("create table only_in_second(a int)", Vec::new());
    assert!(
        first_store
            .domain()
            .stats_table("isolated", "only_in_second")
            .is_none()
    );
}

/// TRUNCATE 分配新表 ID，旧统计可被 GC 清理。
#[test]
fn truncate_allocates_new_table_identity_and_old_stats_are_gc_eligible() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table truncate_ddl(a int, index ia(a))", Vec::new());
    testkit.MustExec(
        "insert into truncate_ddl values (?), (?)",
        vec![DbValue::I64(1), DbValue::I64(2)],
    );
    testkit.MustExec("analyze table truncate_ddl", Vec::new());
    let old_id = table_by_name(&domain, "truncate_ddl").ID;

    testkit.MustExec("truncate table truncate_ddl", Vec::new());

    let new_id = table_by_name(&domain, "truncate_ddl").ID;
    assert_ne!(new_id, old_id);
    assert!(
        domain
            .persisted_stats_meta_rows()
            .expect("read stats metadata")
            .iter()
            .any(|row| row.0 == old_id)
    );
    domain.gc_stats(Duration::ZERO).expect("first old table GC");
    domain
        .gc_stats(Duration::ZERO)
        .expect("second old table GC");
    assert!(
        domain
            .persisted_stats_meta_rows()
            .expect("read stats metadata after GC")
            .iter()
            .all(|row| row.0 != old_id)
    );
}

/// 分区 ADD/TRUNCATE/DROP/REORGANIZE 仅替换目标物理身份。
#[test]
fn partition_ddl_replaces_only_target_physical_identities() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table partition_ddl(a int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    let initial = table_by_name(&domain, "partition_ddl");
    let table_id = initial.ID;
    let p0 = partition_id(&initial, "p0");
    let p1 = partition_id(&initial, "p1");

    testkit.MustExec(
        "alter table partition_ddl add partition (partition p2 values less than (30))",
        Vec::new(),
    );
    let added = table_by_name(&domain, "partition_ddl");
    assert_eq!(added.ID, table_id);
    assert_eq!(partition_id(&added, "p0"), p0);
    assert_eq!(partition_id(&added, "p1"), p1);
    let p2 = partition_id(&added, "p2");
    assert!(![table_id, p0, p1].contains(&p2));

    testkit.MustExec(
        "alter table partition_ddl truncate partition p0",
        Vec::new(),
    );
    let truncated = table_by_name(&domain, "partition_ddl");
    let new_p0 = partition_id(&truncated, "p0");
    assert_ne!(new_p0, p0);
    assert_eq!(partition_id(&truncated, "p1"), p1);
    assert_eq!(partition_id(&truncated, "p2"), p2);

    testkit.MustExec("alter table partition_ddl drop partition p1", Vec::new());
    let dropped = table_by_name(&domain, "partition_ddl");
    assert_eq!(dropped.ID, table_id);
    assert!(
        dropped
            .GetPartitionInfo()
            .expect("partition metadata")
            .Definitions
            .iter()
            .all(|definition| definition.Name.L != "p1")
    );

    testkit.MustExec(
        "alter table partition_ddl reorganize partition p0, p2 into \
         (partition pn values less than (30))",
        Vec::new(),
    );
    let reorganized = table_by_name(&domain, "partition_ddl");
    let pn = partition_id(&reorganized, "pn");
    assert_eq!(reorganized.ID, table_id);
    assert!(![new_p0, p2].contains(&pn));
}

/// EXCHANGE PARTITION 交换元数据身份但不改写 stats lock 行。
#[test]
fn exchange_partition_swaps_metadata_identity_without_rewriting_locks() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table exchange_p(a int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    testkit.MustExec("create table exchange_n(a int)", Vec::new());
    testkit.MustExec("lock stats exchange_p partition p0", Vec::new());
    testkit.MustExec("lock stats exchange_n", Vec::new());
    let partitioned = table_by_name(&domain, "exchange_p");
    let normal = table_by_name(&domain, "exchange_n");
    let old_partition_id = partition_id(&partitioned, "p0");
    let old_normal_id = normal.ID;
    let locked_before = domain
        .stats_locked_rows()
        .expect("locked rows before exchange");

    testkit.MustExec(
        "alter table exchange_p exchange partition p0 with table exchange_n",
        Vec::new(),
    );

    let partitioned = table_by_name(&domain, "exchange_p");
    let normal = table_by_name(&domain, "exchange_n");
    assert_eq!(partition_id(&partitioned, "p0"), old_normal_id);
    assert_eq!(normal.ID, old_partition_id);
    assert_eq!(
        domain
            .stats_locked_rows()
            .expect("locked rows after exchange"),
        locked_before
    );
}

/// 规范元数据编解码往返保留 handle/索引/分区/外键字段。
#[test]
fn canonical_metadata_round_trip_preserves_handle_index_partition_and_fk_fields() {
    let (_store, domain) = CreateMockStoreAndDomain();
    let database = DBInfo {
        ID: 7,
        Name: ast::NewCIStr("test"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        State: StatePublic,
        PlacementPolicyRef: Some(PolicyRefInfo {
            ID: 8,
            Name: ast::NewCIStr("database_policy"),
        }),
        TableName2ID: HashMap::from([("fidelity".to_owned(), 9)]),
        ..DBInfo::default()
    };
    let restored_database =
        DecodeDBInfo(&EncodeDBInfo(&database).expect("encode DBInfo")).expect("decode DBInfo");
    assert_eq!(restored_database.ID, 7);
    assert_eq!(restored_database.Name.L, "test");
    assert_eq!(restored_database.Charset, "utf8mb4");
    assert_eq!(
        restored_database
            .PlacementPolicyRef
            .expect("database policy")
            .Name
            .L,
        "database_policy"
    );
    // Go marks DBInfo.TableName2ID with `json:"-"`: it is a runtime lookup
    // cache rebuilt from table metadata, not part of the persisted DBInfo.
    assert!(restored_database.TableName2ID.is_empty());

    let mut first_column = ColumnInfo::New(11, ast::NewCIStr("a"));
    first_column.State = StatePublic;
    first_column.SetType(8);
    first_column.SetFlag(2);
    first_column.Comment = "primary handle".to_owned();
    let mut second_column = ColumnInfo::New(12, ast::NewCIStr("b"));
    second_column.State = StatePublic;
    second_column.SetType(3);
    second_column.GeneratedExprString = "`a` + 1".to_owned();
    second_column.GeneratedStored = true;
    let table = TableInfo {
        Name: ast::NewCIStr("fidelity"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        Columns: vec![first_column, second_column],
        Indices: vec![IndexInfo {
            ID: 21,
            Name: ast::NewCIStr("PRIMARY"),
            Table: ast::NewCIStr("fidelity"),
            Columns: vec![
                IndexColumn {
                    Name: ast::NewCIStr("a"),
                    Offset: 0,
                    Length: -1,
                    ..IndexColumn::default()
                },
                IndexColumn {
                    Name: ast::NewCIStr("b"),
                    Offset: 1,
                    Length: -1,
                    ..IndexColumn::default()
                },
            ],
            State: StatePublic,
            Unique: true,
            Primary: true,
            Global: true,
            Comment: "clustered primary".to_owned(),
            ..IndexInfo::default()
        }],
        ForeignKeys: vec![FKInfo {
            ID: 31,
            Name: ast::NewCIStr("fk_parent"),
            RefSchema: ast::NewCIStr("test"),
            RefTable: ast::NewCIStr("parent"),
            RefCols: vec![ast::NewCIStr("id")],
            Cols: vec![ast::NewCIStr("b")],
            OnDelete: 2,
            OnUpdate: 4,
            State: StatePublic,
            Version: 1,
        }],
        State: StatePublic,
        PKIsHandle: true,
        IsCommonHandle: true,
        CommonHandleVersion: 1,
        Comment: "full fidelity".to_owned(),
        MaxColumnID: 12,
        MaxIndexID: 21,
        MaxForeignKeyID: 31,
        Partition: Some(PartitionInfo {
            Type: astersql_meta_model::ast::model::PartitionTypeRange,
            Expr: "`a`".to_owned(),
            Enable: true,
            Definitions: vec![PartitionDefinition {
                Name: ast::NewCIStr("p0"),
                LessThan: vec!["100".to_owned()],
                Comment: "first partition".to_owned(),
                ..PartitionDefinition::default()
            }],
            Num: 1,
            ..PartitionInfo::default()
        }),
        ..TableInfo::default()
    };

    domain
        .ddl_create_table("test", table, false)
        .expect("persist full fidelity table");

    let restored = table_by_name(&domain, "fidelity");
    assert!(restored.PKIsHandle);
    assert!(restored.IsCommonHandle);
    assert_eq!(restored.CommonHandleVersion, 1);
    assert_eq!(restored.Comment, "full fidelity");
    assert_eq!(restored.Columns[0].Comment, "primary handle");
    assert_eq!(restored.Columns[1].GeneratedExprString, "`a` + 1");
    assert!(restored.Columns[1].GeneratedStored);
    assert_eq!(restored.Indices[0].Comment, "clustered primary");
    assert!(restored.Indices[0].Global);
    assert_eq!(restored.ForeignKeys[0].Name.L, "fk_parent");
    assert_eq!(restored.ForeignKeys[0].OnDelete, 2);
    assert_eq!(
        restored.GetPartitionInfo().unwrap().Definitions[0].Comment,
        "first partition"
    );
}

/// Domain 从 KV 重启后水合 InfoSchema、统计目录与已分析 handle。
#[test]
fn domain_restart_hydrates_infoschema_catalog_and_analyzed_handle() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table restart_ddl(a int primary key)", Vec::new());
    testkit.MustExec("insert into restart_ddl values (1), (2)", Vec::new());
    testkit.MustExec("analyze table restart_ddl", Vec::new());
    let table_id = table_by_name(&domain, "restart_ddl").ID;

    let restarted = Domain::new_with_storage_handle(
        domain.storage(),
        Arc::new(KvInfoSchemaLoader::new()),
        DomainConfig {
            schema_lease: Duration::ZERO,
            stats_lease: Duration::ZERO,
            ..DomainConfig::default()
        },
    );
    restarted.init().expect("restart domain from persisted KV");
    restarted
        .init_stats_lite(&[])
        .expect("reload persisted statistics after restart");

    assert_eq!(table_by_name(&restarted, "restart_ddl").ID, table_id);
    assert_eq!(
        restarted
            .stats_table("test", "restart_ddl")
            .expect("restarted statistics catalog")
            .0
            .table_id,
        table_id
    );
    let stats = restarted
        .stats_handle()
        .lock()
        .expect("restarted statistics handle")
        .stats_meta(table_id)
        .cloned()
        .expect("restarted table statistics");
    assert!(!stats.pseudo);
    assert_eq!(stats.realtime_count, 2);
    assert_eq!(stats.columns.len(), 1);
}

/// AutoID 的全局高水位必须随 KV 一起存活，Domain 重启后不能重复发放已有行 ID。
#[test]
fn domain_restart_preserves_auto_increment_high_water() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table restart_auto_id(id bigint primary key auto_increment, value int)",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into restart_auto_id(value) values (10), (20)",
        Vec::new(),
    );
    let table_id = table_by_name(&domain, "restart_auto_id").ID;

    let restarted = Domain::new_with_storage_handle(
        domain.storage(),
        Arc::new(KvInfoSchemaLoader::new()),
        DomainConfig {
            schema_lease: Duration::ZERO,
            stats_lease: Duration::ZERO,
            ..DomainConfig::default()
        },
    );
    restarted.init().expect("restart domain from persisted KV");

    let (allocated, _) = restarted
        .allocate_stats_auto_id_with_increment(table_id, None, 0, 1, 1)
        .expect("allocate auto increment ID after restart");
    assert!(
        allocated > 2,
        "restarted Domain reused existing auto increment ID {allocated}"
    );
}

/// 已提交 DDL 在运行时发布失败后可从规范元数据恢复。
#[test]
fn committed_ddl_recovers_runtime_publication_failure_from_canonical_metadata() {
    let (_store, domain) = CreateMockStoreAndDomain();
    // 注入下一次 DDL 发布失败，验证提交后仍可对账恢复。
    domain.fail_next_ddl_publication_for_test();

    domain
        .ddl_create_table(
            "test",
            TableInfo {
                Name: ast::NewCIStr("publication_recovery"),
                Columns: vec![ColumnInfo::New(1, ast::NewCIStr("a"))],
                State: StatePublic,
                ..TableInfo::default()
            },
            false,
        )
        .expect("committed DDL is reconciled after publication failure");

    let table = table_by_name(&domain, "publication_recovery");
    assert_eq!(
        domain
            .stats_table("test", "publication_recovery")
            .expect("reconciled statistics catalog")
            .0
            .table_id,
        table.ID
    );
    assert!(
        domain
            .stats_handle()
            .lock()
            .expect("statistics handle")
            .stats_meta(table.ID)
            .is_some()
    );
}

/// IF NOT EXISTS / IF EXISTS 在元数据上为显式空操作。
#[test]
fn create_and_drop_existence_options_are_explicit_metadata_noops() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table existence_ddl(a int)", Vec::new());
    let table_id = table_by_name(&domain, "existence_ddl").ID;
    let version = domain.info_schema().SchemaMetaVersion();
    let stats_rows = domain.persisted_stats_meta_rows().unwrap().len();

    testkit.MustExec(
        "create table if not exists existence_ddl(a int)",
        Vec::new(),
    );
    assert_eq!(domain.info_schema().SchemaMetaVersion(), version);
    assert_eq!(table_by_name(&domain, "existence_ddl").ID, table_id);
    assert_eq!(
        domain.persisted_stats_meta_rows().unwrap().len(),
        stats_rows
    );

    let error = testkit.QueryToErr("drop table missing_ddl");
    assert!(
        error
            .message()
            .to_ascii_lowercase()
            .contains("unknown table")
    );
    assert_eq!(domain.info_schema().SchemaMetaVersion(), version);

    testkit.MustExec("drop table if exists missing_ddl", Vec::new());
    assert_eq!(domain.info_schema().SchemaMetaVersion(), version);
}

/// DROP COLUMN/INDEX 同步更新 InfoSchema 与统计直方图。
#[test]
fn drop_column_and_index_update_infoschema_and_statistics_together() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table item_ddl(a int, b int, index ia(a), index ib(b))",
        Vec::new(),
    );
    testkit.MustExec("analyze table item_ddl", Vec::new());
    let initial = table_by_name(&domain, "item_ddl");
    let table_id = initial.ID;
    let dropped_column_id = initial
        .Columns
        .iter()
        .find(|column| column.Name.L == "a")
        .unwrap()
        .ID;
    let dropped_index_id = initial
        .Indices
        .iter()
        .find(|index| index.Name.L == "ia")
        .unwrap()
        .ID;

    testkit.MustExec(
        "alter table item_ddl drop index ia, drop column a",
        Vec::new(),
    );

    let restored = table_by_name(&domain, "item_ddl");
    assert_eq!(restored.ID, table_id);
    assert!(restored.Columns.iter().all(|column| column.Name.L != "a"));
    assert!(restored.Indices.iter().all(|index| index.Name.L != "ia"));
    assert!(restored.Indices.iter().any(|index| index.Name.L == "ib"));
    let stats = domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(table_id)
        .cloned()
        .expect("table statistics");
    assert!(!stats.columns.contains_key(&dropped_column_id));
    assert!(!stats.indexes.contains_key(&dropped_index_id));
    assert_eq!(
        domain
            .restricted_stats_query(
                &format!(
                    "select count(*) from mysql.stats_histograms where table_id={table_id} \
                     and hist_id={dropped_index_id}"
                ),
                &[],
            )
            .unwrap(),
        vec![vec!["0".to_owned()]]
    );
}
