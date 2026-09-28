// Copyright 2026 AsterSQL.

// Information Schema 列数值精度与 MySQL 整数显示宽度对齐的单元测试。
//
// 核对 Tiny/Short/Int24/Long/LongLong（含无符号）及 Decimal/Other
// 在 `getNumericPrecision` 下的期望值，并抽样验证字符八位组长度。

use std::time::Duration;

use crate::infoschema_reader::{
    FieldKind, FieldType, calRemainInfoForAnalyzeStatus, calcCharOctLength, getNumericPrecision,
};

/// 核对各整数类型显示宽度及 Decimal 透传、Other 归零行为。
#[test]
fn information_schema_column_precision_matches_mysql_integer_widths() {
    let precision = |kind, unsigned| getNumericPrecision(&FieldType { kind, unsigned }, 37);
    assert_eq!(precision(FieldKind::Tiny, false), 3);
    assert_eq!(precision(FieldKind::Short, false), 5);
    assert_eq!(precision(FieldKind::Int24, false), 7);
    assert_eq!(precision(FieldKind::Int24, true), 8);
    assert_eq!(precision(FieldKind::Long, false), 10);
    assert_eq!(precision(FieldKind::LongLong, false), 19);
    assert_eq!(precision(FieldKind::LongLong, true), 20);
    assert_eq!(precision(FieldKind::Decimal, false), 37);
    assert_eq!(precision(FieldKind::Other, false), 0);
    assert_eq!(calcCharOctLength(12, "utf8mb4"), 48);
}

/// Go 只在 elapsed 恰为零时回退到一秒；非零亚秒耗时必须参与原始估算。
#[test]
fn analyze_remaining_duration_preserves_nonzero_subsecond_elapsed_time() {
    let (remaining, percentage) =
        calRemainInfoForAnalyzeStatus(100, 10, Duration::from_millis(500));

    assert_eq!(remaining, Duration::from_secs(4));
    assert_eq!(percentage, 0.1);
}
