// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// INFORMATION_SCHEMA 表辅助函数单测。
//
// 对应 Go `pkg/infoschema/tables_test.go`：验证根据 Store 标签识别
// TiFlash 存储节点与写角色节点的纯函数逻辑（不依赖真实 PD/TiKV）。

// 对应 pkg/infoschema/tables_test.go。

use std::collections::HashSet;
use std::sync::Arc;

use crate::ClusterTableTiDBIndexUsage;
use crate::infoschema::{CiString, TableInfo};
use crate::tables::{
    ColumnType, FilterClusterServerInfo, FormatStoreServerVersion, GetShardingInfo, ServerInfo,
    StoreInfo, StoreLabel, TableCheckConstraints, TableClusterConfig, TableCollations,
    TableColumnPrivileges, TableColumns, TableConstraints, TableKeyColumn, TablePartitions,
    TableSchemaPrivileges, TableSchemata, TableStatistics, TableTablePrivileges, TableTables,
    TableTiDBCheckConstraints, TableUserPrivileges, TableViews, columnInfo, is_tiflash_store,
    is_tiflash_write_node, table_registry,
};

fn assert_go_column_names(table_name: &str, expected: &[&str]) -> Vec<columnInfo> {
    let columns = table_registry()
        .get(table_name)
        .unwrap_or_else(|| panic!("{table_name} missing from INFORMATION_SCHEMA registry"))
        .columns
        .clone();
    assert_eq!(
        columns.iter().map(|column| column.name).collect::<Vec<_>>(),
        expected,
        "{table_name} columns differ from Go pkg/infoschema/tables.go",
    );
    columns
}

#[test]
fn jdbc_catalog_tables_match_go_column_definitions() {
    let schemata = assert_go_column_names(
        TableSchemata,
        &[
            "CATALOG_NAME",
            "SCHEMA_NAME",
            "DEFAULT_CHARACTER_SET_NAME",
            "DEFAULT_COLLATION_NAME",
            "SQL_PATH",
            "TIDB_PLACEMENT_POLICY_NAME",
        ],
    );
    assert_eq!(schemata[3].size, 32);

    let tables = assert_go_column_names(
        TableTables,
        &[
            "TABLE_CATALOG",
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "TABLE_TYPE",
            "ENGINE",
            "VERSION",
            "ROW_FORMAT",
            "TABLE_ROWS",
            "AVG_ROW_LENGTH",
            "DATA_LENGTH",
            "MAX_DATA_LENGTH",
            "INDEX_LENGTH",
            "DATA_FREE",
            "AUTO_INCREMENT",
            "CREATE_TIME",
            "UPDATE_TIME",
            "CHECK_TIME",
            "TABLE_COLLATION",
            "CHECKSUM",
            "CREATE_OPTIONS",
            "TABLE_COMMENT",
            "TIDB_TABLE_ID",
            "TIDB_ROW_ID_SHARDING_INFO",
            "TIDB_PK_TYPE",
            "TIDB_PLACEMENT_POLICY_NAME",
            "TIDB_TABLE_MODE",
            "TIDB_AFFINITY",
        ],
    );
    assert_eq!(tables[14].column_type, ColumnType::Datetime);
    assert_eq!(tables[17].default_value, Some("utf8mb4_bin"));

    let columns = assert_go_column_names(
        TableColumns,
        &[
            "TABLE_CATALOG",
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "COLUMN_NAME",
            "ORDINAL_POSITION",
            "COLUMN_DEFAULT",
            "IS_NULLABLE",
            "DATA_TYPE",
            "CHARACTER_MAXIMUM_LENGTH",
            "CHARACTER_OCTET_LENGTH",
            "NUMERIC_PRECISION",
            "NUMERIC_SCALE",
            "DATETIME_PRECISION",
            "CHARACTER_SET_NAME",
            "COLLATION_NAME",
            "COLUMN_TYPE",
            "COLUMN_KEY",
            "EXTRA",
            "PRIVILEGES",
            "COLUMN_COMMENT",
            "GENERATION_EXPRESSION",
            "SRS_ID",
        ],
    );
    assert_eq!(columns[4].column_type, ColumnType::Long);
    assert!(columns[4].unsigned);
    assert_eq!(columns[15].column_type, ColumnType::MediumBlob);
    assert_eq!(columns[20].column_type, ColumnType::LongBlob);
    assert!(columns[20].not_null);
    assert!(columns[21].unsigned);
}

