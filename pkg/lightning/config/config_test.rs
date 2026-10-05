// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `config` 模块单元测试。
//
// 覆盖 adjust 发现 PD/端口、路径与 CSV 校验、TOML 加载/未知键、Duration/MaxError、
// 冲突策略、默认值与后端差异等配置行为。

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use regex::Regex;

use crate::{
    BACKEND_LOCAL, BACKEND_TIDB, ByteSize, CHECKPOINT_DRIVER_MYSQL, Charset,
    CheckpointKeepStrategy, CompressionType, Config, ConfigError, DEFAULT_BLOCK_SIZE,
    DuplicateResolutionAlgorithm, Duration, FileRouteRule, KV_WRITE_BATCH_SIZE, NoSettings,
    PostOpLevel, SettingsProvider, TiDbSettings, get_default_filter, load_global_config,
    new_config, parse_charset, toml_codec,
};

#[derive(Clone)]
/// 固定返回结果的 SettingsProvider，用于测试发现逻辑。
struct FixedSettings {
    result: Result<TiDbSettings, String>,
}

impl SettingsProvider for FixedSettings {
    fn settings(&self) -> Result<TiDbSettings, ConfigError> {
        self.result.clone().map_err(ConfigError::Invalid)
    }
}

/// 构造成功发现 port/path 的 SettingsProvider。
fn settings_ok(port: i32, path: &str) -> FixedSettings {
    FixedSettings {
        result: Ok(TiDbSettings {
            port,
            path: path.into(),
        }),
    }
}

/// 构造失败的 SettingsProvider。
fn settings_err(message: &str) -> FixedSettings {
    FixedSettings {
        result: Err(message.into()),
    }
}

/// 填入一组可通过 adjust 的最小合法字段。
fn assign_minimal_legal_value(cfg: &mut Config) {
    cfg.tidb.host = "123.45.67.89".into();
    cfg.tidb.port = 4567;
    cfg.tidb.status_port = 8901;
    cfg.tidb.pd_addr = "234.56.78.90:12345".into();
    cfg.mydumper.source_dir = "file://.".into();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tikv_importer.disk_quota = ByteSize(1);
}

/// 构造几乎全零/空字符串的配置，便于测默认填充。
fn empty_config() -> Config {
    Config {
        task_id: 0,
        app: Default::default(),
        tidb: Default::default(),
        checkpoint: crate::Checkpoint {
            schema: String::new(),
            dsn: String::new(),
            mysql_param: None,
            driver: String::new(),
            enable: false,
            keep_after_success: CheckpointKeepStrategy::Remove,
        },
        mydumper: Default::default(),
        tikv_importer: crate::TikvImporter {
            addr: String::new(),
            backend: String::new(),
            on_duplicate: DuplicateResolutionAlgorithm::None,
            max_kv_pairs: 0,
            send_kv_pairs: 0,
            send_kv_size: ByteSize(0),
            compress_kv_pairs: CompressionType::None,
            region_split_size: ByteSize(0),
            region_split_keys: 0,
            region_split_batch_size: 0,
            region_split_concurrency: 0,
            region_check_backoff_limit: 0,
            sorted_kv_dir: String::new(),
            disk_quota: ByteSize(0),
            range_concurrency: 0,
            duplicate_resolution: DuplicateResolutionAlgorithm::None,
            incremental_import: false,
            parallel_import: false,
            keyspace_name: String::new(),
            add_index_by_sql: false,
            strip_s3_external_id_for_import_sql: false,
            engine_mem_cache_size: ByteSize(0),
            local_writer_mem_cache_size: ByteSize(0),
            store_write_bw_limit: ByteSize(0),
            logical_import_batch_size: ByteSize(96 * 1024),
            logical_import_batch_rows: 65_536,
            logical_import_prep_stmt: false,
            pause_pd_scheduler_scope: "table".into(),
            block_size: ByteSize(0),
        },
        post_restore: Default::default(),
        cron: Default::default(),
        routes: Vec::new(),
        security: Default::default(),
        conflict: crate::Conflict {
            strategy: DuplicateResolutionAlgorithm::None,
            precheck_conflict_before_import: false,
            threshold: -1,
            max_record_rows: -1,
        },
    }
}

/// 创建唯一临时目录供路径相关测试使用。
fn tempfile_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "astersql-lightning-config-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 断言错误消息包含指定子串。
fn assert_error_contains(err: &ConfigError, needle: &str) {
    let text = err.to_string();
    assert!(
        text.contains(needle),
        "expected error to contain {needle:?}, got {text}"
    );
}

/// 使用成功 SettingsProvider 调用 adjust_with_settings。
fn adjust_discover(cfg: &mut Config, port: i32, path: &str) -> Result<(), ConfigError> {
    cfg.adjust_with_settings(&settings_ok(port, path))
}

#[test]
/// 验证 local 后端通过 SettingsProvider 补齐 port 与 pd-addr。
fn test_adjust_pd_addr_and_port() {
    let mut cfg = new_config();
    cfg.mydumper.source_dir = ".".into();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    adjust_discover(&mut cfg, 4444, "123.45.67.89:1234,56.78.90.12:3456").unwrap();
    assert_eq!(4444, cfg.tidb.port);
    assert_eq!("123.45.67.89:1234,56.78.90.12:3456", cfg.tidb.pd_addr);
}

#[test]
/// 验证 strict-format 要求非空 CSV terminator。
fn test_strict_format() {
    let mut cfg = new_config();
    cfg.mydumper.source_dir = ".".into();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.strict_format = true;
    let err = adjust_discover(&mut cfg, 4444, "123.45.67.89:1234,56.78.90.12:3456").unwrap_err();
    assert_error_contains(
        &err,
        "mydumper.strict-format can not be used with empty mydumper.csv.terminator",
    );
    cfg.mydumper.csv.lines_terminated_by = "\r\n".into();
    adjust_discover(&mut cfg, 4444, "123.45.67.89:1234,56.78.90.12:3456").unwrap();
}

