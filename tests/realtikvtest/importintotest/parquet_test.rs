// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! 中文说明开始（自动生成）
//! 中文总览：`parquet_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `parquet_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 57 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `PART0` 是当前文件里的常量。
//! 阅读 `PART0` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `PART0` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `PART0`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `PART0` 的重要阅读参照。
//! 理解 `PART0` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `PART0` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `PART1` 是当前文件里的常量。
//! 阅读 `PART1` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `PART1` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `PART1`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `PART1` 的重要阅读参照。
//! 理解 `PART1` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `PART1` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `SPARK_DATE` 是当前文件里的常量。
//! 阅读 `SPARK_DATE` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `SPARK_DATE` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `SPARK_DATE`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `SPARK_DATE` 的重要阅读参照。
//! 理解 `SPARK_DATE` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `SPARK_DATE` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `SPARK_DT` 是当前文件里的常量。
//! 阅读 `SPARK_DT` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `SPARK_DT` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `SPARK_DT`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `SPARK_DT` 的重要阅读参照。
//! 理解 `SPARK_DT` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `SPARK_DT` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_import_parquet` 是当前文件里的辅助函数。
//! 阅读 `test_import_parquet` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_parquet` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_parquet`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_parquet` 的重要阅读参照。
//! 理解 `test_import_parquet` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_parquet` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `Case` 是当前文件里的状态类型。
//! 阅读 `Case` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `Case` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `Case`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `Case` 的重要阅读参照。
//! 理解 `Case` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `Case` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_import_parquet_with_spark_legacy_dates` 是当前文件里的辅助函数。
//! 阅读 `test_import_parquet_with_spark_legacy_dates` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_parquet_with_spark_legacy_dates` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_parquet_with_spark_legacy_dates`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_parquet_with_spark_legacy_dates` 的重要阅读参照。
//! 理解 `test_import_parquet_with_spark_legacy_dates` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_parquet_with_spark_legacy_dates` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 符号 `test_import_parquet_with_spark_legacy_date_times` 是当前文件里的辅助函数。
//! 阅读 `test_import_parquet_with_spark_legacy_date_times` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_parquet_with_spark_legacy_date_times` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 如果后续断言、返回码或状态变化依赖 `test_import_parquet_with_spark_legacy_date_times`，应从这里理解职责边界。
//! Go 同名文件里的语义意图仍然是 `test_import_parquet_with_spark_legacy_date_times` 的重要阅读参照。
//! 理解 `test_import_parquet_with_spark_legacy_date_times` 的关键在于它怎样串起准备、执行、校验和收尾。
//! 对 `test_import_parquet_with_spark_legacy_date_times` 的说明以行为语义为主，而不是逐行翻译代码细节。
//! 中文说明结束（自动生成）

//! Go-equivalent tests for `parquet_test.go`.

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, reset_engine, serial_guard, testfailpoint, testkit,
};

const PART0: &[u8] = include_bytes!("part0.parquet");
const PART1: &[u8] = include_bytes!("part1.parquet");
const SPARK_DATE: &[u8] = include_bytes!("spark-legacy-date.gz.parquet");
const SPARK_DT: &[u8] = include_bytes!("spark-legacy-datetime.gz.parquet");

/// `TestImportParquet`.
#[test]
fn test_import_parquet() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/lightning/mydump/mockParquetRowCount",
        "return(5)",
    );

    let temp_dir = s.TempDir();
    s.NoError(std::fs::write(temp_dir.join("test.0.parquet"), PART0).map_err(|e| e.to_string()));
    s.NoError(std::fs::write(temp_dir.join("test.1.parquet"), PART1).map_err(|e| e.to_string()));
    let import_path = temp_dir.join("*.parquet");

    struct Case {
        create_sql: &'static str,
        import_sql: &'static str,
        read_sql: &'static str,
        check_count_only: bool,
    }
    let cases = [
        Case {
            create_sql: "CREATE TABLE test.sbtest(id bigint NOT NULL PRIMARY KEY, k bigint NOT NULL, c char(16), pad char(16))",
            import_sql: "IMPORT INTO test.sbtest FROM '%s' FORMAT 'parquet'",
            read_sql: "SELECT id + 1 FROM test.sbtest ORDER BY id",
            check_count_only: false,
        },
        Case {
            create_sql: "CREATE TABLE test.sbtest(id bigint NOT NULL, k bigint NOT NULL, c char(16), pad char(16))",
            import_sql: "IMPORT INTO test.sbtest FROM '%s' FORMAT 'parquet'",
            read_sql: "SELECT _tidb_rowid FROM test.sbtest",
            check_count_only: false,
        },
        Case {
            create_sql: "CREATE TABLE test.sbtest(id bigint NOT NULL PRIMARY KEY AUTO_INCREMENT, k bigint NOT NULL, c char(16), pad char(16))",
            import_sql: "IMPORT INTO test.sbtest(@1, k, c, pad) FROM '%s' FORMAT 'parquet'",
            read_sql: "SELECT id FROM test.sbtest",
            check_count_only: true,
        },
        Case {
            create_sql: "CREATE TABLE test.sbtest(id bigint NOT NULL PRIMARY KEY AUTO_RANDOM, k bigint NOT NULL, c char(16), pad char(16))",
            import_sql: "IMPORT INTO test.sbtest(@1, k, c, pad) FROM '%s' FORMAT 'parquet'",
            read_sql: "SELECT id FROM test.sbtest",
            check_count_only: true,
        },
        Case {
            create_sql: "CREATE TABLE test.sbtest(id bigint NOT NULL PRIMARY KEY NONCLUSTERED, k bigint NOT NULL, c char(16), pad char(16))",
            import_sql: "IMPORT INTO test.sbtest(id, k, c, pad) FROM '%s' FORMAT 'parquet'",
            read_sql: "SELECT _tidb_rowid FROM test.sbtest",
            check_count_only: false,
        },
    ];

    s.tk.MustExec("USE test;");
    for tc in cases {
        s.tk.MustExec("DROP TABLE IF EXISTS sbtest;");
        s.tk.MustExec(tc.create_sql);
        s.tk.MustQuery(
            &tc.import_sql
                .replace("%s", &import_path.display().to_string()),
        );
        let rs = s.tk.MustQuery(tc.read_sql).Rows();
        s.Len(&rs, 20);
        if !tc.check_count_only {
            for i in 0..20 {
                s.EqualValues(format!("{}", i + 1), rs[i][0].clone());
            }
        }
    }
    s.tear_down();
}