#[test]
fn introspection_catalog_tables_match_go_column_definitions() {
    assert_go_column_names(TableClusterConfig, &["TYPE", "INSTANCE", "KEY", "VALUE"]);
    assert_go_column_names(
        ClusterTableTiDBIndexUsage,
        &[
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "INDEX_NAME",
            "QUERY_TOTAL",
            "KV_REQ_TOTAL",
            "ROWS_ACCESS_TOTAL",
            "PERCENTAGE_ACCESS_0",
            "PERCENTAGE_ACCESS_0_1",
            "PERCENTAGE_ACCESS_1_10",
            "PERCENTAGE_ACCESS_10_20",
            "PERCENTAGE_ACCESS_20_50",
            "PERCENTAGE_ACCESS_50_100",
            "PERCENTAGE_ACCESS_100",
            "LAST_ACCESS_TIME",
        ],
    );
    assert_go_column_names(
        TableCollations,
        &[
            "COLLATION_NAME",
            "CHARACTER_SET_NAME",
            "ID",
            "IS_DEFAULT",
            "IS_COMPILED",
            "SORTLEN",
            "PAD_ATTRIBUTE",
        ],
    );
    for (table, columns) in [
        (
            TableUserPrivileges,
            &["GRANTEE", "TABLE_CATALOG", "PRIVILEGE_TYPE", "IS_GRANTABLE"][..],
        ),
        (
            TableSchemaPrivileges,
            &[
                "GRANTEE",
                "TABLE_CATALOG",
                "TABLE_SCHEMA",
                "PRIVILEGE_TYPE",
                "IS_GRANTABLE",
            ][..],
        ),
        (
            TableTablePrivileges,
            &[
                "GRANTEE",
                "TABLE_CATALOG",
                "TABLE_SCHEMA",
                "TABLE_NAME",
                "PRIVILEGE_TYPE",
                "IS_GRANTABLE",
            ][..],
        ),
        (
            TableColumnPrivileges,
            &[
                "GRANTEE",
                "TABLE_CATALOG",
                "TABLE_SCHEMA",
                "TABLE_NAME",
                "COLUMN_NAME",
                "PRIVILEGE_TYPE",
                "IS_GRANTABLE",
            ][..],
        ),
    ] {
        assert_go_column_names(table, columns);
    }
    for (table, required_columns) in [
        (
            TableStatistics,
            &["TABLE_SCHEMA", "TABLE_NAME", "NON_UNIQUE", "INDEX_NAME"][..],
        ),
        (
            TablePartitions,
            &["TABLE_SCHEMA", "TABLE_NAME", "PARTITION_NAME"][..],
        ),
        (
            TableKeyColumn,
            &["TABLE_SCHEMA", "TABLE_NAME", "COLUMN_NAME"][..],
        ),
        (
            TableConstraints,
            &["TABLE_SCHEMA", "TABLE_NAME", "CONSTRAINT_TYPE"][..],
        ),
        (TableViews, &["TABLE_SCHEMA", "TABLE_NAME", "DEFINER"][..]),
    ] {
        let columns = table_registry()
            .get(table)
            .expect("registered introspection table")
            .columns
            .iter()
            .map(|column| column.name)
            .collect::<Vec<_>>();
        for required in required_columns {
            assert!(columns.contains(required), "{table} lacks {required}");
        }
    }
}

#[test]
fn check_constraint_tables_match_go_column_definitions() {
    let check_constraints = assert_go_column_names(
        TableCheckConstraints,
        &[
            "CONSTRAINT_CATALOG",
            "CONSTRAINT_SCHEMA",
            "CONSTRAINT_NAME",
            "CHECK_CLAUSE",
        ],
    );
    assert!(
        check_constraints.iter().all(|column| column.not_null),
        "standard CHECK_CONSTRAINTS columns are NOT NULL in Go",
    );
    assert_eq!(check_constraints[3].column_type, ColumnType::LongBlob);

    let tidb_check_constraints = assert_go_column_names(
        TableTiDBCheckConstraints,
        &[
            "CONSTRAINT_CATALOG",
            "CONSTRAINT_SCHEMA",
            "CONSTRAINT_NAME",
            "CHECK_CLAUSE",
            "TABLE_NAME",
            "TABLE_ID",
        ],
    );
    assert_eq!(tidb_check_constraints[5].column_type, ColumnType::Longlong);
    assert!(!tidb_check_constraints[4].not_null);
    assert!(!tidb_check_constraints[5].not_null);
}