#[test]
/// 验证 pause-pd-scheduler-scope 仅允许 table/global。
fn test_pause_pd_scheduler_scope() {
    let tmp_dir = tempfile_dir();
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = "test".into();
    cfg.mydumper.source_dir = tmp_dir.to_string_lossy().into();
    assert_eq!("table", cfg.tikv_importer.pause_pd_scheduler_scope);
    let settings = settings_ok(4444, "123.45.67.89:1234,56.78.90.12:3456");
    for invalid in ["", "xxx"] {
        cfg.tikv_importer.pause_pd_scheduler_scope = invalid.into();
        assert_error_contains(
            &cfg.adjust_with_settings(&settings).unwrap_err(),
            "pause-pd-scheduler-scope is invalid",
        );
    }
    cfg.tikv_importer.pause_pd_scheduler_scope = "TABLE".into();
    cfg.adjust_with_settings(&settings).unwrap();
    assert_eq!("table", cfg.tikv_importer.pause_pd_scheduler_scope);
    cfg.tikv_importer.pause_pd_scheduler_scope = "globAL".into();
    cfg.adjust_with_settings(&settings).unwrap();
    assert_eq!("global", cfg.tikv_importer.pause_pd_scheduler_scope);
}

#[test]
/// 验证 advertise/path 形式的 PD 地址发现。
fn test_adjust_pd_addr_and_port_via_advertise_addr() {
    let mut cfg = new_config();
    cfg.mydumper.source_dir = ".".into();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    adjust_discover(&mut cfg, 6666, "34.34.34.34:3434").unwrap();
    assert_eq!(6666, cfg.tidb.port);
    assert_eq!("34.34.34.34:3434", cfg.tidb.pd_addr);
}

#[test]
/// 验证 SettingsProvider 返回错误时 adjust 失败信息。
fn test_adjust_page_not_found() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    let err = cfg.adjust_with_settings(&settings_err("404")).unwrap_err();
    assert!(
        Regex::new(r"cannot fetch settings from TiDB.*")
            .unwrap()
            .is_match(&err.to_string())
    );
}

#[test]
/// 验证连接被拒绝时的 adjust 错误。
fn test_adjust_connect_refused() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    let err = cfg
        .adjust_with_settings(&settings_err("connection refused"))
        .unwrap_err();
    assert!(
        Regex::new(r"cannot fetch settings from TiDB.*")
            .unwrap()
            .is_match(&err.to_string())
    );
}

#[test]
/// 验证未设置 backend 时报错。
fn test_adjust_backend_not_set() {
    let mut cfg = new_config();
    cfg.tidb.distsql_scan_concurrency = 1;
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]tikv-importer.backend must not be empty!",
        cfg.adjust().unwrap_err().to_string()
    );
}

#[test]
/// 验证非法 backend 名称。
fn test_adjust_invalid_backend() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = "no_such_backend".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]unsupported `tikv-importer.backend` (no_such_backend)",
        cfg.adjust().unwrap_err().to_string()
    );
}

#[test]
/// 验证 data-source-dir 存在性与绝对路径规范化。
fn test_check_and_adjust_file_path() {
    let tmp_dir = tempfile_dir();
    let slash_path = tmp_dir.to_string_lossy().replace('\\', "/");
    let special_dir = tmp_dir.join("abc??bcd");
    std::fs::create_dir_all(&special_dir).unwrap();
    let special_dir1 = tmp_dir.join("abc%3F%3F%3Fbcd");
    std::fs::create_dir_all(&special_dir1).unwrap();
    let mut cfg = new_config();
    let file_path = |p: &std::path::Path| {
        url::Url::from_file_path(std::fs::canonicalize(p).unwrap())
            .unwrap()
            .path()
            .to_owned()
    };
    let cases = vec![
        (tmp_dir.to_string_lossy().to_string(), file_path(&tmp_dir)),
        (".".into(), file_path(PathBuf::from(".").as_path())),
        (
            special_dir.to_string_lossy().to_string(),
            file_path(&special_dir),
        ),
        (
            special_dir1.to_string_lossy().to_string(),
            file_path(&special_dir1),
        ),
        (format!("file://{slash_path}"), slash_path.clone()),
        (format!("local://{slash_path}"), slash_path),
        ("s3://bucket_name".into(), "".into()),
        ("s3://bucket_name/path/to/dir".into(), "/path/to/dir".into()),
        ("oss://bucketname".into(), "".into()),
        ("oss://bucketname/path/to/dir".into(), "/path/to/dir".into()),
        (
            "oss://bucketname/path/to/dir?region=cn-hangzhou&endpoint=https://oss-cn-hangzhou.aliyuncs.com".into(),
            "/path/to/dir".into(),
        ),
        ("gcs://bucketname/path/to/dir".into(), "/path/to/dir".into()),
        ("gs://bucketname/path/to/dir".into(), "/path/to/dir".into()),
        ("noop:///".into(), "/".into()),
    ];
    for (test, expect) in cases {
        cfg.mydumper.source_dir = test.clone();
        cfg.mydumper
            .adjust_file_path()
            .unwrap_or_else(|e| panic!("{test}: {e}"));
        let u = url::Url::parse(&cfg.mydumper.source_dir).unwrap();
        assert_eq!(expect, u.path(), "input={test}");
    }
}

#[test]
/// 验证文件路由相对/绝对路径调整。
fn test_adjust_file_route_path() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    let tmp_dir = tempfile_dir();
    cfg.mydumper.source_dir = tmp_dir.to_string_lossy().into();
    let invalid_path = tmp_dir.parent().unwrap().join("test123").join("1.sql");
    cfg.mydumper.file_routers = vec![FileRouteRule {
        path: invalid_path.to_string_lossy().into(),
        file_type: "sql".into(),
        schema: "test".into(),
        table: "tbl".into(),
        ..Default::default()
    }];
    cfg.tidb.distsql_scan_concurrency = 1;
    let err = cfg.adjust().unwrap_err();
    assert_error_contains(&err, "file route path");
    let rel = PathBuf::from("test_dir").join("1.sql");
    cfg.mydumper.file_routers[0].path = tmp_dir.join(&rel).to_string_lossy().into();
    cfg.adjust().unwrap();
    assert_eq!(rel.to_string_lossy(), cfg.mydumper.file_routers[0].path);
}

