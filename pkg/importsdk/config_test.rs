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

// SDKConfig 默认值与函数式选项副作用的单元测试。
//
// 对照 Go `TestDefaultSDKConfig` / `TestSDKOptions`：校验默认过滤列表、
// 日志器身份比较，以及非法/空选项不覆盖既有配置的分支。

use crate::*;
use astersql_lightning_log as log;
use astersql_parser_mysql as mysql;
use std::sync::Arc;

/// `Logger` wraps a `dyn Core` behind an `Arc` and does not implement
/// `PartialEq` (mirroring Go's zap.Logger comparison by identity), so
/// equality is checked by comparing the wrapped core pointer.
/// 按底层 Core 的 Arc 指针比较日志器身份（对应 Go zap.Logger 按身份比较）。
fn same_logger(a: &log::Logger, b: &log::Logger) -> bool {
    Arc::ptr_eq(&a.core(), &b.core())
}

/// Mirrors Go's `TestDefaultSDKConfig`: validates the base defaults returned by
/// `defaultSDKConfig` before any option is applied.
/// 对照 Go：校验尚未应用任何选项时的默认并发、过滤列表、字符集与估算开关。
#[test]
fn default_sdk_config_matches_go_defaults() {
    let cfg = defaultSDKConfig();
    assert_eq!(4, cfg.concurrency);
    assert_eq!(
        vec![
            "*.*".to_owned(),
            "!mysql.*".to_owned(),
            "!sys.*".to_owned(),
            "!INFORMATION_SCHEMA.*".to_owned(),
            "!PERFORMANCE_SCHEMA.*".to_owned(),
            "!METRICS_SCHEMA.*".to_owned(),
            "!INSPECTION_SCHEMA.*".to_owned(),
        ],
        cfg.filter
    );
    assert!(same_logger(&log::L(), &cfg.logger));
    assert_eq!("auto", cfg.charset);
    assert_eq!(",", cfg.csv_config.FieldsTerminatedBy);
    assert_eq!("\"", cfg.csv_config.FieldsEnclosedBy);
    assert_eq!("", cfg.csv_config.LinesTerminatedBy);
    assert!(cfg.csv_config.Header);
    assert!(cfg.csv_config.HeaderSchemaMatch);
    assert_eq!(vec![r"\N".to_owned()], cfg.csv_config.FieldNullDefinedBy);
    assert!(cfg.csv_config.BackslashEscape);
    assert_eq!("\\", cfg.csv_config.FieldsEscapedBy);
    assert_eq!("binary", cfg.data_character_set);
    assert!(cfg.estimate_real_size);
}

