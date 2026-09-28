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

// SQL 生成器表驱动测试。
//
// 完整保留 Go `TestGenerateImportSQL` 的用例输入与期望 SQL/错误，验证转义、
// 资源参数拼接、CSV WITH 选项及多空值标记拒绝等行为。

use crate::{CSVConfig, ImportOptions, NewSQLGenerator, TableMeta};

/// 单条表驱动用例：名称、可选表元数据覆盖、选项、期望 SQL 或错误子串。
struct GenerateImportSQLCase {
    /// 用例名称，用于断言失败时定位。
    name: &'static str,
    /// `None` 时使用默认 test_db/test_table 元数据。
    table_meta: Option<TableMeta>,
    /// 导入选项（格式、线程、CSV 配置等）。
    options: ImportOptions,
    /// 成功时的完整期望 SQL；失败用例为空串。
    expected: &'static str,
    /// 非空时表示期望错误信息包含该子串。
    expected_error: &'static str,
}

/// Mirrors Go's `TestGenerateImportSQL` table-driven test: every case from the
/// Go table is preserved verbatim (same inputs, same expected SQL/errors).
#[test]
/// 逐条执行 Go 同表用例，比对生成 SQL 或错误子串。
fn generate_import_sql_matches_go_table() {
    let generator = NewSQLGenerator();
    let default_table_meta = TableMeta {
        Database: "test_db".to_owned(),
        Table: "test_table".to_owned(),
        WildcardPath: "s3://bucket/path/*.csv".to_owned(),
        ..Default::default()
    };

    // 下列用例顺序与字段与 Go 表驱动测试保持一致。
    let cases = vec![
        GenerateImportSQLCase {
            name: "Basic",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv' FORMAT 'csv'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "With S3 Credentials",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                ResourceParameters:
                    "access-key=ak&endpoint=http%3A%2F%2Fminio%3A9000&secret-access-key=sk"
                        .to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv?access-key=ak&endpoint=http%3A%2F%2Fminio%3A9000&secret-access-key=sk' FORMAT 'csv'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "With Options",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                Thread: 4,
                Detached: true,
                MaxWriteSpeed: "100MiB".to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv' FORMAT 'csv' WITH THREAD=4, MAX_WRITE_SPEED='100MiB', DETACHED",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "All Options",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                Thread: 8,
                DiskQuota: "100GiB".to_owned(),
                MaxWriteSpeed: "200MiB".to_owned(),
                SplitFile: true,
                RecordErrors: 100,
                Detached: true,
                CloudStorageURI: "s3://bucket/storage".to_owned(),
                GroupKey: "group1".to_owned(),
                SkipRows: 1,
                CharacterSet: "utf8mb4".to_owned(),
                ChecksumTable: "test_db.checksum_table".to_owned(),
                DisableTiKVImportMode: true,
                DisablePrecheck: true,
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv' FORMAT 'csv' WITH THREAD=8, DISK_QUOTA='100GiB', MAX_WRITE_SPEED='200MiB', SPLIT_FILE, RECORD_ERRORS=100, DETACHED, CLOUD_STORAGE_URI='s3://bucket/storage', GROUP_KEY='group1', SKIP_ROWS=1, CHARACTER_SET='utf8mb4', CHECKSUM_TABLE='test_db.checksum_table', DISABLE_TIKV_IMPORT_MODE, DISABLE_PRECHECK",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "With CSV Config",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                CSVConfig: Some(Box::new(CSVConfig {
                    FieldsTerminatedBy: ",".to_owned(),
                    FieldsEnclosedBy: "\"".to_owned(),
                    FieldsEscapedBy: "\\".to_owned(),
                    LinesTerminatedBy: "\n".to_owned(),
                    FieldNullDefinedBy: vec!["NULL".to_owned()],
                    ..Default::default()
                })),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv' FORMAT 'csv' WITH FIELDS_TERMINATED_BY=',', FIELDS_ENCLOSED_BY='\"', FIELDS_ESCAPED_BY='\\\\', LINES_TERMINATED_BY='\n', FIELDS_DEFINED_NULL_BY='NULL'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "With Cloud Storage URI",
            table_meta: None,
            options: ImportOptions {
                Format: "parquet".to_owned(),
                CloudStorageURI: "s3://bucket/storage".to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv' FORMAT 'parquet' WITH CLOUD_STORAGE_URI='s3://bucket/storage'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "Multiple Null Defined By",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                CSVConfig: Some(Box::new(CSVConfig {
                    FieldNullDefinedBy: vec!["NULL".to_owned(), "\\N".to_owned()],
                    ..Default::default()
                })),
                ..Default::default()
            },
            expected: "",
            expected_error: "IMPORT INTO only supports one FIELDS_DEFINED_NULL_BY value",
        },
        GenerateImportSQLCase {
            name: "Resource Parameters Append",
            table_meta: Some(TableMeta {
                Database: "test_db".to_owned(),
                Table: "test_table".to_owned(),
                WildcardPath: "s3://bucket/path/*.csv?foo=bar".to_owned(),
                ..Default::default()
            }),
            options: ImportOptions {
                Format: "csv".to_owned(),
                ResourceParameters: "access-key=ak".to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv?foo=bar&access-key=ak' FORMAT 'csv'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "Resource Parameters Append To Relative Path",
            table_meta: Some(TableMeta {
                Database: "test_db".to_owned(),
                Table: "test_table".to_owned(),
                WildcardPath: "data/*.csv?foo=bar#part".to_owned(),
                ..Default::default()
            }),
            options: ImportOptions {
                Format: "csv".to_owned(),
                ResourceParameters: "access-key=ak".to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 'data/*.csv?foo=bar&access-key=ak#part' FORMAT 'csv'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "Escaping",
            table_meta: None,
            options: ImportOptions {
                Format: "csv".to_owned(),
                GroupKey: "group'1".to_owned(),
                CharacterSet: "utf8'mb4".to_owned(),
                CSVConfig: Some(Box::new(CSVConfig {
                    FieldsTerminatedBy: "'".to_owned(),
                    FieldsEnclosedBy: "\\".to_owned(),
                    ..Default::default()
                })),
                ..Default::default()
            },
            expected: "IMPORT INTO `test_db`.`test_table` FROM 's3://bucket/path/*.csv' FORMAT 'csv' WITH GROUP_KEY='group''1', CHARACTER_SET='utf8''mb4', FIELDS_TERMINATED_BY='''', FIELDS_ENCLOSED_BY='\\\\'",
            expected_error: "",
        },
        GenerateImportSQLCase {
            name: "Identifier Escaping",
            table_meta: Some(TableMeta {
                Database: "test`db".to_owned(),
                Table: "test`table".to_owned(),
                WildcardPath: "s3://bucket/path/*.csv".to_owned(),
                ..Default::default()
            }),
            options: ImportOptions {
                Format: "csv".to_owned(),
                ..Default::default()
            },
            expected: "IMPORT INTO `test``db`.`test``table` FROM 's3://bucket/path/*.csv' FORMAT 'csv'",
            expected_error: "",
        },
    ];

    // 有 expected_error 则校验错误内容，否则校验 SQL 全文相等。
    for case in cases {
        let table_meta = case.table_meta.as_ref().unwrap_or(&default_table_meta);
        let result = generator.GenerateImportSQL(table_meta, &case.options);
        if !case.expected_error.is_empty() {
            let error = result.expect_err(case.name);
            assert!(
                error.to_string().contains(case.expected_error),
                "case {}: error {} does not contain {}",
                case.name,
                error,
                case.expected_error
            );
        } else {
            let sql = result.unwrap_or_else(|error| panic!("case {}: {error}", case.name));
            assert_eq!(case.expected, sql, "case {}", case.name);
        }
    }
}
