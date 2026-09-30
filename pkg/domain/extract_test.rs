// Copyright 2026 AsterSQL.

use crate::extract::{
    ExtractHandle, ExtractPlanPackage, ExtractSource, ExtractTask, ExtractType, StatementRecord,
    TableNamePair, view_dependencies_from_sql,
};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn go_merge_43_extract_walks_nested_view_ast() {
    let tables = view_dependencies_from_sql(
        "CREATE VIEW test.v AS SELECT a.id FROM test.a AS a WHERE EXISTS (SELECT 1 FROM b WHERE b.id = a.id)",
        "test",
    )
    .unwrap();
    assert_eq!(
        tables,
        vec![
            TableNamePair {
                database: "test".into(),
                table: "a".into(),
                is_view: false
            },
            TableNamePair {
                database: "test".into(),
                table: "b".into(),
                is_view: false
            },
        ]
    );

    let tables = view_dependencies_from_sql(
        "WITH picked AS (SELECT id FROM test.base) SELECT p.id FROM picked p JOIN test.joined j ON j.id = p.id WHERE EXISTS (SELECT 1 FROM test.deep d WHERE d.id = p.id)",
        "test",
    )
    .unwrap();
    assert_eq!(
        tables
            .iter()
            .map(|table| table.table.as_str())
            .collect::<Vec<_>>(),
        ["base", "deep", "joined"]
    );

    let tables = view_dependencies_from_sql(
        "WITH RECURSIVE chain AS (SELECT id FROM test.base UNION ALL SELECT id FROM chain) SELECT id FROM chain",
        "test",
    )
    .unwrap();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].table, "base");

    nested_view_dependencies_use_domain_schema();
}
struct ExtractViewSchemaLoader;

impl crate::InfoSchemaLoader for ExtractViewSchemaLoader {
    fn load_info_schema(
        &self,
        _store: &dyn astersql_kv::Storage,
        _keyspace: &str,
    ) -> Result<crate::LoadedInfoSchema, astersql_kv::errors::SharedError> {
        Ok(self.schema())
    }

    fn load_snapshot_info_schema(
        &self,
        _store: &dyn astersql_kv::Storage,
        _keyspace: &str,
        _timestamp: u64,
    ) -> Result<crate::LoadedInfoSchema, astersql_kv::errors::SharedError> {
        Ok(self.schema())
    }

    fn keyspace_exists(
        &self,
        _store: &dyn astersql_kv::Storage,
        _keyspace: &str,
    ) -> Result<bool, astersql_kv::errors::SharedError> {
        Ok(true)
    }
}

