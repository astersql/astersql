// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// InfoSchema（信息模式：库表列元数据目录）查找与版本语义的单元测试。
//
// 对应 Go `pkg/executor/test/infoschema` 中对 schema 元版本、按名查表、
// 按表/分区 ID 反查，以及系统表可见性的断言意图。InfoSchema 是会话看到的
// 当前 schema 快照；SchemaMetaVersion 随 DDL 提交递增。

use astersql_infoschema::infoschema::{
    ColumnInfo, FindTableByTblOrPartID, ForeignKeyInfo, MockInfoSchemaWithSchemaVer,
    PartitionDefinition, PartitionInfo, TableInfo,
};
use astersql_infoschema::{CiString, InfoSchema};
use astersql_parser_auth::parser::auth::auth::UserIdentity;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, RowsWithSep, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use std::sync::{Arc, Mutex};

/// 对应 Go `TestInspectionTables`：集群组件信息必须完整保留 failpoint 注入的
/// 七类节点、地址、版本、Git 哈希与 server_id。
#[test]
fn inspection_tables_expose_all_injected_cluster_info_columns() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let tk = TestKit::new(store);
    let instances = [
        "pd,127.0.0.1:11080,127.0.0.1:10080,mock-version,mock-githash,0",
        "tidb,127.0.0.1:11080,127.0.0.1:10080,mock-version,mock-githash,1001",
        "tikv,127.0.0.1:11080,127.0.0.1:10080,mock-version,mock-githash,0",
        "tiproxy,127.0.0.1:6000,127.0.0.1:3380,mock-version,mock-githash,0",
        "ticdc,127.0.0.1:8300,127.0.0.1:8301,mock-version,mock-githash,0",
        "tso,127.0.0.1:3379,127.0.0.1:3379,mock-version,mock-githash,0",
        "scheduling,127.0.0.1:4379,127.0.0.1:4379,mock-version,mock-githash,0",
    ];
    let expression = format!("return(\"{}\")", instances.join(";"));
    let _guard = testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/infoschema/mockClusterInfo",
        &expression,
    );
    let expected = [
        "pd 127.0.0.1:11080 127.0.0.1:10080 mock-version mock-githash 0",
        "tidb 127.0.0.1:11080 127.0.0.1:10080 mock-version mock-githash 1001",
        "tikv 127.0.0.1:11080 127.0.0.1:10080 mock-version mock-githash 0",
        "tiproxy 127.0.0.1:6000 127.0.0.1:3380 mock-version mock-githash 0",
        "ticdc 127.0.0.1:8300 127.0.0.1:8301 mock-version mock-githash 0",
        "tso 127.0.0.1:3379 127.0.0.1:3379 mock-version mock-githash 0",
        "scheduling 127.0.0.1:4379 127.0.0.1:4379 mock-version mock-githash 0",
    ];

    tk.MustQuery(
        "select type, instance, status_address, version, git_hash, server_id \
         from information_schema.cluster_info",
        Vec::new(),
    )
    .Check(Rows(&expected));

    let session = tk.Session();
    session
        .SetInspectionTableCacheEnabledForTest(true)
        .expect("enable inspection table cache");
    tk.MustQuery(
        "select type, instance, status_address, version, git_hash, server_id \
         from information_schema.cluster_info",
        Vec::new(),
    )
    .Check(Rows(&expected));
    assert_eq!(
        session
            .InspectionTableCacheRowCountForTest("cluster_info")
            .expect("read inspection table cache"),
        Some(7)
    );

    session
        .SetInspectionTableCacheValueForTest("cluster_info", 0, "type", "modified-pd")
        .expect("mutate cached cluster_info row");
    let mut cached_expected = expected;
    cached_expected[0] = "modified-pd 127.0.0.1:11080 127.0.0.1:10080 mock-version mock-githash 0";
    tk.MustQuery(
        "select type, instance, status_address, version, git_hash, server_id \
         from information_schema.cluster_info",
        Vec::new(),
    )
    .Check(Rows(&cached_expected));
    session
        .SetInspectionTableCacheEnabledForTest(false)
        .expect("disable inspection table cache");
}

