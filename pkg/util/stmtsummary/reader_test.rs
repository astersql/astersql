// Copyright 2026 AsterSQL.

use crate::*;

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
