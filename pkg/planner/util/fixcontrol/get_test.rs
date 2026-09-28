// Copyright 2026 AsterSQL.

use super::*;
use std::collections::HashMap;

#[test]
fn TestGetIntPreservesGoOverflowValueAndError() {
    let values = HashMap::from([
        (1, "9223372036854775808".to_owned()),
        (2, "-9223372036854775809".to_owned()),
    ]);

    let (positive, exists, error) = GetInt(&values, 1);
    assert_eq!(i64::MAX, positive);
    assert!(exists);
    assert!(error.is_err());

    let (negative, exists, error) = GetInt(&values, 2);
    assert_eq!(i64::MIN, negative);
    assert!(exists);
    assert!(error.is_err());
}

#[test]
fn TestGetFloatPreservesGoRangeError() {
    let values = HashMap::from([(1, "1e400".to_owned()), (2, "-1e400".to_owned())]);

    let (positive, exists, error) = GetFloat(&values, 1);
    assert_eq!(f64::INFINITY, positive);
    assert!(exists);
    assert!(error.is_err());

    let (negative, exists, error) = GetFloat(&values, 2);
    assert_eq!(f64::NEG_INFINITY, negative);
    assert!(exists);
    assert!(error.is_err());
}