#[test]
/// 验证非法 TOML/字段解码错误。
fn test_decode_error() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    let err = cfg
        .adjust_with_settings(&settings_err("invalid-string"))
        .unwrap_err();
    assert!(
        Regex::new(r"cannot fetch settings from TiDB.*")
            .unwrap()
            .is_match(&err.to_string())
    );
}

#[test]
/// 验证非法配置项在 adjust 阶段被拒绝。
fn test_invalid_setting() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    cfg.tidb.pd_addr = "234.56.78.90:12345".into();
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]invalid `tidb.port` setting",
        cfg.adjust_with_settings(&settings_ok(0, "x"))
            .unwrap_err()
            .to_string()
    );
}

#[test]
/// 验证非法 pd-addr 格式。
fn test_invalid_pd_addr() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]invalid `tidb.pd-addr` setting",
        cfg.adjust_with_settings(&settings_ok(1234, ",,"))
            .unwrap_err()
            .to_string()
    );
}

#[test]
/// 对齐 Go：settings.path 只检查 host/port 非空且端口不为 "0"，不额外限制为 u16。
fn test_pd_addr_validation_matches_go_settings_path() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    adjust_discover(&mut cfg, 1234, "pd.example:service").unwrap();
    assert_eq!("pd.example:service", cfg.tidb.pd_addr);

    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]invalid `tidb.port` setting",
        adjust_discover(&mut cfg, 1234, "pd.example:0")
            .unwrap_err()
            .to_string()
    );
}

#[test]
/// 已填 port/pd 时不应依赖 SettingsProvider。
fn test_adjust_will_not_contact_server_if_everything_is_defined() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.adjust().unwrap();
    assert_eq!(4567, cfg.tidb.port);
    assert_eq!("234.56.78.90:12345", cfg.tidb.pd_addr);
}

#[test]
/// 非法 batch-import-ratio 回退默认值。
fn test_adjust_will_batch_import_ratio_invalid() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.mydumper.batch_import_ratio = -1.0;
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.adjust().unwrap();
    assert_eq!(0.75, cfg.mydumper.batch_import_ratio);
}

#[test]
/// 验证 tidb.tls 与 security 段组合行为。
fn test_adjust_security_section() {
    let cases: &[(&str, &str, bool, bool)] = &[
        ("", "", false, false),
        ("\n[security]\n", "", false, false),
        (
            "\n[security]\nca-path = \"/path/to/ca.pem\"\n",
            "/path/to/ca.pem",
            false,
            false,
        ),
        (
            "\n[security]\nca-path = \"/path/to/ca.pem\"\n[tidb.security]\n",
            "",
            false,
            false,
        ),
        (
            "\n[security]\nca-path = \"/path/to/ca.pem\"\n[tidb.security]\nca-path = \"/path/to/ca2.pem\"\n",
            "/path/to/ca2.pem",
            false,
            false,
        ),
        (
            "\n[security]\n[tidb.security]\nca-path = \"/path/to/ca2.pem\"\n",
            "/path/to/ca2.pem",
            false,
            false,
        ),
        (
            "\n[security]\n[tidb]\ntls = \"skip-verify\"\n[tidb.security]\n",
            "",
            true,
            false,
        ),
        (
            "\n[security]\n[tidb]\ntls = \"preferred\"\n[tidb.security]\n",
            "",
            true,
            true,
        ),
        (
            "\n[security]\n[tidb]\ntls = \"false\"\n[tidb.security]\n",
            "",
            false,
            false,
        ),
        (
            "\n[security]\n[tidb]\ntls = \"false\"\n[tidb.security]\nca-path = \"/path/to/ca2.pem\"\n",
            "",
            false,
            false,
        ),
        (
            "\n[security]\nca-path = \"/path/to/ca2.pem\"\n[tidb]\ntls = \"false\"\n",
            "",
            false,
            false,
        ),
    ];
    for (input, expected_ca, has_tls, fallback) in cases {
        let mut cfg = new_config();
        assign_minimal_legal_value(&mut cfg);
        cfg.tidb.distsql_scan_concurrency = 1;
        cfg.load_from_toml(input.as_bytes()).unwrap();
        cfg.tidb
            .adjust(&cfg.tikv_importer, &cfg.security, &NoSettings)
            .unwrap();
        let sec = cfg.tidb.security.as_ref().unwrap();
        assert_eq!(*expected_ca, sec.ca_path, "input={input}");
        assert_eq!(*has_tls, sec.tls_config.is_some(), "input={input}");
        assert_eq!(*fallback, sec.allow_fallback_to_plaintext, "input={input}");
    }
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.security.ca_path = "/path/to/ca.pem".into();
    cfg.tidb
        .adjust(&cfg.tikv_importer, &cfg.security, &NoSettings)
        .unwrap();
}

#[test]
/// 验证非法 CSV 分隔符/转义组合。
fn test_invalid_csv() {
    let cases: &[(&str, &str)] = &[
        (
            "[mydumper.csv]\nseparator = ''\n",
            "`mydumper.csv.separator` must not be empty",
        ),
        (
            "[mydumper.csv]\nseparator = 'hello'\ndelimiter = 'hel'\n",
            "must not be prefix of each other",
        ),
        (
            "[mydumper.csv]\nseparator = 'hel'\ndelimiter = 'hello'\n",
            "must not be prefix of each other",
        ),
        (
            "[mydumper.csv]\nseparator = '\\'\nbackslash-escape = false\n",
            "",
        ),
        ("[mydumper.csv]\nseparator = '，'\n", ""),
        ("[mydumper.csv]\ndelimiter = ''\n", ""),
        ("[mydumper.csv]\ndelimiter = 'hello'\n", ""),
        (
            "[mydumper.csv]\ndelimiter = '\\'\nbackslash-escape = false\n",
            "",
        ),
        ("[mydumper.csv]\nseparator = '\\s'\ndelimiter = '\\d'\n", ""),
        (
            "[mydumper.csv]\nseparator = '|'\ndelimiter = '|'\n",
            "must not be prefix of each other",
        ),
        (
            "[mydumper.csv]\nseparator = '\\'\nbackslash-escape = true\n",
            "both as CSV separator",
        ),
        (
            "[mydumper.csv]\ndelimiter = '\\'\nescaped-by = '\\'\n",
            "both as CSV delimiter",
        ),
    ];
    for (input, expect) in cases {
        let mut cfg = new_config();
        assign_minimal_legal_value(&mut cfg);
        cfg.tidb.distsql_scan_concurrency = 1;
        cfg.load_from_toml(input.as_bytes())
            .unwrap_or_else(|e| panic!("load {input}: {e}"));
        let result = cfg.adjust();
        if expect.is_empty() {
            result.unwrap_or_else(|e| panic!("adjust ok expected for {input}: {e}"));
        } else {
            assert_error_contains(
                &result.expect_err(&format!("adjust err expected for {input}")),
                expect,
            );
        }
    }
}

