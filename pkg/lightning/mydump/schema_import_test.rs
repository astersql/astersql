// Copyright 2024 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// SchemaImporter 与 createIfNotExistsStmt 单元测试。
//
// 验证建库/建表/视图依赖顺序、导入计划拓扑、Loader 延迟校验视图、
// 大规模表导入计数，以及 IF NOT EXISTS 改写与未闭合引号错误。
use crate::test_support::{MemoryStorage, file};
use crate::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Default)]
/// 记录 execute 调用并支持预设 query 结果的测试双。
struct RecordingDatabase {
    executed: Mutex<Vec<String>>,
    execute_errors: Mutex<HashMap<String, String>>,
    queried: Mutex<Vec<String>>,
    query_errors: Mutex<HashMap<String, String>>,
    query_results: Mutex<HashMap<String, Vec<Vec<String>>>>,
}

impl RecordingDatabase {
    /// 返回已执行 SQL 列表副本。
    fn executions(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }

    /// Return the queries issued by the importer in call order.
    fn queries(&self) -> Vec<String> {
        self.queried.lock().unwrap().clone()
    }
}

impl SchemaDatabase for RecordingDatabase {
    fn execute(&self, sql: &str) -> Result<(), MydumpError> {
        self.executed.lock().unwrap().push(sql.to_owned());
        if let Some(message) = self.execute_errors.lock().unwrap().get(sql) {
            return Err(MydumpError::Schema(message.clone()));
        }
        Ok(())
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, MydumpError> {
        self.queried.lock().unwrap().push(sql.to_owned());
        if let Some(message) = self.query_errors.lock().unwrap().get(sql) {
            return Err(MydumpError::Schema(message.clone()));
        }
        Ok(self
            .query_results
            .lock()
            .unwrap()
            .get(sql)
            .cloned()
            .unwrap_or_default())
    }
}

#[test]
/// A CREATE TABLE execution error is ignored when the target now exists downstream.
fn create_table_execution_error_is_ignored_when_table_exists() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    let create = "CREATE TABLE IF NOT EXISTS `test`.`t`(`id` INT);";
    database
        .execute_errors
        .lock()
        .unwrap()
        .insert(create.into(), "unsupported collation".into());
    database.query_results.lock().unwrap().insert(
        "SHOW TABLES FROM `test` LIKE 't'".into(),
        vec![vec!["t".into()]],
    );
    let importer = NewSchemaImporter(database.clone(), storage, 1);
    let job = SchemaJob {
        db_name: "test".into(),
        tbl_name: "t".into(),
        stmt_type: SchemaStmtType::SchemaCreateTable,
        sql_str: "CREATE TABLE t(id INT);".into(),
    };

    importer.runCreateTableJob(&job).unwrap();
    assert_eq!(database.executions(), vec![create]);
    assert_eq!(database.queries(), vec!["SHOW TABLES FROM `test` LIKE 't'"]);
}

#[test]
/// A CREATE TABLE execution error remains visible when the downstream table is absent.
fn create_table_execution_error_is_returned_when_table_is_missing() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    let create = "CREATE TABLE IF NOT EXISTS `test`.`t`(`id` INT);";
    database
        .execute_errors
        .lock()
        .unwrap()
        .insert(create.into(), "create table error".into());
    let importer = NewSchemaImporter(database.clone(), storage, 1);
    let job = SchemaJob {
        db_name: "test".into(),
        tbl_name: "t".into(),
        stmt_type: SchemaStmtType::SchemaCreateTable,
        sql_str: "CREATE TABLE t(id INT);".into(),
    };

    let error = importer.runCreateTableJob(&job).unwrap_err();
    assert!(error.to_string().contains("create table error"));
    assert_eq!(database.executions(), vec![create]);
    assert_eq!(database.queries(), vec!["SHOW TABLES FROM `test` LIKE 't'"]);
}

#[test]
/// A downstream lookup error supersedes the CREATE TABLE execution error.
fn create_table_lookup_error_is_returned() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    let create = "CREATE TABLE IF NOT EXISTS `test`.`t`(`id` INT);";
    let lookup = "SHOW TABLES FROM `test` LIKE 't'";
    database
        .execute_errors
        .lock()
        .unwrap()
        .insert(create.into(), "create table error".into());
    database
        .query_errors
        .lock()
        .unwrap()
        .insert(lookup.into(), "lookup error".into());
    let importer = NewSchemaImporter(database.clone(), storage, 1);
    let job = SchemaJob {
        db_name: "test".into(),
        tbl_name: "t".into(),
        stmt_type: SchemaStmtType::SchemaCreateTable,
        sql_str: "CREATE TABLE t(id INT);".into(),
    };

    let error = importer.runCreateTableJob(&job).unwrap_err();
    assert!(error.to_string().contains("lookup error"));
    assert!(!error.to_string().contains("create table error"));
    assert_eq!(database.queries(), vec![lookup]);
}