/// 对应 Go `TestInfoSchemaExcludeNonPublicColumns`：修改列的 reorg 边界上，
/// INFORMATION_SCHEMA.COLUMNS 只能看到原有 public 列，不能泄露临时列。
#[test]
fn columns_exclude_non_public_modify_column_artifacts() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_state_test (a bigint, b bigint, c bigint)",
        Vec::new(),
    );

    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let callback_rows = Arc::clone(&observed);
    let callback_store = store.clone();
    let _guard = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/afterReorgWorkForModifyColumn",
        move || {
            let tk = TestKit::new(callback_store.clone());
            let rows = tk
                .MustQuery(
                    "select column_name from information_schema.columns \
                     where table_schema='test' and table_name='t_state_test' \
                     order by column_name",
                    Vec::new(),
                )
                .Rows();
            *callback_rows.lock().expect("column observations lock") =
                rows.into_iter().map(|row| row[0].clone()).collect();
        },
    );

    tk.MustExec("alter table t_state_test modify column a int", Vec::new());
    assert_eq!(
        *observed.lock().expect("column observations lock"),
        ["a", "b", "c"]
    );
}

/// 对应 Go `TestStatisticShowPublicIndexes`：ADD INDEX 发布前的 DDL 边界，
/// INFORMATION_SCHEMA.STATISTICS 不得出现尚未 public 的索引。
#[test]
fn statistics_only_exposes_public_indexes() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table public_index_t (a int, b int)", Vec::new());

    let observed = Arc::new(Mutex::new(None::<String>));
    let callback_count = Arc::clone(&observed);
    let callback_store = store.clone();
    let _guard = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced",
        move || {
            let tk = TestKit::new(callback_store.clone());
            let rows = tk
                .MustQuery(
                    "select count(1) from information_schema.statistics \
                     where table_schema='test' and table_name='public_index_t' \
                     and index_name='idx'",
                    Vec::new(),
                )
                .Rows();
            *callback_count.lock().expect("index observations lock") = Some(rows[0][0].clone());
        },
    );

    tk.MustExec("alter table public_index_t add index idx(b)", Vec::new());
    assert_eq!(
        observed.lock().expect("index observations lock").as_deref(),
        Some("0")
    );
    tk.MustQuery(
        "select count(1) from information_schema.statistics \
         where table_schema='test' and table_name='public_index_t' and index_name='idx'",
        Vec::new(),
    )
    .Check(Rows(&["1"]));
}

/// 对应 Go `TestKeyspaceMeta`：classic 构建跳过；NextGen 构建完整返回
/// 当前 keyspace 的名称、ID 与可反序列化配置 JSON。
#[test]
fn keyspace_meta_matches_current_nextgen_store_metadata() {
    if astersql_config_kerneltype::IsClassic() {
        return;
    }
    let _guard = testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/infoschema/mockKeyspaceMeta",
        "return(\"SYSTEM|16777214|key_a=a;key_b=b\")",
    );
    let (store, _domain) = CreateMockStoreAndDomain();
    let tk = TestKit::new(store);
    tk.MustQuery("select * from information_schema.keyspace_meta", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["SYSTEM|16777214|{\"key_a\":\"a\",\"key_b\":\"b\"}"],
        ));
}

/// 对应 Go `TestForServersInfo`：虚拟表列顺序和唯一行均来自当前
/// InfoSyncer，并保留全局配置中的实例 labels。
#[test]
fn tidb_servers_info_matches_current_infosync_server() {
    struct ConfigRestore(astersql_config::Config);
    impl Drop for ConfigRestore {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
        }
    }

    let original = astersql_config::get_global_config().as_ref().clone();
    let _restore = ConfigRestore(original.clone());
    let mut configured = original;
    configured.labels = [("dc".to_owned(), "dc1".to_owned())].into();
    astersql_config::store_global_config(configured);

    astersql_domain_infosync::GlobalInfoSyncerInit(
        "infoschema-server".to_owned(),
        Arc::new(|| 1001),
        None,
        None,
        None,
        astersql_domain_infosync::Codec::default(),
        false,
        None,
    )
    .expect("initialize infoschema test InfoSyncer");

    let (store, _domain) = CreateMockStoreAndDomain();
    let tk = TestKit::new(store);
    let info = astersql_domain_infosync::GetServerInfo().expect("current server info");
    assert_eq!(info.Labels.get("dc").map(String::as_str), Some("dc1"));
    tk.MustQuery(
        "select ddl_id, ip, port, status_port, lease, version, git_hash, labels \
         from information_schema.tidb_servers_info",
        Vec::new(),
    )
    .Check(RowsWithSep(
        "|",
        &[&format!(
            "{}|{}|{}|{}|{}|{}|{}|dc=dc1",
            info.ID, info.IP, info.Port, info.StatusPort, info.Lease, info.Version, info.GitHash
        )],
    ));
}

