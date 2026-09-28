// Copyright 2026 AsterSQL.

// 巡检结果（inspection result）过滤与容量解析的单元测试。
//
// 覆盖规则名过滤、时间范围 SQL 条件生成，以及可读容量到字节的换算。

use std::collections::BTreeSet;

use crate::inspection_result::{
    configInspection, inspectionFilter, inspectionName, queryTimeRange,
};

#[test]
/// 过滤器 enable/时间条件与可读容量解析语义。
fn inspection_filter_and_readable_capacity_follow_rule_semantics() {
    let filter = inspectionFilter {
        set: BTreeSet::from(["config".to_owned()]),
        timeRange: queryTimeRange {
            from: "10".to_owned(),
            to: "20".to_owned(),
        },
    };
    assert!(filter.enable("config"));
    assert!(!filter.enable("threshold-check"));
    assert_eq!(
        filter.timeRange.condition(),
        "where time >= '10' and time <= '20'"
    );

    let inspection = configInspection {
        inspectionName: inspectionName("config".to_owned()),
    };
    assert_eq!(
        inspection.convertReadableSizeToByteSize("1536MiB").unwrap(),
        1_610_612_736
    );
    assert_eq!(inspection.convertReadableSizeToByteSize("25").unwrap(), 25);
    assert!(
        inspection
            .convertReadableSizeToByteSize("not-a-size")
            .is_err()
    );
}

#[test]
fn readable_capacity_matches_go_signed_parse_and_wrapping_multiply() {
    let inspection = configInspection {
        inspectionName: inspectionName("config".to_owned()),
    };

    assert_eq!(
        inspection.convertReadableSizeToByteSize("-1KiB").unwrap(),
        u64::MAX - 1023
    );
    assert_eq!(
        inspection
            .convertReadableSizeToByteSize("9223372036854775807PiB")
            .unwrap(),
        (i64::MAX as u64).wrapping_mul(1_u64 << 50)
    );
}