#[test]
/// 验证损坏 TOML 无法加载。
fn test_invalid_toml() {
    let mut cfg = empty_config();
    let err = cfg
        .load_from_toml(
            b"
\t\tinvalid[mydumper.csv]
\t\tdelimiter = '\\'
\t\tbackslash-escape = true
\t",
        )
        .unwrap_err();
    assert!(err.to_string().contains("toml:"), "{err}");
}

#[test]
/// 验证字符串与字符串数组配置兼容。
fn test_string_or_string_slice() {
    let mut cfg = empty_config();
    cfg.load_from_toml(b"[mydumper.csv]\nnull = '\\N'\n")
        .unwrap();
    cfg.load_from_toml(b"[mydumper.csv]\nnull = [ '\\N', 'NULL' ]\n")
        .unwrap();
    assert_error_contains(
        &cfg.load_from_toml(b"[mydumper.csv]\nnull = [ '\\N', 123 ]\n")
            .unwrap_err(),
        "invalid string slice",
    );
}

#[test]
/// 验证未知配置键报错。
fn test_toml_unused_keys() {
    let mut cfg = empty_config();
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]config file contained unknown configuration options: lightning.typo",
        cfg.load_from_toml(b"[lightning]\ntypo = 123\n")
            .unwrap_err()
            .to_string()
    );
}

#[test]
/// 验证 Duration 文本解析。
fn test_duration_unmarshal() {
    let mut duration = Duration::default();
    duration.unmarshal_text(b"13m20s").unwrap();
    assert_eq!(800_000_000_000, duration.0);
    for (input, nanos, encoded) in [
        ("0", 0, "0s"),
        ("-1.5s", -1_500_000_000, "-1.5s"),
        ("250us", 250_000, "250µs"),
        ("250µs", 250_000, "250µs"),
        ("250μs", 250_000, "250µs"),
        ("1.000002ms", 1_000_002, "1.000002ms"),
    ] {
        duration.unmarshal_text(input.as_bytes()).unwrap();
        assert_eq!(nanos, duration.0, "{input}");
        assert_eq!(encoded, duration.go_string(), "{input}");
    }
    let err = duration.unmarshal_text(b"13x20s").unwrap_err();
    assert!(
        Regex::new(r"time: unknown unit .?x.? in duration .?13x20s.?")
            .unwrap()
            .is_match(&err.to_string()),
        "{err}"
    );
}