/// 对应 Go `TestIndexUsageWithData`：普通索引、整数主键、聚簇主键、
/// 字符串聚簇主键与非聚簇主键均累计查询次数、访问比例桶和最后访问时间。
#[test]
fn index_usage_aggregates_real_scan_data_for_all_index_kinds() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    let values = (1..=500)
        .map(|value| format!("({value})"))
        .collect::<Vec<_>>()
        .join(",");
    let cases = [
        ("a int, index idx(a)", "idx", false),
        ("a int primary key", "primary", false),
        ("a bigint primary key clustered", "primary", false),
        ("a varchar(16) primary key clustered", "primary", true),
        ("a int primary key nonclustered", "primary", false),
    ];

    for (definition, index_name, string_primary) in cases {
        tk.MustExec("drop table if exists index_usage_data_t", Vec::new());
        tk.MustExec(
            &format!("create table index_usage_data_t ({definition})"),
            Vec::new(),
        );
        tk.MustQuery(
            "select * from information_schema.tidb_index_usage \
             where table_schema='test' and table_name='index_usage_data_t'",
            Vec::new(),
        )
        .Check(Rows(&[&format!(
            "test index_usage_data_t {index_name} 0 0 0 0 0 0 0 0 0 0 <nil>"
        )]));

        tk.MustExec(
            &format!("insert into index_usage_data_t values {values}"),
            Vec::new(),
        );
        tk.MustExec("analyze table index_usage_data_t all columns", Vec::new());
        let full_sql = if string_primary {
            "select * from index_usage_data_t order by a".to_owned()
        } else {
            format!("select * from index_usage_data_t use index({index_name}) order by a")
        };
        assert_eq!(tk.MustQuery(&full_sql, Vec::new()).Rows().len(), 500);
        let partial_sql = if string_primary {
            "select * from index_usage_data_t where a < '3'".to_owned()
        } else {
            format!(
                "select * from index_usage_data_t use index({index_name}) \
                 where a <= 250 order by a"
            )
        };
        assert_eq!(
            tk.MustQuery(&partial_sql, Vec::new()).Rows().len(),
            if string_primary { 222 } else { 250 }
        );
        tk.Session()
            .ReportUsageStats()
            .expect("report index usage stats");
        let usage = tk
            .MustQuery(
                "select query_total, percentage_access_20_50, percentage_access_100, \
                 last_access_time from information_schema.tidb_index_usage \
                 where table_schema='test' and table_name='index_usage_data_t'",
                Vec::new(),
            )
            .Rows();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0][0], "2");
        assert_eq!(usage[0][1], if string_primary { "1" } else { "0" });
        assert_eq!(usage[0][2], "1");
        assert_ne!(usage[0][3], "<nil>");
    }
}

