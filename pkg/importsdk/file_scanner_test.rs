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

// 说明：Rust SchemaImporter SQL 序列与 Go sqlmock 不完全一致，故断言本 crate 真实行为。
// NOTE: Go's file_scanner_test.go drives schema creation through
// `github.com/DATA-DOG/go-sqlmock` and asserts on hand-written SQL strings
// produced by TiDB's mydump/executor packages (e.g. "IF NOT EXISTS" wrapping,
// `information_schema.SCHEMATA` lookups). The Rust production code instead
// abstracts the database behind the `JobDatabase`/`JobRows` traits (see
// job_manager.rs) and delegates schema creation to
// `astersql-lightning-mydump`'s `SchemaImporter`, which was ported in a
// separate task and issues a different (simpler) sequence of statements than
// the Go mocks assume. These tests use a self-contained in-process
// `CanonicalDatabase` mock and assert on the actual statements captured from
// the real Rust code path rather than pinning to the Go mock's exact SQL, so
// coverage tracks this crate's real behavior instead of upstream internals
// outside this task's writable files.
//
// `FileScanner` 的单元测试。
//
// 覆盖数据文件元数据拷贝与汇总、表发现、敏感路径脱敏、建库建表 SQL 捕获、
// 导入数据量估算（含真实 KV 采样）、压缩实大小与 skip_invalid_files 行为。
// 使用进程内 `CanonicalDatabase` 记录实际发出的 SQL，而非绑定 Go sqlmock 字面量。

use crate::{
    ErrTableNotFound, FileRouteRule, ImportOptions, JobDatabase, JobRows, NewFileScanner,
    NewSQLGenerator, SQLValue, TableMeta, WithEstimateRealSize, WithFileRouters,
    WithSkipInvalidFiles, buildWildcardPath, createDataFileMeta, defaultSDKConfig,
    encodeAuroraWildcardPath, processDataFiles,
};
use astersql_errors as errors;
use astersql_lightning_mydump as mydump;
use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 空结果集的 `JobRows` 实现（本文件查询通常不消费行内容）。
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

/// 仅记录 SQL、永不失败的最小 `JobDatabase` mock（对应 Go sqlmock 无错误配置默认）。
/// CanonicalDatabase is a minimal in-process `JobDatabase` mock that records
/// every statement it receives; it never fails, mirroring the "no errors
/// configured" defaults from Go's sqlmock in the happy-path subtests.
#[derive(Default)]
struct CanonicalDatabase {
    queries: Mutex<Vec<String>>,
    executions: Mutex<Vec<String>>,
}

