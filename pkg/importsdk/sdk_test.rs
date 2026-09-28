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

// ImportSDK 集成风格测试。
//
// 因缺少 fake-GCS 与 sqlmock，改用本地 `file://` 与内存 `JobDatabase` 模拟，
// 覆盖 Dumpling/CSV 源发现、仅数据文件路由、扫描上限与按名建表等 Go 子测意图。

// NOTE: Go's sdk_test.go drives every subtest through a real
// `fake-gcs-server` HTTP server (`github.com/fsouza/fake-gcs-server`) plus
// `go-sqlmock`. Neither a fake GCS server nor a `gs://` storage backend is
// available to this crate (there is no such Rust dependency wired into
// `astersql-objstore`, and adding one is outside this task's writable
// files), so these tests exercise the same `ImportSDK` surface against local
// `file://` storage plus the same in-process `JobDatabase` mock style used in
// `job_manager_test.rs` / `file_scanner_test.rs`. This preserves the intent
// of every Go subtest (schema creation + table discovery + size accounting
// for SQL and CSV sources, file-router-only tables, scan limits with skipped
// invalid files, and single-table creation) without depending on GCS.
//
// "TestOnlyDataFiles" also uses the Go `SHOW CREATE TABLE`-based
// already-exists fallback for tables that have no schema file.

use crate::{
    FileRouteRule, FileScanner, ImportSDK, JobDatabase, JobRows, NewImportSDK, SQLValue,
    WithCharset, WithConcurrency, WithFileRouters, WithFilter, WithMaxScanFiles, WithSQLMode,
    WithSkipInvalidFiles,
};
use astersql_errors as errors;
use astersql_lightning_mydump as mydump;
use astersql_parser_mysql as mysql;
use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 预置查询结果行的简易 `JobRows` 实现（本文件查询路径通常返回空）。
struct CanonicalRows {
    rows: VecDeque<Vec<SQLValue>>,
}

impl JobRows for CanonicalRows {
    fn Next(&mut self) -> Result<Option<Vec<SQLValue>>, errors::SharedError> {
        Ok(self.rows.pop_front())
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        Ok(())
    }
}

/// Records every statement issued through the SDK's `JobDatabase` and never
/// fails, matching the "no errors configured" defaults from Go's sqlmock in
/// the happy-path subtests below.
#[derive(Default)]
/// 记录所有 Exec 语句且永不失败的假数据库，对应 Go sqlmock 无错误默认路径。
struct CanonicalDatabase {
    executions: Mutex<Vec<String>>,
}

impl JobDatabase for CanonicalDatabase {
    fn QueryContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        _query: &str,
    ) -> Result<Box<dyn JobRows>, errors::SharedError> {
        Ok(Box::new(CanonicalRows {
            rows: VecDeque::new(),
        }))
    }

    fn ExecContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<(), errors::SharedError> {
        self.executions.lock().unwrap().push(query.to_owned());
        Ok(())
    }
}

/// 任何查询/执行都应失败的占位数据库，用于构造失败路径（如非法 source）。
struct UnusedDatabase;

impl JobDatabase for UnusedDatabase {
    fn QueryContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        _query: &str,
    ) -> Result<Box<dyn JobRows>, errors::SharedError> {
        Err(errors::New("unexpected query"))
    }

    fn ExecContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        _query: &str,
    ) -> Result<(), errors::SharedError> {
        Err(errors::New("unexpected exec"))
    }
}

/// 向临时目录写入测试文件，失败则 panic。
fn write_file(dir: &std::path::Path, name: &str, contents: &[u8]) {
    std::fs::write(dir.join(name), contents)
        .unwrap_or_else(|error| panic!("write {name}: {error}"));
}

/// 以 `file://` 路径构造 ImportSDK，失败则断言失败。
fn new_sdk(
    dir: &std::path::Path,
    db: Arc<dyn JobDatabase>,
    options: Vec<crate::SDKOption>,
) -> ImportSDK {
    NewImportSDK(&(), &format!("file://{}", dir.display()), db, options)
        .expect("NewImportSDK should succeed")
}