/// 对应 Go `TestUserPrivileges`：未授权用户看不到底层 mysql 元数据；
/// 激活表级或库级角色后，仅显示该角色可访问的表。
#[test]
fn information_schema_rows_follow_authenticated_role_privileges() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut root = TestKit::new(store.clone());
    let authenticate = |tk: &TestKit, username: &str| {
        tk.Session()
            .AuthenticateUserForTest(&UserIdentity {
                username: username.to_owned(),
                hostname: "127.0.0.1".to_owned(),
                ..Default::default()
            })
            .expect("authenticate information-schema user");
    };

    root.MustExec("create user constraints_tester", Vec::new());
    let mut constraints = TestKit::new(store.clone());
    constraints.MustExec("use information_schema", Vec::new());
    authenticate(&constraints, "constraints_tester");
    constraints
        .MustQuery(
            "select * from information_schema.table_constraints \
             where table_name != 'cluster_slow_query'",
            Vec::new(),
        )
        .Check(Rows(&[]));
    root.MustExec("create role r_gc_delete_range", Vec::new());
    root.MustExec(
        "grant all privileges on mysql.gc_delete_range to r_gc_delete_range",
        Vec::new(),
    );
    root.MustExec(
        "grant 'r_gc_delete_range' to 'constraints_tester'",
        Vec::new(),
    );
    constraints.MustExec("set role 'r_gc_delete_range'", Vec::new());
    assert!(
        !constraints
            .MustQuery(
                "select * from information_schema.table_constraints \
                 where table_name='gc_delete_range'",
                Vec::new(),
            )
            .Rows()
            .is_empty()
    );
    constraints
        .MustQuery(
            "select * from information_schema.table_constraints \
             where table_name='tables_priv'",
            Vec::new(),
        )
        .Check(Rows(&[]));

    root.MustExec("create user tester1", Vec::new());
    let mut tester1 = TestKit::new(store.clone());
    tester1.MustExec("use information_schema", Vec::new());
    authenticate(&tester1, "tester1");
    tester1
        .MustQuery(
            "select * from information_schema.statistics \
             where table_name != 'cluster_slow_query'",
            Vec::new(),
        )
        .Check(Rows(&[]));

    root.MustExec("create user tester2", Vec::new());
    root.MustExec("create role r_columns_priv", Vec::new());
    root.MustExec(
        "grant all privileges on mysql.columns_priv to r_columns_priv",
        Vec::new(),
    );
    root.MustExec("grant 'r_columns_priv' to 'tester2'", Vec::new());
    let mut tester2 = TestKit::new(store.clone());
    tester2.MustExec("use information_schema", Vec::new());
    authenticate(&tester2, "tester2");
    tester2.MustExec("set role 'r_columns_priv'", Vec::new());
    assert!(
        !tester2
            .MustQuery(
                "select * from information_schema.statistics \
                 where table_name='columns_priv' and column_name='Host'",
                Vec::new(),
            )
            .Rows()
            .is_empty()
    );
    tester2
        .MustQuery(
            "select * from information_schema.statistics \
             where table_name='tables_priv' and column_name='Host'",
            Vec::new(),
        )
        .Check(Rows(&[]));

    root.MustExec("create user tester3", Vec::new());
    root.MustExec("create role r_all_priv", Vec::new());
    root.MustExec("grant all privileges on mysql.* to r_all_priv", Vec::new());
    root.MustExec("grant 'r_all_priv' to 'tester3'", Vec::new());
    let mut tester3 = TestKit::new(store);
    tester3.MustExec("use information_schema", Vec::new());
    authenticate(&tester3, "tester3");
    tester3.MustExec("set role 'r_all_priv'", Vec::new());
    for table in ["columns_priv", "tables_priv"] {
        assert!(
            !tester3
                .MustQuery(
                    &format!(
                        "select * from information_schema.statistics \
                         where table_name='{table}' and column_name='Host'"
                    ),
                    Vec::new(),
                )
                .Rows()
                .is_empty()
        );
    }
}

/// 验证 mock InfoSchema 保留 schema 版本、分区定义、外键元数据，
/// 且可通过表名（大小写不敏感）与分区 ID 反查到同一张表。
#[test]
fn infoschema_lookup_preserves_version_partition_and_foreign_key_metadata() {
    // 构造含自增列、单分区 p0、外键 fk_customer 的 Orders 表，schema 版本 17。
    let schema = MockInfoSchemaWithSchemaVer(
        vec![TableInfo {
            id: 42,
            name: CiString::new("Orders"),
            columns: vec![ColumnInfo {
                id: 1,
                name: CiString::new("ID"),
                auto_increment: true,
            }],
            partition: Some(PartitionInfo {
                definitions: vec![PartitionDefinition {
                    id: 4201,
                    name: CiString::new("p0"),
                }],
            }),
            foreign_keys: vec![ForeignKeyInfo {
                name: CiString::new("fk_customer"),
                ref_schema: CiString::new("crm"),
                ref_table: CiString::new("customers"),
            }],
            ..TableInfo::default()
        }],
        17,
    );

    assert_eq!(schema.SchemaMetaVersion(), 17);
    // 表名按 CIStr 比较：传入小写 "orders" 应命中 "Orders"。
    let orders = schema
        .TableByName(&CiString::new("TEST"), &CiString::new("orders"))
        .unwrap();
    assert_eq!(orders.Meta().db_id, 1);
    assert!(orders.Meta().columns[0].auto_increment);
    assert_eq!(orders.Meta().foreign_keys[0].ref_table.lower, "customers");

    // 用分区物理 ID 反查：应返回逻辑表 42 与分区名 p0。
    let (partitioned_table, partition) = FindTableByTblOrPartID(schema.as_ref(), 4201);
    assert_eq!(partitioned_table.unwrap().Meta().id, 42);
    assert_eq!(partition.unwrap().name.original, "p0");

    // 系统库 mysql.stats_meta 在 mock 中固定 id=9999。
    let system = schema
        .TableByName(&CiString::new("mysql"), &CiString::new("stats_meta"))
        .unwrap();
    assert_eq!(system.Meta().id, 9999);
}