impl JobDatabase for CanonicalDatabase {
    fn QueryContext(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<Box<dyn JobRows>, errors::SharedError> {
        self.queries.lock().unwrap().push(query.to_owned());
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

/// 向临时目录写入测试用 dump 文件。
fn write_file(dir: &std::path::Path, name: &str, contents: &[u8]) {
    std::fs::write(dir.join(name), contents)
        .unwrap_or_else(|error| panic!("write {name}: {error}"));
}

/// 对照 Go：单文件元数据拷贝与批量大小汇总。
/// Mirrors Go's `TestCreateDataFileMeta` and `TestProcessDataFiles`: copying a
/// single file's metadata and summing sizes across a batch both preserve
/// path/size/format/compression fields.
#[test]
fn canonical_data_file_metadata_and_totals_match_go() {
    let files = vec![
        mydump::FileInfo {
            file_meta: mydump::FileMeta {
                path: "s3://bucket/a.csv.gz".to_owned(),
                file_size: 456,
                real_size: 789,
                source_type: mydump::SourceType::Csv,
                compression: mydump::Compression::Gz,
                ..Default::default()
            },
            ..Default::default()
        },
        mydump::FileInfo {
            file_meta: mydump::FileMeta {
                path: "s3://bucket/b.csv".to_owned(),
                file_size: 20,
                source_type: mydump::SourceType::Csv,
                compression: mydump::Compression::None,
                ..Default::default()
            },
            ..Default::default()
        },
    ];

    let first = createDataFileMeta(&files[0]);
    assert_eq!("s3://bucket/a.csv.gz", first.Path);
    assert_eq!(789, first.Size);
    assert_eq!(mydump::SourceType::Csv, first.Format);
    assert_eq!(mydump::Compression::Gz, first.Compression);

    let (metas, total) = processDataFiles(&files);
    assert_eq!(2, metas.len());
    assert_eq!(809, total);
    assert_eq!("s3://bucket/b.csv", metas[1].Path);
}

#[test]
fn file_scanner_preserves_storage_scheme_in_import_sql() {
    let parameters =
        "region=cn-hangzhou&endpoint=https://oss-cn-hangzhou.aliyuncs.com&role-arn=test-role";
    for scheme in ["s3", "oss"] {
        let wildcard_path =
            buildWildcardPath(&format!("{scheme}://bucket/data/"), "db.tbl.*.csv", false).unwrap();
        assert_eq!(
            format!("{scheme}://bucket/data/db.tbl.*.csv"),
            wildcard_path
        );
        let sql = NewSQLGenerator()
            .GenerateImportSQL(
                &TableMeta {
                    Database: "db".to_owned(),
                    Table: "tbl".to_owned(),
                    WildcardPath: wildcard_path,
                    ..Default::default()
                },
                &ImportOptions {
                    Format: "csv".to_owned(),
                    ResourceParameters: parameters.to_owned(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            format!(
                "IMPORT INTO `db`.`tbl` FROM '{scheme}://bucket/data/db.tbl.*.csv?{parameters}' FORMAT 'csv'"
            ),
            sql
        );
    }
}

/// 对照 Go：总大小、表元数据列表、按名查找与未找到错误。
/// Mirrors Go's `TestFileScanner` "GetTotalSize" / "GetTableMetas" /
/// "GetTableMetaByName" subtests: a single mydumper-style table (schema +
/// one CSV data file) is discovered with the right size and metadata, and
/// an unknown table name is rejected with `ErrTableNotFound`.
#[test]
fn get_total_size_and_table_metas_match_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(dir, "db1.t1-schema.sql", b"CREATE TABLE t1 (id INT);");
    write_file(dir, "db1.t1.001.csv", b"1\n2");

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let cfg = defaultSDKConfig();
    let mut scanner = NewFileScanner(&(), &format!("file://{}", dir.display()), db, cfg)
        .expect("scanner should initialize");

    assert_eq!(3, scanner.GetTotalSize(&()));

    let metas = scanner.GetTableMetas(&()).expect("table metas should load");
    assert_eq!(1, metas.len());
    assert_eq!("db1", metas[0].Database);
    assert_eq!("t1", metas[0].Table);
    assert_eq!(3, metas[0].TotalSize);
    assert_eq!(1, metas[0].DataFiles.len());

    let meta = scanner
        .GetTableMetaByName(&(), "db1", "t1")
        .expect("t1 should exist");
    assert_eq!("db1", meta.Database);
    assert_eq!("t1", meta.Table);

    let error = scanner
        .GetTableMetaByName(&(), "db1", "nonexistent")
        .unwrap_err();
    assert!(errors::ErrorEqual(Some(&error), Some(&ErrTableNotFound)));

    scanner.Close().expect("close should succeed");
}

/// Native Aurora snapshot paths are recognized automatically when callers do
/// not provide explicit file routes.
#[test]
fn aurora_snapshot_paths_are_automatically_mapped() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let path = tmp_dir
        .path()
        .join("export/sales.v1/sales.v1.order.items/1/part-a.parquet");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create directories");
    std::fs::write(&path, b"data").expect("write parquet placeholder");

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut cfg = defaultSDKConfig();
    WithEstimateRealSize(false)(&mut cfg);
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", tmp_dir.path().display()),
        db,
        cfg,
    )
    .expect("scanner should initialize");

    let metas = scanner.GetTableMetas(&()).expect("metadata should load");
    assert_eq!(1, metas.len());
    assert_eq!("sales.v1", metas[0].Database);
    assert_eq!("order.items", metas[0].Table);
}

#[test]
fn aurora_source_never_skips_missing_schema_during_estimation() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let path = tmp_dir.path().join("export/db/db.users/1/part-a.parquet");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create directories");
    std::fs::write(&path, b"data").expect("write parquet placeholder");

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut cfg = defaultSDKConfig();
    WithEstimateRealSize(false)(&mut cfg);
    WithSkipInvalidFiles(true)(&mut cfg);
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", tmp_dir.path().display()),
        db,
        cfg,
    )
    .expect("scanner should initialize");
    let error = scanner
        .EstimateImportDataSize(&())
        .expect_err("Aurora source must not hide a missing schema");
    assert!(error.to_string().contains("schema not found"), "{error}");
}

