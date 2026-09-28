// Copyright 2026 AsterSQL.

use super::parser::ConvertedType;
use super::type_converter::{ConvertedInfo, Datum, convert_int96, new_int96};

#[test]
fn int96_without_rebase_rounds_sub_microsecond_precision() {
    let mut value = new_int96(86_399_999_999);
    let nanos = u64::from_le_bytes(value[..8].try_into().unwrap()) + 500;
    value[..8].copy_from_slice(&nanos.to_le_bytes());

    let info = ConvertedInfo {
        converted: ConvertedType::None,
        scale: 0,
        adjusted_to_utc: false,
        timezone_offset_seconds: 0,
        spark_rebase: None,
    };

    assert_eq!(
        convert_int96(value, &info).unwrap(),
        Datum::TimeMicros(86_400_000_000)
    );
}