#[test]
/// 验证 Duration JSON 编码。
fn test_duration_marshal_json() {
    let mut duration = Duration::default();
    duration.unmarshal_text(b"13m20s").unwrap();
    assert_eq!(br#""13m20s""#, duration.marshal_json().unwrap().as_slice());
}

#[test]
/// 验证 max-error 整数与表形式解码。
fn test_max_error_unmarshal() {
    let cases: &[(&str, Option<&[(&str, i64)]>, &str)] = &[
        (
            "[lightning]\nmax-error = 123\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 123)]),
            "",
        ),
        (
            "[lightning]\nmax-error = -123\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 0)]),
            "",
        ),
        (
            "[lightning]\nmax-error = \"abcde\"\n",
            None,
            "invalid max-error 'abcde'",
        ),
        (
            "[lightning.max-error]\nsyntax = 1\ncharset = 2\ntype = 3\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 3)]),
            "",
        ),
        (
            "[lightning.max-error]\ntype = 1000\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 1000)]),
            "",
        ),
        (
            "[lightning]\nmax-error = { type = 123 }\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 123)]),
            "",
        ),
        (
            "[lightning.max-error]\nnot_exist = 123\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 0)]),
            "",
        ),
        (
            "[lightning.max-error]\ntype = -123\n",
            Some(&[("syntax", 0), ("charset", i64::MAX), ("type", 0)]),
            "",
        ),
        ("[lightning.max-error]\ntype = abc\n", None, "abc"),
    ];
    for (toml_str, expected, err_part) in cases {
        let mut cfg = empty_config();
        let result = cfg.load_from_toml(toml_str.as_bytes());
        if !err_part.is_empty() {
            assert!(
                result.unwrap_err().to_string().contains(err_part),
                "{toml_str}"
            );
        } else {
            result.unwrap();
            for (k, v) in expected.unwrap() {
                let got = match *k {
                    "syntax" => cfg.app.max_error.syntax.load(Ordering::Relaxed),
                    "charset" => cfg.app.max_error.charset.load(Ordering::Relaxed),
                    "type" => cfg.app.max_error.r#type.load(Ordering::Relaxed),
                    _ => panic!(),
                };
                assert_eq!(*v, got, "{toml_str} {k}");
            }
        }
    }
}

#[test]
/// 验证冲突策略字符串解析。
fn test_duplicate_resolution_algorithm() {
    let mut dra = DuplicateResolutionAlgorithm::None;
    for (input, want) in [
        ("", DuplicateResolutionAlgorithm::None),
        ("none", DuplicateResolutionAlgorithm::None),
        ("replace", DuplicateResolutionAlgorithm::Replace),
        ("ignore", DuplicateResolutionAlgorithm::Ignore),
        ("error", DuplicateResolutionAlgorithm::Error),
        ("remove", DuplicateResolutionAlgorithm::Replace),
        ("record", DuplicateResolutionAlgorithm::Replace),
    ] {
        dra.from_string_value(input).unwrap();
        assert_eq!(want, dra);
    }
    assert_eq!("", DuplicateResolutionAlgorithm::None.as_str());
    assert_eq!("replace", DuplicateResolutionAlgorithm::Replace.as_str());
    assert_eq!("ignore", DuplicateResolutionAlgorithm::Ignore.as_str());
    assert_eq!("error", DuplicateResolutionAlgorithm::Error.as_str());
}

#[test]
/// 验证从 TOML 加载完整配置字段。
fn test_load_config() {
    assert!(
        load_global_config(&["-tidb-port".into(), "sss".into()], None)
            .unwrap_err()
            .to_string()
            .contains("tidb-port")
    );
    assert!(matches!(
        load_global_config(&["-V".into()], None).unwrap_err(),
        ConfigError::Help
    ));
    let err = load_global_config(&["-config".into(), "not-exists".into()], None).unwrap_err();
    assert!(
        Regex::new(
            r"(?i).*(no such file or directory|The system cannot find the file specified).*"
        )
        .unwrap()
        .is_match(&err.to_string()),
        "{err}"
    );
    assert_error_contains(
        &load_global_config(&["--server-mode".into()], None).unwrap_err(),
        "If server-mode is enabled, the status-addr must be a valid listen address",
    );
    let path = std::env::current_dir().unwrap();
    let cfg = load_global_config(
        &[
            "-L".into(),
            "debug".into(),
            "-log-file".into(),
            "/path/to/file.log".into(),
            "-tidb-host".into(),
            "172.16.30.11".into(),
            "-tidb-port".into(),
            "4001".into(),
            "-tidb-user".into(),
            "guest".into(),
            "-tidb-password".into(),
            "12345".into(),
            "-pd-urls".into(),
            "172.16.30.11:2379,172.16.30.12:2379".into(),
            "-d".into(),
            path.to_string_lossy().into(),
            "-backend".into(),
            BACKEND_LOCAL.into(),
            "-sorted-kv-dir".into(),
            ".".into(),
            "-checksum=false".into(),
        ],
        None,
    )
    .unwrap();
    assert_eq!("debug", cfg.app.log_config.level);
    assert_eq!("/path/to/file.log", cfg.app.log_config.file);
    assert_eq!("172.16.30.11", cfg.tidb.host);
    assert_eq!(4001, cfg.tidb.port);
    assert_eq!("guest", cfg.tidb.user);
    assert_eq!("12345", cfg.tidb.password);
    assert_eq!("172.16.30.11:2379,172.16.30.12:2379", cfg.tidb.pd_addr);
    assert_eq!(path.to_string_lossy(), cfg.mydumper.source_dir);
    assert_eq!(BACKEND_LOCAL, cfg.tikv_importer.backend);
    assert_eq!(".", cfg.tikv_importer.sorted_kv_dir);
    assert_eq!(PostOpLevel::Off, cfg.post_restore.checksum);
    let mut task_cfg = new_config();
    task_cfg.load_from_global(&cfg).unwrap();
    task_cfg.checkpoint.dsn.clear();
    task_cfg.checkpoint.driver = CHECKPOINT_DRIVER_MYSQL.into();
    task_cfg.tidb.distsql_scan_concurrency = 1;
    task_cfg.adjust().unwrap();
    let dsn = task_cfg
        .checkpoint
        .mysql_param
        .as_ref()
        .unwrap()
        .format_dsn();
    assert!(dsn.contains("guest:12345@tcp(172.16.30.11:4001)/"), "{dsn}");
    assert!(
        Regex::new(r#".*"pd-addr":"172.16.30.11:2379,172.16.30.12:2379".*"#)
            .unwrap()
            .is_match(&task_cfg.string())
    );
    let cfg = load_global_config(&[], None).unwrap();
    assert!(
        Regex::new(r".*lightning\.log.*")
            .unwrap()
            .is_match(&cfg.app.log_config.file)
    );
    assert_eq!(
        "-",
        load_global_config(&["--log-file".into(), "-".into()], None)
            .unwrap()
            .app
            .log_config
            .file
    );
}

#[test]
/// 验证 importer 默认值随 backend 调整。
fn test_default_importer_backend_value() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tikv_importer.backend = "local".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.adjust().unwrap();
    assert_eq!(2, cfg.app.index_concurrency);
    assert_eq!(6, cfg.app.table_concurrency);
    assert_eq!(4096, cfg.tikv_importer.region_split_batch_size);
}

#[test]
/// 验证 Region 并发受可用 CPU 限制。
fn test_region_concurrency_uses_usable_cpu_count() {
    fail::cfg("mockNumCpu", "return(2)").unwrap();
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = fail::remove("mockNumCpu");
        }
    }
    let _guard = Guard;
    assert_eq!(2, new_config().app.region_concurrency);
    let mut cfg = new_config();
    cfg.app.region_concurrency = 3;
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.app.adjust(&cfg.tikv_importer);
    assert_eq!(2, cfg.app.region_concurrency);
}

#[test]
/// 验证 tidb 后端默认并发派生。
fn test_default_tidb_backend_value() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tikv_importer.backend = "tidb".into();
    cfg.app.region_concurrency = 123;
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.adjust().unwrap();
    assert_eq!(123, cfg.app.table_concurrency);
}

#[test]
/// 验证显式配置可覆盖默认值。
fn test_default_could_be_overwritten() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tikv_importer.backend = "local".into();
    cfg.app.index_concurrency = 20;
    cfg.app.table_concurrency = 60;
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.adjust().unwrap();
    assert_eq!(20, cfg.app.index_concurrency);
    assert_eq!(60, cfg.app.table_concurrency);
    assert_eq!(32768, cfg.tikv_importer.send_kv_pairs);
    assert_eq!(
        ByteSize(KV_WRITE_BATCH_SIZE),
        cfg.tikv_importer.send_kv_size
    );
    cfg.tikv_importer.region_split_concurrency = 1;
    cfg.tikv_importer.region_check_backoff_limit = 0;
    cfg.adjust().unwrap();
    cfg.tikv_importer.region_split_batch_size = 0;
    assert_error_contains(
        &cfg.adjust().unwrap_err(),
        "`tikv-importer.region-split-batch-size` got 0, should be larger than 0",
    );
}

