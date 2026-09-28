// Copyright 2026 AsterSQL.

use crate::gc::compute_new_safe_point;
use crate::stubs::{oracle, parse_go_duration};

/// Go 在对 `time.Duration` 最小值取负时按 int64 二补码回绕，结果仍为最小值。
#[test]
fn minimum_gc_offset_matches_go_wrapping_negation() {
    let physical = 1_700_000_000_000i64;
    let offset = parse_go_duration("-2562047h47m16.854775808s")
        .expect("Go accepts time.Duration's minimum value");

    let (_, safe_point) = compute_new_safe_point(physical, 0, offset);
    let go_added_nanos = (i64::MIN as i128) + (physical as i128 * 1_000_000);
    let go_physical = (go_added_nanos / 1_000_000) as i64;

    assert_eq!(safe_point, oracle::ComposeTS(go_physical, 0));
}

/// Go 从组合后的 TSO 提取物理位，因此 logical 进位也参与 safepoint 计算。
#[test]
fn logical_carry_is_extracted_from_composed_ts() {
    let offset = parse_go_duration("1ms").expect("valid Go duration");
    let (_, safe_point) = compute_new_safe_point(10_000, 1 << 18, offset);

    assert_eq!(safe_point, oracle::ComposeTS(10_000, 0));
}