/// 对应 Go `TestPartitionsTable` 的真实 SQL 元数据断言。
#[test]
fn partitions_table_exposes_partition_names_descriptions_and_ids() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_partitions (a int, b int, primary key(a)) \
         partition by range (a) (partition p0 values less than (6), \
         partition p1 values less than (11), partition p2 values less than (16))",
        Vec::new(),
    );

    tk.MustQuery(
        "select partition_name, partition_description \
         from information_schema.partitions \
         where table_name='infoschema_partitions'",
        Vec::new(),
    )
    .Check(Rows(&["p0 6", "p1 11", "p2 16"]));

    let partition_id = tk
        .MustQuery(
            "select tidb_partition_id from information_schema.partitions \
             where table_name='infoschema_partitions' and partition_name='p1'",
            Vec::new(),
        )
        .Rows()[0][0]
        .parse::<i64>()
        .expect("partition id");
    tk.MustQuery(
        &format!(
            "select table_name, partition_name, tidb_partition_id \
             from information_schema.partitions where tidb_partition_id={partition_id}"
        ),
        Vec::new(),
    )
    .Check(Rows(&[&format!("infoschema_partitions p1 {partition_id}")]));
}

/// 对应 Go `TestTablesTable`：表名、库名和物理表 ID 条件必须共同生效。
#[test]
fn tables_table_applies_schema_name_and_id_predicates() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    for database in ["infoschema_db1", "infoschema_db2"] {
        tk.MustExec(&format!("create database {database}"), Vec::new());
        for table in ["t1", "t2"] {
            tk.MustExec(
                &format!("create table {database}.{table} (a int)"),
                Vec::new(),
            );
        }
    }

    let rows = tk
        .MustQuery(
            "select table_schema, table_name, tidb_table_id \
             from information_schema.tables \
             where table_schema='infoschema_db1' order by table_name",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "infoschema_db1");
    assert_eq!(rows[0][1], "t1");
    assert_eq!(rows[1][1], "t2");
    assert_ne!(rows[0][2], rows[1][2]);

    let t1_id = rows[0][2].clone();
    tk.MustQuery(
        &format!(
            "select table_schema, table_name, tidb_table_id \
             from information_schema.tables where table_schema='infoschema_db1' \
             and table_name='t1' and tidb_table_id={t1_id}"
        ),
        Vec::new(),
    )
    .Check(RowsWithSep("|", &[&format!("infoschema_db1|t1|{t1_id}")]));
    tk.MustQuery(
        &format!(
            "select table_schema, table_name, tidb_table_id \
             from information_schema.tables where table_schema='infoschema_db2' \
             and tidb_table_id={t1_id}"
        ),
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 对应 Go `TestColumnTable`：表与视图列均可按库、表、列名过滤。
#[test]
fn columns_table_includes_view_columns_and_filters_them() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_columns (id int primary key, value int, extra int)",
        Vec::new(),
    );
    tk.MustExec(
        "create view infoschema_view (min_id, value, max_extra) as select min(id), value, max(extra) \
         from infoschema_columns group by value",
        Vec::new(),
    );

    tk.MustQuery(
        "select table_schema, table_name, column_name \
         from information_schema.columns where table_schema='test' \
         and table_name='infoschema_columns' order by ordinal_position",
        Vec::new(),
    )
    .Check(RowsWithSep(
        "|",
        &[
            "test|infoschema_columns|id",
            "test|infoschema_columns|value",
            "test|infoschema_columns|extra",
        ],
    ));
    tk.MustQuery(
        "select table_schema, table_name, column_name \
         from information_schema.columns where table_name='infoschema_view' \
         order by ordinal_position",
        Vec::new(),
    )
    .Check(RowsWithSep(
        "|",
        &[
            "test|infoschema_view|min_id",
            "test|infoschema_view|value",
            "test|infoschema_view|max_extra",
        ],
    ));
    tk.MustQuery(
        "select count(*) from information_schema.columns \
         where table_schema='test' and column_name='value'",
        Vec::new(),
    )
    .Check(Rows(&["2"]));
}

