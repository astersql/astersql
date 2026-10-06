// Copyright 2026 AsterSQL.

use super::normal_ddl_fixture::Fixture;

fn update_table(
    fixture: &Fixture,
    table_name: &str,
    update: impl FnOnce(&mut astersql_meta_model::TableInfo),
) {
    let mut table = fixture
        .domain
        .table_by_name("test", table_name)
        .unwrap()
        .as_ref()
        .clone();
    update(&mut table);
    let mut transaction = fixture
        .domain
        .storage_handle()
        .with_storage(|storage| storage.Begin(&[]))
        .unwrap();
    let mut metadata = astersql_meta::TransactionMutator::new(transaction.as_mut());
    metadata.update_table(fixture.db, &mut table).unwrap();
    let version = metadata.gen_schema_version().unwrap();
    metadata
        .set_table_schema_diff(
            &astersql_meta_model::group_3::Job {
                schema_id: fixture.db,
                table_id: table.ID,
                ..Default::default()
            },
            version,
        )
        .unwrap();
    transaction
        .Commit(&astersql_kv::Context::default())
        .unwrap();
    fixture.domain.reload().unwrap();
}

fn assert_unsupported(fixture: &Fixture, sql: &str, expected: &str) {
    let error = fixture.pool.acquire().unwrap().query(sql).unwrap_err();
    assert!(error.contains(expected), "{error}");
}

#[test]
fn materialized_view_dependencies_guard_partitioning_changes() {
    let fixture = Fixture::new();
    update_table(&fixture, "normal_ddl_target", |table| {
        table.MaterializedViewBase = Some(astersql_meta_model::MaterializedViewBaseInfo {
            MLogID: 42,
            MViewIDs: Vec::new(),
        });
    });
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.normal_ddl_target PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (10), PARTITION p1 VALUES LESS THAN MAXVALUE)",
        "ALTER TABLE ... PARTITION BY with materialized view log",
    );

    update_table(&fixture, "normal_ddl_target", |table| {
        table.MaterializedViewBase = Some(astersql_meta_model::MaterializedViewBaseInfo {
            MLogID: 0,
            MViewIDs: vec![84],
        });
        table.Partition = Some(astersql_meta_model::PartitionInfo {
            Type: astersql_meta_model::ast::PartitionType::Range,
            Enable: true,
            Definitions: vec![astersql_meta_model::PartitionDefinition {
                ID: 100,
                Name: astersql_meta_model::ast::NewCIStr("p0"),
                LessThan: vec!["MAXVALUE".into()],
                ..Default::default()
            }],
            ..Default::default()
        });
    });
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.normal_ddl_target REMOVE PARTITIONING",
        "ALTER TABLE ... REMOVE PARTITIONING with materialized view dependencies",
    );
}

#[test]
fn materialized_view_roles_guard_exchange_partition() {
    let fixture = Fixture::new();
    let mut session = fixture.pool.acquire().unwrap();
    session
        .query("CREATE TABLE test.exchange_part (id INT PRIMARY KEY) PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (10), PARTITION p1 VALUES LESS THAN MAXVALUE)")
        .unwrap();
    session
        .query("CREATE TABLE test.exchange_plain (id INT PRIMARY KEY)")
        .unwrap();
    drop(session);

    update_table(&fixture, "exchange_plain", |table| {
        table.MaterializedViewLog = Some(astersql_meta_model::MaterializedViewLogInfo::default());
    });
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.exchange_part EXCHANGE PARTITION p0 WITH TABLE test.exchange_plain",
        "EXCHANGE PARTITION on non-partitioned table with materialized view log",
    );

    update_table(&fixture, "exchange_plain", |table| {
        table.MaterializedViewLog = None;
        table.MaterializedView = Some(astersql_meta_model::MaterializedViewInfo::default());
    });
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.exchange_part EXCHANGE PARTITION p0 WITH TABLE test.exchange_plain",
        "EXCHANGE PARTITION on non-partitioned table materialized view table",
    );

    update_table(&fixture, "exchange_plain", |table| {
        table.MaterializedView = None;
    });
    update_table(&fixture, "exchange_part", |table| {
        table.MaterializedViewBase = Some(astersql_meta_model::MaterializedViewBaseInfo {
            MLogID: 0,
            MViewIDs: vec![84],
        });
    });
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.exchange_part EXCHANGE PARTITION p0 WITH TABLE test.exchange_plain",
        "EXCHANGE PARTITION on partitioned table with materialized view dependencies",
    );
}

#[test]
fn materialized_view_rejects_unique_and_primary_indexes() {
    let fixture = Fixture::new();
    update_table(&fixture, "normal_ddl_target", |table| {
        table.MaterializedView = Some(astersql_meta_model::MaterializedViewInfo::default());
    });

    assert_unsupported(
        &fixture,
        "CREATE UNIQUE INDEX unique_id ON test.normal_ddl_target (id)",
        "Unsupported CREATE UNIQUE INDEX on materialized view table",
    );
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.normal_ddl_target ADD UNIQUE KEY unique_payload (payload)",
        "Unsupported ALTER TABLE ADD UNIQUE INDEX on materialized view table",
    );
    assert_unsupported(
        &fixture,
        "ALTER TABLE test.normal_ddl_target ADD PRIMARY KEY (id) NONCLUSTERED",
        "Unsupported ALTER TABLE ADD PRIMARY KEY on materialized view table",
    );

    fixture
        .pool
        .acquire()
        .unwrap()
        .query("CREATE INDEX payload_idx ON test.normal_ddl_target (payload)")
        .unwrap();
}