/// Mirrors Go's `TestSDKOptions`: every functional option is applied in sequence
/// against the same config object, matching the Go test's side-effect assertions
/// (including the "invalid/empty value keeps the previous setting" branches).
/// 依次应用各 WithXxx，并断言非法/空值保留先前配置。
#[test]
fn sdk_options_apply_side_effects_like_go() {
    let mut cfg = defaultSDKConfig();

    // WithConcurrency only accepts positive numbers; negative values must keep
    // the previously configured value.
    WithConcurrency(10)(&mut cfg);
    assert_eq!(10, cfg.concurrency);
    WithConcurrency(-1)(&mut cfg);
    assert_eq!(10, cfg.concurrency);

    // WithLogger injects an external logger handle used by later scan operations.
    let logger = log::L();
    WithLogger(logger.clone())(&mut cfg);
    assert!(same_logger(&logger, &cfg.logger));

    // WithSQLMode sets the MySQL SQLMode used by the schema parser.
    let mode = mysql::r#const::ModeStrictTransTables;
    WithSQLMode(mode)(&mut cfg);
    assert_eq!(mode, cfg.sql_mode);

    // WithFilter sets the Lightning loader's schema/table filter expressions.
    let filter = vec!["*.*".to_owned()];
    WithFilter(filter.clone())(&mut cfg);
    assert_eq!(filter, cfg.filter);

    // WithFileRouters sets the file routing rules; Go only checks the rules are
    // stored verbatim.
    let routers = vec![FileRouteRule {
        schema: "test".to_owned(),
        ..Default::default()
    }];
    WithFileRouters(routers.clone())(&mut cfg);
    assert_eq!(routers.len(), cfg.file_route_rules.len());
    assert_eq!("test", cfg.file_route_rules[0].schema);

    // WithCharset ignores an empty string so callers cannot accidentally clear
    // the configured charset.
    WithCharset("utf8mb4".to_owned())(&mut cfg);
    assert_eq!("utf8mb4", cfg.charset);
    WithCharset(String::new())(&mut cfg);
    assert_eq!("utf8mb4", cfg.charset);

    // WithCSVConfig replaces the complete CSV dialect used by size estimation.
    let csv_config = CSVConfig {
        FieldsTerminatedBy: "|".to_owned(),
        FieldsEnclosedBy: "'".to_owned(),
        LinesTerminatedBy: "\r\n".to_owned(),
        FieldNullDefinedBy: vec!["NULL".to_owned()],
        Header: false,
        HeaderSchemaMatch: false,
        TrimLastEmptyField: true,
        NotNull: true,
        BackslashEscape: false,
        FieldsEscapedBy: "~".to_owned(),
        LinesStartingBy: "prefix".to_owned(),
        AllowEmptyLine: true,
        QuotedNullIsText: true,
        UnescapedQuote: true,
    };
    WithCSVConfig(csv_config.clone())(&mut cfg);
    assert_eq!("|", cfg.csv_config.FieldsTerminatedBy);
    assert_eq!("\r\n", cfg.csv_config.LinesTerminatedBy);
    assert_eq!(vec!["NULL".to_owned()], cfg.csv_config.FieldNullDefinedBy);
    assert!(cfg.csv_config.TrimLastEmptyField);
    assert!(cfg.csv_config.NotNull);
    assert!(cfg.csv_config.AllowEmptyLine);
    assert!(cfg.csv_config.QuotedNullIsText);
    assert!(cfg.csv_config.UnescapedQuote);

    // WithDataCharacterSet follows WithCharset's non-empty guard.
    WithDataCharacterSet("gb18030".to_owned())(&mut cfg);
    assert_eq!("gb18030", cfg.data_character_set);
    WithDataCharacterSet(String::new())(&mut cfg);
    assert_eq!("gb18030", cfg.data_character_set);

    // WithMaxScanFiles is stored as an option; Go uses a pointer to express
    // "unset".
    WithMaxScanFiles(100)(&mut cfg);
    assert_eq!(Some(100), cfg.max_scan_files);
    WithMaxScanFiles(0)(&mut cfg);
    assert_eq!(Some(100), cfg.max_scan_files);
    WithMaxScanFiles(-1)(&mut cfg);
    assert_eq!(Some(100), cfg.max_scan_files);

    // WithEstimateRealSize controls whether compressed/parquet files get a real
    // size estimate.
    WithEstimateRealSize(false)(&mut cfg);
    assert!(!cfg.estimate_real_size);

    // WithSkipInvalidFiles controls whether invalid tables are skipped.
    WithSkipInvalidFiles(true)(&mut cfg);
    assert!(cfg.skip_invalid_files);

    // WithRoutes sets schema/table rewrite rules; Go verifies the full Routes
    // value is stored unchanged.
    let routes = vec![TableRouteRule {
        SchemaPattern: "source_db".to_owned(),
        TablePattern: "source_table".to_owned(),
        TargetSchema: "target_db".to_owned(),
        TargetTable: "target_table".to_owned(),
    }];
    WithRoutes(routes.clone())(&mut cfg);
    assert_eq!(routes.len(), cfg.routes.len());
    assert_eq!("source_db", cfg.routes[0].SchemaPattern);
    assert_eq!("source_table", cfg.routes[0].TablePattern);
    assert_eq!("target_db", cfg.routes[0].TargetSchema);
    assert_eq!("target_table", cfg.routes[0].TargetTable);
}