impl ExtractViewSchemaLoader {
    fn schema(&self) -> crate::LoadedInfoSchema {
        let mut schema = astersql_infoschema::infoschema::infoSchema::new(1);
        let tables = [
            ("base", None),
            (
                "v2",
                Some("SELECT b.id FROM test.base b JOIN mysql.user u ON u.id = b.id"),
            ),
            ("v1", Some("SELECT id FROM test.v2")),
            ("cycle1", Some("SELECT id FROM test.cycle2")),
            ("cycle2", Some("SELECT id FROM test.cycle1")),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, sql))| {
            astersql_infoschema::Table::from_model(astersql_meta_model::TableInfo {
                ID: index as i64 + 1,
                DBID: 1,
                Name: astersql_parser_ast::NewCIStr(name),
                State: astersql_meta_model::StatePublic,
                View: sql.map(|sql| astersql_meta_model::ViewInfo {
                    SelectStmt: sql.into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
        })
        .collect();
        schema.add_schema(
            astersql_infoschema::DBInfo {
                id: 1,
                name: astersql_infoschema::CiString::new("test"),
                ..Default::default()
            },
            tables,
        );
        schema.add_schema(
            astersql_infoschema::DBInfo {
                id: 2,
                name: astersql_infoschema::CiString::new("mysql"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::from_model(
                astersql_meta_model::TableInfo {
                    ID: 6,
                    DBID: 2,
                    Name: astersql_parser_ast::NewCIStr("user"),
                    State: astersql_meta_model::StatePublic,
                    ..Default::default()
                },
            )],
        );
        crate::LoadedInfoSchema::new(Arc::new(schema), 10)
    }
}

fn nested_view_dependencies_use_domain_schema() {
    let storage = astersql_store_mockstore_mockstorage::NewMockStorage(
        astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
        None,
    )
    .unwrap();
    let domain = Arc::new(crate::domain::Domain::new_mock(
        Arc::try_unwrap(storage).ok().unwrap(),
        Arc::new(ExtractViewSchemaLoader),
    ));
    domain.init().unwrap();
    let source = Arc::new(MockSource::default());
    source.views.lock().unwrap().extend([
        "v1".into(),
        "v2".into(),
        "cycle1".into(),
        "cycle2".into(),
    ]);
    source
        .records
        .lock()
        .unwrap()
        .push(record("v1", "plan", "SELECT id FROM test.v1", "encoded"));
    source.records.lock().unwrap().push(record(
        "cycle1",
        "plan",
        "SELECT id FROM test.cycle1",
        "encoded",
    ));
    let handle = ExtractHandle::new_with_domain(domain, source.clone());
    handle
        .extract_task(&ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH))
        .unwrap();
    let dumped = source.dumped.lock().unwrap();
    let names = dumped[0]
        .tables
        .iter()
        .map(|table| table.table.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from(["base", "v1", "v2", "cycle1", "cycle2", "user"])
    );
}

#[derive(Default)]
struct MockSource {
    persistent: bool,
    records: Mutex<Vec<StatementRecord>>,
    decoded: Mutex<Vec<String>>,
    dumped: Mutex<Vec<ExtractPlanPackage>>,
    views: Mutex<BTreeSet<String>>,
}

impl ExtractSource for MockSource {
    fn statement_records(&self, _task: &ExtractTask) -> Result<Vec<StatementRecord>, String> {
        Ok(self.records.lock().unwrap().clone())
    }

    fn table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String> {
        Ok(Some(TableNamePair {
            database: database.into(),
            table: table.into(),
            is_view: self.views.lock().unwrap().contains(table),
        }))
    }

    fn view_dependencies(&self, _view: &TableNamePair) -> Result<Vec<TableNamePair>, String> {
        Ok(Vec::new())
    }

    fn decode_binary_plan(&self, encoded: &str) -> Result<String, String> {
        self.decoded.lock().unwrap().push(encoded.into());
        Ok(format!("\n{encoded}\n"))
    }

    fn dump_package(
        &self,
        _file_name: &str,
        _task: &ExtractTask,
        package: &ExtractPlanPackage,
    ) -> Result<(), String> {
        self.dumped.lock().unwrap().push(package.clone());
        Ok(())
    }

    fn persistent_statement_summary_enabled(&self) -> bool {
        self.persistent
    }
}

fn record(digest: &str, plan_digest: &str, sql: &str, binary_plan: &str) -> StatementRecord {
    StatementRecord {
        statement_type: "Select".into(),
        schema_name: "test".into(),
        tables: vec![TableNamePair {
            database: "test".into(),
            table: digest.into(),
            is_view: false,
        }],
        digest: digest.into(),
        plan_digest: plan_digest.into(),
        sql: sql.into(),
        binary_plan: binary_plan.into(),
        user_name: "root".into(),
        decoded_plan: String::new(),
        skipped: false,
    }
}

#[test]
fn canonical_extract_plan_task_and_record_validation_keep_go_filters() {
    let task = ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH + Duration::from_secs(1));
    assert_eq!(task.extract_type, ExtractType::Plan);
    assert!(!task.is_background_job);
    assert!(!task.skip_stats);
    assert!(!task.use_history_view);
    let mut record = record("sql", "plan", "select 1", "encoded");
    assert!(record.is_valid());
    // 非 Select（如 Update）应被过滤。
    record.statement_type = "Update".into();
    assert!(!record.is_valid());
    record.statement_type = "Select".into();
    record.schema_name.clear();
    assert!(!record.is_valid());
    record.schema_name = "test".into();
    record.plan_digest.clear();
    assert!(!record.is_valid());
}

#[test]
fn truncated_and_overwritten_records_follow_go_packaging_order() {
    let source = Arc::new(MockSource::default());
    *source.records.lock().unwrap() = vec![
        record("truncated", "p1", "select ...(len: 100)", "unused"),
        record("duplicate", "p2", "select old", "old-plan"),
        record("duplicate", "p2", "select new", "new-plan"),
    ];
    let handle = ExtractHandle::new(source.clone());
    handle
        .extract_task(&ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH))
        .unwrap();

    assert_eq!(&*source.decoded.lock().unwrap(), &["new-plan"]);
    let dumped = source.dumped.lock().unwrap();
    let package = dumped.last().unwrap();
    assert!(package.records.values().any(|r| r.skipped));
    assert!(!package.tables.iter().any(|t| t.table == "truncated"));
    assert!(package.tables.iter().any(|t| t.table == "duplicate"));
}

#[test]
fn reversed_window_is_forwarded_like_go_instead_of_rejected_locally() {
    let source = Arc::new(MockSource::default());
    let handle = ExtractHandle::new(source.clone());
    let task = ExtractTask::new_plan(UNIX_EPOCH + Duration::from_secs(1), UNIX_EPOCH);
    assert!(handle.extract_task(&task).is_ok());
    assert_eq!(source.dumped.lock().unwrap().len(), 1);
}

#[test]
fn history_view_requires_persistent_statement_summary() {
    let source = Arc::new(MockSource::default());
    let handle = ExtractHandle::new(source);
    let mut task = ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH);
    task.use_history_view = true;
    assert_eq!(
        handle.extract_task(&task).unwrap_err(),
        "tidb_stmt_summary_enable_persistent should be enabled for extract task"
    );
}
