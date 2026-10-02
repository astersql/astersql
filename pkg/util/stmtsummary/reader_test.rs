// Copyright 2026 AsterSQL.

use crate::*;

#[test]
fn go_commit_59ca78807e_reader_double_metrics() {
    let reader = NewStmtSummaryReader(None, true, Vec::new(), String::new(), chrono_tz::UTC);
    let mut stats = stmtSummaryStats::default();
    stats.execCount = 2;
    stats.commitCount = 2;
    let large = 1_u64 << 63;
    stats.sumRocksdbDeleteSkippedCount = large;
    stats.sumRocksdbKeySkippedCount = large;
    stats.sumRocksdbBlockCacheHitCount = large;
    stats.sumRocksdbBlockReadCount = large;
    stats.sumRocksdbBlockReadByte = large;
    stats.sumAffectedRows = large;
    stats.sumWriteKeys = 246;
    stats.sumWriteSize = 468;
    stats.sumPrewriteRegionNum = 6;
    stats.sumTxnRetry = 4;
    let factories = columnValueFactoryMap();
    for (name, expected) in [
        (RocksdbDeleteSkippedCountStr, large as f64),
        (AvgRocksdbDeleteSkippedCountStr, large as f64 / 2.0),
        (RocksdbKeySkippedCountStr, large as f64),
        (AvgRocksdbKeySkippedCountStr, large as f64 / 2.0),
        (RocksdbBlockCacheHitCountStr, large as f64),
        (AvgRocksdbBlockCacheHitCountStr, large as f64 / 2.0),
        (RocksdbBlockReadCountStr, large as f64),
        (AvgRocksdbBlockReadCountStr, large as f64 / 2.0),
        (RocksdbBlockReadByteStr, large as f64),
        (AvgRocksdbBlockReadByteStr, large as f64 / 2.0),
        (WriteKeysStr, 246.0),
        (AvgWriteKeysStr, 123.0),
        (WriteSizeStr, 468.0),
        (AvgWriteSizeStr, 234.0),
        (PrewriteRegionsStr, 6.0),
        (AvgPrewriteRegionsStr, 3.0),
        (TxnRetryStr, 4.0),
        (AvgTxnRetryStr, 2.0),
        (AffectedRowsStr, large as f64),
        (AvgAffectedRowsStr, large as f64 / 2.0),
    ] {
        assert_eq!(
            factories[name](&reader, None, None, &stats)
                .into_datum()
                .GetFloat64(),
            expected,
            "{name}"
        );
    }
    stats.execCount = 0;
    assert_eq!(
        factories[AvgRocksdbDeleteSkippedCountStr](&reader, None, None, &stats)
            .into_datum()
            .GetFloat64(),
        0.0
    );
}

#[test]
fn go_merge_36_reader_columns_use_unsigned_float_and_execution_count() {
    let reader = NewStmtSummaryReader(None, true, Vec::new(), String::new(), chrono_tz::UTC);
    let mut stats = stmtSummaryStats::default();
    stats.execCount = 2;
    stats.commitCount = 1;
    stats.sumRocksdbDeleteSkippedCount = i64::MAX as u64 + 3;
    stats.sumAffectedRows = i64::MAX as u64 + 3;
    stats.sumIARemoteReadSegmentCount = 3;
    stats.iaExecCount = 1;
    stats.sumKVTotal = std::time::Duration::from_nanos(10);
    let factories = columnValueFactoryMap();
    let value = |name| factories[name](&reader, None, None, &stats).into_datum();
    assert_eq!(
        value(RocksdbDeleteSkippedCountStr).GetFloat64(),
        stats.sumRocksdbDeleteSkippedCount as f64
    );
    assert_eq!(
        value(AvgRocksdbDeleteSkippedCountStr).GetFloat64(),
        stats.sumRocksdbDeleteSkippedCount as f64 / 2.0
    );
    assert_eq!(
        value(AvgAffectedRowsStr).GetFloat64(),
        stats.sumAffectedRows as f64 / 2.0
    );
    assert_eq!(value(IAExecCountStr).GetInt64(), 1);
    assert_eq!(value(AvgIARemoteReadSegmentCountStr).GetFloat64(), 1.5);
    assert_eq!(value(AvgKvTimeStr).GetInt64(), 5);
}

#[test]
fn plan_column_keeps_go_string_bytes() {
    let reader = NewStmtSummaryReader(None, true, Vec::new(), String::new(), chrono_tz::UTC);
    let mut stats = stmtSummaryStats::default();
    stats.samplePlan = plancodec_dependency::Compress(b"0\t1\t0\t\xff");
    let datum = columnValueFactoryMap()[PlanStr](&reader, None, None, &stats).into_datum();
    assert!(datum.GetBytes().ends_with(b"\xff"));
    let text = types::NewStringDatum(String::new());
    assert_eq!(datum.Kind(), text.Kind());
    assert_eq!(datum.Collation(), text.Collation());
}
