// Copyright 2026 AsterSQL.

use crate::*;

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
