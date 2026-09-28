// Copyright 2026 AsterSQL.

use crate::{DecodeComparableUvarint, DecodeComparableVarint, errors};

#[test]
fn comparable_errors_preserve_go_sentinel_identity() {
    let insufficient = DecodeComparableUvarint(&[]).unwrap_err();
    for error in [
        DecodeComparableUvarint(&[]).unwrap_err(),
        DecodeComparableUvarint(&[255]).unwrap_err(),
        DecodeComparableVarint(&[]).unwrap_err(),
        DecodeComparableVarint(&[0]).unwrap_err(),
    ] {
        assert!(errors::Cause(Some(&error)).unwrap().ptr_eq(&insufficient));
    }
    let invalid = DecodeComparableUvarint(&[0]).unwrap_err();
    let invalid = errors::Cause(Some(&invalid)).unwrap();
    for error in [
        DecodeComparableUvarint(&[7]).unwrap_err(),
        DecodeComparableVarint(&[255, 255, 255, 255, 255, 255, 255, 255, 255]).unwrap_err(),
        DecodeComparableVarint(&[0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap_err(),
    ] {
        assert!(errors::Cause(Some(&error)).unwrap().ptr_eq(&invalid));
    }
    assert!(!invalid.ptr_eq(&insufficient));
}

#[test]
fn integer_order_covers_go_extrema_pairs() {
    use crate::*;
    let signed = [
        i64::MIN,
        i32::MIN as i64,
        i16::MIN as i64,
        i8::MIN as i64,
        -1,
        0,
        1,
        i8::MAX as i64,
        i16::MAX as i64,
        i32::MAX as i64,
        i64::MAX,
    ];
    for a in signed {
        for b in signed {
            assert_eq!(EncodeInt(vec![], a).cmp(&EncodeInt(vec![], b)), a.cmp(&b));
            assert_eq!(
                EncodeIntDesc(vec![], a).cmp(&EncodeIntDesc(vec![], b)),
                b.cmp(&a)
            );
            assert_eq!(
                EncodeComparableVarint(vec![], a).cmp(&EncodeComparableVarint(vec![], b)),
                a.cmp(&b)
            );
        }
    }
    let unsigned = [
        0,
        1,
        i8::MAX as u64,
        u8::MAX as u64,
        i16::MAX as u64,
        u16::MAX as u64,
        i32::MAX as u64,
        u32::MAX as u64,
        i64::MAX as u64,
        u64::MAX,
    ];
    for a in unsigned {
        for b in unsigned {
            assert_eq!(EncodeUint(vec![], a).cmp(&EncodeUint(vec![], b)), a.cmp(&b));
            assert_eq!(
                EncodeUintDesc(vec![], a).cmp(&EncodeUintDesc(vec![], b)),
                b.cmp(&a)
            );
            assert_eq!(
                EncodeComparableUvarint(vec![], a).cmp(&EncodeComparableUvarint(vec![], b)),
                a.cmp(&b)
            );
        }
    }
}