/// 验证 `is_tiflash_store`：存在 `engine=tiflash` 标签即为 TiFlash Store。
#[test]
fn test_is_tiflash_store() {
    let tiflash_store = StoreInfo {
        labels: vec![StoreLabel {
            key: "engine".to_string(),
            value: "tiflash".to_string(),
        }],
    };
    assert!(is_tiflash_store(&tiflash_store));

    let non_tiflash_store = StoreInfo {
        labels: vec![StoreLabel {
            key: "engine".to_string(),
            value: "tikv".to_string(),
        }],
    };
    assert!(!is_tiflash_store(&non_tiflash_store));

    let empty_store = StoreInfo { labels: vec![] };
    assert!(!is_tiflash_store(&empty_store));

    let multi_label_store = StoreInfo {
        labels: vec![
            StoreLabel {
                key: "zone".to_string(),
                value: "zone1".to_string(),
            },
            StoreLabel {
                key: "engine".to_string(),
                value: "tiflash".to_string(),
            },
            StoreLabel {
                key: "region".to_string(),
                value: "us-west".to_string(),
            },
        ],
    };
    assert!(is_tiflash_store(&multi_label_store));
}

/// 验证 `is_tiflash_write_node`：存在 `engine_role=write` 标签即为写节点。
#[test]
fn test_is_tiflash_write_node() {
    let write_node = StoreInfo {
        labels: vec![StoreLabel {
            key: "engine_role".to_string(),
            value: "write".to_string(),
        }],
    };
    assert!(is_tiflash_write_node(&write_node));

    let non_write_node = StoreInfo {
        labels: vec![StoreLabel {
            key: "engine_role".to_string(),
            value: "read".to_string(),
        }],
    };
    assert!(!is_tiflash_write_node(&non_write_node));

    let empty_store = StoreInfo { labels: vec![] };
    assert!(!is_tiflash_write_node(&empty_store));

    let multi_label_store = StoreInfo {
        labels: vec![
            StoreLabel {
                key: "zone".to_string(),
                value: "zone1".to_string(),
            },
            StoreLabel {
                key: "engine_role".to_string(),
                value: "write".to_string(),
            },
            StoreLabel {
                key: "region".to_string(),
                value: "us-west".to_string(),
            },
        ],
    };
    assert!(is_tiflash_write_node(&multi_label_store));
}

#[test]
fn store_helpers_preserve_go_exact_matching_and_prefix_contracts() {
    assert_eq!(FormatStoreServerVersion("vv8.5.0"), "v8.5.0");

    let uppercase_engine = StoreInfo {
        labels: vec![StoreLabel {
            key: "engine".to_string(),
            value: "TiFlash".to_string(),
        }],
    };
    assert!(!is_tiflash_store(&uppercase_engine));

    let uppercase_role = StoreInfo {
        labels: vec![StoreLabel {
            key: "engine_role".to_string(),
            value: "WRITE".to_string(),
        }],
    };
    assert!(!is_tiflash_write_node(&uppercase_role));
}

#[test]
fn cluster_server_filter_matches_go_strings_exactly() {
    let servers = vec![ServerInfo {
        server_type: "TiKV".to_string(),
        address: "127.0.0.1:20160".to_string(),
        ..ServerInfo::default()
    }];
    let node_types = HashSet::from(["TiKV".to_string()]);

    assert_eq!(
        FilterClusterServerInfo(servers, &node_types, &HashSet::new()).len(),
        1
    );
}

#[test]
fn sharding_info_matches_go_metadata_branches() {
    let mut model = astersql_meta_model::TableInfo::default();
    model.AutoRandomBits = 5;
    model.AutoRandomRangeBits = 10;
    let table = TableInfo {
        model_meta: Some(Arc::new(model)),
        ..TableInfo::default()
    };
    assert_eq!(
        GetShardingInfo(&CiString::new("test"), &table).as_deref(),
        Some("PK_AUTO_RANDOM_BITS=5, RANGE BITS=10")
    );
    assert_eq!(
        GetShardingInfo(&CiString::new("INFORMATION_SCHEMA"), &table),
        None
    );
}