#[test]
/// 验证加载非法配置失败。
fn test_load_from_invalid_config() {
    let mut task_cfg = new_config();
    let err = task_cfg
        .load_from_global(&crate::GlobalConfig {
            config_file_content: b"invalid toml".to_vec(),
            ..crate::new_global_config()
        })
        .unwrap_err();
    assert!(Regex::new(r"line 1.*").unwrap().is_match(&err.to_string()));
}

#[test]
/// 验证 post-restore 级别 TOML 解析。
fn test_toml_post_restore() {
    let mut cfg = empty_config();
    assert!(
        cfg.load_from_toml(b"[post-restore]\nchecksum = \"req\"\n")
            .unwrap_err()
            .to_string()
            .contains("invalid op level 'req'")
    );
    assert!(
        cfg.load_from_toml(b"[post-restore]\nanalyze = 123\n")
            .unwrap_err()
            .to_string()
            .contains("invalid op level '123'")
    );
    let kv_map = [
        (r#""off""#, PostOpLevel::Off),
        (r#""required""#, PostOpLevel::Required),
        (r#""optional""#, PostOpLevel::Optional),
        ("true", PostOpLevel::Required),
        ("false", PostOpLevel::Off),
    ];
    for (k, v) in kv_map {
        let mut cfg = empty_config();
        cfg.load_from_toml(format!("[post-restore]\r\nchecksum= {k}\r\n").as_bytes())
            .unwrap();
        assert_eq!(v, cfg.post_restore.checksum);
        let encoded = toml_codec::encode_post_restore(&cfg.post_restore).unwrap();
        let pat = format!(r#"(?s).*checksum = "{}".*"#, regex::escape(v.as_str()));
        assert!(Regex::new(&pat).unwrap().is_match(&encoded), "{encoded}");
    }
}

#[test]
/// 验证 cron Duration 编解码。
fn test_cron_encode_decode() {
    let mut cfg = empty_config();
    cfg.cron.switch_mode = Duration(60_000_000_000);
    cfg.cron.log_progress = Duration(120_000_000_000);
    cfg.cron.check_disk_quota = Duration(3_000_000_000);
    let encoded = toml_codec::encode_cron(&cfg.cron).unwrap();
    assert_eq!(
        "switch-mode = \"1m0s\"\nlog-progress = \"2m0s\"\ncheck-disk-quota = \"3s\"\n",
        encoded
    );
    let mut cfg2 = empty_config();
    cfg2.load_from_toml(format!("[cron]\r\n{encoded}").as_bytes())
        .unwrap();
    assert_eq!(cfg.cron, cfg2.cron);
}

#[test]
/// 验证 disk-quota 调整。
fn test_adjust_disk_quota() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    let base = tempfile_dir();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.disk_quota = ByteSize(0);
    cfg.tikv_importer.sorted_kv_dir = base.to_string_lossy().into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.adjust().unwrap();
    assert_eq!(0, cfg.tikv_importer.disk_quota.0);
}

#[test]
/// 验证逻辑导入预处理语句开关。
fn test_adjust_logical_import_prep_stmt() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tikv_importer.backend = BACKEND_TIDB.into();
    cfg.adjust().unwrap();
    assert!(!cfg.tikv_importer.logical_import_prep_stmt);
    cfg.tikv_importer.logical_import_prep_stmt = true;
    cfg.adjust().unwrap();
    assert!(cfg.tikv_importer.logical_import_prep_stmt);
}

#[test]
/// 验证 conflict 与废弃字段合并/互斥规则。
fn test_adjust_conflict_strategy() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tikv_importer.backend = BACKEND_TIDB.into();
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::None;
    cfg.adjust().unwrap();
    assert_eq!(DuplicateResolutionAlgorithm::Error, cfg.conflict.strategy);

    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::None;
    cfg.adjust().unwrap();
    assert_eq!("", cfg.conflict.strategy.as_str());

    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Replace;
    cfg.adjust().unwrap();

    cfg.tikv_importer.parallel_import = true;
    cfg.conflict.precheck_conflict_before_import = true;
    cfg.adjust().unwrap();

    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Replace;
    cfg.tikv_importer.duplicate_resolution = DuplicateResolutionAlgorithm::Replace;
    cfg.tikv_importer.parallel_import = false;
    assert_error_contains(
        &cfg.adjust().unwrap_err(),
        "conflict.strategy cannot be used with tikv-importer.duplicate-resolution",
    );

    cfg.conflict.strategy = DuplicateResolutionAlgorithm::None;
    cfg.tikv_importer.on_duplicate = DuplicateResolutionAlgorithm::Replace;
    assert_error_contains(
        &cfg.adjust().unwrap_err(),
        "tikv-importer.on-duplicate cannot be used with tikv-importer.duplicate-resolution",
    );

    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Ignore;
    cfg.tikv_importer.duplicate_resolution = DuplicateResolutionAlgorithm::None;
    cfg.tikv_importer.on_duplicate = DuplicateResolutionAlgorithm::None;
    assert_error_contains(
        &cfg.adjust().unwrap_err(),
        "conflict.strategy cannot be set to \"ignore\" when use tikv-importer.backend = \"local\"",
    );

    cfg.tikv_importer.backend = BACKEND_TIDB.into();
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Ignore;
    cfg.conflict.precheck_conflict_before_import = true;
    assert_error_contains(
        &cfg.adjust().unwrap_err(),
        "conflict.precheck-conflict-before-import cannot be set to true when use tikv-importer.backend = \"tidb\"",
    );

    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::None;
    cfg.conflict.precheck_conflict_before_import = false;
    cfg.tikv_importer.duplicate_resolution = DuplicateResolutionAlgorithm::Replace;
    cfg.tikv_importer.on_duplicate = DuplicateResolutionAlgorithm::None;
    cfg.adjust().unwrap();
    assert_eq!(DuplicateResolutionAlgorithm::Replace, cfg.conflict.strategy);
}

#[test]
/// 验证 max_record_rows 推导。
fn test_adjust_max_record_rows() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.conflict.max_record_rows = -1;
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Replace;
    cfg.adjust().unwrap();
    assert_eq!(10000, cfg.conflict.max_record_rows);
    cfg.conflict.max_record_rows = -1;
    cfg.conflict.threshold = 9999;
    cfg.adjust().unwrap();
    assert_eq!(9999, cfg.conflict.max_record_rows);
    cfg.conflict.max_record_rows = 1000;
    cfg.conflict.threshold = 100;
    cfg.adjust().unwrap();
    assert_eq!(100, cfg.conflict.max_record_rows);
}