#[test]
/// 非法 source URL 应失败，且错误信息不得泄露密钥类查询参数。
fn canonical_sdk_rejects_invalid_source_without_leaking_credentials() {
    let error = match NewImportSDK(
        &(),
        "1invalid:?secret-access-key=sk",
        Arc::new(UnusedDatabase),
        Vec::new(),
    ) {
        Ok(_) => panic!("invalid source must fail"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("<redacted-invalid-source>"));
    assert!(!message.contains("secret-access-key=sk"));
}

/// Mirrors Go's `TestDumplingSource`: two databases, each with a single
/// mydumper-style SQL table (schema + two `.sql` insert files), are created
/// and then discovered with the expected sizes and wildcard paths.
#[test]
fn sdk_dumpling_source_creates_and_discovers_two_tables() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(
        dir,
        "db1-schema-create.sql",
        b"CREATE DATABASE IF NOT EXISTS db1;\n",
    );
    write_file(
        dir,
        "db2-schema-create.sql",
        b"CREATE DATABASE IF NOT EXISTS db2;\n",
    );
    write_file(
        dir,
        "db1.tb1-schema.sql",
        b"CREATE TABLE IF NOT EXISTS db1.tb1 (a INT, b VARCHAR(10));\n",
    );
    write_file(
        dir,
        "db1.tb1.001.sql",
        b"INSERT INTO db1.tb1 VALUES (1,'a'),(2,'b');\n",
    );
    write_file(
        dir,
        "db1.tb1.002.sql",
        b"INSERT INTO db1.tb1 VALUES (3,'c'),(4,'d');\n",
    );
    write_file(
        dir,
        "db2.tb2-schema.sql",
        b"CREATE TABLE IF NOT EXISTS db2.tb2 (x INT, y VARCHAR(10));\n",
    );
    write_file(
        dir,
        "db2.tb2.001.sql",
        b"INSERT INTO db2.tb2 VALUES (5,'e'),(6,'f');\n",
    );
    write_file(
        dir,
        "db2.tb2.002.sql",
        b"INSERT INTO db2.tb2 VALUES (7,'g'),(8,'h');\n",
    );

    let db = Arc::new(CanonicalDatabase::default());
    let mut sdk = new_sdk(dir, db.clone(), vec![WithConcurrency(1)]);

    // 建库建表后校验 Exec 记录中出现目标库与 CREATE TABLE。
    sdk.CreateSchemasAndTables(&())
        .expect("CreateSchemasAndTables");
    {
        let executions = db.executions.lock().unwrap();
        assert!(executions.iter().any(|sql| sql.contains("db1")));
        assert!(executions.iter().any(|sql| sql.contains("db2")));
        assert!(
            executions
                .iter()
                .any(|sql| sql.to_ascii_uppercase().contains("CREATE TABLE"))
        );
    }

    let tables_meta = sdk.GetTableMetas(&()).expect("GetTableMetas");
    assert_eq!(2, tables_meta.len());
    assert_eq!("db1", tables_meta[0].Database);
    assert_eq!("tb1", tables_meta[0].Table);
    assert_eq!("db1.tb1-schema.sql", tables_meta[0].SchemaFile);
    assert_eq!(2, tables_meta[0].DataFiles.len());
    assert_eq!("db1.tb1.001.sql", tables_meta[0].DataFiles[0].Path);
    assert_eq!(44, tables_meta[0].DataFiles[0].Size);
    assert_eq!(mydump::SourceType::Sql, tables_meta[0].DataFiles[0].Format);
    assert_eq!(
        mydump::Compression::None,
        tables_meta[0].DataFiles[0].Compression
    );
    assert_eq!("db1.tb1.002.sql", tables_meta[0].DataFiles[1].Path);
    assert_eq!(44, tables_meta[0].DataFiles[1].Size);
    assert_eq!(88, tables_meta[0].TotalSize);
    assert!(tables_meta[0].WildcardPath.ends_with("/db1.tb1.*.sql"));
    assert_eq!("db2", tables_meta[1].Database);
    assert_eq!("tb2", tables_meta[1].Table);
    assert_eq!("db2.tb2-schema.sql", tables_meta[1].SchemaFile);
    assert_eq!(2, tables_meta[1].DataFiles.len());
    assert_eq!("db2.tb2.001.sql", tables_meta[1].DataFiles[0].Path);
    assert_eq!(44, tables_meta[1].DataFiles[0].Size);
    assert_eq!(mydump::SourceType::Sql, tables_meta[1].DataFiles[0].Format);
    assert_eq!(
        mydump::Compression::None,
        tables_meta[1].DataFiles[0].Compression
    );
    assert_eq!("db2.tb2.002.sql", tables_meta[1].DataFiles[1].Path);
    assert_eq!(44, tables_meta[1].DataFiles[1].Size);
    assert_eq!(88, tables_meta[1].TotalSize);
    assert!(tables_meta[1].WildcardPath.ends_with("/db2.tb2.*.sql"));

    let tm1 = sdk
        .GetTableMetaByName(&(), "db1", "tb1")
        .expect("GetTableMetaByName db1.tb1");
    assert_eq!(tables_meta[0].Database, tm1.Database);
    assert_eq!(tables_meta[0].Table, tm1.Table);
    assert_eq!(tables_meta[0].SchemaFile, tm1.SchemaFile);
    assert_eq!(tables_meta[0].TotalSize, tm1.TotalSize);
    assert_eq!(tables_meta[0].WildcardPath, tm1.WildcardPath);
    let tm2 = sdk
        .GetTableMetaByName(&(), "db2", "tb2")
        .expect("GetTableMetaByName db2.tb2");
    assert_eq!(tables_meta[1].Database, tm2.Database);
    assert_eq!(tables_meta[1].Table, tm2.Table);
    assert_eq!(tables_meta[1].SchemaFile, tm2.SchemaFile);
    assert_eq!(tables_meta[1].TotalSize, tm2.TotalSize);
    assert_eq!(tables_meta[1].WildcardPath, tm2.WildcardPath);

    assert_eq!(176, sdk.GetTotalSize(&()));
    sdk.Close().expect("Close");
}