#[test]
/// Errors from statements after CREATE TABLE must remain visible and must not trigger a table lookup.
fn later_statement_execution_error_is_not_ignored() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    database
        .execute_errors
        .lock()
        .unwrap()
        .insert("SET @a = 1;".into(), "later statement error".into());
    let importer = NewSchemaImporter(database.clone(), storage, 1);
    let job = SchemaJob {
        db_name: "test".into(),
        tbl_name: "t".into(),
        stmt_type: SchemaStmtType::SchemaCreateTable,
        sql_str: "CREATE TABLE t(id INT); SET @a = 1;".into(),
    };

    let error = importer.runCreateTableJob(&job).unwrap_err();
    assert!(error.to_string().contains("later statement error"));
    assert_eq!(database.queries(), Vec::<String>::new());
}

/// 构造指向 TableSchema 文件的表元数据。
fn table(db: &str, name: &str, path: &str) -> MDTableMeta {
    let mut table = NewMDTableMeta("auto");
    table.db = db.into();
    table.name = name.into();
    table.schema_file = file(path, SourceType::TableSchema);
    table
}

/// 构造指向 ViewSchema 文件的视图元数据。
fn view(db: &str, name: &str, path: &str) -> MDTableMeta {
    let mut view = NewMDTableMeta("auto");
    view.db = db.into();
    view.name = name.into();
    view.schema_file = file(path, SourceType::ViewSchema);
    view
}

#[test]
/// 端到端：建库 → IF NOT EXISTS 建表 → 按依赖创建 v1 再 v2。
fn TestSchemaImporter() {
    let storage = Arc::new(MemoryStorage::with(&[
        ("t.sql", &b"CREATE TABLE t(id INT);"[..]),
        ("v1.sql", b"CREATE VIEW v1 AS SELECT id FROM t;"),
        ("v2.sql", b"CREATE VIEW v2 AS SELECT id FROM v1;"),
    ]));
    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'test'"
            .into(),
        vec![vec!["t".into(), "BASE TABLE".into()]],
    );
    let importer = NewSchemaImporter(database.clone(), storage, 4);
    let db = MDDatabaseMeta {
        name: "test".into(),
        tables: vec![table("test", "t", "t.sql")],
        views: vec![view("test", "v2", "v2.sql"), view("test", "v1", "v1.sql")],
        ..NewMDDatabaseMeta("auto")
    };
    importer.Run(&[db]).unwrap();
    let executed = database.executions();
    assert!(executed[0].starts_with("CREATE DATABASE"));
    assert!(executed[1].starts_with("CREATE TABLE IF NOT EXISTS"));
    assert!(executed[2].contains("VIEW `test`.`v1`"));
    assert!(executed[3].contains("VIEW `test`.`v2`"));
}

#[test]
/// 导入计划中视图应按依赖拓扑排序（v1 先于 v2）。
fn TestNewSchemaImportPlan() {
    let storage = MemoryStorage::with(&[
        ("v1.sql", b"CREATE VIEW v1 AS SELECT id FROM t;"),
        ("v2.sql", b"CREATE VIEW v2 AS SELECT id FROM v1;"),
    ]);
    let db = MDDatabaseMeta {
        name: "test".into(),
        tables: vec![table("test", "t", "unused.sql")],
        views: vec![view("test", "v2", "v2.sql"), view("test", "v1", "v1.sql")],
        ..NewMDDatabaseMeta("auto")
    };
    let plan = NewSchemaImportPlan(&storage, &[db]).unwrap();
    assert_eq!(
        plan.view_plan.unwrap().ordered,
        vec![tableKey("test", "v1"), tableKey("test", "v2")]
    );
}

#[test]
/// Loader 可先加载空视图文件；Run 时才因缺 CREATE VIEW 失败。
fn TestLoaderSetupDefersViewSchemaValidationUntilRun() {
    let storage = Arc::new(MemoryStorage::with(&[("db.v1-schema-view.sql", b"")]));
    let loader = NewLoaderWithStore(
        LoaderConfig {
            filter: vec!["*.*".into()],
            ..Default::default()
        },
        storage.clone(),
        vec![],
    )
    .unwrap();
    assert_eq!(loader.GetDatabases().len(), 1);
    let importer = NewSchemaImporter(Arc::new(RecordingDatabase::default()), storage, 1);
    let error = importer.Run(loader.GetDatabases()).unwrap_err();
    assert!(error.to_string().contains("`db`.`v1`"));
}