/// 对应 Go `TestIndexUsageTable`：普通/聚簇/非聚簇主键索引出现在使用情况目录。
#[test]
fn index_usage_table_lists_supported_indexes_and_filters() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_index_usage (id int primary key, value int, \
         key idx_value(value), key idx_both(id, value))",
        Vec::new(),
    );
    tk.MustExec(
        "create table infoschema_string_pk (id varchar(32) primary key)",
        Vec::new(),
    );
    let rows = tk
        .MustQuery(
            "select table_schema, table_name, index_name \
             from information_schema.tidb_index_usage \
             where table_schema='test' order by table_name, index_name",
            Vec::new(),
        )
        .Rows();
    assert!(
        rows.iter()
            .any(|r| r == &["test", "infoschema_index_usage", "primary"])
    );
    assert!(
        rows.iter()
            .any(|r| r == &["test", "infoschema_index_usage", "idx_value"])
    );
    assert!(
        rows.iter()
            .any(|r| r == &["test", "infoschema_index_usage", "idx_both"])
    );
    assert!(
        rows.iter()
            .any(|r| r == &["test", "infoschema_string_pk", "primary"])
    );
    tk.MustQuery(
        "select table_schema, table_name, index_name \
         from information_schema.tidb_index_usage where index_name='IDX_VALUE'",
        Vec::new(),
    )
    .Check(RowsWithSep("|", &["test|infoschema_index_usage|idx_value"]));
    tk.MustQuery(
        "select table_schema, table_name, index_name \
         from information_schema.tidb_index_usage where table_name='missing_table'",
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 对应 Go `TestReferencedTableSchemaWithForeignKey`：引用库不能丢失。
#[test]
fn foreign_key_metadata_preserves_referenced_schema() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database infoschema_parent", Vec::new());
    tk.MustExec("create database infoschema_child", Vec::new());
    tk.MustExec(
        "create table infoschema_parent.parent_table (id int primary key)",
        Vec::new(),
    );
    tk.MustExec(
        "create table infoschema_child.child_table (id int, foreign key (id) \
         references infoschema_parent.parent_table(id))",
        Vec::new(),
    );
    tk.MustQuery(
        "select column_name, referenced_column_name, referenced_table_name, \
         table_schema, referenced_table_schema from information_schema.key_column_usage \
         where table_name='child_table' and table_schema='infoschema_child'",
        Vec::new(),
    )
    .Check(RowsWithSep(
        "|",
        &["id|id|parent_table|infoschema_child|infoschema_parent"],
    ));
}

/// 对应 Go `TestSameTableNameInTwoSchemas`：相同表名按库与物理 ID 唯一定位。
#[test]
fn same_table_name_in_two_schemas_isolated_by_schema_and_id() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    for database in ["infoschema_same_db1", "infoschema_same_db2"] {
        tk.MustExec(&format!("create database {database}"), Vec::new());
        tk.MustExec(
            &format!("create table {database}.same_name (a int)"),
            Vec::new(),
        );
    }
    let id1 = tk
        .MustQuery(
            "select tidb_table_id from information_schema.tables \
             where table_schema='infoschema_same_db1' and table_name='same_name'",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    let id2 = tk
        .MustQuery(
            "select tidb_table_id from information_schema.tables \
             where table_schema='infoschema_same_db2' and table_name='same_name'",
            Vec::new(),
        )
        .Rows()[0][0]
        .clone();
    assert_ne!(id1, id2);
    tk.MustQuery(
        &format!(
            "select table_schema, table_name, tidb_table_id from information_schema.tables \
             where table_name='same_name' and tidb_table_id={id1}"
        ),
        Vec::new(),
    )
    .Check(RowsWithSep(
        "|",
        &[&format!("infoschema_same_db1|same_name|{id1}")],
    ));
    tk.MustQuery(
        &format!(
            "select table_schema, table_name, tidb_table_id from information_schema.tables \
             where table_schema='unknown' and tidb_table_id={id2}"
        ),
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 对应 Go `TestJoinSystemTableContainsView`：视图出现在 INFORMATION_SCHEMA
/// 的相关联子查询结果中，且重复读取结果稳定。
#[test]
fn joined_system_table_query_contains_view_and_time_value_columns() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_time_value (at timestamp, value int)",
        Vec::new(),
    );
    tk.MustExec(
        "create view infoschema_time_view (at, value) as select * from infoschema_time_value",
        Vec::new(),
    );
    tk.MustQuery(
        "select table_name from information_schema.tables \
         where table_schema=database() and table_name='infoschema_time_view'",
        Vec::new(),
    )
    .Check(Rows(&["infoschema_time_view"]));
    tk.MustQuery(
        "select column_name from information_schema.columns \
         where table_schema=database() and table_name='infoschema_time_view' \
         order by ordinal_position",
        Vec::new(),
    )
    .Check(Rows(&["at", "value"]));
}