/// `TestImportParquetWithSparkLegacyDates`.
#[test]
fn test_import_parquet_with_spark_legacy_dates() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    let temp_dir = s.TempDir();
    let import_path = temp_dir.join("spark-legacy-date.gz.parquet");
    s.NoError(std::fs::write(&import_path, SPARK_DATE).map_err(|e| e.to_string()));

    s.tk.MustExec("USE test;");
    s.tk.MustExec("DROP TABLE IF EXISTS t;");
    s.tk.MustExec("CREATE TABLE t (d DATE NOT NULL);");
    s.tk.MustQuery(&format!(
        "IMPORT INTO test.t FROM '{}' FORMAT 'parquet'",
        import_path.display()
    ));
    s.tk.MustQuery("SELECT d FROM test.t ORDER BY d")
        .Check(&testkit::Rows(&[
            "0001-01-01",
            "0100-02-28",
            "0100-03-01",
            "0200-02-28",
            "0200-03-01",
            "0300-03-01",
            "0300-03-02",
            "0500-03-02",
            "0500-03-03",
            "0600-03-03",
            "0600-03-04",
            "0700-03-04",
            "0700-03-05",
            "0900-03-05",
            "0900-03-06",
            "1000-03-06",
            "1000-03-07",
            "1100-03-07",
            "1100-03-08",
            "1300-03-08",
            "1300-03-09",
            "1400-03-09",
            "1400-03-10",
            "1500-03-10",
            "1500-03-11",
            "1582-10-04",
            "1582-10-15",
            "1582-10-16",
            "1700-03-01",
            "1900-01-01",
            "1970-01-01",
            "2000-02-29",
            "9999-12-31",
        ]));
    s.tear_down();
}