#[test]
fn aurora_remote_wildcard_uri_preserves_raw_percent_sequences() {
    let encoded = encodeAuroraWildcardPath(
        "s3://bucket/prefix%2E/export/db/db.order%2Eitems/*/part-*.parquet",
    )
    .expect("URI should encode");
    let parsed = url::Url::parse(&encoded).expect("encoded URI");
    assert!(parsed.path().contains("/prefix%252E/"), "{encoded}");
    assert!(parsed.path().contains("db.order%252Eitems"), "{encoded}");
}

/// 对照 Go：解析错误中脱敏或隐藏含凭证的源路径。
/// Mirrors Go's `TestFileScanner` "NewFileScannerRedactsSensitiveSourcePathInParseErrors"
/// and "NewFileScannerHidesMalformedSensitiveSourcePathInParseErrors" subtests:
/// a well-formed S3 URL with credentials must have those credentials redacted
/// (not dropped) in the parse error, while a malformed source string must not
/// echo its raw contents at all.
#[test]
fn new_file_scanner_redacts_or_hides_source_in_parse_errors() {
    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let cfg = defaultSDKConfig();

    let error = NewFileScanner(
        &(),
        "s3://?access-key=ak&secret-access-key=sk&session-token=token",
        db.clone(),
        cfg.clone(),
    )
    .err()
    .expect("s3 source without a store backend must fail to construct");
    let message = error.to_string();
    assert!(message.contains("access-key=xxxxxx"));
    assert!(message.contains("secret-access-key=xxxxxx"));
    assert!(message.contains("session-token=xxxxxx"));
    assert!(!message.contains("access-key=ak"));
    assert!(!message.contains("secret-access-key=sk"));
    assert!(!message.contains("session-token=token"));

    let error = NewFileScanner(&(), "1invalid:?secret-access-key=sk", db, cfg)
        .err()
        .expect("malformed source must fail to construct");
    let message = error.to_string();
    assert!(message.contains("source=<redacted-invalid-source>"));
    assert!(!message.contains("secret-access-key=sk"));
}

/// 对照 Go：建库建表发出 CREATE DATABASE/TABLE，按名创建与未找到。
/// Mirrors Go's `TestFileScanner` "CreateSchemasAndTables" and
/// "CreateSchemaAndTableByName" subtests: schema creation issues at least one
/// `CREATE DATABASE`-ish and one `CREATE TABLE`-ish statement for a
/// discovered table, targeting a single named table works the same way, and
/// an unknown table name is rejected.
#[test]
fn create_schemas_and_tables_matches_go_intent() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(dir, "db1.t1-schema.sql", b"CREATE TABLE t1 (id INT);");
    write_file(dir, "db1.t1.001.csv", b"1\n2");

    let db = Arc::new(CanonicalDatabase::default());
    let cfg = defaultSDKConfig();
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", dir.display()),
        db.clone() as Arc<dyn JobDatabase>,
        cfg.clone(),
    )
    .expect("scanner should initialize");

    scanner
        .CreateSchemasAndTables(&())
        .expect("schema creation should succeed");
    {
        let executions = db.executions.lock().unwrap();
        assert!(
            executions
                .iter()
                .any(|sql| sql.to_ascii_uppercase().contains("CREATE DATABASE")),
            "expected a CREATE DATABASE statement, got {executions:?}"
        );
        assert!(
            executions
                .iter()
                .any(|sql| sql.to_ascii_uppercase().contains("CREATE TABLE")),
            "expected a CREATE TABLE statement, got {executions:?}"
        );
    }

    scanner
        .CreateSchemaAndTableByName(&(), "db1", "t1")
        .expect("selected table should create");
    let error = scanner
        .CreateSchemaAndTableByName(&(), "db1", "nonexistent")
        .unwrap_err();
    assert!(errors::ErrorEqual(Some(&error), Some(&ErrTableNotFound)));
}

