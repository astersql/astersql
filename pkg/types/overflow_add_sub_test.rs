// Copyright 2026 AsterSQL.
// 覆盖整数加减溢出边界：对齐 MySQL BIGINT / BIGINT UNSIGNED 语义。
//
// 溢出（overflow）指运算结果超出目标类型可表示范围。

#[test]
/// 断言无符号/有符号加减在边界值处成功或返回 OverflowError。
fn types_overflow_add_sub_detect_boundaries() {
    use crate::file_group::overflow::{AddInt64, AddUint64, SubInt64, SubUint64};

    // 正常无符号加法与 MAX+1 溢出
    assert_eq!(AddUint64(1, 2).unwrap(), 3);
    let err = AddUint64(u64::MAX, 1).unwrap_err();
    assert_eq!(err.target_type, "BIGINT UNSIGNED");
    assert_eq!(err.expression, format!("({}, 1)", u64::MAX));

    // 有符号加法与 MAX+1 溢出
    assert_eq!(AddInt64(-2, 1).unwrap(), -1);
    assert!(AddInt64(i64::MAX, 1).is_err());

    // 无符号减法与下溢（0-1）
    assert_eq!(SubUint64(2, 1).unwrap(), 1);
    assert!(SubUint64(0, 1).is_err());

    // 0 - MinInt64 无法表示为正的 BIGINT（取负会溢出）
    assert_eq!(SubInt64(2, 1).unwrap(), 1);
    let err = SubInt64(0, i64::MIN).unwrap_err();
    assert_eq!(err.target_type, "BIGINT");
    assert_eq!(err.expression, format!("(0, {})", i64::MIN));
}