#[test]
/// 30 库 × 50 表时执行次数应为 30 + 1500。
fn TestSchemaImporterManyTables() {
    let storage = Arc::new(MemoryStorage::default());
    let mut databases = Vec::new();
    for database_index in 0..30 {
        let database_name = format!("test{database_index:02}");
        let mut database = NewMDDatabaseMeta("auto");
        database.name = database_name.clone();
        for table_index in 0..50 {
            let table_name = format!("t{table_index:03}");
            let path = format!("{database_name}.{table_name}-schema.sql");
            storage.insert(
                &path,
                format!("CREATE TABLE {table_name}(id INT);").into_bytes(),
            );
            database
                .tables
                .push(table(&database_name, &table_name, &path));
        }
        databases.push(database);
    }
    let database = Arc::new(RecordingDatabase::default());
    NewSchemaImporter(database.clone(), storage, 8)
        .Run(&databases)
        .unwrap();
    assert_eq!(database.executions().len(), 30 + 30 * 50);
}

#[test]
/// IF NOT EXISTS 改写、多语句拆分与未闭合引号错误。
fn TestCreateTableIfNotExistsStmt() {
    assert_eq!(
        createIfNotExistsStmt("CREATE TABLE `foo`(`bar` TINYINT(1));", "testdb", "foo").unwrap(),
        vec!["CREATE TABLE IF NOT EXISTS `testdb`.`foo`(`bar` TINYINT(1));"]
    );
    assert_eq!(
        createIfNotExistsStmt(
            "CREATE TABLE IF NOT EXISTS `foo`(`bar` TINYINT(1));",
            "testdb",
            "foo"
        )
        .unwrap(),
        vec!["CREATE TABLE IF NOT EXISTS `testdb`.`foo`(`bar` TINYINT(1));"]
    );
    let statements = createIfNotExistsStmt(
        "SET NAMES 'binary'; CREATE TABLE foo(note VARCHAR(20) COMMENT 'CREATE TABLE');",
        "testdb",
        "foo",
    )
    .unwrap();
    assert_eq!(statements.len(), 2);
    assert_eq!(statements[0], "SET NAMES 'binary';");
    assert_eq!(
        statements[1],
        "CREATE TABLE IF NOT EXISTS `testdb`.`foo`(`note` VARCHAR(20) COMMENT 'CREATE TABLE');"
    );
    assert_eq!(
        createIfNotExistsStmt(
            "CREATE TABLE db1.tb1 (a INT, b VARCHAR(10), PRIMARY KEY (a));",
            "db1",
            "tb1"
        )
        .unwrap(),
        vec!["CREATE TABLE IF NOT EXISTS `db1`.`tb1` (`a` INT,`b` VARCHAR(10),PRIMARY KEY (a));"]
    );
    assert!(
        createIfNotExistsStmt("CREATE TABLE foo(note VARCHAR(20) COMMENT 'x);", "d", "t").is_err()
    );
    assert!(createIfNotExistsStmt("xxxx;", "d", "t").is_err());
}

#[test]
/// Go parity: downstream databases are queried case-insensitively and skipped.
fn existing_database_is_not_recreated() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA".into(),
        vec![vec!["TeSt".into()]],
    );
    let mut db = NewMDDatabaseMeta("auto");
    db.name = "test".into();

    NewSchemaImporter(database.clone(), storage, 1)
        .Run(&[db])
        .unwrap();

    assert!(database.executions().is_empty());
}

#[test]
/// Go parity: a successful SHOW CREATE with zero rows is still considered existing.
fn schema_less_table_is_skipped_when_show_create_succeeds() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA".into(),
        vec![vec!["test".into()]],
    );
    let mut db = NewMDDatabaseMeta("auto");
    db.name = "test".into();
    db.tables.push(table("test", "t", ""));

    NewSchemaImporter(database.clone(), storage, 1)
        .Run(&[db])
        .unwrap();
    assert!(database.executions().is_empty());
}

#[test]
/// Existing schema helpers normalize the first column exactly like Go.
fn existing_schemas_are_lowercased_and_only_use_first_column() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "custom".into(),
        vec![vec!["MiXeD".into(), "ignored".into()]],
    );
    let importer = NewSchemaImporter(database, storage, 1);
    assert_eq!(
        importer.getExistingSchemas("custom").unwrap(),
        HashSet::from(["mixed".to_owned()])
    );
}