/// 对应 Go `TestShowColumnsWithSubQueryView`：SHOW COLUMNS 只读取视图元数据，
/// 不依赖底层表扫描。
#[test]
fn show_columns_from_subquery_view_returns_view_metadata() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_added (id int, name text, some_date timestamp)",
        Vec::new(),
    );
    tk.MustExec(
        "create table infoschema_incremental (id int, name text, some_date timestamp)",
        Vec::new(),
    );
    tk.MustExec(
        "create view infoschema_temp_view (id, name, some_date) as select * from infoschema_added \
         where id > (select max(id) from infoschema_incremental)",
        Vec::new(),
    );
    let show_columns = tk
        .MustQuery("show columns from infoschema_temp_view", Vec::new())
        .Rows();
    assert_eq!(show_columns.len(), 3);
    assert_eq!(show_columns[0][0], "id");
    assert_eq!(show_columns[1][0], "name");
    assert_eq!(show_columns[2][0], "some_date");
    assert!(show_columns.iter().all(|row| row[2] == "YES"));
    tk.MustQuery(
        "select column_name from information_schema.columns \
         where table_name='infoschema_temp_view' order by ordinal_position",
        Vec::new(),
    )
    .Check(Rows(&["id", "name", "some_date"]));
}

/// 对应 Go `TestInfoSchemaConditionWorks` 的核心过滤断言：库、表、约束和
/// 分区名条件不能退化成全表扫描后返回未过滤行。
#[test]
fn infoschema_conditions_filter_constraints_and_unpartitioned_tables() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    for database in ["infoschema_cond_db0", "infoschema_cond_db1"] {
        tk.MustExec(&format!("create database {database}"), Vec::new());
        tk.MustExec(
            &format!(
                "create table {database}.condition_table (id int primary key, value int, \
                 unique key condition_idx(value)) partition by range (id) \
                 (partition p0 values less than (10), partition p1 values less than (20))"
            ),
            Vec::new(),
        );
    }
    tk.MustQuery(
        "select constraint_name, table_schema from information_schema.table_constraints \
         where constraint_name='PRIMARY' and table_schema='infoschema_cond_db0'",
        Vec::new(),
    )
    .Check(Rows(&["PRIMARY infoschema_cond_db0"]));
    tk.MustExec("create database infoschema_no_partition", Vec::new());
    tk.MustExec(
        "create table infoschema_no_partition.plain_table (id int primary key)",
        Vec::new(),
    );
    let unpartitioned = tk
        .MustQuery(
            "select partition_name from information_schema.partitions \
             where table_schema='infoschema_no_partition' and table_name='plain_table' \
             and partition_name is null",
            Vec::new(),
        )
        .Rows();
    assert_eq!(unpartitioned.len(), 1);
    tk.MustQuery(
        "select partition_name from information_schema.partitions \
         where table_schema='infoschema_cond_db0' and table_name='condition_table' \
         and partition_name='p0'",
        Vec::new(),
    )
    .Check(Rows(&["p0"]));
}