/// Mirrors Go's `TestCSVSource`: same shape as the Dumpling scenario, but
/// with CSV data files instead of SQL insert statements.
#[test]
fn sdk_csv_source_creates_and_discovers_two_tables() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(
        dir,
        "db1-schema-create.sql",
        b"CREATE DATABASE IF NOT EXISTS db1;\n",
    );
    write_file(
        dir,
        "db2-schema-create.sql",
        b"CREATE DATABASE IF NOT EXISTS db2;\n",
    );
    write_file(
        dir,
        "db1.tb1-schema.sql",
        b"CREATE TABLE IF NOT EXISTS db1.tb1 (a INT, b VARCHAR(10));\n",
    );
    write_file(dir, "db1.tb1.001.csv", b"1,a\n2,b\n");
    write_file(dir, "db1.tb1.002.csv", b"3,c\n4,d\n");
    write_file(
        dir,
        "db2.tb2-schema.sql",
        b"CREATE TABLE IF NOT EXISTS db2.tb2 (x INT, y VARCHAR(10));\n",
    );
    write_file(dir, "db2.tb2.001.csv", b"5,e\n6,f\n");
    write_file(dir, "db2.tb2.002.csv", b"7,g\n8,h\n");

    let db = Arc::new(CanonicalDatabase::default());
    let mut sdk = new_sdk(dir, db.clone(), vec![WithConcurrency(1)]);

    sdk.CreateSchemasAndTables(&())
        .expect("CreateSchemasAndTables");

    let tables_meta = sdk.GetTableMetas(&()).expect("GetTableMetas");
    assert_eq!(2, tables_meta.len());
    assert_eq!("db1", tables_meta[0].Database);
    assert_eq!("tb1", tables_meta[0].Table);
    assert_eq!("db1.tb1-schema.sql", tables_meta[0].SchemaFile);
    assert_eq!(2, tables_meta[0].DataFiles.len());
    assert_eq!("db1.tb1.001.csv", tables_meta[0].DataFiles[0].Path);
    assert_eq!(8, tables_meta[0].DataFiles[0].Size);
    assert_eq!(mydump::SourceType::Csv, tables_meta[0].DataFiles[0].Format);
    assert_eq!(
        mydump::Compression::None,
        tables_meta[0].DataFiles[0].Compression
    );
    assert_eq!("db1.tb1.002.csv", tables_meta[0].DataFiles[1].Path);
    assert_eq!(8, tables_meta[0].DataFiles[1].Size);
    assert_eq!(16, tables_meta[0].TotalSize);
    assert!(tables_meta[0].WildcardPath.ends_with("/db1.tb1.*.csv"));
    assert_eq!("db2", tables_meta[1].Database);
    assert_eq!("tb2", tables_meta[1].Table);
    assert_eq!("db2.tb2-schema.sql", tables_meta[1].SchemaFile);
    assert_eq!(2, tables_meta[1].DataFiles.len());
    assert_eq!("db2.tb2.001.csv", tables_meta[1].DataFiles[0].Path);
    assert_eq!(8, tables_meta[1].DataFiles[0].Size);
    assert_eq!(mydump::SourceType::Csv, tables_meta[1].DataFiles[0].Format);
    assert_eq!(
        mydump::Compression::None,
        tables_meta[1].DataFiles[0].Compression
    );
    assert_eq!("db2.tb2.002.csv", tables_meta[1].DataFiles[1].Path);
    assert_eq!(8, tables_meta[1].DataFiles[1].Size);
    assert_eq!(16, tables_meta[1].TotalSize);
    assert!(tables_meta[1].WildcardPath.ends_with("/db2.tb2.*.csv"));

    assert_eq!(32, sdk.GetTotalSize(&()));
    sdk.Close().expect("Close");
}

