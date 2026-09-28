// Copyright 2026 AsterSQL.
// Duration（时长）加减溢出测试；底层按 int64 纳秒与 BIGINT 相同边界检查。

use crate::file_group::overflow::{AddDuration, Duration, OverflowError, SubDuration};

fn assert_duration_result(
    result: Result<Duration, OverflowError>,
    expected: Duration,
    overflow: bool,
    lhs: Duration,
    rhs: Duration,
) {
    if overflow {
        let err = result.expect_err("expected duration overflow");
        assert_eq!(err.target_type, "BIGINT");
        assert_eq!(err.expression, format!("({}, {})", lhs, rhs));
    } else {
        assert_eq!(result.unwrap(), expected);
    }
}

#[test]
/// 完整复现 Go `TestAdd` 对 `AddDuration` 复用的 int64 用例表。
fn add_duration_matches_go_int64_table() {
    let cases = [
        (i64::MAX, 1, 0, true),
        (i64::MAX, 0, i64::MAX, false),
        (0, i64::MIN, i64::MIN, false),
        (-1, i64::MIN, 0, true),
        (i64::MAX, i64::MIN, -1, false),
        (1, 1, 2, false),
        (1, -1, 0, false),
    ];

    for (lhs, rhs, expected, overflow) in cases {
        assert_duration_result(AddDuration(lhs, rhs), expected, overflow, lhs, rhs);
    }
}

#[test]
/// 覆盖 Go `SubDuration` 的正常路径、两个异号溢出分支与 `0-MIN` 特例。
fn sub_duration_matches_go_boundaries() {
    let cases = [
        (i64::MIN, 0, i64::MIN, false),
        (i64::MIN, 1, 0, true),
        (i64::MAX, -1, 0, true),
        (0, i64::MIN, 0, true),
        (-1, i64::MIN, i64::MAX, false),
        (i64::MIN, i64::MAX, 0, true),
        (i64::MIN, i64::MIN, 0, false),
        (i64::MIN, -i64::MAX, -1, false),
        (1, 1, 0, false),
    ];

    for (lhs, rhs, expected, overflow) in cases {
        assert_duration_result(SubDuration(lhs, rhs), expected, overflow, lhs, rhs);
    }
}