/// `TestImportParquetWithSparkLegacyDateTimes`.
#[test]
fn test_import_parquet_with_spark_legacy_date_times() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    let temp_dir = s.TempDir();
    let import_path = temp_dir.join("spark-legacy-datetime.gz.parquet");
    s.NoError(std::fs::write(&import_path, SPARK_DT).map_err(|e| e.to_string()));

    s.tk.MustExec("USE test;");
    s.tk.MustExec("DROP TABLE IF EXISTS t;");
    s.tk.MustExec("CREATE TABLE t(v DATETIME(6));");
    s.tk.MustQuery(&format!(
        "IMPORT INTO test.t FROM '{}' FORMAT 'parquet'",
        import_path.display()
    ));

    let expected = [
        "0001-01-01 00:00:00.000000",
        "0001-01-01 00:00:00.000001",
        "0001-06-15 12:34:56.789123",
        "0001-12-31 23:59:59.999999",
        "0050-01-01 00:00:00.000000",
        "0050-06-15 12:34:56.789123",
        "0099-12-31 23:59:59.999999",
        "0100-03-01 00:00:00.000000",
        "0100-03-01 00:00:00.000001",
        "0100-03-01 23:59:59.999999",
        "0100-06-15 12:34:56.789123",
        "0150-01-01 00:00:00.000000",
        "0150-06-15 12:34:56.789123",
        "0199-12-31 23:59:59.999999",
        "0200-03-01 00:00:00.000000",
        "0200-03-01 00:00:00.000001",
        "0200-03-01 23:59:59.999999",
        "0200-06-15 12:34:56.789123",
        "0250-01-01 00:00:00.000000",
        "0250-06-15 12:34:56.789123",
        "0299-12-31 23:59:59.999999",
        "0300-03-01 00:00:00.000000",
        "0300-03-01 00:00:00.000001",
        "0300-03-01 23:59:59.999999",
        "0300-06-15 12:34:56.789123",
        "0400-02-29 00:00:00.000000",
        "0400-02-29 23:59:59.999999",
        "0400-03-01 00:00:00.000000",
        "0400-06-15 12:34:56.789123",
        "0499-12-31 23:59:59.999999",
        "0500-03-01 00:00:00.000000",
        "0500-03-01 00:00:00.000001",
        "0500-03-01 23:59:59.999999",
        "0500-06-15 12:34:56.789123",
        "0550-01-01 00:00:00.000000",
        "0550-06-15 12:34:56.789123",
        "0599-12-31 23:59:59.999999",
        "0600-03-01 00:00:00.000000",
        "0600-03-01 00:00:00.000001",
        "0600-03-01 23:59:59.999999",
        "0600-06-15 12:34:56.789123",
        "0650-01-01 00:00:00.000000",
        "0650-06-15 12:34:56.789123",
        "0699-12-31 23:59:59.999999",
        "0700-03-01 00:00:00.000000",
        "0700-03-01 00:00:00.000001",
        "0700-03-01 23:59:59.999999",
        "0700-06-15 12:34:56.789123",
        "0800-02-29 00:00:00.000000",
        "0800-02-29 23:59:59.999999",
        "0800-03-01 00:00:00.000000",
        "0800-06-15 12:34:56.789123",
        "0899-12-31 23:59:59.999999",
        "0900-03-01 00:00:00.000000",
        "0900-03-01 00:00:00.000001",
        "0900-03-01 23:59:59.999999",
        "0900-06-15 12:34:56.789123",
        "0950-01-01 00:00:00.000000",
        "0950-06-15 12:34:56.789123",
        "0999-12-31 23:59:59.999999",
        "1000-03-01 00:00:00.000000",
        "1000-03-01 00:00:00.000001",
        "1000-03-01 23:59:59.999999",
        "1000-06-15 12:34:56.789123",
        "1050-01-01 00:00:00.000000",
        "1050-06-15 12:34:56.789123",
        "1099-12-31 23:59:59.999999",
        "1100-03-01 00:00:00.000000",
        "1100-03-01 00:00:00.000001",
        "1100-03-01 23:59:59.999999",
        "1100-06-15 12:34:56.789123",
        "1200-02-29 00:00:00.000000",
        "1200-02-29 23:59:59.999999",
        "1200-03-01 00:00:00.000000",
        "1200-06-15 12:34:56.789123",
        "1299-12-31 23:59:59.999999",
        "1300-03-01 00:00:00.000000",
        "1300-03-01 00:00:00.000001",
        "1300-03-01 23:59:59.999999",
        "1300-06-15 12:34:56.789123",
        "1350-01-01 00:00:00.000000",
        "1350-06-15 12:34:56.789123",
        "1399-12-31 23:59:59.999999",
        "1400-03-01 00:00:00.000000",
        "1400-03-01 00:00:00.000001",
        "1400-03-01 23:59:59.999999",
        "1400-06-15 12:34:56.789123",
        "1450-01-01 00:00:00.000000",
        "1450-06-15 12:34:56.789123",
        "1499-12-31 23:59:59.999999",
        "1500-03-01 00:00:00.000000",
        "1500-03-01 00:00:00.000001",
        "1500-03-01 23:59:59.999999",
        "1500-06-15 12:34:56.789123",
        "1550-01-01 00:00:00.000000",
        "1550-06-15 12:34:56.789123",
        "1582-01-01 00:00:00.123456",
        "1582-10-04 00:00:00.000000",
        "1582-10-04 23:59:59.999999",
        "1582-10-15 00:00:00.000000",
        "1582-10-15 00:00:00.000001",
        "1582-10-15 12:34:56.789123",
        "1600-02-29 00:00:00.000000",
        "1600-02-29 23:59:59.999999",
        "1600-03-01 00:00:00.000000",
        "1700-02-28 23:59:59.999999",
        "1700-03-01 00:00:00.000000",
        "1800-02-28 23:59:59.999999",
        "1800-03-01 00:00:00.000000",
        "1899-12-31 00:00:00.000000",
        "1899-12-31 23:59:59.999999",
        "1900-01-01 00:00:00.000000",
        "1900-01-01 00:00:00.000001",
        "1900-01-01 12:34:56.789123",
        "1970-01-01 00:00:00.000000",
        "2000-02-29 23:59:59.999999",
        "9999-12-31 23:59:59.999999",
    ];
    s.tk.MustQuery("SELECT v FROM test.t ORDER BY v")
        .Check(&testkit::RowsWithSep("|", &expected));
    s.tear_down();
}
