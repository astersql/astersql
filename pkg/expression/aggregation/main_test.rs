// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.

// 覆盖本 crate `types` 门面的 Go 整数混合加法契约。

#[test]
fn compute_plus_preserves_go_mixed_integer_contract() {
    let int_uint = super::types::ComputePlus(
        super::types::NewIntDatum(-28),
        super::types::NewUintDatum(72),
    )
    .unwrap();
    assert_eq!(int_uint.Kind(), super::types::KindUint64);
    assert_eq!(int_uint.GetUint64(), 44);

    let uint_int = super::types::ComputePlus(
        super::types::NewUintDatum(72),
        super::types::NewIntDatum(28),
    )
    .unwrap();
    assert_eq!(uint_int.Kind(), super::types::KindUint64);
    assert_eq!(uint_int.GetUint64(), 100);

    assert!(
        super::types::ComputePlus(super::types::NewUintDatum(0), super::types::NewIntDatum(-1))
            .is_err()
    );
}

#[test]
fn compute_plus_preserves_go_decimal_fraction_metadata() {
    let mut left = super::types::NewDecimalDatum(super::types::NewDecFromStringForTest("1.20"));
    left.SetFrac(2);
    let mut right = super::types::NewDecimalDatum(super::types::NewDecFromStringForTest("3.456"));
    right.SetFrac(3);

    let sum = super::types::ComputePlus(left, right).unwrap();
    assert_eq!(sum.Kind(), super::types::KindMysqlDecimal);
    assert_eq!(sum.Frac(), 3);
}