/// 对应 Go `TestInfoSchemaDDLJobs` 的稳定历史部分：已完成的建表/加索引
/// 作业按 JOB_ID 倒序公开，running 过滤不应返回已完成作业。
#[test]
fn infoschema_ddl_jobs_exposes_completed_history_and_running_filter() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database infoschema_ddl_db", Vec::new());
    tk.MustExec(
        "create table infoschema_ddl_db.job_table (id int, value int)",
        Vec::new(),
    );
    tk.MustExec(
        "alter table infoschema_ddl_db.job_table add index job_idx(value)",
        Vec::new(),
    );
    let rows = tk
        .MustQuery(
            "select job_type, schema_state, table_name, state from information_schema.ddl_jobs \
             where db_name='infoschema_ddl_db' and table_name='job_table' order by job_id desc",
            Vec::new(),
        )
        .Rows();
    assert!(rows.len() >= 2, "completed DDL history missing: {rows:?}");
    assert!(rows.iter().all(|row| row[3] == "synced"));
    tk.MustQuery(
        "select job_id from information_schema.ddl_jobs \
         where db_name='infoschema_ddl_db' and state='running'",
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 对应 Go `TestInfoschemaTablesSpecialOptimizationCovered` 的结果契约：
/// 可优化的投影/聚合查询与需要完整元数据的查询都返回正确结果。
#[test]
fn infoschema_tables_projection_and_count_queries_return_correct_results() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database infoschema_opt_db", Vec::new());
    tk.MustExec(
        "create table infoschema_opt_db.opt_table (id int)",
        Vec::new(),
    );
    tk.MustQuery(
        "select table_schema, table_name from information_schema.tables \
         where table_schema='infoschema_opt_db' and table_name='opt_table'",
        Vec::new(),
    )
    .Check(Rows(&["infoschema_opt_db opt_table"]));
    tk.MustQuery(
        "select count(table_name) from information_schema.tables \
         where table_schema='infoschema_opt_db'",
        Vec::new(),
    )
    .Check(Rows(&["1"]));
    tk.MustQuery(
        "select table_schema, table_name from information_schema.tables \
         where table_schema='missing_schema'",
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 对应 Go `TestDataForTableStatsField` 的关键统计变化：插入后刷新统计，
/// INFORMATION_SCHEMA.TABLES 的行数与数据长度随之更新。
#[test]
fn tables_stats_fields_follow_analyze_and_flush_updates() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_stats (id int, value int, key value_idx(value))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into infoschema_stats values (1, 2), (2, 3), (3, 4)",
        Vec::new(),
    );
    tk.MustExec("flush stats_delta *.*", Vec::new());
    tk.MustExec("analyze table infoschema_stats all columns", Vec::new());
    let rows = tk
        .MustQuery(
            "select table_rows, avg_row_length, data_length, index_length \
             from information_schema.tables where table_name='infoschema_stats'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "3");
    assert!(rows[0][1].parse::<u64>().expect("average row length") > 0);
    assert!(rows[0][2].parse::<u64>().expect("data length") > 0);
    assert!(rows[0][3].parse::<u64>().expect("index length") > 0);
    let info_schema = domain.info_schema();
    assert!(info_schema.SchemaMetaVersion() > 0);
}

/// 对应 Go `TestForAnalyzeStatus` 的可观察部分：ANALYZE 后状态表公开完成
/// 作业，且 SHOW ANALYZE STATUS 与直接查询使用同一行集。
#[test]
fn analyze_status_matches_show_analyze_status_after_analyze() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table infoschema_analyze (id int, value int, key value_idx(value))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into infoschema_analyze values (1, 2), (3, 4)",
        Vec::new(),
    );
    tk.MustExec("analyze table infoschema_analyze all columns", Vec::new());
    let direct = tk
        .MustQuery(
            "select * from information_schema.analyze_status \
             where table_name='infoschema_analyze'",
            Vec::new(),
        )
        .Rows();
    assert!(!direct.is_empty());
    assert!(direct.iter().all(|row| row.len() == 14));
    let shown = tk
        .MustQuery(
            "show analyze status where table_name='infoschema_analyze'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(direct.len(), shown.len());
    fn normalize_null(value: &str) -> &str {
        match value {
            "__astersql_internal_null__" | "<nil>" => "<nil>",
            value => value,
        }
    }
    for (direct_row, shown_row) in direct.iter().zip(shown) {
        let direct_values = direct_row[..12]
            .iter()
            .map(|value| normalize_null(value))
            .collect::<Vec<_>>();
        let shown_values = shown_row
            .iter()
            .map(|value| normalize_null(value))
            .collect::<Vec<_>>();
        assert_eq!(direct_values, shown_values);
    }
}
