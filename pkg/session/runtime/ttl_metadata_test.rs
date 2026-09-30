// Copyright 2026 AsterSQL.

use astersql_meta_model::{PartitionDefinition, PartitionInfo, TTLInfo};
use astersql_parser_ast::{NewCIStr, TimeUnitType};

use super::{
    CreateAnalyzeSession,
    ttl_metadata::{collect_physical_ttl_tables, collect_ttl_schedules, physical_ttl_tables},
};

#[test]
fn go_merge_43_ttl_metadata_uses_canonical_table_and_calendar_interval() {
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    session
        .execute("CREATE TABLE ttl_metadata_test (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create table");
    let (_, mut table) = domain
        .stats_table("test", "ttl_metadata_test")
        .expect("canonical table metadata");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    let tables = physical_ttl_tables("test", &table, 200_000).expect("TTL physical table");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].table_id, table.ID);
    assert_eq!(tables[0].key_columns, vec!["id"]);
    assert_eq!(tables[0].ttl_column, "expire_at");
    assert_eq!(tables[0].expire_time(200_000), 113_600);

    table.ID = 0;
    table.Name = NewCIStr("ttl_discovered_test");
    domain
        .ddl_create_table("test", table.clone(), false)
        .expect("publish TTL table metadata");
    let discovered = collect_physical_ttl_tables(domain.info_schema().as_ref(), 200_000)
        .expect("discover TTL tables from InfoSchema");
    assert!(
        discovered
            .iter()
            .any(|item| item.table == "ttl_discovered_test")
    );
    let schedules = collect_ttl_schedules(domain.info_schema().as_ref(), 200_000)
        .expect("read TTL schedule interval");
    assert!(schedules.iter().any(|item| {
        item.table.table == "ttl_discovered_test" && item.job_interval_seconds == 86_400
    }));

    table.TTLInfo.as_mut().unwrap().Enable = false;
    assert!(
        physical_ttl_tables("test", &table, 200_000)
            .expect("disabled TTL")
            .is_empty()
    );
}

#[test]
fn go_merge_43_ttl_metadata_schedules_each_partition_by_physical_id() {
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    session
        .execute("CREATE TABLE ttl_partition_metadata (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create table");
    let (_, mut table) = domain
        .stats_table("test", "ttl_partition_metadata")
        .expect("canonical table metadata");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    table.Partition = Some(PartitionInfo {
        Enable: true,
        Definitions: vec![
            PartitionDefinition {
                ID: 101,
                Name: NewCIStr("p0"),
                ..Default::default()
            },
            PartitionDefinition {
                ID: 102,
                Name: NewCIStr("p1"),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let physical = physical_ttl_tables("test", &table, 200_000).expect("partitioned TTL table");
    assert_eq!(physical.len(), 2);
    assert_eq!(physical[0].physical_id, 101);
    assert_eq!(physical[0].partition_name.as_deref(), Some("p0"));
    assert_eq!(physical[1].physical_id, 102);
    assert_eq!(physical[1].partition_name.as_deref(), Some("p1"));
    assert!(physical.iter().all(|part| part.table_id == table.ID));
}
