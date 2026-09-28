// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// `context` 模块单元测试：表达式上下文与表变更（mutate）上下文的系统变量解析。

use std::collections::HashMap;
use std::sync::Mutex;

use encode::Datum;

use crate::*;

static ROW_CHECKSUM_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 验证 `newLitExprContext` 解析系统变量，以及用户变量的大小写不敏感设置/删除。
#[test]
fn TestLitExprContext() {
    let vars = HashMap::from([
        ("max_allowed_packet".into(), "40960".into()),
        ("div_precision_increment".into(), "9".into()),
        ("time_zone".into(), "SYSTEM".into()),
    ]);
    let mut context = newLitExprContext(1, &vars, 123456).unwrap();
    assert_eq!(context.MaxAllowedPacket, 40960);
    assert_eq!(context.DivPrecisionIncrement, 9);
    assert_eq!(context.CurrentTimestamp, 123456);
    context.setUserVarVal("Example", Datum::Int(7));
    assert_eq!(context.UserVars.get("example"), Some(&Datum::Int(7)));
    context.unsetUserVar("EXAMPLE");
    assert!(!context.UserVars.contains_key("example"));
}

#[test]
fn TestLitExprContextSQLModeAndValidationParity() {
    const STRICT_TRANS_TABLES: u64 = 0x0020_0000;
    const NO_ZERO_IN_DATE: u64 = 0x0080_0000;
    const NO_ZERO_DATE: u64 = 0x0100_0000;
    const ERROR_FOR_DIVISION_BY_ZERO: u64 = 0x0400_0000;
    const ALLOW_INVALID_DATES: u64 = 0x1_0000_0000;

    let non_strict = newLitExprContext(0, &HashMap::new(), 1).unwrap();
    assert!(non_strict.TypeFlags.TruncateAsWarning);
    assert!(non_strict.TypeFlags.IgnoreZeroInDateErr);
    assert_eq!(non_strict.ErrLevels.Truncate, ErrorLevel::Warn);
    assert_eq!(non_strict.ErrLevels.DividedByZero, ErrorLevel::Ignore);

    let strict = newLitExprContext(
        STRICT_TRANS_TABLES | NO_ZERO_IN_DATE | NO_ZERO_DATE | ERROR_FOR_DIVISION_BY_ZERO,
        &HashMap::new(),
        1,
    )
    .unwrap();
    assert!(!strict.TypeFlags.TruncateAsWarning);
    assert!(!strict.TypeFlags.IgnoreZeroInDateErr);
    assert_eq!(strict.ErrLevels.Truncate, ErrorLevel::Error);
    assert_eq!(strict.ErrLevels.BadNull, ErrorLevel::Error);
    assert_eq!(strict.ErrLevels.NoDefault, ErrorLevel::Error);
    assert_eq!(strict.ErrLevels.DividedByZero, ErrorLevel::Error);

    let invalid_dates = newLitExprContext(
        STRICT_TRANS_TABLES | ALLOW_INVALID_DATES,
        &HashMap::new(),
        1,
    )
    .unwrap();
    assert!(invalid_dates.TypeFlags.IgnoreZeroInDateErr);
    assert!(invalid_dates.TypeFlags.IgnoreInvalidDateErr);

    for (name, value) in [
        ("max_allowed_packet", "1023"),
        ("div_precision_increment", "31"),
        ("default_week_format", "8"),
        ("group_concat_max_len", "3"),
        ("block_encryption_mode", "not-a-mode"),
    ] {
        let vars = HashMap::from([(name.to_string(), value.to_string())]);
        assert!(newLitExprContext(0, &vars, 1).is_err(), "{name}={value}");
    }
}

/// 验证 `newLitTableMutateContext` 从系统变量推导行编码、行级校验和、mutation checker 等开关。
#[test]
fn TestLitTableMutateContext() {
    let _guard = ROW_CHECKSUM_TEST_LOCK.lock().unwrap();
    let vars = HashMap::from([
        ("tidb_row_format_version".into(), "2".into()),
        ("tidb_enable_mutation_checker".into(), "1".into()),
        ("TIDB_SHARD_ALLOCATE_STEP".into(), "1234567".into()),
    ]);
    let expression = newLitExprContext(0, &vars, 1).unwrap();
    let table = newLitTableMutateContext(&expression, &vars).unwrap();
    assert_eq!(table.GetRowEncodingConfig(), (true, false));
    assert!(table.EnableMutationChecker());
    assert_eq!(table.ConnectionID(), 0);
    assert!(!table.InRestrictedSQL());
    assert!(table.GetStatisticsSupport());
    assert_eq!(table.AlternativeAllocators().1, false);
    assert!(table.GetMutateBuffers().WriteStmtBuffer.is_empty());
    assert_eq!(table.GetRowIDShardGenerator().GetShardStep(), 1_234_567);
    assert_ne!(
        table.GetRowIDShardGenerator().Next(),
        table.GetRowIDShardGenerator().Next()
    );
}

#[test]
fn TestLitTableMutateContextDefaultsAndValidationParity() {
    let _guard = ROW_CHECKSUM_TEST_LOCK.lock().unwrap();
    let expression = newLitExprContext(0, &HashMap::new(), 1).unwrap();
    let defaults = newLitTableMutateContext(&expression, &HashMap::new()).unwrap();
    assert_eq!(defaults.TxnAssertionLevel(), "OFF");
    assert!(!defaults.EnableMutationChecker());
    assert_eq!(defaults.GetRowEncodingConfig(), (false, false));
    assert_eq!(defaults.GetRowIDShardGenerator().GetShardStep(), i64::MAX);

    let vars = HashMap::from([
        ("tidb_txn_assertion_level".into(), "STRICT".into()),
        ("tidb_enable_mutation_checker".into(), "On".into()),
        ("tidb_row_format_version".into(), "2".into()),
    ]);
    let loaded = newLitTableMutateContext(&expression, &vars).unwrap();
    assert_eq!(loaded.TxnAssertionLevel(), "STRICT");
    assert!(loaded.EnableMutationChecker());
    assert_eq!(loaded.GetRowEncodingConfig(), (true, false));

    SetGlobalRowLevelChecksumEnabled(true);
    let checksum = newLitTableMutateContext(&expression, &vars).unwrap();
    assert_eq!(checksum.GetRowEncodingConfig(), (true, true));
    SetGlobalRowLevelChecksumEnabled(false);

    for (name, value) in [
        ("tidb_txn_assertion_level", "INVALID"),
        ("tidb_enable_mutation_checker", "maybe"),
        ("tidb_row_format_version", "3"),
        ("tidb_shard_allocate_step", "0"),
    ] {
        let vars = HashMap::from([(name.to_string(), value.to_string())]);
        assert!(
            newLitTableMutateContext(&expression, &vars).is_err(),
            "{name}={value}"
        );
    }
}