#[test]
/// 验证 checkpoint DSN 剥离 allowAllFiles。
fn test_remove_allow_all_files() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.checkpoint.driver = CHECKPOINT_DRIVER_MYSQL.into();
    cfg.checkpoint.dsn =
        "guest:12345@tcp(172.16.30.11:4001)/?tls=false&allowAllFiles=true&charset=utf8mb4".into();
    cfg.adjust().unwrap();
    assert_eq!(
        "guest:12345@tcp(172.16.30.11:4001)/?tls=false&charset=utf8mb4",
        cfg.checkpoint.dsn
    );
}

#[test]
/// 验证 data-character-set 解析与非法值。
fn test_data_character_set() {
    for input in [
        "[mydumper]\ndata-character-set = 'binary'\n",
        "[mydumper]\ndata-character-set = 'utf8mb4'\n",
        "[mydumper]\ndata-character-set = 'gb18030'\n",
        "[mydumper]\ndata-invalid-char-replace = \"a\"\n",
    ] {
        let mut cfg = new_config();
        cfg.mydumper.source_dir = "file://.".into();
        cfg.tidb.port = 4000;
        cfg.tidb.pd_addr = "test.invalid:2379".into();
        cfg.tikv_importer.backend = BACKEND_LOCAL.into();
        cfg.tikv_importer.sorted_kv_dir = ".".into();
        cfg.tidb.distsql_scan_concurrency = 1;
        cfg.load_from_toml(input.as_bytes()).unwrap();
        cfg.adjust().unwrap();
    }
}

#[test]
/// 验证 checkpoint keep 策略解析。
fn test_checkpoint_keep_strategy() {
    let cases = [
        (toml::Value::Boolean(true), CheckpointKeepStrategy::Rename),
        (toml::Value::Boolean(false), CheckpointKeepStrategy::Remove),
        (
            toml::Value::String("remove".into()),
            CheckpointKeepStrategy::Remove,
        ),
        (
            toml::Value::String("rename".into()),
            CheckpointKeepStrategy::Rename,
        ),
        (
            toml::Value::String("origin".into()),
            CheckpointKeepStrategy::Origin,
        ),
    ];
    for (key, strategy) in &cases {
        let mut cp = CheckpointKeepStrategy::Remove;
        cp.from_toml_value(key).unwrap();
        assert_eq!(*strategy, cp);
    }
    let mut cfg = empty_config();
    cfg.load_from_toml(b"[checkpoint]\nenable = true\r\n")
        .unwrap();
    assert_eq!(
        CheckpointKeepStrategy::Remove,
        cfg.checkpoint.keep_after_success
    );
    for (key, strategy) in &cases {
        let value = match key {
            toml::Value::String(s) => format!("\"{s}\""),
            toml::Value::Boolean(b) => b.to_string(),
            _ => continue,
        };
        let mut cfg = empty_config();
        cfg.load_from_toml(format!("[checkpoint]\nkeep-after-success = {value}\r\n").as_bytes())
            .unwrap();
        assert_eq!(*strategy, cfg.checkpoint.keep_after_success);
    }
    assert_eq!("remove", CheckpointKeepStrategy::Remove.marshal_text());
    assert_eq!("rename", CheckpointKeepStrategy::Rename.marshal_text());
    assert_eq!("origin", CheckpointKeepStrategy::Origin.marshal_text());
}

#[test]
/// 验证从配置加载字符集。
fn test_load_charset_from_config() {
    for (k, v) in [
        ("binary", Charset::Binary),
        ("BINARY", Charset::Binary),
        ("GBK", Charset::Gbk),
        ("gbk", Charset::Gbk),
        ("Gbk", Charset::Gbk),
        ("gB18030", Charset::Gb18030),
        ("GB18030", Charset::Gb18030),
    ] {
        assert_eq!(v, parse_charset(k).unwrap());
    }
    assert_eq!(
        "found unsupported data-character-set: Unknown",
        parse_charset("Unknown").unwrap_err().to_string()
    );
}

#[test]
/// 验证 tikv-importer 段 adjust。
fn test_adjust_tikv_importer() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir.clear();
    assert_eq!(
        "[Lightning:Config:ErrInvalidConfig]tikv-importer.sorted-kv-dir must not be empty!",
        cfg.tikv_importer.adjust().unwrap_err().to_string()
    );
    cfg.tikv_importer.sorted_kv_dir = "./not-exists".into();
    cfg.tikv_importer.adjust().unwrap();
    let base = tempfile_dir();
    let file = base.join("file");
    std::fs::write(&file, b"").unwrap();
    cfg.tikv_importer.sorted_kv_dir = file.to_string_lossy().into();
    assert!(
        Regex::new(r"tikv-importer.sorted-kv-dir (.*) is not a directory")
            .unwrap()
            .is_match(&cfg.tikv_importer.adjust().unwrap_err().to_string())
    );
    cfg.tikv_importer.sorted_kv_dir = base.to_string_lossy().into();
    cfg.tikv_importer.adjust().unwrap();
    cfg.tikv_importer.parallel_import = true;
    cfg.tikv_importer.add_index_by_sql = true;
    assert_error_contains(
        &cfg.tikv_importer.adjust().unwrap_err(),
        "tikv-importer.add-index-using-ddl cannot be used with tikv-importer.parallel-import",
    );
}