/// 对照 Go：schema 文件中的 DROP TABLE 不应被执行。
/// Mirrors Go's `TestFileScanner` "CreateSchemasAndTablesIgnoresDropTableInSchemaFile".
#[test]
fn create_schemas_and_tables_ignores_drop_table_in_schema_file() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(
        dir,
        "db1.t_drop-schema.sql",
        b"DROP TABLE t_drop; CREATE TABLE t_drop (id INT);",
    );

    let db = Arc::new(CanonicalDatabase::default());
    let cfg = defaultSDKConfig();
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", dir.display()),
        db.clone() as Arc<dyn JobDatabase>,
        cfg,
    )
    .expect("scanner should initialize");

    scanner
        .CreateSchemasAndTables(&())
        .expect("drop-prefixed schema should still succeed");
    let executions = db.executions.lock().unwrap();
    assert!(
        !executions
            .iter()
            .any(|sql| sql.to_ascii_uppercase().contains("DROP TABLE")),
        "DROP TABLE must be filtered before execution, got {executions:?}"
    );
    assert!(
        executions
            .iter()
            .any(|sql| sql.to_ascii_uppercase().contains("CREATE TABLE")),
        "expected a CREATE TABLE statement, got {executions:?}"
    );
}

/// 对照 Go `EstimateImportDataSize`：真实解析 SQL 数据并比较索引 KV 增量。
#[test]
fn estimate_import_data_size_matches_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(
        dir,
        "db1.no_idx-schema.sql",
        b"CREATE TABLE db1.no_idx (id INT PRIMARY KEY, k INT, v VARCHAR(255));",
    );
    write_file(
        dir,
        "db1.no_idx.001.sql",
        b"INSERT INTO db1.no_idx VALUES (1, 1, 'aaaaaaaaaaaaaaaa');\nINSERT INTO db1.no_idx VALUES (2, 2, 'bbbbbbbbbbbbbbbb');\n",
    );
    write_file(
        dir,
        "db1.with_idx-schema.sql",
        b"CREATE TABLE db1.with_idx (id INT PRIMARY KEY, k INT, v VARCHAR(255), KEY idx_k (k), KEY idx_v (v));",
    );
    write_file(
        dir,
        "db1.with_idx.001.sql",
        b"INSERT INTO db1.with_idx VALUES (1, 1, 'aaaaaaaaaaaaaaaa');\nINSERT INTO db1.with_idx VALUES (2, 2, 'bbbbbbbbbbbbbbbb');\n",
    );

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", dir.display()),
        db,
        defaultSDKConfig(),
    )
    .expect("scanner should initialize");

    let estimate = scanner
        .EstimateImportDataSize(&())
        .expect("real KV sampling should succeed");
    assert_eq!(2, estimate.Tables.len());
    let no_idx = estimate
        .Tables
        .iter()
        .find(|table| table.Table == "no_idx")
        .expect("no_idx estimate");
    let with_idx = estimate
        .Tables
        .iter()
        .find(|table| table.Table == "with_idx")
        .expect("with_idx estimate");
    assert!(no_idx.TiKVSize > 0);
    assert!(with_idx.TiKVSize > 0);
    assert!(with_idx.TiKVSize > no_idx.TiKVSize);
    assert_eq!(
        estimate.TotalSourceSize,
        no_idx.SourceSize + with_idx.SourceSize
    );
    assert_eq!(estimate.TotalTiKVSize, no_idx.TiKVSize + with_idx.TiKVSize);
}

