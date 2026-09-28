// Copyright 2026 AsterSQL.

use crate::{ClusterTableCopDestination, GetClusterTableCopDestination, IsClusterTableByName};

#[test]
fn cluster_table_name_checks_match_go_lowercase_contract() {
    assert!(IsClusterTableByName(
        "information_schema",
        "cluster_slow_query"
    ));
    assert!(IsClusterTableByName(
        "performance_schema",
        "tiflash_replica"
    ));

    assert!(!IsClusterTableByName(
        "INFORMATION_SCHEMA",
        "cluster_slow_query"
    ));
    assert!(!IsClusterTableByName(
        "information_schema",
        "CLUSTER_SLOW_QUERY"
    ));
}

#[test]
fn ddl_owner_destination_keeps_go_case_insensitive_lookup() {
    assert_eq!(
        GetClusterTableCopDestination("tiflash_replica"),
        ClusterTableCopDestination::DDLOwner
    );
    assert_eq!(
        GetClusterTableCopDestination("slow_query"),
        ClusterTableCopDestination::AllTiDB
    );
}