#[test]
/// 验证多配置不同 filter 互不影响。
fn test_create_several_configs_with_different_filters() {
    let original = get_default_filter();
    let mut cfg1 = new_config();
    cfg1.load_from_toml(b"[mydumper]\nfilter = [\"db1.tbl1\", \"db2.*\", \"!db2.tbl1\"]\n")
        .unwrap();
    assert_eq!(vec!["db1.tbl1", "db2.*", "!db2.tbl1"], cfg1.mydumper.filter);
    assert_eq!(get_default_filter(), original);
    assert_eq!(original, new_config().mydumper.filter);
    let g1 = load_global_config(
        &[
            "-f".into(),
            "db1.tbl1".into(),
            "-f".into(),
            "db2.*".into(),
            "-f".into(),
            "!db2.tbl1".into(),
        ],
        None,
    )
    .unwrap();
    assert_eq!(vec!["db1.tbl1", "db2.*", "!db2.tbl1"], g1.mydumper.filter);
    assert_eq!(
        original,
        load_global_config(&[], None).unwrap().mydumper.filter
    );
}

#[test]
/// 验证配置文件中的全局 filter/ignore-columns 按 BurntSushi TOML 语义解码。
fn test_load_global_config_mydumper_collections() {
    let dir = tempfile_dir();
    let path = dir.join("global.toml");
    std::fs::write(
        &path,
        br#"
[mydumper]
filter = [
    "db1.tbl1",
    "db2.*",
]

[[mydumper.ignore-columns]]
db = "db1"
table = "tbl1"
columns = ["secret", "token"]

[[mydumper.ignore-columns]]
table-filter = ["db2.*"]
columns = ["legacy"]
"#,
    )
    .unwrap();

    let cfg = load_global_config(
        &["--config".into(), path.to_string_lossy().into_owned()],
        None,
    )
    .unwrap();
    assert_eq!(vec!["db1.tbl1", "db2.*"], cfg.mydumper.filter);
    assert_eq!(2, cfg.mydumper.ignore_columns.len());
    assert_eq!("db1", cfg.mydumper.ignore_columns[0].db);
    assert_eq!("tbl1", cfg.mydumper.ignore_columns[0].table);
    assert_eq!(
        vec!["secret", "token"],
        cfg.mydumper.ignore_columns[0].columns
    );
    assert_eq!(vec!["db2.*"], cfg.mydumper.ignore_columns[1].table_filter);
    assert_eq!(vec!["legacy"], cfg.mydumper.ignore_columns[1].columns);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 验证压缩类型解析。
fn test_compression_type() {
    let mut ct = CompressionType::None;
    ct.from_string_value("").unwrap();
    assert_eq!(CompressionType::None, ct);
    ct.from_string_value("gzip").unwrap();
    assert_eq!(CompressionType::Gzip, ct);
    ct.from_string_value("gz").unwrap();
    assert_eq!(CompressionType::Gzip, ct);
    assert_eq!(
        "invalid compression-type 'zstd', please choose valid option between ['gzip']",
        ct.from_string_value("zstd").unwrap_err().to_string()
    );
    assert_eq!("", CompressionType::None.as_str());
    assert_eq!("gzip", CompressionType::Gzip.as_str());
}

#[test]
/// 验证 conflict.adjust 边界组合。
fn test_adjust_conflict() {
    let mut cfg = new_config();
    assign_minimal_legal_value(&mut cfg);
    let mut dra = DuplicateResolutionAlgorithm::None;
    dra.from_string_value("REPLACE").unwrap();
    cfg.conflict.strategy = dra;
    cfg.conflict.adjust(&cfg.tikv_importer).unwrap();
    assert_eq!(10000, cfg.conflict.threshold);
    dra.from_string_value("IGNORE").unwrap();
    cfg.conflict.strategy = dra;
    assert_error_contains(
        &cfg.conflict.adjust(&cfg.tikv_importer).unwrap_err(),
        "conflict.strategy cannot be set to \"ignore\" when use tikv-importer.backend = \"local\"",
    );
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Error;
    cfg.conflict.threshold = 1;
    assert_error_contains(
        &cfg.conflict.adjust(&cfg.tikv_importer).unwrap_err(),
        "conflict.threshold cannot be set when use conflict.strategy = \"error\"",
    );
    cfg.tikv_importer.backend = BACKEND_TIDB.into();
    cfg.conflict.strategy = DuplicateResolutionAlgorithm::Replace;
    cfg.conflict.max_record_rows = -1;
    cfg.conflict.threshold = -1;
    cfg.conflict.adjust(&cfg.tikv_importer).unwrap();
    assert_eq!(0, cfg.conflict.max_record_rows);
}

#[test]
/// 验证 block-size 默认填充。
fn test_adjust_block_size() {
    let mut cfg = new_config();
    cfg.tikv_importer.backend = BACKEND_LOCAL.into();
    cfg.tikv_importer.sorted_kv_dir = ".".into();
    cfg.tidb.distsql_scan_concurrency = 1;
    cfg.mydumper.source_dir = ".".into();
    cfg.tikv_importer.block_size = ByteSize(0);
    adjust_discover(&mut cfg, 6666, "34.34.34.34:3434").unwrap();
    assert_eq!(DEFAULT_BLOCK_SIZE, cfg.tikv_importer.block_size);
}

#[test]
/// 覆盖 `test_redact_config` 对应配置行为。
fn test_redact_config() {
    for (origin, redact) in [
        ("", ""),
        (":", ":"),
        ("~/file", "~/file"),
        ("gs://bucket/file", "gs://bucket/file"),
        (
            "gs://bucket/file?access-key=123",
            "gs://bucket/file?access-key=123",
        ),
        ("s3://bucket/file", "s3://bucket/file"),
        (
            "s3://bucket/file?other-key=123",
            "s3://bucket/file?other-key=123",
        ),
        (
            "s3://bucket/file?access-key=123",
            "s3://bucket/file?access-key=xxxxxx",
        ),
        (
            "s3://bucket/file?secret-access-key=123",
            "s3://bucket/file?secret-access-key=xxxxxx",
        ),
        (
            "s3://bucket/file?access_key=123",
            "s3://bucket/file?access_key=xxxxxx",
        ),
        (
            "s3://bucket/file?secret_access_key=123",
            "s3://bucket/file?secret_access_key=xxxxxx",
        ),
    ] {
        let mut cfg = new_config();
        cfg.mydumper.source_dir = origin.into();
        assert!(cfg.redact().contains(redact), "origin={origin}");
        assert!(cfg.string().contains(origin), "origin={origin}");
    }
}