/// 对照 Go `EstimateImportDataSizeCSV`：表头被忽略，空文件为零，有数据文件有正值。
#[test]
fn estimate_import_data_size_csv_matches_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(
        dir,
        "db1.empty_csv-schema.sql",
        b"CREATE TABLE db1.empty_csv (id INT PRIMARY KEY, v VARCHAR(255));",
    );
    write_file(dir, "db1.empty_csv.001.csv", b"id,v\n");
    write_file(
        dir,
        "db1.with_csv-schema.sql",
        b"CREATE TABLE db1.with_csv (id INT PRIMARY KEY, v VARCHAR(255), KEY idx_v (v));",
    );
    write_file(dir, "db1.with_csv.001.csv", b"id,v\n1,\"hello,world\"\n");
    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", dir.display()),
        db,
        defaultSDKConfig(),
    )
    .expect("scanner should initialize");
    let estimate = scanner
        .EstimateImportDataSize(&())
        .expect("CSV estimate should succeed");
    let empty = estimate
        .Tables
        .iter()
        .find(|table| table.Table == "empty_csv")
        .expect("empty csv estimate");
    let with_data = estimate
        .Tables
        .iter()
        .find(|table| table.Table == "with_csv")
        .expect("data csv estimate");
    assert_eq!(0, empty.TiKVSize);
    assert!(with_data.TiKVSize > 0);
}

/// 对照 Go `EstimateImportDataSizeMultiStatementSchema`：schema 文件中包含
/// CREATE DATABASE/USE/DROP 后，仍选择与路由表匹配的 CREATE TABLE。
#[test]
fn estimate_import_data_size_multi_statement_schema_matches_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(
        dir,
        "test_db-schema-create.sql",
        b"CREATE DATABASE test_db;",
    );
    write_file(
        dir,
        "test_db.users-schema.sql",
        b"CREATE DATABASE IF NOT EXISTS test_db;\nUSE test_db;\nDROP TABLE IF EXISTS users;\nCREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(255), KEY idx_name (name));",
    );
    write_file(dir, "test_db.users.001.csv", b"1,alice\n2,bob\n");
    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut scanner = NewFileScanner(
        &(),
        &format!("file://{}", dir.display()),
        db,
        defaultSDKConfig(),
    )
    .expect("scanner should initialize");
    let estimate = scanner
        .EstimateImportDataSize(&())
        .expect("multi-statement schema should estimate");
    assert_eq!(1, estimate.Tables.len());
    assert_eq!("users", estimate.Tables[0].Table);
    assert!(estimate.Tables[0].TiKVSize > 0);
    assert_eq!(estimate.Tables[0].TiKVSize, estimate.TotalTiKVSize);
}

/// 对照 Go `EstimateImportDataSizeSkipInvalidFiles`：只跳过坏表，保留好表。
#[test]
fn estimate_import_data_size_skip_invalid_files_matches_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(
        dir,
        "db1.good-schema.sql",
        b"CREATE TABLE db1.good (id INT PRIMARY KEY, v VARCHAR(255));",
    );
    write_file(dir, "db1.good.001.csv", b"1,good\n");
    write_file(
        dir,
        "db1.bad-schema.sql",
        b"CREATE TABL db1.bad (id INT PRIMARY KEY);",
    );
    write_file(dir, "db1.bad.001.csv", b"1\n");

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut cfg = defaultSDKConfig();
    cfg.skip_invalid_files = true;
    let mut scanner = NewFileScanner(&(), &format!("file://{}", dir.display()), db, cfg)
        .expect("scanner should initialize");

    let estimate = scanner
        .EstimateImportDataSize(&())
        .expect("skip_invalid_files should absorb only the bad table error");
    assert_eq!(1, estimate.Tables.len());
    assert_eq!("good", estimate.Tables[0].Table);
    assert!(estimate.Tables[0].SourceSize > 0);
    assert_eq!(estimate.Tables[0].SourceSize, estimate.TotalSourceSize);
    assert_eq!(estimate.Tables[0].TiKVSize, estimate.TotalTiKVSize);
}