/// Mirrors the scanning half of Go's `TestOnlyDataFiles`: a file-router rule
/// maps loose `*.csv` files (with no schema file at all) onto a single
/// table, and that table is still discoverable with the right size and
/// wildcard path. `CreateSchemasAndTables` is intentionally not exercised
/// here; see the module-level NOTE for why.
#[test]
fn sdk_only_data_files_are_discovered_via_file_router() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "part1.csv", b"a,b\n1,a\n2,b\n");
    write_file(dir, "part2.csv", b"a,b\n3,c\n4,d\n");

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut sdk = new_sdk(
        dir,
        db,
        vec![
            WithCharset("utf8".to_owned()),
            WithConcurrency(8),
            WithFilter(vec!["*.*".to_owned()]),
            WithSQLMode(mysql::r#const::ModeANSIQuotes),
            WithFileRouters(vec![FileRouteRule {
                pattern: r".*\.csv$".to_owned(),
                schema: "db".to_owned(),
                table: "tb".to_owned(),
                type_name: "csv".to_owned(),
                ..Default::default()
            }]),
        ],
    );

    sdk.CreateSchemasAndTables(&())
        .expect("existing routed table should not require a schema file");
    let tables_meta = sdk.GetTableMetas(&()).expect("GetTableMetas");
    assert_eq!(1, tables_meta.len());
    assert_eq!("db", tables_meta[0].Database);
    assert_eq!("tb", tables_meta[0].Table);
    assert_eq!("", tables_meta[0].SchemaFile);
    assert_eq!(2, tables_meta[0].DataFiles.len());
    assert_eq!("part1.csv", tables_meta[0].DataFiles[0].Path);
    assert_eq!(12, tables_meta[0].DataFiles[0].Size);
    assert_eq!(mydump::SourceType::Csv, tables_meta[0].DataFiles[0].Format);
    assert_eq!(
        mydump::Compression::None,
        tables_meta[0].DataFiles[0].Compression
    );
    assert_eq!("part2.csv", tables_meta[0].DataFiles[1].Path);
    assert_eq!(12, tables_meta[0].DataFiles[1].Size);
    assert_eq!(24, tables_meta[0].TotalSize);
    assert!(tables_meta[0].WildcardPath.ends_with("/part*.csv"));

    let table_meta = sdk
        .GetTableMetaByName(&(), "db", "tb")
        .expect("GetTableMetaByName");
    assert_eq!(tables_meta[0].WildcardPath, table_meta.WildcardPath);
    assert_eq!(24, sdk.GetTotalSize(&()));
    sdk.Close().expect("Close");
}