#[test]
/// Go runJob executes the parsed statement list only; an empty list is a no-op.
fn empty_statement_list_does_not_execute_raw_sql() {
    let storage = Arc::new(MemoryStorage::default());
    let database = Arc::new(RecordingDatabase::default());
    let importer = NewSchemaImporter(database.clone(), storage, 1);
    importer
        .runJob(
            &SchemaJob {
                db_name: "test".into(),
                tbl_name: "t".into(),
                stmt_type: SchemaStmtType::SchemaCreateTable,
                sql_str: "must not run".into(),
            },
            &[],
        )
        .unwrap();
    assert!(database.executions().is_empty());
}

#[test]
/// Existing downstream views are valid dependencies and are not recreated.
fn existing_view_is_skipped() {
    let storage = Arc::new(MemoryStorage::with(&[(
        "v.sql",
        &b"CREATE VIEW v AS SELECT id FROM t;"[..],
    )]));
    let db = MDDatabaseMeta {
        name: "test".into(),
        tables: vec![table("test", "t", "unused.sql")],
        views: vec![view("test", "v", "v.sql")],
        ..NewMDDatabaseMeta("auto")
    };
    let plan = NewSchemaImportPlan(storage.as_ref(), &[db]).unwrap();
    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'test'"
            .into(),
        vec![
            vec!["t".into(), "BASE TABLE".into()],
            vec!["V".into(), "VIEW".into()],
        ],
    );
    let importer = NewSchemaImporter(database.clone(), storage, 1);
    importer.importViews(&plan).unwrap();
    assert!(database.executions().is_empty());
}

#[test]
/// A downstream non-view object with the target view name is a hard conflict.
fn existing_table_blocks_view_creation() {
    let storage = Arc::new(MemoryStorage::with(&[(
        "v.sql",
        &b"CREATE VIEW v AS SELECT id FROM t;"[..],
    )]));
    let db = MDDatabaseMeta {
        name: "test".into(),
        tables: vec![table("test", "t", "unused.sql")],
        views: vec![view("test", "v", "v.sql")],
        ..NewMDDatabaseMeta("auto")
    };
    let plan = NewSchemaImportPlan(storage.as_ref(), &[db]).unwrap();
    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'test'"
            .into(),
        vec![
            vec!["t".into(), "BASE TABLE".into()],
            vec!["v".into(), "BASE TABLE".into()],
        ],
    );
    let importer = NewSchemaImporter(database, storage, 1);
    assert!(
        importer
            .importViews(&plan)
            .unwrap_err()
            .to_string()
            .contains("non-view object")
    );
}

#[test]
/// 剪除视图占位表后，仍按依赖顺序导入视图。
fn TestSchemaImporterImportsViewsInDependencyOrderAfterPlaceholderPrune() {
    let storage = Arc::new(MemoryStorage::with(&[
        ("t.sql", &b"CREATE TABLE t(id INT);"[..]),
        ("v1.sql", b"CREATE VIEW v1 AS SELECT id FROM t;"),
        ("v2.sql", b"CREATE VIEW v2 AS SELECT id FROM v1;"),
    ]));
    let mut db = NewMDDatabaseMeta("auto");
    db.name = "test".into();
    db.tables = vec![
        table("test", "t", "t.sql"),
        table("test", "v1", "unused-v1.sql"),
        table("test", "v2", "unused-v2.sql"),
    ];
    db.views = vec![view("test", "v2", "v2.sql"), view("test", "v1", "v1.sql")];
    assert_eq!(pruneViewPlaceholders(std::slice::from_mut(&mut db)), 2);
    assert_eq!(db.tables.len(), 1);

    let database = Arc::new(RecordingDatabase::default());
    database.query_results.lock().unwrap().insert(
        "SELECT TABLE_NAME, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = 'test'"
            .into(),
        vec![vec!["t".into(), "BASE TABLE".into()]],
    );
    NewSchemaImporter(database.clone(), storage, 1)
        .Run(&[db])
        .unwrap();
    let executed = database.executions();
    let v1 = executed
        .iter()
        .position(|sql| sql.contains("VIEW `test`.`v1`"))
        .unwrap();
    let v2 = executed
        .iter()
        .position(|sql| sql.contains("VIEW `test`.`v2`"))
        .unwrap();
    assert!(v1 < v2);
}
