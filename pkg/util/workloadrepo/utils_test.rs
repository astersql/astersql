// Copyright 2026 AsterSQL.
use crate::*;
use chrono::{Local, TimeZone};
use std::sync::Arc;

struct UnusedBackend;
impl RepositoryBackend for UnusedBackend {
    fn execute(&self, _: &str, _: &[Value]) -> Result<Vec<Row>, String> {
        unreachable!()
    }
    fn source_columns(&self, _: &str, _: &str) -> Result<Vec<ColumnDefinition>, String> {
        unreachable!()
    }
    fn table_exists(&self, _: &str) -> bool {
        unreachable!()
    }
    fn partitions(&self, _: &str) -> Result<Vec<String>, String> {
        unreachable!()
    }
    fn instance_id(&self) -> Result<String, String> {
        unreachable!()
    }
    fn is_owner(&self) -> bool {
        unreachable!()
    }
    fn etcd_available(&self) -> bool {
        unreachable!()
    }
    fn kv_create(&self, _: &str, _: &str) -> Result<bool, String> {
        unreachable!()
    }
    fn kv_get(&self, _: &str) -> Result<Option<String>, String> {
        unreachable!()
    }
    fn kv_cas(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
        unreachable!()
    }
}

#[test]
fn retention_hook_matches_atoi_and_int32_cast() {
    let w = initializeWorker(Arc::new(UnusedBackend), vec![]);
    for (input, expected) in [
        ("-1", -1),
        ("366", 366),
        ("+30", 30),
        ("2147483648", i32::MIN),
        ("9223372036854775807", -1),
    ] {
        w.setRetentionDays(input).unwrap();
        assert_eq!(w.state.lock().unwrap().retentionDays, expected);
    }
    for input in ["", " 1", "1 ", "1.0", "9223372036854775808"] {
        assert!(w.setRetentionDays(input).is_err());
        assert_eq!(w.state.lock().unwrap().retentionDays, -1);
    }
}

#[test]
fn dest_error_uses_lowercase_and_go_message() {
    assert_eq!(validateDest("TaBLe").unwrap(), "table");
    assert_eq!(validateDest("").unwrap(), "");
    assert_eq!(
        validateDest("INVALID").unwrap_err(),
        format!(
            "Variable '{}' can't be set to the value of 'invalid': valid values are '' and 'table'",
            repositoryDest
        )
    );
}

#[test]
fn partition_parser_requires_exact_go_layout() {
    for name in [
        "p2026011",
        "p202611",
        "p+20260101",
        "p20260101 ",
        "p20260229",
        "P20260101",
    ] {
        assert!(parsePartitionName(name).is_err(), "{name}");
    }
    assert_eq!(
        generatePartitionName(parsePartitionName("p20240229").unwrap()),
        "p20240229"
    );
}

#[test]
fn partition_ranges_preserve_append_skip_and_error_contracts() {
    let now = Local.with_ymd_and_hms(2024, 2, 28, 12, 0, 0).unwrap();
    let first = "PARTITION p20240229 VALUES LESS THAN (TO_DAYS('2024-02-29'))";
    let second = "PARTITION p20240301 VALUES LESS THAN (TO_DAYS('2024-03-01'))";
    for (existing, suffix, skip) in [
        (vec![], format!("{first}, {second}"), false),
        (vec!["p20240229".into()], second.into(), false),
        (vec!["p20240301".into()], "".into(), true),
        (
            vec!["bad".into(), "p20240227".into()],
            format!("{first}, {second}"),
            false,
        ),
    ] {
        let mut sql = "prefix".to_string();
        assert_eq!(
            generatePartitionRanges(&mut sql, &existing, now).unwrap(),
            skip
        );
        assert_eq!(sql, format!("prefix{suffix}"));
    }
    let mut sql = "prefix".to_string();
    assert!(generatePartitionRanges(&mut sql, &["bad".into()], now).is_err());
    assert_eq!(sql, "prefix");
    sql.clear();
    generatePartitionDef(&mut sql, "TS", now).unwrap();
    assert_eq!(
        sql,
        format!(" PARTITION BY RANGE( TO_DAYS(TS) ) ({first}, {second})")
    );
}

#[test]
fn date_errors_match_go() {
    for (name, error) in [
        (
            "p2026011",
            "parsing time \"p2026011\" as \"p20060102\": cannot parse \"1\" as \"02\"",
        ),
        (
            "p20261301",
            "parsing time \"p20261301\": month out of range",
        ),
        ("p20260229", "parsing time \"p20260229\": day out of range"),
        (
            "p20260101 ",
            "parsing time \"p20260101 \": extra text: \" \"",
        ),
    ] {
        assert_eq!(parsePartitionName(name).unwrap_err(), error);
    }
}

#[test]
fn midnight_transition_matches_go_date() {
    // Run this test in a separate process with TZ=America/Sao_Paulo,
    // avoiding unsafe process-global environment mutation in parallel tests.
    if std::env::var("TZ").as_deref() != Ok("America/Sao_Paulo") {
        return;
    }
    assert_eq!(
        parsePartitionName("p20181104").unwrap().to_rfc3339(),
        "2018-11-03T23:00:00-03:00"
    );
    assert_eq!(
        parsePartitionName("p20190217").unwrap().to_rfc3339(),
        "2019-02-17T00:00:00-03:00"
    );
    let now = Local.with_ymd_and_hms(2018, 11, 3, 12, 0, 0).unwrap();
    let mut sql = String::new();
    generatePartitionRanges(&mut sql, &[], now).unwrap();
    assert_eq!(
        sql,
        "PARTITION p20181103 VALUES LESS THAN (TO_DAYS('2018-11-03')), PARTITION p20181105 VALUES LESS THAN (TO_DAYS('2018-11-05'))"
    );
}