/// Mirrors Go's `TestScanLimitation`: `WithMaxScanFiles(1)` caps discovery to
/// a single data file, and `WithSkipInvalidFiles(true)` lets the scan finish
/// without erroring even though the cap makes the mydumper-style listing
/// look incomplete.
#[test]
fn sdk_scan_limitation_caps_discovered_files() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db2.tb2.001.csv", b"5,e\n6,f\n");
    write_file(dir, "db2.tb2.002.csv", b"7,g\n8,h\n");

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut sdk = new_sdk(
        dir,
        db,
        vec![
            WithCharset("utf8".to_owned()),
            WithConcurrency(8),
            WithFilter(vec!["*.*".to_owned()]),
            WithSQLMode(mysql::r#const::ModeANSIQuotes),
            WithSkipInvalidFiles(true),
            WithMaxScanFiles(1),
        ],
    );

    let metas = sdk.GetTableMetas(&()).expect("GetTableMetas");
    assert_eq!(1, metas.len());
    assert_eq!(1, metas[0].DataFiles.len());
    sdk.Close().expect("Close");
}

/// Mirrors Go's `TestCreateTableMetaByName`: with two databases of two
/// tables each, `CreateSchemaAndTableByName` only creates the requested
/// `db1.tb1` and leaves every other table untouched.
#[test]
fn sdk_create_schema_and_table_by_name_targets_single_table() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    for i in 0..2 {
        write_file(
            dir,
            &format!("db{i}-schema-create.sql"),
            format!("CREATE DATABASE IF NOT EXISTS db{i};\n").as_bytes(),
        );
        for j in 0..2 {
            let table_name = format!("db{i}.tb{j}");
            write_file(
                dir,
                &format!("{table_name}-schema.sql"),
                format!("CREATE TABLE IF NOT EXISTS db{i}.tb{j} (a INT, b VARCHAR(10));\n")
                    .as_bytes(),
            );
            write_file(
                dir,
                &format!("{table_name}.001.sql"),
                format!("INSERT INTO db{i}.tb{j} VALUES (1,'a'),(2,'b');\n").as_bytes(),
            );
            write_file(
                dir,
                &format!("{table_name}.002.sql"),
                format!("INSERT INTO db{i}.tb{j} VALUES (3,'c'),(4,'d');\n").as_bytes(),
            );
        }
    }

    let db = Arc::new(CanonicalDatabase::default());
    let mut sdk = new_sdk(dir, db.clone(), vec![WithConcurrency(1)]);

    sdk.CreateSchemaAndTableByName(&(), "db1", "tb1")
        .expect("CreateSchemaAndTableByName");
    let executions = db.executions.lock().unwrap();
    assert_eq!(2, executions.len());
    assert_eq!("CREATE DATABASE IF NOT EXISTS `db1`;", executions[0]);
    assert_eq!(
        "CREATE TABLE IF NOT EXISTS `db1`.`tb1` (`a` INT,`b` VARCHAR(10));",
        executions[1]
    );
    assert!(!executions.iter().any(|sql| sql.contains("tb0")));
    assert!(!executions.iter().any(|sql| sql.contains("db0")));
}