/// 对照 Go estimate_real_size：默认估算解压后的真实大小，关闭时使用存储大小。
/// Mirrors Go's `TestFileScannerWithEstimateRealSize`: the default path reports
/// a real (uncompressed) estimate, while the opt-out path reports FileSize.
#[test]
fn file_scanner_with_estimate_real_size_matches_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "db1-schema-create.sql", b"CREATE DATABASE db1;");
    write_file(dir, "db1.t1-schema.sql", b"CREATE TABLE t1 (id INT);");

    // 构造可压缩的重复行，制造压缩前后大小差异。
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for _ in 0..1000 {
        std::io::Write::write_all(&mut encoder, b"aaaa\n").expect("write gzip row");
    }
    let compressed_data = encoder.finish().expect("finish gzip stream");
    let compressed_size = compressed_data.len() as i64;
    write_file(dir, "db1.t1.001.csv.gz", &compressed_data);

    let db1: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let cfg1 = defaultSDKConfig();
    let mut scanner1 = NewFileScanner(&(), &format!("file://{}", dir.display()), db1, cfg1)
        .expect("scanner1 should initialize");
    let metas1 = scanner1.GetTableMetas(&()).expect("metas1 should load");
    assert_eq!(1, metas1.len());
    assert!(metas1[0].TotalSize > compressed_size);

    let db2: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut cfg2 = defaultSDKConfig();
    WithEstimateRealSize(false)(&mut cfg2);
    let mut scanner2 = NewFileScanner(&(), &format!("file://{}", dir.display()), db2, cfg2)
        .expect("scanner2 should initialize");
    let metas2 = scanner2.GetTableMetas(&()).expect("metas2 should load");
    assert_eq!(1, metas2.len());
    assert_eq!(compressed_size, metas2[0].TotalSize);
    assert_eq!(1, metas2[0].DataFiles.len());
    assert_eq!(compressed_size, metas2[0].DataFiles[0].Size);
}

/// 对照 Go skip_invalid_files：歧义通配表失败，开启跳过后仅保留明确表。
/// Mirrors Go's `TestFileScannerWithSkipInvalidFiles`: when a file-routing
/// rule maps two files onto one table, `GetTableMetas` fails (ambiguous
/// wildcard) unless `skip_invalid_files` is set, in which case only the
/// unambiguous table survives.
#[test]
fn file_scanner_with_skip_invalid_files_matches_go() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let dir = tmp_dir.path();
    write_file(dir, "data1.csv", b"1");
    write_file(dir, "data2.csv", b"1");
    write_file(dir, "data3.csv", b"1");

    // 两条路由：data[1-2]→t1（歧义）、data3→t2（唯一）。
    let rules = vec![
        FileRouteRule {
            pattern: "data[1-2].csv".to_owned(),
            schema: "db1".to_owned(),
            table: "t1".to_owned(),
            type_name: "csv".to_owned(),
            ..Default::default()
        },
        FileRouteRule {
            pattern: "data3.csv".to_owned(),
            schema: "db1".to_owned(),
            table: "t2".to_owned(),
            type_name: "csv".to_owned(),
            ..Default::default()
        },
    ];

    let mut cfg = defaultSDKConfig();
    WithFileRouters(rules)(&mut cfg);

    let db: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut scanner = NewFileScanner(&(), &format!("file://{}", dir.display()), db, cfg.clone())
        .expect("scanner should initialize");
    assert!(scanner.GetTableMetas(&()).is_err());

    cfg.skip_invalid_files = true;
    let db2: Arc<dyn JobDatabase> = Arc::new(CanonicalDatabase::default());
    let mut scanner2 = NewFileScanner(&(), &format!("file://{}", dir.display()), db2, cfg)
        .expect("scanner2 should initialize");
    let metas2 = scanner2
        .GetTableMetas(&())
        .expect("skip_invalid_files should keep the unambiguous table");
    assert_eq!(1, metas2.len());
    assert_eq!("t2", metas2[0].Table);
}
